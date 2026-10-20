//! The blue appliance: bare scratched gunmetal (`chassis_metal`) with a
//! full-height carrier rail (`plate_metal`) standing on it as furniture. No
//! plate: numerals and windows punch straight into the chassis. The rail's
//! groove and hinge bracket are canvas painting, not a procedural metal
//! shader, and are out of this module's scope (see `shells` module docs).

use crate::color::{darker, lighter, hex_literal_to_color, Rgba};
use crate::furniture::Piece;
use crate::layout::Rect as PaintRect;
use crate::params::{MetalParams, PlateMetalParams};
use crate::paint::{Align, Face, Painting, PolygonOp, RectOp, Stop, TextOp};

use super::common::{numeral_line_height, rgb, sans_width, CastingRecipe, FrameRecipe, Rect};

/// This shell's fixed geometry, field for field.
///
/// The crate's one home for these numbers: [`crate::metrics::shells::slide_rule`]
/// composes these same constants into the `ShellMetrics` the bank reads,
/// rather than restating them, and `tests/suite/metrics_homes.rs` pins that
/// composition as a regression check. A correction to any of these numbers
/// belongs here and only here.
pub mod metrics {
    /// Chassis before the rail (29) + the rail's own width (41) + a sliver
    /// before the numerals.
    pub const BANK_PADDING: i32 = 76;
    /// Row 1's window top, mock y 64.
    pub const TOP_PADDING: i32 = 64;
    /// Chassis past the selector's bevel line at mock y 1023.
    pub const BOTTOM_PADDING: i32 = 63;
    pub const RIGHT_PADDING: i32 = 8;
    /// Mock pitch 60.8, less the 43px window leaves this fraction.
    pub const ROW_SPACING: f64 = 17.8;
    pub const COLUMN_GAP: i32 = 8;
    /// The stamped digits' own width, no blank lane.
    pub const NUMERAL_WIDTH: i32 = 36;
    pub const STRIP_PADDING: i32 = 2;
    pub const MIN_ROW_HEIGHT: i32 = 43;
    /// The chassis draws the rail as fixed furniture, so the bank cuts no
    /// second lane out of the rows.
    pub const CHASSIS_CARRIES_TRACK: bool = true;
    /// Rail x on the mock, 29..69.
    pub const TRACK_X: i32 = 29;
    pub const TRACK_WIDTH: i32 = 41;
    pub const CASTING_LIGHT_DIR: [f32; 2] = [-0.55, -0.85];
    pub const CASTING_COLOR: &str = "#453c2d";
}

/// This shell's pager measures, the three the bank's foot needs.
///
/// The one pager of the three whose height is measured rather than declared,
/// which is why the composed shape carries a span and an addend at all.
/// [`crate::metrics::shells::slide_rule`] composes these into `ShellMetrics`
/// without restating them.
pub mod pager {
    /// The group's natural hole, before any squeeze.
    pub const NATURAL_HEIGHT: f64 = 90.0;
    /// The content width this group is drawn at full size for: `squeeze =
    /// min(1, width > 0 ? width / 305 : 1)`.
    pub const SQUEEZE_SPAN: f64 = 305.0;
    /// The bevel line under the hole, unsqueezed: `round(hole_height) + 3`.
    pub const EXTRA: i32 = 3;
}

/// `chassis_metal` covers the whole chassis item.
pub fn chassis_rect(chassis_size: (f64, f64)) -> Rect {
    Rect::new(0.0, 0.0, chassis_size.0, chassis_size.1)
}

/// The rail: x 29, width [`metrics::TRACK_WIDTH`], inset 29 top and bottom.
pub fn rail_rect(chassis_size: (f64, f64)) -> Rect {
    let (_, h) = chassis_size;
    Rect::new(
        metrics::TRACK_X as f64,
        29.0,
        metrics::TRACK_WIDTH as f64,
        (h - 29.0 - 29.0).max(0.0),
    )
}

