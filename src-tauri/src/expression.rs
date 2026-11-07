//! A face's eyes and smile: whether the eyes are closed, whether the face smiles.
//!
//! Both come from Google's MediaPipe Face Landmarker, run as ONNX (tf2onnx conversions of the
//! `face_landmarker.task` models, Apache-2.0). A face mesh model places 478 landmarks on a face
//! cropped around it, and a blendshapes model turns 146 of them into 52 scores, among them how
//! closed each eye is and how much each side of the mouth smiles.
//!
//! This file starts with the geometry both models need and nothing else does: the square, levelled
//! crop a face is shown to the mesh model in, and the way back from the crop to the picture.

// The model run (added next to this geometry) is what uses these functions; until it lands only the
// tests do.
#![cfg_attr(not(test), allow(dead_code))]

use crate::faces::bilinear;
use image::RgbImage;

/// A face found in a picture, in pixels of that picture. `kps` are InsightFace's five points in
/// its order: the eye on the image's left (x, y), the eye on its right, the nose, the mouth's left
/// corner, the mouth's right corner. Same as faces.rs's detections.
pub struct Face {
    pub bbox: [f32; 4],
    pub kps: [f32; 10],
}

/// Faces narrower than this (the box's width, in pixels of the picture read) are not read: their
/// eyes are a few pixels tall, too few to tell open from closed.
pub const MIN_FACE_PX: f32 = 64.0;

/// The mesh model's input is a square of this many pixels.
const CROP: usize = 256;

/// MediaPipe's scale from a face box to the square it crops: a box holds the face, the crop adds
/// the forehead, chin and ears the mesh covers.
const ROI_SCALE: f32 = 1.5;

/// The square a face is read in: its centre and side in picture pixels, and the angle of the line
/// between the eyes in the picture (radians, y down). The crop is turned by that angle so the eyes
/// come out level.
struct Roi {
    cx: f32,
    cy: f32,
    side: f32,
    angle: f32,
}

fn roi(face: &Face) -> Roi {
    let [x1, y1, x2, y2] = face.bbox;
    let k = &face.kps;
    Roi {
        cx: (x1 + x2) / 2.0,
        cy: (y1 + y2) / 2.0,
        side: ROI_SCALE * (x2 - x1).max(y2 - y1),
        angle: (k[3] - k[1]).atan2(k[2] - k[0]),
    }
}

/// Where a point of the crop, in the model's 0..256 pixels, is in the picture.
fn to_picture(roi: &Roi, x: f32, y: f32) -> (f32, f32) {
    let (dx, dy) = ((x / CROP as f32 - 0.5) * roi.side, (y / CROP as f32 - 0.5) * roi.side);
    let (sin, cos) = roi.angle.sin_cos();
    (roi.cx + cos * dx - sin * dy, roi.cy + sin * dx + cos * dy)
}

/// The face's square, levelled, as the mesh model takes it: 256 x 256 RGB, 0..1, row by row
/// (NHWC). Each crop pixel is read at its centre from the picture, blended from its four
/// neighbours; where the square reaches outside the picture it is black.
fn crop(img: &RgbImage, roi: &Roi) -> Vec<f32> {
    let mut out = Vec::with_capacity(CROP * CROP * 3);
    for v in 0..CROP {
        for u in 0..CROP {
            let (x, y) = to_picture(roi, u as f32 + 0.5, v as f32 + 0.5);
            out.extend(bilinear(img, x, y).map(|c| c / 255.0));
        }
    }
    out
}

/// The 146 of the mesh's 478 landmarks the blendshapes model takes, in its order (MediaPipe's
/// face_blendshapes_graph).
const SUBSET: [usize; 146] = [
    0, 1, 4, 5, 6, 7, 8, 10, 13, 14, 17, 21, 33, 37, 39, 40, 46, 52, 53, 54, 55, 58, 61, 63, 65, 66, 67, 70, 78, 80,
    81, 82, 84, 87, 88, 91, 93, 95, 103, 105, 107, 109, 127, 132, 133, 136, 144, 145, 146, 148, 149, 150, 152, 153,
    154, 155, 157, 158, 159, 160, 161, 162, 163, 168, 172, 173, 176, 178, 181, 185, 191, 195, 197, 234, 246, 249, 251,
    263, 267, 269, 270, 276, 282, 283, 284, 285, 288, 291, 293, 295, 296, 297, 300, 308, 310, 311, 312, 314, 317, 318,
    321, 323, 324, 332, 334, 336, 338, 356, 361, 362, 365, 373, 374, 375, 377, 378, 379, 380, 381, 382, 384, 385, 386,
    387, 388, 389, 390, 397, 398, 400, 402, 405, 409, 415, 454, 466, 468, 469, 470, 471, 472, 473, 474, 475, 476, 477,
];

