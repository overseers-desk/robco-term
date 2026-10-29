//! Everything drawn after the chain: the casting, the furniture that stands
//! on it, and the badges over the glass.
//!
//! One pipeline, one instance buffer, one render pass with `LoadOp::Load`,
//! compositing straight onto the swapchain view. The picture the chain
//! finished is already on that image and this goes over it; nothing here
//! begins a pass with `Clear`, so there is no offscreen texture and no copy
//! back.
//!
//! Two draws, because there are two scissors and not because there are two
//! kinds of thing. The cabinet's own instances are cut at the bank column,
//! since a piece is allowed to hang off it. The badges are centred in the
//! well the column leaves and are cut at the target, since a badge is over
//! the glass. Everything else about them is shared: one pipeline, one buffer,
//! one submission.
//!
//! # What each instance is
//!
//! A rectangle in **physical** pixels of the swapchain, a `kind` saying which
//! shader body draws it, and an index into that kind's parameter buffer. The
//! vertex stage places the rectangle and hands the fragment stage a `uv`
//! running 0..1 across it, so a body is written against its own piece rather
//! than against the window.
//!
//! Per-piece parameters ride in a storage buffer indexed by that index, which
//! is what makes one pass enough: nothing is written per draw, so nothing can
//! be overwritten by a later write in the same submission. A chain's uniform
//! buffer could not do that, and the mount that used one had to compile a
//! chain per distinct parameter set to work around it.
//!
//! One pass is also what keeps the bank's own order. The plan alternates
//! shaded pieces and drawn ones -- a row's moulding is struck over the
//! plate and under nothing, a strip's lamps sit inside the moulding -- so a
//! mount that drew all the shaded pieces and then all the drawn ones would
//! put a row's chrome over its own lit window. Every piece is an instance
//! here, in the plan's own order, and the draw is painter's order.
//!
//! # The vector half
//!
//! A piece of drawn furniture carries a [`chassis::paint::Painting`], a list
//! of operations rather than an image, and **each operation is its own
//! instance**: a rounded rectangle, a gradient, an arc, a filled path or a
//! line of text, in the order the painting lists them. The fixed-function
//! blender composites them source-over in that order, which is what one
//! accumulator used to do on the CPU, so a stack of half-transparent lips
//! still adds up the same way.
//!
//! Gradient stops and polygon points live in storage buffers of their own,
//! indexed per instance by an offset and a count, so a five-stop moulding
//! costs the instance nothing and quantises nothing.
//!
//! Text is the one thing still struck on the CPU, because a glyph outline is
//! not a shape the vector vocabulary can name. Each run goes through swash
//! once ([`chassis::paint::text_raster`]) and is packed into the same atlas
//! the display kits' rasters use. The cache is keyed on the run and the box
//! it is aligned in, both of which are measured in the painting's own
//! coordinates: a piece that moves keeps its numerals, so dragging the seam
//! re-strikes nothing.
//!
//! Compositing for text is single-alpha premultiplied, on a coverage that is
//! the largest of the three subpixel channels. This is a narrowing: the CPU
//! rasteriser blended each stripe against the plate under it, so a numeral on
//! an opaque plate carried a real colour fringe, and the largest of the three
//! also sets a heavier edge than their mean would. What a host with no
//! subpixel geometry saw is what every host sees now.
//!
//! # The badges
//!
//! [`Badges`] is the transient stack: one black rounded plate per thing the
//! appliance has to say for a moment, each a rounded rectangle like any other
//! shape here, with the terminal's own glyphs over it. `crate::overlay` is the
//! state machine (when, and how strongly); this is what it says.
//!
//! A badge is drawn after the chain rather than inside it for the same reason
//! the cabinet is: run through the curvature it would bend with the tube whose
//! size it is announcing. It is centred over the screen well, so a wide bank
//! does not push it off-centre over the glass.
//!
//! The plate is twice the text's bounding box, `radius: 5`, with the text
//! centred in it, and plate and glyphs carry one opacity. The typeface is
//! deliberate: this appliance has no platform UI font to draw with and would
//! not want one if it did, so the badge draws from the terminal's own glyph
//! atlas and is struck in the same phosphor face as the screen it sits over.
//!
//! Two rulers meet in it. A glyph is read at the atlas's **integer
//! magnification**, `term::render`'s rule, which is what lands the badge's
//! type on the same pixel lattice as the grid's. The corner radius is a
//! **logical** length and rides the device pixel ratio instead: a logical 5 px
//! stays a logical 5 px when the font's integer scale changes.
//!
//! # Which size is which
//!
//! Two sizes go in and they are deliberately different. The **rectangle** is
//! the bank column, in physical pixels, because it is a scissor and a quad.
//! The casting's `viewport_size` is the **screen well**, in logical pixels,
//! because the casting continues the bezel's metal field leftwards and is
//! drawn in the bezel's coordinates. The bezel reaches that same logical ruler
//! by dividing its own `OutputSize` by `windowScaling * DPR`
//! (`frame_metal.slang`'s `main`, and `crt::params::Params::build`);
//! [`well_ruler`] is this side of it.
//!
//! The two used to travel as one number each way round: a view declared at one
//! size while the quad covered another, in units that were physical
//! everywhere else in the mount. Here the well is a named field of the
//! parameter record and the rectangle is an instance attribute, so neither can
//! be read as the other.

use std::collections::{HashMap, HashSet};
use std::sync::Arc;

use bytemuck::{Pod, Zeroable};
use chassis::furniture::{Piece, PieceParams, Raster};
use chassis::paint::{Align, Face, Fill, Op, TextOp};
use term::GlyphAtlas;

use chassis::params::{ChassisMetalParams, CHASSIS_RECORD_FLOATS, PIECE_RECORD_FLOATS};

/// Which body draws an instance. The fragment stage switches on it, and the
/// values are the ones the WGSL glue below declares.
const KIND_CHASSIS: u32 = 0;
const KIND_PLATE: u32 = 1;
const KIND_LED: u32 = 2;
const KIND_TAPE: u32 = 3;
const KIND_RRECT: u32 = 4;
const KIND_RRECT_LINEAR: u32 = 5;
const KIND_RRECT_RADIAL: u32 = 6;
const KIND_ARC: u32 = 7;
const KIND_POLY: u32 = 8;
const KIND_TEXT: u32 = 9;
/// The badge's plate is a rounded rectangle with a solid fill and no
/// gradient, no border and no clip, which is [`KIND_RRECT`] exactly; only its
/// glyph read is a body of its own.
const KIND_GLYPH: u32 = 10;

/// One texel of transparent gutter between two rasters in the atlas, so a
/// piece sampling at exactly the far edge of its own rectangle reads nothing
/// rather than its neighbour's first lamp.
const GUTTER: u32 = 1;

/// Floats in one vector record, one gradient stop, and one polygon point:
/// what [`Chrome::fit`] sizes those three buffers by.
const VECTOR_RECORD_FLOATS: usize = 44;
const STOP_RECORD_FLOATS: usize = 8;
const POINT_RECORD_FLOATS: usize = 2;

/// One rectangle to draw.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Instance {
    /// `x`, `y`, `width`, `height` in physical pixels of the target.
    rect: [f32; 4],
    kind: u32,
    /// Index into this kind's parameter buffer.
    index: u32,
    _pad: [u32; 2],
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Uniforms {
    /// The target's own size in physical pixels, for the NDC divide.
    viewport: [f32; 2],
    _pad: [f32; 2],
}

/// One drawn operation's parameters, in the **piece's own** device pixels.
///
/// `origin` is where that piece's top-left sits on the target, so the
/// fragment stage subtracts it and is back in the painting's coordinates.
/// Keeping the arithmetic piece-local rather than target-absolute is what
/// keeps a radial gradient's two-circle solve inside f32's reach on a tall
/// column.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct VectorRecord {
    rect: [f32; 4],
    clip: [f32; 4],
    color: [f32; 4],
    border_color: [f32; 4],
    /// A radial gradient's inner circle as `(x, y, r)`; an arc's centre,
    /// radius and line width.
    g0: [f32; 4],
    /// A radial gradient's outer circle as `(x, y, r)`; an arc's start and
    /// end angle.
    g1: [f32; 4],
    /// A text run's place in the atlas, as origin and extent in 0..1.
    atlas: [f32; 4],
    origin: [f32; 2],
    radius: f32,
    border_width: f32,
    clip_radius: f32,
    opacity: f32,
    rotation: f32,
    pivot_x: f32,
    pivot_y: f32,
    /// 1 where a linear gradient runs left to right instead of top to bottom.
    horizontal: f32,
    span_offset: u32,
    span_count: u32,
    has_clip: u32,
    /// The WGSL struct's array stride rounds up to its 16-byte alignment;
    /// these three carry the same rounding on this side.
    _pad: [u32; 3],
}

