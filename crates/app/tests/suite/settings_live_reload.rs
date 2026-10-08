//! Settings live-reload, proved directly against the `SettingsHandle` seam
//! (there's no real render loop to watch re-render here, since the window
//! integration isn't exercised by this test binary, so this test
//! demonstrates the new value reaching the app's settings handle instead):
//!
//! 1. Editing the config file (atomic write-temp-then-rename, per the
//!    machine-write contract) delivers the new value to `handle.current()`
//!    without restarting the process, using `general.font_size` as the
//!    proof key.
//! 2. An invalid edit keeps the last-good value and logs loudly.
//! 3. SIGUSR1 forces a reload, independent of the filesystem watch.
//! 4. The parameter/structural classification lines up with a live edit:
//!    a font_size change classifies `Structural` (it resizes the glyph
//!    atlas, which forces a chain rebuild); a brightness-only change
//!    classifies `Parameter` (a plain uniform push).

use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use app::settings::{self, KeyClass, SettingsHandle};

/// A `log::Log` implementation that captures every record's formatted
/// message, so the "invalid edit ... logs" half of the done-test can be
/// asserted on rather than eyeballed.
struct CapturingLogger;

static MESSAGES: OnceLock<Mutex<Vec<String>>> = OnceLock::new();
static LOGGER: CapturingLogger = CapturingLogger;

fn messages() -> &'static Mutex<Vec<String>> {
    MESSAGES.get_or_init(|| Mutex::new(Vec::new()))
}

impl log::Log for CapturingLogger {
    fn enabled(&self, _metadata: &log::Metadata) -> bool {
        true
    }
    fn log(&self, record: &log::Record) {
        // Recover from poisoning rather than unwrapping. This logger is
        // called from the watcher's own thread, so a mutex poisoned by a
        // failing assertion on the test thread would turn one test failure
        // into a panic-during-panic and abort the whole binary, hiding
        // which test actually failed.
        let mut messages = messages().lock().unwrap_or_else(|e| e.into_inner());
        messages.push(format!("{}", record.args()));
    }
    fn flush(&self) {}
}

fn init_logger() {
    // `set_logger` errors if called twice; tests in this binary share a
    // process, so ignore the "already set" error rather than treating it
    // as a test failure.
    let _ = log::set_logger(&LOGGER);
    log::set_max_level(log::LevelFilter::Info);
}

/// Whether the watcher has logged its rejection of a bad config since
/// `from`. The message buffer is shared by every test in this binary, so
/// this looks for the line rather than for the buffer growing.
fn logged_the_rejection(from: usize) -> bool {
    messages()
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .iter()
        .skip(from)
        .any(|m| m.contains("last-good") || m.contains("failed"))
}

fn wait_until(mut predicate: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        if predicate() {
            return true;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    predicate()
}

/// Write `contents` to `path` via the same write-temp-then-rename sequence
/// every compliant writer in the config contract must use.
fn write_atomic(path: &std::path::Path, contents: &str) {
    let tmp = path.with_extension("tmp");
    fs::write(&tmp, contents).unwrap();
    fs::rename(&tmp, path).unwrap();
}

#[test]
fn file_edit_reaches_the_settings_handle_without_restart() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // Start from an empty file: per the config contract, absence means
    // "every setting takes its default."
    fs::write(&path, "").unwrap();

    // Watchers can legitimately fire more than once for a single
    // write-temp-then-rename (a `Create` and a `Modify` event for the same
    // rename, on some platforms/filesystems), so a second reload can
    // observe `old == new` and classify as `Parameter` even though the
    // edit as a whole was structural. Collect every class seen rather than
    // asserting on only the last one.
    let classes_seen = Arc::new(Mutex::new(Vec::new()));
    let classes_seen_cb = Arc::clone(&classes_seen);
    let handle = SettingsHandle::spawn(path.clone(), move |_old, _new, class| {
        classes_seen_cb.lock().unwrap().push(class);
    })
    .expect("watcher should start");

    let default_font_size = config::Config::default().general.font_size;
    assert_eq!(handle.current().general.font_size, default_font_size);

    // `general.font_size` is this test's proof key.
    write_atomic(&path, "[general]\nfont_size = 30\n");

    assert!(
        wait_until(
            || handle.current().general.font_size == 30,
            Duration::from_secs(5)
        ),
        "editing the config file did not reach the settings handle within the timeout; \
         current value is {}",
        handle.current().general.font_size
    );

    // Font size resizes the glyph atlas, so it is structural per the split
    // in `settings::classify` -- still a visible change delivered without
    // restart, just one that also resets the burn-in ghost. The
    // reset is not driven from this classification: the atlas rebuild in
    // `TerminalSurface::apply_live_settings` calls `burn_in().restart()`
    // itself, and the chain decides rebuild-vs-uniforms by comparing its own
    // `Structure`. What is asserted here is the classification, which is what
    // a host reads.
    assert!(
        classes_seen.lock().unwrap().contains(&KeyClass::Structural),
        "expected a Structural classification among {:?}",
        classes_seen.lock().unwrap()
    );
}