/// The casting's fixed uniforms (`opacity` excluded per the same reasoning
/// as the annunciator's).
pub const CASTING: CastingRecipe = CastingRecipe {
    light_dir: metrics::CASTING_LIGHT_DIR,
    color: metrics::CASTING_COLOR,
    metal: MetalParams {
        grain_amount: 0.4,
        mottle_amount: 1.35,
        scratch_amount: 0.7,
    },
    vignette_strength: 0.3,
};

/// The rail's fixed uniforms. `size_px` is filled in by the caller from
/// [`rail_rect`]'s own size.
pub fn rail_metal_params(size_px: [f32; 2]) -> PlateMetalParams {
    PlateMetalParams {
        size_px,
        light_dir: [-0.55, -0.85],
        base_color: rgb("#2b2418"),
        highlight_color: rgb("#7d735c"),
        shadow_color: rgb("#050403"),
        corner_radius: 4.0,
        bevel_px: 2.0,
        metal: MetalParams {
            grain_amount: 0.35,
            mottle_amount: 0.7,
            scratch_amount: 0.5,
        },
        vignette_strength: 0.35,
        wear_amount: 0.35,
        seam_gain: 0.6,
        seed: 0.53,
    }
}

/// The bezel's fixed uniforms.
pub const FRAME: FrameRecipe = FrameRecipe {
    light_dir: metrics::CASTING_LIGHT_DIR,
    bezel_color: "#2e2820",
    chassis_color: metrics::CASTING_COLOR,
    ridge_color: "#8c8068",
    bezel_margins: [6.0, 10.0, 14.0, 12.0],
    outer_radius: 45.0,
    well_depth: 45.0,
    well_floor: 0.16,
    ridge_gain: 0.45,
    metal: MetalParams {
        grain_amount: 0.35,
        mottle_amount: 1.0,
        scratch_amount: 0.55,
    },
    vignette_strength: 0.35,
    fill_gain: 0.35,
    trough_gain: 0.0,
    // The plate is lit only in a band along its own moulding, with a bright
    // rim standing on the well wall.
    face_band_px: 12.0,
    rim_dist_px: 14.0,
    rim_gain: 1.6,
};

/// The fixed colours of one row's furniture, each a hex colour literal
/// read at /255.
mod row {
    pub const NUMERAL_INK: &str = "#0d0b08";
    pub const NUMERAL_EDGE: &str = "#4a4030";
    pub const PANEL_DARK: &str = "#090d0d";
    pub const RIM_LIGHT: &str = "#b7a283";
    pub const RIM_DARK: &str = "#12100b";
    /// The pixel size struck on both stamped text items.
    pub const NUMERAL_PIXEL_SIZE: f64 = 33.0;
    pub const NUMERAL_LETTER_SPACING: f64 = -1.0;
}

