//! What fills a shell window: the wgpu surface, the rio-vt session
//! behind it, and the input path in front of it.
//!
//! Two events change the grid, and only one of them is a resize.
//! `Resized` moves the window's pixels; `ScaleFactorChanged` moves the
//! *cell* while the window's logical size stays put. Both end at
//! `sync_geometry`, which recomputes the grid from the viewport and
//! applies it to the session, so neither can be handled correctly and
//! the other forgotten.
//!
//! The event loop itself is [`crate::shell`]'s, not this module's: the
//! PTY is not a winit event source, so [`TerminalSurface::tick`] asks
//! the shell to wake it again through [`Tick::wake_at`] rather than
//! spinning a loop of its own.
//!
//! Two paths through this module put several separately-tested pieces in a
//! fixed order, and neither order is a matter of taste. The first is the
//! redraw ([`Glass`]):
//!
//! 1. the settings snapshot reaches the chain, which decides for itself
//!    whether that is a uniform push or a rebuild;
//! 2. the renderer syncs from the session, which is where rio-vt's damage
//!    turns into instance-buffer writes;
//! 3. the clock ticks, once, and the degauss hook is sampled off the same
//!    instant;
//! 4. `Params::build` turns settings plus geometry plus that instant into
//!    uniforms, and the chain takes them;
//! 5. one encoder records the grid into the offscreen target, then the chain
//!    from that target into the screen well's rectangle of the swapchain view,
//!    then the bank column's casting over what the chain left of the rest;
//! 6. present.
//!
//! Nothing between steps 3 and 6 reads a clock again: a frame drawn on two
//! instants fades the burn-in ghost by one of them and animates by the other.
//!
//! The chrome comes last and not first because the chain's own last pass
//! clears the whole image before it draws its rectangle of it; [`crate::chrome`]
//! says more, and it is the reason the casting is composited over the finished
//! frame rather than run as a chain of its own into the same view.
//!
//! # Where the window ends and the well begins
//!
//! With a chassis shown the window is two rectangles, not one
//! (`chassis::WindowLayout`): a bank column at the left and the screen well to
//! its right. Everything that used to mean "the window" means the *well* here:
//! the surface's [`Viewport`], the grid the session is resized to, the
//! offscreen target, the chain's geometry and every uniform normalised by
//! `normalizedScreenScale`. The window's own size survives as
//! `window_size`, and only two things read it: the swapchain, which is the
//! whole window, and the arithmetic that divides the two.
//!
//! A surface with no cabinet (every headless one, and any window whose
//! profile hides the chassis) has a bank of no width, so the well *is* the
//! window and none of the above is visible.
//!
//! The second path is the pointer:
//!
//! 0. the seam gets first refusal: a press, a drag or a hover on the ten pixels
//!    where the bank's plastic meets the glass is the cabinet's
//!    (`chassis::seam`), and never also the grid's;
//! 1. every pointer position that is the grid's is measured from the well's
//!    left edge and goes through [`term::distortion`] first: the click
//!    landed on the bent glass, so it is pushed back through the warp
//!    before it is anything else;
//! 2. the corrected point divides by the cell into an *absolute* grid
//!    cell, which is the coordinate [`term::selection`] works in;
//! 3. [`term::pointer`] decides what the event means (mark the screen,
//!    report to the program, or paste), because that depends on terminal
//!    state (mouse reporting, frozen glass), not on the window;
//! 4. only then does anything happen: a selection, a
//!    [`crate::mouse`]-encoded report down the PTY, or a
//!    [`crate::clipboard`] copy.

use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use chassis::Cabinet;
use config::{Config, CritterSettings, CritterTiming};
use critters::Critters;
use crt::{Chain, Degauss, Geometry, Pacing, Params};
use term::distortion;
use term::fonts::sizing::{ScalePolicy, SizingRequest};
use term::hotspots::UrlFilterChain;
use term::rio_vt::crosswords::pos::Side;
use term::rio_vt::crosswords::Mode;
use term::selection::{Gesture, Kind, MarkedRange, SelectionModel};
use ssh_link::{AskDesk, Link};
use term::{
    CellSize, ChannelSession, ControlModeTap, FontContext, FontEntry, GridRenderer, Marked,
    ResolvedFont, Scheme, ScrollPosition, Session, SessionConfig, Target,
    Viewport,
};
use tmux_cc::SessionId;
use winit::dpi::{PhysicalPosition, PhysicalSize};
use winit::event::{Ime, MouseButton, MouseScrollDelta};
use winit::event_loop::EventLoopProxy;
use winit::keyboard::ModifiersState;
use winit::window::{CursorIcon, Window};

use crate::bank::BankPager;
use crate::channels::{BankId, Channels, Close};
use crate::chord::ChordInput;
use crate::chrome::Chrome;
use crate::frame_stats::Mark;
use crate::gpu::Gpu;
use crate::opener::Opener;
use crate::settings::{self, SettingsHandle};
use crate::shell::{ShellEvent, Surface, Tick};
use crate::ssh::SshRequest;
use crate::tmux::Gateway;
use crate::window::keys::{key_text, modifiers_from};
use crate::{clipboard, paths};

mod bank;
mod keys;
mod picker;
mod pointer;
mod prompt;
mod seam;
mod ssh;
mod tmux;

/// Re-exported from where it was defined until the module split, because
/// [`TerminalSurface::ime_state`] answers with one and that is the name every
/// caller reaches it by.
pub use crate::ime::ImeState;

/// How often the loop wakes to drain the PTY when nothing else is
/// happening. ~125 Hz: comfortably below a frame at 60 Hz, so output
/// never waits a frame longer than it has to, and cheap enough to idle
/// on.
const POLL_INTERVAL: Duration = Duration::from_millis(8);

/// One frame at 60 Hz, the rate `general.effects_frame_skip` counts in.
///
/// Every shader in the chain reads `time`, and it is republished only every
/// `effectsFrameSkip`th frame rather than continuously, so that division is
/// what the CRT actually animates at: 20 Hz at the shipped skip of 3.
/// Nothing here is vsync-paced -- the event loop is a timer, not a frame
/// callback -- so 60 Hz is assumed and the skip multiplies it. If a real
/// display session (R2) says the assumption costs anything, the fix is to
/// measure the present interval and divide that, not to change what the
/// setting means.
pub const EFFECTS_BASE_FRAME: Duration = Duration::from_micros(16_667);

/// How long the glass goes untouched before it counts as unattended.
///
/// Not a setting: what happens either side of it is one behaviour, and a
/// knob for one behaviour is more than the behaviour is worth. Five minutes
/// because the hand leaves the keys long before the eyes leave the screen:
/// a page of output being read is a terminal in use.
const UNATTENDED_AFTER: Duration = Duration::from_secs(300);

/// What the badge says when a PTY channel's write queue sheds: the child this
/// program spawned has stopped reading its tty and the keystrokes aimed at it
/// are being thrown away. See [`TerminalSurface::watch_the_write_queues`] for
/// why the wording is this short.
pub const SHED_PTY: &str = "input dropped";

/// The same, for the queue on the way to a tmux server: the tmux server is
/// not reading its control wire, so what is being dropped is `send-keys`.
pub const SHED_TMUX: &str = "tmux input dropped";

/// The same, for an SSH channel's wire: the network or the remote program
/// has stopped taking bytes, which is a different remedy from either of the
/// two above.
pub const SHED_SSH: &str = "ssh input dropped";

/// What stands on the glass when the air is on a bank with no open row: what
/// the dark screen is, and the one thing to do about it. No timer.
pub const NO_SIGNAL: [&str; 2] = ["no channel on this bank", "Ctrl+Shift+T starts one"];

/// Not a config key: a fixed multiplier applied to the stored `fontScaling`.
/// `SizingRequest::default()` carries the same 0.75, and this name exists so
/// the one place that needs the product for a shader uniform
/// (`totalFontScaling`) does not restate the number.
const BASE_FONT_SCALING: f64 = 0.75;

/// The chain's geometry, from a physical render target and the window it is
/// drawn in. The arithmetic of [`Glass::geometry`], with nothing in it that
/// needs a device, so the unit conversion can be measured without one.
fn chain_geometry(
    target_size: (u32, u32),
    integer_scale: u32,
    cfg: &Config,
    scale_factor: f64,
) -> Geometry {
    let scale = integer_scale.max(1) as f32;
    let (width, height) = (target_size.0 as f32, target_size.1 as f32);
    let ratio = if scale_factor > 0.0 {
        scale_factor as f32
    } else {
        1.0
    };
    let font_width = cfg.screen.font_width.max(0.01) as f32;
    Geometry {
        output_width: width / ratio,
        output_height: height / ratio,
        virtual_width: (width / (scale * font_width)).floor().max(1.0),
        virtual_height: (height / scale).floor().max(1.0),
        total_font_scaling: (BASE_FONT_SCALING * cfg.general.font_scaling) as f32,
        device_pixel_ratio: ratio,
    }
}

/// The bank column's footprint in physical pixels, held to at most one pixel
/// short of the window so the screen well never has none.
///
/// Free rather than a method because [`TerminalSurface::new`] needs it before
/// there is a surface to ask.
fn bank_physical(cabinet: &Cabinet, scale_factor: f64, window_width: u32) -> u32 {
    cabinet
        .bank_width_physical(scale_factor)
        .min(window_width.saturating_sub(1))
}

/// The catalogue entry `screen.font_name` names, looked up in the catalogue
/// `screen.font_source` selects, or the shipped default if it names nothing
/// there.
///
/// Two ways to name nothing, and both end here rather than in a refusal to
/// draw: a hand-edited config, and a profile still carrying a system face's
/// name after its source went back to the bundled faces. The second is the
/// migration, and it is a warning and a legible font rather than a scan --
/// the whole point of the source key is that a bundled profile does not
/// enumerate the machine to find out what it is missing.
fn font_entry(cfg: &Config) -> &'static FontEntry {
    // `--font` outranks the profile and every later reload of it. The
    // machine's catalogue is searched rather than the one the profile
    // selects, because it holds the bundled faces as well, so one flag
    // takes either kind of name. A name that is in neither falls through to
    // the profile's own answer below, with the family said out loud: a
    // misspelt flag that silently drew the default font would be a run
    // whose picture disagrees with the command that asked for it.
    if let Some(name) = font_override() {
        match term::font_by_name(name, term::FontSource::System) {
            Some(entry) => return entry,
            None => log::error!("--font {name:?}: no such family on this machine"),
        }
    }
    let source = chassis::font_source(cfg);
    if let Some(entry) = term::font_by_name(&cfg.screen.font_name, source) {
        return entry;
    }
    log::warn!(
        "the font {:?} is not in the {source:?} catalogue; \
         falling back to the shipped default",
        cfg.screen.font_name,
    );
    term::font_by_name(
        &Config::default().screen.font_name,
        term::FontSource::Bundled,
    )
    .expect("the bundled catalogue always contains the default font")
}

/// The family `--font` named, for the life of the process.
///
/// A `OnceLock` above the windows rather than a field on one: the flag is the
/// process's, every window this process opens draws in it, and a window
/// opened by a later `--new-window` handoff would have no argument list of
/// its own to read it from.
static FONT_OVERRIDE: std::sync::OnceLock<Option<String>> = std::sync::OnceLock::new();

/// Record what `--font` named. The first call wins, which is the one the
/// command line makes before any window exists.
pub fn set_font_override(family: Option<String>) {
    let _ = FONT_OVERRIDE.set(family);
}

fn font_override() -> Option<&'static str> {
    FONT_OVERRIDE.get()?.as_deref()
}

/// What a channel slot holds in this binary: a PTY session carrying the
/// tmux-detecting tap, or a tmux pane's screen the gateway feeds.
pub type AppSession = ChannelSession<ControlModeTap>;

/// Start a channel's session, or log why not. A window that cannot start a
/// second shell keeps the first one and its picture; refusing the slot is the
/// whole of the failure, which is why [`Channels`] takes a closure that may
/// answer `None` rather than a session.
fn spawn(config: &SessionConfig, size: term::TermSize) -> Option<AppSession> {
    match Session::spawn(config, size, ControlModeTap::default()) {
        Ok(s) => Some(ChannelSession::Pty(s)),
        Err(e) => {
            log::error!("could not start a session: {e}");
            None
        }
    }
}

/// The sizing knobs, as this window's settings and monitor set them.
fn sizing_request(cfg: &Config, scale_factor: f64) -> SizingRequest {
    SizingRequest {
        font_scaling: cfg.general.font_scaling,
        line_spacing: cfg.screen.line_spacing,
        font_width: cfg.screen.font_width,
        window_scaling: cfg.general.window_scaling,
        device_pixel_ratio: scale_factor,
        prose_scaling: cfg.screen.prose_scaling,
        ..SizingRequest::default()
    }
}

