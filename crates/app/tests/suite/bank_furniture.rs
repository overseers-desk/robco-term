//! Done-test for the drawn furniture: what each piece of the bank actually
//! puts on the plate, read back off the GPU one piece at a time.
//!
//! The numerals, the mouldings, the pager keys, the screw heads, the
//! switchboard's lever and its tape well are descriptions
//! (`chassis::paint::Painting`), and a description is where a wrong colour, a
//! missing gradient or a dome that came out flat is invisible. So these read
//! pixels: one piece rendered on its own, over nothing, at the size its
//! rectangle asks for, through `app::chrome`'s pass and
//! `shaders/wgsl/vector.wgsl`.
//!
//! Over nothing is the point of the per-piece framing. The casting is left
//! out ([`Chrome::render`] takes `None` for it), so an alpha of zero means
//! the plate shows through there and the assertions can say "bare" and mean
//! it. Where a piece composites on the casting and on the pieces drawn ahead
//! of it, that is `bank_chrome.rs`'s business.
//!
//! Five of these are rules rather than pieces: the two gradient
//! orientations, the scale factor moving the geometry and not the colour, the
//! radial gradient at double scale, and a face the machine cannot supply.
//! They were arithmetic tests of a CPU rasteriser and are the same claims
//! about the shader that replaced it. The radial one is the sharp one: it
//! records a shipped bug where every screw dome came out flat on a HiDPI
//! screen, and the same bug is reachable in WGSL.
//!
//! One guarantee is not here and was given up with the CPU rasteriser: three
//! equal subpixel coverages compositing bit for bit as one grey coverage did.
//! Text now composites on a single alpha, the largest of its three channels.

use app::chrome::Chrome;
use chassis::color::{hex_literal_to_color, Rgba};
use chassis::furniture::Pass;
use chassis::paint::{Align, Face, Fill, Painting, RectOp, Stop, TextOp};
use chassis::{BankStrips, Cabinet, Piece, Rect};
use config::Config;
use gpu::harness::{px_index, Locked};

/// The device, or a printed reason and no test, as the chassis crate's own
/// GPU-backed done-test does it.
macro_rules! device {
    () => {
        match Locked::new() {
            Ok(gpu) => gpu,
            Err(e) => {
                eprintln!("skipping: no headless wgpu device ({e})");
                return;
            }
        }
    };
}

/// A piece's own size in device pixels: the size the raster it replaced was
/// struck at.
fn piece_size(piece: &Piece, scale: f64) -> (u32, u32) {
    let (w, h) = (
        (piece.rect.width as u32).max(1),
        (piece.rect.height as u32).max(1),
    );
    (
        (f64::from(w) * scale).round() as u32,
        (f64::from(h) * scale).round() as u32,
    )
}

/// One piece, alone, on a transparent target of its own size.
///
/// The piece is moved to the origin: a painting is written in coordinates
/// relative to its piece, so where the piece stands on the bank changes
/// nothing about what it draws.
fn render_piece(gpu: &Locked, chrome: &mut Chrome, piece: &Piece, scale: f64) -> Vec<[f32; 4]> {
    let (w, h) = piece_size(piece, scale);
    let mut at_origin = piece.clone();
    at_origin.rect = Rect::new(
        0.0,
        0.0,
        f64::from((piece.rect.width as u32).max(1)),
        f64::from((piece.rect.height as u32).max(1)),
    );

    let output = gpu.make_output(w, h);
    let view = output.create_view(&wgpu::TextureViewDescriptor::default());
    let mut encoder = gpu
        .device
        .create_command_encoder(&wgpu::CommandEncoderDescriptor {
            label: Some("furniture piece"),
        });
    encoder.begin_render_pass(&wgpu::RenderPassDescriptor {
        label: Some("bare"),
        color_attachments: &[Some(wgpu::RenderPassColorAttachment {
            view: &view,
            depth_slice: None,
            resolve_target: None,
            ops: wgpu::Operations {
                load: wgpu::LoadOp::Clear(wgpu::Color::TRANSPARENT),
                store: wgpu::StoreOp::Store,
            },
        })],
        depth_stencil_attachment: None,
        timestamp_writes: None,
        occlusion_query_set: None,
        multiview_mask: None,
    });
    chrome.render(
        &gpu.device,
        &gpu.queue,
        &mut encoder,
        &view,
        (w, h),
        (w, h),
        (w, h),
        scale,
        None,
        &std::sync::Arc::from(vec![at_origin.clone()]),
        None,
    );
    let index = gpu.queue.submit([encoder.finish()]);
    gpu.device
        .poll(wgpu::PollType::Wait {
            submission_index: Some(index),
            timeout: None,
        })
        .expect("device poll");
    gpu.read_output(&output, w, h).expect("readback")
}

