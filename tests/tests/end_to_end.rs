//! End-to-end tests over the example boards.
//!
//! These run the same code paths as `kicase rebuild`, reading the real
//! `.kicad_pcb` files in `examples/`. They need no KiCad instance, which is
//! exactly the point: geometry work is separable from IPC.

use kicase_app::pipeline::{self, RebuildOptions};
use kicase_app::project::Project;
use kicase_app::Kernel;
use kicase_geometry::kernel::CadKernel;
use kicase_model::{DatumNormal, EnclosureConfig};
use std::path::{Path, PathBuf};

fn example(name: &str) -> PathBuf {
    Path::new(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .expect("workspace root")
        .join("examples")
        .join(name)
        .join(format!("{name}.kicad_pcb"))
}

/// Copies an example board into a scratch directory so tests never write into
/// the repository.
fn scratch_copy(name: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix("kicase-e2e").tempdir().expect("temp dir");
    let board = dir.path().join(format!("{name}.kicad_pcb"));
    std::fs::copy(example(name), &board).expect("copy board");
    (dir, board)
}

#[test]
fn rectangular_example_rebuilds_into_step_and_stl() {
    let (_dir, board) = scratch_copy("rectangular-board");
    let mut project = Project::open_file(&board).expect("opens");

    let init = pipeline::init(&mut project).expect("initializes");
    assert!(init.files.iter().any(|f| f.ends_with("enclosure.toml")));
    // The board's four M3 holes are read from the board, not written into the
    // project file: a post is something you draw.
    let reading = project.read_board().expect("reads board");
    assert_eq!(reading.source.mounting_holes.len(), 4);

    let options = RebuildOptions { step: true, stl: true, openscad: true, update_kicad: false };
    let report = pipeline::rebuild(&mut project, options).expect("rebuilds");

    for name in ["bottom.step", "lid.step", "enclosure.step", "bottom.stl", "lid.stl"] {
        let path = project.dir.join(".enclosure/generated").join(name);
        let size = std::fs::metadata(&path).expect(name).len();
        assert!(size > 0, "{name} is empty");
    }
    assert!(project.dir.join(".enclosure/openscad/generated.scad").exists());
    assert!(project.dir.join(".enclosure/openscad/custom.scad").exists());
    assert!(report.orphans.is_empty());
    assert!(report.warnings.is_empty(), "warnings: {:?}", report.warnings);
}

#[test]
fn custom_scad_is_never_overwritten() {
    let (_dir, board) = scratch_copy("rectangular-board");
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");

    let options = RebuildOptions { step: false, stl: false, openscad: true, update_kicad: false };
    pipeline::rebuild(&mut project, options).expect("first rebuild");

    let custom = project.dir.join(".enclosure/openscad/custom.scad");
    std::fs::write(&custom, "// my own edits\n").expect("edit custom.scad");

    pipeline::rebuild(&mut project, options).expect("second rebuild");
    assert_eq!(
        std::fs::read_to_string(&custom).expect("read"),
        "// my own edits\n",
        "custom.scad must never be overwritten"
    );
    // generated.scad, by contrast, is refreshed every time.
    let generated = std::fs::read_to_string(project.dir.join(".enclosure/openscad/generated.scad"))
        .expect("read generated");
    assert!(generated.contains("module enclosure_bottom()"));
}

#[test]
fn settings_survive_closing_and_reopening_the_project() {
    let (_dir, board) = scratch_copy("rectangular-board");

    {
        let mut project = Project::open_file(&board).expect("opens");
        pipeline::init(&mut project).expect("initializes");
        project.config.shell.wall = kicase_geometry::units::mm(3.5);
        project.save_config().expect("saves");
    }

    let reopened = Project::open_file(&board).expect("reopens");
    assert_eq!(reopened.config.shell.wall, kicase_geometry::units::mm(3.5));
    assert!(!reopened.is_new);
}

#[test]
fn usb_example_cuts_the_front_wall_and_not_the_floor() {
    // This test writes its own board rather than reading examples/usb-cutout,
    // so that editing the example by hand cannot break the suite.
    let (_dir, board) = generated_board();
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");

    // Give the connector room, so the opening sits entirely in the side wall.
    // A taller case, so the opening sits wholly in the side wall.
    project.config.shell.total_height = kicase_geometry::units::mm(20.0);

    let graphics = pipeline::list_graphics(&project).expect("lists graphics");
    let datum = graphics
        .iter()
        .find(|g| g.layer == "Enclosure.Datums")
        .expect("the example has a datum line");
    let usb = graphics
        .iter()
        .find(|g| g.layer == "Enclosure.Cuts")
        .expect("the example has a USB opening");

    pipeline::add_datum(&mut project, "front", &datum.uuid, DatumNormal::Auto)
        .expect("datum binds");
    pipeline::add_feature(
        &mut project,
        kicase_model::FeatureConfig {
            id: "usb".into(),
            graphic_uuid: usb.uuid.clone(),
            datum: Some("front".to_string()),
            depth: None,
            clearance: kicase_geometry::units::mm(0.3),
            z_start: None,
            height: None,
            enabled: true,
        },
    )
    .expect("cutout binds");

    let reading = project.read_board().expect("reads board");
    let (with_cut, solids_with) = pipeline::build_only(&project, &reading).expect("builds");

    let mut without = project.config.clone();
    without.features.clear();
    let plain_project = Project { config: without, ..reopen(&board) };
    let (_, solids_without) = pipeline::build_only(&plain_project, &reading).expect("builds");

    let kernel = Kernel::new();
    let removed = kernel.volume(&solids_without.bottom).expect("volume")
        - kernel.volume(&solids_with.bottom).expect("volume");

    // 9.2 x 3.6 plus 0.3 clearance all round, through a 2 mm wall.
    let expected = 9.8 * 3.8 * 2.0;
    assert!(
        (removed - expected).abs() / expected < 0.05,
        "removed {removed} mm^3 from the wall, expected about {expected}"
    );

    // The floor is untouched: the shell is still one body reaching the same Z.
    assert_eq!(kernel.solid_count(&solids_with.bottom).expect("count"), 1);
    let bounds = kernel.bounds(&solids_with.bottom).expect("bounds");
    assert!(bounds.min.z.mm().abs() < 1e-2, "the case bottom is the origin: {}", bounds.min.z);

    // The opening is above the floor: its lowest point clears the cavity floor.
    let cutout = &with_cut.cutouts[0];
    let v_min = cutout.profile.bounds().min.y;
    assert!(v_min.mm() > 0.0, "the opening starts below its datum: {v_min}");
}

#[test]
fn nonrectangular_example_has_no_rectangle_assumptions() {
    let (_dir, board) = scratch_copy("nonrectangular-board");
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");

    let reading = project.read_board().expect("reads board");
    let (_, solids) = pipeline::build_only(&project, &reading).expect("builds");
    let kernel = Kernel::new();
    let size = kernel.bounds(&solids.bottom).expect("bounds").size();

    // 60 x 40 board, 0.75 clearance, 2 mm wall.
    assert!((size.x.mm() - 66.0).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 46.0).abs() < 1e-2, "depth was {}", size.y);
    assert_eq!(kernel.solid_count(&solids.bottom).expect("count"), 1);
}

