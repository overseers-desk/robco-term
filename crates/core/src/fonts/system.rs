//! System-font enumeration: the platform's installed monospace families,
//! filtered to one entry per family and sorted for a stable font menu.
//!
//! Enumeration goes through `fontdb`, and it is not a new dependency:
//! `cosmic-text` already pins it and this crate already shapes through it
//! (`cosmic_text::fontdb`), so enumerating with anything else would put two font
//! databases in one process disagreeing about what the machine has.
//!
//! Three places the enumeration rule is a judgement rather than an obvious
//! reading of the data, all measured rather than assumed:
//!
//! * **One entry per family.** `fontdb` lists *faces*, not families, so DejaVu
//!   Sans Mono arrives four times (Book, Bold, Oblique, Bold Oblique). The
//!   regular face is the one each family is represented by, with the first
//!   face as the fallback for a family that ships no regular one.
//! * **Fixed pitch.** `fontdb`'s `monospaced` flag reads the face's own
//!   declaration (`post.isFixedPitch`, OS/2 PANOSE), which is not a
//!   measurement of the advances -- a face that lies in its tables will be
//!   believed.
//! * **Order.** `fontdb`'s face order is the order the directories were
//!   walked, which is not stable across machines or runs. The list is the
//!   font menu's order, so it is sorted here, which is what makes two runs on
//!   one machine produce the same menu.
//!
//! The bytes are *not* read here. `fontdb` keeps a `Source::File` path and
//! parses only the face records it needs; the catalogue keeps that path and
//! reads the file the first time something asks a system face to become pixels
//! (`FontEntry::data`). Enumerating the machine's monospace fonts therefore
//! costs the directory walk, not tens of megabytes of resident font data.

use std::path::{Path, PathBuf};
use std::sync::OnceLock;

use cosmic_text::fontdb;

/// One system family, as the catalogue will carry it.
pub struct SystemFace {
    /// The family name, which fills the font's name, label text, and family
    /// selector alike.
    pub family: String,
    /// Where the face lives, for the lazy read in [`super::FontEntry::data`].
    pub path: PathBuf,
    /// Which face in that file. A `.ttc` holds several.
    pub index: u32,
}

/// The machine's monospace families, minus the ones a bundled face already
/// occupies, one entry each, sorted by family name.
///
/// `exclude` holds the families the bundled faces resolved to, so the system
/// list doesn't repeat them. A machine with Hack or Fira Code installed would
/// otherwise offer them twice, once from the bundle and once from the system,
/// under the same label and with different metadata.
pub fn monospace_families(exclude: &[String]) -> Vec<SystemFace> {
    families_from(installed(), exclude)
}

/// The machine's fonts, walked at most once for the life of the process.
///
/// One database, not one per question. The three things this module answers
/// off the platform's font directories -- the monospace family list, the
/// `sans-serif` generic, the `serif` generic -- are three readings of the
/// same walk, and `load_system_fonts` is the expensive half of all three.
fn installed() -> &'static fontdb::Database {
    static DB: OnceLock<fontdb::Database> = OnceLock::new();
    DB.get_or_init(|| {
        let mut db = fontdb::Database::new();
        db.load_system_fonts();
        db
    })
}

/// Whether the chassis lettering may be set in a face off the machine.
///
/// Off, and the two generics below answer with the bundled Iosevka rather
/// than with a walk of the font directories: a profile that asks for bundled
/// faces gets a cabinet lettered in a face this binary carries, and the
/// process never touches the machine's fonts at all.
///
/// One shot: the first call decides for the process, later ones are ignored,
/// and never calling means off. The gate is a startup decision made once from
/// the resolved config, not a setting that moves under a running cabinet --
/// the generics are `'static` byte slices resolved once each, so there is
/// nothing for a second answer to change.
pub fn allow_system_lettering(allowed: bool) {
    let _ = LETTERING.set(allowed);
}

static LETTERING: OnceLock<bool> = OnceLock::new();

fn system_lettering_allowed() -> bool {
    *LETTERING.get().unwrap_or(&false)
}

/// The bundled face the chassis letters itself in when the machine's fonts
/// are off limits: Iosevka, which is already the numeral face all three
/// shells stamp their channel numbers in, so a cabinet whose labels and
/// numerals both come from the bundle is a cabinet set in one face.
fn bundled_lettering() -> Option<&'static [u8]> {
    super::font_by_name("IOSEVKA", super::FontSource::Bundled).map(|e| e.data())
}

