//! Mesh-derived measurements.
//!
//! OpenCascade's global-property API is not exposed by the Rust bindings we
//! depend on, so volume and body counts are derived from a fine triangulation.
//! That is accurate enough for the geometry regression tests described in the
//! specification (volume, bounding box, body count) and never affects the
//! exported B-rep.

use kicase_geometry::error::{GeometryError, Result};
use opencascade::primitives::Shape;
use std::collections::HashMap;

/// Triangulation tolerance used for measurements, in millimetres.
const MEASURE_TOLERANCE: f64 = 0.01;

/// Signed volume of a closed triangulated shape, in cubic millimetres.
pub(crate) fn volume(shape: &Shape) -> Result<f64> {
    let mesh = shape
        .mesh_with_tolerance(MEASURE_TOLERANCE)
        .map_err(|e| GeometryError::kernel("mesh", e.to_string()))?;

    let mut total = 0.0;
    for tri in mesh.indices.as_chunks::<3>().0 {
        let (a, b, c) = (mesh.vertices[tri[0]], mesh.vertices[tri[1]], mesh.vertices[tri[2]]);
        total += a.dot(b.cross(c)) / 6.0;
    }
    Ok(total.abs())
}

/// Counts disjoint bodies by finding connected components of the triangulation.
///
/// Vertices are quantized before comparison because OpenCascade emits one
/// vertex array per face, so coincident points are duplicated.
pub(crate) fn solid_count(shape: &Shape) -> Result<usize> {
    let mesh = shape
        .mesh_with_tolerance(MEASURE_TOLERANCE)
        .map_err(|e| GeometryError::kernel("mesh", e.to_string()))?;

    if mesh.vertices.is_empty() {
        return Ok(0);
    }

    // Map each quantized position to a canonical index.
    let quantize = |v: f64| (v * 1_000.0).round() as i64;
    let mut canonical: HashMap<(i64, i64, i64), usize> = HashMap::new();
    let mut vertex_group: Vec<usize> = Vec::with_capacity(mesh.vertices.len());
    for v in &mesh.vertices {
        let key = (quantize(v.x), quantize(v.y), quantize(v.z));
        let next = canonical.len();
        let id = *canonical.entry(key).or_insert(next);
        vertex_group.push(id);
    }

    let mut parent: Vec<usize> = (0..canonical.len()).collect();
    fn find(parent: &mut [usize], mut node: usize) -> usize {
        while parent[node] != node {
            parent[node] = parent[parent[node]];
            node = parent[node];
        }
        node
    }
    fn union(parent: &mut [usize], a: usize, b: usize) {
        let (ra, rb) = (find(parent, a), find(parent, b));
        if ra != rb {
            parent[ra] = rb;
        }
    }

    for tri in mesh.indices.as_chunks::<3>().0 {
        let (a, b, c) = (vertex_group[tri[0]], vertex_group[tri[1]], vertex_group[tri[2]]);
        union(&mut parent, a, b);
        union(&mut parent, b, c);
    }

    let mut roots: Vec<usize> = Vec::new();
    for i in 0..parent.len() {
        let r = find(&mut parent, i);
        if !roots.contains(&r) {
            roots.push(r);
        }
    }
    Ok(roots.len())
}