/// One pixel of a readback, quantised the way the raster it replaced was:
/// premultiplied RGBA, eight bits a channel.
fn px8(pixels: &[[f32; 4]], w: u32, x: u32, y: u32) -> [u8; 4] {
    let p = pixels[px_index(w, x, y)];
    [
        (p[0].clamp(0.0, 1.0) * 255.0).round() as u8,
        (p[1].clamp(0.0, 1.0) * 255.0).round() as u8,
        (p[2].clamp(0.0, 1.0) * 255.0).round() as u8,
        (p[3].clamp(0.0, 1.0) * 255.0).round() as u8,
    ]
}

/// A description on its own, at a rectangle, for the rule tests below.
fn piece_of(painting: Painting, w: f64, h: f64) -> Piece {
    Piece::painted(Rect::new(0.0, 0.0, w, h), painting)
}

/// The per-piece readback for the shipped appliance: what the numerals, the
/// moulding and the pager actually put on the plate.
#[test]
fn a_rows_furniture_strikes_its_numeral_and_moulds_its_window() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let cfg = Config::default();
    let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
    let pieces = cabinet.furniture(&BankStrips::cold_start(cabinet.rows_visible() as usize));
    // The third row: numeral "03", so its jitter is its own.
    let row = &pieces[5 + 2 * 2];
    let (width, height) = piece_size(row, 1.0);
    let out = render_piece(&gpu, &mut chrome, row, 1.0);
    let px = |x: u32, y: u32| px8(&out, width, x, y);

    // The numeral: the lane is 46 wide less the 16px gap, the digits are
    // right-aligned in it, and the ink is the struck face's own colour
    // over nothing: the plate shows through everywhere else, which is
    // what the alpha says.
    let lane = 46 - 16;
    let ink: Vec<[u8; 4]> = (0..lane)
        .flat_map(|x| (0..height).map(move |y| (x as u32, y)))
        .map(|(x, y)| px(x, y))
        .filter(|p| p[3] > 200)
        .collect();
    assert!(!ink.is_empty(), "no numeral struck in the lane");
    // Two strikes: the shadow low and right, the lit face over it. So the
    // lane holds both the dark tone and the light one. The numeral's
    // fill colour is #9c8168.
    let fill = hex_literal_to_color("#9c8168");
    let brightest = ink.iter().map(|p| p[0]).max().unwrap();
    assert!(
        brightest as f32 / 255.0 > fill.r * 0.8,
        "the lit strike is missing: brightest red {brightest}"
    );
    // The lane's own left margin is bare plate: a printer leaves a
    // margin there rather than running the ink to the cut.
    for y in 0..height {
        assert_eq!(px(0, y)[3], 0, "ink at the lane's left cut, row {y}");
    }

    // The moulding: a five-stop vertical gradient across the row, so the
    // rim's top edge is lit, its middle drops dark, and its foot lifts
    // again. Sampled down the rim's own left shoulder, clear of the
    // window.
    let display_x = (46 + 16) as u32;
    // Clear of the rim's own 8px corner radius, and left of the hole.
    let rim_x = display_x - 2;
    let top = px(rim_x, 0)[0];
    let middle = px(rim_x, height / 2)[0];
    let foot = px(rim_x, height - 1)[0];
    assert!(
        top > middle,
        "the rim's lit top edge is missing: {top} vs {middle}"
    );
    assert!(
        foot > middle,
        "the rim's lit foot is missing: {foot} vs {middle}"
    );
    assert_eq!(px(rim_x, 0)[3], 255, "the moulding is transparent");

    // The punched hole inside it is darker than the moulding around it at
    // every height, which is the whole point of a recess.
    for y in 6..height - 6 {
        let hole = px(display_x + 10, y)[0];
        let lip = px(rim_x, y)[0];
        assert!(hole <= lip, "the hole is lighter than its rim at {y}");
    }
}

