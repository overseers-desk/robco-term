//! The chassis: the cabinet the curved screen is set into.
//!
//! The appliance is a screen well with a column of furniture to its left
//! (the channel bank) and a metal casting wrapped around both. This crate owns
//! that casting's geometry, the metal it is poured from, and the furniture
//! that stands on it. It owns no device and draws nothing itself: it turns a
//! window size and a profile into rectangles, named shader parameters and
//! drawing descriptions, and whoever holds the wgpu device draws them.
//!
//! # What lives where
//!
//! - [`metrics`]: the two measure contracts. A shell declares its fixed
//!   furniture; a display declares how it quantises its own width.
//! - [`bank`]: the channel bank's measures, and the one number the rest of the
//!   window turns on: [`bank::BankGeometry::implicit_width`].
//! - [`layout`]: how the window divides into the bank's column and the screen
//!   well, and the screen-scale normalisation every curvature-bearing uniform
//!   is measured in.
//! - [`seam`]: the drag on the boundary between the two, which resizes the
//!   screen region in whole-character steps.
//! - [`frame`]: the bezel's and the chassis metal's uniform sets, and the
//!   derivations that sit between a stored setting and a shader.
//! - [`params`]: the parameter-record structs for the three metals and the
//!   two display kits, the independent-formula side of their per-pass tests.
//! - [`paint`]: the drawing vocabulary the furniture that is not a shader is
//!   described in: rounded rectangles with gradients, arcs, filled paths and
//!   lines of text.
//! - [`displays`]: the two channel-display kits, LED and tape: the raster
//!   each one composes over `term::fonts::led`'s proven glyphs, and the
//!   appearance mapping that colours it.
//! - [`shells`]: the three shells themselves: each one's fixed geometry and
//!   its drawing recipe, i.e. which regions it paints and with which of the
//!   three metals.
//!
//! The shader sources are split by which side of the chain they are drawn
//! on, and [`shaders`] compiles both families in for a host with no source
//! tree to read them from. `shaders/metal/` carries `frame_metal`, the bezel,
//! which is the one piece of cabinet inside the curvature and so is a `.slang`
//! the chain mounts. `shaders/wgsl/` carries everything drawn after it:
//! `chassis_metal` (the casting under the bank), `plate_metal` (the raised
//! plate a shell screws over it), `led_matrix` and `tape_label` (the two
//! channel displays), and `vector` (the drawn furniture). Those five are
//! function bodies with no bindings and no entry points, so the host that
//! concatenates them decides what they read.
//!
//! # The order the numbers come in
//!
//! Nothing here is a binding graph, so the composition order is worth stating
//! once. Every window size, and every settings change that touches any of it:
//!
//! 1. Build the display's [`metrics::DisplayMetrics`] from the profile.
//! 2. Build the [`bank::BankGeometry`] from shell metrics, display metrics, the
//!    character count and the indicator law. Its `implicit_width` is the bank's
//!    footprint.
//! 3. Build the [`layout::WindowLayout`] from the window size and that width.
//! 4. Draw: [`frame::frame_params`] over the well, [`frame::chassis_params`]
//!    over the bank column.
//! 5. Hand `implicit_width` to `app::shell::Shell::set_bank_width`, so the
//!    window's minimum-size hint follows.
//!
//! A seam drag re-enters at step 1 with a new character count. See [`seam`] for
//! why that loop is stable rather than drifting.
//!
//! [`cabinet::Cabinet`] is that whole order assembled, shaped so an
//! `app::shell::Surface` forwards to it method for method. Everything under it
//! stays usable alone: the geometry, the layout and the seam are plain values
//! and pure functions, testable with no window and no device.
//!
//! # Where the chassis stops
//!
//! Chassis chrome lives outside the CRT chain and composites over the glass,
//! in a native pass the application owns (`app::chrome`). That is why this
//! crate depends on no part of `crt` at all, shipped or dev: not for the two
//! small functions the `robco-shader-oracle` crate carries once for both, and
//! not for a rig, since the one pass of ours the chain mounts is measured on
//! the chain's side of the tree. The piece genuinely in both worlds is the
//! bezel; [`frame`]'s module doc says what it is doing there. [`color`] is the
//! same trade made once for the whole crate: the two colour helpers both
//! halves of the chrome need, rather than a dependency on `crt::color`.

