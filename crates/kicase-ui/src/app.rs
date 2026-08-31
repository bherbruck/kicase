//! The designer window itself.

use crate::viewport::{SectionAxis, Viewport};
use crate::{ActionReport, DesignerBackend, DesignerData, ExportKind};
use egui::{Color32, RichText, Ui};
use kicase_geometry::units::{mm, Length};
use kicase_model::{DatumConfig, DatumNormal, EnclosureConfig, FeatureConfig};
use std::collections::HashMap;

pub struct DesignerApp {
    backend: Box<dyn DesignerBackend>,
    config: EnclosureConfig,
    data: DesignerData,
    report: Option<ActionReport>,
    error: Option<String>,
    /// Settings changed since the last save.
    dirty: bool,
    busy_label: Option<&'static str>,
    /// Names typed into the association rows, keyed by graphic UUID.
    pending_ids: HashMap<String, String>,
    /// Datum chosen for each pending cutout, keyed by graphic UUID.
    pending_datums: HashMap<String, String>,
    viewport: Viewport,
    /// Set to save a screenshot after a few frames and exit.
    screenshot: Option<std::path::PathBuf>,
    frames: u32,
    /// How long to wait before the screenshot, so a change can be made first.
    screenshot_after: std::time::Duration,
    started: Option<std::time::Instant>,
    /// Follow the board as it is edited, without anyone pressing Rebuild.
    live: bool,
    /// Whether the backend has been given a way to wake this window.
    waker_set: bool,
}

impl DesignerApp {
    pub fn new(mut backend: Box<dyn DesignerBackend>, config: EnclosureConfig) -> Self {
        let (data, error) = match backend.refresh(&config) {
            Ok(data) => (data, None),
            Err(err) => (DesignerData::default(), Some(err)),
        };
        let mut app = DesignerApp {
            backend,
            config,
            data,
            report: None,
            error,
            dirty: false,
            busy_label: None,
            pending_ids: HashMap::new(),
            pending_datums: HashMap::new(),
            viewport: Viewport::default(),
            screenshot: None,
            frames: 0,
            screenshot_after: std::time::Duration::from_millis(150),
            started: None,
            live: true,
            waker_set: false,
        };
        app.reload_scene();
        app
    }

    /// Renders a few frames, saves a PNG and quits. Used to verify the
    /// viewport actually draws.
    pub fn set_screenshot(&mut self, path: Option<std::path::PathBuf>) {
        self.screenshot = path;
    }

    /// Waits this long before capturing, so a board edit can land first.
    pub fn set_screenshot_delay(&mut self, delay: std::time::Duration) {
        self.screenshot_after = delay;
    }

    /// Shows only the named parts. Used by screenshot checks.
    pub fn show_only(&mut self, parts: &[kicase_model::scene::PartId]) {
        if parts.is_empty() {
            return;
        }
        for part in kicase_model::scene::PartId::ALL {
            self.viewport.visible.insert(part, parts.contains(&part));
        }
    }

    /// Turns the view to a named orientation, as clicking the cube does.
    pub fn set_view(&mut self, view: Option<&str>) {
        let normal = match view {
            Some("top") => [0.0, 0.0, 1.0],
            Some("bottom") => [0.0, 0.0, -1.0],
            Some("front") => [0.0, -1.0, 0.0],
            Some("back") => [0.0, 1.0, 0.0],
            Some("left") => [-1.0, 0.0, 0.0],
            Some("right") => [1.0, 0.0, 0.0],
            Some("iso") | Some("home") => crate::camera::HOME_VIEW,
            _ => return,
        };
        self.viewport.camera.look_along(normal);
        // Screenshots do not wait for the easing, so land immediately.
        while self.viewport.camera.animate(1.0) {}
    }

    /// Opens with the section plane already on, for screenshots.
    pub fn set_section(&mut self, section: Option<crate::viewport::Section>) {
        if let Some(section) = section {
            self.viewport.section = section;
        }
    }

