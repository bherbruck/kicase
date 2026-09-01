//! Geometry golden tests.
//!
//! Following section 32, these assert on measurable properties of the generated
//! B-rep — bounding box, volume, body count — rather than on file bytes, which
//! carry unstable kernel metadata.

use kicase_geometry::kernel::CadKernel;
use kicase_geometry::units::{mm, Length};
use kicase_model::builder::{build, EnclosureSolids};
use kicase_model::config::{DatumConfig, DatumNormal, FeatureConfig};
use kicase_model::source::BoardSource;
use kicase_model::{Enclosure, EnclosureConfig};
/// The backend under test. Every expectation in this file is about the
/// enclosure, not the kernel, so both backends have to meet all of them.
#[cfg(not(feature = "truck"))]
use kicase_occ::OccKernel as Kernel;
use kicase_tests as fixtures;
#[cfg(feature = "truck")]
use kicase_truck::TruckKernel as Kernel;

fn build_enclosure(
    config: &EnclosureConfig,
    source: &BoardSource,
) -> (Kernel, EnclosureSolids<Kernel>) {
    let kernel = Kernel::new();
    let enclosure = Enclosure::resolve(config, source).expect("model resolves");
    let solids = build(&kernel, &enclosure).expect("geometry builds");
    (kernel, solids)
}

/// Section 33: a 50 x 30 board with 1 mm clearance and 2 mm walls has
/// predictable exterior dimensions.
#[test]
fn rectangular_board_has_predictable_exterior_dimensions() {
    let mut config = EnclosureConfig::default();
    config.shell.wall = mm(2.0);

    let (kernel, solids) = build_enclosure(&config, &fixtures::rectangular_board());
    let bounds = kernel.bounds(&solids.bottom).expect("bounded");
    let size = bounds.size();

    // 50 + 2 * (1 + 2) = 56, 30 + 2 * (1 + 2) = 36.
    assert!((size.x.mm() - 56.0).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 36.0).abs() < 1e-2, "depth was {}", size.y);

    // The shell runs from the bottom of the case up to the rim: a 15 mm case
    // with a 2 mm lid leaves 13 mm of shell.
    assert!((size.z.mm() - 13.0).abs() < 1e-2, "height was {}", size.z);
    assert!(bounds.min.z.mm().abs() < 1e-2, "the case bottom is the origin: {}", bounds.min.z);
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);
    assert!(solids.warnings.is_empty(), "unexpected warnings: {:?}", solids.warnings);
}

#[test]
fn shell_volume_matches_walls_plus_floor() {
    let config = EnclosureConfig::default();

    let (kernel, solids) = build_enclosure(&config, &fixtures::rectangular_board());
    let volume = kernel.volume(&solids.bottom).expect("measurable");

    // Solid block minus the cavity: 13 mm of shell, 11 mm of cavity above a
    // 2 mm floor.
    let outer = 56.0 * 36.0 * 13.0;
    let cavity = 52.0 * 32.0 * 11.0;
    let expected = outer - cavity;
    assert!(
        (volume - expected).abs() / expected < 0.01,
        "volume was {volume}, expected about {expected}"
    );
}

/// Corners are drawn, never configured. A board with square corners must
/// produce a shell with square corners: KiCase has no corner radius setting to
/// round them behind the user's back.
#[test]
fn a_straight_box_stays_a_straight_box() {
    let config = EnclosureConfig::default();

    let (kernel, solids) = build_enclosure(&config, &fixtures::rectangular_board());
    let size = kernel.bounds(&solids.bottom).expect("bounded").size();
    assert!((size.x.mm() - 56.0).abs() < 1e-2);
    assert!((size.y.mm() - 36.0).abs() < 1e-2);

    // Square corners means the shell fills its bounding box exactly: outer
    // block minus cavity, with nothing shaved off the corners.
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    let squared = 56.0 * 36.0 * 13.0 - 52.0 * 32.0 * 11.0;
    assert!(
        (volume - squared).abs() / squared < 0.005,
        "volume was {volume}, expected the exact square-cornered {squared}"
    );
}