/// One stop of a gradient, in the shape the WGSL `GradStop` declares.
#[repr(C)]
#[derive(Clone, Copy, Debug, Default, Pod, Zeroable)]
struct StopRecord {
    color: [f32; 4],
    position: f32,
    _pad: [f32; 3],
}

/// The glue around the shader bodies `chassis::shaders` compiles in: the
/// bindings, the vertex stage, and the switch on `kind`.
///
/// The bodies declare no bindings of their own, which is what lets the same
/// text serve this pass and the single-quad measurement rig.
const CHROME_WGSL: &str = r#"
@group(0) @binding(0) var<uniform> chrome: Uniforms;
@group(0) @binding(1) var<storage, read> castings: array<ChassisParams>;
@group(0) @binding(2) var<storage, read> plates: array<PlateParams>;
@group(0) @binding(3) var<storage, read> leds: array<LedParams>;
@group(0) @binding(4) var<storage, read> tapes: array<TapeParams>;
@group(0) @binding(5) var<storage, read> vectors: array<VectorParams>;
// The runs of gradient stops and polygon points every drawn shape indexes
// into, so a five-stop moulding costs the instance nothing.
@group(0) @binding(6) var<storage, read> stops: array<GradStop>;
@group(0) @binding(7) var<storage, read> points: array<vec2<f32>>;
@group(0) @binding(8) var atlas: texture_2d<f32>;
@group(0) @binding(9) var atlas_sampler: sampler;
// The terminal's own glyph atlas, which the badges strike their words from.
// Read with `textureLoad` and no sampler, exactly as the grid reads it.
@group(0) @binding(10) var glyphs: texture_2d<f32>;

// The raster read the display bodies and the text body ask the host for.
// Every raster lives in one atlas, so a body's own 0..1 coordinates are
// mapped through the rectangle its record carries.
//
// `textureSampleLevel` rather than `textureSample`: the bodies read their
// raster inside an early-out branch, and a plain sample may only be taken in
// uniform control flow.
fn chrome_sample(uv: vec2<f32>, rect: vec4<f32>) -> vec4<f32> {
    // Clamped to the raster's own outermost texel centres, not to its rect:
    // the rect's far edge is the gutter between entries, and a body reading
    // its spill band takes taps past 1.0 that have to land on the last lamp
    // the way a per-piece texture's clamp-to-edge put them.
    let half = 0.5 / vec2<f32>(textureDimensions(atlas, 0));
    let lo = rect.xy + half;
    let hi = max(lo, rect.xy + rect.zw - half);
    let p = clamp(rect.xy + uv * rect.zw, lo, hi);
    return textureSampleLevel(atlas, atlas_sampler, p, 0.0);
}

// One glyph of a badge, out of the terminal's atlas at the atlas's own
// integer magnification: recover the integer pixel, undo the quad's origin
// and the magnification, and load that texel.
//
// `rect` is the quad in physical pixels and `g0` carries the texel origin and
// the magnification, so the fragment is inside its own quad and the division
// is over a non-negative offset.
fn chrome_glyph(position: vec2<f32>, v: VectorParams) -> vec4<f32> {
    let cell = vec2<i32>(floor(position)) - vec2<i32>(v.rect.xy);
    let texel = vec2<i32>(v.g0.xy) + cell / i32(max(v.g0.z, 1.0));
    let coverage = textureLoad(glyphs, texel, 0).r;
    let a = v.color.a * coverage;
    return vec4<f32>(v.color.rgb * a, a);
}

struct Uniforms {
    viewport: vec2<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
    // Flat: these reach the fragment stage as the integers the CPU wrote.
    @location(1) @interpolate(flat) kind: u32,
    @location(2) @interpolate(flat) index: u32,
};

@vertex
fn vs(
    @builtin(vertex_index) vertex_index: u32,
    @location(0) rect: vec4<f32>,
    @location(1) kind: u32,
    @location(2) index: u32,
) -> VsOut {
    var corners = array<vec2<f32>, 6>(
        vec2<f32>(0.0, 0.0), vec2<f32>(1.0, 0.0), vec2<f32>(0.0, 1.0),
        vec2<f32>(1.0, 0.0), vec2<f32>(1.0, 1.0), vec2<f32>(0.0, 1.0),
    );
    let corner = corners[vertex_index];
    let px = rect.xy + corner * rect.zw;

    var out: VsOut;
    out.uv = corner;
    out.pos = vec4<f32>(
        px.x / chrome.viewport.x * 2.0 - 1.0,
        1.0 - px.y / chrome.viewport.y * 2.0,
        0.0,
        1.0,
    );
    out.kind = kind;
    out.index = index;
    return out;
}

@fragment
fn fs(input: VsOut) -> @location(0) vec4<f32> {
    if (input.kind == 0u) {
        return chassis_metal(input.uv, castings[input.index]);
    }
    if (input.kind == 1u) {
        return plate_metal(input.uv, plates[input.index]);
    }
    if (input.kind == 2u) {
        return led_matrix(input.uv, leds[input.index]);
    }
    if (input.kind == 3u) {
        return tape_label(input.uv, tapes[input.index]);
    }
    // The drawn half reads the fragment's own position rather than its `uv`:
    // a shape's arithmetic is in the piece's pixels, and the instance is only
    // the box that carries it.
    if (input.kind == 4u) {
        return vector_rect(input.pos.xy, vectors[input.index], 0u);
    }
    if (input.kind == 5u) {
        return vector_rect(input.pos.xy, vectors[input.index], 1u);
    }
    if (input.kind == 6u) {
        return vector_rect(input.pos.xy, vectors[input.index], 2u);
    }
    if (input.kind == 7u) {
        return vector_arc(input.pos.xy, vectors[input.index]);
    }
    if (input.kind == 8u) {
        return vector_polygon(input.pos.xy, vectors[input.index]);
    }
    if (input.kind == 9u) {
        return vector_text(input.pos.xy, vectors[input.index]);
    }
    if (input.kind == 10u) {
        return chrome_glyph(input.pos.xy, vectors[input.index]);
    }
    return vec4<f32>(0.0, 0.0, 0.0, 0.0);
}
"#;

/// What a struck line of text is keyed on: the run, and the box it is aligned
/// in, at the device size it was struck for.
///
/// Every measure in it is in the painting's own coordinates, which a piece
/// moving does not change, so a seam drag keeps the twelve numerals it has.
#[derive(Clone, PartialEq, Eq, Hash)]
struct RunKey {
    face: u8,
    face_name: &'static str,
    pixel_size: u32,
    letter_spacing: u64,
    bold: bool,
    text: String,
    x: u64,
    y: u64,
    width: u64,
    align: u8,
}

fn run_key(op: &TextOp, scale: f64) -> RunKey {
    let (face, face_name) = match op.face {
        Face::Catalogue(name) => (0u8, name),
        Face::Sans => (1, ""),
        Face::Serif => (2, ""),
    };
    RunKey {
        face,
        face_name,
        pixel_size: (op.pixel_size * scale).round().max(1.0) as u32,
        letter_spacing: (op.letter_spacing * scale).to_bits(),
        bold: op.bold,
        text: op.text.clone(),
        x: (op.x * scale).to_bits(),
        y: (op.y * scale).to_bits(),
        width: (op.width * scale).to_bits(),
        align: match op.align {
            Align::Left => 0,
            Align::Center => 1,
            Align::Right => 2,
        },
    }
}

/// One run's mask, and where it lands in the painting it belongs to.
struct StruckRun {
    raster: Raster,
    x: i32,
    y: i32,
}