#[test]
fn the_pager_puts_its_two_keys_where_the_mock_measured_them() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let cfg = Config::default();
    let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
    let mut strips = BankStrips::cold_start(cabinet.rows_visible() as usize);
    // A single-page bank draws its rocker at 0.55, so the measured case
    // is a bank with somewhere to go.
    strips.page_count = 3;
    let pieces = cabinet.furniture(&strips);
    let pager = pieces.last().expect("a pager");
    let (pw, _) = piece_size(pager, 1.0);
    let out = render_piece(&gpu, &mut chrome, pager, 1.0);
    let px = |x: u32, y: u32| px8(&out, pw, x, y);

    // Two 56px caps, the pair centred in the lane at a 92px spread,
    // tops 38px into the block.
    let width = pager.rect.width;
    let spread = 92.0f64.min(width - 2.0 * 56.0 - 8.0);
    let prev_x = ((width - 2.0 * 56.0 - spread) / 2.0) as u32;
    let next_x = prev_x + 56 + spread as u32;
    // The key's first ridge is the brightest thing on it (#f7e8c4), two
    // pixels into the cap and two down from its top.
    for x in [prev_x, next_x] {
        let ridge = px(x + 28, 38 + 3);
        assert!(
            ridge[0] > 200 && ridge[1] > 180,
            "no lit ridge on the key at x {x}: {ridge:?}"
        );
        // ...and the cap's front face below the ridges is near-black.
        let face = px(x + 28, 38 + 30);
        assert!(face[0] < 60, "the cap's front face is not dark: {face:?}");
    }
    // Between the keys is bare plate: the pager draws nothing there.
    assert_eq!(px(prev_x + 56 + 4, 38 + 20)[3], 0);

    // A bank with one page dims the whole rocker to 0.55 rather than
    // hiding it, so the same ridge is there and darker.
    let mut one = BankStrips::cold_start(cabinet.rows_visible() as usize);
    one.page_count = 1;
    let dim_pieces = cabinet.furniture(&one);
    let dim_piece = dim_pieces.last().unwrap();
    let (dw, _) = piece_size(dim_piece, 1.0);
    let dim = render_piece(&gpu, &mut chrome, dim_piece, 1.0);
    let dimmed = px8(&dim, dw, prev_x + 28, 38 + 3)[0];
    assert!(dimmed < px(prev_x + 28, 38 + 3)[0]);
    assert!(dimmed > 0);
}

#[test]
fn a_screw_head_is_domed_lit_and_slotted() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let head = piece_of(chassis::shells::common::screw_head(28.0, 24.0), 28.0, 28.0);
    let out = render_piece(&gpu, &mut chrome, &head, 1.0);
    let px = |x: u32, y: u32| px8(&out, 28, x, y);
    // The countersink's outer ring is dark and the head inside it is not:
    // the light comes from up and left, so the dome is brightest that
    // side and falls away opposite.
    let up_left = px(9, 9);
    let down_right = px(19, 19);
    assert!(
        up_left[0] > down_right[0],
        "the dome is lit from the wrong side: {up_left:?} vs {down_right:?}"
    );
    // The slot is cut through the middle, dark against the dome around it.
    assert!(px(14, 14)[0] < up_left[0], "no slot cut in the head");
    // The corners of the item are outside the countersink disc entirely.
    assert_eq!(px(0, 0)[3], 0);
}

