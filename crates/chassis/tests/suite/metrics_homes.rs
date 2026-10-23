//! Regression pins: each measure group has exactly one home for its
//! arithmetic, and this file checks that home's two shapes -- the bare
//! constants/functions, and the composed shape
//! (`ShellMetrics`/`LedMetrics`/`TapeMetrics` behind `DisplayMetrics`) that
//! reads them -- against hand-computed expected values, so a copy-paste slip
//! in the composition (reading the wrong field, dropping a parameter) fails
//! here even though both shapes trace to the same source numbers.
//!
//! One home per group:
//!
//! - a shell's metrics are owned by `src/shells/*.rs`'s `metrics` module
//!   (bare constants), and
//!   `chassis::metrics::shells::{annunciator,slide_rule,switchboard}` reads
//!   those constants into a `ShellMetrics`.
//! - a display's metrics are owned by `src/displays/{led,tape}/metrics.rs`
//!   (free functions), and `chassis::metrics::{LedMetrics, TapeMetrics}`'s
//!   `DisplayMetrics` impls call straight through to them.
//!
//! What is left to guard is the composition itself, not a second arithmetic
//! implementation -- hence pinned values rather than a home-to-home diff.

use chassis::displays::led::{self, LED_DOT_PITCH, LED_PAD_CELLS, LED_SIDE_PAD_CELLS};
use chassis::displays::tape::{self, END_PAD, LETTER_PIXEL_SIZE, NATURAL_HEIGHT};
use chassis::metrics::{shells, Rgb, TAPE_LETTER_PIXEL_SIZE, TAPE_NATURAL_HEIGHT};
use chassis::shells::common::rgb;
use chassis::{DisplayMetrics, LedMetrics, ShellMetrics, TapeMetrics};

/// The fields the composed `ShellMetrics` carries, checked as one so a new
/// shell cannot be half-wired: every field of `composed` (built by
/// `chassis::metrics::shells::*`) against the same field read directly off
/// the shell's own bare `metrics` module.
fn same_shell(shell: &str, composed: ShellMetrics, bare: ShellMetrics) {
    assert_eq!(
        composed.bank_padding, bare.bank_padding,
        "{shell}: bank_padding"
    );
    assert_eq!(
        composed.top_padding, bare.top_padding,
        "{shell}: top_padding"
    );
    assert_eq!(
        composed.bottom_padding, bare.bottom_padding,
        "{shell}: bottom_padding"
    );
    assert_eq!(
        composed.right_padding, bare.right_padding,
        "{shell}: right_padding"
    );
    assert_eq!(
        composed.row_spacing, bare.row_spacing,
        "{shell}: row_spacing"
    );
    assert_eq!(composed.column_gap, bare.column_gap, "{shell}: column_gap");
    assert_eq!(
        composed.numeral_width, bare.numeral_width,
        "{shell}: numeral_width"
    );
    assert_eq!(
        composed.strip_padding, bare.strip_padding,
        "{shell}: strip_padding"
    );
    assert_eq!(
        composed.min_row_height, bare.min_row_height,
        "{shell}: min_row_height"
    );
    assert_eq!(
        composed.track_width, bare.track_width,
        "{shell}: track_width"
    );
    assert_eq!(
        composed.chassis_carries_track, bare.chassis_carries_track,
        "{shell}: chassis_carries_track"
    );
    assert_eq!(composed.track_x, bare.track_x, "{shell}: track_x");
    assert_eq!(
        composed.casting_light_dir, bare.casting_light_dir,
        "{shell}: casting_light_dir"
    );
    assert_eq!(
        [
            composed.casting_color.r,
            composed.casting_color.g,
            composed.casting_color.b
        ],
        [
            bare.casting_color.r,
            bare.casting_color.g,
            bare.casting_color.b
        ],
        "{shell}: casting_color"
    );
}

/// The casting colour through `shells::common::rgb` (`hex_literal_to_color`,
/// /255): the same divisor `chassis::metrics::Rgb::from_hex_literal` uses, per
/// `chassis::color`'s module doc.
fn casting(hex: &str) -> Rgb {
    let c = rgb(hex);
    Rgb {
        r: c[0],
        g: c[1],
        b: c[2],
    }
}

