//! The channel bank end to end through the surface the binary runs: the keys
//! that open, close and choose a channel, the digit chords that name one, and
//! the transient the tube gives when the choice lands.
//!
//! `app::channels`, `app::chord` and `app::bank` each pin their own state
//! machine with no window and no pty. What none of them can say is that the
//! three are wired to each other and to a real session: that `Ctrl+Shift+T`
//! reaches the model rather than the keytab, that a chord's slot is resolved
//! against the page the bank is showing, and that a switch triggers the degauss
//! the frame samples. These drive the whole path.
//!
//! Same harness as `keyboard_scroll.rs`: `TerminalSurface::headless` is the
//! surface with no swapchain, and `key_input` is `key_pressed` with winit's own
//! `KeyEvent` peeled off (that type cannot be built outside winit). One
//! addition: the surface is given a cabinet, because the chord and the pager
//! live inside the bank, and a profile with no bank has neither.

use std::time::{Duration, Instant};

use app::window::TerminalSurface;
use chassis::Cabinet;
use config::Config;
use term::{CellSize, SessionConfig, Viewport};
use winit::dpi::PhysicalPosition;
use winit::event::MouseButton;
use winit::keyboard::{Key, ModifiersState, NamedKey};

const CELL_W: f32 = 9.0;
const CELL_H: f32 = 18.0;
/// Tall enough that the stock bank shows eleven rows, so two-digit keys (10 and
/// 11) exist on the page and a chord can actually be made to wait for a second
/// digit. `BankGeometry::rows_visible(730, 130)` on the annunciator over LED:
/// the 130 is the shell's own pager off the bank's foot
/// (`Cabinet::pager_height`), which is why the window is 130 px taller than
/// the row arithmetic alone would ask for.
const WINDOW_W: u32 = 720;
const WINDOW_H: u32 = 730;
const ROWS_ON_PAGE: u32 = 11;

/// The chord modifier, forked per platform.
#[cfg(target_os = "macos")]
const CHORD: ModifiersState = ModifiersState::SUPER;
#[cfg(not(target_os = "macos"))]
const CHORD: ModifiersState = ModifiersState::ALT;

const CTRL: ModifiersState = ModifiersState::CONTROL;
const CTRL_SHIFT: ModifiersState = ModifiersState::CONTROL.union(ModifiersState::SHIFT);