/// The well's floor for a profile, in physical pixels: what a window drawn
/// under `cfg` at `scale_factor` will answer from
/// [`crate::shell::Surface::well_minimum`], measured before there is a window
/// to ask.
///
/// The shell opens its first window at least this big, because a window is
/// mapped at the size it was created with: a floor found afterwards can only
/// be applied as a resize the user watches happen.
pub fn well_minimum_for(cfg: &Config, scale_factor: f64) -> (u32, u32) {
    let entry = font_entry(cfg);
    let request = sizing_request(cfg, scale_factor);
    let resolved = term::resolve(entry, &request, ScalePolicy::Floor);
    let cell = logical_cell(
        FontContext::new(entry).cell_metrics(&resolved),
        &resolved,
        scale_factor,
    );
    let mut viewport = Viewport::new(0, 0, scale_factor, cell);
    viewport.margin = settings::distortion_margin(cfg) * scale_factor;
    viewport.well_minimum()
}

/// The well's floor in the logical pixels the chassis is laid out in.
///
/// The floor itself is a physical count of cells (`Viewport::well_minimum`),
/// so the conversion rounds up: a well half a physical pixel short of the
/// grid is a well short of the grid. This is the seam's copy of the rule;
/// the window's own minimum-size hint takes the physical number as it
/// stands, through [`crate::shell::Surface::well_minimum`].
fn logical_well_minimum(viewport: &Viewport) -> (i32, i32) {
    let (width, height) = viewport.well_minimum();
    let scale = viewport.scale_factor.max(f64::EPSILON);
    (
        (f64::from(width) / scale).ceil() as i32,
        (f64::from(height) / scale).ceil() as i32,
    )
}

/// The cell box in logical pixels, which is what [`Viewport`] takes.
///
/// The renderer's cell is `atlas.cell * integer_scale` *physical* pixels, and
/// `Viewport::physical_cell` multiplies by the DPR and rounds, so dividing
/// here and multiplying there is exact rather than approximately exact: the
/// grid the session is resized to is the grid the renderer draws.
fn logical_cell(cell: term::CellMetrics, resolved: &ResolvedFont, scale_factor: f64) -> CellSize {
    let scale = resolved.integer_scale as f64;
    CellSize {
        width: (f64::from(cell.width) * scale / scale_factor) as f32,
        height: (f64::from(cell.height) * scale / scale_factor) as f32,
    }
}

/// Everything behind the glass that needs a GPU: the grid renderer and its
/// atlas, the offscreen target the grid is drawn into, and the CRT chain that
/// takes that target to the swapchain.
///
/// It is one struct because it is one lifetime. Every field is built from the
/// same device, and the one that is not (`pacing`) is the clock that chain is
/// driven by, which has nowhere better to live: a second clock would be a
/// second answer to what time this frame is.
///
/// The degauss transient is not here: it is *triggered* by a channel change
/// and only *sampled* by the frame, so it belongs to the surface's state and
/// not the device's: a window whose chain
/// failed to load still switches channels, and a headless surface -- which is
/// where the channel state machines are tested -- has no `Glass` at all.
struct Glass {
    renderer: GridRenderer,
    /// The shaping side of the face on screen. It is kept rather than dropped
    /// after the atlas is built because a character the bundled face has no
    /// glyph for is resolved against the machine's own fonts on the frame it
    /// first appears, and that resolution needs this context.
    font: FontContext,
    /// What the atlas was built for. A settings edit that moves any of it
    /// means the atlas is the wrong size and has to be rebuilt, which is also
    /// one of the two events `crt::burn_in`'s mount contract calls a
    /// discontinuity of the ghost.
    font_name: String,
    resolved: ResolvedFont,
    /// The chain's input, sized to the screen well rather than to the grid: the
    /// grid is centred in it at a whole-pixel origin, so a glyph still lands
    /// on the pixels it was rasterised for. Sizing it to the grid instead
    /// would leave the chain stretching a slightly-off-aspect picture over the
    /// well, which is a resample of exactly the pixel font this renderer
    /// exists to keep unresampled.
    target: Target,
    chain: Chain,
    /// The bank column's casting, mounted outside the chain. `None`
    /// when its pass could not be loaded, which is a window with a bare well
    /// rather than a process that dies.
    chrome: Chrome,
    pacing: Pacing,
    /// The settings the chain was last given, so a redraw can tell whether
    /// there is anything to apply.
    applied: Config,
    /// Target size and grid size as last logged, so the one line that says
    /// where the picture is appears when it moves and not once a frame.
    logged_geometry: Option<(u32, u32, u32, u32)>,
}

/// What a bank runs on, outside the rows the channel model holds: the
/// transport that carries it and the question standing on it.
///
/// One record per bank, and [`TerminalSurface::sweep_bank_runtimes`] is the
/// whole of when it exists: a bank has a record exactly while
/// [`Channels::manager_of`] answers for it. Dropping the record is what the
/// disconnect and the cancel are made of -- the `Link` is the connection
/// thread's lifetime, and the `AskDesk` going releases whichever thread is
/// parked on a question -- so a bank swept out of the model takes its wire
/// and its prompt with it in one move rather than in four sweeps that could
/// disagree.
#[derive(Default)]
struct BankRuntime {
    /// A tmux attachment's client half. The write side is a second handle
    /// onto the gateway's PTY (`term::Session::control_mode_writer`); the
    /// read side arrives through the gateway's DCS tap on every
    /// [`TerminalSurface::pump`].
    gateway: Option<Gateway<Box<dyn std::io::Write + Send>>>,
    /// One SSH connection, whose drop is the disconnect.
    link: Option<Link>,
    /// The answering end of the connection's question channel, drained on the
    /// same pump as everything else.
    desk: Option<AskDesk>,
    /// The question standing right now and the line being typed into it. At
    /// most one: the transport asks one thing at a time, because it is
    /// blocked on the answer to the last one.
    prompt: Option<prompt::Pending>,
}

/// The surface the binary runs behind the
/// [`crate::shell::Surface`] seam: the shell owns the event loop and the
/// window, this owns what is inside it.
pub struct TerminalSurface {
    /// `None` in a headless surface (see [`TerminalSurface::headless`]).
    /// It is read for the window's own size on a DPI change, and it is
    /// what says whether there is a display to reach a clipboard through.
    window: Option<Arc<Window>>,
    gpu: Option<Gpu>,
    /// This window's channels, one session each, and which of them is on the
    /// air (`crate::channels`). Empty only where the first session could not be
    /// spawned.
    channels: Channels<AppSession>,
    /// What each bank runs on: its gateway or its link, and the question
    /// standing on it ([`BankRuntime`]).
    banks: HashMap<BankId, BankRuntime>,
    /// The destination picker while it stands: the home slot its page holds,
    /// and everything the user has said to it. `Shift+Alt+T` raises it; a
    /// digit, a typed destination or `Esc` retires it.
    picker: Option<crate::picker::Picker>,
    /// The find line while it stands: the query being typed and the hit it
    /// has marked. `Ctrl+Shift+F` raises it, `Esc` takes it down, and it
    /// answers to one channel only ([`crate::find`]).
    find: Option<crate::find::Find>,
    /// Every session seen, by socket, server pid and id: banked, in flight, or
    /// owed; the model holds banks, so a listing is read against this.
    sessions: HashMap<(String, u32, SessionId), tmux::SessionSlot>,
    /// How a channel this window opens later is started: every channel gets
    /// the same configuration, so the shell a `Ctrl+Shift+T` starts is the
    /// shell the window was launched with.
    session_config: SessionConfig,
    /// Where the bank's numerals stand (`crate::bank`), which is what the
    /// chord's slots and a press on a strip are resolved against.
    pager: BankPager,
    /// The digits typed against those numerals (`crate::chord`).
    chord: ChordInput,
    /// Whether the chord modifier is down, so its release can commit -- Alt
    /// everywhere but macOS, where it is Meta.
    chord_modifier: bool,
    /// The channel-change transient. Triggered here, sampled by the frame; see
    /// [`Glass`] for why it does not live there.
    degauss: Degauss,
    /// This window's critters: what walks across the glass, and when. Here
    /// beside the degauss and for its reason -- triggered here, sampled by
    /// the frame -- and per window rather than per application, so two
    /// windows keep their own company.
    critters: Critters,
    /// The pair the glass was last showing, so the work that follows a switch
    /// runs on a switch and not on every pump.
    on_air: (BankId, u32),
    /// The **screen well**, not the window: its size is what the grid, the
    /// offscreen target and the chain are all measured in. See the module doc.
    viewport: Viewport,
    /// The window's own inner size in physical pixels. The swapchain is this
    /// big; the well is this less the bank column.
    window_size: (u32, u32),
    /// The last clipped-frame report, so the log carries one line per
    /// distinct disagreement rather than one per frame.
    logged_clip: Option<(u32, u32, u32, u32, u32)>,
    /// The `monospace_trigger` text the renderer was last handed, so a
    /// reload that did not touch it does not recompile it or rebuild every
    /// row. `None` until the first one is applied.
    monospace_trigger: Option<String>,
    /// The cabinet the well is set into, or `None` for a surface that stands in
    /// no chassis at all: every headless one unless a test asks for one
    /// ([`TerminalSurface::set_cabinet`]), which is what keeps a surface with
    /// no cabinet identical to the pre-cabinet behaviour rather than merely
    /// close to it.
    cabinet: Option<Cabinet>,
    /// The profile the cabinet was last measured for, so a redraw can tell
    /// whether the lamp font or the shell moved. Re-measuring reads a font
    /// face, which is not a per-frame cost worth paying for nothing.
    cabinet_cfg: Config,
    /// The seam took the press that is still down, so the release and the
    /// motions between belong to it and not to the grid.
    seam_press: bool,
    /// What the drag has landed and the settings do not carry yet. The write
    /// happens once, when the button comes up: the cabinet is already at the
    /// new count (`chassis::Cabinet::cursor_moved` takes it on the spot), so
    /// writing per motion would be a file rewrite and a reload per pixel of
    /// travel for a value the screen already shows.
    pending_led_characters: Option<i32>,
    /// Whether the seam claims the pointer's shape: the resize arrows over
    /// the grab strip, which outrank the hand over a link
    /// (`TerminalSurface::set_pointer_shape`).
    seam_cursor: bool,
    /// The shape the window was last told to wear, so it is told only when
    /// the shape changes.
    pointer_shape: CursorIcon,
    /// How this surface reaches its shell. The bank's width is the window's
    /// minimum-width hint and the shell owns every window, so a seam drag has
    /// to cross back ([`ShellEvent::SetBankWidth`]).
    shell_events: Option<EventLoopProxy<ShellEvent>>,
    /// The chords in force, and the `[bindings]` rows they were built from,
    /// so a reload that moved no row rebuilds nothing.
    bindings: crate::bindings::Bindings,
    binding_rows: Vec<config::KeyBinding>,
    /// What the input method is composing but has not committed, and whether
    /// composition is open at all.
    ///
    /// Read by every redraw, which is what puts the composition on the glass at
    /// the cursor ([`TerminalSurface::draw_frame`], and
    /// `term::render::GridRenderer::set_preedit` under it); the commit goes
    /// straight to the child and is kept nowhere. See
    /// [`TerminalSurface::ime_input`].
    ime: ImeState,
    /// What was last looked for, kept so the find line opens with it in
    /// place. One string for the window rather than one per channel: what a
    /// hand reaching for the chord again wants is what it last searched
    /// for, wherever it searched.
    last_find: String,
    eof: bool,
    /// Where the view sits in the scrollback. The wheel moves it.
    scroll: ScrollPosition,
    selection: SelectionModel,
    /// The absolute cell the pointer was last over.
    pointer_cell: (usize, usize),
    /// Whether the pointer is inside the window at all, so a modifier
    /// pressed with it over another window raises no underline here.
    pointer_present: bool,
    /// The URL filters, run over the viewport when the pointer asks what
    /// link it is over (`term::links::link_at`).
    links: UrlFilterChain,
    /// The link under the pointer while the chord that opens one is held:
    /// the cells it occupies, which the renderer underlines, and what
    /// opening it means.
    hover: Option<(MarkedRange, String)>,
    /// A left press that landed on a link with the chord held: the cell it
    /// landed on, and the link, opened if the button comes up on that cell.
    link_press: Option<((usize, usize), String)>,
    /// What opens a link: the platform's handler behind a window, a
    /// recording behind a headless surface (`crate::opener`).
    opener: Opener,
    /// The left button is down and a move extends the selection.
    dragging: bool,
    /// The left press that is down was Control-held and became a secondary
    /// click ([`pointer::control_click_is_secondary`]), so the release that
    /// closes it is a secondary release too.
    ///
    /// Held rather than recomputed because the modifiers are read afresh at
    /// the release: a Control let go while the button is still down would
    /// otherwise send a program a right press and a left release.
    secondary_press: bool,
    /// When and where the last left press landed, and how many presses deep
    /// the run is: 1 a click, 2 a double, 3 a triple. A fourth press on the
    /// same cell starts a new run at 1, which is what every terminal does.
    last_click: Option<(Instant, (usize, usize), u8)>,
    /// The two selections this window writes to, and the platform handle
    /// behind them ([`crate::clipboard`]). Held rather than made per call
    /// because on X11 the primary selection lives inside the process that
    /// owns it.
    clipboard: clipboard::Store,
    /// Pixels of wheel travel not yet worth a line, for the trackpads
    /// that report in pixels rather than notches.
    wheel_pixels: f64,
    /// The live config the pointer's inverse-distortion transform reads
    /// (margin, frame size, screen curvature) on every press/release/move.
    /// `None` in a headless surface with no settings handle
    /// attached (see [`TerminalSurface::headless`] and
    /// [`TerminalSurface::set_settings`]): every distortion parameter then
    /// falls back to the neutral/identity value, matching this crate's
    /// original placeholder behavior so a test that never opts in is
    /// unaffected by a settings handle it never asked for.
    settings: Option<Arc<SettingsHandle>>,
    /// What the settings are when there is no handle to ask.
    ///
    /// `--default-settings` reads and watches no file, so no handle is built
    /// -- but `--profile <name>` still resolves a look, and that resolution
    /// is the whole config this run is meant to wear. Without somewhere to
    /// put it the surface fell back to `Config::default()` on every frame and
    /// the flag reached the log line and nothing else, which is the shape
    /// `xtask snap` screenshots by name.
    ///
    /// A plain `Config` rather than a second handle: there is no file behind
    /// it and nothing to reload, so the only thing it needs to do is be
    /// readable once a frame. Defaults until [`TerminalSurface::set_config`]
    /// says otherwise, which keeps every headless test that never sets one
    /// exactly where it was.
    base: Config,
    /// When the next effects frame is due. The CRT animates on wall time --
    /// the glowing line sweeps, the noise crawls, the ghost fades -- so a
    /// window whose child has said nothing for a minute still owes the screen
    /// a frame. Without this the picture freezes on whatever the last byte of
    /// output left behind, which is a still photograph of a CRT rather than a
    /// CRT.
    ///
    /// Owed only while somebody is there. A still photograph is exactly what
    /// an unattended window should be, so [`Self::unattended`] takes this to
    /// `None` and the first frame after a person comes back is due at once.
    next_effects_frame: Option<Instant>,
    /// Whether this window has the keyboard, and when it was last touched.
    /// Together they say whether anyone is attending the glass, which is what
    /// the effects clock paces itself against: a screen nobody is looking at
    /// animates for nobody. A surface with no window to lose focus from
    /// starts attended, so a headless one keeps the cadence it always had.
    focused: bool,
    last_input: Instant,
    /// Nobody is at this glass, as of the last tick: the keyboard is
    /// elsewhere, or it has been untouched for [`UNATTENDED_AFTER`]. What it
    /// governs is everything the window would have drawn of its own accord.
    unattended: bool,
    /// The output governor: when child output may next ask for a frame, and
    /// whether any arrived since the last one it asked for. The PTY is
    /// polled at ~125 Hz and a flood would otherwise paint
    /// frames a 60 Hz panel never shows (measured on the Intel
    /// LNL: present-interval p50 3.3 ms during a `seq` flood). Output is
    /// therefore coalesced to [`EFFECTS_BASE_FRAME`]; a byte that arrives
    /// between frames waits for the next one, never longer.
    next_output_frame: Option<Instant>,
    output_pending: bool,
    /// Everything that needs a device: the grid renderer, the offscreen
    /// target and the CRT chain. `None` in a headless surface, and also in a
    /// windowed one whose chain failed to load, which is a window that clears
    /// rather than a process that dies.
    glass: Option<Glass>,
    /// The size badge as the shell last reported it: the text and the opacity
    /// its envelope has reached. Held rather than acted on, because it arrives
    /// from the loop's clock and is spent on the next frame
    /// ([`crate::shell::Surface::set_size_badge`]).
    size_badge: (String, f32),
    /// The second badge in the stack: what this appliance has to say for itself
    /// right now (a write queue shedding, a look saved). Raised here and drawn
    /// by [`Self::draw_frame`]; see [`crate::overlay::Notice`] for why it is the
    /// rebuild's own and not a port.
    notice: crate::overlay::Notice,
    /// The companion settings window this surface starts, and whatever it
    /// knows about the one it started last ([`settings::SettingsApp`]).
    settings_app: settings::SettingsApp,
    /// The shed counters as of the last [`Self::pump`]: the local children's
    /// input queues, and the gateways' command queues. A count that has moved is
    /// exactly "something the user typed was thrown away since we last looked",
    /// which is what [`Self::notice`] then says out loud.
    sheds_seen: (u64, u64, u64),
}

