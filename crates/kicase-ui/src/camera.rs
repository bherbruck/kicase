//! An orbit camera.
//!
//! Orthographic by default: for lining parts up, foreshortening is a liability
//! — two edges that are flush should *look* flush.

use kicase_geometry::types::{Bounds3, Point3};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Projection {
    Orthographic,
    Perspective,
}

/// The view the window opens on: the corner where top, front and right all
/// show, which is what every CAD package calls home.
pub const HOME_VIEW: [f32; 3] = [1.0, -1.0, 1.0];

#[derive(Debug, Clone, Copy)]
pub struct Camera {
    /// Rotation about the vertical axis, radians.
    pub yaw: f32,
    /// Elevation, radians, clamped just short of the poles.
    pub pitch: f32,
    /// Distance from the target.
    pub distance: f32,
    /// Point the camera orbits.
    pub target: [f32; 3],
    pub projection: Projection,
    /// Where an orientation click is taking us, if a turn is in progress.
    turning_to: Option<(f32, f32)>,
}

impl Default for Camera {
    fn default() -> Self {
        let mut camera = Camera {
            yaw: 0.0,
            pitch: 0.0,
            distance: 120.0,
            target: [0.0, 0.0, 0.0],
            projection: Projection::Orthographic,
            turning_to: None,
        };
        // Settle on the home view through the same path a cube click uses,
        // rather than writing the angles out twice.
        camera.look_along(HOME_VIEW);
        while camera.animate(1.0) {}
        camera
    }
}

impl Camera {
    /// Frames the given bounds, leaving a margin.
    pub fn frame(&mut self, bounds: &Bounds3) {
        let size = bounds.size();
        let centre = Point3::new(
            (bounds.min.x + bounds.max.x) / 2.0,
            (bounds.min.y + bounds.max.y) / 2.0,
            (bounds.min.z + bounds.max.z) / 2.0,
        );
        self.target = [centre.x.mm() as f32, centre.y.mm() as f32, centre.z.mm() as f32];
        let extent = size.x.mm().max(size.y.mm()).max(size.z.mm()) as f32;
        self.distance = (extent * 1.9).max(10.0);
    }

    /// Turns to look at the model along `normal`, as clicking a face of the
    /// view cube does. The camera ends up on the `normal` side, looking back.
    pub fn look_along(&mut self, normal: [f32; 3]) {
        let n = normalize(normal);
        // Looking *from* +n means the view direction is -n.
        // Just short of the pole: looking exactly straight down leaves the
        // up-vector undefined and the view spins to an arbitrary angle.
        let limit = std::f32::consts::FRAC_PI_2 - 0.01;
        let pitch = n[2].clamp(-1.0, 1.0).asin().clamp(-limit, limit);

        // Straight up or straight down, the yaw is undefined — `atan2(0, 0)`
        // answers zero, which lands the view a quarter turn out. Pick the yaw
        // that puts +X to the right and +Y up the screen, so looking down at
        // the case matches how the board sits in the PCB editor.
        let yaw = if n[0].abs() < 1e-4 && n[1].abs() < 1e-4 {
            std::f32::consts::FRAC_PI_2
        } else {
            (-n[1]).atan2(-n[0])
        };
        self.turning_to = Some((yaw, pitch));
    }

    /// Advances an orientation change. Returns true while still turning, so the
    /// window knows to keep repainting.
    pub fn animate(&mut self, dt: f32) -> bool {
        let Some((target_yaw, target_pitch)) = self.turning_to else {
            return false;
        };
        // Take the short way round rather than unwinding the long way.
        let mut delta_yaw = (target_yaw - self.yaw) % std::f32::consts::TAU;
        if delta_yaw > std::f32::consts::PI {
            delta_yaw -= std::f32::consts::TAU;
        } else if delta_yaw < -std::f32::consts::PI {
            delta_yaw += std::f32::consts::TAU;
        }
        let delta_pitch = target_pitch - self.pitch;

        if delta_yaw.abs() < 1e-3 && delta_pitch.abs() < 1e-3 {
            self.yaw = target_yaw;
            self.pitch = target_pitch;
            self.turning_to = None;
            return false;
        }

        let step = (dt * 9.0).clamp(0.0, 1.0);
        self.yaw += delta_yaw * step;
        self.pitch += delta_pitch * step;
        true
    }

    pub fn orbit(&mut self, delta_yaw: f32, delta_pitch: f32) {
        self.turning_to = None;
        self.yaw += delta_yaw;
        let limit = std::f32::consts::FRAC_PI_2 - 0.01;
        self.pitch = (self.pitch + delta_pitch).clamp(-limit, limit);
    }

    pub fn zoom(&mut self, factor: f32) {
        self.distance = (self.distance * factor).clamp(1.0, 5_000.0);
    }

    /// Pans across the view plane, in world units scaled to the current zoom.
    pub fn pan(&mut self, dx: f32, dy: f32) {
        let (right, up) = self.basis();
        let scale = self.distance * 0.0015;
        for axis in 0..3 {
            self.target[axis] += (-right[axis] * dx + up[axis] * dy) * scale;
        }
    }

