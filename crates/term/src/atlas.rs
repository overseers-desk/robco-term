//! Glyph atlas: cosmic-text does the shaping and the rasterising, we own the
//! bytes between swash and the GPU. Upload takes a borrowed device/queue
//! rather than owning a `Gpu`.
//!
//! That ownership is the entire reason this module exists instead of a call to
//! glyphon. swash hands back an 8-bit coverage mask (`Format::Alpha`), which is
//! antialiasing by construction: a stem that lands half over a pixel comes back
//! as 128, not as on or off. cosmic-text's `CacheKeyFlags::PIXEL_FONT` does not
//! change this: it only rounds the subpixel offset before rasterising
//! (cosmic-text-0.19.0/src/swash.rs:48). For the low-resolution half of the
//! catalogue somebody has to threshold, and the only place it can happen
//! without leaving a trace is on the way into the atlas.
//!
//! Which half is being drawn is the whole of the mode question. One switch
//! decides it: the 16 pixel faces are drawn with antialiasing off and the 8
//! scalable ones with it on. So the
//! atlas has two modes ([`Rasterization`]), the flag picks between them
//! ([`Rasterization::for_face`]), and the texture is R8 coverage either way --
//! the binary mode is coverage that happens to hold only 0 and 255. The shader
//! reads that byte as alpha and never asks which mode produced it, so the
//! antialiasing-on path needed no second pipeline; see `shader.wgsl`.
//!
//! It is also why the rasterising is done here rather than through
//! `cosmic_text::SwashCache`. cosmic-text shapes; it does not get to choose the
//! picture. Its cache renders from `[ColorOutline, ColorBitmap, Outline]` with
//! no monochrome `Source::Bitmap` in the list, so a face with an embedded
//! bitmap strike at the ppem it was drawn at -- which is every low-resolution
//! entry in the catalogue -- comes back as its *outline* instead, and the
//! threshold above then eats the stems. [`crate::fonts::raster`] is the one
//! rule, shared with the LED raster.
//!
//! The atlas grows. A terminal's charset is whatever the far end sends, so
//! the printable-ASCII build is an opening balance rather than the whole
//! account: a character with no slot is shaped, rasterised and appended to
//! the shelf the packer left its pen on, and only the texels that changed go
//! to the GPU. The texture doubles in height when the pen runs off the
//! bottom, and that is the one moment the view behind a bind group is
//! replaced, which is what [`GlyphAtlas::generation`] is counted for.
//! Repacking the whole atlas per arriving character would cost a page of CJK
//! one full rebuild per glyph.
//!
//! Which face covers a character is cosmic-text's answer rather than ours,
//! and [`FontContext::covering_glyphs`] is the one place it is asked: give it
//! text and a pixel size and it says, per glyph, which face and which glyph
//! id in that face. A character the selected family has no glyph for falls to
//! the machine's own fonts, whose database is loaded on the first character
//! that needs it and not before, so a session that shows nothing but ASCII
//! never pays for the enumeration.
//!
//! Two further things are deliberate here:
//!
//! * Cache keys are built with a hard zero subpixel position, so a given
//!   character has exactly one bitmap regardless of where on the line it sits.
//!   Subpixel-positioned variants would defeat the whole exercise.
//! * Glyphs are placed on an integer cell grid computed from the face's own
//!   advance, not at the fractional pen positions cosmic-text would hand us.
//!   A terminal is a grid; pretending otherwise reintroduces the fractional
//!   offsets we just spent the previous point removing.

use std::collections::{HashMap, HashSet};

use cosmic_text::{
    fontdb, Attrs, Buffer, CacheKeyFlags, Family, FontSystem, Metrics, Shaping, SwashContent,
};
use swash::scale::{ScaleContext, Scaler};

use crate::fonts::raster;
use crate::fonts::sizing::ResolvedFont;
use crate::fonts::FontEntry;

/// The face a font that will not load is replaced by: the catalogue's first
/// bundled entry. Bundled, so it is `include_bytes!` of a file in this
/// repository and cannot itself be the thing that went missing.
const FALLBACK_FACE: &str = "TERMINESS_SCALED";

/// How the 8-bit coverage mask becomes atlas content.
///
/// The two arms are selected by one switch -- whether the face is
/// low-resolution -- and nothing else selects between them:
/// [`Rasterization::for_face`] is the whole rule.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Rasterization {
    /// Coverage >= threshold is ink, everything else is paper. The atlas holds
    /// only 0 and 255 afterwards, which is what "antialiasing off" has to mean
    /// if it is to mean anything checkable. This is the low-resolution half of
    /// the catalogue, and the pixel-exactness properties are stated about it.
    Binary { threshold: u8 },
    /// Coverage passed through untouched, to be read as alpha downstream: the
    /// antialiasing-on path, which is the scalable half of the catalogue.
    ///
    /// It is also, unchanged, the control the pixel properties were always
    /// measured against: it uses the identical pipeline, so any intermediate
    /// values in a render of a `Binary` atlas would have to have come from the
    /// pipeline rather than from the rasteriser. That the shipped scalable path
    /// and that control are the same code is the point -- the control is
    /// measuring the thing that ships.
    Coverage,
}

