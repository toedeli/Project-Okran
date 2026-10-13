//! Kenshi's game-data files (FCS `.base`, `.mod`, zone and save files) as text: a folder with a
//! `header.json` and one JSON file per record, numbered in the file's order. Packing the folder
//! gives back the original file byte for byte, so a mod can live in git and be edited by hand or
//! by an agent, and still load in Kenshi and its tools.
//!
//! A record file holds `type` (the FCS type name, or its number when unknown), `string_id`,
//! `name`, `id`, `lead`, `datatype`, then whichever of `bools`, `floats`, `ints`, `vec3s`,
//! `vec4s`, `strings`, `files`, `references` and `instances` it has. Fields keep the file's order;
//! a section whose keys repeat is written as `[key, value]` pairs instead of an object. A float
//! is the shortest number that reads back to the same bits; one that is not finite is the string
//! `"f32:<hex bits>"`. A reference is `{"target": string_id, "values": [a, b, c]}`.

use kenshi_data::ItemType;
use kenshi_io::fcs::{self, File, Header, Instance, Record, Reference};
use serde_json::{Map, Value, json};
use std::path::{Path, PathBuf};

type Result<T> = std::result::Result<T, String>;

fn num(x: f32) -> Value {
    if !x.is_finite() {
        return Value::String(format!("f32:{:08x}", x.to_bits()));
    }
    // Rust prints the shortest decimal that reads back as this f32; JSON readers parse to f64
    // first, so keep it only if that path also lands on the same bits
    let short = format!("{x}").parse::<f64>().ok().filter(|d| (*d as f32).to_bits() == x.to_bits());
    serde_json::Number::from_f64(short.unwrap_or(x as f64)).map_or(Value::Null, Value::Number)
}

fn f32_of(v: &Value) -> Result<f32> {
    match v {
        Value::Number(n) => n.as_f64().map(|d| d as f32).ok_or_else(|| format!("not a float: {n}")),
        Value::String(s) if s.starts_with("f32:") => u32::from_str_radix(&s[4..], 16).map(f32::from_bits).map_err(|e| e.to_string()),
        _ => Err(format!("not a float: {v}")),
    }
}

fn i32_of(v: &Value) -> Result<i32> {
    v.as_i64().and_then(|n| i32::try_from(n).ok()).ok_or_else(|| format!("not an i32: {v}"))
}

fn str_of(v: &Value) -> Result<String> {
    v.as_str().map(str::to_string).ok_or_else(|| format!("not a string: {v}"))
}

fn arr<'a>(v: &'a Value, what: &str) -> Result<&'a Vec<Value>> {
    v.as_array().ok_or_else(|| format!("{what}: not a list"))
}

fn floats<const N: usize>(v: &Value) -> Result<[f32; N]> {
    let a = arr(v, "floats")?;
    if a.len() != N {
        return Err(format!("expected {N} floats, got {}", a.len()));
    }
    let mut out = [0.0; N];
    for (o, x) in out.iter_mut().zip(a) {
        *o = f32_of(x)?;
    }
    Ok(out)
}

/// A section of (key, value) pairs: an object, or `[key, value]` pairs when a key repeats.
fn section<T>(pairs: &[(String, T)], val: impl Fn(&T) -> Value) -> Value {
    let mut seen = std::collections::HashSet::new();
    if pairs.iter().all(|(k, _)| seen.insert(k.as_str())) {
        Value::Object(pairs.iter().map(|(k, v)| (k.clone(), val(v))).collect())
    } else {
        Value::Array(pairs.iter().map(|(k, v)| json!([k, val(v)])).collect())
    }
}

