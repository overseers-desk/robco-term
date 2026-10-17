//! A scripted PTY transcript.
//!
//! Write bytes to a real shell, pump the read loop, read the grid back,
//! compare. The script runs straight through a resize and a DPR change
//! without restarting the session, because the point of the test is that
//! the session survives both: a half-applied resize (grid moved, child
//! not told) and a DPR change that never reaches the grid are the two
//! failures this test exists to prevent.
//!
//! No window and no threads. `Session::pump` is synchronous, so the test
//! drives the loop itself and the whole thing is an ordinary `cargo
//! test` on any machine, headless included.

use std::time::{Duration, Instant};

use term::{
    live_text, CellSize, ControlModeTap, DcsTap, NoopTap, Session, SessionConfig, Viewport,
};

/// `sh` with no rc file and a fixed prompt: the screen is then a
/// function of what we sent and nothing else.
fn config() -> SessionConfig {
    SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![],
        working_directory: None,
        env: vec![
            ("TERM".to_string(), "xterm-256color".to_string()),
            ("PS1".to_string(), "$ ".to_string()),
            // `sh` sources $ENV even non-interactively; empty keeps a
            // developer's own rc file out of the transcript.
            ("ENV".to_string(), String::new()),
        ],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: None,
    }
}

/// Pump until `pred` holds, or fail with the screen that was there.
///
/// The child is a real process, so "has it happened yet" is genuinely a
/// question; polling with a deadline is the honest way to ask it.
fn wait_for<F>(session: &mut Session<NoopTap>, what: &str, pred: F)
where
    F: Fn(&Session<NoopTap>) -> bool,
{
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        session.pump();
        if pred(session) {
            return;
        }
        std::thread::sleep(Duration::from_millis(5));
    }

    let screen = live_text(session.term())
        .into_iter()
        .enumerate()
        .filter(|(_, l)| !l.is_empty())
        .map(|(i, l)| format!("  {i:>3} | {l}"))
        .collect::<Vec<_>>()
        .join("\n");
    panic!("timed out waiting for {what}\n--- screen ---\n{screen}\n--------------");
}

fn send(session: &mut Session<NoopTap>, line: &str) {
    session.write(line.as_bytes()).expect("write to pty");
}

/// Whether any row *contains* `text`.
///
/// `contains` and not equality, deliberately. The shell writes its
/// prompt and the command's output into the same row whenever the
/// output arrives before the prompt's newline is processed, so a row
/// reads "$ HELLO-ROBCO" about as often as it reads "HELLO-ROBCO". Which
/// one you get is a race with a real process, and it is not the race
/// this test is about.
fn on_screen(session: &Session<NoopTap>, text: &str) -> bool {
    live_text(session.term()).iter().any(|l| l.contains(text))
}

/// The child's own view of its size, which only `TIOCSWINSZ` gives it.
/// Asking `tput` is the only way to prove the resize crossed the PTY
/// rather than just moving our grid.
///
/// `tag` distinguishes one call from the next: the transcript returns to
/// 100x30 at the end, and without a tag the *earlier* 100x30 line, still
/// on screen, would satisfy the final assertion without the child ever
/// answering.
fn assert_child_agrees(session: &mut Session<NoopTap>, tag: &str, cols: usize, rows: usize) {
    let expected = format!("SIZE-{tag}-{cols}-{rows}");
    send(
        session,
        &format!("printf 'SIZE-{tag}-%s-%s\\n' $(tput cols) $(tput lines)\n"),
    );
    let want = expected.clone();
    wait_for(session, &format!("child to report {expected}"), move |s| {
        on_screen(s, &want)
    });
}