#[test]
fn rounded_example_keeps_its_arcs() {
    let (_dir, board) = scratch_copy("rounded-board");
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");

    let reading = project.read_board().expect("reads board");
    let (enclosure, solids) = pipeline::build_only(&project, &reading).expect("builds");

    // The outline is lines and arcs, not a polygon approximation.
    let has_arc = enclosure
        .shell
        .cavity_profile
        .outer
        .curves()
        .iter()
        .any(|c| matches!(c, kicase_geometry::profile::Curve2::Arc(_)));
    assert!(has_arc, "the rounded board outline lost its arcs");

    let kernel = Kernel::new();
    let size = kernel.bounds(&solids.bottom).expect("bounds").size();
    assert!((size.x.mm() - 46.0).abs() < 1e-2, "width was {}", size.x);
    assert!((size.y.mm() - 30.0).abs() < 1e-2, "depth was {}", size.y);
}

#[test]
fn validation_reports_a_dangling_graphic_without_rebinding_it() {
    let (_dir, board) = scratch_copy("rectangular-board");
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");

    project.config.datums.push(kicase_model::DatumConfig {
        id: "ghost".into(),
        graphic_uuid: "00000000-0000-0000-0000-000000000000".into(),
        normal: DatumNormal::Auto,
    });

    let report = pipeline::validate(&mut project).expect("validates");
    assert_eq!(report.orphans.len(), 1);
    assert_eq!(report.orphans[0].id, "ghost");
    assert!(!report.is_clean());
}

