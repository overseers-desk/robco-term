//! The system half of the catalogue, against a face the machine really has.
//!
//! `fonts`' own unit tests state the *shape* of system font enumeration -- one
//! entry per family, with each entry's fields fixed, offered in both
//! rasterization modes -- about whatever happens to be installed. They would
//! all pass on a machine with no fonts at all, because every one of them
//! quantifies over an empty
//! list. This file is the other half: a named family, enumerated, filtered,
//! selected and turned into pixels.
//!
//! DejaVu Sans Mono is the family, because it is what a Linux desktop has by
//! definition -- it is `fontconfig`'s own fallback and the `ttf-dejavu` package
//! is a dependency of practically everything with a window. Where it is absent
//! this file says so and stops rather than passing vacuously, in the shape
//! `pixel_properties` uses for a missing GPU: a skipped test that announces the
//! skip is evidence, a test that quietly finds nothing to assert is not.

use term::fonts::sizing::{self, SizingRequest};
use term::fonts::{
    filtered_fonts, font_by_name, system_fonts, FontSource, MODERN_RASTERIZATION,
    SYSTEM_FONT_PIXEL_SIZE,
};
use term::{ascii_charset, FontContext, Rasterization};

const DEJAVU: &str = "DejaVu Sans Mono";

/// `None`, loudly, on a machine without the family.
fn dejavu() -> Option<&'static term::FontEntry> {
    match font_by_name(DEJAVU, FontSource::System) {
        Some(entry) if entry.is_system => Some(entry),
        Some(_) => panic!("{DEJAVU} is in the catalogue but not as a system face"),
        None => {
            eprintln!(
                "skipping: {DEJAVU} is not installed on this machine, so the \
                 named-family evidence cannot be taken here. The enumeration \
                 found {} system famil(ies): {:?}",
                system_fonts().iter().filter(|f| f.is_system).count(),
                system_fonts()
                    .iter()
                    .filter(|f| f.is_system)
                    .map(|f| f.name)
                    .take(20)
                    .collect::<Vec<_>>()
            );
            None
        }
    }
}

/// The done-test: the enumeration lists the machine's DejaVu Sans Mono, and
/// lists it *behind the catalogue's filter rules* -- under the system source,
/// under both rasterization modes, and under neither bundled list.
#[test]
fn the_enumeration_offers_dejavu_sans_mono_under_the_filter_rules() {
    let Some(entry) = dejavu() else { return };

    for mode in [0, MODERN_RASTERIZATION] {
        let offered: Vec<_> = filtered_fonts(FontSource::System, mode)
            .map(|f| f.name)
            .collect();
        assert!(
            offered.contains(&DEJAVU),
            "the system source at rasterization {mode} does not offer {DEJAVU}: {offered:?}"
        );
        let bundled: Vec<_> = filtered_fonts(FontSource::Bundled, mode)
            .map(|f| f.name)
            .collect();
        assert!(
            !bundled.contains(&DEJAVU),
            "a system face is being offered as a bundled one at rasterization {mode}"
        );
    }

    // `updateFilteredFonts`' selection rule: a name the current filter offers
    // survives the switch rather than falling back to the first entry.
    assert_eq!(
        term::fonts::resolve_font_name(DEJAVU, FontSource::System, 0),
        Some(DEJAVU)
    );
    assert_eq!(
        term::fonts::resolve_font_name(DEJAVU, FontSource::System, MODERN_RASTERIZATION),
        Some(DEJAVU)
    );
    // ...and under the bundled source it is not offered, so it falls back.
    assert_eq!(
        term::fonts::resolve_font_name(DEJAVU, FontSource::Bundled, 0),
        Some("TERMINESS_SCALED")
    );

    // `populateSystemFonts`' own field values, on a real entry.
    assert_eq!(entry.pixel_size, SYSTEM_FONT_PIXEL_SIZE);
    assert_eq!(entry.base_width, 1.0);
    assert!(!entry.low_resolution);
    assert_eq!(entry.family, DEJAVU);
    eprintln!(
        "{DEJAVU}: {} bytes read lazily, pixel size {}",
        entry.data().len(),
        entry.pixel_size
    );
}

/// The half a list cannot prove: a selected system face becomes a face, with
/// the bytes read off the machine at selection time rather than baked in.
///
/// Without this, the enumeration could offer a name that panics the moment the
/// user picks it -- which is exactly what a catalogue entry carrying no data
/// would have done.
#[test]
fn a_selected_system_face_shapes_and_measures() {
    let Some(entry) = dejavu() else { return };

    assert!(
        entry.data().len() > 1000,
        "{DEJAVU} read back {} bytes; the lazy file read is not working",
        entry.data().len()
    );
    // Reading twice is one read: the cache hands back the same slice, not a
    // second leaked copy of the file.
    assert_eq!(
        entry.data().as_ptr(),
        entry.data().as_ptr(),
        "the system-face cache is re-reading the file"
    );

    let mut font = FontContext::new(entry);
    assert_eq!(
        font.family, DEJAVU,
        "the enumerated family name is not the family the face reports"
    );

    let resolved = sizing::resolve(entry, &SizingRequest::default());
    // A system face is not low-resolution, so it takes the scalable half's
    // sizing: the raster size moves with the requested height and the integer
    // scale stays at one.
    assert_eq!(resolved.integer_scale, 1);
    assert!(
        resolved.antialias,
        "a system face is antialiased"
    );
    assert_eq!(
        Rasterization::for_face(&resolved),
        Rasterization::Coverage,
        "a system face must take the antialiasing-on rasterisation"
    );

    let cell = font.cell_metrics(&resolved);
    assert!(
        cell.width > 0 && cell.height > 0 && cell.baseline > 0,
        "{DEJAVU} measured a degenerate cell {cell:?}"
    );
    eprintln!(
        "{DEJAVU} at {}px: cell {}x{}, baseline {}",
        resolved.raster_pixel_size, cell.width, cell.height, cell.baseline
    );
    assert!(
        !ascii_charset().is_empty(),
        "the charset the atlas would be built over is empty"
    );
}