impl Rasterization {
    /// The rasterisation mode a resolved face is entitled to, and the one place
    /// that decision is made.
    ///
    /// [`ResolvedFont::antialias`] is `!entry.low_resolution`, computed in
    /// `fonts::sizing::resolve`. Reading it here rather than restating the
    /// flag keeps one answer to "may this face be antialiased?" in the tree:
    /// the sizing seam computes it, the atlas obeys it.
    pub fn for_face(resolved: &ResolvedFont) -> Self {
        if resolved.antialias {
            Rasterization::Coverage
        } else {
            Rasterization::Binary {
                threshold: crate::DEFAULT_THRESHOLD,
            }
        }
    }

    fn apply(self, v: u8) -> u8 {
        match self {
            Rasterization::Binary { threshold } => {
                if v >= threshold {
                    255
                } else {
                    0
                }
            }
            Rasterization::Coverage => v,
        }
    }
}

/// One glyph on its way into the atlas: the mask swash produced, thresholded,
/// with the placement it reported.
struct Raster {
    role: Role,
    c: char,
    w: u32,
    h: u32,
    left: i32,
    top: i32,
    data: Vec<u8>,
}

/// Rasterise one glyph out of a face's scaler and apply the mode.
///
/// `None` is three answers in one, and all three mean the same thing to the
/// packer, which is that the character earns no slot. swash may decline the
/// glyph outright. The glyph may have no ink: space and its relatives, which
/// is why an atlas over printable ASCII holds fewer entries than the charset
/// has characters. Or the face may draw it in colour.
///
/// The colour case is the one worth a word. A face that carries emoji as
/// embedded PNG or as a `COLR` table hands back `SwashContent::Color`, four
/// bytes a pixel, and there is no threshold and no coverage reading that turns
/// those four bytes into the single channel this atlas holds: both modes are
/// alpha and the shader reads one byte. So the glyph is skipped and the
/// character is named in the log, because a cell that draws empty is worth
/// less than a cell that draws empty for a stated reason.
fn rasterise(
    scaler: &mut Scaler,
    role: Role,
    c: char,
    glyph_id: u16,
    rasterization: Rasterization,
) -> Option<Raster> {
    let image = raster::glyph_mask(scaler, glyph_id)?;
    let (w, h) = (image.placement.width, image.placement.height);
    if w == 0 || h == 0 {
        return None;
    }
    if !matches!(image.content, SwashContent::Mask) {
        log::warn!(
            "glyph {c:?} rasterised as {:?} rather than an alpha mask; the \
             atlas is a single coverage channel in both modes, so a colour \
             face's glyph has nowhere to go and the cell draws empty",
            image.content
        );
        return None;
    }
    Some(Raster {
        role,
        c,
        w,
        h,
        left: image.placement.left,
        top: image.placement.top,
        data: image
            .data
            .iter()
            .map(|v| rasterization.apply(*v))
            .collect::<Vec<u8>>(),
    })
}

/// The atlas texture, at whatever height the packer has grown to need.
fn allocate(device: &wgpu::Device, width: u32, height: u32) -> wgpu::Texture {
    device.create_texture(&wgpu::TextureDescriptor {
        label: Some("glyph atlas"),
        size: wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
        mip_level_count: 1,
        sample_count: 1,
        dimension: wgpu::TextureDimension::D2,
        format: wgpu::TextureFormat::R8Unorm,
        usage: wgpu::TextureUsages::TEXTURE_BINDING | wgpu::TextureUsages::COPY_DST,
        view_formats: &[],
    })
}

/// Fill a whole texture from the atlas bytes. Both the callers are the two
/// moments a texture has nothing in it: the initial build, and a doubling.
/// A glyph appended into a texture that already holds the rest writes its own
/// rectangle instead, which is the only region that differs.
fn upload(queue: &wgpu::Queue, texture: &wgpu::Texture, width: u32, height: u32, pixels: &[u8]) {
    queue.write_texture(
        wgpu::TexelCopyTextureInfo {
            texture,
            mip_level: 0,
            origin: wgpu::Origin3d::ZERO,
            aspect: wgpu::TextureAspect::All,
        },
        pixels,
        wgpu::TexelCopyBufferLayout {
            offset: 0,
            bytes_per_row: Some(width),
            rows_per_image: Some(height),
        },
        wgpu::Extent3d {
            width,
            height,
            depth_or_array_layers: 1,
        },
    );
}

