//! The done-test for the `--profile` decision: the flag names a
//! saved appliance or a built-in screen, it visibly changes the look the
//! binary comes up wearing, and a name that is neither is refused.
//!
//! The look assert is made against the real binary rather than against a
//! function, because the thing being proved is that the flag reaches the
//! look *through the startup path* -- and that path forks on
//! `--default-settings`, which is exactly where the flag used to be
//! validated and then dropped on the floor: today it watches a nonexistent
//! `config.<name>.toml` and silently runs defaults.
//!
//! It is headless in the strong sense: the child is launched with no
//! `DISPLAY` and no `WAYLAND_DISPLAY` at all, so it cannot open a window on
//! anything, least of all a developer's live desktop. The `look:` line is
//! printed before the window layer is reached, so the assert lands and the
//! child is killed; whether it could have drawn is a question for the
//! windowed check below.
//!
//! `xtask verify` checks the same two facts against a windowed binary under
//! Xvfb (its item 1 launches `--default-settings --profile <name>`, its item
//! 9 refuses "No Such Profile"); this file is the part that can run in the
//! ordinary test suite.

use std::io::{BufRead, BufReader};
use std::path::Path;
use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::sync::Mutex;
use std::time::{Duration, Instant};

use app::settings::{self, ProfileSelection};
use config::Profile;

const BINARY: &str = env!("CARGO_BIN_EXE_robco-term");

/// Guards the handful of tests below that call `std::env::set_var` on
/// `XDG_CONFIG_HOME` to steer the in-process `settings::select_profile` /
/// `settings::config_path` lookups (as opposed to the tests above, which
/// only ever set env on a spawned [`Command`] and so cannot collide with
/// anything). The var is process-global and `cargo test` runs a binary's
/// tests concurrently by default, so without this a second such test's
/// `set_var` can land between a first test's `set_var` and its later read,
/// pointing that read at the wrong test's scratch directory. Held for a
/// whole test body, not just around the `set_var` call, since the read it
/// is protecting happens later in the same test.
static XDG_CONFIG_HOME_LOCK: Mutex<()> = Mutex::new(());

/// The scratch environment a child gets: its own config and runtime
/// directories, and no display of any kind.
fn scratch_command(dir: &Path) -> Command {
    let mut command = Command::new(BINARY);
    let run = dir.join("run");
    std::fs::create_dir_all(&run).unwrap();
    std::fs::create_dir_all(dir.join("config")).unwrap();
    command
        .env("HOME", dir)
        .env("XDG_CONFIG_HOME", dir.join("config"))
        .env("XDG_DATA_HOME", dir.join("data"))
        .env("XDG_CACHE_HOME", dir.join("cache"))
        .env("XDG_RUNTIME_DIR", &run)
        .env_remove("DISPLAY")
        .env_remove("WAYLAND_DISPLAY")
        .stdin(Stdio::null());
    command
}

/// Launch the binary, read its stderr until the `look:` line appears, then
/// kill it. Returns that line.
fn launched_look(dir: &Path, args: &[&str]) -> String {
    let mut command = scratch_command(dir);
    command.args(args).arg("--verbose").stderr(Stdio::piped());
    let mut child = command.spawn().expect("the binary should launch");
    let stderr = child.stderr.take().expect("stderr was piped");

    let (tx, rx) = mpsc::channel();
    std::thread::spawn(move || {
        for line in BufReader::new(stderr).lines().map_while(Result::ok) {
            if tx.send(line).is_err() {
                break;
            }
        }
    });

    let deadline = Instant::now() + Duration::from_secs(60);
    let mut seen = Vec::new();
    let mut look = None;
    while Instant::now() < deadline && look.is_none() {
        match rx.recv_timeout(Duration::from_millis(500)) {
            Ok(line) => {
                if line.starts_with("look:") {
                    look = Some(line.clone());
                }
                seen.push(line);
            }
            Err(mpsc::RecvTimeoutError::Timeout) => {}
            Err(mpsc::RecvTimeoutError::Disconnected) => break,
        }
    }
    let _ = child.kill();
    let _ = child.wait();

    look.unwrap_or_else(|| {
        panic!(
            "no `look:` line on stderr for {args:?}; saw:\n{}",
            seen.join("\n")
        )
    })
}

