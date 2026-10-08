//! The pointer at every shape a fullscreen window takes (#21, reported as
//! shift-drag selecting nothing in fullscreen and selecting in a window).
//!
//! Nothing in the pointer path branches on fullscreen, and after this suite
//! nothing needs to: a fullscreen window is an ordinary window the compositor
//! resized, so what reaches the surface is a `Resized` carrying the whole
//! display and, on a monitor of another density, a scale factor. Both arrive
//! here through the same two methods the shell calls, and the gestures that
//! follow are the real `mouse_pressed`/`cursor_moved`/`mouse_released`.
//!
//! Three things the reported symptom could have been, each driven rather than
//! reasoned about:
//!
//! * the **mapping**: a window pixel becoming a grid cell through the bank
//!   subtraction, the margin, the frame inset and the inverse curvature, all
//!   of which are functions of the window's own width and height and so all of
//!   which move when it fills the screen;
//! * the **seam and the bank strips**, which get first refusal on every press
//!   and could claim the glass if a fullscreen geometry degenerated;
//! * the **modifier**, since Shift only means anything while the program below
//!   owns the mouse -- which is the state the reported gesture was in.
//!
//! The screens below span 720p windowed to 5120x1440, at three densities. A
//! selection that comes back short is as much a failure as one that comes back
//! empty, so every assertion is measured against the screen's own column count
//! rather than against a bare "something was selected".

use std::fs;
use std::sync::Arc;
use std::time::{Duration, Instant};

use app::settings::SettingsHandle;
use app::shell::Surface;
use app::window::TerminalSurface;
use chassis::Cabinet;
use term::{CellSize, SessionConfig, Viewport};
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::MouseButton;
use winit::keyboard::ModifiersState;

/// The cell in logical pixels, as a font hands it over.
const CELL_W: f64 = 9.0;
const CELL_H: f64 = 18.0;

/// The character run a filled screen is made of, and what a drag across it
/// comes back holding.
const RULE: &str = "abcdefghij";

/// The shapes a fullscreen window arrives at, against the windowed shape as
/// the control. Each name is what a failure prints.
const SCREENS: &[(&str, u32, u32, f64)] = &[
    ("a window", 720, 432, 1.0),
    ("1080p fullscreen", 1920, 1080, 1.0),
    ("1440p fullscreen", 2560, 1440, 1.0),
    ("4K fullscreen at 2x", 3840, 2160, 2.0),
    ("4K fullscreen at 1x", 3840, 2160, 1.0),
    ("an ultrawide fullscreen", 5120, 1440, 1.0),
    ("a laptop panel at 1.5x", 2256, 1504, 1.5),
];

/// A screen filled edge to edge and corner to corner: one unbroken stream of
/// the rule, wrapped by the terminal itself.
///
/// Not lines, deliberately. A line of any fixed length leaves a short row
/// wherever it stops wrapping, and a drag that landed on one of those would
/// come back holding almost nothing for a reason that has nothing to do with
/// the pointer -- a false alarm this suite raised for itself once already.
/// Wrapped continuously, every row but the last is full at every width in the
/// table above.
fn scripted(width: u32, height: u32, scale: f64) -> SessionConfig {
    stream("", width, height, scale)
}

/// The same stream behind a program that has turned mouse reporting on, which
/// is the state Shift exists to override.
fn scripted_reporting_the_mouse(width: u32, height: u32, scale: f64) -> SessionConfig {
    stream("printf '\\033[?1000h\\033[?1006h'; ", width, height, scale)
}

/// Enough of the rule to fill this screen half again over, and no more. The
/// length is worth computing rather than picking generously: five tests times
/// seven screens is thirty-five children, every byte of which some thread of
/// this process has to parse, and a fixed length long enough for the largest
/// screen here starves the rest of the suite of the CPU it is timed against.
fn stream(prefix: &str, width: u32, height: u32, scale: f64) -> SessionConfig {
    let cells = (f64::from(width) / (CELL_W * scale)) * (f64::from(height) / (CELL_H * scale));
    let line = RULE.len() * 60;
    let repeats = (cells * 1.5) as usize / line + 2;
    SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            format!(
                "{prefix}L='{}'; i=0; while [ $i -lt {repeats} ]; do printf '%s' \"$L\"; \
                 i=$((i+1)); done; sleep 10",
                RULE.repeat(60)
            ),
        ],
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