/// The only way to get rounded corners is to draw them.
#[test]
fn corners_are_rounded_only_when_the_outline_has_arcs() {
    let kernel = Kernel::new();
    let config = EnclosureConfig::default();

    let square = Enclosure::resolve(&config, &fixtures::rectangular_board()).expect("resolves");
    let rounded = Enclosure::resolve(&config, &fixtures::drawn_outline_board()).expect("resolves");
    assert!(!square.shell.cavity_profile.has_arcs());
    assert!(rounded.shell.cavity_profile.has_arcs());

    // And the drawn arcs survive into the generated solid.
    let solids = kicase_model::builder::build(&kernel, &rounded).expect("builds");
    let size = kernel.bounds(&solids.bottom).expect("bounded").size();
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    let boxed = size.x.mm() * size.y.mm() * size.z.mm();
    assert!(volume < boxed * 0.6, "rounded shell {volume} should be well under {boxed}");
}

/// Section 33: the shell must follow arc geometry, not a bounding box.
#[test]
fn rounded_board_shell_follows_the_curved_outline() {
    let mut config = EnclosureConfig::default();
    config.shell.wall = mm(2.0);

    let (kernel, solids) = build_enclosure(&config, &fixtures::rounded_board());
    let size = kernel.bounds(&solids.bottom).expect("bounded").size();
    assert!((size.x.mm() - 46.0).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 30.0).abs() < 1e-2, "depth was {}", size.y);

    // A box of the same footprint would be larger; the corners must be missing.
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    let boxed = 46.0 * 30.0 * 13.0 - 42.0 * 26.0 * 11.0;
    assert!(volume < boxed, "curved shell {volume} should be smaller than boxed {boxed}");
}

/// Section 33: nothing may assume a rectangular board.
#[test]
fn l_shaped_board_produces_a_non_rectangular_shell() {
    let config = EnclosureConfig::default();

    let (kernel, solids) = build_enclosure(&config, &fixtures::l_shaped_board());
    let size = kernel.bounds(&solids.bottom).expect("bounded").size();
    assert!((size.x.mm() - 66.0).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 46.0).abs() < 1e-2, "depth was {}", size.y);

    // The L notch must still be missing: a full box would be much heavier.
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    let full_box_shell = 66.0 * 46.0 * 13.0 - 62.0 * 42.0 * 11.0;
    assert!(volume < full_box_shell, "L shell {volume} vs box {full_box_shell}");
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);
}

/// Section 33: the USB opening must intersect the wall but not the floor.
#[test]
fn side_cutout_opens_the_wall_without_touching_the_floor() {
    let mut config = EnclosureConfig::default();
    // Enough headroom that the whole opening sits in the side wall rather than
    // straddling the rim, which keeps the expected volume easy to reason about.
    // A taller case, so the opening sits wholly in the side wall.
    config.shell.total_height = mm(20.0);
    config.datums.push(DatumConfig {
        id: "front".into(),
        graphic_uuid: "datum-front".into(),
        normal: DatumNormal::Auto,
    });
    config.features.push(FeatureConfig {
        id: "usb".into(),
        graphic_uuid: "cut-usb".into(),
        datum: Some("front".into()),
        depth: None,
        clearance: mm(0.3),
        z_start: None,
        height: None,
        enabled: true,
    });

    let source = fixtures::usb_cutout_board();
    let (kernel, with_cut) = build_enclosure(&config, &source);

    config.features.clear();
    config.datums.clear();
    let (_, without_cut) = build_enclosure(&config, &source);

    let cut_volume = kernel.volume(&with_cut.bottom).expect("measurable");
    let plain_volume = kernel.volume(&without_cut.bottom).expect("measurable");
    assert!(cut_volume < plain_volume, "the cutout must remove material");

    // The opening is 9.2 x 3.2 plus 0.3 clearance all round, through a 2 mm
    // wall: (9.8 * 3.8) * 2 mm^3 of material.
    //
    // The tight tolerance is deliberate. A "through" side cut must open the
    // wall its datum belongs to and stop inside the cavity; a cutter that swept
    // the full width of the box would take the opposite wall as well and remove
    // twice this much.
    let removed = plain_volume - cut_volume;
    let expected = 9.8 * 3.8 * 2.0;
    assert!(
        (removed - expected).abs() / expected < 0.05,
        "removed {removed} mm^3, expected about {expected}"
    );

    // The floor is untouched: the enclosure is still one connected body and
    // still starts at the same Z.
    assert_eq!(kernel.solid_count(&with_cut.bottom).expect("countable"), 1);
    let bounds = kernel.bounds(&with_cut.bottom).expect("bounded");
    assert!(bounds.min.z.mm().abs() < 1e-2);
    assert!(with_cut.warnings.is_empty(), "warnings: {:?}", with_cut.warnings);
}