/// A shell that prints one line naming itself and then waits, so a channel can
/// be told from its neighbours by what is on its glass. `$$` is the shell's own
/// pid, which differs per channel without the harness having to give each one a
/// configuration of its own.
///
/// It waits by reading, and sets its window title from whatever it reads: that
/// is how [`the_window_title_is_the_channel_on_the_airs`] makes one channel
/// emit an OSC 0 without the others emitting one too. A channel nothing is
/// written to simply blocks in `read` for the life of the test.
fn scripted() -> SessionConfig {
    SessionConfig {
        program: Some("/bin/sh".to_string()),
        args: vec![
            "-c".to_string(),
            "echo channel-$$; while IFS= read -r t; do printf '\\033]0;%s\\007' \"$t\"; done"
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

fn surface_of_height(height: u32) -> TerminalSurface {
    let viewport = Viewport::new(WINDOW_W, height, 1.0, CellSize::new(CELL_W, CELL_H));
    let mut surface = TerminalSurface::headless(&scripted(), viewport);
    let cfg = Config::default();
    surface.set_cabinet(Cabinet::from_config(
        &cfg,
        f64::from(WINDOW_W),
        f64::from(height),
    ));
    surface
}

fn surface() -> TerminalSurface {
    let surface = surface_of_height(WINDOW_H);
    assert_eq!(
        surface.bank_strips().rows.len() as u32,
        ROWS_ON_PAGE,
        "the harness assumes an eleven-key page"
    );
    surface
}

/// Pump until the visible channel has printed its line, and answer it.
fn wait_for_prompt(surface: &mut TerminalSurface) -> String {
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        surface.pump();
        if let Some(line) = surface
            .viewport_text()
            .iter()
            .find(|l| l.contains("channel-"))
        {
            return line.trim().to_string();
        }
        std::thread::sleep(Duration::from_millis(5));
    }
    panic!(
        "timed out waiting for a channel's own line\n--- screen ---\n{}",
        surface.viewport_text().join("\n")
    );
}

fn character(surface: &mut TerminalSurface, c: &str, modifiers: ModifiersState) {
    surface.key_input(&Key::Character(c.into()), Some(c), modifiers);
}

fn named(surface: &mut TerminalSurface, key: NamedKey, modifiers: ModifiersState) {
    surface.key_input(&Key::Named(key), None, modifiers);
}

/// The chord modifier coming up: what commits a waiting chord.
fn release_chord(surface: &mut TerminalSurface) {
    app::shell::Surface::modifiers_changed(surface, ModifiersState::empty());
}

fn title(surface: &TerminalSurface) -> Option<String> {
    app::shell::Surface::title(surface)
}

/// A point in each of the pager's outer thirds, PREV then NEXT, on the
/// surface's own cabinet: the bank it holds has been through the window's
/// fit, and the press lands on the appliance on screen. The harness runs at
/// a scale factor of 1, so these are physical pixels as they stand.
fn pager_keys(surface: &TerminalSurface) -> (PhysicalPosition<f64>, PhysicalPosition<f64>) {
    let cabinet = surface.cabinet().expect("the harness gave it a cabinet");
    let bank = (cabinet.layout().bank.width, cabinet.layout().bank.height);
    let cfg = Config::default();
    let pager =
        chassis::furniture::pager_rect(cabinet.geometry(), &chassis::shell_metrics(&cfg), bank);
    let y = pager.y + pager.height / 2.0;
    let at = |x: f64, direction: i32| {
        assert_eq!(
            cabinet.pager_at(x, y),
            Some(direction),
            "the harness is pressing somewhere that is not a pager key"
        );
        PhysicalPosition::new(x, y)
    };
    (
        at(pager.x + pager.width / 6.0, -1),
        at(pager.x + pager.width * 5.0 / 6.0, 1),
    )
}

/// The button going down, which is the whole gesture as far as the bank's
/// keys are concerned: the strips and the pager both act on the press.
fn press(surface: &mut TerminalSurface, at: PhysicalPosition<f64>) {
    app::shell::Surface::mouse_pressed(surface, MouseButton::Left, at, ModifiersState::empty());
}

/// Contract item 3: "`Ctrl+Shift+T` opens one more channel/tab" (`xtask
/// contract`).
#[test]
fn ctrl_shift_t_opens_a_second_channel_on_the_lowest_free_slot() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);
    assert_eq!(surface.channels().len(), 1);
    assert_eq!(surface.channels().current_channel(), 1);

    character(&mut surface, "t", CTRL_SHIFT);

    assert_eq!(
        surface.channels().len(),
        2,
        "Ctrl+Shift+T opened no channel"
    );
    assert_eq!(
        surface.channels().current_channel(),
        2,
        "the new channel takes the free slot above the one on the air, and the air with it"
    );
    let second = wait_for_prompt(&mut surface);
    assert_ne!(
        second, first,
        "the second channel is showing the first one's shell"
    );

    // The bank shows what the model holds: two lit keys and nine dark ones.
    let strips = surface.bank_strips();
    assert!(strips.rows[0].open && strips.rows[1].open);
    assert!(strips.rows[1].current && !strips.rows[0].current);
    assert!(strips.rows[2..].iter().all(|r| !r.open));
    assert_eq!(strips.rows[9].numeral, "10");

    // ...and it really is a separate pty, not a second view of one: turning
    // the knob back shows the first shell's own line again.
    named(&mut surface, NamedKey::PageUp, CTRL);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(wait_for_prompt(&mut surface), first);
}

