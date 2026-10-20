//! The amber appliance: furniture cut close at the sides, windows at a
//! 45px pitch, a raised patinated bank plate (`plate_metal`) screwed over
//! the chassis metal (`chassis_metal`) continuing in from the frame
//! (`frame_metal`).

use crate::color::{darker, lighter, hex_literal_to_color, Rgba};
use crate::layout::Rect as PaintRect;
use crate::params::{MetalParams, PlateMetalParams};
use crate::paint::{Align, Face, Painting, RectOp, Stop, TextOp};

use super::common::{
    jitter, numeral_line_height, rgb, sans_width, CastingRecipe, FrameRecipe, Rect,
};

/// This shell's fixed geometry, field for field.
///
/// The crate's one home for these numbers: [`crate::metrics::shells::annunciator`]
/// composes these same constants into the `ShellMetrics` the bank reads,
/// rather than restating them, and `tests/suite/metrics_homes.rs` pins that
/// composition as a regression check. A correction to any of these numbers
/// belongs here and only here.
pub mod metrics {
    pub const BANK_PADDING: i32 = 3;
    /// The plate's headroom above row 1 (screws and blank metal).
    pub const TOP_PADDING: i32 = 61;
    pub const BOTTOM_PADDING: i32 = 8;
    /// A sliver of plate past the window bezels' right lip.
    pub const RIGHT_PADDING: i32 = 14;
    /// Window outer bezels nearly touch (43px bezel, 2px air).
    pub const ROW_SPACING: f64 = 2.0;
    pub const COLUMN_GAP: i32 = 16;
    /// Right-aligned digit column, holds them off the plate edge.
    pub const NUMERAL_WIDTH: i32 = 46;
    pub const STRIP_PADDING: i32 = 4;
    /// Pins the window pitch at 45 (43 + `ROW_SPACING`).
    pub const MIN_ROW_HEIGHT: i32 = 43;
    /// No selector lane on this shell, but the field is always present in
    /// the contract.
    pub const TRACK_WIDTH: i32 = 14;
    pub const CASTING_LIGHT_DIR: [f32; 2] = [0.8, -0.6];
    pub const CASTING_COLOR: &str = "#16130f";
}

/// This shell's pager measures, the three the bank's foot needs.
///
/// A second concern gets a second home: the metrics above say nothing about
/// the pager, and the bank's foot reads the pager's height off the loaded
/// item, so the height is this file's word.
/// [`crate::metrics::shells::annunciator`] composes these into `ShellMetrics`
/// without restating them.
pub mod pager {
    /// A flat 130. The key tops sit 38px into the block and the plate runs
    /// on below them, but the item's own height is the constant.
    pub const NATURAL_HEIGHT: f64 = 130.0;
    /// This shell states no `squeeze`: every measure in it is a constant, so
    /// a narrow lane shrinks nothing. Zero is the composed shape's way of
    /// saying "reads no width".
    pub const SQUEEZE_SPAN: f64 = 0.0;
    /// Nothing is added after the (absent) squeeze.
    pub const EXTRA: i32 = 0;
}

/// The `chassis_metal` region covers the whole chassis item, no margin of
/// its own.
pub fn chassis_rect(chassis_size: (f64, f64)) -> Rect {
    Rect::new(0.0, 0.0, chassis_size.0, chassis_size.1)
}

/// The raised bank plate: 8px left margin, 2px top margin, no right margin,
/// 8px bottom margin, sitting proud of the chassis, cut close at the right
/// where the frame's own moulding takes over.
pub fn plate_rect(chassis_size: (f64, f64)) -> Rect {
    let (w, h) = chassis_size;
    Rect::new(8.0, 2.0, (w - 8.0).max(0.0), (h - 2.0 - 8.0).max(0.0))
}

/// The casting's fixed uniforms: everything but `opacity`, which is the
/// profile's window-opacity setting and not part of this shell's fixed
/// contract.
pub const CASTING: CastingRecipe = CastingRecipe {
    light_dir: metrics::CASTING_LIGHT_DIR,
    color: metrics::CASTING_COLOR,
    metal: MetalParams {
        grain_amount: 0.16,
        mottle_amount: 0.4,
        scratch_amount: 0.08,
    },
    vignette_strength: 0.42,
};