#[test]
fn the_annunciator_shell_metrics_composes_the_bare_constants() {
    use chassis::shells::annunciator::metrics as m;
    use chassis::shells::annunciator::pager as p;
    same_shell(
        "annunciator",
        shells::annunciator(),
        ShellMetrics {
            bank_padding: m::BANK_PADDING,
            top_padding: m::TOP_PADDING,
            bottom_padding: m::BOTTOM_PADDING,
            right_padding: m::RIGHT_PADDING,
            row_spacing: m::ROW_SPACING,
            column_gap: m::COLUMN_GAP,
            numeral_width: m::NUMERAL_WIDTH,
            strip_padding: m::STRIP_PADDING,
            min_row_height: m::MIN_ROW_HEIGHT,
            track_width: m::TRACK_WIDTH,
            // This shell declares neither, so both shapes take the same
            // fallback: no chassis rail, and a rail x at the bank's own
            // padding.
            chassis_carries_track: false,
            track_x: m::BANK_PADDING,
            casting_light_dir: m::CASTING_LIGHT_DIR,
            casting_color: casting(m::CASTING_COLOR),
            // Not a fixed-geometry measure, so it bridges against the
            // pager's own home rather than this one's.
            pager_natural_height: p::NATURAL_HEIGHT,
            pager_squeeze_span: p::SQUEEZE_SPAN,
            pager_extra: p::EXTRA,
            pager_stack_below: p::stack_below(),
            pager_stacked_height: p::STACKED_HEIGHT,
            pager_key_share: p::KEY_SHARE,
        },
    );
    // The bank's foot reads the pager's height, and this pager never
    // squeezes: it is the one line's 57 in a lane wide enough for the
    // labels beside the keys, and 16 more under it for their line over them.
    let span = p::stack_below();
    let one_line = shells::annunciator().pager_height(span.ceil() as i32);
    assert_eq!(one_line, 57);
    assert_eq!(shells::annunciator().pager_height(4000), 57);
    assert_eq!(shells::annunciator().pager_height(span.ceil() as i32 - 1), 73);
    assert_eq!(shells::annunciator().pager_height(180), 73);
}

#[test]
fn the_slide_rule_shell_metrics_composes_the_bare_constants() {
    use chassis::shells::slide_rule::metrics as m;
    use chassis::shells::slide_rule::pager as p;
    same_shell(
        "slide_rule",
        shells::slide_rule(),
        ShellMetrics {
            bank_padding: m::BANK_PADDING,
            top_padding: m::TOP_PADDING,
            bottom_padding: m::BOTTOM_PADDING,
            right_padding: m::RIGHT_PADDING,
            row_spacing: m::ROW_SPACING,
            column_gap: m::COLUMN_GAP,
            numeral_width: m::NUMERAL_WIDTH,
            strip_padding: m::STRIP_PADDING,
            min_row_height: m::MIN_ROW_HEIGHT,
            track_width: m::TRACK_WIDTH,
            chassis_carries_track: m::CHASSIS_CARRIES_TRACK,
            track_x: m::TRACK_X,
            casting_light_dir: m::CASTING_LIGHT_DIR,
            casting_color: casting(m::CASTING_COLOR),
            // The one pager of the three whose height is measured rather
            // than declared.
            pager_natural_height: p::NATURAL_HEIGHT,
            pager_squeeze_span: p::SQUEEZE_SPAN,
            pager_extra: p::EXTRA,
            pager_stack_below: 0.0,
            pager_stacked_height: 0.0,
            pager_key_share: 1.0 / 3.0,
        },
    );
    // The blue shell's rows have 157 px of ground at twelve characters, so
    // its pager stands squeezed: round(90 * 157/305) + 3.
    assert_eq!(shells::slide_rule().pager_height(157), 46 + 3);
    // ...and at its natural span or wider it is the full 93.
    assert_eq!(shells::slide_rule().pager_height(305), 93);
    assert_eq!(shells::slide_rule().pager_height(600), 93);
}

/// This shell's fixed geometry, plus the one measure `src/metrics.rs`
/// keeps as a loose constant because the row-count settle needs it.
#[test]
fn the_switchboard_shell_metrics_composes_the_bare_constants() {
    use chassis::shells::switchboard::metrics as m;
    use chassis::shells::switchboard::pager as p;
    same_shell(
        "switchboard",
        shells::switchboard(),
        ShellMetrics {
            bank_padding: m::BANK_PADDING,
            top_padding: m::TOP_PADDING,
            bottom_padding: m::BOTTOM_PADDING,
            right_padding: m::RIGHT_PADDING,
            row_spacing: m::ROW_SPACING,
            column_gap: m::COLUMN_GAP,
            numeral_width: m::NUMERAL_WIDTH,
            strip_padding: m::STRIP_PADDING,
            min_row_height: m::MIN_ROW_HEIGHT,
            // No track anywhere on this panel: the thrown switch is the mark.
            track_width: 0,
            chassis_carries_track: false,
            track_x: m::BANK_PADDING,
            casting_light_dir: m::CASTING_LIGHT_DIR,
            casting_color: casting(m::CASTING_COLOR),
            // The item inside the rail this shell's own metrics measure,
            // which is the height the settle subtracts.
            pager_natural_height: p::NATURAL_HEIGHT,
            pager_squeeze_span: p::SQUEEZE_SPAN,
            pager_extra: p::EXTRA,
            pager_stack_below: 0.0,
            pager_stacked_height: 0.0,
            pager_key_share: 1.0 / 3.0,
        },
    );
    // The one shell whose pager reads its own metrics back: the derivation
    // is the pager home's, and this pins it against both files.
    assert_eq!(p::NATURAL_HEIGHT, (m::PAGER_HEIGHT - 15) as f64);
    assert_eq!(shells::SWITCHBOARD_PAGER_HEIGHT, m::PAGER_HEIGHT);
    assert_eq!(shells::switchboard().pager_height(200), 89);
}

