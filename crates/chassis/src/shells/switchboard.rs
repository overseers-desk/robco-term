//! The toggle-switch appliance: near-neutral dark gunmetal, `chassis_metal`
//! alone: no plate, no rail; the row plates and pager rail (furniture built
//! elsewhere) hide most of it. Measured off `Deep-Blue.png` (1448x1086, an
//! untracked working file).

use crate::color::{darker, lighter, hex_literal_to_color, with_alpha, Rgba};
use crate::furniture::Piece;
use crate::layout::Rect as PaintRect;
use crate::params::{MetalParams, PlateMetalParams};
use crate::paint::{Align, ArcOp, Face, Painting, PolygonOp, RectOp, Stop, TextOp};

use super::common::{numeral_line_height, rgb, sans_width, CastingRecipe, FrameRecipe, Rect};

/// This shell's fixed geometry, field for field.
///
/// The crate's one home for these numbers: [`crate::metrics::shells::switchboard`]
/// composes these same constants into the `ShellMetrics` the bank reads (and
/// restates `PAGER_HEIGHT` as `SWITCHBOARD_PAGER_HEIGHT`), rather than
/// restating the numbers, and `tests/suite/metrics_homes.rs` pins that composition
/// as a regression check. A correction to any of these numbers belongs here
/// and only here.
pub mod metrics {
    /// The row plates' left edge stands at x 8; this is the bank's whole
    /// left shoulder.
    pub const BANK_PADDING: i32 = 8;
    /// Row 1's label well (bevel ring included) starts at y 34.
    pub const TOP_PADDING: i32 = 34;
    /// Chassis following the pager rail's bottom bevel (y 1065) to the
    /// mock's foot.
    pub const BOTTOM_PADDING: i32 = 21;
    /// Label plate's outer right bevel to the screen well's trough.
    pub const RIGHT_PADDING: i32 = 29;
    /// (908 - 36) / 14 well-top pitch, less the 48px row.
    pub const ROW_SPACING: f64 = 14.29;
    /// Numeral plate's right seam to the label glass.
    pub const COLUMN_GAP: i32 = 18;
    /// The folded lane: stamped numeral and the whole switch well.
    pub const NUMERAL_WIDTH: i32 = 170;
    pub const STRIP_PADDING: i32 = 2;
    pub const MIN_ROW_HEIGHT: i32 = 48;

    /// Switch well, lane coordinates (lane x 8 is well 0).
    pub const SWITCH_WELL_X: i32 = 64;
    pub const SWITCH_WELL_WIDTH: i32 = 102;
    pub const SWITCH_WELL_HEIGHT: i32 = 52;
    pub const SWITCH_WELL_RADIUS: i32 = 10;
    pub const NUMERAL_CENTER_X: i32 = 32;

    /// Pager rail across the bank's foot, mock y 963..1064.
    pub const PAGER_HEIGHT: i32 = 104;
    pub const PAGER_ARROW_WIDTH: i32 = 74;
    pub const PAGER_ARROW_HEIGHT: i32 = 82;
    /// Framed PAGE plate, mock x 118..277.
    pub const PAGE_WINDOW_WIDTH: i32 = 122;
    pub const PAGE_WINDOW_HEIGHT: i32 = 40;
    pub const PAGE_WINDOW_RADIUS: i32 = 6;

    pub const CASTING_LIGHT_DIR: [f32; 2] = [-0.4, -0.9];
    /// The plate face itself, sampled #232830 at mock (300, 26).
    pub const CASTING_COLOR: &str = "#232830";
}

/// This shell's pager measures, the three the bank's foot needs.
///
/// The only one of the three that reads its own metrics back:
/// `implicit_height = pager_height - 15`, so [`pager::NATURAL_HEIGHT`] is derived
/// from [`metrics::PAGER_HEIGHT`] here rather than restated as a literal.
/// [`crate::metrics::shells::switchboard`] composes these into
/// `ShellMetrics`.
pub mod pager {
    /// `pager_height - 15`. The rail plate keeps the mock's measured 104
    /// and stands up into the row-spacing air the bank reserves above it;
    /// the *item* (what the row-count settle subtracts) admits to 15 less.
    pub const NATURAL_HEIGHT: f64 = (super::metrics::PAGER_HEIGHT - 15) as f64;
    /// This shell states no `squeeze`; see the annunciator's for the shape.
    pub const SQUEEZE_SPAN: f64 = 0.0;
    /// Nothing is added after the (absent) squeeze.
    pub const EXTRA: i32 = 0;
}

