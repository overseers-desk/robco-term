//! The window shell: winit event loop, multi-window bookkeeping, and the
//! window-level behavior the CLI/window contract measures.
//!
//! What this owns is the *shell*: how many windows there are, what
//! identity they present to the window manager, what geometry hints they
//! carry, what their titles say, and how a new-window request from another
//! process becomes a window. What it deliberately does not own is what is
//! drawn inside them: the wgpu surface and the rio-vt session, the glyph
//! grid, and input handling all belong elsewhere. Each window therefore
//! holds a [`Surface`] trait object rather than a renderer, so the
//! renderer drops in at one seam
//! ([`ShellConfig::surface_factory`]) without either side rewriting the
//! other.
//!
//! The window contract, in full:
//!
//! - initial size 1024x768; minimum size `bankMinimum + crtMinimumWidth` by
//!   240 (see [`crate::geometry`]);
//! - `--fullscreen` and F11 toggle fullscreen, with or without modifiers,
//!   so Konsole's Ctrl+Shift+F11 is the same key;
//! - Ctrl+Shift+N opens a window, Ctrl+Shift+Q closes one;
//! - the last window closing ends the process;
//! - the title is the current channel's title, falling back to the
//!   application's own name until a session supplies one;
//! - the size overlay, in [`crate::overlay`].

use std::collections::HashMap;
use std::sync::Arc;
use std::time::Instant;

use winit::application::ApplicationHandler;
use winit::dpi::{PhysicalPosition, PhysicalSize, Size};
use winit::event::{ElementState, Ime, KeyEvent, MouseButton, MouseScrollDelta, WindowEvent};
use winit::event_loop::{ActiveEventLoop, ControlFlow, EventLoop, EventLoopProxy};
use winit::keyboard::{KeyCode, ModifiersState, PhysicalKey};
use winit::window::{Fullscreen, Window, WindowAttributes, WindowId};

use crate::geometry;
use crate::instance::NewWindow;
use crate::overlay::{GridSize, SizeOverlay};
use crate::workarea;

/// How often the loop wakes while the size badge is visible. The fade
/// animates at 60 Hz, the same assumption `crate::window::EFFECTS_BASE_FRAME`
/// already makes, and it only applies for the 1.2 s the badge is up.
const OVERLAY_FRAME: std::time::Duration = std::time::Duration::from_millis(16);

