//! What the terminal draws for a character the selected face has never heard
//! of.
//!
//! The atlas opens on printable ASCII, which is 95 of the six figures of
//! characters Unicode assigns. Everything else arrives the way everything in
//! a terminal arrives, out of the far end of a pipe, and the question this
//! file answers is whether the seam can say which face draws it. Seven
//! characters, each standing for a way a bundled monospace face runs out:
//!
//!   * ASCII, the control, which the face covers by construction;
//!   * box drawing, which a terminal face is expected to cover and which
//!     `tmux`, `ncdu` and every TUI frame in existence depend on;
//!   * an accented letter, the smallest possible departure from ASCII;
//!   * CJK, which is where a face's coverage and the machine's diverge most;
//!   * emoji, which no monospace face carries and which arrive in colour;
//!   * a private-use codepoint, where a Nerd Font keeps the powerline
//!     separators a shell prompt is built out of;
//!   * `e` followed by U+0301, which is two characters and has to shape to one
//!     glyph, because the alternative is a cell with an accent in it and the
//!     letter it belongs to in the cell before.
//!
//! Every assertion here is that the character resolves to *some* face, and
//! never to a named one. Which face draws CJK is a fact about the machine the
//! test runs on: this one has whatever the distribution installed, a Windows
//! runner has Segoe and MS Gothic, and a test that pinned either would be
//! asserting the fixture rather than the code.
//!
//! Shaping is arithmetic over font tables and needs no adapter, no window and
//! no display, so the coverage tests run wherever the crate compiles. Only the
//! two that put bytes in a texture ask for a GPU.

use gpu::harness::GpuLock;
use term::atlas::Rasterization;
use term::fonts::sizing::{self, ScalePolicy, SizingRequest};
use term::fonts::{font_by_name, FontEntry, FontSource};
use gpu::Gpu;
use term::{ascii_charset, FontContext};

/// The face a session opens on: a Nerd Font at its designed 12 pixels, which
/// is the low-resolution half of the catalogue and the harder half to
/// rasterise a stranger into.
fn terminess() -> &'static FontEntry {
    font_by_name("TERMINESS_SCALED", FontSource::Bundled)
        .expect("TERMINESS_SCALED in the catalogue")
}

fn pixel_size(entry: &FontEntry) -> f32 {
    sizing::resolve(entry, &SizingRequest::default(), ScalePolicy::Floor).raster_pixel_size as f32
}

/// The seven, with the name each one goes by in a failure message.
const CASES: &[(&str, &str)] = &[
    ("A", "printable ASCII"),
    ("\u{2500}", "box drawing, U+2500"),
    ("\u{e9}", "an accented letter, U+00E9"),
    ("\u{4e2d}", "CJK, U+4E2D"),
    ("\u{1f600}", "emoji, U+1F600"),
    ("\u{e0b0}", "a private-use codepoint, U+E0B0"),
    ("e\u{301}", "a combining mark, e followed by U+0301"),
];

/// The done-test: all seven shape to one glyph, drawn by a face that exists.
///
/// A glyph id of 0 is the face reporting it has nothing for this character,
/// which is the empty cell this whole path is here to remove, so it is the
/// failure. The single glyph is the other half: two glyphs for `e` and U+0301
/// would mean the mark was never composed onto the letter.
#[test]
fn every_kind_of_character_resolves_to_a_face_that_can_draw_it() {
    let entry = terminess();
    let px = pixel_size(entry);
    let mut font = FontContext::new(entry);

    for (text, name) in CASES {
        let glyphs = font.covering_glyphs(text, px);
        assert_eq!(
            glyphs.len(),
            1,
            "{name}: {text:?} shaped to {} glyphs, not one",
            glyphs.len()
        );
        let (face, glyph_id, _) = glyphs[0];
        let family = font
            .font_system
            .db()
            .face(face)
            .and_then(|f| f.families.first().map(|(name, _)| name.clone()))
            .unwrap_or_else(|| panic!("{name}: {text:?} named a face the database does not hold"));
        assert_ne!(
            glyph_id, 0,
            "{name}: {text:?} came back as the missing glyph of {family}, so \
             nothing installed on this machine draws it"
        );
        eprintln!("{name}: {text:?} -> glyph {glyph_id} of {family}");
    }
}

