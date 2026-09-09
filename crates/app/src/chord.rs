//! The channel-selection chord: hold the modifier, type digits, release to
//! commit.
//!
//! Chord input for channel slots: hold the modifier, type digits, release to
//! commit. The host wires the select/store actions and the slot-prefix
//! predicate; this component knows nothing about the model.
//!
//! # Timeout semantics, exactly
//!
//! - The chord modifier is **Alt** (**Meta** on macOS), and the digits are the
//!   `Alt+0`..`Alt+9` shortcuts for a select and `Alt+Shift+0`..`Alt+Shift+9`
//!   for a store.
//! - The **first digit locks the chord's family**: a Shift rolled early or
//!   late on a later digit cannot turn a select into a store or back.
//! - After each digit, the host's predicate is asked whether any slot the
//!   chord could still name has the buffer as a **strict** prefix. If one
//!   does, the chord waits; if none does, it **commits on the spot**. So on a
//!   bank of nine rows, `Alt+3` selects channel 3 immediately, with no wait at
//!   all; the timeout is only ever reached on a bank deep enough for a
//!   two-digit slot.
//! - The wait is **900 ms**, and it exists only for the case where the
//!   modifier release is never observed. Releasing the modifier commits at
//!   once, and so does the window losing focus.
//! - `"0"` commits as **slot 10**, the way a numeric keypad's zero stands at
//!   the end of the row rather than the start. Every other buffer is read as
//!   a decimal integer, so `Alt+1` `Alt+0` is also 10 and `Alt+0` `Alt+1` is
//!   1.
//!
//! Everything above is here; nothing above is anywhere else. The host
//! ([`crate::window`]) owns the keys, the predicate and what a committed slot
//! means.

use std::time::{Duration, Instant};

use winit::keyboard::ModifiersState;

/// The chord modifier: Alt everywhere but macOS, where the Alt key
/// composes text and Meta is the one a shortcut may take.
///
/// The one place that rule is written. The bindings table spells it
/// `chord`, so a shipped row reads the same on every platform; the
/// modifier-change edge reads it to commit a digit chord on the release.
pub fn modifier_mask() -> ModifiersState {
    if cfg!(target_os = "macos") {
        ModifiersState::SUPER
    } else {
        ModifiersState::ALT
    }
}

/// Whether the chord modifier is held.
pub fn modifier_down(modifiers: ModifiersState) -> bool {
    modifiers.contains(modifier_mask())
}

/// Commits the chord if the modifier release is never observed.
pub const COMMIT_TIMEOUT: Duration = Duration::from_millis(900);

/// What a finished chord asks for, in the page slots the bank's numerals
/// read; the host turns these into absolute channels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Chord {
    /// `Alt+<digits>`: bring that slot to the screen.
    Select(u32),
    /// `Alt+Shift+<digits>`: store the session on screen onto that slot.
    Store(u32),
}

impl Chord {
    pub fn slot(self) -> u32 {
        match self {
            Chord::Select(slot) | Chord::Store(slot) => slot,
        }
    }
}

/// The chord's state machine. Pure: it holds digits and a deadline and knows
/// nothing about channels, which is why it can be driven from a test with a
/// hand-made clock.
#[derive(Clone, Debug, Default)]
pub struct ChordInput {
    digits: String,
    store_mode: bool,
    deadline: Option<Instant>,
}

impl ChordInput {
    pub fn new() -> Self {
        Self::default()
    }

    /// Whether digits are waiting to commit.
    pub fn is_pending(&self) -> bool {
        !self.digits.is_empty()
    }

    /// When the chord commits itself if nothing else does, for the host's
    /// wake-up scheduling.
    pub fn deadline(&self) -> Option<Instant> {
        self.deadline
    }

    /// One digit of a chord. `prefix_exists` is the host's predicate, asked
    /// with the buffer so far and the chord's locked family; it answers
    /// differently per family because a store may land on a dark slot and a
    /// select may not.
    pub fn feed_digit(
        &mut self,
        digit: u8,
        store: bool,
        now: Instant,
        prefix_exists: impl FnOnce(&str, bool) -> bool,
    ) -> Option<Chord> {
        if !digit.is_ascii_digit() {
            return None;
        }
        // The first digit locks the chord's family.
        if self.digits.is_empty() {
            self.store_mode = store;
        }
        self.digits.push(digit as char);
        if prefix_exists(&self.digits, self.store_mode) {
            self.deadline = Some(now + COMMIT_TIMEOUT);
            None
        } else {
            self.commit()
        }
    }

    /// Whatever re-labels the numerals abandons the digits typed against the
    /// old labels: they are never committed against the new ones.
    pub fn cancel(&mut self) {
        self.digits.clear();
        self.store_mode = false;
        self.deadline = None;
    }

    /// Commit whatever is buffered. This is what the modifier's release, the
    /// window's deactivation and the timer all call.
    pub fn commit(&mut self) -> Option<Chord> {
        self.deadline = None;
        if self.digits.is_empty() {
            return None;
        }
        // "0" is slot 10; everything else is its own decimal value.
        let slot = if self.digits == "0" {
            10
        } else {
            self.digits.parse::<u32>().unwrap_or(0)
        };
        let store = self.store_mode;
        self.digits.clear();
        self.store_mode = false;
        Some(if store {
            Chord::Store(slot)
        } else {
            Chord::Select(slot)
        })
    }

