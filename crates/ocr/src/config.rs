use std::collections::BTreeMap;
use std::path::Path;

use a3s_use_core::{UseError, UseResult};
use serde::Deserialize;
use serde_yaml::Value;

pub(crate) const MODEL_FAMILY: &str = "PP-OCRv6_small";
pub(crate) const DETECTION_MODEL: &str = "PP-OCRv6_small_det";
pub(crate) const RECOGNITION_MODEL: &str = "PP-OCRv6_small_rec";

#[derive(Debug, Clone)]
pub(crate) struct DetectionConfig {
    pub(crate) scale: f32,
    pub(crate) mean: [f32; 3],
    pub(crate) std: [f32; 3],
    pub(crate) threshold: f32,
    pub(crate) box_threshold: f32,
    pub(crate) max_candidates: usize,
    pub(crate) unclip_ratio: f32,
}

#[derive(Debug, Clone)]
pub(crate) struct RecognitionConfig {
    pub(crate) channels: usize,
    pub(crate) height: usize,
    pub(crate) default_width: usize,
    pub(crate) characters: Vec<String>,
}

#[derive(Debug, Deserialize)]
struct RawConfig {
    #[serde(rename = "Global")]
    global: RawGlobal,
    #[serde(rename = "PreProcess")]
    pre_process: RawPreProcess,
    #[serde(rename = "PostProcess")]
    post_process: RawPostProcess,
}

#[derive(Debug, Deserialize)]
struct RawGlobal {
    model_name: String,
}

#[derive(Debug, Deserialize)]
struct RawPreProcess {
    transform_ops: Vec<BTreeMap<String, Value>>,
}

#[derive(Debug, Deserialize)]
struct RawPostProcess {
    #[serde(default)]
    name: String,
    #[serde(default = "default_threshold")]
    thresh: f32,
    #[serde(default = "default_box_threshold")]
    box_thresh: f32,
    #[serde(default = "default_max_candidates")]
    max_candidates: usize,
    #[serde(default = "default_unclip_ratio")]
    unclip_ratio: f32,
    #[serde(default)]
    character_dict: Vec<String>,
}

pub(crate) fn load_detection(path: &Path) -> UseResult<DetectionConfig> {
    let raw = load(path)?;
    if raw.global.model_name != DETECTION_MODEL {
        return Err(config_error(format!(
            "Expected detection model '{DETECTION_MODEL}', found '{}'.",
            raw.global.model_name
        )));
    }
    if raw.post_process.name != "DBPostProcess" {
        return Err(config_error(format!(
            "Expected DBPostProcess, found '{}'.",
            raw.post_process.name
        )));
    }
    let normalize = transform(&raw.pre_process.transform_ops, "NormalizeImage")
        .ok_or_else(|| config_error("Detection config has no NormalizeImage transform."))?;
    let scale = normalize
        .get("scale")
        .and_then(parse_scale)
        .unwrap_or(1.0 / 255.0);
    let mean = float_triplet(normalize.get("mean"), [0.485, 0.456, 0.406])?;
    let std = float_triplet(normalize.get("std"), [0.229, 0.224, 0.225])?;
    if std.iter().any(|value| *value <= 0.0) {
        return Err(config_error(
            "Detection normalization standard deviations must be positive.",
        ));
    }
    Ok(DetectionConfig {
        scale,
        mean,
        std,
        threshold: raw.post_process.thresh,
        box_threshold: raw.post_process.box_thresh,
        max_candidates: raw.post_process.max_candidates.min(10_000),
        unclip_ratio: raw.post_process.unclip_ratio,
    })
}