/// The per-piece readback for the switchboard's lever assembly at rest
/// (`shells::switchboard::row_furniture`): the machined chamfer, the drop
/// shadow sunk into the well, and the retaining screw's dark recess, each
/// where the resting (not current) lever puts them.
#[test]
fn the_switchboard_lever_lies_flat_over_the_well_at_rest() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let mut cfg = Config::default();
    cfg.chassis.shell = config::Shell::Switchboard;
    let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
    let pieces = cabinet.furniture(&BankStrips::cold_start(3));
    // `the_switchboard_has_no_plate_and_stamps_tape`: the row's own
    // `Plate` shader is [0], its painting (numeral, well, lever) is [1].
    let row = &pieces[1];
    assert_eq!(row.pass, Pass::Painted);
    let (width, _) = piece_size(row, 1.0);
    let out = render_piece(&gpu, &mut chrome, row, 1.0);
    let px = |x: u32, y: u32| px8(&out, width, x, y);

    // `row_overhang`: the switchboard's row paints 16px proud at both
    // ends, and the piece's rect is grown to hold it; every op landed
    // shifted down by that same 16.
    let top = 16.0;
    let g = cabinet.geometry();
    // `metrics::SWITCH_WELL_X + 3`: the lever's rest position.
    let lever_x = 64.0 + 3.0;
    let lever_y = (g.row_height as f64 - 54.0) / 2.0 + top;

    // The machined chamfer down the cap's right side, the brightest
    // metal on the row at rest -- sampled at the quadrilateral's own
    // centroid.
    let chamfer = px((lever_x + 64.0) as u32, (lever_y + 26.0) as u32);
    assert!(chamfer[3] > 0, "no chamfer struck on the lever");
    assert!(
        chamfer[0] > 100,
        "the chamfer should read as the row's brightest metal: {chamfer:?}"
    );

    // The cap's drop shadow into the well, sampled clear of the face and
    // the chamfer above it.
    let shadow = px((lever_x + 20.0) as u32, (lever_y + 50.0) as u32);
    assert_eq!(shadow[3], 255, "the drop shadow region is transparent");
    assert!(
        shadow[0] < 80,
        "the drop shadow should read dark: {shadow:?}"
    );

    // The retaining screw's near-black recess in the cap's left half.
    let screw = px((lever_x + 16.0) as u32, (lever_y + 23.0) as u32);
    assert!(
        screw[0] < 40,
        "the retaining screw's recess should be near-black: {screw:?}"
    );
}