impl Glass {
    /// Build the glass for a window that has a GPU.
    ///
    /// `cfg` is the settings this window opens with. It is not necessarily the
    /// user's: `TerminalSurface::new` runs before the settings handle is
    /// attached, so the first frame's redraw is what reconciles the two, and
    /// it does that through the same two paths a later edit takes
    /// ([`TerminalSurface::apply_live_settings`]).
    fn new(gpu: &Gpu, cfg: &Config, viewport: &Viewport, identity: &str) -> Option<Self> {
        let entry = font_entry(cfg);
        let request = sizing_request(cfg, viewport.scale_factor);
        let (resolved, font, atlas) =
            term::build_font(&gpu.device, &gpu.queue, entry, &request, ScalePolicy::Floor);

        let size = viewport.term_size();
        // White on black, and the phosphor nowhere in it: the chain's last
        // pass converts a grey into the profile's two colours, so a grid
        // drawn in amber here would be tinted twice. The palette rides
        // through to that pass as the colours the program asked for, and the
        // pass weighs each into one brightness. A background colour therefore
        // lights its cell as much as the colour was bright, which is what a
        // monochrome monitor did with a colour signal; flattening the palette
        // here would light every one of them fully and swallow the text.
        let scheme = Scheme::full_color([1.0, 1.0, 1.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
        let mut renderer = GridRenderer::new(
            &gpu.device,
            &gpu.queue,
            atlas,
            size.cols(),
            size.rows(),
            scheme,
        );
        renderer.set_scale(resolved.integer_scale);

        // The well, not the window: the chain draws into the rectangle the bank
        // column leaves, and `Params::build` reads `normalizedScreenScale` off
        // the geometry this target's size produces.
        let target = Target::new(
            &gpu.device,
            viewport.width.max(1),
            viewport.height.max(1),
            gpu::TARGET_FORMAT,
        );

        let dir = paths::preset_dir(identity);
        let chain = match Chain::from_config(&gpu.device, &gpu.queue, &dir, cfg) {
            Ok(chain) => chain,
            Err(e) => {
                log::error!("could not load the CRT chain from {}: {e}", dir.display());
                return None;
            }
        };
        log::info!(
            "glass: {} at {}px x{} scale, {}x{} cells, preset {}",
            entry.name,
            resolved.raster_pixel_size,
            resolved.integer_scale,
            size.cols(),
            size.rows(),
            chain.preset_path().display()
        );

        Some(Self {
            renderer,
            font,
            font_name: entry.name.to_string(),
            resolved,
            target,
            chain,
            chrome: Chrome::new(&gpu.device, gpu.format()),
            // One clock per window, started here, never read anywhere else.
            pacing: Pacing::new(Instant::now()),
            applied: cfg.clone(),
            logged_geometry: None,
        })
    }

    /// The frame's measurements, as `crt::Params` wants them.
    ///
    /// `output_*` is in **logical** pixels, which is what `crt::Geometry`
    /// documents: the render target is physical, so the window's scale
    /// factor is divided back out here. This is the one place in the tree
    /// that conversion happens. Left undone it halves `normalizedScreenScale`
    /// on a 2x display, and with it `ScreenCurvature` and `FrameSize` -- a
    /// flatter tube in a thinner moulding, on exactly the machines that show
    /// it most clearly.
    ///
    /// `virtual_*` is the terminal area in *unscaled* raster pixels, which is
    /// what the rasterization mask is spaced by. It is a raster count rather
    /// than a length, so it stays on the physical size over this renderer's
    /// integer scale. The width is additionally divided by the profile's
    /// `fontWidth`, and both axes are floored to whole pixels: a mask spaced
    /// by 639.4 stripes is not the mask spaced by 639.
    fn geometry(&self, scale_factor: f64) -> Geometry {
        chain_geometry(
            (self.target.width, self.target.height),
            self.resolved.integer_scale,
            &self.applied,
            scale_factor,
        )
    }
}

/// A seed for one window's critters.
///
/// The wall clock's nanoseconds. Two windows opened in the same nanosecond
/// would keep the same company, and would deserve each other.
fn critter_seed() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos() as u64)
        .unwrap_or(0x5EED)
}

/// The number the config asks for, as a duration. The file takes any number
/// where the settings window offers a slider, so this is where an unusable
/// one stops: under a second reads as a second, and one too large for a
/// `Duration` at all falls back to the shipped one, a typo being no reason to
/// lose a session.
fn critter_interval(minutes: f64) -> Duration {
    let shipped = CritterSettings::default().minutes * 60.0;
    Duration::try_from_secs_f64((minutes * 60.0).max(1.0)).unwrap_or_else(|_| {
        log::warn!("critters.minutes = {minutes} is not a usable interval; using {shipped}s");
        Duration::from_secs_f64(shipped)
    })
}

/// This window's critters as they ship, before the settings file has been
/// read. `apply_live_settings` replaces this on the first frame; it is here
/// so the shipped values have one home, which is the schema.
fn shipped_critters() -> Critters {
    let shipped = CritterSettings::default();
    Critters::new(
        critter_seed(),
        shipped.enabled,
        critter_interval(shipped.minutes),
        shipped.timing == CritterTiming::Random,
        critter_cast(&shipped),
    )
}

/// Which pieces the config leaves switched on, in `critters::ART`'s order.
///
/// The one place the table's key names and the art's names are held against
/// each other. A piece added to the art without a key to retire it would
/// arrive here unmatched and be left off, and
/// `crates/app/tests/suite/critters.rs` is what stops that happening
/// quietly.
fn critter_cast(settings: &CritterSettings) -> [bool; critters::ART.len()] {
    let mut cast = [false; critters::ART.len()];
    for (on, art) in cast.iter_mut().zip(critters::ART.iter()) {
        *on = match art.name {
            "dolphins" => settings.dolphins,
            "ducks" => settings.ducks,
            "swan" => settings.swan,
            "whale" => settings.whale,
            "ship" => settings.ship,
            "monster" => settings.monster,
            "pacman" => settings.pacman,
            "locomotive" => settings.locomotive,
            _ => false,
        };
    }
    cast
}

impl TerminalSurface {
    /// Builds a surface for a window the shell has already created.
    /// Failure to get a GPU or a PTY is logged and leaves an empty
    /// surface: a window that shows nothing beats a process that dies
    /// with no window at all, which is what the contract harness would
    /// see.
    pub fn new(
        instance: &wgpu::Instance,
        window: &Arc<Window>,
        session: &SessionConfig,
        frame_stats_enabled: bool,
        ssh: Option<&SshRequest>,
    ) -> Self {
        let window = Arc::clone(window);
        let physical = window.inner_size();

        // The cell comes from the font, and the font's metrics need no GPU:
        // the grid the session is spawned with is therefore the real grid on
        // the first frame, not a guess corrected on the second. The settings
        // are the defaults here because the handle is attached after this
        // returns; a font the user chose instead arrives through
        // `apply_live_settings` on the first redraw.
        let cfg = Config::default();
        let entry = font_entry(&cfg);
        let scale_factor = window.scale_factor();
        let request = sizing_request(&cfg, scale_factor);
        let resolved = term::resolve(entry, &request, ScalePolicy::Floor);
        let cell = logical_cell(
            FontContext::new(entry).cell_metrics(&resolved),
            &resolved,
            scale_factor,
        );

        // A window stands in a cabinet, and the cabinet is what says how much
        // of it is glass. The profile is the defaults for the same reason the
        // font is: the settings handle is attached after this returns, and
        // `set_settings` re-measures.
        let window_size = (physical.width.max(1), physical.height.max(1));
        let mut cabinet = Cabinet::from_config(
            &cfg,
            f64::from(window_size.0) / scale_factor.max(f64::EPSILON),
            f64::from(window_size.1) / scale_factor.max(f64::EPSILON),
        );
        let bank = bank_physical(&cabinet, scale_factor, window_size.0);
        let mut viewport = Viewport::new(
            window_size.0.saturating_sub(bank).max(1),
            window_size.1,
            scale_factor,
            cell,
        );
        viewport.margin = settings::distortion_margin(&cfg) * scale_factor;
        let (floor_w, floor_h) = logical_well_minimum(&viewport);
        cabinet.set_well_minimum(floor_w, floor_h);

        let gpu = match Gpu::new(instance, Arc::clone(&window), frame_stats_enabled) {
            Ok(g) => {
                log::info!("wgpu adapter {} on {}", g.adapter_name, g.backend);
                Some(g)
            }
            Err(e) => {
                log::error!("{e}");
                None
            }
        };
        let glass = gpu
            .as_ref()
            .and_then(|gpu| Glass::new(gpu, &cfg, &viewport, &crate::identity()));

        let mut surface = Self::assemble(
            Some(window),
            gpu,
            session,
            viewport,
            window_size,
            Some(cabinet),
            ssh.is_none(),
        );
        surface.glass = glass;
        // The set comes up on the connection instead of a shell; the tube
        // arms after it, the law `Channels::start` applies to channel 1.
        if let Some(req) = ssh {
            surface.connect_ssh(req);
            surface.channels.started();
            surface.on_air = surface.channels.on_air();
        }
        surface
    }

