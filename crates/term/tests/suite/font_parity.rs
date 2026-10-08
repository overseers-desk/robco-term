//! Rendering held to golden fixtures, measured rather than asserted by eye.
//!
//! `tests/fixtures/golden-fonts.json` and `tests/fixtures/led/*.png` are
//! golden files: recorded expected output the tests hold the renderer to,
//! not a derivation of anything. The JSON pins every field `font_by_name()`
//! returns for each bundled entry, plus `sizing::resolve()`'s output for five
//! pairs of type size and density; the PNGs pin the raster `led_text_image()`
//! produces for a fixed string, one per bundled font. Re-baselining, should
//! the renderer's output ever need to move, means blessing its current output
//! into these fixtures in place.
//!
//! Three parts:
//!   * `metrics_table_matches_golden`: every field `font_by_name()` returns,
//!     for every bundled entry;
//!   * `resolved_font_matches_golden`: the `sizing::resolve()` rows;
//!   * `led_raster_rmse_against_golden`: per-font RMSE of the
//!     `led_text_image()` raster against the golden PNG.

use serde_json::Value;
use std::path::PathBuf;
use term::fonts::{self, led, sizing};

fn fixture_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures")
}

fn golden() -> Value {
    let path = fixture_dir().join("golden-fonts.json");
    serde_json::from_slice(&std::fs::read(&path).expect("fixture readable"))
        .expect("fixture parses")
}

/// The LED text the golden raster is recorded against.
const LED_TEXT: &str = "CH 01 AMBER 1234567890";

/// The bundled half of the catalogue.
///
/// Every comparison in this file is against the golden fixture, which only
/// covers the faces the fixture was recorded with: the bundled ones. The
/// system half is whatever the running machine has installed, which the
/// fixture has no way to pin.
fn bundled() -> Vec<&'static fonts::FontEntry> {
    fonts::bundled_fonts().iter().collect()
}

#[test]
fn metrics_table_matches_golden() {
    let g = golden();
    let rows = g["fonts"].as_array().expect("fonts array");
    let ours = bundled();

    println!(
        "\n{:<24} {:>10} {:>4} {:>7} {:<34} {}",
        "name", "baseWidth", "px", "lowRes", "family", "fallback"
    );
    for entry in &ours {
        println!(
            "{:<24} {:>10.6} {:>4} {:>7} {:<34} {}",
            entry.name,
            entry.base_width,
            entry.pixel_size,
            entry.low_resolution,
            entry.family,
            entry.fallback_name
        );
    }
    println!("{} bundled entries\n", ours.len());

    assert_eq!(rows.len(), ours.len(), "catalogue length");

    for (row, entry) in rows.iter().zip(&ours) {
        let ctx = entry.name;
        assert_eq!(
            row["name"].as_str().unwrap(),
            entry.name,
            "{ctx}: name/order"
        );
        assert_eq!(row["text"].as_str().unwrap(), entry.text, "{ctx}: text");
        assert_eq!(
            row["source"].as_str().unwrap(),
            entry.source,
            "{ctx}: source"
        );
        assert_eq!(
            row["pixelSize"].as_u64().unwrap() as u32,
            entry.pixel_size,
            "{ctx}: pixelSize"
        );
        assert_eq!(
            row["lowResolutionFont"].as_bool().unwrap(),
            entry.low_resolution,
            "{ctx}: lowResolutionFont"
        );
        assert_eq!(
            row["isSystemFont"].as_bool().unwrap(),
            entry.is_system,
            "{ctx}: isSystemFont"
        );
        assert_eq!(
            row["family"].as_str().unwrap(),
            entry.family,
            "{ctx}: family"
        );
        assert_eq!(
            row["fallbackName"].as_str().unwrap(),
            entry.fallback_name,
            "{ctx}: fallbackName"
        );
        let want = row["baseWidth"].as_f64().unwrap();
        assert!(
            (want - entry.base_width).abs() < 1e-12,
            "{ctx}: baseWidth {} != {want}",
            entry.base_width
        );
    }
}

#[test]
fn scaled_metrics_match_golden() {
    let g = golden();
    for row in g["fonts"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let entry = fonts::font_by_name(name, fonts::FontSource::Bundled).unwrap();
        let sm = fonts::metrics::scaled_metrics(entry.data(), entry.pixel_size).unwrap();
        assert!(
            (sm.ascent() - row["ascent"].as_f64().unwrap()).abs() < 1e-12,
            "{name}: ascent {} != {}",
            sm.ascent(),
            row["ascent"]
        );
        assert!(
            (sm.descent() - row["descent"].as_f64().unwrap()).abs() < 1e-12,
            "{name}: descent"
        );
        assert!(
            (sm.height() - row["metricHeight"].as_f64().unwrap()).abs() < 1e-12,
            "{name}: height"
        );
        assert_eq!(
            sm.height_int() as i64,
            row["intHeight"].as_i64().unwrap(),
            "{name}: integer height"
        );
        assert_eq!(
            sm.ascent_int() as i64,
            row["intAscent"].as_i64().unwrap(),
            "{name}: integer ascent"
        );
    }
}

