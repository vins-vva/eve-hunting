//! Renders `assets/icon.svg` into `assets/icon.ico`, the icon `build.rs` embeds in the exe:
//! bitmaps at every size Windows asks for (16 to 128 px, which the window and taskbar icons
//! load) plus a 256 px PNG for Explorer's large views.
//!
//! `cargo run --example make_icon`

use std::fs::File;

use resvg::{tiny_skia, usvg};

const SIZES: [u32; 9] = [16, 20, 24, 32, 40, 48, 64, 128, 256];

fn main() {
    let svg = std::fs::read("assets/icon.svg").expect("assets/icon.svg");
    let tree = usvg::Tree::from_data(&svg, &usvg::Options::default()).expect("a valid SVG");
    let mut dir = ico::IconDir::new(ico::ResourceType::Icon);
    for px in SIZES {
        let mut pixmap = tiny_skia::Pixmap::new(px, px).expect("a non-empty size");
        let scale = px as f32 / tree.size().width();
        resvg::render(&tree, tiny_skia::Transform::from_scale(scale, scale), &mut pixmap.as_mut());
        // tiny-skia keeps premultiplied colour; icons store it straight.
        let rgba: Vec<u8> = pixmap
            .pixels()
            .iter()
            .flat_map(|p| {
                let c = p.demultiply();
                [c.red(), c.green(), c.blue(), c.alpha()]
            })
            .collect();
        let image = ico::IconImage::from_rgba_data(px, px, rgba);
        let entry = if px >= 256 { ico::IconDirEntry::encode_as_png(&image) } else { ico::IconDirEntry::encode_as_bmp(&image) };
        dir.add_entry(entry.expect("an encodable image"));
    }
    dir.write(File::create("assets/icon.ico").expect("assets/icon.ico")).expect("written");
    println!("assets/icon.ico: {} sizes ({SIZES:?})", SIZES.len());
}
