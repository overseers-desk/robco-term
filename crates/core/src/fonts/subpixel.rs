//! The host's LCD stripe geometry, and FreeType's filter over it.
//!
//! Whether text renders subpixel-antialiased is not decided by this renderer
//! on its own: it asks fontconfig. The `FC_RGBA` value on a font's matched
//! pattern decides whether a glyph loads with LCD-filtered coverage
//! (`FT_RENDER_MODE_LCD`) or plain grayscale coverage (`FT_RENDER_MODE_NORMAL`).
//! So the chassis numerals are subpixel-antialiased on a machine whose
//! fontconfig says `rgb`, and grey-antialiased on one that says nothing --
//! **the same binary renders different pixels on two machines**, and any
//! measurement against a reference screenshot inherits that.
//!
//! That is the honest shape of this module: it is not "how the numerals are
//! drawn", it is "how the numerals are drawn *here*". A machine with no rgba
//! set gets [`Layout::None`], which is the grey raster this renderer produced
//! before this module existed, unchanged.
//!
//! # Reading the setting
//!
//! fontconfig's own file, parsed by fontconfig's own grammar:
//! `fontconfig-parser` is not a new dependency, it is the crate `fontdb`
//! already parses these files with (`cosmic-text` -> `fontdb/fontconfig`), so
//! the port and the font enumeration read one machine's configuration through
//! one parser rather than disagreeing about it. Linking libfontconfig would be
//! the other answer and costs a C library, a `pkg-config` build input and a
//! packaging dependency for one integer.
//!
//! [`host_layout`] merges `/etc/fonts/fonts.conf` and lets its own `<include>`
//! chain pull in `conf.d` and the user's file, which is the order fontconfig
//! processes them in (`50-user.conf` is what reaches
//! `$XDG_CONFIG_HOME/fontconfig/fonts.conf`); the user's files are then offered
//! again in case a system config does not include them, which
//! `FontConfig::merge_config` deduplicates by canonical path. This is
//! deliberately *not* `fontdb`'s order, which reads the user's file first and
//! relies on the same deduplication: for a key that later edits overwrite, the
//! order is the answer.
//!
//! Three things fontconfig does that this does not, all of which can only make
//! this read *more* grey than the host:
//!
//!   * **Conditional matches.** Only a `<match>` with no `<test>` counts here.
//!     A configuration that switches rgba per family (nothing ships one; the
//!     distro files are unconditional) would be missed.
//!   * **XSETTINGS.** A desktop's settings manager (GNOME, KDE) can override
//!     fontconfig's answer for `Xft.rgba` at runtime. There is no settings
//!     manager on the parity instrument's display, so fontconfig's on-disk
//!     answer is the actual answer there.
//!   * **Vertical stripes.** `vrgb`/`vbgr` panels are read and then treated as
//!     [`Layout::None`]: this filter is horizontal, and a vertical one that
//!     nothing in reach can be measured against would be a guess with a test
//!     around it.
//!
//! # The filter
//!
//! Coverage is sampled at thirds of a pixel -- the outline is rasterised three
//! times as wide, so each of the three stripes under a pixel gets its own
//! coverage -- and then convolved along the row with FreeType's default five-tap
//! FIR, `{0x08, 0x4D, 0x56, 0x4D, 0x08}` over 256
//! (`src/base/ftlcdfil.c`: the weights `FT_Library_SetLcdFilter` installs for
//! `FT_LCD_FILTER_DEFAULT`, applied by `ft_lcd_filter_fir`). The three-times
//! oversampling is FreeType's own too (`src/smooth/ftsmooth.c`: an
//! `FT_RENDER_MODE_LCD` render scales the outline by three horizontally and
//! filters the row that comes out).
//!
//! The filter is what keeps subpixel text from being a colour effect: without
//! it a stem edge lands entirely on one stripe and the numeral wears a red or
//! blue rim. Spreading each stripe's coverage over its neighbours trades that
//! for a little blur, and because the weights sum to exactly 256 the trade
//! keeps the ink: what a stripe gives away its neighbours receive, so the
//! inside of a stroke wider than the five taps stays fully covered on all
//! three channels. A stroke *narrower* than the taps is dimmed instead, which
//! is the blur half of the same bargain.
//!
//! This host's `/etc/fonts/conf.d/11-lcdfilter-default.conf` asks for
//! `lcddefault`, which is this filter; a host asking for `lcdlight` or
//! `lcdlegacy` would get this one anyway, since `FC_LCD_FILTER` is not read.
//!
//! # What it bought
//!
//! Measured with `xtask snap --deterministic` on this host
//! (`10-sub-pixel-rgb.conf` present, so the render is subpixel), over
//! the annunciator's numeral lane -- the 42x845 pixel column holding all
//! nineteen numerals, `x = 14, y = 65`. The residual against the golden
//! image splits orthogonally into an achromatic part and a chromatic one,
//! and it is the chromatic half this exists to close:
//!
//! | against golden | RMSE | luma | chroma | chroma share |
//! |---|---|---|---|---|
//! | grey (before) | 0.0315 | 0.0211 | 0.0235 | 55.4% |
//! | filtered      | 0.0212 | 0.0190 | 0.0093 | 19.4% |
//!
//! The two rows are one binary snapped twice, the "before" run pointed at an
//! empty `FONTCONFIG_FILE` so that [`host_layout`] answers [`Layout::None`]:
//! that path is the baseline grey renderer, reached by the
//! switch this module added rather than by rebuilding the old tree.
//!
//! Chroma RMSE down 60%, lane SSE down 55%, and the lane's residual stops
//! being mostly colour. Over the whole bank column the same change is
//! 0.0244 -> 0.0229, because the bank is mostly lamp wells and mouldings that
//! this does not touch. The furniture is painted deterministically, so both
//! figures are exact rather than noisy: two runs of one binary differ by
//! 0.00000 in the lane and 0.00006 over the bank, against a full-scene
//! self-floor of 0.0089 that lives entirely in the glass.
//!
//! Every one of those numbers is a statement about this machine. On a host
//! with no rgba set, the golden image is grey and so is this renderer's
//! output, and the chroma residual is zero on both sides before anything
//! here runs.

