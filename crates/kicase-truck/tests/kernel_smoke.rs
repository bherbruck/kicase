//! The truck backend against the same expectations the OpenCascade one meets.

use kicase_geometry::kernel::CadKernel;
use kicase_geometry::profile::{Loop2, Profile2d};
use kicase_geometry::types::{Plane3, Point2};
use kicase_geometry::units::mm;
use kicase_truck::TruckKernel;

fn rect(width: f64, depth: f64) -> Profile2d {
    at(0.0, 0.0, width, depth)
}

fn at(x: f64, y: f64, width: f64, depth: f64) -> Profile2d {
    let min = Point2::from_mm(x, y);
    let max = Point2::from_mm(x + width, y + depth);
    Profile2d::new(Loop2::rectangle(min, max), Vec::new())
}

#[test]
fn a_extruded_rectangle_has_the_volume_of_a_box() {
    let kernel = TruckKernel::new();
    let profile = kernel.make_profile(&rect(10.0, 20.0), &Plane3::xy_at(mm(0.0))).unwrap();
    let solid = kernel.extrude(&profile, mm(3.0)).unwrap();

    assert!((kernel.volume(&solid).unwrap() - 600.0).abs() < 0.5);
    let bounds = kernel.bounds(&solid).unwrap();
    assert!((bounds.max.z.mm() - 3.0).abs() < 0.01, "top at {:?}", bounds.max);
    assert_eq!(kernel.solid_count(&solid).unwrap(), 1);
}

/// The tool overshoots both ends, as every cutter in the pipeline does: a
/// boolean whose result depends on two faces being exactly coincident is
/// unreliable in any kernel, so KiCase never asks for one.
#[test]
fn subtracting_a_smaller_box_hollows_the_larger_one() {
    let kernel = TruckKernel::new();
    let outer = kernel
        .extrude(
            &kernel.make_profile(&rect(10.0, 10.0), &Plane3::xy_at(mm(0.0))).unwrap(),
            mm(10.0),
        )
        .unwrap();
    let inner_profile = at(2.0, 2.0, 6.0, 6.0);
    let inner = kernel
        .extrude(&kernel.make_profile(&inner_profile, &Plane3::xy_at(mm(-1.0))).unwrap(), mm(12.0))
        .unwrap();

    let hollow = kernel.subtract(&outer, &inner).unwrap();
    let volume = kernel.volume(&hollow).unwrap();
    assert!((volume - (1000.0 - 360.0)).abs() < 5.0, "volume was {volume}");
}

#[test]
fn two_separated_boxes_stay_two_bodies() {
    let kernel = TruckKernel::new();
    let a = kernel
        .extrude(&kernel.make_profile(&rect(5.0, 5.0), &Plane3::xy_at(mm(0.0))).unwrap(), mm(5.0))
        .unwrap();
    let far = at(50.0, 0.0, 5.0, 5.0);
    let b = kernel
        .extrude(&kernel.make_profile(&far, &Plane3::xy_at(mm(0.0))).unwrap(), mm(5.0))
        .unwrap();

    let both = kernel.union(&a, &b).unwrap();
    assert_eq!(kernel.solid_count(&both).unwrap(), 2);
    assert!((kernel.volume(&both).unwrap() - 250.0).abs() < 1.0);
}

#[test]
fn a_mesh_covers_the_whole_surface() {
    let kernel = TruckKernel::new();
    let solid = kernel
        .extrude(&kernel.make_profile(&rect(10.0, 10.0), &Plane3::xy_at(mm(0.0))).unwrap(), mm(4.0))
        .unwrap();

    let mesh = kernel.mesh(&solid, mm(0.1)).unwrap();
    assert!(mesh.indices.len() >= 36, "only {} indices", mesh.indices.len());
    assert_eq!(mesh.positions.len(), mesh.normals.len());
}

#[test]
fn exports_land_on_disk() {
    let kernel = TruckKernel::new();
    let solid = kernel
        .extrude(&kernel.make_profile(&rect(8.0, 8.0), &Plane3::xy_at(mm(0.0))).unwrap(), mm(2.0))
        .unwrap();
    let dir = std::env::temp_dir().join("kicase-truck-smoke");

    let step = dir.join("body.step");
    kernel.export_step(&solid, &step).unwrap();
    let text = std::fs::read_to_string(&step).unwrap();
    assert!(text.starts_with("ISO-10303-21;"), "not a STEP file");

    let stl = dir.join("body.stl");
    kernel.export_stl(&solid, &stl, mm(0.1)).unwrap();
    assert!(std::fs::metadata(&stl).unwrap().len() > 84);
}

#[test]
fn a_profile_lands_where_its_plane_puts_it() {
    let kernel = TruckKernel::new();
    let plane = Plane3::xy_at(mm(5.0));
    let solid =
        kernel.extrude(&kernel.make_profile(&rect(4.0, 4.0), &plane).unwrap(), mm(2.0)).unwrap();

    let bounds = kernel.bounds(&solid).unwrap();
    assert!((bounds.min.z.mm() - 5.0).abs() < 0.01, "bottom at {:?}", bounds.min);
    assert!((bounds.max.z.mm() - 7.0).abs() < 0.01, "top at {:?}", bounds.max);
}