/// The raised plate's fixed uniforms. `size_px` is filled in by the caller
/// from [`plate_rect`]'s own size, since it follows the item's own
/// width/height rather than a fixed constant.
pub fn plate_metal_params(size_px: [f32; 2]) -> PlateMetalParams {
    PlateMetalParams {
        size_px,
        light_dir: [-0.22, -0.98],
        base_color: rgb("#2b241c"),
        highlight_color: rgb("#c1a585"),
        shadow_color: rgb("#0e0905"),
        corner_radius: 6.0,
        bevel_px: 2.5,
        metal: MetalParams {
            grain_amount: 0.3,
            mottle_amount: 1.0,
            scratch_amount: 0.5,
        },
        vignette_strength: 0.42,
        wear_amount: 0.7,
        seam_gain: 1.0,
        seed: 0.17,
    }
}

/// The bezel's fixed uniforms, the half of `frame_metal`'s block the
/// settings do not supply.
pub const FRAME: FrameRecipe = FrameRecipe {
    light_dir: metrics::CASTING_LIGHT_DIR,
    bezel_color: "#26211c",
    chassis_color: metrics::CASTING_COLOR,
    ridge_color: "#6e5c48",
    bezel_margins: [0.0, 12.0, 10.0, 9.0],
    outer_radius: 60.0,
    well_depth: 9.0,
    well_floor: 0.4,
    ridge_gain: 0.7,
    metal: MetalParams {
        grain_amount: 0.16,
        mottle_amount: 0.5,
        scratch_amount: 0.15,
    },
    vignette_strength: 0.42,
    fill_gain: 0.95,
    trough_gain: 0.35,
    // The face-band law is switched off for this shell: declared as zeros,
    // not left unset -- every uniform the shader names gets a value here.
    face_band_px: 0.0,
    rim_dist_px: 0.0,
    rim_gain: 0.0,
};

/// The four slotted screws, as `(rectangle, slot angle in degrees)` in the
/// chassis item's own coordinates.
///
/// Anchored rather than pinned by number, so a narrower window keeps its
/// screws: the left pair is pinned to the plate's left at a fixed x, the
/// right pair to the chassis's right edge at a 12 and a 14 pixel margin, and
/// the lower pair 25 pixels off the bottom. The mock's own centres are
/// (32,30), (317,29) and 39px above the bottom edge; the numbers below are
/// those centres less the item's own half-width.
pub fn screw_places(chassis_size: (f64, f64)) -> Vec<(PaintRect, f64)> {
    const SIZE: f64 = 28.0;
    let (w, h) = chassis_size;
    let place = |x: f64, y: f64, angle: f64| (PaintRect::new(x, y, SIZE, SIZE), angle);
    vec![
        place(32.0 - 14.0, 30.0 - 14.0, 24.0),
        place(w - 12.0 - SIZE, 29.0 - 14.0, -49.0),
        place(32.0 - 14.0, h - 25.0 - SIZE, 78.0),
        place(w - 14.0 - SIZE, h - 25.0 - SIZE, -11.0),
    ]
}

/// The fixed colours of one window's furniture, each a hex colour literal
/// read at /255.
mod row {
    pub const NUMERAL_FILL: &str = "#9c8168";
    pub const NUMERAL_SHADOW: &str = "#0f0d0c";
    pub const PANEL_DARK: &str = "#0d0700";
    pub const LIP_LIGHT: &str = "#7a6448";
    pub const CUT_DARK: &str = "#060402";
    /// The pixel size struck on both text items.
    pub const NUMERAL_PIXEL_SIZE: f64 = 34.0;
    /// The letter spacing struck on both text items, in absolute (not
    /// per-em) units.
    pub const NUMERAL_LETTER_SPACING: f64 = -1.0;
}