/// The mount: one pipeline, the instance buffer, and one parameter buffer per
/// kind, each grown to what a frame asked for.
pub struct Chrome {
    pipeline: wgpu::RenderPipeline,
    layout: wgpu::BindGroupLayout,
    uniforms: wgpu::Buffer,
    /// One parameter buffer per kind, each grown to what a frame asked for:
    /// the casting's, then the plate's, the lamp grid's, the tape's, the
    /// drawn shapes', and the two runs those index into.
    castings: wgpu::Buffer,
    plates: wgpu::Buffer,
    leds: wgpu::Buffer,
    tapes: wgpu::Buffer,
    vectors: wgpu::Buffer,
    stops: wgpu::Buffer,
    points: wgpu::Buffer,
    /// How many records each buffer has room for.
    room: [usize; 7],
    instances: wgpu::Buffer,
    /// How many instances the instance buffer has room for.
    capacity: usize,
    /// Every display piece's raster and every struck line of text, in one
    /// texture, so one draw covers the whole bank.
    atlas: wgpu::Texture,
    atlas_size: (u32, u32),
    sampler: wgpu::Sampler,
    bind_group: wgpu::BindGroup,
    /// The lines of text struck so far, by run.
    text: HashMap<RunKey, StruckRun>,
    /// What the atlas already holds: where each raster was written and the
    /// bytes written there. The bank's plan repeats frame after frame and
    /// most of it is a fixed nameplate, and every write costs a texture
    /// transition, so the cheap frame is the one that writes nothing.
    uploaded: HashMap<Placed, ((u32, u32), Arc<[u8]>)>,
    /// The cabinet's half of the last frame, kept for the next one.
    cabinet: Option<CabinetDraw>,
}

