//! The seam drag, end to end through the surface the binary runs: a pointer
//! sequence in window pixels re-fits the channel bank, the new character count
//! reaches the config file, and the reload that comes back is a no-op rather
//! than a jump.
//!
//! Every piece is proven one crate down: `chassis::seam` holds the drag law,
//! `chassis::Cabinet` holds the re-measurement, `config::toml::write_key` holds
//! the file mechanics. What is
//! not proven down there is that they are wired to each other and to a real
//! settings handle, which is what this drives.
//!
//! No window and no display: `TerminalSurface::headless` plus an explicit
//! cabinet is the whole rig, the same way `pointer.rs` drives the grid.

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app::settings::SettingsHandle;
use app::shell::Surface;
use app::window::TerminalSurface;
use chassis::Cabinet;
use config::Config;
use term::{CellSize, SessionConfig, Viewport};
use winit::dpi::PhysicalPosition;
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

const CELL_W: f64 = 9.0;
const CELL_H: f64 = 18.0;
/// The window, in physical pixels at scale factor 1. Wide enough that the
/// drags below are legal ones: the seam stops where the well would fall
/// under the eighty columns of this rig's 9 px cell, which is 778 px of the
/// window before the bank has taken anything at all.
const WINDOW_W: u32 = 1440;
const WINDOW_H: u32 = 768;

/// This rig's well floor: `term::FLOOR_COLS` x `term::FLOOR_ROWS` cells of
/// 9 x 18, plus the 29 px the default profile's distortion margin takes off
/// each edge before any of them are counted.
const WELL_MINIMUM: (i32, i32) = (720 + 58, 432 + 58);

/// The shipped profile's bank: the annunciator's furniture around twelve
/// characters of the measured Cozette cell, the face that cabinet letters
/// its bank in.
const STOCK_BANK: u32 = 205;

fn scripted() -> SessionConfig {
    SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), "sleep 30".to_string()],
        working_directory: None,
        env: vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("ENV".to_string(), String::new()),
        ],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: None,
    }
}

fn surface() -> TerminalSurface {
    let viewport = Viewport::new(
        WINDOW_W,
        WINDOW_H,
        1.0,
        CellSize::new(CELL_W as f32, CELL_H as f32),
    );
    TerminalSurface::headless(&scripted(), viewport)
}

fn none() -> ModifiersState {
    ModifiersState::empty()
}

fn at(x: f64) -> PhysicalPosition<f64> {
    PhysicalPosition::new(x, f64::from(WINDOW_H) / 2.0)
}

fn cabinet(cfg: &Config) -> Cabinet {
    Cabinet::from_config(cfg, f64::from(WINDOW_W), f64::from(WINDOW_H))
}

/// Drag the seam from where it stands to `to`, the way a hand does it.
fn drag(surface: &mut TerminalSurface, from: f64, to: f64) {
    surface.mouse_pressed(MouseButton::Left, at(from), none());
    // Several motions, not one: the drag law is absolute rather than
    // accumulated, and a single jump would not tell the two apart.
    let steps = 8;
    for step in 1..=steps {
        let x = from + (to - from) * f64::from(step) / f64::from(steps);
        surface.cursor_moved(at(x), none());
    }
    surface.mouse_released(MouseButton::Left, at(to), none());
}

fn wait_until(mut done: impl FnMut() -> bool, what: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if done() {
            return;
        }
        std::thread::sleep(Duration::from_millis(10));
    }
    panic!("timed out waiting for {what}");
}

#[test]
fn a_drag_re_fits_the_bank_writes_the_count_and_survives_the_reload() {
    let dir = tempfile::tempdir().expect("temp dir");
    let path = dir.path().join("config.toml");
    // A file with a comment and an unrelated key, so the write is held to the
    // config contract rather than only to landing the number.
    fs::write(
        &path,
        "# the appliance\n[general]\nled_characters = 12  # twelve\n\n[screen]\nbloom = 0.4\n",
    )
    .expect("seed the config");

    let settings = Arc::new(SettingsHandle::spawn(path.clone(), |_, _, _| {}).expect("watcher"));
    let cfg = settings.current();
    assert_eq!(cfg.general.led_characters, 12);

    let mut surface = surface();
    surface.set_settings(Arc::clone(&settings));
    surface.set_cabinet(cabinet(&cfg));

    // The window is divided: the bank takes its column and the glass takes the
    // rest, which is what the session was resized to.
    assert_eq!(surface.cabinet().unwrap().bank_width(), STOCK_BANK);
    assert_eq!(
        surface.cabinet().unwrap().layout().crt.width,
        f64::from(WINDOW_W - STOCK_BANK)
    );

    // Grab the seam at the bank's own edge and pull it right to x = 400.
    // The characters-for-width formula: 12 + round((400 - 205) / 9) = 34
    // characters, and the bank width that count implies is
    // 3 + 46 + 16 + round(6 * 36 * 1.5) + 14 = 403.
    drag(&mut surface, f64::from(STOCK_BANK), 400.0);
    assert_eq!(surface.cabinet().unwrap().bank_width(), 403);

    // The file is the source of truth, so the drag is not real until it is
    // there -- and it is there as one changed value in a file otherwise
    // untouched, comment and all.
    wait_until(
        || fs::read_to_string(&path).unwrap().contains("= 34"),
        "the drag to reach the config file",
    );
    let written = fs::read_to_string(&path).unwrap();
    assert_eq!(
        written,
        "# the appliance\n[general]\nled_characters = 34  # twelve\n\n[screen]\nbloom = 0.4\n"
    );

    // ...and it comes back through the ordinary reload, carrying what the
    // cabinet already holds, so nothing jumps.
    wait_until(
        || settings.current().general.led_characters == 34,
        "the watcher to publish the new count",
    );
    surface.redraw();
    assert_eq!(surface.cabinet().unwrap().bank_width(), 403);
}