/// One row's numeral, its window's raised rim and the punched hole the
/// lamps sit in.
///
/// `row` is the row's own size, which is the bank's content lane by the
/// shell's row height; `display_rect` is the window's rectangle, in that
/// same row's coordinates, so this painting is placed at the row's own
/// rectangle and nothing inside it needs to know where the bank sits.
///
/// A struck numeral, then the rim over the plate, then the hole: this draw
/// order matters where the rim's lit lower lip runs under the hole's own
/// bottom edge.
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
    // One hash per window, so the mill's variation is a fact about the
    // numeral rather than a random number that would flicker.
    let j = jitter(numeral) as f32;

    // The numeral lane is as wide as the plate left of the window less the
    // shell's gap, its height the line's own, centred in the row; the two
    // text strikes are right-aligned in it, the shadow struck two pixels
    // down and right of the lit face.
    let lane_w = display_rect.x - metrics::COLUMN_GAP as f64;
    if lane_w > 0.0 && !numeral.is_empty() {
        let line_h = numeral_line_height(row::NUMERAL_PIXEL_SIZE);
        let lane_y = (row_h - line_h) / 2.0;
        let struck = |dx: f64, dy: f64, color: Rgba, opacity: f32| TextOp {
            face: Face::Catalogue("IOSEVKA"),
            x: dx,
            y: lane_y + dy,
            width: lane_w,
            align: Align::Right,
            pixel_size: row::NUMERAL_PIXEL_SIZE,
            letter_spacing: row::NUMERAL_LETTER_SPACING,
            bold: false,
            text: numeral.to_string(),
            color,
            opacity,
        };
        // The shadow the lit figure throws low and right.
        p.text(struck(2.0, 2.0, hex_literal_to_color(row::NUMERAL_SHADOW), 0.9));
        // The strike itself, worn a few percent by the jitter.
        p.text(struck(
            0.0,
            0.0,
            hex_literal_to_color(row::NUMERAL_FILL),
            0.88 + 0.08 * j,
        ));
    }

    // The raised outer rim, a thin moulding standing proud of the plate,
    // its top edge lit and dropping to a dark cut inside.
    let rim = PaintRect::new(display_rect.x - 10.0, 0.0, display_rect.width + 19.0, row_h);
    p.rect(RectOp::gradient(
        rim,
        8.0,
        vec![
            Stop::new(0.00, lighter(plastic, 1.9 + 0.2 * j)),
            Stop::new(0.06, lighter(plastic, 1.15)),
            Stop::new(0.30, darker(plastic, 1.25)),
            Stop::new(0.90, darker(plastic, 1.7)),
            Stop::new(1.00, lighter(plastic, 1.5)),
        ],
    ));
    // The dark cut on the upper lip...
    p.rect(
        RectOp::solid(
            PaintRect::new(rim.x + 4.0, 3.0, rim.width - 8.0, 2.0),
            1.0,
            hex_literal_to_color(row::CUT_DARK),
        )
        .at_opacity(0.85),
    );
    // ...the bright machined face along the lower lip...
    p.rect(
        RectOp::solid(
            PaintRect::new(rim.x + 5.0, rim.height - 4.0, rim.width - 10.0, 2.0),
            1.0,
            hex_literal_to_color(row::LIP_LIGHT),
        )
        .at_opacity(0.7 + 0.3 * j),
    );
    // ...and a fainter catch down the right lip.
    p.rect(
        RectOp::solid(
            PaintRect::new(rim.x + rim.width - 4.0, 6.0, 2.0, rim.height - 12.0),
            1.0,
            hex_literal_to_color(row::LIP_LIGHT),
        )
        .at_opacity(0.3 + 0.2 * j),
    );

    // The punched hole the lamps live in, recessed, its bright bevel line
    // on the bottom lip where light catches the far wall.
    let panel_dark = hex_literal_to_color(row::PANEL_DARK);
    p.rect(RectOp::gradient(
        PaintRect::new(display_rect.x, 4.0, display_rect.width, row_h - 8.0),
        5.0,
        vec![
            Stop::new(0.0, darker(panel_dark, 1.6)),
            Stop::new(0.45, panel_dark),
            Stop::new(0.93, lighter(panel_dark, 1.6)),
            Stop::new(1.0, lighter(panel_dark, 3.2)),
        ],
    ));
    p
}


/// The PREV/NEXT rocker at the bank's foot.
///
/// Labels engraved in the plate above the keys, solid arrows pointing outward
/// beside them, and two ridged key caps ([`channel_button`]) below. `width` is
/// the content lane the bank hands over; the height is
/// [`pager::NATURAL_HEIGHT`], which is where the bank's row count came from.
///
/// Enabled only when there is more than one page, dimmed to 0.55 opacity
/// otherwise, which is the whole of what the page count changes here, and
/// the reason this takes it at all: a single-page bank draws its rocker at
/// just over half strength, saying without words that the keys do nothing.
pub fn pager(plastic: Rgba, size: (f64, f64), page_count: i32) -> Vec<crate::furniture::Piece> {
    let (width, height) = size;
    let painting = pager_painting(plastic, width, page_count);
    if painting.is_empty() {
        return Vec::new();
    }
    vec![crate::furniture::Piece::painted(
        PaintRect::new(0.0, 0.0, width, height),
        painting,
    )]
}

