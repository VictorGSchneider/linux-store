//! Icon decoding (PNG and JPEG XL to RGBA8), an LRU cache and a per-name accent color.

use std::collections::VecDeque;
use std::io::BufReader;
use std::panic::{AssertUnwindSafe, catch_unwind};
use std::path::Path;
use std::sync::Arc;

/// Largest accepted icon, in pixels per side. Real icons are at most 512.
const MAX_SIDE: u32 = 4096;

/// Memory the JPEG XL decoder may allocate for one icon.
const JXL_ALLOC_LIMIT: usize = 256 * 1024 * 1024;

/// A decoded image: RGBA8, `pixels.len() == width * height * 4`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decodes the icon at `path` (chosen by extension, case-insensitive: `png` or `jxl`).
/// Any failure, including a decoder panic, gives `None`; this never panics.
pub fn load(path: &Path) -> Option<Rgba> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let decode: fn(&Path) -> Option<Rgba> = match ext.as_str() {
        "png" => decode_png,
        "jxl" => decode_jxl,
        _ => return None,
    };
    // Decoders can panic on hostile input; an icon is never worth crashing for.
    catch_unwind(AssertUnwindSafe(|| decode(path)))
        .ok()
        .flatten()
}

fn decode_png(path: &Path) -> Option<Rgba> {
    let file = std::fs::File::open(path).ok()?;
    let mut decoder = png::Decoder::new(BufReader::new(file));
    // Palettes and low bit depths become 8-bit samples; 16-bit samples are cut to 8.
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let mut reader = decoder.read_info().ok()?;
    // The header is known here, before any pixel buffer exists.
    let (width, height) = (reader.info().width, reader.info().height);
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    let mut buf = vec![0; reader.output_buffer_size()?];
    let info = reader.next_frame(&mut buf).ok()?;
    buf.truncate(info.buffer_size());
    let pixels = match info.color_type {
        png::ColorType::Rgba => buf,
        png::ColorType::Rgb => buf
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        png::ColorType::GrayscaleAlpha => buf
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        png::ColorType::Grayscale => buf.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        png::ColorType::Indexed => return None, // EXPAND removes this
    };
    (pixels.len() == width as usize * height as usize * 4).then_some(Rgba {
        width,
        height,
        pixels,
    })
}

fn decode_jxl(path: &Path) -> Option<Rgba> {
    use jxl_oxide::{AllocTracker, InitializeResult, JxlImage, PixelFormat};

    let bytes = std::fs::read(path).ok()?;
    let mut uninit = JxlImage::builder()
        .alloc_tracker(AllocTracker::with_limit(JXL_ALLOC_LIMIT))
        .build_uninit();
    uninit.feed_bytes(&bytes).ok()?;
    // The header comes first: refuse huge images before decoding any frame.
    let mut image = match uninit.try_init().ok()? {
        InitializeResult::Initialized(image) => image,
        InitializeResult::NeedMoreData(_) => return None,
    };
    let (width, height) = (image.width(), image.height());
    if width == 0 || height == 0 || width > MAX_SIDE || height > MAX_SIDE {
        return None;
    }
    image.finalize().ok()?;
    let format = image.pixel_format();
    let render = image.render_frame(0).ok()?;
    let mut samples = vec![0u8; width as usize * height as usize * format.channels()];
    let written = render.stream().write_to_buffer(&mut samples);
    if written != samples.len() {
        return None;
    }
    let pixels = match format {
        PixelFormat::Rgba => samples,
        PixelFormat::Rgb => samples
            .as_chunks::<3>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[1], p[2], 255])
            .collect(),
        PixelFormat::Graya => samples
            .as_chunks::<2>()
            .0
            .iter()
            .flat_map(|p| [p[0], p[0], p[0], p[1]])
            .collect(),
        PixelFormat::Gray => samples.iter().flat_map(|&g| [g, g, g, 255]).collect(),
        PixelFormat::Cmyk | PixelFormat::Cmyka => return None,
    };
    Some(Rgba {
        width,
        height,
        pixels,
    })
}

/// A least-recently-used cache of decoded icons, keyed by path.
pub struct IconCache {
    capacity: usize,
    /// Least recently used first.
    entries: VecDeque<(String, Arc<Rgba>)>,
}