/// The per-piece readback for `chassis::displays::tape::well_chrome` as it
/// actually composites on the switchboard bank, not just as the function
/// returns it in isolation (`displays::tape::mod`'s own unit tests cover
/// that): it draws the well's chrome, and this is the switchboard row that
/// mounts it (`the_switchboard_has_no_plate_and_stamps_tape`'s `pieces[2]`,
/// the well drawn whether a channel lies in it or not).
#[test]
fn the_switchboard_tape_well_composites_its_chrome() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let mut cfg = Config::default();
    cfg.chassis.shell = config::Shell::Switchboard;
    cfg.chassis.channel_display = config::ChannelDisplay::Tape;
    cfg.chassis.channel_indicator = config::ChannelIndicator::Switch;
    let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
    let pieces = cabinet.furniture(&BankStrips::cold_start(3));
    let well = &pieces[2];
    assert_eq!(well.pass, Pass::Painted);
    let (width, height) = piece_size(well, 1.0);
    let out = render_piece(&gpu, &mut chrome, well, 1.0);
    let px = |x: u32, y: u32| px8(&out, width, x, y);
    let mid_x = width / 2;

    // The floor's own vertical gradient runs darkest
    // at the top lip to lightest at the foot, so it is opaque and
    // monotone brightening top to bottom, sampled clear of the side walls
    // and the top shadow band.
    assert_eq!(px(mid_x, 6)[3], 255, "the well floor is transparent");
    let top = px(mid_x, 6)[0];
    let bottom = px(mid_x, height - 1)[0];
    assert!(
        bottom > top,
        "the floor's foot should be lighter than its lip: {top} vs {bottom}"
    );

    // The top lip's shadow, darker right under it than a few
    // pixels down where the shade has faded to the bare gradient.
    let shadow = px(mid_x, 0)[0];
    let past_shadow = px(mid_x, 6)[0];
    assert!(
        shadow <= past_shadow,
        "no shadow under the well's top lip: {shadow} vs {past_shadow}"
    );

    // The left wall falls dark with the key leaning that way
    // and the right one catches a little of it, so at the same height the
    // left edge reads darker than the right.
    let mid_y = height / 2;
    let left = px(1, mid_y)[0];
    let right = px(width - 1, mid_y)[0];
    assert!(
        left < right,
        "the well's left wall should be darker than its right: {left} vs {right}"
    );
}

/// Every shipped combination draws something, at its own rectangle, with
/// no NaN geometry and nothing off the piece it belongs to.
///
/// Only the annunciator over LEDs is measured against the recorded floor
/// (it is the shipped appliance and the configuration that floor was
/// measured on), so this is the net under the other eight: a shell
/// whose numeral lane went negative or whose pager squeezed to nothing
/// fails here rather than in a screenshot nobody takes.
#[test]
fn every_shell_and_kit_paints_a_bank_that_rasterises() {
    use config::{ChannelDisplay, ChannelIndicator, Shell};

    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    for shell in [Shell::Annunciator, Shell::SlideRule, Shell::Switchboard] {
        for display in [ChannelDisplay::Led, ChannelDisplay::Tape] {
            for indicator in [
                ChannelIndicator::Glow,
                ChannelIndicator::Pointer,
                ChannelIndicator::Switch,
            ] {
                let mut cfg = Config::default();
                cfg.chassis.shell = shell;
                cfg.chassis.channel_display = display;
                cfg.chassis.channel_indicator = indicator;
                let cabinet = Cabinet::from_config(&cfg, 1448.0, 1086.0);
                let rows = cabinet.rows_visible() as usize;
                let mut strips = BankStrips::cold_start(rows);
                strips.indicator = chassis::ChannelIndicator::from_setting(match indicator {
                    ChannelIndicator::Pointer => "pointer",
                    ChannelIndicator::Switch => "switch",
                    ChannelIndicator::Glow => "glow",
                });
                strips.page_count = 4;
                let pieces = cabinet.furniture(&strips);
                let what = format!("{shell:?}/{display:?}/{indicator:?}");
                let painted: Vec<&Piece> =
                    pieces.iter().filter(|p| p.pass == Pass::Painted).collect();
                assert!(!painted.is_empty(), "{what} draws nothing at all");
                let mut ink = 0u64;
                for p in painted {
                    assert!(
                        p.rect.width > 0.0 && p.rect.height > 0.0,
                        "{what}: a drawn piece has no area: {:?}",
                        p.rect
                    );
                    let (w, h) = piece_size(p, 1.0);
                    let out = render_piece(&gpu, &mut chrome, p, 1.0);
                    assert_eq!(
                        out.len() as u32,
                        w * h,
                        "{what}: the readback is not the piece's own size"
                    );
                    ink += out.iter().filter(|p| p[3] > 0.0).count() as u64;
                }
                assert!(ink > 1000, "{what} covered only {ink} pixels");
            }
        }
    }
}

