//! The pointer path, from a window pixel to what the event means.
//!
//! The order is the module doc's, and every step of it is here:
//!
//! 1. the position is measured from the well's left edge and pushed back
//!    through [`term::distortion`], because the click landed on bent glass;
//! 2. the corrected point divides by the cell into an *absolute* grid cell,
//!    which is the coordinate [`term::selection`] works in;
//! 3. [`term::pointer`] says what the event means, because that depends on
//!    terminal state rather than on the window;
//! 4. only then does anything happen -- a selection, a [`crate::mouse`]
//!    report down the wire, or a [`crate::clipboard`] paste.
//!
//! The seam gets first refusal on every press, drag and hover before any of
//! it ([`super::seam`]).
//!
//! Fields touched: `pointer_cell`, the cell the pointer was last over;
//! `dragging`, `secondary_press` and `last_click`, the state one gesture
//! carries; `wheel_pixels`, a trackpad's travel not yet worth a line;
//! `scroll`, which the wheel moves; and `viewport` and `settings`, which
//! together are the whole of the inverse-distortion transform.

use std::time::{Duration, Instant};

use term::distortion::{self, correct_distortion, DistortionParams};
use term::pointer::{self, on_press, opens_link, PointerAction, PointerContext};
use term::rio_vt::crosswords::pos::Side;
use term::rio_vt::crosswords::Mode;
use term::selection;
use winit::dpi::PhysicalPosition;
use winit::event::{MouseButton, MouseScrollDelta};
use winit::keyboard::ModifiersState;
use winit::window::CursorIcon;

use crate::input::Modifiers;
use crate::settings;
use crate::{clipboard, mouse};

use super::keys::modifiers_from;
use super::TerminalSurface;

/// Two clicks inside this window on the same cell are a double click.
/// winit does not expose the platform's own double-click interval, so this
/// is the X11/GTK desktop default an application would otherwise be handed.
const DOUBLE_CLICK_INTERVAL: Duration = Duration::from_millis(400);

impl TerminalSurface {
    /// Someone is here. Stamped by every path a hand reaches the glass
    /// through, and it drops the standing effects deadline: a throttled one
    /// can be a fifth of a second out, which is long enough to watch the
    /// picture catch up rather than simply find it running.
    pub(super) fn attended(&mut self) {
        self.last_input = Instant::now();
        self.next_effects_frame = None;
    }

    /// The captured state a distortion computation needs, as this window
    /// supplies it: the well's own pixel size, the grid's size within it,
    /// and whatever [`TerminalSurface::set_settings`] last attached. No
    /// handle means both derived terms are the neutral value, zero frame
    /// inset and zero curvature, leaving the map as flat glass in a bare
    /// cabinet.
    ///
    /// The margin is not among them. It reaches the pointer the same way it
    /// reaches the picture, by shrinking `term_size` and so moving where the
    /// renderer centres the grid in the well (`term::distortion`).
    pub(super) fn distortion_params(&self) -> DistortionParams {
        let (grid_width, grid_height) = self.viewport.term_size().pixel_size();
        let width = self.viewport.width as f64;
        let height = self.viewport.height as f64;
        let normalized_screen_scale = distortion::normalized_screen_scale(width, height);

        let (frame_size, screen_curvature) = match self.settings.as_ref() {
            Some(handle) => {
                let config = handle.current();
                (
                    settings::unscaled_frame_size(&config) * normalized_screen_scale,
                    config.screen.screen_curvature,
                )
            }
            None => (0.0, 0.0),
        };

        DistortionParams {
            width,
            height,
            frame_size,
            screen_curvature,
            screen_curvature_size: distortion::SCREEN_CURVATURE_SIZE,
            normalized_screen_scale,
            total_width: f64::from(grid_width),
            total_height: f64::from(grid_height),
        }
    }

