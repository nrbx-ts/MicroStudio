use serde::{Deserialize, Serialize};
use std::ops::Mul;

use crate::Vector3;

// radians, XYZ application order
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct EulerAngles {
    pub x: f64,
    pub y: f64,
    pub z: f64,
}

impl EulerAngles {
    #[inline]
    pub const fn new(x: f64, y: f64, z: f64) -> Self {
        Self { x, y, z }
    }
}

// row-major rotation, laid out like CFrame:GetComponents()
// col0 right, col1 up, col2 backward (LookVector == -col2)
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct CFrame {
    position: Vector3,
    rotation: [f64; 9],
}

impl CFrame {
    #[inline]
    pub const fn identity() -> Self {
        Self {
            position: Vector3::zero(),
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[inline]
    pub const fn new(position: Vector3) -> Self {
        Self {
            position,
            rotation: [1.0, 0.0, 0.0, 0.0, 1.0, 0.0, 0.0, 0.0, 1.0],
        }
    }

    #[inline]
    pub const fn from_rotation_matrix(position: Vector3, rotation: [f64; 9]) -> Self {
        Self { position, rotation }
    }

    pub fn look_at(position: Vector3, target: Vector3, up: Vector3) -> Self {
        let forward = (target - position).unit();
        if forward.is_zero() {
            return Self::new(position);
        }

        let mut right = forward.cross(up);
        if right.magnitude_squared() < 1e-12 {
            // forward parallel to up: pick any perpendicular axis
            let fallback = if forward.x.abs() < 0.9 {
                Vector3::X_AXIS
            } else {
                Vector3::Z_AXIS
            };
            right = forward.cross(fallback);
        }
        let right = right.unit();
        let true_up = right.cross(forward);

        Self {
            position,
            rotation: [
                right.x,
                true_up.x,
                -forward.x,
                right.y,
                true_up.y,
                -forward.y,
                right.z,
                true_up.z,
                -forward.z,
            ],
        }
    }

    pub fn from_axis_angle(axis: Vector3, angle: f64) -> Self {
        let axis = axis.unit();
        let (s, c) = angle.sin_cos();
        let t = 1.0 - c;
        let Vector3 { x, y, z } = axis;
        Self {
            position: Vector3::zero(),
            rotation: [
                t * x * x + c,
                t * x * y - s * z,
                t * x * z + s * y,
                t * x * y + s * z,
                t * y * y + c,
                t * y * z - s * x,
                t * x * z - s * y,
                t * y * z + s * x,
                t * z * z + c,
            ],
        }
    }

    pub fn from_euler_angles(x: f64, y: f64, z: f64) -> Self {
        let (sx, cx) = x.sin_cos();
        let (sy, cy) = y.sin_cos();
        let (sz, cz) = z.sin_cos();
        Self {
            position: Vector3::zero(),
            rotation: [
                cy * cz,
                -cy * sz,
                sy,
                sx * sy * cz + cx * sz,
                -sx * sy * sz + cx * cz,
                -sx * cy,
                -cx * sy * cz + sx * sz,
                cx * sy * sz + sx * cz,
                cx * cy,
            ],
        }
    }

    pub fn from_basis(position: Vector3, right: Vector3, up: Vector3) -> Self {
        let back = right.cross(up);
        Self {
            position,
            rotation: [
                right.x,
                up.x,
                back.x,
                right.y,
                up.y,
                back.y,
                right.z,
                up.z,
                back.z,
            ],
        }
    }

    #[inline]
    pub const fn position(&self) -> Vector3 {
        self.position
    }

    #[inline]
    pub fn x(&self) -> f64 {
        self.position.x
    }

    #[inline]
    pub fn y(&self) -> f64 {
        self.position.y
    }

    #[inline]
    pub fn z(&self) -> f64 {
        self.position.z
    }

    #[inline]
    pub const fn rotation_components(&self) -> [f64; 9] {
        self.rotation
    }

    #[inline]
    pub fn right_vector(&self) -> Vector3 {
        Vector3::new(self.rotation[0], self.rotation[3], self.rotation[6])
    }

    #[inline]
    pub fn up_vector(&self) -> Vector3 {
        Vector3::new(self.rotation[1], self.rotation[4], self.rotation[7])
    }

    #[inline]
    pub fn look_vector(&self) -> Vector3 {
        Vector3::new(-self.rotation[2], -self.rotation[5], -self.rotation[8])
    }

    #[inline]
    pub fn inverse(&self) -> Self {
        let r = &self.rotation;
        // rotation transpose is its inverse
        let inv = [r[0], r[3], r[6], r[1], r[4], r[7], r[2], r[5], r[8]];
        let p = &self.position;
        let position = Vector3::new(
            -(inv[0] * p.x + inv[1] * p.y + inv[2] * p.z),
            -(inv[3] * p.x + inv[4] * p.y + inv[5] * p.z),
            -(inv[6] * p.x + inv[7] * p.y + inv[8] * p.z),
        );
        Self {
            position,
            rotation: inv,
        }
    }

    #[inline]
    pub fn point_to_world_space(&self, point: Vector3) -> Vector3 {
        self.vector_to_world_space(point) + self.position
    }

    #[inline]
    pub fn point_to_object_space(&self, point: Vector3) -> Vector3 {
        self.inverse().point_to_world_space(point)
    }

    #[inline]
    pub fn vector_to_world_space(&self, v: Vector3) -> Vector3 {
        let r = &self.rotation;
        Vector3::new(
            r[0] * v.x + r[1] * v.y + r[2] * v.z,
            r[3] * v.x + r[4] * v.y + r[5] * v.z,
            r[6] * v.x + r[7] * v.y + r[8] * v.z,
        )
    }

    #[inline]
    pub fn vector_to_object_space(&self, v: Vector3) -> Vector3 {
        let r = &self.rotation;
        Vector3::new(
            r[0] * v.x + r[3] * v.y + r[6] * v.z,
            r[1] * v.x + r[4] * v.y + r[7] * v.z,
            r[2] * v.x + r[5] * v.y + r[8] * v.z,
        )
    }

    #[inline]
    pub fn to_world_space(&self, other: CFrame) -> CFrame {
        *self * other
    }

    #[inline]
    pub fn to_object_space(&self, other: CFrame) -> CFrame {
        self.inverse() * other
    }

    #[inline]
    pub fn lerp(&self, other: CFrame, alpha: f64) -> CFrame {
        let position = self.position.lerp(&other.position, alpha);
        let mut rotation = [0.0; 9];
        for i in 0..9 {
            rotation[i] = self.rotation[i] + (other.rotation[i] - self.rotation[i]) * alpha;
        }
        CFrame {
            position,
            rotation,
        }
    }

    #[inline]
    pub fn fuzzy_eq(&self, other: &CFrame) -> bool {
        self.position.fuzzy_eq(&other.position)
            && self
                .rotation
                .iter()
                .zip(other.rotation.iter())
                .all(|(a, b)| crate::fuzzy_eq(*a, *b))
    }

    // approximate XYZ extraction
    pub fn to_euler_angles_xyz(&self) -> EulerAngles {
        let r = &self.rotation;
        let sy = r[2].clamp(-1.0, 1.0);
        let y = sy.asin();
        let (x, z) = if sy.abs() < 0.999_999 {
            ((-r[5]).atan2(r[8]), (-r[1]).atan2(r[0]))
        } else {
            (r[7].atan2(r[4]), 0.0)
        };
        EulerAngles { x, y, z }
    }

    #[inline]
    pub fn with_position(&self, position: Vector3) -> CFrame {
        CFrame {
            position,
            rotation: self.rotation,
        }
    }
}

impl Mul for CFrame {
    type Output = CFrame;

    #[inline]
    fn mul(self, rhs: CFrame) -> CFrame {
        let a = &self.rotation;
        let b = &rhs.rotation;
        let mut rotation = [0.0; 9];
        for row in 0..3 {
            for col in 0..3 {
                rotation[row * 3 + col] = a[row * 3] * b[col]
                    + a[row * 3 + 1] * b[3 + col]
                    + a[row * 3 + 2] * b[6 + col];
            }
        }
        CFrame {
            position: self.point_to_world_space(rhs.position),
            rotation,
        }
    }
}

impl Mul<Vector3> for CFrame {
    type Output = Vector3;

    #[inline]
    fn mul(self, rhs: Vector3) -> Vector3 {
        self.point_to_world_space(rhs)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::f64::consts::{FRAC_PI_2, PI};

    fn assert_close(a: Vector3, b: Vector3) {
        assert!(a.fuzzy_eq(&b), "expected {b:?}, got {a:?}");
    }

    #[test]
    fn identity_is_neutral() {
        let cf = CFrame::new(Vector3::new(1.0, 2.0, 3.0));
        assert_close(cf.point_to_world_space(Vector3::new(4.0, 5.0, 6.0)), Vector3::new(5.0, 7.0, 9.0));
        assert!(cf.look_vector().fuzzy_eq(&Vector3::new(0.0, 0.0, -1.0)));
    }

    #[test]
    fn look_at_points_down_negative_z() {
        let cf = CFrame::look_at(Vector3::zero(), Vector3::new(0.0, 0.0, -10.0), Vector3::Y_AXIS);
        assert!(cf.look_vector().fuzzy_eq(&Vector3::new(0.0, 0.0, -1.0)));
        assert!(cf.up_vector().fuzzy_eq(&Vector3::Y_AXIS));
        assert!(cf.right_vector().fuzzy_eq(&Vector3::X_AXIS));

        let cf = CFrame::look_at(Vector3::zero(), Vector3::new(10.0, 0.0, 0.0), Vector3::Y_AXIS);
        assert!(cf.look_vector().fuzzy_eq(&Vector3::X_AXIS));
    }

    #[test]
    fn look_at_is_degenerate_safe() {
        let cf = CFrame::look_at(Vector3::zero(), Vector3::Y_AXIS * 5.0, Vector3::Y_AXIS);
        assert!(cf.look_vector().fuzzy_eq(&Vector3::Y_AXIS));
    }

    #[test]
    fn yaw_90_rotates_x_onto_negative_z() {
        let cf = CFrame::from_euler_angles(0.0, FRAC_PI_2, 0.0);
        assert_close(cf.vector_to_world_space(Vector3::X_AXIS), Vector3::new(0.0, 0.0, -1.0));
        assert_close(cf.vector_to_world_space(Vector3::Y_AXIS), Vector3::Y_AXIS);
    }

    #[test]
    fn inverse_round_trips() {
        let cf = CFrame::look_at(
            Vector3::new(3.0, 4.0, 5.0),
            Vector3::new(-1.0, 0.0, 2.0),
            Vector3::Y_AXIS,
        );
        let point = Vector3::new(7.0, 8.0, 9.0);
        let world = cf.point_to_world_space(point);
        assert_close(cf.point_to_object_space(world), point);
        assert!(cf.inverse().fuzzy_eq(&cf.inverse()));
    }

    #[test]
    fn composition_matches_sequential_transforms() {
        let a = CFrame::from_euler_angles(0.3, 0.4, 0.5).with_position(Vector3::new(1.0, 0.0, 0.0));
        let b = CFrame::from_euler_angles(-0.2, 0.1, 0.7).with_position(Vector3::new(0.0, 2.0, 0.0));
        let point = Vector3::new(0.5, -0.5, 1.0);
        let combined = a * b;
        assert_close(combined.point_to_world_space(point), a.point_to_world_space(b.point_to_world_space(point)));
    }

    #[test]
    fn euler_round_trip() {
        let cf = CFrame::from_euler_angles(0.1, 0.2, 0.3);
        let e = cf.to_euler_angles_xyz();
        let rebuilt = CFrame::from_euler_angles(e.x, e.y, e.z);
        assert!(cf.fuzzy_eq(&rebuilt));
    }

    #[test]
    fn axis_angle_matches_euler_for_single_axis() {
        let a = CFrame::from_axis_angle(Vector3::Y_AXIS, PI);
        let b = CFrame::from_euler_angles(0.0, PI, 0.0);
        assert!(a.fuzzy_eq(&b));
    }
}