/// A numeral stamped dark into the metal with a sliver of light on its
/// lower edge, and a window punched straight into the chassis: a tight dark
/// cut ring around a recessed panel, and the bright bevel line on the
/// chassis just under the hole.
///
/// The amber shell's window sits in a raised plate; this one is a hole in the
/// casting, and the two differ everywhere that follows from it: the ring is
/// three pixels rather than ten, the panel is inset two rather than four, and
/// the lit lip is painted *below* the row band instead of inside it.
pub fn row_furniture(
    plastic: Rgba,
    numeral: &str,
    row: (f64, f64),
    display_rect: PaintRect,
) -> Painting {
    let (row_w, row_h) = row;
    let mut p = Painting::new();
    if row_w <= 0.0 || row_h <= 0.0 {
        return p;
    }

    // The numeral lane, right-aligned, the metal's own catch struck a pixel
    // down and right of the ink and at half strength.
    let lane_w = display_rect.x - metrics::COLUMN_GAP as f64;
    if lane_w > 0.0 && !numeral.is_empty() {
        let line_h = numeral_line_height(row::NUMERAL_PIXEL_SIZE);
        let lane_y = (row_h - line_h) / 2.0;
        let stamped = |dx: f64, dy: f64, color: Rgba, opacity: f32| TextOp {
            face: Face::Catalogue("IOSEVKA"),
            x: dx,
            y: lane_y + dy,
            width: lane_w,
            align: Align::Right,
            pixel_size: row::NUMERAL_PIXEL_SIZE,
            letter_spacing: row::NUMERAL_LETTER_SPACING,
            bold: true,
            text: numeral.to_string(),
            color,
            opacity,
        };
        // Metal catching light along the stroke's lower edge...
        p.text(stamped(1.0, 2.0, hex_literal_to_color(row::NUMERAL_EDGE), 0.5));
        // ...the ink itself above it.
        p.text(stamped(0.0, 0.0, hex_literal_to_color(row::NUMERAL_INK), 1.0));
    }

    // The cut ring of the punch, darkest under the top lip.
    let ring = PaintRect::new(display_rect.x - 3.0, 0.0, display_rect.width + 6.0, row_h);
    p.rect(RectOp::gradient(
        ring,
        3.0,
        vec![
            Stop::new(0.00, hex_literal_to_color(row::RIM_DARK)),
            Stop::new(0.55, darker(plastic, 2.2)),
            Stop::new(1.00, darker(plastic, 1.4)),
        ],
    ));

    // The recessed panel, dark under the top lip, its far wall catching
    // light along the bottom.
    let panel_dark = hex_literal_to_color(row::PANEL_DARK);
    p.rect(RectOp::gradient(
        PaintRect::new(display_rect.x, 2.0, display_rect.width, row_h - 4.0),
        2.0,
        vec![
            Stop::new(0.0, darker(panel_dark, 1.7)),
            Stop::new(0.5, panel_dark),
            Stop::new(0.92, lighter(panel_dark, 1.5)),
            Stop::new(1.0, lighter(panel_dark, 2.6)),
        ],
    ));

    // The bright bevel line on the chassis just under the hole. It is
    // painted below the row band, in the air between rows, which is why
    // the row's own painting reaches past its height here.
    p.rect(
        RectOp::solid(
            PaintRect::new(
                display_rect.x - 2.0,
                row_h + 1.0,
                display_rect.width + 4.0,
                2.0,
            ),
            1.0,
            hex_literal_to_color(row::RIM_LIGHT),
        )
        .at_opacity(0.9),
    );
    p
}

/// The bolted steel shoe that rides the chassis's own rail.
///
/// This shell's rail is fixed chassis furniture
/// ([`metrics::CHASSIS_CARRIES_TRACK`]), so this holds nothing but the
/// carriage: a shoe with a specular ridge across its cap and a hard shadow
/// under its foot, centred in the lane the chassis cut for it. The travel
/// between rows is a hand moving the selector and has no place in a still.
/// `plastic` is taken and never read: every colour on the shoe is one of
/// the three fixed metals below.
pub fn selector_track(_plastic: Rgba, size: (f64, f64), target_y: f64) -> Painting {
    let (w, h) = size;
    let mut p = Painting::new();
    if w <= 0.0 || h <= 0.0 {
        return p;
    }
    let rail_metal = hex_literal_to_color("#2b2418");
    let bracket_light = hex_literal_to_color("#b2a47d");
    let screw_glint = hex_literal_to_color("#e3dfd2");
    let black = Rgba::new(0.0, 0.0, 0.0, 1.0);
    let clamp_h = 26.0;
    let clamp_w = 21.0;
    let x = ((w - clamp_w) / 2.0).round();
    let y = (target_y - clamp_h / 2.0).round();

    // The shadow under the shoe.
    p.rect(
        RectOp::solid(
            PaintRect::new(x + 1.0, y + 2.0, clamp_w, clamp_h),
            3.0,
            black,
        )
        .at_opacity(0.5),
    );
    // The shoe itself.
    p.rect(RectOp::gradient(
        PaintRect::new(x, y, clamp_w, clamp_h),
        3.0,
        vec![
            Stop::new(0.0, bracket_light),
            Stop::new(0.18, lighter(rail_metal, 2.0)),
            Stop::new(0.55, lighter(rail_metal, 1.3)),
            Stop::new(1.0, hex_literal_to_color("#0a0704")),
        ],
    ));
    // The specular ridge machined across the cap, and the side bevels.
    p.rect(
        RectOp::solid(
            PaintRect::new(x + 2.0, y + 3.0, clamp_w - 4.0, 1.0),
            0.0,
            screw_glint,
        )
        .at_opacity(0.9),
    );
    p.rect(
        RectOp::solid(
            PaintRect::new(x + 2.0, y + 4.0, clamp_w - 4.0, 1.0),
            0.0,
            black,
        )
        .at_opacity(0.5),
    );
    p.rect(
        RectOp::solid(
            PaintRect::new(x, y + 2.0, 1.0, clamp_h - 4.0),
            0.0,
            bracket_light,
        )
        .at_opacity(0.5),
    );
    p.rect(
        RectOp::solid(
            PaintRect::new(x + clamp_w - 1.0, y + 2.0, 1.0, clamp_h - 4.0),
            0.0,
            black,
        )
        .at_opacity(0.6),
    );
    p
}

