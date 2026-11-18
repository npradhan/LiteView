//! Decoding image bytes into displayable RGBA frames (still or animated).

use std::io::Cursor;
use std::time::Duration;

use egui::ColorImage;
use image::{AnimationDecoder, DynamicImage, ImageDecoder, ImageFormat, ImageReader};

/// Largest edge uploaded to the GPU. Bigger images are downscaled to fit
/// (still far more than any screen can show).
pub const MAX_TEXTURE_SIDE: u32 = 8192;

pub struct Frame {
    pub image: ColorImage,
    pub delay: Duration,
}

pub struct Decoded {
    pub frames: Vec<Frame>,
    /// Original pixel dimensions (before any downscaling).
    pub original_size: [u32; 2],
}

/// Decode a full image (all animation frames).
pub fn decode_full(bytes: &[u8], file_name: &str) -> Result<Decoded, String> {
    if let Some(frames) = try_decode_animation(bytes)? {
        if !frames.is_empty() {
            let original_size = [frames[0].0.width(), frames[0].0.height()];
            let frames = frames
                .into_iter()
                .map(|(img, delay)| Frame {
                    image: to_color_image(fit(DynamicImage::ImageRgba8(img), MAX_TEXTURE_SIDE)),
                    delay,
                })
                .collect();
            return Ok(Decoded {
                frames,
                original_size,
            });
        }
    }
    let img = decode_still(bytes, file_name)?;
    let original_size = [img.width(), img.height()];
    Ok(Decoded {
        frames: vec![Frame {
            image: to_color_image(fit(img, MAX_TEXTURE_SIDE)),
            delay: Duration::ZERO,
        }],
        original_size,
    })
}

/// Decode only the first frame and shrink it to fit within `max_side`.
pub fn decode_thumbnail(
    bytes: &[u8],
    file_name: &str,
    max_side: u32,
) -> Result<ColorImage, String> {
    let img = decode_still(bytes, file_name)?;
    let thumb = if img.width() > max_side || img.height() > max_side {
        img.thumbnail(max_side, max_side)
    } else {
        img
    };
    Ok(to_color_image(thumb))
}

fn fit(img: DynamicImage, max_side: u32) -> DynamicImage {
    if img.width() > max_side || img.height() > max_side {
        img.resize(max_side, max_side, image::imageops::FilterType::Triangle)
    } else {
        img
    }
}

fn to_color_image(img: DynamicImage) -> ColorImage {
    let rgba = img.into_rgba8();
    let size = [rgba.width() as usize, rgba.height() as usize];
    ColorImage::from_rgba_unmultiplied(size, rgba.as_raw())
}

#[derive(Debug, PartialEq, Eq)]
enum Kind {
    Jxl,
    Psd,
    Standard(ImageFormat),
}

fn detect(bytes: &[u8], file_name: &str) -> Option<Kind> {
    if bytes.starts_with(&[0xFF, 0x0A])
        || bytes.starts_with(&[
            0, 0, 0, 0x0C, b'J', b'X', b'L', b' ', 0x0D, 0x0A, 0x87, 0x0A,
        ])
    {
        return Some(Kind::Jxl);
    }
    if bytes.starts_with(b"8BPS") {
        return Some(Kind::Psd);
    }
    if let Ok(fmt) = image::guess_format(bytes) {
        return Some(Kind::Standard(fmt));
    }
    // Formats without reliable magic numbers (e.g. TGA) fall back to the extension.
    let ext = std::path::Path::new(file_name)
        .extension()
        .and_then(|e| e.to_str())?
        .to_ascii_lowercase();
    match ext.as_str() {
        "jxl" => Some(Kind::Jxl),
        "psd" => Some(Kind::Psd),
        "icb" | "vda" | "vst" => Some(Kind::Standard(ImageFormat::Tga)),
        "cur" => Some(Kind::Standard(ImageFormat::Ico)),
        "jfif" | "jpe" => Some(Kind::Standard(ImageFormat::Jpeg)),
        "dib" => Some(Kind::Standard(ImageFormat::Bmp)),
        "apng" => Some(Kind::Standard(ImageFormat::Png)),
        _ => ImageFormat::from_extension(&ext).map(Kind::Standard),
    }
}

