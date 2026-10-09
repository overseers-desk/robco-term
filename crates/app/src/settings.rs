//! Wiring `robco-config` into the application.
//!
//! This module owns four things:
//!
//! 1. **The platform config file path** ([`config_path`]), decided here
//!    because it was previously unowned.
//! 2. **A live [`SettingsHandle`]**: a thin wrapper over
//!    `config::watch::ConfigWatcher<config::Config>` that the event loop
//!    (`crates/app`'s window/session code) polls for the current
//!    settings snapshot, and that a SIGUSR1 handler can force-reload. The
//!    watcher itself -- atomic writes, directory watching, last-good on
//!    parse failure -- is entirely `crates/config`'s; this module adds
//!    nothing to that contract, it only seats it in the app.
//! 3. **The companion settings window** ([`SettingsApp`]): what to run to
//!    put it on the screen, and the one-at-a-time bookkeeping over the child
//!    that is running it.
//! 4. **The parameter/structural key split** ([`KeyClass`], [`classify`]):
//!    a config change either only touches
//!    live shader uniforms (cheap, no rebuild) or forces a filter-chain
//!    rebuild (framebuffer format/size or pass topology changes, which also
//!    resets the burn-in ghost, an accepted default). `crates/crt-render`
//!    is the actual consumer of this split; this module only
//!    computes it so that consumer doesn't have to re-derive it from the
//!    schema.
//!
//! Beyond spawning that one child, this module deliberately depends only on
//! `robco-config` (`config` crate) and the standard library plus `directories`/`log`/`signal-hook`, so it
//! can be lifted into whatever shape `crates/app` eventually takes.

use std::path::PathBuf;
use std::sync::Arc;

use config::toml::ConfigError;
use config::watch::ConfigWatcher;
use config::Config;
use term::distortion::{self, DistortionParams};
use term::Viewport;

/// The qualifier/organization/application triple handed to
/// `directories::ProjectDirs`. No formal org exists for this project, so
/// the qualifier and organization are left empty per the `directories`
/// crate's own convention for unaffiliated tools; only the application name
/// is meaningful.
///
/// Resulting config file path, decided here (previously unowned):
///
/// - Linux: `$XDG_CONFIG_HOME/robco-term/config.toml`
///   (falls back to `~/.config/robco-term/config.toml`)
/// - macOS: `~/Library/Application Support/robco-term/config.toml`
/// - Windows: `%APPDATA%\robco-term\config.toml`
///
/// This is the platform's per-user config base (`BaseDirs::config_dir()`:
/// XDG on Linux, Application Support on macOS, Roaming AppData on Windows)
/// with the application's own directory on it, and the same three
/// spellings the settings window computes in `settings/lib/model.tcl` --
/// the two programs share the file, so they have to share the spelling.
/// (`ProjectDirs::config_dir()` is not that spelling: on Windows it adds a
/// `config` subfolder of its own.) `--profile <name>` (the CLI contract)
/// is expected to select a sibling file, `config.<name>.toml`, in the same
/// directory; that selection is `config_path_for_profile`, used by the CLI
/// parsing.
const APPLICATION: &str = "robco-term";

/// The directory the config file (and, later, named-profile siblings) live
/// in: the platform config base joined with [`APPLICATION`].
///
/// Returns `None` only if the platform gives no home directory at all
/// (e.g. no `$HOME` and no equivalent), which `directories` treats as
/// "cannot place any per-user file"; callers should fall back to the
/// current directory in that case, matching the config contract's rule
/// that a missing config location is not fatal.
pub fn config_dir() -> Option<PathBuf> {
    directories::BaseDirs::new().map(|dirs| dirs.config_dir().join(APPLICATION))
}

/// The default config file's full path: `config_dir()/config.toml`. Falls
/// back to `./config.toml` if the platform has no home directory, so the
/// app always has *a* path to watch rather than failing to start.
pub fn config_path() -> PathBuf {
    config_dir()
        .unwrap_or_else(|| PathBuf::from("."))
        .join(config::toml::FILE_NAME)
}

