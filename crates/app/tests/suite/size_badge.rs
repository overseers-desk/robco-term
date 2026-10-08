//! Done-test for the size overlay: while a window is being resized the badge's
//! pixels are on the frame, they fade, and the glass outside them is untouched.
//!
//! Three claims, all measured on real GPU pixels rather than asserted about
//! state:
//!
//! - the badge **darkens** its own rectangle -- it is a black rounded plate
//!   at half opacity, so what lands there is the frame mixed halfway to
//!   black, not the frame and not black;
//! - it carries **text**: inside that plate there are pixels brighter than the
//!   plate, which is the only thing that separates a drawn badge from a drawn
//!   rectangle. `term::render`'s glyphs are white on it;
//! - every pixel **outside** the rectangle is exactly what was on the frame
//!   before, to the bit. The badge composites with `LoadOp::Load` over a frame
//!   the chain and the column already drew, and a mount that cleared its
//!   attachment would take the glass with it.
//!
//! Then the envelope: at the opacity the state machine reports after the hold
//! has lapsed and the fade has run, the frame comes back bit-identical to the
//! one with no badge at all. The two halves of the badge's behavior meet
//! there -- the machine in `app::overlay` decides *when*, this mount
//! decides *what*, and a fade that reached zero has to leave nothing behind.
//!
//! The mount is `app::chrome`, the pass that draws the cabinet. These render
//! through it with no casting and no furniture, a column width only to say
//! where the well begins, so what lands on the frame is the badge stack and
//! nothing else.

use app::chrome::{Badges, Chrome, Entry};
use app::overlay::{GridSize, SizeOverlay, FADE, HOLD};
use gpu::harness::Locked;
use term::{SizingRequest, Target};

/// A window at the default size the app opens with, and the shipped
/// profile's bank, so the well the badge is centred in is not the whole
/// window.
const WINDOW_W: u32 = 1024;
const WINDOW_H: u32 = 768;
const BANK: u32 = 247;

/// What the frame carries before the badge goes on: a mid-grey no glyph and no
/// plate produces, so "darkened", "lit" and "untouched" are three different
/// answers. 0.4 is exactly 102/255, so the readback has a whole number to be
/// equal to rather than a rounding to be near.
const SENTINEL_F: f64 = 0.4;
const SENTINEL: [u8; 4] = [102, 102, 102, 255];

/// The badge draws onto the swapchain, which presents through `Bgra8Unorm`.
/// The harness's own output format is `Rgba32Float`, which
/// wgpu will not blend, so the frame here is `term::Target`'s `Rgba8Unorm`:
/// eight bits per channel, blendable, and the same depth the real one is.
const FRAME_FORMAT: wgpu::TextureFormat = wgpu::TextureFormat::Rgba8Unorm;

struct Fixture {
    gpu: Locked,
    atlas: term::GlyphAtlas,
    scale: u32,
}

fn fixture() -> Fixture {
    let gpu = Locked::new().expect("headless wgpu device");
    let entry = term::bundled_fonts()
        .first()
        .expect("at least one bundled font")
        .clone();
    let request = SizingRequest::default();
    let (resolved, _font, atlas) = term::build_font(&gpu.device, &gpu.queue, &entry, &request);
    let scale = resolved.integer_scale;
    Fixture { gpu, atlas, scale }
}

/// Clear a frame to [`SENTINEL`], put the badge on it, read it back.
fn frame_with_badge(
    fx: &Fixture,
    badge: &mut Chrome,
    text: &str,
    opacity: f32,
) -> (term::Image, Option<app::chrome::BadgeRect>) {
    let output = Target::new(&fx.gpu.device, WINDOW_W, WINDOW_H, gpu::TARGET_FORMAT);
    let view = &output.view;
    let mut encoder = fx
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("size badge test"),
        });

    // Whatever the chain and the column left on this image.
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("the frame the chain drew"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: SENTINEL_F,
                    g: SENTINEL_F,
                    b: SENTINEL_F,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    let rect = badge.render(
        &fx.gpu.device,
        &fx.gpu.queue,
        &mut encoder,
        view,
        (WINDOW_W, WINDOW_H),
        // No casting and no furniture: the column is here only to say where
        // the well starts, which is what the badge is centred in.
        (BANK, WINDOW_H),
        (WINDOW_W - BANK, WINDOW_H),
        1.0,
        None,
        &std::sync::Arc::from(Vec::new()),
        Some(Badges {
            atlas: &fx.atlas,
            scale: fx.scale,
            entries: &[Entry { text, opacity }],
        }),
    )[0];

    let index = fx.gpu.queue.submit([encoder.finish()]);
    fx.gpu
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .expect("device poll");
    (output.read_rgba(&fx.gpu.device, &fx.gpu.queue), rect)
}

