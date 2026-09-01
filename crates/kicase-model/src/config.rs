//! The `.enclosure/enclosure.toml` project format.
//!
//! Semantics live here, never in the KiCad drawing itself: no meaning is
//! attached to line widths, colours, layer names chosen by the user, or
//! reference designators. Graphics are bound by their persistent KiCad UUID.

use crate::error::{ModelError, Result};
use kicase_geometry::units::{mm, Length};
use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use std::path::{Path, PathBuf};

/// How deep a hole goes when nothing says otherwise.
///
/// A plain number rather than something derived from the stack-up: it shows in
/// the designer as 10 mm, and you can see at a glance what it will do.
pub const DEFAULT_CUT_DEPTH: Length = mm(10.0);

/// Schema version understood by this build.
pub const SCHEMA_VERSION: u32 = 1;

/// Directory, relative to the KiCad project, holding all KiCase state.
pub const PROJECT_DIR: &str = ".enclosure";
/// Project file name inside [`PROJECT_DIR`].
pub const CONFIG_FILE: &str = "enclosure.toml";
/// Generated-output directory inside [`PROJECT_DIR`].
pub const GENERATED_DIR: &str = "generated";
/// OpenSCAD derivative directory inside [`PROJECT_DIR`].
pub const OPENSCAD_DIR: &str = "openscad";

/// Which KiCad user layers hold enclosure geometry.
///
/// Layer *ids* are the canonical binding; the display names KiCad shows are
/// stored alongside purely for diagnostics.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct LayerMapping {
    pub outline: String,
    pub datums: String,
    pub cuts: String,
    pub top: String,
    pub bottom: String,
    pub solids: String,
}

impl Default for LayerMapping {
    fn default() -> Self {
        LayerMapping {
            outline: "User.1".to_string(),
            datums: "User.2".to_string(),
            cuts: "User.3".to_string(),
            top: "User.4".to_string(),
            bottom: "User.5".to_string(),
            solids: "User.6".to_string(),
        }
    }
}

impl LayerMapping {
    pub fn all(&self) -> [&str; 6] {
        [&self.outline, &self.datums, &self.cuts, &self.top, &self.bottom, &self.solids]
    }
}

/// Display names KiCase asks KiCad to give the layers it claims.
///
/// The layer *is* the meaning: what you draw a shape on decides what it does,
/// so there is no "kind" or "face" to configure anywhere.
pub const LAYER_DISPLAY_NAMES: [&str; 6] = [
    "Enclosure",
    "Enclosure.Datums",
    "Enclosure.Cuts",
    "Enclosure.Top",
    "Enclosure.Bottom",
    "Enclosure.Solids",
];

/// Shell settings.
///
/// Only what cannot be drawn lives here. Anything with a shape — the outline,
/// the wall thickness, the corner radii, the clearance around the board, every
/// hole — is drawn in the PCB editor and read back from the drawing. There is
/// no corner radius here, and no PCB clearance: you draw the wall where you
/// want it, and the gap to the board is whatever you drew.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ShellParameters {
    /// Wall thickness used only if the drawn outline somehow carries no line
    /// width. The width of the line you drew is the real source.
    pub wall: Length,
    /// Overall outside height of the closed case, bottom face to top face.
    pub total_height: Length,
    /// How far the underside of the PCB sits above the bottom of the case.
    pub pcb_height: Length,
    /// Bottom thickness: the floor of the case.
    pub floor: Length,
}

