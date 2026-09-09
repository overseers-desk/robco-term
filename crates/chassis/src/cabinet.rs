//! The whole cabinet as one object: the composition order from this crate's
//! module doc, held together so a host does not have to keep it straight.
//!
//! Everything under it is usable on its own: the geometry, the layout and the
//! seam are plain values and pure functions, and stay that way. This is the
//! assembled form, shaped so that a `app::shell::Surface` implementation
//! forwards to it method for method and does no chassis arithmetic itself.
//!
//! # Wiring a `Surface` to it
//!
//! ```ignore
//! impl Surface for TerminalSurface {
//!     fn resized(&mut self, size: PhysicalSize<u32>) {
//!         let logical: LogicalSize<f64> = size.to_logical(self.scale_factor);
//!         self.cabinet.resized(logical.width, logical.height);
//!         // ...the rest of the resize
//!     }
//!
//!     fn cursor_moved(&mut self, position: PhysicalPosition<f64>, _: ModifiersState) {
//!         let x = position.x / self.scale_factor;
//!         if let Some(update) = self.cabinet.cursor_moved(x) {
//!             self.settings.set_led_characters(update.led_characters);
//!             self.shell.set_bank_width(update.bank_width);
//!             self.window.set_cursor(CursorIcon::EwResize);
//!             return;
//!         }
//!         // ...the grid's own pointer handling
//!     }
//!
//!     fn mouse_pressed(&mut self, button: MouseButton, position: PhysicalPosition<f64>, _: ModifiersState) {
//!         let x = position.x / self.scale_factor;
//!         if button == MouseButton::Left && self.cabinet.pointer_pressed(x) {
//!             return; // the press was the seam's
//!         }
//!         // ...the grid's own press handling
//!     }
//!
//!     fn mouse_released(&mut self, _: MouseButton, _: PhysicalPosition<f64>, _: ModifiersState) {
//!         self.cabinet.pointer_released();
//!     }
//!
//!     fn focus_changed(&mut self, focused: bool) {
//!         if !focused {
//!             self.cabinet.pointer_released();
//!         }
//!     }
//! }
//! ```
//!
//! # Which pixel
//!
//! Every measure in this crate is in the **logical** pixel, which winit
//! calls the logical pixel and other UI toolkits call the
//! device-independent pixel. The shells' constants are read off mocks at 1x
//! (a 45 px row pitch, a 3 px shoulder, the 320 px the well is never given
//! less than), and on a 2x display each of them is drawn at twice the
//! physical size, because these coordinates are logical throughout.
//!
//! `app::shell`'s `Surface` delivers physical pixels: `PhysicalSize`,
//! `PhysicalPosition`. So a host on a scaled display divides by the window's
//! scale factor on the way in. On the way out it does not have to: the
//! minimum-size hint is converted per window inside
//! `chassis::layout::min_inner_size_physical`, because a shell holds several
//! windows and they need not share a factor.
//! [`Cabinet::bank_width_physical`] is the multiplication for the other
//! direction of the same question: the bank's footprint in device pixels, for
//! whoever draws it. On an unscaled display the factor is 1.0 and none of this
//! is visible, which is exactly why it is written down here.
//!
//! # Two obligations
//!
//! Neither is the chassis's to discharge. [`SeamUpdate::led_characters`] has
//! to reach the settings, because the seam is a settings edit and the
//! configuration file is the source of truth; and
//! [`SeamUpdate::bank_width`] has to reach `Shell::set_bank_width`, because the
//! appliance draws the bank at that width and a window standing open has to
//! redraw it as the drag moves. The window's minimum-size hint answers a
//! different question -- the bank at its narrowest, from
//! [`Cabinet::min_bank_width`], not the width a drag just landed -- and does
//! not move with this update. The cabinet updates its own copy of the
//! character count as it goes, so a settings reload that arrives later
//! carrying the same value is a no-op rather than a jump.

use crate::bank::BankGeometry;
use crate::frame::{self, ChassisStyle, FrameStyle, Param};
use crate::layout::WindowLayout;
use crate::metrics::{DisplayMetrics, LedMetrics, ShellMetrics, TapeMetrics};
use crate::seam::{SeamContext, SeamCursor, SeamDrag};
use config::Config;