#[derive(Clone, Copy, Debug)]
pub struct GlyphSlot {
    /// Position in the atlas texture, in texels.
    pub atlas_x: u32,
    pub atlas_y: u32,
    /// Size of the bitmap, in texels.
    pub width: u32,
    pub height: u32,
    /// Offset from the pen position to the bitmap's top-left, in *unscaled*
    /// raster pixels. swash's `placement.left` / `placement.top`.
    pub left: i32,
    pub top: i32,
}

/// Which of the two faces a character is drawn from. Every row of the grid
/// is set in one of them, and the atlas keeps both under one texture, keyed
/// by role and character, so a row of either kind is one draw.
#[derive(Clone, Copy, Debug, PartialEq, Eq, Hash)]
pub enum Role {
    /// The configured face, the one the grid's cell is measured from. Every
    /// row is set in it when there is no prose face, and the rows with
    /// something to line up are set in it regardless.
    Mono,
    /// The prose face, each character at its own advance. Rows with nothing
    /// to line up are set in it, when one is configured.
    Prose,
}

/// A shaped advance as whole raster pixels, floored at one so no character
/// leaves the pen where it found it. The rounding is the same concession the
/// cell makes: there is no pixel-exact rendering of a 6.4-pixel step.
fn round_advance(w: f32) -> u32 {
    (w.round().max(1.0)) as u32
}

/// Integer cell metrics, in unscaled raster pixels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct CellMetrics {
    pub width: u32,
    pub height: u32,
    pub baseline: i32,
}

pub struct GlyphAtlas {
    /// Held only to keep the texture alive behind `view`.
    #[allow(dead_code)]
    pub texture: wgpu::Texture,
    pub view: wgpu::TextureView,
    pub width: u32,
    pub height: u32,
    pub cell: CellMetrics,
    pub rasterization: Rasterization,
    /// The size everything in here was rasterised at. Recorded so a DPR change
    /// can assert it did *not* move.
    pub raster_pixel_size: u32,
    /// How many times the texture behind [`Self::view`] has been replaced,
    /// which happens when a glyph is appended past the allocated height.
    /// Anything holding a bind group over this atlas records the number it
    /// bound at and compares, because a binding built before a doubling is a
    /// binding to an allocation this atlas has replaced, and the picture it
    /// draws is whatever that allocation happened to hold.
    pub generation: u64,
    slots: HashMap<(Role, char), GlyphSlot>,
    /// What each character advances the pen by, in unscaled raster pixels:
    /// the shaped advance of the face that covers it, which is the one
    /// measure a glyph's bitmap cannot supply (a space has an advance and no
    /// ink, and a comma has far more ink than advance). Filled beside the
    /// slot table, and for the blank characters too, so a row of spaces
    /// walks the same distance as a row of anything else.
    advances: HashMap<(Role, char), u32>,
    /// Whether a prose face was configured when this atlas was built, so a
    /// renderer holding only the atlas can tell whether a row may be set in
    /// one. The face itself is [`FontContext`]'s.
    pub prose: bool,
    /// Characters that have been through the rasteriser and earned no slot: a
    /// space, a face's blank glyph, a colour glyph, a codepoint nothing on the
    /// machine covers. Remembered because the answer is otherwise paid for
    /// again every time the row it sits in is rebuilt, and a screen of spaces
    /// would reshape a screen of spaces on every redraw.
    blank: HashSet<(Role, char)>,
    /// The atlas as bytes, kept so a glyph can be appended without the GPU
    /// being asked to read back what it already holds, and so a reallocation
    /// has something to re-upload.
    pixels: Vec<u8>,
    /// The shelf packer's pen: where the next glyph goes, and how tall the
    /// shelf it goes on has grown. Carried in the struct because packing does
    /// not finish when the initial charset runs out.
    pen_x: u32,
    pen_y: u32,
    shelf_h: u32,
    /// Every distinct byte value written into the atlas. The evidence for
    /// property 1 is taken here, before the GPU is involved at all.
    pub value_histogram: [u64; 256],
}

impl GlyphAtlas {
    /// The configured face's slot for `c`: what a caller outside the grid,
    /// such as a badge, draws its letters from.
    pub fn slot(&self, c: char) -> Option<&GlyphSlot> {
        self.slot_in(Role::Mono, c)
    }

    pub fn slot_in(&self, role: Role, c: char) -> Option<&GlyphSlot> {
        self.slots.get(&(role, c))
    }