#[test]
fn the_badge_lands_on_the_frame_and_leaves_the_glass_untouched() {
    let fx = fixture();
    let mut badge = Chrome::new(&fx.gpu.device, FRAME_FORMAT);

    // The state machine's own answer, not a number typed here: a resize, and
    // the opacity it reports at that moment.
    let mut overlay = SizeOverlay::new(true);
    overlay.resized(GridSize {
        columns: 80,
        rows: 24,
    });
    overlay.resized(GridSize {
        columns: 100,
        rows: 30,
    });
    let text = overlay.text();
    assert_eq!(text, "100x30");
    let opacity = overlay.opacity();
    assert!(opacity > 0.0, "a fresh resize shows the badge");

    let (frame, rect) = frame_with_badge(&fx, &mut badge, &text, opacity);
    let rect = rect.expect("the badge was drawn");

    // It is centred in the well, not in the window: its middle is the well's
    // middle, which is `BANK / 2` to the right of the window's.
    let badge_centre = rect.x + rect.width as i32 / 2;
    let well_centre = BANK as i32 + (WINDOW_W - BANK) as i32 / 2;
    assert!(
        (badge_centre - well_centre).abs() <= 1,
        "the badge is centred at {badge_centre}, the well at {well_centre}"
    );

    // The plate: darker than the frame was, and consistent with the frame mixed
    // toward black by exactly this opacity. Sampled a few pixels in from the
    // left edge, at half height, so no rounded corner is in the way.
    let plate = frame.pixel(
        (rect.x + 8) as u32,
        (rect.y + rect.height as i32 / 2) as u32,
    )[0];
    assert!(
        plate + 12 < SENTINEL[0],
        "the plate at the badge's left edge is {plate}, no darker than the frame's {}",
        SENTINEL[0]
    );
    let expected = f32::from(SENTINEL[0]) * (1.0 - opacity);
    assert!(
        (f32::from(plate) - expected).abs() < 2.0,
        "the plate reads {plate} where {opacity} of black over {} is {expected}",
        SENTINEL[0]
    );

    // The text: somewhere inside the plate there are pixels brighter than the
    // plate itself. Without this the test passes on a rounded rectangle with
    // no glyphs in it, which is precisely the failure the atlas seam invites.
    let mut lit = 0usize;
    for y in rect.y..rect.y + rect.height as i32 {
        for x in rect.x..rect.x + rect.width as i32 {
            if frame.pixel(x as u32, y as u32)[0] > plate + 12 {
                lit += 1;
            }
        }
    }
    assert!(
        lit > 20,
        "only {lit} lit pixels inside the badge: the plate is there but the text is not"
    );

    // ...and everything outside it is the frame, to the bit.
    for y in 0..WINDOW_H {
        for x in 0..WINDOW_W {
            let inside = (x as i32) >= rect.x
                && (x as i32) < rect.x + rect.width as i32
                && (y as i32) >= rect.y
                && (y as i32) < rect.y + rect.height as i32;
            if inside {
                continue;
            }
            assert_eq!(
                frame.pixel(x, y),
                SENTINEL,
                "the frame at ({x},{y}) is outside the badge and was changed"
            );
        }
    }
}

#[test]
fn a_faded_badge_leaves_the_frame_exactly_as_it_found_it() {
    let fx = fixture();
    let mut badge = Chrome::new(&fx.gpu.device, FRAME_FORMAT);

    let mut overlay = SizeOverlay::new(true);
    overlay.resized(GridSize {
        columns: 80,
        rows: 24,
    });
    overlay.resized(GridSize {
        columns: 100,
        rows: 30,
    });
    // The hold, then the whole fade: 1 s + 200 ms.
    overlay.tick(HOLD + FADE);
    assert_eq!(overlay.opacity(), 0.0);
    assert!(!overlay.visible());

    let (frame, rect) = frame_with_badge(&fx, &mut badge, &overlay.text(), overlay.opacity());
    assert!(rect.is_none(), "a faded badge drew a rectangle anyway");
    for y in 0..WINDOW_H {
        for x in 0..WINDOW_W {
            assert_eq!(
                frame.pixel(x, y),
                SENTINEL,
                "a fully faded badge left a mark at ({x},{y})"
            );
        }
    }
}

