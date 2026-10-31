//! 클립보드 비트맵(RGBA) ↔ 전송용 이미지 바이트 변환. 보낼 때는 PNG로, 받을 때는 PNG·JPEG·GIF·WebP를 푼다.

use std::io::Cursor;

use image::codecs::png::{CompressionType, FilterType, PngEncoder};
use image::{
    DynamicImage, ExtendedColorType, ImageDecoder, ImageEncoder, ImageFormat, ImageReader, Limits,
};

use crate::clipsync::{Image, MAX_RGBA};

/// 받은 이미지의 가로·세로 최대 (픽셀). 보낸 쪽이 알린 값은 믿지 않고 헤더를 직접 본다.
const MAX_SIDE: u32 = 16_384;

/// RGBA 비트맵을 PNG로 인코딩한다. 속도를 위해 빠른 압축을 쓴다.
pub fn encode_png(img: &Image) -> Result<Vec<u8>, String> {
    let (w, h) = (
        u32::try_from(img.width).map_err(|_| "이미지가 너무 큽니다")?,
        u32::try_from(img.height).map_err(|_| "이미지가 너무 큽니다")?,
    );
    let mut out = Vec::new();
    PngEncoder::new_with_quality(&mut out, CompressionType::Fast, FilterType::Adaptive)
        .write_image(&img.rgba, w, h, ExtendedColorType::Rgba8)
        .map_err(|e| e.to_string())?;
    Ok(out)
}

/// PNG·JPEG·GIF·WebP 바이트를 RGBA 비트맵으로 푼다. 다른 형식이나 너무 큰 이미지는 거부한다.
/// JPEG의 회전 정보(EXIF)는 반영한다. GIF는 첫 프레임만 쓴다.
pub fn decode(bytes: &[u8]) -> Result<Image, String> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|e| e.to_string())?;
    if !matches!(
        reader.format(),
        Some(ImageFormat::Png | ImageFormat::Jpeg | ImageFormat::Gif | ImageFormat::WebP)
    ) {
        return Err("지원하지 않는 이미지 형식입니다".into());
    }
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SIDE);
    limits.max_image_height = Some(MAX_SIDE);
    limits.max_alloc = Some(2 * MAX_RGBA as u64);
    reader.limits(limits);

    let mut decoder = reader.into_decoder().map_err(|e| e.to_string())?;
    let orientation = decoder.orientation().ok();
    let mut img = DynamicImage::from_decoder(decoder).map_err(|e| e.to_string())?;
    if let Some(o) = orientation {
        img.apply_orientation(o);
    }
    let rgba = img.into_rgba8();
    let (width, height) = (rgba.width() as usize, rgba.height() as usize);
    if rgba.as_raw().len() > MAX_RGBA {
        return Err("이미지가 너무 큽니다".into());
    }
    Ok(Image {
        width,
        height,
        rgba: rgba.into_raw(),
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    fn gradient(w: usize, h: usize) -> Image {
        let rgba = (0..w * h)
            .flat_map(|i| {
                let (x, y) = (i % w, i / w);
                [
                    (x * 255 / w) as u8,
                    (y * 255 / h) as u8,
                    ((x + y) % 256) as u8,
                    255,
                ]
            })
            .collect();
        Image {
            width: w,
            height: h,
            rgba,
        }
    }

    #[test]
    fn png_round_trips_exactly() {
        let img = gradient(37, 23);
        let png = encode_png(&img).unwrap();
        assert!(png.starts_with(b"\x89PNG"));
        assert!(decode(&png).unwrap() == img);
    }

    #[test]
    fn transparency_survives_a_png_round_trip() {
        let img = Image {
            width: 2,
            height: 1,
            rgba: vec![10, 20, 30, 0, 200, 100, 50, 128],
        };
        assert!(decode(&encode_png(&img).unwrap()).unwrap() == img);
    }

    #[test]
    fn other_formats_decode_too() {
        let img = DynamicImage::ImageRgba8(
            image::RgbaImage::from_raw(8, 4, gradient(8, 4).rgba).unwrap(),
        );
        for format in [ImageFormat::Gif, ImageFormat::WebP] {
            let mut out = Cursor::new(Vec::new());
            img.write_to(&mut out, format).unwrap();
            let got = decode(&out.into_inner()).unwrap_or_else(|e| panic!("{format:?}: {e}"));
            assert_eq!((got.width, got.height), (8, 4), "{format:?}");
        }
        let mut jpeg = Cursor::new(Vec::new());
        img.to_rgb8()
            .write_to(&mut jpeg, ImageFormat::Jpeg)
            .unwrap();
        let got = decode(&jpeg.into_inner()).unwrap();
        assert_eq!((got.width, got.height), (8, 4));
    }

    #[test]
    fn garbage_and_unlisted_formats_are_rejected() {
        assert!(decode(b"not an image at all").is_err());
        assert!(decode(&[]).is_err());
        // BMP는 이미지지만 허용 목록에 없다. (인코더 feature는 플랫폼마다 달라 시그니처만 쓴다.)
        let mut bmp = b"BM".to_vec();
        bmp.extend_from_slice(&[0u8; 64]);
        assert!(decode(&bmp).unwrap_err().contains("지원하지 않는"));
    }

    #[test]
    fn truncated_png_is_rejected() {
        let png = encode_png(&gradient(40, 40)).unwrap();
        assert!(decode(&png[..png.len() / 2]).is_err());
    }

    #[test]
    fn oversized_dimensions_are_rejected_by_the_header_alone() {
        // 1x1 PNG의 IHDR 크기만 20000x20000으로 바꾼다 (CRC가 틀려도 한도 검사가 먼저다).
        let mut png = encode_png(&gradient(1, 1)).unwrap();
        png[16..20].copy_from_slice(&20_000u32.to_be_bytes());
        png[20..24].copy_from_slice(&20_000u32.to_be_bytes());
        assert!(decode(&png).is_err());
    }
}
