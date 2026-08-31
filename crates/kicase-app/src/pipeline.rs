//! Initialise, rebuild, export, validate.
//!
//! Everything a user can ask KiCase to do goes through here, so the command
//! line, the KiCad menu actions and the designer window all behave identically.

use crate::project::{Origin, Project};
use anyhow::{Context, Result};
use kicase_export::paths::ExportPaths;
use kicase_export::{
    export_openscad, export_step, export_stl, preview_footprint_for, write_preview_library,
    PREVIEW_LIBRARY, PREVIEW_REFERENCE,
};
use kicase_geometry::types::Vector3;
use kicase_geometry::units::Length;
use kicase_kicad::board::BoardReading;
use kicase_kicad::client::PreviewOutcome;
use kicase_model::builder::{build, EnclosureSolids};
use kicase_model::model::{Enclosure, Orphan};
use kicase_model::EnclosureConfig;
/// The backend the app builds with: pure Rust, so there is no C++
/// toolchain to install and nothing to cross-compile on any platform.
use kicase_truck::TruckKernel as Kernel;
use std::path::PathBuf;

/// What to write during a rebuild.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RebuildOptions {
    pub step: bool,
    pub stl: bool,
    pub openscad: bool,
    /// Create or keep the KiCad preview footprint and refresh the editor.
    pub update_kicad: bool,
}

impl Default for RebuildOptions {
    fn default() -> Self {
        RebuildOptions { step: true, stl: false, openscad: false, update_kicad: true }
    }
}

impl RebuildOptions {
    /// Everything a plain `kicase rebuild` does.
    pub fn rebuild(config: &EnclosureConfig) -> Self {
        RebuildOptions { openscad: config.export.openscad, ..RebuildOptions::default() }
    }
}

/// Result of a rebuild, for the CLI and the UI to report.
#[derive(Debug, Clone, Default)]
pub struct RebuildReport {
    pub files: Vec<PathBuf>,
    pub warnings: Vec<String>,
    pub orphans: Vec<Orphan>,
    pub notes: Vec<String>,
    /// Computed fitment results: interference and clearance, with numbers.
    pub fit: Vec<kicase_model::FitCheck>,
    pub preview: Option<PreviewOutcome>,
    pub refreshed: bool,
}

impl RebuildReport {
    pub fn is_clean(&self) -> bool {
        self.warnings.is_empty()
            && self.orphans.is_empty()
            && !self.fit.iter().any(|check| check.status.is_problem())
    }

    /// Fitment results worth showing first: anything that is not simply fine.
    pub fn fit_problems(&self) -> impl Iterator<Item = &kicase_model::FitCheck> {
        self.fit.iter().filter(|check| check.status != kicase_model::FitStatus::Ok)
    }
}

/// Sets up a project: claim layers, detect mounting holes, write the project
/// file and put the preview footprint on the board.
pub fn init(project: &mut Project) -> Result<RebuildReport> {
    let mut report = RebuildReport::default();
    let reading = project.read_board()?;

    if let Origin::Live(session) = &project.origin {
        let existing = (!project.is_new).then_some(&project.config.layers);
        let (plan, notes) = session.claim_layers(&reading, existing)?;
        project.config.layers = plan.mapping;
        report.notes.extend(notes);
    } else {
        report
            .notes
            .push("No KiCad session: keeping the layer mapping from the project file.".to_string());
    }

    // Re-read with the final layer mapping, so graphics land on the right roles.
    let reading = project.read_board()?;
    project.config.version = kicase_model::SCHEMA_VERSION;
    project.save_config()?;
    project.is_new = false;

    report.files.push(EnclosureConfig::config_path(&project.dir));
    report.notes.extend(reading.skipped.iter().cloned());

    // Lay the preview footprint down in a project-local library straight away,
    // so it can be placed before the first rebuild.
    if let Ok(enclosure) = Enclosure::resolve(&project.config, &reading.source) {
        let paths = ExportPaths::new(&project.dir);
        let footprint = preview_footprint_for(&enclosure, &ExportPaths::preview_model_references());
        report.files.extend(write_preview_library(&paths, &footprint)?);
        if !reading.footprints.iter().any(|f| f.reference.as_deref() == Some(PREVIEW_REFERENCE)) {
            report.notes.push(format!(
                "To see the enclosure in the 3D viewer, place the \"{PREVIEW_REFERENCE}\" \
                 footprint once from the \"{PREVIEW_LIBRARY}\" library (press A in the PCB \
                 editor). It can go anywhere on the board."
            ));
        }
    }

    Ok(report)
}