/// The path for a named profile, `--profile <name>` (the CLI
/// contract): a sibling file `config.<name>.toml` in the same directory as
/// the default config file, so profiles are visually and physically
/// grouped and a directory watch on the default file's parent also covers
/// every profile file.
pub fn config_path_for_profile(name: &str) -> PathBuf {
    profile_path_in(&config_dir().unwrap_or_else(|| PathBuf::from(".")), name)
}

/// Where a saved look sits, so the reader ([`config_path_for_profile`],
/// which `--profile` goes through) and the writer
/// ([`SettingsHandle::save_profile_as`], which saves beside the file it
/// watches) cannot drift apart. The name itself is
/// `config::toml::profile_file_name`'s to spell, alongside the config
/// file's own; this function only says which directory it lands in.
pub fn profile_path_in(dir: &std::path::Path, name: &str) -> PathBuf {
    dir.join(config::toml::profile_file_name(name))
}

/// What `--profile <name>` resolved to.
///
/// The rule: a name on the command line is read as a whole appliance first,
/// since that is what the user saved under a name of their own; failing
/// that it is a screen, which the cabinet standing takes without change. So
/// the name is looked up in the user's saved profiles, then in the built-in
/// screens, in that order. A saved appliance is a TOML file beside the
/// config file.
///
/// One deliberate choice: an unknown name refuses to start (contract item 9,
/// `xtask verify`) rather than logging a warning and coming up wearing
/// whatever settings were already active. The reason is the harness rather
/// than the user: `xtask snap` screenshots by profile name, and a silent
/// fallback would file the wrong picture under the right name, which is the
/// one failure an eval harness cannot have. Refusing is also the answer that
/// cannot lose a look silently.
#[derive(Debug, Clone, PartialEq)]
pub enum ProfileSelection {
    /// A saved appliance: `config.<name>.toml` beside the config file.
    ///
    /// A look, not a config. The file holds the two axes and nothing else
    /// (that is exactly what [`SettingsHandle::save_profile_as`] writes),
    /// and applying it moves exactly those two axes, never the general
    /// settings: those are established once from the config file and
    /// `--profile` is applied over them afterwards. So this is an overlay on
    /// the user's own config, the same shape as
    /// [`ProfileSelection::ScreenPreset`], and the user's config is what gets
    /// watched.
    SavedProfile {
        /// Where the look was read from, for the `--verbose` line.
        path: PathBuf,
        look: Box<config::Profile>,
    },
    /// A built-in screen preset: applying it takes the screen and leaves the
    /// cabinet standing, so this is an overlay on the user's own config
    /// rather than a replacement for it.
    ScreenPreset(Box<config::ScreenSettings>),
}

/// Why a `--profile <name>` could not be honored. There is only one
/// reason, but it carries what the name could have been.
#[derive(Debug, Clone)]
pub struct UnknownProfile {
    pub name: String,
    /// The built-in screen names, for the message the binary prints.
    pub known_screens: Vec<String>,
}

/// Resolve `--profile <name>`: saved appliance, then built-in screen, then
/// refuse.
///
/// `user_files` is false under `--default-settings`, whose contract line
/// is "starts fresh, ignoring any real user config" (`xtask snap`'s
/// `CONTRACT`, item 1). A saved profile is real user config, so that run
/// can only name a built-in screen -- and item 1's other half, "seeded from
/// the named built-in profile", is exactly the overlay this returns.
pub fn select_profile(name: &str, user_files: bool) -> Result<ProfileSelection, UnknownProfile> {
    if user_files {
        let path = config_path_for_profile(name);
        if path.is_file() {
            // Read as a config so each axis's `name` key still selects the
            // preset it was struck from (`config::toml::resolve_presets`),
            // then keep only the two axes: the general settings a saved
            // profile does not carry must come from the user's own config,
            // not from this file's absent `[general]`.
            match Config::load(&path) {
                Ok(config) => {
                    return Ok(ProfileSelection::SavedProfile {
                        path,
                        look: Box::new(config::Profile::from_config(&config)),
                    })
                }
                // A file that will not parse is not a look this run can wear.
                // Falling through rather than returning lets a built-in
                // screen of the same name still answer, and refuses out loud
                // if none does -- the same posture as an unknown name, for
                // the same reason: a silent fallback would file the wrong
                // picture under the right name.
                Err(err) => log::error!(
                    "robco-term: saved profile {} could not be read, ignoring it: {err}",
                    path.display()
                ),
            }
        }
    }
    let presets = config::presets::screen_presets();
    if let Some(preset) = presets.iter().find(|p| p.name == name) {
        return Ok(ProfileSelection::ScreenPreset(Box::new(preset.clone())));
    }
    Err(UnknownProfile {
        name: name.to_string(),
        known_screens: presets.into_iter().map(|p| p.name).collect(),
    })
}

