//! Artwork for tags and edited covers: decode, center-crop to a square
//! (Android `SongCoverFiles.centerCropSquare`; video thumbnails are 16:9),
//! shrink to at most 1200 px and encode as JPEG q92 (Android export uses
//! 1200 px / JPEG 92).

use image::imageops::FilterType;

use crate::tags::Cover;

pub const COVER_PX: u32 = 1200;

/// `bytes` (any format `image` reads) → square JPEG cover; `None` when it
/// doesn't decode.
pub fn prepare_cover(bytes: &[u8]) -> Option<Cover> {
    let img = image::load_from_memory(bytes).ok()?;
    let img = crop_dark_bars(img);
    let (w, h) = (img.width(), img.height());
    if w == 0 || h == 0 {
        return None;
    }
    let side = w.min(h);
    let square = if w != h { img.crop_imm((w - side) / 2, (h - side) / 2, side, side) } else { img };
    let px = side.min(COVER_PX);
    let resized = if side > px { square.resize_exact(px, px, FilterType::Lanczos3) } else { square };
    let rgb = resized.to_rgb8();
    let mut jpeg = Vec::new();
    image::codecs::jpeg::JpegEncoder::new_with_quality(&mut jpeg, 92).encode_image(&rgb).ok()?;
    Some(Cover { jpeg, width: rgb.width(), height: rgb.height() })
}

/// YouTube `hqdefault` thumbnails letterbox 16:9 frames in black bars; cut
/// rows at the top/bottom that are (nearly) all black.
fn crop_dark_bars(img: image::DynamicImage) -> image::DynamicImage {
    let rgb = img.to_rgb8();
    let (w, h) = rgb.dimensions();
    if w < 16 || h < 16 {
        return img;
    }
    let dark_row = |y: u32| {
        let mut sum = 0u64;
        for x in (0..w).step_by(4) {
            let p = rgb.get_pixel(x, y);
            sum += p[0] as u64 + p[1] as u64 + p[2] as u64;
        }
        sum / ((w as u64).div_ceil(4) * 3) < 16
    };
    let mut top = 0;
    while top < h / 3 && dark_row(top) {
        top += 1;
    }
    let mut bottom = h;
    while bottom > h - h / 3 && dark_row(bottom - 1) {
        bottom -= 1;
    }
    if top == 0 && bottom == h || bottom <= top {
        return img;
    }
    img.crop_imm(0, top, w, bottom - top)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn png(w: u32, h: u32, f: impl Fn(u32, u32) -> [u8; 3]) -> Vec<u8> {
        let img = image::RgbImage::from_fn(w, h, |x, y| image::Rgb(f(x, y)));
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, image::ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn crops_to_square_and_limits_size() {
        let c = prepare_cover(&png(1600, 900, |_, _| [120, 80, 200])).unwrap();
        assert_eq!((c.width, c.height), (900, 900));
        let c = prepare_cover(&png(2000, 2000, |_, _| [10, 200, 30])).unwrap();
        assert_eq!((c.width, c.height), (1200, 1200));
        assert_eq!(&c.jpeg[..2], &[0xFF, 0xD8]);
        assert!(prepare_cover(b"not an image").is_none());
    }

    #[test]
    fn letterbox_bars_are_removed() {
        // 480x360 with 45 px black bars → 480x270 content → 270 square.
        let c = prepare_cover(&png(480, 360, |_, y| if !(45..315).contains(&y) { [0, 0, 0] } else { [200, 150, 100] })).unwrap();
        assert_eq!((c.width, c.height), (270, 270));
    }
}
