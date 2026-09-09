//! The settings dump: everything an external settings tool needs that the
//! config file itself does not carry.
//!
//! The config file is a diff against defaults (`docs/config-format.md`), so
//! a tool showing effective values needs the defaults, the preset tables the
//! two `name` keys select among, and the value lists for the enum-shaped
//! keys. All of that lives in this crate (and, for the font catalogue, in
//! the running binary), so `robco-term --dump-settings` prints it: the
//! settings window reads this at open and states none of it itself.
//!
//! The output is TOML: `[general]` / `[screen]` / `[chassis]` / `[ssh]` /
//! `[critters]` / `[serial]` hold the fully-resolved defaults, `[ssh_host_defaults]`
//! what a fresh `[[ssh.host]]` row holds (row defaults, not a config table
//! of its own),
//! `[[screen_presets]]` / `[[chassis_presets]]` the built-in presets with
//! every field resolved (a consumer never redoes the diff-against-default
//! resolution `presets.rs` states them in), `[[fonts]]` the bundled font
//! catalogue, and `[values]` the admissible strings for each enum key.
//!
//! What the machine has installed is [`dump_fonts_only`], a `[[fonts]]`
//! document of its own, because that answer costs a walk of the platform's
//! font directories and this one does not: a tool asks for it when the user
//! is choosing a system font and not on every refresh.

use serde::Serialize;

use crate::schema::{
    ChannelDisplay, ChannelIndicator, ChassisSettings, GeneralSettings, Rasterization,
    ScreenSettings, SelectionModel, Shell, CritterTiming,
};

/// One font catalogue entry: the key settings persist and the label a menu
/// shows for it.
#[derive(Debug, Clone, Serialize)]
pub struct FontListing {
    pub name: String,
    pub text: String,
}

#[derive(Serialize)]
struct Values {
    rasterization: Vec<Rasterization>,
    shell: Vec<Shell>,
    channel_indicator: Vec<ChannelIndicator>,
    channel_display: Vec<ChannelDisplay>,
    selection_model: Vec<SelectionModel>,
    timing: Vec<CritterTiming>,
}

#[derive(Serialize)]
struct Dump {
    general: GeneralSettings,
    screen: ScreenSettings,
    chassis: ChassisSettings,
    ssh: crate::schema::SshSettings,
    critters: crate::schema::CritterSettings,
    serial: crate::schema::SerialSettings,
    bindings: crate::schema::BindingsSettings,
    /// What a fresh `[[ssh.host]]` row holds before the user types: the
    /// shipped `[ssh]` table has no rows, so the per-row defaults appear
    /// nowhere else in this dump.
    ssh_host_defaults: crate::schema::SshHost,
    screen_presets: Vec<ScreenSettings>,
    chassis_presets: Vec<ChassisSettings>,
    fonts: Vec<FontListing>,
    values: Values,
}

/// The whole dump as a TOML document. `fonts` comes from the caller because
/// the catalogue lives in the binary, not in this crate.
pub fn dump(fonts: Vec<FontListing>) -> String {
    let dump = Dump {
        general: GeneralSettings::default(),
        screen: ScreenSettings::default(),
        chassis: ChassisSettings::default(),
        ssh: crate::schema::SshSettings::default(),
        critters: crate::schema::CritterSettings::default(),
        serial: crate::schema::SerialSettings::default(),
        bindings: crate::schema::BindingsSettings::default(),
        ssh_host_defaults: crate::schema::SshHost::default(),
        screen_presets: crate::presets::screen_presets(),
        chassis_presets: crate::presets::chassis_presets(),
        fonts,
        values: Values {
            rasterization: RASTERIZATIONS.to_vec(),
            shell: SHELLS.to_vec(),
            channel_indicator: CHANNEL_INDICATORS.to_vec(),
            channel_display: CHANNEL_DISPLAYS.to_vec(),
            selection_model: SELECTION_MODELS.to_vec(),
            timing: CRITTER_TIMINGS.to_vec(),
        },
    };
    toml_edit::ser::to_string_pretty(&dump).expect("settings dump serializes")
}

/// A `[[fonts]]` table and nothing else.
///
/// The catalogue half of the dump asked for on its own, because the two
/// halves cost different things: the defaults, presets and value lists are
/// this crate's own constants, while the machine's monospace families are a
/// walk of the platform's font directories. A tool that wants the second
/// half says so, and a tool that wants the settings does not pay for it.
#[derive(Serialize)]
struct FontsOnly {
    fonts: Vec<FontListing>,
}

pub fn dump_fonts_only(fonts: Vec<FontListing>) -> String {
    toml_edit::ser::to_string_pretty(&FontsOnly { fonts }).expect("font dump serializes")
}

