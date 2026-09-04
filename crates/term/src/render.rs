//! The grid renderer: a screen of cells plus a thresholded atlas, drawn in one
//! pass with every coordinate an integer before it reaches the GPU.
//!
//! Three things are load bearing and none of them is negotiable later:
//!
//! * **Layout happens once, in unscaled raster pixels.** The instance buffer
//!   holds cell-grid geometry at 1x; the magnification is a uniform. Scaling
//!   therefore cannot perturb the layout, because the layout has already
//!   happened, and a DPR change costs one uniform write rather than a rebuild
//!   of the atlas. The counterexample makes the property concrete:
//!   re-rasterising Terminess at twice the size instead of scaling its
//!   geometry moved 3960 pixels.
//! * **The instance array is a fixed grid**, four blocks of `cols * rows` plus
//!   a two-instance cursor tail and a row's worth of input-method composition
//!   behind it. A damaged line is four contiguous ranges, so rio-vt's per-line
//!   damage translates into `write_buffer` calls that touch only the lines that
//!   changed instead of re-uploading the screen.
//! * **Blank cells still occupy their slot**, as a degenerate zero-area quad.
//!   Keeping the stride fixed is what makes the previous point arithmetic
//!   rather than bookkeeping.

use bytemuck::{Pod, Zeroable};
use wgpu::util::DeviceExt as _;

use crate::atlas::{FontContext, GlyphAtlas, Role};
use crate::cells::{Cell, CellGrid, CursorShape, CursorState};
use crate::color::{Rgba, Scheme};
use gpu::{Gpu, Image, Target, TARGET_FORMAT};
use crate::selection::MarkedRange;

/// One quad. All integers: the CPU decides the exact pixels, the GPU only
/// fills them in.
#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable, PartialEq)]
pub struct Instance {
    /// Top-left in unscaled raster pixels, relative to the grid origin.
    dst: [i32; 2],
    /// Size in unscaled raster pixels. Zero means "draw nothing".
    size: [i32; 2],
    /// Top-left of the glyph bitmap in the atlas, in texels. A negative x
    /// marks a solid fill (background, underline, cursor block).
    src: [i32; 2],
    color: [f32; 4],
}

const SOLID: [i32; 2] = [-1, -1];

const EMPTY: Instance = Instance {
    dst: [0, 0],
    size: [0, 0],
    src: SOLID,
    color: [0.0, 0.0, 0.0, 0.0],
};

/// Instance blocks, in draw order. Backgrounds first so glyphs land on top of
/// them, decorations over the glyphs, cursor last.
const BLOCK_BG: usize = 0;
const BLOCK_GLYPH: usize = 1;
const BLOCK_UNDERLINE: usize = 2;
const BLOCK_STRIKEOUT: usize = 3;
const BLOCKS: usize = 4;
/// The cursor block and the glyph redrawn on top of it.
const CURSOR_INSTANCES: usize = 2;
/// What one cell of an input method's composition costs: the filled plate under
/// it and the character struck into that plate. See [`GridRenderer::set_preedit`].
const PREEDIT_BLOCKS: usize = 2;

/// The rows the renderer lays out for a screen of `rows`: the screen and one
/// spare row under it. The spare row is what a scrollback position between
/// two lines shows at the bottom edge: the picture is drawn shifted up by a
/// fraction of a row (`set_origin`), the top row loses that fraction off the
/// top, and the spare row fills the same fraction at the bottom. It is
/// filled only while the view is scrolled back at all (`vt::sync`), and
/// blank otherwise.
fn render_rows(rows: usize) -> usize {
    rows + 1
}

/// The whole instance array: the four grid blocks over the rows laid out
/// (the screen plus the spare row), the cursor's tail, and room for a
/// composition as wide as the screen.
///
/// The pre-edit's room is fixed rather than grown per composition for the same
/// reason every other block is: a fixed stride is what makes a partial upload
/// arithmetic. A composition longer than the row is clipped at the row's end,
/// matching how pre-edit text is bounded to the row it starts on.
fn instance_count(cols: usize, rows: usize) -> usize {
    BLOCKS * cols * render_rows(rows) + CURSOR_INSTANCES + PREEDIT_BLOCKS * cols
}

#[repr(C)]
#[derive(Clone, Copy, Debug, Pod, Zeroable)]
struct Uniforms {
    viewport: [f32; 2],
    scale: i32,
    origin_x: i32,
    origin_y: i32,
    _pad: [i32; 3],
}

/// What one `sync` did, so a caller (and a test) can see that damage tracking
/// is actually tracking damage rather than quietly redrawing everything.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct SyncStats {
    /// Every row was rebuilt: a resize, a scroll, or rio-vt reporting full
    /// damage.
    pub full: bool,
    /// Rows whose instances were rewritten.
    pub rows_updated: usize,
    /// Rows rewritten only because the cursor entered or left them.
    pub cursor_rows: usize,
    /// Rows rewritten only because the selection entered or left them.
    pub marked_rows: usize,
    /// Rows rewritten only because the link under the pointer entered or
    /// left them.
    pub link_rows: usize,
}

/// A run of cells the renderer paints specially, as it is told it once a
/// frame: what the pointer has marked, and the link the pointer rests on.
///
/// The renderer is handed the range and never the model that produced it:
/// the drag anchor, the word mode and the swap detection are the window
/// layer's business, and a renderer holding them would be a second place to
/// ask what is selected. It is also what keeps the two selection models
/// ([`crate::selection`]) out of the painting path: they agree on a
/// [`MarkedRange`] and disagree about everything upstream of it.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Marked {
    /// The marked cells, in absolute lines.
    pub range: MarkedRange,
    /// The absolute index of the line the top row of the screen is showing.
    /// It is what turns a screen row into the absolute line
    /// [`MarkedRange::contains`] asks about, and it is the caller's because
    /// the renderer has no idea how far back the view is scrolled.
    pub top_line: usize,
}