use std::path::{Path, PathBuf};

use fontconfig_parser::{
    Constant, EditMode, Expression, FontConfig, MatchTarget, Property, PropertyKind, Value,
};

/// The stripe order under a pixel, as far as this renderer models it.
///
/// The `FC_RGBA_*` values this is folded down from are fontconfig's own:
/// unknown 0, rgb 1, bgr 2, vrgb 3, vbgr 4, none 5.
#[derive(Clone, Copy, Debug, Default, PartialEq, Eq)]
pub enum Layout {
    /// Grey coverage, one value per pixel: what an unset, `none`, `unknown` or
    /// vertically striped host gets.
    #[default]
    None,
    /// Stripes left to right red, green, blue.
    Rgb,
    /// Stripes left to right blue, green, red.
    Bgr,
}

impl Layout {
    /// fontconfig's `FC_RGBA_*` integer, folded to what this filter can render.
    fn from_fc_rgba(rgba: Option<u32>) -> Self {
        match rgba {
            Some(1) => Layout::Rgb,
            Some(2) => Layout::Bgr,
            // 0 unknown, 3 vrgb, 4 vbgr, 5 none, absent: grey.
            _ => Layout::None,
        }
    }

    /// Whether a raster in this layout carries three coverages per pixel.
    pub fn is_subpixel(self) -> bool {
        self != Layout::None
    }
}

/// What this machine's fontconfig says the panel's stripes are, read once for
/// the life of the process.
///
/// Read once for the life of the process: fontconfig's configuration is
/// loaded at startup, and a running process would need restarting to see an
/// edit.
pub fn host_layout() -> Layout {
    use std::sync::OnceLock;

    static LAYOUT: OnceLock<Layout> = OnceLock::new();
    *LAYOUT.get_or_init(|| {
        let layout = layout_from_files(&default_config_files());
        log::debug!("fontconfig rgba resolves to {layout:?}");
        layout
    })
}