    fn handle_screenshot(&mut self, ctx: &egui::Context) {
        let Some(path) = self.screenshot.clone() else { return };
        self.frames += 1;
        let started = *self.started.get_or_insert_with(std::time::Instant::now);
        // Wait for the scene to draw, and for any pending edit to be noticed.
        if self.frames > 3
            && started.elapsed() >= self.screenshot_after
            && self.frames.is_multiple_of(4)
        {
            ctx.send_viewport_cmd(egui::ViewportCommand::Screenshot);
        }
        ctx.request_repaint();

        let captured = ctx.input(|i| {
            i.events.iter().find_map(|event| match event {
                egui::Event::Screenshot { image, .. } => Some(image.clone()),
                _ => None,
            })
        });
        if let Some(image) = captured {
            if let Err(err) = save_png(&path, &image) {
                tracing::error!("could not save {}: {err}", path.display());
            } else {
                tracing::info!("wrote {}", path.display());
            }
            ctx.send_viewport_cmd(egui::ViewportCommand::Close);
        }
    }

    fn refresh(&mut self) {
        match self.backend.refresh(&self.config) {
            Ok(data) => {
                self.data = data;
                self.error = None;
            },
            Err(err) => self.error = Some(err),
        }
        self.reload_scene();
    }

    /// Rebuilds what the viewport draws. A failure here is not fatal: the
    /// settings panel still works, the viewport just says there is nothing yet.
    fn reload_scene(&mut self) {
        match self.backend.scene(&self.config) {
            Ok(scene) => self.viewport.set_scene(scene),
            Err(err) => tracing::debug!("no scene to draw: {err}"),
        }
    }

    fn run_action(&mut self, label: &'static str, action: Action) {
        self.busy_label = Some(label);
        let result = match action {
            Action::Initialize => self.backend.initialize(&mut self.config),
            Action::Rebuild => self.backend.rebuild(&self.config),
            Action::Export(kind) => self.backend.export(&self.config, kind),
        };
        self.busy_label = None;
        match result {
            Ok(report) => {
                self.report = Some(report);
                self.error = None;
                self.dirty = false;
                self.refresh();
            },
            Err(err) => {
                self.error = Some(err);
                self.report = None;
            },
        }
    }

    fn save_if_dirty(&mut self) {
        if !self.dirty {
            return;
        }
        match self.backend.save(&self.config) {
            Ok(()) => self.dirty = false,
            Err(err) => self.error = Some(err),
        }
    }
}

enum Action {
    Initialize,
    Rebuild,
    Export(ExportKind),
}

impl eframe::App for DesignerApp {
    fn update(&mut self, ctx: &egui::Context, _frame: &mut eframe::Frame) {
        self.handle_screenshot(ctx);

        // Follow the board. The watcher wakes this window when something
        // actually changes, so an idle designer draws nothing and costs
        // nothing — no ticking to ask.
        if !self.waker_set {
            let ctx = ctx.clone();
            self.backend.set_repaint_waker(std::sync::Arc::new(move || ctx.request_repaint()));
            self.waker_set = true;
        }
        if self.live && self.backend.board_changed() {
            self.refresh();
        }

        egui::TopBottomPanel::top("header").show(ctx, |ui| {
            ui.horizontal(|ui| {
                ui.heading("KiCad Enclosure Designer");
            });
            ui.horizontal_wrapped(|ui| {
                match &self.data.kicad_version {
                    Some(version) => ui.label(format!("Connected to KiCad {version}")),
                    None => ui.label(
                        RichText::new("Working from a board file (KiCad not connected)")
                            .color(Color32::from_rgb(200, 150, 40)),
                    ),
                };
            });
            if !self.data.project_dir.is_empty() {
                ui.label(RichText::new(&self.data.project_dir).weak().small());
            }
            if !self.data.board_summary.is_empty() {
                ui.label(RichText::new(&self.data.board_summary).small());
            }
            ui.add_space(4.0);
        });

        egui::TopBottomPanel::bottom("actions").show(ctx, |ui| {
            ui.add_space(4.0);
            if let Some(label) = self.busy_label {
                ui.label(format!("{label}..."));
            }
            ui.horizontal_wrapped(|ui| {
                if ui.button("Initialize Enclosure").clicked() {
                    self.run_action("Initializing", Action::Initialize);
                }
                if ui.add_enabled(self.dirty, egui::Button::new("Save settings")).clicked() {
                    self.save_if_dirty();
                }
            });
            ui.horizontal_wrapped(|ui| {
                if ui.button("Rebuild").clicked() {
                    self.save_if_dirty();
                    self.run_action("Rebuilding", Action::Rebuild);
                }
                if ui.button(ExportKind::Step.label()).clicked() {
                    self.save_if_dirty();
                    self.run_action("Exporting STEP", Action::Export(ExportKind::Step));
                }
                if ui.button(ExportKind::Stl.label()).clicked() {
                    self.save_if_dirty();
                    self.run_action("Exporting STL", Action::Export(ExportKind::Stl));
                }
                if ui.button(ExportKind::OpenScad.label()).clicked() {
                    self.save_if_dirty();
                    self.run_action("Writing OpenSCAD", Action::Export(ExportKind::OpenScad));
                }
            });
            ui.add_space(4.0);
            self.status_area(ui);
            ui.add_space(4.0);
        });

        egui::SidePanel::left("settings").resizable(true).default_width(340.0).show(ctx, |ui| {
            egui::ScrollArea::vertical().show(ui, |ui| {
                self.shell_section(ui);
                ui.separator();
                self.lid_section(ui);
                ui.separator();
                ui.separator();
                self.mounting_holes_section(ui);
                ui.separator();
                self.features_section(ui);
                ui.separator();
                self.graphics_section(ui);
                ui.separator();
                self.export_section(ui);
            });
        });

        egui::CentralPanel::default().show(ctx, |ui| {
            self.viewport_controls(ui);
            ui.separator();
            self.viewport.show(ui);
        });
    }