/// Is this row and column of the screen inside the marked range?
///
/// A free function rather than a method because the row-invalidation below
/// asks it of the *old* marking and the new one in the same breath.
/// Whether a row has something in it to line up with the rows around it.
///
/// The characters that only mean anything by their column: the box-drawing
/// block, a bar, a rule of three or more dashes or double dashes, and a run
/// of five or more spaces, which is also what a tab has become by the time
/// it reaches the grid. The row's trailing blanks are not a run: every row
/// is padded to the grid's width with them.
///
/// A heuristic, and it errs both ways. A table row with none of these is
/// set in prose and drifts from its rules; a sentence with a bar in it is
/// set in the configured face. Both are cosmetic and both are visible.
pub fn structured(cells: &[Cell]) -> bool {
    let end = cells
        .iter()
        .rposition(|cell| cell.c != ' ' && cell.c != '\0')
        .map_or(0, |i| i + 1);
    let (mut spaces, mut rule, mut prev) = (0, 0, '\0');
    for cell in &cells[..end] {
        let c = cell.c;
        if c == '|' || ('\u{2500}'..='\u{257F}').contains(&c) {
            return true;
        }
        spaces = if c == ' ' { spaces + 1 } else { 0 };
        rule = if (c == '-' || c == '=') && c == prev {
            rule + 1
        } else {
            1
        };
        if spaces >= 5 || (rule >= 3 && (c == '-' || c == '=')) {
            return true;
        }
        prev = c;
    }
    false
}

fn marked_at(marked: Option<&Marked>, row: usize, col: usize) -> bool {
    marked.is_some_and(|m| m.range.contains(col, m.top_line + row))
}

/// Whether a row is marked differently than it was: the damage a change of
/// selection makes.
fn marking_differs(
    before: Option<&Marked>,
    after: Option<&Marked>,
    row: usize,
    cols: usize,
) -> bool {
    (0..cols).any(|col| marked_at(before, row, col) != marked_at(after, row, col))
}

/// Inverse video for one marked cell.
///
/// A phosphor terminal had no alpha to tint a highlight with: it swapped the
/// glyph and the plate, and so does this. Two things the plain swap does not
/// survive, both of which the cursor already answers on its own cell:
///
/// * a transparent plate is no plate at all, so a swapped-in transparent
///   colour falls back to the scheme's background made opaque -- the same
///   rule `cells::vt::cell_from_square` applies to SGR 7 and
///   `render::vt::cursor_state` applies to the character under the block;
/// * a cell whose glyph and plate are already the same colour, which a
///   program gets by naming one colour for both, comes out of the swap still
///   the same colour and would be marked invisibly. There is no second
///   colour to reach for, so the glyph goes to the dim end of the one there
///   is.
fn inverted(cell: Cell, scheme: &Scheme) -> Cell {
    let opaque = |mut color: Rgba| {
        if color[3] == 0.0 {
            color = scheme.background;
            color[3] = 1.0;
        }
        color
    };
    let mut fg = opaque(cell.bg);
    let bg = opaque(cell.fg);
    if fg[..3] == bg[..3] {
        fg = crate::color::dim(fg, scheme.dim_factor);
    }
    Cell {
        fg,
        bg,
        // The plate would swallow a line drawn in the colour it is painted
        // in, so the decorations follow the glyph rather than keeping the
        // colour SGR 58 gave them.
        line_color: fg,
        ..cell
    }
}

pub struct GridRenderer {
    pipeline: wgpu::RenderPipeline,
    bind_group_layout: wgpu::BindGroupLayout,
    bind_group: wgpu::BindGroup,
    uniform_buffer: wgpu::Buffer,
    instance_buffer: wgpu::Buffer,

    atlas: GlyphAtlas,
    /// The atlas generation `bind_group` was built over. An appended glyph
    /// that pushes the atlas past its allocated height puts a new texture
    /// behind the atlas's view, and the binding held here is then a binding to
    /// the allocation that was thrown away.
    bound_generation: u64,
    scheme: Scheme,

    cols: usize,
    /// The terminal's rows. The grid, the instances and the strides run one
    /// row taller: see [`render_rows`].
    rows: usize,
    /// The cells as last uploaded, the spare row included. Kept so a partial
    /// update can be built without asking the terminal for rows it did not
    /// damage.
    grid: CellGrid,
    instances: Vec<Instance>,
    cursor: Option<CursorState>,
    /// What the pointer has marked, as of the last [`vt::sync`]. Painted into
    /// the cells themselves (see [`Self::build_row`]) rather than over the
    /// finished picture, so the highlight bends with the curvature and glows
    /// with the phosphor like everything else on the tube.
    marked: Option<Marked>,
    /// The link under the pointer, as of the last [`vt::sync`], underlined
    /// in the cells the way the marking is inverted in them. Nothing of it
    /// reaches the grid: a copy of the screen carries no underline the
    /// session did not send.
    link: Option<Marked>,
    /// The critter standing over the screen, as of the last
    /// [`GridRenderer::set_critter`]: `(row, column, character)`, row-major.
    /// Substituted into the cell on the way past like the marking above, and
    /// for the same reason.
    critter: Vec<(usize, usize, char)>,
    /// The cells held here belong to a screen that is no longer the one on
    /// the glass, so the next [`vt::sync`] rebuilds every row rather than
    /// the damaged ones. Nothing the terminal reports can stand in for
    /// this: per-line damage is a statement about one screen's own lines,
    /// and a screen that has been swapped for another damaged nothing.
    stale: bool,
    /// Whether the program below has switched to the alternate screen, which
    /// sets every row in the configured face: a full-screen program lays its
    /// picture out by column, whether or not any one row says so.
    alt_screen: bool,
    /// What an input method is composing right now, drawn at the cursor and
    /// belonging to no cell of the grid. See [`GridRenderer::set_preedit`].
    preedit: String,
    scale: u32,
    origin: [i32; 2],
    /// How far up the picture is drawn from `origin`, in unscaled raster
    /// pixels: the fraction of a row a scrollback position sits between two
    /// lines. See [`Self::set_origin`].
    shift: i32,
}