/// A shape drawn on the top layer is a hole through the lid. Nothing has to be
/// said about it anywhere: the layer is the instruction.
#[test]
fn a_shape_on_the_top_layer_holes_the_lid() {
    let config = EnclosureConfig::default();
    let kernel = Kernel::new();

    let (_, with_cut) = build_enclosure(&config, &fixtures::top_cutout_board());
    let (_, without_cut) = build_enclosure(&config, &fixtures::rectangular_board());

    let removed = kernel.volume(&without_cut.lid).unwrap() - kernel.volume(&with_cut.lid).unwrap();
    // A 5 mm circle through a 2 mm lid.
    let expected = std::f64::consts::PI * 2.5f64.powi(2) * 2.0;
    assert!(
        (removed - expected).abs() / expected < 0.1,
        "removed {removed} mm^3 from the lid, expected about {expected}"
    );
}

/// A shallow cut is still a cut. KiCad snaps a drawing to the micron, so a
/// depth of five of them is something a user can ask for and has to get —
/// silently leaving the material there is the one answer that is never right.
#[test]
fn a_cut_only_microns_deep_is_still_made() {
    let mut source = fixtures::rectangular_board();
    source.graphics.push(kicase_model::BoardGraphic {
        uuid: kicase_model::KiCadUuid::new("engraving"),
        role: kicase_model::LayerRole::Top,
        curves: fixtures::rect_curves(20.0, 12.0, 30.0, 18.0),
        closed: true,
        stroke_width: None,
    });

    let mut config = EnclosureConfig::default();
    config.features.push(kicase_model::FeatureConfig {
        id: "engraving".into(),
        graphic_uuid: "engraving".into(),
        datum: None,
        depth: Some(mm(0.005)),
        clearance: Length::ZERO,
        z_start: None,
        height: None,
        enabled: true,
    });

    let kernel = Kernel::new();
    let (_, engraved) = build_enclosure(&config, &source);
    let (_, plain) = build_enclosure(&EnclosureConfig::default(), &fixtures::rectangular_board());

    let removed = kernel.volume(&plain.lid).unwrap() - kernel.volume(&engraved.lid).unwrap();
    let expected = 10.0 * 6.0 * 0.005;
    assert!(
        (removed - expected).abs() / expected < 0.05,
        "removed {removed} mm^3 from the lid, expected {expected}"
    );
    assert!(engraved.warnings.is_empty(), "unexpected warnings: {:?}", engraved.warnings);
}

/// A circle drawn on the solids layer, with a smaller one on the bottom layer
/// through it, is a standoff: post and screw hole, both drawn.
#[test]
fn a_drawn_circle_over_a_mounting_hole_is_a_standoff() {
    use kicase_geometry::types::Circle2;

    let mut source = fixtures::rectangular_board();
    for (index, (x, y)) in
        [(3.5, 3.5), (46.5, 3.5), (46.5, 26.5), (3.5, 26.5)].into_iter().enumerate()
    {
        source.graphics.push(kicase_model::BoardGraphic {
            uuid: kicase_model::KiCadUuid::new(format!("post-{index}")),
            role: kicase_model::LayerRole::Solids,
            curves: kicase_geometry::profile::Loop2::circle(Circle2::new(
                fixtures::point(x, y),
                mm(3.0),
            ))
            .curves()
            .to_vec(),
            closed: true,
            stroke_width: None,
        });
        source.graphics.push(kicase_model::BoardGraphic {
            uuid: kicase_model::KiCadUuid::new(format!("screw-{index}")),
            role: kicase_model::LayerRole::Bottom,
            curves: kicase_geometry::profile::Loop2::circle(Circle2::new(
                fixtures::point(x, y),
                mm(1.25),
            ))
            .curves()
            .to_vec(),
            closed: true,
            stroke_width: None,
        });
    }

    let config = EnclosureConfig::default();
    let (kernel, with_posts) = build_enclosure(&config, &source);
    let (_, without) = build_enclosure(&config, &fixtures::rectangular_board());

    let added =
        kernel.volume(&with_posts.bottom).unwrap() - kernel.volume(&without.bottom).unwrap();

    // Four posts 6 mm across, rising from the 2 mm floor to the board at 6 mm,
    // each bored 2.5 mm through the post and the floor beneath it.
    let post = std::f64::consts::PI * 3.0f64.powi(2) * 4.0;
    let bore = std::f64::consts::PI * 1.25f64.powi(2) * 6.0;
    let expected = 4.0 * (post - bore);
    assert!(
        (added - expected).abs() / expected < 0.05,
        "drawn standoffs added {added} mm^3, expected about {expected}"
    );
    assert_eq!(kernel.solid_count(&with_posts.bottom).unwrap(), 1);
}