pub(crate) fn load_recognition(path: &Path) -> UseResult<RecognitionConfig> {
    let raw = load(path)?;
    if raw.global.model_name != RECOGNITION_MODEL {
        return Err(config_error(format!(
            "Expected recognition model '{RECOGNITION_MODEL}', found '{}'.",
            raw.global.model_name
        )));
    }
    if raw.post_process.name != "CTCLabelDecode" {
        return Err(config_error(format!(
            "Expected CTCLabelDecode, found '{}'.",
            raw.post_process.name
        )));
    }
    if raw.post_process.character_dict.is_empty() || raw.post_process.character_dict.len() > 100_000
    {
        return Err(config_error(
            "Recognition character dictionary is empty or unreasonably large.",
        ));
    }
    let resize = transform(&raw.pre_process.transform_ops, "RecResizeImg")
        .ok_or_else(|| config_error("Recognition config has no RecResizeImg transform."))?;
    let shape = resize
        .get("image_shape")
        .and_then(Value::as_sequence)
        .ok_or_else(|| config_error("RecResizeImg.image_shape must be an integer triplet."))?;
    if shape.len() != 3 {
        return Err(config_error(
            "RecResizeImg.image_shape must contain channels, height, and width.",
        ));
    }
    let dimensions = shape
        .iter()
        .map(|value| value.as_u64().and_then(|value| usize::try_from(value).ok()))
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| config_error("RecResizeImg.image_shape contains an invalid dimension."))?;
    if dimensions[0] != 3
        || !(16..=256).contains(&dimensions[1])
        || !(32..=4096).contains(&dimensions[2])
    {
        return Err(config_error(format!(
            "Unsupported PP-OCRv6 recognition input shape {:?}.",
            dimensions
        )));
    }
    Ok(RecognitionConfig {
        channels: dimensions[0],
        height: dimensions[1],
        default_width: dimensions[2],
        characters: raw.post_process.character_dict,
    })
}

fn load(path: &Path) -> UseResult<RawConfig> {
    let metadata = std::fs::metadata(path).map_err(|error| {
        config_error(format!(
            "Failed to inspect PP-OCRv6 config '{}': {error}",
            path.display()
        ))
    })?;
    if !metadata.is_file() || metadata.len() == 0 || metadata.len() > 2 * 1024 * 1024 {
        return Err(config_error(format!(
            "PP-OCRv6 config '{}' must be a non-empty regular file no larger than 2 MiB.",
            path.display()
        )));
    }
    let text = std::fs::read_to_string(path).map_err(|error| {
        config_error(format!(
            "Failed to read PP-OCRv6 config '{}': {error}",
            path.display()
        ))
    })?;
    serde_yaml::from_str(&text).map_err(|error| {
        config_error(format!(
            "Failed to parse PP-OCRv6 config '{}': {error}",
            path.display()
        ))
    })
}

fn transform<'a>(
    transforms: &'a [BTreeMap<String, Value>],
    name: &str,
) -> Option<&'a serde_yaml::Mapping> {
    transforms
        .iter()
        .find_map(|transform| transform.get(name))
        .and_then(Value::as_mapping)
}

fn float_triplet(value: Option<&Value>, default: [f32; 3]) -> UseResult<[f32; 3]> {
    let Some(values) = value.and_then(Value::as_sequence) else {
        return Ok(default);
    };
    if values.len() != 3 {
        return Err(config_error(
            "Detection normalization mean and std must contain three values.",
        ));
    }
    let mut output = [0.0_f32; 3];
    for (index, value) in values.iter().enumerate() {
        output[index] = yaml_f32(value)
            .ok_or_else(|| config_error("Detection normalization contains a non-number."))?;
    }
    Ok(output)
}

fn parse_scale(value: &Value) -> Option<f32> {
    if let Some(value) = yaml_f32(value) {
        return Some(value);
    }
    let value = value.as_str()?.trim();
    if let Some((numerator, denominator)) = value.split_once('/') {
        let numerator = numerator.trim_matches('.').parse::<f32>().ok()?;
        let denominator = denominator.trim_matches('.').parse::<f32>().ok()?;
        return (denominator != 0.0).then_some(numerator / denominator);
    }
    value.parse().ok()
}

fn yaml_f32(value: &Value) -> Option<f32> {
    value
        .as_f64()
        .map(|value| value as f32)
        .filter(|value| value.is_finite())
}

fn default_threshold() -> f32 {
    0.3
}

fn default_box_threshold() -> f32 {
    0.6
}

fn default_max_candidates() -> usize {
    1_000
}

fn default_unclip_ratio() -> f32 {
    1.5
}

fn config_error(message: impl Into<String>) -> UseError {
    UseError::new("use.ocr.model_config_invalid", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn parses_fractional_detection_scale() {
        let value = Value::String("1./255.".to_string());
        assert!((parse_scale(&value).unwrap() - 1.0 / 255.0).abs() < f32::EPSILON);
    }
}