/// A session that stays inside ASCII pays nothing for the machine's fonts.
///
/// The enumeration walks the font directories, opens every file and parses
/// every face, and it is in front of the first frame if it is not deferred.
/// So it is deferred, and this is the guard: build the atlas the way a window
/// does, ask the atlas for every character it holds, and the database is still
/// the one face it was built with.
#[test]
fn an_ascii_session_never_loads_the_system_font_database() {
    let entry = terminess();
    let px = pixel_size(entry);
    let mut font = FontContext::new(entry);

    for c in ascii_charset().chars() {
        let mut buf = [0u8; 4];
        font.covering_glyphs(c.encode_utf8(&mut buf), px);
    }
    font.cell_metrics(&sizing::resolve(
        entry,
        &SizingRequest::default(),
        ScalePolicy::Floor,
    ));

    assert!(
        !font.system_fonts_loaded(),
        "printable ASCII pulled in the system font database, which is {} \
         faces of file reads and parsing in front of the first frame",
        font.font_system.db().len()
    );
    assert_eq!(
        font.font_system.db().len(),
        1,
        "the database holds more than the selected face"
    );
}

/// Every property below is stated about an atlas texture, so a machine with no
/// adapter cannot answer them. It says so and stops rather than passing
/// vacuously; `ROBCO_SKIP_GPU_TESTS=1` is the deliberate opt-out. The tuple's
/// order is its drop order: the device goes first, the lock after it.
fn gpu() -> Option<(Gpu, GpuLock)> {
    let lock = match GpuLock::acquire() {
        Ok(lock) => lock,
        Err(e) => panic!("cannot take the GPU lock: {e}"),
    };
    match Gpu::new() {
        Ok(gpu) => Some((gpu, lock)),
        Err(e) => {
            if std::env::var("ROBCO_SKIP_GPU_TESTS").is_ok() {
                eprintln!("skipping: no wgpu adapter ({e}), ROBCO_SKIP_GPU_TESTS set");
                None
            } else {
                panic!("no wgpu adapter: {e}");
            }
        }
    }
}

/// An appended glyph is the glyph the initial build would have packed.
///
/// `W` is withheld from the charset, so the atlas comes back without it, and
/// then it is asked for. The comparison is against the same face's `W` in an
/// atlas built over the whole charset: same size, same bearings, same bytes on
/// the way to the texture. Only the position differs, because a glyph appended
/// after 94 others lands where the pen had got to rather than where `W` sits
/// in codepoint order.
#[test]
fn a_withheld_glyph_appended_afterwards_is_the_glyph_the_build_would_have_packed() {
    let Some((gpu, _lock)) = gpu() else { return };
    let entry = terminess();
    let resolved = sizing::resolve(entry, &SizingRequest::default(), ScalePolicy::Floor);
    let mode = Rasterization::for_face(&resolved);

    let mut font = FontContext::new(entry);
    let whole = font.build_atlas(&gpu.device, &gpu.queue, &resolved, &ascii_charset(), mode);
    let expected = *whole.slot('W').expect("W in an atlas over printable ASCII");

    let withheld: String = ascii_charset().chars().filter(|c| *c != 'W').collect();
    let mut atlas = font.build_atlas(&gpu.device, &gpu.queue, &resolved, &withheld, mode);
    assert!(
        atlas.slot('W').is_none(),
        "the withheld charset produced a W anyway, so this proves nothing"
    );
    let bytes_before = atlas.total_value_count();

    let appended = atlas.glyph(&gpu.device, &gpu.queue, &mut font, term::atlas::Role::Mono, 'W').expect(
        "W appended to an atlas that was built without it; a miss that stays a \
         miss is the empty cell this path exists to remove",
    );
    assert_eq!(
        (appended.width, appended.height, appended.left, appended.top),
        (expected.width, expected.height, expected.left, expected.top),
        "the appended W is a different picture from the packed one"
    );
    assert_eq!(
        atlas.total_value_count() - bytes_before,
        (expected.width * expected.height) as u64,
        "the append counted a different number of bytes than the glyph has"
    );
    assert!(
        atlas.slot('W').is_some(),
        "the appended W is not in the atlas afterwards, so the next frame \
         rasterises it again"
    );
}

