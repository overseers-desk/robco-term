//! A terminal with no window: rio-vt driven over a real PTY, so the escape
//! sequence conformance suite can be run against the core this rebuild sits
//! on without a display server, a GPU, or a font.
//!
//! Why it exists, and why it is not a toy. esctest runs *inside* the terminal
//! under test: it is spawned as the child of a PTY, writes escape sequences to
//! its own tty and reads the terminal's replies back. So a conformance run
//! needs exactly three things, none of which is a renderer: a PTY, something
//! that advances the VT state machine, and something that answers queries. The
//! glyph grid renders whatever the grid holds, so pointing the suite at the
//! core is pointing it at everything a pixel could disagree about.
//!
//! The loop is the one the application owns the bytes for: they are read
//! from the PTY and handed to `Processor::advance` *and* to a second parser of
//! our own. That second parser is where the app taps tmux control mode
//! (`term::tmux_cc::ControlModeTap`); here it earns its keep because window operations
//! (`CSI 8 ; rows ; cols t`, the resize esctest performs before every single
//! test) are the terminal *application's* business and rio-vt passes them
//! through unhandled.
//!
//! Usage:
//!
//! ```text
//! esctest-harness --cols 80 --rows 25 -- python3 esctest.py --include CUP ...
//! ```
//!
//! Exit code is the child's, so a wrapper script can gate on it.

#[cfg(unix)]
use std::io::{Read, Write};
#[cfg(unix)]
use std::sync::{Arc, Mutex};

#[cfg(unix)]
use rio_vt::ansi::CursorShape;
#[cfg(unix)]
use rio_vt::crosswords::grid::Dimensions;
#[cfg(unix)]
use rio_vt::crosswords::Crosswords;
#[cfg(unix)]
use rio_vt::event::{EventListener, RioEvent, WindowId, WindowSize};
#[cfg(unix)]
use rio_vt::performer::handler::Processor;
#[cfg(unix)]
use rio_vt::performer::parser::{Params, Parser, Perform};
#[cfg(unix)]
use rio_vt::teletypewriter::{create_pty_with_spawn, ProcessReadWrite};

/// Cell size in pixels. Only pixel-unit reports (`CSI 14 t`, `CSI 16 t`) can
/// see it; it is the metric a real window would take from `CellMetrics`.
#[cfg(unix)]
const CELL_W: u16 = 8;
#[cfg(unix)]
const CELL_H: u16 = 16;
#[cfg(unix)]
const SCROLLBACK: usize = 1000;

#[derive(Clone, Copy)]
#[cfg(unix)]
struct Size {
    cols: usize,
    rows: usize,
}

#[cfg(unix)]
impl Dimensions for Size {
    fn total_lines(&self) -> usize {
        self.rows
    }
    fn screen_lines(&self) -> usize {
        self.rows
    }
    fn columns(&self) -> usize {
        self.cols
    }
}

/// Everything the terminal wants to say back to the child, in order.
///
/// rio-vt reports through its listener rather than through a return value
/// (`RioEvent::PtyWrite` for the ordinary CSI replies, and one callback
/// variant for the pixel-size report, which only the window layer can answer).
#[derive(Clone, Default)]
#[cfg(unix)]
struct Replies {
    out: Arc<Mutex<Vec<u8>>>,
    size: Arc<Mutex<WindowSize>>,
}

#[cfg(unix)]
impl Replies {
    fn take(&self) -> Vec<u8> {
        std::mem::take(&mut *self.out.lock().unwrap())
    }

    fn push(&self, text: &str) {
        self.out.lock().unwrap().extend_from_slice(text.as_bytes());
    }

    fn set_size(&self, cols: usize, rows: usize) {
        *self.size.lock().unwrap() = window_size(cols, rows);
    }
}

#[cfg(unix)]
impl EventListener for Replies {
    fn send_event(&self, event: RioEvent, _id: WindowId) {
        match event {
            RioEvent::PtyWrite(_, text) => self.push(&text),
            RioEvent::TextAreaSizeRequest(_, callback) => {
                let size = *self.size.lock().unwrap();
                self.push(&callback(size));
            }
            _ => {}
        }
    }
}

#[cfg(unix)]
fn window_size(cols: usize, rows: usize) -> WindowSize {
    WindowSize {
        rows: rows as u16,
        cols: cols as u16,
        width: cols as u16 * CELL_W,
        height: rows as u16 * CELL_H,
    }
}