    /// The 900 ms timer, driven by the host's own clock rather than a
    /// platform timer: commits if the deadline has passed, and answers `None`
    /// otherwise, including when nothing is pending.
    pub fn tick(&mut self, now: Instant) -> Option<Chord> {
        match self.deadline {
            Some(at) if now >= at => self.commit(),
            _ => None,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A bank of nine open slots: no two-digit slot exists, so no digit is ever
    /// a strict prefix of one.
    fn nine(_buf: &str, _store: bool) -> bool {
        false
    }

    /// A bank deep enough that 1 is a strict prefix of 12.
    fn deep(buf: &str, _store: bool) -> bool {
        buf == "1"
    }

    fn t0() -> Instant {
        Instant::now()
    }

    #[test]
    fn a_digit_no_deeper_slot_extends_commits_on_the_spot() {
        let mut chord = ChordInput::new();
        let now = t0();
        assert_eq!(
            chord.feed_digit(b'3', false, now, nine),
            Some(Chord::Select(3))
        );
        assert!(!chord.is_pending());
        assert_eq!(
            chord.deadline(),
            None,
            "nothing is waiting, so nothing is timed"
        );
    }

    #[test]
    fn a_digit_a_deeper_slot_extends_waits_nine_hundred_milliseconds() {
        let mut chord = ChordInput::new();
        let now = t0();
        assert_eq!(chord.feed_digit(b'1', false, now, deep), None);
        assert!(chord.is_pending());
        assert_eq!(chord.deadline(), Some(now + COMMIT_TIMEOUT));

        // A millisecond short of the deadline it is still waiting...
        assert_eq!(
            chord.tick(now + COMMIT_TIMEOUT - Duration::from_millis(1)),
            None
        );
        // ...and at it, the chord commits what it has.
        assert_eq!(chord.tick(now + COMMIT_TIMEOUT), Some(Chord::Select(1)));
        assert!(!chord.is_pending());
    }

    #[test]
    fn a_second_digit_lands_the_deeper_slot_before_the_timeout() {
        let mut chord = ChordInput::new();
        let now = t0();
        chord.feed_digit(b'1', false, now, deep);
        let later = now + Duration::from_millis(300);
        assert_eq!(
            chord.feed_digit(b'2', false, later, deep),
            Some(Chord::Select(12)),
            "12 is nobody's strict prefix, so it commits without waiting again"
        );
    }

    #[test]
    fn the_modifier_release_commits_before_the_timer_does() {
        let mut chord = ChordInput::new();
        let now = t0();
        chord.feed_digit(b'1', false, now, deep);
        assert_eq!(chord.commit(), Some(Chord::Select(1)));
        // ...and the timer has nothing left to fire on.
        assert_eq!(chord.tick(now + COMMIT_TIMEOUT * 2), None);
    }

    #[test]
    fn the_first_digit_locks_the_family() {
        let mut chord = ChordInput::new();
        let now = t0();
        // A store chord whose second digit arrives with Shift already rolled
        // off is still a store.
        chord.feed_digit(b'1', true, now, deep);
        assert_eq!(
            chord.feed_digit(b'2', false, now, deep),
            Some(Chord::Store(12))
        );

        // ...and the other way round.
        chord.feed_digit(b'1', false, now, deep);
        assert_eq!(
            chord.feed_digit(b'2', true, now, deep),
            Some(Chord::Select(12))
        );
    }

    #[test]
    fn zero_is_slot_ten() {
        let mut chord = ChordInput::new();
        let now = t0();
        assert_eq!(
            chord.feed_digit(b'0', false, now, nine),
            Some(Chord::Select(10))
        );
        assert_eq!(
            chord.feed_digit(b'0', true, now, nine),
            Some(Chord::Store(10))
        );
        // A leading zero typed as part of a longer chord is ordinary decimal.
        chord.feed_digit(b'0', false, now, |buf, _| buf == "0");
        assert_eq!(
            chord.feed_digit(b'1', false, now, nine),
            Some(Chord::Select(1))
        );
        // ...and so is a trailing one.
        chord.feed_digit(b'1', false, now, deep);
        assert_eq!(
            chord.feed_digit(b'0', false, now, deep),
            Some(Chord::Select(10))
        );
    }

    #[test]
    fn a_cancel_abandons_the_digits_rather_than_committing_them() {
        let mut chord = ChordInput::new();
        let now = t0();
        chord.feed_digit(b'1', true, now, deep);
        chord.cancel();
        assert!(!chord.is_pending());
        assert_eq!(chord.deadline(), None);
        assert_eq!(chord.commit(), None);
        assert_eq!(chord.tick(now + COMMIT_TIMEOUT * 2), None);
        // The family went with them: the next chord starts clean.
        assert_eq!(
            chord.feed_digit(b'4', false, now, nine),
            Some(Chord::Select(4))
        );
    }

    #[test]
    fn a_commit_with_nothing_buffered_is_nothing() {
        let mut chord = ChordInput::new();
        assert_eq!(chord.commit(), None);
        assert_eq!(chord.tick(t0()), None);
    }
}