/// `Ctrl+PgUp`/`Ctrl+PgDown` walk the open slots of the current page, and
/// wrap.
#[test]
fn ctrl_page_keys_cycle_the_open_channels_and_wrap() {
    let mut surface = surface();
    wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.channels().current_channel(), 3);

    named(&mut surface, NamedKey::PageDown, CTRL);
    assert_eq!(surface.channels().current_channel(), 1, "wraps forward");
    named(&mut surface, NamedKey::PageUp, CTRL);
    assert_eq!(surface.channels().current_channel(), 3, "and backwards");
    named(&mut surface, NamedKey::PageUp, CTRL);
    assert_eq!(surface.channels().current_channel(), 2);
}

/// `Ctrl+Shift+W` closes the channel on the air, and the nearest surviving
/// one takes it. The slot it left goes dark and stays dark: no renumbering.
#[test]
fn ctrl_shift_w_closes_the_channel_on_the_air() {
    let mut surface = surface();
    wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.channels().len(), 2);

    character(&mut surface, "w", CTRL_SHIFT);
    assert_eq!(surface.channels().len(), 1);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(surface.channels().slot_title(0, 2), None);
    assert!(!surface.bank_strips().rows[1].open);
}

/// The chord's semantics:
///
/// - The predicate decides. A digit no deeper key extends commits on the
///   spot: `Alt+7` on an eleven-key page selects slot 7 with no wait at all,
///   because no open slot's numeral begins with 7 and has more digits.
/// - The first digit locks the family; `"0"` is slot 10.
/// - The modifier's release commits whatever is waiting, which is the
///   ordinary end of a chord; the 900 ms timer is only the fallback for a
///   release that is never observed, and `app::chord`'s own tests drive it.
#[test]
fn an_alt_digit_chord_selects_the_channel_it_names() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    let second = wait_for_prompt(&mut surface);
    assert_eq!(surface.channels().current_channel(), 2);

    character(&mut surface, "1", CHORD);
    release_chord(&mut surface);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(wait_for_prompt(&mut surface), first);

    character(&mut surface, "2", CHORD);
    release_chord(&mut surface);
    assert_eq!(surface.channels().current_channel(), 2);
    assert_eq!(wait_for_prompt(&mut surface), second);

    // A chord naming a dark slot changes nothing: a select lands only on open
    // slots.
    character(&mut surface, "7", CHORD);
    release_chord(&mut surface);
    assert_eq!(surface.channels().current_channel(), 2);
}

/// The shifted family stores the session on screen onto the key it names,
/// and a store may land on a dark slot. The session on screen keeps its
/// screen; its slot number moves.
#[test]
fn an_alt_shift_digit_chord_stores_the_session_onto_the_slot() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    let second = wait_for_prompt(&mut surface);

    // Alt+Shift+7 onto a dark slot. Seven names no deeper key, so it commits
    // before the modifier is even released.
    character(&mut surface, "7", CHORD | ModifiersState::SHIFT);
    assert_eq!(surface.channels().current_channel(), 7);
    assert_eq!(wait_for_prompt(&mut surface), second);
    assert_eq!(
        surface.channels().slot_title(0, 2),
        None,
        "the slot it left"
    );

    // Alt+Shift+1 onto the occupied slot 1: keys 10 and 11 are engraved on this
    // page and a store may land on either, so "1" is a live prefix and the
    // chord waits for the release rather than committing on the digit.
    character(&mut surface, "1", CHORD | ModifiersState::SHIFT);
    assert_eq!(
        surface.channels().current_channel(),
        7,
        "the chord committed on the digit instead of waiting for the release"
    );
    release_chord(&mut surface);

    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(
        wait_for_prompt(&mut surface),
        second,
        "the session on screen kept its screen; only its slot number moved"
    );
    // The two swapped, so the displaced session is on the slot this one left.
    named(&mut surface, NamedKey::PageDown, CTRL);
    assert_eq!(surface.channels().current_channel(), 7);
    assert_eq!(
        wait_for_prompt(&mut surface),
        first,
        "the displaced session"
    );
}

