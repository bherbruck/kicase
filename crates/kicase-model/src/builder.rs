//! The build pipeline: semantic enclosure to solids.
//!
//! Generic over [`CadKernel`], so the whole pipeline is backend-independent.
//! Failures that only affect one feature are downgraded to warnings so the rest
//! of the enclosure still gets built — a bad cutout must not cost the user
//! their whole model.

use crate::config::CutFace;
use crate::error::{ModelError, Result, Warning};
use crate::fit::CutRecord;
use crate::model::{CutPlacement, Cutout, Enclosure};
use kicase_geometry::error::GeometryError;
use kicase_geometry::kernel::CadKernel;
use kicase_geometry::profile::{Loop2, Profile2d};
use kicase_geometry::types::{Plane3, Transform3d, Vector3};
use kicase_geometry::units::{mm, Length};

/// Extra sweep length added to cutters so booleans never rely on coincident
/// faces.
const OVERSHOOT: Length = mm(5.0);

/// The generated solids.
pub struct EnclosureSolids<K: CadKernel> {
    pub bottom: K::Solid,
    pub lid: K::Solid,
    /// Non-fatal problems collected during the build.
    pub warnings: Vec<Warning>,
    /// How much material each cutout actually removed. Measured here, while
    /// both the before and after solids exist, so fitment checking never has to
    /// rebuild anything.
    pub cuts: Vec<CutRecord>,
}

/// The bottom shell and the lid before any feature is applied to them.
pub struct ShellSolids<K: CadKernel> {
    pub bottom: K::Solid,
    pub lid: K::Solid,
    /// Problems from building the shell itself, without the model's own.
    pub warnings: Vec<Warning>,
}

/// What one cutout left behind.
pub struct CutResult<K: CadKernel> {
    pub bottom: K::Solid,
    pub lid: K::Solid,
    pub warnings: Vec<Warning>,
    /// False when the cutter could not be built at all, so nothing was removed
    /// and there is nothing to measure.
    pub cut: bool,
}

/// Builds the bottom shell and the lid, measuring every cut on the way past.
///
/// This is the composition of the steps below, and it is the whole of what they
/// are for: a caller that already holds one of them — the designer window,
/// which rebuilds whenever the board moves — can keep it and replay only what
/// changed. Composing them here rather than restating the order somewhere else
/// is what keeps such a caller honest.
pub fn build<K: CadKernel>(kernel: &K, enclosure: &Enclosure) -> Result<EnclosureSolids<K>> {
    let shell = build_shell(kernel, enclosure)?;
    let (mut bottom, mut lid) = (shell.bottom, shell.lid);
    let mut warnings = enclosure.warnings.clone();
    warnings.extend(shell.warnings);

    for solid in &enclosure.solids {
        let (updated, warning) = apply_solid(kernel, &bottom, solid, enclosure)?;
        bottom = updated;
        warnings.extend(warning);
    }

    let margin = cavity_margin(enclosure);
    let mut cuts: Vec<CutRecord> = Vec::new();
    // Carried across iterations: what one cutout leaves behind is what the next
    // one starts from, and measuring a solid is not cheap.
    let (mut before_bottom, mut before_lid) = if enclosure.cutouts.is_empty() {
        (0.0, 0.0)
    } else {
        (kernel.volume(&bottom).unwrap_or(0.0), kernel.volume(&lid).unwrap_or(0.0))
    };
    for cutout in &enclosure.cutouts {
        let result = apply_cutout(kernel, &bottom, &lid, cutout, enclosure, margin)?;
        (bottom, lid) = (result.bottom, result.lid);
        warnings.extend(result.warnings);
        if !result.cut {
            continue;
        }
        let after_bottom = kernel.volume(&bottom).unwrap_or(0.0);
        let after_lid = kernel.volume(&lid).unwrap_or(0.0);
        cuts.push(CutRecord {
            id: cutout.id.clone(),
            removed_from_bottom: (before_bottom - after_bottom).max(0.0),
            removed_from_lid: (before_lid - after_lid).max(0.0),
        });
        (before_bottom, before_lid) = (after_bottom, after_lid);
    }

    Ok(EnclosureSolids { bottom, lid, warnings, cuts })
}

