use ob_core::{Error, Result};
use serde::Deserialize;

/// Maximum allowed dimension (width or height) to prevent abuse.
const MAX_DIMENSION: u32 = 4096;

/// Default JPEG quality.
const DEFAULT_JPEG_QUALITY: u8 = 85;

/// Image transformation parameters (parsed from URL query params).
#[derive(Debug, Clone, Deserialize)]
pub struct TransformParams {
    /// Target width in pixels.
    pub w: Option<u32>,
    /// Target height in pixels.
    pub h: Option<u32>,
    /// Fit mode: "cover" (crop to fill), "contain" (fit within), "fill" (stretch).
    #[serde(default = "default_fit")]
    pub fit: String,
    /// Output quality (1-100, for JPEG/WebP).
    pub q: Option<u8>,
    /// Output format: "jpeg", "png", "webp".
    pub format: Option<String>,
}

fn default_fit() -> String {
    "cover".to_string()
}

impl Default for TransformParams {
    fn default() -> Self {
        Self {
            w: None,
            h: None,
            fit: default_fit(),
            q: None,
            format: None,
        }
    }
}

impl TransformParams {
    /// Returns `true` if any transformation is requested.
    pub fn has_transforms(&self) -> bool {
        self.w.is_some() || self.h.is_some() || self.format.is_some()
    }

    /// Clamp dimensions to MAX_DIMENSION.
    fn clamped_dimensions(&self) -> (Option<u32>, Option<u32>) {
        let w = self.w.map(|v| v.min(MAX_DIMENSION));
        let h = self.h.map(|v| v.min(MAX_DIMENSION));
        (w, h)
    }
}

/// Apply image transformations to raw image bytes.
/// Returns `(transformed_bytes, content_type)`.
pub fn transform_image(data: &[u8], params: &TransformParams) -> Result<(Vec<u8>, String)> {
    use image::ImageReader;
    use std::io::Cursor;

    if !params.has_transforms() {
        // No transforms requested — return original unchanged.
        // We can't know the original content type here, so the caller should
        // handle this case before calling transform_image.
        return Ok((data.to_vec(), "application/octet-stream".to_string()));
    }

    // Load image from bytes
    let reader = ImageReader::new(Cursor::new(data))
        .with_guessed_format()
        .map_err(|e| Error::Validation(format!("Cannot read image: {e}")))?;

    let img = reader
        .decode()
        .map_err(|e| Error::Validation(format!("Failed to decode image: {e}")))?;

    // Apply resize
    let (target_w, target_h) = params.clamped_dimensions();
    let img = apply_resize(img, target_w, target_h, &params.fit);

    // Determine output format
    let out_format = params.format.as_deref().unwrap_or("jpeg");

    // Encode
    let (bytes, content_type) = encode_image(&img, out_format, params.q)?;

    Ok((bytes, content_type))
}

/// Resize the image according to the fit mode.
fn apply_resize(
    img: DynamicImage,
    target_w: Option<u32>,
    target_h: Option<u32>,
    fit: &str,
) -> DynamicImage {
    use image::imageops::FilterType;

    let (w, h) = match (target_w, target_h) {
        (Some(w), Some(h)) if w > 0 && h > 0 => (w, h),
        (Some(w), None) if w > 0 => {
            // Scale height proportionally
            let ratio = w as f64 / img.width() as f64;
            let h = (img.height() as f64 * ratio).round() as u32;
            (w, h.max(1))
        }
        (None, Some(h)) if h > 0 => {
            // Scale width proportionally
            let ratio = h as f64 / img.height() as f64;
            let w = (img.width() as f64 * ratio).round() as u32;
            (w.max(1), h)
        }
        _ => return img, // No valid dimensions
    };

    match fit {
        "cover" => resize_cover(img, w, h),
        "contain" => img.resize(w, h, FilterType::Lanczos3),
        "fill" => img.resize_exact(w, h, FilterType::Lanczos3),
        _ => resize_cover(img, w, h), // default to cover
    }
}

/// Cover mode: resize to fill both dimensions, then crop the center.
fn resize_cover(img: DynamicImage, target_w: u32, target_h: u32) -> DynamicImage {
    use image::imageops::FilterType;

    let src_w = img.width() as f64;
    let src_h = img.height() as f64;
    let scale_w = target_w as f64 / src_w;
    let scale_h = target_h as f64 / src_h;

    // Use the larger scale so the image covers the target area
    let scale = scale_w.max(scale_h);
    let intermediate_w = (src_w * scale).round() as u32;
    let intermediate_h = (src_h * scale).round() as u32;

    let resized = img.resize_exact(
        intermediate_w.max(1),
        intermediate_h.max(1),
        FilterType::Lanczos3,
    );

    // Crop center
    let x = (intermediate_w.saturating_sub(target_w)) / 2;
    let y = (intermediate_h.saturating_sub(target_h)) / 2;
    resized.crop_imm(x, y, target_w, target_h)
}