/// `Ctrl+Shift+Left`/`Right` walk the session on screen along the bank, one
/// slot per press, swapping with an occupied neighbour and taking a dark one
/// outright. The ends of the bank are walls.
#[test]
fn ctrl_shift_arrows_walk_the_channel_along_the_bank() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    let second = wait_for_prompt(&mut surface);
    assert_eq!(surface.channels().current_channel(), 2);

    // Left onto slot 1, which is occupied: the two swap.
    named(&mut surface, NamedKey::ArrowLeft, CTRL_SHIFT);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(
        wait_for_prompt(&mut surface),
        second,
        "the session on screen kept its screen; only its slot number moved"
    );
    named(&mut surface, NamedKey::PageDown, CTRL);
    assert_eq!(surface.channels().current_channel(), 2);
    assert_eq!(wait_for_prompt(&mut surface), first, "the displaced session");

    // Right onto slot 3, which is dark: nothing is displaced.
    named(&mut surface, NamedKey::ArrowRight, CTRL_SHIFT);
    assert_eq!(surface.channels().current_channel(), 3);
    assert_eq!(surface.channels().slot_title(0, 2), None, "the slot it left");
}

/// Slot 1 has nothing to its left, and the press leaves the bank as it stands.
#[test]
fn ctrl_shift_left_on_the_first_slot_is_a_wall() {
    let mut surface = surface();
    let only = wait_for_prompt(&mut surface);
    assert_eq!(surface.channels().current_channel(), 1);

    named(&mut surface, NamedKey::ArrowLeft, CTRL_SHIFT);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(wait_for_prompt(&mut surface), only);
}

/// `"0"` commits as slot 10, and a two-digit chord is read as an ordinary
/// decimal.
#[test]
fn zero_names_the_tenth_key_and_two_digits_name_the_key_they_spell() {
    let mut surface = surface();
    wait_for_prompt(&mut surface);

    character(&mut surface, "0", CHORD | ModifiersState::SHIFT);
    assert_eq!(surface.channels().current_channel(), 10);

    // Alt+Shift+1 waits (10 and 11 are engraved), and Alt+Shift+1 again spells
    // 11, which nothing extends, so it lands without a release.
    character(&mut surface, "1", CHORD | ModifiersState::SHIFT);
    assert_eq!(surface.channels().current_channel(), 10);
    character(&mut surface, "1", CHORD | ModifiersState::SHIFT);
    assert_eq!(surface.channels().current_channel(), 11);
}

/// Wired to `crt::Degauss::trigger`: turning the knob makes the tube flinch.
/// A store does not, because the LED blink is the store's acknowledgement
/// and the tube holds steady.
#[test]
fn a_channel_switch_triggers_the_degauss_and_a_store_does_not() {
    let mut surface = surface();
    wait_for_prompt(&mut surface);
    assert_eq!(
        surface.degauss_state(Instant::now()),
        crt::DegaussState::IDLE,
        "bringing up the first channel is not a channel change"
    );

    // Opening a channel moves the air onto it, which is a channel change.
    // Sampled at a stamp taken before the keystroke: the transient is 200 ms
    // long and the keystroke's own cost is the scheduler's, so a stamp taken
    // after it can land past the end. The sampler saturates a time before
    // the trigger to its start, which is the peak.
    let before = Instant::now();
    character(&mut surface, "t", CTRL_SHIFT);
    let running = surface.degauss_state(before);
    assert!(running.is_active(), "the tube did not flinch: {running:?}");
    assert!(running.brightness > 1.0 && running.scale_y < 1.0);

    // It eases out over the mockup's 200 ms and then leaves nothing behind.
    let after = Instant::now() + crt::degauss::DURATION;
    assert_eq!(surface.degauss_state(after), crt::DegaussState::IDLE);

    // Re-selecting the channel already on the air changes nothing, so nothing
    // flinches.
    character(&mut surface, "2", CHORD);
    release_chord(&mut surface);
    assert_eq!(
        surface.degauss_state(Instant::now()),
        crt::DegaussState::IDLE
    );

    // ...and neither does a store.
    character(&mut surface, "4", CHORD | ModifiersState::SHIFT);
    assert_eq!(surface.channels().current_channel(), 4, "the store landed");
    assert_eq!(
        surface.degauss_state(Instant::now()),
        crt::DegaussState::IDLE,
        "a store is not a channel change"
    );

    // Choosing another channel is one again, and so is closing onto one.
    let before = Instant::now();
    character(&mut surface, "1", CHORD);
    release_chord(&mut surface);
    assert!(surface.degauss_state(before).is_active());
    let _ = surface.degauss_state(Instant::now() + crt::degauss::DURATION);
    let before = Instant::now();
    character(&mut surface, "w", CTRL_SHIFT);
    assert!(surface.degauss_state(before).is_active());
}