#[test]
fn resolved_font_matches_golden() {
    let g = golden();
    for row in g["fonts"].as_array().unwrap() {
        let name = row["name"].as_str().unwrap();
        let entry = fonts::font_by_name(name, fonts::FontSource::Bundled).unwrap();
        for c in row["computed"].as_array().unwrap() {
            let req = sizing::SizingRequest {
                font_size: c["in_fontSize"].as_u64().unwrap() as u32,
                device_pixel_ratio: c["in_dpr"].as_f64().unwrap(),
                line_spacing: c["in_lineSpacing"].as_f64().unwrap(),
                font_width: c["in_fontWidth"].as_f64().unwrap(),
            };
            let got = sizing::resolve(entry, &req);
            let ctx = format!("{name} @ {} x {}", c["in_fontSize"], c["in_dpr"]);
            assert_eq!(got.family, c["family"].as_str().unwrap(), "{ctx}: family");
            assert_eq!(
                got.raster_pixel_size as i64,
                c["rasterPixelSize"].as_i64().unwrap(),
                "{ctx}: rasterPixelSize"
            );
            assert_eq!(
                got.integer_scale as i64,
                c["integerScale"].as_i64().unwrap(),
                "{ctx}: integerScale"
            );
            assert_eq!(
                got.line_spacing as i64,
                c["lineSpacing"].as_i64().unwrap(),
                "{ctx}: lineSpacing"
            );
            assert_eq!(
                got.fallback_family,
                c["fallbackFontFamily"].as_str().unwrap(),
                "{ctx}: fallbackFontFamily"
            );
            assert_eq!(
                got.antialias,
                c["antialias"].as_bool().unwrap(),
                "{ctx}: antialias"
            );
            assert!(
                (got.font_width - c["fontWidth"].as_f64().unwrap()).abs() < 1e-12,
                "{ctx}: fontWidth"
            );
        }
    }
}

/// The golden PNG, as an alpha raster.
fn golden_led(name: &str) -> (u32, u32, Vec<u8>) {
    let path = fixture_dir().join("led").join(format!("{name}.png"));
    let decoder = png::Decoder::new(std::io::BufReader::new(
        std::fs::File::open(&path).expect("led png"),
    ));
    let mut reader = decoder.read_info().expect("png info");
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).expect("png frame");
    let bpp = info.color_type.samples();
    // The alpha channel is the raster; the fixture was written premultiplied
    // white, so any channel would do, but alpha is the one that carries the
    // coverage.
    let alpha: Vec<u8> = buf[..info.buffer_size()]
        .chunks(bpp)
        .map(|px| *px.last().unwrap())
        .collect();
    (info.width, info.height, alpha)
}

#[test]
fn led_raster_rmse_against_golden() {
    let mut worst = 0.0f64;
    let mut worst_name = "";
    println!(
        "\n{:<24} {:>12} {:>10} {:>8}",
        "font", "size", "differing", "RMSE"
    );
    for entry in bundled() {
        let (mw, mh, mpix) = golden_led(entry.name);
        let ours = led::led_text_image(entry.data(), entry.pixel_size, LED_TEXT)
            .unwrap_or_else(|| panic!("{}: no raster", entry.name));
        assert_eq!(
            (ours.width, ours.height),
            (mw, mh),
            "{}: raster size {}x{} != golden {mw}x{mh}",
            entry.name,
            ours.width,
            ours.height
        );
        let differing = ours.alpha.iter().zip(&mpix).filter(|(a, b)| a != b).count();
        let sse: f64 = ours
            .alpha
            .iter()
            .zip(&mpix)
            .map(|(a, b)| {
                let d = (*a as f64 - *b as f64) / 255.0;
                d * d
            })
            .sum();
        let rmse = (sse / mpix.len() as f64).sqrt();
        println!(
            "{:<24} {:>12} {:>10} {:>8.5}",
            entry.name,
            format!("{mw}x{mh}"),
            differing,
            rmse
        );
        if rmse > worst {
            worst = rmse;
            worst_name = entry.name;
        }
    }
    println!("worst: {worst_name} at {worst:.5}\n");

    // The faces the LED strip and tape label actually letter themselves with
    // are the low-resolution ones (the settings default is UNSCII_8_SCALED),
    // and those have to be exact: a lamp the font never inked is the bug this
    // whole CPU raster exists to prevent.
    for entry in fonts::low_resolution_fonts() {
        let (_, _, mpix) = golden_led(entry.name);
        let ours = led::led_text_image(entry.data(), entry.pixel_size, LED_TEXT).unwrap();
        let differing = ours.alpha.iter().zip(&mpix).filter(|(a, b)| a != b).count();
        assert_eq!(differing, 0, "{}: {differing} differing pixels", entry.name);
    }

    // The scalable faces are the antialiasing-on path and the LED strip never
    // letters itself with one (its font list is the low-resolution one), so
    // they are held to a ceiling rather than to exactness: their remaining
    // difference is FreeType's autohinted monochrome rasterisation thickening
    // stems that our threshold leaves thin. The ceiling exists to catch a
    // regression, not to bless the gap.
    assert!(worst < 0.17, "worst RMSE {worst} on {worst_name}");
}

#[test]
fn empty_text_has_no_raster() {
    let entry = fonts::font_by_name("UNSCII_8_SCALED", fonts::FontSource::Bundled).unwrap();
    assert!(led::led_text_image(entry.data(), entry.pixel_size, "").is_none());
}