/// A bare surface the size a window of this shape and density is, told its
/// size again through `resized` -- which is the event a fullscreen transition
/// actually delivers, and the only one it delivers.
fn surface(width: u32, height: u32, scale: f64) -> TerminalSurface {
    let viewport = Viewport::new(
        width,
        height,
        scale,
        CellSize::new(CELL_W as f32, CELL_H as f32),
    );
    let mut surface = TerminalSurface::headless(&scripted(width, height, scale), viewport);
    surface.resized(PhysicalSize::new(width, height));
    surface
}

/// The same surface with the furniture on: a bank column beside the glass and
/// a live settings handle behind the distortion, which is the shape the
/// shipped binary is actually pressed on. The margin, the frame inset and the
/// curvature are then the profile's own, so a press runs through the real
/// inverse warp rather than the identity a surface with no settings handle
/// reduces it to.
///
/// The `TempDir` comes back with the surface because dropping it takes the
/// config file with it, and the watcher behind the handle is watching that
/// file.
fn dressed(
    width: u32,
    height: u32,
    scale: f64,
    session: SessionConfig,
) -> (TerminalSurface, tempfile::TempDir) {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("config.toml");
    // An empty table: every key the pointer reads is the shipped default.
    fs::write(&path, "[general]\n").unwrap();
    let handle = Arc::new(SettingsHandle::spawn(path, |_, _, _| {}).expect("watcher should start"));
    let cfg = handle.current();

    let viewport = Viewport::new(
        width,
        height,
        scale,
        CellSize::new(CELL_W as f32, CELL_H as f32),
    );
    let mut surface = TerminalSurface::headless(&session, viewport);
    surface.set_settings(Arc::clone(&handle));
    surface.set_cabinet(Cabinet::from_config(
        &cfg,
        f64::from(width) / scale,
        f64::from(height) / scale,
    ));
    surface.resized(PhysicalSize::new(width, height));
    (surface, dir)
}