#[test]
fn transcript_survives_a_resize_and_a_dpr_change() {
    // 800x480 physical at DPR 1 with a 10x20 cell: an 80x24 grid.
    let mut viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let size = viewport.term_size();
    assert_eq!((size.cols(), size.rows()), (80, 24));

    let mut session = Session::spawn(&config(), size, NoopTap::default()).expect("spawn session");

    // ---- the transcript, before anything moves -------------------------
    send(&mut session, "printf 'HELLO-ROBCO\\n'\n");
    wait_for(&mut session, "HELLO-ROBCO", |s| on_screen(s, "HELLO-ROBCO"));
    assert_child_agrees(&mut session, "a", 80, 24);

    // A cell read individually, not just a row read as text: this is
    // where a NUL would surface if the normalisation were missing.
    // The row's starting column is not asserted, because the prompt may
    // or may not share it; what must hold is that the row is exactly one
    // char per column and that everything past the text is a space.
    {
        let t = session.term();
        // The *last* matching row: the shell echoes the command line,
        // so "HELLO-ROBCO" appears first inside `printf 'HELLO-ROBCO\n'`
        // and only then as the output we actually want to inspect.
        let row = live_text(t)
            .into_iter()
            .rposition(|l| l.contains("HELLO-ROBCO"))
            .expect("HELLO-ROBCO on screen");
        let cells = term::row_cells(t, row as i32);
        assert_eq!(cells.len(), 80, "a row is exactly one char per column");
        let text: String = cells.iter().collect();
        let end = text.find("HELLO-ROBCO").expect("text in the row") + "HELLO-ROBCO".len();
        assert!(
            cells[end..].iter().all(|&c| c == ' '),
            "untouched cells must read as spaces, not NUL: {:?}",
            &cells[end..]
        );
    }

    // ---- a window resize -----------------------------------------------
    viewport.width = 1000;
    viewport.height = 600;
    let resized = viewport.term_size();
    assert_eq!((resized.cols(), resized.rows()), (100, 30));
    session.resize(resized).expect("resize");
    assert_eq!(session.size(), resized);
    assert_eq!(session.grid_cols(), 100);
    assert_eq!(session.grid_rows(), 30);
    assert_child_agrees(&mut session, "b", 100, 30);

    // The session still works as a terminal afterwards, which a resize
    // that corrupted the grid would not survive.
    send(&mut session, "printf 'AFTER-RESIZE\\n'\n");
    wait_for(&mut session, "AFTER-RESIZE", |s| {
        on_screen(s, "AFTER-RESIZE")
    });

    // ---- a DPR change, with the window untouched -----------------------
    // Dragging to a HiDPI screen: same physical pixels, denser display,
    // so the cell doubles and half as much fits. There is no `Resized`
    // event behind this, which is exactly why it gets its own handling.
    viewport.scale_factor = 2.0;
    let scaled = viewport.term_size();
    assert_eq!(
        (scaled.cols(), scaled.rows()),
        (50, 15),
        "a doubled DPR halves the grid"
    );
    session.resize(scaled).expect("resize for dpr");
    assert_eq!(session.grid_cols(), 50);
    assert_eq!(session.grid_rows(), 15);
    assert_child_agrees(&mut session, "c", 50, 15);

    send(&mut session, "printf 'AFTER-DPR\\n'\n");
    wait_for(&mut session, "AFTER-DPR", |s| on_screen(s, "AFTER-DPR"));

    // ---- and back down again -------------------------------------------
    // Returning to DPR 1 must restore the grid, not leave it stuck at
    // the scaled size.
    viewport.scale_factor = 1.0;
    let restored = viewport.term_size();
    assert_eq!((restored.cols(), restored.rows()), (100, 30));
    session.resize(restored).expect("resize back");
    assert_child_agrees(&mut session, "d", 100, 30);
}