#[test]
fn a_shape_on_the_solids_layer_adds_material() {
    let config = EnclosureConfig::default();
    let kernel = Kernel::new();

    let (_, with_rib) = build_enclosure(&config, &fixtures::rib_board());
    let (_, without_rib) = build_enclosure(&config, &fixtures::rectangular_board());

    // The default reaches from the cavity floor (2 mm) to the board (6 mm).
    let enclosure = Enclosure::resolve(&config, &fixtures::rib_board()).expect("resolves");
    let solid = &enclosure.solids[0];
    assert_eq!(solid.z_start, mm(2.0));
    assert_eq!(solid.height, mm(4.0));

    let added =
        kernel.volume(&with_rib.bottom).unwrap() - kernel.volume(&without_rib.bottom).unwrap();
    let expected = 30.0 * 2.0 * 4.0;
    assert!(
        (added - expected).abs() / expected < 0.02,
        "rib added {added} mm^3, expected {expected}"
    );
}

/// A second closed loop on the `Enclosure` layer is a wall, drawn the same way
/// the outline is: it stands on the cavity floor, runs to the rim and is
/// hollow, and it never reaches the outside of the case.
#[test]
fn an_island_on_the_enclosure_layer_is_an_internal_wall() {
    let config = EnclosureConfig::default();

    let (kernel, solids) = build_enclosure(&config, &fixtures::island_board());

    // The plain shell, plus the 14 x 10 centre line offset 1 mm each way and
    // run the 11 mm from the floor to the rim.
    let expected = 56.0 * 36.0 * 13.0 - 52.0 * 32.0 * 11.0 + (16.0 * 12.0 - 12.0 * 8.0) * 11.0;
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    assert!(
        (volume - expected).abs() / expected < 1e-3,
        "bottom was {volume} mm^3, expected {expected}"
    );

    // Welded into the floor, not left floating in the cavity.
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);

    // An interior wall stays interior: it changes nothing about the exterior.
    let bounds = kernel.bounds(&solids.bottom).expect("bounded");
    let size = bounds.size();
    assert!((size.x.mm() - 56.0).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 36.0).abs() < 1e-2, "depth was {}", size.y);
    assert!((size.z.mm() - 13.0).abs() < 1e-2, "height was {}", size.z);
    assert!(bounds.min.z.mm().abs() < 1e-2, "the case bottom is the origin: {}", bounds.min.z);

    // Nothing about a divider is drawn on the lid, so the lid stays a plain
    // plate with a lip — the same one the board without the island gets.
    let lid = kernel.volume(&solids.lid).expect("measurable");
    let plain = 56.0 * 36.0 * 2.0 + (51.6 * 31.6 - 49.2 * 29.2) * 3.0;
    assert!((lid - plain).abs() / plain < 1e-3, "lid was {lid} mm^3, expected {plain}");
    assert_eq!(kernel.solid_count(&solids.lid).expect("countable"), 1);

    assert!(solids.warnings.is_empty(), "unexpected warnings: {:?}", solids.warnings);
}

/// An interior wall drawn narrower than its own stroke has no inside left, so
/// it is a solid post. The inward offset does not refuse it — it turns inside
/// out — so boring it anyway would leave a hole nobody drew.
#[test]
fn an_island_narrower_than_its_stroke_is_a_solid_post() {
    let config = EnclosureConfig::default();

    let (kernel, solids) = build_enclosure(&config, &fixtures::post_board());
    let volume = kernel.volume(&solids.bottom).expect("measurable");

    // The plain shell plus a full 5 x 5 mm post, floor to rim.
    let expected = 56.0 * 36.0 * 13.0 - 52.0 * 32.0 * 11.0 + 5.0 * 5.0 * 11.0;
    assert!(
        (volume - expected).abs() / expected < 1e-3,
        "bottom was {volume} mm^3, expected the solid-post {expected}"
    );
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);
    assert!(solids.warnings.is_empty(), "unexpected warnings: {:?}", solids.warnings);
}