    /// What the pen advances by after drawing `c` in `role`. The cell's own
    /// width for a character the atlas has never been asked for, which is the
    /// honest answer while nothing has measured it.
    pub fn advance_in(&self, role: Role, c: char) -> u32 {
        self.advances
            .get(&(role, c))
            .copied()
            .unwrap_or(self.cell.width)
            .max(1)
    }

    /// The slot a character draws from, rasterising and appending it when the
    /// atlas does not hold one yet.
    ///
    /// This is the whole of what a character outside the initial charset
    /// costs: one shaping, one rasterisation, one `write_texture` of the
    /// glyph's own rectangle, and a doubling of the texture on the rare
    /// occasion the shelf runs off the bottom. `None` is the settled answer
    /// that the character has no ink to draw, and is reached again from
    /// [`Self::blank`] without touching the font.
    pub fn glyph(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        font: &mut FontContext,
        role: Role,
        c: char,
    ) -> Option<GlyphSlot> {
        let key = (role, c);
        // Before the two early returns rather than after them: a character
        // whose answer is settled still owes the pen a distance, and a space
        // reaches the second of them.
        if !self.advances.contains_key(&key) {
            let w = font.advance(role, c, self.raster_pixel_size as f32);
            self.advances.insert(key, round_advance(w));
        }
        if let Some(slot) = self.slots.get(&key) {
            return Some(*slot);
        }
        if self.blank.contains(&key) {
            return None;
        }
        let raster = font.glyph_raster(role, c, self.raster_pixel_size as f32, self.rasterization);
        let placed = raster.and_then(|r| self.append(device, queue, r));
        if placed.is_none() {
            self.blank.insert(key);
        }
        placed
    }

    /// Put one rasterised glyph on the shelf and send it to the GPU.
    fn append(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        r: Raster,
    ) -> Option<GlyphSlot> {
        if r.w > self.width {
            // A glyph wider than the whole texture has no shelf to sit on.
            // Nothing in a terminal face at a terminal size comes close to
            // 512 texels, so this is a face doing something the catalogue
            // never anticipated rather than a case worth packing around.
            log::warn!(
                "glyph {:?} rasterised {} texels wide, which is wider than the \
                 {}-texel atlas; it draws as an empty cell",
                r.c,
                r.w,
                self.width
            );
            return None;
        }
        if self.pen_x + r.w > self.width {
            self.pen_x = 0;
            self.pen_y += self.shelf_h;
            self.shelf_h = 0;
        }
        let needed = self.pen_y + r.h;
        let grew = needed > self.height;
        if grew {
            let mut height = self.height.max(1);
            while height < needed {
                height *= 2;
            }
            self.reallocate(device, height);
        }

        let slot = GlyphSlot {
            atlas_x: self.pen_x,
            atlas_y: self.pen_y,
            width: r.w,
            height: r.h,
            left: r.left,
            top: r.top,
        };
        for row in 0..r.h {
            let src = (row * r.w) as usize;
            let dst = ((slot.atlas_y + row) * self.width + slot.atlas_x) as usize;
            self.pixels[dst..dst + r.w as usize].copy_from_slice(&r.data[src..src + r.w as usize]);
        }
        for v in &r.data {
            self.value_histogram[*v as usize] += 1;
        }
        // A grown texture is a fresh allocation and has to be filled; an
        // ungrown one differs from what the GPU holds by exactly this glyph's
        // rectangle, which is the only region worth the bus.
        if grew {
            upload(queue, &self.texture, self.width, self.height, &self.pixels);
        } else {
            queue.write_texture(
                wgpu::TexelCopyTextureInfo {
                    texture: &self.texture,
                    mip_level: 0,
                    origin: wgpu::Origin3d {
                        x: slot.atlas_x,
                        y: slot.atlas_y,
                        z: 0,
                    },
                    aspect: wgpu::TextureAspect::All,
                },
                &r.data,
                wgpu::TexelCopyBufferLayout {
                    offset: 0,
                    bytes_per_row: Some(r.w),
                    rows_per_image: Some(r.h),
                },
                wgpu::Extent3d {
                    width: r.w,
                    height: r.h,
                    depth_or_array_layers: 1,
                },
            );
        }

        self.pen_x += r.w;
        self.shelf_h = self.shelf_h.max(r.h);
        self.slots.insert((r.role, r.c), slot);
        Some(slot)
    }