impl Chrome {
    pub fn new(device: &wgpu::Device, format: wgpu::TextureFormat) -> Self {
        let source = format!(
            "{}{}{}{}{}{}{CHROME_WGSL}",
            chassis::shaders::COMMON_WGSL,
            chassis::shaders::CHASSIS_METAL_WGSL,
            chassis::shaders::PLATE_METAL_WGSL,
            chassis::shaders::LED_MATRIX_WGSL,
            chassis::shaders::TAPE_LABEL_WGSL,
            chassis::shaders::VECTOR_WGSL,
        );
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("chassis chrome"),
            source: wgpu::ShaderSource::Wgsl(source.into()),
        });
        let layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("chassis chrome"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                storage_entry(1),
                storage_entry(2),
                storage_entry(3),
                storage_entry(4),
                storage_entry(5),
                storage_entry(6),
                storage_entry(7),
                wgpu::BindGroupLayoutEntry {
                    binding: 8,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 9,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Sampler(wgpu::SamplerBindingType::NonFiltering),
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 10,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });
        let pipeline_layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("chassis chrome"),
            bind_group_layouts: &[Some(&layout)],
            immediate_size: 0,
        });
        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("chassis chrome"),
            layout: Some(&pipeline_layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 16,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Uint32,
                            offset: 20,
                            shader_location: 2,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format,
                    // Premultiplied source-over, so a piece with a soft edge
                    // lands on what is already there rather than replacing
                    // it. The casting is opaque and composites as a copy.
                    blend: Some(wgpu::BlendState::PREMULTIPLIED_ALPHA_BLENDING),
                    write_mask: wgpu::ColorWrites::ALL,
                })],
            }),
            primitive: wgpu::PrimitiveState::default(),
            depth_stencil: None,
            multisample: wgpu::MultisampleState::default(),
            multiview_mask: None,
            cache: None,
        });

        let capacity = 256;
        let room = [1usize, 4, 24, 24, 256, 128, 64];
        let uniforms = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("chassis chrome uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let castings = make_records(device, "castings", room[0] * CHASSIS_RECORD_FLOATS);
        let plates = make_records(device, "plates", room[1] * PIECE_RECORD_FLOATS);
        let leds = make_records(device, "lamp grids", room[2] * PIECE_RECORD_FLOATS);
        let tapes = make_records(device, "tapes", room[3] * PIECE_RECORD_FLOATS);
        let vectors = make_records(device, "shapes", room[4] * VECTOR_RECORD_FLOATS);
        let stops = make_records(device, "gradient stops", room[5] * STOP_RECORD_FLOATS);
        let points = make_records(device, "polygon points", room[6] * POINT_RECORD_FLOATS);
        let instances = make_instances(device, capacity);
        let atlas_size = (1, 1);
        let atlas = make_atlas(device, atlas_size);
        let sampler = device.create_sampler(&wgpu::SamplerDescriptor {
            label: Some("chassis chrome atlas"),
            // Nearest, and never anything else: a lamp grid reads exactly one
            // texel per cell and a filtered sample would light lamps between
            // two of them; the tape's dilation reads a nearest sample by hand
            // and the kit's oracle test is written against that. A line of
            // text is laid one texel to the device pixel for the same reason.
            mag_filter: wgpu::FilterMode::Nearest,
            min_filter: wgpu::FilterMode::Nearest,
            ..Default::default()
        });
        let bind_group = make_bind(
            device,
            &layout,
            &uniforms,
            &[&castings, &plates, &leds, &tapes, &vectors, &stops, &points],
            &atlas,
            &sampler,
            None,
        );
        Self {
            pipeline,
            layout,
            uniforms,
            castings,
            plates,
            leds,
            tapes,
            vectors,
            stops,
            points,
            room,
            instances,
            capacity,
            atlas,
            atlas_size,
            sampler,
            bind_group,
            text: HashMap::new(),
            uploaded: HashMap::new(),
            cabinet: None,
        }
    }

    /// Record the chrome over whatever is already on `output`.
    ///
    /// `target` is the swapchain image's size in physical pixels; `column` is
    /// the bank's rectangle at its left edge, also physical; `well` is the
    /// screen well's physical size, which [`well_ruler`] puts on the logical
    /// ruler the casting's field is measured in; `scale_factor` is the
    /// window's device pixel ratio, for that conversion.
    ///
    /// `casting` is the bank's floor. `None` draws no floor at all, which is
    /// a mount asked for the furniture on its own and nothing under it.
    ///
    /// `pieces` is `chassis::Cabinet::furniture`, whose rectangles are
    /// logical and are scaled by the same ratio on the way in.
    ///
    /// `badges` is the stack over the glass, top first, centred in the well
    /// the column leaves. The answer is index-aligned with its entries --
    /// where each badge landed, or `None` for one that drew nothing (faded
    /// out, or empty text) -- so a caller can assert on the rectangle this
    /// chose rather than re-deriving it.
    ///
    /// A column of no width draws no cabinet, which is what a hidden chassis
    /// is: no bank rather than a bank of zero pixels. The badges still go on,
    /// because the glass is still there.
    #[allow(clippy::too_many_arguments)]
    pub fn render(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        output: &wgpu::TextureView,
        target: (u32, u32),
        column: (u32, u32),
        well: (u32, u32),
        scale_factor: f64,
        casting: Option<&ChassisMetalParams>,
        pieces: &std::sync::Arc<[Piece]>,
        badges: Option<Badges>,
    ) -> Vec<Option<BadgeRect>> {
        let mut rects = vec![None; badges.as_ref().map_or(0, |b| b.entries.len())];
        if target.0 == 0 || target.1 == 0 {
            return rects;
        }
        let cabinet = column.0 > 0 && column.1 > 0;
        let kept = std::sync::Arc::clone(pieces);
        let pieces: &[Piece] = if cabinet { pieces } else { &[] };
        let casting = if cabinet { casting } else { None };

        let key = CabinetKey {
            pieces: pieces.as_ptr(),
            count: pieces.len(),
            column,
            well,
            scale: scale_factor.to_bits(),
            casting: casting.copied(),
            atlas: self.atlas_size,
            struck: self.text.len(),
        };

        // The cabinet is the same picture until a channel changes or the
        // window is resized, so its records are built against the key above
        // and kept. `regrown` stays false on a reuse: the atlas cannot have
        // moved without the key moving with it.
        let mut regrown = false;
        if self.cabinet.as_ref().is_none_or(|c| c.key != key) {
            // Where each piece lands, asked once and read three times below.
            let dests: Vec<Option<(i32, i32, u32, u32)>> = pieces
                .iter()
                .map(|p| scale_rect(p.rect, scale_factor, column))
                .collect();

            // The lines of text first: a run this frame asks for and has not
            // struck before goes through swash here, and the atlas is packed
            // after, because a record carries where its mask landed.
            let runs = self.strike_text(pieces, &dests, scale_factor);
            let plan = pack(pieces, &runs, &self.text);
            regrown = self.fit_atlas(device, plan.size);
            // A new texture holds none of what the old one did.
            if regrown {
                self.uploaded.clear();
            }
            for (what, at) in &plan.places {
                if let Some(raster) = raster_of(pieces, &self.text, what.clone()) {
                    // The very same bytes, not merely equal ones: the bank hands
                    // back the rasters it built earlier, so the common answer is
                    // yes and it costs a pointer comparison.
                    let held = self.uploaded.get(what);
                    if held.is_some_and(|(was, bytes)| was == at && Arc::ptr_eq(bytes, &raster.rgba)) {
                        continue;
                    }
                    upload(queue, &self.atlas, raster, *at);
                    self.uploaded
                        .insert(what.clone(), (*at, Arc::clone(&raster.rgba)));
                }
            }
            let uv_of = |what: Placed| -> [f32; 4] {
                let (w, h) = (self.atlas_size.0 as f32, self.atlas_size.1 as f32);
                match plan.places.iter().find(|(p, _)| *p == what) {
                    Some((_, (x, y))) => match raster_of(pieces, &self.text, what) {
                        Some(r) => [
                            *x as f32 / w,
                            *y as f32 / h,
                            r.width.max(1) as f32 / w,
                            r.height.max(1) as f32 / h,
                        ],
                        None => [0.0, 0.0, 1.0, 1.0],
                    },
                    None => [0.0, 0.0, 1.0, 1.0],
                }
            };

            let mut castings: Vec<[f32; CHASSIS_RECORD_FLOATS]> = Vec::new();
            let mut plates: Vec<[f32; PIECE_RECORD_FLOATS]> = Vec::new();
            let mut leds: Vec<[f32; PIECE_RECORD_FLOATS]> = Vec::new();
            let mut tapes: Vec<[f32; PIECE_RECORD_FLOATS]> = Vec::new();
            let mut vectors: Vec<VectorRecord> = Vec::new();
            let mut stops: Vec<StopRecord> = Vec::new();
            let mut points: Vec<[f32; 2]> = Vec::new();
            let mut instances: Vec<Instance> = Vec::new();

            // The casting is the bank's floor, so it goes first and everything
            // else keeps the plan's own order: one draw, painter's order, no sort.
            if let Some(casting) = casting {
                let ruler = well_ruler(well, scale_factor);
                castings.push(casting.record([ruler.0 as f32, ruler.1 as f32]));
                instances.push(Instance {
                    rect: [0.0, 0.0, column.0 as f32, column.1 as f32],
                    kind: KIND_CHASSIS,
                    index: 0,
                    _pad: [0, 0],
                });
            }

            let scale = if scale_factor > 0.0 {
                scale_factor
            } else {
                1.0
            };
            for (i, piece) in pieces.iter().enumerate() {
                let Some(dest) = dests[i] else {
                    continue;
                };
                let rect = [dest.0 as f32, dest.1 as f32, dest.2 as f32, dest.3 as f32];
                let (kind, index) = match piece.params {
                    Some(PieceParams::Plate(p)) => {
                        plates.push(p.record());
                        (KIND_PLATE, plates.len() - 1)
                    }
                    Some(PieceParams::Led(p)) => {
                        leds.push(p.record(uv_of(Placed::Source(i))));
                        (KIND_LED, leds.len() - 1)
                    }
                    Some(PieceParams::Tape(p)) => {
                        tapes.push(p.record(uv_of(Placed::Source(i))));
                        (KIND_TAPE, tapes.len() - 1)
                    }
                    None => {
                        let Some(painting) = piece.paint.as_ref() else {
                            continue;
                        };
                        for (o, op) in painting.ops.iter().enumerate() {
                            let text = runs.get(&(i, o)).and_then(|k| self.text.get(k));
                            let atlas = match runs.get(&(i, o)) {
                                Some(k) => uv_of(Placed::Run(k.clone())),
                                None => [0.0, 0.0, 1.0, 1.0],
                            };
                            let Some((kind, record, span)) =
                                op_record(op, scale, dest, text, atlas, stops.len(), points.len())
                            else {
                                continue;
                            };
                            let Some(rect) = span.rect else {
                                continue;
                            };
                            stops.extend(span.stops);
                            points.extend(span.points);
                            vectors.push(record);
                            instances.push(Instance {
                                rect,
                                kind,
                                index: (vectors.len() - 1) as u32,
                                _pad: [0, 0],
                            });
                        }
                        continue;
                    }
                };
                instances.push(Instance {
                    rect,
                    kind,
                    index: index as u32,
                    _pad: [0, 0],
                });
            }

            self.cabinet = Some(CabinetDraw {
                key,
                _pieces: kept,
                castings,
                plates,
                leds,
                tapes,
                vectors,
                stops,
                points,
                instances,
            });
        }
        let mut draw = self
            .cabinet
            .take()
            .expect("the cabinet is built just above when its key has moved");
        let cabinet_count = draw.instances.len();
        let cabinet_vectors = draw.vectors.len();
        // The cabinet's own draw ends at `cabinet_count`; what follows goes
        // over the glass under a scissor of its own. The badges are appended
        // to the kept records and cut away again below, so a settled bank
        // reaches this point without allocating.
        if let Some(badges) = badges.as_ref() {
            rects = badge_instances(
                badges,
                (column.0 as i32, 0, well.0, well.1),
                scale_factor,
                &mut draw.vectors,
                &mut draw.instances,
            );
        }

        let mut rebind = self.fit(device, 0, draw.castings.len(), CHASSIS_RECORD_FLOATS);
        rebind |= self.fit(device, 1, draw.plates.len(), PIECE_RECORD_FLOATS);
        rebind |= self.fit(device, 2, draw.leds.len(), PIECE_RECORD_FLOATS);
        rebind |= self.fit(device, 3, draw.tapes.len(), PIECE_RECORD_FLOATS);
        rebind |= self.fit(device, 4, draw.vectors.len(), VECTOR_RECORD_FLOATS);
        rebind |= self.fit(device, 5, draw.stops.len(), STOP_RECORD_FLOATS);
        rebind |= self.fit(device, 6, draw.points.len(), POINT_RECORD_FLOATS);
        if draw.instances.len() > self.capacity {
            self.capacity = draw.instances.len().next_power_of_two();
            self.instances = make_instances(device, self.capacity);
        }
        if rebind || regrown {
            self.rebind(device);
        }

        write_records(queue, &self.castings, &draw.castings);
        write_records(queue, &self.plates, &draw.plates);
        write_records(queue, &self.leds, &draw.leds);
        write_records(queue, &self.tapes, &draw.tapes);
        write_slice(queue, &self.vectors, &draw.vectors);
        write_slice(queue, &self.stops, &draw.stops);
        write_slice(queue, &self.points, &draw.points);
        let instances = std::mem::take(&mut draw.instances);
        if instances.is_empty() {
            draw.instances = instances;
            self.keep(draw, cabinet_count, cabinet_vectors);
            return rects;
        }
        queue.write_buffer(&self.instances, 0, bytemuck::cast_slice(&instances));
        queue.write_buffer(
            &self.uniforms,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [target.0 as f32, target.1 as f32],
                _pad: [0.0, 0.0],
            }),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("chassis chrome"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view: output,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    // Load, not Clear: the glass is already on this image.
                    load: wgpu::LoadOp::Load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // A frame carrying badges binds the terminal's atlas, and does it
        // afresh rather than against a cached identity: a font change swaps
        // the atlas underneath this mount and wgpu 30 has no cheap identity to
        // compare one against (`global_id` is gone), so the choice is a stale
        // binding or a bind group per frame. Badges are drawn only while one
        // of them is up, so the second costs nothing worth a correctness risk.
        let with_glyphs = badges.as_ref().map(|b| {
            make_bind(
                device,
                &self.layout,
                &self.uniforms,
                &[
                    &self.castings,
                    &self.plates,
                    &self.leds,
                    &self.tapes,
                    &self.vectors,
                    &self.stops,
                    &self.points,
                ],
                &self.atlas,
                &self.sampler,
                Some(&b.atlas.view),
            )
        });

        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(
            0,
            Some(with_glyphs.as_ref().unwrap_or(&self.bind_group)),
            &[],
        );
        pass.set_vertex_buffer(0, self.instances.slice(..));
        // The bank clips its children, and this is that clip: a piece is
        // allowed to hang off the column and one routinely does, so the
        // scissor is the column's rectangle clamped to the target.
        if cabinet_count > 0 {
            let w = column.0.min(target.0);
            let h = column.1.min(target.1);
            pass.set_scissor_rect(0, 0, w, h);
            pass.draw(0..6, 0..cabinet_count as u32);
        }
        // The badges are over the glass, so their clip is the whole target.
        if instances.len() > cabinet_count {
            pass.set_scissor_rect(0, 0, target.0, target.1);
            pass.draw(0..6, cabinet_count as u32..instances.len() as u32);
        }
        drop(pass);
        draw.instances = instances;
        self.keep(draw, cabinet_count, cabinet_vectors);
        rects
    }

    /// Put the cabinet's records back with the badges cut off them, ready to
    /// be reused as they are on the next frame.
    fn keep(&mut self, mut draw: CabinetDraw, instances: usize, vectors: usize) {
        draw.instances.truncate(instances);
        draw.vectors.truncate(vectors);
        self.cabinet = Some(draw);
    }

    /// Grow one kind's parameter buffer to hold `wanted` records; `true` when
    /// it was replaced and the bind group has to be rebuilt.
    fn fit(&mut self, device: &wgpu::Device, kind: usize, wanted: usize, floats: usize) -> bool {
        if wanted <= self.room[kind] {
            return false;
        }
        self.room[kind] = wanted.next_power_of_two();
        let size = self.room[kind] * floats;
        let (buffer, label) = match kind {
            0 => (&mut self.castings, "castings"),
            1 => (&mut self.plates, "plates"),
            2 => (&mut self.leds, "lamp grids"),
            3 => (&mut self.tapes, "tapes"),
            4 => (&mut self.vectors, "shapes"),
            5 => (&mut self.stops, "gradient stops"),
            _ => (&mut self.points, "polygon points"),
        };
        *buffer = make_records(device, label, size);
        true
    }

    /// Strike every line of text this frame's paintings ask for that is not
    /// struck already, and forget the runs the frame did not ask for.
    ///
    /// The answer says which run each text op belongs to, by `(piece,
    /// operation)`: the second pass builds records in the same order and
    /// reads its mask out of that.
    fn strike_text(
        &mut self,
        pieces: &[Piece],
        dests: &[Option<(i32, i32, u32, u32)>],
        scale_factor: f64,
    ) -> HashMap<(usize, usize), RunKey> {
        let scale = if scale_factor > 0.0 {
            scale_factor
        } else {
            1.0
        };
        let mut asked: HashMap<(usize, usize), RunKey> = HashMap::new();
        let mut alive: HashSet<RunKey> = HashSet::new();
        for (i, piece) in pieces.iter().enumerate() {
            if dests[i].is_none() {
                continue;
            }
            let Some(painting) = piece.paint.as_ref() else {
                continue;
            };
            for (o, op) in painting.ops.iter().enumerate() {
                let Op::Text(t) = op else {
                    continue;
                };
                if t.opacity <= 0.0 {
                    continue;
                }
                let key = run_key(t, scale);
                if !self.text.contains_key(&key) {
                    let Some((raster, x, y)) = chassis::paint::text_raster(t, scale) else {
                        continue;
                    };
                    self.text.insert(key.clone(), StruckRun { raster, x, y });
                }
                alive.insert(key.clone());
                asked.insert((i, o), key);
            }
        }
        self.text.retain(|k, _| alive.contains(k));
        asked
    }

    /// Grow the atlas to hold `size` texels. Only ever grows: the bank's plan
    /// is the same list of rectangles frame after frame, so after the first
    /// one this allocates nothing.
    fn fit_atlas(&mut self, device: &wgpu::Device, size: (u32, u32)) -> bool {
        let wanted = (
            size.0.max(self.atlas_size.0).max(1),
            size.1.max(self.atlas_size.1).max(1),
        );
        if wanted == self.atlas_size {
            return false;
        }
        self.atlas_size = wanted;
        self.atlas = make_atlas(device, wanted);
        true
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        self.bind_group = make_bind(
            device,
            &self.layout,
            &self.uniforms,
            &[
                &self.castings,
                &self.plates,
                &self.leds,
                &self.tapes,
                &self.vectors,
                &self.stops,
                &self.points,
            ],
            &self.atlas,
            &self.sampler,
            None,
        );
    }
}