/// Which display kit the strips are cut from.
///
/// The kit is a measured value rather than a profile: the tape's one
/// character is an advance of Departure Mono at 20 px and the LED's is the
/// profile lamp font's cell. Keeping it a value is what lets every measure
/// under this module be tested with no font stack at all.
/// [`crate::display_kit`] is the one place a `Config` becomes one, and
/// [`Cabinet::from_config`] the constructor that goes through it, so a host
/// with a profile and a window never handles a kit itself.
#[derive(Clone, Copy, Debug, PartialEq)]
pub enum Display {
    Led(LedMetrics),
    Tape(TapeMetrics),
}

impl DisplayMetrics for Display {
    fn unit_width(&self) -> f64 {
        match self {
            Display::Led(d) => d.unit_width(),
            Display::Tape(d) => d.unit_width(),
        }
    }

    fn min_units(&self) -> i32 {
        match self {
            Display::Led(d) => d.min_units(),
            Display::Tape(d) => d.min_units(),
        }
    }

    fn width_for_units(&self, n: i32) -> i32 {
        match self {
            Display::Led(d) => d.width_for_units(n),
            Display::Tape(d) => d.width_for_units(n),
        }
    }

    fn height_for_pad_cells(&self, pad_cells: i32) -> i32 {
        match self {
            Display::Led(d) => d.height_for_pad_cells(pad_cells),
            Display::Tape(d) => d.height_for_pad_cells(pad_cells),
        }
    }

    fn pad_cells_for_hole(&self, hole_height: i32) -> i32 {
        match self {
            Display::Led(d) => d.pad_cells_for_hole(hole_height),
            Display::Tape(d) => d.pad_cells_for_hole(hole_height),
        }
    }
}

/// What a drag landed, and the two places it has to go.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct SeamUpdate {
    /// The new `general.led_characters`, for the settings.
    pub led_characters: i32,
    /// The new bank width in logical pixels, which is what
    /// `app::shell::Shell::set_bank_width` takes: the shell holds several
    /// windows and they need not share a scale factor, so the conversion
    /// happens per window at the hint site
    /// (`chassis::layout::min_inner_size_physical`). Use
    /// [`Cabinet::bank_width_physical`] for the other question: the bank's
    /// own footprint in device pixels, for drawing it.
    pub bank_width: u32,
}

/// What [`Cabinet::furniture`] was last built from. Every argument
/// `furniture::bank_pieces` reads, so a key that matches means the pieces it
/// would return are the ones already in hand.
#[derive(Clone, Debug, PartialEq)]
struct FurnitureKey {
    cfg: Config,
    display: Display,
    shell: ShellMetrics,
    geometry: BankGeometry,
    bank: (f64, f64),
    strips: crate::BankStrips,
}

/// The assembled cabinet.
#[derive(Clone, Debug)]
pub struct Cabinet {
    cfg: Config,
    display: Display,
    shell: ShellMetrics,
    frame_style: FrameStyle,
    chassis_style: ChassisStyle,
    geometry: BankGeometry,
    layout: WindowLayout,
    well_minimum: (i32, i32),
    seam: SeamDrag,
    /// The bank folded away by a chord: the well takes the whole window
    /// and nothing of the bank is drawn, measured or hit, the same cabinet
    /// standing around the glass. This window's runtime state, like
    /// fullscreen, and no setting: a reload leaves it where it is.
    folded: bool,
    /// The last furniture built and what it was built from. The bank is a
    /// nameplate over a column of channel windows: it changes when a channel
    /// does, or when the window is resized, and not on the twenty frames a
    /// second the glass's own flicker asks for, so every one of those frames
    /// would otherwise widen the same lamp grids and strike the same
    /// engraved lines to arrive back where it started.
    remembered: std::cell::RefCell<Option<(FurnitureKey, std::sync::Arc<[crate::Piece]>)>>,
}