#[test]
fn a_press_the_seam_took_never_also_marks_the_screen() {
    let mut surface = surface();
    surface.set_cabinet(cabinet(&Config::default()));

    // On the grab strip: the seam's, and the grid sees nothing.
    surface.mouse_pressed(MouseButton::Left, at(f64::from(STOCK_BANK)), none());
    surface.cursor_moved(at(500.0), none());
    surface.mouse_released(MouseButton::Left, at(500.0), none());
    assert_eq!(surface.last_selection(), None);
    assert!(surface.cabinet().unwrap().bank_width() > STOCK_BANK);

    // Off it, out on the glass: the grid's, and the bank does not move.
    let width = surface.cabinet().unwrap().bank_width();
    surface.mouse_pressed(MouseButton::Left, at(700.0), none());
    surface.cursor_moved(at(800.0), none());
    surface.mouse_released(MouseButton::Left, at(800.0), none());
    assert_eq!(surface.cabinet().unwrap().bank_width(), width);
}

#[test]
fn a_hidden_chassis_has_no_seam_and_no_column() {
    let mut cfg = Config::default();
    cfg.general.chassis_shown = false;
    let mut surface = surface();
    surface.set_cabinet(cabinet(&cfg));

    let bank = surface.cabinet().unwrap();
    assert_eq!(bank.bank_width(), 0);
    assert!(!bank.is_shown());
    // The well is the whole window, which is the geometry a surface with no
    // cabinet at all has.
    assert_eq!(bank.layout().crt.width, f64::from(WINDOW_W));
    // No bank, so the window's floor is the well's floor and nothing else.
    assert_eq!(bank.min_inner_size(), WELL_MINIMUM);

    // The bank's own measures still exist and nothing reads them: a drag where
    // the boundary would have been moves nothing.
    assert_eq!(bank.geometry().implicit_width, STOCK_BANK as i32);
    drag(&mut surface, f64::from(STOCK_BANK), 400.0);
    assert_eq!(surface.cabinet().unwrap().bank_width(), 0);
    assert_eq!(
        surface.cabinet().unwrap().layout().crt.width,
        f64::from(WINDOW_W)
    );
}

#[test]
fn a_fold_gives_the_well_the_whole_window_and_an_unfold_the_bank_back() {
    let mut surface = surface();
    surface.set_cabinet(cabinet(&Config::default()));
    assert_eq!(surface.cabinet().unwrap().bank_width(), STOCK_BANK);

    assert!(surface.toggle_bank_fold());
    let bank = surface.cabinet().unwrap();
    assert!(bank.is_folded());
    assert!(!bank.is_shown());
    assert_eq!(bank.bank_width(), 0);
    assert_eq!(bank.layout().crt.width, f64::from(WINDOW_W));
    assert_eq!(bank.min_inner_size(), WELL_MINIMUM);
    // A drag where the seam stood moves nothing while the bank is folded.
    drag(&mut surface, f64::from(STOCK_BANK), 400.0);
    assert_eq!(surface.cabinet().unwrap().bank_width(), 0);

    assert!(surface.toggle_bank_fold());
    let bank = surface.cabinet().unwrap();
    assert!(!bank.is_folded());
    assert_eq!(bank.bank_width(), STOCK_BANK);
}

#[test]
fn a_fold_with_no_cabinet_declines() {
    let mut surface = surface();
    assert!(!surface.toggle_bank_fold());
}

#[test]
fn a_drag_with_no_settings_moves_the_bank_and_writes_nothing() {
    // `--default-settings` is the contract's "never touch the user's real
    // config" switch, and a surface run under it has no handle at all. The
    // appliance still resizes; the drag simply does not outlive the process.
    let mut surface = surface();
    surface.set_cabinet(cabinet(&Config::default()));
    drag(&mut surface, f64::from(STOCK_BANK), 400.0);
    assert_eq!(surface.cabinet().unwrap().bank_width(), 403);
}
