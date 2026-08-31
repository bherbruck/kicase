//! Fitment checks.
//!
//! Everything here is computed from the generated B-rep, not eyeballed in a
//! viewer: booleans for interference, arithmetic for clearances. It runs as
//! part of every rebuild so that tweaking a datum and knowing whether it fits
//! is one step, not two programs.

use crate::model::{CutPlacement, Enclosure};
use crate::source::MountingHole;
use kicase_geometry::error::Result;
use kicase_geometry::kernel::CadKernel;
use kicase_geometry::types::Plane3;
use kicase_geometry::units::{mm, Length};

/// Volume below which an intersection counts as nothing. Boolean results carry
/// a little numerical noise, and a sliver this small is not a real collision.
const NEGLIGIBLE_VOLUME: f64 = 0.01;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FitStatus {
    /// Checked and fine.
    Ok,
    /// Works, but worth knowing.
    Warning,
    /// Will not fit or will not do what was intended.
    Problem,
}

impl FitStatus {
    pub fn is_problem(self) -> bool {
        matches!(self, FitStatus::Problem)
    }

    pub fn symbol(self) -> &'static str {
        match self {
            FitStatus::Ok => "ok",
            FitStatus::Warning => "warn",
            FitStatus::Problem => "FAIL",
        }
    }
}

/// One checked fact about the enclosure, with the numbers behind it.
#[derive(Debug, Clone, PartialEq)]
pub struct FitCheck {
    /// What the check is about: a feature id, or `board` / `lid`.
    pub subject: String,
    pub status: FitStatus,
    /// Always states the measurement, so a tweak can be sized rather than
    /// guessed at.
    pub message: String,
}

impl FitCheck {
    fn new(subject: impl Into<String>, status: FitStatus, message: impl Into<String>) -> Self {
        FitCheck { subject: subject.into(), status, message: message.into() }
    }
}

impl std::fmt::Display for FitCheck {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "[{}] {}: {}", self.status.symbol(), self.subject, self.message)
    }
}

/// How much material each cutout actually removed, recorded during the build.
#[derive(Debug, Clone, PartialEq)]
pub struct CutRecord {
    pub id: String,
    pub removed_from_bottom: f64,
    pub removed_from_lid: f64,
}

/// Runs every fitment check.
pub fn check_fit<K: CadKernel>(
    kernel: &K,
    enclosure: &Enclosure,
    bottom: &K::Solid,
    lid: &K::Solid,
    cuts: &[CutRecord],
    holes: &[MountingHole],
) -> Result<Vec<FitCheck>> {
    let mut checks = Vec::new();

    checks.push(board_fits(kernel, enclosure, bottom)?);
    checks.push(lid_clears_shell(kernel, bottom, lid)?);
    checks.extend(cutout_checks(enclosure, cuts));
    checks.extend(mounting_holes_have_posts(enclosure, holes));

    Ok(checks)
}

/// Does the PCB actually fit in the cavity?
fn board_fits<K: CadKernel>(
    kernel: &K,
    enclosure: &Enclosure,
    bottom: &K::Solid,
) -> Result<FitCheck> {
    let layout = enclosure.layout;
    let board_plane = Plane3::xy_at(layout.pcb_bottom);
    let profile = kernel.make_profile(&enclosure.board_profile, &board_plane)?;
    let board = kernel.extrude(&profile, layout.pcb_top - layout.pcb_bottom)?;

    Ok(match overlap_volume(kernel, bottom, &board) {
        Ok(volume) if volume > NEGLIGIBLE_VOLUME => FitCheck::new(
            "board",
            FitStatus::Problem,
            format!(
                "the PCB overlaps the enclosure by {volume:.1} mm^3 — \
                 increase the PCB clearance, or move the outline you drew"
            ),
        ),
        Ok(_) => {
            FitCheck::new("board", FitStatus::Ok, "the PCB clears the shell and every standoff")
        },
        Err(err) => unmeasured("board", "whether the PCB clears the enclosure", err),
    })
}