#[test]
fn lid_sits_on_the_rim_and_carries_a_lip() {
    let config = EnclosureConfig::default();

    let (kernel, solids) = build_enclosure(&config, &fixtures::rectangular_board());
    let bounds = kernel.bounds(&solids.lid).expect("bounded");

    // The lid plate spans the full exterior footprint.
    let size = bounds.size();
    assert!((size.x.mm() - 56.0).abs() < 1e-2, "lid width was {}", size.x);
    assert!((size.y.mm() - 36.0).abs() < 1e-2, "lid depth was {}", size.y);

    // It reaches from the bottom of the lip to the top of the plate:
    // 3 mm lip + 2 mm plate, with the top at the case's total height.
    assert!((size.z.mm() - 5.0).abs() < 1e-2, "lid height was {}", size.z);
    assert!((bounds.max.z.mm() - 15.0).abs() < 1e-2, "lid top was {}", bounds.max.z);
    assert!((bounds.min.z.mm() - 10.0).abs() < 1e-2, "lip bottom was {}", bounds.min.z);
    assert_eq!(kernel.solid_count(&solids.lid).expect("countable"), 1);
}

#[test]
fn the_lid_lip_clears_the_cavity_wall() {
    let mut config = EnclosureConfig::default();
    config.lid.fit_clearance = mm(0.2);

    let (kernel, solids) = build_enclosure(&config, &fixtures::rectangular_board());

    // Bottom and lid must not overlap: intersecting them yields nothing.
    let overlap = kernel.intersect(&solids.bottom, &solids.lid).expect("boolean runs");
    let volume = kernel.volume(&overlap).unwrap_or(0.0);
    assert!(volume < 0.01, "lid and shell interfere by {volume} mm^3");

    // And the check that reports it agrees, so a backend cannot pass the line
    // above by declining to answer.
    let report = kicase_model::fit::check_fit(
        &kernel,
        &Enclosure::resolve(&config, &fixtures::rectangular_board()).expect("resolves"),
        &solids.bottom,
        &solids.lid,
        &solids.cuts,
        &[],
    )
    .expect("fitment report");
    let lid = report.iter().find(|c| c.subject == "lid").expect("a lid check");
    assert_eq!(lid.status, kicase_model::fit::FitStatus::Ok, "{}", lid.message);
}

/// An island drawn inside another island is a wall inside a wall. Both survive:
/// the outer one is hollowed before the inner one is welded in, so hollowing it
/// cannot erase its neighbour. Nothing but the build order keeps that true.
#[test]
fn an_island_inside_an_island_leaves_both_walls_standing() {
    let config = EnclosureConfig::default();
    let (kernel, solids) = build_enclosure(&config, &fixtures::nested_island_board());

    // Plain shell 7904, plus a ring per island over the 11 mm cavity. The outer
    // island's centre line is 26 x 18 at 2 mm, so 28 x 20 less 24 x 16 = 176;
    // the inner is 10 x 8 at 2 mm, so 12 x 10 less 8 x 6 = 72. Lose the inner
    // wall and this is 792 mm^3 light.
    let expected = 7904.0 + (176.0 + 72.0) * 11.0;
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    assert!(
        (volume - expected).abs() / expected < 1e-3,
        "nested islands gave {volume} mm^3, expected {expected}"
    );
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);
}

/// Fitment checking has to be quick enough to sit in a rebuild, and the shape
/// that makes it slow is the ordinary one: a lid and a shell that meet all the
/// way round without interfering. Every contact between them is a place the
/// interference search has to look and never find anything, and on a rounded
/// outline there are tens of thousands of them. Left unbounded that search took
/// six minutes on this very board, which read as the whole application hanging.
#[test]
fn checking_a_rounded_case_for_interference_is_quick() {
    let config = EnclosureConfig::default();
    let source = fixtures::rounded_board();
    let (kernel, solids) = build_enclosure(&config, &source);
    let enclosure = Enclosure::resolve(&config, &source).expect("resolves");

    let started = std::time::Instant::now();
    let report = kicase_model::fit::check_fit(
        &kernel,
        &enclosure,
        &solids.bottom,
        &solids.lid,
        &solids.cuts,
        &[],
    )
    .expect("a fitment report");
    let taken = started.elapsed();

    assert!(!report.is_empty(), "the report must actually contain checks");
    // Generous next to the ~1 s this takes, and still three hundred times under
    // the six minutes it used to be: this guards the complexity, not the clock.
    assert!(taken < std::time::Duration::from_secs(30), "checking a rounded case took {taken:?}");
}