impl Cabinet {
    /// Build for a profile, a display kit and a window size in logical pixels
    /// (see the module doc on which pixel, and why it matters only on a scaled
    /// display).
    pub fn new(cfg: &Config, display: Display, width: f64, height: f64) -> Self {
        let shell = crate::shell_metrics(cfg);
        // A starting value: the bare tube's own floor, which stands until the
        // host has resolved a font and can say what a terminal grid costs it
        // (`set_well_minimum`). The bank is fitted against it from the first
        // measurement, so a window opened too narrow draws a narrowed bank on
        // its first frame rather than on its second.
        let well_minimum = (
            crate::layout::CRT_MINIMUM_WIDTH,
            crate::layout::MINIMUM_HEIGHT,
        );
        let geometry = Self::measure(cfg, &shell, &display, width, f64::from(well_minimum.0));
        let layout =
            WindowLayout::new(width, height, Self::footprint(cfg.general.chassis_shown, &geometry));
        Cabinet {
            cfg: cfg.clone(),
            display,
            shell,
            frame_style: crate::frame_style(cfg),
            chassis_style: crate::chassis_style(cfg),
            geometry,
            layout,
            well_minimum,
            seam: SeamDrag::new(),
            folded: false,
            remembered: std::cell::RefCell::new(None),
        }
    }

    /// Build for a profile alone, measuring the display kit off the font stack
    /// ([`crate::display_kit`]) rather than taking one.
    ///
    /// This is the constructor a host uses: it has a `Config` and a window, and
    /// no reason to know which display kit a cabinet carries, both of them
    /// lettering the bank from the cabinet's own face.
    pub fn from_config(cfg: &Config, width: f64, height: f64) -> Self {
        Cabinet::new(cfg, crate::display_kit(cfg), width, height)
    }

    /// The bank's measures for this profile in a window this wide.
    ///
    /// Measured twice when the window cannot hold the configured strip: once
    /// at the count the settings ask for, because that is the count
    /// [`crate::seam::fitted_characters`] takes its delta from, and once more
    /// at the count the fit landed. Both are arithmetic over integers, with no
    /// font work and no allocation between them.
    ///
    /// The configured count is not touched. It stays in `cfg` as the user's
    /// wish, and the window gets what it has room for.
    fn measure(
        cfg: &Config,
        shell: &ShellMetrics,
        display: &Display,
        window_width: f64,
        well_minimum_width: f64,
    ) -> BankGeometry {
        let indicator = crate::channel_indicator(cfg);
        let configured = cfg.general.led_characters;
        let at_configured = BankGeometry::new(shell, display, configured, indicator);
        let fitted = crate::seam::fitted_characters(
            &at_configured,
            configured,
            display.unit_width(),
            window_width,
            well_minimum_width,
        );
        if fitted == configured {
            at_configured
        } else {
            BankGeometry::new(shell, display, fitted, indicator)
        }
    }

    /// The bank's footprint, zero when no bank stands. A hidden chassis, or
    /// a folded bank, is not a bank of no width; it is no bank, and the well
    /// takes the whole window.
    fn footprint(stands: bool, geometry: &BankGeometry) -> f64 {
        if stands {
            geometry.implicit_width as f64
        } else {
            0.0
        }
    }

    /// Whether a bank stands beside the well: the chassis is drawn and the
    /// bank is not folded. The one predicate every measure, draw and hit
    /// test of the bank turns on.
    fn bank_stands(&self) -> bool {
        self.cfg.general.chassis_shown && !self.folded
    }

    /// Fold the bank away, or bring it back at its configured count.
    /// Returns the bank width to hand `Shell::set_bank_width`, as a settings
    /// reload does.
    pub fn set_folded(&mut self, folded: bool) -> u32 {
        self.folded = folded;
        self.remeasure();
        self.bank_width()
    }

    pub fn is_folded(&self) -> bool {
        self.folded
    }

    /// A settings reload, or a display kit re-measured after a font change.
    /// Returns the bank width to hand `Shell::set_bank_width`.
    pub fn apply_settings(&mut self, cfg: &Config, display: Display) -> u32 {
        self.cfg = cfg.clone();
        self.display = display;
        self.shell = crate::shell_metrics(cfg);
        self.frame_style = crate::frame_style(cfg);
        self.chassis_style = crate::chassis_style(cfg);
        self.remeasure();
        self.bank_width()
    }