impl Default for ShellParameters {
    fn default() -> Self {
        ShellParameters {
            wall: mm(2.0),
            total_height: mm(15.0),
            pcb_height: mm(6.0),
            floor: mm(2.0),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LidParameters {
    pub thickness: Length,
    /// Gap between the lip and the cavity wall.
    pub fit_clearance: Length,
    /// How far the lip reaches down into the cavity.
    pub lip_depth: Length,
    pub lip_thickness: Length,
}

impl Default for LidParameters {
    fn default() -> Self {
        LidParameters {
            thickness: mm(2.0),
            fit_clearance: mm(0.2),
            lip_depth: mm(3.0),
            lip_thickness: mm(1.2),
        }
    }
}

/// Which way a datum's wall normal points relative to the drawn line.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum DatumNormal {
    /// Left of the line direction.
    Left,
    /// Right of the line direction.
    Right,
    /// Away from the centre of the enclosure.
    #[default]
    Auto,
}

/// A side datum, as stored in the project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// A side datum.
///
/// The line you draw *is* the bottom edge of the case wall, so there is nothing
/// to configure about its height: distance from the line is height above the
/// bottom of the case. All that remains is which side of the line the wall
/// faces, and even that is worked out automatically unless you say otherwise.
pub struct DatumConfig {
    pub id: String,
    pub graphic_uuid: String,
    #[serde(default)]
    pub normal: DatumNormal,
}

/// Which horizontal face a hole goes through. Decided by the layer it is drawn
/// on, never configured.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum CutFace {
    #[default]
    Top,
    Bottom,
}

/// A cutout or added solid, as stored in the project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
/// Refinements to a drawn feature.
///
/// What a shape *does* comes from the layer it is drawn on, so nothing here
/// says "cutout" or "top": a shape on `Enclosure.Top` is a hole through the
/// lid because that is where it was drawn. An entry only exists to add
/// something the drawing cannot carry — the datum a side opening belongs to,
/// a clearance, or how far a solid rises.
pub struct FeatureConfig {
    pub id: String,
    pub graphic_uuid: String,
    /// Side openings only: the datum whose wall this goes through.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub datum: Option<String>,
    /// Extra clearance added all round a cutout.
    #[serde(default)]
    pub clearance: Length,
    /// Holes only: how far the hole reaches in from the face it was drawn on.
    ///
    /// The layer gives the direction — a hole on the bottom layer cuts upward,
    /// one on the top layer cuts down. Left out, the hole goes through the part
    /// it was drawn on and no further; make it long enough and it will pass
    /// through the other part too.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub depth: Option<Length>,
    /// Solids only: where the extrusion starts. Defaults to the cavity floor.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub z_start: Option<Length>,
    /// Solids only: how tall it is. Defaults to reaching the underside of the
    /// PCB, which makes a plain drawn circle a standoff.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<Length>,
    #[serde(default = "default_true")]
    pub enabled: bool,
}

fn default_true() -> bool {
    true
}

impl FeatureConfig {
    /// True when this entry refines the given KiCad graphic.
    pub fn uuid_matches(&self, uuid: &str) -> bool {
        self.graphic_uuid == uuid
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ExportParameters {
    /// Chord tolerance used when tessellating for STL.
    pub stl_tolerance: Length,
    /// Whether `kicase rebuild` also refreshes the OpenSCAD derivative.
    pub openscad: bool,
}

impl Default for ExportParameters {
    fn default() -> Self {
        ExportParameters { stl_tolerance: mm(0.05), openscad: false }
    }
}

/// The whole project file.
#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct EnclosureConfig {
    pub version: u32,
    #[serde(default)]
    pub layers: LayerMapping,
    #[serde(default)]
    pub shell: ShellParameters,
    #[serde(default)]
    pub lid: LidParameters,
    #[serde(default)]
    pub export: ExportParameters,
    #[serde(default, rename = "datum")]
    pub datums: Vec<DatumConfig>,
    #[serde(default, rename = "feature")]
    pub features: Vec<FeatureConfig>,
}

impl Default for EnclosureConfig {
    fn default() -> Self {
        EnclosureConfig {
            version: SCHEMA_VERSION,
            layers: LayerMapping::default(),
            shell: ShellParameters::default(),
            lid: LidParameters::default(),
            export: ExportParameters::default(),
            datums: Vec::new(),
            features: Vec::new(),
        }
    }
}

impl EnclosureConfig {
    /// Parses a project file, rejecting versions this build does not understand.
    ///
    /// Unknown *fields* are ignored so that a newer KiCase can add settings
    /// without breaking older ones; unknown *enum variants* are a hard error,
    /// because silently dropping a feature would produce wrong geometry.
    pub fn from_toml(text: &str) -> Result<Self> {
        let probe: VersionProbe =
            toml::from_str(text).map_err(|e| ModelError::Parse(e.to_string()))?;
        if probe.version != SCHEMA_VERSION {
            return Err(ModelError::UnsupportedVersion {
                found: probe.version,
                supported: SCHEMA_VERSION,
            });
        }
        // Deliberately not validated here. A setting that cannot build is
        // still a setting the user has to be able to see and correct, and the
        // place they correct it is the designer — which cannot open if loading
        // the project it is meant to fix is what fails. `Enclosure::resolve`
        // validates at the point of building, which is where the answer
        // actually matters.
        Ok(toml::from_str(text).map_err(|e| ModelError::Parse(e.to_string()))?)
    }