/// The per-piece readback for `chassis::shells::slide_rule::screw_places`:
/// the hinge bracket's three screws, at their measured bank-coordinate
/// centres, turned from the bracket's own sheet metal.
#[test]
fn the_slide_rule_bolts_its_hinge_bracket_screws_to_the_casting() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let mut cfg = Config::default();
    cfg.chassis.shell = config::Shell::SlideRule;
    let cabinet = Cabinet::from_config(&cfg, 1024.0, 768.0);
    let pieces = cabinet.furniture(&BankStrips::cold_start(2));
    assert_eq!(pieces[0].pass, Pass::Plate, "the rail");
    let screws = &pieces[1..4];
    assert_eq!(screws.len(), 3);
    for p in screws {
        assert_eq!(p.pass, Pass::Painted);
        assert_eq!((p.rect.width, p.rect.height), (22.0, 22.0));
    }
    // The measured bank-coordinate centres, less the 22px head's own
    // half-width.
    for (p, (cx, cy)) in screws
        .iter()
        .zip([(46.0, 69.0), (64.0, 84.0), (47.0, 102.0)])
    {
        assert_eq!((p.rect.x, p.rect.y), (cx - 11.0, cy - 11.0));
    }

    // The dome is lit from the default light key (unoverridden in the
    // bracket's three), the same `(-0.6, -0.8)` `common::screw_head`'s
    // own default is, so it reads the same way
    // `a_screw_head_is_domed_lit_and_slotted` does: brighter up-left than
    // down-right.
    let out = render_piece(&gpu, &mut chrome, &screws[0], 1.0);
    let px = |x: u32, y: u32| px8(&out, 22, x, y);
    let up_left = px(6, 6);
    let down_right = px(15, 15);
    assert!(
        up_left[0] > down_right[0],
        "the bracket screw's dome is lit from the wrong side: {up_left:?} vs {down_right:?}"
    );
}

#[test]
fn a_vertical_gradient_runs_top_to_bottom_across_the_items_own_height() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    // The default gradient orientation, and the one every moulding in
    // the three shells uses: position 0 is the item's top edge.
    let black = Rgba::new(0.0, 0.0, 0.0, 1.0);
    let white = Rgba::new(1.0, 1.0, 1.0, 1.0);
    let mut p = Painting::new();
    p.rect(RectOp::gradient(
        Rect::new(0.0, 0.0, 4.0, 100.0),
        0.0,
        vec![Stop::new(0.0, black), Stop::new(1.0, white)],
    ));
    let out = render_piece(&gpu, &mut chrome, &piece_of(p, 4.0, 100.0), 1.0);
    let px = |x: u32, y: u32| px8(&out, 4, x, y);
    assert_eq!(px(2, 0)[0], 1); // half a pixel in from the top
    assert_eq!(px(2, 99)[0], 254);
    // Monotone all the way down, which is what says the ramp is the
    // item's height and not the image's.
    for y in 1..100 {
        assert!(px(2, y)[0] >= px(2, y - 1)[0]);
    }
}

#[test]
fn a_horizontal_gradient_runs_left_to_right_by_orientation() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let a = Rgba::new(0.0, 0.0, 0.0, 1.0);
    let b = Rgba::new(1.0, 1.0, 1.0, 1.0);
    let mut p = Painting::new();
    p.rect(RectOp::horizontal_gradient(
        Rect::new(0.0, 0.0, 100.0, 4.0),
        0.0,
        vec![Stop::new(0.0, a), Stop::new(1.0, b)],
    ));
    let out = render_piece(&gpu, &mut chrome, &piece_of(p, 100.0, 4.0), 1.0);
    let px = |x: u32, y: u32| px8(&out, 100, x, y);
    assert_eq!(px(0, 2)[0], 1);
    assert_eq!(px(99, 2)[0], 254);
    assert_eq!(px(50, 0)[0], px(50, 3)[0]);
}

