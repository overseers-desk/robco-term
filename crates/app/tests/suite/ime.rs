//! Input-method events, end to end into the child process.
//!
//! ## What this proves, and what it cannot
//!
//! It drives `Ime` values into the surface the binary runs and reads what the
//! child received. Everything on the rebuild's side of the input method is
//! therefore under test: the commit string becoming UTF-8 bytes, those bytes
//! reaching the PTY, the child's own echo coming back onto the screen, and the
//! pre-edit going nowhere near it.
//!
//! What it does **not** prove is that a real fcitx5 or ibus produces those
//! values on this machine. That round trip needs an X or Wayland server with an
//! input-method framework running, an engine selected, and keystrokes delivered
//! into a focused window -- none of which exists under Xvfb, and none of which
//! can be faked from inside the process, because the part being tested would be
//! the part doing the faking. The events here are synthesised, and the honest
//! reading of a green run is: *if* the platform delivers `Ime::Commit`, the
//! text lands in the child.
//!
//! Whether the platform delivers it is a hands-on check, and this is the whole
//! of it, so nobody has to reconstruct it later:
//!
//! 1. A desktop session with fcitx5 or ibus running and a CJK engine added,
//!    with `GTK_IM_MODULE` / `QT_IM_MODULE` / `XMODIFIERS` pointing at it.
//! 2. Run `robco-term`, focus the window, switch the input method on.
//! 3. Type `nihongo`, press space, press Enter. **Expected:** the romaji appear
//!    at the terminal cursor as a lit block with the letters struck into it (the
//!    pre-edit, drawn by `term::render::GridRenderer::set_preedit`), the
//!    candidate window sits at that same cursor (`set_ime_cursor_area`, from
//!    `TerminalSurface::ime_cursor_area`), and on Enter the composition vanishes
//!    and the chosen word appears in the shell, all at once.
//! 4. The failures that would mean this wiring is wrong: nothing appears on
//!    Enter (the events are not arriving -- look at `set_ime_allowed`); the
//!    romaji appear one character at a time *in the shell* rather than as a
//!    composition (the keystrokes are reaching `key_pressed` behind the input
//!    method's back); the composition draws but the candidate window still lands
//!    in a corner of the screen (the platform ignored the cursor area, or the
//!    rectangle is being computed from the wrong window -- it is in this
//!    window's physical pixels, bank column included); or the composition is
//!    left behind on the glass after a commit.
//!
//! Everything up to the framework is measured: the pre-edit's pixels in
//! `crates/core/tests/preedit.rs`, the caret rectangle below.
//!
//! Not run here, and not claimed here.
//!
//! Same harness as `keyboard_scroll.rs`: `TerminalSurface::headless` is the
//! surface with no swapchain, and `ime_input` is `Surface::ime` with winit's
//! own event dispatch peeled off.

use std::time::{Duration, Instant};

use app::window::TerminalSurface;
use term::{CellSize, SessionConfig, Viewport};
use winit::event::Ime;

const CELL_W: f64 = 9.0;
const CELL_H: f64 = 18.0;
const COLS: u32 = 80;
const ROWS: u32 = 10;

/// Japanese, Chinese and Korean, the population IME composition has to
/// support, plus an emoji for a code point outside the BMP. All of it
/// multi-byte: a path that truncated to one byte per character would pass
/// on ASCII and fail here.
const COMPOSED: &str = "日本語 中文 한국어 🌒";

/// The child every test here runs: `cat`, echoing whatever it is given, with
/// the line discipline out of the way.
///
/// Both `stty` flags are load bearing and neither is decoration.
///
/// * `-echo` stops the *tty driver* from putting the input back on the screen.
///   With it on, text would appear whether or not the child ever read a byte,
///   and the test would pass with the PTY write going nowhere.
/// * `-icanon` stops the driver from holding the input until a newline. An IME
///   commit is a word, not a line -- nobody presses Enter to finish composing
///   -- so in canonical mode `cat` would sit waiting and the text would be
///   parked in a kernel buffer. That is not a bug in the wiring, it is what a
///   real terminal does too, but it is not what this test is measuring.
const ECHOING_CHILD: &str = "stty -echo -icanon; echo READY; cat";