/// `chassis_metal` covers the whole chassis item: no plate, no rail, unlike
/// the other two shells.
pub fn chassis_rect(chassis_size: (f64, f64)) -> Rect {
    Rect::new(0.0, 0.0, chassis_size.0, chassis_size.1)
}

/// The casting's fixed uniforms (`opacity` excluded as in the other two
/// shells).
pub const CASTING: CastingRecipe = CastingRecipe {
    light_dir: metrics::CASTING_LIGHT_DIR,
    color: metrics::CASTING_COLOR,
    metal: MetalParams {
        grain_amount: 0.45,
        mottle_amount: 0.7,
        scratch_amount: 0.45,
    },
    vignette_strength: 0.22,
};

/// The bezel's fixed uniforms.
pub const FRAME: FrameRecipe = FrameRecipe {
    light_dir: metrics::CASTING_LIGHT_DIR,
    bezel_color: "#20242b",
    chassis_color: metrics::CASTING_COLOR,
    ridge_color: "#737a83",
    bezel_margins: [0.0, 6.0, 10.0, 10.0],
    outer_radius: 26.0,
    well_depth: 30.0,
    well_floor: 0.18,
    ridge_gain: 0.4,
    metal: MetalParams {
        grain_amount: 0.35,
        mottle_amount: 0.8,
        scratch_amount: 0.45,
    },
    vignette_strength: 0.4,
    fill_gain: 0.35,
    trough_gain: 0.0,
    // Same face-band law as the slide rule's, shallower well.
    face_band_px: 10.0,
    rim_dist_px: 12.0,
    rim_gain: 1.3,
};

/// The row's fixed colours, each a hex colour literal read at /255 except
/// the two derived from the plastic and the one that is the profile's own
/// phosphor.
mod row {
    pub const PLATE_HIGHLIGHT: &str = "#99a1ac";
    pub const PLATE_SHADOW: &str = "#040507";
    pub const NUMERAL_PAINT: &str = "#cfc4ba";
    pub const WELL_DARK: &str = "#0a0c10";
    /// How far the plates stand past the row band, into the seam.
    pub const PLATE_REACH: f64 = 4.0;
    /// The pixel size struck on all three stamped text items.
    pub const NUMERAL_PIXEL_SIZE: f64 = 38.0;
    pub const NUMERAL_LETTER_SPACING: f64 = -2.0;
}

