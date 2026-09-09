//! The `[bindings]` table end to end through the surface the binary runs: a
//! row lays over a shipped chord, the two unbinds do what their names say,
//! `esc` puts its bytes on the wire, and a row that does not parse leaves
//! the shipped chord standing.
//!
//! Same harness as `meta_keys.rs`: a shell that reads a line and prints it
//! back through `cat -v` inside brackets, so what reached the pty is on the
//! glass, an escape as `^[`.

use std::time::{Duration, Instant};

use app::shell::Surface;
use app::window::TerminalSurface;
use chassis::{Cabinet, Display, LedMetrics};
use config::{Config, KeyBinding};
use term::{CellSize, SessionConfig, Viewport};
use winit::keyboard::{Key, ModifiersState, NamedKey};

const CTRL_SHIFT: ModifiersState = ModifiersState::CONTROL.union(ModifiersState::SHIFT);

fn scripted() -> SessionConfig {
    SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "echo ready; while IFS= read -r l; do printf '[%s]\\n' \"$l\" | cat -v; done"
                .to_string(),
            String::new(),
        ],
        working_directory: None,
        env: vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("ENV".to_string(), String::new()),
        ],
        scrollback: 200,
        grapheme_clustering: false,
        rate: None,
    }
}

fn surface() -> TerminalSurface {
    let viewport = Viewport::new(1440, 768, 1.0, CellSize::new(9.0, 18.0));
    let mut surface = TerminalSurface::headless(&scripted(), viewport);
    wait_for(&mut surface, "ready");
    surface
}

fn wait_for(surface: &mut TerminalSurface, text: &str) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        surface.pump();
        if let Some(line) = surface.viewport_text().iter().find(|l| l.contains(text)) {
            return line.trim().to_string();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {text:?}\n--- screen ---\n{}",
        surface.viewport_text().join("\n")
    );
}

fn character(surface: &mut TerminalSurface, c: &str, modifiers: ModifiersState) {
    surface.key_input(&Key::Character(c.into()), Some(c), modifiers);
}

fn enter(surface: &mut TerminalSurface) {
    surface.key_input(&Key::Named(NamedKey::Enter), None, ModifiersState::empty());
}

fn row(key: &str, with: &str, action: &str, esc: &str) -> KeyBinding {
    KeyBinding {
        key: key.to_string(),
        with: with.to_string(),
        action: action.to_string(),
        esc: esc.to_string(),
    }
}

fn configured(rows: Vec<KeyBinding>) -> TerminalSurface {
    let mut surface = surface();
    let mut cfg = Config::default();
    cfg.bindings.keys = rows;
    surface.set_config(cfg);
    surface
}

/// The line the child echoes back once `a`, the key under test, and `b`
/// have been typed: what the chord put on the wire sits between them.
fn between(surface: &mut TerminalSurface) -> String {
    character(surface, "a", ModifiersState::empty());
    character(surface, "b", ModifiersState::empty());
    enter(surface);
    wait_for(surface, "]")
}

#[test]
fn a_shipped_chord_leaves_nothing_on_the_wire_with_no_table_at_all() {
    let mut surface = surface();
    character(&mut surface, "a", ModifiersState::empty());
    character(&mut surface, "C", CTRL_SHIFT);
    assert_eq!(between(&mut surface), "[aab]");
}

#[test]
fn pass_lets_the_key_through_to_the_program() {
    let mut surface = configured(vec![row("c", "ctrl | shift", "pass", "")]);
    character(&mut surface, "a", ModifiersState::empty());
    character(&mut surface, "C", CTRL_SHIFT);
    assert_eq!(between(&mut surface), "[aCab]");
}

#[test]
fn swallow_takes_a_key_the_program_would_otherwise_get() {
    let mut surface = configured(vec![row("x", "", "swallow", "")]);
    character(&mut surface, "a", ModifiersState::empty());
    character(&mut surface, "x", ModifiersState::empty());
    assert_eq!(between(&mut surface), "[aab]");
}

#[test]
fn esc_writes_the_escape_and_its_string() {
    let mut surface = configured(vec![row("f5", "", "", "[15~")]);
    character(&mut surface, "a", ModifiersState::empty());
    surface.key_input(&Key::Named(NamedKey::F5), None, ModifiersState::empty());
    assert_eq!(between(&mut surface), "[a^[[15~ab]");
}

#[test]
fn a_row_that_does_not_parse_leaves_the_shipped_chord_standing() {
    let mut surface = configured(vec![row("c", "ctrl | shift", "no_such_action", "")]);
    character(&mut surface, "a", ModifiersState::empty());
    character(&mut surface, "C", CTRL_SHIFT);
    assert_eq!(between(&mut surface), "[aab]");
}

#[test]
fn a_new_trigger_joins_the_shipped_set() {
    let mut surface = configured(vec![row("y", "ctrl | shift", "copy", "")]);
    character(&mut surface, "a", ModifiersState::empty());
    character(&mut surface, "Y", CTRL_SHIFT);
    character(&mut surface, "C", CTRL_SHIFT);
    assert_eq!(between(&mut surface), "[aab]", "both chords are the terminal's now");
}

#[test]
fn the_fold_chord_folds_the_bank() {
    let mut surface = surface();
    surface.set_cabinet(Cabinet::new(
        &Config::default(),
        Display::Led(LedMetrics::default()),
        1440.0,
        768.0,
    ));
    assert!(!surface.cabinet().unwrap().is_folded());
    character(&mut surface, "B", CTRL_SHIFT);
    assert!(surface.cabinet().unwrap().is_folded());
    character(&mut surface, "b", CTRL_SHIFT);
    assert!(!surface.cabinet().unwrap().is_folded());
    // The chord put nothing on the wire either way.
    character(&mut surface, "a", ModifiersState::empty());
    assert_eq!(between(&mut surface), "[aab]");
}

#[test]
fn a_window_key_with_no_shell_declines_to_the_program() {
    // Headless, there is no shell to ask, and a declined action is an
    // unbound key: the press reaches the program instead of vanishing.
    let mut surface = surface();
    character(&mut surface, "a", ModifiersState::empty());
    character(&mut surface, "N", CTRL_SHIFT);
    assert_eq!(between(&mut surface), "[aNab]");
}