fn pager_painting(plastic: Rgba, width: f64, page_count: i32) -> Painting {
    let mut p = Painting::new();
    if width <= 0.0 {
        return p;
    }
    let label_color = hex_literal_to_color(LABEL_COLOR);
    let opacity: f32 = if page_count > 1 { 1.0 } else { 0.55 };

    // The key cap and the air between the caps; the pair stands centred in
    // whatever lane the bank hands over.
    let key_width = KEY_WIDTH;
    let key_spread = 92.0f64.min(width - 2.0 * key_width - 8.0);
    let prev_key_x = (width - 2.0 * key_width - key_spread) / 2.0;
    let next_key_x = prev_key_x + key_width + key_spread;

    // The two labels, centred over their own keys, and an arrow outside
    // each pointing away from it. Both are set in the application's own
    // default font.
    let label = |x: f64, text: &str| TextOp {
        face: Face::Sans,
        x,
        y: 5.0,
        width: 0.0,
        align: Align::Left,
        pixel_size: 15.0,
        letter_spacing: 2.0,
        bold: true,
        text: text.to_string(),
        color: label_color,
        opacity,
    };
    let arrow = |x: f64, text: &str| TextOp {
        face: Face::Sans,
        x,
        y: 6.0,
        width: 0.0,
        align: Align::Left,
        pixel_size: 14.0,
        letter_spacing: 0.0,
        bold: false,
        text: text.to_string(),
        color: label_color,
        opacity,
    };
    // Each label is centred on its key by its own painted width, so the
    // width has to be measured before the position can be worked out.
    let (prev_w, next_w) = (
        sans_width("PREV", 15.0, 2.0, true),
        sans_width("NEXT", 15.0, 2.0, true),
    );
    let prev_x = prev_key_x + key_width / 2.0 - prev_w / 2.0;
    let next_x = next_key_x + key_width / 2.0 - next_w / 2.0;
    let left_arrow_w = sans_width("\u{25C0}", 14.0, 0.0, false);
    p.text(arrow(prev_x - left_arrow_w - 8.0, "\u{25C0}"));
    p.text(label(prev_x, "PREV"));
    p.text(label(next_x, "NEXT"));
    p.text(arrow(next_x + next_w + 8.0, "\u{25B6}"));

    // The two keys, 38px into the block.
    for x in [prev_key_x, next_key_x] {
        for mut op in channel_button(plastic, key_width, KEY_HEIGHT).ops {
            if let crate::paint::Op::Rect(r) = &mut op {
                r.rect.x += x;
                r.rect.y += KEY_TOP;
                r.opacity *= opacity;
            }
            p.ops.push(op);
        }
    }
    p
}

/// The pager's label colour.
const LABEL_COLOR: &str = "#6a5642";
/// The key cap's measures and where its top sits in the block.
const KEY_WIDTH: f64 = 56.0;
const KEY_HEIGHT: f64 = 40.0;
const KEY_TOP: f64 = 38.0;