/// Regenerates geometry and writes the requested files.
pub fn rebuild(project: &mut Project, options: RebuildOptions) -> Result<RebuildReport> {
    let mut report = RebuildReport::default();
    let reading = project.read_board()?;
    report.notes.extend(reading.skipped.iter().cloned());

    let enclosure = Enclosure::resolve(&project.config, &reading.source)
        .context("resolving the enclosure model")?;
    report.orphans = enclosure.orphans.clone();
    report.notes.extend(enclosure.notes.iter().cloned());

    let kernel = Kernel::new();
    let solids: EnclosureSolids<Kernel> =
        build(&kernel, &enclosure).context("generating enclosure geometry")?;
    report.warnings = solids.warnings.iter().map(|w| w.to_string()).collect();
    report.fit = kicase_model::check_fit(
        &kernel,
        &enclosure,
        &solids.bottom,
        &solids.lid,
        &solids.cuts,
        &reading.source.mounting_holes,
    )
    .unwrap_or_default();

    let paths = ExportPaths::new(&project.dir);

    // Where the preview footprint sits, so the assembly can be written relative
    // to it. Without one, the assembly is written in board coordinates.
    let preview_origin = reading
        .footprints
        .iter()
        .find(|footprint| footprint.reference.as_deref() == Some(PREVIEW_REFERENCE))
        .map(|footprint| Vector3::from_2d(footprint.position, Length::ZERO))
        .unwrap_or(Vector3::ZERO);

    if options.step {
        report.files.extend(export_step(&kernel, &solids, &paths, preview_origin)?);
    }
    if options.stl {
        report.files.extend(export_stl(
            &kernel,
            &solids,
            &paths,
            project.config.export.stl_tolerance,
        )?);
    }
    if options.openscad {
        report.files.extend(export_openscad(&enclosure, &paths)?);
    }

    if options.update_kicad {
        let footprint = preview_footprint_for(&enclosure, &ExportPaths::preview_model_references());
        let placed =
            reading.footprints.iter().any(|f| f.reference.as_deref() == Some(PREVIEW_REFERENCE));

        if !placed {
            // Prepare the footprint so the user can place it in one action.
            report.files.extend(write_preview_library(&paths, &footprint)?);
        }

        if let Origin::Live(session) = &project.origin {
            if placed {
                report.preview = Some(PreviewOutcome::Preserved);
            } else {
                // Try the API first: if a future KiCad implements this command,
                // KiCase should use it without needing a change here.
                match session.ensure_preview_footprint(&footprint) {
                    Ok(outcome) => report.preview = Some(outcome),
                    Err(_) => report.notes.push(format!(
                        "KiCad 10 cannot add the preview footprint over its API, so place it \
                         once by hand: press A in the PCB editor, choose the \"{PREVIEW_LIBRARY}\" \
                         library, and place \"{PREVIEW_REFERENCE}\". It can go anywhere; KiCase \
                         lines the enclosure up with wherever you put it. Every later rebuild \
                         updates it automatically."
                    )),
                }
            }

            // RefreshEditor is not implemented by the KiCad 10.0.3 PCB editor,
            // so a failure here is expected and not worth alarming anyone with.
            report.refreshed = session.refresh_editor().is_ok();
        }

        if placed {
            report.notes.push(
                "If the 3D viewer is already open, close and reopen it (or press Alt+3 again) \
                 to reload the updated model."
                    .to_string(),
            );
        }
    }

    // Highlight anything the user needs to look at.
    let offenders: Vec<String> = enclosure
        .warnings
        .iter()
        .filter_map(|w| w.uuid.clone())
        .chain(report.orphans.iter().map(|o| o.uuid.clone()))
        .collect();
    project.select_in_kicad(offenders);

    project.save_config()?;
    Ok(report)
}

/// Checks the project without writing any geometry.
pub fn validate(project: &mut Project) -> Result<RebuildReport> {
    let mut report = RebuildReport::default();
    let reading = project.read_board()?;
    report.notes.extend(reading.skipped.iter().cloned());

    let enclosure = Enclosure::resolve(&project.config, &reading.source)?;
    report.orphans = enclosure.orphans.clone();
    report.warnings = enclosure.warnings.iter().map(|w| w.to_string()).collect();
    report.notes.extend(enclosure.notes.iter().cloned());
    Ok(report)
}

/// Builds the model and solids without writing anything, for the UI's preview
/// of dimensions and diagnostics.
pub fn build_only(
    project: &Project,
    reading: &BoardReading,
) -> Result<(Enclosure, EnclosureSolids<Kernel>)> {
    let enclosure = Enclosure::resolve(&project.config, &reading.source)?;
    let kernel = Kernel::new();
    let solids = build(&kernel, &enclosure)?;
    Ok((enclosure, solids))
}

/// One graphic on an enclosure layer, for listing and association.
#[derive(Debug, Clone, PartialEq)]
pub struct GraphicSummary {
    pub uuid: String,
    pub layer: &'static str,
    pub closed: bool,
    /// Size and position, so a user can tell two rectangles apart.
    pub description: String,
    /// The id of the project entry already bound to this graphic, if any.
    pub bound_to: Option<String>,
}

