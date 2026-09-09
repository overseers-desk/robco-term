//! Settings schema for the terminal's three persisted configuration groups.
//!
//! Three groups, and the boundary between them is the single place that
//! says what is persisted:
//!
//! - [`GeneralSettings`], the `[general]` table: the user's own settings.
//! - [`ScreenSettings`], the `[screen]` table: everything behind the glass.
//! - [`ChassisSettings`], the `[chassis]` table: the cabinet.
//!
//! Everything else -- values derived from other properties
//! (e.g. a frame color computed from a hex string, a screen radius computed
//! from a slider) -- is runtime/derived state, not schema, and is
//! deliberately not modelled here. A saved look (a screen and chassis pair
//! under a name) is [`crate::profile`]'s.

use serde::{Deserialize, Serialize};

/// The app-level knobs that stand apart from any screen or cabinet: window
/// chrome, performance budget, and the two properties that are the user's
/// rather than a look's. `chassisShown` is whether a cabinet is drawn at
/// all, and `ledCharacters` is the strip width a hand dragged the seam to;
/// neither is a thing a cabinet gets to choose on the user's behalf.
///
/// The face a cabinet letters its bank in is the cabinet's
/// ([`ChassisSettings::bank_font_name`]), since a bank need not be lamps: one
/// may be lettered by hand, another stamped by a label maker, and each wants
/// a face of its own.

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct GeneralSettings {
    /// How many frames the shader-effect chain skips between redraws, while
    /// somebody is at the glass. Nobody there and the picture is held
    /// instead of being animated more slowly, so this is the one cadence.
    pub effects_frame_skip: i32,
    pub window_scaling: f64,
    pub show_terminal_size: bool,
    pub font_scaling: f64,
    /// Carried for schema parity with the frozen v1 shape; this build has
    /// no menubar and nothing reads the key (docs/config.md, `[general]`).
    /// Wiring it is a deliberately open design fork, not a gap in this
    /// port.
    pub show_menubar: bool,
    pub bloom_quality: f64,
    pub burn_in_quality: f64,
    /// Carried for schema parity; nothing reads it here (docs/config.md,
    /// `[general]`). `--program`/`-e` are this build's way to run something
    /// other than the shell. Wiring it is a deliberately open design fork.
    pub use_custom_command: bool,
    /// Carried for schema parity alongside `use_custom_command`; nothing
    /// reads it (docs/config.md, `[general]`).
    pub custom_command: String,
    /// Visible characters in the channel bank strip. The user's own
    /// setting, not a profile's.
    pub led_characters: i32,
    /// Whether the chassis casting/bezel/bank is drawn around the screen at
    /// all. The user's own setting, not a profile's.
    pub chassis_shown: bool,
    /// Whether the grid measures text by grapheme cluster (DEC private
    /// mode 2027) rather than one column per code point.
    ///
    /// Off, because the programs that draw tables in a terminal measure
    /// with `wcwidth`: whiptail, tmux and a shell's line editor all place
    /// U+26A0 followed by a variation selector in one column, and a grid
    /// that gives it two puts every column after it one place right. A
    /// program that wants cluster widths can still ask for them at
    /// runtime with `CSI ? 2027 h`.
    pub grapheme_clustering: bool,
    /// Which house's selection model the pointer follows.
    pub selection_model: SelectionModel,
}

/// Which selection model the pointer follows.
///
/// The two houses disagree about what a click points at. `Konsole` points
/// at a cell and grows a range of cells. `Rio` points at the seam between
/// two cells, so a drag begun on the right half of a character leaves that
/// character out, and it brings rio's own word separators and its
/// bracket-matching double click with it.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum SelectionModel {
    Konsole,
    Rio,
}

/// Which family of rasterization the shader pipeline applies to glyphs,
/// stored as the screen's `rasterization` integer. Variant names mirror the
/// stored integers' own spelling so the TOML value reads the same word.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum Rasterization {
    NoRasterization,
    ScanlineRasterization,
    PixelRasterization,
    SubpixelRasterization,
    ModernRasterization,
}

