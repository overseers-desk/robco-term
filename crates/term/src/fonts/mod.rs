//! The bundled font catalogue.
//!
//! Three things live here that no text stack can supply for us:
//!
//!   * the catalogue itself, one entry per bundled font, carrying the
//!     metadata (`base_width`, `pixel_size`, `low_resolution`,
//!     `fallback_name`) that decides how a face may be rasterised;
//!   * [`metrics`], the scaled-metric arithmetic (see that module for the
//!     rule and the evidence);
//!   * [`sizing`], the arithmetic from a requested type size to a raster
//!     size and a whole-number magnification;
//!   * [`raster`], the one rule for turning a face into pixels -- the
//!     embedded bitmap strike first, the outline only as a fallback -- which
//!     both rasterising callers in this crate share.
//!
//! System fonts are [`system`]'s, enumerated through `fontdb` and appended
//! to the catalogue after the bundled faces. The `is_system` flag is what
//! the filtering rules read to tell the two halves apart.
//!
//! There are two catalogues, not one, and which one a caller gets is an
//! argument rather than a default: [`bundled_fonts`] is every face this
//! binary carries and never touches the machine, [`system_fonts`] is those
//! plus whatever the machine has and is the only function here that walks
//! a font directory. A profile asking for bundled faces therefore never
//! pays for an enumeration it will not offer.

pub mod led;
pub mod metrics;
pub mod raster;
pub mod sizing;
pub mod subpixel;
pub mod system;
pub mod text;

use std::path::PathBuf;
use std::sync::OnceLock;

/// The rasterization value that means "modern": it selects the
/// non-low-resolution half of the catalogue.
pub const MODERN_RASTERIZATION: i32 = 4;
/// The pixel size at which system (non-bundled) fonts render.
pub const SYSTEM_FONT_PIXEL_SIZE: u32 = 32;

/// Which half of the catalogue the font list offers.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum FontSource {
    Bundled = 0,
    System = 1,
}

/// One row of the bundled font table, before the family is resolved and
/// `base_width` is recomputed for low-resolution faces.
#[derive(Clone, Copy, Debug)]
struct BundledFont {
    name: &'static str,
    text: &'static str,
    source: &'static str,
    /// The literal `base_width` value for this entry. Used as-is for the
    /// scalable faces and as the degenerate-metrics fallback; for a
    /// low-resolution face it is normally replaced by the computed value.
    fallback_base_width: f64,
    pixel_size: u32,
    low_resolution: bool,
    fallback_name: &'static str,
    data: &'static [u8],
}

/// A catalogue entry as `fontByName()` reports it.
#[derive(Clone, Debug, PartialEq)]
pub struct FontEntry {
    /// Stable key, e.g. `"TERMINESS_SCALED"`. What settings persist.
    pub name: &'static str,
    /// Menu label, e.g. `"Terminess"`. Empty for a face carried only to
    /// cover another's gaps, which is what keeps it out of every list a
    /// user chooses from.
    pub text: &'static str,
    /// The bundled resource path, `:/fonts/<dir>/<file>`, used as the
    /// entry's stable identifier.
    pub source: &'static str,
    /// Horizontal cell-width factor. Metric-derived for a low-resolution
    /// face, the declared literal otherwise.
    pub base_width: f64,
    /// The size this face is designed at. Load bearing: a low-resolution face
    /// is rasterised here and nowhere else.
    pub pixel_size: u32,
    /// True for the bitmap/pixel faces. Gates antialiasing and integer
    /// scaling, and selects which half of the catalogue is offered.
    pub low_resolution: bool,
    /// True for the faces [`system`] enumerated off the machine, false for the
    /// bundled ones.
    pub is_system: bool,
    /// Family name as the font itself reports it.
    pub family: String,
    /// Catalogue name of the face that covers this one's gaps, or `""`.
    pub fallback_name: &'static str,
    /// Where the face's bytes come from. Private, because the two arms are not
    /// the caller's business: [`FontEntry::data`] is.
    data: FontData,
}

