//! What stands on the casting: the shell's plate, and one channel display per
//! row.
//!
//! [`bank`](crate::bank) and [`layout`](crate::layout) divide the window and measure the
//! bank's footprint; this module is the next question, which is what goes
//! *inside* that footprint: one shell's plate and rows of `ChannelRow`s,
//! reduced to what a host with a device needs and nothing more: a rectangle,
//! the uniforms of the pass to run over it, and, for the two display kits,
//! the glyph raster that pass samples.
//!
//! It draws nothing and owns no device, like the rest of the crate. The mount
//! is `app::chrome`.
//!
//! # What is here
//!
//! The bank furniture is two kinds of thing, and only one of them is a
//! shader. The plates, rails and channel displays are drawn by the three
//! procedural metals and the two display passes. The numerals (two stacked
//! text items in Iosevka at 34 px), the window mouldings around each strip
//! (gradient rectangles), the pager, the selector carriage, the screw heads
//! and the tape kit's well chrome are vector painting instead.
//!
//! Both kinds are here now, and they meet at [`Piece`]: a shaded piece names a
//! pass and carries its uniforms, a painted one carries a
//! [`crate::paint::Painting`] and names [`Pass::Painted`]. The mount treats
//! them the same way (scratch, then premultiplied blit), which is what
//! keeps the composition order fixed across the join, and it is the reason
//! the two kinds are one list rather than two.
//!
//! The switchboard's lever is painted at rest; its throw, the swing from
//! rest to thrown with its easing overshoot, is not animated. The cap itself
//! is painted: drop shadow, front face, chamfer and lit top sliver over the
//! well's left, alongside the riveted plate, stamped numeral, well, glow and
//! bevel.
//!
//! # The channel seam
//!
//! A strip's content is channel state, and the channel state machines live in
//! `app::{channels,bank}`. [`crate::BankStrips`] is the seam between them:
//! the whole of what the furniture asks the channel model, built by
//! `app::bank::BankPager::strips` from the real slots and read here. This
//! module reads channel state nowhere else, and holds none.
//!
//! This module shipped its own `ChannelView`/`ChannelSlot` pair while the
//! channel state machine was still being built elsewhere; the merge
//! collapsed the two shapes into [`crate::BankStrips`], which is the wider
//! of the two (it carries the page,
//! the indicator and where the selector rides as well as the row contents) and
//! the one the real model already produces.

use crate::bank::BankGeometry;
use crate::cabinet::Display;
use crate::color::{self, Rgba};
use crate::displays::{led, raster, tape};
use crate::layout::Rect;
use crate::metrics::{LedMetrics, ShellMetrics, TapeMetrics};
use crate::params::{LedMetalParams, PlateMetalParams, TapeMetalParams};
use crate::strip::{BankStrips, StripRow};

use config::Config;

/// Which pass draws a piece. The first three bodies are
/// [`crate::shaders`]'s constants of the same names; the fourth is not a pass
/// at all.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Pass {
    /// `plate_metal.wgsl`.
    Plate,
    /// `led_matrix.wgsl`.
    LedMatrix,
    /// `tape_label.wgsl`.
    TapeLabel,
    /// Not a shader: a [`crate::paint::Painting`], rasterised by the mount at
    /// the window's own ratio and blitted like the rest. This is the vector
    /// half (the numerals, the window mouldings, the pager, the selector
    /// carriage, the screw heads and the tape kit's well chrome), and the
    /// piece carries its description in [`Piece::paint`] rather than uniforms
    /// in [`Piece::params`].
    Painted,
}

/// One shaded piece's uniforms, in the shape the pass that reads them wants.
///
/// The variants stand one to one with the three shader arms of [`Pass`], and
/// a piece's pass is struck from the variant it carries
/// ([`Piece::shaded`]), so the block a mount writes cannot name one body and
/// be filled for another.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum PieceParams {
    Plate(PlateMetalParams),
    Led(LedMetalParams),
    Tape(TapeMetalParams),
}

impl PieceParams {
    /// Which pass reads these uniforms.
    pub fn pass(&self) -> Pass {
        match self {
            PieceParams::Plate(_) => Pass::Plate,
            PieceParams::Led(_) => Pass::LedMatrix,
            PieceParams::Tape(_) => Pass::TapeLabel,
        }
    }
}

/// A glyph raster, widened to the RGBA8 a texture upload wants
/// ([`crate::displays::raster::to_rgba8`]'s convention: `[a, a, a, a]`).
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct Raster {
    pub width: u32,
    pub height: u32,
    /// Shared rather than owned: the bank hands the same pieces back frame
    /// after frame (`Cabinet::furniture` remembers them), and a consumer that
    /// holds one from an earlier frame can ask whether these are the very
    /// bytes it already has rather than compare them.
    pub rgba: std::sync::Arc<[u8]>,
}