/// The hinge bracket's three slotted screws.
///
/// The bracket itself -- an angular sheet-metal plate, with its own
/// weathering (a lit/dark cut outline, a pressed hinge knuckle, twenty-six
/// patina blotches and seven hundred grime-and-glint specks, all seeded) --
/// is still out of this module's scope; drawing it wants a painted-polygon
/// fill this stack has no primitive for (`crate::paint`'s module doc: rect,
/// arc, text, one flat-coloured polygon) and a noise generator besides.
/// What is in scope is the bracket's own *outline*, carried only as far as
/// placing the three screws it anchors, at their measured centres directly
/// in bank coordinates -- (46,69), (64,84), (47,102), the same frame
/// `chassis_size` and every other shell placement in this module is
/// measured in. The screws stand on bare casting without the plate under
/// them, which is the residual this leaves: three bolts with nothing drawn
/// to say what they hold down.
pub fn screw_places(_chassis_size: (f64, f64)) -> Vec<(PaintRect, f64)> {
    const SIZE: f64 = 22.0;
    let place = |cx: f64, cy: f64, angle: f64| {
        (
            PaintRect::new(cx - SIZE / 2.0, cy - SIZE / 2.0, SIZE, SIZE),
            angle,
        )
    };
    vec![
        place(46.0, 69.0, 32.0),
        place(64.0, 84.0, -63.0),
        place(47.0, 102.0, 74.0),
    ]
}

/// Turned from the hinge bracket's own sheet metal rather than the kit's
/// default steel, at the default light direction -- the same `(-0.6, -0.8)`
/// `common::screw_head`'s own default already is.
pub fn bracket_screw(size: f64, slot_angle: f64) -> Painting {
    super::common::screw_head_with(
        size,
        slot_angle,
        super::common::ScrewColors {
            metal_light: hex_literal_to_color("#b2a47d"),
            metal_mid: hex_literal_to_color("#4a4234"),
            metal_dark: hex_literal_to_color("#0d0a06"),
            glint: hex_literal_to_color("#e3dfd2"),
        },
        (-0.6, -0.8),
    )
}

