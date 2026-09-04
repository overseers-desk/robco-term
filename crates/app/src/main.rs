//! The RobCo Terminal binary.
//!
//! Startup order is load-bearing at three points:
//!
//! 1. **Identity before anything else.** The binary's basename decides the
//!    WM_CLASS, the instance lock and the data directory, so it is
//!    resolved first and everything downstream is told what it is.
//! 2. **Crash log before the single-instance check**, so a crash in the
//!    arbitration itself is still logged.
//! 3. **Single-instance check before the event loop.** A second launch
//!    hands its request over and exits without ever opening a display
//!    connection, which is what makes it fast and what makes it work when
//!    the second invocation has no display of its own.
//!
//! The shell is the outer frame; what it puts in a window is the wgpu
//! surface and rio-vt session, behind the `shell::Surface` seam, with the
//! settings handle watching the config file alongside.

// A GUI-subsystem binary, or Windows allocates a console window beside the
// cabinet just to hold the startup log. What a double-click loses is the
// log, and the crash file is the trace that remains there; a launch from
// `cmd` keeps its output through the parent console adopted below.
#![cfg_attr(windows, windows_subsystem = "windows")]

use std::process::ExitCode;
use std::sync::Arc;

use app::cli::{self, Outcome};
use app::instance::{self, NewWindow, Role};
use app::shell::{Shell, ShellConfig, ShellEvent};
use app::window::TerminalSurface;
use app::{crashlog, paths, settings};
use term::SessionConfig;

/// Adopt the console this process was launched from, where there is one.
///
/// The GUI subsystem starts a process with no console at all, so a launch
/// from `cmd` would run mute: version text, the startup log and a panic
/// all written to handles nobody holds. Attaching to the parent's console
/// puts them back on the screen that launched us, while a double-click,
/// having no parent console, attaches to nothing and opens no window.
/// The one middle ground Windows offers between a console binary (a
/// second window on every double-click) and a mute one.
#[cfg(windows)]
fn adopt_parent_console() {
    #[link(name = "kernel32")]
    extern "system" {
        fn AttachConsole(process_id: u32) -> i32;
    }
    const ATTACH_PARENT_PROCESS: u32 = u32::MAX;
    unsafe {
        AttachConsole(ATTACH_PARENT_PROCESS);
    }
}

/// Write to stdout, shrugging off a stream that is gone. `print!` panics
/// when its write fails, and a GUI-subsystem binary's caller may close the
/// pipe without waiting (`cmd` and PowerShell wait only for console
/// binaries), so version text was able to kill the process. Whoever closed
/// the stream has stopped reading; there is nobody to tell.
fn say(text: &str) {
    use std::io::Write;
    let _ = std::io::stdout().write_all(text.as_bytes());
}

/// Catalogue entries as the dump carries them: the key settings persist and
/// the label a menu shows for it, and nothing else either dump needs.
///
/// A face with no label is one the catalogue carries to cover another's gaps.
/// The dump is what the settings window builds its font list from, so an
/// unlabelled face has nothing to contribute to it.
fn listings(fonts: &[term::FontEntry]) -> Vec<config::dump::FontListing> {
    fonts
        .iter()
        .filter(|f| !f.text.is_empty())
        .map(|f| config::dump::FontListing {
            name: f.name.to_string(),
            text: f.text.to_string(),
        })
        .collect()
}