/// A riveted plate carrying the stamped numeral and the toggle well, the
/// lever laid over it at rest, and the raised moulding around the label
/// well the display kit lays its tape in.
///
/// Two pieces, because the plate under it all is a `plate_metal` pass and
/// everything on top of it is painting.
///
/// **The lever's throw is not here.** The swing is a rotating item with a
/// throw animation and an easing overshoot, and nothing animates it. What
/// this draws is the lever
/// at rest (`current: false`'s pose, which is what a still snap shows):
/// plate, rivets, numeral, well, the lit floor the thrown lever would
/// uncover, the pivot socket, the bevel line, the spill, and now the cap
/// lying flat over the well's left with its drop shadow, front face,
/// machined chamfer, lit top sliver, cut outline and retaining screw. Left
/// out of the cap itself: seeded scratch-and-grime noise (ninety strokes
/// and ten blobs per row) has no primitive in this stack (`crate::paint`'s
/// module doc: gradients, text, one arc), so the face reads as clean metal
/// rather than as worn.
pub fn row_furniture(
    plastic: Rgba,
    numeral: &str,
    row: (f64, f64),
    current: bool,
    glow: Rgba,
) -> Vec<Piece> {
    let (row_w, row_h) = row;
    if row_w <= 0.0 || row_h <= 0.0 {
        return Vec::new();
    }
    // Each plate weathers on its own seed, keyed off the stamped numeral,
    // so no two rows wear alike: the numeral parsed as a number, `* 0.137`,
    // then taken modulo 1 for the fraction.
    let seed = ((numeral.parse::<f64>().unwrap_or(0.0) * 0.137) % 1.0) as f32;

    let plate_face = lighter(plastic, 1.4);
    let plate_rect = PaintRect::new(
        0.0,
        -row::PLATE_REACH,
        metrics::NUMERAL_WIDTH as f64,
        row_h + 2.0 * row::PLATE_REACH,
    );

    // The switch plate, `plate_metal` over the folded lane.
    let mut pieces = vec![Piece::shaded(
        plate_rect,
        crate::furniture::PieceParams::Plate(PlateMetalParams {
            size_px: [plate_rect.width as f32, plate_rect.height as f32],
            light_dir: metrics::CASTING_LIGHT_DIR,
            base_color: [plate_face.r, plate_face.g, plate_face.b],
            highlight_color: rgb(row::PLATE_HIGHLIGHT),
            shadow_color: rgb(row::PLATE_SHADOW),
            corner_radius: 5.0,
            bevel_px: 2.0,
            metal: MetalParams {
                grain_amount: 0.35,
                mottle_amount: 0.7,
                scratch_amount: 0.5,
            },
            vignette_strength: 0.3,
            wear_amount: 0.4,
            seam_gain: 0.6,
            seed,
        }),
        None,
    )];

    let mut p = Painting::new();

    // The plate's four corner rivets, small domes with a glint on the side
    // the key light lands. They are children of the plate, so their
    // coordinates are the plate's.
    let rivet = |x: f64, y: f64| {
        RectOp::gradient(
            PaintRect::new(x, plate_rect.y + y, 5.0, 5.0),
            2.5,
            vec![
                Stop::new(0.0, hex_literal_to_color("#6d747e")),
                Stop::new(0.55, hex_literal_to_color("#2c3037")),
                Stop::new(1.0, hex_literal_to_color("#0a0b0e")),
            ],
        )
    };
    p.rect(rivet(5.0, 5.0));
    p.rect(rivet(plate_rect.width - 10.0, 5.0));
    p.rect(rivet(5.0, plate_rect.height - 10.0));
    p.rect(rivet(plate_rect.width - 10.0, plate_rect.height - 10.0));

    // The stamped numeral, centred in its own lane. Raised paint catching
    // the light on its face, its strike shadow thrown down and right, and
    // the paint struck twice a hair apart because the mock's stencil
    // strokes are heavier than any weight this face carries.
    let lane_x = 6.0;
    let lane_w = (metrics::NUMERAL_CENTER_X as f64 - 8.0) * 2.0 + 4.0;
    if !numeral.is_empty() {
        let line_h = numeral_line_height(row::NUMERAL_PIXEL_SIZE);
        let lane_y = (row_h - line_h) / 2.0;
        let paint = hex_literal_to_color(row::NUMERAL_PAINT);
        let stamped = |dx: f64, dy: f64, color: Rgba, opacity: f32| TextOp {
            face: Face::Catalogue("IOSEVKA"),
            x: lane_x + dx,
            y: lane_y + dy,
            width: lane_w,
            align: Align::Center,
            pixel_size: row::NUMERAL_PIXEL_SIZE,
            letter_spacing: row::NUMERAL_LETTER_SPACING,
            bold: true,
            text: numeral.to_string(),
            color,
            opacity,
        };
        p.text(stamped(1.0, 2.0, hex_literal_to_color("#05060a"), 0.75));
        p.text(stamped(0.0, 0.0, paint, 1.0));
        p.text(stamped(0.8, 0.0, paint, 1.0));
    }

    // The switch well and everything sunk in it.
    let well = PaintRect::new(
        metrics::SWITCH_WELL_X as f64,
        (row_h - metrics::SWITCH_WELL_HEIGHT as f64) / 2.0,
        metrics::SWITCH_WELL_WIDTH as f64,
        metrics::SWITCH_WELL_HEIGHT as f64,
    );
    let radius = metrics::SWITCH_WELL_RADIUS as f64;
    let well_dark = hex_literal_to_color(row::WELL_DARK);
    p.rect(RectOp::gradient(
        well,
        radius,
        vec![
            Stop::new(0.00, darker(well_dark, 1.8)),
            Stop::new(0.50, well_dark),
            Stop::new(0.88, lighter(well_dark, 1.9)),
            Stop::new(1.00, lighter(well_dark, 3.2)),
        ],
    ));
    // The top lip's shadow down the near wall, and the left wall falling
    // dark with the key leaning that way. Both are clipped to the well.
    let clear = Rgba::new(0.0, 0.0, 0.0, 0.0);
    let shade = |a: f32| Rgba::new(0.0, 0.0, 0.0, a);
    p.rect(
        RectOp::gradient(
            PaintRect::new(well.x, well.y, well.width, 6.0),
            0.0,
            vec![Stop::new(0.0, shade(0.8)), Stop::new(1.0, clear)],
        )
        .clipped_to(well, radius),
    );
    p.rect(
        RectOp::horizontal_gradient(
            PaintRect::new(well.x, well.y, 6.0, well.height),
            0.0,
            vec![Stop::new(0.0, shade(0.6)), Stop::new(1.0, clear)],
        )
        .clipped_to(well, radius),
    );
    // The lit floor the thrown lever uncovers, the profile's own phosphor
    // flooding the well's right side.
    if current {
        p.rect(
            RectOp::horizontal_gradient(
                PaintRect::new(
                    well.x + 56.0,
                    well.y + 2.0,
                    well.width - 56.0 - 2.0,
                    well.height - 4.0,
                ),
                radius - 2.0,
                vec![
                    Stop::new(0.00, lighter(glow, 1.5)),
                    Stop::new(0.14, glow),
                    Stop::new(0.45, darker(glow, 2.6)),
                    Stop::new(0.85, darker(glow, 6.0)),
                    Stop::new(1.00, darker(glow, 9.0)),
                ],
            )
            .clipped_to(well, radius),
        );
    }
    // The pivot screw's socket, its rim taking the glow when the floor
    // lights.
    let socket = PaintRect::new(
        well.x + 78.0,
        well.y + (well.height - 15.0) / 2.0,
        15.0,
        15.0,
    );
    p.rect(
        RectOp {
            border: Some((
                2.0,
                if current {
                    lighter(glow, 1.6)
                } else {
                    hex_literal_to_color("#3a4048")
                },
            )),
            ..RectOp::solid(socket, 7.5, hex_literal_to_color("#0c0e12"))
        }
        .clipped_to(well, radius),
    );
    p.rect(
        RectOp::solid(
            PaintRect::new(socket.x + 5.0, socket.y + 5.0, 5.0, 5.0),
            2.5,
            if current {
                hex_literal_to_color("#ffffff")
            } else {
                hex_literal_to_color("#20242a")
            },
        )
        .clipped_to(well, radius),
    );

    // The bright bevel line on the plate just under the well's bottom lip.
    p.rect(
        RectOp::solid(
            PaintRect::new(
                well.x + 3.0,
                well.y + well.height + 1.0,
                well.width - 6.0,
                2.0,
            ),
            1.0,
            hex_literal_to_color("#79818c"),
        )
        .at_opacity(0.45),
    );
    // The soft spill the lit well throws past its own lips.
    if current {
        p.rect(
            RectOp::solid(
                PaintRect::new(
                    well.x - 13.0,
                    well.y - 13.0,
                    well.width + 26.0,
                    well.height + 26.0,
                ),
                radius + 13.0,
                with_alpha(glow, 1.0),
            )
            .at_opacity(0.06),
        );
    }

    // The lever, a heavy dark cap laid over the well's left. At rest
    // (`current: false`) it sits at `well.x + 3`, unrotated -- the pivot
    // that steers the throw's rotation is immaterial here.
    let lever_w = 74.0;
    let lever_h = 54.0;
    let lever = PaintRect::new(well.x + 3.0, (row_h - lever_h) / 2.0, lever_w, lever_h);
    let cap_face = hex_literal_to_color("#343a41");
    let cap_chamfer = hex_literal_to_color("#79818c");

    // The cap's drop shadow into the well.
    p.rect(RectOp::solid(
        PaintRect::new(lever.x + 6.0, lever.y + 8.0, 62.0, 44.0),
        6.0,
        Rgba::new(0.0, 0.0, 0.0, 0.55),
    ));

    // The front face, a dark slab lit faintly from the upper left.
    let face = PaintRect::new(lever.x + 2.0, lever.y + 4.0, lever_w - 16.0, lever_h - 10.0);
    p.rect(RectOp::gradient(
        face,
        6.0,
        vec![
            Stop::new(0.0, lighter(cap_face, 1.35)),
            Stop::new(0.5, cap_face),
            Stop::new(1.0, darker(cap_face, 1.5)),
        ],
    ));

    // The machined chamfer down the cap's right side, the brightest metal
    // on the row at rest. `PolygonOp` carries a flat colour only
    // (`crate::paint`'s module doc), so this is the ramp's own 0.45 stop --
    // `cap_chamfer` unshifted -- standing for the whole gradient.
    p.polygon(PolygonOp {
        points: vec![
            (lever.x + lever_w - 16.0, lever.y + 4.0),
            (lever.x + lever_w - 4.0, lever.y + 10.0),
            (lever.x + lever_w - 4.0, lever.y + lever_h - 12.0),
            (lever.x + lever_w - 16.0, lever.y + lever_h - 6.0),
        ],
        color: cap_chamfer,
        opacity: 1.0,
    });

    // The lit sliver along the cap's top edge, a hairline rectangle.
    p.rect(RectOp::solid(
        PaintRect::new(
            lever.x + 6.0,
            lever.y + 4.5 - 0.75,
            lever_w - 15.0 - 6.0,
            1.5,
        ),
        0.0,
        crate::color::rgba(0.42, 0.46, 0.52, 0.9),
    ));

    // The cut shadow closing the cap's outline, stroked over the face
    // rather than filled -- a border on a fill nobody sees.
    p.rect(RectOp {
        border: Some((1.5, crate::color::rgba(0.0, 0.0, 0.0, 0.6))),
        ..RectOp::solid(face, 6.0, Rgba::new(0.0, 0.0, 0.0, 0.0))
    });

    // The retaining screw's recess in the cap's left half, a dark disc
    // with a lit arc on the side the key light lands.
    let screw = (lever.x + 16.0, lever.y + lever_h / 2.0 - 4.0);
    p.rect(RectOp::solid(
        PaintRect::new(screw.0 - 4.5, screw.1 - 4.5, 9.0, 9.0),
        4.5,
        crate::color::rgba(0.0, 0.0, 0.0, 0.6),
    ));
    p.arc(ArcOp {
        center: screw,
        radius: 4.5,
        line_width: 1.2,
        start: std::f64::consts::PI * 0.15,
        end: std::f64::consts::PI * 0.85,
        color: crate::color::rgba(0.5, 0.54, 0.6, 0.7),
    });

    // The chamfer catching the lit well when the lever is thrown -- absent
    // at rest, so nothing to paint unless the row is current.
    if current {
        p.rect(
            RectOp::solid(
                PaintRect::new(lever.x + lever_w - 9.0, lever.y + 8.0, 5.0, lever_h - 20.0),
                2.0,
                lighter(glow, 1.35),
            )
            .at_opacity(0.55),
        );
    }

    if !p.is_empty() {
        pieces.push(Piece::painted(PaintRect::new(0.0, 0.0, row_w, row_h), p));
    }
    pieces
}