/// A lid driven into the wall must never be reported as fine. Whether the
/// kernel measures the overlap or admits it cannot, the check that names the
/// lid has to say something is up — and the one thing it may never do is cost
/// the user the whole report, which is what a propagated failure does.
#[test]
fn an_interfering_lid_is_reported_without_costing_the_rest_of_the_report() {
    let mut config = EnclosureConfig::default();
    config.lid.fit_clearance = mm(-0.4);

    let (kernel, solids) = build_enclosure(&config, &fixtures::rectangular_board());
    let enclosure = Enclosure::resolve(&config, &fixtures::rectangular_board()).expect("resolves");
    let report = kicase_model::fit::check_fit(
        &kernel,
        &enclosure,
        &solids.bottom,
        &solids.lid,
        &solids.cuts,
        &[],
    )
    .expect("a report, even when a check cannot be answered");

    let lid = report.iter().find(|c| c.subject == "lid").expect("a lid check");
    assert_ne!(
        lid.status,
        kicase_model::fit::FitStatus::Ok,
        "an interfering lid was reported as: {}",
        lid.message
    );
    // The board check still ran, which is the part that used to be lost.
    assert!(report.iter().any(|c| c.subject == "board"), "the rest of the report survived");
}

/// The drawn outline is taken completely literally: the path is the centre line
/// of the wall and the stroke width is its thickness, so the wall occupies
/// exactly the stroke KiCad renders in 2D.
#[test]
fn a_drawn_outline_sets_the_wall_thickness_from_its_stroke_width() {
    let mut config = EnclosureConfig::default();
    // A deliberately wrong setting: the drawing must win over it.
    config.shell.wall = mm(9.0);

    let source = fixtures::drawn_outline_board();
    let enclosure = Enclosure::resolve(&config, &source).expect("model resolves");

    assert!(enclosure.shell.wall_from_drawing);
    assert_eq!(enclosure.shell.wall, mm(2.5), "wall must come from the stroke width");
    assert!(enclosure.shell.cavity_profile.has_arcs(), "the drawn corners must survive");

    let kernel = Kernel::new();
    let solids = kicase_model::builder::build(&kernel, &enclosure).expect("geometry builds");
    let size = kernel.bounds(&solids.bottom).expect("bounded").size();

    // 50 x 35 centre line, 2.5 mm stroke: 1.25 mm of wall either side.
    assert!((size.x.mm() - 52.5).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 37.5).abs() < 1e-2, "depth was {}", size.y);
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);
}

#[test]
fn the_cavity_of_a_drawn_outline_is_the_inside_of_the_stroke() {
    let config = EnclosureConfig::default();
    let source = fixtures::drawn_outline_board();
    let enclosure = Enclosure::resolve(&config, &source).expect("model resolves");
    let kernel = Kernel::new();
    let solids = kicase_model::builder::build(&kernel, &enclosure).expect("geometry builds");

    // Volume of the walls plus the floor of a 52.5 x 37.5 shell whose cavity is
    // 47.5 x 32.5. Rounded corners make it slightly less than the square case,
    // so compare against that as an upper bound and check it is close.
    let volume = kernel.volume(&solids.bottom).expect("measurable");
    let squared = 52.5 * 37.5 * 13.0 - 47.5 * 32.5 * 11.0;
    assert!(volume < squared, "rounded shell {volume} should be under the square case {squared}");
    assert!(
        volume > squared * 0.9,
        "rounded shell {volume} is implausibly small next to {squared}"
    );
}

/// Each drawn segment carries its own thickness: draw one wall fatter and only
/// that wall gets fatter.
#[test]
fn every_segment_keeps_the_width_it_was_drawn_with() {
    let mut source = fixtures::rectangular_board();
    // The first segment is the front edge, running along y = -2.
    source.enclosure_outline_widths[0] = mm(6.0);

    let config = EnclosureConfig::default();
    let enclosure = Enclosure::resolve(&config, &source).expect("model resolves");

    // The model kept both widths rather than collapsing them to one.
    assert!(enclosure.shell.wall_widths.iter().any(|w| *w == mm(6.0)));
    assert!(enclosure.shell.wall_widths.iter().any(|w| *w == mm(2.0)));

    let kernel = Kernel::new();
    let solids = kicase_model::builder::build(&kernel, &enclosure).expect("geometry builds");
    let bounds = kernel.bounds(&solids.bottom).expect("bounded");

    // The 6 mm front wall reaches 3 mm below its centre line at y = -2, while
    // the other three walls still reach only 1 mm beyond theirs.
    assert!((bounds.min.y.mm() + 5.0).abs() < 1e-2, "front edge was at {}", bounds.min.y);
    assert!((bounds.max.y.mm() - 33.0).abs() < 1e-2, "back edge was at {}", bounds.max.y);
    assert!((bounds.min.x.mm() + 3.0).abs() < 1e-2, "left edge was at {}", bounds.min.x);
    assert!((bounds.max.x.mm() - 53.0).abs() < 1e-2, "right edge was at {}", bounds.max.x);
}

