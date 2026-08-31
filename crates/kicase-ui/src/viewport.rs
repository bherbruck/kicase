//! The 3D viewport.
//!
//! KiCad's own 3D viewer cannot do what enclosure work needs — it has no live
//! reload, no section plane, and no per-part visibility — so KiCase draws the
//! scene itself. The geometry comes from the same B-rep the STEP files do,
//! triangulated for display only.
//!
//! Rendering is a raw GL pass inside an egui paint callback: a handful of
//! triangles, flat-shaded, with the section implemented as a clip plane in the
//! fragment shader.

use crate::camera::{Camera, Projection};
use egui_glow::glow::{self, HasContext};
use kicase_geometry::types::Bounds3;
use kicase_model::scene::{PartId, Scene};
use std::collections::BTreeMap;
use std::sync::{Arc, Mutex};

/// Which way the section plane faces.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SectionAxis {
    X,
    Y,
    Z,
}

impl SectionAxis {
    pub const ALL: [SectionAxis; 3] = [SectionAxis::X, SectionAxis::Y, SectionAxis::Z];

    pub fn label(self) -> &'static str {
        match self {
            SectionAxis::X => "X",
            SectionAxis::Y => "Y",
            SectionAxis::Z => "Z",
        }
    }

    fn normal(self) -> [f32; 3] {
        match self {
            SectionAxis::X => [1.0, 0.0, 0.0],
            SectionAxis::Y => [0.0, 1.0, 0.0],
            SectionAxis::Z => [0.0, 0.0, 1.0],
        }
    }
}

/// A cutting plane swept through the model.
#[derive(Debug, Clone, Copy)]
pub struct Section {
    pub enabled: bool,
    pub axis: SectionAxis,
    /// Where the plane sits along its axis, as a fraction of the model's extent.
    pub position: f32,
    /// Keep the near side rather than the far side.
    pub flipped: bool,
    /// Sweep the plane back and forth on its own.
    pub sweeping: bool,
}

impl Section {
    /// Turns the section on at a given fraction of the model's extent.
    pub fn at(axis: SectionAxis, position: f32) -> Self {
        Section { enabled: true, axis, position, flipped: false, sweeping: false }
    }
}

impl Default for Section {
    fn default() -> Self {
        Section {
            enabled: false,
            axis: SectionAxis::Y,
            position: 0.5,
            flipped: false,
            sweeping: false,
        }
    }
}

impl Section {
    /// The clip plane in world space: `xyz` is the normal, `w` the offset.
    /// Fragments where `dot(normal, position) > w` are discarded.
    ///
    /// Which half goes is decided from where the camera is: the half between
    /// you and the plane is the one removed, so a section always opens the
    /// model towards you however the view is turned. `flipped` swaps it.
    fn plane(&self, bounds: &Bounds3, eye: [f32; 3]) -> [f32; 4] {
        let n = self.axis.normal();
        let (min, max) = match self.axis {
            SectionAxis::X => (bounds.min.x.mm(), bounds.max.x.mm()),
            SectionAxis::Y => (bounds.min.y.mm(), bounds.max.y.mm()),
            SectionAxis::Z => (bounds.min.z.mm(), bounds.max.z.mm()),
        };
        // A little margin so the extremes fully show and fully hide.
        let span = (max - min) as f32;
        let at = min as f32 + span * self.position;

        // Positive when the camera sits on the +normal side of the plane.
        let eye_side = n[0] * eye[0] + n[1] * eye[1] + n[2] * eye[2] - at;
        let cut_positive_side = (eye_side >= 0.0) != self.flipped;

        if cut_positive_side {
            [n[0], n[1], n[2], at]
        } else {
            [-n[0], -n[1], -n[2], -at]
        }
    }
}

/// Everything the viewport needs to draw, rebuilt whenever the model changes.
pub struct Viewport {
    pub camera: Camera,
    pub section: Section,
    pub visible: BTreeMap<PartId, bool>,
    /// World bounds of the current scene, for framing and for the section.
    bounds: Option<Bounds3>,
    /// Triangles ready to upload, and a counter so the GL side knows when the
    /// model has changed underneath it.
    scene: Option<Arc<Scene>>,
    version: u64,
    gl: Arc<Mutex<GlState>>,
}