/// The badge's corner radius, in logical pixels.
const CORNER_RADIUS: f32 = 5.0;

/// One badge in the stack: what it says, and how strongly.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct Entry<'a> {
    pub text: &'a str,
    pub opacity: f32,
}

/// The badge stack a frame carries, and the atlas it is struck from.
///
/// Slot 0 is the size overlay's own position and everything below it stacks
/// beneath that: what this appliance has to say -- the grid's new size, a
/// write queue shedding (`crate::overlay::Notice`), a carrier's plate -- is
/// independent and can be true at the same moment, so stacking is what keeps
/// one from being invisible whenever another is up.
pub struct Badges<'a> {
    pub atlas: &'a GlyphAtlas,
    /// The atlas's integer magnification, `term::render`'s ruler.
    pub scale: u32,
    /// Top first.
    pub entries: &'a [Entry<'a>],
}

/// Where a badge went, in physical pixels: origin and size. Returned so a
/// test can assert on the rectangle the mount chose rather than re-deriving
/// it, and `None` when there was nothing to draw.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct BadgeRect {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// The stack's instances, appended to the frame's buffers, and where each
/// badge landed.
///
/// `well` is the screen well in physical pixels: the rectangle the chain drew
/// into, which is the window less the bank column. A badge centres in exactly
/// that, not in the window, so a wide bank does not push it off-centre over
/// the glass.
fn badge_instances(
    badges: &Badges,
    well: (i32, i32, u32, u32),
    scale_factor: f64,
    vectors: &mut Vec<VectorRecord>,
    instances: &mut Vec<Instance>,
) -> Vec<Option<BadgeRect>> {
    let mut rects: Vec<Option<BadgeRect>> = vec![None; badges.entries.len()];
    let scale = badges.scale.max(1);
    let cell = badges.atlas.cell;
    if cell.width == 0 || cell.height == 0 {
        return rects;
    }
    let (well_x, well_y, well_w, well_h) = well;
    // The step down the stack: one badge (two text heights) plus a text
    // height of air, so two plates read as two and not as one tall one.
    // Every badge is the same height -- the atlas cell decides it -- so the
    // step does not depend on which entry is being placed.
    let text_h = cell.height as i32;
    let step = 3 * text_h * scale as i32;

    let mut push = |rect: [f32; 4], kind: u32, record: VectorRecord| {
        vectors.push(record);
        instances.push(Instance {
            rect,
            kind,
            index: (vectors.len() - 1) as u32,
            _pad: [0, 0],
        });
    };

    for (slot, entry) in badges.entries.iter().enumerate() {
        if entry.opacity <= 0.0 || entry.text.is_empty() {
            continue;
        }
        // The plate is twice the text's bounding box, and the text sits in
        // the middle of it.
        let chars: Vec<char> = entry.text.chars().collect();
        let text_w = (chars.len() as u32 * cell.width) as i32;
        let badge_w = text_w * 2;
        let badge_h = text_h * 2;

        // Centred in the well, on whole physical pixels: half a pixel of
        // offset would put every glyph between two of them, which is the
        // same rule `draw_frame` centres the grid by.
        let origin_x = well_x + (well_w as i32 - badge_w * scale as i32) / 2;
        let origin_y = well_y + (well_h as i32 - badge_h * scale as i32) / 2 + slot as i32 * step;
        let alpha = entry.opacity.clamp(0.0, 1.0);
        let plate = [
            origin_x as f32,
            origin_y as f32,
            (badge_w * scale as i32) as f32,
            (badge_h * scale as i32) as f32,
        ];
        // `color: "black"`, faded by this badge's own envelope. The corner
        // radius is a logical length, so it rides the device pixel ratio
        // rather than the font's integer scale, and is never larger than the
        // box can hold: a badge narrower than 2r would invert its own
        // corners, which `vector_rrect_distance` clamps.
        push(
            plate,
            KIND_RRECT,
            VectorRecord {
                rect: plate,
                color: [0.0, 0.0, 0.0, alpha],
                radius: CORNER_RADIUS * scale_factor.max(f64::EPSILON) as f32,
                opacity: 1.0,
                ..VectorRecord::default()
            },
        );

        // The text's own top-left inside the badge: centred, so half the
        // doubling on each side.
        let text_x = text_w / 2;
        let text_y = text_h / 2;
        for (i, c) in chars.iter().enumerate() {
            let Some(glyph) = badges.atlas.slot(*c) else {
                continue;
            };
            if glyph.width == 0 || glyph.height == 0 {
                continue;
            }
            let dst = [
                text_x + i as i32 * cell.width as i32 + glyph.left,
                text_y + cell.baseline - glyph.top,
            ];
            let quad = [
                (origin_x + dst[0] * scale as i32) as f32,
                (origin_y + dst[1] * scale as i32) as f32,
                (glyph.width * scale) as f32,
                (glyph.height * scale) as f32,
            ];
            push(
                quad,
                KIND_GLYPH,
                VectorRecord {
                    rect: quad,
                    // `color: "white"`.
                    color: [1.0, 1.0, 1.0, alpha],
                    g0: [
                        glyph.atlas_x as f32,
                        glyph.atlas_y as f32,
                        scale as f32,
                        0.0,
                    ],
                    opacity: 1.0,
                    ..VectorRecord::default()
                },
            );
        }

        rects[slot] = Some(BadgeRect {
            x: origin_x,
            y: origin_y,
            width: (badge_w * scale as i32).max(0) as u32,
            height: (badge_h * scale as i32).max(0) as u32,
        });
    }
    rects
}