impl IconCache {
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity,
            entries: VecDeque::new(),
        }
    }

    /// Returns the icon and marks it as the most recently used.
    pub fn get(&mut self, path: &str) -> Option<Arc<Rgba>> {
        let at = self.entries.iter().position(|(key, _)| key == path)?;
        let entry = self.entries.remove(at)?;
        let image = entry.1.clone();
        self.entries.push_back(entry);
        Some(image)
    }

    /// Stores the icon as the most recently used, evicting the least recently used one
    /// when over capacity. A capacity of 0 stores nothing.
    pub fn put(&mut self, path: String, image: Arc<Rgba>) {
        self.entries.retain(|(key, _)| *key != path);
        if self.capacity == 0 {
            return;
        }
        self.entries.push_back((path, image));
        while self.entries.len() > self.capacity {
            self.entries.pop_front();
        }
    }
}

/// A deterministic color for `name`, each channel in 60..=200 (readable with white text).
pub fn accent(name: &str) -> (u8, u8, u8) {
    // FNV-1a, 64 bit: stable across runs and platforms, unlike `DefaultHasher`.
    let mut hash = 0xcbf2_9ce4_8422_2325u64;
    for byte in name.bytes() {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    let channel = |shift: u32| 60 + ((hash >> shift) % 141) as u8;
    (channel(0), channel(21), channel(42))
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn fixture(name: &str) -> PathBuf {
        Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/fixtures")
            .join(name)
    }

    fn temp_file(name: &str, bytes: &[u8]) -> (tempdir::Guard, PathBuf) {
        let guard = tempdir::Guard::new();
        let path = guard.0.join(name);
        std::fs::write(&path, bytes).unwrap();
        (guard, path)
    }

    /// A throwaway directory removed on drop.
    mod tempdir {
        use std::path::PathBuf;
        use std::sync::atomic::{AtomicU32, Ordering};

        static N: AtomicU32 = AtomicU32::new(0);

        pub struct Guard(pub PathBuf);

        impl Guard {
            pub fn new() -> Self {
                let dir = std::env::temp_dir().join(format!(
                    "store-app-icons-{}-{}",
                    std::process::id(),
                    N.fetch_add(1, Ordering::Relaxed)
                ));
                std::fs::create_dir_all(&dir).unwrap();
                Guard(dir)
            }
        }

        impl Drop for Guard {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
    }

    fn assert_red_half_alpha(img: &Rgba) {
        assert_eq!((img.width, img.height), (8, 8));
        assert_eq!(img.pixels.len(), 8 * 8 * 4);
        for px in img.pixels.chunks(4) {
            assert!(px[0] >= 250, "red channel {}", px[0]);
            assert!(px[1] <= 5, "green channel {}", px[1]);
            assert!(px[2] <= 5, "blue channel {}", px[2]);
            assert!((px[3] as i32 - 128).abs() <= 2, "alpha {}", px[3]);
        }
    }

    #[test]
    fn loads_png_with_alpha() {
        let img = load(&fixture("icon.png")).expect("png decodes");
        assert_red_half_alpha(&img);
    }

    #[test]
    fn loads_jxl_with_alpha() {
        let img = load(&fixture("icon.jxl")).expect("jxl decodes");
        assert_red_half_alpha(&img);
    }

    #[test]
    fn uppercase_extension_is_accepted() {
        let bytes = std::fs::read(fixture("icon.png")).unwrap();
        let (_g, path) = temp_file("ICON.PNG", &bytes);
        assert!(load(&path).is_some());
    }

    #[test]
    fn missing_file_is_none() {
        assert!(load(Path::new("/nonexistent/icon.png")).is_none());
        assert!(load(Path::new("/nonexistent/icon.jxl")).is_none());
    }

    #[test]
    fn truncated_png_is_none() {
        let bytes = std::fs::read(fixture("icon.png")).unwrap();
        let (_g, path) = temp_file("cut.png", &bytes[..bytes.len() / 2]);
        assert!(load(&path).is_none());
    }

    #[test]
    fn truncated_jxl_is_none() {
        let bytes = std::fs::read(fixture("icon.jxl")).unwrap();
        let (_g, path) = temp_file("cut.jxl", &bytes[..bytes.len() / 2]);
        assert!(load(&path).is_none());
    }

    #[test]
    fn unknown_extension_is_none() {
        let bytes = std::fs::read(fixture("icon.png")).unwrap();
        let (_g, path) = temp_file("icon.bmp", &bytes);
        assert!(load(&path).is_none());
        let (_g2, path2) = temp_file("icon", &bytes);
        assert!(load(&path2).is_none());
    }

    #[test]
    fn empty_file_is_none() {
        let (_g, png) = temp_file("empty.png", b"");
        assert!(load(&png).is_none());
        let (_g2, jxl) = temp_file("empty.jxl", b"");
        assert!(load(&jxl).is_none());
    }

    /// A valid, uniformly red RGBA PNG of the given size.
    fn encoded_png(width: u32, height: u32) -> Vec<u8> {
        let mut out = Vec::new();
        let mut encoder = png::Encoder::new(&mut out, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 0, 0, 255].repeat((width * height) as usize))
            .unwrap();
        writer.finish().unwrap();
        out
    }

    #[test]
    fn oversized_dimensions_are_rejected() {
        let (_g, ok) = temp_file("ok.png", &encoded_png(4096, 1));
        assert_eq!(load(&ok).map(|i| i.width), Some(4096));
        let (_g2, wide) = temp_file("wide.png", &encoded_png(4097, 1));
        assert!(load(&wide).is_none());
        let (_g3, tall) = temp_file("tall.png", &encoded_png(1, 4097));
        assert!(load(&tall).is_none());
    }

    #[test]
    fn hostile_header_is_rejected_without_a_body() {
        // A real PNG whose IHDR claims 60000x60000 (CRC patched), cut right after the header.
        let mut bytes = encoded_png(1, 1);
        bytes.truncate(8 + 8 + 13 + 4);
        bytes[16..20].copy_from_slice(&60000u32.to_be_bytes());
        bytes[20..24].copy_from_slice(&60000u32.to_be_bytes());
        let crc = crc32(&bytes[12..29]);
        bytes[29..33].copy_from_slice(&crc.to_be_bytes());
        let (_g, path) = temp_file("hostile.png", &bytes);
        assert!(load(&path).is_none());
    }

    fn crc32(data: &[u8]) -> u32 {
        let mut crc = 0xffff_ffffu32;
        for &b in data {
            crc ^= u32::from(b);
            for _ in 0..8 {
                crc = if crc & 1 != 0 {
                    (crc >> 1) ^ 0xedb8_8320
                } else {
                    crc >> 1
                };
            }
        }
        !crc
    }

    #[test]
    fn lru_evicts_the_least_recently_used() {
        let img = |n: u32| {
            Arc::new(Rgba {
                width: n,
                height: 1,
                pixels: vec![0; n as usize * 4],
            })
        };
        let mut cache = IconCache::new(2);
        cache.put("a".into(), img(1));
        cache.put("b".into(), img(2));
        assert!(cache.get("a").is_some()); // a is now the most recent
        cache.put("c".into(), img(3)); // evicts b
        assert!(cache.get("b").is_none());
        assert_eq!(cache.get("a").unwrap().width, 1);
        assert_eq!(cache.get("c").unwrap().width, 3);
    }

    #[test]
    fn lru_zero_capacity_stores_nothing() {
        let mut cache = IconCache::new(0);
        cache.put(
            "a".into(),
            Arc::new(Rgba {
                width: 1,
                height: 1,
                pixels: vec![0; 4],
            }),
        );
        assert!(cache.get("a").is_none());
    }

    #[test]
    fn accent_is_stable_and_in_range() {
        assert_eq!(accent("firefox"), accent("firefox"));
        assert_ne!(accent("firefox"), accent("gimp"));
        for name in [
            "",
            "a",
            "firefox",
            "Ünïcode ☃",
            "a-very-long-package-name-0123456789",
        ] {
            let (r, g, b) = accent(name);
            for c in [r, g, b] {
                assert!((60..=200).contains(&c), "{name}: {c}");
            }
        }
    }
}