#[test]
fn the_scale_factor_moves_the_geometry_and_not_the_uniforms() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    // The same description at 2x is the same picture on twice the grid: a
    // 2px lip is four device pixels tall, and its colour is unchanged.
    let c = hex_literal_to_color("#7a6448");
    let mut p = Painting::new();
    p.rect(RectOp::solid(Rect::new(1.0, 1.0, 6.0, 2.0), 0.0, c));
    let piece = piece_of(p, 8.0, 4.0);
    let one = render_piece(&gpu, &mut chrome, &piece, 1.0);
    let two = render_piece(&gpu, &mut chrome, &piece, 2.0);
    assert_eq!(px8(&one, 8, 3, 1), px8(&two, 16, 6, 2));
    assert_eq!(px8(&one, 8, 3, 2)[3], 255);
    assert_eq!(px8(&two, 16, 6, 5)[3], 255);
    assert_eq!(px8(&two, 16, 6, 6)[3], 0);
}

/// The same gradient on a 2x display is the same gradient.
///
/// A shape's arithmetic walks device pixels, so a radial's two circles have
/// to be scaled with the rest of the geometry. Unscaled they stayed in the
/// top-left quarter of the rectangle while the fragment covered all of it,
/// and every pixel past `to`'s radius padded out to the last stop: the screw
/// head's dome came out flat on any HiDPI screen.
#[test]
fn a_radial_gradient_at_double_scale_samples_where_it_does_at_single() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let a = Rgba::new(0.0, 0.0, 0.0, 0.0);
    let b = Rgba::new(0.0, 0.0, 0.0, 1.0);
    let mut p = Painting::new();
    p.rect(RectOp {
        fill: Fill::Radial {
            from: (20.0, 20.0, 5.0),
            to: (20.0, 20.0, 20.0),
            stops: vec![Stop::new(0.0, a), Stop::new(1.0, b)],
        },
        ..RectOp::solid(Rect::new(0.0, 0.0, 40.0, 40.0), 20.0, b)
    });
    let piece = piece_of(p, 40.0, 40.0);
    let single = render_piece(&gpu, &mut chrome, &piece, 1.0);
    let double = render_piece(&gpu, &mut chrome, &piece, 2.0);

    // Corresponding points: a logical (x, y) is device (x, y) at 1x and
    // (2x, 2y) at 2x. The half-pixel offsets differ, so the two rasters
    // are not bit-identical and the comparison is by ramp value.
    for (x, y) in [(20, 20), (20, 8), (20, 14), (26, 20), (20, 32)] {
        let one = px8(&single, 40, x, y)[3] as i32;
        let two = px8(&double, 80, x * 2, y * 2)[3] as i32;
        assert!(
            (one - two).abs() <= 8,
            "logical ({x}, {y}): {one} at 1x but {two} at 2x"
        );
    }

    // Not vacuous: the points above really do span the ramp, so an
    // unscaled gradient (which pads to the last stop almost everywhere)
    // could not have matched them.
    let centre = px8(&single, 40, 20, 20)[3];
    let edge = px8(&single, 40, 20, 2)[3];
    assert_eq!(centre, 0, "the inner circle should be transparent");
    assert!(edge > 200, "the outer edge should be opaque: {edge}");
}

#[test]
fn a_missing_face_paints_nothing_rather_than_the_wrong_face() {
    let gpu = device!();
    let mut chrome = Chrome::new(&gpu.device, gpu::harness::OUTPUT_FORMAT);

    let mut p = Painting::new();
    p.text(TextOp {
        face: Face::Catalogue("NO SUCH FACE"),
        x: 0.0,
        y: 0.0,
        width: 46.0,
        align: Align::Right,
        pixel_size: 34.0,
        letter_spacing: 0.0,
        bold: false,
        text: "08".to_string(),
        color: Rgba::new(1.0, 1.0, 1.0, 1.0),
        opacity: 1.0,
    });
    let out = render_piece(&gpu, &mut chrome, &piece_of(p, 46.0, 40.0), 1.0);
    assert!(out.iter().all(|p| p == &[0.0, 0.0, 0.0, 0.0]));
}