/// What the cabinet's half of a frame was built from. A frame whose key
/// matches the one in hand would rebuild the same records from the same
/// pieces, so it reuses them instead.
///
/// The pieces are identified by the address of their allocation, and
/// [`CabinetDraw`] holds the `Arc` alive so that no later allocation can
/// take that address and pass for it.
#[derive(PartialEq)]
struct CabinetKey {
    pieces: *const Piece,
    count: usize,
    column: (u32, u32),
    well: (u32, u32),
    scale: u64,
    casting: Option<ChassisMetalParams>,
    atlas: (u32, u32),
    struck: usize,
}

/// The cabinet's records, and what they were built from. The badges' own
/// records are appended to these each frame and truncated away again, so the
/// vectors keep their capacity and a settled bank allocates nothing.
struct CabinetDraw {
    key: CabinetKey,
    _pieces: std::sync::Arc<[Piece]>,
    castings: Vec<[f32; CHASSIS_RECORD_FLOATS]>,
    plates: Vec<[f32; PIECE_RECORD_FLOATS]>,
    leds: Vec<[f32; PIECE_RECORD_FLOATS]>,
    tapes: Vec<[f32; PIECE_RECORD_FLOATS]>,
    vectors: Vec<VectorRecord>,
    stops: Vec<StopRecord>,
    points: Vec<[f32; 2]>,
    instances: Vec<Instance>,
}

/// One thing in the atlas: a display piece's own raster, or a struck line.
#[derive(Clone, PartialEq, Eq, Hash)]
enum Placed {
    Source(usize),
    Run(RunKey),
}

/// Where each raster this frame needs goes in the atlas.
///
/// One column, each raster under the last with a gutter row between: the
/// rasters are a lamp grid, a glyph mask or a two-figure numeral, tens of
/// texels on a side, so a shelf packer would buy nothing a bank of twenty
/// strips can measure.
struct Packing {
    size: (u32, u32),
    /// `(what, top-left in texels)`, in the plan's own order.
    places: Vec<(Placed, (u32, u32))>,
}

fn raster_of<'a>(
    pieces: &'a [Piece],
    text: &'a HashMap<RunKey, StruckRun>,
    what: Placed,
) -> Option<&'a Raster> {
    match what {
        Placed::Source(i) => pieces[i].source.as_ref(),
        Placed::Run(key) => text.get(&key).map(|r| &r.raster),
    }
}

fn pack(
    pieces: &[Piece],
    runs: &HashMap<(usize, usize), RunKey>,
    text: &HashMap<RunKey, StruckRun>,
) -> Packing {
    let mut places: Vec<(Placed, (u32, u32))> = Vec::new();
    let mut seen: HashSet<Placed> = HashSet::new();
    let mut width = 1u32;
    let mut y = 0u32;
    let mut put = |what: Placed, raster: &Raster, width: &mut u32, y: &mut u32| {
        if !seen.insert(what.clone()) {
            return;
        }
        let (w, h) = (raster.width.max(1), raster.height.max(1));
        places.push((what, (0, *y)));
        *width = (*width).max(w + GUTTER);
        *y += h + GUTTER;
    };
    for (i, piece) in pieces.iter().enumerate() {
        if let Some(raster) = piece.source.as_ref() {
            put(Placed::Source(i), raster, &mut width, &mut y);
        }
        if let Some(painting) = piece.paint.as_ref() {
            for o in 0..painting.ops.len() {
                let Some(key) = runs.get(&(i, o)) else {
                    continue;
                };
                let Some(run) = text.get(key) else {
                    continue;
                };
                put(Placed::Run(key.clone()), &run.raster, &mut width, &mut y);
            }
        }
    }
    Packing {
        size: (width, y.max(1)),
        places,
    }
}

/// The runs one drawn shape adds to the two shared buffers, and the box its
/// instance covers.
#[derive(Default)]
struct Span {
    rect: Option<[f32; 4]>,
    stops: Vec<StopRecord>,
    points: Vec<[f32; 2]>,
}

/// The record and instance rectangle for one drawn operation, or `None` for
/// one that covers nothing.
///
/// `dest` is the piece's rectangle on the target; every measure in the record
/// is the operation's own, scaled to device pixels and left in the painting's
/// coordinates.
fn op_record(
    op: &Op,
    scale: f64,
    dest: (i32, i32, u32, u32),
    text: Option<&StruckRun>,
    atlas: [f32; 4],
    stop_base: usize,
    point_base: usize,
) -> Option<(u32, VectorRecord, Span)> {
    let mut v = VectorRecord {
        origin: [dest.0 as f32, dest.1 as f32],
        opacity: 1.0,
        ..VectorRecord::default()
    };
    let mut span = Span::default();
    let kind = match op {
        Op::Rect(r) => {
            if r.opacity <= 0.0 || r.rect.width <= 0.0 || r.rect.height <= 0.0 {
                return None;
            }
            let rect = scaled(r.rect.x, r.rect.y, r.rect.width, r.rect.height, scale);
            v.rect = rect;
            v.radius = (r.radius * scale) as f32;
            v.opacity = r.opacity;
            if let Some((cr, radius)) = r.clip.as_ref() {
                v.clip = scaled(cr.x, cr.y, cr.width, cr.height, scale);
                v.clip_radius = (radius * scale) as f32;
                v.has_clip = 1;
            }
            if let Some((bw, color)) = r.border {
                v.border_width = (bw * scale) as f32;
                v.border_color = [color.r, color.g, color.b, color.a];
            }
            if let Some((angle, (px, py))) = r.rotation {
                v.rotation = angle as f32;
                v.pivot_x = (px * scale) as f32;
                v.pivot_y = (py * scale) as f32;
            }
            // A rotated rectangle sweeps a wider box; its diagonal covers it.
            let pad = 1.0
                + if r.rotation.is_some() {
                    f64::from(rect[2] * rect[2] + rect[3] * rect[3]).sqrt()
                } else {
                    0.0
                };
            let (mut x0, mut y0) = (f64::from(rect[0]) - pad, f64::from(rect[1]) - pad);
            let (mut x1, mut y1) = (
                f64::from(rect[0] + rect[2]) + pad,
                f64::from(rect[1] + rect[3]) + pad,
            );
            // Clipped: nothing outside the clip can be drawn, so the smaller
            // of the two boxes is the one to cover.
            if r.clip.is_some() {
                x0 = x0.max(f64::from(v.clip[0]) - 1.0);
                y0 = y0.max(f64::from(v.clip[1]) - 1.0);
                x1 = x1.min(f64::from(v.clip[0] + v.clip[2]) + 1.0);
                y1 = y1.min(f64::from(v.clip[1] + v.clip[3]) + 1.0);
            }
            span.rect = op_rect(x0, y0, x1, y1, dest);
            match &r.fill {
                Fill::Solid(c) => {
                    v.color = [c.r, c.g, c.b, c.a];
                    KIND_RRECT
                }
                Fill::Linear { horizontal, stops } => {
                    v.horizontal = if *horizontal { 1.0 } else { 0.0 };
                    v.span_offset = stop_base as u32;
                    v.span_count = stops.len() as u32;
                    span.stops = stops.iter().map(stop_record).collect();
                    KIND_RRECT_LINEAR
                }
                Fill::Radial { from, to, stops } => {
                    // The two circles are authored in the same logical pixels
                    // as the rectangle and the shape's arithmetic walks device
                    // ones, so they are scaled here with the rest of the
                    // geometry. Unscaled, at DPR 2 the circles stayed in the
                    // top-left quarter while the fragment covered all of it,
                    // and every screw dome came out flat.
                    v.g0 = [
                        (from.0 * scale) as f32,
                        (from.1 * scale) as f32,
                        (from.2 * scale) as f32,
                        0.0,
                    ];
                    v.g1 = [
                        (to.0 * scale) as f32,
                        (to.1 * scale) as f32,
                        (to.2 * scale) as f32,
                        0.0,
                    ];
                    v.span_offset = stop_base as u32;
                    v.span_count = stops.len() as u32;
                    span.stops = stops.iter().map(stop_record).collect();
                    KIND_RRECT_RADIAL
                }
            }
        }
        Op::Arc(a) => {
            let (cx, cy) = (a.center.0 * scale, a.center.1 * scale);
            let radius = a.radius * scale;
            let half = (a.line_width * scale) / 2.0;
            v.g0 = [
                cx as f32,
                cy as f32,
                radius as f32,
                (a.line_width * scale) as f32,
            ];
            v.g1 = [a.start as f32, a.end as f32, 0.0, 0.0];
            v.color = [a.color.r, a.color.g, a.color.b, a.color.a];
            let reach = radius + half + 1.0;
            span.rect = op_rect(cx - reach, cy - reach, cx + reach, cy + reach, dest);
            KIND_ARC
        }
        Op::Polygon(g) => {
            if g.points.len() < 3 || g.opacity <= 0.0 {
                return None;
            }
            let pts: Vec<[f32; 2]> = g
                .points
                .iter()
                .map(|(x, y)| [(x * scale) as f32, (y * scale) as f32])
                .collect();
            let (mut x0, mut y0) = (f64::MAX, f64::MAX);
            let (mut x1, mut y1) = (f64::MIN, f64::MIN);
            for p in &pts {
                x0 = x0.min(f64::from(p[0]));
                y0 = y0.min(f64::from(p[1]));
                x1 = x1.max(f64::from(p[0]));
                y1 = y1.max(f64::from(p[1]));
            }
            v.color = [g.color.r, g.color.g, g.color.b, g.color.a];
            v.opacity = g.opacity;
            v.span_offset = point_base as u32;
            v.span_count = pts.len() as u32;
            span.points = pts;
            span.rect = op_rect(x0, y0, x1, y1, dest);
            KIND_POLY
        }
        Op::Text(t) => {
            if t.opacity <= 0.0 {
                return None;
            }
            let run = text?;
            let (w, h) = (run.raster.width as f64, run.raster.height as f64);
            v.rect = [run.x as f32, run.y as f32, w as f32, h as f32];
            v.atlas = atlas;
            v.color = [t.color.r, t.color.g, t.color.b, t.color.a];
            v.opacity = t.opacity;
            span.rect = op_rect(
                f64::from(run.x),
                f64::from(run.y),
                f64::from(run.x) + w,
                f64::from(run.y) + h,
                dest,
            );
            KIND_TEXT
        }
    };
    Some((kind, v, span))
}

