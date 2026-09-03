//! Draw one screen of text twice, at a fixed cell and at each character's own
//! advance, and write both out as PNGs.
//!
//! It exists to make the proportional experiment visible without the window,
//! the CRT chain or a display: the question it answers is what the glyphs do,
//! and everything between the grid and the glass would only be in the way.
//! `crate::proportional` is read once per process, so the two pictures are two
//! runs of this example rather than two calls in one.
//!
//! Usage:
//!
//! ```text
//! cargo run -p robco-term --example proportional -- <font family> <out.png>
//! ROBCO_PROPORTIONAL=1 cargo run -p robco-term --example proportional -- "DejaVu Sans" prop.png
//! ```

use gpu::Gpu;
use term::atlas::Rasterization;
use term::cells::CellGrid;
use term::color::Scheme;
use term::fonts::sizing::{self, ScalePolicy, SizingRequest};
use term::fonts::{font_by_name, FontSource};
use term::render::GridRenderer;
use term::{ascii_charset, FontContext};

/// Lines chosen for what they separate. The first three say whether the
/// advance is the character's own; the table and the two identical sentences
/// say what that costs a screen that expects a column to be a column.
const LINES: &[&str] = &[
    "iiiiiiiiiiiiiiiiiiiiiiiiiiiiiiii",
    "MMMMMMMMMMMMMMMMMMMMMMMMMMMMMMMM",
    "Wilhelm illicit minimal titillate",
    "",
    "The quick brown fox jumps over a",
    "The quick brown fox jumps over a",
    "",
    "+--------+--------+",
    "| alpha  | beta   |",
    "+--------+--------+",
];
const COLS: usize = 34;

fn main() {
    let mut args = std::env::args().skip(1);
    let family = args.next().unwrap_or_else(|| "DejaVu Sans".to_string());
    let out = args.next().unwrap_or_else(|| "proportional.png".to_string());

    let spec = font_by_name(&family, FontSource::System)
        .unwrap_or_else(|| panic!("{family} is not in the system catalogue"));
    let gpu = Gpu::new().expect("an offscreen device");

    let resolved = sizing::resolve(spec, &SizingRequest::default(), ScalePolicy::Floor);
    let mut font = FontContext::new(spec);
    let atlas = font.build_atlas(
        &gpu.device,
        &gpu.queue,
        &resolved,
        &ascii_charset(),
        Rasterization::for_face(&resolved),
    );
    println!(
        "{family}: raster {}px, cell {}x{}, proportional {}",
        resolved.raster_pixel_size,
        atlas.cell.width,
        atlas.cell.height,
        term::proportional()
    );

    let scheme = Scheme::monochrome([1.0, 0.69, 0.0, 1.0], [0.0, 0.0, 0.0, 1.0]);
    let mut renderer = GridRenderer::new(
        &gpu.device,
        &gpu.queue,
        atlas,
        COLS,
        LINES.len(),
        scheme.clone(),
    );
    let grid = CellGrid::from_lines(LINES, COLS, LINES.len(), &scheme);
    renderer.set_grid(&gpu.queue, &grid, None);

    let image = renderer.render_to_image(&gpu, wgpu::Color::BLACK);
    let file = std::fs::File::create(&out).expect("the output path");
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        image.width,
        image.height,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    encoder
        .write_header()
        .expect("a png header")
        .write_image_data(&image.pixels)
        .expect("the pixels");
    println!("wrote {out} ({}x{})", image.width, image.height);
}