    /// Take a taller texture and count the swap. The atlas bytes grow with it
    /// and go up to the GPU in the append that asked for the height.
    ///
    /// The old texture goes when the field is overwritten, and with it the
    /// view every bind group built over this atlas was made from. Hence
    /// [`Self::generation`]: a stale binding is not an error wgpu reports, it
    /// is a picture drawn from a dead allocation.
    fn reallocate(&mut self, device: &wgpu::Device, height: u32) {
        self.pixels.resize((self.width * height) as usize, 0);
        self.height = height;
        self.texture = allocate(device, self.width, height);
        self.view = self
            .texture
            .create_view(&wgpu::TextureViewDescriptor::default());
        self.generation += 1;
    }

    pub fn intermediate_value_count(&self) -> u64 {
        (1..255u8).map(|v| self.value_histogram[v as usize]).sum()
    }

    pub fn total_value_count(&self) -> u64 {
        self.value_histogram.iter().sum()
    }
}

/// Holds the shaping side of cosmic-text for one catalogue face, bundled or
/// system.
pub struct FontContext {
    pub font_system: FontSystem,
    /// swash's own scaler state. It is here rather than a `SwashCache` because
    /// this crate picks the rasterising sources itself; see the module doc and
    /// [`crate::fonts::raster`].
    scale_context: ScaleContext,
    pub font_id: fontdb::ID,
    pub family: String,
    /// The family the prose role shapes through, when one is configured.
    /// Checked against the database at construction, so a name the machine
    /// does not have is a logged refusal here and never a silent substitute
    /// at the shaper.
    prose_family: Option<String>,
    /// The catalogue name of the selected face, which is where its fallback
    /// chain is written down.
    name: &'static str,
    /// Whether the face's bundled fallbacks have been read into the database.
    bundled_fallbacks: bool,
    /// Whether the machine's own fonts have been read into the database. See
    /// [`Self::covering_glyphs`] for when that happens and why it waits.
    system_fonts: bool,
}

impl FontContext {
    pub fn new(spec: &FontEntry) -> Self {
        let mut db = fontdb::Database::new();
        db.load_font_data(spec.data().to_vec());
        // A bundled face cannot fail here -- it is `include_bytes!` of a file
        // in this repository. A system face can: `FontEntry::data` reads it
        // from the machine at selection time, and a font uninstalled since the
        // menu was built comes back empty.
        //
        // That is an ordinary thing for a machine to do, and both sides of the
        // seam already say so: `FontEntry::data`'s doc calls the empty slice
        // something "the atlas reports as a face it cannot build rather than
        // as a wrong picture", and `system::face_data`'s says "the caller
        // turns it into a fallback". Aborting was neither. So the bundled
        // default face is loaded instead and the substitution is named in the
        // log -- the user gets the wrong font, which they can see and fix,
        // rather than no terminal at all.
        if db.faces().next().is_none() {
            log::warn!(
                "the face {:?} produced no font ({} bytes read); falling back \
                 to {FALLBACK_FACE}. A system font removed since the catalogue \
                 was built does this.",
                spec.name,
                spec.data().len()
            );
            if let Some(fallback) =
                crate::fonts::font_by_name(FALLBACK_FACE, crate::fonts::FontSource::Bundled)
            {
                db.load_font_data(fallback.data().to_vec());
            }
        }
        let face = db
            .faces()
            .next()
            .unwrap_or_else(|| {
                // Not a machine's doing: the bundled fallback is compiled into
                // this binary, so reaching here means the build itself is
                // wrong and there is nothing to degrade to.
                panic!(
                    "the face {:?} produced no font and neither did the \
                     bundled {FALLBACK_FACE}",
                    spec.name
                )
            })
            .clone();
        let font_id = face.id;
        let family = face
            .families
            .first()
            .map(|(name, _)| name.clone())
            .unwrap_or_default();
        // The prose face is a family the machine has, so asking for one
        // reads the machine's fonts here, up front. The check is against the
        // database and not left to the shaper: cosmic-text answers an
        // unknown family with whatever face it likes, and a prose row set in
        // a face nobody named would be a wrong picture with no line in the
        // log to explain it.
        let mut system_fonts = false;
        let prose_family = crate::prose_family().and_then(|wanted| {
            db.load_system_fonts();
            system_fonts = true;
            let known = db
                .faces()
                .any(|f| f.families.iter().any(|(name, _)| name == wanted));
            if known {
                Some(wanted.to_string())
            } else {
                log::error!(
                    "the prose face {wanted:?} is not a family on this machine; \
                     every row is set in {family:?}"
                );
                None
            }
        });
        // Locale is fixed rather than read from the environment so two runs on
        // two machines shape the same text the same way.
        let font_system = FontSystem::new_with_locale_and_db("en-US".to_string(), db);
        Self {
            font_system,
            scale_context: ScaleContext::new(),
            font_id,
            family,
            prose_family,
            name: spec.name,
            bundled_fallbacks: false,
            system_fonts,
        }
    }