/// A face's bytes, or where to get them.
///
/// A bundled face is `include_bytes!`, so it is already `'static` and already
/// resident. A system face is a path the enumeration found, read the first time
/// somebody asks it to become pixels: the machine's monospace list runs to
/// dozens of files and loading all of them to build a menu would be paying
/// megabytes for a list of names.
#[derive(Clone, Debug, PartialEq)]
enum FontData {
    Bundled(&'static [u8]),
    /// The file and which face inside it. The index is carried because a `.ttc`
    /// holds several and the reader will need it once anything in the tree
    /// shapes from a collection.
    System(PathBuf, u32),
}

impl FontEntry {
    /// The face's bytes.
    ///
    /// For a bundled face this is the `include_bytes!` slice and costs nothing.
    /// For a system face it is the file, read and cached on first use
    /// ([`system::face_data`]); an unreadable file comes back empty, which the
    /// atlas reports as a face it cannot build rather than as a wrong picture.
    pub fn data(&self) -> &'static [u8] {
        match &self.data {
            FontData::Bundled(data) => data,
            FontData::System(path, _) => system::face_data(path),
        }
    }
}

macro_rules! bundled {
    ($name:literal, $text:literal, $dir:literal, $file:literal,
     $base_width:literal, $pixel_size:literal, $low:literal, $fallback:literal) => {
        BundledFont {
            name: $name,
            text: $text,
            source: concat!(":/fonts/", $dir, "/", $file),
            fallback_base_width: $base_width,
            pixel_size: $pixel_size,
            low_resolution: $low,
            fallback_name: $fallback,
            data: include_bytes!(concat!("../../assets/fonts/", $dir, "/", $file)),
        }
    };
}

