//! A shed write queue reaches the glass, not only the log.
//!
//! Both write queues are capped so a peer that stopped reading
//! cannot grow this process without bound. A cap that bites throws the user's
//! typing away, and before this test the whole of the evidence was a
//! `log::warn!`: keystrokes vanished and the screen said nothing about it. This
//! drives the real path -- a child that never reads its tty, a megabytes-long
//! paste at it, the surface's own pump -- and reads what the user would have
//! seen on the glass.
//!
//! The badge itself needs a device and a frame; `crates/app/tests/size_badge.rs`
//! is where its pixels are measured. What is measured here is the state under
//! it: `TerminalSurface::notice`, which is what the frame draws.

use std::time::{Duration, Instant};

use app::overlay::{NOTICE_FADE, NOTICE_HOLD};
use app::window::{TerminalSurface, SHED_PTY};
use term::{CellSize, SessionConfig, Viewport, INPUT_CAP};

const CELL_W: f64 = 9.0;
const CELL_H: f64 = 18.0;
const COLS: u32 = 80;
const ROWS: u32 = 10;

/// A child that never reads a byte of its tty: it prints once and sleeps. The
/// kernel's own tty buffer takes a few kilobytes and then refuses everything,
/// which is exactly the state the queue exists for and, held long enough, the
/// state the cap sheds in.
const DEAF_CHILD: &str = "echo READY; sleep 30";

fn surface() -> TerminalSurface {
    let viewport = Viewport::new(
        COLS * CELL_W as u32,
        ROWS * CELL_H as u32,
        1.0,
        CellSize::new(CELL_W as f32, CELL_H as f32),
    );
    TerminalSurface::headless(
        &SessionConfig {
            program: Some("/bin/sh".to_string()),
            args: vec!["-c".to_string(), DEAF_CHILD.to_string()],
            working_directory: None,
            env: vec![
                ("TERM".to_string(), "xterm-256color".to_string()),
                ("ENV".to_string(), String::new()),
            ],
            scrollback: 1000,
            grapheme_clustering: false,
            rate: None,
        },
        viewport,
    )
}

fn wait_for_screen(surface: &mut TerminalSurface, text: &str) {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        surface.pump();
        if surface.viewport_text().join("\n").contains(text) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for {text:?}\n--- screen ---\n{}",
        surface.viewport_text().join("\n")
    );
}

/// The done-test: type past the cap at a child that is not listening, and the
/// appliance says so on the glass.
#[test]
fn typing_thrown_away_by_a_full_queue_says_so_on_the_glass() {
    let mut surface = surface();
    wait_for_screen(&mut surface, "READY");

    // Nothing has been lost yet, so there is nothing to say.
    surface.pump();
    assert!(
        !surface.notice().visible_at(Instant::now()),
        "a badge went up before anything was dropped: {:?}",
        surface.notice().text()
    );

    // A paste far larger than the queue can hold, at a child that reads none of
    // it. The first megabytes queue; the writes past the cap are thrown away.
    //
    // Twice the cap, because a write reaches the pty before it reaches the
    // queue and only the tail the master would not take is queued. The
    // master's own buffer therefore comes off the total, and a margin
    // narrower than that buffer decides the test on how much the kernel felt
    // like absorbing. No pty buffer is megabytes deep, so at twice the cap
    // the queue is over it whatever the master took.
    let chunk = vec![b'x'; 64 << 10];
    let mut offered = 0usize;
    while offered <= 2 * INPUT_CAP {
        surface.write(&chunk);
        offered += chunk.len();
    }

    // The counters are read on the pump, which is where the badge goes up.
    surface.pump();
    let now = Instant::now();
    assert_eq!(
        surface.notice().text(),
        SHED_PTY,
        "the shed happened and the glass said {:?}",
        surface.notice().text()
    );
    assert!(
        surface.notice().visible_at(now),
        "the badge is up but drawn at zero opacity"
    );

    // And it is transient, like every other badge in this appliance: after the
    // hold and the fade it is gone, so a shell that started reading again does
    // not leave the user staring at old news.
    assert!(!surface.notice().visible_at(now + NOTICE_HOLD + NOTICE_FADE));
}

/// A key that asks for a shell and gets none is answered on the glass, not
/// only in the log: the card reads [`app::window::NO_SHELL`] for its hold.
#[test]
fn a_shell_that_cannot_start_says_so_on_the_glass() {
    let viewport = Viewport::new(
        COLS * CELL_W as u32,
        ROWS * CELL_H as u32,
        1.0,
        CellSize::new(CELL_W as f32, CELL_H as f32),
    );
    let mut surface = TerminalSurface::headless(
        &SessionConfig {
            program: Some("/nonexistent/shell".to_string()),
            ..SessionConfig::default()
        },
        viewport,
    );
    assert!(surface.channels().current().is_none(), "no shell came up");
    let now = Instant::now();
    assert!(!surface.notice().visible_at(now), "the first failure is startup's, not a key's");
    surface.new_channel();
    assert_eq!(surface.notice().text(), app::window::NO_SHELL);
    assert!(surface.notice().visible_at(now));
}