    /// A settings reload, re-measuring the display kit from the new profile.
    /// The [`Cabinet::from_config`] half of [`Cabinet::apply_settings`], and
    /// the one a host that never handles a kit itself calls: a change of
    /// `chassis.bank_font_name` or of `chassis.channel_display` moves the cell
    /// the strips are cut from, so the kit has to be re-measured and not only
    /// re-applied.
    pub fn apply_config(&mut self, cfg: &Config) -> u32 {
        self.apply_settings(cfg, crate::display_kit(cfg))
    }

    /// The window changed size, in logical pixels. The well takes most of
    /// the difference, and the bank takes the rest: a window too narrow to
    /// hold the configured strip beside a terminal grid draws a narrower
    /// strip, down to the display's own least.
    pub fn resized(&mut self, width: f64, height: f64) {
        self.layout = WindowLayout::new(width, height, self.layout.bank.width);
        self.remeasure();
    }

    /// Re-derive the bank from the profile, the kit and the window as they
    /// all now stand.
    ///
    /// The window's width is recovered from the layout rather than passed in.
    /// `crt.right()` is the window's own right edge whatever the bank is
    /// doing, since the two rectangles are cut from it and meet at the seam.
    fn remeasure(&mut self) {
        let window_width = self.layout.crt.right();
        self.geometry = Self::measure(
            &self.cfg,
            &self.shell,
            &self.display,
            window_width,
            f64::from(self.well_minimum.0),
        );
        self.layout = WindowLayout::new(
            window_width,
            self.layout.bank.height,
            Self::footprint(self.bank_stands(), &self.geometry),
        );
    }

    /// The bank's footprint in logical pixels.
    pub fn bank_width(&self) -> u32 {
        self.layout.bank.width.max(0.0) as u32
    }

    /// The bank's footprint in physical pixels, which is the unit
    /// `Window::set_min_inner_size` measures a `Size::Physical` in.
    pub fn bank_width_physical(&self, scale_factor: f64) -> u32 {
        (self.layout.bank.width.max(0.0) * scale_factor.max(0.0)).round() as u32
    }

    /// The least screen the well is ever given, in logical pixels.
    ///
    /// The host sets it, because the measure is the host's: the well holds a
    /// terminal grid and only the host knows what its font charges for one.
    /// The seam's travel and the window's minimum-size hint are then the same
    /// number seen from two sides, which is why it is held here rather than
    /// asked for at each of the two.
    pub fn set_well_minimum(&mut self, width: i32, height: i32) {
        if self.well_minimum == (width, height) {
            return;
        }
        self.well_minimum = (width, height);
        // The floor is one of the fit's three inputs, and it moves with the
        // font rather than with the window, so a font that grew mid-session
        // narrows the bank with no resize behind it.
        self.remeasure();
    }

    /// The floor as it now stands.
    pub fn well_minimum(&self) -> (i32, i32) {
        self.well_minimum
    }

    /// The count the settings ask for, whatever the window has room to draw.
    ///
    /// The bank on screen is [`BankGeometry::characters`]; the two are the
    /// same number in any window with room for the strip.
    pub fn configured_characters(&self) -> i32 {
        self.cfg.general.led_characters
    }

    /// The narrowest this bank is ever drawn: its footprint at the display's
    /// own least strip, or at the configured count when that is narrower
    /// still, since the fit does not widen.
    ///
    /// This is the bank term of the window's minimum-size hint. The hint is a
    /// constraint on the window's width, so it cannot be a function of the
    /// window's width: a hint that rose as the bank widened would chase a
    /// window the user had narrowed, `Shell` growing the window to meet a
    /// hint it had just raised.
    pub fn min_bank_width(&self) -> u32 {
        if !self.bank_stands() {
            return 0;
        }
        let least = self
            .configured_characters()
            .min(self.geometry.min_units)
            .max(0);
        BankGeometry::new(
            &self.shell,
            &self.display,
            least,
            crate::channel_indicator(&self.cfg),
        )
        .implicit_width
        .max(0) as u32
    }

