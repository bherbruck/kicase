//! The semantic enclosure model, the `enclosure.toml` project format, and the
//! kernel-agnostic build pipeline.
//!
//! This crate owns *meaning*. It knows what a datum is, what a cutout does, and
//! how the enclosure stacks up in Z. It does not know what KiCad is, and it is
//! generic over the CAD kernel.

pub mod builder;
pub mod config;
pub mod error;
pub mod fit;
pub mod model;
pub mod scene;
pub mod source;

pub use config::{
    CutFace, DatumConfig, DatumNormal, EnclosureConfig, ExportParameters, FeatureConfig,
    LayerMapping, LidParameters, ShellParameters, CONFIG_FILE, DEFAULT_CUT_DEPTH, GENERATED_DIR,
    LAYER_DISPLAY_NAMES, OPENSCAD_DIR, PROJECT_DIR, SCHEMA_VERSION,
};
pub use error::{ModelError, Result, Warning};
pub use fit::{check_fit, CutRecord, FitCheck, FitStatus};
pub use model::{
    AddedSolid, CutPlacement, Cutout, Enclosure, Lid, Orphan, OrphanKind, Shell, SideDatum, ZLayout,
};
pub use scene::{build_scene, build_scene_of, PartId, Scene, ScenePart, DISPLAY_TOLERANCE};
pub use source::{BoardGraphic, BoardSource, KiCadUuid, LayerRole, MountingHole};