    /// A window pixel becomes an absolute grid cell: measured from the well's
    /// left edge, through the inverse distortion, divided by the cell, offset
    /// by where the view sits.
    ///
    /// The bank column is subtracted first because every term after it is the
    /// glass's own: the distortion is the curvature of a tube that starts where
    /// the casting ends, and a click 200 px into a window with a 247 px bank is
    /// not on the glass at all.
    /// The cell under the pointer, and which half of it the pointer is on.
    ///
    /// The side is what the rio selection model anchors on: it points at the
    /// seam between two cells, so a drag begun on the right half of a
    /// character starts after that character. The Konsole model ignores it.
    fn cell_side_at(&self, position: PhysicalPosition<f64>) -> ((usize, usize), Side) {
        let x = position.x - f64::from(self.bank_physical());
        let point = correct_distortion(x, position.y, &self.distortion_params());
        let size = self.viewport.term_size();
        let (column, side) = size.column_side_at(point.x);
        // The picture is drawn shifted up by the position's fraction of a
        // row, so a point on the glass is that much further down the grid.
        let y = point.y + f64::from(self.shift_physical());
        let row = (y / f64::from(size.cell_height)).floor();
        // The spare row under the last one is on the glass while the picture
        // is shifted, and a point on it is on that line.
        let last = if self.shift_physical() > 0 {
            size.rows()
        } else {
            size.rows().saturating_sub(1)
        };
        let row = row.clamp(0.0, last as f64) as usize;
        ((column, self.top_line() + row), side)
    }

    /// How far up the picture is drawn from the grid's rectangle, in
    /// physical pixels: the scrollback position's fraction of a row, rounded
    /// to whole raster pixels exactly as the renderer rounds it, so a point
    /// on the glass maps to the cell drawn under it.
    pub(super) fn shift_physical(&self) -> i32 {
        let scale = self
            .glass
            .as_ref()
            .map_or(1, |glass| glass.resolved.integer_scale)
            .max(1) as f32;
        let cell_h = f32::from(self.viewport.term_size().cell_height) / scale;
        (self.scroll.shift() * cell_h).round() as i32 * scale as i32
    }

    /// The absolute index of the line at the top of the view.
    pub(super) fn top_line(&self) -> usize {
        match self.channels.session() {
            Some(session) => session
                .term()
                .history_size()
                .saturating_sub(self.scroll.offset()),
            None => 0,
        }
    }

    pub(super) fn selection_window(&self) -> selection::Window {
        let size = self.viewport.term_size();
        selection::Window {
            top_line: self.top_line(),
            lines: size.rows(),
            columns: size.cols(),
        }
    }

    pub(super) fn mode_contains(&self, mode: Mode) -> bool {
        self.channels
            .session()
            .is_some_and(|s| s.term().mode().contains(mode))
    }

    /// Whether the program below asked for the pointer.
    ///
    /// Public because it is also what the pointer *shape* is set from: an
    /// I-beam shows exactly while the program is not listening, which is
    /// the renderer's to apply.
    pub fn terminal_uses_mouse(&self) -> bool {
        self.channels
            .session()
            .is_some_and(|s| s.term().mode().intersects(Mode::MOUSE_MODE))
    }

    fn pointer_context(&self) -> PointerContext {
        PointerContext {
            terminal_uses_mouse: self.terminal_uses_mouse(),
            // `frozen_glass` is one thing and one thing only: this channel is
            // the gateway. An earlier revision had no channel model
            // to read it from and stood a scrolled-back view in for it
            // instead; the model exists now, and that substitute is retired.
            //
            // What it buys: over the gateway a drag marks the screen instead
            // of reporting to the program, the pointer keeps its I-beam, and
            // the middle button's paste goes nowhere -- the glass is still a
            // picture to read and copy from; it is no longer a surface to
            // type at.
            frozen_glass: self.is_gateway_on_air(),
        }
    }

    /// The link under `cell` as the glass shows it now, or `None` off any.
    fn link_under(&mut self, cell: (usize, usize)) -> Option<(selection::MarkedRange, String)> {
        let top_line = self.top_line();
        let session = self.channels.session()?;
        term::links::link_at(&mut self.links, session.term(), top_line, cell)
    }