/// Whatever the renderer hangs inside a window: the wgpu surface and
/// the terminal session behind it.
///
/// The shell forwards the raw window events and decides none of them.
/// It takes its own keyboard shortcuts (F11, Ctrl+Shift+N/Q) first
/// because those are window management, and it tracks the pointer
/// position because winit's button events do not carry one; what any of
/// it *means* (a keystroke's bytes, whether a click marks the screen or
/// reaches the program) is the surface's, since it depends on terminal
/// state the shell cannot see. Every method below has a no-op default,
/// so [`EmptySurface`] and the contract harness ignore the lot.
pub trait Surface {
    /// The window was resized to this physical pixel size.
    fn resized(&mut self, _size: PhysicalSize<u32>) {}
    /// A key went down. The shell has already taken its own shortcuts
    /// (F11, Ctrl+Shift+N/Q); everything left is the surface's.
    fn key_pressed(&mut self, _event: &KeyEvent, _modifiers: ModifiersState) {}
    /// The modifier set changed, in either direction.
    ///
    /// The shell tracks this for its own shortcuts anyway. It is forwarded
    /// because a *release* is a real event for a surface holding a chord: the
    /// channel chord commits when the modifier that was holding it comes up,
    /// and winit reports no key-release for a modifier that the
    /// surface could take instead.
    fn modifiers_changed(&mut self, _modifiers: ModifiersState) {}
    /// The input method said something: composition started or ended, the
    /// pre-edit changed, or a string was committed.
    ///
    /// A separate path from [`Surface::key_pressed`] because it is a separate
    /// event source. While an IME is composing, the keystrokes belong to the
    /// IME and not to the terminal -- winit's X11 backend runs the key through
    /// `XFilterEvent` and does not deliver the ones the IME consumed -- and
    /// what the user meant arrives later, whole, as `Ime::Commit`.
    fn ime(&mut self, _event: &Ime) {}
    /// A mouse button went down at this physical-pixel position.
    ///
    /// winit's `MouseInput` carries no position of its own, so the shell
    /// tracks the last `CursorMoved` per window and passes it here. That
    /// keeps every surface from having to keep the same shadow copy.
    fn mouse_pressed(
        &mut self,
        _button: MouseButton,
        _position: PhysicalPosition<f64>,
        _modifiers: ModifiersState,
    ) {
    }
    /// A mouse button came up. See [`Surface::mouse_pressed`] for where
    /// the position comes from.
    fn mouse_released(
        &mut self,
        _button: MouseButton,
        _position: PhysicalPosition<f64>,
        _modifiers: ModifiersState,
    ) {
    }
    /// The pointer moved to this physical-pixel position.
    fn cursor_moved(&mut self, _position: PhysicalPosition<f64>, _modifiers: ModifiersState) {}
    /// The pointer left the window. winit reports the leaving and not where
    /// the pointer went, and no other event tells a surface its pointer is
    /// no longer over it.
    fn cursor_left(&mut self) {}
    /// The wheel turned.
    fn mouse_wheel(&mut self, _delta: MouseScrollDelta, _modifiers: ModifiersState) {}
    /// The window gained or lost keyboard focus.
    fn focus_changed(&mut self, _focused: bool) {}
    /// Open the settings window.
    ///
    /// The right-click's own door, reached from outside the glass: the macOS
    /// menu bar carries a Settings item, and the shell routes it to the
    /// focused window's surface because the bookkeeping that keeps a second
    /// settings window from opening lives there and nowhere else. A surface
    /// with no settings window takes the default and does nothing.
    fn open_settings(&mut self) {}
    /// Called once per turn of the loop, before it goes back to waiting.
    ///
    /// The PTY is not a winit event source: nothing wakes the loop when
    /// the child writes, so a surface that owns one asks to be woken again
    /// through [`Tick::wake_at`]. A surface with nothing to poll returns
    /// the default and the loop waits, which is what the empty shell the
    /// contract harness drives does.
    fn tick(&mut self) -> Tick {
        Tick::default()
    }
    /// The scale factor changed (a DPI change, or the window moved to
    /// another monitor).
    fn scale_factor_changed(&mut self, _scale_factor: f64) {}
    /// Draw one frame.
    fn redraw(&mut self) {}
    /// The size of one character cell in physical pixels, which is what
    /// turns a window size into the grid size the overlay prints. The
    /// renderer answers this from the real font metrics; the default is the
    /// 8x16 of a stock VGA text mode, so the shell has an answer before
    /// there is a font.
    fn cell_size(&self) -> PhysicalSize<u32> {
        PhysicalSize::new(8, 16)
    }
    /// The least screen well this surface will work in, in physical pixels:
    /// the room a `term::FLOOR_COLS` x `term::FLOOR_ROWS` grid takes at the
    /// font it is drawing with, which is what the window's minimum-size hint
    /// reserves beside the channel bank.
    ///
    /// The default answers it for the default cell, through the same
    /// arithmetic the real surface uses, so a shell whose windows are empty
    /// still carries a floor that means something.
    fn well_minimum(&self) -> PhysicalSize<u32> {
        let cell = self.cell_size();
        let (width, height) = term::Viewport::new(
            0,
            0,
            1.0,
            term::CellSize::new(cell.width as f32, cell.height as f32),
        )
        .well_minimum();
        PhysicalSize::new(width, height)
    }
    /// The size badge to put on the next frame: its text, and the opacity
    /// [`crate::overlay::SizeOverlay`] has faded it to.
    ///
    /// The shell pushes rather than the surface pulling, because the overlay
    /// is the shell's: it is fed by window resizes and by the loop's own
    /// clock, both of which are shell events, and a surface that reached back
    /// for it would need a handle on the thing that owns it. An opacity of
    /// zero means "draw nothing", so a surface can hold this verbatim and let
    /// [`crate::chrome::Badges`] decide.
    fn set_size_badge(&mut self, _text: &str, _opacity: f32) {}
    /// The title the session wants, if it has one yet.
    fn title(&self) -> Option<String> {
        None
    }
}

/// Builds the surface for a newly created window. The second argument is the
/// destination this window dials, if it has one: the resolved request itself,
/// so what a `[[ssh.host]]` row named -- its key above all -- reaches the
/// window whole rather than through a spelling that carries none of it.
pub type SurfaceFactory =
    Box<dyn FnMut(&Arc<Window>, Option<&crate::ssh::SshRequest>) -> Box<dyn Surface>>;

/// What a surface wants after one [`Surface::tick`].
#[derive(Debug, Clone, Copy, Default)]
pub struct Tick {
    /// Something changed; draw a frame.
    pub redraw: bool,
    /// Wake the loop no later than this, whatever else happens.
    pub wake_at: Option<Instant>,
    /// The thing behind the window is over (the child exited). The shell
    /// closes the window, and the process with it if it was the last one.
    pub finished: bool,
}

/// A window with nothing in it. This is what the shell runs with until the
/// real renderer is merged in, and what the contract harness
/// drives: every property the contract measures (class, geometry, hints,
/// title, count) is the shell's, not the renderer's.
pub struct EmptySurface;

impl Surface for EmptySurface {}