impl ProfileSelection {
    /// The config file this run should read and watch.
    ///
    /// The user's own, whichever kind of profile was named. Both kinds are a
    /// look laid over the general settings, and the general settings only
    /// ever live in one file.
    pub fn config_path(&self) -> PathBuf {
        config_path()
    }

    /// Put the named look on top of a config just read from that file.
    ///
    /// Applied on *every* load rather than once at startup, because the
    /// command line outranks the file for as long as the run lasts: a
    /// `--profile` selection is a later word than the file, and a live edit
    /// of the config file must not quietly take the named look back off.
    ///
    /// The two arms both leave the general settings alone: a screen preset
    /// moves the screen and leaves the cabinet standing, a saved appliance
    /// moves both axes and leaves the general settings standing. Neither
    /// touches a general key, which is why editing one in the config file
    /// still reaches a run launched under `--profile`.
    pub fn overlay(&self, config: &mut Config) {
        match self {
            ProfileSelection::ScreenPreset(preset) => config.screen = (**preset).clone(),
            ProfileSelection::SavedProfile { look, .. } => look.apply_to(config),
        }
    }
}

pub use config::structural::{classify, KeyClass};

/// Linear interpolation from `a` at `t = 0.0` to `b` at `t = 1.0`. Several
/// config values are stored as a raw `0.0..1.0` slider value, and the value
/// everything else actually reads is derived through this.
fn lint(a: f64, b: f64, t: f64) -> f64 {
    a + (b - a) * t
}

/// The screen radius in pixels: `raw_screen_radius` interpolated across
/// [`config::SCREEN_RADIUS_PX`].
pub fn screen_radius(config: &Config) -> f64 {
    let (lo, hi) = config::SCREEN_RADIUS_PX;
    lint(lo, hi, config.raw_screen_radius())
}

/// The pixel value [`term::Viewport::margin`] wants directly (scaled by
/// `scale_factor` at that boundary), with no `normalized_screen_scale`
/// factor applied on top, unlike `frame_size` below:
///
/// ```text
/// margin = lint(1.0, 40.0, screen.margin) + (1.0 - sqrt(0.5)) * screen_radius
/// ```
///
/// `screen_radius` here is the chassis- or screen-governed one above, not
/// `config.screen.screen_radius` directly.
pub fn distortion_margin(config: &Config) -> f64 {
    lint(1.0, 40.0, config.screen.margin)
        + (1.0 - std::f64::consts::FRAC_1_SQRT_2) * screen_radius(config)
}

/// The frame size at unit screen scale: `raw_frame_size * 0.05`.
///
/// One number reaches the shader under three magnitudes, and this is the
/// middle one. The setting the user moves is a 0-to-1 slider, 0.45 on the
/// shipped cabinet, reached through `Config::raw_frame_size`. This is that
/// slider at the scale the distortion is written in, 0.0225. The third is
/// this multiplied by [`term::distortion::normalized_screen_scale`], which
/// is what [`term::distortion::DistortionParams::frame_size`] wants and what
/// varies with the window.
///
/// The name says which of the three it is, since a value read into a uniform
/// expecting another of them misdraws the moulding rather than failing.
pub fn unscaled_frame_size(config: &Config) -> f64 {
    config.raw_frame_size() * 0.05
}