/// The two parts as the drawn outline alone describes them.
///
/// This is the expensive half of a build and the half that depends on nothing
/// a user moves feature by feature: it is settled entirely by
/// [`Shell`](crate::model::Shell), [`Lid`](crate::model::Lid) and
/// [`ZLayout`](crate::model::ZLayout).
pub fn build_shell<K: CadKernel>(kernel: &K, enclosure: &Enclosure) -> Result<ShellSolids<K>> {
    let mut warnings = Vec::new();
    let layout = enclosure.layout;
    let shell = &enclosure.shell;

    GeometryError::require_positive("wall thickness", shell.wall)?;
    if !layout.cavity_height().is_positive() {
        return Err(ModelError::NonPositive {
            name: "cavity height",
            value: layout.cavity_height().mm(),
        });
    }

    // Everything is modelled on the XY plane at Z = 0 and then translated into
    // place, so that a single kernel profile can be reused at several heights.
    let base = Plane3::xy_at(Length::ZERO);

    // --- outlines ---------------------------------------------------------
    // The drawn outline is the centre line of the wall. Offsetting it by half
    // the line width each way gives the two faces of the wall directly, per
    // segment, so a segment drawn thicker is thicker only along that stretch.
    // Doing it here rather than as a union of stroked prisms keeps the whole
    // shell to a single extrusion and one subtraction.
    let outline = &shell.cavity_profile.outer;
    let half: Vec<Length> = (0..outline.curves().len())
        .map(|index| shell.wall_widths.get(index).copied().unwrap_or(shell.wall) / 2.0)
        .collect();
    let inward: Vec<Length> = half.iter().map(|h| -*h).collect();

    // Neither of these carries the islands. An island lies inside the outline
    // by construction, so it has nothing to say about the exterior, and a hole
    // in the cavity cutter is punched clean through the floor — and through the
    // lid too, since the lid plate is this same footprint. Interior walls are
    // added further down, as walls.
    let footprint_2d = Profile2d::simple(
        outline
            .offset_each(&half)
            .ok_or_else(|| GeometryError::kernel("wall", "the outline cannot carry that wall"))?,
    );
    let cavity_2d = Profile2d::simple(
        outline
            .offset_each(&inward)
            // A wall thicker than the shape it is drawn on leaves no cavity at
            // all, and the offset says so by folding through the middle rather
            // than by coming back empty.
            .filter(|cavity| cavity.stands_off(outline, smallest(&half)))
            .ok_or_else(|| {
                GeometryError::kernel("wall", "the wall is thicker than the outline it is drawn on")
            })?,
    );

    let footprint = kernel.make_profile(&footprint_2d, &base)?;
    let cavity = kernel.make_profile(&cavity_2d, &base)?;

    // --- bottom shell -----------------------------------------------------
    let body = kernel.extrude(&footprint, layout.shell_height())?;
    let body = lift(kernel, &body, layout.case_bottom)?;

    // The cavity runs past the rim so the shell is genuinely open-topped.
    let void = kernel.extrude(&cavity, layout.cavity_height() + OVERSHOOT)?;
    let void = lift(kernel, &void, layout.cavity_floor)?;
    let mut bottom = kernel.subtract(&body, &void)?;

    // --- interior walls ---------------------------------------------------
    // An island is a wall, so it is built like the outline: the centre line
    // offset half its stroke each way. It arrives as a boss welded into the
    // floor and then bored out to a ring, because a cutter with a hole in it
    // and two solids meeting face to face are the two shapes boolean engines
    // get wrong. Outermost first, since a bore sweeps its island's whole
    // interior and would take a nested divider with it.
    for island in &shell.islands {
        let result = (|| -> Result<K::Solid> {
            let half: Vec<Length> = (0..island.outline.curves().len())
                .map(|index| island.widths.get(index).copied().unwrap_or(shell.wall) / 2.0)
                .collect();
            let inward: Vec<Length> = half.iter().map(|h| -*h).collect();
            let outer = island.outline.offset_each(&half).ok_or_else(|| {
                GeometryError::kernel("island", "the interior wall cannot carry that width")
            })?;

            let sink = sink_depth(layout);
            let boss = kernel.make_profile(&Profile2d::simple(outer), &base)?;
            let boss = kernel.extrude(&boss, layout.cavity_height() + sink)?;
            let boss = lift(kernel, &boss, layout.cavity_floor - sink)?;
            let mut welded = kernel.union(&bottom, &boss)?;

            // Drawn narrower than its own stroke, the wall covers the whole
            // interior and the island is a solid post with nothing to hollow.
            let inner = island
                .outline
                .offset_each(&inward)
                .filter(|inner| inner.stands_off(&island.outline, smallest(&half)));
            if let Some(inner) = inner {
                // From the cavity floor, so the compartment floor is level with
                // the rest of it, and past the rim so the top of the ring is
                // unambiguous — the same reach the main cavity takes.
                let bore = kernel.make_profile(&Profile2d::simple(inner), &base)?;
                let bore = kernel.extrude(&bore, layout.cavity_height() + OVERSHOOT)?;
                let bore = lift(kernel, &bore, layout.cavity_floor)?;
                welded = kernel.subtract(&welded, &bore)?;
            }
            Ok(welded)
        })();
        match result {
            Ok(updated) => bottom = updated,
            Err(err) => warnings
                .push(Warning::new(format!("an interior wall could not be generated: {err}"))),
        }
    }

    // --- lid --------------------------------------------------------------
    // The lid is a plate over the shell footprint. Its lip is a boss reaching
    // up into the plate, bored out to a ring. Built this way nothing ever meets
    // face to face and no cutter has a hole in it, which are the two shapes
    // boolean engines get wrong.
    let plate = kernel.extrude(&footprint, enclosure.lid.thickness)?;
    let mut lid = lift(kernel, &plate, layout.rim)?;

    let lip = lip_profiles(enclosure);
    if lip.is_none() && enclosure.lid.lip_depth.is_positive() {
        warnings.push(Warning::new(
            "the cavity is too small for a lid lip; the lid is a plain plate".to_owned(),
        ));
    }
    if let Some((lip_outer_2d, lip_inner_2d)) = lip {
        let depth = enclosure.lid.lip_depth;
        // Half the plate, so the boss stays buried inside it.
        let weld = enclosure.lid.thickness / 2.0;

        let boss = kernel.make_profile(&Profile2d::simple(lip_outer_2d), &base)?;
        let boss = kernel.extrude(&boss, depth + weld)?;
        let boss = lift(kernel, &boss, layout.rim - depth)?;
        lid = kernel.union(&lid, &boss)?;

        let bore = kernel.make_profile(&Profile2d::simple(lip_inner_2d), &base)?;
        // Stops level with the underside of the plate: it hollows the boss
        // and leaves the plate its full thickness. That top face is buried in
        // solid material, so it is a blind pocket rather than a coincidence.
        let bore = kernel.extrude(&bore, depth + OVERSHOOT)?;
        let bore = lift(kernel, &bore, layout.rim - depth - OVERSHOOT)?;
        lid = kernel.subtract(&lid, &bore)?;
    }

    Ok(ShellSolids { bottom, lid, warnings })
}