    pub fn eye(&self) -> [f32; 3] {
        let forward = self.forward();
        [
            self.target[0] - forward[0] * self.distance,
            self.target[1] - forward[1] * self.distance,
            self.target[2] - forward[2] * self.distance,
        ]
    }

    /// Unit vector from the eye toward the target. Z is up.
    pub fn forward(&self) -> [f32; 3] {
        let (sp, cp) = self.pitch.sin_cos();
        let (sy, cy) = self.yaw.sin_cos();
        [cp * cy, cp * sy, -sp]
    }

    /// Right and up vectors of the view, for projecting points to the screen.
    pub fn basis(&self) -> ([f32; 3], [f32; 3]) {
        let forward = self.forward();
        let world_up = [0.0, 0.0, 1.0];
        let right = normalize(cross(forward, world_up));
        let up = normalize(cross(right, forward));
        (right, up)
    }

    /// Column-major view-projection matrix, ready for OpenGL.
    pub fn view_projection(&self, aspect: f32) -> [f32; 16] {
        let eye = self.eye();
        let forward = self.forward();
        let (right, up) = self.basis();

        // View matrix: world -> camera.
        let view = [
            right[0],
            up[0],
            -forward[0],
            0.0,
            right[1],
            up[1],
            -forward[1],
            0.0,
            right[2],
            up[2],
            -forward[2],
            0.0,
            -dot(right, eye),
            -dot(up, eye),
            dot(forward, eye),
            1.0,
        ];

        let near = 0.01;
        let far = self.distance * 8.0 + 1_000.0;
        let projection = match self.projection {
            Projection::Orthographic => {
                // Height chosen so that zoom behaves the same as perspective.
                let half_h = self.distance * 0.32;
                let half_w = half_h * aspect;
                [
                    1.0 / half_w,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    1.0 / half_h,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    -2.0 / (far - near),
                    0.0,
                    0.0,
                    0.0,
                    -(far + near) / (far - near),
                    1.0,
                ]
            },
            Projection::Perspective => {
                let fov = 0.6f32;
                let f = 1.0 / (fov / 2.0).tan();
                [
                    f / aspect,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    f,
                    0.0,
                    0.0,
                    0.0,
                    0.0,
                    (far + near) / (near - far),
                    -1.0,
                    0.0,
                    0.0,
                    2.0 * far * near / (near - far),
                    0.0,
                ]
            },
        };

        multiply(&projection, &view)
    }
}

fn cross(a: [f32; 3], b: [f32; 3]) -> [f32; 3] {
    [a[1] * b[2] - a[2] * b[1], a[2] * b[0] - a[0] * b[2], a[0] * b[1] - a[1] * b[0]]
}