/// One thing to draw on the casting.
#[derive(Clone, Debug, PartialEq)]
pub struct Piece {
    pub pass: Pass,
    /// Where it goes, in the bank column's own coordinates, logical pixels
    /// (see [`crate::cabinet`]'s module doc on which pixel). A host on a
    /// scaled display multiplies by the window's factor.
    pub rect: Rect,
    /// The pass's uniforms, or `None` for a [`Pass::Painted`] piece.
    pub params: Option<PieceParams>,
    /// The texture the pass samples as `Source`, for the two display passes.
    /// `None` for the plate, which is procedural and samples nothing.
    pub source: Option<Raster>,
    /// What to paint, for a [`Pass::Painted`] piece and nothing else.
    ///
    /// It is a description and not an image on purpose: the mount rasterises
    /// it at the window's ratio, so the antialiasing lands on the device grid
    /// (see [`crate::paint`]'s module doc), and it re-rasterises only when the
    /// description changes, which on a bank standing still is never.
    pub paint: Option<crate::paint::Painting>,
}

impl Piece {
    /// A piece drawn by one of the three procedural passes, the pass being
    /// the one the uniforms are for.
    pub fn shaded(rect: Rect, params: PieceParams, source: Option<Raster>) -> Self {
        Self {
            pass: params.pass(),
            rect,
            params: Some(params),
            source,
            paint: None,
        }
    }

    /// A piece of painted furniture, at its rectangle in the bank column's
    /// own coordinates.
    pub fn painted(rect: Rect, painting: crate::paint::Painting) -> Self {
        Self {
            pass: Pass::Painted,
            rect,
            params: None,
            source: None,
            paint: Some(painting),
        }
    }
}

/// The derived font colour, which is what the LED strip's lamps are struck
/// from, not the stored hex.
///
/// `crt::params`'s `screen_colors` is the same derivation for the glass, and
/// its doc carries the reasoning in full (the /256 parser, the `saturation`
/// mix, the `0.7 + contrast * 0.3` weight, and why pushing the stored string
/// instead leaves contrast moving nothing). This is that arithmetic's chrome
/// half, duplicated here for the same reason [`crate::color`] carries its
/// own copy of two colour-mixing functions the glass chain also needs: the
/// chassis does not import the glass chain.
pub fn font_color(cfg: &Config) -> Rgba {
    let saturated = color::mix(
        color::str_to_color(&cfg.screen.font_color),
        color::str_to_color("#FFFFFF"),
        cfg.screen.saturation_color as f32 * 0.5,
    );
    let background = color::str_to_color(&cfg.screen.background_color);
    color::mix(
        background,
        saturated,
        0.7 + cfg.screen.contrast as f32 * 0.3,
    )
}

/// `led_matrix.wgsl`'s parameters for one window.
///
/// `spill` is the margin in pixels the drawn rectangle stands proud of the
/// strip on each side; the shader wants it as a fraction of that grown
/// rectangle, which is why this takes the grown size too.
pub fn led_params(
    grid: (u32, u32),
    colors: led::Colors,
    glow: f32,
    spill_strength: f32,
    spill: (f64, f64),
    grown: (f64, f64),
) -> LedMetalParams {
    let frac = |margin: f64, size: f64| {
        if size > 0.0 {
            (margin / size) as f32
        } else {
            0.0
        }
    };
    let rgba = |c: Rgba| [c.r, c.g, c.b, c.a];
    LedMetalParams {
        grid_size: [grid.0 as f32, grid.1 as f32],
        spill_margin: [frac(spill.0, grown.0), frac(spill.1, grown.1)],
        spill_dead: [led::SPILL_DEAD.0, led::SPILL_DEAD.1],
        lit_color: rgba(colors.lit),
        dim_color: rgba(colors.dim),
        panel_color: rgba(colors.panel),
        dot_radius: led::DOT_RADIUS,
        threshold: led::THRESHOLD,
        glow,
        spill_strength,
    }
}

/// `tape_label.wgsl`'s parameters for one label of `size` pixels whose glyph
/// box is `glyph_rect`.
pub fn tape_params(size: (f64, f64), glyph_rect: (f32, f32, f32, f32)) -> TapeMetalParams {
    let tape_color = tape::tape_color();
    let letter_color = tape::letter_color();
    TapeMetalParams {
        size_px: [size.0 as f32, size.1 as f32],
        light_dir: [tape::DISPLAY_LIGHT_DIR.0, tape::DISPLAY_LIGHT_DIR.1],
        glyph_rect_px: [glyph_rect.0, glyph_rect.1, glyph_rect.2, glyph_rect.3],
        tape_color: [
            tape_color.r,
            tape_color.g,
            tape_color.b,
            tape_color.a,
        ],
        letter_color: [
            letter_color.r,
            letter_color.g,
            letter_color.b,
            letter_color.a,
        ],
        bevel_px: tape::BEVEL_PX,
        dilate_px: tape::DILATE_PX,
        sheen_amount: tape::SHEEN_AMOUNT,
        grain_amount: tape::GRAIN_AMOUNT,
        seed: tape::SEED,
    }
}