/// Events the shell's event loop accepts from elsewhere: another thread, or a
/// [`Surface`] that has something to say back to the shell that owns it.
#[derive(Debug, Clone)]
pub enum ShellEvent {
    /// Another launch of this binary asked for a window.
    NewWindow(NewWindow),
    /// A surface's seam drag re-fitted the channel bank, in logical pixels.
    ///
    /// This is a surface reaching its shell, and it exists because the two
    /// halves of one rule live on opposite sides of the seam: the bank's width
    /// is the cabinet's (`chassis::Cabinet`), and the minimum-size hint that
    /// stops the window being dragged under the well's floor from the other
    /// side is the shell's, because the shell owns the windows. A surface holds
    /// an `EventLoopProxy` rather than a `&mut Shell` for the ordinary reason:
    /// the shell owns the surface, so the arrow cannot run the other way
    /// directly.
    SetBankWidth {
        /// The bank as it now stands, which sizes a window opened next.
        width: u32,
        /// The bank at its narrowest, which is the hint's own term: a bank
        /// fitted to a window cannot set the floor that window is held to.
        minimum: u32,
    },
    /// The application menu's Settings item was chosen ([`crate::menu`]).
    ///
    /// The right-click's own door, reached from outside the glass. It
    /// arrives at the shell rather than at a surface because a menu bar
    /// belongs to the application and not to any one window: which window it
    /// means is the shell's answer, the shell being the only thing that
    /// knows which one has focus.
    OpenSettings,
    /// A surface's `close_window` chord. The window is the shell's to
    /// remove, and which window is the shell's answer, as for
    /// [`ShellEvent::OpenSettings`].
    CloseWindow,
    /// The application menu's Quit item was chosen ([`crate::menu`]).
    ///
    /// Deliberately not AppKit's `terminate:`, which ends the process with
    /// `exit()` and returns to nothing: `instance::Primary` would leave its
    /// socket and lock file behind on every Command-Q. Through the loop's own
    /// exit, Command-Q is the same shutdown as the last window closing. The
    /// Dock's Quit and a system log-out still take `terminate:` and still
    /// leave those two files, which the next launch clears as the corpses
    /// they are (`crate::instance`).
    Quit,
}

/// Everything the shell needs to know that it does not decide itself.
pub struct ShellConfig {
    /// The binary's basename: WM_CLASS, and the fallback window title.
    pub identity: String,
    /// Whether the first window comes up fullscreen (`--fullscreen`).
    pub fullscreen: bool,
    /// The destination every window this process opens dials, the way the
    /// session config's program applies to every window: `--ssh`'s spelling
    /// parsed, or the `[ssh]` table's default row resolved. `None` opens
    /// shells, which is the default.
    ///
    /// A resolved request rather than a spelling, because the row names
    /// things no spelling has room for: the key it offers is part of the
    /// destination, and re-parsing a `user@host:port` on the way to a window
    /// would drop it.
    pub ssh: Option<crate::ssh::SshRequest>,
    /// The `showTerminalSize` setting for the size overlay.
    pub show_terminal_size: bool,
    /// The channel bank's width in *logical* pixels, which sets the
    /// minimum-width hint. Zero when no bank stands, i.e. the chassis is
    /// hidden; each window scales it by its own factor at the hint site
    /// (`chassis::layout::min_inner_size_physical`).
    ///
    /// Real now: a window standing in a chassis draws the bank column
    /// (`app::chrome`), so the room the hint reserves is room the appliance
    /// uses. On the shipped profile that is 247 px, which is the annunciator's
    /// furniture around twelve characters of the measured lamp cell, and a
    /// minimum width of 567. A shell whose windows are empty
    /// ([`ShellConfig::empty`], the contract harness's shape) keeps it at zero,
    /// and so does a profile with `general.chassis_shown = false`: hiding
    /// the chassis takes the bank with it, so that is no bank rather than a
    /// bank of no width.
    ///
    /// It moves at runtime, because the seam drag re-fits the strips: a surface
    /// sends [`ShellEvent::SetBankWidth`] and every window's hint follows.
    pub bank_width: u32,
    /// The channel bank at its narrowest, in *logical* pixels: its footprint
    /// at the display's least strip, which is the term the minimum-size hint
    /// reserves (`chassis::Cabinet::min_bank_width`).
    ///
    /// Apart from [`ShellConfig::bank_width`] because the two answer different
    /// questions. The bank's live width says how much room the appliance is
    /// using and sizes the window opened next; its least says how much room a
    /// window is obliged to keep, and holding still is the whole of its job:
    /// a hint that rose as the bank widened would grow a window the user had
    /// narrowed.
    pub bank_minimum: u32,
    /// The screen well's floor in *logical* pixels -- filled from
    /// `crate::window::well_minimum_for` at scale factor 1, where the logical
    /// and physical numbers are the same one -- for the size the first window
    /// opens at: the room this profile's font needs for a
    /// `term::FLOOR_COLS` x `term::FLOOR_ROWS` grid. Zero leaves the default
    /// window size alone, which is what a shell whose windows are empty
    /// ([`ShellConfig::empty`]) wants.
    ///
    /// It is asked for here, before any window exists, because that is the
    /// only moment it can be acted on: a window is mapped at the size it was
    /// created with, and a resize asked for between creation and the first
    /// frame is a resize the user watches happen. The rule itself is each
    /// window's own minimum-size hint, which every surface measures against
    /// the font it is drawing with and re-applies as that font moves.
    pub well_minimum: (u32, u32),
    /// Builds whatever goes inside a window. Called once per window, so
    /// each window gets its own surface and session.
    pub surface_factory: SurfaceFactory,
}

