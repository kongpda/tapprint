//! Turn a receipt PNG, or a file opened in the window, into a 1-bit image a
//! thermal head can burn.

use std::io::Cursor;

use hayro::hayro_interpret::InterpreterSettings;
use hayro::hayro_syntax::{LoadPdfError, Pdf};
use hayro::vello_cpu::color::palette::css::WHITE;
use hayro::vello_cpu::peniko::ImageAlphaType;
use hayro::{PixmapSettings, RenderCache, RenderSettings, render};
use image::imageops::FilterType;
use image::{
    DynamicImage, GrayImage, ImageDecoder as _, ImageError, ImageFormat, ImageReader, Limits, Luma,
    RgbaImage,
};

/// Dots across the TM-T88V head. 80 mm paper is 512 dots; 58 mm paper is 360.
pub const DOTS_80MM: u32 = 512;
pub const DOTS_58MM: u32 = 360;

/// A receipt taller than this is rejected. 8 000 dots is about 1.1 m at 180 dpi.
const MAX_HEIGHT: u32 = 8_000;

/// Packed thermal bitmap. Bit 7 of each byte is the leftmost dot. `1` is black.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MonoRaster {
    pub width_bytes: u16,
    pub height: u32,
    pub packed: Vec<u8>,
}

#[derive(Debug)]
pub struct RasterError(pub String);

impl From<ImageError> for RasterError {
    fn from(error: ImageError) -> Self {
        Self(error.to_string())
    }
}

/// Decode a PNG and fit it to the head width, scaling up or down.
///
/// A page rendered at screen resolution is narrower than 512 dots. Leaving it
/// narrow prints a small receipt down the left side of the paper.
///
/// `dither` spreads grey into a dot pattern, for photos and shaded logos.
/// Leave it off for text: the plain cut-off keeps thin strokes solid.
pub fn png_to_raster(
    png: &[u8],
    max_width_dots: u32,
    dither: bool,
) -> Result<MonoRaster, RasterError> {
    check_width(max_width_dots)?;
    finish(decode_png(png)?, max_width_dots, dither)
}

/// A PDF with more pages than this is refused. Each page is a slip of its own.
const MAX_PDF_PAGES: usize = 50;

/// Decode a PNG, JPEG or PDF the person at the till opened in the window, and
/// fit each page to the head width. An image is one page.
///
/// Only for files picked by hand. Pages and the print server send PNG through
/// [`png_to_raster`], so these wider decoders never see their bytes.
///
/// A JPEG is almost always a photo, so it is dithered. A PNG or PDF is usually
/// a screenshot or a document, so it keeps the plain cut-off for sharp text.
pub fn file_to_rasters(bytes: &[u8], max_width_dots: u32) -> Result<Vec<MonoRaster>, RasterError> {
    check_width(max_width_dots)?;
    if is_pdf(bytes) {
        return pdf_to_rasters(bytes, max_width_dots);
    }
    let (image, format) = decode_file(bytes)?;
    Ok(vec![finish(
        image,
        max_width_dots,
        format == ImageFormat::Jpeg,
    )?])
}

/// PDF readers accept the header anywhere in the first 1 024 bytes.
fn is_pdf(bytes: &[u8]) -> bool {
    bytes
        .get(..1_024)
        .unwrap_or(bytes)
        .windows(5)
        .any(|window| window == b"%PDF-")
}

fn pdf_to_rasters(bytes: &[u8], max_width_dots: u32) -> Result<Vec<MonoRaster>, RasterError> {
    let pdf = Pdf::new(bytes.to_vec()).map_err(|error| {
        RasterError(
            match error {
                LoadPdfError::Decryption(_) => "password-protected PDFs cannot be printed",
                LoadPdfError::Invalid => "the PDF could not be read",
            }
            .to_string(),
        )
    })?;
    let pages = pdf.pages();
    if pages.is_empty() {
        return Err(RasterError("the PDF has no pages".to_string()));
    }
    if pages.len() > MAX_PDF_PAGES {
        return Err(RasterError(format!(
            "the PDF has {} pages; up to {MAX_PDF_PAGES} can be printed",
            pages.len()
        )));
    }
    let cache = RenderCache::new();
    let interpreter = InterpreterSettings::default();
    let render_settings = RenderSettings::default();
    pages
        .iter()
        .map(|page| {
            let (width, height) = page.render_dimensions();
            if !(width >= 1.0 && height >= 1.0) {
                return Err(RasterError("a PDF page is empty".to_string()));
            }
            let scale = max_width_dots as f32 / width;
            // Refuse before drawing: a very long page would be drawn in full first.
            if height * scale > MAX_HEIGHT as f32 {
                return Err(RasterError("a PDF page is too long for a slip".to_string()));
            }
            let pixmap = render(
                page,
                &cache,
                &interpreter,
                &render_settings,
                &PixmapSettings {
                    x_scale: scale,
                    y_scale: scale,
                    bg_color: WHITE,
                },
            );
            let (width, height) = (u32::from(pixmap.width()), u32::from(pixmap.height()));
            let rgba = RgbaImage::from_raw(
                width,
                height,
                pixmap.take_rgba8(ImageAlphaType::AlphaPremultiplied),
            )
            .ok_or_else(|| RasterError("a PDF page could not be drawn".to_string()))?;
            finish(DynamicImage::ImageRgba8(rgba), max_width_dots, false)
        })
        .collect()
}