    /// A surface with no window and no GPU: a session, a geometry, and
    /// everything the pointer path touches.
    ///
    /// The pointer path reads the terminal and the grid arithmetic and
    /// nothing else: the window only ever tells it how big it is. So
    /// this is what lets a press-drag-release be an ordinary `cargo
    /// test`, the same way a scripted PTY transcript already is one crate
    /// down.
    ///
    /// It stands in no chassis: the viewport it is given is the whole of it,
    /// well and window alike. [`TerminalSurface::set_cabinet`] is how a test
    /// that wants one asks, and it is the only way a headless surface gets one.
    pub fn headless(session: &SessionConfig, viewport: Viewport) -> Self {
        let window_size = (viewport.width, viewport.height);
        Self::assemble(None, None, session, viewport, window_size, None, true)
    }

    fn assemble(
        window: Option<Arc<Window>>,
        gpu: Option<Gpu>,
        session: &SessionConfig,
        viewport: Viewport,
        window_size: (u32, u32),
        cabinet: Option<Cabinet>,
        start_home: bool,
    ) -> Self {
        let columns = viewport.term_size().cols();
        let window_has_display = window.is_some();
        // The set comes up on channel 1, and the tube is armed only after
        // it, so the first channel is not a channel change and nothing
        // flinches.
        let mut channels = Channels::new();
        let size = viewport.term_size();
        if start_home {
            channels.start(|| spawn(session, size));
        }
        let on_air = channels.on_air();
        Self {
            window,
            gpu,
            channels,
            logged_clip: None,
            monospace_trigger: None,
            banks: HashMap::new(),
            picker: None,
            find: None,
            sessions: HashMap::new(),
            on_air,
            session_config: session.clone(),
            pager: BankPager::new(),
            chord: ChordInput::new(),
            chord_modifier: false,
            degauss: Degauss::new(),
            critters: shipped_critters(),
            viewport,
            window_size,
            cabinet,
            cabinet_cfg: Config::default(),
            seam_press: false,
            pending_led_characters: None,
            seam_cursor: false,
            pointer_shape: CursorIcon::Default,
            shell_events: None,
            bindings: crate::bindings::Bindings::shipped(),
            binding_rows: Vec::new(),
            ime: ImeState::default(),
            last_find: String::new(),
            eof: false,
            scroll: ScrollPosition::default(),
            // The default model until a press reads the settings: the
            // settings handle is attached after this returns, and nothing is
            // on the glass to mark yet.
            selection: SelectionModel::new(Kind::Konsole, columns),
            pointer_cell: (0, 0),
            pointer_present: false,
            links: UrlFilterChain::with_url_filter(),
            hover: None,
            link_press: None,
            opener: if window_has_display {
                Opener::platform()
            } else {
                Opener::recording()
            },
            dragging: false,
            secondary_press: false,
            last_click: None,
            clipboard: if window_has_display {
                clipboard::Store::platform()
            } else {
                clipboard::Store::memory()
            },
            wheel_pixels: 0.0,
            settings: None,
            base: Config::default(),
            next_effects_frame: None,
            focused: true,
            last_input: Instant::now(),
            unattended: false,
            next_output_frame: None,
            output_pending: false,
            glass: None,
            size_badge: (String::new(), 0.0),
            notice: crate::overlay::Notice::default(),
            sheds_seen: (0, 0, 0),
            settings_app: settings::SettingsApp::default(),
        }
    }

    /// Drain every channel's PTY once. The shell calls [`Surface::tick`] for
    /// this; a test with no event loop calls it directly.
    ///
    /// Every channel and not only the one on the air: every channel keeps
    /// running while only one of them is shown, so a shell that prints while
    /// another channel is up has printed by the time you turn the knob back.
    /// Answers how many bytes the *visible* channel produced, since that is
    /// the one a redraw would show.
    pub fn pump(&mut self) -> usize {
        let current = self.channels.on_air();
        let mut visible_bytes = 0;
        let mut died: Vec<(BankId, u32)> = Vec::new();
        for row in self.channels.rows_mut() {
            let pumped = row.session.pump();
            if (row.bank, row.channel) == current {
                visible_bytes = pumped.bytes;
            }
            if pumped.eof {
                died.push((row.bank, row.channel));
            }
            // On bank 0, the one this program manages, a shell's title is its
            // own and rio-vt keeps whatever the OSC set; on an attachment the
            // title is tmux's to give.
            if row.bank == 0 {
                let title = row.session.term().title.clone();
                if title != row.title {
                    row.title = title.trim().to_string();
                }
            }
        }
        // A channel whose session finished tells the model, and the model
        // decides whether that ends the appliance.
        for (bank, channel) in died {
            log::info!("channel {channel} on bank {bank} exited");
            if channel == 1 && self.is_tmux(bank) {
                // A gateway's transport died under it; `session_died` is about
                // to collapse the bank (`gateway_died`), and a client with no
                // channel under it has no wire. No client half at all means a
                // tmux this surface started that never opened its envelope.
                let had = self
                    .banks
                    .get_mut(&bank)
                    .and_then(|runtime| runtime.gateway.take())
                    .is_some();
                if !had {
                    log::warn!("tmux: bank {bank}'s client died before the protocol opened");
                }
                self.forget_bank(bank, !had);
            }
            if self.channels.session_died(bank, channel) == Close::CloseWindow {
                self.eof = true;
            }
        }
        // A dead row may have been an SSH bank's last: the model swept the
        // bank, and what the bank ran on follows it here.
        self.sweep_bank_runtimes();
        // Then, and only then, what the connections are asking: the wire
        // events above carry the lines that explain the questions below.
        self.pump_asks();
        // A pane row's own `pump` above read nothing and could not: its bytes
        // arrive off the gateway's wire, drained here, after the loop that
        // counted. Counted only there, a tmux window on the air never asked for
        // a redraw at all, and a window whose effects are not running would
        // have shown nothing.
        visible_bytes += self.pump_gateways();
        self.channel_changed();
        self.watch_the_write_queues();
        // The grid's inset follows the live settings here as well as in
        // `apply_live_settings`, because a surface with no window never
        // redraws and would otherwise count its columns against a margin
        // the settings have moved off. The pointer reads that same grid.
        // A surface that attached no handle keeps the neutral inset, which
        // is the geometry it was built with.
        if let Some(handle) = self.settings.as_ref() {
            let cfg = handle.current();
            self.ensure_margin(&cfg);
        }
        visible_bytes
    }

    /// One record per bank the model still holds, and none for any it does
    /// not. The rule is the whole of when a [`BankRuntime`] exists, so it is
    /// stated once and applied in one place.
    ///
    /// Dropping the record hangs up the connection and cancels whatever
    /// question was standing on it, which is what releases the thread parked
    /// on the far side of it.
    fn sweep_bank_runtimes(&mut self) {
        self.banks
            .retain(|bank, _| self.channels.manager_of(*bank).is_some());
    }

    /// What the glass paints as marked: the find line's hit while it has
    /// one, and the pointer's selection otherwise.
    ///
    /// The find line wins because it is the thing the user is looking at:
    /// a search raised over an old selection is a new question, and two
    /// highlights at once would leave the answer to it unreadable.
    pub fn marked_range(&self) -> Option<term::MarkedRange> {
        let on = self.channels.on_air();
        self.find
            .as_ref()
            .filter(|find| find.on == on)
            .and_then(crate::find::Find::mark)
            .or_else(|| {
                self.channels
                    .session()
                    .and_then(|session| self.selection.range(session.term()))
            })
    }

    pub fn find_query(&self) -> Option<&str> {
        self.find.as_ref().map(crate::find::Find::query)
    }

    fn watch_the_write_queues(&mut self) {
        let mut pty: u64 = 0;
        let mut ssh: u64 = 0;
        for row in self.channels.rows_mut() {
            match &row.session {
                ChannelSession::Ssh(_) => ssh += row.session.sheds(),
                _ => pty += row.session.sheds(),
            }
        }
        let tmux: u64 = self
            .banks
            .values()
            .filter_map(|runtime| runtime.gateway.as_ref())
            .map(Gateway::sheds)
            .sum();
        let seen = self.sheds_seen;
        self.sheds_seen = (pty, tmux, ssh);
        if pty > seen.0 {
            self.notice.raise(SHED_PTY, Instant::now());
        } else if tmux > seen.1 {
            self.notice.raise(SHED_TMUX, Instant::now());
        } else if ssh > seen.2 {
            self.notice.raise(SHED_SSH, Instant::now());
        }
    }

    /// What the user is looking at, one row per entry.
    ///
    /// The viewport and not the live screen: scrolled back, these differ,
    /// and this is the one a renderer (or a test asking whether the
    /// screen is ready yet) wants.
    pub fn viewport_text(&self) -> Vec<String> {
        match self.channels.session() {
            Some(session) => term::viewport_text(session.term()),
            None => Vec::new(),
        }
    }

    /// The text of the last completed selection, if there was one: the
    /// primary selection's slot, which is where selecting puts it.
    pub fn last_selection(&self) -> Option<&str> {
        self.clipboard.last(clipboard::Target::Primary)
    }

    /// The link the pointer opened last, as the window's opener recorded it.
    pub fn last_opened(&self) -> Option<&str> {
        self.opener.last_opened()
    }

    /// The link under the pointer right now, the one underlined on the glass.
    pub fn hovered_link(&self) -> Option<&str> {
        self.hover.as_ref().map(|(_, url)| url.as_str())
    }

    /// This window's clipboard store, for a caller that wants to see which
    /// of the two selections a gesture wrote. A headless surface's store is
    /// the whole of what it has.
    pub fn clipboard_store(&self) -> &clipboard::Store {
        &self.clipboard
    }

    /// Lines the view is scrolled back above the bottom of the history.
    pub fn scroll_offset(&self) -> usize {
        self.scroll.offset()
    }

    /// A wheel notch's glide is under way: the view is on its way to the
    /// offset the notch asked for and moves on every frame until it arrives.
    pub fn is_gliding(&self) -> bool {
        self.scroll.is_gliding()
    }

    /// One thing the input method said, applied. The state is
    /// [`ImeState::apply`]'s; what a commit produced is typed at the child
    /// here, because only the surface has one.
    ///
    /// Public for the reason `key_input` is: winit's `KeyEvent` cannot be
    /// built outside winit, but `Ime` can, so this is the seam a test drives
    /// without a display server. [`Surface::ime`] is one line calling it.
    pub fn ime_input(&mut self, event: &Ime) {
        if let Some(bytes) = self.ime.apply(event) {
            self.type_bytes(&bytes);
        }
    }

    /// What the input method is composing. [`Self::draw_frame`] hands it to the
    /// glyph renderer every redraw; see [`TerminalSurface::ime_input`].
    pub fn ime_state(&self) -> &ImeState {
        &self.ime
    }

    /// Where the caret is, in this window's own physical pixels: the
    /// cursor's cell, mapped from grid coordinates into the window's own.
    ///
    /// The window shows the bent picture directly rather than a flat
    /// rendered image, so the cell is pushed through
    /// `term::distortion::forward_distort` -- the same map the pointer path
    /// inverts on every click -- and the bank column's width is added,
    /// because the well starts where the casting stops. The caret the
    /// candidate window is told about is therefore the caret the user can
    /// see, which is what the rectangle is for.
    ///
    /// `None` when there is no channel, or when the cursor is not on the visible
    /// screen at all (scrolled back into history): there is no caret on the
    /// glass to point at, and a rectangle from the wrong row would move the
    /// candidate window somewhere the user is not looking.
    pub fn ime_cursor_area(&self) -> Option<(PhysicalPosition<f64>, PhysicalSize<f64>)> {
        let session = self.channels.session()?;
        let cursor = session.term().cursor();
        // The caret's row on the viewport: its live row, moved down by the
        // lines the view is scrolled back, the same sum the renderer draws
        // it at.
        let row = cursor.pos.row.0 + session.term().grid.display_offset() as i32;
        if row < 0 {
            return None;
        }
        let (row, col) = (row as usize, cursor.pos.col.0);
        let size = self.viewport.term_size();
        if row >= size.rows() || col >= size.cols() {
            return None;
        }
        let (cell_w, cell_h) = (f64::from(size.cell_width), f64::from(size.cell_height));
        // The cell's rectangle in grid-texture pixels -- `cell_at`'s output
        // space -- forward through the warp into well pixels. The picture
        // is drawn shifted up by the position's fraction of a row, and so
        // is the caret.
        let params = self.distortion_params();
        let x = col as f64 * cell_w;
        let y = row as f64 * cell_h - f64::from(self.shift_physical());
        let top_left = distortion::forward_distort(x, y, &params);
        let bottom_right = distortion::forward_distort(x + cell_w, y + cell_h, &params);
        let bank = f64::from(self.bank_physical());
        Some((
            PhysicalPosition::new(top_left.x + bank, top_left.y),
            PhysicalSize::new(
                (bottom_right.x - top_left.x).max(1.0),
                (bottom_right.y - top_left.y).max(1.0),
            ),
        ))
    }