impl ShellConfig {
    /// A shell with empty windows: the shape the contract harness drives.
    pub fn empty(identity: impl Into<String>) -> Self {
        ShellConfig {
            identity: identity.into(),
            fullscreen: false,
            ssh: None,
            show_terminal_size: true,
            bank_width: 0,
            bank_minimum: 0,
            well_minimum: (0, 0),
            surface_factory: Box::new(|_, _| Box::new(EmptySurface)),
        }
    }
}

struct WindowState {
    window: Arc<Window>,
    surface: Box<dyn Surface>,
    overlay: SizeOverlay,
    /// Whether the surface currently holds a badge to draw, so the clearing
    /// push happens once rather than on every turn of an idle loop.
    badge_shown: bool,
    fullscreen: bool,
    title: String,
    /// The last position `CursorMoved` reported, because `MouseInput`
    /// does not carry one and the surface needs to know where the click
    /// landed.
    cursor: PhysicalPosition<f64>,
    /// The minimum-size hint as this window last carried it. Winit does not
    /// read one back, and both halves of it move at runtime, so the last one
    /// set is kept to tell a hint that moved from a hint asked for again.
    hint: (u32, u32),
}

impl WindowState {
    /// Re-apply the window's minimum-size hint if either half of it has
    /// moved: the bank's width, which the seam drag moves, or the well's
    /// floor, which the font and the screen's margin move.
    ///
    /// A window already under the new hint is resized up to it. A window
    /// manager enforces a minimum size on the next drag, not on the hint
    /// arriving, so a font that grew mid-session would otherwise leave the
    /// window standing at a size the hint says is not allowed.
    fn settle_min_inner_size(&mut self, bank_minimum: u32) {
        let floor = self.surface.well_minimum();
        let (min_width, min_height) = chassis::layout::min_inner_size_physical(
            bank_minimum,
            self.window.scale_factor(),
            (floor.width, floor.height),
        );
        if self.hint == (min_width, min_height) {
            return;
        }
        self.hint = (min_width, min_height);
        self.window
            .set_min_inner_size(Some(Size::Physical(PhysicalSize::new(
                min_width, min_height,
            ))));
        let now = self.window.inner_size();
        if now.width < min_width || now.height < min_height {
            let _ = self
                .window
                .request_inner_size(Size::Physical(PhysicalSize::new(
                    now.width.max(min_width),
                    now.height.max(min_height),
                )));
        }
    }
}

/// The application shell.
pub struct Shell {
    config: ShellConfig,
    windows: HashMap<WindowId, WindowState>,
    modifiers: ModifiersState,
    last_tick: Instant,
    /// Set once the first window is up, so a `resumed` on a platform that
    /// sends it more than once does not open a second window.
    started: bool,
    /// Which window last took keyboard focus, for the events that name the
    /// application rather than a window: the menu bar's items, which have to
    /// pick a window themselves.
    ///
    /// A record and not an authority. It goes stale, a window being able to
    /// close while focused and macOS being able to leave an application
    /// focused with no window at all, so [`menu_target`] reads it as a
    /// preference over the windows that actually stand.
    focused: Option<WindowId>,
    /// The menu's way back to this loop, taken in [`Shell::run`] where the
    /// loop is in hand and handed on at `resumed`.
    menu_proxy: Option<EventLoopProxy<ShellEvent>>,
}

impl Shell {
    pub fn new(config: ShellConfig) -> Self {
        Shell {
            config,
            windows: HashMap::new(),
            modifiers: ModifiersState::empty(),
            last_tick: Instant::now(),
            started: false,
            focused: None,
            menu_proxy: None,
        }
    }

    /// Builds the event loop the shell runs on, and a proxy for handing
    /// new-window requests to it from the single-instance listener
    /// thread.
    pub fn event_loop(
    ) -> Result<(EventLoop<ShellEvent>, EventLoopProxy<ShellEvent>), winit::error::EventLoopError>
    {
        let mut builder = EventLoop::<ShellEvent>::with_user_event();
        // Before the loop is built, because the menu bar this program puts up
        // goes where winit would otherwise have put its own, a few lines
        // earlier in the same launch. A no-op off macOS (`crate::menu`).
        crate::menu::suppress_default(&mut builder);
        let event_loop = builder.build()?;
        let proxy = event_loop.create_proxy();
        Ok((event_loop, proxy))
    }

    /// Runs until the last window closes.
    pub fn run(
        mut self,
        event_loop: EventLoop<ShellEvent>,
    ) -> Result<(), winit::error::EventLoopError> {
        // The menu's way back to this loop, taken here because this is where
        // the loop is in hand and `resumed` is where the menu goes up.
        self.menu_proxy = Some(event_loop.create_proxy());
        event_loop.set_control_flow(ControlFlow::Wait);
        event_loop.run_app(&mut self)
    }