/// The rail across the bank's foot: two square arrow keys flanking the
/// framed PAGE plate, all riveted onto one raised plate, with the page
/// count on mechanical rolls behind the counter window.
pub fn pager(plastic: Rgba, size: (f64, f64), page_index: i32, page_count: i32) -> Vec<Piece> {
    let (w, h) = size;
    if w <= 0.0 || h <= 0.0 {
        return Vec::new();
    }
    // The group's natural span; narrower content squeezes it all.
    let squeeze = 1.0f64.min(w / 381.0);
    let key_w = metrics::PAGER_ARROW_WIDTH as f64 * squeeze;
    let key_h = metrics::PAGER_ARROW_HEIGHT as f64 * squeeze;
    let plate_w = 159.0 * squeeze;
    let plate_h = 90.0 * squeeze;
    let prev_x = 20.0 * squeeze;
    let next_x = w - 20.0 * squeeze - key_w;
    let plate_x = (w - plate_w) / 2.0;
    // Two digits a side always, as the counter's rolls are painted.
    let pad = |n: i32| {
        if n < 10 {
            format!("0{n}")
        } else {
            n.to_string()
        }
    };
    let label = format!("{}/{}", pad(page_index + 1), pad(page_count));

    // The plate faces, both lifted off the plastic.
    let plate_face = lighter(plastic, 1.18);
    let key_face = lighter(plastic, 2.0);
    let highlight = rgb(row::PLATE_HIGHLIGHT);
    let shadow = rgb(row::PLATE_SHADOW);
    let engrave_ink = hex_literal_to_color("#101318");
    let engrave_light = hex_literal_to_color("#8d949e");
    let roll_dark = hex_literal_to_color("#0b0c0e");
    let digit_paint = hex_literal_to_color("#c6c6c4");

    let mut pieces = Vec::new();
    let plate = |rect: PaintRect,
                 base: Rgba,
                 radius: f32,
                 bevel: f32,
                 mottle: f32,
                 wear: f32,
                 seam: f32,
                 seed: f32| {
        Piece::shaded(
            rect,
            crate::furniture::PieceParams::Plate(PlateMetalParams {
                size_px: [rect.width as f32, rect.height as f32],
                light_dir: metrics::CASTING_LIGHT_DIR,
                base_color: [base.r, base.g, base.b],
                highlight_color: highlight,
                shadow_color: shadow,
                corner_radius: radius,
                bevel_px: bevel,
                metal: MetalParams {
                    grain_amount: 0.35,
                    mottle_amount: mottle,
                    scratch_amount: 0.5,
                },
                vignette_strength: if radius > 7.5 { 0.35 } else { 0.3 },
                wear_amount: wear,
                seam_gain: seam,
                seed,
            }),
            None,
        )
    };

    // The rail plate, which keeps the mock's measured height and stands up
    // into the air the bank reserved above this item.
    let rail_h = metrics::PAGER_HEIGHT as f64 - 3.0;
    let rail = PaintRect::new(0.0, h - rail_h - 3.0, w, rail_h);
    pieces.push(plate(rail, plate_face, 8.0, 3.0, 0.7, 0.45, 0.6, 0.23));

    let mut chrome = Painting::new();
    // The rail's four corner screws, in the smaller steel the switchboard
    // bolts everything with.
    let steel = SwitchboardScrew {
        metal_light: "#8b929c",
        metal_mid: "#3a3f46",
        metal_dark: "#0a0b0e",
        glint: "#d8dde4",
        light: metrics::CASTING_LIGHT_DIR,
    };
    for (x, y, angle) in [
        (3.0, 3.0, 47.0),
        (rail.width - 16.0, 3.0, -21.0),
        (3.0, rail.height - 16.0, 68.0),
        (rail.width - 16.0, rail.height - 16.0, -74.0),
    ] {
        pieces.push(Piece::painted(
            PaintRect::new(rail.x + x, rail.y + y, 13.0, 13.0),
            steel.head(13.0, angle),
        ));
    }
    // The rail's bright bevel line on the chassis under it.
    chrome.rect(
        RectOp::solid(
            PaintRect::new(2.0, h - 2.0, w - 4.0, 2.0),
            1.0,
            hex_literal_to_color("#79818c"),
        )
        .at_opacity(0.5),
    );

    // The two arrow keys, each a raised cap on the rail with four screws
    // and a heavy solid arrow engraved dark into its face.
    for (x, direction, wear_seed) in [(prev_x, -1.0f64, 0.31f32), (next_x, 1.0, 0.67)] {
        let cap = PaintRect::new(x, rail.y + (rail.height - key_h) / 2.0, key_w, key_h);
        pieces.push(plate(cap, key_face, 7.0, 3.0, 0.65, 0.45, 0.7, wear_seed));
        let seed = wear_seed as f64;
        for (sx, sy, angle) in [
            (2.0, 2.0, 12.0 + seed * 90.0),
            (cap.width - 13.0, 2.0, -40.0 - seed * 70.0),
            (2.0, cap.height - 13.0, 77.0 - seed * 50.0),
            (cap.width - 13.0, cap.height - 13.0, -8.0 + seed * 60.0),
        ] {
            pieces.push(Piece::painted(
                PaintRect::new(cap.x + sx, cap.y + sy, 11.0, 11.0),
                steel.head(11.0, angle),
            ));
        }
        // The engraved arrow, its lit lower edge struck first and the ink
        // laid over it.
        let aw = (54.0 * squeeze).round();
        let ah = (40.0 * squeeze).round();
        let ax = cap.x + (cap.width - aw) / 2.0;
        let ay = cap.y + (cap.height - ah) / 2.0;
        let points = arrow_outline(aw, ah, direction < 0.0);
        let shift = |dx: f64, dy: f64| -> Vec<(f64, f64)> {
            points
                .iter()
                .map(|(px, py)| (ax + px + dx, ay + py + dy))
                .collect()
        };
        chrome.polygon(PolygonOp {
            points: shift(0.8, 1.2),
            color: crate::color::rgba(0.55, 0.58, 0.63, 0.55),
            opacity: 1.0,
        });
        chrome.polygon(PolygonOp {
            points: shift(0.0, 0.0),
            color: engrave_ink,
            opacity: 1.0,
        });
    }

    // The framed PAGE plate.
    let page_plate = PaintRect::new(
        plate_x,
        rail.y + (rail.height - plate_h) / 2.0,
        plate_w,
        plate_h,
    );
    pieces.push(plate(page_plate, key_face, 7.0, 3.0, 0.65, 0.4, 0.7, 0.53));
    for (sx, sy, angle) in [
        (2.0, 2.0, 33.0),
        (page_plate.width - 13.0, 2.0, -66.0),
        (2.0, page_plate.height - 13.0, 59.0),
        (page_plate.width - 13.0, page_plate.height - 13.0, -27.0),
    ] {
        pieces.push(Piece::painted(
            PaintRect::new(page_plate.x + sx, page_plate.y + sy, 11.0, 11.0),
            steel.head(11.0, angle),
        ));
    }
    // "PAGE", engraved, the light its lower edge catches laid under the
    // dark cut.
    let engrave_size = 9.0f64.max((21.0 * squeeze).round());
    let engrave_spacing = 5.0 * squeeze;
    let engrave_w = sans_width("PAGE", engrave_size, engrave_spacing, true);
    let engrave_x = page_plate.x + (page_plate.width - engrave_w) / 2.0;
    let engrave_y = page_plate.y + (8.0 * squeeze).round();
    let engraved = |dx: f64, dy: f64, color: Rgba, opacity: f32| TextOp {
        face: Face::Sans,
        x: engrave_x + dx,
        y: engrave_y + dy,
        width: 0.0,
        align: Align::Left,
        pixel_size: engrave_size,
        letter_spacing: engrave_spacing,
        bold: true,
        text: "PAGE".to_string(),
        color,
        opacity,
    };
    chrome.text(engraved(0.5, 1.0, engrave_light, 0.6));
    chrome.text(engraved(0.0, 0.0, engrave_ink, 1.0));

    // The counter window: a bevel ring dropping to the rolls, and one roll
    // per character of the label.
    let ring_w = metrics::PAGE_WINDOW_WIDTH as f64 * squeeze + 6.0;
    let ring_h = metrics::PAGE_WINDOW_HEIGHT as f64 * squeeze + 6.0;
    let ring = PaintRect::new(
        page_plate.x + (page_plate.width - ring_w) / 2.0,
        page_plate.y + (34.0 * squeeze).round(),
        ring_w,
        ring_h,
    );
    let window_radius = metrics::PAGE_WINDOW_RADIUS as f64;
    chrome.rect(RectOp::solid(
        ring,
        window_radius + 2.0,
        hex_literal_to_color("#0b0d10"),
    ));
    let rolls = PaintRect::new(
        ring.x + 3.0,
        ring.y + 3.0,
        ring.width - 6.0,
        ring.height - 6.0,
    );
    chrome.rect(RectOp::solid(
        rolls,
        window_radius,
        Rgba::new(0.0, 0.0, 0.0, 1.0),
    ));
    let count = label.chars().count() as f64;
    let roll_w = (rolls.width - (count - 1.0)) / count;
    for (i, ch) in label.chars().enumerate() {
        let rx = rolls.x + i as f64 * (roll_w + 1.0);
        chrome.rect(
            RectOp::gradient(
                PaintRect::new(rx, rolls.y, roll_w, rolls.height),
                0.0,
                vec![
                    Stop::new(0.00, darker(roll_dark, 1.8)),
                    Stop::new(0.30, lighter(roll_dark, 2.6)),
                    Stop::new(0.55, lighter(roll_dark, 3.0)),
                    Stop::new(1.00, darker(roll_dark, 1.6)),
                ],
            )
            .clipped_to(rolls, window_radius),
        );
        // One painted character on its drum, in a serif face: the only
        // place in the three shells that names one.
        let digit = ch.to_string();
        let size = 10.0f64.max((30.0 * squeeze).round());
        let dw = serif_width(&digit, size, true);
        let dh = serif_line_height(size);
        chrome.text(TextOp {
            face: Face::Serif,
            x: rx + (roll_w - dw) / 2.0,
            y: rolls.y + (rolls.height - dh) / 2.0,
            width: 0.0,
            align: Align::Left,
            pixel_size: size,
            letter_spacing: 0.0,
            bold: true,
            text: digit,
            color: digit_paint,
            opacity: 1.0,
        });
    }
    // The window lip's shadow over the top of the rolls.
    chrome.rect(
        RectOp::gradient(
            PaintRect::new(rolls.x, rolls.y, rolls.width, 5.0),
            0.0,
            vec![
                Stop::new(0.0, Rgba::new(0.0, 0.0, 0.0, 0.7)),
                Stop::new(1.0, Rgba::new(0.0, 0.0, 0.0, 0.0)),
            ],
        )
        .clipped_to(rolls, window_radius),
    );

    if !chrome.is_empty() {
        // The rail stands up into the air the bank reserved above this
        // item, so its negative `y` is the top of everything riveted to
        // it: the "PAGE" engraving above all. The piece's rectangle is
        // that whole band and not the item's own, or the engraving is cut
        // off at the item's top edge.
        let top = rail.y.min(0.0);
        pieces.push(Piece::painted(
            PaintRect::new(0.0, top, w, h - top),
            chrome.translated(0.0, -top),
        ));
    }
    pieces
}

