//! How shallow a cut this backend can make, and what it does about one it
//! cannot.
//!
//! KiCad snaps a drawing to the micron, so anything a user can draw has to be
//! cut. Below that there is a floor — a cut and a touch stop being telling
//! apart — and the only thing that must never happen there is silence.

use kicase_geometry::kernel::CadKernel;
use kicase_geometry::profile::{Loop2, Profile2d};
use kicase_geometry::types::{Plane3, Point2};
use kicase_geometry::units::{mm, Length};
use kicase_truck::{TruckKernel, TruckSolid};

/// A 20 x 20 x 10 mm block standing on Z = 0.
fn block(kernel: &TruckKernel) -> TruckSolid {
    let profile =
        Profile2d::simple(Loop2::rectangle(Point2::from_mm(0.0, 0.0), Point2::from_mm(20.0, 20.0)));
    let face = kernel.make_profile(&profile, &Plane3::xy_at(Length::ZERO)).unwrap();
    kernel.extrude(&face, mm(10.0)).unwrap()
}

/// A 5 x 5 mm cutter reaching `depth` into the top of the block and well clear
/// of it above, which is how every cutter in the pipeline is shaped.
fn cutter(kernel: &TruckKernel, depth: f64) -> TruckSolid {
    let profile =
        Profile2d::simple(Loop2::rectangle(Point2::from_mm(5.0, 5.0), Point2::from_mm(10.0, 10.0)));
    let face = kernel.make_profile(&profile, &Plane3::xy_at(mm(10.0 - depth))).unwrap();
    kernel.extrude(&face, mm(5.0)).unwrap()
}

#[test]
fn a_cut_well_under_a_micron_is_made() {
    let kernel = TruckKernel::new();
    let block = block(&kernel);
    let before = kernel.volume(&block).unwrap();

    let depth = 8.0e-5;
    let cut = kernel.subtract(&block, &cutter(&kernel, depth)).expect("the cut is made");
    let removed = before - kernel.volume(&cut).unwrap();

    let expected = 5.0 * 5.0 * depth;
    assert!(
        (removed - expected).abs() / expected < 0.05,
        "removed {removed} mm^3, expected {expected}"
    );
}

/// Below the floor the two bodies stop being distinguishable from a pair that
/// merely touch. That has to be said out loud: a cut the user drew and did not
/// get is exactly the failure this backend used to hide.
#[test]
fn a_cut_below_the_floor_is_reported_rather_than_dropped() {
    let kernel = TruckKernel::new();
    let message = match kernel.subtract(&block(&kernel), &cutter(&kernel, 3.0e-6)) {
        Err(error) => error.to_string(),
        Ok(_) => panic!("a cut this shallow cannot be made, and saying nothing is not an option"),
    };
    assert!(
        message.contains("below what this kernel can resolve"),
        "the depth has to be named: {message}"
    );
}

/// The other half of the same measurement. Two bodies resting against one
/// another share no volume, and the enclosure is built out of such pairs — a
/// lid on a rim, a pocket that stops short of the far wall — so a touch must
/// stay a no-op however finely it is probed.
#[test]
fn a_body_resting_on_another_is_left_alone() {
    let kernel = TruckKernel::new();
    let block = block(&kernel);
    let before = kernel.volume(&block).unwrap();

    let resting = cutter(&kernel, 0.0);
    let cut = kernel.subtract(&block, &resting).expect("touching is not an error");
    assert!((kernel.volume(&cut).unwrap() - before).abs() < 1e-6, "the block lost material");
    assert_eq!(kernel.solid_count(&cut).unwrap(), 1);
}