/// The done-test: `--profile <known>` visibly changes the launched
/// look.
///
/// Two launches differing only in the flag, and the screen they come up
/// wearing differs accordingly -- name *and* a measure, so this cannot pass
/// on a relabelled default. `--default-settings` is on for both, which
/// makes the comparison a statement about the flag alone: no config file
/// exists in either scratch directory, so the only thing that could have
/// moved the look is the profile.
#[test]
fn a_known_profile_changes_the_launched_look() {
    let dir = tempfile::tempdir().unwrap();

    let plain = launched_look(dir.path(), &["--default-settings"]);
    assert!(
        plain.contains(r#"screen="Default Amber""#) && plain.contains(r##"font_color="#ff8100""##),
        "the unflagged launch should wear the shipped screen; got: {plain}"
    );

    let named = launched_look(
        dir.path(),
        &["--default-settings", "--profile", "Deep Blue"],
    );
    assert!(
        named.contains(r#"screen="Deep Blue""#),
        "--profile did not reach the look; got: {named}"
    );
    assert!(
        named.contains(r##"font_color="#7fb4ff""##),
        "--profile moved the name but not the measures, which is the defect \
         this test exists to catch; got: {named}"
    );
    assert_ne!(plain, named);

    // The chassis stands: applying a screen preset moves the screen and
    // leaves the cabinet alone.
    assert!(named.contains(r#"chassis="Annunciator""#), "got: {named}");
}

/// Contract item 9, checked here as well as in `xtask verify`, because the
/// refusal is deliberate rather than obvious (warning and carrying on is
/// the easier default to reach for), and an unobvious decision with no test
/// is a decision waiting to be "fixed" back to the easier default.
#[test]
fn an_unknown_profile_is_refused() {
    let dir = tempfile::tempdir().unwrap();
    let output = scratch_command(dir.path())
        .args(["--default-settings", "--profile", "No Such Profile"])
        .output()
        .expect("the binary should run");

    assert!(
        !output.status.success(),
        "an unknown profile must be refused, not silently defaulted; exit {:?}",
        output.status.code()
    );
    let stderr = String::from_utf8_lossy(&output.stderr);
    assert!(
        stderr.contains("unknown profile: No Such Profile"),
        "the refusal should name what it refused; got: {stderr}"
    );
    assert!(
        stderr.contains("Default Amber"),
        "the refusal should list what it would have accepted; got: {stderr}"
    );
}

/// A saved appliance outranks a built-in screen of the same name: it is read
/// as a whole appliance first, since that is what the user saved under a
/// name of their own; failing that it is a screen.
#[test]
fn a_saved_appliance_outranks_a_built_in_screen_of_the_same_name() {
    let _guard = XDG_CONFIG_HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    let profile_dir = settings::config_path().parent().unwrap().to_path_buf();
    std::fs::create_dir_all(&profile_dir).unwrap();

    // No file yet: "Deep Blue" is the built-in screen.
    assert!(matches!(
        settings::select_profile("Deep Blue", true),
        Ok(ProfileSelection::ScreenPreset(_))
    ));

    // Save an appliance under that same name, and it wins.
    std::fs::write(profile_dir.join("config.Deep Blue.toml"), "").unwrap();
    assert!(matches!(
        settings::select_profile("Deep Blue", true),
        Ok(ProfileSelection::SavedProfile { .. })
    ));

    // ...except under `--default-settings`, which ignores real user config
    // and so falls through to the built-in screen.
    assert!(matches!(
        settings::select_profile("Deep Blue", false),
        Ok(ProfileSelection::ScreenPreset(_))
    ));

    // And a name that is neither is refused either way.
    assert!(settings::select_profile("No Such Profile", true).is_err());
    assert!(settings::select_profile("No Such Profile", false).is_err());
}

/// A saved appliance is a preset pair plus overrides, and it reaches the
/// live handle that way: the file names two presets and moves one measure,
/// and what the app reads is those two presets with that one measure moved.
#[test]
fn a_saved_appliance_resolves_as_a_preset_pair_plus_overrides() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.workshop.toml");
    std::fs::write(
        &path,
        "[screen]\nname = \"Deep Blue\"\nbloom = 0.9\n\n[chassis]\nname = \"Slide Rule\"\n",
    )
    .unwrap();

    let handle =
        settings::SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher should start");
    let config = handle.current();

    let deep_blue = config::presets::screen_presets()
        .into_iter()
        .find(|p| p.name == "Deep Blue")
        .unwrap();
    let slide_rule = config::presets::chassis_presets()
        .into_iter()
        .find(|p| p.name == "Slide Rule")
        .unwrap();

    assert_eq!(config.screen.font_color, deep_blue.font_color);
    assert_eq!(config.screen.screen_curvature, deep_blue.screen_curvature);
    assert_eq!(config.screen.bloom, 0.9, "the override should stand");
    assert_eq!(config.chassis, slide_rule);
}

/// The resolution is not a startup trick: it survives a live edit, because
/// it is the loader the watcher reloads through. Without this, the first
/// time a user touched their profile file the look would fall back to the
/// defaults -- settings lost silently, mid-session.
#[test]
fn the_preset_resolution_survives_a_live_edit() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.workshop.toml");
    std::fs::write(&path, "[screen]\nname = \"Deep Blue\"\n").unwrap();

    let handle =
        settings::SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher should start");
    let deep_blue = config::presets::screen_presets()
        .into_iter()
        .find(|p| p.name == "Deep Blue")
        .unwrap();
    assert_eq!(handle.current().screen, deep_blue);

    // Edit the file the way every compliant writer does.
    let tmp = dir.path().join(".config.workshop.toml.tmp");
    std::fs::write(&tmp, "[screen]\nname = \"Deep Blue\"\nbloom = 0.15\n").unwrap();
    std::fs::rename(&tmp, &path).unwrap();

    let deadline = Instant::now() + Duration::from_secs(5);
    while Instant::now() < deadline && handle.current().screen.bloom != 0.15 {
        std::thread::sleep(Duration::from_millis(20));
    }

    let after = handle.current();
    assert_eq!(after.screen.bloom, 0.15, "the edit should arrive");
    assert_eq!(
        after.screen.font_color, deep_blue.font_color,
        "the named preset must survive the reload, not be dropped for the default"
    );
    assert_eq!(after.screen.name, "Deep Blue");
}

/// Keeping the look on air under a name, then coming back to it: the save
/// writes an appliance the next `--profile` finds, and reloading it gives
/// the same look back.
#[test]
fn a_saved_look_is_what_the_flag_finds_and_reloads_as_itself() {
    let _guard = XDG_CONFIG_HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);

    // A session whose look has been tuned away from the shipped one, in the
    // config directory `--profile` reads from. That is not decoration: a save
    // lands beside the file its handle watches, and in every real run that file
    // is this directory's own `config.toml` (`ProfileSelection::config_path`
    // answers `config_path()` whichever profile was named). A handle pointed
    // somewhere else would be saving somewhere else, which is the arrangement
    // this test would then be measuring instead of the one the binary has.
    let live = settings::config_path();
    std::fs::create_dir_all(live.parent().unwrap()).unwrap();
    std::fs::write(
        &live,
        "[screen]\nname = \"Deep Blue\"\nbloom = 0.9\njitter = 0.65\n\n[chassis]\nname = \"Slide Rule\"\n",
    )
    .unwrap();
    let handle = settings::SettingsHandle::spawn(live, |_, _, _| {}).expect("watcher should start");

    handle
        .save_profile_as("workshop")
        .expect("saving the current look should succeed");
    let saved = Profile::from_config(&handle.current());

    // The flag now finds it as a saved appliance.
    match settings::select_profile("workshop", true) {
        Ok(ProfileSelection::SavedProfile { look, .. }) => {
            // The selection carries the look already, read off the file the
            // save wrote: it is what `overlay` will lay over the user's own
            // config, so it is the thing worth comparing.
            let reloaded = *look;
            assert_eq!(
                reloaded, saved,
                "a saved look came back as a different look"
            );
            assert_eq!(reloaded.screen.bloom, 0.9);
            assert_eq!(reloaded.screen.jitter, 0.65);
            assert_eq!(reloaded.chassis.name, "Slide Rule");
        }
        other => panic!("expected a saved appliance, got {other:?}"),
    }
}

/// The whole point of a profile being a look: relaunching under one must not
/// take the user's general settings away.
///
/// A saved profile file carries the two axes and nothing else -- that is what
/// `save_profile_as` writes, and applying it moves exactly those two axes,
/// leaving the general settings alone. Read as a whole config instead, the
/// file's absent `[general]` would silently resolve to schema defaults, so
/// every general key the user had set would be dropped by naming a look on
/// the command line.
#[test]
fn a_general_key_survives_a_relaunch_under_a_saved_profile() {
    let _guard = XDG_CONFIG_HOME_LOCK
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let dir = tempfile::tempdir().unwrap();
    let config_home = dir.path().join("config");
    std::env::set_var("XDG_CONFIG_HOME", &config_home);
    std::fs::create_dir_all(&config_home).unwrap();

    // The user's own config: a general key set away from its default, and a
    // look that is not the one they will name.
    let live = config_home.join("robco-term").join("config.toml");
    std::fs::create_dir_all(live.parent().unwrap()).unwrap();
    std::fs::write(
        &live,
        "[general]\nfont_size = 30\n\n[screen]\nname = \"Deep Blue\"\nbloom = 0.9\n",
    )
    .unwrap();

    let default_size = config::Config::default().general.font_size;
    assert_ne!(default_size, 30, "the proof key must differ from default");

    // Save the look under a name, the way the settings window would.
    let handle =
        settings::SettingsHandle::spawn(live.clone(), |_, _, _| {}).expect("watcher should start");
    assert_eq!(handle.current().general.font_size, 30);
    handle.save_profile_as("workshop").expect("save the look");
    drop(handle);

    // Relaunch under `--profile workshop`, exactly as `main` resolves it.
    let selection = settings::select_profile("workshop", true).expect("the saved look resolves");
    let relaunched = settings::SettingsHandle::spawn_with_profile(
        selection.config_path(),
        Some(selection.clone()),
        |_, _, _| {},
    )
    .expect("watcher should start");
    let config = relaunched.current();

    // The general key the user set is still theirs...
    assert_eq!(
        config.general.font_size, 30,
        "naming a look took the user's general settings away"
    );
    // ...and the look the flag named is on.
    assert_eq!(config.screen.bloom, 0.9);
    assert_eq!(config.screen.name, "Deep Blue");

    // The file being watched is the user's own config, not the profile: that
    // is where a general key lives, so that is what a live edit must reach.
    assert_eq!(selection.config_path(), live);
}
