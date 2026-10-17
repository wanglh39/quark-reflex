include!("../src/icon_render.rs");

fn main() {
    use image::codecs::ico::IcoFrame;
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder};
    use std::io::Cursor;
    let sizes = [16u32, 24, 32, 48, 64, 128, 256];
    let frames: Vec<IcoFrame<'_>> = sizes
        .iter()
        .map(|&s| {
            let (rgba, w, h) = render_icon_rgba(s);
            let mut png = Vec::new();
            PngEncoder::new(Cursor::new(&mut png))
                .write_image(&rgba, w, h, ExtendedColorType::Rgba8)
                .expect("PNG 编码失败");
            IcoFrame::with_encoded(png, w, h, ExtendedColorType::Rgba8)
                .expect("构建 IcoFrame 失败")
        })
        .collect();
    std::fs::create_dir_all("assets").expect("创建 assets 失败");
    let f = std::fs::File::create("assets/icon.ico").expect("创建 icon.ico 失败");
    let mut enc = image::codecs::ico::IcoEncoder::new(std::io::BufWriter::new(f));
    enc.encode_images(&frames).expect("编码 ICO 失败");
    println!("assets/icon.ico 已生成（尺寸 {sizes:?}）");
}