    /// Tell the platform where the caret is, so the candidate window follows
    /// it. [`ImeState::publish`] does the telling; the rectangle is this
    /// surface's, off [`Self::ime_cursor_area`].
    ///
    /// Called once per turn of the loop rather than from [`Self::ime_input`],
    /// because the caret moves for reasons the input method never hears
    /// about: the shell echoing a character, a program repainting, a resize.
    ///
    /// Only while an input method has the keyboard, because only then is
    /// anyone reading the answer, and deriving the rectangle costs a settings
    /// snapshot. The first composition after the method takes the keyboard is
    /// one loop turn away, which is under ten milliseconds and before any
    /// candidate window is up.
    fn publish_ime_cursor(&mut self) {
        if !self.ime.enabled {
            return;
        }
        let Some(window) = self.window.clone() else {
            return;
        };
        let Some(area) = self.ime_cursor_area() else {
            return;
        };
        self.ime.publish(&window, area);
    }

    /// What the appliance is saying on its own behalf right now, if anything.
    ///
    /// Drawing the badge needs a device and a frame; this is the state under
    /// it, so a test with neither can read what the user would have seen.
    pub fn notice(&self) -> &crate::overlay::Notice {
        &self.notice
    }

    /// [`NO_SIGNAL`] while the air stands on a bank with nothing to put on the
    /// glass. The frame draws what this says, so a test with no device can ask.
    pub fn no_signal(&self) -> Option<[&'static str; 2]> {
        self.channels.session().is_none().then_some(NO_SIGNAL)
    }

    /// The channel state itself, for a test or for whoever mounts the bank.
    pub fn channels(&self) -> &Channels<AppSession> {
        &self.channels
    }

    /// What the degauss transient is doing at `now`. The frame samples this
    /// itself; this is how a surface with no GPU can be asked.
    pub fn degauss_state(&mut self, now: Instant) -> crt::DegaussState {
        self.degauss.sample(now)
    }

    /// The settings this frame is drawn from.
    ///
    /// The handle if there is one, because a watched file outranks anything
    /// resolved once at startup; otherwise the config the run was launched
    /// with ([`TerminalSurface::set_config`]), which under
    /// `--default-settings --profile <name>` is the named look.
    fn live_config(&self) -> Config {
        match self.settings.as_ref() {
            Some(handle) => handle.current(),
            None => self.base.clone(),
        }
    }

    /// The config this run resolved before any window existed: the CLI's
    /// `--profile` overlay on whatever the flags left standing.
    ///
    /// Only consulted when no settings handle is attached. With one, the
    /// watched file is the authority and this is never read, so `main` can
    /// set it either way without having to know which it built.
    pub fn set_config(&mut self, config: Config) {
        self.base = config;
        let cfg = self.live_config();
        self.refresh_bindings(&cfg);
    }

    /// Rebuild the chords when the `[bindings]` rows moved, and only then:
    /// the lookup runs on every keystroke and the build on a change.
    fn refresh_bindings(&mut self, cfg: &Config) {
        if cfg.bindings.keys != self.binding_rows {
            self.bindings = crate::bindings::Bindings::build(&cfg.bindings.keys);
            self.binding_rows = cfg.bindings.keys.clone();
        }
    }

    /// Whether a channel opened now measures text by grapheme cluster
    /// (mode 2027), as the settings stand at this moment.
    ///
    /// Read here rather than carried from startup, so an edit in the
    /// settings window reaches the next channel opened without a restart.
    /// A channel already on the air keeps the policy it was built with,
    /// because its screen was drawn under that policy and the program
    /// filling it counted columns the same way.
    fn grapheme_clustering(&self) -> bool {
        self.live_config().general.grapheme_clustering
    }

    /// The speed a local shell's output is taken at, as the settings stand.
    /// `None` is as fast as the child writes, which is the shipped state.
    fn serial_rate(&self) -> Option<u32> {
        self.live_config().serial.rate()
    }

    /// Which selection model the pointer follows, as the settings stand at
    /// this moment. Read at the start of a gesture rather than carried from
    /// startup, so an edit in the settings window reaches the next drag.
    fn selection_kind(&self) -> Kind {
        match self.live_config().general.selection_model {
            config::schema::SelectionModel::Konsole => Kind::Konsole,
            config::schema::SelectionModel::Rio => Kind::Rio,
        }
    }

    /// Put the model the settings now name in place. The two models hold
    /// their state in different shapes and in different coordinates, so the
    /// one going out takes its marks with it and the gesture about to start
    /// begins on empty glass.
    fn retarget_selection(&mut self, kind: Kind) {
        if self.selection.kind() == kind {
            return;
        }
        let columns = self.selection.columns();
        self.selection.clear();
        self.selection = SelectionModel::new(kind, columns);
    }

    /// The session a channel opened now should run: this process's, with
    /// the width policy taken from the settings as they stand.
    fn session_now(&self) -> SessionConfig {
        SessionConfig {
            grapheme_clustering: self.grapheme_clustering(),
            rate: self.serial_rate(),
            ..self.session_config.clone()
        }
    }

    /// Attach the live settings handle the pointer's inverse-distortion
    /// transform reads on every press/release/move.
    /// `SettingsHandle` is a thin, cheaply-`Arc`-cloned wrapper over
    /// `config::watch::ConfigWatcher`, so `handle.current()` always
    /// answers with whatever the watcher's last successful reload
    /// published -- a settings edit therefore changes the pointer mapping
    /// without restarting the process, with no polling or callback wiring
    /// needed here.
    pub fn set_settings(&mut self, settings: Arc<SettingsHandle>) {
        let cfg = settings.current();
        self.settings = Some(settings);
        // The cabinet was measured for the defaults in `new`, because the
        // handle did not exist yet. Take the real profile now rather than on
        // the first redraw, so the first frame is already the right shape.
        self.apply_cabinet_settings(&cfg);
        self.refresh_bindings(&cfg);
    }

    /// The programmatic accessor: the live-preview throughput
    /// instrument's rolling p50/p99s, or `None` on a surface with no GPU
    /// (headless, or one whose `Gpu::new` failed). Reads `--frame-stats`'s
    /// same numbers the periodic log line prints, without scraping a log.
    pub fn frame_stats(&self) -> Option<crate::frame_stats::Stats> {
        self.gpu.as_ref().map(Gpu::frame_stats)
    }

    /// How this surface reaches its shell.
    ///
    /// One event travels this way today: the bank's width, which is the
    /// window's minimum-width hint and therefore the shell's to apply. A
    /// surface with no proxy (every headless one) still drags its seam; the
    /// hint is simply not a thing it has.
    pub fn set_shell_events(&mut self, proxy: EventLoopProxy<ShellEvent>) {
        self.shell_events = Some(proxy);
    }

    /// Stand this surface in a cabinet.
    ///
    /// [`TerminalSurface::new`] builds one from the profile; this is for a
    /// headless surface, which otherwise has none at all. Either way the well
    /// is re-divided on the spot and the shell is told the new bank width.
    pub fn set_cabinet(&mut self, cabinet: Cabinet) {
        self.cabinet = Some(cabinet);
        self.relayout();
        self.announce_bank_width();
    }

    /// The cabinet, for a test measuring what a drag landed.
    pub fn cabinet(&self) -> Option<&Cabinet> {
        self.cabinet.as_ref()
    }

    // ---- the window's division ---------------------------------------

    /// The bank column's width in physical pixels: zero with no cabinet, and
    /// never so wide that the well has no pixels left (a window manager is
    /// free to ignore the minimum-size hint).
    fn bank_physical(&self) -> u32 {
        match self.cabinet.as_ref() {
            Some(cabinet) => bank_physical(cabinet, self.viewport.scale_factor, self.window_size.0),
            None => 0,
        }
    }

    /// Re-divide the window into the bank column and the screen well, and
    /// carry the well's new size through to the session, the surface and the
    /// chain's target.
    fn relayout(&mut self) {
        let scale = self.viewport.scale_factor.max(f64::EPSILON);
        let (window_w, window_h) = self.window_size;
        if let Some(cabinet) = self.cabinet.as_mut() {
            cabinet.resized(f64::from(window_w) / scale, f64::from(window_h) / scale);
        }
        self.viewport.height = window_h.max(1);
        // The margin is physical (see `Viewport::margin`), so a DPR change
        // moves it even with the settings themselves untouched -- the same
        // reason `relayout` is what `scale_factor_changed` calls.
        let cfg = self.live_config();
        self.ensure_margin(&cfg);
        // Floor before boundary: the bank's width is a function of the floor
        // (`apply_live_settings` says why). Ordered so this reads only the
        // cell and the margin, settled above, and nothing it is about to
        // write.
        self.settle_well_minimum();
        let bank = self.bank_physical();
        self.viewport.width = window_w.saturating_sub(bank).max(1);
        self.sync_geometry();
        self.settle_rows();
    }

    /// Hand the cabinet the well's floor as this window's font now measures
    /// it, so the seam drag stops where the window's own hint stops.
    fn settle_well_minimum(&mut self) {
        let (width, height) = logical_well_minimum(&self.viewport);
        if let Some(cabinet) = self.cabinet.as_mut() {
            cabinet.set_well_minimum(width, height);
        }
    }

    /// How many rows the bank shows, measured off the window it now stands
    /// in.
    ///
    /// This runs on every resize with no debounce: it is arithmetic over a
    /// `Vec`, and running it on the spot costs nothing. The pager's
    /// footprint comes off the foot first (`Cabinet::pager_height`); the
    /// height it divides is the window's, in logical pixels, because that is
    /// what the bank column actually stands in.
    fn settle_rows(&mut self) {
        let Some(cabinet) = self.cabinet.as_ref().filter(|c| c.is_shown()) else {
            return;
        };
        let geometry = *cabinet.geometry();
        let pager_height = cabinet.pager_height();
        let height = (f64::from(self.window_size.1) / self.viewport.scale_factor.max(f64::EPSILON))
            .round() as i32;
        self.pager
            .settle(&geometry, height, pager_height, &self.channels);
        self.settle_bank();
    }

    /// Tell the shell what the bank now measures, and what it measures at its
    /// narrowest: the first sizes a window opened next, the second is the
    /// minimum-size hint's own term.
    fn announce_bank_width(&self) {
        let (Some(proxy), Some(cabinet)) = (self.shell_events.as_ref(), self.cabinet.as_ref())
        else {
            return;
        };
        if proxy
            .send_event(ShellEvent::SetBankWidth {
                width: cabinet.bank_width(),
                minimum: cabinet.min_bank_width(),
            })
            .is_err()
        {
            log::debug!("the shell is gone; the bank width has nowhere to go");
        }
    }

    /// Take a profile the cabinet has not seen. Re-measuring reads the lamp
    /// font, so it happens on a change and not on a frame.
    fn apply_cabinet_settings(&mut self, cfg: &Config) {
        if self.cabinet.is_none() || *cfg == self.cabinet_cfg {
            return;
        }
        self.cabinet_cfg = cfg.clone();
        if let Some(cabinet) = self.cabinet.as_mut() {
            cabinet.apply_config(cfg);
        }
        self.relayout();
        self.announce_bank_width();
    }

    // ---- the redraw path --------------------------------------------