// Every variant of each enum-shaped key, in declared order. The exhaustive
// matches in `all_variants_are_listed` are what keep these lists honest: a
// new variant fails to compile there until it is added here.
const RASTERIZATIONS: [Rasterization; 5] = [
    Rasterization::NoRasterization,
    Rasterization::ScanlineRasterization,
    Rasterization::PixelRasterization,
    Rasterization::SubpixelRasterization,
    Rasterization::ModernRasterization,
];
const SHELLS: [Shell; 3] = [Shell::Annunciator, Shell::SlideRule, Shell::Switchboard];
const CHANNEL_INDICATORS: [ChannelIndicator; 3] = [
    ChannelIndicator::Glow,
    ChannelIndicator::Pointer,
    ChannelIndicator::Switch,
];
const CHANNEL_DISPLAYS: [ChannelDisplay; 2] = [ChannelDisplay::Led, ChannelDisplay::Tape];
const SELECTION_MODELS: [SelectionModel; 2] = [SelectionModel::Konsole, SelectionModel::Rio];
const CRITTER_TIMINGS: [CritterTiming; 2] = [CritterTiming::Clock, CritterTiming::Random];

#[cfg(test)]
mod tests {
    use super::*;

    /// Compile-time drift guard for the `[values]` lists: adding an enum
    /// variant breaks one of these matches until the list above grows too.
    #[allow(dead_code)]
    fn all_variants_are_listed(
        r: Rasterization,
        s: Shell,
        i: ChannelIndicator,
        d: ChannelDisplay,
        m: SelectionModel,
        t: CritterTiming,
    ) {
        match r {
            Rasterization::NoRasterization
            | Rasterization::ScanlineRasterization
            | Rasterization::PixelRasterization
            | Rasterization::SubpixelRasterization
            | Rasterization::ModernRasterization => {}
        }
        match s {
            Shell::Annunciator | Shell::SlideRule | Shell::Switchboard => {}
        }
        match i {
            ChannelIndicator::Glow | ChannelIndicator::Pointer | ChannelIndicator::Switch => {}
        }
        match d {
            ChannelDisplay::Led | ChannelDisplay::Tape => {}
        }
        match m {
            SelectionModel::Konsole | SelectionModel::Rio => {}
        }
        match t {
            CritterTiming::Clock | CritterTiming::Random => {}
        }
    }

    #[test]
    fn dump_parses_back_and_carries_the_preset_lists() {
        let text = dump(vec![FontListing {
            name: "TERMINESS_SCALED".into(),
            text: "Terminess".into(),
        }]);
        let doc: toml_edit::DocumentMut = text.parse().expect("dump is valid TOML");

        let screens = doc["screen_presets"].as_array_of_tables().unwrap();
        assert_eq!(screens.len(), crate::presets::screen_presets().len());
        // Fully resolved: a preset table carries every screen key, not just
        // the diff presets.rs states it as.
        let deep_blue = screens
            .iter()
            .find(|t| t["name"].as_str() == Some("Deep Blue"))
            .expect("Deep Blue is a built-in screen");
        assert!(deep_blue.contains_key("brightness"));
        assert!(deep_blue.contains_key("font_name"));

        let chassis = doc["chassis_presets"].as_array_of_tables().unwrap();
        assert_eq!(chassis.len(), crate::presets::chassis_presets().len());

        assert_eq!(doc["screen"]["name"].as_str(), Some("Default Amber"));
        assert_eq!(doc["chassis"]["name"].as_str(), Some("Annunciator"));
        assert_eq!(
            doc["fonts"].as_array_of_tables().unwrap().iter().next().unwrap()["name"].as_str(),
            Some("TERMINESS_SCALED")
        );
        let rasterizations = doc["values"]["rasterization"].as_array().unwrap();
        assert_eq!(rasterizations.len(), RASTERIZATIONS.len());
        assert_eq!(rasterizations.iter().next().unwrap().as_str(), Some("no_rasterization"));
        assert_eq!(
            doc["values"]["shell"].as_array().unwrap().iter().nth(1).unwrap().as_str(),
            Some("slide-rule")
        );
    }

    #[test]
    fn the_font_dump_is_the_catalogue_and_nothing_else() {
        let text = dump_fonts_only(vec![FontListing {
            name: "DejaVu Sans Mono".into(),
            text: "DejaVu Sans Mono".into(),
        }]);
        let doc: toml_edit::DocumentMut = text.parse().expect("dump is valid TOML");
        let fonts = doc["fonts"].as_array_of_tables().unwrap();
        assert_eq!(fonts.len(), 1);
        assert_eq!(
            fonts.iter().next().unwrap()["text"].as_str(),
            Some("DejaVu Sans Mono")
        );
        // Nothing of the settings dump rides along: this is the answer to a
        // different question, not that answer with a font list appended.
        assert!(!doc.contains_key("screen"));
        assert!(!doc.contains_key("screen_presets"));
        assert!(!doc.contains_key("values"));
    }
}