/// The transform a pointer position is pushed back through
/// ([`term::distortion::correct_distortion`]) for a well of `viewport`'s
/// size under `config`. No config is flat glass in a bare cabinet: zero
/// frame inset and zero curvature, the map reduced to where the grid sits
/// in the well.
///
/// Every length here is the well's physical pixels, the unit a pointer
/// position arrives in, except the curvature's scale: `crt`'s `Geometry`
/// normalises the shader's `ScreenCurvature` and `FrameSize` over the
/// well's *logical* size, so the pointer scales by that size too, or the
/// picture is bent by the scale factor more than a click is un-bent and a
/// press near the glass's edge lands a row in from the character under it.
///
/// The margin is not a term here. It reaches the pointer the same way it
/// reaches the picture, by shrinking `term_size` and so moving where the
/// renderer centres the grid in the well (`term::distortion`).
pub fn distortion_params(config: Option<&Config>, viewport: &Viewport) -> DistortionParams {
    let (grid_width, grid_height) = viewport.term_size().pixel_size();
    let width = f64::from(viewport.width);
    let height = f64::from(viewport.height);
    let scale = viewport.scale_factor.max(f64::EPSILON);
    let normalized_screen_scale =
        distortion::normalized_screen_scale(width / scale, height / scale);

    let (frame_size, screen_curvature) = match config {
        Some(config) => (
            unscaled_frame_size(config) * normalized_screen_scale,
            config.screen.screen_curvature,
        ),
        None => (0.0, 0.0),
    };

    DistortionParams {
        width,
        height,
        frame_size,
        screen_curvature,
        screen_curvature_size: distortion::SCREEN_CURVATURE_SIZE,
        normalized_screen_scale,
        total_width: f64::from(grid_width),
        total_height: f64::from(grid_height),
    }
}

/// The live handle the event loop polls, and a SIGUSR1 handler
/// force-reloads.
///
/// Wraps `config::watch::ConfigWatcher<Config>`: all the atomicity,
/// directory-watching, and keep-last-good behavior is `crates/config`'s;
/// this type only adds the app-facing entry points (`current`,
/// `force_reload`, `path`) plus the structural/parameter classification
/// hook run on every successful reload (`on_reload` passed to
/// `ConfigWatcher::spawn_with`).
pub struct SettingsHandle {
    watcher: ConfigWatcher<Config>,
    path: PathBuf,
}

impl SettingsHandle {
    /// Load `path` (a missing file is the all-defaults `Config`, per the
    /// config contract) and start watching it. `on_change` runs on every
    /// successful reload with the previous and new snapshots plus the
    /// computed [`KeyClass`].
    ///
    /// The renderer does not hang off this callback, and [`KeyClass`] is not
    /// what decides between a uniform push and a chain rebuild: `crt::Chain`
    /// makes that call itself, by comparing the `Structure` it derives from
    /// each `Config` (`Chain::apply_settings`), and
    /// `TerminalSurface::apply_live_settings` polls [`Self::current`] once a
    /// frame to hand it there. The callback runs on the watcher's thread and
    /// the device belongs to the event loop, which is why the poll and not the
    /// callback is the path a reload takes to the glass. What this hook is for
    /// is everything a *host* wants on a reload without owning the render
    /// state -- the binary logs the class here, and the live-reload
    /// integration test reads it.
    pub fn spawn(
        path: PathBuf,
        on_change: impl FnMut(&Config, &Config, KeyClass) + Send + 'static,
    ) -> notify::Result<Self> {
        Self::spawn_with_profile(path, None, on_change)
    }