/// Welds one drawn solid onto the bottom shell.
///
/// A solid that cannot be built costs the user that solid and nothing else: it
/// comes back as a warning beside the shell it failed to reach, not as a failed
/// build.
pub fn apply_solid<K: CadKernel>(
    kernel: &K,
    bottom: &K::Solid,
    solid: &crate::model::AddedSolid,
    enclosure: &Enclosure,
) -> Result<(K::Solid, Option<Warning>)> {
    let layout = enclosure.layout;
    let base = Plane3::xy_at(Length::ZERO);
    let result = (|| -> Result<K::Solid> {
        let profile = kernel.make_profile(&solid.profile, &base)?;
        // A solid that starts at or below the cavity floor is standing on it,
        // so sink it in: the extra length is buried in material that is already
        // there, and the union is a genuine overlap rather than two solids
        // touching on a shared face. A solid told to start lower than the floor
        // would otherwise reach out of the underside of the case, so the sink
        // stops short of it.
        let half_floor = (layout.cavity_floor - layout.case_bottom) / 2.0;
        let sunk = (solid.z_start - half_floor).max(layout.case_bottom + half_floor);
        let sink = if solid.z_start <= layout.cavity_floor && half_floor.is_positive() {
            (solid.z_start - sunk).max(Length::ZERO)
        } else {
            Length::ZERO
        };
        let extruded = kernel.extrude(&profile, solid.height + sink)?;
        let placed = lift(kernel, &extruded, solid.z_start - sink)?;
        Ok(kernel.union(bottom, &placed)?)
    })();
    match result {
        Ok(updated) => Ok((updated, None)),
        Err(err) => Ok((
            unchanged(kernel, bottom)?,
            Some(Warning::about(
                solid.uuid.as_str(),
                format!("added solid \"{}\" could not be generated: {err}", solid.id),
            )),
        )),
    }
}