fn section_of<T>(v: Option<&Value>, val: impl Fn(&Value) -> Result<T>) -> Result<Vec<(String, T)>> {
    match v {
        None => Ok(vec![]),
        Some(Value::Object(m)) => m.iter().map(|(k, x)| Ok((k.clone(), val(x)?))).collect(),
        Some(Value::Array(a)) => a
            .iter()
            .map(|p| match p.as_array().map(Vec::as_slice) {
                Some([k, x]) => Ok((str_of(k)?, val(x)?)),
                _ => Err(format!("expected [key, value], got {p}")),
            })
            .collect(),
        Some(x) => Err(format!("expected an object, got {x}")),
    }
}

pub fn type_name(kind: i32) -> Option<&'static str> {
    ItemType::from_i32(kind).map(ItemType::name)
}

fn kind_of(v: &Value) -> Result<i32> {
    match v {
        Value::String(s) => (0..200)
            .find(|&k| type_name(k) == Some(s.as_str()))
            .ok_or_else(|| format!("unknown record type {s}")),
        _ => i32_of(v),
    }
}

pub fn record_json(r: &Record) -> Value {
    let mut m = Map::new();
    m.insert("type".into(), type_name(r.kind).map_or_else(|| json!(r.kind), |n| json!(n)));
    m.insert("string_id".into(), json!(r.string_id));
    m.insert("name".into(), json!(r.name));
    m.insert("id".into(), json!(r.id));
    m.insert("lead".into(), json!(r.lead));
    m.insert("datatype".into(), json!(r.datatype));
    let mut put = |k: &str, v: Value, empty: bool| {
        if !empty {
            m.insert(k.into(), v);
        }
    };
    put("bools", section(&r.bools, |b| json!(b)), r.bools.is_empty());
    put("floats", section(&r.floats, |f| num(*f)), r.floats.is_empty());
    put("ints", section(&r.ints, |i| json!(i)), r.ints.is_empty());
    put("vec3s", section(&r.vec3s, |v| Value::Array(v.iter().map(|x| num(*x)).collect())), r.vec3s.is_empty());
    put("vec4s", section(&r.vec4s, |v| Value::Array(v.iter().map(|x| num(*x)).collect())), r.vec4s.is_empty());
    put("strings", section(&r.strings, |s| json!(s)), r.strings.is_empty());
    put("files", section(&r.files, |s| json!(s)), r.files.is_empty());
    let refs = |list: &Vec<Reference>| Value::Array(list.iter().map(|x| json!({"target": x.target, "values": x.values})).collect());
    put("references", section(&r.references, refs), r.references.is_empty());
    let inst = |i: &Instance| {
        json!({
            "id": i.id,
            "target": i.target,
            "position": i.position.iter().map(|x| num(*x)).collect::<Vec<_>>(),
            "rotation": i.rotation.iter().map(|x| num(*x)).collect::<Vec<_>>(),
            "states": i.states,
        })
    };
    put("instances", Value::Array(r.instances.iter().map(inst).collect()), r.instances.is_empty());
    Value::Object(m)
}

pub fn record_from(v: &Value) -> Result<Record> {
    let o = v.as_object().ok_or("a record must be an object")?;
    let get = |k: &str| o.get(k).ok_or_else(|| format!("record has no {k}"));
    let refs = |x: &Value| {
        arr(x, "references")?
            .iter()
            .map(|r| {
                let values = arr(&r["values"], "values")?;
                if values.len() != 3 {
                    return Err(format!("a reference needs 3 values: {r}"));
                }
                Ok(Reference { target: str_of(&r["target"])?, values: [i32_of(&values[0])?, i32_of(&values[1])?, i32_of(&values[2])?] })
            })
            .collect::<Result<Vec<_>>>()
    };
    let instances = match o.get("instances") {
        None => vec![],
        Some(x) => arr(x, "instances")?
            .iter()
            .map(|i| {
                Ok(Instance {
                    id: str_of(&i["id"])?,
                    target: str_of(&i["target"])?,
                    position: floats::<3>(&i["position"])?,
                    rotation: floats::<4>(&i["rotation"])?,
                    states: arr(&i["states"], "states")?.iter().map(str_of).collect::<Result<_>>()?,
                })
            })
            .collect::<Result<Vec<_>>>()?,
    };
    Ok(Record {
        lead: i32_of(get("lead")?)?,
        kind: kind_of(get("type")?)?,
        id: i32_of(get("id")?)?,
        name: str_of(get("name")?)?,
        string_id: str_of(get("string_id")?)?,
        datatype: get("datatype")?.as_u64().and_then(|n| u32::try_from(n).ok()).ok_or("datatype: not a u32")?,
        bools: section_of(o.get("bools"), |x| x.as_bool().ok_or_else(|| format!("not a bool: {x}")))?,
        floats: section_of(o.get("floats"), f32_of)?,
        ints: section_of(o.get("ints"), i32_of)?,
        vec3s: section_of(o.get("vec3s"), floats::<3>)?,
        vec4s: section_of(o.get("vec4s"), floats::<4>)?,
        strings: section_of(o.get("strings"), str_of)?,
        files: section_of(o.get("files"), str_of)?,
        references: section_of(o.get("references"), refs)?,
        instances,
    })
}