/// Writes a self-contained board: a 50 x 30 PCB, a 2 mm wall drawn around it,
/// a datum along the front wall, and a USB-sized opening 7.6 mm above the case
/// bottom — the height a connector sitting on the board ends up at.
fn generated_board() -> (tempfile::TempDir, PathBuf) {
    generated_board_with("")
}

/// The same board plus whatever extra items the caller needs in it.
fn generated_board_with(extra: &str) -> (tempfile::TempDir, PathBuf) {
    let dir = tempfile::Builder::new().prefix("kicase-usb").tempdir().expect("temp dir");
    let board = dir.path().join("usb.kicad_pcb");

    let mut items = String::new();
    let mut line = |x1: f64, y1: f64, x2: f64, y2: f64, layer: &str, w: f64| {
        items.push_str(&format!(
            "(gr_line (start {x1} {y1}) (end {x2} {y2}) \
             (stroke (width {w}) (type default)) (layer \"{layer}\") \
             (uuid \"{:08x}-0000-0000-0000-000000000000\"))\n",
            (x1 * 1000.0 + y1 * 7.0 + x2 * 13.0 + y2 * 17.0) as u32
        ));
    };
    // Board outline: 50 x 30 at (100, 80).
    line(100.0, 80.0, 150.0, 80.0, "Edge.Cuts", 0.1);
    line(150.0, 80.0, 150.0, 110.0, "Edge.Cuts", 0.1);
    line(150.0, 110.0, 100.0, 110.0, "Edge.Cuts", 0.1);
    line(100.0, 110.0, 100.0, 80.0, "Edge.Cuts", 0.1);
    // Wall: 54 x 34 centre line, drawn 2 mm wide.
    line(98.0, 78.0, 152.0, 78.0, "User.1", 2.0);
    line(152.0, 78.0, 152.0, 112.0, "User.1", 2.0);
    line(152.0, 112.0, 98.0, 112.0, "User.1", 2.0);
    line(98.0, 112.0, 98.0, 78.0, "User.1", 2.0);
    // Datum along the front wall.
    line(100.0, 112.0, 150.0, 112.0, "User.2", 0.1);

    // The opening: 9.2 x 3.6, drawn 7.6 mm beyond the datum line.
    items.push_str(
        "(gr_rect (start 120.4 119.6) (end 129.6 122.8) \
         (stroke (width 0.1) (type default)) (fill no) (layer \"User.3\") \
         (uuid \"cccccccc-0000-0000-0000-000000000000\"))\n",
    );

    let text = format!(
        "(kicad_pcb (version 20250513) (generator \"kicase-tests\")\n\
         (general (thickness 1.6))\n\
         (layers (0 \"F.Cu\" signal) (2 \"B.Cu\" signal) (25 \"Edge.Cuts\" user)\n\
         (39 \"User.1\" user) (41 \"User.2\" user) (43 \"User.3\" user) (45 \"User.4\" user))\n\
         (net 0 \"\")\n{items}{extra})\n"
    );
    std::fs::write(&board, text).expect("write board");
    (dir, board)
}

/// Reopens a project from disk, used where a second independent handle is
/// needed for a comparison build.
fn reopen(board: &Path) -> Project {
    Project::open_file(board).expect("reopens")
}

fn _config_type_is_used(_: &EnclosureConfig) {}