    /// The family the prose role is set in, when the machine has it.
    pub fn prose_family(&self) -> Option<&str> {
        self.prose_family.as_deref()
    }

    /// The family a role shapes through: the configured face, or the prose
    /// face where there is one. A prose row on a context with no prose face
    /// is set in the configured face, which is what the renderer asks for
    /// anyway once it has read [`GlyphAtlas::prose`].
    fn family_of(&self, role: Role) -> &str {
        match role {
            Role::Mono => &self.family,
            Role::Prose => self.prose_family.as_deref().unwrap_or(&self.family),
        }
    }

    /// Whether the machine's font database has been read. False is the
    /// evidence that a session has asked for nothing the selected face does
    /// not itself cover.
    pub fn system_fonts_loaded(&self) -> bool {
        self.system_fonts
    }

    /// Which face covers each glyph of `text` at this pixel size, which glyph
    /// id in that face draws it, and how wide it is. One entry per glyph, in
    /// order, deliberately flat: a terminal owns the cell assignment, and a
    /// character is not always a glyph. `e` followed by U+0301 shapes to one
    /// precomposed glyph rather than to a letter and a floating accent.
    ///
    /// This is the seam. It is the one question this crate asks a text stack,
    /// and cosmic-text is behind it only because something has to be. Swapping
    /// in another shaper is rewriting this body and nothing else, which is why
    /// there is no trait here: one implementor buys indirection and hides
    /// where the answer comes from.
    ///
    /// A glyph id of 0 is the face saying it has no glyph for that character,
    /// and it is what widens the database. Two widenings stand behind the
    /// selected face, taken in turn: the bundled faces its catalogue row names
    /// as covering its gaps, then the machine's own fonts. The bundled ones go
    /// first because they are chosen, compiled in and the same everywhere,
    /// where what a machine has installed is neither known nor identical
    /// between two of them.
    ///
    /// Both wait for a character that needs them. Doing either up front would
    /// put megabytes of parsing, and for the machine an enumeration and its
    /// file reads, in front of the first frame of every session, including
    /// every session that never leaves ASCII.
    pub fn covering_glyphs(&mut self, text: &str, pixel_size: f32) -> Vec<(fontdb::ID, u16, f32)> {
        self.covering_glyphs_in(Role::Mono, text, pixel_size)
    }

    /// [`Self::covering_glyphs`] for either role.
    pub fn covering_glyphs_in(
        &mut self,
        role: Role,
        text: &str,
        pixel_size: f32,
    ) -> Vec<(fontdb::ID, u16, f32)> {
        let mut shaped = self.shape(role, text, pixel_size);
        let uncovered = |s: &[(fontdb::ID, u16, f32)]| s.iter().any(|(_, id, _)| *id == 0);
        if !self.bundled_fallbacks && uncovered(&shaped) {
            self.bundled_fallbacks = true;
            for face in crate::fonts::fallback_faces(self.name) {
                self.font_system
                    .db_mut()
                    .load_font_data(face.data().to_vec());
                log::debug!(
                    "{:?} is not covered by {}; {} is loaded now",
                    text,
                    self.family,
                    face.name
                );
            }
            shaped = self.shape(role, text, pixel_size);
        }
        if self.system_fonts || !uncovered(&shaped) {
            return shaped;
        }
        self.system_fonts = true;
        self.font_system.db_mut().load_system_fonts();
        log::debug!(
            "{:?} is covered by no bundled face; the machine's fonts are loaded \
             now, {} faces of them",
            text,
            self.font_system.db().len()
        );
        self.shape(role, text, pixel_size)
    }

    fn shape(&mut self, role: Role, text: &str, pixel_size: f32) -> Vec<(fontdb::ID, u16, f32)> {
        let metrics = Metrics::new(pixel_size, pixel_size);
        let mut buffer = Buffer::new(&mut self.font_system, metrics);
        let family = self.family_of(role).to_string();
        let attrs = Attrs::new()
            .family(Family::Name(&family))
            // Tell cosmic-text this is a pixel face. On its own this only
            // rounds the subpixel offset, but it is the correct signal to send
            // and it keeps our cache keys honest.
            .cache_key_flags(CacheKeyFlags::PIXEL_FONT);
        buffer.set_text(text, &attrs, Shaping::Advanced, None);
        buffer.shape_until_scroll(&mut self.font_system, false);

        let mut out = Vec::new();
        for run in buffer.layout_runs() {
            for glyph in run.glyphs.iter() {
                out.push((glyph.font_id, glyph.glyph_id, glyph.w));
            }
        }
        out
    }