/// The composed `LedMetrics`/`DisplayMetrics` shape against the bare
/// `displays::led::metrics` functions called directly with the schema's own
/// constants -- the same ones a live profile always supplies
/// (`chassis::led_metrics`) -- at sampled cells (two the bundled faces
/// actually produce, UNSCII 8x9-ish and a 16-high face, and a
/// fractional-pitch case for the rounding). This still catches a delegation
/// bug (the wrong field forwarded to the wrong parameter) even though both
/// sides now trace to the same formula.
#[test]
fn the_led_strip_metrics_composes_the_bare_functions() {
    let stock = LedMetrics::default();
    assert_eq!(stock.dot_pitch, LED_DOT_PITCH);
    assert_eq!(stock.min_characters as u32, led::MIN_LED_CHARACTERS);
    assert_eq!(stock.pad_cells as u32, LED_PAD_CELLS);
    assert_eq!(stock.side_pad_cells as u32, LED_SIDE_PAD_CELLS);

    for &(lamp_cell_width, lamp_cell_height) in &[(5u32, 8u32), (8, 9), (8, 16), (9, 11)] {
        let composed = LedMetrics {
            lamp_cell_width: lamp_cell_width as i32,
            lamp_cell_height: lamp_cell_height as i32,
            ..LedMetrics::default()
        };

        assert_eq!(
            composed.unit_width(),
            led::metrics::unit_width(lamp_cell_width, LED_DOT_PITCH),
            "unit_width at cell {lamp_cell_width}x{lamp_cell_height}"
        );
        assert_eq!(
            composed.min_units() as u32,
            led::metrics::min_units(led::MIN_LED_CHARACTERS)
        );

        for n in [0u32, 1, 7, 8, 12, 40] {
            assert_eq!(
                i64::from(composed.width_for_units(n as i32)),
                led::metrics::width_for_units(lamp_cell_width, n, LED_SIDE_PAD_CELLS, LED_DOT_PITCH),
                "width_for_units({n}) at cell {lamp_cell_width}x{lamp_cell_height}"
            );
        }
        for pad in [0u32, 1, 4, 15, 32] {
            assert_eq!(
                i64::from(composed.height_for_pad_cells(pad as i32)),
                led::metrics::height_for_pad_cells(lamp_cell_height, pad, LED_DOT_PITCH),
                "height_for_pad_cells({pad}) at cell {lamp_cell_width}x{lamp_cell_height}"
            );
        }
        for hole in [0i32, 6, 12, 35, 44, 60] {
            assert_eq!(
                i64::from(composed.pad_cells_for_hole(hole)),
                led::metrics::pad_cells_for_hole(
                    lamp_cell_height,
                    f64::from(hole),
                    LED_DOT_PITCH,
                    LED_PAD_CELLS
                ),
                "pad_cells_for_hole({hole}) at cell {lamp_cell_width}x{lamp_cell_height}"
            );
        }
    }
}

/// The same, over the kit's fixed letter.
#[test]
fn the_tape_well_metrics_composes_the_bare_functions() {
    assert_eq!(TAPE_LETTER_PIXEL_SIZE as u32, LETTER_PIXEL_SIZE);
    assert_eq!(TAPE_NATURAL_HEIGHT as u32, NATURAL_HEIGHT);

    let entry = term::font_by_name(tape::FONT_NAME, term::FontSource::Bundled)
        .expect("the bundled tape face");
    let advance = tape::metrics::unit_width(entry.data());
    assert!(advance > 0.0, "the tape face measured no advance");

    let composed = TapeMetrics {
        unit_width: advance,
        end_pad: END_PAD as i32,
        min_characters: led::MIN_LED_CHARACTERS as i32,
    };

    assert_eq!(composed.unit_width(), advance);
    assert_eq!(composed.min_units() as u32, tape::metrics::min_units());

    for n in [0u32, 1, 8, 12, 40] {
        assert_eq!(
            i64::from(composed.width_for_units(n as i32)),
            tape::metrics::width_for_units(advance, n, END_PAD),
            "width_for_units({n})"
        );
    }
    // The height pair is a pass-through on this kit, in both directions and
    // including the negative hole the LED panel would have floored
    // differently.
    for hole in [-8i32, 0, 30, 44, 100] {
        let bare = tape::metrics::pad_cells_for_hole(f64::from(hole));
        assert_eq!(
            i64::from(composed.pad_cells_for_hole(hole)),
            bare,
            "pad_cells_for_hole({hole})"
        );
        assert_eq!(
            i64::from(composed.height_for_pad_cells(bare as i32)),
            tape::metrics::height_for_pad_cells(bare),
            "height_for_pad_cells({bare})"
        );
    }
}
