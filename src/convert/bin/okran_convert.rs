//! Converts Kenshi's formats from your own install into modern, editable ones, and checks that the
//! conversions give back the original.

use clap::{Parser, Subcommand};
use kenshi_convert::fcs_text;
use kenshi_io::fcs;
use std::path::{Path, PathBuf};
use std::time::Instant;

#[derive(Parser)]
#[command(about = "Converts Kenshi's formats from your own install into modern, editable ones")]
struct Cli {
    #[command(subcommand)]
    job: Job,
}

#[derive(Subcommand)]
enum Job {
    /// A .base, .mod, zone or save file into a folder of JSON (header.json, records/*.json).
    DataUnpack { file: PathBuf, folder: PathBuf },
    /// A folder written by data-unpack back into a file.
    DataPack { folder: PathBuf, file: PathBuf },
    /// Round-trips every FCS file under the given files or folders through text and compares the
    /// bytes with the original.
    DataCheck {
        paths: Vec<PathBuf>,
        /// Where to unpack (kept for inspection); a temporary folder is used otherwise.
        #[arg(long)]
        keep: Option<PathBuf>,
        /// Only files with these extensions (comma-separated).
        #[arg(long, default_value = "base,mod")]
        ext: String,
    },
    /// One Ogre mesh as glTF, skinned and animated when it names a skeleton the install has.
    Model {
        mesh: PathBuf,
        out: PathBuf,
        /// The Kenshi folder, to find the skeleton the mesh names.
        #[arg(long)]
        install: PathBuf,
        /// The animation to put first (an importer plays the first one).
        #[arg(long)]
        first: Option<String>,
        /// Write the world positions of these bones at three keyframes of the first animation, as
        /// Okran's own animation code works them out, to check an importer against.
        #[arg(long)]
        expect: Option<PathBuf>,
        #[arg(long, value_delimiter = ',', default_value = "Bip01 Head,Bip01 L Hand,Bip01 R Foot")]
        bones: Vec<String>,
    },
    /// Every Ogre mesh under a folder as glTF (skinned, without animations, when its skeleton is
    /// found), counting what converts.
    Models {
        folder: PathBuf,
        #[arg(long)]
        install: PathBuf,
        /// Write the .glb files here (mirroring the folders); otherwise they are only built.
        #[arg(long)]
        write: Option<PathBuf>,
    },
    /// The world's files: the heightmap as a 16-bit PNG (checked pixel for pixel), and every Havok
    /// navmesh tile and PhysX file through the readers.
    World {
        /// The Kenshi folder.
        install: PathBuf,
        /// Write the heightmap PNG here.
        #[arg(long)]
        heightmap: Option<PathBuf>,
    },
    /// Surveys every DDS texture under a folder: format, whether the decoder reads it, zstd size,
    /// and a lossless PNG check on a sample.
    Textures {
        folder: PathBuf,
        /// Make and check a PNG of every Nth texture that decodes.
        #[arg(long, default_value_t = 20)]
        png_every: usize,
        #[arg(long, default_value_t = 6)]
        zstd_level: i32,
    },
}

fn main() -> std::process::ExitCode {
    match Cli::parse().job {
        Job::DataUnpack { file, folder } => {
            let f = fcs::read(&file).unwrap_or_else(|e| panic!("{}: {e:?}", file.display()));
            let n = fcs_text::unpack(&f, &folder).expect("write");
            println!("{n} records into {}", folder.display());
        }
        Job::DataPack { folder, file } => match fcs_text::pack(&folder) {
            Ok(f) => {
                fcs::save(&f, &file).expect("write");
                println!("{} records into {}", f.records.len(), file.display());
            }
            Err(e) => {
                eprintln!("{e}");
                return std::process::ExitCode::FAILURE;
            }
        },
        Job::DataCheck { paths, keep, ext } => {
            let exts: Vec<String> = ext.split(',').map(|e| e.trim().to_lowercase()).collect();
            let mut files = vec![];
            for p in &paths {
                collect(p, &exts, &mut files);
            }
            files.sort();
            let work = keep.unwrap_or_else(|| std::env::temp_dir().join("okran-data-check"));
            return data_check(&files, &work);
        }
        Job::Textures { folder, png_every, zstd_level } => textures(&folder, png_every, zstd_level),
        Job::Model { mesh, out, install, first, expect, bones } => model(&mesh, &out, &install, first.as_deref(), expect.as_deref(), &bones),
        Job::Models { folder, install, write } => models(&folder, &install, write.as_deref()),
        Job::World { install, heightmap } => world(&install, heightmap.as_deref()),
    }
    std::process::ExitCode::SUCCESS
}