    fn open_window(
        &mut self,
        event_loop: &ActiveEventLoop,
        fullscreen: bool,
        ssh: Option<&crate::ssh::SshRequest>,
    ) {
        // A fullscreen window's inner size is the compositor's to choose, so
        // nothing is measured for one and no property is read.
        let usable = (!fullscreen)
            .then(|| workarea::usable_size(event_loop))
            .flatten();
        let (width, height) = opening_size(
            (
                self.config.bank_width + self.config.well_minimum.0,
                self.config.well_minimum.1,
            ),
            (
                self.config.bank_minimum + self.config.well_minimum.0,
                self.config.well_minimum.1,
            ),
            usable,
        );

        let mut attributes = WindowAttributes::default()
            .with_title(self.config.identity.clone())
            .with_inner_size(Size::Physical(PhysicalSize::new(width, height)));

        // The binary's own basename is the application's identity, so a
        // renamed copy is a separate application to the window manager
        // too. Contract item 2 reads exactly this through
        // `xdotool search --class`.
        attributes = with_identity(attributes, &self.config.identity);

        if fullscreen {
            attributes = attributes.with_fullscreen(Some(Fullscreen::Borderless(None)));
        }

        let window = match event_loop.create_window(attributes) {
            Ok(w) => Arc::new(w),
            Err(e) => {
                eprintln!("cannot create a window: {e}");
                return;
            }
        };

        // Off by default in winit, and a terminal that cannot be typed into in
        // Japanese, Chinese or Korean is not a terminal for the people who type
        // in them. Without this call `WindowEvent::Ime` never arrives at
        // all and the composed text is silently dropped -- not degraded,
        // dropped -- exactly the silent-failure shape an IME regression takes.
        window.set_ime_allowed(true);

        let surface = (self.config.surface_factory)(&window, ssh);
        let mut overlay = SizeOverlay::new(self.config.show_terminal_size);
        overlay.resized(grid_size(window.inner_size(), surface.cell_size()));

        let id = window.id();
        let title = self.config.identity.clone();
        let mut state = WindowState {
            window,
            surface,
            overlay,
            badge_shown: false,
            fullscreen,
            title,
            cursor: PhysicalPosition::new(0.0, 0.0),
            hint: (0, 0),
        };
        // The hint is applied after creation rather than through the window's
        // attributes: half of it is the surface's font, which is resolved
        // against this window's scale factor, and that exists only once the
        // window is on a monitor.
        state.settle_min_inner_size(self.config.bank_minimum);
        self.windows.insert(id, state);
    }

    /// Re-applies the minimum-size hint to every window. The bank's width
    /// changes at runtime as the user drags the seam, and the hint is
    /// what contract item 4 measures, so it has to follow.
    pub fn set_bank_width(&mut self, width: u32, minimum: u32) {
        if (self.config.bank_width, self.config.bank_minimum) == (width, minimum) {
            return;
        }
        self.config.bank_width = width;
        self.config.bank_minimum = minimum;
        for state in self.windows.values_mut() {
            state.settle_min_inner_size(minimum);
        }
    }
}

/// Sets the window's WM_CLASS (X11) / app id (Wayland) to the binary's
/// basename. Split out so the non-Linux builds simply do not have it.
#[cfg(all(unix, not(target_os = "macos")))]
fn with_identity(attributes: WindowAttributes, identity: &str) -> WindowAttributes {
    use winit::platform::wayland::WindowAttributesExtWayland;
    use winit::platform::x11::WindowAttributesExtX11;
    // X11's WM_CLASS is (instance, class) and `xdotool search --class`
    // reads the class half; both are set to the identity here.
    let attributes = WindowAttributesExtX11::with_name(attributes, identity, identity);
    WindowAttributesExtWayland::with_name(attributes, identity, identity)
}

#[cfg(not(all(unix, not(target_os = "macos"))))]
fn with_identity(attributes: WindowAttributes, _identity: &str) -> WindowAttributes {
    attributes
}

/// Which window an application-level request means: the focused one while it
/// still stands, else any window, else none.
///
/// The menu bar is the application's, so a press on it names no window and
/// one has to be chosen. Generic over the id so it can be tested without a
/// display: `WindowId::dummy()` is a single value, and two distinct ids
/// cannot be minted in a unit test.
fn menu_target<Id: Copy + PartialEq>(focused: Option<Id>, open: &[Id]) -> Option<Id> {
    focused
        .filter(|id| open.contains(id))
        .or_else(|| open.first().copied())
}

