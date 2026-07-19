use a3s_use_core::{UseError, UseResult};
use clipper2::{Centi, EndType, JoinType};
use image::{GrayImage, Luma};
use imageproc::contours::find_contours;
use imageproc::geometry::{contour_area, min_area_rect};
use imageproc::point::Point;

use crate::config::{DetectionConfig, RecognitionConfig};

#[derive(Debug, Clone)]
pub(crate) struct Detection {
    pub(crate) polygon: [Point<f32>; 4],
    pub(crate) confidence: f32,
}

#[derive(Debug, Clone, PartialEq)]
pub(crate) struct Recognition {
    pub(crate) text: String,
    pub(crate) confidence: f32,
}

pub(crate) fn detection_boxes(
    output: &[f32],
    shape: &[usize],
    original_width: u32,
    original_height: u32,
    config: &DetectionConfig,
) -> UseResult<Vec<Detection>> {
    if shape.len() != 4 || shape[0] != 1 || shape[1] != 1 {
        return Err(output_error(format!(
            "PP-OCRv6 detection output shape must be [1, 1, H, W], found {shape:?}."
        )));
    }
    let height = shape[2];
    let width = shape[3];
    let map_len = height
        .checked_mul(width)
        .ok_or_else(|| output_error("PP-OCRv6 detection output dimensions overflowed."))?;
    if width == 0 || height == 0 || output.len() != map_len {
        return Err(output_error(
            "PP-OCRv6 detection output length does not match its shape.",
        ));
    }
    let width_u32 = u32::try_from(width)
        .map_err(|_| output_error("PP-OCRv6 detection output width is too large."))?;
    let height_u32 = u32::try_from(height)
        .map_err(|_| output_error("PP-OCRv6 detection output height is too large."))?;
    let mask = GrayImage::from_fn(width_u32, height_u32, |x, y| {
        let index = y as usize * width + x as usize;
        Luma([if output[index] > config.threshold {
            255
        } else {
            0
        }])
    });

    let mut detections = Vec::new();
    for contour in find_contours::<i32>(&mask)
        .into_iter()
        .take(config.max_candidates)
    {
        if contour.points.len() < 3 {
            continue;
        }
        let mini = order_points(min_area_rect(&contour.points));
        if minimum_side(&mini) < 3.0 {
            continue;
        }
        let score = box_score(output, width, height, &mini);
        if score < config.box_threshold {
            continue;
        }
        let area = contour_area(&mini);
        let perimeter = polygon_perimeter(&mini);
        if !area.is_finite() || !perimeter.is_finite() || perimeter <= f64::EPSILON {
            continue;
        }
        let distance = area * f64::from(config.unclip_ratio) / perimeter;
        let path = mini
            .iter()
            .map(|point| (f64::from(point.x), f64::from(point.y)))
            .collect::<Vec<_>>();
        let inflated: Vec<Vec<(f64, f64)>> =
            clipper2::inflate::<Centi>(path, distance, JoinType::Round, EndType::Polygon, 2.0)
                .into();
        if inflated.len() != 1 || inflated[0].len() < 3 {
            continue;
        }
        let inflated = inflated[0]
            .iter()
            .filter(|(x, y)| x.is_finite() && y.is_finite())
            .map(|(x, y)| {
                Point::new(
                    x.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
                    y.round().clamp(f64::from(i32::MIN), f64::from(i32::MAX)) as i32,
                )
            })
            .collect::<Vec<_>>();
        if inflated.len() < 3 {
            continue;
        }
        let expanded = order_points(min_area_rect(&inflated));
        if minimum_side(&expanded) < 5.0 {
            continue;
        }
        let polygon = expanded.map(|point| {
            Point::new(
                (point.x as f32 / width as f32 * original_width as f32)
                    .round()
                    .clamp(0.0, original_width.saturating_sub(1) as f32),
                (point.y as f32 / height as f32 * original_height as f32)
                    .round()
                    .clamp(0.0, original_height.saturating_sub(1) as f32),
            )
        });
        detections.push(Detection {
            polygon,
            confidence: score.clamp(0.0, 1.0),
        });
    }
    sort_reading_order(&mut detections);
    Ok(detections)
}

pub(crate) fn decode_ctc(
    output: &[f32],
    shape: &[usize],
    config: &RecognitionConfig,
) -> UseResult<Recognition> {
    if shape.len() != 3 || shape[0] != 1 || shape[1] == 0 || shape[2] == 0 {
        return Err(output_error(format!(
            "PP-OCRv6 recognition output shape must be [1, T, C], found {shape:?}."
        )));
    }
    let timesteps = shape[1];
    let classes = shape[2];
    let expected_classes = config.characters.len() + 2;
    if classes != expected_classes {
        return Err(output_error(format!(
            "PP-OCRv6 recognition class count is {classes}, but the model dictionary requires {expected_classes}."
        )));
    }
    if output.len() != timesteps.saturating_mul(classes) {
        return Err(output_error(
            "PP-OCRv6 recognition output length does not match its shape.",
        ));
    }

    let mut text = String::new();
    let mut confidence = 0.0_f32;
    let mut selected = 0_usize;
    let mut previous = usize::MAX;
    for timestep in 0..timesteps {
        let row = &output[timestep * classes..(timestep + 1) * classes];
        let (index, score) = row
            .iter()
            .copied()
            .enumerate()
            .max_by(|left, right| left.1.total_cmp(&right.1))
            .ok_or_else(|| output_error("PP-OCRv6 recognition output row is empty."))?;
        if index != 0 && index != previous {
            if index == config.characters.len() + 1 {
                text.push(' ');
            } else if let Some(character) = config.characters.get(index - 1) {
                text.push_str(character);
            }
            confidence += score;
            selected += 1;
        }
        previous = index;
    }
    Ok(Recognition {
        text,
        confidence: if selected == 0 {
            0.0
        } else {
            (confidence / selected as f32).clamp(0.0, 1.0)
        },
    })
}

