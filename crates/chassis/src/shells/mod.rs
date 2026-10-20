//! One module per shell:
//!
//! | module | appliance |
//! |---|---|
//! | [`annunciator`] | the amber appliance |
//! | [`slide_rule`] | the blue appliance |
//! | [`switchboard`] | the toggle-switch appliance |
//!
//! Each module holds two things for its shell:
//!
//! - **Metrics**: every fixed measure as a Rust constant. This is the
//!   geometry/layout contract the bank furniture (row furniture, pager,
//!   selector track, handled elsewhere) is built against; keeping it
//!   here regardless of which later consumer reads which field is what
//!   keeps the contract in one place per shell.
//! - **Chassis + frame**: the drawing recipe. Each region is painted with
//!   one of the three procedural metal shaders whose math lives in
//!   the `robco-shader-oracle` crate (`chassis_metal`, `plate_metal`, `frame_metal`). What
//!   differs shell to shell is which regions exist, their geometry, and the
//!   fixed parameters (colors, light direction, grain/mottle/scratch/vignette
//!   amounts) that shell chooses: held here as functions that build
//!   [`crate::params`]'s structs, so a done-test can hand them
//!   straight to the oracle (or to a real GPU render of the same preset).
//!
//! Two things every shell's chassis does identically, factored into
//! [`common`] rather than repeated three times: the frame-region field
//! mapping (viewport size / field scale / field offset) that lets the
//! chassis metal continue the frame's own procedural field, and the
//! frame's own viewport-size formula.
//!
//! Each module also holds its shell's **bank furniture** (row furniture,
//! pager, selector track, and where the shell has them the chassis screw
//! placements) as [`crate::paint::Painting`]s and, where a shell sinks a
//! `plate_metal` cap into one of them, as mixed piece lists.
//! [`row_furniture`], [`pager`], [`selector_track`] and [`screws`] below are
//! the per-shell dispatchers `crate::furniture` builds its plan from.
//!
//! Three pieces of canvas furniture stay out, each named where it would
//! have gone: the slide rule's rail groove and its hinge bracket's own
//! weathered plate body (the three screws bolted through it are painted, at
//! their own measured bank coordinates, [`slide_rule::screw_places`]), and
//! the switchboard's lever throw -- the lever itself is painted at rest,
//! only its swing and easing overshoot stay unbuilt
//! ([`switchboard::row_furniture`]).

pub mod annunciator;
pub mod common;
pub mod slide_rule;
pub mod switchboard;

use crate::color::Rgba;
use crate::furniture::Piece;
use crate::layout::Rect as PaintRect;
use crate::params::PlateMetalParams;
use crate::paint::Painting;
use common::Rect;

/// The one region a shell screws over its casting in `plate_metal`, if it has
/// one, for a chassis item of `chassis_size` logical pixels.
///
/// Three shells, three answers, and the third is the reason this returns an
/// `Option` rather than a rect a caller could draw at zero size: the
/// switchboard's chassis has no plate and no rail at all -- the thrown
/// switch is the whole mark and nothing rides the chassis -- so there is
/// nothing to draw, not a plate of no area.
///
/// The annunciator's is the raised bank plate
/// ([`annunciator::plate_rect`]/[`annunciator::plate_metal_params`]); the slide
/// rule's is the carrier rail ([`slide_rule::rail_rect`]/
/// [`slide_rule::rail_metal_params`]), which is fixed chassis furniture on that
/// shell and drawn whatever the profile's indicator law says: the law decides
/// whether a *carriage* rides the rail, not whether the rail is cast.
pub fn plate_region(
    shell: config::Shell,
    chassis_size: (f64, f64),
) -> Option<(Rect, PlateMetalParams)> {
    let rect = match shell {
        config::Shell::Annunciator => annunciator::plate_rect(chassis_size),
        config::Shell::SlideRule => slide_rule::rail_rect(chassis_size),
        config::Shell::Switchboard => return None,
    };
    let size_px = [rect.width as f32, rect.height as f32];
    let params = match shell {
        config::Shell::Annunciator => annunciator::plate_metal_params(size_px),
        config::Shell::SlideRule => slide_rule::rail_metal_params(size_px),
        config::Shell::Switchboard => unreachable!("returned above"),
    };
    Some((rect, params))
}

