//! `kicase` — the KiCad enclosure designer.
//!
//! The same binary is both the KiCad IPC plugin (KiCad launches it with a
//! subcommand from `plugin.json`) and an ordinary command line tool.

use anyhow::{Context, Result};
use clap::{Args, Parser, Subcommand};
use kicase_app::pipeline::{self, RebuildOptions, RebuildReport};
use kicase_app::project::Project;
use kicase_app::AppBackend;
use std::path::PathBuf;
use std::process::ExitCode;

#[derive(Parser, Debug)]
#[command(
    name = "kicase",
    version,
    about = "Design simple parametric enclosures from a KiCad PCB",
    long_about = "KiCase reads a KiCad board, interprets drawings on the enclosure user \
                  layers, and generates real B-rep enclosure geometry as STEP and STL.\n\n\
                  With no --board, KiCase talks to a running KiCad 10 over its IPC API \
                  (KiCad 10 requires the GUI to be running for this). Pass --board to work \
                  from a saved .kicad_pcb instead, which needs no KiCad at all but cannot \
                  update the board."
)]
struct Cli {
    /// Defaults to opening the designer, so KiCad launching the executable with
    /// no arguments still does something sensible.
    #[command(subcommand)]
    command: Option<Command>,

    /// Log more detail; repeat for more.
    #[arg(short, long, global = true, action = clap::ArgAction::Count)]
    verbose: u8,
}

#[derive(Subcommand, Debug)]
enum Command {
    /// Open the enclosure designer window.
    Designer(BoardArgs),
    /// Set up the enclosure layers and project file for this board.
    Init(BoardArgs),
    /// Regenerate enclosure geometry and update KiCad's 3D preview.
    Rebuild(RebuildArgs),
    /// Write enclosure files without touching the board.
    Export(ExportArgs),
    /// Check the project and report problems, without generating geometry.
    Validate(BoardArgs),
    /// List the graphics on the enclosure layers, with their UUIDs.
    List(BoardArgs),
    /// Bind a line on the datum layer to a named datum. The line is the bottom
    /// edge of the case wall; height is measured up from it.
    AddDatum(AddDatumArgs),
    /// Bind a closed graphic to a cutout.
    AddCutout(AddCutoutArgs),
    /// Bind a closed graphic to an added solid.
    AddSolid(AddSolidArgs),
    /// Remove a datum, cutout, solid or mounting-hole entry by id.
    Remove(RemoveArgs),
}

#[derive(Args, Debug, Clone)]
struct AddDatumArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Name for the datum, e.g. "front".
    #[arg(long)]
    id: String,
    /// UUID of the line, as shown by `kicase list`.
    #[arg(long)]
    uuid: String,
    /// Which way the wall normal points.
    #[arg(long, value_enum, default_value = "auto")]
    normal: NormalArg,
}

#[derive(Args, Debug, Clone)]
struct AddCutoutArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Name for the cutout, e.g. "usb".
    #[arg(long)]
    id: String,
    /// UUID of the closed graphic.
    #[arg(long)]
    uuid: String,
    /// Datum to fold the shape onto. Omit for a top or bottom opening.
    #[arg(long)]
    datum: Option<String>,
    /// Extra clearance all round, in millimetres.
    #[arg(long, default_value_t = 0.0)]
    clearance: f64,
    /// How far the hole reaches in from the face it was drawn on.
    #[arg(long, value_name = "MM")]
    depth: Option<f64>,
}

#[derive(Args, Debug, Clone)]
struct AddSolidArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Name for the solid, e.g. "rib".
    #[arg(long)]
    id: String,
    /// UUID of the closed graphic.
    #[arg(long)]
    uuid: String,
    /// Where the extrusion starts, in enclosure Z millimetres.
    #[arg(long, allow_negative_numbers = true)]
    z_start: Option<f64>,
    /// How tall the extrusion is, in millimetres.
    #[arg(long)]
    height: Option<f64>,
}

#[derive(Args, Debug, Clone)]
struct RemoveArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Id of the entry to remove.
    #[arg(long)]
    id: String,
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
enum NormalArg {
    Left,
    Right,
    Auto,
}

impl From<NormalArg> for kicase_model::DatumNormal {
    fn from(value: NormalArg) -> Self {
        match value {
            NormalArg::Left => kicase_model::DatumNormal::Left,
            NormalArg::Right => kicase_model::DatumNormal::Right,
            NormalArg::Auto => kicase_model::DatumNormal::Auto,
        }
    }
}

#[derive(clap::ValueEnum, Debug, Clone, Copy)]
enum FaceArg {
    Top,
    Bottom,
}

impl From<FaceArg> for kicase_model::CutFace {
    fn from(value: FaceArg) -> Self {
        match value {
            FaceArg::Top => kicase_model::CutFace::Top,
            FaceArg::Bottom => kicase_model::CutFace::Bottom,
        }
    }
}