    fn on_exit(&mut self, gl: Option<&eframe::glow::Context>) {
        if let Some(gl) = gl {
            self.viewport.destroy(gl);
        }
    }
}

impl DesignerApp {
    /// The row above the viewport: what to show, and where to cut.
    fn viewport_controls(&mut self, ui: &mut Ui) {
        ui.horizontal_wrapped(|ui| {
            ui.label(RichText::new("Show").strong());
            for part in kicase_model::scene::PartId::ALL {
                let visible = self.viewport.visible.entry(part).or_insert(true);
                ui.checkbox(visible, part.label());
            }

            ui.separator();
            ui.checkbox(&mut self.live, "Live")
                .on_hover_text("Follow the board as you edit it in KiCad");

            let mut ortho = self.viewport.camera.projection == crate::Projection::Orthographic;
            if ui
                .checkbox(&mut ortho, "Ortho")
                .on_hover_text(
                    "Orthographic: no perspective, so edges that line up look like they line up",
                )
                .changed()
            {
                self.viewport.camera.projection = if ortho {
                    crate::Projection::Orthographic
                } else {
                    crate::Projection::Perspective
                };
            }
            if ui.button("Fit").clicked() {
                self.viewport.frame_camera();
            }
        });

        ui.horizontal_wrapped(|ui| {
            ui.checkbox(&mut self.viewport.section.enabled, "Section");
            ui.add_enabled_ui(self.viewport.section.enabled, |ui| {
                for axis in SectionAxis::ALL {
                    let selected = self.viewport.section.axis == axis;
                    if ui.selectable_label(selected, axis.label()).clicked() {
                        self.viewport.section.axis = axis;
                    }
                }
                ui.add(
                    egui::Slider::new(&mut self.viewport.section.position, 0.0..=1.0)
                        .show_value(false),
                );
                if let Some(mm) = self.viewport.section_millimetres() {
                    ui.label(RichText::new(format!("{mm:.2} mm")).weak().small());
                }
                ui.checkbox(&mut self.viewport.section.flipped, "Flip");
                ui.checkbox(&mut self.viewport.section.sweeping, "Sweep");
            });
        });

        if self.viewport.has_scene() {
            ui.label(
                RichText::new(format!(
                    "{} triangles — drag to orbit, shift-drag to pan, scroll to zoom",
                    self.viewport.triangle_count()
                ))
                .weak()
                .small(),
            );
        }
    }

    fn shell_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Shell").strong());
        let drawn_wall = self.data.drawn_wall.clone();
        let shell = &mut self.config.shell;
        let mut dirty = false;

