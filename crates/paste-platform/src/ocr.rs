use std::io::Cursor;

use image::{ImageReader, Limits};
use objc2::{AnyThread, rc::Retained, runtime::AnyObject};
use objc2_foundation::{NSArray, NSData, NSDictionary};
use objc2_vision::{
    VNImageOption, VNImageRequestHandler, VNRecognizeTextRequest, VNRequest,
    VNRequestTextRecognitionLevel,
};
use thiserror::Error;

const MAX_SOURCE_BYTES: usize = 64 * 1024 * 1024;
const MAX_SOURCE_DIMENSION: u32 = 16_384;
const MAX_DECODED_BYTES: u64 = 256 * 1024 * 1024;
const MAX_OCR_TEXT_BYTES: usize = 4 * 1024 * 1024;

#[derive(Clone, Debug, Eq, PartialEq)]
pub struct RecognizedText {
    pub text: String,
    pub line_count: usize,
}

#[derive(Debug, Error)]
pub enum OcrError {
    #[error("image representation exceeds the OCR safety limit")]
    SourceTooLarge,
    #[error("image data cannot be decoded for OCR")]
    InvalidImage,
    #[error("Vision text recognition failed: {0}")]
    Vision(String),
    #[error("no text was recognized in the image")]
    NoText,
}

pub fn recognize_text(image_bytes: &[u8]) -> Result<RecognizedText, OcrError> {
    validate_image(image_bytes)?;

    let data = NSData::with_bytes(image_bytes);
    let options = NSDictionary::<VNImageOption, AnyObject>::new();
    let handler = VNImageRequestHandler::initWithData_options(
        VNImageRequestHandler::alloc(),
        &data,
        &options,
    );
    let request = VNRecognizeTextRequest::new();
    request.setRecognitionLevel(VNRequestTextRecognitionLevel::Accurate);
    request.setUsesLanguageCorrection(true);
    request.setAutomaticallyDetectsLanguage(true);

    let base_request: Retained<VNRequest> = request.clone().into_super().into_super();
    let requests = NSArray::from_retained_slice(&[base_request]);
    handler
        .performRequests_error(&requests)
        .map_err(|error| OcrError::Vision(error.localizedDescription().to_string()))?;

    let mut lines = Vec::new();
    let mut total_bytes = 0_usize;
    for observation in request
        .results()
        .into_iter()
        .flat_map(|items| items.to_vec())
    {
        let Some(candidate) = observation.topCandidates(1).firstObject() else {
            continue;
        };
        let line = candidate.string().to_string();
        let line = line.trim();
        if line.is_empty() {
            continue;
        }
        let added_bytes = line.len() + usize::from(!lines.is_empty());
        if total_bytes.saturating_add(added_bytes) > MAX_OCR_TEXT_BYTES {
            break;
        }
        total_bytes += added_bytes;
        lines.push(line.to_owned());
    }
    if lines.is_empty() {
        return Err(OcrError::NoText);
    }
    Ok(RecognizedText {
        line_count: lines.len(),
        text: lines.join("\n"),
    })
}

fn validate_image(image_bytes: &[u8]) -> Result<(), OcrError> {
    if image_bytes.is_empty() || image_bytes.len() > MAX_SOURCE_BYTES {
        return Err(OcrError::SourceTooLarge);
    }
    let mut reader = ImageReader::new(Cursor::new(image_bytes))
        .with_guessed_format()
        .map_err(|_| OcrError::InvalidImage)?;
    let mut limits = Limits::default();
    limits.max_image_width = Some(MAX_SOURCE_DIMENSION);
    limits.max_image_height = Some(MAX_SOURCE_DIMENSION);
    limits.max_alloc = Some(MAX_DECODED_BYTES);
    reader.limits(limits);
    reader.decode().map_err(|_| OcrError::InvalidImage)?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn rejects_non_image_bytes_before_calling_vision() {
        assert!(matches!(
            recognize_text(b"not an image"),
            Err(OcrError::InvalidImage)
        ));
    }
}