fn main() -> ExitCode {
    #[cfg(windows)]
    adopt_parent_console();

    let identity = app::identity();

    let options = match cli::parse(&identity, std::env::args_os().skip(1)) {
        Outcome::Run(options) => options,
        Outcome::Print(text) => {
            say(&text);
            return ExitCode::SUCCESS;
        }
        Outcome::Fail(text) => {
            use std::io::Write;
            let _ = std::io::stderr().write_all(text.as_bytes());
            return ExitCode::FAILURE;
        }
    };

    // The settings dump is pure output: no window, no instance lock, no
    // config file touched. Handled before anything else starts so external
    // tools can call it while a terminal is running.
    //
    // The bundled catalogue only. What the machine has installed is the other
    // dump below, because it is the one that costs a walk of the platform's
    // font directories: a settings tool refreshing its view pays for that
    // when it asks for it and not otherwise.
    if options.dump_settings {
        say(&config::dump::dump(listings(term::bundled_fonts())));
        return ExitCode::SUCCESS;
    }

    // The machine's own monospace families, as a `[[fonts]]` document:
    // the ones this terminal's renderer resolves, which is why the settings
    // window asks this binary instead of enumerating fonts itself. This is
    // the scan, asked for by name: the enumeration runs here and the
    // process exits with it, having opened no window and read no config.
    if options.list_renderable_fonts {
        let installed: Vec<_> = term::system_fonts()
            .iter()
            .filter(|f| f.is_system)
            .cloned()
            .collect();
        say(&config::dump::dump_fonts_only(listings(&installed)));
        return ExitCode::SUCCESS;
    }

    // `--settings` is the settings window and nothing else: no glass, no
    // instance lock, no config watch. Handled here, beside the dump, because
    // everything below this line is the terminal starting up and none of it
    // is wanted.
    //
    // Where the window is linked into this binary, this process becomes it:
    // `settings_embed::run` hands the interpreter its argv and the payload
    // and does not come back. Where it is not, the companion application is
    // started and waited on, so `--settings` means the same thing on every
    // platform the terminal runs on rather than being a Windows-only
    // spelling. `--settings-selftest` is the same door with the interpreter
    // asked to prove it came up and leave; on a build with no interpreter in
    // it there is nothing to prove, and the stub's refusal is the answer.
    if options.settings || options.settings_selftest {
        let selftest = options.settings_selftest;
        #[cfg(all(windows, feature = "embedded-settings"))]
        {
            // The diagnostics file is for an interpreter that fails before it
            // has a window to fail in: it goes where the rest of this
            // identity's per-user files go.
            let diagfile = paths::data_dir(&identity).join("settings.log");
            return app::settings_embed::run(
                if selftest { &["--selftest"] } else { &[] },
                &diagfile,
            );
        }
        #[cfg(not(all(windows, feature = "embedded-settings")))]
        {
            if selftest {
                // Nothing linked in to test. The stub says so in one line,
                // which is the honest answer to a proof asked of a build that
                // carries nothing to prove.
                return app::settings_embed::run(&["--selftest"], std::path::Path::new(""));
            }
            let (program, args) = app::settings::settings_command();
            let mut command = std::process::Command::new(&program);
            command.args(args);
            // The spawner names itself, as the right-click spawn does, so
            // the window's font fetch asks this terminal and not whichever
            // robco-term a PATH holds.
            if let Ok(me) = std::env::current_exe() {
                command.env("ROBCO_SETTINGS_TERMINAL", me);
            }
            match command.status() {
                Ok(status) => {
                    return if status.success() {
                        ExitCode::SUCCESS
                    } else {
                        ExitCode::FAILURE
                    }
                }
                Err(e) => {
                    eprintln!("could not start {}: {e}", program.display());
                    return ExitCode::FAILURE;
                }
            }
        }
    }

    // `term` at warn so the glass crate's own news is heard: it says out
    // loud when a face will not load or the pty will not take a write, and
    // a filter naming only the binary and its library leaves that unsaid
    // unless the run happens to carry RUST_LOG.
    let mut builder = env_logger::Builder::from_env(
        env_logger::Env::default().default_filter_or("robco_app=info,app=info,term=warn"),
    );
    if options.verbose {
        builder.filter_level(log::LevelFilter::Debug);
    }
    let _ = builder.try_init();

    let crash_log = crashlog::install(&paths::crash_dir(&identity));
    if options.verbose {
        match &crash_log {
            Some(path) => eprintln!("crash log armed: {}", path.display()),
            None => eprintln!("crash log not armed"),
        }
    }

    // The process's own working directory is what a PTY child inherits, so
    // setting it once at startup, before anything spawns, is enough.
    if let Some(workdir) = &options.workdir {
        if let Err(e) = std::env::set_current_dir(workdir) {
            eprintln!("--workdir {}: {e}", workdir.display());
            return ExitCode::FAILURE;
        }
    }

    // `--profile <name>` names a saved appliance or a built-in screen, in
    // that order, and an unknown name is refused rather than papered over
    // with the default (see `settings::ProfileSelection` for both halves of
    // that). Resolved before the single-instance handoff, so a bad name is
    // a bad name whether or not another instance is already up.
    //
    // Under `--default-settings` the saved-appliance half is off: that flag
    // means "ignoring any real user config", and a saved profile is real
    // user config.
    let profile = match &options.profile {
        None => None,
        Some(name) => match settings::select_profile(name, !options.default_settings) {
            Ok(selection) => Some(selection),
            Err(unknown) => {
                eprintln!("unknown profile: {}", unknown.name);
                eprintln!("available profiles:");
                for name in unknown.known_screens {
                    eprintln!("  {name}");
                }
                return ExitCode::FAILURE;
            }
        },
    };
    if options.verbose {
        match &profile {
            Some(settings::ProfileSelection::SavedProfile { path, .. }) => {
                eprintln!("profile: saved appliance {}", path.display());
            }
            Some(settings::ProfileSelection::ScreenPreset(preset)) => {
                eprintln!("profile: built-in screen {:?}", preset.name);
            }
            None => {}
        }
    }
    if options.verbose {
        eprintln!("default settings: {}", options.default_settings);
        if let Some(command) = &options.command {
            eprintln!("command: {:?} {:?}", command.program, command.args);
        }
    }

    let request = NewWindow {
        fullscreen: options.fullscreen,
        ssh: options.ssh.clone(),
    };
    let role = instance::acquire(&identity, request);
    if matches!(role, Role::Delivered) {
        // A primary took the new-window request: from the launcher's point
        // of view the window was opened.
        return ExitCode::SUCCESS;
    }

    // The config file behind a live handle. `--default-settings`
    // is the contract's "never touch the user's real config" switch, so
    // that is the one case where nothing is loaded and nothing is watched.
    // This is attached to every `TerminalSurface` the shell opens
    // (below), so the pointer's inverse-distortion transform reads live
    // margin/frame/curvature settings rather than the identity
    // placeholder.
    let settings_handle = if options.default_settings {
        None
    } else {
        let path = match &profile {
            Some(selection) => selection.config_path(),
            None => settings::config_path(),
        };
        match settings::SettingsHandle::spawn_with_profile(
            path.clone(),
            profile.clone(),
            |_old, new, class| {
                log::debug!(
                    "settings applied ({class:?}); font_scaling={} bloom={}",
                    new.general.font_scaling,
                    new.screen.bloom
                );
            },
        ) {
            Ok(handle) => {
                let handle = Arc::new(handle);
                if let Err(err) = settings::install_sigusr1_handler(Arc::clone(&handle)) {
                    log::error!("could not install SIGUSR1 handler: {err}");
                }
                if options.verbose {
                    let initial = handle.current();
                    eprintln!(
                        "settings: {} (screen={:?} chassis={:?})",
                        path.display(),
                        initial.screen.name,
                        initial.chassis.name
                    );
                }
                Some(handle)
            }
            Err(err) => {
                log::error!(
                    "could not start watching {}: {err}; continuing with defaults, no live reload",
                    path.display()
                );
                None
            }
        }
    };

    // What the first window's minimum-size hint reserves for the channel bank,
    // before any surface exists to measure one: the cabinet the shipped or the
    // user's profile asks for. The bank's width does not depend on the window's
    // size, so the default size is only there to build a cabinet at all. Every
    // later change -- a settings reload, a seam drag -- reaches the shell as
    // `ShellEvent::SetBankWidth` from the surface that measured it.
    let initial_config = match &settings_handle {
        Some(handle) => handle.current(),
        None => {
            // `--default-settings`: no file is read and none is watched, so
            // the overlay the handle's loader would have applied has to be
            // applied here instead. This is contract item 1's second half,
            // "seeded from the named built-in profile": without it the flag
            // pair would start from the defaults and quietly ignore the
            // name it was given.
            let mut config = config::Config::default();
            if let Some(selection) = &profile {
                selection.overlay(&mut config);
            }
            config
        }
    };

    // Whether this run may letter its cabinet in a face off the machine,
    // decided once, here, from the config that resolved above and before any
    // window exists to paint a label. A profile that asks for bundled faces
    // gets the bundled numeral face for the pager's words and counter rolls
    // too, and the process never walks the platform's font directories at
    // all. The two dumps return above this line and never decide, which is
    // what keeps `--dump-settings` scan-free.
    // Before any window is built, so the first frame is already drawn in
    // the face the flag named.
    app::window::set_font_override(options.font.clone());
    term::set_prose_family(options.prose_font.clone());

    term::fonts::system::allow_system_lettering(
        initial_config.screen.font_source == config::FontSource::SystemFonts,
    );

    // The look this run actually came up wearing, after the file, the
    // profile and the defaults have all had their say. Printed because it
    // is the only place the three meet: `--profile` is honored through two
    // different paths depending on `--default-settings`, and a line naming
    // the resolved screen is what makes "the flag changed the look" a thing
    // that can be checked rather than assumed.
    if options.verbose {
        eprintln!(
            "look: screen={:?} chassis={:?} font_color={:?} background_color={:?}",
            initial_config.screen.name,
            initial_config.chassis.name,
            initial_config.screen.font_color,
            initial_config.screen.background_color,
        );
    }
    let (default_width, default_height) = app::geometry::DEFAULT_SIZE;
    let bank = chassis::Cabinet::from_config(
        &initial_config,
        f64::from(default_width),
        f64::from(default_height),
    );
    let (bank_width, bank_minimum) = (bank.bank_width(), bank.min_bank_width());

    let (event_loop, proxy) = match Shell::event_loop() {
        Ok(pair) => pair,
        Err(e) => {
            eprintln!("cannot create the event loop: {e}");
            return ExitCode::FAILURE;
        }
    };

    // Start serving new-window requests only once there is an event loop
    // to hand them to. Until this line a second launch finds the lock
    // held but nothing listening, and degrades to an independent
    // instance -- a window either way, which is the outcome that matters.
    let _primary = match role {
        Role::Primary(mut primary) => {
            if options.verbose {
                eprintln!(
                    "single instance: primary on {}",
                    primary.socket_path().display()
                );
            }
            primary.serve(move |request| {
                let _ = proxy.send_event(ShellEvent::NewWindow(request));
            });
            Some(primary)
        }
        _ => None,
    };

    // `-e cmd args...` beats `--program`: `-e` is the one that swallows the
    // rest of the command line.
    let mut session = SessionConfig::default();
    if let Some(command) = &options.command {
        session.program = Some(command.program.to_string_lossy().into_owned());
        session.args = command
            .args
            .iter()
            .map(|a| a.to_string_lossy().into_owned())
            .collect();
    } else if let Some(program) = &options.program {
        session.program = Some(program.to_string_lossy().into_owned());
    }

    let mut shell_config = ShellConfig::empty(&identity);
    shell_config.fullscreen = options.fullscreen;
    // The flag outranks the config's default connection; both feed the
    // same field every window this process opens reads, and both feed it a
    // resolved request, so the row's key travels with its destination. The
    // flag's spelling was validated at the CLI, before any window existed;
    // one that fails anyway names nothing, and the config's default stands.
    shell_config.ssh = options
        .ssh
        .as_deref()
        .and_then(|spec| app::ssh::SshRequest::parse(spec).ok())
        .or_else(|| app::ssh::default_request(&initial_config));
    shell_config.bank_width = bank_width;
    shell_config.bank_minimum = bank_minimum;
    // At scale factor 1, which is the unit the default window size is quoted
    // in; each window re-measures against its own factor once it has one.
    shell_config.well_minimum = app::window::well_minimum_for(&initial_config, 1.0);
    // A second proxy, for the other direction: the single-instance listener's
    // hands new windows in, and each surface hands its bank width back out.
    let surface_proxy = event_loop.create_proxy();
    let frame_stats_enabled = options.frame_stats;
    let instance = app::gpu::create_instance(event_loop.owned_display_handle());
    shell_config.surface_factory = Box::new(move |window, ssh| {
        // The destination arrives resolved, keys and all; nothing is
        // re-read on the way in, and `None` opens the window on a shell.
        let mut surface =
            TerminalSurface::new(&instance, window, &session.clone(), frame_stats_enabled, ssh);
        surface.set_shell_events(surface_proxy.clone());
        // The config this run resolved, whether or not a file is behind it.
        // Under `--default-settings` there is no handle to attach and this is
        // the only thing carrying `--profile`'s look to the glass; with a
        // handle the surface prefers the watched file and never reads it.
        surface.set_config(initial_config.clone());
        if let Some(handle) = &settings_handle {
            surface.set_settings(Arc::clone(handle));
        }
        Box::new(surface)
    });

    if let Err(e) = Shell::new(shell_config).run(event_loop) {
        eprintln!("event loop stopped: {e}");
        return ExitCode::FAILURE;
    }
    ExitCode::SUCCESS
}