    /// Like [`spawn`](Self::spawn), with `--profile <name>`'s selection
    /// applied to every snapshot.
    ///
    /// The selection is a *loader* rather than a one-off startup step
    /// because it is part of what the file means for this run: see
    /// [`ProfileSelection::overlay`]. Both kinds of selection do work here --
    /// a saved appliance moves two axes, a named screen one -- and neither
    /// moves a general key, so the file being read stays the authority on
    /// those for as long as the run lasts.
    pub fn spawn_with_profile(
        path: PathBuf,
        profile: Option<ProfileSelection>,
        mut on_change: impl FnMut(&Config, &Config, KeyClass) + Send + 'static,
    ) -> notify::Result<Self> {
        if let Some(dir) = path.parent().filter(|p| !p.as_os_str().is_empty()) {
            // The watcher watches the *directory*; it must exist before we
            // can watch it. A brand-new install has no config directory
            // yet, so create it rather than treating that as an error.
            let _ = std::fs::create_dir_all(dir);
        }

        // `Config::load`, not `toml::load`: each axis's `name` key selects
        // the built-in preset it was struck from, and the file's other keys
        // override it (`config::toml::resolve_presets`). That resolution
        // is what makes a profile "a preset pair plus overrides" rather
        // than a full blob, and it has to run on every reload, not only
        // here, or the first live edit would drop it.
        let load = move |path: &std::path::Path| {
            let mut config = Config::load(path)?;
            if let Some(profile) = &profile {
                profile.overlay(&mut config);
            }
            Ok(config)
        };

        let initial = load(&path).unwrap_or_else(|err| {
            log::error!(
                "robco-term: initial config load of {} failed, starting from built-in defaults: {err}",
                path.display()
            );
            Config::default()
        });

        let mut previous = initial.clone();
        let log_path = path.clone();
        let watcher =
            ConfigWatcher::spawn_with_loader(&path, initial, load, move |new: &Config| {
                let class = classify(&previous, new);
                log::info!(
                    "robco-term: config reloaded from {} ({class:?}); font_scaling={}",
                    log_path.display(),
                    new.general.font_scaling
                );
                on_change(&previous, new, class);
                previous = new.clone();
            })?;

        Ok(Self { watcher, path })
    }

    /// The current in-memory settings snapshot.
    pub fn current(&self) -> Config {
        self.watcher.current()
    }

    /// Read one setting without copying the whole snapshot to get at it. See
    /// [`config::watch::ConfigWatcher::with`]; `read` runs under the lock.
    pub fn with<R>(&self, read: impl FnOnce(&Config) -> R) -> R {
        self.watcher.with(read)
    }

    /// Force an immediate reload, bypassing the filesystem watch. Wire this
    /// to a SIGUSR1 handler; see [`install_sigusr1_handler`].
    pub fn force_reload(&self) {
        self.watcher.force_reload();
    }

    /// The config file path this handle is watching.
    pub fn path(&self) -> &std::path::Path {
        &self.path
    }

    /// Write one settings key back to the file this handle watches.
    ///
    /// The file is the source of truth, so a setting the application itself
    /// changes (today the seam drag's `general.led_characters`) is changed
    /// *there* and arrives back through the ordinary reload. A profile save
    /// goes through [`Self::save_profile_as`] instead: 37 keys one at a time
    /// here would be 37 reloads. Nothing writes the in-memory snapshot: a
    /// second way to set a setting is a second answer to what the setting is.
    ///
    /// One key at a time, scalars only, and no way to reach the document
    /// itself. That is the whole surface on purpose: `config::toml::write_key`
    /// carries `docs/config-format.md`'s obligations (atomic temp-then-rename,
    /// comments and unknown keys preserved, only the touched key's bytes
    /// changed), and an API that can only name one key cannot break them.
    ///
    /// The write lands in the watched directory, so the watcher picks it up and
    /// republishes the snapshot on its own thread; a caller that has already
    /// acted on the new value (the cabinet does, so the seam does not lag the
    /// hand by a round trip) sees that reload arrive carrying what it wrote,
    /// which is a no-op rather than a jump.
    pub fn write_key(&self, key: &str, value: config::toml::Scalar) -> Result<(), ConfigError> {
        config::toml::write_key(&self.path, key, value)
    }

    /// Keep the look currently on air as a saved profile under `name`.
    ///
    /// The appliance is written whole into `config.<name>.toml`, which is
    /// what `--profile <name>` will then find first. Both axes go in, the
    /// general settings stay out, and the write is the one atomic edit
    /// `config::profile::save_to` makes -- see there for why the writer
    /// grew a multi-key form rather than the save making 37 single-key
    /// writes.
    ///
    /// The file goes beside the one this handle watches, which in a real run is
    /// the directory [`config_path_for_profile`] reads from -- so what is saved
    /// here is what `--profile <name>` finds. A handle pointed somewhere else (a
    /// test's temp directory) saves there and not into the user's own config
    /// directory, which is the only behaviour that lets this be tested at all.
    pub fn save_profile_as(&self, name: &str) -> Result<(), ConfigError> {
        let profile = config::Profile::from_config(&self.current());
        config::profile::save_to(&self.look_path(name), &profile)
    }