/// One row's furniture (numeral, moulding, punched hole) for whichever
/// shell the profile is wearing.
///
/// The three shells' row furniture agree on their interface and on nothing
/// else: an amber plate with a machined bezel, a blue chassis with the window
/// punched straight into it, and a riveted switchboard plate carrying a
/// stamped numeral over a toggle well. `current` and `glow` reach only the
/// switchboard, whose lit well is the mark of the channel on the air.
///
/// A list rather than one painting, for that same shell's sake: its row stands
/// on a `plate_metal` plate of its own, so its furniture is mixed the way its
/// pager is. The other two are one painting each.
///
/// Two of the three paint past the row band: rows are not clipped, so the
/// slide rule's lit bevel line sits in the air *below* the hole and the
/// switchboard's plate and spill stand proud of the band at both ends. The
/// piece's rectangle is the raster's bounds, so it is grown by
/// `row_overhang` and the painting shifted down into it; a row cut at its
/// own height would swallow those. The rows are still painted in order, so
/// a row's overhang lands under the next row's furniture exactly as a
/// simple top-to-bottom stack puts it.
pub fn row_furniture(
    shell: config::Shell,
    plastic: Rgba,
    numeral: &str,
    row: (f64, f64),
    display_rect: PaintRect,
    current: bool,
    glow: Rgba,
) -> Vec<Piece> {
    let (top, bottom) = row_overhang(shell);
    let one = |p: Painting| {
        if p.is_empty() {
            Vec::new()
        } else {
            vec![Piece::painted(
                PaintRect::new(0.0, -top, row.0, row.1 + top + bottom),
                p.translated(0.0, top),
            )]
        }
    };
    match shell {
        config::Shell::Annunciator => one(annunciator::row_furniture(
            plastic,
            numeral,
            row,
            display_rect,
        )),
        config::Shell::SlideRule => one(slide_rule::row_furniture(
            plastic,
            numeral,
            row,
            display_rect,
        )),
        config::Shell::Switchboard => {
            let (top, bottom) = (top, bottom);
            switchboard::row_furniture(plastic, numeral, row, current, glow)
                .into_iter()
                .map(|mut piece| {
                    if let Some(paint) = piece.paint.take() {
                        piece.rect = PaintRect::new(0.0, -top, row.0, row.1 + top + bottom);
                        piece.paint = Some(paint.translated(0.0, top));
                    }
                    piece
                })
                .collect()
        }
    }
}

/// How far past the row band a shell's row furniture paints, top and bottom,
/// in logical pixels.
///
/// - The **annunciator** paints nothing outside the band: its rim is the row's
///   own height and its hole is inset four pixels inside that.
/// - The **slide rule** puts the bright bevel line on the chassis one pixel
///   *below* the hole, two pixels tall.
/// - The **switchboard** stands its plate four pixels proud at each end, and
///   the lit well's spill reaches thirteen past the well, which is itself
///   52 tall in a 48 band.
fn row_overhang(shell: config::Shell) -> (f64, f64) {
    match shell {
        config::Shell::Annunciator => (0.0, 0.0),
        config::Shell::SlideRule => (0.0, 3.0),
        config::Shell::Switchboard => (16.0, 16.0),
    }
}

/// The shell's pager, at the bank's foot, as pieces in the pager item's
/// own coordinates.
///
/// A list and not one painting, because two of the three pagers are mixed
/// assemblies: the slide rule sinks a `plate_metal` key cap into each of its
/// outer recesses and burns the page number in the profile's own LED kit
/// between them, and the switchboard rivets everything onto a `plate_metal`
/// rail. Only the annunciator's rocker is painting end to end.
pub fn pager(
    shell: config::Shell,
    plastic: Rgba,
    size: (f64, f64),
    page_index: i32,
    page_count: i32,
    cfg: &config::Config,
) -> Vec<Piece> {
    match shell {
        config::Shell::Annunciator => annunciator::pager(plastic, size, page_count),
        config::Shell::SlideRule => slide_rule::pager(plastic, size, page_index, page_count, cfg),
        config::Shell::Switchboard => switchboard::pager(plastic, size, page_index, page_count),
    }
}

/// The shell's selector track, for a profile whose indicator is the
/// pointer.
///
/// The switchboard has no such thing: its mark is the thrown lever and its
/// metrics never offer a lane, so a pointer profile on that shell gets
/// an empty painting rather than a track drawn from another shell's rules.
pub fn selector_track(
    shell: config::Shell,
    plastic: Rgba,
    size: (f64, f64),
    target_y: f64,
) -> Painting {
    match shell {
        config::Shell::Annunciator => annunciator::selector_track(plastic, size, target_y),
        config::Shell::SlideRule => slide_rule::selector_track(plastic, size, target_y),
        config::Shell::Switchboard => Painting::new(),
    }
}

/// The screws a shell's chassis bolts its plate down with, as pieces in
/// the bank column's own coordinates.
///
/// `common::ScrewHead` is one component with one set of measures; what differs
/// per shell is how many there are, where, and -- for the slide rule's, turned
/// from the hinge bracket's own sheet-metal palette rather than the kit's
/// default steel (`common::ScrewColors`'s own doc) -- what colour. The
/// switchboard screws nothing to its casting (it has no plate to screw down),
/// so its list is empty.
pub fn screws(shell: config::Shell, chassis_size: (f64, f64)) -> Vec<Piece> {
    match shell {
        config::Shell::Annunciator => annunciator::screw_places(chassis_size)
            .into_iter()
            .map(|(rect, angle)| Piece::painted(rect, common::screw_head(rect.width, angle)))
            .collect(),
        config::Shell::SlideRule => slide_rule::screw_places(chassis_size)
            .into_iter()
            .map(|(rect, angle)| Piece::painted(rect, slide_rule::bracket_screw(rect.width, angle)))
            .collect(),
        config::Shell::Switchboard => Vec::new(),
    }
}