fn check_width(max_width_dots: u32) -> Result<(), RasterError> {
    if max_width_dots == 0 || max_width_dots > 2_048 {
        return Err(RasterError("paper width is not usable".to_string()));
    }
    Ok(())
}

fn finish(
    image: DynamicImage,
    max_width_dots: u32,
    dither: bool,
) -> Result<MonoRaster, RasterError> {
    let mut gray = fit_width(image, max_width_dots)?;
    if dither {
        floyd_steinberg(&mut gray);
    }
    pack_luma(&gray)
}

/// Turn every pixel black or white, passing each one's error on to the
/// pixels not yet visited, so the average shade survives.
fn floyd_steinberg(gray: &mut GrayImage) {
    let width = gray.width() as usize;
    if width == 0 {
        return;
    }
    let mut values: Vec<i32> = gray
        .as_raw()
        .iter()
        .map(|value| i32::from(*value))
        .collect();
    let len = values.len();
    for index in 0..len {
        let old = values[index].clamp(0, 255);
        let new = if old < 128 { 0 } else { 255 };
        values[index] = new;
        let error = old - new;
        let x = index % width;
        let mut spread = |target: usize, weight: i32| {
            if let Some(value) = values.get_mut(target) {
                *value += error * weight / 16;
            }
        };
        if x + 1 < width {
            spread(index + 1, 7);
        }
        if x > 0 {
            spread(index + width - 1, 3);
        }
        spread(index + width, 5);
        if x + 1 < width {
            spread(index + width + 1, 1);
        }
    }
    for (pixel, value) in gray.iter_mut().zip(values) {
        *pixel = u8::try_from(value.clamp(0, 255)).unwrap_or(u8::MAX);
    }
}

fn decode_png(png: &[u8]) -> Result<DynamicImage, RasterError> {
    let mut limits = Limits::default();
    limits.max_image_width = Some(4_096);
    limits.max_image_height = Some(16_000);
    // A browser canvas is RGBA at the screen's pixel ratio. A long receipt at
    // 2x is 1024 x 16 000, about 64 MB decoded, so 16 MB rejected real slips.
    // The 1.5 MB body cap already bounds what a caller can send.
    limits.max_alloc = Some(64 * 1024 * 1024);
    let mut reader = ImageReader::new(Cursor::new(png));
    reader.set_format(ImageFormat::Png);
    reader.limits(limits);
    Ok(reader.decode()?)
}

/// A phone photo is about 4 000 x 3 000, and 48 MP cameras reach 8 064 x 6 048,
/// about 150 MB decoded. A local file is not bounded by the 1.5 MB body cap.
fn decode_file(bytes: &[u8]) -> Result<(DynamicImage, ImageFormat), RasterError> {
    let mut reader = ImageReader::new(Cursor::new(bytes))
        .with_guessed_format()
        .map_err(|error| RasterError(error.to_string()))?;
    let format = match reader.format() {
        Some(format @ (ImageFormat::Png | ImageFormat::Jpeg)) => format,
        _ => {
            return Err(RasterError(
                "only PNG, JPEG and PDF files can be printed".to_string(),
            ));
        }
    };
    let mut limits = Limits::default();
    limits.max_image_width = Some(10_000);
    limits.max_image_height = Some(16_000);
    limits.max_alloc = Some(256 * 1024 * 1024);
    reader.limits(limits);
    let mut decoder = reader.into_decoder()?;
    // Phones store a portrait photo sideways and say so in its EXIF tag.
    let orientation = decoder.orientation()?;
    let mut image = DynamicImage::from_decoder(decoder)?;
    image.apply_orientation(orientation);
    Ok((image, format))
}