/// The application's own view of the byte stream, running beside rio-vt's.
///
/// It watches for the window operations a terminal application owns rather
/// than its VT core: resize by characters, and the position/state reports that
/// would otherwise leave a query unanswered and a test hanging on its read.
#[derive(Default)]
#[cfg(unix)]
struct WinOps {
    resize: Option<(usize, usize)>,
    replies: Vec<String>,
    /// A pending `DECRQCRA`: request id, and the rectangle in 1-based
    /// inclusive screen coordinates (top, left, bottom, right).
    checksum: Option<(u16, [usize; 4])>,
}

#[cfg(unix)]
impl Perform for WinOps {
    fn csi_dispatch(&mut self, params: &Params, intermediates: &[u8], _ignore: bool, action: char) {
        // DECRQCRA: report a checksum over a rectangle of the screen. This is
        // how esctest reads the screen back: every content assertion in the
        // suite is one of these per cell, and it is a terminal-application
        // facility rather than a VT-core one, so it lives here beside the
        // window operations rather than in rio-vt.
        if action == 'y' && intermediates == [b'*'] {
            let flat: Vec<u16> = params.iter().map(|p| p[0]).collect();
            let pid = flat.first().copied().unwrap_or(0);
            let at = |i: usize| flat.get(i).copied().unwrap_or(0) as usize;
            self.checksum = Some((pid, [at(2), at(3), at(4), at(5)]));
            return;
        }
        if action != 't' || !intermediates.is_empty() {
            return;
        }
        let flat: Vec<u16> = params.iter().map(|p| p[0]).collect();
        match flat.first().copied().unwrap_or(1) {
            // Resize the text area to Ps2 lines by Ps3 columns. esctest issues
            // this before every test, and a terminal that ignores it reports a
            // size the suite did not ask for for the rest of the run.
            8 => {
                let rows = flat.get(1).copied().unwrap_or(0) as usize;
                let cols = flat.get(2).copied().unwrap_or(0) as usize;
                if rows > 0 && cols > 0 {
                    self.resize = Some((cols, rows));
                }
            }
            // Report window position. There is no window; the origin is the
            // only honest answer and it keeps the reader from timing out.
            13 => self.replies.push("\x1b[3;0;0t".to_string()),
            // Report window state: not iconified.
            11 => self.replies.push("\x1b[1t".to_string()),
            _ => {}
        }
    }
}