/// The files fontconfig would read, in the order it reads them.
fn default_config_files() -> Vec<PathBuf> {
    // fontconfig's own override, and the only one it honours when set.
    if let Ok(file) = std::env::var("FONTCONFIG_FILE") {
        return vec![PathBuf::from(file)];
    }
    let mut files = vec![PathBuf::from("/etc/fonts/fonts.conf")];
    let xdg = std::env::var("XDG_CONFIG_HOME")
        .map(PathBuf::from)
        .or_else(|_| std::env::var("HOME").map(|h| Path::new(&h).join(".config")));
    if let Ok(dir) = xdg {
        files.push(dir.join("fontconfig/fonts.conf"));
    }
    if let Ok(home) = std::env::var("HOME") {
        files.push(Path::new(&home).join(".fonts.conf"));
    }
    files
}

/// [`host_layout`] against a given set of configuration files, so the rules
/// above can be measured against files whose contents are known rather than
/// only against whatever the machine running the tests is configured with.
fn layout_from_files(files: &[PathBuf]) -> Layout {
    let mut config = FontConfig::default();
    for file in files {
        // A missing file is the ordinary case for the user's two, and a
        // machine with no fontconfig at all is a machine that renders grey.
        let _ = config.merge_config(file);
    }
    Layout::from_fc_rgba(rgba_first_value(&config))
}

/// The `rgba` value a consumer reading element 0 of the pattern would get.
///
/// A consumer reads element 0 (`FcPatternGetInteger(pattern, FC_RGBA, 0, &value)`),
/// and fontconfig's shipped `10-sub-pixel-rgb.conf` says so in as many words:
/// it uses `mode="append"` rather than `assign` precisely so that a desktop
/// that already put a value at the front keeps it, "so that most clients would
/// take a look at the first place only". So the edits are replayed for what
/// ends up at the front of the list, which is every mode's whole effect on the
/// one element anybody reads.
fn rgba_first_value(config: &FontConfig) -> Option<u32> {
    let mut first = None;
    for rule in &config.matches {
        // `scan` edits rewrite a font as it is catalogued, not as it is
        // rendered; `pattern` and `font` edits both reach the pattern the
        // engine reads.
        if rule.target == MatchTarget::Scan || !rule.tests.is_empty() {
            continue;
        }
        for edit in &rule.edits {
            let Property::Rgba(expr) = &edit.value else {
                continue;
            };
            let Some(value) = rgba_value(expr) else {
                continue;
            };
            match edit.mode {
                // Replaces the front element, or becomes it.
                EditMode::Assign
                | EditMode::AssignReplace
                | EditMode::Prepend
                | EditMode::PrependFirst => first = Some(value),
                // Lands behind whatever is already at the front.
                EditMode::Append | EditMode::AppendLast => first = first.or(Some(value)),
                EditMode::Delete | EditMode::DeleteAll => first = None,
            }
        }
    }
    first
}

/// `<const>rgb</const>` or `<int>1</int>`; anything computed is skipped, since
/// nothing that ships computes this one.
fn rgba_value(expr: &Expression) -> Option<u32> {
    match expr {
        Expression::Simple(Value::Constant(c)) => Constant::get_value(*c, PropertyKind::Rgba),
        Expression::Simple(Value::Int(i)) => Some(*i),
        _ => None,
    }
}

/// FreeType's `FT_LCD_FILTER_DEFAULT` weights, over 256.
pub const DEFAULT_FILTER: [u32; 5] = [0x08, 0x4D, 0x56, 0x4D, 0x08];

/// Fold a row-major raster sampled at thirds of a pixel down to one coverage
/// per stripe per pixel.
///
/// `samples` is `3 * width` columns wide: the coverage of each third of each
/// pixel. The five-tap FIR runs along the thirds, and the three thirds under a
/// pixel then become its red, green and blue coverages -- reversed for
/// [`Layout::Bgr`], which is the same panel wired the other way round.
///
/// The convolution reads zero past either end of the row rather than the pixel
/// beyond it. FreeType pads its bitmap by a whole pixel on each side and so
/// keeps the two thirds of a pixel of blur the filter throws outside the
/// glyph; here the raster is the `Text` item's own raster, whose width is what
/// the painting stack positions against, and growing it by a pixel to keep a
/// tail of at most `0x08/256` of an edge sample would move every numeral on
/// the plate to hold onto it.
pub fn fold(samples: &[u8], width: u32, height: u32, layout: Layout) -> Vec<[u8; 3]> {
    let stride = (width * 3) as usize;
    let mut out = vec![[0u8; 3]; (width * height) as usize];
    let mut row = vec![0u8; stride];
    for y in 0..height as usize {
        let src = &samples[y * stride..(y + 1) * stride];
        filter_row(src, &mut row);
        for x in 0..width as usize {
            let third = &row[x * 3..x * 3 + 3];
            out[y * width as usize + x] = match layout {
                Layout::Bgr => [third[2], third[1], third[0]],
                _ => [third[0], third[1], third[2]],
            };
        }
    }
    out
}