/// Grey on white paper. A canvas export is transparent where nothing was
/// drawn, and those pixels hold black, so alpha must be laid over white first.
fn gray_on_white(image: DynamicImage) -> GrayImage {
    if !image.color().has_alpha() {
        return image.into_luma8();
    }
    let rgba = image.into_rgba8();
    GrayImage::from_fn(rgba.width(), rgba.height(), |x, y| {
        let [r, g, b, a] = rgba.get_pixel(x, y).0;
        let alpha = u32::from(a);
        let over_white = |channel: u8| (u32::from(channel) * alpha + 255 * (255 - alpha)) / 255;
        // Rec. 709 weights, the same ones `to_luma8` uses.
        let luma = (2126 * over_white(r) + 7152 * over_white(g) + 722 * over_white(b)) / 10_000;
        Luma([u8::try_from(luma).unwrap_or(u8::MAX)])
    })
}

fn fit_width(image: DynamicImage, max_width_dots: u32) -> Result<GrayImage, RasterError> {
    let gray = gray_on_white(image);
    let (width, height) = gray.dimensions();
    if width == 0 || height == 0 {
        return Err(RasterError("receipt image is empty".to_string()));
    }
    let scaled_height = if width == max_width_dots {
        height
    } else {
        u32::try_from((u64::from(height) * u64::from(max_width_dots) / u64::from(width)).max(1))
            .unwrap_or(u32::MAX)
    };
    if scaled_height == 0 || scaled_height > MAX_HEIGHT {
        return Err(RasterError("receipt image is too tall".to_string()));
    }
    if width == max_width_dots {
        return Ok(gray);
    }
    Ok(image::imageops::resize(
        &gray,
        max_width_dots,
        scaled_height,
        FilterType::CatmullRom,
    ))
}

fn pack_luma(gray: &GrayImage) -> Result<MonoRaster, RasterError> {
    let width = gray.width();
    let height = gray.height();
    let width_bytes_u32 = width.div_ceil(8);
    let width_bytes = u16::try_from(width_bytes_u32)
        .map_err(|_| RasterError("receipt image is too wide".to_string()))?;
    let stride = usize::try_from(width_bytes_u32)
        .map_err(|_| RasterError("receipt image is too wide".to_string()))?;
    let rows = usize::try_from(height)
        .map_err(|_| RasterError("receipt image is too tall".to_string()))?;
    let packed_len = stride
        .checked_mul(rows)
        .filter(|len| *len <= 2_000_000)
        .ok_or_else(|| RasterError("receipt image is too tall".to_string()))?;
    let mut packed = vec![0u8; packed_len];
    for (x, y, pixel) in gray.enumerate_pixels() {
        // Antialiased strokes sit between black and white. Values under 200
        // still burn, so thin Khmer stems are not dropped. The head has no grey.
        if pixel.0[0] < 200 {
            let byte_index = y as usize * width_bytes_u32 as usize + (x as usize / 8);
            let shift = 7 - (x % 8);
            if let Some(byte) = packed.get_mut(byte_index) {
                *byte |= 1 << shift;
            }
        }
    }
    Ok(MonoRaster {
        width_bytes,
        height,
        packed,
    })
}

const SLIP_TITLE: &str = "TAPPRINT";
const SLIP_NOTE: &str = "TEST SLIP";
/// Each glyph dot is this many printed dots square, so the text reads at
/// arm's length: 20×28 dots, about 3.5 mm tall at 203 dpi.
const SLIP_SCALE: u32 = 4;
const SLIP_HEIGHT: u32 = 240;

/// A short slip used by the Test button, so the till can check the head
/// without a receipt from the browser.
pub fn test_slip(width_dots: u32) -> MonoRaster {
    let width = width_dots.clamp(8, 2_048);
    let height = SLIP_HEIGHT;
    let stride = width.div_ceil(8) as usize;
    let mut packed = vec![0u8; stride * height as usize];
    let mut draw = |x: u32, y: u32| {
        if x < width
            && y < height
            && let Some(byte) = packed.get_mut(y as usize * stride + x as usize / 8)
        {
            *byte |= 0x80 >> (x % 8);
        }
    };
    for x in 0..width {
        for y in [0, 1, height - 2, height - 1] {
            draw(x, y);
        }
    }
    stamp(&mut draw, 16, 28, SLIP_TITLE);
    let paper = if width_dots >= DOTS_80MM {
        "80 MM"
    } else {
        "58 MM"
    };
    stamp(&mut draw, 16, 88, paper);
    stamp(&mut draw, 16, 148, SLIP_NOTE);
    MonoRaster {
        // At most 2 048 dots, so 256 bytes, which fits.
        width_bytes: u16::try_from(stride).unwrap_or(256),
        height,
        packed,
    }
}