/// Offers one cutout to both parts.
///
/// A cutter is a single solid applied to both, so an opening that straddles the
/// joint line stays consistent across them. Nothing selects parts by hand: how
/// far a hole goes is the only control, so a long enough hole in the bottom
/// comes out the top.
pub fn apply_cutout<K: CadKernel>(
    kernel: &K,
    bottom: &K::Solid,
    lid: &K::Solid,
    cutout: &Cutout,
    enclosure: &Enclosure,
    margin: Length,
) -> Result<CutResult<K>> {
    let mut warnings = Vec::new();
    let cutter = match build_cutter(kernel, cutout, enclosure, margin) {
        Ok(cutter) => cutter,
        Err(err) => {
            warnings.push(Warning::about(
                cutout.uuid.as_str(),
                format!("cutout \"{}\" could not be generated: {err}", cutout.id),
            ));
            return Ok(CutResult {
                bottom: unchanged(kernel, bottom)?,
                lid: unchanged(kernel, lid)?,
                warnings,
                cut: false,
            });
        },
    };

    let cut_bottom = match kernel.subtract(bottom, &cutter) {
        Ok(updated) => updated,
        Err(err) => {
            warnings.push(Warning::about(
                cutout.uuid.as_str(),
                format!("boolean cut failed near cutout \"{}\": {err}", cutout.id),
            ));
            unchanged(kernel, bottom)?
        },
    };
    let cut_lid = match kernel.subtract(lid, &cutter) {
        Ok(updated) => updated,
        Err(err) => {
            warnings.push(Warning::about(
                cutout.uuid.as_str(),
                format!("boolean cut failed on the lid near cutout \"{}\": {err}", cutout.id),
            ));
            unchanged(kernel, lid)?
        },
    };
    Ok(CutResult { bottom: cut_bottom, lid: cut_lid, warnings, cut: true })
}

/// How deep a weld standing on the cavity floor is sunk into it.
///
/// Half the floor is deep enough for the union to be a real overlap rather than
/// two solids resting on a shared face, and shallow enough that what is sunk
/// stays inside the floor instead of reaching out under it.
fn sink_depth(layout: crate::model::ZLayout) -> Length {
    let half_floor = (layout.cavity_floor - layout.case_bottom) / 2.0;
    if half_floor.is_positive() {
        half_floor
    } else {
        Length::ZERO
    }
}

/// The narrowest of a set of offsets, which is the one an inward offset has to
/// clear for the whole loop to have cleared.
fn smallest(distances: &[Length]) -> Length {
    distances.iter().copied().reduce(Length::min).unwrap_or(Length::ZERO)
}

/// The same solid, owned afresh, for a step that decided to change nothing.
fn unchanged<K: CadKernel>(kernel: &K, solid: &K::Solid) -> Result<K::Solid> {
    lift(kernel, solid, Length::ZERO)
}

/// Translates a solid along Z.
fn lift<K: CadKernel>(kernel: &K, solid: &K::Solid, z: Length) -> Result<K::Solid> {
    if z == Length::ZERO {
        // Still round-trip through the kernel so the caller always owns a fresh
        // solid, which keeps the ownership story simple.
        return Ok(kernel.transform(solid, Transform3d::IDENTITY)?);
    }
    Ok(kernel
        .transform(solid, Transform3d::translation(Vector3::new(Length::ZERO, Length::ZERO, z)))?)
}

/// The two outlines of the inset lip: the outside of the ring and the inside.
///
/// The lip is sized from the representative wall thickness rather than
/// per-segment, so on a case with mixed wall thicknesses it sits to the
/// thickest wall and leaves a little more room against thinner ones.
///
/// Offsetting happens in neutral geometry, so every backend gets the same lip
/// and none has to provide a curve-offset of its own. `None` means the cavity
/// is too small to hold one.
fn lip_profiles(enclosure: &Enclosure) -> Option<(Loop2, Loop2)> {
    let lid = &enclosure.lid;
    if !lid.lip_depth.is_positive() || !lid.lip_thickness.is_positive() {
        return None;
    }
    let centre_line = &enclosure.shell.cavity_profile.outer;
    let inset = enclosure.shell.wall / 2.0 + lid.fit_clearance;
    let outer = centre_line.offset(-inset)?;
    let inner = centre_line.offset(-(inset + lid.lip_thickness))?;
    Some((outer, inner))
}