/// Do the two printed parts collide with each other?
fn lid_clears_shell<K: CadKernel>(
    kernel: &K,
    bottom: &K::Solid,
    lid: &K::Solid,
) -> Result<FitCheck> {
    Ok(match overlap_volume(kernel, bottom, lid) {
        Ok(volume) if volume > NEGLIGIBLE_VOLUME => FitCheck::new(
            "lid",
            FitStatus::Problem,
            format!(
                "the lid interferes with the shell by {volume:.1} mm^3 — \
                 increase the lid fit clearance"
            ),
        ),
        Ok(_) => {
            FitCheck::new("lid", FitStatus::Ok, "the lid drops into the shell without interference")
        },
        Err(err) => unmeasured("lid", "whether the lid and the shell collide", err),
    })
}

/// How much of `a` and `b` occupy the same space.
fn overlap_volume<K: CadKernel>(
    kernel: &K,
    a: &K::Solid,
    b: &K::Solid,
) -> kicase_geometry::error::Result<f64> {
    let overlap = kernel.intersect(a, b)?;
    Ok(kernel.volume(&overlap).unwrap_or(0.0))
}

/// What to report when the kernel could not work out whether two parts collide.
///
/// A warning, not a problem, and the distinction is the point. Parts that meet
/// by design — a lid resting on a rim, a lip in its cavity — are the hardest
/// case there is for an interference test, so this fires on ordinary enclosures
/// that are perfectly fine. Calling those a failure would put a red line on
/// almost every build, and a report that always cries failure is one nobody
/// reads, which costs more safety than it buys. It stays visible, and it says
/// plainly that the answer is unknown rather than good.
///
/// Reported rather than propagated: one unanswerable check must not cost the
/// user every other check in the report.
fn unmeasured(id: &str, clause: &str, err: kicase_geometry::error::GeometryError) -> FitCheck {
    FitCheck::new(
        id,
        FitStatus::Warning,
        format!("could not work out {clause} ({err}) — check this one by eye in the 3D view"),
    )
}