/// The lamp grid one strip's window samples, one texel per lamp.
///
/// The strip's own raster is `term::fonts::led::led_text_image`'s (the
/// proven glyph-raster path), laid into a `grid_w` x `grid_h` field of unlit
/// lamps at an offset: in by `left_pad_cells` (in *cells*, hence the
/// multiply by the cell width -- a bank strip's own window passes
/// [`led::LED_SIDE_PAD_CELLS`] as its left pad; a fixture with no side pad
/// of its own, the slide rule's page counter, passes zero) and down by
/// `top_pad_cells`. Empty text is a field of unlit lamps, which is exactly
/// what a dark slot shows.
pub fn led_grid(
    font_data: &[u8],
    pixel_size: u32,
    text: &str,
    lamp_cell_width: u32,
    grid: (u32, u32),
    left_pad_cells: u32,
    top_pad_cells: u32,
) -> Raster {
    let (grid_w, grid_h) = (grid.0.max(1), grid.1.max(1));
    let mut alpha = vec![0u8; (grid_w * grid_h) as usize];
    if !text.is_empty() {
        if let Some(r) = term::fonts::led::led_text_image(font_data, pixel_size, text) {
            let x0 = left_pad_cells * lamp_cell_width;
            let y0 = top_pad_cells;
            for y in 0..r.height.min(grid_h.saturating_sub(y0)) {
                for x in 0..r.width.min(grid_w.saturating_sub(x0)) {
                    alpha[((y + y0) * grid_w + (x + x0)) as usize] =
                        r.alpha[(y * r.width + x) as usize];
                }
            }
        }
    }
    let widened = raster::to_rgba8(&term::fonts::led::LedRaster {
        width: grid_w,
        height: grid_h,
        alpha,
    });
    Raster {
        width: grid_w,
        height: grid_h,
        rgba: widened.into(),
    }
}

/// Where one row's display window stands, in the bank column's own
/// coordinates.
///
/// The rows stack at the content ground, and the strip stands past the
/// numeral lane and its gap, vertically centred in the row.
///
/// Shared by the drawing and the hit test on purpose: the window *is* the
/// key -- pressing it reaches the channel, so the row carries no separate
/// button -- so the rectangle a press is tested against has to be the
/// rectangle that was drawn.
pub fn strip_rect(geometry: &BankGeometry, shell: &ShellMetrics, row: usize) -> Rect {
    let strip_x = (geometry.content_x + shell.numeral_width + shell.column_gap) as f64;
    let strip_w = geometry.strip_width as f64;
    let strip_h = geometry.strip_height as f64;
    let pitch = geometry.row_height as f64 + geometry.row_spacing;
    let strip_y = geometry.top_padding as f64
        + row as f64 * pitch
        + (geometry.row_height as f64 - strip_h) / 2.0;
    Rect::new(strip_x, strip_y, strip_w, strip_h)
}

/// Which row's window a press at `(x, y)` landed in, both in the bank column's
/// own coordinates, for a page of `rows` engraved keys.
///
/// `None` for a press on the bare plate between or beside the windows: the
/// row's press area is the display item and not the row.
pub fn strip_at(
    geometry: &BankGeometry,
    shell: &ShellMetrics,
    rows: usize,
    x: f64,
    y: f64,
) -> Option<usize> {
    (0..rows).find(|&row| within(strip_rect(geometry, shell, row), x, y))
}

/// Where the pager stands, in the bank column's own coordinates.
///
/// It fills the content lane at the bank's foot, its own natural height above
/// the shell's bottom padding. Measured here and not only drawn here for the
/// same reason [`strip_rect`] is: [`pager_at`] tests the rectangle that was
/// drawn. The selector parks at its middle.
pub fn pager_rect(geometry: &BankGeometry, shell: &ShellMetrics, bank: (f64, f64)) -> Rect {
    let content_width = geometry.content_width(bank.0 as i32);
    let pager_h = shell.pager_height(content_width) as f64;
    Rect::new(
        geometry.content_x as f64,
        bank.1 - geometry.bottom_padding as f64 - pager_h,
        content_width as f64,
        pager_h,
    )
}

/// Which way a press at `(x, y)` walks the pages, both in the bank column's
/// own coordinates: `-1` in the pager's left third, `1` in its right third.
///
/// Every shell puts PREV on the left, NEXT on the right and a counter or
/// plate between, so thirds catch every aimed press without per-shell
/// geometry; the middle stays dead so the counter is not a key.
pub fn pager_at(
    geometry: &BankGeometry,
    shell: &ShellMetrics,
    bank: (f64, f64),
    x: f64,
    y: f64,
) -> Option<i32> {
    let rect = pager_rect(geometry, shell, bank);
    if !within(rect, x, y) {
        return None;
    }
    match ((x - rect.x) / rect.width * 3.0) as i32 {
        0 => Some(-1),
        2 => Some(1),
        _ => None,
    }
}

/// A point inside a rectangle, top and left edges included and the far ones
/// not: one answer for both of this file's hit tests.
fn within(r: Rect, x: f64, y: f64) -> bool {
    x >= r.x && x < r.x + r.width && y >= r.y && y < r.y + r.height
}

