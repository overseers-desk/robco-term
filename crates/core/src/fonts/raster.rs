//! One answer to "how a bundled face becomes pixels".
//!
//! Every bundled low-resolution face in the catalogue is a Terminus-style
//! conversion: an outline table for scalability plus **embedded monochrome
//! bitmap strikes** at the ppem values the face was actually drawn at.
//! `TerminessNerdFontMono-Regular.ttf` carries `EBLC`/`EBDT` strikes at 12, 14,
//! 16, 18, 20, 22, 24, 28 and 32 ppem, and the catalogue's `pixelSize` for a
//! low-resolution entry is one of those numbers (`TERMINESS_SCALED` is 12,
//! `TERMINESS` is 32) -- any other value would ask the strike table for a
//! size it doesn't have.
//!
//! Rendering with antialiasing off loads the strike at that exact ppem: the
//! designed pixel font, one bit per pixel, nothing to threshold. A face's
//! *outline* at the same ppem is a different picture entirely -- Terminess's
//! 1000-unit em rasterised at 12 ppem puts a nominal one-pixel stem across
//! two pixels at about 96/255 coverage each, and a 50% threshold then
//! deletes both halves. That is a hyphen that disappears, an `i` that reads
//! as `:`, and a `w` that reads as `u`.
//!
//! So the strike is asked for first and the outline is the fallback, and that
//! rule lives here rather than at each call site, because there are two of them
//! and they must not drift: the terminal's glyph atlas ([`crate::atlas`]) and
//! the chassis LED's text raster ([`super::led`]). `cosmic_text::SwashCache`
//! cannot be used for either -- its source list is
//! `[ColorOutline, ColorBitmap(BestFit), Outline]`
//! (cosmic-text-0.19.0/src/swash.rs:58-65) with no monochrome `Source::Bitmap`
//! in it at all, so it can only ever return the outline.

use swash::scale::image::Image;
use swash::scale::{Render, Scaler, Source, StrikeWith};
use swash::zeno::{Format, Transform};

/// Rasterise one glyph as an 8-bit coverage mask.
///
/// `StrikeWith::ExactSize` and not `BestFit`: a strike scaled to a size it was
/// not drawn at is a resampled pixel font, which is the one thing this
/// renderer exists to avoid. A face with no strike at this ppem -- every
/// scalable entry in the catalogue -- falls through to the outline, which is
/// what it wants anyway.
pub fn glyph_mask(scaler: &mut Scaler, glyph_id: u16) -> Option<Image> {
    Render::new(&[Source::Bitmap(StrikeWith::ExactSize), Source::Outline])
        .format(Format::Alpha)
        .render(scaler, glyph_id)
}

/// The same glyph rasterised three times as wide, for the LCD filter: one
/// coverage per third of a pixel, which is one per stripe.
///
/// The outline, and only the outline. The strike-first rule above is about a
/// pixel font asked for at the ppem it was drawn at, and this is the other
/// path entirely -- an antialiased render on a subpixel screen. FreeType
/// is built the same way round: `FT_RENDER_MODE_LCD` is a rendering mode for
/// an outline, and a glyph that came out of a monochrome strike has no
/// coverage inside a pixel to filter. So a face with a strike here would be
/// asked for its outline, which the plate's faces (Iosevka, the system sans
/// and serif) are scalable-only anyway.
///
/// The three-times scale goes on the outline *after* hinting, which is where
/// `src/smooth/ftsmooth.c` puts it: the glyph is fitted to the pixel grid it
/// will be shown on, and only then sampled at thirds. The returned
/// `placement.left` and `width` are therefore in thirds of a pixel; `top` and
/// `height` are untouched, since nothing is scaled vertically.
pub fn glyph_mask_thirds(scaler: &mut Scaler, glyph_id: u16) -> Option<Image> {
    Render::new(&[Source::Outline])
        .format(Format::Alpha)
        .transform(Some(Transform::scale(3.0, 1.0)))
        .render(scaler, glyph_id)
}
