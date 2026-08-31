//! The enclosure designer window.
//!
//! The UI is deliberately presentational: it edits an [`EnclosureConfig`] and
//! calls a [`DesignerBackend`] for anything that touches KiCad, the CAD kernel
//! or the filesystem. That keeps the window testable and stops KiCad-specific
//! behaviour leaking into it.
//!
//! There is no 3D viewport here, by design: KiCad's own 3D viewer is the
//! viewport.

mod app;
pub mod camera;
pub mod viewcube;
pub mod viewport;

pub use app::DesignerApp;
pub use camera::{Camera, Projection};
use kicase_model::EnclosureConfig;
pub use viewport::{Section, SectionAxis, Viewport};

/// What an export button asks for.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ExportKind {
    Step,
    Stl,
    OpenScad,
}

impl ExportKind {
    pub fn label(self) -> &'static str {
        match self {
            ExportKind::Step => "Export STEP",
            ExportKind::Stl => "Export STL",
            ExportKind::OpenScad => "Generate OpenSCAD project",
        }
    }
}

/// One row in the features tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ItemInfo {
    pub id: String,
    /// UUID of the KiCad graphic behind it, so the window can bind settings.
    pub uuid: String,
    pub detail: String,
    /// Set when the row refers to a KiCad object that no longer exists.
    pub orphaned: bool,
}

/// A graphic on one of the enclosure layers, offered for association.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct GraphicRow {
    pub uuid: String,
    /// Which enclosure layer it is on.
    pub layer: String,
    pub description: String,
    /// True when the graphic forms a closed region on its own.
    pub closed: bool,
    /// Id of the project entry already bound to it, if any.
    pub bound_to: Option<String>,
}

/// A detected mounting hole.
#[derive(Debug, Clone, PartialEq)]
pub struct HoleInfo {
    pub id: String,
    pub detail: String,
    pub orphaned: bool,
}

/// Board-derived information shown in the window.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct DesignerData {
    pub project_dir: String,
    /// KiCad version string, or `None` when working from a board file.
    pub kicad_version: Option<String>,
    pub board_summary: String,
    pub datums: Vec<ItemInfo>,
    pub cutouts: Vec<ItemInfo>,
    pub solids: Vec<ItemInfo>,
    pub holes: Vec<HoleInfo>,
    pub orphans: Vec<ItemInfo>,
    /// Graphics available for association.
    pub graphics: Vec<GraphicRow>,
    /// Overall exterior size, once geometry has been generated.
    pub dimensions: Option<String>,
    /// Set when the wall thickness comes from the width of the drawn outline,
    /// in which case the wall setting is inert.
    pub drawn_wall: Option<String>,
    pub problems: Vec<String>,
}

/// The outcome of a button press.
#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct ActionReport {
    pub headline: String,
    pub lines: Vec<String>,
    pub ok: bool,
}

impl ActionReport {
    pub fn ok(headline: impl Into<String>) -> Self {
        ActionReport { headline: headline.into(), lines: Vec::new(), ok: true }
    }

    pub fn failed(headline: impl Into<String>) -> Self {
        ActionReport { headline: headline.into(), lines: Vec::new(), ok: false }
    }

    pub fn with(mut self, line: impl Into<String>) -> Self {
        self.lines.push(line.into());
        self
    }
}

/// Everything the window needs from the rest of KiCase.
pub trait DesignerBackend {
    /// Re-reads the board and reports what is on it.
    fn refresh(&mut self, config: &EnclosureConfig) -> Result<DesignerData, String>;

    /// Claims layers, detects mounting holes and writes `enclosure.toml`.
    fn initialize(&mut self, config: &mut EnclosureConfig) -> Result<ActionReport, String>;

    /// Regenerates geometry and updates KiCad.
    fn rebuild(&mut self, config: &EnclosureConfig) -> Result<ActionReport, String>;

    /// Writes one kind of output.
    fn export(
        &mut self,
        config: &EnclosureConfig,
        kind: ExportKind,
    ) -> Result<ActionReport, String>;

    /// Persists settings without generating geometry.
    fn save(&mut self, config: &EnclosureConfig) -> Result<(), String>;

    /// Builds the displayable scene: the board and both enclosure parts,
    /// triangulated. Called after every rebuild so the viewport stays in step.
    fn scene(&mut self, config: &EnclosureConfig) -> Result<kicase_model::scene::Scene, String>;

    /// True when the board has changed since the last call.
    ///
    /// Polled while live updates are on, so moving a footprint in KiCad shows
    /// up here without anyone pressing anything.
    fn board_changed(&mut self) -> bool {
        false
    }

    /// Hands the backend a way to wake the window.
    ///
    /// Without this the window has to tick to ask whether anything changed,
    /// which costs CPU forever on a machine with no GPU to spare.
    fn set_repaint_waker(&mut self, _waker: std::sync::Arc<dyn Fn() + Send + Sync>) {}
}

/// Opens the designer window.
pub fn run(
    backend: Box<dyn DesignerBackend>,
    config: EnclosureConfig,
) -> Result<(), eframe::Error> {
    run_with_screenshot(backend, config, None, None, Vec::new(), None, None)
}

/// Opens the designer, optionally saving a screenshot and closing.
///
/// The screenshot path exists so the viewport can be checked without a person
/// looking at it: rendering that silently draws nothing looks exactly like
/// rendering that works.
pub fn run_with_screenshot(
    backend: Box<dyn DesignerBackend>,
    config: EnclosureConfig,
    screenshot: Option<std::path::PathBuf>,
    section: Option<Section>,
    show_only: Vec<kicase_model::scene::PartId>,
    screenshot_delay: Option<std::time::Duration>,
    view: Option<String>,
) -> Result<(), eframe::Error> {
    let options = eframe::NativeOptions {
        // Without this the surface has no depth attachment, the depth test is
        // silently a no-op, and the model draws in submission order — which
        // looks exactly like an x-ray view.
        depth_buffer: 24,
        viewport: eframe::egui::ViewportBuilder::default()
            .with_inner_size([1180.0, 800.0])
            .with_min_inner_size([720.0, 520.0])
            .with_title("KiCad Enclosure Designer"),
        ..Default::default()
    };
    eframe::run_native(
        "KiCase",
        options,
        Box::new(move |_cc| {
            let mut app = DesignerApp::new(backend, config);
            app.set_screenshot(screenshot);
            app.set_section(section);
            app.show_only(&show_only);
            if let Some(delay) = screenshot_delay {
                app.set_screenshot_delay(delay);
            }
            app.set_view(view.as_deref());
            Ok(Box::new(app))
        }),
    )
}