pub mod bank;
pub mod cabinet;
pub mod color;
pub mod displays;
pub mod frame;
pub mod furniture;
pub mod js;
pub mod layout;
pub mod metrics;
pub mod paint;
pub mod params;
pub mod seam;
pub mod shaders;
pub mod shells;
pub mod strip;

pub use bank::{BankGeometry, ChannelIndicator};
pub use cabinet::{Cabinet, Display, SeamUpdate};
pub use furniture::{Pass, Piece, PieceParams, StripHit};
pub use layout::{Rect, WindowLayout};
pub use metrics::{DisplayMetrics, LedMetrics, ShellMetrics, TapeMetrics};
pub use seam::{SeamContext, SeamCursor, SeamDrag};
pub use strip::{BankStrips, StripRow};

use config::Config;

/// The shell metrics the profile asks for.
pub fn shell_metrics(cfg: &Config) -> ShellMetrics {
    match cfg.chassis.shell {
        config::Shell::Annunciator => metrics::shells::annunciator(),
        config::Shell::SlideRule => metrics::shells::slide_rule(),
        config::Shell::Switchboard => metrics::shells::switchboard(),
    }
}

/// The bezel the profile asks for.
pub fn frame_style(cfg: &Config) -> frame::FrameStyle {
    match cfg.chassis.shell {
        config::Shell::Annunciator => frame::styles::annunciator_frame(),
        config::Shell::SlideRule => frame::styles::slide_rule_frame(),
        config::Shell::Switchboard => frame::styles::switchboard_frame(),
    }
}

/// The chassis surface the profile asks for.
pub fn chassis_style(cfg: &Config) -> frame::ChassisStyle {
    match cfg.chassis.shell {
        config::Shell::Annunciator => frame::styles::annunciator_chassis(),
        config::Shell::SlideRule => frame::styles::slide_rule_chassis(),
        config::Shell::Switchboard => frame::styles::switchboard_chassis(),
    }
}

/// The display kit the profile asks for, measured off the font stack the
/// profile's [`font_source`] names -- the bundled catalogue for an
/// unconfigured profile, so measuring a cabinet costs no font scan.
///
/// This is the one function in the crate that touches a font. Everything else
/// takes the measured kit as a value, which is what keeps the geometry testable
/// with no font stack at all; this is where a *profile* becomes such a value,
/// so a host does not have to know which of the two kits reads the user's lamp
/// font and which carries its own.
pub fn display_kit(cfg: &Config) -> Display {
    let source = font_source(cfg);
    match cfg.chassis.channel_display {
        config::ChannelDisplay::Led => {
            Display::Led(led_metrics(&cfg.chassis.bank_font_name, source))
        }
        config::ChannelDisplay::Tape => {
            Display::Tape(tape_metrics(&cfg.chassis.bank_font_name, source))
        }
    }
}

/// Which catalogue a profile's font names are looked up in.
///
/// The config key and the catalogue's own selector are two spellings of one
/// choice, and this crate is where they meet: it is the only one that depends
/// on both. Under [`term::fonts::FontSource::Bundled`] nothing a cabinet
/// looks up can reach a face off the machine, which is what keeps an
/// unconfigured profile from scanning.
pub fn font_source(cfg: &Config) -> term::fonts::FontSource {
    match cfg.screen.font_source {
        config::FontSource::BundledFonts => term::fonts::FontSource::Bundled,
        config::FontSource::SystemFonts => term::fonts::FontSource::System,
    }
}

