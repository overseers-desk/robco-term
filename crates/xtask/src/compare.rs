//! Consolidates the RMSE-over-PNG measurement three earlier efforts each
//! wrote ad hoc: `crates/core/tests/suite/font_parity.rs`'s single-channel
//! `led_raster_rmse_against_golden`, `crates/core/src/fonts/subpixel.rs`'s
//! luma/chroma split (documented there, never checked in as reusable code),
//! and the whole-scene/glass/bank figures recorded by hand, which were
//! produced by shelling out to ImageMagick's `compare -metric RMSE` rather
//! than by anything in this repository. This is pure Rust: the `png` crate
//! is already in the workspace graph via `term`, so no ImageMagick
//! invocation is needed to decode two screenshots and difference them.
//!
//! # The metric
//!
//! Both images are decoded to 8-bit RGB (an alpha channel, if present, is
//! dropped -- `import`'s screenshots carry none, and a comparison exists to
//! judge colour, not compositing). For a pixel with per-channel differences
//! `(dr, dg, db)`, each normalized to `[-1, 1]` by dividing by 255:
//!
//! - **RMSE** is the root-mean-square of `dr, dg, db` pooled across every
//!   pixel and channel in the compared region -- the direct three-channel
//!   extension of `font_parity.rs`'s single-channel formula.
//! - **luma** is the RMS of the per-pixel *average* difference
//!   `avg = (dr + dg + db) / 3`: the achromatic component, the difference a
//!   greyscale rendering of the same two pixels would show.
//! - **chroma** is the RMS of what's left: `(dr - avg, dg - avg, db - avg)`
//!   per pixel, pooled the same way as RMSE.
//!
//! `avg` is the projection of `(dr, dg, db)` onto the equal-channel axis, so
//! luma and chroma are pixelwise orthogonal and the split is exact, not an
//! approximation: `RMSE² = luma² + chroma²` (`Metrics::chroma_share` reports
//! `chroma² / (luma² + chroma²)`, matching the "chroma share" column
//! `subpixel.rs`'s doc table already reports by hand). Checked in
//! `pythagorean_identity_holds_on_a_mixed_fixture` below.
//!
//! # Regions
//!
//! `--region bank` and `--region glass` carry the two crops this repo's
//! measurements actually use, so a caller can name the region instead of
//! recomputing pixel offsets:
//!
//! - **bank**: `x` in `0..247`, full height. 247 is not an arbitrary crop --
//!   it is the shipped default profile's own bank width
//!   (`Cabinet::bank_width`, pinned by
//!   `chassis::a_cabinet_built_from_the_shipped_profile_alone_stands_247_px_wide`):
//!   3 (bank padding) + 46 (numeral lane) + 16 (column gap) + 168 (twelve
//!   characters of UNSCII_8_SCALED, the shipped LED font's measured cell) +
//!   14 (right padding). A binary snapped at a different profile or a
//!   different channel-bank width needs `--crop`, not this region.
//! - **glass**: the remainder of the frame after the bank column, inset 10px
//!   on every side, at DPR 1 -- the convention this repo's recorded
//!   glass-crop figures were measured at. A screenshot taken at a different
//!   DPR needs `--crop` with hand-scaled offsets; this
//!   command does not read DPR out of the PNG.
//! - **full**: no crop.
//!
//! `--crop x,y,w,h` takes a literal pixel rectangle for anything the named
//! regions don't cover.
//!
//! # Workflow
//!
//! `xtask snap --deterministic` both binaries at the same profile and size,
//! then `xtask compare` per region. Before trusting a cross-binary number,
//! establish the self-floor: snap the *same* binary twice (a fresh
//! Xvfb/scratch-HOME run each time, deterministic content) and compare those
//! two screenshots. That number is the noise this host's rendering jitter
//! contributes on its own -- curvature resampling, antialiasing, the bloom
//! pass -- and a cross-binary figure close to it is not evidence of parity,
//! it is evidence of measuring noise. Only a cross-binary figure well above
//! both binaries' self-floors reflects an actual rendering difference.
use std::path::{Path, PathBuf};

use anyhow::{bail, Context, Result};
use clap::ValueEnum;

/// The shipped default profile's bank width, pixels, at DPR 1. See the
/// module doc for the derivation and the test that pins it.
const BANK_WIDTH: u32 = 247;