/// The atlas grows rather than overflowing, and says when the texture moved.
///
/// One glyph in the initial charset makes the texture about as short as a
/// texture gets, and then 94 more are appended into it. Every one has to come
/// back with a slot inside the allocation, and the generation counter has to
/// have moved, because a caller holding a bind group over the old texture has
/// no other way to know its binding points at an allocation that is gone.
#[test]
fn appending_past_the_allocated_height_grows_the_texture_and_says_so() {
    let Some((gpu, _lock)) = gpu() else { return };
    let entry = terminess();
    let resolved = sizing::resolve(entry, &SizingRequest::default(), ScalePolicy::Floor);
    let mode = Rasterization::for_face(&resolved);

    let mut font = FontContext::new(entry);
    let mut atlas = font.build_atlas(&gpu.device, &gpu.queue, &resolved, "M", mode);
    let opening_height = atlas.height;

    for c in ascii_charset().chars() {
        let Some(slot) = atlas.glyph(&gpu.device, &gpu.queue, &mut font, term::atlas::Role::Mono, c) else {
            // A space has no ink and earns no slot, which is as true of an
            // appended character as of a packed one.
            continue;
        };
        assert!(
            slot.atlas_x + slot.width <= atlas.width && slot.atlas_y + slot.height <= atlas.height,
            "{c:?} was placed at {},{} in a {}x{} atlas, which is off the end \
             of the texture",
            slot.atlas_x,
            slot.atlas_y,
            atlas.width,
            atlas.height
        );
    }

    assert!(
        atlas.height > opening_height,
        "94 glyphs went into a {opening_height}-texel-tall atlas without it \
         growing, so either the pen or the height is not being carried"
    );
    assert!(
        atlas.generation > 0,
        "the atlas grew from {opening_height} to {} texels without counting a \
         texture swap, so a bind group over it would still point at the {}-tall \
         allocation",
        atlas.height,
        opening_height
    );
}

/// A face whose catalogue row names a fallback reaches it without reading the
/// machine, and reaches through it to the face that one names in turn.
///
/// Fixedsys Excelsior carries neither 中 nor 🔥. Unifont covers the first and
/// its plane-1 companion the second, so both are answers this binary carries,
/// identical on a bare container and a loaded desktop. `system_fonts_loaded`
/// staying false is the whole assertion: it is the evidence that the machine
/// was never asked.
#[test]
fn a_bundled_fallback_answers_before_the_machine_is_asked() {
    let entry = font_by_name("EXCELSIOR_SCALED", FontSource::Bundled)
        .expect("EXCELSIOR_SCALED in the catalogue");
    let px = pixel_size(entry);
    let mut font = FontContext::new(entry);

    for (text, name) in [("\u{4e2d}", "CJK, U+4E2D"), ("\u{1f525}", "emoji, U+1F525")] {
        let glyphs = font.covering_glyphs(text, px);
        assert_eq!(glyphs.len(), 1, "{name} shaped to {} glyphs", glyphs.len());
        assert_ne!(glyphs[0].1, 0, "{name} is covered by no bundled face");
        assert!(
            !font.system_fonts_loaded(),
            "{name} sent the atlas to the machine's fonts"
        );
    }
}