/// The LED cell the profile's lamp font implies: `max(1, round(advance_width))`
/// by `max(1, ceil(height))`, taken off the advance and scaled height of
/// `"M"` at the face's own catalogue pixel size ([`displays::led::cell_metrics`],
/// over the proven 26.6 path). The four remaining measures are the
/// settings' own defaults.
///
/// `source` is the catalogue the name is looked up in ([`font_source`]). A
/// `bank_font_name` naming no face that catalogue offers falls back to the
/// lamp strip's own shipped face rather than refusing to measure: a
/// hand-edited config, or a system face named under a bundled profile, is not
/// a reason to have no bank.
pub fn led_metrics(font_name: &str, source: term::fonts::FontSource) -> LedMetrics {
    let entry = term::fonts::font_by_name_or(font_name, source, displays::led::DEFAULT_LED_FONT_NAME)
        .expect("the bundled catalogue always carries the default lamp font");
    let (lamp_cell_width, lamp_cell_height) = displays::led::cell_metrics(entry.data(), entry.pixel_size);
    LedMetrics {
        lamp_cell_width: lamp_cell_width as i32,
        lamp_cell_height: lamp_cell_height as i32,
        dot_pitch: displays::led::LED_DOT_PITCH,
        min_characters: displays::led::MIN_LED_CHARACTERS as i32,
        pad_cells: displays::led::LED_PAD_CELLS as i32,
        side_pad_cells: displays::led::LED_SIDE_PAD_CELLS as i32,
    }
}

/// The punch wheel's letter size for the face this cabinet stamps its tape
/// in: the advance of `"M"` at 20 px, the 12 px of blank tape at either end,
/// and the same character floor the lamp strip takes.
///
/// The letter size is the wheel's and not the face's, so a cabinet that names
/// another face stamps it at the same 20 px. A name `source`'s catalogue does
/// not offer falls back to the tape's own, the way the lamp strip falls back
/// to its.
pub fn tape_metrics(font_name: &str, source: term::fonts::FontSource) -> TapeMetrics {
    let entry = term::fonts::font_by_name_or(font_name, source, displays::tape::FONT_NAME)
        .expect("the bundled catalogue always carries the tape's own face");
    TapeMetrics {
        unit_width: displays::tape::metrics::unit_width(entry.data()),
        end_pad: displays::tape::END_PAD as i32,
        min_characters: displays::tape::metrics::min_units() as i32,
    }
}