/// The OpenSCAD derivative is an approximation, but it has to be an
/// approximation of the same part: an interior wall that exists in the STEP
/// and not in the `.scad` is a different enclosure, not a coarser one.
#[test]
fn the_openscad_derivative_carries_interior_walls() {
    let dir = tempfile::Builder::new().prefix("kicase-scad").tempdir().expect("temp dir");
    let paths = kicase_export::paths::ExportPaths::new(dir.path());
    let config = EnclosureConfig::default();

    let with_island =
        kicase_model::Enclosure::resolve(&config, &kicase_tests::island_board()).expect("resolves");
    kicase_export::export_openscad(&with_island, &paths).expect("writes the derivative");
    let scad = std::fs::read_to_string(paths.generated_scad()).expect("read generated");

    // The wall the user drew, offset 1 mm each way onto its two faces.
    assert!(scad.contains("interior_walls();"), "the bottom must build its interior walls");
    assert!(scad.contains("[17.0000, 9.0000]"), "the outside of the island wall is missing");
    assert!(scad.contains("[19.0000, 11.0000]"), "the inside of the island wall is missing");

    // And the cavity is hollowed before anything stands in it, or the wall
    // would be swept away by the same cut that makes room for it.
    let bottom = scad.split("module enclosure_bottom()").nth(1).expect("the bottom module");
    let bottom = bottom.split("\nmodule ").next().expect("just that module");
    assert!(
        bottom.find("cavity_profile();") < bottom.find("interior_walls();"),
        "the cavity must be cut before the walls are added:\n{bottom}"
    );
}

/// The designer window keeps the shell between edits and replays only the
/// features after the one that moved. Whatever it hands back has to be the same
/// enclosure a cold build produces, or the picture drifts from the STEP.
#[test]
fn an_incremental_refresh_agrees_with_a_cold_build() {
    use kicase_model::scene::PartId;
    use kicase_ui::DesignerBackend;

    let (_dir, board) = generated_board();
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");
    project.config.shell.total_height = kicase_geometry::units::mm(20.0);

    let graphics = pipeline::list_graphics(&project).expect("lists graphics");
    let datum = graphics.iter().find(|g| g.layer == "Enclosure.Datums").expect("a datum line");
    let usb = graphics.iter().find(|g| g.layer == "Enclosure.Cuts").expect("an opening");
    pipeline::add_datum(&mut project, "front", &datum.uuid, DatumNormal::Auto).expect("binds");
    pipeline::add_feature(
        &mut project,
        kicase_model::FeatureConfig {
            id: "usb".into(),
            graphic_uuid: usb.uuid.clone(),
            datum: Some("front".to_string()),
            depth: None,
            clearance: kicase_geometry::units::mm(0.3),
            z_start: None,
            height: None,
            enabled: true,
        },
    )
    .expect("cutout binds");
    let mut config = project.config.clone();

    // Warm one backend on the settings as bound, then give the opening more
    // clearance and refresh again: that second refresh keeps the shell.
    let mut warm = kicase_app::AppBackend::new(reopen(&board));
    warm.refresh(&config).expect("first refresh");
    config.features[0].clearance = kicase_geometry::units::mm(0.5);
    let warm_data = warm.refresh(&config).expect("second refresh");
    let warm_scene = warm.scene(&config).expect("scene");

    // A backend that never saw the earlier settings builds the same thing.
    let mut cold = kicase_app::AppBackend::new(reopen(&board));
    let cold_data = cold.refresh(&config).expect("refresh");
    let cold_scene = cold.scene(&config).expect("scene");

    assert_eq!(warm_data.problems, cold_data.problems, "the reports disagree");
    assert_eq!(warm_data.dimensions, cold_data.dimensions, "the exterior sizes disagree");
    for part in PartId::ALL {
        let warm = &warm_scene.part(part).expect("part").mesh;
        let cold = &cold_scene.part(part).expect("part").mesh;
        assert_eq!(warm.triangle_count(), cold.triangle_count(), "{}", part.label());
        let (warm, cold) = (enclosed_volume(warm), enclosed_volume(cold));
        assert!(
            (warm - cold).abs() <= cold.abs() * 1e-9,
            "{} came out as {warm} mm^3 warm and {cold} mm^3 cold",
            part.label()
        );
    }
}