#[test]
fn parameter_only_edit_reaches_the_handle_and_classifies_as_parameter() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "").unwrap();

    let last_class = Arc::new(Mutex::new(None));
    let last_class_cb = Arc::clone(&last_class);
    let handle = SettingsHandle::spawn(path.clone(), move |_old, _new, class| {
        *last_class_cb.lock().unwrap() = Some(class);
    })
    .expect("watcher should start");

    write_atomic(&path, "[screen]\nbrightness = 1.7\n");

    // Wait on the callback, not on `current()`. The watcher publishes the
    // new snapshot before it calls back, so polling `current()` can win the
    // race and leave the classification not yet recorded.
    assert!(wait_until(
        || *last_class.lock().unwrap() == Some(KeyClass::Parameter),
        Duration::from_secs(5)
    ));
    assert_eq!(handle.current().screen.brightness, 1.7);
}

#[test]
fn invalid_edit_keeps_last_good_and_logs() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "[general]\nfont_size = 28\n").unwrap();

    let handle = SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher should start");
    assert_eq!(handle.current().general.font_size, 28);

    let messages_before = messages().lock().unwrap().len();
    write_atomic(&path, "this is not [valid toml at all");

    // Give the watcher a chance to observe and reject the bad write; there
    // is no successful-reload signal to wait on here (that's the point), so
    // the rejection's own log line is the signal. Waiting on the buffer
    // merely *growing* is not enough: the buffer is process-wide and the
    // other tests in this binary run alongside, so a sibling's line would
    // end the wait before this test's rejection had been logged.
    wait_until(
        || logged_the_rejection(messages_before),
        Duration::from_secs(3),
    );

    assert_eq!(
        handle.current().general.font_size,
        28,
        "last-good value must survive an unparseable write"
    );

    assert!(
        logged_the_rejection(messages_before),
        "expected a loud log line about the failed reload; got {:?}",
        &messages().lock().unwrap_or_else(|e| e.into_inner())[messages_before..]
    );

    // A subsequent good write still recovers normally.
    write_atomic(&path, "[general]\nfont_size = 36\n");
    assert!(wait_until(
        || handle.current().general.font_size == 36,
        Duration::from_secs(5)
    ));
}

#[cfg(unix)]
#[test]
fn sigusr1_forces_a_reload() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "[general]\nfont_size = 24\n").unwrap();

    let handle =
        Arc::new(SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher should start"));
    settings::install_sigusr1_handler(Arc::clone(&handle))
        .expect("installing the SIGUSR1 handler should succeed");
    // Give the signal-handling thread a moment to actually start blocking
    // on the signal before we raise it.
    std::thread::sleep(Duration::from_millis(100));

    assert_eq!(handle.current().general.font_size, 24);

    // Write in place (not via write-temp-then-rename) so this test does not
    // depend on the directory watcher having already picked the change up;
    // SIGUSR1 must force the reload on its own.
    fs::write(&path, "[general]\nfont_size = 40\n").unwrap();

    // SAFETY: `libc::raise` sends a signal to the current process; this is
    // exactly the "for scripts" use case the config contract names for
    // SIGUSR1 (e.g. `kill -USR1 <pid>`), reproduced in-process.
    let rc = unsafe { libc::raise(libc::SIGUSR1) };
    assert_eq!(rc, 0, "raising SIGUSR1 failed");

    assert!(
        wait_until(
            || handle.current().general.font_size == 40,
            Duration::from_secs(5)
        ),
        "SIGUSR1 did not force a reload within the timeout; current value is {}",
        handle.current().general.font_size
    );
}