impl Default for Viewport {
    fn default() -> Self {
        let mut visible = BTreeMap::new();
        for part in PartId::ALL {
            visible.insert(part, true);
        }
        Viewport {
            camera: Camera::default(),
            section: Section::default(),
            visible,
            bounds: None,
            scene: None,
            version: 0,
            gl: Arc::new(Mutex::new(GlState::default())),
        }
    }
}

impl Viewport {
    /// Hands the viewport a freshly built scene.
    pub fn set_scene(&mut self, scene: Scene) {
        let bounds = scene.bounds();
        let first = self.scene.is_none();
        self.bounds = bounds;
        self.scene = Some(Arc::new(scene));
        self.version += 1;
        if first {
            if let Some(bounds) = self.bounds {
                self.camera.frame(&bounds);
            }
        }
    }

    pub fn has_scene(&self) -> bool {
        self.scene.is_some()
    }

    pub fn triangle_count(&self) -> usize {
        self.scene.as_ref().map(|s| s.triangle_count()).unwrap_or(0)
    }

    pub fn frame_camera(&mut self) {
        if let Some(bounds) = self.bounds {
            self.camera.frame(&bounds);
        }
    }

    /// Position of the section plane in millimetres, for display.
    pub fn section_millimetres(&self) -> Option<f64> {
        let bounds = self.bounds?;
        let (min, max) = match self.section.axis {
            SectionAxis::X => (bounds.min.x.mm(), bounds.max.x.mm()),
            SectionAxis::Y => (bounds.min.y.mm(), bounds.max.y.mm()),
            SectionAxis::Z => (bounds.min.z.mm(), bounds.max.z.mm()),
        };
        Some(min + (max - min) * self.section.position as f64)
    }

    /// Draws the scene into `rect`, handling orbit, pan and zoom.
    pub fn show(&mut self, ui: &mut egui::Ui) -> egui::Response {
        let available = ui.available_size();
        let (rect, response) = ui.allocate_exact_size(available, egui::Sense::click_and_drag());

        // Orbit with the left button, pan with the middle or with shift. The
        // cube claims its own corner, so a drag that starts there is ignored.
        let cube_corner = egui::Rect::from_min_size(
            egui::Pos2::new(rect.right() - crate::viewcube::SIZE - 12.0, rect.top() + 12.0),
            egui::Vec2::splat(crate::viewcube::SIZE),
        );
        let started_on_cube =
            response.interact_pointer_pos().is_some_and(|p| cube_corner.contains(p));
        if response.dragged() && !started_on_cube {
            let delta = response.drag_delta();
            let panning =
                ui.input(|i| i.modifiers.shift) || response.dragged_by(egui::PointerButton::Middle);
            if panning {
                self.camera.pan(delta.x, delta.y);
            } else {
                // Drag right, the model turns right: the camera goes the
                // other way, which is the opposite sign to the drag.
                self.camera.orbit(-delta.x * 0.008, delta.y * 0.008);
            }
        }
        if response.hovered() {
            let scroll = ui.input(|i| i.smooth_scroll_delta.y);
            if scroll.abs() > 0.0 {
                self.camera.zoom(1.0 - scroll * 0.002);
            }
        }

        // Ease through an orientation change started from the cube.
        let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
        if self.camera.animate(dt) {
            ui.ctx().request_repaint();
        }

        if self.section.sweeping {
            let dt = ui.input(|i| i.stable_dt).clamp(0.0, 0.1);
            // A slow triangle wave: out to one side, back to the other.
            self.section.position = (self.section.position + dt * 0.25) % 1.0;
            ui.ctx().request_repaint();
        }

        let (Some(scene), Some(bounds)) = (self.scene.clone(), self.bounds) else {
            ui.painter().text(
                rect.center(),
                egui::Align2::CENTER_CENTER,
                "No geometry yet — press Rebuild",
                egui::FontId::proportional(14.0),
                ui.visuals().weak_text_color(),
            );
            return response;
        };

        let aspect = (rect.width() / rect.height().max(1.0)).max(0.01);
        let draw = DrawCall {
            scene,
            version: self.version,
            mvp: self.camera.view_projection(aspect),
            eye: self.camera.eye(),
            clip: self.section.plane(&bounds, self.camera.eye()),
            clipping: self.section.enabled,
            visible: self.visible.clone(),
            gl: self.gl.clone(),
        };

        let callback = egui::PaintCallback {
            rect,
            callback: Arc::new(egui_glow::CallbackFn::new(move |_info, painter| {
                draw.paint(painter.gl());
            })),
        };
        ui.painter().add(callback);

        // The orientation cube sits over the scene, in the corner.
        if crate::viewcube::show(ui, rect, &mut self.camera) {
            ui.ctx().request_repaint();
        }
        response
    }