#[derive(Args, Debug, Clone)]
struct BoardArgs {
    /// Work from this saved board file instead of a running KiCad.
    #[arg(long, value_name = "FILE")]
    board: Option<PathBuf>,
    /// Render a few frames, save a screenshot here, and exit. For checking the
    /// viewport without a person watching it.
    #[arg(long, value_name = "PNG", hide = true)]
    screenshot: Option<PathBuf>,
    /// Open with the section plane on, at this fraction of the model.
    #[arg(long, value_name = "0..1", hide = true)]
    section: Option<f32>,
    /// Show only these parts: pcb, bottom, lid.
    #[arg(long, value_name = "PARTS", value_delimiter = ',', hide = true)]
    show: Vec<String>,
    /// Seconds to wait before the screenshot.
    #[arg(long, value_name = "SECONDS", hide = true)]
    screenshot_delay: Option<f32>,
    /// Open looking from this direction, as clicking the view cube does.
    #[arg(long, value_name = "top|bottom|front|back|left|right|iso", hide = true)]
    view: Option<String>,
}

#[derive(Args, Debug, Clone)]
struct RebuildArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Also write STL files.
    #[arg(long)]
    stl: bool,
    /// Also write the OpenSCAD project.
    #[arg(long)]
    openscad: bool,
    /// Do not create the preview footprint or refresh the editor.
    #[arg(long)]
    no_kicad: bool,
}

#[derive(Args, Debug, Clone)]
struct ExportArgs {
    #[command(flatten)]
    board: BoardArgs,
    /// Write bottom.step, lid.step and enclosure.step.
    #[arg(long)]
    step: bool,
    /// Write bottom.stl and lid.stl.
    #[arg(long)]
    stl: bool,
    /// Write the OpenSCAD project.
    #[arg(long)]
    openscad: bool,
}

fn main() -> ExitCode {
    let cli = Cli::parse();
    init_tracing(cli.verbose);

    match run(&cli) {
        Ok(()) => ExitCode::SUCCESS,
        Err(err) => {
            eprintln!("error: {err:#}");
            ExitCode::FAILURE
        },
    }
}