/// The press of a preset. A dark slot starts a session on it, an open one
/// comes to the screen. This is the call site for a click on a strip; the
/// hit test is the furniture's.
#[test]
fn a_press_on_a_strip_opens_a_dark_slot_and_selects_an_open_one() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);

    // The fifth key is dark; pressing it starts a shell there.
    let dark = surface.bank_strips().rows[4].clone();
    assert!(!dark.open);
    surface.press_strip(dark.channel);
    assert_eq!(surface.channels().current_channel(), 5);
    let fifth = wait_for_prompt(&mut surface);
    assert_ne!(fifth, first);
    assert!(surface.bank_strips().rows[4].open);

    // The first is open now, so pressing it selects rather than reopens.
    surface.press_strip(1);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(surface.channels().len(), 2);
    assert_eq!(wait_for_prompt(&mut surface), first);
}

/// The middle of the page's `row`th window, counted from 0, on the surface's
/// own cabinet, in physical pixels at the harness's scale of 1.
fn strip_key(surface: &TerminalSurface, row: usize) -> PhysicalPosition<f64> {
    let cabinet = surface.cabinet().expect("the harness gave it a cabinet");
    let shell = chassis::shell_metrics(&Config::default());
    let strip = chassis::furniture::strip_rect(cabinet.geometry(), &shell, row);
    PhysicalPosition::new(strip.x + strip.width / 2.0, strip.y + strip.height / 2.0)
}

fn drag(surface: &mut TerminalSurface, to: PhysicalPosition<f64>) {
    app::shell::Surface::cursor_moved(surface, to, ModifiersState::empty());
}

fn release(surface: &mut TerminalSurface, at: PhysicalPosition<f64>) {
    app::shell::Surface::mouse_released(surface, MouseButton::Left, at, ModifiersState::empty());
}

/// A window dragged onto a dark one shows the drop before it is made, and
/// the drop stores the channel there as `Alt+Shift+digit` would.
#[test]
fn a_window_dragged_onto_a_dark_one_moves_its_channel_there() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);
    let (one, five) = (strip_key(&surface, 0), strip_key(&surface, 4));

    press(&mut surface, one);
    drag(&mut surface, five);
    let strips = surface.bank_strips();
    assert!(strips.rows[4].ghost && strips.rows[4].open, "the drop is shown");
    assert!(!strips.rows[0].open, "and the window it leaves dark");
    assert_eq!(surface.channels().current_channel(), 1, "but not made");

    release(&mut surface, five);
    assert_eq!(surface.channels().current_channel(), 5);
    assert_eq!(surface.channels().slot_title(0, 1), None);
    assert!(surface.bank_strips().rows.iter().all(|row| !row.ghost));
    assert_eq!(wait_for_prompt(&mut surface), first);
}

/// A drop onto an open window swaps the two, and one let go of off the bank
/// leaves it as it stands.
#[test]
fn a_window_dropped_on_another_swaps_and_one_dropped_elsewhere_does_not() {
    let mut surface = surface();
    let first = wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    let second = wait_for_prompt(&mut surface);

    let (one, two) = (strip_key(&surface, 0), strip_key(&surface, 1));
    let glass = PhysicalPosition::new(f64::from(WINDOW_W) - 40.0, 100.0);
    press(&mut surface, two);
    drag(&mut surface, one);
    let strips = surface.bank_strips();
    assert!(strips.rows[0].ghost && strips.rows[1].ghost, "a swap changes both");
    drag(&mut surface, glass);
    assert!(surface.bank_strips().rows.iter().all(|row| !row.ghost));
    release(&mut surface, glass);
    assert_eq!(surface.channels().current_channel(), 2);

    press(&mut surface, two);
    drag(&mut surface, one);
    release(&mut surface, one);
    assert_eq!(surface.channels().current_channel(), 1);
    assert_eq!(wait_for_prompt(&mut surface), second);
    named(&mut surface, NamedKey::PageDown, CTRL);
    assert_eq!(surface.channels().current_channel(), 2);
    assert_eq!(wait_for_prompt(&mut surface), first, "the displaced session");
}