fn box_score(output: &[f32], width: usize, height: usize, polygon: &[Point<i32>; 4]) -> f32 {
    let min_x = polygon
        .iter()
        .map(|point| point.x)
        .min()
        .unwrap_or(0)
        .clamp(0, width.saturating_sub(1) as i32) as usize;
    let max_x = polygon
        .iter()
        .map(|point| point.x)
        .max()
        .unwrap_or(0)
        .clamp(0, width.saturating_sub(1) as i32) as usize;
    let min_y = polygon
        .iter()
        .map(|point| point.y)
        .min()
        .unwrap_or(0)
        .clamp(0, height.saturating_sub(1) as i32) as usize;
    let max_y = polygon
        .iter()
        .map(|point| point.y)
        .max()
        .unwrap_or(0)
        .clamp(0, height.saturating_sub(1) as i32) as usize;
    let polygon = polygon.map(|point| Point::new(point.x as f32, point.y as f32));
    let mut sum = 0.0_f32;
    let mut count = 0_usize;
    for y in min_y..=max_y {
        for x in min_x..=max_x {
            if point_in_convex_polygon(Point::new(x as f32 + 0.5, y as f32 + 0.5), &polygon) {
                sum += output[y * width + x];
                count += 1;
            }
        }
    }
    if count == 0 {
        0.0
    } else {
        sum / count as f32
    }
}

fn point_in_convex_polygon(point: Point<f32>, polygon: &[Point<f32>; 4]) -> bool {
    let mut sign = 0_i8;
    for index in 0..4 {
        let start = polygon[index];
        let end = polygon[(index + 1) % 4];
        let cross =
            (end.x - start.x) * (point.y - start.y) - (end.y - start.y) * (point.x - start.x);
        if cross.abs() <= f32::EPSILON {
            continue;
        }
        let current = if cross > 0.0 { 1 } else { -1 };
        if sign != 0 && sign != current {
            return false;
        }
        sign = current;
    }
    true
}

fn minimum_side(points: &[Point<i32>; 4]) -> f64 {
    (0..4)
        .map(|index| distance(points[index], points[(index + 1) % 4]))
        .fold(f64::INFINITY, f64::min)
}

fn polygon_perimeter(points: &[Point<i32>; 4]) -> f64 {
    (0..4)
        .map(|index| distance(points[index], points[(index + 1) % 4]))
        .sum()
}

fn distance(left: Point<i32>, right: Point<i32>) -> f64 {
    let x = f64::from(left.x - right.x);
    let y = f64::from(left.y - right.y);
    x.hypot(y)
}

fn order_points(mut points: [Point<i32>; 4]) -> [Point<i32>; 4] {
    points.sort_by(|left, right| left.x.cmp(&right.x).then(left.y.cmp(&right.y)));
    let (top_left, bottom_left) = if points[0].y <= points[1].y {
        (points[0], points[1])
    } else {
        (points[1], points[0])
    };
    let (top_right, bottom_right) = if points[2].y <= points[3].y {
        (points[2], points[3])
    } else {
        (points[3], points[2])
    };
    [top_left, top_right, bottom_right, bottom_left]
}

fn sort_reading_order(detections: &mut [Detection]) {
    detections.sort_by(|left, right| {
        left.polygon[0]
            .y
            .total_cmp(&right.polygon[0].y)
            .then_with(|| left.polygon[0].x.total_cmp(&right.polygon[0].x))
    });
    for index in 1..detections.len() {
        let mut cursor = index;
        while cursor > 0 {
            let current = detections[cursor].polygon[0];
            let previous = detections[cursor - 1].polygon[0];
            if (current.y - previous.y).abs() < 10.0 && current.x < previous.x {
                detections.swap(cursor, cursor - 1);
                cursor -= 1;
            } else {
                break;
            }
        }
    }
}

fn output_error(message: impl Into<String>) -> UseError {
    UseError::new("use.ocr.provider_output_invalid", message)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ctc_decoder_removes_blanks_and_repeated_classes() {
        let config = RecognitionConfig {
            channels: 3,
            height: 48,
            default_width: 320,
            characters: vec!["A".to_string(), "B".to_string()],
        };
        let output = [
            0.9, 0.1, 0.0, 0.0, // blank
            0.1, 0.8, 0.1, 0.0, // A
            0.9, 0.1, 0.0, 0.0, // blank
            0.1, 0.8, 0.1, 0.0, // A
            0.1, 0.1, 0.8, 0.0, // B
        ];
        let result = decode_ctc(&output, &[1, 5, 4], &config).unwrap();
        assert_eq!(result.text, "AAB");
        assert!((result.confidence - 0.8).abs() < f32::EPSILON);
    }
}