    /// Where a look saved under `name` lands: a sibling of the watched file.
    pub fn look_path(&self, name: &str) -> PathBuf {
        let dir = self
            .path
            .parent()
            .filter(|p| !p.as_os_str().is_empty())
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("."));
        profile_path_in(&dir, name)
    }
}

/// Install a Unix SIGUSR1 handler that calls `handle.force_reload()`.
///
/// Per `docs/rebuild-stack.md`, SIGUSR1 on macOS/Linux forces a reload for
/// scripts; the file watch remains the primary mechanism because it is the
/// only one that exists on all three platforms. `crates/config` deliberately
/// leaves installing the signal handler to the binary (a process-wide,
/// one-per-binary decision); this is that installation.
///
/// Spawns a background thread that blocks on the signal and calls
/// `force_reload()` each time it fires; returns once the handler thread is
/// running. No-op signature on non-Unix so callers don't need `cfg`.
#[cfg(unix)]
pub fn install_sigusr1_handler(handle: Arc<SettingsHandle>) -> std::io::Result<()> {
    use signal_hook::consts::SIGUSR1;
    use signal_hook::iterator::Signals;

    let mut signals = Signals::new([SIGUSR1])?;
    std::thread::Builder::new()
        .name("robco-config-sigusr1".to_string())
        .spawn(move || {
            for signal in signals.forever() {
                debug_assert_eq!(signal, SIGUSR1);
                log::info!("robco-term: SIGUSR1 received, forcing config reload");
                handle.force_reload();
            }
        })?;
    Ok(())
}

#[cfg(not(unix))]
pub fn install_sigusr1_handler(_handle: Arc<SettingsHandle>) -> std::io::Result<()> {
    Ok(())
}

/// The companion settings application, a separate executable shipped beside
/// the terminal's own. The name is fixed rather than taken from
/// [`crate::identity`]: a renamed copy of the terminal binary is a second
/// identity for the terminal, and it still edits its settings with the one
/// settings application the install carries.
#[cfg(not(all(windows, feature = "embedded-settings")))]
const SETTINGS_BINARY: &str = if cfg!(windows) {
    "robco-settings.exe"
} else {
    "robco-settings"
};

/// What to run to put the settings window on the screen: a program and the
/// arguments it takes.
///
/// Two builds, one answer. Where the settings window is linked into this
/// binary there is no companion executable to find, and the program to run
/// is this one with `--settings`: a second copy of the terminal that never
/// becomes a terminal. Everywhere else it is the companion application,
/// looked for beside this binary first (so an install that is a directory of
/// files runs its own rather than whichever one happens to be earlier on the
/// PATH) and by bare name after (the fallback for a packaged install that
/// puts the two in different directories).
///
/// A separate process either way, which is the point: [`SettingsApp::open`]'s
/// one-at-a-time bookkeeping, its null stdio and its shrug at a missing
/// binary are the same code for both, and the terminal goes on running while
/// the window is up.
#[cfg(all(windows, feature = "embedded-settings"))]
pub fn settings_command() -> (std::path::PathBuf, Vec<&'static str>) {
    // A path that cannot be read is not a reason to give up: the binary's own
    // name is what a launcher would have used, and the PATH is where it looks.
    let program = std::env::current_exe()
        .unwrap_or_else(|_| std::path::PathBuf::from(format!("{}.exe", crate::identity())));
    (program, vec!["--settings"])
}

/// The same question on a build that ships the settings window beside the
/// terminal rather than inside it: the companion executable, beside this
/// binary first and by bare name after, taking no arguments. The whole rule
/// is on the embedded arm of this function.
#[cfg(not(all(windows, feature = "embedded-settings")))]
pub fn settings_command() -> (std::path::PathBuf, Vec<&'static str>) {
    let beside = std::env::current_exe()
        .ok()
        .and_then(|exe| exe.parent().map(|dir| dir.join(SETTINGS_BINARY)))
        .filter(|path| path.is_file());
    (
        beside.unwrap_or_else(|| std::path::PathBuf::from(SETTINGS_BINARY)),
        Vec::new(),
    )
}

