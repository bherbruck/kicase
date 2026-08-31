//! Arcs are not optional: every drawn corner radius and every drawn hole is one.

use kicase_geometry::kernel::CadKernel;
use kicase_geometry::profile::{Loop2, Profile2d};
use kicase_geometry::types::{Circle2, Plane3, Point2};
use kicase_geometry::units::{mm, Length};
use kicase_truck::TruckKernel;

fn rect(x: f64, y: f64, w: f64, d: f64) -> Loop2 {
    Loop2::rectangle(Point2::from_mm(x, y), Point2::from_mm(x + w, y + d))
}

#[test]
fn a_drawn_circle_extrudes_to_a_cylinder() {
    let k = TruckKernel::new();
    let disc = Loop2::circle(Circle2::new(Point2::from_mm(0.0, 0.0), mm(5.0)));
    let profile = k.make_profile(&Profile2d::simple(disc), &Plane3::xy_at(Length::ZERO)).unwrap();
    let solid = k.extrude(&profile, mm(10.0)).unwrap();
    let volume = k.volume(&solid).unwrap();
    let expected = std::f64::consts::PI * 25.0 * 10.0;
    assert!((volume - expected).abs() / expected < 0.02, "volume {volume}, expected {expected}");
}

#[test]
fn a_cylinder_bores_a_hole_in_a_block() {
    let k = TruckKernel::new();
    let base = Plane3::xy_at(Length::ZERO);
    let block = k
        .extrude(
            &k.make_profile(&Profile2d::simple(rect(-10.0, -10.0, 20.0, 20.0)), &base).unwrap(),
            mm(5.0),
        )
        .unwrap();
    let disc = Loop2::circle(Circle2::new(Point2::from_mm(0.0, 0.0), mm(3.0)));
    let drill = k
        .extrude(
            &k.make_profile(&Profile2d::simple(disc), &Plane3::xy_at(mm(-1.0))).unwrap(),
            mm(7.0),
        )
        .unwrap();
    let bored = k.subtract(&block, &drill).expect("bore");
    let volume = k.volume(&bored).unwrap();
    let expected = 400.0 * 5.0 - std::f64::consts::PI * 9.0 * 5.0;
    assert!((volume - expected).abs() / expected < 0.02, "volume {volume}, expected {expected}");
}

#[test]
fn a_rounded_rectangle_hollows_out() {
    let k = TruckKernel::new();
    let base = Plane3::xy_at(Length::ZERO);
    let outer =
        Loop2::rounded_rectangle(Point2::from_mm(0.0, 0.0), Point2::from_mm(40.0, 30.0), mm(4.0));
    let inner =
        Loop2::rounded_rectangle(Point2::from_mm(2.0, 2.0), Point2::from_mm(38.0, 28.0), mm(2.0));
    let body =
        k.extrude(&k.make_profile(&Profile2d::simple(outer), &base).unwrap(), mm(10.0)).unwrap();
    let void = k
        .extrude(
            &k.make_profile(&Profile2d::simple(inner), &Plane3::xy_at(mm(2.0))).unwrap(),
            mm(12.0),
        )
        .unwrap();
    let shell = k.subtract(&body, &void).expect("hollow");

    // 40x30 r4 is 1186.27 mm^2 over 10 mm; the 36x26 r2 void takes 932.57 mm^2
    // over the 8 mm of it that lies inside.
    let volume = k.volume(&shell).unwrap();
    let expected = 1186.27 * 10.0 - 932.57 * 8.0;
    assert!((volume - expected).abs() / expected < 0.005, "volume {volume}, expected {expected}");
}

#[test]
fn an_offset_rounded_outline_hollows_out() {
    // Exactly what the shell builder does: one drawn outline, offset out for
    // the footprint and in for the cavity.
    let k = TruckKernel::new();
    let base = Plane3::xy_at(Length::ZERO);
    let outline =
        Loop2::rounded_rectangle(Point2::from_mm(0.0, 0.0), Point2::from_mm(40.0, 30.0), mm(4.0));
    let half = vec![mm(1.0); outline.curves().len()];
    let inward: Vec<Length> = half.iter().map(|h| -*h).collect();
    let footprint = outline.offset_each(&half).expect("outer offset");
    let cavity = outline.offset_each(&inward).expect("inner offset");
    println!(
        "outline {} curves, footprint {} curves, cavity {} curves",
        outline.curves().len(),
        footprint.curves().len(),
        cavity.curves().len()
    );

    let body = k
        .extrude(&k.make_profile(&Profile2d::simple(footprint), &base).unwrap(), mm(10.0))
        .unwrap();
    let void = k
        .extrude(
            &k.make_profile(&Profile2d::simple(cavity), &Plane3::xy_at(mm(2.0))).unwrap(),
            mm(12.0),
        )
        .unwrap();
    let shell = k.subtract(&body, &void).expect("hollow");

    // Offsetting 40x30 r4 out by 1 gives 42x32 r5 = 1322.54 mm^2 over 10 mm;
    // offsetting in by 1 gives 38x28 r3 = 1056.27 mm^2 over the 8 mm inside.
    let volume = k.volume(&shell).unwrap();
    let expected = 1322.54 * 10.0 - 1056.27 * 8.0;
    assert!((volume - expected).abs() / expected < 0.005, "volume {volume}, expected {expected}");
}

/// STEP is the canonical artefact, so whatever surface a drawn corner ends up
/// on has to survive being written out.
#[test]
fn a_rounded_solid_writes_out_as_step() {
    let k = TruckKernel::new();
    let outline =
        Loop2::rounded_rectangle(Point2::from_mm(0.0, 0.0), Point2::from_mm(40.0, 24.0), mm(6.0));
    let solid = k
        .extrude(
            &k.make_profile(&Profile2d::simple(outline), &Plane3::xy_at(Length::ZERO)).unwrap(),
            mm(13.0),
        )
        .unwrap();

    let path = std::env::temp_dir().join("kicase-rounded-arcs.step");
    k.export_step(&solid, &path).expect("writes STEP");
    let text = std::fs::read_to_string(&path).expect("reads back");
    let _ = std::fs::remove_file(&path);
    assert!(text.contains("ISO-10303-21"), "not a STEP file");
    // One per drawn corner, at the radius it was drawn at.
    assert_eq!(text.matches("CYLINDRICAL_SURFACE").count(), 4, "the corners lost their geometry");
}
