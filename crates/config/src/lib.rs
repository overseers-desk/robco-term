//! `robco-config`: RobCo Terminal settings (lib name `config`; the package
//! is named `robco-config` because a bare `config` package name is too
//! easily shadowed on crates.io, even though this crate is not published).
//!
//! The TOML config file is the single source of truth for settings; a key
//! absent from it means its built-in default, so the file is a diff
//! against defaults rather than a full dump of every setting. The rules
//! that follow from that are written up for third-party tool authors in
//! `docs/config-format.md`.
//!
//! The schema half models the three persisted settings groups. See
//! `src/schema.rs` for the boundary rule and the types, and
//! `src/presets.rs` for the built-in screen and chassis presets.
//!
//! The plumbing half (`src/toml.rs`, `src/watch.rs`) is deliberately
//! schema-agnostic: it operates on `toml_edit::DocumentMut` and on
//! `T: serde::de::DeserializeOwned`, never on a concrete settings type.
//!
//! A user's saved look (a screen and chassis pair under a name) is modelled
//! in [`profile`] as the appliance split along two axes, kept one profile
//! per TOML file beside the config file: the file is the source of truth
//! here, and a roster inside one value would be a second store with its
//! own rules.

pub mod dump;
pub mod presets;
pub mod profile;
pub mod schema;
pub mod structural;
pub mod toml;
pub mod watch;

pub use profile::Profile;

pub use schema::{
    BindingsSettings, ChannelDisplay, ChannelIndicator, ChassisSettings, CritterSettings,
    CritterTiming, FontSource, GeneralSettings, KeyBinding, Rasterization, ScreenSettings,
    SerialSettings, Shell, SshHost, SshSettings,
};

/// Every settings table together: the shape a config file written by this
/// crate takes, one unit for the tables it carries.
///
/// Deliberately excludes custom profiles; see the module comment.
///
/// `#[serde(default)]` here, and on each table's own struct, is what
/// actually makes the
/// diff-against-defaults contract in `docs/config-format.md` hold for a
/// *partial* file, not just a missing one: without it, serde requires every
/// field of a struct present in the input, so a file containing only
/// `[general]\nfont_scaling = 2.0` would fail to deserialize entirely (an
/// `[general]` table missing ten other required keys), rather than filling
/// them in from `GeneralSettings::default()`. `read_document`/`deserialize`
/// in `toml.rs` already handle a wholly-missing *file* correctly (empty
/// `DocumentMut` -> this container default fills in every field); this
/// annotation is what extends the same guarantee to a present-but-partial
/// table, key by key, which is the shape every real edit takes.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct Config {
    pub general: GeneralSettings,
    pub screen: ScreenSettings,
    pub chassis: ChassisSettings,
    pub ssh: SshSettings,
    pub critters: CritterSettings,
    pub serial: SerialSettings,
    pub bindings: BindingsSettings,
}

impl Config {
    /// Read a config file and resolve it: the two axes' `name` keys select
    /// their built-in preset as the base, and every other key present
    /// overrides it ([`toml::resolve_presets`]).
    ///
    /// This is the load path for *every* config file, the user's own and a
    /// `--profile` file alike, and it is one function on purpose. The same
    /// TOML text has to mean the same look wherever it sits: a rule that
    /// applied only to profile files would make a look change meaning when
    /// it was copied into `config.toml`, which is a quiet way to lose a
    /// user's settings.
    ///
    /// A missing file is the all-defaults config, per the
    /// diff-against-defaults contract; nothing here writes anything.
    pub fn load(path: &std::path::Path) -> Result<Config, toml::ConfigError> {
        let mut doc = toml::read_document(path)?;
        warn_retired_keys(&doc);
        toml::resolve_presets(&mut doc);
        toml::deserialize(doc)
    }
}

/// Say so once when a file carries a key this schema has moved.
///
/// An unknown key is otherwise silent: the deserializer fills the field it
/// knows from the default and drops the rest, so a user whose bank font
/// stopped taking effect would have nothing to read but the bank. One line
/// naming where the setting went is the whole of it; the key itself is not
/// honoured, since a value in two tables is the arrangement being left
/// behind.
fn warn_retired_keys(doc: &::toml_edit::DocumentMut) {
    const RETIRED: &[(&str, &str, &str)] = &[(
        "general",
        "led_font_name",
        "chassis.bank_font_name, so a cabinet letters its own bank",
    )];
    for (table, key, moved_to) in RETIRED {
        if doc.get(table).and_then(|t| t.get(key)).is_some() {
            log::warn!("{table}.{key} has moved to {moved_to}");
        }
    }
}

