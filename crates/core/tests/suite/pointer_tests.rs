//! The pointer routing table: whether a press marks the screen, reaches the
//! program, pastes, or opens the settings application, and the modifier
//! chords a press reads on the way.

use term::pointer::{
    column_selection_mode, control_click_is_secondary, on_press, opens_link, preserve_line_breaks,
    Button, Modifiers, PointerAction, PointerContext,
};

/// The chord that copies a wrapped command as one unbroken run: Command on
/// macOS, where Control is the secondary click, and Control everywhere else.
fn join_lines() -> Modifiers {
    if cfg!(target_os = "macos") {
        Modifiers {
            meta: true,
            ..Modifiers::NONE
        }
    } else {
        Modifiers {
            control: true,
            ..Modifiers::NONE
        }
    }
}

#[test]
fn a_program_that_asked_for_the_mouse_gets_it() {
    let ctx = PointerContext {
        terminal_uses_mouse: true,
        frozen_glass: false,
    };
    assert!(!ctx.marks_selection());
    assert_eq!(
        on_press(ctx, Button::Left, Modifiers::NONE, false),
        PointerAction::ReportToProgram
    );
    // Shift is the user's override.
    assert_eq!(
        on_press(ctx, Button::Left, Modifiers::NONE.with_shift(), false),
        PointerAction::Mark
    );
}

#[test]
fn frozen_glass_holds_shift_down_for_the_user() {
    let ctx = PointerContext {
        terminal_uses_mouse: true,
        frozen_glass: true,
    };
    assert!(ctx.marks_selection(), "an anchor always marks");
    assert!(ctx.marking(Modifiers::NONE).shift);
    assert_eq!(
        on_press(ctx, Button::Left, Modifiers::NONE, false),
        PointerAction::Mark
    );
    assert_eq!(
        on_press(ctx, Button::Middle, Modifiers::NONE, false),
        PointerAction::Ignore,
        "a paste is inert on an anchor, so the event never reaches the core"
    );
    assert_eq!(
        on_press(ctx, Button::Right, Modifiers::NONE, false),
        PointerAction::OpenSettings,
        "an anchor is still glass the user is looking at, so the right press \
         reaches the settings application rather than the program"
    );
}

#[test]
fn a_right_press_on_plain_glass_opens_the_settings_application() {
    let ctx = PointerContext {
        terminal_uses_mouse: false,
        frozen_glass: false,
    };
    assert_eq!(
        on_press(ctx, Button::Right, Modifiers::NONE, false),
        PointerAction::OpenSettings
    );
    assert_eq!(
        on_press(ctx, Button::Right, Modifiers::NONE.with_shift(), false),
        PointerAction::Ignore,
        "Shift is the chord a marking drag is held with, and it keeps the \
         right press inert so no window opens over the drag"
    );
}

#[test]
fn a_program_tracking_the_mouse_still_gets_the_right_button() {
    let ctx = PointerContext {
        terminal_uses_mouse: true,
        frozen_glass: false,
    };
    assert_eq!(
        on_press(ctx, Button::Right, Modifiers::NONE, false),
        PointerAction::ReportToProgram,
        "vim asked for the mouse, so its own menu wins over the appliance's"
    );
}

#[test]
fn a_left_press_on_a_link_anchors_and_activates() {
    let ctx = PointerContext {
        terminal_uses_mouse: false,
        frozen_glass: false,
    };
    assert_eq!(
        on_press(ctx, Button::Left, Modifiers::NONE, true),
        PointerAction::MarkAndActivateHotSpot
    );
}

#[test]
fn the_copy_modifiers_match_the_recorded_chords() {
    assert!(preserve_line_breaks(Modifiers::NONE));
    assert!(!preserve_line_breaks(join_lines()));
    assert!(
        preserve_line_breaks(Modifiers {
            control: true,
            alt: true,
            ..Modifiers::NONE
        }),
        "Ctrl+Alt is the block-selection chord, not the join-lines one"
    );
    assert!(column_selection_mode(Modifiers {
        control: true,
        alt: true,
        ..Modifiers::NONE
    }));
}

/// Control with the left button is macOS's secondary click and nothing
/// anywhere else, so the two platforms disagree about this press on purpose.
#[test]
fn control_with_the_left_button_is_the_secondary_click_on_macos() {
    let control = Modifiers {
        control: true,
        ..Modifiers::NONE
    };
    assert_eq!(
        control_click_is_secondary(control),
        cfg!(target_os = "macos")
    );
    assert!(
        !control_click_is_secondary(Modifiers {
            alt: true,
            ..control
        }),
        "Ctrl+Alt drags a rectangle on every platform, macOS included"
    );
    assert!(!control_click_is_secondary(Modifiers::NONE));
}

#[test]
fn the_chord_that_joins_lines_on_a_drag_opens_a_link_on_a_click() {
    assert!(opens_link(join_lines()));
    assert!(!opens_link(Modifiers::NONE));
    assert!(!opens_link(Modifiers::NONE.with_shift()));
    for modifiers in [Modifiers::NONE, join_lines(), Modifiers::NONE.with_shift()] {
        assert_eq!(preserve_line_breaks(modifiers), !opens_link(modifiers));
    }
    // Ctrl+Alt is the rectangle drag, and opens nothing on any platform.
    let rectangle = Modifiers {
        control: true,
        alt: true,
        ..Modifiers::NONE
    };
    assert!(!opens_link(rectangle));
}