/// Volume enclosed by a closed triangle mesh, by the divergence theorem.
///
/// A stale cache shows up here and nowhere cheaper: it leaves the part the
/// right shape and the wrong size, so triangle counts and bounding boxes both
/// agree with a build that is genuinely different.
fn enclosed_volume(mesh: &kicase_geometry::types::TriangleMesh) -> f64 {
    mesh.indices
        .as_chunks::<3>()
        .0
        .iter()
        .map(|triangle| {
            let p = |index: u32| {
                let point = mesh.positions[index as usize];
                [point.x.mm(), point.y.mm(), point.z.mm()]
            };
            let (a, b, c) = (p(triangle[0]), p(triangle[1]), p(triangle[2]));
            (a[0] * (b[1] * c[2] - b[2] * c[1]) - a[1] * (b[0] * c[2] - b[2] * c[0])
                + a[2] * (b[0] * c[1] - b[1] * c[0]))
                / 6.0
        })
        .sum::<f64>()
        .abs()
}

/// Every example in the repository has to build. They are the boards the
/// project ships as its own worked answers, and a change that refuses one of
/// them is a change that refuses a real drawing.
#[test]
fn every_example_board_builds() {
    for name in [
        "rectangular-board",
        "rounded-board",
        "drawn-outline",
        "nonrectangular-board",
        "usb-cutout",
    ] {
        let (_dir, board) = scratch_copy(name);
        let mut project = Project::open_file(&board).expect("opens");
        pipeline::init(&mut project).expect("initializes");
        let reading = project.read_board().expect("reads board");
        let (_, solids) = pipeline::build_only(&project, &reading)
            .unwrap_or_else(|error| panic!("{name} no longer builds: {error}"));

        let kernel = Kernel::new();
        let volume = kernel.volume(&solids.bottom).expect("measurable");
        assert!(volume > 0.0, "{name} built an empty shell");
        assert_eq!(kernel.solid_count(&solids.lid).expect("countable"), 1, "{name} lid");
    }
}

/// Writes a 6 x 4 x 3 mm block as a STEP file, to stand in for the model a
/// footprint would point at.
///
/// Made with the kernel rather than shipped as a fixture, so the test carries
/// no third-party file and no dependency on KiCad's model library being
/// installed.
fn write_block_model(dir: &Path) -> PathBuf {
    use kicase_geometry::profile::{Loop2, Profile2d};
    use kicase_geometry::types::{Plane3, Point2};
    use kicase_geometry::units::{mm, Length};

    let kernel = Kernel::new();
    let outline = Loop2::rectangle(Point2::from_mm(-3.0, -2.0), Point2::from_mm(3.0, 2.0));
    let profile = kernel
        .make_profile(&Profile2d::simple(outline), &Plane3::xy_at(Length::ZERO))
        .expect("profile");
    let block = kernel.extrude(&profile, mm(3.0)).expect("block");
    let path = dir.join("block.step");
    kernel.export_step(&block, &path).expect("writes the model");
    path
}

/// A footprint at KiCad (120, 95) carrying that block as its 3D model.
const MODEL_FOOTPRINT: &str = "(footprint \"test:block\" (layer \"F.Cu\") (at 120 95)\n\
     (uuid \"ffffffff-0000-0000-0000-000000000000\")\n\
     (property \"Reference\" \"U1\")\n\
     (model \"${KIPRJMOD}/block.step\" (offset (xyz 0 0 0)) (scale (xyz 1 1 1)) \
     (rotate (xyz 0 0 0)))\n)\n";

/// Everything the exports are made of, ignoring the clock.
///
/// The STEP header carries a generation timestamp, which is not geometry; the
/// rest of the file is compared byte for byte.
fn step_body(path: &Path) -> String {
    std::fs::read_to_string(path)
        .expect("read step")
        .lines()
        .filter(|line| !line.starts_with("FILE_NAME"))
        .collect::<Vec<_>>()
        .join("\n")
}