    /// What the pointer is over, read afresh: a link while the chord that
    /// opens one is held and the pointer is in the window, nothing otherwise.
    pub(super) fn refresh_hover(&mut self, mods: Modifiers) {
        let hover = if opens_link(mods) && self.pointer_present {
            self.link_under(self.pointer_cell)
        } else {
            None
        };
        self.set_hover(hover);
    }

    /// The link under the pointer changed. The renderer's underline moves
    /// with the next frame, asked for here since a motion asks for none of
    /// its own, and the pointer's shape changes now.
    pub(super) fn set_hover(&mut self, hover: Option<(selection::MarkedRange, String)>) {
        if hover == self.hover {
            return;
        }
        self.hover = hover;
        self.set_pointer_shape();
        if let Some(window) = self.window.as_ref() {
            window.request_redraw();
        }
    }

    /// The one writer of the pointer's shape, in claim order: the seam's
    /// resize arrows, the hand over a link, the platform's default. The
    /// window is told only when the shape changes.
    pub(super) fn set_pointer_shape(&mut self) {
        let shape = if self.seam_cursor {
            CursorIcon::EwResize
        } else if self.hover.is_some() {
            CursorIcon::Pointer
        } else {
            CursorIcon::Default
        };
        if shape == self.pointer_shape {
            return;
        }
        self.pointer_shape = shape;
        if let Some(window) = self.window.as_ref() {
            window.set_cursor(shape);
        }
    }

    /// The pointer left the window: nothing is under it, and a modifier
    /// pressed over another window raises no underline here.
    pub(super) fn on_cursor_left(&mut self) {
        self.pointer_present = false;
        self.set_hover(None);
    }

    /// Report a button or wheel event to the program, in whichever of the
    /// two wire formats it asked for.
    fn report_mouse(
        &mut self,
        button: mouse::MouseButton,
        cell: (usize, usize),
        modifiers: Modifiers,
        pressed: bool,
    ) {
        let column = cell.0.min(u16::MAX as usize) as u16;
        let row = cell
            .1
            .saturating_sub(self.top_line())
            .min(u16::MAX as usize) as u16;
        let bytes = if self.mode_contains(Mode::SGR_MOUSE) {
            mouse::encode_sgr(button, column, row, modifiers, pressed)
        } else {
            // X10 has no per-button release code: every release is a 3.
            let button = if pressed {
                button
            } else {
                mouse::MouseButton::Release
            };
            mouse::encode_x10(button, column, row, modifiers)
        };
        self.write(&bytes);
    }