#[test]
fn scrollback_survives_the_lines_leaving_the_screen() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let mut session =
        Session::spawn(&config(), viewport.term_size(), NoopTap::default()).expect("spawn");

    send(
        &mut session,
        "i=0; while [ $i -lt 60 ]; do printf 'FILL-%03d\\n' $i; i=$((i+1)); done\n",
    );
    wait_for(&mut session, "FILL-059", |s| on_screen(s, "FILL-059"));

    // FILL-000 is long gone from a 24-row screen, so finding it proves
    // the read-back reaches above the screen and normalises there too.
    // `contains`, not equality: the shell's prompt shares the line with
    // the first line of output, so that row reads "$ FILL-000".
    assert!(
        !live_text(session.term())
            .iter()
            .any(|l| l.contains("FILL-000")),
        "FILL-000 should have scrolled off"
    );
    assert!(session.history_size() > 0);
    assert!(
        term::all_text(session.term())
            .iter()
            .any(|l| l.contains("FILL-000")),
        "FILL-000 not found in scrollback"
    );
}

/// A paste is one write, and a big one. The tty's input buffer is a few
/// kilobytes deep, and the master is `O_NONBLOCK`, so a write past that
/// point takes a prefix and refuses the rest: the whole middle of a pasted
/// script would be missing, and the shell would run whatever the two ends
/// joined up to say. This asserts the queue behind [`Session::write`] --
/// every byte reaches the child, in order, however far past the buffer the
/// paste goes.
///
/// The child is `cat` under `stty raw -echo`, so what comes back is what
/// arrived: no line discipline echo doubling it, no CR/LF translation
/// changing its length. It announces itself first, because the paste must
/// not race the `stty` that makes those guarantees true.
#[test]
fn a_paste_far_past_the_ttys_buffer_reaches_the_child_whole() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let config = SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "stty raw -echo; printf 'RAW-READY\\n'; exec cat".to_string(),
        ],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: None,
    };
    let mut session =
        Session::spawn(&config, viewport.term_size(), NoopTap::default()).expect("spawn");
    wait_for(&mut session, "cat to come up raw", |s| {
        on_screen(s, "RAW-READY")
    });

    // Sixteen times the 4 KiB a Linux tty will hold, in one call, with no
    // newline anywhere in it: nothing here can be flushed early by the line
    // discipline, so the only thing that can deliver it is the queue.
    const PASTE: usize = 64 * 1024;
    let paste: Vec<u8> = (0..PASTE).map(|i| b'a' + (i % 26) as u8).collect();
    session.write(&paste).expect("the paste is accepted");

    // What `cat` reads it writes back, so the byte count returning is the
    // byte count that arrived.
    let mut echoed = 0usize;
    let deadline = Instant::now() + Duration::from_secs(20);
    while echoed < PASTE {
        echoed += session.pump().bytes;
        assert!(
            Instant::now() < deadline,
            "the paste stalled at {echoed} of {PASTE} bytes"
        );
        std::thread::sleep(Duration::from_millis(2));
    }
    assert_eq!(echoed, PASTE, "the child read exactly what was pasted");
}

/// The other half of the paste above: a child that is *not* reading.
///
/// The queue that carries a paste past the tty's buffer had no ceiling, so a
/// child that had stopped reading its tty -- stopped, wedged, or busy for a
/// long time -- turned every keystroke and every terminal report aimed at it
/// into permanent memory, with nothing anywhere to drain it. This measures the
/// high-water mark over an offer four times the cap and asserts the cap is
/// what bounded it.
///
/// `sleep` is the child because it never reads stdin at all, which is the
/// worst case rather than a slow one: the tty's own buffer takes a few
/// kilobytes and then nothing more moves for the length of the test.
#[test]
fn a_child_that_never_reads_bounds_the_queue_instead_of_growing_it() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let config = SessionConfig {
        program: Some("/bin/sleep".to_string()),
        args: vec!["30".to_string()],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: None,
    };
    let mut session =
        Session::spawn(&config, viewport.term_size(), NoopTap::default()).expect("spawn");

    const CHUNK: usize = 64 * 1024;
    let chunk: Vec<u8> = (0..CHUNK).map(|i| b'a' + (i % 26) as u8).collect();
    let cap = term::INPUT_CAP;

    let mut offered = 0usize;
    let mut high_water = 0usize;
    while offered < 4 * cap {
        session.write(&chunk).expect("a refusal is not an error");
        offered += CHUNK;
        high_water = high_water.max(session.queued_input());
        // The read side runs too, exactly as the host's loop runs it: this is
        // the queue under a live pump and not a queue nobody is servicing.
        session.pump();
    }

    assert!(
        high_water <= cap + CHUNK,
        "the queue reached {high_water} byte(s) against a cap of {cap}, \
         with {offered} offered"
    );
    assert!(
        high_water >= cap,
        "the cap must be the thing that bound it, not the offer: {high_water}"
    );
    // And the session is still usable: a refusal is not a death.
    assert!(!session.is_finished(), "the child outlived the shed");
}

