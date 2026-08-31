//! Backend smoke tests: the operations the shell builder depends on.

use kicase_geometry::kernel::CadKernel;
use kicase_geometry::profile::{Loop2, Profile2d};
use kicase_geometry::types::{Plane3, Point2, Point3};
use kicase_geometry::units::{mm, Length};
use kicase_occ::OccKernel;

fn rect_profile(w: f64, h: f64) -> Profile2d {
    Profile2d::simple(Loop2::rectangle(Point2::from_mm(0.0, 0.0), Point2::from_mm(w, h)))
}

#[test]
fn extrudes_a_rectangular_prism() {
    let k = OccKernel::new();
    let profile = k.make_profile(&rect_profile(50.0, 30.0), &Plane3::xy_at(Length::ZERO)).unwrap();
    let solid = k.extrude(&profile, mm(10.0)).unwrap();

    let bounds = k.bounds(&solid).unwrap();
    let size = bounds.size();
    assert!((size.x.mm() - 50.0).abs() < 1e-3, "x was {}", size.x);
    assert!((size.y.mm() - 30.0).abs() < 1e-3, "y was {}", size.y);
    assert!((size.z.mm() - 10.0).abs() < 1e-3, "z was {}", size.z);
    assert!((k.volume(&solid).unwrap() - 15_000.0).abs() < 1.0);
    assert_eq!(k.solid_count(&solid).unwrap(), 1);
}

#[test]
fn subtracting_a_cavity_leaves_walls_and_a_floor() {
    let k = OccKernel::new();
    let outer = k.make_profile(&rect_profile(54.0, 34.0), &Plane3::xy_at(Length::ZERO)).unwrap();
    let body = k.extrude(&outer, mm(12.0)).unwrap();

    let inner_2d =
        Profile2d::simple(Loop2::rectangle(Point2::from_mm(2.0, 2.0), Point2::from_mm(52.0, 32.0)));
    let inner = k.make_profile(&inner_2d, &Plane3::xy_at(mm(2.0))).unwrap();
    let cavity = k.extrude(&inner, mm(20.0)).unwrap();

    let shell = k.subtract(&body, &cavity).unwrap();

    let expected = 54.0 * 34.0 * 12.0 - 50.0 * 30.0 * 10.0;
    let volume = k.volume(&shell).unwrap();
    assert!((volume - expected).abs() < 1.0, "volume was {volume}, expected {expected}");
    assert_eq!(k.solid_count(&shell).unwrap(), 1);
}

#[test]
fn cuts_through_a_side_wall_using_a_datum_plane() {
    let k = OccKernel::new();
    let outer = k.make_profile(&rect_profile(40.0, 20.0), &Plane3::xy_at(Length::ZERO)).unwrap();
    let block = k.extrude(&outer, mm(10.0)).unwrap();

    // A datum on the y = 0 wall: u along +X, v along +Z, normal along -Y.
    // The plane sits 5 mm outside the wall so the cutter starts clear of it.
    let plane = Plane3::new(
        Point3::from_mm(0.0, -5.0, 0.0),
        kicase_geometry::types::Vector3::X,
        kicase_geometry::types::Vector3::Z,
    );
    let opening =
        Profile2d::simple(Loop2::rectangle(Point2::from_mm(10.0, 2.0), Point2::from_mm(20.0, 6.0)));
    let cutter_profile = k.make_profile(&opening, &plane).unwrap();
    // The plane normal (u x v) points along -Y, so a negative distance drives
    // the cutter into the block, deep enough to pass through the far wall.
    let cutter = k.extrude(&cutter_profile, mm(-30.0)).unwrap();
    let result = k.subtract(&block, &cutter).unwrap();

    let expected = 40.0 * 20.0 * 10.0 - 10.0 * 4.0 * 20.0;
    let volume = k.volume(&result).unwrap();
    assert!((volume - expected).abs() < 1.0, "volume was {volume}, expected {expected}");
}

#[test]
fn writes_step_and_stl_files() {
    let k = OccKernel::new();
    let profile = k.make_profile(&rect_profile(10.0, 10.0), &Plane3::xy_at(Length::ZERO)).unwrap();
    let solid = k.extrude(&profile, mm(5.0)).unwrap();

    let dir = std::env::temp_dir().join("kicase-occ-smoke");
    let step = dir.join("box.step");
    let stl = dir.join("box.stl");
    k.export_step(&solid, &step).unwrap();
    k.export_stl(&solid, &stl, mm(0.05)).unwrap();

    assert!(std::fs::metadata(&step).unwrap().len() > 0);
    assert!(std::fs::metadata(&stl).unwrap().len() > 0);
    let _ = std::fs::remove_dir_all(dir);
}

#[test]
fn meshes_a_solid_for_display() {
    let k = OccKernel::new();
    let profile = k.make_profile(&rect_profile(20.0, 10.0), &Plane3::xy_at(Length::ZERO)).unwrap();
    let solid = k.extrude(&profile, mm(5.0)).unwrap();

    let mesh = k.mesh(&solid, mm(0.05)).unwrap();
    assert!(!mesh.is_empty());
    assert_eq!(mesh.positions.len(), mesh.normals.len());
    assert_eq!(mesh.indices.len() % 3, 0);
    // A box has 6 faces, so at least 12 triangles.
    assert!(mesh.triangle_count() >= 12, "only {} triangles", mesh.triangle_count());

    // The mesh must cover the same space as the solid.
    let bounds = mesh.bounds().expect("meshed geometry has bounds");
    let size = bounds.size();
    assert!((size.x.mm() - 20.0).abs() < 0.1, "x was {}", size.x);
    assert!((size.y.mm() - 10.0).abs() < 0.1, "y was {}", size.y);
    assert!((size.z.mm() - 5.0).abs() < 0.1, "z was {}", size.z);

    // Every index must address a real vertex.
    assert!(mesh.indices.iter().all(|i| (*i as usize) < mesh.positions.len()));
}