/// One check per cutout, combining what it removed with where it sits.
///
/// A cutout that removes nothing is the classic result of a datum that is
/// slightly off: the opening exists in 2D but misses the wall in 3D. When it
/// does cut, the Z span against the floor and the rim is the number you are
/// chasing while nudging the datum.
fn cutout_checks(enclosure: &Enclosure, cuts: &[CutRecord]) -> Vec<FitCheck> {
    let layout = enclosure.layout;
    let mut checks = Vec::new();

    for cutout in &enclosure.cutouts {
        let record = cuts.iter().find(|r| r.id == cutout.id);
        let removed = record.map(|r| r.removed_from_bottom + r.removed_from_lid).unwrap_or(0.0);

        if removed <= NEGLIGIBLE_VOLUME {
            // Say where it went and where the wall is, so the fix is a
            // measurement rather than a guess.
            let detail = match &cutout.placement {
                CutPlacement::Side { datum: datum_id, .. } => enclosure
                    .datums
                    .iter()
                    .find(|d| &d.id == datum_id)
                    .map(|datum| {
                        let b = cutout.profile.bounds();
                        format!(
                            " It maps to z {:.2}..{:.2} mm, but the wall only spans \
                             {:.2}..{:.2} mm. Height is the distance from the datum line, \
                             so move the shape {:.2} mm closer to the line.",
                            (datum.z + b.min.y).mm(),
                            (datum.z + b.max.y).mm(),
                            layout.cavity_floor.mm(),
                            layout.rim.mm(),
                            ((datum.z + b.min.y) - layout.cavity_floor).mm().max(0.0),
                        )
                    })
                    .unwrap_or_default(),
                CutPlacement::Vertical { .. } => String::new(),
            };
            checks.push(FitCheck::new(
                &cutout.id,
                FitStatus::Problem,
                format!("removes no material — the opening misses the wall.{detail}"),
            ));
            continue;
        }

        let CutPlacement::Side { datum: datum_id, .. } = &cutout.placement else {
            let face = match (&cutout.placement, cutout.depth) {
                (CutPlacement::Vertical { face }, Some(depth)) => {
                    let from = match face {
                        crate::config::CutFace::Top => "the top",
                        crate::config::CutFace::Bottom => "the bottom",
                    };
                    format!("{depth} in from {from}")
                },
                (CutPlacement::Vertical { face: crate::config::CutFace::Top }, None) => {
                    "the lid".to_string()
                },
                _ => "the floor".to_string(),
            };
            checks.push(FitCheck::new(
                &cutout.id,
                FitStatus::Ok,
                format!("removes {removed:.1} mm^3 through {face}"),
            ));
            continue;
        };
        let Some(datum) = enclosure.datums.iter().find(|d| &d.id == datum_id) else {
            continue;
        };

        // The profile is in datum-local coordinates, so V is world Z.
        let bounds = cutout.profile.bounds();
        let z_min = datum.z + bounds.min.y - cutout.clearance;
        let z_max = datum.z + bounds.max.y + cutout.clearance;
        let span = format!("spans z {:.2} to {:.2} mm", z_min.mm(), z_max.mm());

        let check = if z_min < layout.cavity_floor {
            FitCheck::new(
                &cutout.id,
                FitStatus::Problem,
                format!(
                    "{span} and cuts into the floor by {:.2} mm — \
                     raise the datum or move the opening up",
                    (layout.cavity_floor - z_min).mm()
                ),
            )
        } else if z_max > layout.rim {
            FitCheck::new(
                &cutout.id,
                FitStatus::Warning,
                format!(
                    "{span}, reaching {:.2} mm above the rim, so the opening is split \
                     between the shell and the lid",
                    (z_max - layout.rim).mm()
                ),
            )
        } else {
            // Cutting the lid's lip is expected whenever the opening reaches up
            // into it; the lip would otherwise block the opening.
            let lip = record.map(|r| r.removed_from_lid > NEGLIGIBLE_VOLUME).unwrap_or(false);
            let lip_note = if lip { ", passing through the lid's lip as it should" } else { "" };
            FitCheck::new(
                &cutout.id,
                FitStatus::Ok,
                format!(
                    "{span}: {:.2} mm above the floor, {:.2} mm below the rim{lip_note}",
                    (z_min - layout.cavity_floor).mm(),
                    (layout.rim - z_max).mm()
                ),
            )
        };
        checks.push(check);
    }
    checks
}

/// Mounting holes on the board with nothing under them.
///
/// KiCase does not create standoffs any more — you draw them — but it still
/// knows where the board's non-plated holes are, so it can say when one has no
/// post beneath it.
fn mounting_holes_have_posts(enclosure: &Enclosure, holes: &[MountingHole]) -> Vec<FitCheck> {
    let mut checks = Vec::new();
    for hole in holes {
        if hole.drill_diameter < LIKELY_SCREW_DRILL {
            continue;
        }
        let covered = enclosure
            .solids
            .iter()
            .any(|solid| solid.profile.outer.to_polygon(32).contains(hole.position));
        let name = hole.reference.clone().unwrap_or_else(|| hole.uuid.to_string());
        if covered {
            checks.push(FitCheck::new(
                &name,
                FitStatus::Ok,
                format!("{} hole has a post under it", hole.drill_diameter),
            ));
        } else {
            checks.push(FitCheck::new(
                &name,
                FitStatus::Warning,
                format!(
                    "{} mounting hole at ({:.2}, {:.2}) has no post under it. Draw a circle \
                     on the solids layer there, and a smaller one on the bottom layer for \
                     the screw.",
                    hole.drill_diameter,
                    hole.position.x.mm(),
                    hole.position.y.mm()
                ),
            ));
        }
    }
    checks
}

/// Below this, a non-plated hole is more likely tooling than a screw.
const LIKELY_SCREW_DRILL: Length = mm(2.0);