/// Three recesses punched into the chassis like the LED windows above them,
/// PREV and NEXT sunk in the outer two as dark metal key caps, and the page
/// number burning in the profile's own display kit between them.
///
/// Mixed, like the switchboard's: the two key caps are `plate_metal` passes
/// and the page window is the display kit, so this returns pieces rather
/// than one painting. Every measure squeezes with the lane the bank hands
/// over, which is why this shell is the one whose pager height is measured
/// rather than declared.
pub fn pager(
    plastic: Rgba,
    size: (f64, f64),
    page_index: i32,
    _page_count: i32,
    cfg: &config::Config,
) -> Vec<Piece> {
    let (w, h) = size;
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    let _ = plastic;
    // The squeeze and the three measures that follow it.
    let squeeze = 1.0f64.min(if w > 0.0 {
        w / pager::SQUEEZE_SPAN
    } else {
        1.0
    });
    let key_w = 71.0 * squeeze;
    let window_w = 82.0 * squeeze;
    let hole_h = pager::NATURAL_HEIGHT * squeeze;
    let prev_x = 12.0 * squeeze;
    let next_x = w - 22.0 * squeeze - key_w;
    let window_x = (prev_x + key_w + next_x - window_w) / 2.0;
    // Two digits always, as the bank's numerals are stamped.
    let n = page_index + 1;
    let page_label = if n < 10 {
        format!("0{n}")
    } else {
        n.to_string()
    };

    // The pager's own palette.
    let hole_dark = hex_literal_to_color("#060504");
    let cap_face = hex_literal_to_color("#221e16");
    let cap_highlight = hex_literal_to_color("#aa9a82");
    let label_metal = hex_literal_to_color("#8a7c64");
    let arrow_metal = hex_literal_to_color("#786b56");
    let bevel_light = hex_literal_to_color("#b7a283");
    let panel_dark = hex_literal_to_color("#0b0a07");
    let black = Rgba::new(0.0, 0.0, 0.0, 1.0);

    let mut pieces = Vec::new();
    let mut p = Painting::new();

    // One recess of the selector, the window vocabulary continued: a tight
    // dark cut ring and the bright bevel line on the chassis under the
    // hole.
    let recess = |x: f64, width: f64, p: &mut Painting| {
        let r = PaintRect::new(x, 1.0, width, hole_h);
        p.rect(RectOp::gradient(
            r,
            4.0,
            vec![
                Stop::new(0.00, hex_literal_to_color("#12100b")),
                Stop::new(0.10, hole_dark),
                Stop::new(0.85, lighter(hole_dark, 1.6)),
                Stop::new(1.00, lighter(hole_dark, 2.4)),
            ],
        ));
        p.rect(
            RectOp::solid(
                PaintRect::new(r.x + 1.0, r.y + r.height + 1.0, r.width - 2.0, 2.0),
                1.0,
                bevel_light,
            )
            .at_opacity(0.7),
        );
        r
    };

    // The two keys, each a dark metal cap sunk in its recess, the label
    // engraved above a solid triangle pointing off the bank.
    for (x, label, direction, wear_seed) in [
        (prev_x, "PREV", -1.0f64, 0.31f32),
        (next_x, "NEXT", 1.0, 0.67),
    ] {
        let hole = recess(x, key_w, &mut p);
        let cap = PaintRect::new(
            hole.x + 5.0 * squeeze,
            hole.y - 1.0 + 6.0 * squeeze,
            key_w - 10.0 * squeeze,
            hole_h - 14.0 * squeeze,
        );
        pieces.push(Piece::shaded(
            cap,
            crate::furniture::PieceParams::Plate(PlateMetalParams {
                size_px: [cap.width as f32, cap.height as f32],
                light_dir: [-0.55, -0.85],
                base_color: [cap_face.r, cap_face.g, cap_face.b],
                highlight_color: [cap_highlight.r, cap_highlight.g, cap_highlight.b],
                shadow_color: rgb("#040302"),
                corner_radius: 4.0,
                bevel_px: 4.0,
                metal: MetalParams {
                    grain_amount: 0.3,
                    mottle_amount: 0.6,
                    scratch_amount: 0.4,
                },
                vignette_strength: 0.3,
                wear_amount: 0.3,
                seam_gain: 0.7,
                seed: wear_seed,
            }),
            None,
        ));

        // The machined ridge across the cap's shoulder, the brightest
        // thing on the key, with its cut shadow under it.
        let ridge_y = cap.y + (6.0 * squeeze).round();
        let ridge_h = 2.0f64.max((4.0 * squeeze).round());
        p.rect(
            RectOp::solid(
                PaintRect::new(cap.x + 3.0, ridge_y, cap.width - 6.0, ridge_h),
                1.0,
                cap_highlight,
            )
            .at_opacity(0.95),
        );
        p.rect(
            RectOp::solid(
                PaintRect::new(cap.x + 3.0, ridge_y + ridge_h, cap.width - 6.0, 1.0),
                0.0,
                black,
            )
            .at_opacity(0.45),
        );

        // The label engraved in the cap: the dark cut first, then the
        // light its lower edge catches, laid over it.
        let engrave_size = 9.0f64.max((16.0 * squeeze).round());
        let engrave_spacing = 2.0 * squeeze;
        let engrave_w = sans_width(label, engrave_size, engrave_spacing, true);
        let engrave_x = cap.x + (cap.width - engrave_w) / 2.0;
        let engrave_y = cap.y + (14.0 * squeeze).round();
        let engraved = |dy: f64, color: Rgba, opacity: f32| TextOp {
            face: Face::Sans,
            x: engrave_x,
            y: engrave_y + dy,
            width: 0.0,
            align: Align::Left,
            pixel_size: engrave_size,
            letter_spacing: engrave_spacing,
            bold: true,
            text: label.to_string(),
            color,
            opacity,
        };
        p.text(engraved(-1.0, black, 0.55));
        p.text(engraved(0.0, label_metal, 1.0));

        // The solid triangle struck under the label, pointing off the bank
        // the way its key walks the pages.
        let aw = (22.0 * squeeze).round();
        let ah = (24.0 * squeeze).round();
        let ax = cap.x + (cap.width - aw) / 2.0;
        let ay = cap.y + (40.0 * squeeze).round();
        let points = if direction < 0.0 {
            vec![(ax + aw, ay), (ax + aw, ay + ah), (ax, ay + ah / 2.0)]
        } else {
            vec![(ax, ay), (ax, ay + ah), (ax + aw, ay + ah / 2.0)]
        };
        p.polygon(PolygonOp {
            points,
            color: arrow_metal,
            opacity: 1.0,
        });
    }

    // The page window: the same recess, with the shared LED display kit
    // burning the page number behind it.
    let hole = recess(window_x, window_w, &mut p);
    let panel = PaintRect::new(hole.x + 3.0, hole.y + 3.0, window_w - 6.0, hole_h - 5.0);
    p.rect(RectOp::gradient(
        panel,
        2.0,
        vec![
            Stop::new(0.0, darker(panel_dark, 1.7)),
            Stop::new(0.5, panel_dark),
            Stop::new(0.94, lighter(panel_dark, 1.7)),
            Stop::new(1.0, lighter(panel_dark, 2.8)),
        ],
    ));
    if !p.is_empty() {
        // Each recess's bright bevel line sits one pixel *below* the hole
        // and is two pixels tall, which is the `+ 3` in this pager's own
        // implicit height. The last of those three pixels is the line's
        // own bottom edge, so the piece takes a pixel of slack rather than
        // cutting it.
        pieces.push(Piece::painted(PaintRect::new(0.0, 0.0, w, h + 1.0), p));
    }

    // Behind that panel a *second instance of the profile's whole display
    // kit* burns the page number: its own character count (2, always -- the
    // label is padded above for exactly this), its own pad overrides (none,
    // on any side), its own throttled spill, and the whole item scaled up
    // (capped at 2x) to fill the panel. A sibling piece and not more
    // painting in `p`, because it is the same shaded `LedMatrix` pass the
    // bank's own strips use, not a rectangle.
    if let Some(counter) = counter_lamps(cfg, panel, &page_label) {
        pieces.push(counter);
    }
    pieces
}