    pub(super) fn on_mouse_pressed(
        &mut self,
        button: MouseButton,
        position: PhysicalPosition<f64>,
        modifiers: ModifiersState,
    ) {
        self.attended();
        // macOS's secondary click, before anything downstream reads the
        // button: Control with the left button is a right press for the whole
        // window, the seam and the bank's strips included, exactly as the
        // right button itself is. Everywhere else this is the button that
        // arrived.
        let mods = modifiers_from(modifiers);
        let button = if button == MouseButton::Left && pointer::control_click_is_secondary(mods) {
            self.secondary_press = true;
            MouseButton::Right
        } else {
            button
        };

        // The seam first: a press it took is the cabinet's and must not also
        // mark the screen (`chassis::cabinet`'s wiring sketch). The bank's own
        // windows come next, for the same reason and in the same order: the
        // grab strip lies over the bank's right edge, so the seam is the
        // narrower claim and gets first refusal. The pager's keys at the
        // bank's foot are the third claim and the last before the grid: they
        // stand below the rows, so where a shell rides its rail up into the
        // air above its own item, the row that owns that air keeps it.
        if self.seam_pressed(button, position)
            || self.strip_pressed(button, position)
            || self.pager_pressed(button, position)
        {
            return;
        }
        let Some(button) = pointer_button(button) else {
            return;
        };
        let (cell, side) = self.cell_side_at(position);
        self.pointer_cell = cell;
        self.pointer_present = true;

        // The link under the press, asked only when the chord that opens
        // one is held: a plain press marks, whatever it lands on.
        let link = if opens_link(mods) {
            self.link_under(cell)
        } else {
            None
        };
        match on_press(self.pointer_context(), button, mods, link.is_some()) {
            PointerAction::Mark | PointerAction::MarkAndActivateHotSpot => {
                self.retarget_selection(self.selection_kind());
                let now = Instant::now();
                let count = match self.last_click {
                    Some((at, at_cell, count))
                        if at_cell == cell
                            && count < 3
                            && now.duration_since(at) < DOUBLE_CLICK_INTERVAL =>
                    {
                        count + 1
                    }
                    _ => 1,
                };
                match count {
                    // The word under the pointer, and word mode stays on
                    // so a drag afterwards extends by whole words.
                    2 => {
                        let text = self.select_word_at(cell, side);
                        self.copy_on_select(text);
                    }
                    // The whole logical line, wrapping and all.
                    3 => {
                        let text = self.select_line_at(cell, side);
                        self.copy_on_select(text);
                    }
                    _ => self.begin_selection(cell, side, mods),
                }
                self.dragging = true;
                self.last_click = Some((now, cell, count));
                // A link opens when the button comes up on the cell it went
                // down on: a press that travels is a drag, and selects the
                // link's text instead of following it. The first press of a
                // run is the one that opens; the double and the triple that
                // may follow it select the word and the line.
                self.link_press = link.filter(|_| count == 1).map(|(_, url)| (cell, url));
            }
            PointerAction::ReportToProgram => {
                self.report_mouse(report_button(button), cell, mods, true)
            }
            PointerAction::PastePrimary { bracketed } => {
                self.paste_from(clipboard::Target::Primary, bracketed)
            }
            PointerAction::OpenSettings => self.settings_app.open(),
            PointerAction::Ignore => {}
        }
    }

    pub(super) fn on_mouse_released(
        &mut self,
        button: MouseButton,
        position: PhysicalPosition<f64>,
        modifiers: ModifiersState,
    ) {
        // The other half of the press's substitution, and the reason it is
        // remembered rather than recomputed: the press went out as a right
        // one, so this release does too, whether or not Control is still
        // down.
        let button = if button == MouseButton::Left && std::mem::take(&mut self.secondary_press) {
            MouseButton::Right
        } else {
            button
        };

        if self.seam_press && button == MouseButton::Left {
            self.seam_released();
            return;
        }
        let Some(button) = pointer_button(button) else {
            return;
        };
        let mods = modifiers_from(modifiers);
        let (cell, side) = self.cell_side_at(position);
        self.pointer_cell = cell;

        // Before the drag's own release below, which returns: the press
        // that put a link here also set `dragging`.
        if button == pointer::Button::Left {
            if let Some((pressed_at, url)) = self.link_press.take() {
                if cell == pressed_at {
                    self.opener.open(&url);
                }
            }
        }

        if button == pointer::Button::Left && self.dragging {
            self.dragging = false;
            // The selection goes to the primary selection here, not on a
            // keystroke: it is what a middle click pastes the moment the
            // button comes up.
            let text = self.end_selection(side);
            self.copy_on_select(text);
            return;
        }
        if self.terminal_uses_mouse() && !mods.shift {
            self.report_mouse(report_button(button), cell, mods, false);
        }
    }

    pub(super) fn on_cursor_moved(
        &mut self,
        position: PhysicalPosition<f64>,
        modifiers: ModifiersState,
    ) {
        // Every motion goes to the seam, drag or no drag: it tracks the pointer
        // for the cursor's shape and moves nothing until it is held.
        if self.seam_moved(position) {
            return;
        }
        let (cell, side) = self.cell_side_at(position);
        // Every pointer path is expressed in cells, so a move within one
        // cell has nothing to say, including to a program tracking
        // motion, which is addressed in cells too.
        if cell == self.pointer_cell {
            return;
        }
        self.pointer_cell = cell;
        self.pointer_present = true;
        let mods = modifiers_from(modifiers);
        self.refresh_hover(mods);

        if self.dragging {
            self.drag_selection_to(cell, side);
            return;
        }
        if !mods.shift && self.mode_contains(Mode::MOUSE_MOTION) {
            self.report_mouse(mouse::MouseButton::Release, cell, mods, true);
        }
    }