    /// Shape one character, rasterise it out of whichever face covers it, and
    /// apply the mode. The atlas's append path; [`Self::build_atlas`] is the
    /// same three steps taken in bulk, over a charset instead of a character.
    fn glyph_raster(
        &mut self,
        role: Role,
        c: char,
        px: f32,
        rasterization: Rasterization,
    ) -> Option<Raster> {
        let mut buf = [0u8; 4];
        let (face, glyph_id, _) = self
            .covering_glyphs_in(role, c.encode_utf8(&mut buf), px)
            .first()
            .copied()?;
        let font = self.font_system.get_font(face, fontdb::Weight::NORMAL)?;
        let mut scaler = self
            .scale_context
            .builder(font.as_swash())
            .size(px)
            .hint(true)
            .build();
        rasterise(&mut scaler, role, c, glyph_id, rasterization)
    }

    /// One character's advance at a given raster size in a role, shaped
    /// through whichever face covers it. `0.0` for a codepoint nothing covers.
    pub fn advance(&mut self, role: Role, c: char, px: f32) -> f32 {
        let mut buf = [0u8; 4];
        self.covering_glyphs_in(role, c.encode_utf8(&mut buf), px)
            .first()
            .map(|(_, _, w)| *w)
            .unwrap_or(0.0)
    }

    /// Cell metrics for a face at a given raster size. The advance comes from
    /// the face, then is rounded to an integer. A terminal cell that is 6.4
    /// pixels wide has no pixel-exact rendering to offer.
    pub fn cell_metrics(&mut self, resolved: &ResolvedFont) -> CellMetrics {
        let px = resolved.raster_pixel_size as f32;
        let advance = self
            .covering_glyphs("M", px)
            .first()
            .map(|(_, _, w)| *w)
            .unwrap_or(px * 0.5);
        // Cell width is the face's own advance, rounded, and nothing else.
        //
        // An aspect-ratio squeeze factor deliberately does not appear here.
        // That factor -- `0.5 * glyph_height / glyph_width` -- squeezes the
        // rendered terminal horizontally at display time. Folding it into the
        // advance packs cells closer than the glyphs are wide, which for Pet
        // Me at 8px means a 4-pixel cell holding an 8-pixel glyph, and
        // neighbouring characters overwrite each other. See
        // `ResolvedFont::font_width` for where that factor has to be applied
        // instead.
        let width = (advance.round() as i64).max(1) as u32;
        let height = (resolved.raster_pixel_size as i32 + resolved.line_spacing).max(1) as u32;
        // Baseline from the face's own ascent, snapped to a whole pixel.
        let baseline = self
            .font_system
            .get_font(self.font_id, fontdb::Weight::NORMAL)
            .map(|f| {
                let m = f.as_swash().metrics(&[]);
                let upem = if m.units_per_em == 0 {
                    1000.0
                } else {
                    m.units_per_em as f32
                };
                (m.ascent / upem * px).round() as i32
            })
            .unwrap_or((px * 0.8).round() as i32);
        CellMetrics {
            width,
            height,
            baseline,
        }
    }