use image::DynamicImage;

/// Encode a `DynamicImage` into bytes in the requested format.
fn encode_image(
    img: &DynamicImage,
    format: &str,
    quality: Option<u8>,
) -> Result<(Vec<u8>, String)> {
    use image::ImageFormat;
    use image::codecs::jpeg::JpegEncoder;
    use std::io::Cursor;

    let mut buf = Cursor::new(Vec::new());

    match format {
        "jpeg" | "jpg" => {
            let q = quality.unwrap_or(DEFAULT_JPEG_QUALITY);
            let encoder = JpegEncoder::new_with_quality(&mut buf, q);
            img.write_with_encoder(encoder)
                .map_err(|e| Error::Internal(format!("JPEG encode failed: {e}")))?;
            Ok((buf.into_inner(), "image/jpeg".to_string()))
        }
        "png" => {
            img.write_to(&mut buf, ImageFormat::Png)
                .map_err(|e| Error::Internal(format!("PNG encode failed: {e}")))?;
            Ok((buf.into_inner(), "image/png".to_string()))
        }
        "webp" => {
            img.write_to(&mut buf, ImageFormat::WebP)
                .map_err(|e| Error::Internal(format!("WebP encode failed: {e}")))?;
            Ok((buf.into_inner(), "image/webp".to_string()))
        }
        _ => Err(Error::Validation(format!(
            "Unsupported output format: {format}. Use jpeg, png, or webp"
        ))),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_has_transforms_false_when_default() {
        let params = TransformParams::default();
        assert!(!params.has_transforms());
    }

    #[test]
    fn test_has_transforms_true_with_width() {
        let params = TransformParams {
            w: Some(200),
            ..Default::default()
        };
        assert!(params.has_transforms());
    }

    #[test]
    fn test_has_transforms_true_with_height() {
        let params = TransformParams {
            h: Some(200),
            ..Default::default()
        };
        assert!(params.has_transforms());
    }

    #[test]
    fn test_has_transforms_true_with_format() {
        let params = TransformParams {
            format: Some("webp".to_string()),
            ..Default::default()
        };
        assert!(params.has_transforms());
    }

    #[test]
    fn test_default_fit_is_cover() {
        let params = TransformParams::default();
        assert_eq!(params.fit, "cover");
    }

    #[test]
    fn test_dimension_clamping() {
        let params = TransformParams {
            w: Some(10000),
            h: Some(5000),
            ..Default::default()
        };
        let (w, h) = params.clamped_dimensions();
        assert_eq!(w, Some(MAX_DIMENSION));
        assert_eq!(h, Some(MAX_DIMENSION));
    }

    #[test]
    fn test_dimension_no_clamping_when_within_bounds() {
        let params = TransformParams {
            w: Some(800),
            h: Some(600),
            ..Default::default()
        };
        let (w, h) = params.clamped_dimensions();
        assert_eq!(w, Some(800));
        assert_eq!(h, Some(600));
    }

    /// Create a 10x10 red PNG image programmatically.
    fn create_test_png(width: u32, height: u32) -> Vec<u8> {
        use image::{ImageBuffer, ImageFormat, Rgba};
        use std::io::Cursor;

        let img = ImageBuffer::from_fn(width, height, |_x, _y| Rgba([255u8, 0, 0, 255]));
        let mut buf = Cursor::new(Vec::new());
        img.write_to(&mut buf, ImageFormat::Png).unwrap();
        buf.into_inner()
    }

    #[test]
    fn test_transform_no_op_returns_original() {
        let png = create_test_png(10, 10);
        let params = TransformParams::default();
        let (result, _ct) = transform_image(&png, &params).unwrap();
        assert_eq!(result, png);
    }

    #[test]
    fn test_transform_resize_contain() {
        let png = create_test_png(100, 50);
        let params = TransformParams {
            w: Some(50),
            h: Some(50),
            fit: "contain".to_string(),
            format: Some("png".to_string()),
            ..Default::default()
        };
        let (result, ct) = transform_image(&png, &params).unwrap();
        assert_eq!(ct, "image/png");

        // Decode and verify aspect ratio is maintained (contain fits within 50x50)
        let decoded = image::load_from_memory(&result).unwrap();
        // 100x50 contain to 50x50 → 50x25 (width hits limit, height scales proportionally)
        assert_eq!(decoded.width(), 50);
        assert_eq!(decoded.height(), 25);
    }

    #[test]
    fn test_transform_resize_fill() {
        let png = create_test_png(100, 50);
        let params = TransformParams {
            w: Some(30),
            h: Some(30),
            fit: "fill".to_string(),
            format: Some("png".to_string()),
            ..Default::default()
        };
        let (result, ct) = transform_image(&png, &params).unwrap();
        assert_eq!(ct, "image/png");

        let decoded = image::load_from_memory(&result).unwrap();
        assert_eq!(decoded.width(), 30);
        assert_eq!(decoded.height(), 30);
    }

    #[test]
    fn test_transform_resize_cover() {
        let png = create_test_png(100, 50);
        let params = TransformParams {
            w: Some(30),
            h: Some(30),
            fit: "cover".to_string(),
            format: Some("png".to_string()),
            ..Default::default()
        };
        let (result, ct) = transform_image(&png, &params).unwrap();
        assert_eq!(ct, "image/png");

        let decoded = image::load_from_memory(&result).unwrap();
        assert_eq!(decoded.width(), 30);
        assert_eq!(decoded.height(), 30);
    }

    #[test]
    fn test_transform_format_conversion_to_jpeg() {
        let png = create_test_png(10, 10);
        let params = TransformParams {
            w: Some(10),
            format: Some("jpeg".to_string()),
            ..Default::default()
        };
        let (_result, ct) = transform_image(&png, &params).unwrap();
        assert_eq!(ct, "image/jpeg");
    }

    #[test]
    fn test_transform_format_conversion_to_webp() {
        let png = create_test_png(10, 10);
        let params = TransformParams {
            w: Some(10),
            format: Some("webp".to_string()),
            ..Default::default()
        };
        let (_result, ct) = transform_image(&png, &params).unwrap();
        assert_eq!(ct, "image/webp");
    }

    #[test]
    fn test_transform_invalid_format_errors() {
        let png = create_test_png(10, 10);
        let params = TransformParams {
            w: Some(10),
            format: Some("bmp".to_string()),
            ..Default::default()
        };
        let result = transform_image(&png, &params);
        assert!(result.is_err());
    }

    #[test]
    fn test_transform_width_only_scales_proportionally() {
        let png = create_test_png(100, 50);
        let params = TransformParams {
            w: Some(50),
            format: Some("png".to_string()),
            ..Default::default()
        };
        let (result, _) = transform_image(&png, &params).unwrap();
        let decoded = image::load_from_memory(&result).unwrap();
        assert_eq!(decoded.width(), 50);
        assert_eq!(decoded.height(), 25);
    }

    #[test]
    fn test_transform_height_only_scales_proportionally() {
        let png = create_test_png(100, 50);
        let params = TransformParams {
            h: Some(25),
            format: Some("png".to_string()),
            ..Default::default()
        };
        let (result, _) = transform_image(&png, &params).unwrap();
        let decoded = image::load_from_memory(&result).unwrap();
        assert_eq!(decoded.width(), 50);
        assert_eq!(decoded.height(), 25);
    }

    #[test]
    fn test_transform_max_dimension_clamping() {
        let png = create_test_png(10, 10);
        let params = TransformParams {
            w: Some(10000),
            h: Some(10000),
            fit: "fill".to_string(),
            format: Some("png".to_string()),
            ..Default::default()
        };
        let (result, _) = transform_image(&png, &params).unwrap();
        let decoded = image::load_from_memory(&result).unwrap();
        assert_eq!(decoded.width(), MAX_DIMENSION);
        assert_eq!(decoded.height(), MAX_DIMENSION);
    }

    #[test]
    fn test_invalid_image_data_errors() {
        let garbage = b"not an image at all";
        let params = TransformParams {
            w: Some(10),
            ..Default::default()
        };
        let result = transform_image(garbage, &params);
        assert!(result.is_err());
    }

    #[test]
    fn test_deserialize_params_from_query_string() {
        let params: TransformParams =
            serde_json::from_str(r#"{"w":200,"h":100,"fit":"contain","q":90,"format":"webp"}"#)
                .unwrap();
        assert_eq!(params.w, Some(200));
        assert_eq!(params.h, Some(100));
        assert_eq!(params.fit, "contain");
        assert_eq!(params.q, Some(90));
        assert_eq!(params.format.as_deref(), Some("webp"));
    }

    #[test]
    fn test_deserialize_params_defaults() {
        let params: TransformParams = serde_json::from_str(r#"{}"#).unwrap();
        assert!(params.w.is_none());
        assert!(params.h.is_none());
        assert_eq!(params.fit, "cover");
        assert!(params.q.is_none());
        assert!(params.format.is_none());
    }
}