/// The bundled font table, in declared order: the low-resolution faces
/// first, then the scalable ones, with Departure Mono (low-resolution)
/// sitting among the latter. The list's order is the font menu's order, so
/// the order itself is meaningful, not incidental.
#[rustfmt::skip]
const BUNDLED: &[BundledFont] = &[
    bundled!("TERMINESS_SCALED", "Terminess", "terminus", "TerminessNerdFontMono-Regular.ttf", 1.0, 12, true, ""),
    bundled!("BIGBLUE_TERMINAL_SCALED", "BigBlue Terminal", "bigblue-terminal", "BigBlueTerm437NerdFontMono-Regular.ttf", 1.0, 12, true, ""),
    bundled!("EXCELSIOR_SCALED", "Fixedsys Excelsior", "fixedsys-excelsior", "FSEX301-L2.ttf", 1.0, 16, true, "UNIFONT"),
    bundled!("GREYBEARD_SCALED", "Greybeard", "greybeard", "Greybeard-16px.ttf", 1.0, 16, true, "UNIFONT"),
    bundled!("COMMODORE_PET_SCALED", "Commodore PET", "pet-me", "PetMe.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("GOHU_11_SCALED", "Gohu 11", "gohu", "GohuFont11NerdFontMono-Regular.ttf", 1.0, 11, true, ""),
    bundled!("COZETTE_SCALED", "Cozette", "cozette", "CozetteVector.ttf", 1.0, 13, true, ""),
    bundled!("UNSCII_8_SCALED", "Unscii 8", "unscii", "unscii-8.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("UNSCII_8_THIN_SCALED", "Unscii 8 Thin", "unscii", "unscii-8-thin.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("UNIFONT", "Unifont", "unifont", "unifont.otf", 1.0, 16, true, "UNIFONT_UPPER"),
    bundled!("APPLE_II_SCALED", "Apple ][", "apple2", "PrintChar21.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("ATARI_400_SCALED", "Atari 400-800", "atari-400-800", "AtariClassic-Regular.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("COMMODORE_64_SCALED", "Commodore 64", "pet-me", "PetMe64.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("IBM_EGA_8x8", "IBM EGA 8x8", "oldschool-pc-fonts", "PxPlus_IBM_EGA_8x8.ttf", 0.5, 8, true, "UNSCII_8_SCALED"),
    bundled!("IBM_VGA_8x16", "IBM VGA 8x16", "oldschool-pc-fonts", "PxPlus_IBM_VGA_8x16.ttf", 1.0, 16, true, "UNIFONT"),
    bundled!("TERMINESS", "Terminess", "terminus", "TerminessNerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("HACK", "Hack", "hack", "HackNerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("FIRA_CODE", "Fira Code", "fira-code", "FiraCodeNerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("IOSEVKA", "Iosevka", "iosevka", "IosevkaTermNerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("JETBRAINS_MONO", "JetBrains Mono", "jetbrains-mono", "JetBrainsMonoNerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("IBM_3278", "IBM 3278", "ibm-3278", "3270NerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("SOURCE_CODE_PRO", "Source Code Pro", "source-code-pro", "SauceCodeProNerdFontMono-Regular.ttf", 1.0, 32, false, ""),
    bundled!("DEPARTURE_MONO_SCALED", "Departure Mono", "departure-mono", "DepartureMonoNerdFontMono-Regular.otf", 1.0, 11, true, ""),
    bundled!("UNIFONT_UPPER", "", "unifont", "unifont_upper.otf", 1.0, 16, true, ""),
    bundled!("OPENDYSLEXIC", "OpenDyslexic", "opendyslexic", "OpenDyslexicMNerdFontMono-Regular.otf", 1.0, 32, false, ""),
];

fn resolve_all() -> Vec<FontEntry> {
    let mut all = bundled_fonts().to_vec();
    // Bundled, then system, in one list. The system half is appended, never
    // interleaved, so the bundled menu order this catalogue's test pins is
    // untouched by what is installed on the machine.
    let bundled_families: Vec<String> = all
        .iter()
        .map(|f| f.family.clone())
        .filter(|f| !f.is_empty())
        .collect();
    all.extend(
        system::monospace_families(&bundled_families)
            .into_iter()
            .map(|face| {
                // Field for field: the family name is the name, the label
                // and the family; the width is the literal 1.0 (there is no
                // metric-derived width on this path); the size is
                // `SYSTEM_FONT_PIXEL_SIZE`; and a system face is never
                // low-resolution, so it takes the antialiasing-on
                // rasterisation like the bundled scalable faces do.
                //
                // The name is leaked because every other entry's is `'static` and
                // `resolve_font_name` hands one back: the catalogue is a process
                // singleton built once, so its strings live as long as it does
                // whether or not the allocator is told.
                let name: &'static str = Box::leak(face.family.clone().into_boxed_str());
                FontEntry {
                    name,
                    text: name,
                    source: "",
                    base_width: 1.0,
                    pixel_size: SYSTEM_FONT_PIXEL_SIZE,
                    low_resolution: false,
                    is_system: true,
                    family: face.family,
                    fallback_name: "",
                    data: FontData::System(face.path, face.index),
                }
            }),
    );
    all
}

/// A system entry whose file is not there, for the one case the atlas has to
/// survive rather than assert away: a face the catalogue offered and the
/// machine has since lost. `FontEntry::data` answers an empty slice for it,
/// exactly as it does for a font uninstalled between enumeration and
/// selection.
#[cfg(test)]
pub(crate) fn missing_system_face(name: &'static str, path: PathBuf) -> FontEntry {
    FontEntry {
        name,
        text: name,
        source: "",
        base_width: 1.0,
        pixel_size: SYSTEM_FONT_PIXEL_SIZE,
        low_resolution: false,
        is_system: true,
        family: name.to_string(),
        fallback_name: "",
        data: FontData::System(path, 0),
    }
}

fn resolve_bundled() -> Vec<FontEntry> {
    BUNDLED
        .iter()
        .map(|b| {
            let family = metrics::family_name(b.data).unwrap_or_default();
            // addBundledFont(): a low-resolution face takes the metric-derived
            // width, a scalable one keeps the literal.
            let base_width = if b.low_resolution {
                metrics::compute_base_width(b.data, b.pixel_size, b.fallback_base_width)
            } else {
                b.fallback_base_width
            };
            FontEntry {
                name: b.name,
                text: b.text,
                source: b.source,
                base_width,
                pixel_size: b.pixel_size,
                low_resolution: b.low_resolution,
                is_system: false,
                family,
                fallback_name: b.fallback_name,
                data: FontData::Bundled(b.data),
            }
        })
        .collect()
}

/// The bundled catalogue, in declared order. Resolved once, off bytes this
/// binary already carries: nothing here reads a directory, opens a font file
/// or asks the platform anything, so a process that only ever wants bundled
/// faces never scans.
pub fn bundled_fonts() -> &'static [FontEntry] {
    static BUNDLED_FONTS: OnceLock<Vec<FontEntry>> = OnceLock::new();
    BUNDLED_FONTS.get_or_init(resolve_bundled)
}

/// The bundled catalogue plus the machine's monospace families, in that
/// order. Resolved once, and the resolution walks the platform's font
/// directories: this is the only function in the crate that does.
pub fn system_fonts() -> &'static [FontEntry] {
    static SYSTEM_FONTS: OnceLock<Vec<FontEntry>> = OnceLock::new();
    SYSTEM_FONTS.get_or_init(resolve_all)
}

/// The catalogue a source names. The dispatch is the whole reason the two
/// catalogues are separate functions: a caller states which one it wants and
/// the cost follows from the statement.
pub fn fonts_for(source: FontSource) -> &'static [FontEntry] {
    match source {
        FontSource::Bundled => bundled_fonts(),
        FontSource::System => system_fonts(),
    }
}

/// The entry a name selects within a source's catalogue. A system face is
/// not found under [`FontSource::Bundled`], which is what makes a persisted
/// system font name under a bundled profile a miss the caller handles rather
/// than a silent enumeration.
pub fn font_by_name(name: &str, source: FontSource) -> Option<&'static FontEntry> {
    fonts_for(source).iter().find(|f| f.name == name)
}

/// [`font_by_name`], and if `source`'s catalogue does not offer `name`, the
/// bundled entry named `fallback` instead. The fallback is looked up
/// bundled: the bundled catalogue always resolves it.
pub fn font_by_name_or(
    name: &str,
    source: FontSource,
    fallback: &str,
) -> Option<&'static FontEntry> {
    font_by_name(name, source).or_else(|| font_by_name(fallback, FontSource::Bundled))
}

/// The bundled low-resolution faces, the list the LED and tape displays
/// letter themselves from.
pub fn low_resolution_fonts() -> impl Iterator<Item = &'static FontEntry> {
    bundled_fonts()
        .iter()
        .filter(|f| f.low_resolution && !f.text.is_empty())
}

/// The bundled faces that cover `name`'s gaps, nearest first, following each
/// face's own fallback in turn.
///
/// The catalogue states one hop per row, so a face reaching a second one does
/// it by naming a face that names another: `UNIFONT_UPPER`, which carries the
/// codepoints above U+FFFF, is reached through `UNIFONT` rather than listed
/// again on every face behind it.
/// A row naming itself, which is how a face says it is the end of its own
/// chain, stops the walk, as does a name already collected.
pub fn fallback_faces(name: &str) -> Vec<&'static FontEntry> {
    let mut chain = Vec::new();
    let mut next = font_by_name(name, FontSource::Bundled).map(|f| f.fallback_name);
    while let Some(step) = next.filter(|s| !s.is_empty() && *s != name) {
        if chain.iter().any(|f: &&FontEntry| f.name == step) {
            break;
        }
        let Some(entry) = font_by_name(step, FontSource::Bundled) else {
            break;
        };
        chain.push(entry);
        next = Some(entry.fallback_name);
    }
    chain
}