#[cfg(unix)]
fn main() {
    let mut args = std::env::args().skip(1);
    let mut cols = 80usize;
    let mut rows = 25usize;
    let mut command: Vec<String> = Vec::new();
    while let Some(arg) = args.next() {
        match arg.as_str() {
            "--cols" => cols = args.next().and_then(|v| v.parse().ok()).unwrap_or(cols),
            "--rows" => rows = args.next().and_then(|v| v.parse().ok()).unwrap_or(rows),
            "--" => {
                command.extend(args.by_ref());
                break;
            }
            other => {
                eprintln!("esctest-harness: unexpected argument {other:?}");
                std::process::exit(2);
            }
        }
    }
    if command.is_empty() {
        eprintln!("usage: esctest-harness [--cols N] [--rows N] -- <command> [args...]");
        std::process::exit(2);
    }

    let replies = Replies::default();
    replies.set_size(cols, rows);

    let mut term = Crosswords::new(
        Size { cols, rows },
        CursorShape::Block,
        replies.clone(),
        WindowId::from(0u64),
        0,
        SCROLLBACK,
    );
    let mut processor = Processor::default();

    let mut pty = create_pty_with_spawn(
        Some(&command[0]),
        command[1..].to_vec(),
        &None,
        Some(vec![
            ("TERM".to_string(), "xterm".to_string()),
            // esctest reads its own stdout; unbuffered Python keeps the
            // request/response ordering the suite assumes.
            ("PYTHONUNBUFFERED".to_string(), "1".to_string()),
            ("LC_ALL".to_string(), "en_US.UTF-8".to_string()),
        ]),
        cols as u16,
        rows as u16,
        cols as u16 * CELL_W,
        rows as u16 * CELL_H,
    )
    .expect("spawn pty");

    let mut winops_parser = Parser::default();
    let mut buf = [0u8; 8192];
    let exit_code = loop {
        let read = match pty.read(&mut buf) {
            Ok(0) => break wait_for_child(&pty),
            Ok(n) => n,
            // teletypewriter opens the master non-blocking (it is built for an
            // event loop we are not using), so "nothing yet" arrives as an
            // error rather than as a block. Sleeping a millisecond is well
            // inside esctest's one-second read timeout and costs nothing;
            // treating it as end-of-stream, which is the obvious reading of
            // the error, closes the master and kills the child.
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock => {
                std::thread::sleep(std::time::Duration::from_millis(1));
                continue;
            }
            Err(e) if e.kind() == std::io::ErrorKind::Interrupted => continue,
            // EIO on a Linux PTY means the child closed the slave: normal exit.
            Err(_) => break wait_for_child(&pty),
        };
        let bytes = &buf[..read];

        // Both consumers see every byte, and they see them in step. The tap
        // runs one byte ahead; the moment it recognises a window operation the
        // terminal is advanced up to that byte, the operation is applied, and
        // the stream continues. Advancing the whole chunk first instead would
        // answer a size query with the size the terminal had *before* a resize
        // earlier in the same write, which is exactly what esctest does in its
        // per-test reset.
        let mut pending = 0usize;
        for i in 0..bytes.len() {
            let mut winops = WinOps::default();
            winops_parser.advance(&mut winops, &bytes[i..=i]);
            if winops.resize.is_none() && winops.replies.is_empty() && winops.checksum.is_none() {
                continue;
            }
            processor.advance(&mut term, &bytes[pending..=i]);
            pending = i + 1;

            if let Some((new_cols, new_rows)) = winops.resize {
                term.resize(Size {
                    cols: new_cols,
                    rows: new_rows,
                });
                replies.set_size(new_cols, new_rows);
                let _ = pty.set_winsize(window_size(new_cols, new_rows).into());
            }
            if let Some((pid, rect)) = winops.checksum {
                replies.push(&checksum_report(&term, pid, rect));
            }
            for reply in &winops.replies {
                replies.push(reply);
            }
        }
        processor.advance(&mut term, &bytes[pending..]);

        let out = replies.take();
        if !out.is_empty() && pty.write_all(&out).and_then(|_| pty.flush()).is_err() {
            break wait_for_child(&pty);
        }

        // Reading the grid every frame is what a renderer would do; doing it
        // here keeps the damage path on the same code the application runs
        // rather than leaving it untested until there is a window.
        let _ = term.damage();
        term.reset_damage();
    };

    std::process::exit(exit_code);
}

/// `DCS Pid ! ~ xxxx ST`, the DECRQCRA reply.
///
/// The number is xterm's: the 16-bit sum of the cells' codepoints, negated.
/// Untouched cells read `'\0'` from rio-vt and are counted as blanks, which is
/// the same normalisation the renderer does at the grid boundary and the
/// reason esctest sees a blank screen as blank rather than as eighty NULs per
/// line.
#[cfg(unix)]
fn checksum_report<L: EventListener>(term: &Crosswords<L>, pid: u16, rect: [usize; 4]) -> String {
    use rio_vt::crosswords::pos::{Column, Line};

    let (rows, cols) = (term.grid.screen_lines(), term.grid.columns());
    let top = rect[0].max(1);
    let left = rect[1].max(1);
    let bottom = if rect[2] == 0 {
        rows
    } else {
        rect[2].min(rows)
    };
    let right = if rect[3] == 0 {
        cols
    } else {
        rect[3].min(cols)
    };

    let mut sum: u32 = 0;
    for row in top..=bottom {
        for col in left..=right {
            if row > rows || col > cols {
                continue;
            }
            let c = term.grid[Line(row as i32 - 1)][Column(col - 1)].c();
            sum += if c == '\0' { 0x20 } else { c as u32 };
        }
    }
    format!(
        "\x1bP{pid}!~{:04X}\x1b\\",
        (0x10000u32.wrapping_sub(sum & 0xFFFF)) & 0xFFFF
    )
}

#[cfg(unix)]
fn wait_for_child(pty: &rio_vt::teletypewriter::Pty) -> i32 {
    // The child has closed its end; give it a moment to be reaped so the exit
    // status is the suite's own rather than a guess.
    for _ in 0..100 {
        match pty.waitpid() {
            Ok(Some(status)) => return status,
            Ok(None) => std::thread::sleep(std::time::Duration::from_millis(20)),
            Err(_) => return 0,
        }
    }
    0
}

/// The harness drives esctest as the child of a real pty, the shape the
/// suite itself assumes; conformance runs live on Unix boxes.
#[cfg(not(unix))]
fn main() {
    eprintln!("esctest-harness: the conformance suite runs against a Unix pty; run it there");
}
