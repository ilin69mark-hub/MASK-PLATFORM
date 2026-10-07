//! Reading a raster file into memory.
//!
//! Lives in the CLI rather than in `mask-kernel` on purpose. The kernel is
//! compiled to WebAssembly for the browser studio, and a zlib decoder is dead
//! weight in that bundle: the browser already has a decoded bitmap on a canvas,
//! so it hands the kernel raw pixels and never touches a file format. Keeping
//! the decoder out of the kernel also keeps the kernel free of I/O, which is
//! what lets the same code run in both places unchanged.
//!
//! PNG is the format people actually have. Netpbm is here too because it is
//! trivial to read and trivial to write by hand, which makes it the right
//! format for tests and for scripted pipelines.

use std::fs;
use std::path::Path;

use mask_kernel::raster::{ImageError, Raster};

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("cannot read {path}: {source}")]
    Io {
        path: String,
        #[source]
        source: std::io::Error,
    },

    #[error("cannot decode {path}: {message}")]
    Format { path: String, message: String },

    #[error(transparent)]
    Image(#[from] ImageError),
}

/// Decode by extension, falling back to sniffing the magic bytes.
///
/// Extension-only dispatch breaks on the common case of a `.png` that is really
/// a JPEG, and silently produces garbage geometry rather than an error.
pub fn decode_file(path: &Path) -> Result<Raster, DecodeError> {
    let bytes = fs::read(path).map_err(|source| DecodeError::Io {
        path: display(path),
        source,
    })?;

    if bytes.starts_with(b"\x89PNG\r\n\x1a\n") {
        return decode_png(&bytes, path);
    }
    if bytes.starts_with(b"P5") || bytes.starts_with(b"P6") {
        return decode_netpbm(&bytes, path);
    }
    if bytes.starts_with(&[0xFF, 0xD8, 0xFF]) {
        return Err(DecodeError::Format {
            path: display(path),
            message: "JPEG found. Save as PNG: JPEG artefacts around high-contrast \
                      edges trace into ragged, uncuttable boundaries."
                .to_string(),
        });
    }
    if bytes.starts_with(b"GIF8") {
        return Err(DecodeError::Format {
            path: display(path),
            message: "GIF found. It is limited to 256 colours and is not a \
                      production input format; save as PNG."
                .to_string(),
        });
    }

    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();
    Err(DecodeError::Format {
        path: display(path),
        message: format!(
            "unrecognised image format (extension {ext:?}). Supported: PNG, PGM, PPM."
        ),
    })
}

fn display(path: &Path) -> String {
    path.display().to_string()
}

fn decode_png(bytes: &[u8], path: &Path) -> Result<Raster, DecodeError> {
    let decoder = png::Decoder::new(std::io::Cursor::new(bytes));
    let mut reader = decoder.read_info().map_err(|e| DecodeError::Format {
        path: display(path),
        message: e.to_string(),
    })?;

    // png 0.18 can decline to size the buffer up front for streaming decoders.
    let size = reader
        .output_buffer_size()
        .ok_or_else(|| DecodeError::Format {
            path: display(path),
            message: "PNG does not report a known output size".to_string(),
        })?;
    let mut buf = vec![0; size];
    let info = reader
        .next_frame(&mut buf)
        .map_err(|e| DecodeError::Format {
            path: display(path),
            message: e.to_string(),
        })?;

    let channels = match (info.color_type, info.bit_depth) {
        (png::ColorType::Grayscale, png::BitDepth::Eight) => 1,
        (png::ColorType::GrayscaleAlpha, png::BitDepth::Eight) => 2,
        (png::ColorType::Rgb, png::BitDepth::Eight) => 3,
        (png::ColorType::Rgba, png::BitDepth::Eight) => 4,
        (_, depth) => {
            return Err(DecodeError::Format {
                path: display(path),
                message: format!(
                    "unsupported PNG: {:?} at {depth:?}-bit. Convert to 8-bit \
                     greyscale, greyscale+alpha, RGB or RGBA.",
                    info.color_type
                ),
            })
        }
    };

    // Greyscale+alpha is not a channel count the kernel accepts, and the alpha
    // channel of a greyscale image carries real information, so expand it into
    // RGBA rather than dropping it.
    let (data, channels) = if channels == 2 {
        let mut rgba = Vec::with_capacity(info.width as usize * info.height as usize * 4);
        for px in buf[..info.buffer_size()].as_chunks::<2>().0 {
            rgba.extend_from_slice(&[px[0], px[0], px[0], px[1]]);
        }
        (rgba, 4)
    } else {
        (buf[..info.buffer_size()].to_vec(), channels)
    };

    Raster::new(info.width as usize, info.height as usize, channels, data)
        .map_err(DecodeError::from)
}