    /// Take whatever the settings say now, before anything is recorded.
    ///
    /// Two things can move, and they are not the same thing:
    ///
    /// * the *font*, which resizes the cell and therefore the grid, so the
    ///   atlas is rebuilt and the session resized;
    /// * everything else, which the chain takes itself, deciding between a
    ///   uniform push and a rebuild by comparing the `Structure` it derives
    ///   from each `Config` (`crt::chain`). `app::settings::classify` names
    ///   the same split but is informational only.
    ///
    /// This is a poll of the handle rather than the settings *callback* an
    /// earlier sketch named, and deliberately: the callback runs on the
    /// watcher's own thread, and the chain, the device and the atlas are the
    /// event loop's. `SettingsHandle::current()` is the published snapshot,
    /// so polling it once per frame sees every edit exactly as the callback
    /// would, on the thread that is allowed to act on it.
    fn apply_live_settings(&mut self) {
        let cfg = self.live_config();
        self.refresh_bindings(&cfg);
        self.critters.configure(
            cfg.critters.enabled,
            critter_interval(cfg.critters.minutes),
            cfg.critters.timing == CritterTiming::Random,
            critter_cast(&cfg.critters),
        );

        // Every channel and not only the one on the air: an unattended
        // channel is still reading its shell, and a rate that reached only
        // the visible one would be a different terminal depending on which
        // knob position it was left at.
        let rate = cfg.serial.rate();
        for row in self.channels.rows_mut() {
            row.session.set_rate(rate);
        }

        // The cabinet first: it decides how much of the window is glass, and
        // the font sizing below is measured against the glass.
        self.apply_cabinet_settings(&cfg);

        self.ensure_monospace_trigger(&cfg);
        let refonted = self.ensure_font(&cfg);
        let remargined = self.ensure_margin(&cfg);
        if refonted || remargined {
            // Both of them move the floor: the cell is what the eighty
            // columns are counted in, and the margin is what is taken off
            // the well before they are counted.
            // `relayout` rather than the floor and the geometry alone: the
            // bank's width is a function of the well's floor, so a font that
            // moves the floor moves the boundary the well is measured from.
            // Settling the floor without re-dividing the window leaves the
            // well the width it had against the old boundary, and the chain
            // then draws a picture of that width at the new boundary's
            // offset, off the right edge of the swapchain.
            self.relayout();
        }

        let well = (self.viewport.width.max(1), self.viewport.height.max(1));
        if let (Some(gpu), Some(glass)) = (self.gpu.as_ref(), self.glass.as_mut()) {
            if cfg != glass.applied {
                match glass.chain.apply_settings(&gpu.device, &gpu.queue, &cfg) {
                    Ok(applied) => log::debug!("settings reached the chain: {applied:?}"),
                    Err(e) => log::error!("could not apply settings to the chain: {e}"),
                }
                glass.applied = cfg;
            }

            // The two discontinuities the settings cannot express, which
            // `crt::burn_in`'s mount contract leaves to the application: a
            // resized target and a rebuilt atlas both mean the accumulator's
            // contents describe a picture that is no longer on screen, and a
            // ghost of the old one would otherwise decay in place over it.
            //
            // The target is the *well*, so a seam drag resizes it too: the
            // glass really is narrower afterwards, and a target still sized to
            // the old well would stretch the picture into the new one.
            let (width, height) = well;
            if (glass.target.width, glass.target.height) != (width, height) {
                log::debug!(
                    "target {}x{} -> {width}x{height}",
                    glass.target.width,
                    glass.target.height
                );
                glass.target = Target::new(&gpu.device, width, height, gpu::TARGET_FORMAT);
                glass.chain.burn_in().restart();
            }
            if refonted {
                glass.chain.burn_in().restart();
            }
        }
    }

    /// Recompute the well's grid inset from the live settings and this
    /// window's DPR, and say whether it moved.
    ///
    /// `settings::distortion_margin` is logical; [`Viewport::margin`] is
    /// physical, so it is scaled by `scale_factor` here, the same boundary
    /// [`Viewport::physical_cell`] crosses for the cell. This is the only
    /// place that reads `distortion_margin`: the pointer path picks the
    /// result up from [`Viewport::margin`] instead, by way of
    /// [`Viewport::term_size`].
    /// Hand the renderer the profile's `monospace_trigger` when it has
    /// changed. A pattern that will not compile is refused in the log and
    /// the one in force stays, so a half-typed edit in the settings file
    /// does not flip every row while it is being typed.
    fn ensure_monospace_trigger(&mut self, cfg: &Config) {
        let wanted = &cfg.screen.monospace_trigger;
        if self.monospace_trigger.as_deref() == Some(wanted.as_str()) {
            return;
        }
        // Recorded only once a renderer has taken it: a surface with no glass
        // yet applies it on the first pass that has one.
        let Some(glass) = self.glass.as_mut() else { return };
        match regex::Regex::new(wanted) {
            Ok(trigger) => {
                glass.renderer.set_monospace_trigger(trigger);
                self.monospace_trigger = Some(wanted.clone());
            }
            Err(e) => log::error!("screen.monospace_trigger {wanted:?} is not a pattern: {e}"),
        }
    }

    fn ensure_margin(&mut self, cfg: &Config) -> bool {
        let margin = settings::distortion_margin(cfg) * self.viewport.scale_factor;
        if (margin - self.viewport.margin).abs() > f64::EPSILON {
            self.viewport.margin = margin;
            true
        } else {
            false
        }
    }

    /// Rebuild the atlas if the font the settings ask for is not the font the
    /// atlas holds. Returns whether anything moved.
    ///
    /// The comparison is on the *resolved* font and not on the setting: two
    /// different `font_scaling` values that floor to the same integer scale
    /// rasterise the same atlas, and rebuilding it for them would throw the
    /// burn-in ghost away for no visible reason.
    fn ensure_font(&mut self, cfg: &Config) -> bool {
        let entry = font_entry(cfg);
        let request = sizing_request(cfg, self.viewport.scale_factor);
        let resolved = term::resolve(entry, &request, ScalePolicy::Floor);

        match self.glass.as_ref() {
            Some(glass) if glass.font_name == entry.name && glass.resolved == resolved => {
                return false
            }
            // No glass means no atlas to rebuild and no cell to move: a
            // headless surface takes its viewport from whoever built it.
            None => return false,
            Some(_) => {}
        }

        self.viewport.cell = logical_cell(
            FontContext::new(entry).cell_metrics(&resolved),
            &resolved,
            self.viewport.scale_factor,
        );
        if let (Some(gpu), Some(glass)) = (self.gpu.as_ref(), self.glass.as_mut()) {
            let (_, font, atlas) =
                term::build_font(&gpu.device, &gpu.queue, entry, &request, ScalePolicy::Floor);
            glass.renderer.set_scale(resolved.integer_scale);
            glass.renderer.set_atlas(&gpu.device, &gpu.queue, atlas);
            glass.font = font;
            glass.font_name = entry.name.to_string();
            glass.resolved = resolved;
            log::info!(
                "font changed to {} at {}px x{}",
                glass.font_name,
                glass.resolved.raster_pixel_size,
                glass.resolved.integer_scale
            );
        }
        true
    }

    /// Record and present one frame.
    fn draw_frame(&mut self) {
        // The window's division, taken before the glass is borrowed: the well
        // is where the chain's picture goes and the column is what the casting
        // is drawn into, and a hidden chassis makes the second of them empty.
        let bank = self.bank_physical();
        let shown = self.cabinet.as_ref().filter(|cabinet| cabinet.is_shown());
        let column_params = shown.map(|cabinet| cabinet.chassis_params());
        // What stands on the casting: the real rows of the real channel model,
        // exactly what `bank_strips` hands the keyboard and the hit test.
        let strips = self.bank_strips();
        let column_pieces = shown
            .map(|cabinet| cabinet.furniture(&strips))
            .unwrap_or_default();
        // What the pointer has marked, taken before the glass is borrowed for
        // the same reason the division above is: `top_line` reads the session
        // and the renderer is about to be held mutably. A selection that has
        // been cleared, or was never made, leaves this `None` and the glass
        // shows no highlight at all.
        let top_line = self.top_line();
        let no_signal = self.no_signal();
        let marked = self
            .marked_range()
            .map(|range| Marked { range, top_line });
        // And the link under the pointer, on the same terms.
        let link = self
            .hover
            .as_ref()
            .map(|(range, _)| Marked { range: *range, top_line });

        let Some(gpu) = self.gpu.as_mut() else { return };
        let Some(mut frame) = gpu.acquire() else {
            return;
        };

        let (glass, session) = match (self.glass.as_mut(), self.channels.session_mut()) {
            (Some(glass), Some(session)) => (glass, session),
            // A window with no chain or no child still has to put something on
            // the screen, or the compositor shows whatever was there before.
            (glass, _) => {
                // No glass means no marks reach the query set, so timing
                // this frame is withdrawn rather than resolved half-written.
                frame.discard_timing();
                frame.clear();
                // The bank the user paged to is still theirs to look at, so the
                // casting and the message go on through the badge pass.
                if let (Some(glass), Some(message)) = (glass, no_signal) {
                    let (width, height) = (glass.target.width, glass.target.height);
                    let entries = message.map(|text| crate::chrome::Entry {
                        text,
                        opacity: crate::overlay::NOTICE_OPACITY,
                    });
                    glass.chrome.render(
                        &gpu.device,
                        &gpu.queue,
                        &mut frame.encoder,
                        &frame.view,
                        (bank + width, height),
                        (bank, height),
                        (width, height),
                        self.viewport.scale_factor,
                        column_params.as_ref(),
                        &column_pieces,
                        Some(crate::chrome::Badges {
                            atlas: glass.renderer.atlas(),
                            scale: glass.resolved.integer_scale,
                            entries: &entries,
                        }),
                    );
                }
                gpu.present(frame);
                return;
            }
        };

        let scale_factor = self.viewport.scale_factor;

        // One instant for the whole frame: the chain's clock and the degauss
        // transient are two readings of the same moment.
        let now = Instant::now();
        let time = glass.pacing.tick(now);
        let degauss = self.degauss.sample(now);

        glass.renderer.sync(
            &gpu.device,
            &gpu.queue,
            &mut glass.font,
            session.term_mut(),
            &mut self.scroll,
            marked.as_ref(),
            link.as_ref(),
        );
        // What the input method is composing goes into the grid, at the
        // cursor, before the grid is drawn, so it runs through the CRT chain
        // with the rest of the tube rather than floating flat over it
        // (`term::render::GridRenderer::set_preedit`).
        glass.renderer.set_preedit(&gpu.queue, &self.ime.preedit);

        // The critter, off the same instant as the chain's clock above, and
        // painted into the cells for the reason the pre-edit just was: in the
        // grid it wears the phosphor and bends with the tube. The renderer
        // rebuilds only the rows it arrived on or left, so the frames where
        // it has not moved, which is most of them, cost nothing.
        let (cols, rows) = (glass.renderer.cols(), glass.renderer.rows());
        if self.unattended {
            self.critters.withdraw();
        } else {
            self.critters.tick(now, cols, rows);
        }
        glass.renderer.set_critter(
            &gpu.device,
            &gpu.queue,
            &mut glass.font,
            self.critters.cells(),
        );

        // The grid is centred in the target at a whole-pixel origin: the
        // remainder of dividing the window by the cell is padding, and half a
        // pixel of it would put every glyph between two of them.
        let (target_width, target_height) = (glass.target.width, glass.target.height);
        let (grid_width, grid_height) = glass.renderer.pixel_size();
        let geometry = (target_width, target_height, grid_width, grid_height);
        if glass.logged_geometry != Some(geometry) {
            glass.logged_geometry = Some(geometry);
            log::debug!(
                "frame {}: {}x{} cells, grid {grid_width}x{grid_height} centred in \
                 target {target_width}x{target_height}",
                time.index,
                glass.renderer.cols(),
                glass.renderer.rows(),
            );
        }
        // A position between two lines draws the picture that fraction of a
        // row up from there (`term::viewport`), in whole raster pixels.
        let shift = (self.scroll.shift() * glass.renderer.atlas().cell.height as f32).round();
        glass.renderer.set_origin(
            (target_width as i32 - grid_width as i32).max(0) / 2,
            (target_height as i32 - grid_height as i32).max(0) / 2,
            shift as i32,
        );

        let mut params =
            Params::build(&glass.applied, &glass.geometry(scale_factor), time, degauss);
        // Every shader that reads `time` sees a value held for
        // `effectsFrameSkip` frames and then jumped, not a continuous clock,
        // so the effects run at 60/skip Hz -- 20 Hz at the shipped skip of 3
        // -- and not once per frame drawn. `EFFECTS_BASE_FRAME` is the same
        // 60 Hz assumption the redraw scheduler already makes, and its doc
        // says what to do if a real display session disagrees.
        //
        // Only the `Time` uniform. `FrameTime::now` stays continuous, because
        // the burn-in decay is measured in real seconds between real frames,
        // and quantising that would fade the ghost in steps.
        let step = EFFECTS_BASE_FRAME.as_secs_f32()
            * glass.applied.general.effects_frame_skip.max(1) as f32;
        params.set("Time", (time.elapsed / step).floor() * step);
        glass.chain.set_params(&params);

        // One encoder, three recordings: the grid into the offscreen target,
        // the chain from that target into the well's rectangle of the swapchain
        // image, and the bank column's casting over the rest. The clear
        // is opaque black because the bloom pass reads the alpha of what it
        // blurs as its own mask, and a transparent surround would mask the
        // glow away at the edges.
        //
        // The frame-stats timing marks straddle each recording plus the
        // frame as a whole; `Frame::mark` is a no-op when this frame is not being
        // GPU-timed, so every call site here is unconditional. The column's
        // marks bracket its `if` rather than sitting inside it, so the query
        // set is always eight-for-eight written even with the chassis
        // hidden -- a hidden-column frame legitimately measures ~0 there.
        // The swapchain image is the authority on what may be drawn into it,
        // so the two rectangles laid onto it below are clipped to it here.
        // They are computed from the window's division into bank and well,
        // and a division that disagrees with the image by a pixel used to
        // reach wgpu as a validation error, which is a panic and takes the
        // process with it. Clipped, the same disagreement costs a stripe of
        // one frame and a line in the log, and the terminal stays on the
        // screen to be read.
        let (surface_width, surface_height) = gpu.size();
        let bank = bank.min(surface_width);
        let well = (
            target_width.min(surface_width - bank),
            target_height.min(surface_height),
        );
        if well != (target_width, target_height) {
            let clip = (bank, target_width, target_height, surface_width, surface_height);
            if self.logged_clip != Some(clip) {
                self.logged_clip = Some(clip);
                log::error!(
                    "the well does not fit the window it is drawn in: \
                     bank {bank} plus well {target_width}x{target_height} \
                     against a {surface_width}x{surface_height} surface; \
                     the frame is clipped to {}x{}",
                    well.0,
                    well.1,
                );
            }
        }

        frame.mark(Mark::FrameStart);
        frame.mark(Mark::GridStart);
        glass.renderer.draw(
            &gpu.queue,
            &mut frame.encoder,
            &glass.target.view,
            target_width,
            target_height,
            wgpu::LoadOp::Clear(wgpu::Color {
                r: 0.0,
                g: 0.0,
                b: 0.0,
                a: 1.0,
            }),
        );
        frame.mark(Mark::GridEnd);
        frame.mark(Mark::ChainStart);
        if let Err(e) = glass.chain.frame_at(
            &glass.target.texture,
            &frame.view,
            (bank, 0),
            well,
            gpu.format(),
            &mut frame.encoder,
            time,
        ) {
            log::error!("could not record a chain frame: {e}");
        }
        frame.mark(Mark::ChainEnd);
        // The casting is chrome, so it goes on afterwards rather than
        // through. It also has to: the chain's last pass cleared this whole
        // image before drawing its rectangle of it, so what stands to the left
        // of the well right now is transparent black.
        //
        // The two rectangles the chrome takes are in different rulers on
        // purpose: the column is physical because it is a scissor, and the
        // well reaches the chrome physical and is put on the casting's
        // logical ruler there.
        //
        // The size badge rides along, over the glass and over the casting
        // both, drawn topmost. It is centred in the *well*, so it starts
        // where the column stops. `opacity` gates it: the shell's overlay
        // reports zero whenever the badge is not up, and the mount draws
        // nothing rather than a transparent quad.
        //
        // `showTerminalSize` is gated here and not at the state machine,
        // because it is live-reloadable and this is the side of the seam that
        // holds live settings: the shell reads the setting once, when it builds
        // a window, while `glass.applied` is whatever the last config edit
        // said. This stops the drawing itself rather than the timer that
        // drives it.
        //
        // Below it, in the stack's second slot, whatever the appliance itself
        // has to say this moment (`crate::overlay::Notice`): a write queue
        // that shed, a look that was saved. This is not gated by
        // `showTerminalSize` either -- the setting is about the resize
        // badge, and hiding a loss behind a cosmetic switch would be a
        // second way to lose the news.
        frame.mark(Mark::ChromeStart);
        let window = (bank + well.0, well.1);
        let badge_opacity = if glass.applied.general.show_terminal_size {
            self.size_badge.1
        } else {
            0.0
        };
        let entries = [
            crate::chrome::Entry {
                text: &self.size_badge.0,
                opacity: badge_opacity,
            },
            crate::chrome::Entry {
                text: self.notice.text(),
                opacity: self.notice.opacity_at(now),
            },
        ];
        glass.chrome.render(
            &gpu.device,
            &gpu.queue,
            &mut frame.encoder,
            &frame.view,
            window,
            (bank, well.1),
            well,
            scale_factor,
            column_params.as_ref(),
            &column_pieces,
            Some(crate::chrome::Badges {
                atlas: glass.renderer.atlas(),
                scale: glass.resolved.integer_scale,
                entries: &entries,
            }),
        );
        frame.mark(Mark::ChromeEnd);
        frame.mark(Mark::FrameEnd);
        gpu.present(frame);
    }