    pub(super) fn on_mouse_wheel(&mut self, delta: MouseScrollDelta, modifiers: ModifiersState) {
        self.attended();
        // The view is about to move under the pointer, so the link it was
        // over is no longer under it; the next motion asks again.
        self.set_hover(None);
        let mods = modifiers_from(modifiers);
        let cell_height = f64::from(self.viewport.term_size().cell_height).max(1.0);

        // Shift is the user's override: it scrolls the view even while a
        // program is tracking the mouse. A program hears whole notches, so
        // a trackpad's pixels are banked until they add up to a line; the
        // view itself takes them as they come.
        if self.terminal_uses_mouse() && !mods.shift {
            let notches = match delta {
                MouseScrollDelta::LineDelta(_, lines) => f64::from(lines),
                MouseScrollDelta::PixelDelta(pixels) => {
                    self.wheel_pixels += pixels.y;
                    let lines = (self.wheel_pixels / cell_height).trunc();
                    self.wheel_pixels -= lines * cell_height;
                    lines
                }
            };
            let notches = notches.trunc() as i32;
            if notches == 0 {
                return;
            }
            let button = if notches > 0 {
                mouse::MouseButton::WheelUp
            } else {
                mouse::MouseButton::WheelDown
            };
            let cell = self.pointer_cell;
            for _ in 0..notches.abs() {
                self.report_mouse(button, cell, mods, true);
            }
            return;
        }

        // Positive is up and into history, the sign `ScrollPosition` takes.
        // A notch sets the view gliding; a trackpad's pixels move it under
        // the fingers (`term::viewport`).
        let Some(session) = self.channels.session_mut() else {
            return;
        };
        match delta {
            MouseScrollDelta::LineDelta(_, lines) => {
                let notches = lines.trunc() as i32;
                if notches != 0 {
                    self.scroll
                        .scroll_wheel(session.term_mut(), notches, Instant::now());
                }
            }
            MouseScrollDelta::PixelDelta(pixels) => {
                self.scroll
                    .scroll_pixels(session.term_mut(), pixels.y as f32, cell_height as f32);
            }
        }
    }

    pub(super) fn on_focus_changed(&mut self, focused: bool) {
        self.focused = focused;
        if focused {
            // Back at the glass: the throttled deadline standing from a
            // moment ago would otherwise hold the picture for a fifth of a
            // second after the window is live again.
            self.attended();
        }
        if !focused {
            // A window that loses focus never sees the button come up,
            // so a drag left running would resume on the next move. The seam's
            // release also writes what its drag landed, which is why it is the
            // same call the button makes and not a flag cleared here.
            self.dragging = false;
            self.seam_released();
            // And the press that was a secondary click is over with it: a
            // flag left standing would turn the next window's first ordinary
            // press into a right release.
            self.secondary_press = false;
            // The link a press landed on goes with it, and so does the
            // underline: the release that would have opened it goes to
            // another window.
            self.link_press = None;
            self.set_hover(None);
            // A window that goes away under a half-typed chord commits it
            // rather than holding the digits for whenever it comes back.
            self.chord_modifier = false;
            self.commit_chord();
        }
        if self.mode_contains(Mode::FOCUS_IN_OUT) {
            let bytes: &[u8] = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.write(bytes);
        }
    }
}

/// Which of the three buttons the routing table knows about this is.
/// Back and forward are not bound to anything.
fn pointer_button(button: MouseButton) -> Option<pointer::Button> {
    match button {
        MouseButton::Left => Some(pointer::Button::Left),
        MouseButton::Middle => Some(pointer::Button::Middle),
        MouseButton::Right => Some(pointer::Button::Right),
        _ => None,
    }
}

fn report_button(button: pointer::Button) -> mouse::MouseButton {
    match button {
        pointer::Button::Left => mouse::MouseButton::Left,
        pointer::Button::Middle => mouse::MouseButton::Middle,
        pointer::Button::Right => mouse::MouseButton::Right,
    }
}