/// The stack: two badges up at once are two plates, the second below the first,
/// each carrying its own words at its own strength.
///
/// One record per badge is what makes this a real claim rather than a
/// formality: drawn as two calls sharing one uniform buffer inside one
/// encoder, both plates would render with the *second* call's origin and
/// opacity, i.e. one plate where two were asked for. The instanced pass
/// indexes a record per badge instead, and this measures that it does.
#[test]
fn two_badges_stack_and_neither_is_drawn_with_the_others_uniforms() {
    let fx = fixture();
    let mut badge = Chrome::new(&fx.gpu.device, FRAME_FORMAT);

    let output = Target::new(&fx.gpu.device, WINDOW_W, WINDOW_H, gpu::TARGET_FORMAT);
    let mut encoder = fx
        .gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("badge stack test"),
        });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("the frame the chain drew"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &output.view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color {
                    r: SENTINEL_F,
                    g: SENTINEL_F,
                    b: SENTINEL_F,
                    a: 1.0,
                }),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });

    // The size badge at its own half opacity, and a notice under it at the
    // stronger one `app::overlay::NOTICE_OPACITY` picks.
    let rects = badge.render(
        &fx.gpu.device,
        &fx.gpu.queue,
        &mut encoder,
        &output.view,
        (WINDOW_W, WINDOW_H),
        (BANK, WINDOW_H),
        (WINDOW_W - BANK, WINDOW_H),
        1.0,
        None,
        &std::sync::Arc::from(Vec::new()),
        Some(Badges {
            atlas: &fx.atlas,
            scale: fx.scale,
            entries: &[
                Entry {
                    text: "100x30",
                    opacity: 0.5,
                },
                Entry {
                    text: app::window::SHED_PTY,
                    opacity: 0.8,
                },
            ],
        }),
    );
    let index = fx.gpu.queue.submit([encoder.finish()]);
    fx.gpu
        .device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .expect("device poll");
    let frame = output.read_rgba(&fx.gpu.device, &fx.gpu.queue);

    let size = rects[0].expect("the size badge was drawn");
    let notice = rects[1].expect("the notice was drawn");
    assert!(
        notice.y >= size.y + size.height as i32,
        "the notice at y={} overlaps the size badge ending at y={}",
        notice.y,
        size.y + size.height as i32
    );
    assert!(
        notice.width > size.width,
        "the wider text got the narrower plate: {} vs {}",
        notice.width,
        size.width
    );

    // Each plate carries its own opacity: the stronger one is nearer black.
    let probe = |rect: &app::chrome::BadgeRect| {
        frame.pixel(
            (rect.x + 8) as u32,
            (rect.y + rect.height as i32 / 2) as u32,
        )[0]
    };
    let size_plate = probe(&size);
    let notice_plate = probe(&notice);
    assert!(
        notice_plate < size_plate,
        "the notice's plate ({notice_plate}) is no darker than the size badge's ({size_plate}), \
         so one set of uniforms drew both"
    );
    assert!(
        (f32::from(size_plate) - f32::from(SENTINEL[0]) * 0.5).abs() < 2.0,
        "the size badge's plate reads {size_plate}, not the frame at half opacity"
    );

    // ...and each one has text on it, not just a rectangle.
    for (name, rect, plate) in [
        ("size badge", size, size_plate),
        ("notice", notice, notice_plate),
    ] {
        let mut lit = 0usize;
        for y in rect.y..rect.y + rect.height as i32 {
            for x in rect.x..rect.x + rect.width as i32 {
                if frame.pixel(x as u32, y as u32)[0] > plate + 12 {
                    lit += 1;
                }
            }
        }
        assert!(lit > 20, "the {name}'s plate is there but its text is not");
    }
}

/// Halfway through the fade the plate is halfway back to the frame. This is
/// the animation as a pixel measurement: the envelope in `app::overlay` is
/// wired to the mount, rather than the mount reading a constant.
#[test]
fn the_fade_is_visible_in_the_pixels() {
    let fx = fixture();
    let mut badge = Chrome::new(&fx.gpu.device, FRAME_FORMAT);

    let mut overlay = SizeOverlay::new(true);
    overlay.resized(GridSize {
        columns: 80,
        rows: 24,
    });
    overlay.resized(GridSize {
        columns: 100,
        rows: 30,
    });

    let full = overlay.opacity();
    let (bright, rect) = frame_with_badge(&fx, &mut badge, &overlay.text(), full);
    let rect = rect.expect("the badge was drawn");
    let probe = (
        (rect.x + 8) as u32,
        (rect.y + rect.height as i32 / 2) as u32,
    );
    let bright_plate = bright.pixel(probe.0, probe.1)[0];

    overlay.tick(HOLD);
    overlay.tick(FADE / 2);
    let half = overlay.opacity();
    assert!(half > 0.0 && half < full, "{half} is not inside {full}");
    let (faded, _) = frame_with_badge(&fx, &mut badge, &overlay.text(), half);

    // Fading means moving back toward the frame: lighter than at full
    // strength, still darker than the bare frame.
    let faded_plate = faded.pixel(probe.0, probe.1)[0];
    assert!(
        faded_plate > bright_plate,
        "the half-faded plate ({faded_plate}) is no lighter than the full one ({bright_plate})"
    );
    assert!(
        faded_plate < SENTINEL[0],
        "the half-faded plate ({faded_plate}) has already reached the frame's {}",
        SENTINEL[0]
    );
}