    fn sync_geometry(&mut self) {
        let size = self.viewport.term_size();
        // A selection is a linear index over a grid of a given width, so
        // it means something else at a new one. Konsole cleared it on
        // resize for exactly that reason, and `set_columns` does.
        if self.selection.columns() != size.cols() {
            self.selection.set_columns(size.cols());
        }
        // Every channel, not only the one on the air. There is one rectangle
        // of glass in the appliance and every channel is laid into it, so a
        // channel resized only when it came back to the screen would redraw
        // its whole screen at the moment it was looked at -- resizing every
        // channel up front avoids exactly that flicker.
        for session in self.channels.sessions_mut() {
            if let Err(e) = session.resize(size) {
                log::error!("could not resize the pty: {e}");
            }
        }
        // The picker's page is bytes already parsed, so a reflow leaves it
        // ragged: paint it again at the new geometry.
        self.paint_picker();
        // The client-size law's resize half: the glass's grid to every
        // gateway, debounced by the gateway itself against the drag's burst.
        self.set_client_size();
        if let Some(gpu) = self.gpu.as_mut() {
            // The swapchain is the whole window; only the chain's picture is
            // the well.
            gpu.resize(self.window_size.0, self.window_size.1);
        }
    }

    /// Send bytes to the channel on the air. Public because a paste, a mouse
    /// report and a test's scripted input all mean the same thing to the child.
    pub fn write(&mut self, bytes: &[u8]) {
        // The gateway swallows every byte, and that is deliberate rather than
        // a gap in this path. On frozen glass every byte-producing path is
        // suppressed one by one: the middle button's paste is a paste like
        // any other and inert on a gateway, and the pointer is handed a
        // Shift it did not press so a drag marks the screen instead of
        // reporting to a program -- nothing the mouse does is written to the
        // control-mode wire. What is left is the keyboard. Every key the
        // emulation would encode is held by [`Self::gateway_key`], and the
        // one key with a meaning goes out as `detach-client`; the shortcut
        // layer runs earlier still, so `Ctrl+Shift+V` does arrive here, a
        // paste like the middle button's and swallowed on the same terms.
        //
        // So this is the same swallow, for the same reason. Its teeth are
        // protocol hygiene: the gateway's pty is the control wire,
        // where tmux reads every line as a command and answers it with a block
        // the codec never asked for: one stray byte desyncs the pairing queue
        // for good.
        if self.is_gateway_on_air() {
            return;
        }
        if let Some(session) = self.channels.session_mut() {
            if let Err(e) = session.write(bytes) {
                log::error!("could not write to the pty: {e}");
            }
        }
    }

    /// The [`selection::Gesture`] is assembled at each call site rather than
    /// held, because the window is read off `self` and the grid is borrowed
    /// out of the channel model.
    fn begin_selection(&mut self, cell: (usize, usize), side: Side, mods: term::pointer::Modifiers) {
        let win = self.selection_window();
        let Some(session) = self.channels.session_mut() else {
            return;
        };
        let gesture = Gesture {
            term: session.term_mut(),
            win,
            side,
        };
        self.selection.press(gesture, cell, mods);
    }

    fn drag_selection_to(&mut self, cell: (usize, usize), side: Side) {
        let win = self.selection_window();
        let Some(session) = self.channels.session_mut() else {
            return;
        };
        let gesture = Gesture {
            term: session.term_mut(),
            win,
            side,
        };
        self.selection.drag_to(gesture, cell);
    }

    fn select_word_at(&mut self, cell: (usize, usize), side: Side) -> Option<String> {
        let win = self.selection_window();
        let session = self.channels.session_mut()?;
        let gesture = Gesture {
            term: session.term_mut(),
            win,
            side,
        };
        self.selection.double_click(gesture, cell)
    }

    fn select_line_at(&mut self, cell: (usize, usize), side: Side) -> Option<String> {
        let win = self.selection_window();
        let session = self.channels.session_mut()?;
        let gesture = Gesture {
            term: session.term_mut(),
            win,
            side,
        };
        self.selection.triple_click(gesture, cell)
    }

    fn end_selection(&mut self, side: Side) -> Option<String> {
        let win = self.selection_window();
        let session = self.channels.session_mut()?;
        let gesture = Gesture {
            term: session.term_mut(),
            win,
            side,
        };
        self.selection.release(gesture)
    }

    /// Selecting writes the primary selection and nothing else, which is
    /// what the middle button pastes. The clipboard is left where the user
    /// put it: a run marked to paste two lines down must not cost them what
    /// they copied ten minutes ago.
    fn copy_on_select(&mut self, text: Option<String>) {
        let Some(text) = text.filter(|t| !t.is_empty()) else {
            return;
        };
        if let Err(e) = self.clipboard.set(clipboard::Target::Primary, &text) {
            log::debug!("could not write the primary selection: {e}");
        }
    }

    /// `Ctrl+Shift+C`. The selection is on the primary selection already;
    /// this is what puts it on the clipboard, where a browser or an editor
    /// will look for it.
    fn copy_selection(&mut self) {
        let Some(text) = self
            .clipboard
            .last(clipboard::Target::Primary)
            .map(str::to_owned)
        else {
            return;
        };
        if let Err(e) = self.clipboard.set(clipboard::Target::Clipboard, &text) {
            log::debug!("could not copy the selection: {e}");
        }
    }

    /// `force_bracketed` is the pointer's Ctrl asking for brackets the
    /// terminal's own mode did not.
    fn paste_from(&mut self, target: clipboard::Target, force_bracketed: bool) {
        match self.clipboard.get(target) {
            // An empty selection is nothing to type, and typing it would
            // still send a pair of paste brackets to a program waiting for
            // a command.
            Ok(text) if text.is_empty() => {}
            Ok(text) => {
                // A question standing on the air takes the paste instead of
                // the wire.
                if self.paste_into_prompt(&text) {
                    return;
                }
                // The terminal's own DECSET 2004 decides bracketing; the
                // routing table asks for it too when Ctrl was held.
                let bracketed = force_bracketed || self.mode_contains(Mode::BRACKETED_PASTE);
                let bytes = clipboard::bracket_paste(&text, bracketed);
                self.type_bytes(&bytes);
            }
            Err(e) => log::debug!("could not paste: {e}"),
        }
    }
}

impl Surface for TerminalSurface {
    fn resized(&mut self, size: winit::dpi::PhysicalSize<u32>) {
        log::debug!("resized: {}x{}", size.width, size.height);
        self.window_size = (size.width.max(1), size.height.max(1));
        self.relayout();
    }

    fn scale_factor_changed(&mut self, scale_factor: f64) {
        self.viewport.scale_factor = scale_factor;
        if let Some(window) = self.window.as_ref() {
            let physical = window.inner_size();
            self.window_size = (physical.width.max(1), physical.height.max(1));
        }
        self.relayout();
    }

    fn redraw(&mut self) {
        self.apply_live_settings();
        self.draw_frame();
    }

    fn set_size_badge(&mut self, text: &str, opacity: f32) {
        if self.size_badge.0 != text {
            self.size_badge.0.clear();
            self.size_badge.0.push_str(text);
        }
        self.size_badge.1 = opacity;
    }

    fn cell_size(&self) -> winit::dpi::PhysicalSize<u32> {
        // The same arithmetic the grid is divided by, so the overlay's cell
        // count and the session's cannot disagree.
        let (width, height) = self.viewport.physical_cell();
        winit::dpi::PhysicalSize::new(u32::from(width).max(1), u32::from(height).max(1))
    }

    fn well_minimum(&self) -> winit::dpi::PhysicalSize<u32> {
        let (width, height) = self.viewport.well_minimum();
        winit::dpi::PhysicalSize::new(width, height)
    }

    fn key_pressed(&mut self, event: &winit::event::KeyEvent, modifiers: ModifiersState) {
        self.attended();
        self.key_input(&event.logical_key, key_text(event), modifiers);
    }

    fn ime(&mut self, event: &Ime) {
        self.ime_input(event);
    }

    fn open_settings(&mut self) {
        self.settings_app.open();
    }

    // The five pointer events, each one line: what they mean is
    // [`super::pointer`]'s, and the trait's names are winit's.
    fn mouse_pressed(
        &mut self,
        button: MouseButton,
        position: PhysicalPosition<f64>,
        modifiers: ModifiersState,
    ) {
        self.on_mouse_pressed(button, position, modifiers);
    }