fn scripted(script: &str) -> SessionConfig {
    SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec!["-c".to_string(), format!("{script}; sleep 10")],
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

fn surface(script: &str) -> TerminalSurface {
    let viewport = Viewport::new(
        COLS * CELL_W as u32,
        ROWS * CELL_H as u32,
        1.0,
        CellSize::new(CELL_W as f32, CELL_H as f32),
    );
    TerminalSurface::headless(&scripted(script), viewport)
}

/// The screen as one string, with every space taken out.
///
/// A CJK character occupies two cells, and `viewport_text` renders the second
/// one -- the spacer the emulator writes after a wide glyph -- as a space. So
/// `日本語` comes back off the grid as `日 本 語 `, and a plain `contains` on
/// the composed string finds nothing while the text is sitting right there.
///
/// Squeezing the spaces out of both sides is the smallest thing that reads the
/// grid correctly, and it costs nothing this file needs: the claim being made
/// is that every committed code point reached the child in order, which
/// survives the squeeze exactly. Nothing here is asserting about spacing.
fn screen(surface: &TerminalSurface) -> String {
    surface.viewport_text().join("\n").replace(' ', "")
}

fn squeezed(text: &str) -> String {
    text.replace(' ', "")
}

/// Pump until the screen says `text`, or fail with what it did say.
fn wait_for_screen(surface: &mut TerminalSurface, text: &str) {
    let needle = squeezed(text);
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        surface.pump();
        if screen(surface).contains(&needle) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {text:?}\n--- screen ---\n{}",
        surface.viewport_text().join("\n")
    );
}