/// Where the glyph font comes from, stored as the screen's `fontSource`
/// integer.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum FontSource {
    BundledFonts,
    SystemFonts,
}

/// The `[screen]` table. "Everything behind the glass": phosphor,
/// geometry, type, the effects that age them, and the moulding the tube
/// itself came out of the factory in (used as the frame when no chassis is
/// shown). A screen says nothing about the cabinet it may be mounted in, so
/// any screen can pair with any [`ChassisSettings`].
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ScreenSettings {
    /// Identity, not a measure. Loading an unnamed screen object falls back
    /// to the default screen's name, never to whatever name was standing
    /// (see `loadScreenObject`).
    pub name: String,
    /// Hex color string, e.g. `"#000000"`, stored under the key
    /// `backgroundColor`.
    pub background_color: String,
    /// Hex color string, stored under the key `fontColor`.
    pub font_color: String,
    pub flickering: f64,
    pub horizontal_sync: f64,
    pub static_noise: f64,
    pub chroma_color: f64,
    pub saturation_color: f64,
    pub screen_curvature: f64,
    pub glowing_line: f64,
    pub burn_in: f64,
    pub bloom: f64,
    pub rasterization: Rasterization,
    pub jitter: f64,
    pub rgb_shift: f64,
    pub brightness: f64,
    pub contrast: f64,
    pub ambient_light: f64,
    pub window_opacity: f64,
    pub font_name: String,
    pub font_source: FontSource,
    pub font_width: f64,
    pub line_spacing: f64,
    pub margin: f64,
    /// Carried for schema parity with the frozen v1 shape; the cursor does
    /// not blink in this build regardless of the value (docs/config.md,
    /// `[screen]`). Wiring it is a deliberately open design fork.
    pub blinking_cursor: bool,
    /// The screen's own moulding, stored under the key `frameSize`, used as
    /// the frame only when no chassis is shown.
    pub frame_size: f64,
    pub screen_radius: f64,
    pub frame_color: String,
    pub frame_shininess: f64,
}

/// How the chassis marks which channel is on screen: one of `"glow"` /
/// `"pointer"` / `"switch"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelIndicator {
    Glow,
    Pointer,
    Switch,
}

/// What the channel bank's windows are made of: one of `"led"` / `"tape"`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ChannelDisplay {
    Led,
    Tape,
}

/// Which component kit paints the chassis body: one of `"annunciator"` /
/// `"slide-rule"` / `"switchboard"`. Kebab-case matches the stored string
/// literals exactly (`"slide-rule"`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "kebab-case")]
pub enum Shell {
    Annunciator,
    SlideRule,
    Switchboard,
}

/// The `[chassis]` table. "The cabinet the screen is mounted in": its
/// casting, its bezel, and the way its bank marks the channel on air.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ChassisSettings {
    /// Identity, not a measure. Loading an unnamed chassis object falls
    /// back to the default chassis's name (see `loadChassisObject`).
    pub name: String,
    pub shell: Shell,
    pub channel_indicator: ChannelIndicator,
    pub channel_display: ChannelDisplay,
    pub frame_size: f64,
    pub screen_radius: f64,
    pub frame_color: String,
    pub frame_shininess: f64,
    /// The face this cabinet letters its channel bank in.
    ///
    /// A name matching no bundled face falls back to the kit's own: the lamp
    /// strip's shipped face, or the tape's, depending on which display the
    /// cabinet carries.
    pub bank_font_name: String,
}

