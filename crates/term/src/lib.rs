//! RobCo Terminal core.
//!
//! Everything that is a terminal and nothing that is a window: the rio-vt
//! session, the PTY read loop driving it, the grid read-back seam, the DCS
//! tap the tmux gateway hangs on, the session a tmux pane feeds,
//! and the glyph path that draws a screen of cells. `crates/app` owns the
//! window, the surface and the event loop, and calls in here.
//!
//! The split is what lets the done-tests run without a display: a session
//! needs no window, so a scripted PTY transcript is an ordinary `cargo test`,
//! and the renderer draws into an offscreen texture it reads back.
//!
//! The glyph path is the one glyphon could not provide: cosmic-text shapes
//! and rasterises, this crate thresholds the coverage mask on its way into
//! an atlas it owns, and the fragment shader reads texels with
//! `textureLoad` rather than through a sampler. Magnifying is integer
//! geometry, never re-rasterisation: the counterexample
//! (Terminess re-rasterised at twice the size differs from the doubled bitmap
//! in 3960 pixels) is re-asserted in this crate's tests so the shortcut cannot
//! be taken back later by accident.
//!
//! Layering:
//!
//! * [`session`] / [`dcs`] / [`size`]: the emulation core and its PTY loop.
//! * [`tmux_cc`] / [`tmux_pane`]: the tmux control-mode envelope on the
//!   wire, and the session variant a tmux pane feeds.
//! * [`grid`]: the grid seam. `GridView` is the one definition of what a
//!   line says; [`rio_grid`] adapts rio-vt's `Crosswords` onto it and carries
//!   the read-back-as-text path with it.
//! * [`selection`] / [`hotspots`] / [`links`] / [`pointer`]: what the user
//!   has selected, which spans the URL filters match, the link under a cell
//!   (an OSC 8 hyperlink first, a matched span otherwise), and whether a
//!   pointer event marks the screen or reaches the program. Scrollback
//!   search is rio-vt's own (`Crosswords::search_next`), driven from
//!   `crates/app`'s find line.
//! * [`fonts`]: the bundled catalogue, and [`fonts::sizing`], the seam: a
//!   catalogue row plus the user's knobs in, a `ResolvedFont` out. Nothing
//!   downstream sees the catalogue's raw per-entry properties again.
//! * [`atlas`]: shaping, rasterising, thresholding, packing.
//! * [`cells`] / [`color`]: what a screen of text is, and the one place a
//!   rio-vt `Square` becomes a coloured cell.
//! * [`viewport`]: scrollback policy over rio-vt's display offset.
//! * [`render`]: the damage-driven instance buffer and the draw.
//!
//! The offscreen target and its readback are not here: `robco-gpu` owns them,
//! along with the feature set a device is created with, because the application
//! and the test harness make devices too. The grid is drawn into a
//! `gpu::Target`, not into the swapchain, so the CRT chain can filter the grid
//! without filtering the chassis around it, and [`Gpu`], [`Image`] and
//! [`Target`] are re-exported here because the renderer's signatures name them.

pub mod atlas;
pub mod cells;
pub mod color;
pub mod dcs;
pub mod distortion;
pub mod fonts;
pub mod grid;
pub mod hotspots;
pub mod links;
pub mod pointer;
pub mod render;
pub mod rio_grid;
pub mod selection;
pub mod session;
pub mod size;
pub mod ssh_channel;
pub mod tmux_cc;
pub mod tmux_pane;
pub mod viewport;

