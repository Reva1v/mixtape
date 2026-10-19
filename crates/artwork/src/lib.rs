//! Cover art: square thumbnails at the sizes the UI shows, and a colour palette.
//!
//! Everything is computed once when a cover arrives, so the UI never decodes a large
//! image or extracts colours at runtime.
//!
//! A cover is kept at one of two levels of [`Detail`]: `Small` (64 + 300 px) for things only
//! browsed — home shelves, search, catalog and mix pages — and `Full` (also 1024 px) for
//! library items, which fill page headers and the full-screen player.

use fast_image_resize::{ResizeOptions, Resizer};
use image::codecs::jpeg::JpegEncoder;
use image::{DynamicImage, RgbImage};
use kmeans_colors::{Sort, get_kmeans};
use palette::{FromColor, IntoColor, Lab, Srgb};
use std::collections::HashSet;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime};

/// Thumbnail edge sizes in pixels: list rows, grid cards, full-screen player.
pub const SIZES: [u32; 3] = [64, 300, 1024];
/// The size only a [`Detail::Full`] cover has.
pub const FULL_EDGE: u32 = 1024;

/// How much of a cover is kept on disk.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Detail {
    /// 64 and 300 px: rows and cards.
    Small,
    /// 64, 300 and 1024 px: library items.
    Full,
}

impl Detail {
    fn sizes(self) -> &'static [u32] {
        match self {
            Detail::Small => &SIZES[..2],
            Detail::Full => &SIZES,
        }
    }

    /// The largest size of this level; its file marks the level as stored.
    fn edge(self) -> u32 {
        self.sizes()[self.sizes().len() - 1]
    }
}

/// What [`CoverStore::cleanup`] removed.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct Cleanup {
    /// Whole covers no row refers to.
    pub covers: usize,
    /// 1024 px files of covers outside the library.
    pub full_files: usize,
    pub bytes: u64,
}

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

/// Largest cover edge accepted, in pixels; covers come from the network.
pub const MAX_EDGE: u32 = 8192;
/// Most memory the decoder may allocate for one cover.
pub const MAX_DECODE_ALLOC: u64 = 128 * 1024 * 1024;