    /// Releases GL resources. Call from `eframe::App::on_exit`.
    pub fn destroy(&self, gl: &glow::Context) {
        if let Ok(mut state) = self.gl.lock() {
            state.destroy(gl);
        }
    }
}

/// One frame's worth of drawing state, moved into the paint callback.
struct DrawCall {
    scene: Arc<Scene>,
    version: u64,
    mvp: [f32; 16],
    eye: [f32; 3],
    clip: [f32; 4],
    clipping: bool,
    visible: BTreeMap<PartId, bool>,
    gl: Arc<Mutex<GlState>>,
}

impl DrawCall {
    fn paint(&self, gl: &glow::Context) {
        let Ok(mut state) = self.gl.lock() else { return };
        state.ensure_program(gl);
        state.ensure_scene(gl, &self.scene, self.version);

        let Some(program) = state.program else { return };

        unsafe {
            gl.enable(glow::DEPTH_TEST);
            gl.depth_func(glow::LESS);
            gl.clear(glow::DEPTH_BUFFER_BIT);
            // Both faces are drawn: the far side of a sectioned solid is what
            // makes the cut read as material rather than a hole.
            gl.disable(glow::CULL_FACE);
            gl.use_program(Some(program));

            set_mat4(gl, program, "u_mvp", &self.mvp);
            set_vec3(gl, program, "u_light", normalize(self.eye));
            set_vec4(gl, program, "u_clip", self.clip);
            set_float(gl, program, "u_clip_on", if self.clipping { 1.0 } else { 0.0 });
            set_vec3(gl, program, "u_cut", [0.85, 0.45, 0.30]);

            for part in &state.parts {
                if !self.visible.get(&part.id).copied().unwrap_or(true) {
                    continue;
                }
                set_vec3(gl, program, "u_color", part.color);
                gl.bind_vertex_array(Some(part.vao));
                gl.draw_elements(glow::TRIANGLES, part.index_count, glow::UNSIGNED_INT, 0);
            }

            gl.bind_vertex_array(None);
            gl.use_program(None);
            gl.disable(glow::DEPTH_TEST);
        }
    }
}

#[derive(Default)]
struct GlState {
    program: Option<glow::Program>,
    parts: Vec<GlPart>,
    uploaded: u64,
}

struct GlPart {
    id: PartId,
    color: [f32; 3],
    vao: glow::VertexArray,
    vbo: glow::Buffer,
    ebo: glow::Buffer,
    index_count: i32,
}

impl GlState {
    fn ensure_program(&mut self, gl: &glow::Context) {
        if self.program.is_some() {
            return;
        }
        self.program = compile(gl);
    }

    fn ensure_scene(&mut self, gl: &glow::Context, scene: &Scene, version: u64) {
        if self.uploaded == version && !self.parts.is_empty() {
            return;
        }
        self.release_parts(gl);

        for part in &scene.parts {
            if part.mesh.is_empty() {
                continue;
            }
            // Interleaved position + normal, in millimetres.
            let mut vertices: Vec<f32> = Vec::with_capacity(part.mesh.positions.len() * 6);
            for (index, position) in part.mesh.positions.iter().enumerate() {
                let normal = part.mesh.normals.get(index).copied().unwrap_or_default();
                vertices.extend_from_slice(&[
                    position.x.mm() as f32,
                    position.y.mm() as f32,
                    position.z.mm() as f32,
                    normal.x.mm() as f32,
                    normal.y.mm() as f32,
                    normal.z.mm() as f32,
                ]);
            }

            unsafe {
                let vao = match gl.create_vertex_array() {
                    Ok(vao) => vao,
                    Err(_) => continue,
                };
                let vbo = match gl.create_buffer() {
                    Ok(buffer) => buffer,
                    Err(_) => continue,
                };
                let ebo = match gl.create_buffer() {
                    Ok(buffer) => buffer,
                    Err(_) => continue,
                };

                gl.bind_vertex_array(Some(vao));
                gl.bind_buffer(glow::ARRAY_BUFFER, Some(vbo));
                gl.buffer_data_u8_slice(glow::ARRAY_BUFFER, bytes_of(&vertices), glow::STATIC_DRAW);
                gl.bind_buffer(glow::ELEMENT_ARRAY_BUFFER, Some(ebo));
                gl.buffer_data_u8_slice(
                    glow::ELEMENT_ARRAY_BUFFER,
                    bytes_of(&part.mesh.indices),
                    glow::STATIC_DRAW,
                );

                let stride = 6 * std::mem::size_of::<f32>() as i32;
                gl.enable_vertex_attrib_array(0);
                gl.vertex_attrib_pointer_f32(0, 3, glow::FLOAT, false, stride, 0);
                gl.enable_vertex_attrib_array(1);
                gl.vertex_attrib_pointer_f32(
                    1,
                    3,
                    glow::FLOAT,
                    false,
                    stride,
                    3 * std::mem::size_of::<f32>() as i32,
                );
                gl.bind_vertex_array(None);

                self.parts.push(GlPart {
                    id: part.id,
                    color: part.id.color(),
                    vao,
                    vbo,
                    ebo,
                    index_count: part.mesh.indices.len() as i32,
                });
            }
        }
        self.uploaded = version;
    }