    pub fn to_toml(&self) -> Result<String> {
        toml::to_string_pretty(self).map_err(|e| ModelError::Serialize(e.to_string()))
    }

    /// Loads `<project>/.enclosure/enclosure.toml`.
    pub fn load(project_dir: &Path) -> Result<Self> {
        let path = Self::config_path(project_dir);
        if !path.exists() {
            return Err(ModelError::NotInitialized { path: path.display().to_string() });
        }
        let text = std::fs::read_to_string(&path)
            .map_err(|source| ModelError::io(path.display(), source))?;
        Self::from_toml(&text)
    }

    pub fn save(&self, project_dir: &Path) -> Result<()> {
        let path = Self::config_path(project_dir);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|source| ModelError::io(parent.display(), source))?;
        }
        std::fs::write(&path, self.to_toml()?)
            .map_err(|source| ModelError::io(path.display(), source))
    }

    pub fn config_path(project_dir: &Path) -> PathBuf {
        project_dir.join(PROJECT_DIR).join(CONFIG_FILE)
    }

    pub fn generated_dir(project_dir: &Path) -> PathBuf {
        project_dir.join(PROJECT_DIR).join(GENERATED_DIR)
    }

    pub fn openscad_dir(project_dir: &Path) -> PathBuf {
        project_dir.join(PROJECT_DIR).join(OPENSCAD_DIR)
    }

    /// Checks the invariants that would otherwise surface as confusing
    /// geometry failures much later.
    pub fn validate(&self) -> Result<()> {
        let positive = |name: &'static str, value: Length| -> Result<()> {
            if value.is_positive() && value.is_finite() {
                Ok(())
            } else {
                Err(ModelError::NonPositive { name, value: value.mm() })
            }
        };
        positive("wall thickness", self.shell.wall)?;
        positive("bottom thickness", self.shell.floor)?;
        positive("total height", self.shell.total_height)?;
        positive("PCB height", self.shell.pcb_height)?;
        positive("top thickness", self.lid.thickness)?;

        // Everything is measured from the bottom of the case, so these are the
        // orderings that have to hold for the case to exist at all.
        if self.shell.pcb_height <= self.shell.floor {
            return Err(ModelError::ImpossibleStack {
                detail: format!(
                    "the PCB sits {} above the bottom of the case, which is at or below the \
                     {} floor. Raise the PCB height.",
                    self.shell.pcb_height, self.shell.floor
                ),
            });
        }
        let rim = self.shell.total_height - self.lid.thickness;
        if rim <= self.shell.pcb_height {
            return Err(ModelError::ImpossibleStack {
                detail: format!(
                    "a {} case with a {} lid leaves the rim at {}, at or below the PCB at {}. \
                     Increase the total height.",
                    self.shell.total_height, self.lid.thickness, rim, self.shell.pcb_height
                ),
            });
        }
        positive("STL tolerance", self.export.stl_tolerance)?;

        let mut seen = HashSet::new();
        for datum in &self.datums {
            if !seen.insert(datum.id.as_str()) {
                return Err(ModelError::DuplicateId { id: datum.id.clone() });
            }
        }
        let mut seen = HashSet::new();
        for feature in &self.features {
            if !seen.insert(feature.id.as_str()) {
                return Err(ModelError::DuplicateId { id: feature.id.clone() });
            }
            if let Some(datum) = &feature.datum {
                if !self.datums.iter().any(|d| &d.id == datum) {
                    return Err(ModelError::UnknownDatum {
                        id: feature.id.clone(),
                        datum: datum.clone(),
                    });
                }
            }
        }
        Ok(())
    }

    pub fn datum(&self, id: &str) -> Option<&DatumConfig> {
        self.datums.iter().find(|d| d.id == id)
    }
}