/// A rate bounds the read itself, and nothing queues behind it.
///
/// Measured against an unthrottled session of the same age running the same
/// program, because what a machine moves in an interval is not a constant,
/// and the ratio between the two is what a rate claims to set. The interval
/// is however long the unthrottled session takes to move five thousand
/// bytes, and the throttled one is bounded by what 120 bytes a second, 1200
/// baud, earns over that measured span, with a second's worth of credit as
/// the ceiling above it.
#[test]
fn a_rate_bounds_what_the_read_loop_takes() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let dumping = |rate| SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "stty raw -echo; yes x | head -c 20000".to_string(),
        ],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 1000,
        grapheme_clustering: false,
        rate,
    };
    let size = viewport.term_size();
    let mut free = Session::spawn(&dumping(None), size, NoopTap::default()).expect("spawn");
    // Stamped before the spawn, so the span it measures covers every
    // moment the throttled session could have been earning credit.
    let started = Instant::now();
    let mut slow = Session::spawn(&dumping(Some(120)), size, NoopTap::default()).expect("spawn");

    let (mut free_bytes, mut slow_bytes) = (0usize, 0usize);
    let deadline = Instant::now() + Duration::from_secs(10);
    while free_bytes <= 5_000 {
        assert!(
            Instant::now() < deadline,
            "the unthrottled session took {free_bytes} bytes of the dump, so the \
             comparison is measuring something other than the rate"
        );
        free_bytes += free.pump().bytes;
        slow_bytes += slow.pump().bytes;
        std::thread::sleep(Duration::from_millis(5));
    }
    let span = started.elapsed();

    let earned = (120.0 * span.as_secs_f64()).ceil() as usize + 120;
    assert!(
        slow_bytes <= earned,
        "the throttled session took {slow_bytes} bytes where 120 a second \
         earns at most {earned} over {span:?}"
    );
}

/// The rate lifts while the control-mode envelope is open.
///
/// A gateway's PTY carries tmux's protocol rather than a picture of a
/// shell. The capture that fills a session's windows arrives as one burst
/// for the gateway to parse, and metering that at reading speed would hold
/// a whole bank off the screen for a minute.
#[test]
fn control_mode_lifts_the_rate() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let config = SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "stty raw -echo; printf '\\033P1000p'; yes x | head -c 20000".to_string(),
        ],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: Some(120),
    };
    let mut session = Session::spawn(&config, viewport.term_size(), ControlModeTap::default())
        .expect("spawn");

    let deadline = Instant::now() + Duration::from_secs(20);
    while !session.tap().in_control_mode() {
        assert!(
            Instant::now() < deadline,
            "the child never opened the envelope"
        );
        session.pump();
        std::thread::sleep(Duration::from_millis(5));
    }

    // Five thousand bytes is more than 120 a second earns inside the
    // deadline, so reaching it is the lift, however the run is scheduled.
    let mut bytes = 0usize;
    let deadline = Instant::now() + Duration::from_secs(10);
    while bytes <= 5_000 {
        assert!(
            Instant::now() < deadline,
            "the gateway's stream was metered at the rate: {bytes} bytes"
        );
        bytes += session.pump().bytes;
        std::thread::sleep(Duration::from_millis(5));
    }
}