/// Pump for a while and answer whether the screen ever said `text`. The
/// negative of `wait_for_screen`, for the things that must *not* arrive.
fn never_on_screen(surface: &mut TerminalSurface, text: &str, within: Duration) -> bool {
    let needle = squeezed(text);
    let deadline = Instant::now() + within;
    while Instant::now() < deadline {
        surface.pump();
        if screen(surface).contains(&needle) {
            return false;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    true
}

/// The done-test: a scripted `Ime::Commit` reaches the PTY.
///
/// `cat` is the child, so what the terminal shows is what the child read: the
/// text is not on the screen because the surface drew it, it is on the screen
/// because it went down the PTY, through a process, and came back up.
#[test]
fn a_committed_string_reaches_the_child_as_utf8() {
    let mut surface = surface(ECHOING_CHILD);
    wait_for_screen(&mut surface, "READY");

    // The whole exchange an input method has: it takes the keyboard, shows a
    // composition, refines it, then commits.
    surface.ime_input(&Ime::Enabled);
    surface.ime_input(&Ime::Preedit("にほん".to_string(), Some((9, 9))));
    surface.ime_input(&Ime::Preedit("日本".to_string(), Some((6, 6))));
    surface.ime_input(&Ime::Commit(COMPOSED.to_string()));
    surface.ime_input(&Ime::Preedit(String::new(), None));
    surface.ime_input(&Ime::Disabled);

    // The child has it, and has it whole.
    wait_for_screen(&mut surface, COMPOSED);

    // And the pre-edit that was never committed is not in the child's input.
    // It was on screen only if the surface wrote it, which is the bug this
    // guards: an implementation that writes every `Preedit` sends the user's
    // half-typed kana to the shell.
    assert!(
        !screen(&surface).contains(&squeezed("にほん")),
        "an uncommitted pre-edit reached the child\n--- screen ---\n{}",
        surface.viewport_text().join("\n")
    );
}

/// The caret rectangle the input method is told about (`set_ime_cursor_area`)
/// is the cursor's own cell, and it moves as the cursor does.
///
/// A headless surface has no settings handle, so every distortion term is the
/// neutral one and the map from a grid cell to a window pixel is the identity --
/// which is the point: the rectangle is derived through the same
/// `term::distortion` the pointer path inverts, so at zero curvature it lands
/// exactly on the cell, and at a real curvature it lands where that cell is
/// *drawn* rather than where a flat grid would have put it.
#[test]
fn the_caret_rectangle_is_the_cursors_cell_and_follows_it() {
    let mut surface = surface(ECHOING_CHILD);
    wait_for_screen(&mut surface, "READY");

    let (position, size) = surface.ime_cursor_area().expect("a caret on the glass");
    assert!(
        (size.width - CELL_W).abs() < 1e-6 && (size.height - CELL_H).abs() < 1e-6,
        "the caret rectangle is {size:?}, not one cell"
    );
    // The prompt is on the row under READY, at column zero.
    assert!(
        position.x.abs() < 1e-6 && (position.y - CELL_H).abs() < 1e-6,
        "the caret is at {position:?}, not at the start of the second row"
    );

    // Three characters through the child and back: the cursor has moved three
    // cells, and so has the rectangle.
    surface.ime_input(&Ime::Commit("abc".to_string()));
    wait_for_screen(&mut surface, "abc");
    let (moved, _) = surface.ime_cursor_area().expect("a caret on the glass");
    assert!(
        (moved.x - 3.0 * CELL_W).abs() < 1e-6,
        "the caret is at x={} after three characters, not at {}",
        moved.x,
        3.0 * CELL_W
    );
}

/// The pre-edit is held here and drawn by the renderer; what this holds is
/// the state that mount reads, including winit's inner offsets, which the
/// caret-rectangle math must not ignore.
#[test]
fn the_preedit_is_held_and_not_sent() {
    let mut surface = surface(ECHOING_CHILD);
    wait_for_screen(&mut surface, "READY");

    assert!(!surface.ime_state().enabled);

    surface.ime_input(&Ime::Enabled);
    assert!(surface.ime_state().enabled);

    surface.ime_input(&Ime::Preedit("かんじ".to_string(), Some((3, 9))));
    assert_eq!(surface.ime_state().preedit, "かんじ");
    assert_eq!(surface.ime_state().cursor, Some((3, 9)));

    // Nothing was sent, so the child has nothing to echo. Half a second is
    // several hundred pump cycles on a PTY that answers in microseconds.
    assert!(
        never_on_screen(&mut surface, "かんじ", Duration::from_millis(500)),
        "the pre-edit was written to the child"
    );

    // A commit clears the composition, whichever order the platform sends the
    // trailing empty pre-edit in.
    surface.ime_input(&Ime::Commit("漢字".to_string()));
    assert_eq!(surface.ime_state().preedit, "");
    assert_eq!(surface.ime_state().cursor, None);
    wait_for_screen(&mut surface, "漢字");

    // `Disabled` puts it back to where it started: an input method that goes
    // away mid-composition must not leave a word behind for the next one.
    surface.ime_input(&Ime::Preedit("すて".to_string(), None));
    surface.ime_input(&Ime::Disabled);
    assert_eq!(surface.ime_state(), &app::window::ImeState::default());
}

/// An empty commit is what an input method sends when a composition is
/// abandoned. It must not reach the child at all -- not even as zero bytes,
/// which would still be a write syscall and, on a program watching for input,
/// a wakeup that means nothing.
#[test]
fn an_abandoned_composition_sends_nothing() {
    let mut surface = surface(ECHOING_CHILD);
    wait_for_screen(&mut surface, "READY");

    surface.ime_input(&Ime::Enabled);
    surface.ime_input(&Ime::Preedit("あ".to_string(), Some((3, 3))));
    surface.ime_input(&Ime::Commit(String::new()));
    surface.ime_input(&Ime::Disabled);

    assert_eq!(surface.ime_state().preedit, "");
    // Then a real one, to show the surface is still live rather than wedged.
    surface.ime_input(&Ime::Commit("ok-after-abandon".to_string()));
    wait_for_screen(&mut surface, "ok-after-abandon");
}
