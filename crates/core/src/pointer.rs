//! What a pointer event means: mark the screen, or forward it to the program?
//!
//! It is here rather than in the window layer because it is terminal
//! semantics: it depends on whether the program below has asked for mouse
//! reporting, and because keeping it as plain functions makes it testable
//! without a pointer.
//!
//! The rule underneath everything: a program that turned mouse reporting on
//! owns the pointer, and Shift is the user's override. One twist on top of
//! that: on frozen glass (a detached, scrolled-back view) Shift is held down
//! for the user, so dragging always marks a selection and nothing the mouse
//! does is written to the wire.

/// The modifier keys.
///
/// One type for the whole application, not just for this module: there is
/// one keyboard under the user's hands, and the keyboard encoder, the
/// mouse reporter and the routing table below all read the same state.
/// `crates/app` reaches it as `app::input::Modifiers`. The rules here
/// consult Shift, Control and Alt everywhere, and Meta on macOS, where
/// Control is the secondary click and the unbroken copy moves to Command;
/// the keytab's CSI modifier parameter counts Meta as well.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub struct Modifiers {
    pub shift: bool,
    pub control: bool,
    pub alt: bool,
    pub meta: bool,
}

impl Modifiers {
    pub const NONE: Modifiers = Modifiers {
        shift: false,
        control: false,
        alt: false,
        meta: false,
    };

    pub fn with_shift(mut self) -> Self {
        self.shift = true;
        self
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Button {
    Left,
    Middle,
    Right,
}

/// The state the pointer decisions read.
#[derive(Clone, Copy, Debug)]
pub struct PointerContext {
    /// The program below turned on mouse reporting (`terminalUsesMouse`).
    pub terminal_uses_mouse: bool,
    /// The channel is a tmux attachment's gateway, so the glass below is the
    /// picture its shell left when it entered control mode and the pty under
    /// it is the protocol's wire.
    pub frozen_glass: bool,
}

impl PointerContext {
    /// Whether dragging marks the screen rather than driving the program.
    /// True whenever the program is not listening, and forced true on
    /// frozen glass.
    pub fn marks_selection(&self) -> bool {
        self.frozen_glass || !self.terminal_uses_mouse
    }

    /// On frozen glass, Shift is held down for the user.
    pub fn marking(&self, modifiers: Modifiers) -> Modifiers {
        if self.frozen_glass {
            modifiers.with_shift()
        } else {
            modifiers
        }
    }

    /// The emulation marks unless the program asked for the mouse.
    fn mouse_marks(&self) -> bool {
        !self.terminal_uses_mouse
    }
}

/// What should happen to a pointer event.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum PointerAction {
    /// Begin or extend a selection at this cell.
    Mark,
    /// Report the event to the program below.
    ReportToProgram,
    /// Paste the primary selection (middle click while marking).
    PastePrimary { bracketed: bool },
    /// A link was pressed: anchor a selection as usual *and* activate the
    /// link. `mousePressEvent` does both, in that order: the anchor is set
    /// first and the hotspot check follows it, so a press on a link that turns
    /// into a drag selects the link text instead of following it.
    MarkAndActivateHotSpot,
    /// Open the companion settings application. A right press on plain glass
    /// is the only way in from the window, so the app runs the settings
    /// binary that ships beside it rather than doing anything to the screen.
    OpenSettings,
    /// Swallow it. Middle click on frozen glass: a paste is inert on an
    /// anchor, so the event never reaches the core.
    Ignore,
}

/// The press routing table.
///
/// `over_hot_spot` reports whether a link lies under the cell and the press
/// carries the chord that opens one ([`opens_link`]); the caller folds both
/// into it, so the table reads no modifier for links. A left press that
/// marks and lands on one anchors the selection and activates the link.
///
/// The three buttons divide as: left marks or reports, middle pastes the
/// primary selection or reports, and right opens the settings application or
/// reports. Whichever half of each pair applies is decided by the one
/// `marks` computation below, so a program that owns the mouse still
/// receives every button.
pub fn on_press(
    ctx: PointerContext,
    button: Button,
    modifiers: Modifiers,
    over_hot_spot: bool,
) -> PointerAction {
    // Two cases are intercepted before the core sees anything.
    if ctx.frozen_glass && button == Button::Middle {
        return PointerAction::Ignore;
    }
    let marking = ctx.marking(modifiers);
    // A right press with Shift held is deliberately inert: Shift is the
    // override that takes the pointer back from the program, and a press that
    // was made to reach the emulation should not open a window over the glass
    // in the middle of a drag the user is holding Shift for. The unmodified
    // right press falls through to the table, where marking glass opens the
    // settings application. `modifiers` and not `marking` is read here on
    // purpose: frozen glass holds Shift down for the user, and that borrowed
    // Shift is about selection, not about suppressing the menu.
    if modifiers.shift && button == Button::Right {
        return PointerAction::Ignore;
    }

    let marks = ctx.mouse_marks() || marking.shift;
    match button {
        Button::Left => {
            if marks {
                if over_hot_spot {
                    PointerAction::MarkAndActivateHotSpot
                } else {
                    PointerAction::Mark
                }
            } else {
                PointerAction::ReportToProgram
            }
        }
        Button::Middle => {
            if marks {
                PointerAction::PastePrimary {
                    bracketed: marking.control,
                }
            } else {
                PointerAction::ReportToProgram
            }
        }
        Button::Right => {
            if marks {
                PointerAction::OpenSettings
            } else {
                PointerAction::ReportToProgram
            }
        }
    }
}

/// Whether Control with the left button is this platform's secondary click.
///
/// macOS delivers a Control-click as a left press carrying the Control flag,
/// and converts it to a secondary click only for a view that hands AppKit an
/// `NSMenu`. A drawn grid never does, so every Mac terminal makes the
/// conversion itself and this one is no exception. Alt is excluded because
/// Ctrl+Alt is the rectangle drag, which keeps its meaning there.
pub fn control_click_is_secondary(modifiers: Modifiers) -> bool {
    cfg!(target_os = "macos") && modifiers.control && !modifiers.alt
}

/// Control without Alt, or Command on macOS, where Control is the secondary
/// click ([`control_click_is_secondary`]) and cannot also mean this. One
/// chord with two readings, decided by what the hand does next: held through
/// a drag it copies a wrapped run unbroken ([`preserve_line_breaks`]), held
/// through a click on a link it opens the link ([`opens_link`]).
fn ctrl_or_cmd(modifiers: Modifiers) -> bool {
    if cfg!(target_os = "macos") {
        modifiers.meta
    } else {
        modifiers.control && !modifiers.alt
    }
}

/// `_preserveLineBreaks`, set from the press modifiers: the drag that copies
/// a wrapped selection as one unbroken run.
pub fn preserve_line_breaks(modifiers: Modifiers) -> bool {
    !ctrl_or_cmd(modifiers)
}

/// Whether a press on a link opens it: the chord [`preserve_line_breaks`]
/// reads, taken at the click rather than the drag, so a click made to focus
/// the window or to anchor a selection follows no link.
pub fn opens_link(modifiers: Modifiers) -> bool {
    ctrl_or_cmd(modifiers)
}

/// `_columnSelectionMode`: Ctrl+Alt makes the drag rectangular.
pub fn column_selection_mode(modifiers: Modifiers) -> bool {
    modifiers.control && modifiers.alt
}