fn collect(p: &Path, exts: &[String], out: &mut Vec<PathBuf>) {
    if p.is_dir() {
        if let Ok(rd) = std::fs::read_dir(p) {
            for e in rd.flatten() {
                collect(&e.path(), exts, out);
            }
        }
    } else if p.extension().is_some_and(|x| exts.contains(&x.to_string_lossy().to_lowercase())) {
        out.push(p.to_path_buf());
    }
}

fn data_check(files: &[PathBuf], work: &Path) -> std::process::ExitCode {
    let began = Instant::now();
    let (mut parsed, mut plain_same, mut text_same, mut records, mut bytes, mut text_bytes) = (0, 0, 0, 0usize, 0u64, 0u64);
    let mut failures = vec![];
    for (i, path) in files.iter().enumerate() {
        let original = match std::fs::read(path) {
            Ok(b) => b,
            Err(e) => {
                failures.push(format!("{}: read: {e}", path.display()));
                continue;
            }
        };
        let f = match fcs::parse(&original) {
            Ok(f) => f,
            Err(e) => {
                failures.push(format!("{}: parse: {e:?}", path.display()));
                continue;
            }
        };
        parsed += 1;
        records += f.records.len();
        bytes += original.len() as u64;
        if fcs::write(&f) == original {
            plain_same += 1;
        } else {
            failures.push(format!("{}: read and write differ", path.display()));
        }
        let dir = work.join(format!("{i:04}"));
        let _ = fcs_text::unpack(&f, &dir);
        text_bytes += folder_size(&dir);
        match fcs_text::pack(&dir) {
            Ok(back) if fcs::write(&back) == original => text_same += 1,
            Ok(back) => {
                let at = f.records.iter().zip(&back.records).position(|(a, b)| a != b);
                let why = match at {
                    Some(k) => format!("record {k} '{}' differs:\n    was {:?}\n    now {:?}", f.records[k].name, f.records[k], back.records[k]),
                    None if f.header != back.header => format!("header differs: {:?} / {:?}", f.header, back.header),
                    None => "records and header equal; latin1 or trailer differ".into(),
                };
                failures.push(format!("{}: through text, bytes differ: {why}", path.display()));
            }
            Err(e) => failures.push(format!("{}: pack: {e}", path.display())),
        }
    }
    println!("{} files, {parsed} parsed, {records} records, {:.1} MB", files.len(), bytes as f64 / 1e6);
    println!("  read then write: {plain_same} byte-identical");
    println!("  through text and back: {text_same} byte-identical ({:.1} MB of JSON)", text_bytes as f64 / 1e6);
    println!("  {:.1} s", began.elapsed().as_secs_f64());
    for f in failures.iter().take(20) {
        println!("  FAIL {f}");
    }
    if failures.is_empty() { std::process::ExitCode::SUCCESS } else { std::process::ExitCode::FAILURE }
}

fn folder_size(p: &Path) -> u64 {
    std::fs::read_dir(p).map_or(0, |rd| {
        rd.flatten()
            .map(|e| {
                let p = e.path();
                if p.is_dir() { folder_size(&p) } else { e.metadata().map_or(0, |m| m.len()) }
            })
            .sum()
    })
}

#[derive(Default)]
struct FormatStats {
    files: usize,
    bytes: u64,
    decoded: usize,
    zstd: u64,
    cubemaps: usize,
}

fn textures(folder: &Path, png_every: usize, level: i32) {
    use kenshi_convert::texture::{dds_info, is_dds, png_round_trip, zstd_len};
    let began = Instant::now();
    let mut files = vec![];
    collect(folder, &["dds".to_string()], &mut files);
    files.retain(|p| is_dds(p));
    files.sort();
    let mut by: std::collections::BTreeMap<String, FormatStats> = Default::default();
    let (mut decoded_n, mut pngs, mut png_same, mut png_bytes, mut png_src) = (0usize, 0usize, 0usize, 0u64, 0u64);
    let mut fails = vec![];
    for p in &files {
        let Ok(b) = std::fs::read(p) else { continue };
        let Some(info) = dds_info(&b) else {
            fails.push(format!("{}: not a DDS", p.display()));
            continue;
        };
        let s = by.entry(info.format.clone()).or_default();
        s.files += 1;
        s.bytes += b.len() as u64;
        s.cubemaps += usize::from(info.cubemap || info.volume);
        let payload = if info.format.starts_with("DX10") { &b[148.min(b.len())..] } else { &b[128..] };
        s.zstd += zstd_len(payload, level) as u64 + 128;
        match kenshi_io::dds::decode(&b) {
            Ok(rgba) => {
                s.decoded += 1;
                decoded_n += 1;
                if decoded_n % png_every == 0 {
                    let (len, same) = png_round_trip(&rgba);
                    pngs += 1;
                    png_same += usize::from(same);
                    png_bytes += len as u64;
                    png_src += b.len() as u64;
                }
            }
            Err(e) => {
                if fails.len() < 8 {
                    fails.push(format!("{} ({}): {e}", p.display(), info.format));
                }
            }
        }
    }
    let total: u64 = by.values().map(|s| s.bytes).sum();
    let zstd: u64 = by.values().map(|s| s.zstd).sum();
    println!("{} DDS files, {:.0} MB; the decoder reads {decoded_n}", files.len(), total as f64 / 1e6);
    println!("{:<14} {:>6} {:>9} {:>8} {:>9} {:>6}", "format", "files", "MB", "decoded", "zstd MB", "cube");
    for (f, s) in &by {
        println!("{f:<14} {:>6} {:>9.1} {:>8} {:>9.1} {:>6}", s.files, s.bytes as f64 / 1e6, s.decoded, s.zstd as f64 / 1e6, s.cubemaps);
    }
    println!("zstd level {level}: {:.0} MB to {:.0} MB ({:.0}%)", total as f64 / 1e6, zstd as f64 / 1e6, 100.0 * zstd as f64 / total.max(1) as f64);
    println!("PNG sample: {pngs} textures, {png_same} decode back to identical pixels; {:.0} MB of DDS became {:.0} MB of PNG", png_src as f64 / 1e6, png_bytes as f64 / 1e6);
    println!("{:.0} s", began.elapsed().as_secs_f64());
    for f in &fails {
        println!("  not read: {f}");
    }
}