/// Control mode takes the wire, and the queue lets go of it.
///
/// The gateway writes this PTY through its own `dup` of the master while the
/// input queue writes it here, and the whole arrangement rests on one thing:
/// while control mode is active, nothing writes that fd but the gateway. A
/// tail left queued from before the envelope opened -- the rest of a paste the
/// tty's buffer refused, which is the case this queue exists for, and a paste
/// ending in `tmux -CC` is how the two meet -- would flush underneath the
/// gateway on the next pump and splice a half-line into the middle of a
/// command.
///
/// The child stops reading, then announces control mode: the queue must be
/// empty by the time the pump that saw the envelope returns, because the host
/// runs the gateway's first write later in that same pump.
#[test]
fn control_mode_opening_takes_the_queued_input_with_it() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let config = SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            // Reads nothing from here on, then opens the envelope.
            "stty raw -echo; sleep 0.3; printf '\\033P1000p'; sleep 30".to_string(),
        ],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: None,
    };
    let mut session =
        Session::spawn(&config, viewport.term_size(), ControlModeTap::default()).expect("spawn");

    // Far past what a tty will hold for a child that is not reading, so a tail
    // is certain to be left queued here.
    let paste: Vec<u8> = (0..256 * 1024).map(|i| b'a' + (i % 26) as u8).collect();
    session.write(&paste).expect("the paste is accepted");
    assert!(
        session.queued_input() > 0,
        "the tty took the whole paste; there is no backpressure to test"
    );

    let deadline = Instant::now() + Duration::from_secs(20);
    while !session.tap().in_control_mode() {
        assert!(
            Instant::now() < deadline,
            "the child never opened the envelope"
        );
        session.pump();
        std::thread::sleep(Duration::from_millis(5));
    }

    // The pump that saw the envelope is the one that had to let go.
    assert_eq!(
        session.queued_input(),
        0,
        "the queue is still holding bytes the gateway's wire would splice"
    );

    // And it stays let go. The tty under this child is full and it is still
    // not reading, so a second paste this size could only end up queued --
    // unless the write was refused outright, which is what a gateway
    // swallowing every byte typed, pasted or reported at it means.
    session.write(&paste).expect("swallowed, not an error");
    assert_eq!(
        session.queued_input(),
        0,
        "a write during control mode reached the queue, and from there the wire"
    );
    session.pump();
    assert_eq!(session.queued_input(), 0);
}

/// The tap this test hangs on the session counts and nothing else
/// (`NoopTap`, the test double the shipped `ControlModeTap` grew out of),
/// because the wiring is what is under test here and not the policy above
/// it. This asserts what owning the read loop buys: the body reaches the
/// tap, and it does not reach the grid.
#[test]
fn a_dcs_block_reaches_the_tap_and_stays_off_the_grid() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let mut session =
        Session::spawn(&config(), viewport.term_size(), NoopTap::default()).expect("spawn");

    // `printf` emits the escape itself, so the DCS arrives from the
    // child as real PTY output rather than being injected by the test.
    // The command line is echoed, so the assertion below looks for the
    // body's text on the *output* line, which is the line after it.
    send(
        &mut session,
        "printf '\\033P1000p%%begin 1 1 1\\r\\n%%end 1 1 1\\033\\\\AFTER-DCS\\n'\n",
    );
    // `contains`: the prompt shares the line, so it reads "$ AFTER-DCS".
    wait_for(&mut session, "AFTER-DCS", |s| {
        live_text(s.term())
            .iter()
            .skip(1)
            .any(|l| l.contains("AFTER-DCS"))
    });

    let tap = session.tap();
    assert_eq!(tap.hooks, 1, "DCS hook did not fire on the tap");
    assert_eq!(tap.unhooks, 1, "DCS block never closed on the tap");
    assert!(tap.body_bytes > 0, "no DCS body bytes reached the tap");

    // The echoed command line contains "%begin" as literal text, so the
    // check is that no line *is* the body: the DCS body was never
    // rendered as its own output line.
    let screen = live_text(session.term());
    assert!(
        !screen.iter().any(|l| l.trim() == "%begin 1 1 1"),
        "DCS body leaked onto the grid: {screen:?}"
    );
}