/// No config file on disk is the launch experience most users get, and it
/// must reach the shipped default look with zero configuration.
/// `crates/config` froze its defaults precisely for this. Resolve a
/// `Settings` (here, the `SettingsHandle`'s initial
/// snapshot) against a directory with no config file, and check every
/// field against `config::Config::default()` -- the frozen v1 default,
/// itself derived from `presets::screen_presets()[0]` /
/// `presets::chassis_presets()[0]` (see `schema.rs`'s `Default` impls) so
/// this test is driven from the schema/preset data, not a hand-copied
/// literal list that could drift from it.
#[test]
fn zero_config_launch_resolves_to_the_frozen_v1_default() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    // No `fs::write` at all: the config file does not exist.
    let path = dir.path().join("config.toml");
    assert!(!path.exists(), "test setup error: file should not exist");

    let handle = SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher should start");
    let resolved = handle.current();
    let expected = config::Config::default();

    assert_eq!(
        resolved, expected,
        "no-config resolution does not equal the frozen v1 default \
         (config::Config::default(): Default Amber screen + Annunciator \
         chassis + top-level general properties); this is a finding to \
         report, not to silently patch"
    );

    // Spelled out per-field too, so a future field added to the schema
    // without updating `Config`'s `PartialEq`/`Default` symmetry (an
    // unlikely but silent way for this guarantee to erode) still shows up
    // as a specific failing assertion rather than one opaque struct diff.
    assert_eq!(resolved.general, expected.general);
    assert_eq!(resolved.screen, expected.screen);
    assert_eq!(resolved.chassis, expected.chassis);
    assert_eq!(resolved.screen.name, "Default Amber");
    assert_eq!(resolved.chassis.name, "Annunciator");
}

#[test]
fn structural_key_change_classifies_as_structural_end_to_end() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "").unwrap();

    let seen = Arc::new(AtomicUsize::new(0));
    let classes_seen = Arc::new(Mutex::new(Vec::new()));
    let seen_cb = Arc::clone(&seen);
    let classes_seen_cb = Arc::clone(&classes_seen);
    let handle = SettingsHandle::spawn(path.clone(), move |_old, _new, class| {
        seen_cb.fetch_add(1, Ordering::SeqCst);
        classes_seen_cb.lock().unwrap().push(class);
    })
    .expect("watcher should start");

    write_atomic(&path, "[general]\nwindow_scaling = 2.0\n");

    // Same race as above: wait on the callback first, then read the value.
    assert!(wait_until(
        || seen.load(Ordering::SeqCst) >= 1,
        Duration::from_secs(5)
    ));
    assert!(wait_until(
        || handle.current().general.window_scaling == 2.0,
        Duration::from_secs(5)
    ));
    // See the comment in `file_edit_reaches_the_settings_handle_without_restart`:
    // a single edit can fire the watcher more than once, so assert
    // membership rather than the exact last class.
    assert!(
        classes_seen.lock().unwrap().contains(&KeyClass::Structural),
        "expected a Structural classification among {:?}",
        classes_seen.lock().unwrap()
    );
}

/// A three-digit hex colour, live-reloaded onto a running appliance.
///
/// The colour keys are `String`s in the schema, so nothing rejects `"#000"`
/// at load: it reaches the handle intact and only becomes an `Rgba` when the
/// next frame's uniforms are built. Reading it with Rust's slicing panicked
/// there, which on this path means the process dies partway through a reload
/// the user asked for. A malformed or short colour value must degrade
/// instead of taking the process down with it.
#[test]
fn a_short_hex_colour_survives_a_live_reload_and_the_frame_after_it() {
    init_logger();
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    fs::write(&path, "").unwrap();

    let handle = SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher should start");

    // A general key set first, so the reload below is a real one and the
    // colour is not the only thing that moved.
    write_atomic(
        &path,
        "[screen]\nbackground_color = \"#000\"\nfont_color = \"#0f0\"\n",
    );
    assert!(
        wait_until(
            || handle.current().screen.background_color == "#000",
            Duration::from_secs(5)
        ),
        "the short colour never reached the handle; it is {:?}",
        handle.current().screen.background_color
    );

    // The frame the reload asks for. A panic here is the bug; the assertion
    // is that the uniforms get built at all.
    let cfg = handle.current();
    let mut pacing = crt::Pacing::new(Instant::now());
    let t = pacing.tick_by(Duration::from_millis(16));
    let params = crt::Params::build(&cfg, &crt::Geometry::default(), t, crt::DegaussState::IDLE);
    assert!(
        params.iter().count() > 0,
        "the reloaded config built no frame"
    );

    // The chassis reads the same keys through its own copy of the parser,
    // and that copy had the same defect.
    assert_eq!(chassis::furniture::font_color(&cfg).a, 1.0);
}