/// The whole plan: the shell's plate, then one display per row.
///
/// `bank` is the column's rectangle, logical pixels; every returned rect is in
/// its coordinates with the origin at its top-left corner. The order is the
/// order to draw in: the plate goes down first and the rows stand on it.
pub fn bank_pieces(
    cfg: &Config,
    shell: &ShellMetrics,
    display: &Display,
    geometry: &BankGeometry,
    bank: (f64, f64),
    strips: &BankStrips,
) -> Vec<Piece> {
    let mut pieces = Vec::new();
    if bank.0 <= 0.0 || bank.1 <= 0.0 {
        return pieces;
    }

    if let Some((rect, params)) = crate::shells::plate_region(cfg.chassis.shell, bank) {
        if rect.width > 0.0 && rect.height > 0.0 {
            pieces.push(Piece::shaded(
                Rect::new(rect.x, rect.y, rect.width, rect.height),
                PieceParams::Plate(params),
                None,
            ));
        }
    }

    // The chassis's screws are drawn after its plate, so they sit on the
    // plate and under everything the bank draws.
    pieces.extend(crate::shells::screws(cfg.chassis.shell, bank));

    if geometry.strip_width <= 0 || geometry.strip_height <= 0 {
        return pieces;
    }

    let plastic = plastic(cfg);

    let pager_rect = pager_rect(geometry, shell, bank);

    // The selector sits at the track's lane, inset by the bank's padding
    // top and bottom, and it is drawn before the rows: the carriage stands
    // beside a window, never over one.
    if strips.indicator == crate::ChannelIndicator::Pointer && strips.pointer_shown {
        let track = Rect::new(
            geometry.track_x as f64,
            geometry.bank_padding as f64,
            geometry.track_width as f64,
            (bank.1 - 2.0 * geometry.bank_padding as f64).max(0.0),
        );
        if track.width > 0.0 && track.height > 0.0 {
            let target =
                geometry.pointer_y(strips.current_row, pager_rect.y + pager_rect.height / 2.0);
            let painting = crate::shells::selector_track(
                cfg.chassis.shell,
                plastic,
                (track.width, track.height),
                target,
            );
            if !painting.is_empty() {
                pieces.push(Piece::painted(track, painting));
            }
        }
    }

    for (i, row) in strips.rows.iter().enumerate() {
        let strip = strip_rect(geometry, shell, i);
        // The row is the content lane by the shell's row height, and its
        // furniture fills it. The window's rectangle inside that row is
        // computed below from the shared `strip_rect`.
        let row_rect = Rect::new(
            geometry.content_x as f64,
            strip.y - (geometry.row_height as f64 - strip.height) / 2.0,
            (bank.0 - geometry.content_x as f64).max(0.0),
            geometry.row_height as f64,
        );
        let display_rect = Rect::new(
            strip.x - row_rect.x,
            (row_rect.height - strip.height) / 2.0,
            strip.width,
            strip.height,
        );
        for mut piece in crate::shells::row_furniture(
            cfg.chassis.shell,
            plastic,
            &row.numeral,
            (row_rect.width, row_rect.height),
            display_rect,
            row.current,
            font_color(cfg),
        ) {
            piece.rect.x += row_rect.x;
            piece.rect.y += row_rect.y;
            pieces.push(piece);
        }
        // Already decided by whoever built the page: the glow law lights
        // only the channel on the air, and the other two light every open
        // window because the mark is the selector's or the lever's.
        let bright = row.bright;
        match display {
            Display::Led(kit) => pieces.extend(led_piece(cfg, kit, geometry, strip, row, bright)),
            Display::Tape(kit) => {
                // The well is fixed furniture and is drawn always; the tape
                // is only there when a channel is. So the chrome goes down
                // whatever the slot holds, and the label, when there is
                // one, lies in it.
                pieces.push(Piece::painted(strip, tape::well_chrome(strip)));
                pieces.extend(tape_piece(cfg, kit, strip, row, bright));
            }
        }
    }

    // Last, and over everything: the pager is drawn as the final piece of
    // the bank.
    if pager_rect.width > 0.0 && pager_rect.height > 0.0 {
        for mut piece in crate::shells::pager(
            cfg.chassis.shell,
            plastic,
            (pager_rect.width, pager_rect.height),
            strips.page_index,
            strips.page_count,
            cfg,
        ) {
            // The shells build the pager in its own coordinates; the bank
            // is where it stands.
            piece.rect.x += pager_rect.x;
            piece.rect.y += pager_rect.y;
            pieces.push(piece);
        }
    }
    pieces
}

/// The appliance's plastic, which every piece of painted furniture takes its
/// shade from.
///
/// Three derived colours go in, not the profile's stored hex: the frame
/// colour through the shared hex parser, and the *mixed* font and background
/// colours that [`font_color`] and [`background_color`] compute.
/// `crt::params` builds the same value for the frame pass.
pub fn plastic(cfg: &Config) -> Rgba {
    let frame = if cfg.general.chassis_shown {
        &cfg.chassis.frame_color
    } else {
        &cfg.screen.frame_color
    };
    color::frame_base_color(
        color::str_to_color(frame),
        font_color(cfg),
        background_color(cfg),
        cfg.screen.ambient_light as f32,
    )
}

