//! A building's mesh parts and where each sits, as plain offsets: the renderer turns them into
//! transforms, and the simulation reads them (a turret's pivot) without an engine.

use kenshi_data::{GameData, GameDataContainer};

/// A building part's mesh with its material and the chain of nodes from the building down to it:
/// each node's "offset X/Y/Z" from its parent part (Ogre child nodes) and its swivel axis (a part
/// of "rotation function" 5 turns: "rotation axis" 1 about y, a turret's yaw; 0 about x, its pitch).
#[derive(Debug, Clone)]
pub struct PartChain<'a> {
    pub mesh: &'a str,
    pub material: Option<&'a GameData>,
    pub chain: Vec<([f32; 3], Option<i32>)>,
}

/// Every mesh part of a building record, its "destroyed mesh" in place of its mesh when
/// `destroyed`, walked down the "parts" references (at most nine levels).
pub fn part_chains<'a>(data: &'a GameDataContainer, b: &'a GameData, destroyed: bool) -> Vec<PartChain<'a>> {
    fn walk<'a>(
        data: &'a GameDataContainer,
        node: &'a GameData,
        inherited: Option<&'a GameData>,
        chain: &[([f32; 3], Option<i32>)],
        depth: u32,
        destroyed: bool,
        out: &mut Vec<PartChain<'a>>,
    ) {
        if depth > 8 {
            return;
        }
        for part in node.refs("parts").iter().filter_map(|(sid, _)| data.get(sid)) {
            let mat = part.refs("material").first().and_then(|(sid, _)| data.get(sid)).or(inherited);
            let f = |k: &str| part.floats.get(k).copied().unwrap_or(0.0);
            let swivel = (part.ints.get("rotation function") == Some(&5)).then(|| part.ints.get("rotation axis").copied().unwrap_or(1));
            let mut here = chain.to_vec();
            here.push(([f("offset X"), f("offset Y"), f("offset Z")], swivel));
            let ruin = destroyed
                .then(|| part.files.get("destroyed mesh"))
                .flatten()
                .filter(|s| s.to_lowercase().ends_with(".mesh"));
            if let Some(m) = ruin.or_else(|| part.files.get("phs or mesh").filter(|s| s.to_lowercase().ends_with(".mesh"))) {
                out.push(PartChain { mesh: m.as_str(), material: mat, chain: here.clone() });
            }
            walk(data, part, mat, &here, depth + 1, destroyed, out);
        }
    }
    let mut out = Vec::new();
    let mat = b.refs("material").first().and_then(|(sid, _)| data.get(sid));
    walk(data, b, mat, &[], 0, destroyed, &mut out);
    out
}