/// Decode binary netpbm: `P5` greyscale or `P6` RGB, maxval 255.
fn decode_netpbm(bytes: &[u8], path: &Path) -> Result<Raster, DecodeError> {
    let bad = |message: &str| DecodeError::Format {
        path: display(path),
        message: message.to_string(),
    };

    // bytes[0] is always 'P'; the format number is the second byte.
    let format = bytes[1];
    let channels = match format {
        b'5' => 1,
        b'6' => 3,
        other => {
            return Err(bad(&format!(
                "netpbm format P{} is not supported; only P5 and P6 are",
                other as char
            )))
        }
    };

    // Header: magic, width, height, maxval, then a single whitespace byte.
    // Comments run from '#' to end of line and may appear between fields.
    let mut fields: Vec<u32> = Vec::new();
    // Past the two-byte magic, so the format number is not read as width.
    let mut i = 2;
    while fields.len() < 3 {
        while i < bytes.len() && bytes[i].is_ascii_whitespace() {
            i += 1;
        }
        if i < bytes.len() && bytes[i] == b'#' {
            while i < bytes.len() && bytes[i] != b'\n' {
                i += 1;
            }
            continue;
        }
        let start = i;
        while i < bytes.len() && bytes[i].is_ascii_digit() {
            i += 1;
        }
        if start == i {
            return Err(bad("malformed netpbm header"));
        }
        let text = std::str::from_utf8(&bytes[start..i]).map_err(|_| bad("header is not ASCII"))?;
        fields.push(
            text.parse()
                .map_err(|_| bad("header number out of range"))?,
        );
    }

    let (width, height, maxval) = (fields[0] as usize, fields[1] as usize, fields[2]);
    if maxval != 255 {
        return Err(bad(&format!(
            "netpbm maxval {maxval} is not supported; only 255 is"
        )));
    }
    // Exactly one whitespace byte separates the header from the raster.
    i += 1;

    let expected = width * height * channels;
    let data = bytes
        .get(i..i + expected)
        .ok_or_else(|| {
            bad(&format!(
                "truncated: expected {expected} bytes of pixel data"
            ))
        })?
        .to_vec();

    Raster::new(width, height, channels, data).map_err(DecodeError::from)
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;

    fn temp(name: &str, bytes: &[u8]) -> PathBuf {
        let path = std::env::temp_dir().join(format!("maskctl-imageio-{name}"));
        fs::write(&path, bytes).expect("write temp file");
        path
    }

    /// A 4x2 greyscale PGM: two black rows then two white.
    fn pgm() -> Vec<u8> {
        let mut bytes = b"P5\n4 2\n255\n".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]);
        bytes.extend_from_slice(&[255, 255, 255, 255]);
        bytes
    }

    #[test]
    fn pgm_decodes_to_greyscale() {
        let path = temp("grey.pgm", &pgm());
        let raster = decode_file(&path).expect("decode");
        assert_eq!(
            (raster.width(), raster.height(), raster.channels()),
            (4, 2, 1)
        );
        assert_eq!(raster.luma(0, 0), 0.0);
        assert_eq!(raster.luma(3, 1), 255.0);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn ppm_decodes_to_rgb() {
        let mut bytes = b"P6\n2 1\n255\n".to_vec();
        bytes.extend_from_slice(&[255, 0, 0, 0, 0, 255]);
        let path = temp("colour.ppm", &bytes);
        let raster = decode_file(&path).expect("decode");
        assert_eq!(raster.channels(), 3);
        assert_eq!(raster.pixel(0, 0), [255, 0, 0, 255]);
        assert_eq!(raster.pixel(1, 0), [0, 0, 255, 255]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn netpbm_comments_are_skipped() {
        let mut bytes = b"P5\n# written by something\n4 1\n# another\n255\n".to_vec();
        bytes.extend_from_slice(&[0, 255, 0, 255]);
        let path = temp("commented.pgm", &bytes);
        let raster = decode_file(&path).expect("decode");
        assert_eq!((raster.width(), raster.height()), (4, 1));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_truncated_file_is_refused_rather_than_half_decoded() {
        let mut bytes = b"P5\n4 2\n255\n".to_vec();
        bytes.extend_from_slice(&[0, 0, 0, 0]); // half the rows
        let path = temp("short.pgm", &bytes);
        assert!(matches!(
            decode_file(&path),
            Err(DecodeError::Format { .. })
        ));
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_lying_extension_does_not_decide_the_format() {
        // A PNG renamed to .pgm must still decode, because dispatch looks at the
        // magic bytes rather than trusting the name.
        let mut png_bytes: Vec<u8> = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgb);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer
                .write_image_data(&[10, 20, 30, 40, 50, 60])
                .expect("png data");
        }
        let path = temp("mislabelled.pgm", &png_bytes);
        let raster = decode_file(&path).expect("sniffed as png");
        assert_eq!(raster.pixel(0, 0), [10, 20, 30, 255]);
        let _ = fs::remove_file(path);
    }

    #[test]
    fn jpeg_is_refused_with_a_reason() {
        let path = temp("photo.jpg", &[0xFF, 0xD8, 0xFF, 0xE0, 0, 0]);
        let message = decode_file(&path).expect_err("jpeg is not supported");
        assert!(
            message.to_string().contains("ragged"),
            "the error should explain the consequence: {message}"
        );
        let _ = fs::remove_file(path);
    }

    #[test]
    fn an_unknown_format_says_what_is_supported() {
        let path = temp("mystery.bin", b"not an image at all");
        let message = decode_file(&path).expect_err("unknown");
        assert!(message.to_string().contains("PNG"), "{message}");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn a_missing_file_reports_the_path() {
        let message = decode_file(Path::new("/nonexistent/nope.png")).expect_err("missing");
        assert!(message.to_string().contains("nope.png"), "{message}");
    }

    #[test]
    fn png_rgba_keeps_its_alpha() {
        let mut png_bytes: Vec<u8> = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 2, 1);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer
                .write_image_data(&[1, 2, 3, 255, 4, 5, 6, 0])
                .expect("png data");
        }
        let path = temp("alpha.png", &png_bytes);
        let raster = decode_file(&path).expect("decode");
        assert_eq!(raster.channels(), 4);
        assert_eq!(raster.pixel(1, 0)[3], 0, "transparent stays transparent");
        let _ = fs::remove_file(path);
    }

    #[test]
    fn greyscale_alpha_is_expanded_to_rgba() {
        let mut png_bytes: Vec<u8> = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png_bytes, 1, 1);
            encoder.set_color(png::ColorType::GrayscaleAlpha);
            encoder.set_depth(png::BitDepth::Eight);
            let mut writer = encoder.write_header().expect("png header");
            writer.write_image_data(&[77, 88]).expect("png data");
        }
        let path = temp("grey-alpha.png", &png_bytes);
        let raster = decode_file(&path).expect("decode");
        // Two channels are not representable in Raster, so alpha must survive.
        assert_eq!(raster.channels(), 4);
        assert_eq!(raster.pixel(0, 0), [77, 77, 77, 88]);
        let _ = fs::remove_file(path);
    }
}