/// Stepping the pager over another of the current bank's own screenfuls views
/// a page without stealing the air, and the numerals restart at 1 on the new
/// page. Crossing into *another* bank's stretch is a band switch and does move
/// the air; that needs a second bank, so it is pinned where one stands
/// (`tmux_flow`), and the rule itself in `app::channels`.
#[test]
fn alt_page_keys_step_within_a_bank_without_moving_the_air() {
    // A three-key page, so a second page exists as soon as the next free slot
    // is past the first three: paging only unrolls a page far enough to
    // reach the slot a new channel would take.
    // 370 = the 240 the rows need plus the annunciator pager's 130.
    let mut surface = surface_of_height(370);
    assert_eq!(surface.bank_strips().rows.len(), 3);
    wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.bank_strips().page_count, 2);
    assert_eq!(
        surface.channels().banks().len(),
        1,
        "both pages are the one bank's, so the step stays inside it"
    );
    let before = surface.channels().current_channel();

    named(&mut surface, NamedKey::PageDown, CHORD);
    assert_eq!(surface.channels().current_channel(), before);
    let strips = surface.bank_strips();
    assert_eq!(strips.page_index, 1);
    assert_eq!(strips.rows[0].label, 1, "the keys are reused");
    assert_eq!(strips.rows[0].channel, 4, "the slot behind them is not");
    assert!(strips.rows.iter().all(|r| !r.current));
    assert_eq!(strips.current_row, None);

    // The page on view is the page Ctrl+Shift+T acts on. Both stretches are
    // the same machine's page here, so the new channel still takes the free
    // slot above the one on the air...
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.channels().current_channel(), 4);
    // ...and the bank turns back to the channel on the air, which on this
    // stretch is its first key.
    let strips = surface.bank_strips();
    assert_eq!(strips.page_index, 1);
    assert_eq!(strips.current_row, Some(0));
    assert!(strips.rows[0].current && strips.rows[0].open);
}

/// The drawn rocker is a key and not a picture: a press on NEXT walks the view
/// the way `Alt`+`PgDown` does, PREV walks it back, and a press past the last
/// page does nothing, which is what the dimmed rocker means.
///
/// The whole path, which is what this harness is for: the press arrives as a
/// window event, the hit test is the furniture's own key rectangles
/// (`chassis::Cabinet::pager_at`), and the step is `TerminalSurface::step_bank`
/// -- the same one the keys make, so the two gestures cannot part company.
#[test]
fn a_press_on_the_pagers_keys_walks_the_pages() {
    // The three-key page of the test above, so a second page exists.
    let mut surface = surface_of_height(370);
    wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.bank_strips().page_count, 2);
    let before = surface.channels().current_channel();
    let (prev, next) = pager_keys(&surface);

    press(&mut surface, next);
    assert_eq!(
        surface.bank_strips().page_index,
        1,
        "the NEXT key did not walk the view"
    );
    // Both pages are the one bank's, so this is the view-only step: the air
    // stays where it was.
    assert_eq!(surface.channels().current_channel(), before);

    // Past the last page the step's own clamp answers: the press is taken and
    // the view stands still.
    press(&mut surface, next);
    assert_eq!(surface.bank_strips().page_index, 1);

    press(&mut surface, prev);
    assert_eq!(surface.bank_strips().page_index, 0);
    press(&mut surface, prev);
    assert_eq!(surface.bank_strips().page_index, 0, "and clamps at page one");
    assert_eq!(surface.channels().current_channel(), before);
}