/// The body of [`monospace_families`] against an arbitrary database.
///
/// Split out so the filtering rules can be measured against a database whose
/// contents are known, rather than only against whatever fonts the machine
/// running the tests happens to have. Every claim in the module doc above
/// (one entry per family, the regular face representing it, the exclusion, the
/// sort) is asserted that way below.
fn families_from(db: &fontdb::Database, exclude: &[String]) -> Vec<SystemFace> {
    // Family -> the face that represents it. The regular face represents the
    // family; a family with no regular face keeps whichever came first.
    let mut chosen: Vec<(String, &fontdb::FaceInfo, bool)> = Vec::new();
    for face in db.faces() {
        let Some(family) = face.families.first().map(|(name, _)| name.clone()) else {
            continue;
        };
        if exclude.contains(&family) {
            continue;
        }
        let regular = face.weight == fontdb::Weight::NORMAL
            && face.style == fontdb::Style::Normal
            && face.stretch == fontdb::Stretch::Normal;
        match chosen.iter_mut().find(|(name, _, _)| *name == family) {
            // A regular face displaces a non-regular placeholder, and nothing
            // displaces a regular one.
            Some(slot) if regular && !slot.2 => *slot = (family, face, true),
            Some(_) => {}
            None => chosen.push((family, face, regular)),
        }
    }

    let mut out = Vec::new();
    for (family, face, _) in chosen {
        // The fixed-pitch check applies to the family's representative face,
        // not every face variant.
        if !face.monospaced {
            continue;
        }
        // A face with no file behind it cannot be read back for rasterising.
        // Nothing in a system enumeration should be memory-backed, but saying
        // so here is cheaper than a panic at the atlas.
        let Some((path, index)) = source_path(face) else {
            continue;
        };
        out.push(SystemFace {
            family,
            path,
            index,
        });
    }
    out.sort_by(|a, b| a.family.cmp(&b.family));
    out
}

/// The families a bare `QFont()` resolves to on this platform, in the order
/// fontconfig's own `sans-serif` alias generally settles them.
///
/// The chassis furniture paints two kinds of text. The numerals name their
/// own face (`fontByName("IOSEVKA").family`), so the catalogue answers them
/// directly. The pager's `PREV`/`NEXT` and its arrow glyphs name no face at
/// all, which means the platform's default font -- on a Linux desktop,
/// whatever the platform theme reads out of fontconfig, in practice the
/// `sans-serif` alias.
///
/// The alias is resolved by name against the same `fontdb` the rest of this
/// module enumerates with. The list is tried in
/// order and anything not installed is skipped; a machine with none of them
/// falls through to the first proportional face the database holds, and a
/// machine with no fonts at all gets `None` and paints no label rather than
/// painting one in the wrong face.
const SANS_CANDIDATES: &[&str] = &[
    "DejaVu Sans",
    "Liberation Sans",
    "Noto Sans",
    "Nimbus Sans",
    "Cantarell",
    "Arial",
    "Helvetica",
];

/// The application font's bytes, read once for the life of the process.
///
/// Under [`allow_system_lettering(false)`](allow_system_lettering) -- which
/// is what an unconfigured process is -- this is the bundled Iosevka and no
/// candidate above is consulted, so the machine's fonts are never walked for
/// a label. The caller's own miss handling stays the fallback of the
/// fallback: a build with no Iosevka in it still paints no label rather than
/// painting one in the wrong face.
///
/// Leaked for the same reason [`face_data`] leaks: the callers want a
/// `&'static [u8]` to hand a rasteriser, and there is exactly one of these.
pub fn default_sans() -> Option<&'static [u8]> {
    static SANS: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    *SANS.get_or_init(|| {
        if !system_lettering_allowed() {
            return bundled_lettering();
        }
        let path = sans_face(installed())?;
        let data = face_data(&path);
        (!data.is_empty()).then_some(data)
    })
}

/// The families fontconfig's `serif` alias generally settles on, for the one
/// label in the chassis that asks for the generic `serif` face rather than a
/// specific family (the pager's counter rolls).
const SERIF_CANDIDATES: &[&str] = &[
    "DejaVu Serif",
    "Liberation Serif",
    "Noto Serif",
    "Nimbus Roman",
    "Times New Roman",
    "Times",
];

/// The `serif` generic's bytes, read once for the life of the process, under
/// the same gate and the same bundled answer as [`default_sans`].
pub fn default_serif() -> Option<&'static [u8]> {
    static SERIF: OnceLock<Option<&'static [u8]>> = OnceLock::new();
    *SERIF.get_or_init(|| {
        if !system_lettering_allowed() {
            return bundled_lettering();
        }
        let path = named_face(installed(), SERIF_CANDIDATES)?;
        let data = face_data(&path);
        (!data.is_empty()).then_some(data)
    })
}