/// Decodes a downloaded cover with size and memory limits, so a hostile or broken image
/// (a tiny file claiming 100 000 × 100 000 px) fails instead of exhausting memory.
fn decode(bytes: &[u8]) -> Result<DynamicImage> {
    let mut reader = image::ImageReader::new(std::io::Cursor::new(bytes)).with_guessed_format()?;
    let mut limits = image::Limits::default();
    limits.max_image_width = Some(MAX_EDGE);
    limits.max_image_height = Some(MAX_EDGE);
    limits.max_alloc = Some(MAX_DECODE_ALLOC);
    reader.limits(limits);
    Ok(reader.decode()?)
}

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

    /// The cover is on disk, at least at [`Detail::Small`].
    pub fn exists(&self, key: &str) -> bool {
        self.has(key, Detail::Small)
    }

    /// The cover is on disk with at least `detail`.
    pub fn has(&self, key: &str, detail: Detail) -> bool {
        self.path(key, detail.edge()).map(|p| p.exists()).unwrap_or(false)
    }

    /// The file to serve for `size`: the stored size nearest from above, or else the
    /// largest smaller one on disk (a small cover asked for at 1024). The flag is true
    /// for such a smaller stand-in.
    pub fn best_path(&self, key: &str, size: u32) -> Option<(PathBuf, bool)> {
        let wanted = self.path(key, size).ok()?;
        if wanted.exists() {
            return Some((wanted, false));
        }
        let nearest = Self::nearest_size(size);
        SIZES
            .into_iter()
            .rev()
            .filter(|s| *s < nearest)
            .map(|s| self.root.join(key).join(format!("{s}.jpg")))
            .find(|p| p.exists())
            .map(|p| (p, true))
    }

    /// Decodes `bytes`, centre-crops to a square, writes the sizes of `detail` and returns
    /// the palette.
    pub fn process(&self, key: &str, bytes: &[u8], detail: Detail) -> Result<Processed> {
        if !Self::valid_key(key) {
            return Err(Error::InvalidKey);
        }
        let img = square(decode(bytes)?.to_rgb8());
        let dir = self.root.join(key);
        std::fs::create_dir_all(&dir)?;
        let mut resizer = Resizer::new();
        let mut small = None;
        for &size in detail.sizes() {
            let thumb = resize(&mut resizer, &img, size)?;
            write_jpeg(&dir.join(format!("{size}.jpg")), &thumb)?;
            if size == SIZES[0] {
                small = Some(thumb);
            }
        }
        let palette = palette_of(small.as_ref().expect("smallest size is always produced"));
        Ok(Processed { palette })
    }

    /// Deletes covers whose key is not in `keep`, and the 1024 px file of covers not in
    /// `keep_full`. Files written within `min_age` stay: a card on screen or a sync still
    /// saving may not be in the database yet.
    pub fn cleanup(&self, keep: &HashSet<String>, keep_full: &HashSet<String>, min_age: Duration) -> Result<Cleanup> {
        let mut out = Cleanup::default();
        let entries = match std::fs::read_dir(&self.root) {
            Ok(e) => e,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(out),
            Err(e) => return Err(e.into()),
        };
        let now = SystemTime::now();
        let old = |m: &std::fs::Metadata| m.modified().ok().and_then(|t| now.duration_since(t).ok()).is_some_and(|age| age >= min_age);
        for entry in entries.flatten() {
            let Ok(key) = entry.file_name().into_string() else { continue };
            if !Self::valid_key(&key) || !entry.file_type().is_ok_and(|t| t.is_dir()) {
                continue;
            }
            let dir = entry.path();
            let files: Vec<(PathBuf, std::fs::Metadata)> = std::fs::read_dir(&dir)?
                .flatten()
                .filter_map(|f| Some((f.path(), f.metadata().ok()?)))
                .filter(|(_, m)| m.is_file())
                .collect();
            if !keep.contains(&key) {
                if files.iter().all(|(_, m)| old(m)) {
                    let bytes: u64 = files.iter().map(|(_, m)| m.len()).sum();
                    if std::fs::remove_dir_all(&dir).is_ok() {
                        out.covers += 1;
                        out.bytes += bytes;
                    }
                }
                continue;
            }
            if !keep_full.contains(&key) {
                let full = dir.join(format!("{FULL_EDGE}.jpg"));
                if let Some((_, m)) = files.iter().find(|(p, _)| *p == full)
                    && old(m)
                    // Never leave a cover without any thumbnail.
                    && dir.join(format!("{}.jpg", SIZES[1])).exists()
                    && std::fs::remove_file(&full).is_ok()
                {
                    out.full_files += 1;
                    out.bytes += m.len();
                }
            }
        }
        Ok(out)
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
    fn oversized_images_are_refused() {
        let img = RgbImage::new(MAX_EDGE + 1, 1);
        let mut png = std::io::Cursor::new(Vec::new());
        img.write_to(&mut png, ImageFormat::Png).unwrap();
        let dir = tempfile::tempdir().unwrap();
        let store = CoverStore::new(dir.path());
        let err = store.process("big", png.get_ref(), Detail::Small).unwrap_err();
        assert!(matches!(err, Error::Image(image::ImageError::Limits(_))), "{err:?}");
        assert!(!store.exists("big"));
    }

    #[test]
    fn writes_all_sizes_and_palette() {
        let dir = tempfile::tempdir().unwrap();
        let store = CoverStore::new(dir.path());
        let out = store.process("MPREabc_-1", &sample_png(), Detail::Full).unwrap();
        for size in SIZES {
            let p = store.path("MPREabc_-1", size).unwrap();
            let img = image::open(&p).unwrap();
            let expect = size.min(300);
            assert_eq!((img.width(), img.height()), (expect, expect), "{size}");
        }
        assert!(store.has("MPREabc_-1", Detail::Full));
        assert!(out.palette.len() >= 2, "{:?}", out.palette);
        assert!(out.palette.iter().all(|c| c.len() == 7 && c.starts_with('#')));
    }

    #[test]
    fn small_covers_stand_in_for_the_large_size() {
        let dir = tempfile::tempdir().unwrap();
        let store = CoverStore::new(dir.path());
        store.process("k", &sample_png(), Detail::Small).unwrap();
        assert!(store.exists("k") && !store.has("k", Detail::Full));
        assert!(!dir.path().join("k/1024.jpg").exists());
        let (p, smaller) = store.best_path("k", 1024).unwrap();
        assert!(smaller && p.ends_with("300.jpg"));
        let (p, smaller) = store.best_path("k", 64).unwrap();
        assert!(!smaller && p.ends_with("64.jpg"));
        assert!(store.best_path("missing", 300).is_none());
        // Moving into the library fills in the large size.
        store.process("k", &sample_png(), Detail::Full).unwrap();
        assert!(store.has("k", Detail::Full));
        assert!(!store.best_path("k", 1024).unwrap().1);
    }

    #[test]
    fn cleanup_drops_unreferenced_covers_and_extra_large_files() {
        let dir = tempfile::tempdir().unwrap();
        let store = CoverStore::new(dir.path());
        for key in ["lib", "browse", "gone"] {
            store.process(key, &sample_png(), Detail::Full).unwrap();
        }
        std::fs::write(dir.path().join("stray.txt"), b"x").unwrap();
        let keep: HashSet<String> = ["lib", "browse"].map(String::from).into();
        let keep_full: HashSet<String> = ["lib"].map(String::from).into();
        // Fresh files are protected.
        let none = store.cleanup(&keep, &keep_full, Duration::from_secs(3600)).unwrap();
        assert_eq!(none, Cleanup::default());
        let out = store.cleanup(&keep, &keep_full, Duration::ZERO).unwrap();
        assert_eq!((out.covers, out.full_files), (1, 1));
        assert!(out.bytes > 0);
        assert!(!dir.path().join("gone").exists());
        assert!(store.has("lib", Detail::Full));
        assert!(store.exists("browse") && !store.has("browse", Detail::Full));
        assert!(dir.path().join("stray.txt").exists());
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