/// The faces offered for a given source and rasterization mode. Modern
/// rasterization offers the scalable faces, every other mode offers the
/// low-resolution ones; system fonts are offered whatever the mode.
pub fn filtered_fonts(
    source: FontSource,
    rasterization: i32,
) -> impl Iterator<Item = &'static FontEntry> {
    let modern = rasterization == MODERN_RASTERIZATION;
    fonts_for(source).iter().filter(move |f| {
        let matches_source = match source {
            FontSource::Bundled => !f.is_system,
            FontSource::System => f.is_system,
        };
        let offered = f.is_system || !f.text.is_empty();
        matches_source && offered && (f.is_system || modern == !f.low_resolution)
    })
}

/// Fallback for a font name the current filter does not offer: falls back
/// to the first face that it does.
pub fn resolve_font_name(
    name: &str,
    source: FontSource,
    rasterization: i32,
) -> Option<&'static str> {
    let mut filtered = filtered_fonts(source, rasterization).peekable();
    let first = filtered.peek().map(|f| f.name);
    match filtered.find(|f| f.name == name) {
        Some(found) => Some(found.name),
        None => first,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    /// The bundled half, which is the half that is the same on every machine.
    fn bundled() -> &'static [FontEntry] {
        bundled_fonts()
    }

    #[test]
    fn the_catalogue_matches_the_recorded_entries() {
        // The bundled catalogue is pinned exactly, and carries nothing off
        // the machine.
        let bundled = bundled();
        assert_eq!(bundled.len(), 25);
        assert_eq!(bundled[0].name, "TERMINESS_SCALED");
        assert_eq!(bundled.last().unwrap().name, "OPENDYSLEXIC");
        assert!(bundled.iter().all(|f| !f.is_system));
        // System entries are appended, never interleaved: the head of the
        // system catalogue is the bundled catalogue in declared order.
        let system = system_fonts();
        let n = bundled.len();
        assert!(system[..n].iter().all(|f| !f.is_system));
        assert_eq!(system[..n], bundled[..]);
        // The default font name.
        assert!(font_by_name("TERMINESS_SCALED", FontSource::Bundled).is_some());
        // The settings default for the channel-bank lettering.
        assert!(font_by_name("UNSCII_8_SCALED", FontSource::Bundled).is_some());
        assert!(font_by_name("NO_SUCH_FONT", FontSource::Bundled).is_none());
    }

    #[test]
    fn each_rasterization_mode_offers_its_own_half() {
        let modern: Vec<_> = filtered_fonts(FontSource::Bundled, MODERN_RASTERIZATION)
            .map(|f| f.name)
            .collect();
        let legacy: Vec<_> = filtered_fonts(FontSource::Bundled, 0)
            .map(|f| f.name)
            .collect();
        // Every face a user can choose is in exactly one half. UNIFONT_UPPER
        // is in neither: it carries no label, so nothing offers it, and it
        // reaches the glass only as another face's fallback.
        let offered = bundled().iter().filter(|f| !f.text.is_empty()).count();
        assert_eq!(modern.len() + legacy.len(), offered);
        assert!(!modern.contains(&"UNIFONT_UPPER") && !legacy.contains(&"UNIFONT_UPPER"));
        assert!(modern.contains(&"HACK"));
        assert!(!modern.contains(&"UNSCII_8_SCALED"));
        assert!(legacy.contains(&"UNSCII_8_SCALED"));
        // Departure Mono sits among the scalable faces in the list but is a
        // low-resolution face, so the legacy half is where it is offered.
        assert!(legacy.contains(&"DEPARTURE_MONO_SCALED"));
        // A system face is offered whatever the rasterization mode, so the
        // two system lists are the same list.
        let sys_legacy: Vec<_> = filtered_fonts(FontSource::System, 0)
            .map(|f| f.name)
            .collect();
        let sys_modern: Vec<_> = filtered_fonts(FontSource::System, MODERN_RASTERIZATION)
            .map(|f| f.name)
            .collect();
        assert_eq!(sys_legacy, sys_modern);
        // ...and no bundled face is in it, in either mode.
        assert!(!sys_legacy.contains(&"HACK"));
    }

    /// The system half: one entry per family, with the system-face field
    /// values, offered under the system source only, in both rasterization
    /// modes.
    ///
    /// Stated about *whatever* the machine has rather than about a named
    /// family, because a test that needs DejaVu installed is a test that fails
    /// on a machine rather than about one. The named-family evidence is
    /// `tests/suite/system_fonts.rs`, which says so out loud when it skips.
    #[test]
    fn the_system_half_is_populate_system_fonts() {
        // The one test here that asks the machine anything, so it says the
        // scanning catalogue's name out loud.
        let system: Vec<_> = system_fonts().iter().filter(|f| f.is_system).collect();
        for f in &system {
            assert_eq!(f.name, f.text, "{}: name and label are the family", f.name);
            assert_eq!(f.family, f.name, "{}: family is the name", f.name);
            assert_eq!(f.source, "", "{}: a system face has no resource", f.name);
            assert_eq!(f.base_width, 1.0, "{}: the literal baseWidth", f.name);
            assert_eq!(f.pixel_size, SYSTEM_FONT_PIXEL_SIZE, "{}", f.name);
            assert!(!f.low_resolution, "{}: never low-resolution", f.name);
            assert_eq!(f.fallback_name, "", "{}: no fallback entry", f.name);
        }
        let mut names: Vec<_> = system.iter().map(|f| f.name).collect();
        let count = names.len();
        names.sort_unstable();
        names.dedup();
        assert_eq!(names.len(), count, "a family was enumerated twice");
        // A bundled family is never offered twice.
        for f in &system {
            assert!(
                !bundled_fonts()
                    .iter()
                    .any(|b| !b.family.is_empty() && b.family == f.family),
                "{} is offered as both a bundled and a system face",
                f.name
            );
        }
    }

    #[test]
    fn a_filtered_out_name_falls_back_to_the_first_offered() {
        // Switching rasterization while a now-unavailable face is selected
        // moves the selection.
        assert_eq!(
            resolve_font_name("HACK", FontSource::Bundled, 0),
            Some("TERMINESS_SCALED")
        );
        assert_eq!(
            resolve_font_name("HACK", FontSource::Bundled, MODERN_RASTERIZATION),
            Some("HACK")
        );
    }

    #[test]
    fn the_low_resolution_list_is_what_the_displays_letter_from() {
        let names: Vec<_> = low_resolution_fonts().map(|f| f.name).collect();
        assert_eq!(names.len(), 16);
        assert!(names.contains(&"UNSCII_8_SCALED"));
        assert!(!names.contains(&"OPENDYSLEXIC"));
    }
}