        // Anything the user drew wins over the equivalent setting, and the
        // setting is shown as inert rather than quietly ignored.
        match &drawn_wall {
            Some(width) => from_drawing_row(ui, "Wall", &format!("{width} from the line width")),
            None => dirty |= length_row(ui, "Wall", &mut shell.wall, 0.4..=20.0),
        }
        // Everything is measured from the bottom of the case.
        dirty |= length_row(ui, "Total height", &mut shell.total_height, 1.0..=200.0);
        dirty |= length_row(ui, "PCB height", &mut shell.pcb_height, 0.1..=150.0);
        dirty |= length_row(ui, "Bottom thickness", &mut shell.floor, 0.4..=20.0);
        self.dirty |= dirty;

        if drawn_wall.is_some() {
            ui.label(
                RichText::new(
                    "The enclosure outline you drew is used at true size: the path is the                      centre of the wall and the line width is its thickness. Edit it in the                      PCB editor and rebuild.",
                )
                .weak()
                .small(),
            );
        }

        if let Some(dimensions) = &self.data.dimensions {
            ui.label(RichText::new(format!("Exterior: {dimensions}")).weak().small());
        }
    }

    fn lid_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Lid").strong());
        let lid = &mut self.config.lid;
        let mut dirty = false;
        dirty |= length_row(ui, "Top thickness", &mut lid.thickness, 0.4..=20.0);
        dirty |= length_row(ui, "Fit clearance", &mut lid.fit_clearance, 0.0..=2.0);
        dirty |= length_row(ui, "Lip depth", &mut lid.lip_depth, 0.0..=20.0);
        dirty |= length_row(ui, "Lip thickness", &mut lid.lip_thickness, 0.0..=10.0);
        self.dirty |= dirty;
    }

    /// Mounting holes are read from the board and reported, never configured:
    /// a post is a circle you draw on the solids layer.
    fn mounting_holes_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Mounting holes").strong());
        if self.data.holes.is_empty() {
            ui.label(RichText::new("No non-plated through holes detected.").weak().small());
            return;
        }
        for hole in &self.data.holes {
            ui.horizontal(|ui| {
                ui.label(&hole.id);
                ui.label(RichText::new(&hole.detail).weak().small());
            });
        }
        ui.label(
            RichText::new(
                "Draw a circle on the solids layer over a hole to stand the board on it, \
                 and a smaller circle on the bottom layer for the screw.",
            )
            .weak()
            .small(),
        );
    }

    fn features_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Features").strong());
        item_tree(ui, "Datums", &self.data.datums);
        self.cutouts_tree(ui);
        item_tree(ui, "Solids", &self.data.solids);

        if !self.data.orphans.is_empty() {
            ui.add_space(4.0);
            ui.label(
                RichText::new("Orphaned entries").strong().color(Color32::from_rgb(200, 90, 60)),
            );
            ui.label(
                RichText::new(
                    "These name KiCad objects that no longer exist. KiCase will not \
                     guess a replacement; delete them or point them at a new graphic.",
                )
                .weak()
                .small(),
            );
            let mut remove: Option<String> = None;
            for orphan in &self.data.orphans {
                ui.horizontal(|ui| {
                    ui.label(format!("{} ({})", orphan.id, orphan.detail));
                    if ui.small_button("Remove").clicked() {
                        remove = Some(orphan.id.clone());
                    }
                });
            }
            if let Some(id) = remove {
                self.config.datums.retain(|d| d.id != id);
                self.config.features.retain(|f| f.id != id);
                self.dirty = true;
                self.refresh();
            }
        }
    }

    /// Cutouts, each with the one thing a drawing cannot say: how deep it goes.
    ///
    /// Zero means "through the part it was drawn on", which is what almost
    /// every hole wants. Set it longer and the hole carries on into the other
    /// part.
    fn cutouts_tree(&mut self, ui: &mut Ui) {
        let cutouts = self.data.cutouts.clone();
        egui::CollapsingHeader::new(format!("Cutouts ({})", cutouts.len()))
            .default_open(true)
            .show(ui, |ui| {
                if cutouts.is_empty() {
                    ui.label(RichText::new("none").weak().small());
                }
                for item in &cutouts {
                    ui.horizontal(|ui| {
                        ui.label(&item.id);
                        ui.label(RichText::new(&item.detail).weak().small());
                    });
                    ui.horizontal(|ui| {
                        ui.add_space(12.0);
                        ui.label(RichText::new("depth").weak().small());

                        let existing = self
                            .config
                            .features
                            .iter()
                            .find(|f| f.uuid_matches(&item.uuid))
                            .and_then(|f| f.depth);
                        let mut millimetres =
                            existing.unwrap_or(kicase_model::DEFAULT_CUT_DEPTH).mm();
                        let response = ui.add(
                            egui::DragValue::new(&mut millimetres)
                                .speed(0.1)
                                .range(0.0..=200.0)
                                .suffix(" mm")
                                .fixed_decimals(2),
                        );
                        if response.changed() {
                            self.set_cutout_depth(&item.id, &item.uuid, millimetres);
                        }
                        ui.label(RichText::new("in from the face it is drawn on").weak().small());
                    });
                }
            });
    }

    /// Writes a depth onto a cutout, creating its entry if it had none.
    fn set_cutout_depth(&mut self, id: &str, uuid: &str, millimetres: f64) {
        let depth = Some(mm(millimetres));
        if let Some(entry) = self.config.features.iter_mut().find(|f| f.uuid_matches(uuid)) {
            entry.depth = depth;
        } else {
            self.config.features.push(FeatureConfig {
                id: id.to_string(),
                graphic_uuid: uuid.to_string(),
                datum: None,
                depth,
                clearance: Length::ZERO,
                z_start: None,
                height: None,
                enabled: true,
            });
        }
        self.dirty = true;
        self.save_if_dirty();
        self.refresh();
    }

    /// The only two bindings a drawing cannot express by itself: naming a
    /// datum, and saying which datum a side opening belongs to.
    ///
    /// Everything else needs nothing here — a shape on the top, bottom or
    /// solids layer already means what it means.
    fn graphics_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Board graphics").strong());

        let needs_binding: Vec<_> = self
            .data
            .graphics
            .iter()
            .filter(|g| {
                g.bound_to.is_none()
                    && (g.layer == "Enclosure.Datums" || g.layer == "Enclosure.Cuts")
            })
            .cloned()
            .collect();

        if needs_binding.is_empty() {
            ui.label(
                RichText::new(
                    "Nothing to bind. Shapes on the top, bottom and solids layers are used \
                     as they are drawn; only side openings need to be told which datum they \
                     belong to.",
                )
                .weak()
                .small(),
            );
            return;
        }

        let datum_names: Vec<String> = self.config.datums.iter().map(|d| d.id.clone()).collect();
        let mut added = false;

        for graphic in &needs_binding {
            ui.group(|ui| {
                ui.label(RichText::new(&graphic.description).small());
                ui.label(RichText::new(&graphic.layer).weak().small());

                let is_datum = graphic.layer == "Enclosure.Datums";
                let suggestion = if is_datum {
                    format!("datum{}", self.config.datums.len() + 1)
                } else {
                    format!("cut{}", self.config.features.len() + 1)
                };
                let id_entry = self.pending_ids.entry(graphic.uuid.clone()).or_insert(suggestion);
                ui.horizontal(|ui| {
                    ui.label("Name");
                    ui.add(egui::TextEdit::singleline(id_entry).desired_width(120.0));
                });
                let id = id_entry.clone();

                ui.horizontal_wrapped(|ui| {
                    if is_datum {
                        if ui.button("Add as datum").clicked() && !id.is_empty() {
                            self.config.datums.push(DatumConfig {
                                id: id.clone(),
                                graphic_uuid: graphic.uuid.clone(),
                                normal: DatumNormal::Auto,
                            });
                            added = true;
                        }
                        return;
                    }

                    if datum_names.is_empty() {
                        ui.label(RichText::new("Add a datum first").weak().small());
                        return;
                    }

                    let selected = self
                        .pending_datums
                        .entry(graphic.uuid.clone())
                        .or_insert_with(|| datum_names[0].clone())
                        .clone();
                    egui::ComboBox::from_id_salt(format!("datum-{}", graphic.uuid))
                        .selected_text(&selected)
                        .show_ui(ui, |ui| {
                            let entry = self
                                .pending_datums
                                .entry(graphic.uuid.clone())
                                .or_insert_with(|| datum_names[0].clone());
                            for name in &datum_names {
                                ui.selectable_value(entry, name.clone(), name);
                            }
                        });
                    if ui.button("Attach to datum").clicked() && !id.is_empty() {
                        let datum = self.pending_datums.get(&graphic.uuid).cloned();
                        self.config.features.push(FeatureConfig {
                            id: id.clone(),
                            graphic_uuid: graphic.uuid.clone(),
                            datum,
                            depth: None,
                            clearance: Length::ZERO,
                            z_start: None,
                            height: None,
                            enabled: true,
                        });
                        added = true;
                    }
                });
            });
        }

        if added {
            self.dirty = true;
            self.save_if_dirty();
            self.refresh();
        }
    }

    fn export_section(&mut self, ui: &mut Ui) {
        ui.label(RichText::new("Export").strong());
        let export = &mut self.config.export;
        let mut dirty = length_row(ui, "STL tolerance", &mut export.stl_tolerance, 0.001..=1.0);
        dirty |= ui
            .checkbox(&mut export.openscad, "Write the OpenSCAD project on every rebuild")
            .changed();
        self.dirty |= dirty;
        ui.label(
            RichText::new(
                "OpenSCAD output is a derivative for hacking. custom.scad is never \
                 overwritten, and changes there do not feed back into the STEP model.",
            )
            .weak()
            .small(),
        );
    }

    fn status_area(&mut self, ui: &mut Ui) {
        if let Some(error) = &self.error {
            ui.label(RichText::new(error).color(Color32::from_rgb(220, 80, 60)));
            return;
        }
        if !self.data.problems.is_empty() {
            for problem in &self.data.problems {
                ui.label(RichText::new(problem).color(Color32::from_rgb(200, 150, 40)).small());
            }
        }
        if let Some(report) = &self.report {
            let color = if report.ok {
                Color32::from_rgb(90, 170, 100)
            } else {
                Color32::from_rgb(220, 80, 60)
            };
            ui.label(RichText::new(&report.headline).color(color));
            for line in &report.lines {
                ui.label(RichText::new(line).weak().small());
            }
        }
    }
}