fn dot(a: [f32; 3], b: [f32; 3]) -> f32 {
    a[0] * b[0] + a[1] * b[1] + a[2] * b[2]
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = dot(v, v).sqrt();
    if len <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Column-major 4x4 multiply.
fn multiply(a: &[f32; 16], b: &[f32; 16]) -> [f32; 16] {
    let mut out = [0.0f32; 16];
    for col in 0..4 {
        for row in 0..4 {
            let mut sum = 0.0;
            for k in 0..4 {
                sum += a[k * 4 + row] * b[col * 4 + k];
            }
            out[col * 4 + row] = sum;
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;
    use kicase_geometry::units::mm;

    /// The window opens on the corner every CAD package opens on: top, front
    /// and right all in view.
    #[test]
    fn the_default_view_shows_top_front_and_right() {
        let camera = Camera::default();
        let forward = camera.forward();
        let facing = |normal: [f32; 3]| {
            normal[0] * forward[0] + normal[1] * forward[1] + normal[2] * forward[2]
        };
        // A face is visible when its outward normal points back at the camera.
        assert!(facing([0.0, 0.0, 1.0]) < -0.1, "the top is not in view");
        assert!(facing([0.0, -1.0, 0.0]) < -0.1, "the front is not in view");
        assert!(facing([1.0, 0.0, 0.0]) < -0.1, "the right is not in view");
        // And the three opposite faces are hidden.
        assert!(facing([0.0, 0.0, -1.0]) > 0.1);
        assert!(facing([0.0, 1.0, 0.0]) > 0.1);
        assert!(facing([-1.0, 0.0, 0.0]) > 0.1);
    }

    #[test]
    fn framing_centres_on_the_bounds() {
        let mut camera = Camera::default();
        camera.frame(&Bounds3 {
            min: Point3::from_mm(0.0, 0.0, -6.0),
            max: Point3::from_mm(56.0, 36.0, 6.6),
        });
        assert!((camera.target[0] - 28.0).abs() < 1e-4);
        assert!((camera.target[1] - 18.0).abs() < 1e-4);
        assert!(camera.distance > 56.0, "distance {} is too close", camera.distance);
    }

    #[test]
    fn pitch_cannot_flip_over_the_pole() {
        let mut camera = Camera::default();
        camera.orbit(0.0, 100.0);
        assert!(camera.pitch < std::f32::consts::FRAC_PI_2);
        camera.orbit(0.0, -200.0);
        assert!(camera.pitch > -std::f32::consts::FRAC_PI_2);
    }

    #[test]
    fn the_eye_sits_one_distance_from_the_target() {
        let camera = Camera::default();
        let eye = camera.eye();
        let d = ((eye[0] - camera.target[0]).powi(2)
            + (eye[1] - camera.target[1]).powi(2)
            + (eye[2] - camera.target[2]).powi(2))
        .sqrt();
        assert!((d - camera.distance).abs() < 1e-3, "distance was {d}");
    }

    #[test]
    fn clicking_a_face_looks_straight_down_that_axis() {
        for normal in [
            [0.0, 0.0, 1.0],
            [0.0, 0.0, -1.0],
            [1.0, 0.0, 0.0],
            [-1.0, 0.0, 0.0],
            [0.0, 1.0, 0.0],
            [0.0, -1.0, 0.0],
        ] {
            let mut camera = Camera::default();
            camera.look_along(normal);
            // Run the turn to completion.
            for _ in 0..500 {
                if !camera.animate(1.0 / 60.0) {
                    break;
                }
            }
            // The eye ends up on the normal's side, looking back at the
            // target. Vertical views stop a hundredth of a radian short of the
            // pole on purpose, so allow for that.
            let forward = camera.forward();
            for axis in 0..3 {
                assert!(
                    (forward[axis] + normal[axis]).abs() < 0.02,
                    "looking along {normal:?} gave forward {forward:?}"
                );
            }
        }
    }

    /// Looking straight down, the model should sit the way it does in the PCB
    /// editor: X to the right, Y up the screen.
    #[test]
    fn the_top_view_is_not_a_quarter_turn_out() {
        let mut camera = Camera::default();
        camera.look_along([0.0, 0.0, 1.0]);
        while camera.animate(1.0 / 60.0) {}

        let (right, up) = camera.basis();
        assert!((right[0] - 1.0).abs() < 1e-3, "screen right was {right:?}, wanted +X");
        assert!((up[1] - 1.0).abs() < 1e-3, "screen up was {up:?}, wanted +Y");
    }

    /// From underneath, the view is mirrored: X still right, Y downward.
    #[test]
    fn the_bottom_view_is_mirrored_not_rotated() {
        let mut camera = Camera::default();
        camera.look_along([0.0, 0.0, -1.0]);
        while camera.animate(1.0 / 60.0) {}

        let (right, up) = camera.basis();
        assert!((right[0] - 1.0).abs() < 1e-3, "screen right was {right:?}, wanted +X");
        assert!((up[1] + 1.0).abs() < 1e-3, "screen up was {up:?}, wanted -Y");
    }

    /// The side views keep Z up the screen, as an elevation should.
    #[test]
    fn a_side_view_keeps_z_up_the_screen() {
        for normal in [[0.0, -1.0, 0.0], [0.0, 1.0, 0.0], [1.0, 0.0, 0.0], [-1.0, 0.0, 0.0]] {
            let mut camera = Camera::default();
            camera.look_along(normal);
            while camera.animate(1.0 / 60.0) {}
            let (_, up) = camera.basis();
            assert!(
                (up[2] - 1.0).abs() < 1e-3,
                "looking along {normal:?} put screen up at {up:?}, wanted +Z"
            );
        }
    }

    #[test]
    fn a_turn_takes_the_short_way_round() {
        let mut camera = Camera { yaw: 3.0, ..Camera::default() };
        camera.look_along([-1.0, 0.0, 0.0]);
        // Target yaw is 0; from 3.0 rad the short way is downward, not up
        // through 2*pi.
        camera.animate(1.0 / 60.0);
        assert!(camera.yaw < 3.0, "yaw went the long way: {}", camera.yaw);
    }

    #[test]
    fn dragging_cancels_a_turn_in_progress() {
        let mut camera = Camera::default();
        camera.look_along([0.0, 0.0, 1.0]);
        camera.orbit(0.1, 0.0);
        assert!(!camera.animate(1.0 / 60.0), "a drag should cancel the turn");
    }

    #[test]
    fn an_orthographic_view_does_not_foreshorten() {
        // Two points the same distance apart, at different depths, must project
        // to the same length under orthographic projection.
        let camera = Camera { projection: Projection::Orthographic, ..Camera::default() };
        let m = camera.view_projection(1.0);
        let project = |p: [f32; 3]| {
            let x = m[0] * p[0] + m[4] * p[1] + m[8] * p[2] + m[12];
            let w = m[3] * p[0] + m[7] * p[1] + m[11] * p[2] + m[15];
            x / w
        };
        let near_pair = project([10.0, 0.0, 0.0]) - project([0.0, 0.0, 0.0]);
        let far_pair = project([10.0, 0.0, 40.0]) - project([0.0, 0.0, 40.0]);
        assert!(
            (near_pair - far_pair).abs() < 1e-5,
            "orthographic projection foreshortened: {near_pair} vs {far_pair}"
        );
        let _ = mm(1.0);
    }
}