impl GridRenderer {
    pub fn new(
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        atlas: GlyphAtlas,
        cols: usize,
        rows: usize,
        scheme: Scheme,
    ) -> Self {
        let shader = device.create_shader_module(wgpu::ShaderModuleDescriptor {
            label: Some("robco grid renderer"),
            source: wgpu::ShaderSource::Wgsl(include_str!("shader.wgsl").into()),
        });

        let bind_group_layout = device.create_bind_group_layout(&wgpu::BindGroupLayoutDescriptor {
            label: Some("robco grid bindings"),
            entries: &[
                wgpu::BindGroupLayoutEntry {
                    binding: 0,
                    visibility: wgpu::ShaderStages::VERTEX_FRAGMENT,
                    ty: wgpu::BindingType::Buffer {
                        ty: wgpu::BufferBindingType::Uniform,
                        has_dynamic_offset: false,
                        min_binding_size: None,
                    },
                    count: None,
                },
                wgpu::BindGroupLayoutEntry {
                    binding: 1,
                    visibility: wgpu::ShaderStages::FRAGMENT,
                    ty: wgpu::BindingType::Texture {
                        // Unfilterable, so the type system agrees with the
                        // shader: there is no sampler and nothing to filter
                        // with.
                        sample_type: wgpu::TextureSampleType::Float { filterable: false },
                        view_dimension: wgpu::TextureViewDimension::D2,
                        multisampled: false,
                    },
                    count: None,
                },
            ],
        });

        let layout = device.create_pipeline_layout(&wgpu::PipelineLayoutDescriptor {
            label: Some("robco grid layout"),
            bind_group_layouts: &[Some(&bind_group_layout)],
            immediate_size: 0,
        });

        let pipeline = device.create_render_pipeline(&wgpu::RenderPipelineDescriptor {
            label: Some("robco grid pipeline"),
            layout: Some(&layout),
            vertex: wgpu::VertexState {
                module: &shader,
                entry_point: Some("vs"),
                compilation_options: Default::default(),
                buffers: &[Some(wgpu::VertexBufferLayout {
                    array_stride: std::mem::size_of::<Instance>() as u64,
                    step_mode: wgpu::VertexStepMode::Instance,
                    attributes: &[
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Sint32x2,
                            offset: 0,
                            shader_location: 0,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Sint32x2,
                            offset: 8,
                            shader_location: 1,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Sint32x2,
                            offset: 16,
                            shader_location: 2,
                        },
                        wgpu::VertexAttribute {
                            format: wgpu::VertexFormat::Float32x4,
                            offset: 24,
                            shader_location: 3,
                        },
                    ],
                })],
            },
            fragment: Some(wgpu::FragmentState {
                module: &shader,
                entry_point: Some("fs"),
                compilation_options: Default::default(),
                targets: &[Some(wgpu::ColorTargetState {
                    format: TARGET_FORMAT,
                    // Premultiplied source over destination. With a binary
                    // atlas this is a choice between two exact outcomes, so
                    // overlapping quads cannot blend into a third; with a
                    // coverage atlas it is what makes an antialiased glyph
                    // composite over its cell background at all.
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

        let instances = vec![EMPTY; instance_count(cols, rows)];
        let instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("robco grid instances"),
            contents: bytemuck::cast_slice(&instances),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        let uniform_buffer = device.create_buffer(&wgpu::BufferDescriptor {
            label: Some("robco grid uniforms"),
            size: std::mem::size_of::<Uniforms>() as u64,
            usage: wgpu::BufferUsages::UNIFORM | wgpu::BufferUsages::COPY_DST,
            mapped_at_creation: false,
        });
        let bind_group = Self::make_bind_group(device, &bind_group_layout, &uniform_buffer, &atlas);

        let grid = CellGrid::new(cols, render_rows(rows), &scheme);
        let mut this = Self {
            pipeline,
            bind_group_layout,
            bind_group,
            uniform_buffer,
            instance_buffer,
            bound_generation: atlas.generation,
            atlas,
            scheme,
            cols,
            rows,
            grid,
            instances,
            cursor: None,
            marked: None,
            link: None,
            stale: false,
            alt_screen: false,
            critter: Vec::new(),
            preedit: String::new(),
            scale: 1,
            origin: [0, 0],
            shift: 0,
        };
        this.upload_all(queue);
        this
    }

    fn make_bind_group(
        device: &wgpu::Device,
        layout: &wgpu::BindGroupLayout,
        uniforms: &wgpu::Buffer,
        atlas: &GlyphAtlas,
    ) -> wgpu::BindGroup {
        device.create_bind_group(&wgpu::BindGroupDescriptor {
            label: Some("robco grid bind group"),
            layout,
            entries: &[
                wgpu::BindGroupEntry {
                    binding: 0,
                    resource: uniforms.as_entire_binding(),
                },
                wgpu::BindGroupEntry {
                    binding: 1,
                    resource: wgpu::BindingResource::TextureView(&atlas.view),
                },
            ],
        })
    }

    pub fn atlas(&self) -> &GlyphAtlas {
        &self.atlas
    }

    pub fn scheme(&self) -> &Scheme {
        &self.scheme
    }

    pub fn cols(&self) -> usize {
        self.cols
    }

    pub fn rows(&self) -> usize {
        self.rows
    }

    pub fn scale(&self) -> u32 {
        self.scale
    }

    /// The magnification, as an integer. Everything the sizing seam decides
    /// arrives here: `ResolvedFont::integer_scale`.
    ///
    /// Nothing is rebuilt. That is the whole point: a monitor change moves
    /// this number and the atlas, the layout and the instance buffer all stay
    /// exactly as they were.
    pub fn set_scale(&mut self, scale: u32) {
        assert!(scale >= 1, "integer scale must be at least 1");
        self.scale = scale;
    }

    /// Where the grid's rectangle sits in the target, in physical pixels, and
    /// how far up from there the picture is drawn: `shift` is the fraction of
    /// a row a scrollback position lies between two lines, in unscaled raster
    /// pixels (the caller rounds it; `round(shift * cell_height)`). The
    /// rectangle is what `draw` clips to, so the shifted top row and the
    /// spare row at the bottom stay inside it.
    ///
    /// Whole pixels only, both of them, and the shift a whole raster pixel
    /// so it is `scale` whole physical pixels: the glyph shader recovers its
    /// texel from the pixel's integer position, and a picture placed between
    /// two pixels would have no texel to recover.
    pub fn set_origin(&mut self, x: i32, y: i32, shift: i32) {
        self.origin = [x, y];
        self.shift = shift.max(0);
    }

    /// Swap in a rebuilt atlas: a font change, or a scalable face that really
    /// does have to be re-rasterised. A DPR change is *not* one of these.
    pub fn set_atlas(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, atlas: GlyphAtlas) {
        self.atlas = atlas;
        self.rebind(device);
        self.upload_all(queue);
    }

    /// Give the atlas a character it does not hold, and answer whether it can
    /// draw the character afterwards.
    ///
    /// The point of the pair being here rather than at the caller is the
    /// texture: appending can double the atlas's height, and a doubling
    /// replaces the texture the bind group was built over. So the generation
    /// is compared and the binding rebuilt on the spot, and the frame this
    /// character first appears in is the frame that draws it.
    ///
    /// `false` is a settled no: a space, or a codepoint nothing installed on
    /// the machine has a glyph for. Either way the cell draws empty and the
    /// atlas remembers, so asking again is a hash lookup.
    pub fn admit_glyph(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &mut FontContext,
        role: Role,
        c: char,
    ) -> bool {
        let drawn = self.atlas.glyph(device, queue, font, role, c).is_some();
        if self.atlas.generation != self.bound_generation {
            self.rebind(device);
        }
        drawn
    }

    /// Put every character a row carries into the atlas, so the instances
    /// built from that row have a slot to point at. A row passes through here
    /// before it is copied into the grid, because a glyph the atlas gained may
    /// have moved the texture, and the pipeline is rebound once for the row
    /// rather than once for each instance in it.
    ///
    /// Cheap on the ordinary path. [`GlyphAtlas::glyph`] answers from its slot
    /// table for a character the atlas holds, and from its blank set for one
    /// no loaded face covers, so a character reaches the rasteriser once over
    /// the life of an atlas rather than once a frame.
    pub fn admit_row(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &mut FontContext,
        cells: &[Cell],
    ) {
        // The row's own role, read off the cells it is about to be built
        // from, so the face the builder reaches for is the face these slots
        // were cut in.
        let role = self.role_of(cells);
        for cell in cells {
            self.admit_glyph(device, queue, font, role, cell.c);
        }
    }

    fn rebind(&mut self, device: &wgpu::Device) {
        self.bind_group = Self::make_bind_group(
            device,
            &self.bind_group_layout,
            &self.uniform_buffer,
            &self.atlas,
        );
        self.bound_generation = self.atlas.generation;
    }

    pub fn resize(&mut self, device: &wgpu::Device, queue: &wgpu::Queue, cols: usize, rows: usize) {
        if cols == self.cols && rows == self.rows {
            return;
        }
        self.cols = cols;
        self.rows = rows;
        self.grid.resize(cols, render_rows(rows), &self.scheme);
        self.instances = vec![EMPTY; instance_count(cols, rows)];
        self.instance_buffer = device.create_buffer_init(&wgpu::util::BufferInitDescriptor {
            label: Some("robco grid instances"),
            contents: bytemuck::cast_slice(&self.instances),
            usage: wgpu::BufferUsages::VERTEX | wgpu::BufferUsages::COPY_DST,
        });
        self.upload_all(queue);
    }

    /// Pixel size of the whole grid at the current scale.
    pub fn pixel_size(&self) -> (u32, u32) {
        (
            self.cols as u32 * self.atlas.cell.width * self.scale,
            self.rows as u32 * self.atlas.cell.height * self.scale,
        )
    }

    // ---- content ---------------------------------------------------------

    /// Replace the whole screen. The path a test, a full-damage frame, or a
    /// resize takes.
    pub fn set_grid(&mut self, queue: &wgpu::Queue, grid: &CellGrid, cursor: Option<CursorState>) {
        assert_eq!(grid.cols, self.cols);
        assert_eq!(grid.rows, self.rows);
        let screen = grid.cells.len();
        self.grid.cells[..screen].copy_from_slice(&grid.cells);
        self.blank_spare_row();
        self.cursor = cursor;
        self.upload_all(queue);
    }

    /// The rows laid out: the screen and the spare row.
    fn render_rows(&self) -> usize {
        render_rows(self.rows)
    }

    /// The spare row is a row of the grid with no cell of the screen behind
    /// it; nothing shows there unless `vt::sync` fills it.
    fn blank_spare_row(&mut self) {
        let spare = self.rows;
        let blank = Cell::blank(&self.scheme);
        self.grid.row_mut(spare).fill(blank);
    }

    /// Replace one row. The path a damaged line takes.
    pub fn set_row(&mut self, queue: &wgpu::Queue, row: usize, cells: &[Cell]) {
        let n = cells.len().min(self.cols);
        self.grid.row_mut(row)[..n].copy_from_slice(&cells[..n]);
        self.build_row(row);
        self.upload_row(queue, row);
    }

    pub fn set_cursor(&mut self, queue: &wgpu::Queue, cursor: Option<CursorState>) {
        if cursor == self.cursor {
            return;
        }
        self.cursor = cursor;
        self.build_cursor();
        self.upload_cursor(queue);
        // A composition stands at the cursor, so it goes where the cursor goes,
        // whichever path moved it (see [`Self::set_preedit`]).
        if !self.preedit.is_empty() {
            self.build_preedit();
            self.upload_preedit(queue);
        }
    }

    /// The screen under this renderer has been swapped for another one, so
    /// every row it holds is stale whatever the terminals on either side of
    /// the swap have reported as damaged.
    pub fn invalidate(&mut self) {
        self.stale = true;
    }

    /// Draw what an input method is composing, at the cursor.
    ///
    /// The composition's geometry is one row high, starting at the cursor
    /// cell, as many cells wide as the composition. What's painted there is
    /// the default background, then a cursor-style fill over the *whole*
    /// rectangle -- which for a block cursor with no explicit cursor colour
    /// fills it in the foreground and inverts the character colour -- and
    /// then the characters in that inverted colour. So a composition reads
    /// as a run of block cursor with the half-typed word struck into it, and
    /// that is what this draws: the plate in [`CursorState::color`], the
    /// glyphs in [`CursorState::text_color`].
    ///
    /// It is drawn **in the grid**, not over the finished picture, so the
    /// pre-edit goes through the CRT chain with everything else on the tube:
    /// it bends with the curvature and glows with the phosphor instead of
    /// floating flat above them.
    ///
    /// One cell per `char` under the fixed grid, the same ruler the grid
    /// itself uses; under [`crate::proportional`] it is each character's own
    /// advance, again the grid's own ruler. Clipped at the last column.
    /// Empty text takes the composition off the screen, which is what a commit
    /// and an abandoned composition both send.
    /// Tell the renderer whether the program is on its alternate screen. A
    /// change rebuilds every row, because every row's face may have changed
    /// with nothing in its cells to show for it.
    pub fn set_alternate_screen(&mut self, on: bool) {
        if self.alt_screen != on {
            self.alt_screen = on;
            self.stale = true;
        }
    }

    pub fn set_preedit(&mut self, queue: &wgpu::Queue, text: &str) {
        if self.preedit == text {
            return;
        }
        self.preedit.clear();
        self.preedit.push_str(text);
        self.build_preedit();
        self.upload_preedit(queue);
    }

    /// Stand a critter over the screen, or take it down with an empty slice.
    ///
    /// A critter is a few characters that are not the session's, standing on
    /// the screen for a second or two while something walks across it. It is
    /// drawn **in the grid** and never into it: the cells this renderer keeps
    /// stay a faithful copy of what the terminal wrote, so text scrolls
    /// behind a critter, a selection copied across one yields what the
    /// session sent, and the block cursor under one still shows the
    /// session's own character -- which is why a critter appears to pass
    /// behind the cursor rather than swallow it.
    ///
    /// Being in the grid is also what puts it through the CRT chain with
    /// everything else on the tube: it bends with the curvature and glows
    /// with the phosphor, where a figure composited over the finished picture
    /// (`app::chrome`'s badges) would sit flat on the glass in front of it.
    ///
    /// `cells` is row-major and holds a character per cell it wants. Rows are
    /// rebuilt only where the critter arrived or left, so a critter that has
    /// not moved since the last frame costs nothing at all, which is what the
    /// returned count of rewritten rows lets a test hold this to.
    pub fn set_critter(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &mut FontContext,
        cells: &[(usize, usize, char)],
    ) -> usize {
        if self.critter == cells {
            return 0;
        }
        for &(row, _, c) in cells {
            let role = if row < self.render_rows() {
                self.row_role(row)
            } else {
                Role::Mono
            };
            self.admit_glyph(device, queue, font, role, c);
        }
        // The union of where it was and where it is: the row it has just left
        // has to be built again from the grid, which still holds the
        // session's own characters for it.
        let mut rows: Vec<usize> = self
            .critter
            .iter()
            .chain(cells)
            .map(|(row, _, _)| *row)
            .filter(|row| *row < self.rows)
            .collect();
        rows.sort_unstable();
        rows.dedup();
        self.critter.clear();
        self.critter.extend_from_slice(cells);
        for row in &rows {
            self.build_row(*row);
            self.upload_row(queue, *row);
        }
        rows.len()
    }

    /// The half-open range of [`Self::critter`] that falls on this row. The
    /// list is row-major, so a row is one contiguous run of it.
    fn critter_row(&self, row: usize) -> (usize, usize) {
        let lo = self.critter.partition_point(|(r, _, _)| *r < row);
        let hi = lo + self.critter[lo..].partition_point(|(r, _, _)| *r == row);
        (lo, hi)
    }

    /// What [`Self::set_critter`] was last given.
    pub fn critter(&self) -> &[(usize, usize, char)] {
        &self.critter
    }

    /// What [`Self::set_preedit`] was last given.
    pub fn preedit(&self) -> &str {
        &self.preedit
    }

    pub fn grid(&self) -> &CellGrid {
        &self.grid
    }

    fn upload_all(&mut self, queue: &wgpu::Queue) {
        for row in 0..self.render_rows() {
            self.build_row(row);
        }
        self.build_cursor();
        self.build_preedit();
        queue.write_buffer(
            &self.instance_buffer,
            0,
            bytemuck::cast_slice(&self.instances),
        );
    }

    fn upload_row(&self, queue: &wgpu::Queue, row: usize) {
        let stride = std::mem::size_of::<Instance>() as u64;
        for block in 0..BLOCKS {
            let start = block * self.cols * self.render_rows() + row * self.cols;
            queue.write_buffer(
                &self.instance_buffer,
                start as u64 * stride,
                bytemuck::cast_slice(&self.instances[start..start + self.cols]),
            );
        }
    }

    fn upload_cursor(&self, queue: &wgpu::Queue) {
        let stride = std::mem::size_of::<Instance>() as u64;
        let start = BLOCKS * self.cols * self.render_rows();
        queue.write_buffer(
            &self.instance_buffer,
            start as u64 * stride,
            bytemuck::cast_slice(&self.instances[start..start + CURSOR_INSTANCES]),
        );
    }

    fn preedit_base(&self) -> usize {
        BLOCKS * self.cols * self.render_rows() + CURSOR_INSTANCES
    }

    fn upload_preedit(&self, queue: &wgpu::Queue) {
        let stride = std::mem::size_of::<Instance>() as u64;
        let start = self.preedit_base();
        let count = PREEDIT_BLOCKS * self.cols;
        queue.write_buffer(
            &self.instance_buffer,
            start as u64 * stride,
            bytemuck::cast_slice(&self.instances[start..start + count]),
        );
    }

    fn slot(&self, block: usize, row: usize, col: usize) -> usize {
        block * self.cols * self.render_rows() + row * self.cols + col
    }

    /// Which face a row of cells is set in.
    ///
    /// Mono when there is no prose face, on the alternate screen, and for a
    /// row with something to line up ([`structured`]). Prose otherwise. A
    /// function of the row's cells and nothing else, decided again at every
    /// rebuild: a row is set in the face its content earns at that moment,
    /// and no flag outlives the content that set it.
    fn role_of(&self, cells: &[Cell]) -> Role {
        if !self.atlas.prose || self.alt_screen || structured(cells) {
            Role::Mono
        } else {
            Role::Prose
        }
    }

    fn row_role(&self, row: usize) -> Role {
        self.role_of(&self.grid.cells[row * self.cols..(row + 1) * self.cols])
    }

    /// What `c` advances the pen by in a role, in unscaled raster pixels: the
    /// cell's width in the configured face, the character's own advance in
    /// the prose face.
    fn pitch(&self, role: Role, c: char) -> i32 {
        match role {
            Role::Mono => self.atlas.cell.width as i32,
            Role::Prose => self.atlas.advance_in(role, c) as i32,
        }
    }

    /// Where column `col` of `row` begins, in unscaled raster pixels: the sum
    /// of the pitches to its left, which for a mono row is the multiplication
    /// it replaces.
    ///
    /// It reads the grid's own characters, never the critter's. A figure
    /// walking the row borrows the cell's picture and leaves its measure
    /// alone, so the text under it does not slide sideways as it passes.
    fn pen_x(&self, row: usize, col: usize) -> i32 {
        match self.row_role(row) {
            Role::Mono => col as i32 * self.atlas.cell.width as i32,
            Role::Prose => (0..col.min(self.cols))
                .map(|c| self.pitch(Role::Prose, self.grid.cells[row * self.cols + c].c))
                .sum(),
        }
    }

    fn build_row(&mut self, row: usize) {
        let (bg_base, glyph_base, under_base, strike_base) = (
            self.slot(BLOCK_BG, row, 0),
            self.slot(BLOCK_GLYPH, row, 0),
            self.slot(BLOCK_UNDERLINE, row, 0),
            self.slot(BLOCK_STRIKEOUT, row, 0),
        );
        let cell_h = self.atlas.cell.height as i32;
        let baseline = self.atlas.cell.baseline;
        // The critter's cells for this row, as a run of the row-major list,
        // walked alongside the columns rather than searched per cell.
        let (mut at, end) = self.critter_row(row);
        let role = self.row_role(row);
        // The pen, walked along the row rather than multiplied into it. On a
        // mono row every step is the cell and the two are the same number.
        let mut x = 0i32;
        for col in 0..self.cols {
            let mut cell = self.grid.cells[row * self.cols + col];
            // Taken before the critter substitution below, so the measure is
            // the session's character and not the figure standing on it.
            let cell_w = self.pitch(role, cell.c);
            // The selection is drawn here, at the one point the cell's
            // background is already being decided, and as a change to the
            // cell rather than a quad laid over it: an overlay would ride on
            // top of the finished picture, outside the curvature and the
            // phosphor, and sit on the glass instead of behind it.
            if marked_at(self.marked.as_ref(), row, col) {
                cell = inverted(cell, &self.scheme);
            }
            // The link under the pointer is underlined the same way, and
            // after the marking, so a selected link keeps its swapped
            // colours: the line is drawn in whatever the glyph is drawn in.
            if marked_at(self.link.as_ref(), row, col) {
                cell.underline = true;
                cell.line_color = cell.fg;
            }
            // A critter stands on the cell, and after the marking rather than
            // before it: the highlight belongs to the screen, and a critter
            // walking through a selection is standing on a highlighted
            // screen. Only the character is taken. The colours stay the
            // cell's, so the figure is struck in the phosphor of whatever it
            // is walking over rather than carrying a palette of its own.
            while at < end && self.critter[at].1 < col {
                at += 1;
            }
            if at < end && self.critter[at].1 == col {
                cell.c = self.critter[at].2;
            }
            let y = row as i32 * cell_h;

            self.instances[bg_base + col] = if cell.bg[3] > 0.0 {
                Instance {
                    dst: [x, y],
                    size: [cell_w, cell_h],
                    src: SOLID,
                    color: cell.bg,
                }
            } else {
                EMPTY
            };

            self.instances[glyph_base + col] =
                glyph_instance(&self.atlas, role, cell.c, x, y + baseline, cell.fg);

            // An underline sits one pixel below the baseline, a strikeout at
            // a third of the ascent above it: unscaled raster pixels, so both
            // magnify with everything else instead of thinning out.
            self.instances[under_base + col] = if cell.underline {
                Instance {
                    dst: [x, y + (baseline + 1).min(cell_h - 1)],
                    size: [cell_w, 1],
                    src: SOLID,
                    color: cell.line_color,
                }
            } else {
                EMPTY
            };
            self.instances[strike_base + col] = if cell.strikeout {
                Instance {
                    dst: [x, y + (baseline - baseline / 3).max(0)],
                    size: [cell_w, 1],
                    src: SOLID,
                    color: cell.line_color,
                }
            } else {
                EMPTY
            };

            x += cell_w;
        }
    }

    fn build_cursor(&mut self) {
        let base = BLOCKS * self.cols * self.render_rows();
        self.instances[base] = EMPTY;
        self.instances[base + 1] = EMPTY;

        let Some(cursor) = self.cursor else { return };
        if cursor.shape == CursorShape::Hidden || cursor.row >= self.rows || cursor.col >= self.cols
        {
            return;
        }

        let cell_h = self.atlas.cell.height as i32;
        let baseline = self.atlas.cell.baseline;
        let role = self.row_role(cursor.row);
        let x = self.pen_x(cursor.row, cursor.col);
        let cell_w = self.pitch(role, self.grid.cells[cursor.row * self.cols + cursor.col].c);
        let y = cursor.row as i32 * cell_h;

        let (dst, size) = match cursor.shape {
            CursorShape::Block => ([x, y], [cell_w, cell_h]),
            // Two unscaled pixels, so the bar is still visible at 1x and grows
            // with the scale rather than staying hairline at 3x.
            CursorShape::Underline => ([x, y + cell_h - 2], [cell_w, 2]),
            CursorShape::Beam => ([x, y], [2, cell_h]),
            CursorShape::Hidden => return,
        };
        self.instances[base] = Instance {
            dst,
            size,
            src: SOLID,
            color: cursor.color,
        };
        // Only a block cursor covers the character; the other shapes leave it
        // legible and need no redraw.
        if cursor.shape == CursorShape::Block {
            let cell = self.grid.cells[cursor.row * self.cols + cursor.col];
            self.instances[base + 1] =
                glyph_instance(&self.atlas, role, cell.c, x, y + baseline, cursor.text_color);
        }
    }

    /// Lay the composition into its tail block. See [`Self::set_preedit`] for
    /// what it looks like and where the shape comes from.
    fn build_preedit(&mut self) {
        let base = self.preedit_base();
        for slot in base..base + PREEDIT_BLOCKS * self.cols {
            self.instances[slot] = EMPTY;
        }
        if self.preedit.is_empty() {
            return;
        }
        // Uses the cursor's *position* and not its shape: wherever the cursor
        // is, the composition paints its own block over that cell. A program
        // that hid its cursor mid-composition still shows the composition.
        let Some(cursor) = self.cursor else { return };
        if cursor.row >= self.rows || cursor.col >= self.cols {
            return;
        }

        let cell_h = self.atlas.cell.height as i32;
        let baseline = self.atlas.cell.baseline;
        let y = cursor.row as i32 * cell_h;
        // A block cursor with no cursor colour of its own fills the rectangle,
        // with the characters inverted over it; both colours are already on
        // the cursor state, which is where the grid's own cursor reads them
        // from.
        // The composition is not in the grid yet, so its own characters carry
        // the pen from the cursor's cell rather than the row's.
        let role = self.row_role(cursor.row);
        let mut x = self.pen_x(cursor.row, cursor.col);
        for (i, c) in self.preedit.chars().enumerate() {
            let col = cursor.col + i;
            if col >= self.cols {
                break;
            }
            let cell_w = self.pitch(role, c);
            self.instances[base + i * PREEDIT_BLOCKS] = Instance {
                dst: [x, y],
                size: [cell_w, cell_h],
                src: SOLID,
                color: cursor.color,
            };
            self.instances[base + i * PREEDIT_BLOCKS + 1] =
                glyph_instance(&self.atlas, role, c, x, y + baseline, cursor.text_color);
            x += cell_w;
        }
    }

    // ---- drawing ---------------------------------------------------------

    /// Record the grid into an existing pass target. `width`/`height` are the
    /// target's size in physical pixels.
    pub fn draw(
        &self,
        queue: &wgpu::Queue,
        encoder: &mut wgpu::CommandEncoder,
        view: &wgpu::TextureView,
        width: u32,
        height: u32,
        load: wgpu::LoadOp<wgpu::Color>,
    ) {
        // The picture is drawn `shift` raster pixels above the rectangle it
        // is clipped to. The shader's texel recovery does not mind where the
        // origin lands, below zero included: it reads `floor(pixel) -
        // origin` and the vertex placed the quad at `origin + dst * scale`,
        // so the two cancel and the division by `scale` stays a floor.
        let origin_y = self.origin[1] - self.shift * self.scale as i32;
        queue.write_buffer(
            &self.uniform_buffer,
            0,
            bytemuck::bytes_of(&Uniforms {
                viewport: [width as f32, height as f32],
                scale: self.scale as i32,
                origin_x: self.origin[0],
                origin_y,
                _pad: [0; 3],
            }),
        );

        let mut pass = encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
            label: Some("robco grid pass"),
            color_attachments: &[Some(wgpu::RenderPassColorAttachment {
                view,
                depth_slice: None,
                resolve_target: None,
                ops: wgpu::Operations {
                    load,
                    store: wgpu::StoreOp::Store,
                },
            })],
            depth_stencil_attachment: None,
            timestamp_writes: None,
            occlusion_query_set: None,
            multiview_mask: None,
        });
        // Clip to the grid's own rectangle, so the row shifted off its top
        // and the spare row under the last one end where the screen ends
        // rather than in the padding around it. The load op has already run
        // by now, so the padding is cleared whatever the rectangle. The
        // rectangle is pinned to the target: the centring padding is often
        // a pixel or none, and a rectangle starting above the target is not
        // one wgpu will take.
        let (grid_w, grid_h) = self.pixel_size();
        let x0 = self.origin[0].clamp(0, width as i32);
        let y0 = self.origin[1].clamp(0, height as i32);
        let x1 = (self.origin[0] + grid_w as i32).clamp(x0, width as i32);
        let y1 = (self.origin[1] + grid_h as i32).clamp(y0, height as i32);
        if x1 == x0 || y1 == y0 {
            return;
        }
        pass.set_scissor_rect(x0 as u32, y0 as u32, (x1 - x0) as u32, (y1 - y0) as u32);
        pass.set_pipeline(&self.pipeline);
        pass.set_bind_group(0, &self.bind_group, &[]);
        pass.set_vertex_buffer(0, self.instance_buffer.slice(..));
        pass.draw(0..6, 0..self.instances.len() as u32);
    }

    /// Draw the grid into a fresh offscreen texture and read it back. The
    /// measurement path: every pixel property in this crate is stated about
    /// bytes this returned.
    pub fn render_to_image(&self, gpu: &Gpu, clear: wgpu::Color) -> Image {
        let (width, height) = self.pixel_size();
        self.render_to_image_sized(gpu, clear, width, height)
    }

    /// The same, into a target of the caller's size: what the grid looks
    /// like centred in, or shifted within, a surface larger than itself.
    pub fn render_to_image_sized(
        &self,
        gpu: &Gpu,
        clear: wgpu::Color,
        width: u32,
        height: u32,
    ) -> Image {
        let target = Target::new(&gpu.device, width.max(1), height.max(1), TARGET_FORMAT);
        let mut encoder = gpu
            .device
            .create_command_encoder(&wgpu::CommandEncoderDescriptor { label: None });
        self.draw(
            &gpu.queue,
            &mut encoder,
            &target.view,
            target.width,
            target.height,
            wgpu::LoadOp::Clear(clear),
        );
        gpu.queue.submit(Some(encoder.finish()));
        target.read_rgba(&gpu.device, &gpu.queue)
    }
}

fn glyph_instance(
    atlas: &GlyphAtlas,
    role: Role,
    c: char,
    x: i32,
    baseline_y: i32,
    color: Rgba,
) -> Instance {
    match atlas.slot_in(role, c) {
        // The pen sits at the cell's left edge on the baseline; the glyph's
        // own bearing moves it from there. Integers throughout.
        Some(slot) if color[3] > 0.0 => Instance {
            dst: [x + slot.left, baseline_y - slot.top],
            size: [slot.width as i32, slot.height as i32],
            src: [slot.atlas_x as i32, slot.atlas_y as i32],
            color,
        },
        _ => EMPTY,
    }
}

/// Driving the renderer from a live rio-vt terminal.
pub mod vt {
    use super::*;
    use crate::cells::vt::fill_row;
    use crate::viewport::ScrollPosition;
    use rio_vt::ansi::CursorShape as VtCursorShape;
    use rio_vt::crosswords::grid::Dimensions;
    use rio_vt::crosswords::Crosswords;
    use rio_vt::crosswords::TermDamage;
    use rio_vt::event::EventListener;

    impl GridRenderer {
        /// Pull one frame's worth of change out of the terminal.
        ///
        /// The damage rules, in the order they override each other:
        ///
        /// 1. A geometry change (resize) rebuilds everything.
        /// 2. A viewport move rebuilds everything: rio-vt's per-line damage is
        ///    in viewport coordinates and says nothing about lines that
        ///    scrolled in from history.
        /// 3. `TermDamage::Full` rebuilds everything.
        /// 4. Otherwise only the damaged lines are rebuilt, plus the line the
        ///    cursor left and the line it arrived on, plus any line whose
        ///    cells have just been marked or unmarked.
        ///
        /// `marked` is what the pointer has selected, this frame, or `None`
        /// for a screen with no selection on it. It is a per-frame argument
        /// rather than state the renderer is given once because that is what
        /// it is: the window layer owns the gesture and hands down the range
        /// it has arrived at, and a frame that passes `None` takes the
        /// highlight off the glass with no separate call to remember.
        pub fn sync<L: EventListener>(
            &mut self,
            device: &wgpu::Device,
            queue: &wgpu::Queue,
            font: &mut FontContext,
            term: &mut Crosswords<L>,
            viewport: &mut ScrollPosition,
            marked: Option<&Marked>,
            link: Option<&Marked>,
        ) -> SyncStats {
            let mut stats = SyncStats::default();

            let (cols, rows) = (term.grid.columns(), term.grid.screen_lines());
            if cols != self.cols || rows != self.rows {
                self.resize(device, queue, cols, rows);
                stats.full = true;
            }
            if viewport.sync(term).moved {
                stats.full = true;
            }
            self.set_alternate_screen(
                term.mode()
                    .contains(rio_vt::crosswords::Mode::ALT_SCREEN),
            );
            if std::mem::take(&mut self.stale) {
                stats.full = true;
            }

            let mut damaged: Vec<usize> = Vec::new();
            match term.damage() {
                TermDamage::Full => stats.full = true,
                TermDamage::Partial(lines) => {
                    damaged.extend(lines.filter(|l| l.damaged).map(|l| l.line));
                }
            }
            term.reset_damage();

            let cursor = cursor_state(term, &self.scheme);
            let previous = self.cursor;

            // Which rows the selection moved across, asked while the old
            // marking is still here to compare against. A row the selection
            // has left needs rebuilding as much as one it has entered: the
            // highlight is painted into the cells, so the cells have to be
            // built again to come back.
            let marked_rows: Vec<usize> = if self.marked.as_ref() == marked {
                Vec::new()
            } else {
                (0..self.render_rows())
                    .filter(|&row| marking_differs(self.marked.as_ref(), marked, row, self.cols))
                    .collect()
            };
            self.marked = marked.cloned();

            // The link under the pointer moves the same way, and a row it
            // has left is rebuilt for the same reason.
            let link_rows: Vec<usize> = if self.link.as_ref() == link {
                Vec::new()
            } else {
                (0..self.render_rows())
                    .filter(|&row| marking_differs(self.link.as_ref(), link, row, self.cols))
                    .collect()
            };
            self.link = link.cloned();

            // The spare row under the screen shows the line after the last
            // one on screen, which exists only while the view is scrolled
            // back: `fill_row(rows)` is `Line(rows - display_offset)`, a
            // line of the screen for any offset of one or more, and past
            // its end at zero, where the row is blank instead. A position
            // between two lines always has an offset of one or more (the
            // ceiling rule in `crate::viewport`), so the row is filled
            // whenever it can show.
            let spare_shown = term.grid.display_offset() > 0;
            let mut scratch = vec![Cell::blank(&self.scheme); self.cols];
            if stats.full {
                for row in 0..self.rows {
                    fill_row(term, row, &self.scheme, &mut scratch);
                    self.admit_row(device, queue, font, &scratch);
                    self.grid.row_mut(row).copy_from_slice(&scratch);
                }
                self.fill_spare_row(term, spare_shown, &mut scratch);
                self.admit_row(device, queue, font, &scratch);
                self.cursor = cursor;
                self.upload_all(queue);
                stats.rows_updated = self.rows;
                return stats;
            }

            // The cursor's own line has to be refreshed on both sides of a
            // move: the block cursor paints over a cell, so the cell it left
            // has to come back.
            let mut rows_to_update: Vec<usize> = damaged;
            if cursor != previous {
                for state in [cursor, previous].into_iter().flatten() {
                    if state.row < self.rows && !rows_to_update.contains(&state.row) {
                        rows_to_update.push(state.row);
                        stats.cursor_rows += 1;
                    }
                }
            }
            for row in marked_rows {
                if !rows_to_update.contains(&row) {
                    rows_to_update.push(row);
                    stats.marked_rows += 1;
                }
            }
            for row in link_rows {
                if !rows_to_update.contains(&row) {
                    rows_to_update.push(row);
                    stats.link_rows += 1;
                }
            }
            rows_to_update.sort_unstable();
            rows_to_update.dedup();

            let limit = self.rows;
            for row in rows_to_update.iter().copied().filter(|r| *r < limit) {
                fill_row(term, row, &self.scheme, &mut scratch);
                self.admit_row(device, queue, font, &scratch);
                self.grid.row_mut(row).copy_from_slice(&scratch);
                self.build_row(row);
                self.upload_row(queue, row);
                stats.rows_updated += 1;
            }
            // The spare row shows a line of the live screen; a screen that
            // changed under a scrolled view may have changed that line.
            if spare_shown && !rows_to_update.is_empty() {
                self.fill_spare_row(term, true, &mut scratch);
                self.admit_row(device, queue, font, &scratch);
                self.build_row(self.rows);
                self.upload_row(queue, self.rows);
            }

            if cursor != previous {
                self.cursor = cursor;
                self.build_cursor();
                self.upload_cursor(queue);
                // The composition stands at the cursor, so a cursor that moved
                // moves it too, redrawn against the same new cursor position.
                if !self.preedit.is_empty() {
                    self.build_preedit();
                    self.upload_preedit(queue);
                }
            }

            stats
        }
    }

    impl GridRenderer {
        /// Fill the spare row from the terminal, or blank it, per `shown`.
        fn fill_spare_row<L: EventListener>(
            &mut self,
            term: &Crosswords<L>,
            shown: bool,
            scratch: &mut [Cell],
        ) {
            if shown {
                fill_row(term, self.rows, &self.scheme, scratch);
                self.grid.row_mut(self.rows).copy_from_slice(scratch);
            } else {
                self.blank_spare_row();
            }
        }
    }

    fn cursor_state<L: EventListener>(
        term: &Crosswords<L>,
        scheme: &Scheme,
    ) -> Option<CursorState> {
        let state = term.cursor();
        // A hidden cursor still *has* a position, and is carried as one rather
        // than dropped: `build_cursor` draws nothing for `Hidden`, but
        // `build_preedit` needs the position regardless of whether the cursor
        // itself is being painted.
        let shape = match state.content {
            VtCursorShape::Block => CursorShape::Block,
            VtCursorShape::Underline => CursorShape::Underline,
            VtCursorShape::Beam => CursorShape::Beam,
            VtCursorShape::Hidden => CursorShape::Hidden,
        };
        // The cursor's row on the *viewport*: its row on the live screen,
        // moved down by however far the view is scrolled back, the same
        // arithmetic `fill_row` runs the other way. Scrolled back past it,
        // the row is at or beyond the screen's last and nothing draws it.
        let row = state.pos.row.0 + term.grid.display_offset() as i32;
        if row < 0 {
            return None;
        }
        // The character under a block cursor is redrawn in the background
        // colour. A transparent background would make it vanish instead of
        // invert, so fall back to opaque black in that case.
        let mut text_color = scheme.background;
        if text_color[3] == 0.0 {
            text_color = [0.0, 0.0, 0.0, 1.0];
        }
        Some(CursorState {
            col: state.pos.col.0,
            row: row as usize,
            shape,
            color: scheme.cursor,
            text_color,
        })
    }
}