pub use atlas::{CellMetrics, FontContext, GlyphAtlas, Rasterization};
pub use cells::{Cell, CellGrid, CursorShape, CursorState};
pub use color::{Rgba, Scheme};
pub use dcs::{DcsParser, DcsTap, NoopTap};
pub use distortion::{correct_distortion, DistortionParams};
pub use fonts::sizing::{resolve, ResolvedFont, ScalePolicy, SizingRequest};
pub use fonts::{bundled_fonts, font_by_name, system_fonts, FontEntry, FontSource};
// From `robco-gpu`, not from a module of this crate.
pub use gpu::{Gpu, Image, Target};
pub use grid::{GridView, ScriptedGrid};
pub use hotspots::{HotSpot, HotSpotType, UrlFilterChain, UrlType};
pub use links::link_at;
pub use render::{GridRenderer, Marked, SyncStats};
// The two grid-to-text answers, both at the root because both are asked
// from outside: `live_text` is what the program below has written on the
// screen, `viewport_text` is what the user is looking at. They differ the
// moment the view is scrolled back, which is the case the renderer is in.
pub use cells::vt::viewport_text;
pub use rio_grid::{all_text, cell_char, live_text, row_cells, row_text, screen_contains, RioGrid};
// `distortion::Point` and `selection::Window` stay behind their modules.
// Both are common words that mean something else one crate up (a winit
// `Window`, a pixel point), and neither is asked for often enough to be
// worth the collision at the root.
pub use selection::{Kind, MarkedRange, SelectionModel};
pub use session::{Pumped, Replies, ReplyListener, Session, SessionConfig, Term, INPUT_CAP};
pub use size::{CellSize, TermSize, Viewport, FLOOR_COLS, FLOOR_ROWS};
pub use ssh_channel::{SshChannel, SshEvent, SshWire};
pub use tmux_cc::ControlModeTap;
pub use tmux_pane::{ChannelSession, TmuxPane};
pub use viewport::{ScrollPosition, ViewportChange};

/// Re-exported so downstream crates pin the same rio-vt this crate does
/// rather than adding a second, possibly different, dependency on it.
pub use rio_vt;

/// The threshold this crate measured to work, and the one the atlas uses
/// unless a caller says otherwise: coverage at or above half is ink.
pub const DEFAULT_THRESHOLD: u8 = 128;

/// Printable ASCII: all a terminal atlas holds. A character outside it has
/// no slot, and draws as an empty cell with no tofu box and no log line.
pub fn ascii_charset() -> String {
    (0x20u8..0x7f).map(|b| b as char).collect()
}

/// The common opening move: resolve the sizing, build an atlas over printable
/// ASCII in the mode the face is entitled to, and hand back both halves.
///
/// The mode is not a parameter, and deliberately: there is one switch that
/// decides whether a face is antialiased, at [`Rasterization::for_face`]. A
/// caller that could pass its own would be a second place the question is
/// answered.
pub fn build_font(
    device: &wgpu::Device,
    queue: &wgpu::Queue,
    spec: &FontEntry,
    request: &SizingRequest,
    policy: ScalePolicy,
) -> (ResolvedFont, FontContext, GlyphAtlas) {
    let resolved = resolve(spec, request, policy);
    let mut font = FontContext::new(spec);
    let atlas = font.build_atlas(
        device,
        queue,
        &resolved,
        &ascii_charset(),
        Rasterization::for_face(&resolved),
    );
    (resolved, font, atlas)
}

/// Whether the grid is drawn with each character at its own width, read once
/// from `ROBCO_PROPORTIONAL` in the environment.
///
/// An experiment, and it lives on an environment variable rather than in the
/// profile for that reason: nothing in the settings window offers it, an
/// unset variable leaves every measurement and every pixel where they were,
/// and the whole of it comes out in one revert.
///
/// Three things read it. The font menu stops filtering the machine's
/// families down to the fixed-pitch ones, because a proportional grid drawn
/// in a monospace face is the grid it already was. The cell the geometry
/// divides by becomes the advance of "x" widened by a fifth, so a line of
/// ordinary text fits the columns advertised and a line of capitals runs
/// into the margin that fifth reserves. And the renderer walks a pen along
/// each row instead of multiplying the column by the cell, which is what
/// takes the vertical alignment away.
pub fn proportional() -> bool {
    static ON: std::sync::OnceLock<bool> = std::sync::OnceLock::new();
    *ON.get_or_init(|| {
        std::env::var("ROBCO_PROPORTIONAL")
            .map(|v| v != "0" && !v.is_empty())
            .unwrap_or(false)
    })
}
