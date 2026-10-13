//! glTF 2.0 binary (`.glb`) from Ogre meshes and skeletons: each triangle-list submesh with its
//! normals and UVs, and, given a skeleton, its bones as nodes, the skin (the four strongest weights
//! a vertex) and its animations. Ogre keyframes are offsets from the bind pose; glTF wants each
//! bone's local transform, so a key is folded into the bind pose as `kenshi_anim::Rig::sample`
//! does. Morph targets (face poses) and materials beyond one default are not written.

use glam::{Quat, Vec3};
use kenshi_anim::Rig;
use kenshi_io::ogre_mesh::{BoneAssignment, Geometry, Mesh};
use kenshi_io::ogre_skeleton::Skeleton;
use serde_json::{Value, json};

const OP_TRIANGLE_LIST: u16 = 4;
const FLOAT: u32 = 5126;
const UNSIGNED_INT: u32 = 5125;
const UNSIGNED_SHORT: u32 = 5123;

/// What went into a file, for checking it against what an importer reads.
#[derive(Debug, Default, Clone, PartialEq)]
pub struct Written {
    pub primitives: usize,
    pub vertices: usize,
    pub triangles: usize,
    pub bones: usize,
    pub animations: usize,
    /// Submeshes left out: not triangle lists, or without positions.
    pub skipped: usize,
}

#[derive(Default)]
struct Bin {
    data: Vec<u8>,
    views: Vec<Value>,
    accessors: Vec<Value>,
}

impl Bin {
    fn view(&mut self, bytes: &[u8], target: Option<u32>) -> usize {
        while !self.data.len().is_multiple_of(4) {
            self.data.push(0);
        }
        let mut v = json!({"buffer": 0, "byteOffset": self.data.len(), "byteLength": bytes.len()});
        if let Some(t) = target {
            v["target"] = json!(t);
        }
        self.data.extend_from_slice(bytes);
        self.views.push(v);
        self.views.len() - 1
    }

    fn accessor(&mut self, view: usize, component: u32, count: usize, ty: &str, bounds: Option<(Vec<f32>, Vec<f32>)>) -> usize {
        let mut a = json!({"bufferView": view, "componentType": component, "count": count, "type": ty});
        if let Some((min, max)) = bounds {
            a["min"] = json!(min);
            a["max"] = json!(max);
        }
        self.accessors.push(a);
        self.accessors.len() - 1
    }

    fn floats<const N: usize>(&mut self, items: &[[f32; N]], ty: &str, target: Option<u32>, bounds: bool) -> usize {
        let bytes: Vec<u8> = items.iter().flatten().flat_map(|f| f.to_le_bytes()).collect();
        let b = bounds.then(|| {
            let mut min = vec![f32::MAX; N];
            let mut max = vec![f32::MIN; N];
            for it in items {
                for k in 0..N {
                    min[k] = min[k].min(it[k]);
                    max[k] = max[k].max(it[k]);
                }
            }
            (min, max)
        });
        let v = self.view(&bytes, target);
        self.accessor(v, FLOAT, items.len(), ty, b)
    }
}

/// Up to four (joint, weight) pairs a vertex, strongest first, summing to 1.
fn weights(count: usize, assigned: &[BoneAssignment], joint_of: &dyn Fn(u16) -> Option<usize>, fallback: usize) -> (Vec<[u16; 4]>, Vec<[f32; 4]>) {
    let mut per: Vec<Vec<(usize, f32)>> = vec![vec![]; count];
    for a in assigned {
        if let (Some(list), Some(j)) = (per.get_mut(a.vertex as usize), joint_of(a.bone)) {
            list.push((j, a.weight));
        }
    }
    let (mut joints, mut ws) = (Vec::with_capacity(count), Vec::with_capacity(count));
    for mut list in per {
        list.sort_by(|a, b| b.1.total_cmp(&a.1));
        list.truncate(4);
        let sum: f32 = list.iter().map(|x| x.1).sum();
        if list.is_empty() || sum <= 0.0 {
            list = vec![(fallback, 1.0)];
        }
        let sum: f32 = list.iter().map(|x| x.1).sum();
        let (mut j, mut w) = ([0u16; 4], [0f32; 4]);
        for (k, (joint, weight)) in list.iter().enumerate() {
            j[k] = *joint as u16;
            w[k] = weight / sum;
        }
        joints.push(j);
        ws.push(w);
    }
    (joints, ws)
}

fn unit(n: [f32; 3]) -> [f32; 3] {
    let v = Vec3::from(n);
    if v.length_squared() > 1e-12 { v.normalize().to_array() } else { [0.0, 1.0, 0.0] }
}