fn item_tree(ui: &mut Ui, title: &str, items: &[ItemInfoRef]) {
    egui::CollapsingHeader::new(format!("{title} ({})", items.len())).default_open(true).show(
        ui,
        |ui| {
            if items.is_empty() {
                ui.label(RichText::new("none").weak().small());
            }
            for item in items {
                ui.horizontal(|ui| {
                    let label = RichText::new(&item.id);
                    ui.label(if item.orphaned {
                        label.color(Color32::from_rgb(200, 90, 60))
                    } else {
                        label
                    });
                    ui.label(RichText::new(&item.detail).weak().small());
                });
            }
        },
    );
}

type ItemInfoRef = crate::ItemInfo;

/// Writes an egui colour image out as a PNG.
fn save_png(
    path: &std::path::Path,
    image: &egui::ColorImage,
) -> Result<(), Box<dyn std::error::Error>> {
    let file = std::fs::File::create(path)?;
    let mut encoder = png::Encoder::new(
        std::io::BufWriter::new(file),
        image.width() as u32,
        image.height() as u32,
    );
    encoder.set_color(png::ColorType::Rgba);
    encoder.set_depth(png::BitDepth::Eight);
    let mut writer = encoder.write_header()?;
    let bytes: Vec<u8> = image.pixels.iter().flat_map(|p| [p.r(), p.g(), p.b(), p.a()]).collect();
    writer.write_image_data(&bytes)?;
    Ok(())
}

/// A row showing that a setting is superseded by something the user drew.
fn from_drawing_row(ui: &mut Ui, label: &str, detail: &str) {
    ui.horizontal(|ui| {
        ui.label(label);
        ui.label(RichText::new(detail).weak());
    });
}

/// A labelled millimetre field.
fn length_row(
    ui: &mut Ui,
    label: &str,
    value: &mut Length,
    range: std::ops::RangeInclusive<f64>,
) -> bool {
    let mut millimetres = value.mm();
    let response = ui.horizontal(|ui| {
        ui.label(label);
        ui.add(
            egui::DragValue::new(&mut millimetres)
                .speed(0.05)
                .range(range)
                .suffix(" mm")
                .fixed_decimals(2),
        )
    });
    if response.inner.changed() {
        *value = mm(millimetres);
        true
    } else {
        false
    }
}
