//! Eyeball a font's LED raster next to the golden fixture's, for when the
//! parity test reports differing pixels:
//! `cargo run -p term --example led_diff -- GOHU_11_SCALED`.
use std::path::PathBuf;
use term::fonts::{self, led};

const LED_TEXT: &str = "CH 01 AMBER 1234567890";

fn main() {
    let name = std::env::args()
        .nth(1)
        .unwrap_or_else(|| "GOHU_11_SCALED".into());
    let cols: usize = std::env::args()
        .nth(2)
        .and_then(|s| s.parse().ok())
        .unwrap_or(40);
    let entry = fonts::font_by_name(&name, fonts::FontSource::Bundled).expect("known font");
    let ours = led::led_text_image(entry.data(), entry.pixel_size, LED_TEXT).unwrap();

    let path = PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .join("tests/fixtures/led")
        .join(format!("{name}.png"));
    let decoder = png::Decoder::new(std::io::BufReader::new(std::fs::File::open(path).unwrap()));
    let mut reader = decoder.read_info().unwrap();
    let mut buf = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut buf).unwrap();
    let bpp = info.color_type.samples();
    let golden: Vec<u8> = buf[..info.buffer_size()]
        .chunks(bpp)
        .map(|px| *px.last().unwrap())
        .collect();

    println!(
        "{name}: ours {}x{}, golden {}x{}",
        ours.width, ours.height, info.width, info.height
    );
    let cols = cols.min(ours.width as usize);
    for (label, pix) in [("golden", &golden), ("ours", &ours.alpha)] {
        println!("--- {label}");
        for y in 0..ours.height as usize {
            let row: String = (0..cols)
                .map(|x| {
                    if pix[y * ours.width as usize + x] > 127 {
                        '#'
                    } else {
                        '.'
                    }
                })
                .collect();
            println!("{row}");
        }
    }
}