/// The derived background colour, the other half of the pair [`font_color`]
/// computes; see that function's doc for the whole derivation.
pub fn background_color(cfg: &Config) -> Rgba {
    let saturated = color::mix(
        color::str_to_color(&cfg.screen.font_color),
        color::str_to_color("#FFFFFF"),
        cfg.screen.saturation_color as f32 * 0.5,
    );
    let background = color::str_to_color(&cfg.screen.background_color);
    color::mix(
        saturated,
        background,
        0.7 + cfg.screen.contrast as f32 * 0.3,
    )
}

/// One LED window, glyph raster through shader uniforms, end to end.
fn led_piece(
    cfg: &Config,
    kit: &LedMetrics,
    geometry: &BankGeometry,
    strip: Rect,
    row: &StripRow,
    bright: bool,
) -> Option<Piece> {
    // The count the strip was measured at, not the count the settings ask
    // for: in a window too narrow for the configured bank the two part
    // company, and the lamps belong to the width beside them.
    let characters = geometry.characters.max(0) as u32;
    let lamp_cell_width = kit.lamp_cell_width.max(1) as u32;
    let lamp_cell_height = kit.lamp_cell_height.max(1) as u32;
    let pad_cells_y = geometry.pad_cells_y.max(0) as u32;
    let grid = led::grid_size(
        lamp_cell_width,
        lamp_cell_height,
        characters,
        led::LED_SIDE_PAD_CELLS,
        led::LED_SIDE_PAD_CELLS,
        pad_cells_y,
    );

    let entry = term::fonts::font_by_name_or(
        &cfg.chassis.bank_font_name,
        crate::font_source(cfg),
        led::DEFAULT_LED_FONT_NAME,
    )?;
    let shown = led::visible_text(&row.title, row.open, characters as usize);
    // The top band splits the vertical pad in half, rounded down.
    let source = led_grid(
        entry.data(),
        entry.pixel_size,
        shown,
        lamp_cell_width,
        grid,
        led::LED_SIDE_PAD_CELLS,
        pad_cells_y / 2,
    );

    // Both margins are struck from the strip's *height*, and the drawn
    // rectangle stands proud of the strip by them on every side.
    let (spill_x, spill_y) = led::spill_margins(strip.height);
    let (spill_x, spill_y) = (spill_x as f64, spill_y as f64);
    let grown = Rect::new(
        strip.x - spill_x,
        strip.y - spill_y,
        strip.width + 2.0 * spill_x,
        strip.height + 2.0 * spill_y,
    );

    let colors = led::window_colors(font_color(cfg), row.open, bright);
    Some(Piece::shaded(
        grown,
        PieceParams::Led(led_params(
            grid,
            colors,
            led::glow(bright),
            led::spill_strength(row.open, bright),
            (spill_x, spill_y),
            (grown.width, grown.height),
        )),
        Some(source),
    ))
}