pub fn header_json(f: &File) -> Value {
    let h = &f.header;
    let mut latin1: Vec<&String> = f.latin1.iter().collect();
    latin1.sort();
    json!({
        "filetype": h.filetype,
        "version": h.version,
        "author": h.author,
        "description": h.description,
        "dependencies": h.dependencies,
        "references": h.references,
        "tail": h.tail.iter().map(|b| format!("{b:02x}")).collect::<String>(),
        "id_counter": h.id_counter,
        "trailer": f.trailer,
        "trailers": f.trailers,
        "has_trailer": f.has_trailer,
        "latin1": latin1,
    })
}

fn ints(v: &Value) -> Result<Vec<i32>> {
    arr(v, "list")?.iter().map(i32_of).collect()
}

/// The file from its header and records. In a v17 file a record's stored length (`lead`, when
/// nonzero) is worked out again, so an edited record stays valid; other files keep `lead` as it
/// was (v16 files hold other values there, which the game does not check).
pub fn file_from(h: &Value, records: Vec<Record>) -> Result<File> {
    let tail = str_of(&h["tail"])?;
    let tail = (0..tail.len()).step_by(2).map(|i| u8::from_str_radix(&tail[i..i + 2], 16).map_err(|e| e.to_string())).collect::<Result<Vec<u8>>>()?;
    let header = Header {
        filetype: i32_of(&h["filetype"])?,
        version: i32_of(&h["version"])?,
        author: str_of(&h["author"])?,
        description: str_of(&h["description"])?,
        dependencies: str_of(&h["dependencies"])?,
        references: str_of(&h["references"])?,
        tail,
        id_counter: i32_of(&h["id_counter"])?,
        record_count: records.len() as i32,
    };
    let mut f = File {
        header,
        records,
        trailer: ints(&h["trailer"])?,
        trailers: arr(&h["trailers"], "trailers")?.iter().map(ints).collect::<Result<_>>()?,
        has_trailer: h["has_trailer"].as_bool().ok_or("has_trailer: not a bool")?,
        latin1: arr(&h["latin1"], "latin1")?.iter().map(str_of).collect::<Result<_>>()?,
    };
    for i in 0..f.records.len() {
        if f.header.filetype == fcs::FILETYPE_V17 && f.records[i].lead != 0 {
            f.records[i].lead = record_len(&f, i);
        }
    }
    Ok(f)
}

/// The byte length of record `i` as written, its `lead` field included.
fn record_len(f: &File, i: usize) -> i32 {
    let one = File {
        header: Header {
            filetype: fcs::FILETYPE_V16,
            version: 0,
            author: String::new(),
            description: String::new(),
            dependencies: String::new(),
            references: String::new(),
            tail: vec![],
            id_counter: 0,
            record_count: 1,
        },
        records: vec![f.records[i].clone()],
        trailer: vec![],
        trailers: vec![],
        has_trailer: false,
        latin1: f.latin1.clone(),
    };
    // a v16 header with empty strings is 32 bytes
    (fcs::write(&one).len() - 32) as i32
}

