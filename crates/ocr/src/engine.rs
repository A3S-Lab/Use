use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use image::{imageops, ImageBuffer, Rgb, RgbImage};
use imageproc::geometric_transformations::{warp_into, Interpolation, Projection};
use imageproc::point::Point;
use ort::session::builder::GraphOptimizationLevel;
use ort::session::Session;
use ort::value::TensorRef;

use crate::assets::ModelAssets;
use crate::config::{load_detection, load_recognition, DetectionConfig, RecognitionConfig};
use crate::postprocess::{decode_ctc, detection_boxes, Detection};
use crate::preprocess::{detection_input, recognition_input};

const RECOGNITION_BATCH_SIZE: usize = 8;
const MAX_CROP_PIXELS: u64 = 64 * 1024 * 1024;

#[derive(Debug, Clone)]
pub(crate) struct EngineBlock {
    pub(crate) polygon: [Point<f32>; 4],
    pub(crate) detection_confidence: f32,
    pub(crate) text: String,
    pub(crate) confidence: f32,
}

pub(crate) struct PpOcrV6Engine {
    detection: Session,
    recognition: Session,
    detection_config: DetectionConfig,
    recognition_config: RecognitionConfig,
}

impl PpOcrV6Engine {
    pub(crate) fn load(assets: &ModelAssets) -> UseResult<Self> {
        let detection_config = load_detection(&assets.detection_config)?;
        let recognition_config = load_recognition(&assets.recognition_config)?;
        let detection = load_session(&assets.detection_model, "detection")?;
        let recognition = load_session(&assets.recognition_model, "recognition")?;
        Ok(Self {
            detection,
            recognition,
            detection_config,
            recognition_config,
        })
    }

    pub(crate) fn extract(&mut self, image: &RgbImage) -> UseResult<Vec<EngineBlock>> {
        let input = detection_input(image, &self.detection_config)?;
        let (shape, output) =
            run_session(&mut self.detection, &input.data, input.shape, "detection")?;
        let detections = detection_boxes(
            &output,
            &shape,
            input.original_width,
            input.original_height,
            &self.detection_config,
        )?;
        if detections.is_empty() {
            return Ok(Vec::new());
        }

        let crops = detections
            .iter()
            .map(|detection| perspective_crop(image, detection))
            .collect::<UseResult<Vec<_>>>()?;
        let mut blocks = Vec::with_capacity(detections.len());
        for (detection_batch, crop_batch) in detections
            .chunks(RECOGNITION_BATCH_SIZE)
            .zip(crops.chunks(RECOGNITION_BATCH_SIZE))
        {
            let input = recognition_input(crop_batch, &self.recognition_config)?;
            let (shape, output) = run_session(
                &mut self.recognition,
                &input.data,
                input.shape,
                "recognition",
            )?;
            if shape.len() != 3 || shape[0] != detection_batch.len() {
                return Err(engine_error(
                    "use.ocr.provider_output_invalid",
                    format!(
                        "PP-OCRv6 recognition output shape must be [N, T, C] for N={}, found {shape:?}.",
                        detection_batch.len()
                    ),
                ));
            }
            let item_len = shape[1].checked_mul(shape[2]).ok_or_else(|| {
                engine_error(
                    "use.ocr.provider_output_invalid",
                    "PP-OCRv6 recognition output dimensions overflowed.",
                )
            })?;
            if output.len() != detection_batch.len().saturating_mul(item_len) {
                return Err(engine_error(
                    "use.ocr.provider_output_invalid",
                    "PP-OCRv6 recognition output length does not match its batch shape.",
                ));
            }
            for (index, detection) in detection_batch.iter().enumerate() {
                let start = index * item_len;
                let recognition = decode_ctc(
                    &output[start..start + item_len],
                    &[1, shape[1], shape[2]],
                    &self.recognition_config,
                )?;
                blocks.push(EngineBlock {
                    polygon: detection.polygon,
                    detection_confidence: detection.confidence,
                    text: recognition.text,
                    confidence: recognition.confidence,
                });
            }
        }
        Ok(blocks)
    }
}

fn load_session(path: &Path, role: &str) -> UseResult<Session> {
    let session = Session::builder()
        .map_err(|error| runtime_error(role, "create an ONNX Runtime session", error))?
        .with_optimization_level(GraphOptimizationLevel::Level3)
        .map_err(|error| runtime_error(role, "configure graph optimization", error))?
        .commit_from_file(path)
        .map_err(|error| runtime_error(role, "load the ONNX model", error))?;
    if session.inputs.len() != 1 || session.outputs.len() != 1 {
        return Err(engine_error(
            "use.ocr.model_invalid",
            format!(
                "PP-OCRv6 {role} model must expose exactly one input and one output; found {} inputs and {} outputs.",
                session.inputs.len(),
                session.outputs.len()
            ),
        ));
    }
    Ok(session)
}