/// How the profile marks the channel on screen.
pub fn channel_indicator(cfg: &Config) -> ChannelIndicator {
    match cfg.chassis.channel_indicator {
        config::ChannelIndicator::Pointer => ChannelIndicator::Pointer,
        config::ChannelIndicator::Switch => ChannelIndicator::Switch,
        config::ChannelIndicator::Glow => ChannelIndicator::Glow,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_profile_names_the_catalogue_its_faces_are_looked_up_in() {
        // The shipped profile asks for bundled faces, which is what makes
        // measuring the stock cabinet a scan-free thing to do.
        let mut cfg = Config::default();
        assert_eq!(cfg.screen.font_source, config::FontSource::BundledFonts);
        assert_eq!(font_source(&cfg), term::FontSource::Bundled);
        cfg.screen.font_source = config::FontSource::SystemFonts;
        assert_eq!(font_source(&cfg), term::FontSource::System);
    }

    #[test]
    fn every_shell_the_config_can_name_has_metrics_a_bezel_and_a_casting() {
        // The enum is closed, so this is a match-exhaustiveness check with the
        // values attached: a fourth shell added to `robco-config` fails to
        // compile here rather than falling back to the amber one at run time.
        for shell in [
            config::Shell::Annunciator,
            config::Shell::SlideRule,
            config::Shell::Switchboard,
        ] {
            let mut cfg = Config::default();
            cfg.chassis.shell = shell;
            let m = shell_metrics(&cfg);
            let f = frame_style(&cfg);
            let c = chassis_style(&cfg);
            // A shell that declares no measures would divide the bank by zero.
            assert!(m.min_row_height > 0, "{shell:?} has no row height");
            assert!(m.numeral_width > 0, "{shell:?} has no numeral column");
            assert!(f.outer_radius > 0.0, "{shell:?} has a square bezel");
            assert!(c.vignette_strength > 0.0, "{shell:?} has a flat casting");
            // The frame and the chassis are one casting, so both styles
            // read the same light and metal off the metrics.
            assert!(m.casting_light_dir[1] < 0.0, "{shell:?} is lit from below");
        }
    }

    #[test]
    fn the_stock_profile_measures_its_lamp_font_rather_than_a_fixture() {
        // `LedMetrics::default()`'s 5x8 cell is a fixture and says so; the
        // shipped cabinet letters its bank in Cozette, whose "M" advances
        // 6 px at its own catalogue pixel size and whose scaled height ceils
        // to 13. The difference is the whole point of measuring: a bank built
        // on the fixture is 184 px wide and the appliance the user gets is
        // 205.
        let cfg = Config::default();
        assert_eq!(cfg.chassis.bank_font_name, "COZETTE_SCALED");
        let led = led_metrics(&cfg.chassis.bank_font_name, term::FontSource::Bundled);
        assert_eq!((led.lamp_cell_width, led.lamp_cell_height), (6, 13));
        assert_eq!(led.unit_width(), 9.0); // 6 * 1.5
        assert_eq!(led.width_for_units(12), 126); // round(6 * 14 * 1.5)
        assert_ne!(led, LedMetrics::default());

        // A name no bundled face answers to falls back on the strip's own
        // shipped face rather than refusing, which is a different face from
        // the one this cabinet names.
        assert_eq!(
            led_metrics("NO_SUCH_FACE", term::FontSource::Bundled),
            led_metrics(displays::led::DEFAULT_LED_FONT_NAME, term::FontSource::Bundled)
        );
        assert_ne!(led_metrics("NO_SUCH_FACE", term::FontSource::Bundled), led);

        // Departure Mono at 20 px, the tape's one letter size, measured the
        // same way.
        let tape = tape_metrics("DEPARTURE_MONO_SCALED", term::FontSource::Bundled);
        assert_eq!(tape.end_pad, 12);
        assert_eq!(tape.min_characters, 8);
        assert!(
            tape.unit_width > 5.0 && tape.unit_width < 20.0,
            "unit_width={}",
            tape.unit_width
        );

        // And the cabinet picks between them, each kit lettering the bank in
        // the face that cabinet names: the shipped cabinet's lamps in
        // Cozette, the switchboard's tape stamped in Departure Mono.
        assert_eq!(display_kit(&cfg), Display::Led(led));
        let switchboard = config::presets::chassis_presets()
            .into_iter()
            .find(|c| c.name == "Switchboard")
            .expect("the switchboard is a shipped cabinet");
        assert_eq!(switchboard.bank_font_name, "DEPARTURE_MONO_SCALED");
        let mut tape_cfg = cfg.clone();
        tape_cfg.chassis = switchboard;
        assert_eq!(display_kit(&tape_cfg), Display::Tape(tape));
    }

    #[test]
    fn two_cabinets_letter_their_banks_in_their_own_faces() {
        // The point of the setting being the cabinet's: taking a look takes
        // the face its bank is lettered in, and a bank cut for a wider cell
        // is a wider bank.
        let mut cfg = Config::default();
        let widths: Vec<u32> = ["COZETTE_SCALED", "UNSCII_8_SCALED"]
            .into_iter()
            .map(|face| {
                cfg.chassis.bank_font_name = face.to_string();
                Cabinet::from_config(&cfg, 1024.0, 768.0).bank_width()
            })
            .collect();
        assert_eq!(widths, vec![205, 247]);
    }

    #[test]
    fn a_tape_cabinet_stamps_the_face_it_names_and_falls_back_to_its_own() {
        // The tape's letter size is the punch wheel's, so another face is
        // stamped at the same 20 px and measures differently; a face the
        // catalogue does not carry leaves the wheel its own.
        let departure = tape_metrics("DEPARTURE_MONO_SCALED", term::FontSource::Bundled);
        assert_ne!(tape_metrics("COZETTE_SCALED", term::FontSource::Bundled), departure);
        assert_eq!(tape_metrics("NO_SUCH_FACE", term::FontSource::Bundled), departure);
    }

    #[test]
    fn a_cabinet_built_from_the_shipped_profile_alone_stands_205_px_wide() {
        // The whole constructor path, profile to cabinet, with no kit handed
        // in: the sum over the annunciator's own measures -- 3 (bank_padding)
        // + 46 (numeral_width) + 16 (column_gap) + 126 (twelve characters of
        // the measured cell) + 14 (right_padding). The glow indicator draws
        // no rail, so no lane is cut for one.
        let cfg = Config::default();
        let c = Cabinet::from_config(&cfg, 1024.0, 768.0);
        assert_eq!(c.bank_width(), 3 + 46 + 16 + 126 + 14);
        assert_eq!(c.bank_width(), 205);
        assert_eq!(c.layout().crt.width, 1024.0 - 205.0);
        // The hint holds the bank at its least strip (eight characters of
        // the measured cell) beside the bare tube's floor, since the fit is
        // what gives the window back the difference.
        assert_eq!(c.min_bank_width(), 169);
        assert_eq!(c.min_inner_size(), (169 + 320, 240));

        // A hidden chassis is no bank at all, and the well takes the window.
        let mut bare = cfg.clone();
        bare.general.chassis_shown = false;
        let c = Cabinet::from_config(&bare, 1024.0, 768.0);
        assert_eq!(c.bank_width(), 0);
        assert_eq!(c.layout().crt.width, 1024.0);
        assert_eq!(c.min_inner_size(), (320, 240));
    }

    #[test]
    fn a_reload_re_measures_the_kit_and_not_only_the_geometry() {
        // `chassis.bank_font_name` moves the cell the strips are cut from, so
        // a reload that only re-applied the standing kit would keep the old
        // bank. UNSCII 8's cell is 8 px against Cozette's 6, which adds 42 px
        // to twelve characters of strip and the same 42 to the bank.
        let cfg = Config::default();
        let mut c = Cabinet::from_config(&cfg, 1024.0, 768.0);
        assert_eq!(c.bank_width(), 205);

        let mut wider = cfg.clone();
        wider.chassis.bank_font_name = "UNSCII_8_SCALED".to_string();
        let width = c.apply_config(&wider);
        assert_eq!(width, c.bank_width());
        assert_eq!(width, 205 + 42);
        assert_eq!(
            c.bank_width(),
            Cabinet::from_config(&wider, 1024.0, 768.0).bank_width()
        );
    }

    #[test]
    fn the_stock_profile_is_the_amber_appliance_over_led_strips_under_the_glow() {
        let cfg = Config::default();
        assert_eq!(cfg.chassis.shell, config::Shell::Annunciator);
        assert_eq!(cfg.chassis.channel_display, config::ChannelDisplay::Led);
        assert_eq!(channel_indicator(&cfg), ChannelIndicator::Glow);
        assert_eq!(cfg.general.led_characters, 12);
        assert!(cfg.general.chassis_shown);

        // And the whole composition, end to end, at this crate's own
        // default window size.
        let g = BankGeometry::new(
            &shell_metrics(&cfg),
            &LedMetrics::default(),
            cfg.general.led_characters,
            channel_indicator(&cfg),
        );
        let layout = WindowLayout::new(1024.0, 768.0, g.implicit_width as f64);
        assert_eq!(g.implicit_width, 184);
        assert_eq!(layout.crt.width, 840.0);
        assert_eq!(
            layout::min_inner_size(g.implicit_width, (320, 240)),
            (504, 240)
        );
    }
}