/// The companion settings window as this process knows it: the child it
/// started, while that child is still running.
///
/// Held for two reasons at once. It is what makes the right press
/// single-instance, since a child that has not exited means the window is
/// already open, and holding it is what lets the next press reap it: a
/// `Child` dropped without a wait leaves a zombie until the terminal itself
/// exits, and the terminal is a process that can stay up for weeks.
#[derive(Default)]
pub struct SettingsApp {
    child: Option<std::process::Child>,
}

impl SettingsApp {
    /// Start the companion settings application, one at a time.
    ///
    /// The held child is asked first, and that question doubles as the reaping:
    /// a `try_wait` that answers `Some` has collected the exit status, so a
    /// settings window that has been opened and closed a hundred times leaves
    /// no line in the process table behind it.
    ///
    /// A missing binary is not an error worth a notice on the glass. The
    /// terminal is usable without its settings application, and a build from
    /// source that ran `cargo build -p robco-app` and nothing else legitimately
    /// has no `robco-settings` to run, so this says so once in the log and
    /// leaves the screen alone.
    pub fn open(&mut self) {
        if let Some(child) = self.child.as_mut() {
            match child.try_wait() {
                Ok(None) => {
                    log::debug!("the settings application is already up");
                    return;
                }
                // Reaped. A status that is not success is the trace of a
                // window that never reached the screen, and the next right
                // press is where it can still be said.
                Ok(Some(status)) => {
                    if !status.success() {
                        log::warn!("the settings application exited with {status}");
                    }
                    self.child = None;
                }
                // The status cannot be read, so whether it is still running is
                // unknown. Forgetting the handle and starting another one is
                // the wrong half of that guess to take: a second window is
                // worse than a right press that did nothing.
                Err(e) => {
                    log::debug!("could not ask after the settings application: {e}");
                    return;
                }
            }
        }

        let (program, args) = settings_command();

        // Detached from this terminal's own input: a child that inherited
        // stdin would be reading the keystrokes meant for the terminal's own
        // child, and its stdout has nothing to say to anyone.
        //
        // Its stderr is the exception, and passes through to whatever
        // launched this terminal. A settings program that dies in its own
        // startup is the one failure this code cannot see: the spawn
        // succeeded, so nothing below reports it, and a closed stderr turns
        // a legible complaint from the interpreter into a right press that
        // did nothing at all.
        let mut command = std::process::Command::new(&program);
        command
            .args(args)
            .stdin(std::process::Stdio::null())
            .stdout(std::process::Stdio::null());
        // The window asks this terminal for what only a terminal can answer
        // (the machine's faces it can render), so the spawner names itself
        // rather than leaving the child to guess among the binaries a PATH
        // may hold. An unreadable own path is not fatal: the child falls
        // back to the sibling and PATH arms it keeps for hand launches.
        if let Ok(me) = std::env::current_exe() {
            command.env("ROBCO_SETTINGS_TERMINAL", me);
        }
        match command.spawn() {
            Ok(child) => self.child = Some(child),
            Err(e) => log::warn!("could not start {}: {e}", program.display()),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The settings window computes this same path in Tcl
    /// (`settings/lib/model.tcl`), so the file's parent is the application
    /// directory itself: any segment between them (as `ProjectDirs` adds on
    /// Windows) is a file the two programs no longer share.
    #[test]
    fn config_path_ends_in_app_dir_and_config_toml() {
        let path = config_path();
        assert_eq!(path.file_name().unwrap(), "config.toml");
        assert_eq!(
            path.parent().and_then(|dir| dir.file_name()).unwrap(),
            APPLICATION,
            "expected {path:?} to sit directly in the {APPLICATION:?} directory"
        );
    }

    #[test]
    fn profile_path_is_a_sibling_of_the_default_path() {
        let default_path = config_path();
        let profile_path = config_path_for_profile("phosphor");
        assert_eq!(profile_path.parent(), default_path.parent());
        assert_eq!(profile_path.file_name().unwrap(), "config.phosphor.toml");
    }

    #[test]
    fn font_scaling_change_is_structural() {
        // Font size resizes the glyph atlas and the grid's cell geometry,
        // so it is one of the "scale" structural triggers, not a plain
        // uniform push.
        let old = Config::default();
        let mut new = old.clone();
        new.general.font_scaling = 2.0;
        assert_eq!(classify(&old, &new), KeyClass::Structural);
    }

    #[test]
    fn brightness_change_alone_is_a_parameter_change() {
        let old = Config::default();
        let mut new = old.clone();
        new.screen.brightness += 0.1;
        assert_eq!(classify(&old, &new), KeyClass::Parameter);
    }

    #[test]
    fn window_scaling_change_is_structural() {
        let old = Config::default();
        let mut new = old.clone();
        new.general.window_scaling = 2.0;
        assert_eq!(classify(&old, &new), KeyClass::Structural);
    }

    #[test]
    fn rasterization_change_is_a_parameter_change_per_r0a() {
        let old = Config::default();
        let mut new = old.clone();
        new.screen.rasterization = match old.screen.rasterization {
            config::Rasterization::ModernRasterization => config::Rasterization::NoRasterization,
            _ => config::Rasterization::ModernRasterization,
        };
        assert_eq!(classify(&old, &new), KeyClass::Parameter);
    }

    #[test]
    fn no_change_is_a_parameter_class() {
        let old = Config::default();
        let new = old.clone();
        assert_eq!(classify(&old, &new), KeyClass::Parameter);
    }

    fn approx_eq(a: f64, b: f64, eps: f64) -> bool {
        (a - b).abs() <= eps
    }

    /// The frozen v1 default is Default Amber + Annunciator, chassis
    /// shown: `distortion_margin`/`unscaled_frame_size` therefore read
    /// the *chassis*'s frame/radius (0.45/0.44), not the screen's own
    /// (0.1/0.1), and `config.screen.margin` (0.3) either way, since
    /// margin has no chassis counterpart. Expected values hand-computed
    /// from the formulas above:
    /// `screen_radius = lint(4, 120, 0.44) = 55.04`,
    /// `margin = lint(1, 40, 0.3) + (1 - 1/sqrt(2)) * 55.04 = 28.820842763492422`,
    /// `frame_size = 0.45 * 0.05 = 0.0225`.
    #[test]
    fn default_config_derives_the_default_margin_and_frame_size() {
        let config = Config::default();
        assert!(config.general.chassis_shown);
        assert!(approx_eq(screen_radius(&config), 55.04, 1e-9));
        assert!(approx_eq(
            distortion_margin(&config),
            28.820842763492422,
            1e-9
        ));
        assert!(approx_eq(unscaled_frame_size(&config), 0.0225, 1e-9));
    }

    /// With the chassis off, the screen's own frame/radius govern instead
    /// (the chassis-or-screen split of [`Config::raw_frame_size`]).
    /// Default Amber's own frame_size/screen_radius are 0.1/0.1:
    /// `screenRadius = lint(4, 120, 0.1) = 15.6`,
    /// `frameSize = 0.1 * 0.05 = 0.005`. `margin` is unaffected by
    /// `chassis_shown` (it reads `config.screen.margin` and the
    /// chassis-or-screen `screen_radius` either way; only the radius half
    /// of that sum changes).
    #[test]
    fn chassis_hidden_derives_from_the_screens_own_frame_and_radius() {
        let mut config = Config::default();
        config.general.chassis_shown = false;
        assert!(approx_eq(screen_radius(&config), 15.6, 1e-9));
        assert!(approx_eq(unscaled_frame_size(&config), 0.005, 1e-9));
        let expected_margin = 1.0 + 39.0 * 0.3 + (1.0 - std::f64::consts::FRAC_1_SQRT_2) * 15.6;
        assert!(approx_eq(distortion_margin(&config), expected_margin, 1e-9));
    }

    /// A live edit to `screen.margin` changes `distortion_margin`'s output
    /// with nothing else touched -- the property the pointer-path wiring
    /// test in `crates/app/tests` exercises end to end through a real
    /// `SettingsHandle`; this is the pure-function half of that claim.
    #[test]
    fn distortion_margin_tracks_screen_margin() {
        let mut config = Config::default();
        let before = distortion_margin(&config);
        config.screen.margin = (config.screen.margin + 0.4).min(1.0);
        let after = distortion_margin(&config);
        assert_ne!(before, after);
    }
}
