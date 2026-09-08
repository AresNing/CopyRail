use std::io::Cursor;

use image::{ImageFormat, ImageReader, Limits};
use paste_domain::{CapturedRepresentation, RepresentationKind};
use thiserror::Error;

const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SOURCE_DIMENSION: u32 = 16_384;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct GeneratedThumbnail {
    pub media_type: String,
    pub bytes: Vec<u8>,
    pub pixel_width: u32,
    pub pixel_height: u32,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum ImageRotation {
    Clockwise,
    CounterClockwise,
}

#[derive(Debug, Error)]
pub enum PreviewError {
    #[error("clipboard item does not contain a supported image representation")]
    Unsupported,
    #[error("image representation exceeds the preview safety limit")]
    SourceTooLarge,
    #[error("preview dimensions must be greater than zero")]
    InvalidDimensions,
    #[error(transparent)]
    Image(#[from] image::ImageError),
}

pub fn generate_image_thumbnail(
    representations: &[CapturedRepresentation],
    max_width: u32,
    max_height: u32,
) -> Result<GeneratedThumbnail, PreviewError> {
    if max_width == 0 || max_height == 0 {
        return Err(PreviewError::InvalidDimensions);
    }
    let (representation, format) = representations
        .iter()
        .find_map(|representation| {
            let format = match representation.kind {
                RepresentationKind::Png => ImageFormat::Png,
                RepresentationKind::Tiff => ImageFormat::Tiff,
                _ => return None,
            };
            Some((representation, format))
        })
        .ok_or(PreviewError::Unsupported)?;
    if representation.bytes.len() > MAX_SOURCE_BYTES {
        return Err(PreviewError::SourceTooLarge);
    }

    let mut reader = ImageReader::with_format(Cursor::new(&representation.bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    let thumbnail = reader.decode()?.thumbnail(max_width, max_height);
    let mut encoded = Cursor::new(Vec::new());
    thumbnail.write_to(&mut encoded, ImageFormat::Png)?;

    Ok(GeneratedThumbnail {
        media_type: "image/png".into(),
        bytes: encoded.into_inner(),
        pixel_width: thumbnail.width(),
        pixel_height: thumbnail.height(),
    })
}

pub fn rotate_image(
    representations: &[CapturedRepresentation],
    rotation: ImageRotation,
) -> Result<GeneratedThumbnail, PreviewError> {
    let (representation, format) = supported_image_representation(representations)?;
    let image = decode_image(representation, format)?;
    let rotated = match rotation {
        ImageRotation::Clockwise => image.rotate90(),
        ImageRotation::CounterClockwise => image.rotate270(),
    };
    let mut encoded = Cursor::new(Vec::new());
    rotated.write_to(&mut encoded, ImageFormat::Png)?;
    Ok(GeneratedThumbnail {
        media_type: "image/png".into(),
        bytes: encoded.into_inner(),
        pixel_width: rotated.width(),
        pixel_height: rotated.height(),
    })
}

fn supported_image_representation(
    representations: &[CapturedRepresentation],
) -> Result<(&CapturedRepresentation, ImageFormat), PreviewError> {
    representations
        .iter()
        .find_map(|representation| {
            let format = match representation.kind {
                RepresentationKind::Png => ImageFormat::Png,
                RepresentationKind::Tiff => ImageFormat::Tiff,
                _ => return None,
            };
            Some((representation, format))
        })
        .ok_or(PreviewError::Unsupported)
}

fn decode_image(
    representation: &CapturedRepresentation,
    format: ImageFormat,
) -> Result<image::DynamicImage, PreviewError> {
    if representation.bytes.len() > MAX_SOURCE_BYTES {
        return Err(PreviewError::SourceTooLarge);
    }
    let mut reader = ImageReader::with_format(Cursor::new(&representation.bytes), format);
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    reader.decode().map_err(Into::into)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn creates_a_bounded_png_thumbnail() {
        let image = image::DynamicImage::new_rgb8(8, 4);
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode source image");
        let representation = CapturedRepresentation {
            native_type: None,
            kind: RepresentationKind::Png,
            mime_type: Some("image/png".into()),
            file_name: None,
            bytes: encoded.into_inner(),
        };

        let thumbnail =
            generate_image_thumbnail(&[representation], 4, 4).expect("generate image thumbnail");
        assert_eq!(thumbnail.media_type, "image/png");
        assert_eq!((thumbnail.pixel_width, thumbnail.pixel_height), (4, 2));
        assert!(thumbnail.bytes.starts_with(b"\x89PNG"));
    }

    #[test]
    fn rejects_zero_sized_preview_requests() {
        assert!(matches!(
            generate_image_thumbnail(&[], 0, 100),
            Err(PreviewError::InvalidDimensions)
        ));
    }

    #[test]
    fn rotates_an_image_without_changing_its_encoded_safety_format() {
        let image = image::DynamicImage::new_rgb8(7, 3);
        let mut encoded = Cursor::new(Vec::new());
        image
            .write_to(&mut encoded, ImageFormat::Png)
            .expect("encode source image");
        let representation = CapturedRepresentation {
            native_type: None,
            kind: RepresentationKind::Png,
            mime_type: Some("image/png".into()),
            file_name: None,
            bytes: encoded.into_inner(),
        };

        let rotated =
            rotate_image(&[representation], ImageRotation::Clockwise).expect("rotate source image");
        assert_eq!((rotated.pixel_width, rotated.pixel_height), (3, 7));
        assert!(rotated.bytes.starts_with(b"\x89PNG"));
    }
}