fn skeleton_for(mesh: &kenshi_io::ogre_mesh::Mesh, res: &kenshi_io::resources::Resources) -> Option<kenshi_io::ogre_skeleton::Skeleton> {
    let path = res.path(mesh.skeleton.as_ref()?)?;
    kenshi_io::ogre_skeleton::read(&path).ok()
}

fn model(mesh_path: &Path, out: &Path, install: &Path, first: Option<&str>, expect: Option<&Path>, bones: &[String]) {
    let res = kenshi_io::resources::Resources::index(install);
    let mesh = kenshi_io::ogre_mesh::read(mesh_path).unwrap_or_else(|e| panic!("{}: {e:?}", mesh_path.display()));
    let skel = skeleton_for(&mesh, &res);
    let name = mesh_path.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
    let (bytes, w) = kenshi_convert::gltf::glb(&name, &mesh, skel.as_ref(), None, first);
    std::fs::write(out, &bytes).expect("write");
    println!("{}: {:.1} MB, {w:?}", out.display(), bytes.len() as f64 / 1e6);
    if let (Some(path), Some(skel)) = (expect, &skel) {
        let rig = kenshi_anim::Rig::new(skel);
        let anim = first.and_then(|f| skel.animations.iter().find(|a| a.name == f)).or(skel.animations.first()).expect("an animation");
        let longest = anim.tracks.iter().max_by_key(|t| t.keyframes.len()).expect("a track");
        let n = longest.keyframes.len();
        let times: Vec<f32> = [0, n / 3, 2 * n / 3].iter().map(|&k| longest.keyframes[k].time).collect();
        let mut positions = serde_json::Map::new();
        for b in bones {
            let Some(i) = rig.names.iter().position(|x| x == b) else { continue };
            let at: Vec<[f32; 3]> = times.iter().map(|&t| rig.model_space(&rig.sample(anim, t))[i].w_axis.truncate().to_array()).collect();
            positions.insert(b.clone(), serde_json::json!(at));
        }
        let doc = serde_json::json!({"animation": anim.name, "times": times, "positions": positions, "written": format!("{w:?}")});
        std::fs::write(path, serde_json::to_string_pretty(&doc).expect("json")).expect("write");
        println!("expected positions of {} bones at {:?} in '{}'", positions.len(), times, anim.name);
    }
}