/// The viewport draws the board and both enclosure parts as separate meshes,
/// so each can be shown, hidden and sectioned on its own.
#[test]
fn the_scene_carries_every_part_where_it_belongs() {
    let config = EnclosureConfig::default();
    let source = fixtures::rectangular_board();
    let enclosure = Enclosure::resolve(&config, &source).expect("model resolves");
    let kernel = Kernel::new();
    let solids = kicase_model::builder::build(&kernel, &enclosure).expect("geometry builds");

    let scene =
        kicase_model::build_scene(&kernel, &enclosure, &solids, kicase_model::DISPLAY_TOLERANCE)
            .expect("scene builds");

    assert_eq!(scene.parts.len(), 3, "board plus two enclosure parts");
    for part in &scene.parts {
        assert!(!part.mesh.is_empty(), "{:?} has no triangles", part.id);
        assert_eq!(
            part.mesh.positions.len(),
            part.mesh.normals.len(),
            "{:?} normals do not match positions",
            part.id
        );
        assert!(
            part.mesh.indices.iter().all(|i| (*i as usize) < part.mesh.positions.len()),
            "{:?} has an out-of-range index",
            part.id
        );
    }

    // The board sits inside the case, at the height the settings put it.
    let pcb = scene.part(kicase_model::PartId::Pcb).expect("the board is in the scene");
    let bounds = pcb.mesh.bounds().expect("the board has bounds");
    assert!((bounds.min.z.mm() - 6.0).abs() < 0.1, "board underside at {}", bounds.min.z);
    assert!((bounds.max.z.mm() - 7.6).abs() < 0.1, "board top at {}", bounds.max.z);
    assert!((bounds.size().x.mm() - 50.0).abs() < 0.1);
    assert!((bounds.size().y.mm() - 30.0).abs() < 0.1);

    // The lid sits above the shell, not inside it.
    let lid = scene.part(kicase_model::PartId::Lid).expect("the lid is in the scene");
    assert!((lid.mesh.bounds().unwrap().max.z.mm() - 15.0).abs() < 0.1);

    // Every part is a different colour, so a section reads clearly.
    let colours: Vec<[f32; 3]> = scene.parts.iter().map(|p| p.id.color()).collect();
    assert_ne!(colours[0], colours[1]);
    assert_ne!(colours[1], colours[2]);
}

/// A hole reaches in from the face it was drawn on, by its depth. Left alone
/// that is the default 10 mm; made long enough it carries on through the lid.
#[test]
fn a_bottom_hole_reaches_as_far_as_it_is_told_to() {
    let mut source = fixtures::rectangular_board();
    source.graphics.push(kicase_model::BoardGraphic {
        uuid: kicase_model::KiCadUuid::new("vent"),
        role: kicase_model::LayerRole::Bottom,
        curves: fixtures::rect_curves(20.0, 12.0, 30.0, 18.0),
        closed: true,
        stroke_width: None,
    });

    let kernel = Kernel::new();
    let plain = EnclosureConfig::default();

    // Nothing said: the hole is the default 10 mm deep, which opens the floor
    // and anything standing on it without reaching the lid.
    assert_eq!(kicase_model::DEFAULT_CUT_DEPTH, mm(10.0));
    let (_, shallow) = build_enclosure(&plain, &source);
    let (_, none) = build_enclosure(&plain, &fixtures::rectangular_board());
    let lid_loss = kernel.volume(&none.lid).unwrap() - kernel.volume(&shallow.lid).unwrap();
    assert!(lid_loss.abs() < 0.01, "the lid should be untouched, lost {lid_loss}");
    let floor_loss = kernel.volume(&none.bottom).unwrap() - kernel.volume(&shallow.bottom).unwrap();
    assert!((floor_loss - 10.0 * 6.0 * 2.0).abs() < 1.0, "floor lost {floor_loss}");

    // Long enough to reach the lid: now it goes all the way through.
    let mut deep = EnclosureConfig::default();
    deep.features.push(kicase_model::FeatureConfig {
        id: "vent".into(),
        graphic_uuid: "vent".into(),
        datum: None,
        depth: Some(mm(20.0)),
        clearance: Length::ZERO,
        z_start: None,
        height: None,
        enabled: true,
    });
    let (_, through) = build_enclosure(&deep, &source);
    let deep_lid_loss = kernel.volume(&none.lid).unwrap() - kernel.volume(&through.lid).unwrap();
    assert!(
        (deep_lid_loss - 10.0 * 6.0 * 2.0).abs() < 1.0,
        "a 20 mm hole from the bottom should exit the lid, lost {deep_lid_loss}"
    );
}