/// How far the outside of the wall is from a datum plane, along its normal.
///
/// The datum line can be drawn anywhere near the wall it names — on the wall,
/// inside the cavity, or out past the edge of the board — so the surface a
/// pocket starts from is found by measuring, not by assuming.
fn outer_surface_distance(enclosure: &Enclosure, plane: &Plane3) -> Length {
    let normal = plane.normal().xy();
    let origin = plane.origin.xy();
    let furthest = enclosure
        .shell
        .cavity_profile
        .outer
        .to_polygon(48)
        .points
        .iter()
        .map(|p| (*p - origin).dot(normal))
        .fold(f64::NEG_INFINITY, f64::max);

    mm(furthest) + enclosure.shell.wall / 2.0
}

/// How far past the inside of a wall a "through" side cutter carries on.
///
/// Through means through the *wall the datum belongs to*, not through the whole
/// box: a cutter that ran the full width would open the opposite wall as well.
/// So the sweep stops a little way into the cavity, where there is nothing left
/// to cut — but never so deep that a small enclosure gets cut from both sides.
pub fn cavity_margin(enclosure: &Enclosure) -> Length {
    let bounds = enclosure.shell.cavity_profile.bounds();
    OVERSHOOT.min(bounds.width().min(bounds.height()) * 0.4)
}

/// Builds the solid that gets subtracted for one cutout.
fn build_cutter<K: CadKernel>(
    kernel: &K,
    cutout: &Cutout,
    enclosure: &Enclosure,
    margin: Length,
) -> Result<K::Solid> {
    match &cutout.placement {
        CutPlacement::Side { plane, .. } => {
            // Every length here is measured from the outer surface, never from
            // the datum line, because the line can be drawn anywhere near the
            // wall it names. The sweep starts clear of that surface; without a
            // depth it runs on through the wall and into the cavity, with one
            // it stops that far in and leaves a pocket.
            let normal = plane.normal();
            let start_at = outer_surface_distance(enclosure, plane) + OVERSHOOT;
            let start = Plane3::new(plane.origin + normal * start_at.mm(), plane.u, plane.v);
            let profile = make_cut_profile(kernel, &cutout.profile, &start, cutout.clearance)?;
            let reach = OVERSHOOT
                + match cutout.depth {
                    Some(depth) => depth,
                    None => enclosure.shell.wall + margin,
                };
            Ok(kernel.extrude(&profile, -reach)?)
        },
        CutPlacement::Vertical { face } => {
            let layout = enclosure.layout;
            // Left unsaid, a hole is the default depth: deep enough to clear a
            // floor and anything standing on it, shallow enough not to punch
            // out the other side of a normal case.
            let reach = cutout.depth.unwrap_or(crate::config::DEFAULT_CUT_DEPTH);
            match face {
                CutFace::Bottom => {
                    let plane = Plane3::xy_at(layout.case_bottom - OVERSHOOT);
                    let profile =
                        make_cut_profile(kernel, &cutout.profile, &plane, cutout.clearance)?;
                    Ok(kernel.extrude(&profile, reach + OVERSHOOT)?)
                },
                CutFace::Top => {
                    let plane = Plane3::xy_at(layout.lid_top + OVERSHOOT);
                    let profile =
                        make_cut_profile(kernel, &cutout.profile, &plane, cutout.clearance)?;
                    Ok(kernel.extrude(&profile, -(reach + OVERSHOOT))?)
                },
            }
        },
    }
}

fn make_cut_profile<K: CadKernel>(
    kernel: &K,
    profile: &Profile2d,
    plane: &Plane3,
    clearance: Length,
) -> Result<K::Profile> {
    // Clearance is applied to the drawn shape before it reaches the kernel.
    let grown;
    let profile = if clearance.is_positive() {
        grown = profile.offset(clearance).ok_or_else(|| {
            GeometryError::kernel("clearance", "the opening is too small for that clearance")
        })?;
        &grown
    } else {
        profile
    };
    Ok(kernel.make_profile(profile, plane)?)
}