    fn release_parts(&mut self, gl: &glow::Context) {
        for part in self.parts.drain(..) {
            unsafe {
                gl.delete_vertex_array(part.vao);
                gl.delete_buffer(part.vbo);
                gl.delete_buffer(part.ebo);
            }
        }
    }

    fn destroy(&mut self, gl: &glow::Context) {
        self.release_parts(gl);
        if let Some(program) = self.program.take() {
            unsafe { gl.delete_program(program) };
        }
    }
}

const VERTEX_SHADER: &str = r#"#version 330 core
layout (location = 0) in vec3 a_pos;
layout (location = 1) in vec3 a_normal;
uniform mat4 u_mvp;
out vec3 v_normal;
out vec3 v_world;
void main() {
    v_normal = a_normal;
    v_world = a_pos;
    gl_Position = u_mvp * vec4(a_pos, 1.0);
}
"#;

const FRAGMENT_SHADER: &str = r#"#version 330 core
in vec3 v_normal;
in vec3 v_world;
uniform vec3 u_color;
uniform vec3 u_cut;
uniform vec3 u_light;
uniform vec4 u_clip;
uniform float u_clip_on;
out vec4 f_color;
void main() {
    if (u_clip_on > 0.5 && dot(u_clip.xyz, v_world) > u_clip.w) {
        discard;
    }
    vec3 n = normalize(v_normal);
    vec3 base = u_color;
    if (!gl_FrontFacing) {
        n = -n;
        // Faces revealed by the section plane are called out; ordinary interior
        // surfaces just read as shadowed, or the whole model looks cut open.
        base = u_clip_on > 0.5 ? u_cut : u_color * 0.55;
    }
    float lambert = max(dot(n, normalize(u_light)), 0.0);
    float ambient = 0.38;
    vec3 shaded = base * (ambient + 0.62 * lambert);
    f_color = vec4(shaded, 1.0);
}
"#;

fn compile(gl: &glow::Context) -> Option<glow::Program> {
    unsafe {
        let program = gl.create_program().ok()?;
        let mut shaders = Vec::new();
        for (kind, source) in
            [(glow::VERTEX_SHADER, VERTEX_SHADER), (glow::FRAGMENT_SHADER, FRAGMENT_SHADER)]
        {
            let shader = gl.create_shader(kind).ok()?;
            gl.shader_source(shader, source);
            gl.compile_shader(shader);
            if !gl.get_shader_compile_status(shader) {
                tracing::error!("viewport shader failed: {}", gl.get_shader_info_log(shader));
                return None;
            }
            gl.attach_shader(program, shader);
            shaders.push(shader);
        }
        gl.link_program(program);
        if !gl.get_program_link_status(program) {
            tracing::error!("viewport program failed: {}", gl.get_program_info_log(program));
            return None;
        }
        for shader in shaders {
            gl.detach_shader(program, shader);
            gl.delete_shader(shader);
        }
        Some(program)
    }
}

unsafe fn set_mat4(gl: &glow::Context, program: glow::Program, name: &str, value: &[f32; 16]) {
    if let Some(location) = gl.get_uniform_location(program, name) {
        gl.uniform_matrix_4_f32_slice(Some(&location), false, value);
    }
}

unsafe fn set_vec3(gl: &glow::Context, program: glow::Program, name: &str, value: [f32; 3]) {
    if let Some(location) = gl.get_uniform_location(program, name) {
        gl.uniform_3_f32(Some(&location), value[0], value[1], value[2]);
    }
}