fn decode_still(bytes: &[u8], file_name: &str) -> Result<DynamicImage, String> {
    match detect(bytes, file_name).ok_or("unrecognised image format")? {
        Kind::Jxl => {
            let decoder = jxl_oxide::integration::JxlDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())
        }
        Kind::Psd => {
            let psd = psd::Psd::from_bytes(bytes).map_err(|e| e.to_string())?;
            image::RgbaImage::from_raw(psd.width(), psd.height(), psd.rgba())
                .map(DynamicImage::ImageRgba8)
                .ok_or_else(|| "invalid PSD pixel data".to_owned())
        }
        Kind::Standard(fmt) => {
            let mut reader = ImageReader::with_format(Cursor::new(bytes), fmt);
            reader.no_limits();
            let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
            let orientation = decoder.orientation().ok();
            let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
            if let Some(o) = orientation {
                img.apply_orientation(o);
            }
            Ok(img)
        }
    }
}

type RawFrames = Vec<(image::RgbaImage, Duration)>;

/// Returns `Ok(None)` when the image is not animated (or not an animatable format).
fn try_decode_animation(bytes: &[u8]) -> Result<Option<RawFrames>, String> {
    let frames = match image::guess_format(bytes) {
        Ok(ImageFormat::Gif) => {
            let dec = image::codecs::gif::GifDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            dec.into_frames().collect_frames()
        }
        Ok(ImageFormat::Png) => {
            let dec = image::codecs::png::PngDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            if !dec.is_apng().unwrap_or(false) {
                return Ok(None);
            }
            dec.apng()
                .map_err(|e| e.to_string())?
                .into_frames()
                .collect_frames()
        }
        Ok(ImageFormat::WebP) => {
            let dec = image::codecs::webp::WebPDecoder::new(Cursor::new(bytes))
                .map_err(|e| e.to_string())?;
            if !dec.has_animation() {
                return Ok(None);
            }
            dec.into_frames().collect_frames()
        }
        _ => return Ok(None),
    }
    .map_err(|e| e.to_string())?;

    if frames.len() <= 1 {
        // Single-frame "animations" are displayed as stills through the normal path.
        return Ok(None);
    }
    Ok(Some(
        frames
            .into_iter()
            .map(|f| {
                let (n, d) = f.delay().numer_denom_ms();
                let ms = if d == 0 { 0 } else { n / d };
                // Same convention as web browsers: tiny delays mean "default speed".
                let ms = if ms <= 10 { 100 } else { ms };
                (f.into_buffer(), Duration::from_millis(ms as u64))
            })
            .collect(),
    ))
}

#[cfg(test)]
mod tests {
    use super::*;

    fn encode(img: &DynamicImage, fmt: ImageFormat) -> Vec<u8> {
        let mut out = Cursor::new(Vec::new());
        img.write_to(&mut out, fmt).unwrap();
        out.into_inner()
    }

    #[test]
    fn decodes_common_formats() {
        let img = DynamicImage::ImageRgba8(image::RgbaImage::from_pixel(
            40,
            30,
            image::Rgba([10, 200, 30, 255]),
        ));
        for (fmt, name) in [
            (ImageFormat::Png, "a.png"),
            (ImageFormat::Bmp, "a.bmp"),
            (ImageFormat::Tga, "a.tga"),
            (ImageFormat::Tiff, "a.tiff"),
            (ImageFormat::Gif, "a.gif"),
            (ImageFormat::Qoi, "a.qoi"),
            (ImageFormat::WebP, "a.webp"),
        ] {
            let bytes = encode(&img, fmt);
            let d = decode_full(&bytes, name).unwrap_or_else(|e| panic!("{name}: {e}"));
            assert_eq!(d.original_size, [40, 30], "{name}");
            assert_eq!(d.frames.len(), 1);
            let t = decode_thumbnail(&bytes, name, 20).unwrap();
            assert_eq!(t.size, [20, 15], "{name}");
        }
    }

    #[test]
    fn decodes_animated_gif() {
        use image::codecs::gif::GifEncoder;
        let mut buf = Vec::new();
        {
            let mut enc = GifEncoder::new(&mut buf);
            let frames = (0..3).map(|i| {
                image::Frame::from_parts(
                    image::RgbaImage::from_pixel(8, 8, image::Rgba([i * 80, 0, 0, 255])),
                    0,
                    0,
                    image::Delay::from_numer_denom_ms(50, 1),
                )
            });
            enc.encode_frames(frames).unwrap();
        }
        let d = decode_full(&buf, "x.gif").unwrap();
        assert_eq!(d.frames.len(), 3);
        assert_eq!(d.frames[0].delay, Duration::from_millis(50));
    }
}