    /// The window's least inner size: the bank at its narrowest beside a well
    /// at its floor, in logical pixels.
    pub fn min_inner_size(&self) -> (i32, i32) {
        crate::layout::min_inner_size(self.min_bank_width() as i32, self.well_minimum)
    }

    pub fn geometry(&self) -> &BankGeometry {
        &self.geometry
    }

    pub fn layout(&self) -> &WindowLayout {
        &self.layout
    }

    /// Whether a bank is drawn at all: the chassis shown and the bank
    /// unfolded. The bezel is not this question; it reads the profile.
    pub fn is_shown(&self) -> bool {
        self.bank_stands()
    }

    /// The uniforms for the bezel over the screen well.
    pub fn frame_params(&self) -> Vec<Param> {
        frame::frame_params(&self.frame_style, &self.shell, &self.cfg, &self.layout)
    }

    /// The parameter block for the casting under the bank column.
    pub fn chassis_params(&self) -> crate::params::ChassisMetalParams {
        frame::chassis_params(&self.chassis_style, &self.shell, &self.layout)
    }

    /// How many rows this bank shows, the shell's own pager taken off its
    /// foot first.
    ///
    /// Zero for a bank that is not shown, and zero for the pre-load beat the
    /// arithmetic refuses (`BankGeometry::rows_visible`), so a caller can
    /// always iterate the answer.
    pub fn rows_visible(&self) -> i32 {
        if !self.is_shown() {
            return 0;
        }
        self.geometry
            .rows_visible(self.layout.bank.height as i32, self.pager_height())
            .unwrap_or(0)
    }

    /// What the shell's pager takes off the bank's foot before the rows
    /// divide what is left.
    ///
    /// Public because the row-count settle lives in the app (it owns the
    /// channel model the count feeds), and it measures against the window's own
    /// height rather than the layout's; it still has to subtract the same
    /// footprint this cabinet's own [`Cabinet::rows_visible`] does, and one
    /// answer is the point.
    pub fn pager_height(&self) -> i32 {
        let content_width = self.geometry.content_width(self.bank_width() as i32);
        self.shell.pager_height(content_width)
    }

    /// What stands on the casting: the shell's plate and one channel display
    /// per row, in draw order, in the bank column's own logical pixels.
    ///
    /// The step 4 of this crate's composition order that the mount owns, and
    /// the one that takes an argument from outside the chassis:
    /// [`crate::BankStrips`] is the channel model's half. See
    /// [`crate::furniture`] for that seam and for what is drawn here.
    pub fn furniture(&self, strips: &crate::BankStrips) -> std::sync::Arc<[crate::Piece]> {
        if !self.is_shown() {
            return std::sync::Arc::from(Vec::new());
        }
        let bank = (self.layout.bank.width, self.layout.bank.height);
        let key = FurnitureKey {
            cfg: self.cfg.clone(),
            display: self.display,
            shell: self.shell,
            geometry: self.geometry,
            bank,
            strips: strips.clone(),
        };
        if let Some((held, pieces)) = self.remembered.borrow().as_ref() {
            if *held == key {
                return std::sync::Arc::clone(pieces);
            }
        }
        let pieces: std::sync::Arc<[crate::Piece]> =
            std::sync::Arc::from(crate::furniture::bank_pieces(
                &self.cfg,
                &self.shell,
                &self.display,
                &self.geometry,
                bank,
                strips,
            ));
        *self.remembered.borrow_mut() = Some((key, std::sync::Arc::clone(&pieces)));
        pieces
    }

    /// Which row's window a press at `(x, y)` landed in, both in the bank
    /// column's own logical pixels, for a page of `rows` engraved keys.
    ///
    /// The hit test belongs to whoever drew the windows, which is why it is
    /// here and not in the channel model; what to *do* about the press is
    /// `app::bank::BankPager::press`'s (open a dark slot, select an open one).
    pub fn strip_at(&self, rows: usize, x: f64, y: f64) -> Option<usize> {
        if !self.is_shown() {
            return None;
        }
        crate::furniture::strip_at(&self.geometry, &self.shell, rows, x, y)
    }