/// Components are decoration. A board that carries 3D models and the same board
/// without them must export the same enclosure, byte for byte — otherwise a
/// connector mesh has found its way into the geometry.
#[test]
fn components_never_reach_the_enclosure_export() {
    let (_plain_dir, plain) = generated_board_with("");
    let (model_dir, with_models) = generated_board_with(MODEL_FOOTPRINT);
    write_block_model(model_dir.path());

    let options = RebuildOptions { step: true, stl: true, openscad: false, update_kicad: false };
    let mut plain_project = Project::open_file(&plain).expect("opens");
    pipeline::init(&mut plain_project).expect("initializes");
    let plain_report = pipeline::rebuild(&mut plain_project, options).expect("rebuilds");

    let mut model_project = Project::open_file(&with_models).expect("opens");
    pipeline::init(&mut model_project).expect("initializes");
    let model_report = pipeline::rebuild(&mut model_project, options).expect("rebuilds");

    assert_eq!(
        plain_report.files.len(),
        model_report.files.len(),
        "the two boards wrote different sets of files"
    );
    for (plain_file, model_file) in plain_report.files.iter().zip(&model_report.files) {
        assert_eq!(
            plain_file.file_name(),
            model_file.file_name(),
            "the two boards wrote different files"
        );
        if plain_file.extension().is_some_and(|e| e == "step") {
            assert_eq!(
                step_body(plain_file),
                step_body(model_file),
                "{} differs between a board with components and one without",
                plain_file.display()
            );
        } else {
            assert_eq!(
                std::fs::read(plain_file).expect("read"),
                std::fs::read(model_file).expect("read"),
                "{} differs between a board with components and one without",
                plain_file.display()
            );
        }
    }
}

/// The whole point of the feature: a footprint's model is read, placed and
/// drawn, and it lands where the footprint is rather than at the origin.
#[test]
fn a_footprint_model_is_loaded_placed_and_drawn() {
    use kicase_geometry::units::mm;
    use kicase_model::PartId;
    use kicase_ui::DesignerBackend;

    let (dir, board) = generated_board_with(MODEL_FOOTPRINT);
    write_block_model(dir.path());
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");
    let config = project.config.clone();

    let mut backend = kicase_app::AppBackend::new(project);
    // Models load on their own thread, so the scene fills in over the next few
    // refreshes rather than in the first one.
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(30);
    let bounds = loop {
        backend.refresh(&config).expect("refresh");
        let scene = backend.scene(&config).expect("scene");
        let part = scene.part(PartId::Components).expect("the components part is always present");
        if let Some(bounds) = part.mesh.bounds() {
            break bounds;
        }
        assert!(std::time::Instant::now() < deadline, "the model never loaded");
        std::thread::sleep(std::time::Duration::from_millis(20));
    };

    // The board is at KiCad (100..150, 80..110), so the footprint at (120, 95)
    // is at (120, -95) with Y up, and the 6 x 4 block straddles it.
    assert!((bounds.min.x.mm() - 117.0).abs() < 1e-6, "x {:?}", bounds.min.x);
    assert!((bounds.max.x.mm() - 123.0).abs() < 1e-6, "x {:?}", bounds.max.x);
    assert!((bounds.min.y.mm() + 97.0).abs() < 1e-6, "y {:?}", bounds.min.y);
    assert!((bounds.max.y.mm() + 93.0).abs() < 1e-6, "y {:?}", bounds.max.y);
    // It stands on the top face of the board, not at z = 0.
    assert!(bounds.min.z > mm(0.5), "the block sank into the board: {:?}", bounds.min.z);

    // An enclosure edit must not re-read or re-tessellate a single model: the
    // cache is keyed on the file, and nothing about the shell is the file.
    let before = backend.scene(&config).expect("scene");
    let mut taller = config.clone();
    taller.shell.total_height = config.shell.total_height + mm(3.0);
    backend.refresh(&taller).expect("refresh");
    let after = backend.scene(&taller).expect("scene");
    assert!(
        backend.models_arrived().is_none(),
        "an enclosure edit sent models back through the loader"
    );
    assert_eq!(
        before.part(PartId::Components).expect("components").mesh.triangle_count(),
        after.part(PartId::Components).expect("components").mesh.triangle_count(),
        "the components were rebuilt by an edit that did not move them"
    );
    assert_ne!(
        before.part(PartId::Lid).expect("lid").mesh.triangle_count() as i64
            + before.part(PartId::Lid).expect("lid").mesh.bounds().expect("lid").max.z.mm() as i64,
        after.part(PartId::Lid).expect("lid").mesh.triangle_count() as i64
            + after.part(PartId::Lid).expect("lid").mesh.bounds().expect("lid").max.z.mm() as i64,
        "the taller enclosure was not actually rebuilt"
    );
}

