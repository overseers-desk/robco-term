//! The destination picker `Shift+Alt+T` raises: where one new channel
//! goes, chosen on the glass, and whether that choice becomes the default.
//!
//! The page is a channel: the chord takes a free home slot with a bare
//! screen ([`term::TmuxPane`], a grid with no transport), these functions
//! paint it by escape sequences through its own parser, and the surface
//! intercepts its keys ahead of the keytab.
//! So the phosphor, the curvature and the cursor all apply for free, and
//! nothing new is rendered anywhere.
//!
//! Pure over the config's `[[ssh.host]]` rows and one small state, no I/O,
//! no session, no file, the host wires the actions. Digits choose, the
//! bank's own idiom; `1` is localhost, matching the settings tab's radio
//! order; `2` to `9` are the first eight configured rows; `Esc` steps back
//! out of the page.
//!
//! There is no arm for a hand-typed destination. A hostname alone cannot
//! name a user or a key file, so typing one here connected wrong more often
//! than right; naming a destination is the settings window's job, and this
//! page only ever lists what it was given.
//!
//! The checkbox is the whole of the default connection's provenance. Nothing
//! else in the program writes `ssh.default`: the destination the user is
//! looking at, at the moment they choose it, is the only place where "and
//! from now on, this one" is a question with an obvious subject. A tick
//! costs one keystroke on a page already open, and an untouched page writes
//! nothing at all.

use config::SshHost;
use winit::keyboard::{Key, NamedKey};

/// How many rows a digit can reach: `1` is localhost, `2` to `9` are the
/// first eight configured servers. Rows past that are named in the page's
/// footer as the settings window's, rather than paged here.
pub const DIGIT_ROWS: usize = 8;

/// What one key means to the picker.
#[derive(Debug, PartialEq, Eq)]
pub enum Verdict {
    /// Open a local shell.
    Localhost,
    /// Open a connection to `hosts[index]`.
    Host(usize),
    /// Close the picker, open nothing.
    Cancel,
    /// Not a key that opens anything: swallowed, so the page under it stays
    /// put. The page may well have changed -- the checkbox toggled -- which
    /// is why the host repaints on this verdict.
    Ignored,
}

/// The page while it stands: which slot it is on, and everything the user
/// has said to it so far.
pub struct Picker {
    /// The home slot its page holds.
    pub slot: u32,
    /// Whether the destination chosen here becomes the default connection.
    make_default: bool,
}

impl Picker {
    /// A page on `slot`, showing the list, the checkbox clear: the state a
    /// chord raises, and the state a page that writes nothing is in.
    pub fn new(slot: u32) -> Self {
        Self { slot, make_default: false }
    }

    /// Whether the user ticked the box. Read by the host when a verdict
    /// opens something, because that is when the tick means anything.
    pub fn make_default(&self) -> bool {
        self.make_default
    }

    /// Read one key against the page over `hosts`.
    ///
    /// `Tab` toggles the checkbox. It is not a digit, so it cannot be a key
    /// the user meant for anything else, and the box is reachable without
    /// the hand leaving the keyboard for a mouse the glass does not have.
    pub fn key(&mut self, logical: &Key, hosts: &[SshHost]) -> Verdict {
        if matches!(logical, Key::Named(NamedKey::Tab)) {
            self.make_default = !self.make_default;
            return Verdict::Ignored;
        }
        match logical {
            Key::Named(NamedKey::Escape) => Verdict::Cancel,
            Key::Character(c) => {
                let [digit] = c.as_bytes() else {
                    return Verdict::Ignored;
                };
                match digit {
                    b'1' => Verdict::Localhost,
                    b'2'..=b'9' => {
                        let index = usize::from(digit - b'2');
                        if index < hosts.len().min(DIGIT_ROWS) {
                            Verdict::Host(index)
                        } else {
                            Verdict::Ignored
                        }
                    }
                    _ => Verdict::Ignored,
                }
            }
            _ => Verdict::Ignored,
        }
    }
}

/// A row's label: how the destination is spelled on the page.
fn label(row: &SshHost) -> String {
    let mut label = String::new();
    if !row.user.is_empty() {
        label.push_str(&row.user);
        label.push('@');
    }
    label.push_str(&row.host);
    if row.port != 22 {
        label.push(':');
        label.push_str(&row.port.to_string());
    }
    label
}