impl Default for GeneralSettings {
    /// The values in force before a screen/chassis preset is loaded over
    /// them. Unlike screen/chassis, general settings have no preset list to
    /// fall back to, so these are the real default.
    fn default() -> Self {
        GeneralSettings {
            effects_frame_skip: 3,
            window_scaling: 1.0,
            show_terminal_size: true,
            font_scaling: 1.0,
            show_menubar: false,
            bloom_quality: 0.5,
            burn_in_quality: 0.5,
            use_custom_command: false,
            custom_command: String::new(),
            led_characters: 12,
            chassis_shown: true,
            grapheme_clustering: false,
            selection_model: SelectionModel::Konsole,
        }
    }
}

impl Default for ScreenSettings {
    /// The "Default Amber" look, the terminal's default screen: first entry
    /// of the built-in screen list, loaded on every first start (and on
    /// `--default-settings`), so it is the screen the terminal actually
    /// ships wearing. The built-in presets state themselves as diffs
    /// against this.
    fn default() -> Self {
        ScreenSettings {
            name: "Default Amber".to_string(),
            background_color: "#000000".to_string(),
            font_color: "#ff8100".to_string(),
            flickering: 0.1,
            horizontal_sync: 0.1,
            static_noise: 0.1,
            chroma_color: 0.2,
            saturation_color: 0.2,
            screen_curvature: 0.2,
            glowing_line: 0.2,
            burn_in: 0.3,
            bloom: 0.6,
            rasterization: Rasterization::NoRasterization,
            jitter: 0.2,
            rgb_shift: 0.0,
            brightness: 0.5,
            contrast: 0.8,
            ambient_light: 0.3,
            window_opacity: 1.0,
            font_name: "TERMINESS_SCALED".to_string(),
            font_source: FontSource::BundledFonts,
            font_width: 1.0,
            line_spacing: 0.1,
            margin: 0.3,
            blinking_cursor: false,
            frame_size: 0.1,
            screen_radius: 0.1,
            frame_color: "#cfcfcf".to_string(),
            frame_shininess: 0.3,
        }
    }
}

impl Default for ChassisSettings {
    /// The "Annunciator" preset, first entry of the built-in chassis list
    /// and the terminal's default chassis name. Startup loads this chassis
    /// on every first start.
    fn default() -> Self {
        crate::presets::chassis_presets()
            .into_iter()
            .next()
            .expect("chassis_presets() always yields at least Annunciator")
    }
}

/// The `[ssh]` table: the pre-configured servers a new session can start
/// on. Read at launch by the terminal; written by the settings window
/// under the machine-write contract like every other table.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct SshSettings {
    /// The `host` of the `[[ssh.host]]` row new sessions start on. Empty
    /// means localhost, a local shell, which is the shipped default and
    /// today's behaviour unchanged. A value matching no row is logged at
    /// launch and behaves as empty, so a stale name cannot cost a window.
    pub default: String,
    /// The pre-configured servers, as `[[ssh.host]]` tables.
    #[serde(rename = "host")]
    pub hosts: Vec<SshHost>,
}

/// One pre-configured server: an `[[ssh.host]]` row.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SshHost {
    pub host: String,
    /// Empty means the invoking user's own name, the reading `ssh` gives
    /// a bare hostname.
    pub user: String,
    pub port: u16,
    /// The private key file for this host, tried ahead of the agent; a
    /// leading `~/` means home, and empty falls back to the agent and the
    /// default `~/.ssh` keys (docs/ssh.md). An encrypted key waits on the
    /// passphrase prompt.
    pub key: String,
}

impl Default for SshHost {
    fn default() -> Self {
        SshHost {
            host: String::new(),
            user: String::new(),
            port: 22,
            key: String::new(),
        }
    }
}

/// The `[bindings]` table: the terminal's own chords, as the user moves
/// them. Empty is the shipped set exactly; a row with the same key and
/// modifiers as a shipped chord replaces it, a new trigger is added, and
/// the later of two rows on one trigger stands. The shipped rows and the
/// action vocabulary live in the application, which is where the actions
/// are; this crate carries the rows as the strings the file spells them
/// in.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct BindingsSettings {
    /// The rows, written as an array of inline tables:
    /// `keys = [ { key = "b", with = "ctrl | shift", action = "fold_bank" } ]`.
    pub keys: Vec<KeyBinding>,
}

