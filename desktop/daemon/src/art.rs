//! Album art of what plays on this computer, made small for the phone's music card.
//!
//! Only local files (`file://` art URLs, as Firefox, Chrome, VLC and most players give):
//! the daemon never fetches anything from the web, so players that only offer `https://`
//! art (Spotify) show no picture.

use std::path::{Path, PathBuf};

use image::{codecs::jpeg::JpegEncoder, imageops::FilterType};
use phonelink_core::message::{is_art_image, MAX_ART_BYTES};

/// Pictures larger than this are not read at all.
const MAX_FILE_BYTES: u64 = 5 * 1024 * 1024;
/// Longest side of the thumbnail.
const MAX_SIDE: u32 = 256;

/// The local file behind an MPRIS `mpris:artUrl`, if it is one.
pub fn art_path(url: &str) -> Option<PathBuf> {
    let path = url.trim().strip_prefix("file://")?;
    let path = PathBuf::from(crate::clipboard::percent_decode(path));
    (path.is_absolute() && path.is_file()).then_some(path)
}

/// A JPEG of at most 256 px a side and `MAX_ART_BYTES`, or `None` if the file is too big,
/// not a picture, or can't be made small enough.
pub fn thumbnail(path: &Path) -> Option<Vec<u8>> {
    if std::fs::metadata(path).ok()?.len() > MAX_FILE_BYTES {
        return None;
    }
    let img = image::ImageReader::open(path).ok()?.with_guessed_format().ok()?.decode().ok()?;
    let img = if img.width() > MAX_SIDE || img.height() > MAX_SIDE {
        img.resize(MAX_SIDE, MAX_SIDE, FilterType::Triangle)
    } else {
        img
    };
    let rgb = img.to_rgb8();
    for quality in [80, 65, 50] {
        let mut out = Vec::new();
        JpegEncoder::new_with_quality(&mut out, quality).encode_image(&rgb).ok()?;
        if out.len() <= MAX_ART_BYTES && is_art_image(&out) {
            return Some(out);
        }
    }
    None
}

/// The key the phone matches art and track by: 16 hex digits of the picture's hash.
pub fn key(jpeg: &[u8]) -> String {
    blake3::hash(jpeg).to_hex()[..16].to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_big_picture_becomes_a_small_jpeg() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("cover art.png");
        image::RgbImage::from_fn(900, 600, |x, y| image::Rgb([(x % 256) as u8, (y % 256) as u8, 120]))
            .save(&png)
            .unwrap();
        let url = format!("file://{}", png.display()).replace(' ', "%20");
        let path = art_path(&url).expect("a local file");
        let jpeg = thumbnail(&path).unwrap();
        assert!(is_art_image(&jpeg) && jpeg.starts_with(&[0xFF, 0xD8, 0xFF]));
        let back = image::load_from_memory(&jpeg).unwrap();
        assert_eq!((back.width(), back.height()), (256, 171));
        assert_eq!(key(&jpeg).len(), 16);

        assert!(art_path("https://i.scdn.co/image/abc").is_none(), "never from the web");
        assert!(art_path("file:///no/such/file.png").is_none());
        std::fs::write(dir.path().join("x.png"), b"not a picture").unwrap();
        assert!(thumbnail(&dir.path().join("x.png")).is_none());
    }
}