fn stamp(draw: &mut impl FnMut(u32, u32), x: u32, y: u32, text: &str) {
    let mut cursor = x;
    for ch in text.chars() {
        if let Some(glyph) = glyph(ch) {
            for (row, bits) in glyph.iter().enumerate() {
                for col in 0..5 {
                    if bits & (1 << (4 - col)) != 0 {
                        for dy in 0..SLIP_SCALE {
                            for dx in 0..SLIP_SCALE {
                                draw(
                                    cursor + col * SLIP_SCALE + dx,
                                    y + row as u32 * SLIP_SCALE + dy,
                                );
                            }
                        }
                    }
                }
            }
        }
        cursor = cursor.saturating_add(6 * SLIP_SCALE);
    }
}

/// 5×7 glyphs, bit 4 on the left. Only the test slip needs these.
fn glyph(ch: char) -> Option<[u8; 7]> {
    Some(match ch {
        ' ' => [0, 0, 0, 0, 0, 0, 0],
        '0' => [
            0b01110, 0b10001, 0b10011, 0b10101, 0b11001, 0b10001, 0b01110,
        ],
        '5' => [
            0b11111, 0b10000, 0b11110, 0b00001, 0b00001, 0b10001, 0b01110,
        ],
        '8' => [
            0b01110, 0b10001, 0b10001, 0b01110, 0b10001, 0b10001, 0b01110,
        ],
        'A' => [
            0b01110, 0b10001, 0b10001, 0b11111, 0b10001, 0b10001, 0b10001,
        ],
        'E' => [
            0b11111, 0b10000, 0b10000, 0b11110, 0b10000, 0b10000, 0b11111,
        ],
        'I' => [
            0b01110, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b01110,
        ],
        'L' => [
            0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b10000, 0b11111,
        ],
        'M' => [
            0b10001, 0b11011, 0b10101, 0b10101, 0b10001, 0b10001, 0b10001,
        ],
        'N' => [
            0b10001, 0b11001, 0b10101, 0b10011, 0b10001, 0b10001, 0b10001,
        ],
        'P' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10000, 0b10000, 0b10000,
        ],
        'R' => [
            0b11110, 0b10001, 0b10001, 0b11110, 0b10100, 0b10010, 0b10001,
        ],
        'S' => [
            0b01111, 0b10000, 0b10000, 0b01110, 0b00001, 0b00001, 0b11110,
        ],
        'T' => [
            0b11111, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100, 0b00100,
        ],
        _ => return None,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::codecs::png::PngEncoder;
    use image::{ExtendedColorType, ImageEncoder, RgbaImage};
    use std::io::Cursor;

    fn png_row(pixels: &[u8]) -> Vec<u8> {
        let width = u32::try_from(pixels.len()).unwrap();
        let image = GrayImage::from_raw(width, 1, pixels.to_vec()).unwrap();
        let mut png = Cursor::new(Vec::new());
        PngEncoder::new(&mut png)
            .write_image(image.as_raw(), width, 1, ExtendedColorType::L8)
            .unwrap();
        png.into_inner()
    }

    #[test]
    fn black_dot_sets_the_high_bit() {
        let png = png_row(&[0, 255, 255, 255, 255, 255, 255, 255]);
        let raster = png_to_raster(&png, 8, false).unwrap();
        assert_eq!(raster.width_bytes, 1);
        assert_eq!(raster.height, 1);
        assert_eq!(raster.packed, vec![0b1000_0000]);
    }

    #[test]
    fn grey_text_still_burns_and_white_does_not() {
        let png = png_row(&[199, 200]);
        let raster = png_to_raster(&png, 2, false).unwrap();
        assert_eq!(raster.packed, vec![0b1000_0000]);
    }

    #[test]
    fn wider_than_the_head_is_scaled_to_the_head() {
        let png = png_row(&[0u8; 600]);
        let raster = png_to_raster(&png, 512, false).unwrap();
        assert_eq!(u32::from(raster.width_bytes) * 8, 512);
    }

    #[test]
    fn narrower_than_the_head_is_stretched_to_the_head() {
        let png = png_row(&[0u8; 8]);
        let raster = png_to_raster(&png, 512, false).unwrap();
        assert_eq!(u32::from(raster.width_bytes) * 8, 512);
        assert!(raster.packed.iter().any(|byte| *byte != 0));
    }

    #[test]
    fn a_transparent_background_prints_as_white_paper() {
        // Transparent black is what a browser canvas holds where nothing was drawn.
        let image = RgbaImage::from_raw(2, 1, vec![0, 0, 0, 0, 0, 0, 0, 255]).unwrap();
        let mut png = Cursor::new(Vec::new());
        PngEncoder::new(&mut png)
            .write_image(image.as_raw(), 2, 1, ExtendedColorType::Rgba8)
            .unwrap();
        let raster = png_to_raster(&png.into_inner(), 2, false).unwrap();
        assert_eq!(raster.packed, vec![0b0100_0000]);
    }

    #[test]
    fn the_test_slip_fills_the_head_and_carries_ink() {
        for dots in [DOTS_80MM, DOTS_58MM] {
            let slip = test_slip(dots);
            assert_eq!(u32::from(slip.width_bytes) * 8, dots);
            assert_eq!(
                slip.packed.len(),
                usize::from(slip.width_bytes) * SLIP_HEIGHT as usize
            );
            // The top border is solid, so the first row is all black.
            assert!(
                slip.packed[..usize::from(slip.width_bytes)]
                    .iter()
                    .all(|b| *b == 0xff)
            );
        }
    }

    #[test]
    fn every_letter_on_the_test_slip_has_a_glyph() {
        // A missing glyph prints as a gap, so the slip would read wrong.
        for text in [SLIP_TITLE, SLIP_NOTE, "80 MM", "58 MM"] {
            for ch in text.chars() {
                assert!(glyph(ch).is_some(), "no glyph for {ch:?}");
            }
        }
    }

    #[test]
    fn a_long_receipt_from_a_retina_canvas_still_prints() {
        // Chrome on a Retina Mac exports the canvas at 2x: 1024 wide, RGBA.
        // This one compresses to a small PNG, but decodes to over 16 MB.
        let (width, height) = (1024, 4_200);
        let image = RgbaImage::from_pixel(width, height, image::Rgba([255, 255, 255, 255]));
        let mut png = Cursor::new(Vec::new());
        PngEncoder::new(&mut png)
            .write_image(image.as_raw(), width, height, ExtendedColorType::Rgba8)
            .unwrap();
        let raster = png_to_raster(&png.into_inner(), DOTS_80MM, false).unwrap();
        assert_eq!(raster.height, 2_100);
    }

    #[test]
    fn dithering_keeps_a_grey_photo_grey_instead_of_solid_black() {
        // Mid grey is under the text cut-off, so without dithering a photo
        // prints as a black block. Dithered, roughly the right share burns.
        let (width, height) = (64u32, 64u32);
        let image = GrayImage::from_pixel(width, height, Luma([160]));
        let mut png = Cursor::new(Vec::new());
        PngEncoder::new(&mut png)
            .write_image(image.as_raw(), width, height, ExtendedColorType::L8)
            .unwrap();
        let png = png.into_inner();
        let dots = |raster: &MonoRaster| {
            raster
                .packed
                .iter()
                .map(|byte| byte.count_ones())
                .sum::<u32>()
        };
        let plain = png_to_raster(&png, width, false).unwrap();
        assert_eq!(dots(&plain), width * height);
        let dithered = png_to_raster(&png, width, true).unwrap();
        let share = f64::from(dots(&dithered)) / f64::from(width * height);
        // 160 of 255 is 63 % white, so about 37 % of dots burn.
        assert!((0.30..0.45).contains(&share), "share {share}");
    }

    #[test]
    fn a_slip_that_would_be_taller_than_the_limit_is_rejected() {
        let image = GrayImage::from_raw(8, 200, vec![255u8; 8 * 200]).unwrap();
        let mut png = Cursor::new(Vec::new());
        PngEncoder::new(&mut png)
            .write_image(image.as_raw(), 8, 200, ExtendedColorType::L8)
            .unwrap();
        let error = png_to_raster(&png.into_inner(), 512, false).unwrap_err();
        assert!(error.0.contains("too tall"));
    }

    fn jpeg_square(shade: u8) -> Vec<u8> {
        let image = GrayImage::from_pixel(16, 16, Luma([shade]));
        let mut jpeg = Cursor::new(Vec::new());
        image::codecs::jpeg::JpegEncoder::new(&mut jpeg)
            .write_image(image.as_raw(), 16, 16, ExtendedColorType::L8)
            .unwrap();
        jpeg.into_inner()
    }

    // Pages and the print server must stay PNG-only: JPEG decoding is only
    // for files the person at the till picked, so it must not leak into the
    // decoder that reads bytes from the network.
    #[test]
    fn pages_cannot_send_a_jpeg() {
        let error = png_to_raster(&jpeg_square(0), 16, false).unwrap_err();
        assert!(!error.0.is_empty());
    }

    #[test]
    fn an_opened_jpeg_prints() {
        let raster = file_to_rasters(&jpeg_square(0), 16).unwrap().remove(0);
        assert_eq!(raster.width_bytes, 2);
        assert_eq!(raster.height, 16);
        assert!(raster.packed.iter().all(|byte| *byte == 0xFF));
    }

    // A grey photo must come out as a dot pattern, not a solid black block.
    #[test]
    fn an_opened_jpeg_is_dithered() {
        let raster = file_to_rasters(&jpeg_square(128), 16).unwrap().remove(0);
        assert!(raster.packed.iter().any(|byte| *byte != 0xFF));
        assert!(raster.packed.iter().any(|byte| *byte != 0x00));
    }

    // A screenshot keeps the plain cut-off, so grey text burns solid.
    #[test]
    fn an_opened_png_is_not_dithered() {
        let raster = file_to_rasters(&png_row(&[128; 8]), 8).unwrap().remove(0);
        assert_eq!(raster.packed, vec![0xFF]);
    }

    #[test]
    fn an_opened_file_of_another_kind_is_refused() {
        let error = file_to_rasters(b"GIF89a not printable", 8).unwrap_err();
        assert_eq!(error.0, "only PNG, JPEG and PDF files can be printed");
    }

    /// Two pages, the first with a black box filling its left half. Built by
    /// hand with exact byte offsets, so the test needs no PDF writer.
    fn two_page_pdf() -> Vec<u8> {
        let content = "0 g 0 0 50 200 re f";
        let objects = [
            "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
            "<< /Type /Pages /Kids [3 0 R 4 0 R] /Count 2 >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] /Contents 5 0 R >>".to_string(),
            "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 100 200] >>".to_string(),
            format!(
                "<< /Length {} >>\nstream\n{content}\nendstream",
                content.len()
            ),
        ];
        let mut pdf = b"%PDF-1.4\n".to_vec();
        let mut offsets = Vec::new();
        for (index, object) in objects.iter().enumerate() {
            offsets.push(pdf.len());
            pdf.extend_from_slice(format!("{} 0 obj\n{object}\nendobj\n", index + 1).as_bytes());
        }
        let xref = pdf.len();
        pdf.extend_from_slice(
            format!("xref\n0 {}\n0000000000 65535 f \n", objects.len() + 1).as_bytes(),
        );
        for offset in offsets {
            pdf.extend_from_slice(format!("{offset:010} 00000 n \n").as_bytes());
        }
        pdf.extend_from_slice(
            format!(
                "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref}\n%%EOF\n",
                objects.len() + 1
            )
            .as_bytes(),
        );
        pdf
    }

    // Each PDF page is a slip of its own, fitted to the paper, and drawn where
    // the page puts it: black on the left half, white on the right.
    #[test]
    fn an_opened_pdf_prints_a_slip_per_page() {
        let pages = file_to_rasters(&two_page_pdf(), 16).unwrap();
        assert_eq!(pages.len(), 2);
        let first = &pages[0];
        assert_eq!(first.width_bytes, 2);
        assert_eq!(first.height, 32);
        assert!(first.packed.chunks(2).all(|row| row == [0xFF, 0x00]));
        assert!(pages[1].packed.iter().all(|byte| *byte == 0));
    }

    #[test]
    fn a_broken_pdf_is_refused() {
        let error = file_to_rasters(b"%PDF-1.7 truncated", 8).unwrap_err();
        assert_eq!(error.0, "the PDF could not be read");
    }
}