/// Lists the graphics on the enclosure layers and what they are bound to.
pub fn list_graphics(project: &Project) -> Result<Vec<GraphicSummary>> {
    let reading = project.read_board()?;
    let config = &project.config;
    Ok(reading
        .source
        .graphics
        .iter()
        .map(|graphic| {
            let uuid = graphic.uuid.to_string();
            let bound_to = config
                .datums
                .iter()
                .find(|d| d.graphic_uuid == uuid)
                .map(|d| d.id.clone())
                .or_else(|| {
                    config.features.iter().find(|f| f.graphic_uuid == uuid).map(|f| f.id.clone())
                });
            GraphicSummary {
                uuid,
                layer: match graphic.role {
                    kicase_model::LayerRole::Datums => "Enclosure.Datums",
                    kicase_model::LayerRole::Cuts => "Enclosure.Cuts",
                    kicase_model::LayerRole::Top => "Enclosure.Top",
                    kicase_model::LayerRole::Bottom => "Enclosure.Bottom",
                    kicase_model::LayerRole::Solids => "Enclosure.Solids",
                    kicase_model::LayerRole::Outline => "Enclosure",
                    kicase_model::LayerRole::BoardOutline => "Edge.Cuts",
                },
                closed: graphic.closed,
                description: describe(graphic),
                bound_to,
            }
        })
        .collect())
}

fn describe(graphic: &kicase_model::BoardGraphic) -> String {
    // Every defining point of every curve, so an arc's extent is included and
    // a circle does not read as a zero-height line.
    let mut points = Vec::new();
    for curve in &graphic.curves {
        points.push(curve.start());
        points.push(curve.end());
        if let kicase_geometry::profile::Curve2::Arc(arc) = curve {
            points.push(arc.mid);
        }
    }
    match kicase_geometry::types::Bounds2::from_points(points) {
        Some(bounds) if graphic.closed => format!(
            "{:.2} x {:.2} mm at ({:.2}, {:.2})",
            bounds.width().mm(),
            bounds.height().mm(),
            bounds.min.x.mm(),
            bounds.min.y.mm()
        ),
        Some(bounds) => format!(
            "line from ({:.2}, {:.2}) to ({:.2}, {:.2})",
            bounds.min.x.mm(),
            bounds.min.y.mm(),
            bounds.max.x.mm(),
            bounds.max.y.mm()
        ),
        None => "empty".to_string(),
    }
}

/// Binds a line on the datum layer to a named datum.
pub fn add_datum(
    project: &mut Project,
    id: &str,
    uuid: &str,
    normal: kicase_model::DatumNormal,
) -> Result<()> {
    ensure_graphic(project, uuid, false)?;
    ensure_unique_id(project, id)?;
    project.config.datums.push(kicase_model::DatumConfig {
        id: id.to_string(),
        graphic_uuid: uuid.to_string(),
        normal,
    });
    project.config.validate()?;
    project.save_config()
}

/// Attaches a drawn side opening to the datum whose wall it goes through.
///
/// Only side openings need this: a shape on the top, bottom or solids layer
/// already means what it means, and is used without any entry at all.
pub fn add_feature(project: &mut Project, entry: kicase_model::FeatureConfig) -> Result<()> {
    ensure_graphic(project, &entry.graphic_uuid, true)?;
    ensure_unique_id(project, &entry.id)?;
    project.config.features.push(entry);
    project.config.validate()?;
    project.save_config()
}

/// Removes a datum or feature entry by id.
pub fn remove_entry(project: &mut Project, id: &str) -> Result<bool> {
    let before = project.config.datums.len() + project.config.features.len();
    project.config.datums.retain(|d| d.id != id);
    project.config.features.retain(|f| f.id != id);
    if project.config.datums.len() + project.config.features.len() == before {
        return Ok(false);
    }
    project.save_config()?;
    Ok(true)
}

fn ensure_graphic(project: &Project, uuid: &str, closed: bool) -> Result<()> {
    let reading = project.read_board()?;
    let graphic = reading
        .source
        .graphic(uuid)
        .ok_or_else(|| anyhow::anyhow!("no enclosure graphic on the board has uuid {uuid}"))?;
    if closed && !graphic.closed {
        return Err(anyhow::anyhow!(
            "graphic {uuid} is not a closed shape; a cutout needs a rectangle, circle or \
             closed polygon"
        ));
    }
    if !closed && graphic.as_line().is_none() {
        return Err(anyhow::anyhow!("graphic {uuid} is not a straight line; a datum needs one"));
    }
    Ok(())
}

fn ensure_unique_id(project: &Project, id: &str) -> Result<()> {
    let taken = project.config.datums.iter().any(|d| d.id == id)
        || project.config.features.iter().any(|f| f.id == id);
    if taken {
        return Err(anyhow::anyhow!("\"{id}\" is already used by another entry"));
    }
    Ok(())
}

/// Chord tolerance used for STL when nothing else is configured.
pub const DEFAULT_STL_TOLERANCE: Length = kicase_geometry::units::mm(0.05);
