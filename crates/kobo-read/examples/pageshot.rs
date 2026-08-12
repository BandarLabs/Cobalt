//! Renders pages of a document exactly as the device would, and writes them
//! out as PNGs, so that a change to the way something is set can be looked at
//! rather than argued about.

use std::collections::BTreeMap;

use kobo_read::{Memory, Reader};
use kobo_ui::{Chrome, DisplayMetrics, PictureCache, PictureHandle, Surface};

fn main() {
    let mut args = std::env::args().skip(1);
    let path = args.next().expect("a document to read");
    let wanted: Vec<usize> = args.filter_map(|a| a.parse().ok()).collect();

    // Without this the renderer falls back to the compiled-in grid face, which
    // has neither lower case nor mathematics in it.
    let _ = kobo_text::install(kobo_ui::display_metrics_from_env());

    let bytes = std::fs::read(&path).expect("the document");
    let document = kobo_doc::read(&path, &bytes).expect("a readable document");

    let profile = kobo_profile::CLARA_BW_391;
    let metrics = DisplayMetrics {
        width: i32::try_from(profile.width).unwrap(),
        height: i32::try_from(profile.height).unwrap(),
        pixels_per_inch: i32::from(profile.pixels_per_inch),
        text_scale: kobo_ui::display_metrics_from_env().text_scale,
    };

    // The pictures a real application would decode and hand over.
    // What one line of body text measures, which is the size a formula set
    // into that text should match.
    let body_em = u32::try_from(kobo_ui::FontSize::Body.line_height()).unwrap_or(28);
    let image_count = document.images.len();
    let mut cache = PictureCache::new(64 * 1024 * 1024);
    let mut tiles = BTreeMap::new();
    let mut next = 1u32;
    for (name, encoded) in &document.images {
        let picture = match kobo_image::decode(encoded) {
            Ok(picture) => picture,
            Err(error) => {
                println!("  {name}: will not decode: {error}");
                continue;
            }
        };
        // A formula is drawn at a fixed pixels-to-the-em so that it has detail
        // to spare; it is set on the page at the size of the text it belongs
        // to, never stretched to the width of the column.
        let (mut width, mut height) = if name.starts_with("formula:") {
            let scaled = |side: u32| (side * body_em).div_ceil(48).max(1);
            (scaled(picture.width()), scaled(picture.height()))
        } else {
            picture.size_within(
                u32::try_from(metrics.width).unwrap(),
                u32::try_from(metrics.height).unwrap(),
            )
        };
        let column = u32::try_from(metrics.width).unwrap();
        if width > column {
            height = height * column / width.max(1);
            width = column;
        }
        let mut fitted = match picture.fit(width, height) {
            Ok(fitted) => fitted,
            Err(error) => {
                println!("  {name}: will not fit to {width}x{height}: {error}");
                continue;
            }
        };
        fitted.dither(16);
        let handle = PictureHandle(next);
        next += 1;
        // The fit preserves the picture's shape, so what came back is the
        // size to declare, not the box it was asked to fit into.
        let (width, height) = (fitted.width(), fitted.height());
        let grey = fitted.into_grey();
        if cache.put(handle, width, height, grey) {
            tiles.insert(
                name.clone(),
                kobo_ui::TilePicture::new(handle, width, height),
            );
        } else {
            println!("  {name}: cache refused {width}x{height}");
        }
    }

    println!("{} pictures decoded of {} images", tiles.len(), image_count);
    let mut reader = Reader::open(document, Memory::default(), &metrics);
    reader.set_pictures(tiles, &metrics);
    println!("{} pages", reader.page_count());

    let wanted = if wanted.is_empty() { vec![0] } else { wanted };
    let last = wanted.iter().copied().max().unwrap_or(0);
    for page in 0..=last {
        if page > 0 && !reader.forward() {
            break;
        }
        if !wanted.contains(&page) {
            continue;
        }
        let screen = reader.screen("paper");
        let mut surface = Surface::new(
            usize::try_from(metrics.width).unwrap(),
            usize::try_from(metrics.height).unwrap(),
        );
        kobo_ui::render_all(
            &screen,
            &metrics,
            &Chrome::default(),
            &cache,
            &mut surface,
            None,
        );
        let png = kobo_image::encode_png_grey(
            u32::try_from(surface.width).unwrap(),
            u32::try_from(surface.height).unwrap(),
            &surface.pixels,
        )
        .expect("a page");
        std::fs::write(format!("/tmp/page_{page}.png"), png).unwrap();
        println!("wrote /tmp/page_{page}.png");
    }
}