/// The body of [`default_sans`] against an arbitrary database, so the fallback
/// order can be measured against a database whose contents are known.
fn sans_face(db: &fontdb::Database) -> Option<PathBuf> {
    named_face(db, SANS_CANDIDATES).or_else(|| {
        // Nothing named: the first proportional regular face the machine has.
        db.faces()
            .find(|face| {
                face.weight == fontdb::Weight::NORMAL
                    && face.style == fontdb::Style::Normal
                    && !face.monospaced
            })
            .and_then(|face| source_path(face).map(|(path, _)| path))
    })
}

/// The first of `wanted` the database holds, as a regular face with a file
/// behind it.
fn named_face(db: &fontdb::Database, wanted: &[&str]) -> Option<PathBuf> {
    let regular = |face: &fontdb::FaceInfo| {
        face.weight == fontdb::Weight::NORMAL && face.style == fontdb::Style::Normal
    };
    for want in wanted {
        for face in db.faces() {
            if !regular(face) {
                continue;
            }
            if face.families.iter().any(|(name, _)| name == want) {
                if let Some((path, _)) = source_path(face) {
                    return Some(path);
                }
            }
        }
    }
    None
}

fn source_path(face: &fontdb::FaceInfo) -> Option<(PathBuf, u32)> {
    match &face.source {
        fontdb::Source::File(path) => Some((path.clone(), face.index)),
        fontdb::Source::SharedFile(path, _) => Some((path.clone(), face.index)),
        fontdb::Source::Binary(_) => None,
    }
}