/// The solid arrow with a stem, mirrored for NEXT.
fn arrow_outline(w: f64, h: f64, left: bool) -> Vec<(f64, f64)> {
    let stem = h * 0.36;
    let px = |x: f64| if left { x } else { w - x };
    vec![
        (px(0.0), h / 2.0),
        (px(w * 0.42), 0.0),
        (px(w * 0.42), (h - stem) / 2.0),
        (px(w), (h - stem) / 2.0),
        (px(w), (h + stem) / 2.0),
        (px(w * 0.42), (h + stem) / 2.0),
        (px(w * 0.42), h),
    ]
}

/// The switchboard's own screw palette: every screw head on this shell
/// overrides the kit's four metals and its light direction with these, and
/// there are sixteen of them across the pager alone.
struct SwitchboardScrew {
    metal_light: &'static str,
    metal_mid: &'static str,
    metal_dark: &'static str,
    glint: &'static str,
    light: [f32; 2],
}

impl SwitchboardScrew {
    fn head(&self, size: f64, slot_angle: f64) -> Painting {
        super::common::screw_head_with(
            size,
            slot_angle,
            super::common::ScrewColors {
                metal_light: hex_literal_to_color(self.metal_light),
                metal_mid: hex_literal_to_color(self.metal_mid),
                metal_dark: hex_literal_to_color(self.metal_dark),
                glint: hex_literal_to_color(self.glint),
            },
            (self.light[0] as f64, self.light[1] as f64),
        )
    }
}


fn serif_width(text: &str, pixel_size: f64, bold: bool) -> f64 {
    let Some(data) = term::fonts::system::default_serif() else {
        return 0.0;
    };
    term::fonts::text::natural_width(&term::fonts::text::TextSpec {
        data,
        pixel_size: pixel_size as u32,
        text,
        letter_spacing: 0.0,
        bold,
    })
}

fn serif_line_height(pixel_size: f64) -> f64 {
    term::fonts::system::default_serif()
        .and_then(|d| term::fonts::metrics::scaled_metrics(d, pixel_size as u32))
        .map(|m| m.height())
        .unwrap_or(pixel_size)
}

