//! Draw one screen of text with a configured face and a prose face, and
//! write it out as a PNG.
//!
//! It exists to make the two-face grid visible without the window, the CRT
//! chain or a display: the question it answers is which rows the renderer
//! sets in which face, and what each does to the text, and everything
//! between the grid and the glass would only be in the way.
//!
//! Usage:
//!
//! ```text
//! cargo run -p robco-term --example proportional -- <mono family> <prose family> <out.png>
//! cargo run -p robco-term --example proportional -- "DejaVu Sans Mono" "DejaVu Serif" both.png
//! ```
//!
//! The mono family is a catalogue name, bundled or installed; the prose
//! family is any family the machine has. `-` for the prose family sets every
//! row in the mono face.

use gpu::Gpu;
use term::atlas::Rasterization;
use term::cells::CellGrid;
use term::color::Scheme;
use term::fonts::sizing::{self, ScalePolicy, SizingRequest};
use term::fonts::{font_by_name, FontSource};
use term::render::GridRenderer;
use term::{ascii_charset, FontContext};

/// Lines chosen for what they separate. The first three are prose rows and
/// say whether the advance is the character's own; the table and the ruled
/// lines carry what `render::structured` looks for, and say whether a row
/// with something to line up keeps its columns.
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
    "",
    "name         size   date",
    "readme.md    1204   Sep 3",
    "----------------------------",
];
const COLS: usize = 34;

fn main() {
    let mut args = std::env::args().skip(1);
    let family = args.next().unwrap_or_else(|| "DejaVu Sans Mono".to_string());
    let prose = args.next().unwrap_or_else(|| "DejaVu Serif".to_string());
    let out = args.next().unwrap_or_else(|| "proportional.png".to_string());
    term::set_prose_family(Some(prose).filter(|p| p != "-"));

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
        "{family}: raster {}px, cell {}x{}, prose face {:?}",
        resolved.raster_pixel_size,
        atlas.cell.width,
        atlas.cell.height,
        font.prose_family()
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
    // The prose face's glyphs are cut on demand, row by row, the way the
    // live terminal admits them; the opening atlas holds the mono face only.
    for row in 0..LINES.len() {
        renderer.admit_row(
            &gpu.device,
            &gpu.queue,
            &mut font,
            &grid.cells[row * COLS..(row + 1) * COLS],
        );
    }
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