/// The window's title is the current channel's, and it follows the knob. A
/// local shell's title is its own.
#[test]
fn the_window_title_is_the_channel_on_the_airs() {
    let mut surface = surface();
    wait_for_prompt(&mut surface);
    // The scripted shell sets no title, so there is none to report and the
    // shell keeps its own identity.
    assert_eq!(title(&surface), None);

    // The scripted shell turns a line of input into an OSC 0 of its own, so
    // this is the channel on the air setting its own title.
    surface.write(b"first channel\n");
    let deadline = Instant::now() + Duration::from_secs(10);
    while title(&surface).as_deref() != Some("first channel") {
        surface.pump();
        assert!(
            Instant::now() < deadline,
            "the title never reached the window; screen:\n{}",
            surface.viewport_text().join("\n")
        );
        std::thread::sleep(Duration::from_millis(5));
    }

    // A second channel has a title of its own, which is to say none yet.
    character(&mut surface, "t", CTRL_SHIFT);
    wait_for_prompt(&mut surface);
    assert_eq!(title(&surface), None);
    // The bank reads the same titles the window does.
    assert_eq!(surface.bank_strips().rows[0].title, "first channel");

    // ...and turning back restores the first one's, which the model kept.
    named(&mut surface, NamedKey::PageUp, CTRL);
    surface.pump();
    assert_eq!(title(&surface).as_deref(), Some("first channel"));
}

/// The pager's step has to outlive the pump that follows it.
///
/// `channel_changed` runs on every pump, about every 8ms, and it used to call
/// `ensure_visible` unconditionally. That call recomputes the bank's
/// `page_index` from the channel on the air and knows nothing about a manual
/// step, so a page turned by hand was turned back within a frame: Alt+PageUp
/// and Alt+PageDown appeared to do nothing at all, and a chord begun on one
/// page could never be finished on another. The fix is to run
/// `ensure_visible` only when the air itself moves -- the current channel or
/// current page actually changes -- and at no other time.
#[test]
fn a_paged_bank_holds_its_page_across_the_pump_and_a_cross_page_chord_survives() {
    let mut surface = surface_of_height(370);
    assert_eq!(surface.bank_strips().rows.len(), 3);
    wait_for_prompt(&mut surface);
    character(&mut surface, "t", CTRL_SHIFT);
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.bank_strips().page_count, 2);

    let on_air = surface.channels().current_channel();
    named(&mut surface, NamedKey::PageDown, CHORD);
    assert_eq!(surface.bank_strips().page_index, 1, "the step itself");

    // The pump the user's next keystroke is always a few of behind. Nothing
    // has changed channel, so nothing may move the bank.
    for _ in 0..8 {
        surface.pump();
        assert_eq!(
            surface.bank_strips().page_index,
            1,
            "a pump put the bank back on the air's page"
        );
    }
    assert_eq!(
        surface.channels().current_channel(),
        on_air,
        "the pump moved the air as well"
    );

    // And back the other way, across the same pumps.
    named(&mut surface, NamedKey::PageUp, CHORD);
    assert_eq!(surface.bank_strips().page_index, 0);
    for _ in 0..8 {
        surface.pump();
        assert_eq!(surface.bank_strips().page_index, 0);
    }

    // A chord spanning a page turn: step the bank, pump between the halves
    // the way the event loop really does, then commit. The chord names a
    // channel on the page the bank was left on.
    named(&mut surface, NamedKey::PageDown, CHORD);
    for _ in 0..4 {
        surface.pump();
    }
    assert_eq!(
        surface.bank_strips().page_index,
        1,
        "the chord's page was taken back before it could be committed"
    );

    // A real switch still brings the bank to the channel it put on the air:
    // the guard gates the pump, not the switch.
    character(&mut surface, "t", CTRL_SHIFT);
    assert_eq!(surface.channels().current_channel(), 4);
    let strips = surface.bank_strips();
    assert_eq!(strips.page_index, 1);
    assert_eq!(strips.current_row, Some(0));
}