/// The vertex attributes of one geometry (submeshes may share one).
fn attrs(bin: &mut Bin, geo: &Geometry, assigned: &[BoneAssignment], w: &mut Written, rig: Option<&Rig>, fallback: usize) -> Option<Value> {
    let positions = geo.positions()?;
    let n = positions.len();
    w.vertices += n;
    let mut a = json!({"POSITION": bin.floats(&positions, "VEC3", Some(34962), true)});
    if let Some(normals) = geo.normals().filter(|v| v.len() == n) {
        let normals: Vec<[f32; 3]> = normals.into_iter().map(unit).collect();
        a["NORMAL"] = json!(bin.floats(&normals, "VEC3", Some(34962), false));
    }
    if let Some(uvs) = geo.uvs(0).filter(|v| v.len() == n) {
        a["TEXCOORD_0"] = json!(bin.floats(&uvs, "VEC2", Some(34962), false));
    }
    if let Some(rig) = rig {
        let (joints, ws) = weights(n, assigned, &|h| rig.index_of_handle(h), fallback);
        let bytes: Vec<u8> = joints.iter().flatten().flat_map(|j| j.to_le_bytes()).collect();
        let v = bin.view(&bytes, Some(34962));
        a["JOINTS_0"] = json!(bin.accessor(v, UNSIGNED_SHORT, n, "VEC4", None));
        a["WEIGHTS_0"] = json!(bin.floats(&ws, "VEC4", Some(34962), false));
    }
    Some(a)
}