fn stop_record(stop: &chassis::paint::Stop) -> StopRecord {
    StopRecord {
        color: [stop.color.r, stop.color.g, stop.color.b, stop.color.a],
        position: stop.position as f32,
        _pad: [0.0; 3],
    }
}

fn scaled(x: f64, y: f64, w: f64, h: f64, scale: f64) -> [f32; 4] {
    [
        (x * scale) as f32,
        (y * scale) as f32,
        (w * scale) as f32,
        (h * scale) as f32,
    ]
}

/// One operation's box, in the piece's own device pixels, cut at the piece's
/// bounds and moved onto the target.
///
/// An operation reaching past its piece is clipped there: the piece's own
/// rectangle is the box nothing it draws may cross.
fn op_rect(x0: f64, y0: f64, x1: f64, y1: f64, dest: (i32, i32, u32, u32)) -> Option<[f32; 4]> {
    let lx0 = x0.floor().max(0.0);
    let ly0 = y0.floor().max(0.0);
    let lx1 = x1.ceil().min(f64::from(dest.2));
    let ly1 = y1.ceil().min(f64::from(dest.3));
    if lx1 <= lx0 || ly1 <= ly0 {
        return None;
    }
    Some([
        lx0 as f32 + dest.0 as f32,
        ly0 as f32 + dest.1 as f32,
        (lx1 - lx0) as f32,
        (ly1 - ly0) as f32,
    ])
}

fn write_records<const N: usize>(queue: &wgpu::Queue, buffer: &wgpu::Buffer, records: &[[f32; N]]) {
    if records.is_empty() {
        return;
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(records));
}

fn write_slice<T: Pod>(queue: &wgpu::Queue, buffer: &wgpu::Buffer, records: &[T]) {
    if records.is_empty() {
        return;
    }
    queue.write_buffer(buffer, 0, bytemuck::cast_slice(records));
}

fn upload(queue: &wgpu::Queue, atlas: &wgpu::Texture, raster: &Raster, at: (u32, u32)) {
    let (w, h) = (raster.width.max(1), raster.height.max(1));
    if raster.rgba.len() < (w * h * 4) as usize {
        return;
    }
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture: atlas,
            mip_level: 0,
            origin: wgpu::Origin3d {
                x: at.0,
                y: at.1,
                z: 0,
            },
            aspect: wgpu::TextureAspect::All,
        },
        &raster.rgba,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(w * 4),
            rows_per_image: Some(h),
        },
        wgpu::Extent3d {
            width: w,
            height: h,
            depth_or_array_layers: 1,
        },
    );
}