fn file_name(i: usize, r: &Record) -> String {
    let ty = type_name(r.kind).map_or_else(|| r.kind.to_string(), str::to_string);
    let name: String = r.name.chars().map(|c| if c.is_alphanumeric() || " -_.()'".contains(c) { c } else { '_' }).take(60).collect();
    format!("{i:05} {ty} {}.json", name.trim_end_matches(['.', ' ']))
}

/// Writes `f` as `dir/header.json` and `dir/records/<number> <type> <name>.json`.
pub fn unpack(f: &File, dir: &Path) -> std::io::Result<usize> {
    let records = dir.join("records");
    if records.exists() {
        std::fs::remove_dir_all(&records)?;
    }
    std::fs::create_dir_all(&records)?;
    std::fs::write(dir.join("header.json"), serde_json::to_string_pretty(&header_json(f))? + "\n")?;
    for (i, r) in f.records.iter().enumerate() {
        std::fs::write(records.join(file_name(i, r)), serde_json::to_string_pretty(&record_json(r))? + "\n")?;
    }
    Ok(f.records.len())
}

/// Reads a folder written by [`unpack`] (records in file-name order) back into a file.
pub fn pack(dir: &Path) -> Result<File> {
    let read = |p: &Path| -> Result<Value> {
        let text = std::fs::read_to_string(p).map_err(|e| format!("{}: {e}", p.display()))?;
        serde_json::from_str(&text).map_err(|e| format!("{}: {e}", p.display()))
    };
    let header = read(&dir.join("header.json"))?;
    let mut files: Vec<PathBuf> = std::fs::read_dir(dir.join("records"))
        .map_err(|e| e.to_string())?
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x == "json"))
        .collect();
    files.sort();
    let records = files.iter().map(|p| read(p).and_then(|v| record_from(&v).map_err(|e| format!("{}: {e}", p.display())))).collect::<Result<Vec<_>>>()?;
    file_from(&header, records)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn floats_keep_their_bits_and_read_short() {
        for x in [0.1f32, -0.0, 1.0e-40, 3.402_823_5e38, 16777217.0, 0.3, f32::NAN, f32::INFINITY, -1.5e-7] {
            let v = num(x);
            let text = serde_json::to_string(&v).unwrap();
            let back = f32_of(&serde_json::from_str(&text).unwrap()).unwrap();
            assert_eq!(back.to_bits(), x.to_bits(), "{x} -> {text}");
        }
        assert_eq!(serde_json::to_string(&num(0.1)).unwrap(), "0.1");
    }

    #[test]
    fn a_record_survives_text() {
        let r = Record {
            lead: 0,
            kind: 0,
            id: 7,
            name: "Storage: Plate".into(),
            string_id: "123-gamedata.base".into(),
            datatype: 0x8000_0000,
            bools: vec![("a".into(), true), ("a".into(), false)],
            floats: vec![("weight".into(), 0.1)],
            ints: vec![("cost".into(), -5)],
            vec3s: vec![("offset".into(), [1.0, 2.0, 3.5])],
            vec4s: vec![],
            strings: vec![("text".into(), "é \"q\"".into())],
            files: vec![("mesh".into(), "a/b.mesh".into())],
            references: vec![("parts".into(), vec![Reference { target: "9-x.mod".into(), values: [1, 0, 0] }])],
            instances: vec![Instance { id: "i".into(), target: "t".into(), position: [0.0; 3], rotation: [1.0, 0.0, 0.0, 0.0], states: vec!["s".into()] }],
        };
        let text = serde_json::to_string_pretty(&record_json(&r)).unwrap();
        assert_eq!(record_from(&serde_json::from_str(&text).unwrap()).unwrap(), r);
    }
}