/// One pager key, in its own coordinates.
///
/// A metal cap of four machined ridges lit from above, each ridge a specular
/// line over its own shadow, the cap's front face dropping to near-black, a
/// worn bevel down both sides and the shadow the key throws onto the plate.
/// `pressed` and `hovered` are not taken: a snapshot of a bank nobody is
/// touching is the only state the column is asked to paint, and its resting
/// opacity for that state is 0.97.
pub fn channel_button(_plastic: Rgba, width: f64, height: f64) -> Painting {
    let ridge_highlight = hex_literal_to_color("#f7e8c4");
    let ridge_base = hex_literal_to_color("#c7a381");
    let ridge_shadow = hex_literal_to_color("#5e4630");
    let front_face = hex_literal_to_color("#1c1411");
    let black = Rgba::new(0.0, 0.0, 0.0, 1.0);

    let mut p = Painting::new();
    // The key's shadow on the plate.
    p.rect(RectOp::solid(PaintRect::new(2.0, 3.0, width, height), 4.0, black).at_opacity(0.45));
    // The cap itself, at rest.
    const REST_OPACITY: f32 = 0.97;
    p.rect(
        RectOp::solid(PaintRect::new(0.0, 0.0, width, height), 3.0, front_face)
            .at_opacity(REST_OPACITY),
    );
    // Four ridges, each a specular line over its own shadow, stacked from
    // (2, 2) at five pixels of pitch across the cap's inner width.
    let ridge_w = width - 4.0;
    for i in 0..4 {
        let y = 2.0 + i as f64 * 5.0;
        p.rect(
            RectOp::solid(PaintRect::new(2.0, y, ridge_w, 2.0), 1.0, ridge_highlight)
                .at_opacity(REST_OPACITY),
        );
        p.rect(
            RectOp::solid(PaintRect::new(2.0, y + 2.0, ridge_w, 2.0), 0.0, ridge_base)
                .at_opacity(REST_OPACITY),
        );
        p.rect(
            RectOp::solid(
                PaintRect::new(2.0, y + 4.0, ridge_w, 1.0),
                0.0,
                ridge_shadow,
            )
            .at_opacity(REST_OPACITY),
        );
    }
    // The cap's rolled front edge under the last ridge.
    p.rect(
        RectOp::solid(
            PaintRect::new(2.0, 2.0 + 20.0, ridge_w, 2.0),
            0.0,
            ridge_shadow,
        )
        .at_opacity(REST_OPACITY),
    );
    // Worn bevels down the cap's sides, lit left and dark right.
    p.rect(
        RectOp::solid(PaintRect::new(0.0, 2.0, 1.0, height - 6.0), 0.0, ridge_base)
            .at_opacity(0.5 * REST_OPACITY),
    );
    p.rect(
        RectOp::solid(
            PaintRect::new(width - 1.0, 2.0, 1.0, height - 6.0),
            0.0,
            black,
        )
        .at_opacity(0.6 * REST_OPACITY),
    );
    // A dim catch along the front face's foot.
    p.rect(
        RectOp::solid(
            PaintRect::new(3.0, height - 2.0, width - 6.0, 1.0),
            0.0,
            ridge_base,
        )
        .at_opacity(0.25 * REST_OPACITY),
    );
    p
}

/// The milled slot down the bank's left edge with the clamp riding it, for
/// a profile whose indicator is the pointer.
///
/// `size` is the track item's own, which the bank sets to its lane by the
/// bank's height less its padding; `target_y` is where the clamp's centre
/// belongs, in the track's own coordinates, which the bank works out. The
/// travel between rows is a hand moving the selector and has no place in a
/// still: the clamp is painted where it is going.
pub fn selector_track(plastic: Rgba, size: (f64, f64), target_y: f64) -> Painting {
    let (w, h) = size;
    let mut p = Painting::new();
    if w <= 0.0 || h <= 0.0 {
        return p;
    }
    let clamp_height = 16.0;
    let groove_width = 5.0f64.max((w * 0.4).round());

    // The slot, dark at the near wall and lifting toward the far one: a
    // horizontal ramp, since the cut runs down the item.
    p.rect(RectOp::horizontal_gradient(
        PaintRect::new((w - groove_width) / 2.0, 0.0, groove_width, h),
        groove_width / 2.0,
        vec![
            Stop::new(0.0, darker(plastic, 3.6)),
            Stop::new(0.65, darker(plastic, 2.8)),
            Stop::new(1.0, darker(plastic, 1.9)),
        ],
    ));

    // The clamp, rounded to the row it reads.
    let clamp_y = (target_y - clamp_height / 2.0).round();
    // The shadow the clamp drops into its own slot.
    p.rect(
        RectOp::solid(
            PaintRect::new(1.0, clamp_y + 2.0, w - 1.0, clamp_height),
            3.0,
            darker(plastic, 3.2),
        )
        .at_opacity(0.45),
    );
    // The body, lit along its top face and shaded under the lip.
    p.rect(RectOp::gradient(
        PaintRect::new(0.0, clamp_y, w, clamp_height),
        3.0,
        vec![
            Stop::new(0.0, lighter(plastic, 1.6)),
            Stop::new(0.4, lighter(plastic, 1.15)),
            Stop::new(1.0, darker(plastic, 1.7)),
        ],
    ));
    // The nose that reads the row, on the bank side.
    let nose_h = clamp_height * 0.5;
    p.rect(RectOp::gradient(
        PaintRect::new(
            w - 3.0,
            clamp_y + (clamp_height - nose_h) / 2.0,
            3.0,
            nose_h,
        ),
        1.0,
        vec![
            Stop::new(0.0, lighter(plastic, 1.7)),
            Stop::new(1.0, darker(plastic, 1.2)),
        ],
    ));
    p
}