/// Pump until every row but the last is the same full width, and answer what
/// that width is. `viewport_text` trims each row's trailing blanks, so equal
/// lengths across the screen is exactly the statement that no row has any --
/// and the width it settles at is the screen's column count, which is what
/// every assertion below is measured against.
fn wait_for_a_full_screen(surface: &mut TerminalSurface) -> usize {
    let deadline = Instant::now() + Duration::from_secs(20);
    while Instant::now() < deadline {
        surface.pump();
        let screen = surface.viewport_text();
        let full = screen.first().map_or(0, String::len);
        if full > 40 && screen[..screen.len() - 1].iter().all(|l| l.len() == full) {
            return full;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    let screen = surface.viewport_text();
    panic!(
        "timed out waiting for a full screen; the rows measured {:?}",
        screen.iter().map(String::len).collect::<Vec<_>>()
    );
}

fn shift() -> ModifiersState {
    ModifiersState::SHIFT
}

/// Press, move once, release, and answer what the release copied. One move is
/// all a drag needs to mark, and it is what a test can state exactly.
fn drag(
    surface: &mut TerminalSurface,
    from: PhysicalPosition<f64>,
    to: PhysicalPosition<f64>,
    modifiers: ModifiersState,
) -> String {
    surface.mouse_pressed(MouseButton::Left, from, modifiers);
    surface.cursor_moved(to, modifiers);
    surface.mouse_released(MouseButton::Left, to, modifiers);
    surface.last_selection().unwrap_or_default().to_string()
}

/// The bank column's width in window pixels: where the well starts.
fn bank(surface: &TerminalSurface, scale: f64) -> f64 {
    surface.cabinet().map_or(0.0, |c| f64::from(c.bank_width())) * scale
}

/// A quarter of the way across the well to three quarters of the way across
/// it, along the middle row. Measured over the well and not the window,
/// because a window narrow enough stands a quarter of its width inside the
/// bank, and a press there picks the window up instead of marking.
fn across(
    surface: &TerminalSurface,
    width: u32,
    height: u32,
    scale: f64,
) -> (PhysicalPosition<f64>, PhysicalPosition<f64>) {
    let bank = bank(surface, scale);
    let well = f64::from(width) - bank;
    let y = f64::from(height) * 0.5;
    (
        PhysicalPosition::new(bank + well * 0.25, y),
        PhysicalPosition::new(bank + well * 0.75, y),
    )
}

#[test]
fn a_drag_across_the_glass_marks_it_at_every_shape_a_fullscreen_window_takes() {
    for &(name, width, height, scale) in SCREENS {
        let mut surface = surface(width, height, scale);
        let cols = wait_for_a_full_screen(&mut surface);
        let (from, to) = across(&surface, width, height, scale);

        let marked = drag(&mut surface, from, to, shift());
        assert!(
            marked.len() >= cols / 3,
            "{name} ({width}x{height} at {scale}x): a drag across half a {cols}-column \
             screen marked {} characters",
            marked.len()
        );
    }
}

#[test]
fn the_bank_and_the_curvature_do_not_take_the_drag_at_any_shape() {
    for &(name, width, height, scale) in SCREENS {
        let (mut surface, _dir) = dressed(width, height, scale, scripted(width, height, scale));
        let cols = wait_for_a_full_screen(&mut surface);
        let (from, to) = across(&surface, width, height, scale);

        let marked = drag(&mut surface, from, to, shift());
        assert!(
            marked.len() >= cols / 3,
            "{name} ({width}x{height} at {scale}x), dressed: a drag across half a \
             {cols}-column screen marked {} characters",
            marked.len()
        );
    }
}

/// The press that starts where the text does. The bezel between the bank and
/// the grid is the window's handle, so a drag beginning a fifth of the way
/// into the well is about the closest a selection can start -- and the bank
/// stands at a different width fullscreen than windowed, because a window
/// narrow enough fits the bank down and a full screen does not.
#[test]
fn a_drag_that_starts_beside_the_bank_marks_at_every_shape() {
    for &(name, width, height, scale) in SCREENS {
        let (mut surface, _dir) = dressed(width, height, scale, scripted(width, height, scale));
        let cols = wait_for_a_full_screen(&mut surface);
        let bank = bank(&surface, scale);

        let y = f64::from(height) * 0.5;
        let from = PhysicalPosition::new(bank + (f64::from(width) - bank) * 0.2, y);
        let to = PhysicalPosition::new(f64::from(width) * 0.6, y);
        let marked = drag(&mut surface, from, to, shift());
        assert!(
            marked.len() >= cols / 8,
            "{name} ({width}x{height} at {scale}x): a drag from beside the bank to \
             the middle of a {cols}-column screen marked {} characters",
            marked.len()
        );
    }
}

/// Down the glass rather than across it, from under the top bezel to above
/// the bottom one. The vertical mapping has its own margin, its own frame
/// inset and its own half of the radial term, and the row a press lands on
/// is what decides whether a drag has moved at all.
///
/// Measured in characters rather than lines: the screen is one wrapped stream,
/// so it has no line breaks to count, and characters are the finer ruler
/// anyway.
#[test]
fn a_drag_down_the_glass_marks_the_rows_it_crossed() {
    for &(name, width, height, scale) in SCREENS {
        let (mut surface, _dir) = dressed(width, height, scale, scripted(width, height, scale));
        let cols = wait_for_a_full_screen(&mut surface);

        let x = f64::from(width) * 0.5;
        let from = PhysicalPosition::new(x, f64::from(height) * 0.15);
        let to = PhysicalPosition::new(x, f64::from(height) * 0.85);
        let marked = drag(&mut surface, from, to, shift());
        assert!(
            marked.len() >= cols * 4,
            "{name} ({width}x{height} at {scale}x): a drag down the glass marked {} \
             characters, under four {cols}-column rows' worth",
            marked.len()
        );
    }
}

/// The reported gesture exactly: a program below has taken the mouse, and
/// Shift is how the user takes it back. Both halves are asserted, because the
/// half that says the unmodified drag marked nothing is only worth something
/// beside the half that says the modified one marked the screen.
#[test]
fn shift_takes_the_pointer_back_from_the_program_at_every_shape() {
    for &(name, width, height, scale) in SCREENS {
        let (mut surface, _dir) = dressed(
            width,
            height,
            scale,
            scripted_reporting_the_mouse(width, height, scale),
        );
        let cols = wait_for_a_full_screen(&mut surface);
        assert!(
            surface.terminal_uses_mouse(),
            "{name}: test setup: the program should have taken the mouse"
        );
        let (from, to) = across(&surface, width, height, scale);

        let unmodified = drag(&mut surface, from, to, ModifiersState::empty());
        assert!(
            unmodified.is_empty(),
            "{name} ({width}x{height} at {scale}x): a drag marked {unmodified:?} while \
             the program below owned the pointer"
        );

        let marked = drag(&mut surface, from, to, shift());
        assert!(
            marked.len() >= cols / 3,
            "{name} ({width}x{height} at {scale}x): a shift-drag across half a \
             {cols}-column screen marked {} characters with the program below \
             holding the mouse",
            marked.len()
        );
    }
}