fn run_session(
    session: &mut Session,
    data: &[f32],
    shape: [usize; 4],
    role: &str,
) -> UseResult<(Vec<usize>, Vec<f32>)> {
    let expected = shape
        .iter()
        .try_fold(1_usize, |total, dimension| total.checked_mul(*dimension));
    if expected != Some(data.len()) {
        return Err(engine_error(
            "use.ocr.provider_input_invalid",
            format!("PP-OCRv6 {role} tensor length does not match its shape."),
        ));
    }
    let input = TensorRef::from_array_view((shape, data))
        .map_err(|error| runtime_error(role, "create an ONNX Runtime input tensor", error))?;
    let outputs = session
        .run(ort::inputs![input])
        .map_err(|error| runtime_error(role, "run ONNX inference", error))?;
    if outputs.len() != 1 {
        return Err(engine_error(
            "use.ocr.provider_output_invalid",
            format!(
                "PP-OCRv6 {role} inference returned {} outputs instead of one.",
                outputs.len()
            ),
        ));
    }
    let output = outputs.values().next().ok_or_else(|| {
        engine_error(
            "use.ocr.provider_output_invalid",
            format!("PP-OCRv6 {role} inference returned no output tensor."),
        )
    })?;
    let (output_shape, output_data) = output
        .try_extract_tensor::<f32>()
        .map_err(|error| runtime_error(role, "read the ONNX output tensor", error))?;
    let output_shape = output_shape
        .iter()
        .map(|dimension| {
            usize::try_from(*dimension).map_err(|_| {
                engine_error(
                    "use.ocr.provider_output_invalid",
                    format!("PP-OCRv6 {role} output contains an invalid dimension {dimension}."),
                )
            })
        })
        .collect::<UseResult<Vec<_>>>()?;
    if output_data.iter().any(|value| !value.is_finite()) {
        return Err(engine_error(
            "use.ocr.provider_output_invalid",
            format!("PP-OCRv6 {role} output contains a non-finite value."),
        ));
    }
    Ok((output_shape, output_data.to_vec()))
}

fn perspective_crop(image: &RgbImage, detection: &Detection) -> UseResult<RgbImage> {
    let polygon = detection.polygon;
    let width = distance(polygon[0], polygon[1])
        .max(distance(polygon[2], polygon[3]))
        .round()
        .max(1.0) as u32;
    let height = distance(polygon[0], polygon[3])
        .max(distance(polygon[1], polygon[2]))
        .round()
        .max(1.0) as u32;
    let pixels = u64::from(width)
        .checked_mul(u64::from(height))
        .ok_or_else(|| crop_error("PP-OCRv6 text crop dimensions overflowed."))?;
    if pixels > MAX_CROP_PIXELS {
        return Err(crop_error(
            "PP-OCRv6 text crop exceeds the 64 megapixel safety limit.",
        ));
    }

    let source = polygon.map(|point| (point.x, point.y));
    let destination = [
        (0.0, 0.0),
        (width.saturating_sub(1) as f32, 0.0),
        (
            width.saturating_sub(1) as f32,
            height.saturating_sub(1) as f32,
        ),
        (0.0, height.saturating_sub(1) as f32),
    ];
    let projection = Projection::from_control_points(source, destination).ok_or_else(|| {
        crop_error("PP-OCRv6 detected a degenerate text polygon that cannot be rectified.")
    })?;
    let mut crop = ImageBuffer::new(width, height);
    warp_into(
        image,
        &projection,
        Interpolation::Bicubic,
        Rgb([255, 255, 255]),
        &mut crop,
    );
    if f64::from(height) / f64::from(width) >= 1.5 {
        Ok(imageops::rotate270(&crop))
    } else {
        Ok(crop)
    }
}

fn distance(left: Point<f32>, right: Point<f32>) -> f32 {
    (left.x - right.x).hypot(left.y - right.y)
}

fn runtime_error(role: &str, action: &str, error: impl std::fmt::Display) -> UseError {
    engine_error(
        "use.ocr.runtime_failed",
        format!("Failed to {action} for PP-OCRv6 {role}: {error}"),
    )
}

fn crop_error(message: impl Into<String>) -> UseError {
    engine_error("use.ocr.crop_invalid", message)
}

fn engine_error(code: &str, message: impl Into<String>) -> UseError {
    UseError::new(code, message)
}