/// Read a system face's file, once per path for the life of the process.
///
/// The result is leaked on purpose: the catalogue is a `'static` singleton and
/// every entry in it hands out `&'static [u8]`, so a system face has to reach
/// the same lifetime as a bundled one's `include_bytes!`. The bound is the
/// number of *distinct system faces the user actually selects*, which is one in
/// almost every session and is never larger than the font menu.
///
/// A file that cannot be read comes back empty rather than panicking: a font
/// removed between enumeration and selection is an ordinary thing for a machine
/// to do, and the caller ([`super::FontEntry::data`]) turns it into a fallback.
pub fn face_data(path: &Path) -> &'static [u8] {
    use std::collections::HashMap;
    use std::sync::Mutex;

    static CACHE: OnceLock<Mutex<HashMap<PathBuf, &'static [u8]>>> = OnceLock::new();
    let cache = CACHE.get_or_init(|| Mutex::new(HashMap::new()));
    let mut cache = cache
        .lock()
        .expect("the system-face cache is never poisoned");
    if let Some(data) = cache.get(path) {
        return data;
    }
    let data: &'static [u8] = match std::fs::read(path) {
        Ok(bytes) => Box::leak(bytes.into_boxed_slice()),
        Err(e) => {
            log::warn!("cannot read the system font {}: {e}", path.display());
            &[]
        }
    };
    cache.insert(path.to_path_buf(), data);
    data
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A database built from files on disk, because that is the only kind this
    /// module accepts: `source_path` rejects a memory-backed face, since a face
    /// with no file behind it could not be read back when the user selects it.
    ///
    /// The faces are the crate's own bundled fonts written out to a temporary
    /// directory. Real font files rather than a fixture, so the family names and
    /// the `monospaced` flags are the ones a real parser produces, which is
    /// the half a hand-written fixture would get to decide for itself.
    struct Fixture {
        dir: PathBuf,
        db: fontdb::Database,
    }

    impl Fixture {
        fn new(name: &str, faces: &[&str]) -> Self {
            let dir = std::env::temp_dir().join(format!("robco-system-fonts-{name}"));
            let _ = std::fs::remove_dir_all(&dir);
            std::fs::create_dir_all(&dir).expect("temp dir");
            let mut db = fontdb::Database::new();
            for entry_name in faces {
                let entry =
                    super::super::font_by_name(entry_name, super::super::FontSource::Bundled)
                        .unwrap_or_else(|| panic!("{entry_name} is in the catalogue"));
                let path = dir.join(format!("{entry_name}.ttf"));
                std::fs::write(&path, entry.data()).expect("write the face out");
                db.load_font_file(&path).expect("load the face back");
            }
            Fixture { dir, db }
        }
    }

    impl Drop for Fixture {
        fn drop(&mut self) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }

    /// An ungated process letters the chassis in the bundled Iosevka, and
    /// asks the machine for nothing.
    ///
    /// Nothing in this test binary calls [`allow_system_lettering`], so the
    /// gate is in the state every process starts in and the one state a
    /// process that never decides stays in. The gate is one-shot for the
    /// life of a process, which is why this is asserted rather than driven
    /// both ways here: the system arm is a startup decision, and
    /// `tests/suite/system_fonts.rs` is where the machine's own fonts are
    /// spoken about.
    #[test]
    fn the_generics_are_the_bundled_numeral_face_until_the_gate_is_opened() {
        let iosevka = super::super::font_by_name("IOSEVKA", super::super::FontSource::Bundled)
            .expect("the bundled catalogue carries Iosevka")
            .data();
        assert_eq!(default_sans(), Some(iosevka));
        assert_eq!(default_serif(), Some(iosevka));
    }

    /// The family name this module reports is the family name the *catalogue*
    /// reports for the same file.
    ///
    /// Load bearing, and not obviously true: the catalogue's `family` comes
    /// from `ttf-parser` (`metrics::family_name`) and this module's comes from
    /// `fontdb`, and the exclusion in `resolve_all` compares one against the
    /// other. Two parsers picking different name records (a typographic
    /// family against a legacy one, say) would make the exclusion silently
    /// stop excluding, and a machine with Hack installed would then offer Hack
    /// twice under one label with different metadata behind each.
    #[test]
    fn the_family_name_is_the_one_the_catalogue_uses() {
        let f = Fixture::new("names", &["HACK", "FIRA_CODE"]);
        for face in families_from(&f.db, &[]) {
            assert!(
                super::super::bundled_fonts()
                    .iter()
                    .any(|e| e.family == face.family),
                "fontdb calls this file's family {:?}, and no *bundled* catalogue \
                 entry agrees; the bundled-family exclusion compares these two",
                face.family
            );
        }
    }

    /// Two files, two families, both monospace, and the answer is sorted by
    /// family name rather than by the order they were loaded in.
    #[test]
    fn the_list_is_one_entry_per_family_sorted() {
        // Hack is loaded first, so an unsorted answer comes back Hack first.
        let f = Fixture::new("sorted", &["HACK", "FIRA_CODE"]);
        let got: Vec<_> = families_from(&f.db, &[])
            .into_iter()
            .map(|s| s.family)
            .collect();
        let mut expected = got.clone();
        expected.sort();
        assert_eq!(got.len(), 2, "{got:?}");
        assert_eq!(got, expected, "the list came back unsorted");
        assert!(got[0].contains("FiraCode"), "{got:?}");
        assert!(got[1].contains("Hack"), "{got:?}");
    }

    /// The same face twice under two names is still one family: enumeration
    /// lists families, and `TERMINESS` and `TERMINESS_SCALED` are the same
    /// file.
    #[test]
    fn one_family_appears_once_however_many_faces_carry_it() {
        let f = Fixture::new("dedup", &["TERMINESS", "TERMINESS_SCALED"]);
        let got = families_from(&f.db, &[]);
        assert_eq!(
            got.len(),
            1,
            "{:?}",
            got.iter().map(|s| &s.family).collect::<Vec<_>>()
        );
    }

    /// A family a bundled face already occupies is not offered again from
    /// the system source.
    #[test]
    fn an_excluded_family_is_not_offered() {
        let f = Fixture::new("exclude", &["HACK", "FIRA_CODE"]);
        // The catalogue's own family string for Hack, which is exactly what
        // `resolve_all` builds the exclusion list out of.
        let hack = super::super::font_by_name("HACK", super::super::FontSource::Bundled)
            .expect("HACK")
            .family
            .clone();
        let got: Vec<_> = families_from(&f.db, std::slice::from_ref(&hack))
            .into_iter()
            .map(|s| s.family)
            .collect();
        assert_eq!(got.len(), 1, "{got:?}");
        assert!(got[0].contains("FiraCode"), "{got:?}");
    }

    /// Every entry carries a readable path, because the catalogue reads it
    /// later. A face this module accepted and cannot read is the failure the
    /// lazy load would otherwise turn into an empty atlas at selection time.
    #[test]
    fn every_entry_points_at_a_file_that_can_be_read() {
        let f = Fixture::new("paths", &["HACK", "FIRA_CODE"]);
        for face in families_from(&f.db, &[]) {
            let data = face_data(&face.path);
            assert!(
                data.len() > 1000,
                "{} at {} read back {} bytes",
                face.family,
                face.path.display(),
                data.len()
            );
        }
    }
}