/// A model that is missing is a warning the user sees, and the enclosure still
/// builds.
/// A project whose numbers cannot build still has to open. The designer is
/// where those numbers get corrected, so refusing to load makes the mistake
/// unfixable from the UI: pressing the toolbar button does nothing at all and
/// leaves a dead process behind, which is what a hung plugin looks like.
#[test]
fn a_project_that_cannot_build_still_opens_and_says_why() {
    use kicase_geometry::units::mm;
    use kicase_ui::DesignerBackend;

    let (_dir, board) = generated_board();
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");

    // The PCB sitting inside the floor: buildable settings, impossible case.
    project.config.shell.floor = mm(2.0);
    project.config.shell.pcb_height = mm(2.0);
    project.save_config().expect("writes the settings");

    // Re-open from disk, which is what pressing the toolbar button does.
    let reopened = Project::open_file(&board).expect("a project that cannot build still loads");
    let config = reopened.config.clone();
    let mut backend = kicase_app::AppBackend::new(reopened);
    let data = backend.refresh(&config).expect("the window still gets its data");

    assert!(
        data.problems.iter().any(|p| p.contains("PCB")),
        "the reason must reach the window: {:?}",
        data.problems
    );
    // And the setting that caused it is still there to be corrected.
    assert_eq!(config.shell.pcb_height, mm(2.0));
}

/// A model that exists but is not readable fails on the loader thread, long
/// after the refresh that would have collected the complaint. The count alone
/// cannot carry that: a failed model counts as finished, so a viewport quietly
/// missing a component looked exactly like one that had loaded everything.
#[test]
fn a_model_that_fails_to_load_is_reported_when_it_arrives() {
    use kicase_ui::DesignerBackend;

    let (dir, board) = generated_board_with(
        "(footprint \"test:broken\" (layer \"F.Cu\") (at 120 95)\n\
         (uuid \"eeeeeeee-1111-0000-0000-000000000000\")\n\
         (property \"Reference\" \"U3\")\n\
         (model \"${KIPRJMOD}/truncated.step\")\n)\n",
    );
    // A real file, so it resolves; nonsense inside, so the loader rejects it.
    std::fs::write(dir.path().join("truncated.step"), "ISO-10303-21;\nnot a step file")
        .expect("write the broken model");

    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");
    let config = project.config.clone();

    let mut backend = kicase_app::AppBackend::new(project);
    let first = backend.refresh(&config).expect("the enclosure still builds");
    assert!(first.dimensions.is_some(), "the enclosure must still be built: {:?}", first.problems);

    // Wait for the loader rather than assuming it has finished.
    let mut progress = None;
    for _ in 0..200 {
        if let Some(arrived) = backend.models_arrived() {
            progress = Some(arrived);
            break;
        }
        std::thread::sleep(std::time::Duration::from_millis(25));
    }
    let progress = progress.expect("the loader reported back within five seconds");
    assert!(
        progress.problems.iter().any(|p| p.contains("truncated.step")),
        "a model that failed to load must say so as it arrives, not silently count as done: {:?}",
        progress
    );
}

#[test]
fn a_missing_model_is_a_warning_and_not_a_failure() {
    use kicase_ui::DesignerBackend;

    let missing = "(footprint \"test:gone\" (layer \"F.Cu\") (at 120 95)\n\
         (uuid \"eeeeeeee-0000-0000-0000-000000000000\")\n\
         (property \"Reference\" \"U2\")\n\
         (model \"${KIPRJMOD}/not-here.step\")\n)\n";
    let (_dir, board) = generated_board_with(missing);
    let mut project = Project::open_file(&board).expect("opens");
    pipeline::init(&mut project).expect("initializes");
    let config = project.config.clone();

    let mut backend = kicase_app::AppBackend::new(project);
    let data = backend.refresh(&config).expect("the enclosure still builds");
    assert!(
        data.dimensions.is_some(),
        "the enclosure must still be built and measured: {:?}",
        data.problems
    );
    assert!(
        data.problems.iter().any(|p| p.contains("not-here.step") && p.contains("not found")),
        "the missing model must be reported: {:?}",
        data.problems
    );
}