/// The size the first window is created at: the appliance's default, raised
/// to hold the bank the profile asks for beside a terminal grid, then held
/// down to the desktop it has to fit on, then raised again past nothing.
///
/// `want` is that raised default, `floor` the window's own minimum, and
/// `usable` the desktop with its panels taken off, or `None` where no monitor
/// answered. The three are in physical pixels, the unit the default size is
/// quoted in; the floor is logical and the two are the same number on the
/// display the default was chosen for, with any other scale factor corrected
/// by the hint the window carries.
///
/// The floor wins over the desktop, deliberately. On a screen too small to
/// hold the least bank beside eighty columns by twenty-four, the window is
/// wider than the desktop and the terminal is a terminal; the alternative is
/// a window that fits the screen and cannot show a grid every curses program
/// assumes it has.
fn opening_size(want: (u32, u32), floor: (u32, u32), usable: Option<(u32, u32)>) -> (u32, u32) {
    let (want_w, want_h) = (
        geometry::DEFAULT_SIZE.0.max(want.0),
        geometry::DEFAULT_SIZE.1.max(want.1),
    );
    match usable {
        Some((usable_w, usable_h)) => (
            want_w.min(usable_w).max(floor.0),
            want_h.min(usable_h).max(floor.1),
        ),
        None => (want_w, want_h),
    }
}

/// A window's size in character cells.
fn grid_size(size: PhysicalSize<u32>, cell: PhysicalSize<u32>) -> GridSize {
    GridSize {
        columns: size.width / cell.width.max(1),
        rows: size.height / cell.height.max(1),
    }
}

impl ApplicationHandler<ShellEvent> for Shell {
    fn resumed(&mut self, event_loop: &ActiveEventLoop) {
        if self.started {
            return;
        }
        self.started = true;
        // Here rather than before the loop ran: macOS puts a menu bar up
        // during `applicationDidFinishLaunching:`, which is the call this
        // event arrives inside, so this is the first moment both on the main
        // thread and past launch.
        if let Some(proxy) = self.menu_proxy.clone() {
            crate::menu::install(&self.config.identity, proxy);
        }
        let fullscreen = self.config.fullscreen;
        let ssh = self.config.ssh.clone();
        self.open_window(event_loop, fullscreen, ssh.as_ref());
    }

    fn user_event(&mut self, event_loop: &ActiveEventLoop, event: ShellEvent) {
        match event {
            ShellEvent::NewWindow(request) => {
                // A handoff that named a destination gets it; one that did
                // not follows this process's own answer, config default
                // included, like any other new window. The handoff crosses
                // as the spelling it was typed as, which is all a command
                // line ever had, so it is parsed here; a spelling that fails
                // to parse is no destination and the window opens on this
                // process's own.
                let ssh = request
                    .ssh
                    .as_deref()
                    .and_then(|spec| crate::ssh::SshRequest::parse(spec).ok())
                    .or_else(|| self.config.ssh.clone());
                self.open_window(event_loop, request.fullscreen, ssh.as_ref());
            }
            ShellEvent::SetBankWidth { width, minimum } => self.set_bank_width(width, minimum),
            ShellEvent::OpenSettings => {
                let open: Vec<WindowId> = self.windows.keys().copied().collect();
                if let Some(id) = menu_target(self.focused, &open) {
                    if let Some(state) = self.windows.get_mut(&id) {
                        state.surface.open_settings();
                    }
                }
            }
            ShellEvent::CloseWindow => {
                let open: Vec<WindowId> = self.windows.keys().copied().collect();
                if let Some(id) = menu_target(self.focused, &open) {
                    self.windows.remove(&id);
                    if self.windows.is_empty() {
                        event_loop.exit();
                    }
                }
            }
            ShellEvent::Quit => event_loop.exit(),
        }
    }

