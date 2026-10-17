//! Cover art: square thumbnails at the sizes the UI shows, and a colour palette.
//!
//! Everything is computed once when a cover arrives, so the UI never decodes a large
//! image or extracts colours at runtime.

use fast_image_resize::{ResizeOptions, Resizer};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbImage};
use kmeans_colors::{Sort, get_kmeans};
use palette::{FromColor, IntoColor, Lab, Srgb};
use std::path::{Path, PathBuf};

/// Thumbnail edge sizes in pixels: list rows, grid cards, full-screen player.
pub const SIZES: [u32; 3] = [64, 300, 1024];

const JPEG_QUALITY: u8 = 86;
const PALETTE_COLORS: usize = 5;

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("image: {0}")]
    Image(#[from] image::ImageError),
    #[error("resize: {0}")]
    Resize(#[from] fast_image_resize::ResizeError),
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("invalid cover key")]
    InvalidKey,
}

pub type Result<T> = std::result::Result<T, Error>;

/// Thumbnails live in `<root>/<key>/<size>.jpg`.
#[derive(Debug, Clone)]
pub struct CoverStore {
    root: PathBuf,
}

#[derive(Debug, Clone, PartialEq)]
pub struct Processed {
    /// Colours as `#rrggbb`, most prominent first.
    pub palette: Vec<String>,
}

impl CoverStore {
    pub fn new(root: impl Into<PathBuf>) -> Self {
        Self { root: root.into() }
    }

    /// Keys come from YouTube ids, so only `[A-Za-z0-9_-]` is accepted. This also keeps
    /// the `cover://` protocol from reading outside the store.
    pub fn valid_key(key: &str) -> bool {
        !key.is_empty()
            && key.len() <= 128
            && key.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    }

    /// Picks the stored size closest to `requested` from above.
    pub fn nearest_size(requested: u32) -> u32 {
        SIZES.into_iter().find(|s| *s >= requested).unwrap_or(SIZES[SIZES.len() - 1])
    }

    pub fn path(&self, key: &str, size: u32) -> Result<PathBuf> {
        if !Self::valid_key(key) {
            return Err(Error::InvalidKey);
        }
        Ok(self.root.join(key).join(format!("{}.jpg", Self::nearest_size(size))))
    }

    pub fn exists(&self, key: &str) -> bool {
        self.path(key, SIZES[SIZES.len() - 1]).map(|p| p.exists()).unwrap_or(false)
    }

    /// Decodes `bytes`, centre-crops to a square, writes every thumbnail size and returns
    /// the palette.
    pub fn process(&self, key: &str, bytes: &[u8]) -> Result<Processed> {
        if !Self::valid_key(key) {
            return Err(Error::InvalidKey);
        }
        let img = square(image::load_from_memory(bytes)?.to_rgb8());
        let dir = self.root.join(key);
        std::fs::create_dir_all(&dir)?;
        let mut resizer = Resizer::new();
        let mut small = None;
        for size in SIZES {
            let thumb = resize(&mut resizer, &img, size)?;
            write_jpeg(&dir.join(format!("{size}.jpg")), &thumb)?;
            if size == SIZES[0] {
                small = Some(thumb);
            }
        }
        let palette = palette_of(small.as_ref().expect("smallest size is always produced"));
        Ok(Processed { palette })
    }
}

impl CoverStore {
    /// Palette of a cover already in the store, from its smallest thumbnail.
    pub fn palette(&self, key: &str) -> Result<Vec<String>> {
        let img = image::open(self.path(key, SIZES[0])?)?.to_rgb8();
        Ok(palette_of(&img))
    }
}

fn square(img: RgbImage) -> RgbImage {
    let (w, h) = img.dimensions();
    if w == h {
        return img;
    }
    let side = w.min(h);
    DynamicImage::ImageRgb8(img).crop_imm((w - side) / 2, (h - side) / 2, side, side).to_rgb8()
}

fn resize(resizer: &mut Resizer, src: &RgbImage, size: u32) -> Result<RgbImage> {
    if src.width() <= size {
        return Ok(src.clone());
    }
    let mut dst = RgbImage::new(size, size);
    resizer.resize(src, &mut dst, &ResizeOptions::new())?;
    Ok(dst)
}

fn write_jpeg(path: &Path, img: &RgbImage) -> Result<()> {
    let tmp = path.with_extension("jpg.tmp");
    {
        let mut file = std::io::BufWriter::new(std::fs::File::create(&tmp)?);
        JpegEncoder::new_with_quality(&mut file, JPEG_QUALITY).encode_image(img)?;
    }
    std::fs::rename(tmp, path)?;
    Ok(())
}

/// K-means in Lab space over the 64 px thumbnail; colours ordered by share of the image.
fn palette_of(img: &RgbImage) -> Vec<String> {
    let lab: Vec<Lab> = img
        .pixels()
        .map(|p| Srgb::new(p[0], p[1], p[2]).into_format::<f32>().into_color())
        .collect();
    let k = PALETTE_COLORS.min(lab.len().max(1));
    let result = get_kmeans(k, 20, 5.0, false, &lab, 42);
    let mut data = Lab::sort_indexed_colors(&result.centroids, &result.indices);
    data.sort_by(|a, b| b.percentage.total_cmp(&a.percentage));
    data.into_iter()
        .map(|d| {
            let rgb: Srgb<u8> = Srgb::from_color(d.centroid).into_format();
            format!("#{:02x}{:02x}{:02x}", rgb.red, rgb.green, rgb.blue)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageFormat, Rgb};

    fn sample_png() -> Vec<u8> {
        // Left half red, right half blue, 400x300 so the square crop is exercised.
        let img = RgbImage::from_fn(400, 300, |x, _| {
            if x < 200 { Rgb([220, 20, 60]) } else { Rgb([30, 60, 200]) }
        });
        let mut out = std::io::Cursor::new(Vec::new());
        img.write_to(&mut out, ImageFormat::Png).unwrap();
        out.into_inner()
    }

    #[test]
    fn writes_all_sizes_and_palette() {
        let dir = tempfile::tempdir().unwrap();
        let store = CoverStore::new(dir.path());
        let out = store.process("MPREabc_-1", &sample_png()).unwrap();
        for size in SIZES {
            let p = store.path("MPREabc_-1", size).unwrap();
            let img = image::open(&p).unwrap();
            let expect = size.min(300);
            assert_eq!((img.width(), img.height()), (expect, expect), "{size}");
        }
        assert!(out.palette.len() >= 2, "{:?}", out.palette);
        assert!(out.palette.iter().all(|c| c.len() == 7 && c.starts_with('#')));
    }

    #[test]
    fn rejects_bad_keys_and_picks_sizes() {
        assert!(!CoverStore::valid_key("../etc"));
        assert!(!CoverStore::valid_key(""));
        assert!(CoverStore::valid_key("UCabc-_09"));
        assert_eq!(CoverStore::nearest_size(48), 64);
        assert_eq!(CoverStore::nearest_size(64), 64);
        assert_eq!(CoverStore::nearest_size(200), 300);
        assert_eq!(CoverStore::nearest_size(4000), 1024);
    }
}