/// The page-window lamps: two characters, no side or vertical padding,
/// powered and lit but not bright, spill throttled to 0.12 (the scaled
/// lamps would wash the bezel; the page window glows quieter than a channel
/// window), the whole item scaled up to `min(2.0, (panel.width - 8) /
/// implicit_width)` and centred in the panel.
///
/// Only the LED kit is built: `chassis::display_kit` and `displays::led` are
/// the pieces this reaches, per this module's own scope. Every shipped preset
/// wearing this chassis ships the LED kit (`config::presets::chassis_presets`),
/// so a profile parked here with the tape kit chosen instead is an unshipped
/// combination; `None` leaves the window dark rather than drawing a kit this
/// pass does not carry.
///
/// `panel` is the page window's own recessed floor, in the pager's own
/// coordinates. Clipping the lamp field's spill to the panel's rounded
/// bounds is not done here: a shaded [`crate::furniture::Pass`] piece has no
/// clip of its own, only the bank's own outer one (`app::chrome`'s scissor), so the
/// grown rectangle below can spill past the panel's edges. Left as a
/// residual rather than fixed: the spill this margin would want clipped is
/// itself a fifth the strength of a channel window's own (0.12 against
/// 0.3-1.0).
fn counter_lamps(cfg: &config::Config, panel: PaintRect, page_label: &str) -> Option<Piece> {
    let crate::Display::Led(kit) = crate::display_kit(cfg) else {
        return None;
    };
    let lamp_cell_width = kit.lamp_cell_width.max(1) as u32;
    let lamp_cell_height = kit.lamp_cell_height.max(1) as u32;
    let grid = crate::displays::led::grid_size(lamp_cell_width, lamp_cell_height, 2, 0, 0, 0);
    let entry = term::fonts::font_by_name_or(
        &cfg.chassis.bank_font_name,
        crate::font_source(cfg),
        crate::displays::led::DEFAULT_LED_FONT_NAME,
    )?;
    let source = crate::furniture::led_grid(
        entry.data(),
        entry.pixel_size,
        page_label,
        lamp_cell_width,
        grid,
        0,
        0,
    );

    // The grid at the kit's dot pitch, unscaled -- what the scale below is
    // measured against and what its margins grow.
    let implicit_w = grid.0 as f64 * crate::displays::led::LED_DOT_PITCH;
    let implicit_h = grid.1 as f64 * crate::displays::led::LED_DOT_PITCH;
    let (spill_x, spill_y) = crate::displays::led::spill_margins(implicit_h);
    let (spill_x, spill_y) = (spill_x as f64, spill_y as f64);
    let grown_w = implicit_w + 2.0 * spill_x;
    let grown_h = implicit_h + 2.0 * spill_y;

    // `min(2.0, (panel.width - 8) / max(1, implicit_width))`. The scale is
    // a paint-time transform about the item's own centre; the grown
    // rectangle grows equally on every side around that same centre, so
    // scaling before or after centring it in `panel` lands the same
    // picture.
    let scale = if implicit_w > 0.0 {
        2.0f64.min((panel.width - 8.0) / implicit_w.max(1.0))
    } else {
        1.0
    };
    // The device rect the mount draws is the grown rectangle *scaled*; the
    // fraction `led_params` turns the margins into is scale-invariant, so it
    // is built from the unscaled pair (`spill_x`/`spill_y` over `grown_w`/
    // `grown_h`) and not this one.
    let rect = PaintRect::new(
        panel.x + (panel.width - grown_w * scale) / 2.0,
        panel.y + (panel.height - grown_h * scale) / 2.0,
        grown_w * scale,
        grown_h * scale,
    );

    let colors = crate::furniture::font_color(cfg);
    let colors = crate::displays::led::window_colors(colors, true, false);
    Some(Piece::shaded(
        rect,
        crate::furniture::PieceParams::Led(crate::furniture::led_params(
            grid,
            colors,
            crate::displays::led::glow(false),
            0.12,
            (spill_x, spill_y),
            (grown_w, grown_h),
        )),
        Some(source),
    ))
}