/// The glass crop's inset from the frame edge and the bank/glass seam,
/// pixels, at DPR 1.
const GLASS_INSET: u32 = 10;

#[derive(Clone, Copy, Debug, ValueEnum)]
#[value(rename_all = "lower")]
pub enum Region {
    Bank,
    Glass,
    Full,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Crop {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// Parses `--crop x,y,w,h`.
pub fn parse_crop(s: &str) -> Result<Crop, String> {
    let parts: Vec<&str> = s.split(',').collect();
    let [x, y, w, h] = parts.as_slice() else {
        return Err(format!("expected x,y,w,h, got {s:?}"));
    };
    let parse = |field: &str, name: &str| {
        field
            .trim()
            .parse::<u32>()
            .map_err(|e| format!("{name} {field:?} in {s:?}: {e}"))
    };
    Ok(Crop {
        x: parse(x, "x")?,
        y: parse(y, "y")?,
        w: parse(w, "w")?,
        h: parse(h, "h")?,
    })
}

pub struct CompareArgs {
    pub a: PathBuf,
    pub b: PathBuf,
    pub crop: Option<Crop>,
    pub region: Option<Region>,
    pub channels: bool,
}

pub fn run(args: CompareArgs) -> Result<()> {
    let (aw, ah, apix) = load_rgb(&args.a)?;
    let (bw, bh, bpix) = load_rgb(&args.b)?;
    if (aw, ah) != (bw, bh) {
        bail!(
            "size mismatch: {} is {aw}x{ah}, {} is {bw}x{bh}",
            args.a.display(),
            args.b.display()
        );
    }

    let crop = resolve_crop(args.crop, args.region, aw, ah)?;
    let ca = crop_rgb(&apix, aw, crop);
    let cb = crop_rgb(&bpix, bw, crop);

    let metrics = Metrics::compute(&ca, &cb);
    if args.channels {
        println!(
            "RMSE {:.5}  luma {:.5}  chroma {:.5}  chroma-share {:.1}%",
            metrics.rmse,
            metrics.luma,
            metrics.chroma,
            metrics.chroma_share() * 100.0
        );
    } else {
        println!("{:.5}", metrics.rmse);
    }
    Ok(())
}

fn resolve_crop(crop: Option<Crop>, region: Option<Region>, w: u32, h: u32) -> Result<Crop> {
    let crop = match (crop, region) {
        (Some(c), None) => c,
        (None, Some(Region::Full)) | (None, None) => Crop { x: 0, y: 0, w, h },
        (None, Some(Region::Bank)) => {
            if w <= BANK_WIDTH {
                bail!("image is only {w}px wide, narrower than the {BANK_WIDTH}px bank region");
            }
            Crop {
                x: 0,
                y: 0,
                w: BANK_WIDTH,
                h,
            }
        }
        (None, Some(Region::Glass)) => {
            let x0 = BANK_WIDTH + GLASS_INSET;
            if w <= x0 + GLASS_INSET || h <= 2 * GLASS_INSET {
                bail!(
                    "image ({w}x{h}) is too small for the glass region \
                     (bank {BANK_WIDTH}px + {GLASS_INSET}px inset on every side)"
                );
            }
            Crop {
                x: x0,
                y: GLASS_INSET,
                w: w - x0 - GLASS_INSET,
                h: h - 2 * GLASS_INSET,
            }
        }
        (Some(_), Some(_)) => bail!("--crop and --region are mutually exclusive"),
    };
    if crop.x + crop.w > w || crop.y + crop.h > h {
        bail!(
            "crop {}x{}+{}+{} does not fit inside a {w}x{h} image",
            crop.w,
            crop.h,
            crop.x,
            crop.y
        );
    }
    if crop.w == 0 || crop.h == 0 {
        bail!("crop is empty ({}x{})", crop.w, crop.h);
    }
    Ok(crop)
}

/// Decodes any PNG the `png` crate can read to interleaved 8-bit RGB,
/// dropping alpha and expanding palette/grayscale/16-bit sources -- the same
/// decode shape `font_parity.rs::golden_led` uses for its fixtures, widened
/// from a single kept channel to three.
fn load_rgb(path: &Path) -> Result<(u32, u32, Vec<u8>)> {
    let file = std::fs::File::open(path).with_context(|| format!("opening {}", path.display()))?;
    let mut decoder = png::Decoder::new(std::io::BufReader::new(file));
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder
        .read_info()
        .with_context(|| format!("reading PNG header: {}", path.display()))?;
    let mut buf = vec![0; reader.output_buffer_size().expect("output buffer size")];
    let info = reader
        .next_frame(&mut buf)
        .with_context(|| format!("decoding PNG frame: {}", path.display()))?;
    let bpp = info.color_type.samples();
    let data = &buf[..info.buffer_size()];
    let rgb: Vec<u8> = match info.color_type {
        png::ColorType::Rgb => data.to_vec(),
        png::ColorType::Rgba => data
            .chunks_exact(bpp)
            .flat_map(|px| [px[0], px[1], px[2]])
            .collect(),
        png::ColorType::Grayscale => data.iter().flat_map(|&g| [g, g, g]).collect(),
        png::ColorType::GrayscaleAlpha => data
            .chunks_exact(bpp)
            .flat_map(|px| [px[0], px[0], px[0]])
            .collect(),
        other => bail!(
            "{}: unsupported PNG color type after expansion: {other:?}",
            path.display()
        ),
    };
    Ok((info.width, info.height, rgb))
}

/// Pulls a `Crop` out of a full-width interleaved-RGB buffer.
fn crop_rgb(pix: &[u8], width: u32, crop: Crop) -> Vec<u8> {
    let mut out = Vec::with_capacity((crop.w * crop.h * 3) as usize);
    for row in crop.y..crop.y + crop.h {
        let start = ((row * width + crop.x) * 3) as usize;
        let end = start + (crop.w * 3) as usize;
        out.extend_from_slice(&pix[start..end]);
    }
    out
}

pub struct Metrics {
    pub rmse: f64,
    pub luma: f64,
    pub chroma: f64,
}

impl Metrics {
    /// `a` and `b` are equal-length interleaved RGB8 buffers. See the module
    /// doc for the luma/chroma derivation.
    fn compute(a: &[u8], b: &[u8]) -> Self {
        assert_eq!(a.len(), b.len());
        assert_eq!(a.len() % 3, 0);
        let pixels = a.len() / 3;

        let mut total_sse = 0.0f64;
        let mut luma_sse_sum = 0.0f64; // sum of per-pixel avg^2, not yet normalized
        for i in 0..pixels {
            let d = |c: usize| (a[3 * i + c] as f64 - b[3 * i + c] as f64) / 255.0;
            let (dr, dg, db) = (d(0), d(1), d(2));
            total_sse += dr * dr + dg * dg + db * db;
            let avg = (dr + dg + db) / 3.0;
            luma_sse_sum += avg * avg;
        }

        let samples = (pixels * 3) as f64;
        let rmse = (total_sse / samples).sqrt();
        let luma = (luma_sse_sum / pixels as f64).sqrt();
        // total_sse = sum(3*avg^2 + chroma_pixel) = 3*luma_sse_sum + chroma_sse_sum
        let chroma_sse_sum = (total_sse - 3.0 * luma_sse_sum).max(0.0);
        let chroma = (chroma_sse_sum / samples).sqrt();

        Metrics { rmse, luma, chroma }
    }

    pub fn chroma_share(&self) -> f64 {
        let (l2, c2) = (self.luma * self.luma, self.chroma * self.chroma);
        if l2 + c2 == 0.0 {
            0.0
        } else {
            c2 / (l2 + c2)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures/compare")
            .join(name)
    }

    /// The fixture pair pins the RMSE math itself. `a.png` is two black
    /// pixels; `b.png` is `(255, 0, 0)` then `(0, 0, 0)` -- one full-scale
    /// red diff, one identical pixel. By hand, normalizing to `[0, 1]`:
    ///
    /// - pixel 0: `(dr, dg, db) = (1, 0, 0)`, `sse = 1`, `avg = 1/3`
    /// - pixel 1: all zero
    ///
    /// `total_sse = 1`, pooled over `2*3 = 6` samples: `RMSE = sqrt(1/6)`.
    /// `luma_sse_sum = (1/3)^2 = 1/9`, pooled over 2 pixels:
    /// `luma = sqrt(1/18)`. `chroma_sse_sum = 1 - 3*(1/9) = 2/3`, pooled over
    /// 6 samples: `chroma = sqrt(1/9) = 1/3` exactly.
    #[test]
    fn fixture_rmse_matches_the_hand_computed_value() {
        let (aw, ah, apix) = load_rgb(&fixture("a.png")).unwrap();
        let (bw, bh, bpix) = load_rgb(&fixture("b.png")).unwrap();
        assert_eq!((aw, ah), (2, 1));
        assert_eq!((bw, bh), (2, 1));

        let m = Metrics::compute(&apix, &bpix);
        let eps = 1e-9;
        assert!(
            (m.rmse - (1.0f64 / 6.0).sqrt()).abs() < eps,
            "rmse={}",
            m.rmse
        );
        assert!(
            (m.luma - (1.0f64 / 18.0).sqrt()).abs() < eps,
            "luma={}",
            m.luma
        );
        assert!((m.chroma - 1.0 / 3.0).abs() < eps, "chroma={}", m.chroma);
    }

    /// The orthogonal split holds exactly, not just on the two-pixel
    /// fixture: this is the identity `chroma_share` and the module doc both
    /// rely on, checked against a handful of pixels with independent,
    /// non-matching channel differences (no shortcut like "all channels
    /// equal" or "only one channel differs" available to hide a mistake).
    #[test]
    fn pythagorean_identity_holds_on_a_mixed_fixture() {
        let a: Vec<u8> = vec![10, 200, 40, 0, 0, 0, 255, 255, 255];
        let b: Vec<u8> = vec![250, 5, 90, 255, 128, 3, 0, 10, 244];
        let m = Metrics::compute(&a, &b);
        let lhs = m.rmse * m.rmse;
        let rhs = m.luma * m.luma + m.chroma * m.chroma;
        assert!(
            (lhs - rhs).abs() < 1e-12,
            "rmse^2={lhs} luma^2+chroma^2={rhs}"
        );
    }

    /// Two pixels, identical images: every metric is exactly zero, not just
    /// small -- the floor of the instrument itself, independent of any
    /// screenshot's rendering jitter.
    #[test]
    fn identical_images_measure_zero() {
        let px = vec![12u8, 34, 56, 78, 90, 123];
        let m = Metrics::compute(&px, &px);
        assert_eq!((m.rmse, m.luma, m.chroma), (0.0, 0.0, 0.0));
    }

    #[test]
    fn parse_crop_reads_x_y_w_h() {
        assert_eq!(
            parse_crop("1,2,3,4").unwrap(),
            Crop {
                x: 1,
                y: 2,
                w: 3,
                h: 4
            }
        );
        assert!(parse_crop("1,2,3").is_err());
        assert!(parse_crop("1,2,3,x").is_err());
    }

    #[test]
    fn region_bank_is_the_shipped_247px_column_at_full_height() {
        let crop = resolve_crop(None, Some(Region::Bank), 1448, 1086).unwrap();
        assert_eq!(
            crop,
            Crop {
                x: 0,
                y: 0,
                w: 247,
                h: 1086
            }
        );
    }

    #[test]
    fn region_glass_is_the_remainder_inset_10px() {
        let crop = resolve_crop(None, Some(Region::Glass), 1448, 1086).unwrap();
        assert_eq!(
            crop,
            Crop {
                x: 257,
                y: 10,
                w: 1448 - 257 - 10,
                h: 1086 - 20
            }
        );
    }

    #[test]
    fn region_full_is_the_whole_frame() {
        let crop = resolve_crop(None, Some(Region::Full), 100, 50).unwrap();
        assert_eq!(
            crop,
            Crop {
                x: 0,
                y: 0,
                w: 100,
                h: 50
            }
        );
    }

    #[test]
    fn crop_and_region_together_is_an_error() {
        let c = Crop {
            x: 0,
            y: 0,
            w: 1,
            h: 1,
        };
        assert!(resolve_crop(Some(c), Some(Region::Full), 100, 50).is_err());
    }

    #[test]
    fn a_crop_that_does_not_fit_is_an_error() {
        let c = Crop {
            x: 90,
            y: 0,
            w: 20,
            h: 10,
        };
        assert!(resolve_crop(Some(c), None, 100, 50).is_err());
    }
}