fn make_atlas(device: &wgpu::Device, size: (u32, u32)) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("chassis chrome atlas"),
        size: wgpu::Extent3d {
            width: size.0.max(1),
            height: size.1.max(1),
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::Rgba8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

fn storage_entry(binding: u32) -> wgpu::BindGroupLayoutEntry {
    wgpu::BindGroupLayoutEntry {
        binding,
        visibility: wgpu::ShaderStages::FRAGMENT,
        ty: wgpu::BindingType::Buffer {
            ty: wgpu::BufferBindingType::Storage { read_only: true },
            has_dynamic_offset: false,
            min_binding_size: None,
        },
        count: None,
    }
}

fn make_records(device: &wgpu::Device, label: &str, floats: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some(&format!("chassis chrome {label}")),
        size: (floats.max(1) * 4) as u64,
        usage: wgpu::BufferUsages::STORAGE | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

fn make_instances(device: &wgpu::Device, capacity: usize) -> wgpu::Buffer {
    device.create_buffer(&wgpu::BufferDescriptor {
        label: Some("chassis chrome instances"),
        size: (capacity * std::mem::size_of::<Instance>()) as u64,
        usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        mapped_at_creation: false,
    })
}

/// The bind group, over every parameter buffer in binding order.
///
/// `glyphs` is the terminal's atlas where a frame carries badges. A frame
/// without them binds this mount's own atlas there instead: the binding has
/// to hold a texture whether or not a body reads one, and no body does.
fn make_bind(
    device: &wgpu::Device,
    layout: &wgpu::BindGroupLayout,
    uniforms: &wgpu::Buffer,
    records: &[&wgpu::Buffer; 7],
    atlas: &wgpu::Texture,
    sampler: &wgpu::Sampler,
    glyphs: Option<&wgpu::TextureView>,
) -> wgpu::BindGroup {
    let view = atlas.create_view(&wgpu::TextureViewDescriptor::default());
    let mut entries = vec![wgpu::BindGroupEntry {
        binding: 0,
        resource: uniforms.as_entire_binding(),
    }];
    for (i, buffer) in records.iter().enumerate() {
        entries.push(wgpu::BindGroupEntry {
            binding: 1 + i as u32,
            resource: buffer.as_entire_binding(),
        });
    }
    entries.push(wgpu::BindGroupEntry {
        binding: 8,
        resource: wgpu::BindingResource::TextureView(&view),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 9,
        resource: wgpu::BindingResource::Sampler(sampler),
    });
    entries.push(wgpu::BindGroupEntry {
        binding: 10,
        resource: wgpu::BindingResource::TextureView(glyphs.unwrap_or(&view)),
    });
    device.create_bind_group(&wgpu::BindGroupDescriptor {
        label: Some("chassis chrome"),
        layout,
        entries: &entries,
    })
}

/// The screen well in the logical pixels the casting measures its field in,
/// from the physical size the swapchain is in.
///
/// The bezel this casting continues reaches the same ruler by dividing its
/// `OutputSize` by `windowScaling * device_pixel_ratio`
/// (`crt::params::Params::build`); the chrome has no window scaling to undo,
/// because it draws straight onto the swapchain and is never scaled by a
/// preset, so the whole conversion is the ratio. Floored at one pixel: a
/// zero-sized well divides through the shader's field mapping.
fn well_ruler(well: (u32, u32), scale_factor: f64) -> (u32, u32) {
    let ratio = if scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    (
        ((well.0 as f64 / ratio).round() as u32).max(1),
        ((well.1 as f64 / ratio).round() as u32).max(1),
    )
}

/// A furniture piece's rectangle, from `chassis`'s logical pixels to the
/// target's physical ones.
///
/// Returns `(x, y, width, height)` with `x`/`y` signed, because a piece is
/// allowed to hang off the column and one routinely does: an LED window's
/// spill margin reaches a strip's height and a half past its own left edge,
/// which is well outside the bank on the annunciator. The scissor cuts it at
/// the bank's bounds; what this returns is the *whole* rectangle, so the part
/// that is on the column lands in the right place.
///
/// `None` for a piece with no area, or one entirely off the column.
pub(crate) fn scale_rect(
    rect: chassis::Rect,
    scale_factor: f64,
    column: (u32, u32),
) -> Option<(i32, i32, u32, u32)> {
    let ratio = if scale_factor > 0.0 {
        scale_factor
    } else {
        1.0
    };
    // The edges are rounded rather than the origin and size, so two rows a
    // whole pitch apart stay a whole pitch apart at any ratio.
    let x0 = (rect.x * ratio).round();
    let y0 = (rect.y * ratio).round();
    let x1 = ((rect.x + rect.width) * ratio).round();
    let y1 = ((rect.y + rect.height) * ratio).round();
    let (w, h) = ((x1 - x0) as i64, (y1 - y0) as i64);
    if w <= 0 || h <= 0 {
        return None;
    }
    if x1 <= 0.0 || y1 <= 0.0 || x0 >= f64::from(column.0) || y0 >= f64::from(column.1) {
        return None;
    }
    Some((x0 as i32, y0 as i32, w as u32, h as u32))
}

#[cfg(test)]
mod tests {
    use super::*;

    use std::time::{Duration, Instant};

    use config::Config;
    use crt::{DegaussState, Geometry, Pacing, Params};

    /// The buffer layouts the WGSL declares, which the shader reads by
    /// offset: a field added without a matching change there has to fail here
    /// rather than silently shifting one number into another.
    #[test]
    fn the_blocks_are_the_size_the_shader_declares() {
        assert_eq!(std::mem::size_of::<Uniforms>(), 16);
        assert_eq!(std::mem::size_of::<Instance>(), 32);
        assert_eq!(CHASSIS_RECORD_FLOATS * 4, 64);
        // A WGSL struct's array stride rounds up to its own alignment, which
        // for both of these is the 16 bytes their `vec4` fields demand.
        assert_eq!(
            std::mem::size_of::<VectorRecord>(),
            VECTOR_RECORD_FLOATS * 4
        );
        assert_eq!(std::mem::size_of::<VectorRecord>() % 16, 0);
        assert_eq!(std::mem::size_of::<StopRecord>(), STOP_RECORD_FLOATS * 4);
        assert_eq!(std::mem::size_of::<[f32; 2]>(), POINT_RECORD_FLOATS * 4);
    }

    /// The frame pass's own ruler, computed the way the shader computes it:
    /// `OutputSize / windowScaling`, where `OutputSize` is the render target
    /// (physical pixels) scaled by the preset's `windowScaling` and the
    /// uniform is whatever `Params::build` pushed under that name.
    fn frame_ruler(well_physical: (u32, u32), window_scaling: f64, dpr: f32) -> (f64, f64) {
        let mut cfg = Config::default();
        cfg.general.window_scaling = window_scaling;
        let geom = Geometry {
            output_width: well_physical.0 as f32 / dpr,
            output_height: well_physical.1 as f32 / dpr,
            device_pixel_ratio: dpr,
            ..Geometry::default()
        };
        let mut pacing = Pacing::new(Instant::now());
        let time = pacing.tick_by(Duration::from_millis(16));
        let params = Params::build(&cfg, &geom, time, DegaussState::IDLE);
        let divisor = f64::from(params.get("windowScaling").expect("the frame's ruler"));
        // The framebuffer: `scale_type = original`, `scale = windowScaling`.
        let output = (
            well_physical.0 as f64 * window_scaling,
            well_physical.1 as f64 * window_scaling,
        );
        (output.0 / divisor, output.1 / divisor)
    }

    #[test]
    fn the_well_the_casting_is_given_is_the_ruler_the_bezel_divides_to() {
        // The bezel and the bank casting are one field across the seam, so the
        // size the chrome hands `chassis_metal` has to be the size
        // `frame_metal` arrives at from the other side: at every ratio, and
        // whatever the window scaling does to the frame pass's framebuffer.
        for &(well, scaling, dpr) in &[
            ((840u32, 768u32), 1.0f64, 1.0f32),
            ((840, 768), 1.0, 2.0),
            ((840, 768), 2.0, 2.0),
            ((1680, 1536), 0.5, 2.0),
        ] {
            let (fx, fy) = frame_ruler(well, scaling, dpr);
            let (cx, cy) = well_ruler(well, f64::from(dpr));
            assert!(
                (fx - f64::from(cx)).abs() < 1.0 && (fy - f64::from(cy)).abs() < 1.0,
                "chrome hands over {cx}x{cy}, bezel measures {fx}x{fy} \
                 (well {well:?}, windowScaling {scaling}, dpr {dpr})"
            );
        }
    }

    #[test]
    fn the_ruler_halves_the_well_on_a_two_times_display() {
        assert_eq!(well_ruler((840, 768), 1.0), (840, 768));
        assert_eq!(well_ruler((1680, 1536), 2.0), (840, 768));
        // ...and never hands the shader a zero to divide its field by.
        assert_eq!(well_ruler((1, 1), 4.0), (1, 1));
        assert_eq!(well_ruler((0, 0), 1.0), (1, 1));
        // A scale factor a window never reports is not a divide by zero.
        assert_eq!(well_ruler((840, 768), 0.0), (840, 768));
    }

    #[test]
    fn a_dpr_of_two_halves_the_frames_ruler_and_the_window_scaling_drops_out() {
        // The 1024x768 logical well of the default window, on a 2x display:
        // the shader must land on the logical size at both scalings.
        for scaling in [1.0, 2.0, 0.5] {
            let (x, y) = frame_ruler((2048, 1536), scaling, 2.0);
            assert!(
                (x - 1024.0).abs() < 1e-6 && (y - 768.0).abs() < 1e-6,
                "{x}x{y}"
            );
        }
        // At DPR 1 the same well is 1024x768 physical and lands in the same
        // place, which is what says the DPR is the only thing that changed.
        let (x, y) = frame_ruler((1024, 768), 1.0, 1.0);
        assert!((x - 1024.0).abs() < 1e-6 && (y - 768.0).abs() < 1e-6);
    }

    #[test]
    fn a_rectangle_scales_by_its_edges_and_a_piece_off_the_column_is_dropped() {
        let column = (200u32, 400u32);
        let r = chassis::Rect::new(10.0, 20.0, 30.0, 40.0);
        assert_eq!(scale_rect(r, 1.0, column), Some((10, 20, 30, 40)));
        assert_eq!(scale_rect(r, 2.0, column), Some((20, 40, 60, 80)));
        // A piece hanging off the left keeps its whole rectangle, negative
        // origin and all: the scissor is what cuts it.
        let spill = chassis::Rect::new(-15.0, 20.0, 30.0, 40.0);
        assert_eq!(scale_rect(spill, 1.0, column), Some((-15, 20, 30, 40)));
        // ...and one entirely off it is not drawn at all.
        assert_eq!(
            scale_rect(chassis::Rect::new(-40.0, 20.0, 30.0, 40.0), 1.0, column),
            None
        );
        assert_eq!(
            scale_rect(chassis::Rect::new(10.0, 20.0, 0.0, 40.0), 1.0, column),
            None
        );
    }

    /// An operation's box is cut at the piece it belongs to, which is what
    /// the raster it replaced was cut by.
    #[test]
    fn an_operation_reaching_past_its_piece_is_cut_at_the_pieces_bounds() {
        let dest = (100, 50, 40, 20);
        assert_eq!(
            op_rect(0.0, 0.0, 40.0, 20.0, dest),
            Some([100.0, 50.0, 40.0, 20.0])
        );
        // Past the right and bottom edges, and past the left and top.
        assert_eq!(
            op_rect(-8.0, -3.0, 60.0, 90.0, dest),
            Some([100.0, 50.0, 40.0, 20.0])
        );
        // Whole pixels, outward: a box from 2.4 to 5.1 covers pixels 2..6.
        assert_eq!(
            op_rect(2.4, 1.2, 5.1, 4.9, dest),
            Some([102.0, 51.0, 4.0, 4.0])
        );
        // Entirely off the piece.
        assert_eq!(op_rect(-30.0, 0.0, -10.0, 20.0, dest), None);
    }
}