/// A `.glb` of `mesh`, skinned and animated when `skeleton` is given. `animations` limits which
/// animations go in (all when `None`); `first` puts one of them first.
pub fn glb(name: &str, mesh: &Mesh, skeleton: Option<&Skeleton>, animations: Option<&[String]>, first: Option<&str>) -> (Vec<u8>, Written) {
    let mut bin = Bin::default();
    let mut w = Written::default();
    let rig = skeleton.map(Rig::new);
    let fallback = rig.as_ref().and_then(|r| r.parents.iter().position(Option::is_none)).unwrap_or(0);

    let shared = mesh.shared.as_ref().and_then(|g| attrs(&mut bin, g, &mesh.bone_assignments, &mut w, rig.as_ref(), fallback));
    let mut primitives = vec![];
    for s in &mesh.submeshes {
        let a = if s.use_shared { shared.clone() } else { s.geometry.as_ref().and_then(|g| attrs(&mut bin, g, &s.bone_assignments, &mut w, rig.as_ref(), fallback)) };
        let (Some(a), true) = (a, s.operation == OP_TRIANGLE_LIST) else {
            w.skipped += 1;
            continue;
        };
        let bytes: Vec<u8> = s.indices.iter().flat_map(|i| i.to_le_bytes()).collect();
        let v = bin.view(&bytes, Some(34963));
        let idx = bin.accessor(v, UNSIGNED_INT, s.indices.len(), "SCALAR", None);
        w.triangles += s.indices.len() / 3;
        primitives.push(json!({"attributes": a, "indices": idx, "mode": 4, "material": 0}));
    }
    w.primitives = primitives.len();

    let mut nodes = vec![];
    let mut scene = vec![];
    let mut skins = vec![];
    let mut anims = vec![];
    if let Some(rig) = &rig {
        w.bones = rig.bind.len();
        for (i, b) in rig.bind.iter().enumerate() {
            let children: Vec<usize> = (0..rig.parents.len()).filter(|&c| rig.parents[c] == Some(i)).collect();
            let mut node = json!({
                "name": rig.names[i],
                "translation": b.translation.to_array(),
                "rotation": b.rotation.to_array(),
                "scale": b.scale.to_array(),
            });
            if !children.is_empty() {
                node["children"] = json!(children);
            }
            nodes.push(node);
            if rig.parents[i].is_none() {
                scene.push(i);
            }
        }
        let ibm: Vec<[f32; 16]> = rig.inverse_bind().iter().map(|m| m.to_cols_array()).collect();
        let ibm = bin.floats(&ibm, "MAT4", None, false);
        skins.push(json!({"joints": (0..rig.bind.len()).collect::<Vec<_>>(), "inverseBindMatrices": ibm}));

        let skel = skeleton.expect("rig comes from the skeleton");
        let mut list: Vec<_> = skel.animations.iter().filter(|a| animations.is_none_or(|names| names.iter().any(|n| n == &a.name))).collect();
        if let Some(f) = first {
            list.sort_by_key(|a| a.name != f);
        }
        for anim in list {
            let (mut channels, mut samplers) = (vec![], vec![]);
            for track in &anim.tracks {
                let Some(i) = rig.index_of_handle(track.bone) else { continue };
                let keys: Vec<_> = track.keyframes.iter().fold(vec![], |mut acc: Vec<&kenshi_io::ogre_skeleton::Keyframe>, k| {
                    if acc.last().is_none_or(|l| k.time > l.time) {
                        acc.push(k);
                    }
                    acc
                });
                if keys.is_empty() {
                    continue;
                }
                let b = rig.bind[i];
                let times: Vec<[f32; 1]> = keys.iter().map(|k| [k.time]).collect();
                let input = bin.floats(&times, "SCALAR", None, true);
                let t: Vec<[f32; 3]> = keys.iter().map(|k| (b.translation + Vec3::from(k.translate)).to_array()).collect();
                let mut prev: Option<Quat> = None;
                let r: Vec<[f32; 4]> = keys
                    .iter()
                    .map(|k| {
                        let q = Quat::from_xyzw(k.rotation[0], k.rotation[1], k.rotation[2], k.rotation[3]).normalize();
                        let mut q = (b.rotation * q).normalize();
                        // keep neighbours on the same side, so engines interpolate the short way
                        if prev.is_some_and(|p| p.dot(q) < 0.0) {
                            q = -q;
                        }
                        prev = Some(q);
                        q.to_array()
                    })
                    .collect();
                for (path, out) in [("translation", bin.floats(&t, "VEC3", None, false)), ("rotation", bin.floats(&r, "VEC4", None, false))] {
                    samplers.push(json!({"input": input, "output": out, "interpolation": "LINEAR"}));
                    channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": i, "path": path}}));
                }
                if keys.iter().any(|k| k.scale.is_some_and(|s| s != [1.0; 3])) {
                    let s: Vec<[f32; 3]> = keys.iter().map(|k| (b.scale * Vec3::from(k.scale.unwrap_or([1.0; 3]))).to_array()).collect();
                    let out = bin.floats(&s, "VEC3", None, false);
                    samplers.push(json!({"input": input, "output": out, "interpolation": "LINEAR"}));
                    channels.push(json!({"sampler": samplers.len() - 1, "target": {"node": i, "path": "scale"}}));
                }
            }
            if !channels.is_empty() {
                anims.push(json!({"name": anim.name, "channels": channels, "samplers": samplers}));
            }
        }
        w.animations = anims.len();
    }
    let mut mesh_node = json!({"name": name, "mesh": 0});
    if !skins.is_empty() {
        mesh_node["skin"] = json!(0);
    }
    nodes.push(mesh_node);
    scene.insert(0, nodes.len() - 1);

    let mut doc = json!({
        "asset": {"version": "2.0", "generator": "okran-convert"},
        "scene": 0,
        "scenes": [{"nodes": scene}],
        "nodes": nodes,
        "meshes": [{"name": name, "primitives": primitives}],
        "materials": [{"name": "default", "pbrMetallicRoughness": {"metallicFactor": 0.0, "roughnessFactor": 0.8}}],
        "buffers": [{"byteLength": 0}],
        "bufferViews": [],
        "accessors": [],
    });
    while !bin.data.len().is_multiple_of(4) {
        bin.data.push(0);
    }
    doc["buffers"][0]["byteLength"] = json!(bin.data.len());
    doc["bufferViews"] = json!(bin.views);
    doc["accessors"] = json!(bin.accessors);
    if !skins.is_empty() {
        doc["skins"] = json!(skins);
    }
    if !anims.is_empty() {
        doc["animations"] = json!(anims);
    }
    let mut text = serde_json::to_vec(&doc).expect("json");
    while !text.len().is_multiple_of(4) {
        text.push(b' ');
    }
    let total = 12 + 8 + text.len() + 8 + bin.data.len();
    let mut out = Vec::with_capacity(total);
    out.extend_from_slice(b"glTF");
    out.extend_from_slice(&2u32.to_le_bytes());
    out.extend_from_slice(&(total as u32).to_le_bytes());
    out.extend_from_slice(&(text.len() as u32).to_le_bytes());
    out.extend_from_slice(b"JSON");
    out.extend_from_slice(&text);
    out.extend_from_slice(&(bin.data.len() as u32).to_le_bytes());
    out.extend_from_slice(b"BIN\0");
    out.extend_from_slice(&bin.data);
    (out, w)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn weights_keep_the_four_strongest_and_sum_to_one() {
        let a = |vertex, bone, weight| BoneAssignment { vertex, bone, weight };
        let assigned = [a(0, 1, 0.1), a(0, 2, 0.4), a(0, 3, 0.2), a(0, 4, 0.2), a(0, 5, 0.1)];
        let (j, w) = weights(2, &assigned, &|h| Some(h as usize), 0);
        assert_eq!(j[0][0], 2);
        assert!((w[0].iter().sum::<f32>() - 1.0).abs() < 1e-6);
        assert_eq!((j[1], w[1]), ([0; 4], [1.0, 0.0, 0.0, 0.0]));
    }
}