unsafe fn set_vec4(gl: &glow::Context, program: glow::Program, name: &str, value: [f32; 4]) {
    if let Some(location) = gl.get_uniform_location(program, name) {
        gl.uniform_4_f32(Some(&location), value[0], value[1], value[2], value[3]);
    }
}

unsafe fn set_float(gl: &glow::Context, program: glow::Program, name: &str, value: f32) {
    if let Some(location) = gl.get_uniform_location(program, name) {
        gl.uniform_1_f32(Some(&location), value);
    }
}

fn bytes_of<T>(values: &[T]) -> &[u8] {
    unsafe {
        std::slice::from_raw_parts(values.as_ptr() as *const u8, std::mem::size_of_val(values))
    }
}

fn normalize(v: [f32; 3]) -> [f32; 3] {
    let len = (v[0] * v[0] + v[1] * v[1] + v[2] * v[2]).sqrt();
    if len <= f32::EPSILON {
        [0.0, 0.0, 1.0]
    } else {
        [v[0] / len, v[1] / len, v[2] / len]
    }
}

/// Whether the camera is currently orthographic, for the UI toggle.
pub fn is_orthographic(camera: &Camera) -> bool {
    camera.projection == Projection::Orthographic
}

#[cfg(test)]
mod tests {
    use super::*;
    use kicase_geometry::types::Point3;

    fn bounds() -> Bounds3 {
        Bounds3 { min: Point3::from_mm(0.0, 0.0, 0.0), max: Point3::from_mm(50.0, 30.0, 10.0) }
    }

    /// An eye far out on the +axis side, so the near half is the + half.
    fn eye_on_plus(axis: SectionAxis) -> [f32; 3] {
        match axis {
            SectionAxis::X => [500.0, 0.0, 0.0],
            SectionAxis::Y => [0.0, 500.0, 0.0],
            SectionAxis::Z => [0.0, 0.0, 500.0],
        }
    }

    #[test]
    fn the_section_plane_sweeps_across_the_model() {
        let eye = eye_on_plus(SectionAxis::X);
        let mut section = Section { enabled: true, axis: SectionAxis::X, ..Section::default() };
        section.position = 0.0;
        assert!((section.plane(&bounds(), eye)[3] - 0.0).abs() < 1e-5);
        section.position = 1.0;
        assert!((section.plane(&bounds(), eye)[3] - 50.0).abs() < 1e-5);
        section.position = 0.5;
        assert!((section.plane(&bounds(), eye)[3] - 25.0).abs() < 1e-5);
    }

    /// Whichever side you are looking from, the section takes away the half
    /// between you and the plane.
    #[test]
    fn the_section_always_opens_towards_the_viewer() {
        let section =
            Section { enabled: true, axis: SectionAxis::Y, position: 0.5, ..Section::default() };

        // Looking from +Y: the +Y half is nearer, so that is the half removed.
        let from_plus = section.plane(&bounds(), [0.0, 500.0, 0.0]);
        assert!(from_plus[1] > 0.0, "should discard the +Y half, got {from_plus:?}");

        // Looking from -Y: the other half goes instead.
        let from_minus = section.plane(&bounds(), [0.0, -500.0, 0.0]);
        assert!(from_minus[1] < 0.0, "should discard the -Y half, got {from_minus:?}");
    }

    #[test]
    fn flipping_reverses_whichever_side_was_chosen() {
        let mut section =
            Section { enabled: true, axis: SectionAxis::Y, position: 0.5, ..Section::default() };
        let eye = [0.0, 500.0, 0.0];
        let normal = section.plane(&bounds(), eye)[1];
        section.flipped = true;
        assert!(
            section.plane(&bounds(), eye)[1] * normal < 0.0,
            "flip should reverse the chosen side"
        );
    }

    #[test]
    fn each_axis_uses_its_own_extent() {
        let mut section = Section { enabled: true, position: 1.0, ..Section::default() };
        section.axis = SectionAxis::Y;
        assert!((section.plane(&bounds(), eye_on_plus(SectionAxis::Y))[3] - 30.0).abs() < 1e-5);
        section.axis = SectionAxis::Z;
        assert!((section.plane(&bounds(), eye_on_plus(SectionAxis::Z))[3] - 10.0).abs() < 1e-5);
    }

    #[test]
    fn parts_start_visible() {
        let viewport = Viewport::default();
        assert!(PartId::ALL.iter().all(|p| viewport.visible[p]));
    }
}