fn models(folder: &Path, install: &Path, write: Option<&Path>) {
    let began = Instant::now();
    let res = kenshi_io::resources::Resources::index(install);
    let mut files = vec![];
    collect(folder, &["mesh".to_string()], &mut files);
    files.sort();
    let mut skels: std::collections::HashMap<String, Option<kenshi_io::ogre_skeleton::Skeleton>> = Default::default();
    let (mut ok, mut skinned, mut bytes, mut glb_bytes, mut verts, mut skipped_parts) = (0, 0, 0u64, 0u64, 0usize, 0usize);
    let mut fails = vec![];
    for p in &files {
        bytes += std::fs::metadata(p).map_or(0, |m| m.len());
        let mesh = match kenshi_io::ogre_mesh::read(p) {
            Ok(m) => m,
            Err(e) => {
                fails.push(format!("{}: {e:?}", p.display()));
                continue;
            }
        };
        let skel = mesh.skeleton.clone().and_then(|s| skels.entry(s.to_lowercase()).or_insert_with(|| skeleton_for(&mesh, &res)).clone());
        let name = p.file_stem().map_or_else(String::new, |s| s.to_string_lossy().into_owned());
        let (glb, w) = kenshi_convert::gltf::glb(&name, &mesh, skel.as_ref(), Some(&[]), None);
        if w.primitives == 0 {
            fails.push(format!("{}: no triangle-list submesh ({} skipped)", p.display(), w.skipped));
            continue;
        }
        ok += 1;
        skinned += usize::from(w.bones > 0);
        verts += w.vertices;
        skipped_parts += w.skipped;
        glb_bytes += glb.len() as u64;
        if let Some(dir) = write {
            let rel = p.strip_prefix(folder).unwrap_or(p).with_extension("glb");
            let out = dir.join(rel);
            let _ = std::fs::create_dir_all(out.parent().expect("parent"));
            let _ = std::fs::write(out, &glb);
        }
    }
    println!("{} meshes, {:.0} MB; {ok} converted ({skinned} skinned), {verts} vertices, {:.0} MB of glb; {skipped_parts} submeshes left out", files.len(), bytes as f64 / 1e6, glb_bytes as f64 / 1e6);
    println!("{:.0} s", began.elapsed().as_secs_f64());
    for f in fails.iter().take(12) {
        println!("  not converted: {f}");
    }
    println!("  ({} not converted)", fails.len());
}

fn world(install: &Path, heightmap: Option<&Path>) {
    let land = install.join("data/newland/land");
    let began = Instant::now();
    let mut tiff = kenshi_io::tiff::Tiff::open(&land.join("fullmap.tif")).expect("fullmap.tif");
    let (w, h) = (tiff.width, tiff.height);
    let heights = tiff.window(0, 0, w, h, 1).expect("heights");
    let raw: Vec<u8> = heights.iter().flat_map(|v| v.to_be_bytes()).collect();
    let img = image::ImageBuffer::<image::Luma<u16>, Vec<u16>>::from_raw(w, h, heights.clone()).expect("size");
    let mut png = Vec::new();
    image::DynamicImage::ImageLuma16(img).write_to(&mut std::io::Cursor::new(&mut png), image::ImageFormat::Png).expect("png");
    let mut reader = image::ImageReader::with_format(std::io::Cursor::new(&png), image::ImageFormat::Png);
    reader.no_limits();
    let back = reader.decode().expect("read back").into_luma16().into_raw();
    let zstd = kenshi_convert::texture::zstd_len(&raw, 6);
    println!(
        "heightmap {w} x {h}, 16 bit: {:.0} MB raw, {:.0} MB as 16-bit PNG, {:.0} MB zstd; PNG reads back {}; {:.0} s",
        raw.len() as f64 / 1e6,
        png.len() as f64 / 1e6,
        zstd as f64 / 1e6,
        if back == heights { "identical" } else { "DIFFERENT" },
        began.elapsed().as_secs_f64()
    );
    if let Some(out) = heightmap {
        std::fs::write(out, &png).expect("write");
    }
    let mut files = vec![];
    collect(&install.join("data"), &["hkt".to_string(), "phs".to_string(), "nxu".to_string()], &mut files);
    let (mut hkt, mut hkt_ok, mut phs, mut phs_ok, mut nxu, mut nxu_ok) = (0, 0, 0, 0, 0, 0);
    let mut fails = vec![];
    for p in &files {
        let ext = p.extension().map(|e| e.to_string_lossy().to_lowercase()).unwrap_or_default();
        let ok: Result<(), String> = match ext.as_str() {
            "hkt" => {
                hkt += 1;
                std::fs::read(p).map_err(|e| e.to_string()).and_then(|b| kenshi_io::hkt::Tagfile::parse(&b).map(|_| ()).map_err(|e| e.to_string()))
            }
            "phs" => {
                phs += 1;
                kenshi_io::phs::read(p).map(|_| ()).map_err(|e| e.to_string())
            }
            _ => {
                nxu += 1;
                kenshi_io::nxu::read(p).map(|_| ()).map_err(|e| e.to_string())
            }
        };
        match ok {
            Ok(()) => match ext.as_str() {
                "hkt" => hkt_ok += 1,
                "phs" => phs_ok += 1,
                _ => nxu_ok += 1,
            },
            Err(e) => fails.push(format!("{}: {e}", p.display())),
        }
    }
    println!("Havok navmesh and other tagfiles: {hkt_ok} of {hkt} read; PhysX .phs: {phs_ok} of {phs}; PhysX .nxu: {nxu_ok} of {nxu}");
    for f in fails.iter().take(8) {
        println!("  not read: {f}");
    }
    println!("  ({} not read)", fails.len());
}