    /// Rasterise every character in `charset` at the resolved size, threshold
    /// it, and upload one atlas.
    pub fn build_atlas(
        &mut self,
        device: &wgpu::Device,
        queue: &wgpu::Queue,
        resolved: &ResolvedFont,
        charset: &str,
        rasterization: Rasterization,
    ) -> GlyphAtlas {
        let px = resolved.raster_pixel_size as f32;
        let cell = self.cell_metrics(resolved);

        let mut chars: Vec<char> = charset.chars().collect();
        chars.sort_unstable();
        chars.dedup();

        // Shaping first, rasterising second, because they want the same `self`:
        // shaping needs the whole `FontSystem`, and a scaler holds a `FontRef`
        // borrowed out of one face for as long as it lives. That borrow is
        // also why the glyphs are grouped by the face that covers them and
        // rasterised a group at a time. One scaler can speak for one face, and
        // a charset wide enough to leave the selected family is answered by
        // several.
        let mut by_face: HashMap<fontdb::ID, Vec<(char, u16)>> = HashMap::new();
        let mut advances: HashMap<(Role, char), u32> = HashMap::new();
        for c in chars {
            let mut buf = [0u8; 4];
            if let Some((face, glyph_id, w)) = self
                .covering_glyphs(c.encode_utf8(&mut buf), px)
                .first()
                .copied()
            {
                by_face.entry(face).or_default().push((c, glyph_id));
                advances.insert((Role::Mono, c), round_advance(w));
            }
        }

        let mut rasters = Vec::new();
        for (face, wanted) in by_face {
            // `None` only if a face the shaper just named has gone from the
            // database, which nothing removes it from. An atlas short one
            // glyph is still a legal one, so this is an `if let` rather than a
            // panic.
            let Some(font) = self.font_system.get_font(face, fontdb::Weight::NORMAL) else {
                continue;
            };
            // `hint(true)` is what cosmic-text asked for and what the LED
            // raster asks for: it changes nothing on the strike path and is the
            // right answer on the outline one.
            let mut scaler = self
                .scale_context
                .builder(font.as_swash())
                .size(px)
                .hint(true)
                .build();
            for (c, glyph_id) in wanted {
                if let Some(r) = rasterise(&mut scaler, Role::Mono, c, glyph_id, rasterization) {
                    rasters.push(r);
                }
            }
        }
        // Back into codepoint order, which is the order the charset was sorted
        // into and the order the shelves are packed in. A face's glyphs
        // arriving together is a fact about the borrow above and must not
        // become a fact about the picture: the same charset packs the same way
        // whichever face covers what.
        rasters.sort_unstable_by_key(|r| r.c);

        // Shelf packing. Padding between glyphs would be pointless: nothing
        // ever filters across a glyph boundary, because nothing ever filters.
        let atlas_w: u32 = 512;
        let mut slots = HashMap::new();
        let (mut pen_x, mut pen_y, mut shelf_h) = (0u32, 0u32, 0u32);
        for r in &rasters {
            if pen_x + r.w > atlas_w {
                pen_x = 0;
                pen_y += shelf_h;
                shelf_h = 0;
            }
            slots.insert(
                (r.role, r.c),
                GlyphSlot {
                    atlas_x: pen_x,
                    atlas_y: pen_y,
                    width: r.w,
                    height: r.h,
                    left: r.left,
                    top: r.top,
                },
            );
            pen_x += r.w;
            shelf_h = shelf_h.max(r.h);
        }
        // The pen stops where the charset ran out rather than where the
        // texture ends, and the next character to arrive carries on from
        // there. The one texel of height an empty atlas is given keeps the
        // texture legal, and the first append doubles past it.
        let atlas_h = (pen_y + shelf_h).max(1);

        let mut pixels = vec![0u8; (atlas_w * atlas_h) as usize];
        for r in &rasters {
            let slot = slots[&(r.role, r.c)];
            for row in 0..r.h {
                let src = (row * r.w) as usize;
                let dst = ((slot.atlas_y + row) * atlas_w + slot.atlas_x) as usize;
                pixels[dst..dst + r.w as usize].copy_from_slice(&r.data[src..src + r.w as usize]);
            }
        }

        let mut value_histogram = [0u64; 256];
        for r in &rasters {
            for v in &r.data {
                value_histogram[*v as usize] += 1;
            }
        }

        let texture = allocate(device, atlas_w, atlas_h);
        upload(queue, &texture, atlas_w, atlas_h, &pixels);
        let view = texture.create_view(&wgpu::TextureViewDescriptor::default());

        GlyphAtlas {
            texture,
            view,
            width: atlas_w,
            height: atlas_h,
            cell,
            rasterization,
            raster_pixel_size: resolved.raster_pixel_size,
            generation: 0,
            slots,
            advances,
            prose: self.prose_family.is_some(),
            blank: HashSet::new(),
            pixels,
            pen_x,
            pen_y,
            shelf_h,
            value_histogram,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A face the machine has lost since the catalogue was built.
    ///
    /// `fonts/mod.rs` promises this degrades ("an unreadable file comes back
    /// empty, which the atlas reports as a face it cannot build rather than
    /// as a wrong picture"), and `fonts/system.rs` says the caller "turns it
    /// into a fallback". The atlas used to abort the process instead, so
    /// uninstalling a font that happened to be the selected one meant the
    /// terminal would no longer start at all -- and it could not be started
    /// to change the setting back.
    #[test]
    fn a_face_with_no_data_builds_the_context_on_the_bundled_fallback() {
        let entry = crate::fonts::missing_system_face(
            "GoneAway",
            std::path::PathBuf::from("/nonexistent/robco/gone-away.ttf"),
        );
        assert!(entry.data().is_empty(), "the test's premise: no bytes");

        // The assertion is first that this returns at all.
        let context = FontContext::new(&entry);

        // And that what it stood up is the bundled default, not an empty
        // shell that will produce blank glyphs forever.
        let fallback = crate::fonts::font_by_name(FALLBACK_FACE, crate::fonts::FontSource::Bundled)
            .expect("the bundled fallback");
        assert_eq!(
            context.family, fallback.family,
            "the fallback face is not the one the atlas fell back to"
        );
    }
}