/// One tape label: the well plus the stamped label inside it.
///
/// The well's gradient floor around the label is a vector rectangle, not a
/// pass; see this module's doc on where that line runs.
fn tape_piece(
    cfg: &Config,
    kit: &TapeMetrics,
    strip: Rect,
    row: &StripRow,
    _bright: bool,
) -> Option<Piece> {
    /// The label's inset inside the well.
    const WELL_INSET: f64 = 3.0;

    // The cabinet's own face, the one `tape_metrics` measured the wheel from,
    // so the stamp and the spacing it was cut for agree.
    let entry = term::fonts::font_by_name_or(
        &cfg.chassis.bank_font_name,
        crate::font_source(cfg),
        tape::FONT_NAME,
    )?;
    let label_h = (strip.height - 2.0 * WELL_INSET).max(1.0);
    let letter_scale = tape::letter_scale(label_h);
    let raster_size = tape::raster_size(label_h, letter_scale);

    // A dark slot shows blank tape, which is a label with no glyph box
    // rather than no label: the stamp still runs over an empty slot.
    let characters = kit.min_characters.max(0) as usize;
    let shown = tape::visible_text(&row.title, row.open, characters).to_uppercase();
    let raster = term::fonts::led::led_text_image(entry.data(), raster_size, &shown);
    let (glyph_w, glyph_h) = raster
        .as_ref()
        .map(|r| (r.width as f64 / 2.0, r.height as f64 / 2.0))
        .unwrap_or((0.0, 0.0));
    let label_w = tape::tape_label_implicit_width(glyph_w, tape::END_PAD as f64).max(1) as f64;
    let rect = Rect::new(strip.x + WELL_INSET, strip.y + WELL_INSET, label_w, label_h);
    let glyph_rect = tape::glyph_rect_px(label_w, label_h, glyph_w, glyph_h);
    let source = raster.map(|r| Raster {
        width: r.width,
        height: r.height,
        rgba: raster::to_rgba8(&r).into(),
    });
    Some(Piece::shaded(
        rect,
        PieceParams::Tape(tape_params((label_w, label_h), glyph_rect)),
        // A blank tape has no glyph raster at all; the pass samples nothing
        // inside a zero-area glyph box, so a 1x1 dark texel is enough to bind.
        Some(source.unwrap_or(Raster {
            width: 1,
            height: 1,
            rgba: vec![0, 0, 0, 0].into(),
        })),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Cabinet;

    /// The lamp uniforms of a piece the caller knows is an LED window.
    fn led_of(p: &Piece) -> LedMetalParams {
        match p.params {
            Some(PieceParams::Led(led)) => led,
            other => panic!("not an LED window: {other:?}"),
        }
    }

    #[test]
    fn a_press_lands_in_the_window_it_was_drawn_in() {
        let cfg = Config::default();
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        let rows = cabinet.rows_visible() as usize;
        let g = cabinet.geometry();
        let shell = crate::shell_metrics(&cfg);

        // The rectangle the hit test uses is the one the drawing used, which is
        // the whole point of sharing `strip_rect`: a press in the middle of the
        // third window is the third row and nothing else.
        let third = strip_rect(g, &shell, 2);
        let mid = |r: Rect| (r.x + r.width / 2.0, r.y + r.height / 2.0);
        let (x, y) = mid(third);
        assert_eq!(cabinet.strip_at(rows, x, y), Some(2));
        assert_eq!(cabinet.strip_at(rows, third.x, third.y), Some(2));

        // The numeral lane to the left of the windows is plate, and a row
        // carries no press area of its own there.
        assert_eq!(cabinet.strip_at(rows, third.x - 1.0, y), None);
        // ...and so is the air between two rows, where the pitch exceeds the
        // window's own height.
        let fourth = strip_rect(g, &shell, 3);
        assert!(fourth.y > third.y + third.height);
        assert_eq!(cabinet.strip_at(rows, x, third.y + third.height), None);
        // A row past the page's last engraved key is nobody's.
        assert_eq!(
            cabinet.strip_at(rows, x, mid(strip_rect(g, &shell, rows)).1),
            None
        );
    }

    /// A press in the pager's left third is PREV, the right third NEXT, and
    /// the middle -- counter, plate, whatever the shell stands there -- dead.
    #[test]
    fn a_press_on_the_pagers_thirds_walks_the_pages() {
        let cfg = Config::default();
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        let bank = (cabinet.layout().bank.width, cabinet.layout().bank.height);
        let pager = pager_rect(cabinet.geometry(), &crate::shell_metrics(&cfg), bank);
        let y = pager.y + pager.height / 2.0;
        assert_eq!(cabinet.pager_at(pager.x + pager.width / 6.0, y), Some(-1));
        assert_eq!(cabinet.pager_at(pager.x + pager.width * 5.0 / 6.0, y), Some(1));
        assert_eq!(cabinet.pager_at(pager.x + pager.width / 2.0, y), None);
        // Above the pager are the rows, whose press is `strip_at`'s.
        assert_eq!(cabinet.pager_at(pager.x + pager.width / 6.0, pager.y - 1.0), None);
        // Off the pager's own edges, and on a chassis nobody drew.
        assert_eq!(cabinet.pager_at(pager.x - 1.0, y), None);
        let mut hidden = cfg.clone();
        hidden.general.chassis_shown = false;
        assert_eq!(
            Cabinet::from_config(&hidden, 1024.0, 768.0).pager_at(pager.x + 1.0, y),
            None
        );
    }

    #[test]
    fn the_shipped_appliance_puts_a_plate_and_a_strip_per_row_on_the_casting() {
        let cfg = Config::default();
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        let rows = cabinet.rows_visible();
        // With the annunciator's own 130px pager:
        // floor((768 - 61 - 8 - 130 - 2 + 2) / 45) = floor(569/45) = 12.
        assert_eq!(rows, 12);

        let pieces = cabinet.furniture(&BankStrips::cold_start(rows as usize));
        // Item order, top to bottom: the plate, the chassis's four screws
        // over it, then one row's furniture and one row's window per
        // engraved key, then the pager.
        assert_eq!(pieces.len(), 1 + 4 + 2 * rows as usize + 1);
        assert_eq!(pieces[0].pass, Pass::Plate);
        // The annunciator's plate: 8px left margin, 2px top margin, no
        // right margin, 8px bottom margin, over a 205px bank.
        assert_eq!(pieces[0].rect, Rect::new(8.0, 2.0, 197.0, 758.0));
        assert!(pieces[0].source.is_none());

        // Four 28px screws, two at the plate's head and two 25px off its
        // foot, the right pair pinned to the chassis's right edge so a
        // narrower window keeps them.
        for p in &pieces[1..5] {
            assert_eq!(p.pass, Pass::Painted);
            assert_eq!((p.rect.width, p.rect.height), (28.0, 28.0));
        }
        assert_eq!((pieces[1].rect.x, pieces[1].rect.y), (18.0, 16.0));
        assert_eq!(pieces[2].rect.x, 205.0 - 12.0 - 28.0);
        assert_eq!(pieces[3].rect.y, 768.0 - 25.0 - 28.0);

        let row_piece = |i: usize| &pieces[5 + 2 * i];
        let strip_piece = |i: usize| &pieces[6 + 2 * i];

        // Every row is furniture then an LED window, and only the first one is
        // powered.
        for i in 0..rows as usize {
            assert_eq!(row_piece(i).pass, Pass::Painted);
            assert!(row_piece(i).paint.is_some());
            assert_eq!(strip_piece(i).pass, Pass::LedMatrix);
            assert!(strip_piece(i).source.is_some());
        }
        let spill = |p: &Piece| led_of(p).spill_strength;
        assert_eq!(spill(strip_piece(0)), 1.0); // powered and bright
        assert_eq!(spill(strip_piece(1)), 0.0); // dark

        // The strip stands past the numeral lane, the row pitch is the
        // shell's, and the drawn rectangle is the strip grown by its spill
        // margins.
        let g = cabinet.geometry();
        let strip_x = (g.content_x + 46 + 16) as f64;
        let (sx, sy) = led::spill_margins(g.strip_height as f64);
        assert_eq!(strip_piece(0).rect.x, strip_x - sx as f64);
        assert_eq!(
            strip_piece(0).rect.width,
            g.strip_width as f64 + 2.0 * sx as f64
        );
        assert_eq!(
            strip_piece(1).rect.y - strip_piece(0).rect.y,
            g.row_height as f64 + g.row_spacing
        );
        // The strip's right edge is the bank less the shell's right padding.
        assert_eq!(strip_x + g.strip_width as f64, 205.0 - 14.0);
        // ...and the grown rectangle overhangs the bank on purpose: the
        // spill is light thrown on the plate, and the plate runs on.
        assert!(strip_piece(0).rect.y - sy as f64 <= 61.0);

        // The row's furniture covers the content lane at the row's own pitch,
        // and the window it paints its moulding around is the one the strip
        // was drawn in: the same sharing `strip_rect` gives the hit test.
        let furniture = row_piece(0);
        assert_eq!(furniture.rect.x, g.content_x as f64);
        assert_eq!(furniture.rect.width, 205.0 - g.content_x as f64);
        assert_eq!(furniture.rect.height, g.row_height as f64);
        assert_eq!(
            row_piece(1).rect.y - furniture.rect.y,
            g.row_height as f64 + g.row_spacing
        );

        // The pager stands last, in the content lane, its own height above
        // the shell's bottom padding.
        let pager = pieces.last().unwrap();
        assert_eq!(pager.pass, Pass::Painted);
        assert_eq!(pager.rect.x, g.content_x as f64);
        assert_eq!(pager.rect.height, 130.0);
        assert_eq!(pager.rect.y + 130.0, 768.0 - 8.0);
    }

    #[test]
    fn the_switchboard_has_no_plate_and_stamps_tape() {
        let mut cfg = Config::default();
        cfg.chassis.shell = config::Shell::Switchboard;
        cfg.chassis.channel_display = config::ChannelDisplay::Tape;
        cfg.chassis.channel_indicator = config::ChannelIndicator::Switch;
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        let pieces = cabinet.furniture(&BankStrips::cold_start(3));
        // The switchboard chassis has no plate region and no screws on the
        // casting at all, so the first piece is already a row. This
        // shell's row is four of them, because its plate is a `plate_metal`
        // pass under painted rivets, well and numeral, and the tape kit lays a
        // painted well under its stamped label.
        assert_eq!(
            pieces[..4].iter().map(|p| p.pass).collect::<Vec<_>>(),
            vec![Pass::Plate, Pass::Painted, Pass::Painted, Pass::TapeLabel]
        );
        // Under the switch law every open window is bright, so the one open
        // slot's brightness is not what tells the laws apart. The geometry
        // is: the label is inset 3px inside the well at both ends.
        assert_eq!(
            pieces[3].rect.height,
            cabinet.geometry().strip_height as f64 - 6.0
        );
        // ...and the well the label lies in is the whole strip, drawn
        // whether a channel is there or not.
        assert_eq!(
            pieces[2].rect.height,
            cabinet.geometry().strip_height as f64
        );

        // The rail across the bank's foot follows the three rows.
        let pager: Vec<Pass> = pieces[12..].iter().map(|p| p.pass).collect();
        assert_eq!(pager[0], Pass::Plate, "the pager's rail plate");
        assert!(pager.contains(&Pass::Painted));
    }

    #[test]
    fn the_slide_rule_screws_its_rail_over_the_casting() {
        let mut cfg = Config::default();
        cfg.chassis.shell = config::Shell::SlideRule;
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        let pieces = cabinet.furniture(&BankStrips::cold_start(2));
        assert_eq!(pieces[0].pass, Pass::Plate);
        // x 29, width 41, inset 29 top and bottom: the rail, which is this
        // shell's plate region.
        assert_eq!(pieces[0].rect, Rect::new(29.0, 29.0, 41.0, 768.0 - 58.0));
    }

    /// The done-test's per-piece readback for the slide rule's page counter
    /// lamps (`shells::slide_rule::counter_lamps`): a second display-kit
    /// instance mounts behind the page window, at a two-character,
    /// no-padding, throttled-spill configuration.
    #[test]
    fn the_slide_rule_pagers_counter_lamps_burn_the_page_number() {
        let mut cfg = Config::default();
        cfg.chassis.shell = config::Shell::SlideRule;
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        let mut strips = BankStrips::cold_start(cabinet.rows_visible() as usize);
        strips.page_count = 3;
        strips.page_index = 0; // "01"
        let pieces = cabinet.furniture(&strips);
        // `bank_pieces`'s own doc: "Last, and over everything": the pager,
        // and within it the counter lamps are the last thing this shell's
        // own `pager` pushes.
        let counter = pieces.last().expect("a pager");
        assert_eq!(counter.pass, Pass::LedMatrix);
        let source = counter.source.as_ref().expect("a lamp raster");
        assert!(
            source.rgba.iter().any(|&b| b > 0),
            "no glyph struck for the page label"
        );

        let p = led_of(counter);
        // `characters: 2`, every pad at 0.
        let kit = crate::led_metrics(&cfg.chassis.bank_font_name, term::FontSource::Bundled);
        assert_eq!(p.grid_size[0], (kit.lamp_cell_width.max(1) * 2) as f32);
        assert_eq!(p.grid_size[1], kit.lamp_cell_height.max(1) as f32);
        // `spill_strength: 0.12`, unlike a channel window's own
        // `led::spill_strength`.
        assert_eq!(p.spill_strength, 0.12);
        // The counter's own `bright: false` binding.
        assert_eq!(p.glow, led::glow(false));

        // A different page turns different lamps: the raster is the label's
        // own, not a fixture.
        strips.page_index = 8; // "09"
        let turned = cabinet.furniture(&strips);
        let turned_counter = turned.last().expect("a pager");
        assert_ne!(
            counter.source, turned_counter.source,
            "the counter lamps did not follow the page label"
        );
    }

    #[test]
    fn a_hidden_chassis_has_no_furniture() {
        let mut cfg = Config::default();
        cfg.general.chassis_shown = false;
        let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
        assert!(cabinet.furniture(&BankStrips::cold_start(12)).is_empty());
    }

    #[test]
    fn the_lamp_grid_lays_the_proven_raster_in_a_field_of_dark_ones() {
        let entry =
            term::fonts::font_by_name(led::DEFAULT_LED_FONT_NAME, term::fonts::FontSource::Bundled)
                .unwrap();
        let cell = 8;
        let grid = (cell * (12 + 2), 8 + 15);
        let r = led_grid(
            entry.data(),
            entry.pixel_size,
            "1",
            cell,
            grid,
            led::LED_SIDE_PAD_CELLS,
            15 / 2,
        );
        assert_eq!((r.width, r.height), grid);
        assert_eq!(r.rgba.len() as u32, grid.0 * grid.1 * 4);

        // The side pad column and the top band are unlit, whatever the glyph.
        let lit = |x: u32, y: u32| r.rgba[((y * grid.0 + x) * 4) as usize];
        for y in 0..grid.1 {
            for x in 0..cell {
                assert_eq!(lit(x, y), 0, "the left pad cell is lit at ({x},{y})");
            }
        }
        for y in 0..(15 / 2) {
            for x in 0..grid.0 {
                assert_eq!(lit(x, y), 0, "the top band is lit at ({x},{y})");
            }
        }
        // ...and the glyph itself did arrive, in the first character cell
        // past the pad.
        let glyph = term::fonts::led::led_text_image(entry.data(), entry.pixel_size, "1").unwrap();
        assert!(glyph.alpha.iter().any(|&a| a > 0));
        for y in 0..glyph.height {
            for x in 0..glyph.width {
                assert_eq!(
                    lit(x + cell, y + 15 / 2),
                    glyph.alpha[(y * glyph.width + x) as usize],
                    "the raster moved at ({x},{y})"
                );
            }
        }

        // Empty text is a field of unlit lamps, not an absent one.
        let dark = led_grid(
            entry.data(),
            entry.pixel_size,
            "",
            cell,
            grid,
            led::LED_SIDE_PAD_CELLS,
            7,
        );
        assert!(dark.rgba.iter().all(|&b| b == 0));
        assert_eq!(dark.rgba.len(), r.rgba.len());
    }

    #[test]
    fn the_lamps_are_struck_from_the_derived_font_colour_not_the_stored_hex() {
        // `crt::params::screen_colors`' chrome half. At the shipped contrast
        // of 0.8 the stored "#ff8100" is a fifth too bright, which is the
        // whole reason this derivation exists.
        let cfg = Config::default();
        let stored = color::str_to_color(&cfg.screen.font_color);
        let derived = font_color(&cfg);
        assert_ne!(derived, stored);
        assert!(derived.r < stored.r);
        // The lamp law then normalises to full brightness, so the *lit*
        // colour is near the hue's peak either way; what moves is dim/panel.
        let a = led::window_colors(derived, true, true);
        let b = led::window_colors(stored, true, true);
        assert!((a.lit.r - b.lit.r).abs() < 1e-3);
    }
}