/// A program that asks the terminal a question hears an answer.
///
/// `Crosswords` writes nothing itself: a primary-DA probe or a cursor-position
/// report becomes a `RioEvent::PtyWrite` handed to its listener, and rio-vt's
/// own `VoidListener` takes the trait's default for that, which is nothing. A
/// child's every query was parsed, answered into the void, and left the child
/// waiting on a read that was never going to complete -- vim's DA probe at
/// startup, a script placing itself by CPR.
///
/// The child asks before it has printed anything, so the cursor is still at
/// the home position and the expected CPR is exact rather than a shape. It
/// then becomes `cat -v`, which writes back what it reads with the escapes
/// made visible, so the answers arrive on the grid as literal text instead of
/// as sequences the parser would consume on the way in. `stty raw -echo`
/// first, for the same reason the paste test needs it: what comes back must
/// be what the session sent, undoubled by the line discipline.
#[test]
fn a_query_from_the_child_is_answered_on_the_wire() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let config = SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "stty raw -echo; printf '\\033[c\\033[6n'; exec cat -v".to_string(),
        ],
        working_directory: None,
        env: vec![("TERM".to_string(), "xterm-256color".to_string())],
        scrollback: 1000,
        grapheme_clustering: false,
        rate: None,
    };
    let mut session =
        Session::spawn(&config, viewport.term_size(), NoopTap::default()).expect("spawn");

    // Primary DA, as rio-vt answers it: a VT220 with sixel, selective erase,
    // ANSI colour and OSC 52.
    wait_for(&mut session, "the DA reply to reach the child", |s| {
        on_screen(s, "^[[?62;4;6;22;52c")
    });
    // CPR, from the home position the child never left.
    wait_for(&mut session, "the CPR reply to reach the child", |s| {
        on_screen(s, "^[[1;1R")
    });
}

/// The startup race, pinned.
///
/// Pumping immediately after `spawn` lands in the window where the
/// parent has closed its copy of the slave fd and the child has not yet
/// opened it. The PTY reports EIO there, which is indistinguishable by
/// errno from the shell having exited; treating it as EOF killed the
/// session before it started, and only on a loaded machine.
#[test]
fn pumping_before_the_child_is_up_does_not_end_the_session() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let mut session =
        Session::spawn(&config(), viewport.term_size(), NoopTap::default()).expect("spawn");

    // Hammer the loop through the exec window.
    for _ in 0..200 {
        let pumped = session.pump();
        assert!(
            !pumped.eof,
            "the session declared EOF before the child had started"
        );
    }
    assert!(!session.is_finished());

    // And it is still a working terminal afterwards.
    send(&mut session, "printf 'ALIVE\\n'\n");
    wait_for(&mut session, "ALIVE", |s| on_screen(s, "ALIVE"));
}

/// The other half: a child that really does exit must be reported, or
/// the app would never close its window.
#[test]
fn a_child_that_exits_is_reported_as_eof() {
    let viewport = Viewport::new(800, 480, 1.0, CellSize::new(10.0, 20.0));
    let mut session =
        Session::spawn(&config(), viewport.term_size(), NoopTap::default()).expect("spawn");

    send(&mut session, "printf 'BYE\\n'\n");
    wait_for(&mut session, "BYE", |s| on_screen(s, "BYE"));
    send(&mut session, "exit\n");

    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if session.pump().eof {
            break;
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    assert!(session.is_finished(), "the exited child was never reported");
}