/// One binding: a key, the modifiers held with it, and what it does. A row
/// carries `action` or `esc`, one of the two.
///
/// Unknown fields are refused rather than dropped, unlike every other
/// table here: a dropped field in a four-field row is a wrong binding, not
/// a missing setting, and the file's reader keeps its last good state and
/// logs, which is the loud answer a misspelt field wants.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, Default)]
#[serde(default, deny_unknown_fields)]
pub struct KeyBinding {
    /// A single character, matched to the key the layout produces without
    /// regard to case, or a named key: `pageup`, `f5`, `enter`, `digit`.
    pub key: String,
    /// Modifiers joined by `|`: `ctrl`, `shift`, `alt`, `super`, and
    /// `chord` for the platform's chord modifier. Empty means none.
    pub with: String,
    /// An action name, or one of the two unbinds: `pass` lets the key
    /// reach the program, `swallow` sends nothing.
    pub action: String,
    /// An escape sequence to write to the program in place of an action:
    /// the bytes after the leading `ESC`, so `"[15~"` sends F5.
    pub esc: String,
}

/// The `[serial]` table: the speed a local shell's output is taken at.
///
/// Unset, a build behaves as one without any of this in it. Set, the read
/// loop takes what the rate has earned and leaves the rest in the tty
/// buffer, so a program blocks in `write` once that fills, the way it would
/// behind a serial line. It is the reading that is slowed and never the
/// drawing, so what the glass shows is what the session sent, whether the
/// line is slow or not. `term::Session::set_rate` carries the rest.
///
/// A remote shell is not reached by this. An SSH channel's bytes and a tmux
/// pane's have been read off their transport by the time they arrive, and
/// holding them back then would be a picture of a slow line rather than one.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SerialSettings {
    /// Whether output is metered at all.
    pub enabled: bool,
    /// Bits a second. Eight data bits, no parity and one stop bit is ten
    /// bits a character, so a tenth of this is the bytes a second the
    /// terminal reads; 19200 fills an 80 by 25 screen in a second.
    pub baud: u32,
}

impl Default for SerialSettings {
    fn default() -> Self {
        SerialSettings {
            enabled: false,
            baud: 19200,
        }
    }
}

impl SerialSettings {
    /// Bytes a second for the read loop, or `None` for as fast as the child
    /// writes. The one place the ten bits a character are divided out.
    pub fn rate(&self) -> Option<u32> {
        self.enabled.then(|| self.baud / 10)
    }
}

/// Where in its interval a critter arrives: at the start, on the clock's own
/// mark, or at a random point inside it. `docs/config.md` carries what each
/// means to a user.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum CritterTiming {
    Clock,
    Random,
}

/// The `[critters]` table: whether the appliance amuses itself, when, and
/// with which of its eight pieces.
///
/// One key per piece rather than a list: the settings window's rows are keys.
/// The names are `critters::ART`'s, held to it by a test in `crates/app`.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct CritterSettings {
    /// Whether anything crosses the glass at all.
    pub enabled: bool,
    pub timing: CritterTiming,
    /// Minutes between one critter and the next. [`CritterTiming`] settles
    /// where in that interval it lands.
    pub minutes: f64,
    pub dolphins: bool,
    pub ducks: bool,
    pub swan: bool,
    pub whale: bool,
    pub ship: bool,
    pub monster: bool,
    pub pacman: bool,
    pub locomotive: bool,
}

impl Default for CritterSettings {
    fn default() -> Self {
        CritterSettings {
            enabled: true,
            timing: CritterTiming::Clock,
            minutes: 15.0,
            dolphins: true,
            ducks: true,
            swan: true,
            whale: true,
            ship: true,
            monster: true,
            pacman: true,
            locomotive: true,
        }
    }
}