fn run(cli: &Cli) -> Result<()> {
    let default = Command::Designer(BoardArgs {
        board: None,
        screenshot: None,
        section: None,
        show: Vec::new(),
        screenshot_delay: None,
        view: None,
    });
    match cli.command.as_ref().unwrap_or(&default) {
        Command::Designer(args) => {
            let mut project = Project::open(args.board.as_deref())?;
            // Opening the designer on a board that has never been set up is the
            // normal way to start, and it arrives here by someone pressing the
            // toolbar button in KiCad. Claiming the layers is the one thing
            // that has to happen first, it can only happen with KiCad running,
            // and KiCad is running — so do it rather than send them to a
            // terminal to ask for it.
            // Only with KiCad on the other end: claiming layers renames them on
            // the board, which a board file cannot do. Left alone otherwise, so
            // the window can say what is missing instead of recording a setup
            // that did not happen.
            if project.is_new && project.origin.session().is_some() {
                match pipeline::init(&mut project) {
                    Ok(report) => print_report("Set up the enclosure layers", &report),
                    Err(err) => eprintln!("warning: could not set up the layers: {err:#}"),
                }
            }
            let config = project.config.clone();
            let backend = AppBackend::new(project);
            let section =
                args.section.map(|at| kicase_ui::Section::at(kicase_ui::SectionAxis::Y, at));
            let show_only = args
                .show
                .iter()
                .filter_map(|name| match name.as_str() {
                    "pcb" => Some(kicase_model::PartId::Pcb),
                    "bottom" => Some(kicase_model::PartId::Bottom),
                    "lid" => Some(kicase_model::PartId::Lid),
                    "components" => Some(kicase_model::PartId::Components),
                    _ => None,
                })
                .collect();
            kicase_ui::run_with_screenshot(
                Box::new(backend),
                config,
                args.screenshot.clone(),
                section,
                show_only,
                args.screenshot_delay.map(std::time::Duration::from_secs_f32),
                args.view.clone(),
            )
            .map_err(|err| anyhow::anyhow!("the designer window could not open: {err}"))
        },
        Command::Init(args) => {
            let mut project = Project::open(args.board.as_deref())?;
            let report = pipeline::init(&mut project).context("initializing the enclosure")?;
            print_report("Initialized", &report);
            println!(
                "Draw on these layers: {} (outline), {} (datums), {} (cuts), {} (solids).",
                project.config.layers.outline,
                project.config.layers.datums,
                project.config.layers.cuts,
                project.config.layers.solids
            );
            Ok(())
        },
        Command::Rebuild(args) => {
            let mut project = Project::open(args.board.board.as_deref())?;
            let mut options = RebuildOptions::rebuild(&project.config);
            options.stl |= args.stl;
            options.openscad |= args.openscad;
            if args.no_kicad {
                options.update_kicad = false;
            }
            let report = pipeline::rebuild(&mut project, options).context("rebuilding")?;
            print_report("Rebuilt", &report);
            Ok(())
        },
        Command::Export(args) => {
            let mut project = Project::open(args.board.board.as_deref())?;
            // With no flags, write everything the project is configured for.
            let none_selected = !args.step && !args.stl && !args.openscad;
            let options = RebuildOptions {
                step: args.step || none_selected,
                stl: args.stl || none_selected,
                openscad: args.openscad || (none_selected && project.config.export.openscad),
                update_kicad: false,
            };
            let report = pipeline::rebuild(&mut project, options).context("exporting")?;
            print_report("Exported", &report);
            Ok(())
        },
        Command::List(args) => {
            let project = Project::open(args.board.as_deref())?;
            let graphics = pipeline::list_graphics(&project)?;
            if graphics.is_empty() {
                println!("No graphics on the enclosure layers yet.");
                println!(
                    "Draw on {} (datums), {} (cuts) or {} (solids), then run this again.",
                    project.config.layers.datums,
                    project.config.layers.cuts,
                    project.config.layers.solids
                );
                return Ok(());
            }
            for graphic in graphics {
                let bound = match &graphic.bound_to {
                    Some(id) => format!("  <- \"{id}\""),
                    None => String::new(),
                };
                println!(
                    "{}  {:<18} {}{}",
                    graphic.uuid, graphic.layer, graphic.description, bound
                );
            }
            Ok(())
        },
        Command::AddDatum(args) => {
            let mut project = Project::open(args.board.board.as_deref())?;
            pipeline::add_datum(&mut project, &args.id, &args.uuid, args.normal.into())?;
            println!("Added datum \"{}\".", args.id);
            Ok(())
        },
        Command::AddCutout(args) => {
            let mut project = Project::open(args.board.board.as_deref())?;
            pipeline::add_feature(
                &mut project,
                kicase_model::FeatureConfig {
                    id: args.id.clone(),
                    graphic_uuid: args.uuid.clone(),
                    datum: args.datum.clone(),
                    depth: args.depth.map(kicase_geometry::units::mm),
                    clearance: kicase_geometry::units::mm(args.clearance),
                    z_start: None,
                    height: None,
                    enabled: true,
                },
            )?;
            println!("Added cutout \"{}\".", args.id);
            Ok(())
        },
        Command::AddSolid(args) => {
            let mut project = Project::open(args.board.board.as_deref())?;
            pipeline::add_feature(
                &mut project,
                kicase_model::FeatureConfig {
                    id: args.id.clone(),
                    graphic_uuid: args.uuid.clone(),
                    datum: None,
                    depth: None,
                    clearance: kicase_geometry::units::mm(0.0),
                    z_start: args.z_start.map(kicase_geometry::units::mm),
                    height: args.height.map(kicase_geometry::units::mm),
                    enabled: true,
                },
            )?;
            println!("Added solid \"{}\".", args.id);
            Ok(())
        },
        Command::Remove(args) => {
            let mut project = Project::open(args.board.board.as_deref())?;
            if pipeline::remove_entry(&mut project, &args.id)? {
                println!("Removed \"{}\".", args.id);
                Ok(())
            } else {
                anyhow::bail!("no entry named \"{}\"", args.id)
            }
        },
        Command::Validate(args) => {
            let mut project = Project::open(args.board.as_deref())?;
            let report = pipeline::validate(&mut project).context("validating")?;
            print_report("Validated", &report);
            if !report.is_clean() {
                anyhow::bail!("the enclosure project has unresolved problems");
            }
            Ok(())
        },
    }
}

fn print_report(headline: &str, report: &RebuildReport) {
    println!("{headline}.");
    for file in &report.files {
        println!("  wrote {}", file.display());
    }
    for orphan in &report.orphans {
        println!("  orphaned {} \"{}\" ({})", kind_of(orphan), orphan.id, orphan.uuid);
    }
    for warning in &report.warnings {
        println!("  warning: {warning}");
    }
    for check in &report.fit {
        println!("  fit {check}");
    }
    for note in &report.notes {
        println!("  note: {note}");
    }
    if let Some(preview) = report.preview {
        println!("  preview footprint: {preview:?}");
    }
}

fn kind_of(orphan: &kicase_model::Orphan) -> &'static str {
    match orphan.kind {
        kicase_model::OrphanKind::Datum => "datum",
        kicase_model::OrphanKind::Feature => "feature",
        kicase_model::OrphanKind::MountingHole => "mounting hole",
    }
}

fn init_tracing(verbosity: u8) {
    let default = match verbosity {
        0 => "kicase=info",
        1 => "kicase=debug",
        _ => "kicase=trace",
    };
    let filter = tracing_subscriber::EnvFilter::try_from_env("KICASE_LOG")
        .unwrap_or_else(|_| tracing_subscriber::EnvFilter::new(default));
    let _ =
        tracing_subscriber::fmt().with_env_filter(filter).with_writer(std::io::stderr).try_init();
}