use serde::{Deserialize, Serialize};

/// The screen radius knob maps through `lint(4.0, 120.0, raw)` to pixels;
/// this pair is that range's one home.
pub const SCREEN_RADIUS_PX: (f64, f64) = (4.0, 120.0);

impl Config {
    /// While a chassis stands, its bezel is the frame; bare, the screen
    /// wears the moulding it came in. These four accessors are that
    /// split's one home: whichever of chassis or screen is showing
    /// governs, at the pointer's end and the picture's end alike.
    pub fn raw_frame_size(&self) -> f64 {
        if self.general.chassis_shown {
            self.chassis.frame_size
        } else {
            self.screen.frame_size
        }
    }

    pub fn raw_screen_radius(&self) -> f64 {
        if self.general.chassis_shown {
            self.chassis.screen_radius
        } else {
            self.screen.screen_radius
        }
    }

    pub fn raw_frame_color(&self) -> &str {
        if self.general.chassis_shown {
            &self.chassis.frame_color
        } else {
            &self.screen.frame_color
        }
    }

    pub fn raw_frame_shininess(&self) -> f64 {
        if self.general.chassis_shown {
            self.chassis.frame_shininess
        } else {
            self.screen.frame_shininess
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    // These tests round-trip through the `toml` crate rather than the
    // `toml_edit` the crate itself uses, as an independent reader. Name it
    // explicitly: the glob above brings in this crate's own `toml` module,
    // which would otherwise win over the dependency of the same name.
    use ::toml;

    #[test]
    fn a_bindings_block_in_rio_syntax_reads_as_rows() {
        let cfg: Config = ::toml::from_str(
            r#"
[bindings]
keys = [
  { key = "b", with = "ctrl | shift", action = "fold_bank" },
  { key = "f5", esc = "[15~" },
]
"#,
        )
        .unwrap();
        assert_eq!(cfg.bindings.keys.len(), 2);
        assert_eq!(cfg.bindings.keys[0].with, "ctrl | shift");
        assert_eq!(cfg.bindings.keys[1].esc, "[15~");
        assert_eq!(cfg.bindings.keys[1].action, "", "an absent field is empty");
        let rows: Config = ::toml::from_str("[[bindings.keys]]\nkey = \"b\"\n").unwrap();
        assert_eq!(rows.bindings.keys[0].key, "b", "the array-of-tables spelling reads the same");
    }

    /// A misspelt field in a four-field row is a wrong binding, not a
    /// missing setting, so it is a parse error the reader keeps last-good
    /// over, where every other table drops what it does not know.
    #[test]
    fn a_binding_row_with_an_unknown_field_is_refused() {
        let err = ::toml::from_str::<Config>(
            "[bindings]\nkeys = [ { key = \"b\", mods = \"ctrl\" } ]\n",
        )
        .unwrap_err();
        assert!(err.to_string().contains("mods"), "{err}");
        let lenient: Config = ::toml::from_str("[general]\nno_such_key = 1\n").unwrap();
        assert_eq!(lenient, Config::default());
    }

    #[test]
    fn the_ssh_table_fills_absent_row_keys_from_the_row_default() {
        let text = r#"
[ssh]
default = "vault"

[[ssh.host]]
host = "vault"
user = "overseer"

[[ssh.host]]
host = "gw"
port = 2222
key = "/home/overseer/.ssh/id_gw"
"#;
        let cfg: Config = toml::from_str(text).unwrap();
        assert_eq!(cfg.ssh.default, "vault");
        assert_eq!(cfg.ssh.hosts.len(), 2);
        assert_eq!(cfg.ssh.hosts[0].port, 22, "an absent port is ssh's own");
        assert_eq!(cfg.ssh.hosts[1].user, "", "an absent user is the invoker's");
        assert_eq!(cfg.ssh.hosts[1].port, 2222);

        // Absent table: today's behaviour, a local shell, nothing pinned.
        let cfg: Config = toml::from_str("").unwrap();
        assert_eq!(cfg.ssh, SshSettings::default());
        assert!(cfg.ssh.default.is_empty());
    }

    #[test]
    fn a_file_still_carrying_the_retired_bank_font_key_loads() {
        // The key is not honoured, and the file is not refused for carrying
        // it: an unknown key is a key this schema does not model, which is
        // what a diff-against-defaults file is allowed to hold. The warning
        // the load emits is the user's cue, and is not a value.
        let dir = tempfile::tempdir().expect("temp dir");
        let path = dir.path().join("config.toml");
        std::fs::write(
            &path,
            "[general]\nled_font_name = \"TERMINESS_SCALED\"\nled_characters = 20\n",
        )
        .expect("seed the config");

        let cfg = Config::load(&path).expect("a file with a retired key still loads");
        assert_eq!(cfg.general.led_characters, 20);
        assert_eq!(
            cfg.chassis.bank_font_name,
            ChassisSettings::default().bank_font_name
        );
    }

    #[test]
    fn general_settings_default_round_trips_through_toml() {
        let original = GeneralSettings::default();
        let toml_string = toml::to_string(&original).unwrap();
        let restored: GeneralSettings = toml::from_str(&toml_string).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn screen_settings_default_round_trips_through_toml() {
        let original = ScreenSettings::default();
        let toml_string = toml::to_string(&original).unwrap();
        let restored: ScreenSettings = toml::from_str(&toml_string).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn chassis_settings_default_round_trips_through_toml() {
        let original = ChassisSettings::default();
        let toml_string = toml::to_string(&original).unwrap();
        let restored: ChassisSettings = toml::from_str(&toml_string).unwrap();
        assert_eq!(original, restored);
    }

    #[test]
    fn config_default_round_trips_through_toml() {
        let original = Config::default();
        let toml_string = toml::to_string(&original).unwrap();
        let restored: Config = toml::from_str(&toml_string).unwrap();
        assert_eq!(original, restored);
    }

    /// A file that names one critter and nothing else still leaves the
    /// other seven switched on: the shipped state is every piece in the
    /// cast, and retiring one is a diff against that like any other key.
    #[test]
    fn retiring_one_critter_leaves_the_rest_of_the_cast() {
        let config: Config = toml::from_str("[critters]\nlocomotive = false\n").unwrap();
        assert!(!config.critters.locomotive);
        assert!(config.critters.pacman);
        assert!(config.critters.enabled);
        assert_eq!(config.critters.minutes, 15.0);
    }

    /// A rate named without the switch is a rate and not a slow terminal:
    /// the table is a diff against defaults like any other, and the default
    /// is off. Anyone reading the file sees what it would be if switched on.
    #[test]
    fn a_baud_without_the_switch_leaves_the_line_at_full_speed() {
        let config: Config = toml::from_str("[serial]\nbaud = 300\n").unwrap();
        assert_eq!(config.serial.baud, 300);
        assert!(!config.serial.enabled);
        assert_eq!(config.serial.rate(), None);
        assert_eq!(SerialSettings::default().baud, 19200);
    }

    /// The diff-against-defaults contract (`docs/config-format.md`) means a
    /// file naming only one key of one table must deserialize, filling
    /// every other field -- in that table and the untouched ones --
    /// from the built-in defaults. This is what
    /// `#[serde(default)]` on `Config`/`GeneralSettings`/`ScreenSettings`/
    /// `ChassisSettings` buys: without it, a partial table is a hard
    /// deserialize error (serde requires every struct field present unless
    /// told otherwise).
    #[test]
    fn a_single_changed_key_deserializes_with_every_other_key_defaulted() {
        let partial = "[general]\nfont_scaling = 2.0\n";
        let restored: Config = toml::from_str(partial).unwrap();
        let mut expected = Config::default();
        expected.general.font_scaling = 2.0;
        assert_eq!(restored, expected);
    }

    /// An empty file (no tables at all) must also resolve to the frozen v1
    /// defaults -- the "zero configuration" launch experience.
    #[test]
    fn an_empty_file_resolves_to_every_default() {
        let restored: Config = toml::from_str("").unwrap();
        assert_eq!(restored, Config::default());
    }

    #[test]
    fn default_screen_is_default_amber() {
        // Startup loads this exact preset every first run, so it -- not just
        // the first list entry in the abstract -- is the schema default.
        assert_eq!(ScreenSettings::default().name, "Default Amber");
        assert_eq!(
            ScreenSettings::default(),
            presets::screen_presets()[0].clone()
        );
    }

    #[test]
    fn default_chassis_is_annunciator() {
        assert_eq!(ChassisSettings::default().name, "Annunciator");
        assert_eq!(
            ChassisSettings::default(),
            presets::chassis_presets()[0].clone()
        );
    }

    #[test]
    fn preset_counts_match_the_built_in_lists() {
        // Screens: 14 presets (Default Amber .. E-Ink).
        // Chassis: 3 presets (Annunciator, Slide Rule, Switchboard).
        assert_eq!(presets::screen_presets().len(), 14);
        assert_eq!(presets::chassis_presets().len(), 3);
    }

    #[test]
    fn every_screen_preset_round_trips_through_toml() {
        for preset in presets::screen_presets() {
            let toml_string = toml::to_string(&preset).unwrap();
            let restored: ScreenSettings = toml::from_str(&toml_string).unwrap();
            assert_eq!(
                preset, restored,
                "preset {:?} failed to round-trip",
                preset.name
            );
        }
    }

    #[test]
    fn every_chassis_preset_round_trips_through_toml() {
        for preset in presets::chassis_presets() {
            let toml_string = toml::to_string(&preset).unwrap();
            let restored: ChassisSettings = toml::from_str(&toml_string).unwrap();
            assert_eq!(
                preset, restored,
                "preset {:?} failed to round-trip",
                preset.name
            );
        }
    }
    /// `docs/config.md` documents every key and its shipped default. That
    /// table is a hand-written home for facts whose authority is the schema
    /// and the shipped presets, so it drifts silently: a field added, a
    /// default changed, and the doc still says the old thing. This pins it.
    /// `Config::default()` is the shipped profile (Default Amber over
    /// Annunciator), so its serialized keys and values are the ground truth
    /// the three tables must match, key for key and default for default.
    /// Prose descriptions stay hand-written and unchecked; only the key and
    /// the Default cell are machine facts. A generator was rejected in #4:
    /// it would not delete the lines it costs.
    #[test]
    fn docs_config_md_matches_the_shipped_defaults() {
        use std::collections::BTreeMap;

        // Ground truth: every `section.key = value` of the default config,
        // the value spelled exactly as TOML serializes it.
        let toml = toml::to_string(&Config::default()).expect("default config serializes");
        let mut truth: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut section = String::new();
        for line in toml.lines() {
            let line = line.trim();
            if let Some(name) = line.strip_prefix('[').and_then(|l| l.strip_suffix(']')) {
                section = name.to_string();
            } else if let Some((key, value)) = line.split_once(" = ") {
                truth.insert(
                    (section.clone(), key.trim().to_string()),
                    value.trim().to_string(),
                );
            }
        }

        // The doc's tables. A section opens at `### `...`[name]``; a data
        // row is `| `key` | `default` | ... |`.
        let doc = include_str!(concat!(env!("CARGO_MANIFEST_DIR"), "/../../docs/config.md"));
        let strip = |cell: &str| cell.trim().trim_matches('`').to_string();
        let mut documented: BTreeMap<(String, String), String> = BTreeMap::new();
        let mut section = String::new();
        for line in doc.lines() {
            if line.starts_with("### ") {
                section = ["general", "screen", "chassis", "ssh", "critters", "serial", "bindings"]
                    .into_iter()
                    .find(|s| line.contains(&format!("[{s}]")))
                    .unwrap_or("")
                    .to_string();
            } else if !section.is_empty() && line.starts_with("| `") {
                let cols: Vec<&str> = line.trim_matches('|').split('|').collect();
                if cols.len() >= 2 {
                    documented.insert((section.clone(), strip(cols[0])), strip(cols[1]));
                }
            }
        }

        let truth_keys: Vec<_> = truth.keys().cloned().collect();
        let doc_keys: Vec<_> = documented.keys().cloned().collect();
        assert_eq!(
            truth_keys, doc_keys,
            "docs/config.md and the schema disagree on which keys exist; a key \
             was added or removed without updating the doc table"
        );
        for (key, want) in &truth {
            assert_eq!(
                documented.get(key),
                Some(want),
                "docs/config.md lists a stale default for {}.{}: doc says {:?}, \
                 the shipped default is {:?}",
                key.0,
                key.1,
                documented.get(key),
                want
            );
        }
    }
}
