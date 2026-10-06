//! Files a browser cannot show as they are, shown as something it can.
//!
//! Browsers decode JPEG, PNG, GIF, WebP, AVIF, BMP, ICO and SVG themselves.
//! TIFF -- what scanners and many cameras write -- TGA, the PNM family and QOI
//! they do not; those are decoded here and sent on as PNG when the preview asks
//! (`?as=png`). Everything else is served untouched.
use crate::{ApiError, Result};
use axum::{
    body::Body,
    extract::Request,
    http::{HeaderValue, StatusCode, header},
    response::{IntoResponse, Response},
};
use std::path::{Path, PathBuf};
use tower::ServiceExt;
use tower_http::services::ServeFile;

/// Formats decoded here for a preview, by extension.
const CONVERTIBLE: &[&str] = &["tif", "tiff", "tga", "pnm", "pbm", "pgm", "ppm", "qoi"];
/// Larger source files are not decoded for a preview; download is offered instead.
const MAX_CONVERT_BYTES: u64 = 200 * 1024 * 1024;

pub fn convertible(path: &Path) -> bool {
    path.extension()
        .and_then(|extension| extension.to_str())
        .is_some_and(|extension| CONVERTIBLE.contains(&extension.to_ascii_lowercase().as_str()))
}

fn refused(status: StatusCode, message: String) -> ApiError {
    ApiError { status, message }
}

/// The image at `path` re-encoded as PNG.
pub fn to_png(path: &Path) -> Result<Vec<u8>> {
    let size = std::fs::metadata(path).map_err(ApiError::internal)?.len();
    if size > MAX_CONVERT_BYTES {
        return Err(refused(
            StatusCode::PAYLOAD_TOO_LARGE,
            "image is too large to convert for preview".into(),
        ));
    }
    let mut reader = image::ImageReader::open(path)
        .map_err(ApiError::internal)?
        .with_guessed_format()
        .map_err(ApiError::internal)?;
    // A small header can claim an enormous canvas; refuse that before allocating it.
    let mut limits = image::Limits::default();
    limits.max_alloc = Some(1024 * 1024 * 1024);
    reader.limits(limits);
    let decoded = reader.decode().map_err(|error| {
        refused(
            StatusCode::UNSUPPORTED_MEDIA_TYPE,
            format!("cannot decode this image: {error}"),
        )
    })?;
    let mut out = std::io::Cursor::new(Vec::new());
    decoded
        .write_to(&mut out, image::ImageFormat::Png)
        .map_err(ApiError::internal)?;
    Ok(out.into_inner())
}

/// Serves a file for preview: converted to PNG when asked and possible,
/// otherwise as it is (with ranges, so video and audio can seek). Either way
/// the response is sandboxed, so an SVG or HTML file runs nothing.
pub async fn serve(target: PathBuf, as_png: bool, request: Request) -> Result<Response> {
    let mut response = if as_png && convertible(&target) {
        let bytes = tokio::task::spawn_blocking(move || to_png(&target))
            .await
            .map_err(ApiError::internal)??;
        ([(header::CONTENT_TYPE, "image/png")], Body::from(bytes)).into_response()
    } else {
        ServeFile::new(target)
            .oneshot(request)
            .await
            .map_err(ApiError::internal)?
            .into_response()
    };
    response.headers_mut().insert(
        header::CONTENT_SECURITY_POLICY,
        HeaderValue::from_static("sandbox; default-src 'none'; style-src 'unsafe-inline'"),
    );
    Ok(response)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_tiff_becomes_a_png_and_other_formats_are_left_alone() {
        let dir = std::env::temp_dir().join(format!("agentdock-media-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let tiff = dir.join("scan.TIFF");
        image::RgbImage::from_pixel(3, 2, image::Rgb([200, 10, 10]))
            .save_with_format(&tiff, image::ImageFormat::Tiff)
            .unwrap();
        assert!(convertible(&tiff));
        assert!(!convertible(&dir.join("photo.jpg")));
        let png = to_png(&tiff).unwrap();
        let decoded = image::load_from_memory_with_format(&png, image::ImageFormat::Png).unwrap();
        assert_eq!((decoded.width(), decoded.height()), (3, 2));
        std::fs::write(dir.join("broken.tif"), b"not an image").unwrap();
        assert!(to_png(&dir.join("broken.tif")).is_err());
        std::fs::remove_dir_all(dir).ok();
    }
}