#[cfg(test)]
mod tests {
    use super::*;
    use image::Rgb;
    use std::f32::consts::{FRAC_PI_6, PI};

    /// A face box of the given size at (100, 100) with the eyes `d` apart on a line tilted by `tilt`.
    fn face(w: f32, h: f32, tilt: f32) -> Face {
        let (left, d) = ((120.0, 130.0), 40.0);
        let right = (left.0 + d * tilt.cos(), left.1 + d * tilt.sin());
        Face {
            bbox: [100.0, 100.0, 100.0 + w, 100.0 + h],
            kps: [left.0, left.1, right.0, right.1, 150.0, 160.0, 130.0, 190.0, 170.0, 190.0],
        }
    }

    /// Where a picture point falls in the crop (the inverse of `to_picture`).
    fn to_crop(roi: &Roi, x: f32, y: f32) -> (f32, f32) {
        let (dx, dy) = (x - roi.cx, y - roi.cy);
        let (sin, cos) = roi.angle.sin_cos();
        let (u, v) = (cos * dx + sin * dy, -sin * dx + cos * dy);
        ((u / roi.side + 0.5) * CROP as f32, (v / roi.side + 0.5) * CROP as f32)
    }

    #[test]
    fn roi_is_level_square_and_one_and_a_half_faces() {
        let f = face(80.0, 100.0, FRAC_PI_6);
        let r = roi(&f);
        assert_eq!((r.cx, r.cy), (140.0, 150.0));
        assert_eq!(r.side, 150.0);
        assert!((r.angle - FRAC_PI_6).abs() < 1e-5);
        // Both eyes land at the same height in the crop: it is level.
        let (_, ly) = to_crop(&r, f.kps[0], f.kps[1]);
        let (_, ry) = to_crop(&r, f.kps[2], f.kps[3]);
        assert!((ly - ry).abs() < 1e-3, "{ly} vs {ry}");
        // A wider-than-tall box sizes the square by its width.
        assert_eq!(roi(&face(120.0, 90.0, 0.0)).side, 180.0);
    }

    #[test]
    fn back_projection_inverts_the_crop() {
        let r = Roi { cx: 140.0, cy: 150.0, side: 150.0, angle: PI / 6.0 };
        let (x, y) = to_picture(&r, 128.0, 128.0);
        assert!((x - 140.0).abs() < 1e-4 && (y - 150.0).abs() < 1e-4);
        // The crop's top-left corner is half a side to the left and above the centre, turned by the angle.
        let (x, y) = to_picture(&r, 0.0, 0.0);
        let (sin, cos) = (PI / 6.0).sin_cos();
        assert!((x - (140.0 - 75.0 * cos + 75.0 * sin)).abs() < 1e-3);
        assert!((y - (150.0 - 75.0 * sin - 75.0 * cos)).abs() < 1e-3);
        // Going there and back lands where it started.
        let (u, v) = to_crop(&r, x, y);
        assert!(u.abs() < 1e-3 && v.abs() < 1e-3);
    }

    #[test]
    fn crop_is_the_picture_where_it_has_one_and_black_outside() {
        let img = RgbImage::from_pixel(200, 200, Rgb([255, 102, 51]));
        // A face at the picture's top-left corner: most of its square is outside.
        let f =
            Face { bbox: [0.0, 0.0, 60.0, 60.0], kps: [15.0, 20.0, 45.0, 20.0, 30.0, 30.0, 20.0, 45.0, 40.0, 45.0] };
        let r = roi(&f);
        let px = crop(&img, &r);
        assert_eq!(px.len(), CROP * CROP * 3);
        let at = |u: usize, v: usize| &px[(v * CROP + u) * 3..][..3];
        for (got, want) in at(128, 128).iter().zip([1.0, 0.4, 0.2]) {
            assert!((got - want).abs() < 1e-4, "{got} vs {want}");
        }
        // The square's top-left corner is at (-45, -45) in the picture.
        assert_eq!(at(0, 0), [0.0; 3]);
    }

    #[test]
    fn subset_has_146_points_all_below_478() {
        assert_eq!(SUBSET.len(), 146);
        assert!(SUBSET.iter().all(|&i| i < 478));
        assert!(SUBSET.windows(2).all(|w| w[0] < w[1]), "ascending, so no point twice");
    }
}