#[derive(Deserialize)]
struct VersionProbe {
    version: u32,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn round_trips_through_toml() {
        let mut config = EnclosureConfig::default();
        config.datums.push(DatumConfig {
            id: "front".into(),
            graphic_uuid: "8b27-aaaa".into(),
            normal: DatumNormal::Right,
        });
        config.features.push(FeatureConfig {
            id: "usb".into(),
            graphic_uuid: "abc3-bbbb".into(),
            datum: Some("front".into()),
            depth: None,
            clearance: mm(0.3),
            z_start: None,
            height: None,
            enabled: true,
        });

        let text = config.to_toml().expect("serializes");
        let parsed = EnclosureConfig::from_toml(&text).expect("parses");
        assert_eq!(config, parsed);
    }

    #[test]
    fn rejects_future_schema_versions() {
        let text = "version = 99\n";
        match EnclosureConfig::from_toml(text) {
            Err(ModelError::UnsupportedVersion { found, supported }) => {
                assert_eq!(found, 99);
                assert_eq!(supported, SCHEMA_VERSION);
            },
            other => panic!("expected a version error, got {other:?}"),
        }
    }

    #[test]
    fn ignores_unknown_fields_for_forward_compatibility() {
        let text = r#"
            version = 1
            future_setting = "hello"

            [shell]
            wall = 2.5
            some_future_shell_option = 7
        "#;
        let config = EnclosureConfig::from_toml(text).expect("unknown fields are ignored");
        assert_eq!(config.shell.wall, mm(2.5));
        // Unspecified fields keep their defaults.
        assert_eq!(config.shell.floor, mm(2.0));
    }

    #[test]
    fn rejects_unknown_enum_variants() {
        let text = r#"
            version = 1

            [[datum]]
            id = "front"
            graphic_uuid = "1234"
            normal = "moon_surface"
        "#;
        let err = EnclosureConfig::from_toml(text).expect_err("unknown variant must fail");
        assert!(matches!(err, ModelError::Parse(_)), "got {err:?}");
    }

    /// Loading and validating are separate on purpose: a project whose numbers
    /// cannot build still has to open, because the designer is where they get
    /// corrected. So these check `validate`, which is what the build calls,
    /// rather than the parse.
    #[test]
    fn rejects_a_cutout_pointing_at_a_missing_datum() {
        let text = r#"
            version = 1

            [[feature]]
            id = "usb"
            graphic_uuid = "abc"
            datum = "front"
        "#;
        let config = EnclosureConfig::from_toml(text).expect("a bad datum still loads");
        let err = config.validate().expect_err("dangling datum must fail to validate");
        assert!(matches!(err, ModelError::UnknownDatum { .. }), "got {err:?}");
    }

    #[test]
    fn rejects_zero_wall_thickness() {
        let text = "version = 1\n\n[shell]\nwall = 0.0\n";
        let config = EnclosureConfig::from_toml(text).expect("a zero wall still loads");
        let err = config.validate().expect_err("zero wall must fail to validate");
        assert!(matches!(err, ModelError::NonPositive { name: "wall thickness", .. }));
    }

    #[test]
    fn spec_example_parses() {
        // The example from the specification, field for field.
        let text = r#"
            version = 1

            [layers]
            outline = "User.1"
            datums = "User.2"
            cuts = "User.3"
            solids = "User.4"

            [shell]
            wall = 2.0
            pcb_standoff_height = 4.0
            component_clearance_z = 3.0

            [[datum]]
            id = "front"
            graphic_uuid = "8b27"
            normal = "right"

            [[feature]]
            id = "usb"
            graphic_uuid = "abc3"
            datum = "front"
            depth = 3.0
            clearance = 0.3
        "#;
        let config = EnclosureConfig::from_toml(text).expect("spec example parses");
        assert_eq!(config.datums[0].normal, DatumNormal::Right);
        assert_eq!(config.features[0].clearance, mm(0.3));
        assert_eq!(config.features[0].depth, Some(mm(3.0)));
    }
}
