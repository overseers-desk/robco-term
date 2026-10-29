//! Handing a link to the platform.
//!
//! A URL, an e-mail address or an OSC 8 URI the pointer opened goes to
//! whatever the desktop opens such things with: `xdg-open` on Linux and the
//! BSDs, `open` on macOS, the shell's URL handler on Windows. The terminal
//! neither knows nor asks which browser that is, and the link travels as one
//! argument, never through a shell.
//!
//! One [`Opener`] per window, on the [`crate::settings::SettingsApp`] model:
//! the children it started are held until the next request reaps them, so a
//! hundred links opened over a week leave no zombie behind. A failure to
//! start, and a handler that exits badly afterwards, are answered on the
//! glass by the caller, with the detail in the log.
//!
//! A headless surface gets the recording form, which keeps the last link it
//! was handed and starts nothing: what the pointer tests read back.

use std::process::{Child, Command, Stdio};

pub struct Opener {
    /// Whether a link starts the platform's handler, or is only kept.
    launches: bool,
    children: Vec<Child>,
    last: Option<String>,
}

impl Opener {
    /// The opener a window on a display gets.
    pub fn platform() -> Self {
        Self {
            launches: true,
            children: Vec::new(),
            last: None,
        }
    }

    /// The opener a headless surface gets: it remembers and starts nothing.
    pub fn recording() -> Self {
        Self {
            launches: false,
            children: Vec::new(),
            last: None,
        }
    }

    /// The last link handed over, whichever form this is.
    pub fn last_opened(&self) -> Option<&str> {
        self.last.as_deref()
    }

    /// Hand `url` to the platform's handler. A refusal says why, in words
    /// for the glass; the log has the detail.
    pub fn open(&mut self, url: &str) -> Result<(), String> {
        self.last = Some(url.to_string());
        if !self.launches {
            return Ok(());
        }
        self.reap();
        let Some((program, args)) = handler(url) else {
            log::warn!("this platform has no handler to open {url} with");
            return Err("no handler to open links".into());
        };
        // Detached from the terminal's own input, as the settings window is:
        // a handler that inherited stdin would read the keystrokes meant for
        // the terminal's child. Its stderr passes through, so a handler that
        // dies in its own startup says so where the terminal was launched.
        let mut command = Command::new(program);
        command
            .args(args)
            .stdin(Stdio::null())
            .stdout(Stdio::null());
        match command.spawn() {
            Ok(child) => self.children.push(child),
            Err(e) => {
                log::warn!("could not start {program}: {e}");
                return Err(format!("could not start {program}"));
            }
        }
        Ok(())
    }

    /// Collect the handlers that have exited, answering what the ones that
    /// exited badly said, for the glass. `xdg-open` and `open` return as
    /// soon as the browser has the link, so this is nearly always all of
    /// them, and a handler still running is left to finish. The surface
    /// calls it from its pump, so a handler that fails after it was started
    /// is answered within a frame or two of failing.
    pub fn reap(&mut self) -> Vec<String> {
        let mut failed = Vec::new();
        self.children.retain_mut(|child| match child.try_wait() {
            Ok(Some(status)) => {
                if !status.success() {
                    log::warn!("the link handler exited with {status}");
                    failed.push(format!("the link handler exited with {status}"));
                }
                false
            }
            Ok(None) => true,
            Err(e) => {
                log::debug!("could not ask after the link handler: {e}");
                true
            }
        });
        failed
    }
}

/// The program that opens a link on this platform, with the arguments that
/// precede the link itself.
#[cfg(all(unix, not(target_os = "macos")))]
fn handler(url: &str) -> Option<(&'static str, Vec<&str>)> {
    Some(("xdg-open", vec![url]))
}

#[cfg(target_os = "macos")]
fn handler(url: &str) -> Option<(&'static str, Vec<&str>)> {
    Some(("open", vec![url]))
}

/// `rundll32`'s URL handler takes the link as an argument and reads no
/// shell syntax, unlike `cmd /C start`, which would treat `&` in a query
/// string as a command separator.
#[cfg(windows)]
fn handler(url: &str) -> Option<(&'static str, Vec<&str>)> {
    Some(("rundll32", vec!["url.dll,FileProtocolHandler", url]))
}

/// A platform with no handler known here opens nothing, and says so once
/// per link in the log.
#[cfg(not(any(unix, windows)))]
fn handler(_url: &str) -> Option<(&'static str, Vec<&str>)> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_recording_opener_keeps_the_link_and_starts_nothing() {
        let mut opener = Opener::recording();
        assert_eq!(opener.last_opened(), None);
        assert_eq!(opener.open("https://example.com/?a=1&b=2"), Ok(()));
        assert_eq!(opener.last_opened(), Some("https://example.com/?a=1&b=2"));
        assert!(opener.children.is_empty());
    }
}