    fn mouse_released(
        &mut self,
        button: MouseButton,
        position: PhysicalPosition<f64>,
        modifiers: ModifiersState,
    ) {
        self.on_mouse_released(button, position, modifiers);
    }

    fn cursor_moved(&mut self, position: PhysicalPosition<f64>, modifiers: ModifiersState) {
        self.on_cursor_moved(position, modifiers);
    }

    fn cursor_left(&mut self) {
        self.on_cursor_left();
    }

    fn mouse_wheel(&mut self, delta: MouseScrollDelta, modifiers: ModifiersState) {
        self.on_mouse_wheel(delta, modifiers);
    }

    fn focus_changed(&mut self, focused: bool) {
        self.on_focus_changed(focused);
    }

    fn tick(&mut self) -> Tick {
        if self.channels.is_empty() {
            return Tick::default();
        }
        if self.eof {
            return Tick {
                finished: true,
                ..Tick::default()
            };
        }
        let bytes = self.pump();
        // Output written while the view is scrolled up moves the offset,
        // because the history under the view grew; the terminal is the
        // authority and this is where the scroll position hears about it.
        if let Some(session) = self.channels.session() {
            self.scroll.sync(session.term());
        }
        // A wheel glide moves the picture on every frame until it arrives.
        // Its frames are the output governor's: a picture that moved is
        // output as far as the screen is concerned, and the governor already
        // paces that to the frame rate and wakes the loop for it.
        let gliding = self.scroll.is_gliding();
        if gliding {
            if let Some(session) = self.channels.session_mut() {
                self.scroll.advance(session.term_mut(), Instant::now());
            }
            self.output_pending = true;
        }
        // Where the caret is, for the input method's candidate window.
        self.publish_ime_cursor();
        if self.eof {
            log::info!("the last channel is gone; closing");
            return Tick {
                finished: true,
                ..Tick::default()
            };
        }
        let now = Instant::now();
        let poll = now + POLL_INTERVAL;
        // The soonest sync deadline of any channel, not only the visible one:
        // an unattended channel's synchronised update has to end on time too,
        // or its screen is half-applied when the knob turns back to it.
        let mut wake_at = self
            .channels
            .rows_mut()
            .filter_map(|row| row.session.sync_deadline())
            .fold(poll, Instant::min);
        // The chord's own 900 ms, which nothing else would wake for: a chord
        // typed and then left alone has to commit on its own.
        if let Some(chord) = self.chord.tick(now) {
            self.apply_chord(Some(chord));
        }
        if let Some(deadline) = self.chord.deadline() {
            wake_at = wake_at.min(deadline);
        }

        // Whether anybody is there. A window that has lost the keyboard, or
        // has held it untouched for `UNATTENDED_AFTER`, is a screen nobody
        // is looking at, and the two things this window does of its own
        // accord -- animating the tube, walking a critter across it -- are
        // both done for a person. Neither is done for an empty chair.
        self.unattended = !self.focused || now.duration_since(self.last_input) >= UNATTENDED_AFTER;

        // The effects clock. A window with no glass has no effects to run, so
        // it keeps to drawing only what changed; an unattended one holds the
        // picture it has, which is the same thing for a different reason.
        // Output still draws either way, so a screen left alone still shows
        // what its child says: what stops is the animation over it.
        let mut effects_due = false;
        if self.glass.is_some() && !self.unattended {
            let cfg = match self.settings.as_ref() {
                Some(handle) => handle.current().general,
                None => Config::default().general,
            };
            let interval = EFFECTS_BASE_FRAME * cfg.effects_frame_skip.max(1) as u32;
            match self.next_effects_frame {
                Some(at) if at > now => wake_at = wake_at.min(at),
                _ => {
                    effects_due = true;
                    self.next_effects_frame = Some(now + interval);
                    wake_at = wake_at.min(now + interval);
                }
            }
        } else {
            // Nothing owed while the picture is held, and the first frame
            // after somebody comes back is owed at once rather than an
            // interval later.
            self.next_effects_frame = None;
        }

        // The critters' own deadline: the next column of a crossing in hand,
        // or the instant the next one is due. A deadline in the future is
        // something to sleep until; a deadline already past is a frame owed
        // now, and it has to be *asked for*, because `draw_frame` is the only
        // thing that advances a crossing. A wake that draws nothing would
        // leave the step where it was and the deadline where it was, and the
        // loop would spin on a `WaitUntil` in the past for as long as the
        // critter was on screen.
        //
        // Gated on the glass for the reason the effects clock above is: a
        // window with nothing to draw on owes the critters no frames.
        let mut critter_due = false;
        if self.glass.is_some() {
            if self.unattended {
                // A crossing in hand when the chair empties comes off, and
                // the frame that takes it off is owed here. INVARIANTS.md
                // has the critters gone once they have stopped drawing, and
                // a held picture with one frozen into it is not gone. The
                // arrival due is forfeited, not banked for the return.
                critter_due = self.critters.withdraw();
            } else {
                match self.critters.wake_at(now) {
                    Some(at) if at > now => wake_at = wake_at.min(at),
                    Some(_) => critter_due = true,
                    None => {}
                }
            }
        }

        // The output governor (see [`Self::next_output_frame`]): output asks
        // for a frame at most once per [`EFFECTS_BASE_FRAME`], and output
        // that arrived between frames is carried as pending, so the frame it
        // waits for is always asked for. An effects frame paints everything
        // anyway, so it discharges the pending output with it.
        if bytes > 0 {
            self.output_pending = true;
        }
        let mut output_due = false;
        if self.output_pending {
            match self.next_output_frame {
                Some(at) if at > now && !effects_due => wake_at = wake_at.min(at),
                _ => {
                    output_due = true;
                    self.output_pending = false;
                    self.next_output_frame = Some(now + EFFECTS_BASE_FRAME);
                }
            }
        }

        Tick {
            redraw: output_due || effects_due || critter_due,
            wake_at: Some(wake_at),
            finished: false,
        }
    }

    /// The window's title tracks the current channel's title. `None` falls
    /// back to the application's name, which the shell already holds as its
    /// identity.
    fn title(&self) -> Option<String> {
        self.channels.current_title().map(str::to_string)
    }

    /// The release of the chord modifier commits whatever digits are
    /// waiting. winit has no key-release filter of its own, so the edge is
    /// taken off the modifier state the shell already tracks: the modifier
    /// was down, and now it is not.
    fn modifiers_changed(&mut self, modifiers: ModifiersState) {
        let down = crate::chord::modifier_down(modifiers);
        let released = self.chord_modifier && !down;
        self.chord_modifier = down;
        if released {
            self.commit_chord();
        }
        // The chord that opens a link, pressed or let go with the pointer
        // resting on one, is the other edge a surface reads here.
        self.refresh_hover(modifiers_from(modifiers));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crt::{DegaussState, Geometry, Params};

    /// One frame's uniforms from a geometry, with the clock and the transient
    /// held still.
    fn params(cfg: &Config, geom: &Geometry) -> Params {
        Params::build(
            cfg,
            geom,
            crt::FrameTime {
                elapsed: 0.0,
                now: 0.0,
                delta: 0.0,
                index: 0,
            },
            DegaussState::IDLE,
        )
    }

    /// `crt::Geometry`'s `output_*` is in logical pixels, and the render
    /// target is physical. On an ordinary display the two are the same number
    /// and nothing shows; at scale factor 2 the difference is the whole of
    /// `normalizedScreenScale`, and with it the curvature and the moulding.
    #[test]
    fn the_chain_is_measured_in_logical_pixels_on_a_2x_display() {
        let cfg = Config::default();

        // The same window, twice: 1024x768 logical on a 1x display, and the
        // same 1024x768 logical on a 2x one, where the target is 2048x1536.
        // The font is twice the raster scale on the 2x display for the same
        // reason the target is twice the size.
        let one = chain_geometry((1024, 768), 1, &cfg, 1.0);
        let two = chain_geometry((2048, 1536), 2, &cfg, 2.0);

        assert_eq!((one.output_width, one.output_height), (1024.0, 768.0));
        assert_eq!(
            (two.output_width, two.output_height),
            (1024.0, 768.0),
            "the same window is the same size in logical pixels whatever the \
             display's scale factor"
        );

        // Which is the point: every uniform normalised by the screen scale is
        // the same on both, rather than half on the 2x display.
        let (a, b) = (params(&cfg, &one), params(&cfg, &two));
        for name in ["ScreenCurvature", "FrameSize", "screenRadius"] {
            let (x, y) = (a.get(name).unwrap(), b.get(name).unwrap());
            assert!(
                (x - y).abs() < 1e-6,
                "{name} is {x} at 1x and {y} at 2x; the window is the same size"
            );
        }

        // The raster grid is not a length, so it is *not* logical: it is a
        // count of unscaled font pixels, and the 2x display fits the same
        // number of them across because its font is scaled up to match.
        assert_eq!(one.virtual_width, two.virtual_width);
        assert_eq!(one.virtual_height, two.virtual_height);

        // And the rasterization fade still measures device pixels per raster
        // pixel, which is where the ratio comes back in: a 2x display at
        // twice the integer scale is the same density.
        assert!(
            (a.get("RasterizationIntensity").unwrap() - b.get("RasterizationIntensity").unwrap())
                .abs()
                < 1e-6
        );
    }

    /// The virtual width is `floor(width / (scale * font_width))` and the
    /// virtual height is `floor(height / scale)`. Both terms of the width
    /// matter -- a narrowed font packs more columns of raster into the same
    /// glass, and the mask is spaced by the count.
    #[test]
    fn the_virtual_resolution_takes_the_font_width_and_floors() {
        let mut cfg = Config::default();
        assert_eq!(cfg.screen.font_width, 1.0);
        let wide = chain_geometry((1000, 750), 3, &cfg, 1.0);
        // floor(1000/3) = 333, not 333.33; floor(750/3) = 250 exactly.
        assert_eq!(wide.virtual_width, 333.0);
        assert_eq!(wide.virtual_height, 250.0);

        cfg.screen.font_width = 0.5;
        let narrow = chain_geometry((1000, 750), 3, &cfg, 1.0);
        assert_eq!(narrow.virtual_width, 666.0);
        assert_eq!(
            narrow.virtual_height, 250.0,
            "font width narrows the columns and leaves the rows alone"
        );
    }

    /// Done-test: the well's grid is inset by `settings::distortion_margin`
    /// on every edge before its own column/row count is taken -- the space
    /// the grid is sized from shrinks by twice the margin on each axis,
    /// `margin` itself being a lerp between two configured bounds plus a
    /// curvature term ([`settings::distortion_margin`], the same derivation
    /// [`TerminalSurface::ensure_margin`] reads to set [`Viewport::margin`]).
    ///
    /// The margin is logical; [`Viewport::margin`] is physical, so it is
    /// scaled by the sampled DPR here the same way a caller wiring this up
    /// (`TerminalSurface::new`/`apply_live_settings`) has to.
    #[test]
    fn the_grid_is_inset_by_the_distortion_margin_before_dividing_by_the_cell() {
        let mut cfg = Config::default();
        cfg.general.chassis_shown = false;
        cfg.screen.margin = 0.4;
        cfg.screen.screen_radius = 0.7;

        // The formula, spelled out independently of
        // `settings::distortion_margin` so this test would notice either
        // side drifting from the other.
        let lint = |a: f64, b: f64, t: f64| a + (b - a) * t;
        let screen_radius = lint(4.0, 120.0, 0.7);
        let expected_margin_logical =
            lint(1.0, 40.0, 0.4) + (1.0 - std::f64::consts::FRAC_1_SQRT_2) * screen_radius;
        let margin_logical = settings::distortion_margin(&cfg);
        assert!(
            (margin_logical - expected_margin_logical).abs() < 1e-9,
            "{margin_logical} != {expected_margin_logical}"
        );

        // A sampled well size and DPR, not defaults, so the inset is a real
        // number of pixels rather than an edge case.
        let scale_factor = 1.5;
        let mut viewport = Viewport::new(1000, 820, scale_factor, CellSize::new(9.0, 18.0));
        viewport.margin = margin_logical * scale_factor;

        let size = viewport.term_size();
        assert_eq!(
            (size.cols(), size.rows()),
            (62, 25),
            "875x695 physical pixels left after a 125px (2 * round(62.33..)) \
             inset, divided by the 14x27 physical cell"
        );

        // Positioning: `draw_frame` centres the grid in the well at a
        // whole-pixel origin (`(target - grid).max(0) / 2`); with the grid
        // now smaller by the margin on every edge, that same centring
        // already seats it inset by (roughly) the margin rather than
        // flush against the well's own edge.
        let (grid_w, grid_h) = size.pixel_size();
        let origin = (
            (1000i32 - i32::from(grid_w)).max(0) / 2,
            (820i32 - i32::from(grid_h)).max(0) / 2,
        );
        assert_eq!(origin, (66, 72));
    }
}