    /// Which way a press at `(x, y)` walks the pages, both in the bank
    /// column's own logical pixels: `-1` for PREV, `1` for NEXT. Here for
    /// [`Cabinet::strip_at`]'s reason; what to do about the press is the
    /// app's. A press past the last page is not refused here: the step
    /// clamps.
    pub fn pager_at(&self, x: f64, y: f64) -> Option<i32> {
        if !self.is_shown() {
            return None;
        }
        crate::furniture::pager_at(
            &self.geometry,
            &self.shell,
            (self.layout.bank.width, self.layout.bank.height),
            x,
            y,
        )
    }

    /// The cursor the seam claims, if any.
    pub fn cursor(&self) -> SeamCursor {
        self.seam.cursor()
    }

    /// A left button went down at window x, in logical pixels. Returns whether
    /// the seam took it; a press the seam took must not also reach the grid.
    pub fn pointer_pressed(&mut self, x: f64) -> bool {
        self.seam
            .pointer_pressed(x, self.layout.bank.width, self.is_shown())
    }

    /// The button came up, or the window lost focus with it still down.
    pub fn pointer_released(&mut self) {
        self.seam.pointer_released();
    }

    /// The pointer moved to window x, in logical pixels. Returns an update when
    /// the drag landed a new character count.
    pub fn cursor_moved(&mut self, x: f64) -> Option<SeamUpdate> {
        let ctx = SeamContext {
            geometry: &self.geometry,
            // The count on screen, not the count the settings ask for: a drag
            // that began on a fitted bank measures from the strip the hand
            // can see, or its first motion jumps by the difference.
            led_characters: self.geometry.characters,
            unit_width: self.display.unit_width(),
            window_width: self.layout.crt.right(),
            well_minimum_width: f64::from(self.well_minimum.0),
            bank_width: self.layout.bank.width,
        };
        let shown = self.bank_stands();
        let chars = self.seam.pointer_moved(x, &ctx, shown)?;
        // Take the new count on the spot. The settings write is the host's, and
        // the reload that follows it is a round trip: measuring the next motion
        // against the old geometry until it came back would put the seam a
        // frame behind the hand, and at speed several characters behind.
        self.cfg.general.led_characters = chars;
        self.remeasure();
        Some(SeamUpdate {
            led_characters: chars,
            bank_width: self.bank_width(),
        })
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn stock() -> Cabinet {
        Cabinet::new(
            &Config::default(),
            Display::Led(LedMetrics::default()),
            1024.0,
            768.0,
        )
    }

    #[test]
    fn the_assembled_cabinet_agrees_with_its_parts() {
        let c = stock();
        assert_eq!(c.bank_width(), 184);
        assert_eq!(c.layout().crt.width, 840.0);
        // The hint reserves the bank at its least, not at the twelve
        // characters it is standing at: 154 px of strip beside the bare
        // tube's 320.
        assert_eq!(c.min_bank_width(), 154);
        assert_eq!(c.min_inner_size(), (474, 240));
        assert_eq!(c.frame_params().len(), 36);
        assert_eq!(c.chassis_params().field_scale, [184.0 / 840.0, 1.0]);
    }

    #[test]
    fn a_folded_bank_is_no_bank_until_it_comes_back() {
        let mut c = stock();
        assert_eq!(c.set_folded(true), 0);
        assert!(!c.is_shown());
        assert_eq!(c.layout().crt.width, 1024.0);
        assert_eq!(c.min_inner_size(), (320, 240));
        assert!(!c.pointer_pressed(184.0));
        assert_eq!(c.cursor_moved(300.0), None);
        // The profile still says the chassis is shown: the fold is this
        // window's, and a reload of the same profile leaves it folded.
        assert_eq!(
            c.apply_settings(&Config::default(), Display::Led(LedMetrics::default())),
            0
        );
        assert!(c.is_folded());
        assert_eq!(c.set_folded(false), 184);
        assert!(c.is_shown());
        assert_eq!(c.layout().crt.width, 840.0);
        assert_eq!(c.min_inner_size(), (474, 240));
    }

    #[test]
    fn a_hidden_chassis_gives_the_well_the_whole_window_and_no_seam() {
        let mut cfg = Config::default();
        cfg.general.chassis_shown = false;
        let mut c = Cabinet::new(&cfg, Display::Led(LedMetrics::default()), 1024.0, 768.0);
        assert_eq!(c.bank_width(), 0);
        assert_eq!(c.layout().crt.width, 1024.0);
        assert_eq!(c.min_inner_size(), (320, 240));
        // The bank's own measures still exist; nothing is drawn from them.
        assert_eq!(c.geometry().implicit_width, 184);
        assert!(!c.pointer_pressed(184.0));
        assert_eq!(c.cursor_moved(300.0), None);
        assert_eq!(c.cursor(), SeamCursor::Unclaimed);
    }

    #[test]
    fn the_hosts_floor_narrows_the_bank_and_moves_the_hint() {
        // One number, three places. A host whose font needs 1018 px of well
        // for its eighty columns leaves a 1024 px window nothing to spare, so
        // the bank falls to the display's least strip on the spot, with no
        // resize and no hand behind it; the hint stands at that least strip
        // beside the new floor; and the setting is untouched.
        let mut c = stock();
        assert_eq!(c.geometry().characters, 12);
        c.set_well_minimum(1018, 730);
        assert_eq!(c.geometry().characters, c.geometry().min_units);
        assert_eq!(c.configured_characters(), 12);
        assert_eq!(c.min_inner_size(), (154 + 1018, 730));
    }

    #[test]
    fn a_window_too_narrow_fits_the_bank_and_leaves_the_setting_alone() {
        let mut c = stock();
        c.set_well_minimum(320, 240);
        assert_eq!(c.geometry().characters, 12);
        assert_eq!(c.bank_width(), 184);

        c.resized(460.0, 768.0);
        let fitted = c.geometry().characters;
        assert!(
            fitted < 12 && fitted >= c.geometry().min_units,
            "the bank did not fit: {fitted}"
        );
        assert!(c.bank_width() < 184);
        assert_eq!(c.configured_characters(), 12);
    }

    #[test]
    fn widening_gives_the_bank_back_its_configured_width() {
        let mut c = stock();
        c.resized(460.0, 768.0);
        assert!(c.geometry().characters < 12);
        c.resized(1920.0, 768.0);
        assert_eq!(c.geometry().characters, 12);
        assert_eq!(c.bank_width(), 184);
    }

    #[test]
    fn the_hint_holds_still_while_the_bank_fits() {
        // The hint is a constraint on the window's width, so it does not move
        // with the window's width. Were it to, the shell would grow a window
        // to meet a hint its own narrowing had just raised.
        let mut c = stock();
        let hint = c.min_inner_size();
        c.resized(460.0, 768.0);
        assert_eq!(c.min_inner_size(), hint);
        c.resized(1920.0, 768.0);
        assert_eq!(c.min_inner_size(), hint);
    }

    #[test]
    fn a_drag_starts_from_the_bank_the_hand_can_see() {
        // The window is chosen to fit the bank strictly between the display's
        // least strip and the twelve the settings ask for, since a bank
        // already at its least cannot move and a bank at its configured width
        // has nothing to prove. Dragging one character inwards from the drawn
        // seam lands one below the drawn count; measured from the configured
        // count instead, the same pointer would land a character higher, and
        // the seam would jump out from under the hand on its first motion,
        // which is the fault this test stands against.
        let mut c = stock();
        c.resized(495.0, 768.0);
        let drawn = c.geometry().characters;
        assert!(
            drawn > c.geometry().min_units && drawn < 12,
            "the window did not fit the bank between the two bounds: {drawn}"
        );

        let edge = c.bank_width() as f64;
        assert!(c.pointer_pressed(edge));
        let landed = c.cursor_moved(edge - 8.0).map(|u| u.led_characters);
        assert_eq!(landed, Some(drawn - 1));
    }

    #[test]
    fn the_bank_width_the_window_hint_wants_is_the_scaled_one() {
        // The shells' constants are logical: a 3 px shoulder is 3 px of mock,
        // and on a 2x display it is 6 px of screen. The hint takes physical.
        let c = stock();
        assert_eq!(c.bank_width(), 184);
        assert_eq!(c.bank_width_physical(1.0), 184);
        assert_eq!(c.bank_width_physical(2.0), 368);
        // A fractional factor is the ordinary case on a scaled desktop.
        assert_eq!(c.bank_width_physical(1.5), 276);
        assert_eq!(c.bank_width_physical(1.25), 230);
    }

    #[test]
    fn a_resize_moves_the_well_and_leaves_the_bank_alone() {
        let mut c = stock();
        let before = c.bank_width();
        c.resized(1920.0, 1080.0);
        assert_eq!(c.bank_width(), before);
        assert_eq!(c.layout().crt.width, 1920.0 - 184.0);
        assert_eq!(c.layout().crt.height, 1080.0);
        assert_eq!(c.layout().bank.height, 1080.0);
    }

    #[test]
    fn a_drag_updates_the_cabinet_before_the_settings_come_back() {
        let mut c = stock();
        assert!(c.pointer_pressed(184.0));
        let update = c
            .cursor_moved(300.0)
            .expect("a 116 px drag lands somewhere");
        // The count and the width move together, and the width is the one the
        // window's minimum-size hint needs.
        assert_eq!(update.led_characters, 27);
        assert_eq!(update.bank_width, c.bank_width());
        assert_eq!(c.geometry().implicit_width as u32, update.bank_width);
        // The well gave up exactly what the bank took.
        assert_eq!(c.layout().crt.right(), 1024.0);
        assert_eq!(c.layout().crt.width, 1024.0 - update.bank_width as f64);
        // A settings reload carrying what the drag just wrote is a no-op.
        let mut cfg = Config::default();
        cfg.general.led_characters = update.led_characters;
        let width = c.apply_settings(&cfg, Display::Led(LedMetrics::default()));
        assert_eq!(width, update.bank_width);
    }

    #[test]
    fn a_drag_across_the_window_tracks_the_hand_the_whole_way() {
        // The loop closed: press, then walk the pointer out and back, with the
        // cabinet re-measuring itself at every step. What is checked is that
        // the bank's edge stays under the hand -- the property that an
        // accumulating drag would lose to rounding residue.
        let mut c = stock();
        assert!(c.pointer_pressed(184.0));
        let unit = LedMetrics::default().unit_width();
        let mut xs: Vec<f64> = (0..40).map(|i| 184.0 + i as f64 * 12.0).collect();
        xs.extend((0..40).map(|i| 664.0 - i as f64 * 12.0));
        for x in xs {
            c.cursor_moved(x);
            let error = (c.bank_width() as f64 - x).abs();
            assert!(
                error <= unit,
                "seam drifted {error} px from the hand at {x}"
            );
        }
        // ...and the floor still holds when the hand runs off the left edge.
        c.cursor_moved(-500.0);
        assert_eq!(c.cfg.general.led_characters, 8);
    }

    #[test]
    fn the_display_kit_travels_with_the_profile() {
        let mut cfg = Config::default();
        cfg.chassis.shell = config::Shell::Switchboard;
        cfg.chassis.channel_indicator = config::ChannelIndicator::Switch;
        cfg.chassis.channel_display = config::ChannelDisplay::Tape;
        let tape = TapeMetrics {
            unit_width: (177.0 - 24.0) / 12.0,
            end_pad: 12,
            min_characters: 8,
        };
        let c = Cabinet::new(&cfg, Display::Tape(tape), 1440.0, 900.0);
        // 8 + 170 + 18 + 177 + 29, which is the mock's own plate running
        // x 8..402 to the screen well's trough.
        assert_eq!(c.bank_width(), 402);
        assert_eq!(c.layout().crt.width, 1440.0 - 402.0);
    }
}