    fn window_event(
        &mut self,
        event_loop: &ActiveEventLoop,
        window_id: WindowId,
        event: WindowEvent,
    ) {
        match event {
            WindowEvent::CloseRequested | WindowEvent::Destroyed => {
                self.windows.remove(&window_id);
                if self.windows.is_empty() {
                    event_loop.exit();
                }
            }
            WindowEvent::ModifiersChanged(modifiers) => {
                self.modifiers = modifiers.state();
                // Every window, not only the one the event names: a modifier is
                // the keyboard's state and X11 delivers the change to whichever
                // window has the pointer, which need not be the one holding a
                // chord.
                for state in self.windows.values_mut() {
                    state.surface.modifiers_changed(self.modifiers);
                }
            }
            WindowEvent::Resized(size) => {
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.resized(size);
                    let cell = state.surface.cell_size();
                    state.overlay.resized(grid_size(size, cell));
                    state.window.request_redraw();
                }
            }
            WindowEvent::ScaleFactorChanged { scale_factor, .. } => {
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.scale_factor_changed(scale_factor);
                    // Both halves of the hint are in device pixels and the
                    // factor converting them has just changed, so neither
                    // half is the number it was.
                    state.settle_min_inner_size(self.config.bank_minimum);
                }
            }
            WindowEvent::RedrawRequested => {
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.redraw();
                    // The fallback is the application's own name, and it is a
                    // *fallback*, not a floor. A channel that has never set a
                    // title, or one that has just come to the screen after a
                    // titled one, puts the identity back on the window rather
                    // than leaving the last channel's title standing over
                    // somebody else's shell.
                    let title = state
                        .surface
                        .title()
                        .unwrap_or_else(|| self.config.identity.clone());
                    if title != state.title {
                        state.window.set_title(&title);
                        state.title = title;
                    }
                }
            }
            WindowEvent::KeyboardInput { event, .. } => {
                if event.state != ElementState::Pressed {
                    return;
                }
                match event.physical_key {
                    // Explicit F11: the GNOME platform theme maps the
                    // platform's standard full-screen shortcut to Ctrl+F11,
                    // leaving plain F11 unbound, so it has to be handled
                    // directly here.
                    //
                    // No modifier guard, deliberately: F11 is the full-screen
                    // key whatever is held down with it, which is how
                    // Konsole's `Ctrl+Shift+F11` and GNOME Terminal's bare
                    // `F11` are the same key here. Nothing else in this
                    // build binds a modified F11, and the keytab's own
                    // `\E[23;*~` for it is a sequence no full-screen hand
                    // means to send.
                    PhysicalKey::Code(KeyCode::F11) => {
                        if let Some(state) = self.windows.get_mut(&window_id) {
                            state.fullscreen = !state.fullscreen;
                            state.window.set_fullscreen(
                                state.fullscreen.then(|| Fullscreen::Borderless(None)),
                            );
                            // The surface learns the new size only from the
                            // `Resized` that follows; #21's suspect is that
                            // event arriving late or not at all, and this
                            // line beside `resized`'s own is what shows it.
                            let inner = state.window.inner_size();
                            log::debug!(
                                "fullscreen toggled {}: inner {}x{}",
                                state.fullscreen,
                                inner.width,
                                inner.height
                            );
                        }
                    }
                    // Not one of the shell's own; the surface decides
                    // what it means (the input crate's keytab encoding).
                    _ => {
                        let modifiers = self.modifiers;
                        if let Some(state) = self.windows.get_mut(&window_id) {
                            state.surface.key_pressed(&event, modifiers);
                            // For the same reason the wheel asks: the keytab's
                            // scroll actions move the view with no pty traffic
                            // behind them, so nothing else would ask for the
                            // frame that shows it.
                            state.window.request_redraw();
                        }
                    }
                }
            }
            WindowEvent::Ime(ime) => {
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.ime(&ime);
                    // A commit is text arriving with no keystroke behind it, so
                    // nothing else on this turn of the loop would ask for the
                    // frame that shows what the child echoed back.
                    state.window.request_redraw();
                }
            }
            WindowEvent::CursorMoved { position, .. } => {
                let modifiers = self.modifiers;
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.cursor = position;
                    state.surface.cursor_moved(position, modifiers);
                }
            }
            WindowEvent::CursorLeft { .. } => {
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.cursor_left();
                }
            }
            WindowEvent::MouseInput {
                state: element_state,
                button,
                ..
            } => {
                let modifiers = self.modifiers;
                if let Some(state) = self.windows.get_mut(&window_id) {
                    let position = state.cursor;
                    match element_state {
                        ElementState::Pressed => {
                            state.surface.mouse_pressed(button, position, modifiers)
                        }
                        ElementState::Released => {
                            state.surface.mouse_released(button, position, modifiers)
                        }
                    }
                    // A click marks or unmarks a selection, which is a
                    // visible change with no PTY traffic behind it, so
                    // nothing else would ask for the frame.
                    state.window.request_redraw();
                }
            }
            WindowEvent::MouseWheel { delta, .. } => {
                let modifiers = self.modifiers;
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.mouse_wheel(delta, modifiers);
                    state.window.request_redraw();
                }
            }
            WindowEvent::Focused(focused) => {
                if focused {
                    self.focused = Some(window_id);
                } else if self.focused == Some(window_id) {
                    self.focused = None;
                }
                if let Some(state) = self.windows.get_mut(&window_id) {
                    state.surface.focus_changed(focused);
                }
            }
            _ => {}
        }
    }

    fn about_to_wait(&mut self, event_loop: &ActiveEventLoop) {
        let now = Instant::now();
        let delta = now.saturating_duration_since(self.last_tick);
        self.last_tick = now;

        // The size overlay's state machine is kept fed -- it is a function of
        // elapsed time, and a badge that skipped the seconds it was not drawn
        // in would be wrong the moment something drew it.
        //
        // The wake is back, and only while the badge is on screen. It was
        // dropped when nothing drew the overlay, because waking a core to
        // animate an opacity no pass read was a terminal spinning for nothing;
        // `crate::chrome` reads it now, so the frames that carry the 200 ms fade
        // have to be asked for. Outside those 1.2 seconds this costs nothing:
        // an invisible badge asks for no redraw and no wake, and the loop goes
        // back to waiting exactly as it did before.
        let mut wake_at: Option<Instant> = None;
        let mut finished: Vec<WindowId> = Vec::new();
        let bank_minimum = self.config.bank_minimum;
        for (id, state) in self.windows.iter_mut() {
            state.overlay.tick(delta);
            let opacity = state.overlay.opacity();
            if opacity > 0.0 {
                state.surface.set_size_badge(&state.overlay.text(), opacity);
                state.badge_shown = true;
                state.window.request_redraw();
                let at = now + OVERLAY_FRAME;
                wake_at = Some(wake_at.map_or(at, |cur: Instant| cur.min(at)));
            } else if state.badge_shown {
                // Once, on the way down, and then not again: `text()` formats a
                // string, and doing that every turn of an idle loop to say
                // "still nothing" is the cost this branch exists to avoid.
                state.surface.set_size_badge("", 0.0);
                state.badge_shown = false;
            }
            // The well's floor moves with the font and with the screen's
            // margin, both of which the user edits mid-session and neither of
            // which reaches the shell as an event. The check is arithmetic
            // over numbers the surface already holds, and the hint is only
            // written when it moved.
            state.settle_min_inner_size(bank_minimum);
            let tick = state.surface.tick();
            if tick.redraw {
                state.window.request_redraw();
            }
            if let Some(at) = tick.wake_at {
                wake_at = Some(wake_at.map_or(at, |cur: Instant| cur.min(at)));
            }
            if tick.finished {
                finished.push(*id);
            }
        }
        for id in finished {
            self.windows.remove(&id);
        }
        if self.windows.is_empty() && self.started {
            event_loop.exit();
            return;
        }

        event_loop.set_control_flow(match wake_at {
            Some(at) => ControlFlow::WaitUntil(at),
            None => ControlFlow::Wait,
        });
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn the_menu_means_the_focused_window() {
        assert_eq!(menu_target(Some(2), &[1, 2, 3]), Some(2));
    }

    #[test]
    fn a_stale_focus_does_not_name_a_window_that_has_closed() {
        assert_eq!(menu_target(Some(9), &[7, 8]), Some(7));
    }

    #[test]
    fn a_menu_press_with_nothing_focused_still_reaches_a_window() {
        // macOS leaves an application focused with every window minimised,
        // and a menu item that quietly did nothing would be worse than one
        // that picks.
        assert_eq!(menu_target(None, &[7, 8]), Some(7));
    }

    #[test]
    fn a_menu_press_with_no_windows_names_nothing() {
        assert_eq!(menu_target(Some(1), &[] as &[u32]), None);
    }

    /// The macOS menu hands its proxy to an Objective-C action through a
    /// `static`, which holds only while the proxy is `Send + Sync`; winit
    /// makes it so for a payload that is `Send`. This fails to compile on
    /// every platform the moment a variant carries something that is not.
    #[test]
    fn the_shell_event_stays_sendable_across_the_menu_static() {
        fn assert_shareable<T: Send + Sync>() {}
        assert_shareable::<EventLoopProxy<ShellEvent>>();
    }

    #[test]
    fn a_roomy_desktop_leaves_the_opening_size_alone() {
        // 1024x768 is the appliance's own default and this desktop has room
        // for it, so nothing here changes what the window opens at.
        let opened = opening_size((567, 240), (519, 240), Some((1920, 1080)));
        assert_eq!(opened, geometry::DEFAULT_SIZE);
    }

    #[test]
    fn a_profile_wider_than_the_default_opens_at_its_own_width() {
        // The shipped cabinet with a font whose grid costs 1018 px: the
        // window opens at the bank plus that, rather than at 1024 and being
        // grown a frame later.
        let opened = opening_size((1265, 730), (1217, 730), Some((1920, 1080)));
        assert_eq!(opened, (1265, 768));
    }

    #[test]
    fn a_desktop_narrower_than_the_appliance_holds_the_window_down_to_it() {
        let opened = opening_size((1265, 730), (1217, 730), Some((1152, 864)));
        assert_eq!(opened, (1217, 768));
    }

    #[test]
    fn the_terminal_grid_beats_the_desktop_when_the_two_cannot_both_hold() {
        // A netbook screen against a profile whose least bank and eighty
        // columns want more than it has: the window is wider than the
        // desktop, which is the decision this function records.
        let opened = opening_size((1265, 730), (1217, 730), Some((1024, 600)));
        assert_eq!(opened, (1217, 730));
        assert!(opened.0 > 1024 && opened.1 > 600);
    }

    #[test]
    fn no_monitor_leaves_the_appliance_its_own_default() {
        assert_eq!(opening_size((1265, 730), (1217, 730), None), (1265, 768));
    }

    #[test]
    fn a_window_size_becomes_a_grid_size_by_cell_division() {
        let grid = grid_size(PhysicalSize::new(800, 600), PhysicalSize::new(8, 16));
        assert_eq!(
            grid,
            GridSize {
                columns: 100,
                rows: 37
            }
        );
    }

    /// A degenerate cell size must not divide by zero: a renderer that
    /// has not measured its font yet reports what it has.
    #[test]
    fn a_zero_cell_size_does_not_divide_by_zero() {
        let grid = grid_size(PhysicalSize::new(800, 600), PhysicalSize::new(0, 0));
        assert_eq!(
            grid,
            GridSize {
                columns: 800,
                rows: 600
            }
        );
    }
}