/// One row through the five-tap FIR: `ft_lcd_filter_fir`'s arithmetic, an
/// accumulation over 256 shifted back down and clamped.
fn filter_row(src: &[u8], dst: &mut [u8]) {
    for (i, out) in dst.iter_mut().enumerate() {
        let mut acc = 0u32;
        for (k, weight) in DEFAULT_FILTER.iter().enumerate() {
            let j = i as isize + k as isize - 2;
            if j >= 0 && (j as usize) < src.len() {
                acc += weight * src[j as usize] as u32;
            }
        }
        *out = (acc >> 8).min(255) as u8;
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A directory of fontconfig files, written out and read back through the
    /// real parser: the rules being measured are rules about fontconfig's
    /// grammar, and a hand-built `FontConfig` would let this module decide for
    /// itself what the grammar means.
    struct Configs(PathBuf);

    impl Configs {
        fn new(name: &str) -> Self {
            let dir = std::env::temp_dir().join(format!("robco-subpixel-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            Configs(dir)
        }

        /// One file holding `body` inside a `<fontconfig>` document.
        fn file(&self, name: &str, body: &str) -> PathBuf {
            let path = self.0.join(name);
            std::fs::write(&path, format!("<fontconfig>{body}</fontconfig>")).expect("write");
            path
        }

        fn layout(&self, files: &[PathBuf]) -> Layout {
            layout_from_files(files)
        }
    }

    impl Drop for Configs {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.0);
        }
    }

    const APPEND_RGB: &str = r#"<match target="pattern">
        <edit name="rgba" mode="append"><const>rgb</const></edit></match>"#;

    #[test]
    fn an_unconfigured_machine_renders_grey() {
        let c = Configs::new("empty");
        let nothing = c.file("empty.conf", "");
        assert_eq!(c.layout(&[nothing]), Layout::None);
        assert_eq!(c.layout(&[c.0.join("does-not-exist.conf")]), Layout::None);
        assert!(!Layout::None.is_subpixel());
    }

    /// The file fontconfig actually ships, verbatim in shape: a `target`ed
    /// match, no tests, `mode="append"`, a `<const>`.
    #[test]
    fn the_shipped_sub_pixel_rgb_file_reads_as_rgb() {
        let c = Configs::new("shipped");
        let f = c.file("10-sub-pixel-rgb.conf", APPEND_RGB);
        assert_eq!(c.layout(&[f]), Layout::Rgb);
    }

    /// `append` lands *behind* the front element, which is the whole reason
    /// the shipped file uses it: a desktop that has already assigned a value
    /// keeps it.
    #[test]
    fn an_append_does_not_displace_a_value_already_at_the_front() {
        let c = Configs::new("append");
        let bgr = c.file(
            "00-desktop.conf",
            r#"<match target="pattern">
               <edit name="rgba" mode="assign"><const>bgr</const></edit></match>"#,
        );
        let rgb = c.file("10-sub-pixel-rgb.conf", APPEND_RGB);
        assert_eq!(c.layout(&[bgr.clone(), rgb.clone()]), Layout::Bgr);
        // ...and the reverse order is the reverse answer, because now the
        // assign is the later edit and does displace the appended one.
        assert_eq!(c.layout(&[rgb, bgr]), Layout::Bgr);
    }

    /// `none` is a machine that has turned subpixel rendering off, and it is
    /// not the same statement as saying nothing: it overrides an earlier
    /// value.
    #[test]
    fn none_and_the_vertical_layouts_all_render_grey() {
        let c = Configs::new("off");
        let rgb = c.file("10-sub-pixel-rgb.conf", APPEND_RGB);
        for value in ["none", "vrgb", "vbgr", "unknown"] {
            let off = c.file(
                "99-off.conf",
                &format!(
                    r#"<match target="pattern">
                       <edit name="rgba" mode="assign"><const>{value}</const></edit></match>"#
                ),
            );
            assert_eq!(
                c.layout(&[rgb.clone(), off]),
                Layout::None,
                "rgba={value} should render grey"
            );
        }
    }

    /// A `<match>` with a `<test>` is a rule about *some* fonts, and this
    /// module only claims to know the unconditional default.
    #[test]
    fn a_conditional_match_is_not_the_default() {
        let c = Configs::new("conditional");
        let f = c.file(
            "50-family.conf",
            r#"<match target="font">
               <test name="family"><string>Iosevka</string></test>
               <edit name="rgba" mode="assign"><const>rgb</const></edit></match>"#,
        );
        assert_eq!(c.layout(&[f]), Layout::None);
    }

    /// The weights are FreeType's, and they sum to exactly 256 -- which is the
    /// property that makes the filter blur without moving ink.
    #[test]
    fn the_filter_weights_are_freetypes_and_conserve_coverage() {
        assert_eq!(DEFAULT_FILTER, [0x08, 0x4D, 0x56, 0x4D, 0x08]);
        assert_eq!(DEFAULT_FILTER.iter().sum::<u32>(), 256);

        // A fully covered run stays fully covered on every channel.
        let solid = vec![255u8; 3 * 5];
        let out = fold(&solid, 5, 1, Layout::Rgb);
        assert_eq!(out[2], [255, 255, 255], "{out:?}");
    }

    /// The one thing subpixel rendering *is*: a stroke's two edges wear
    /// opposite tints, and which edge wears which is the panel's stripe order.
    ///
    /// Under `rgb` the blue stripe is the rightmost of a pixel, so it is the
    /// stripe nearest a stroke standing to its right: the pixel left of the
    /// stroke goes blue, and the pixel to its right -- whose red stripe is the
    /// one nearest the ink -- goes red. That red edge on the right is the
    /// fringe an `rgba=rgb` screenshot of the chassis numerals carries.
    #[test]
    fn a_strokes_left_edge_goes_blue_and_its_right_edge_goes_red_under_rgb() {
        // Five pixels, the middle one inked across all three of its thirds.
        let mut samples = vec![0u8; 3 * 5];
        for s in samples.iter_mut().take(9).skip(6) {
            *s = 255;
        }

        let rgb = fold(&samples, 5, 1, Layout::Rgb);
        let (left, right) = (rgb[1], rgb[3]);
        assert!(left[2] > left[0], "no blue on the stroke's left: {rgb:?}");
        assert!(right[0] > right[2], "no red on the stroke's right: {rgb:?}");
        // The stroke itself takes no net tint -- it is symmetric, so the two
        // outer stripes lose the same amount to the blank on their side --
        // but it is dimmed, because a stroke one pixel wide is narrower than
        // the filter is: this is the blur the filter trades the fringes for.
        assert_eq!(rgb[2][0], rgb[2][2], "{rgb:?}");
        assert!(rgb[2][0] < 255 && rgb[2][1] > rgb[2][0], "{rgb:?}");

        // The same panel wired backwards puts the same two fringes on the
        // other edges, which is the whole content of `bgr`.
        let bgr = fold(&samples, 5, 1, Layout::Bgr);
        assert_eq!(bgr[1], [left[2], left[1], left[0]], "{bgr:?}");
        assert_eq!(bgr[3], [right[2], right[1], right[0]], "{bgr:?}");
    }

    /// The filter spreads an isolated stripe over its neighbours rather than
    /// leaving it alone, and keeps what it spreads: this is the difference
    /// between filtered subpixel text and the colour-fringed kind.
    #[test]
    fn the_filter_spreads_a_lone_stripe_without_losing_it() {
        let mut samples = vec![0u8; 3 * 3];
        samples[4] = 255; // the middle third of the middle pixel
        let out = fold(&samples, 3, 1, Layout::Rgb);
        let lit: u32 = out.iter().flatten().map(|&c| c as u32).sum();
        assert!(out[1][0] > 0 && out[1][2] > 0, "the stripe did not spread");
        // 255 in, and the weights sum to one, so 255 out give or take the
        // truncation FreeType does at each tap.
        assert!((250..=255).contains(&lit), "coverage went missing: {lit}");
    }
}