/// The whole page as bytes for the picker channel's parser: clear, home,
/// the numbered destinations, the checkbox, the footer.
///
/// Repainted whole for every key the page took, and on resize. A dozen
/// lines is cheaper than tracking which of them a keystroke moved.
pub fn paint(hosts: &[SshHost], state: &Picker) -> Vec<u8> {
    let mut lines: Vec<String> = vec![String::new(), "  SELECT DESTINATION".into(), String::new()];
    lines.push("   1  localhost (a local shell)".into());
    for (index, row) in hosts.iter().take(DIGIT_ROWS).enumerate() {
        lines.push(format!("   {}  {}", index + 2, label(row)));
    }
    if hosts.len() > DIGIT_ROWS {
        lines.push(format!(
            "\x1b[2m      and {} more, reachable through the settings window\x1b[0m",
            hosts.len() - DIGIT_ROWS
        ));
    }
    lines.push(String::new());
    lines.push(format!(
        "  [{}] Tab  make this the default connection",
        if state.make_default { "x" } else { " " }
    ));
    lines.push(String::new());
    lines.push("\x1b[2m  a digit connects; Esc cancels\x1b[0m".to_string());

    let mut page = String::from("\x1b[2J\x1b[H");
    page.push_str(&lines.join("\r\n"));
    page.push_str("\r\n");
    page.into_bytes()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn host(name: &str, user: &str, port: u16) -> SshHost {
        SshHost { host: name.into(), user: user.into(), port, key: String::new() }
    }

    fn ch(s: &str) -> Key {
        Key::Character(s.into())
    }

    fn named(key: NamedKey) -> Key {
        Key::Named(key)
    }

    #[test]
    fn digits_map_to_rows_and_the_rest_is_swallowed() {
        let hosts = vec![host("vault", "overseer", 22), host("gw", "", 2222)];
        let mut picker = Picker::new(3);
        let mut key = |k: Key| picker.key(&k, &hosts);
        assert_eq!(key(ch("1")), Verdict::Localhost);
        assert_eq!(key(ch("2")), Verdict::Host(0));
        assert_eq!(key(ch("3")), Verdict::Host(1));
        assert_eq!(key(ch("4")), Verdict::Ignored, "no fourth row");
        assert_eq!(key(ch("0")), Verdict::Ignored, "no typed arm to open");
        assert_eq!(key(ch("x")), Verdict::Ignored);
        assert_eq!(key(named(NamedKey::Escape)), Verdict::Cancel);
        assert_eq!(key(named(NamedKey::Enter)), Verdict::Ignored);
    }

    #[test]
    fn the_page_numbers_localhost_first_and_names_the_overflow() {
        let mut hosts: Vec<SshHost> = (0..10).map(|n| host(&format!("h{n}"), "", 22)).collect();
        hosts[1] = host("vault", "overseer", 2222);
        let page = String::from_utf8(paint(&hosts, &Picker::new(3))).unwrap();
        assert!(page.contains("1  localhost"));
        assert!(page.contains("3  overseer@vault:2222"), "{page}");
        assert!(page.contains("9  h7"));
        assert!(!page.contains("h8"), "digits stop at nine rows");
        assert!(page.contains("and 2 more"));
        assert!(page.contains("[ ] Tab  make this the default"), "{page}");
    }

    #[test]
    fn tab_ticks_the_box_and_the_page_shows_it() {
        let hosts = vec![host("vault", "overseer", 22)];
        let mut picker = Picker::new(3);
        assert!(!picker.make_default(), "an untouched page writes nothing");
        assert_eq!(picker.key(&named(NamedKey::Tab), &hosts), Verdict::Ignored);
        assert!(picker.make_default());
        let page = String::from_utf8(paint(&hosts, &picker)).unwrap();
        assert!(page.contains("[x] Tab  make this the default connection"), "{page}");

        picker.key(&named(NamedKey::Tab), &hosts);
        assert!(!picker.make_default(), "the same key unticks it");
        let page = String::from_utf8(paint(&hosts, &picker)).unwrap();
        assert!(page.contains("[ ] Tab"), "{page}");
    }

    /// The tick is the page's own state: it stands until the page is
    /// cancelled or a row is chosen, not tied to any one row.
    #[test]
    fn the_tick_outlasts_a_cancelled_choice() {
        let hosts = vec![host("vault", "overseer", 22)];
        let mut picker = Picker::new(3);
        picker.key(&named(NamedKey::Tab), &hosts);
        assert_eq!(picker.key(&named(NamedKey::Escape), &hosts), Verdict::Cancel);
        assert!(picker.make_default());
    }
}