/// A wall of lines and tangent arcs must produce a real shell.
///
/// Regression: strokes that only *touch* at their shared endpoints union into
/// an invalid solid, and every boolean after that returns nonsense — the shell
/// came out completely empty while the lid looked fine.
#[test]
fn a_rounded_wall_produces_a_solid_shell() {
    use kicase_geometry::profile::Loop2;

    // A 60 x 40 centre line with 8.5 mm corners, drawn 1.5 mm wide: lines and
    // arcs meeting tangentially all the way round.
    let outline = fixtures::rounded_rect_curves_at(-5.0, -5.0, 60.0, 40.0, 8.5);
    let source = kicase_model::BoardSource {
        board_outline: Loop2::rectangle(fixtures::point(0.0, 0.0), fixtures::point(50.0, 30.0))
            .curves()
            .to_vec(),
        enclosure_outline_widths: vec![mm(1.5); outline.len()],
        enclosure_outline: outline,
        ..kicase_model::BoardSource::default()
    };

    let config = EnclosureConfig::default();
    let (kernel, solids) = build_enclosure(&config, &source);

    let volume = kernel.volume(&solids.bottom).expect("the shell has a volume");
    assert!(volume > 0.0, "the shell came out empty");
    assert_eq!(kernel.solid_count(&solids.bottom).expect("countable"), 1);

    let size = kernel.bounds(&solids.bottom).expect("bounded").size();
    // 60 x 40 centre line plus half of the 1.5 mm wall either side.
    assert!((size.x.mm() - 61.5).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 41.5).abs() < 1e-2, "depth was {}", size.y);
    assert!((size.z.mm() - 13.0).abs() < 1e-2, "height was {}", size.z);

    // And the shell is hollow: nothing like a solid block.
    let solid_block = 61.5 * 41.5 * 13.0;
    assert!(volume < solid_block * 0.5, "volume {volume} looks solid, not a shell");
}

/// Depth works on a side opening too: without one it goes through the wall,
/// with a short one it stops partway and leaves a pocket.
#[test]
fn a_side_opening_can_be_a_blind_pocket() {
    let mut config = EnclosureConfig::default();
    config.datums.push(DatumConfig {
        id: "front".into(),
        graphic_uuid: "datum-front".into(),
        normal: DatumNormal::Auto,
    });
    let entry = |depth| kicase_model::FeatureConfig {
        id: "usb".into(),
        graphic_uuid: "cut-usb".into(),
        datum: Some("front".into()),
        depth,
        clearance: Length::ZERO,
        z_start: None,
        height: None,
        enabled: true,
    };

    let source = fixtures::usb_cutout_board();
    let kernel = Kernel::new();

    // No depth: through the 2 mm wall.
    config.features = vec![entry(None)];
    let (_, through) = build_enclosure(&config, &source);

    // Half a millimetre: a shallow pocket, nothing like the full wall.
    config.features = vec![entry(Some(mm(0.5)))];
    let (_, pocket) = build_enclosure(&config, &source);

    config.features.clear();
    let (_, plain) = build_enclosure(&config, &source);

    let plain_volume = kernel.volume(&plain.bottom).unwrap();
    let through_removed = plain_volume - kernel.volume(&through.bottom).unwrap();
    let pocket_removed = plain_volume - kernel.volume(&pocket.bottom).unwrap();

    // 9.2 x 3.2 through a 2 mm wall.
    let expected_through = 9.2 * 3.2 * 2.0;
    assert!(
        (through_removed - expected_through).abs() / expected_through < 0.05,
        "through cut removed {through_removed}, expected about {expected_through}"
    );

    // A quarter of the depth removes roughly a quarter of the material.
    let expected_pocket = 9.2 * 3.2 * 0.5;
    assert!(
        (pocket_removed - expected_pocket).abs() / expected_pocket < 0.15,
        "pocket removed {pocket_removed}, expected about {expected_pocket}"
    );
    assert!(pocket_removed < through_removed, "a pocket must remove less than a through cut");
}
