//! Kenshi's DDS textures surveyed for modern use: each file's format (from its header), whether
//! the engine's decoder reads it, how small zstd makes the GPU-ready data (the compression KTX2
//! files carry), and, for a sample, whether a PNG made from it decodes back to the same pixels.

use std::path::Path;

/// What a DDS header says, enough to name the format.
#[derive(Debug, Clone, PartialEq)]
pub struct DdsInfo {
    pub width: u32,
    pub height: u32,
    pub mips: u32,
    /// DXT1, DXT5, ATI2, DX10:<dxgi>, RGBA32, RGB24, L8, A8L8, A8, ...
    pub format: String,
    pub cubemap: bool,
    pub volume: bool,
}

fn u32_at(b: &[u8], at: usize) -> u32 {
    b.get(at..at + 4).map_or(0, |s| u32::from_le_bytes(s.try_into().expect("4 bytes")))
}

pub fn dds_info(b: &[u8]) -> Option<DdsInfo> {
    if b.len() < 128 || &b[..4] != b"DDS " {
        return None;
    }
    let (flags, bits) = (u32_at(b, 80), u32_at(b, 88));
    let fourcc = &b[84..88];
    let format = if flags & 0x4 != 0 {
        if fourcc == b"DX10" { format!("DX10:{}", u32_at(b, 128)) } else { String::from_utf8_lossy(fourcc).trim_end_matches('\0').to_string() }
    } else if flags & 0x40 != 0 {
        format!("{}{bits}", if flags & 0x1 != 0 { "RGBA" } else { "RGB" })
    } else if flags & 0x20000 != 0 {
        if flags & 0x1 != 0 { format!("A8L8 ({bits} bit)") } else { format!("L{bits}") }
    } else if flags & 0x2 != 0 {
        format!("A{bits}")
    } else {
        format!("flags {flags:#x}")
    };
    let caps2 = u32_at(b, 112);
    Some(DdsInfo {
        width: u32_at(b, 16),
        height: u32_at(b, 12),
        mips: u32_at(b, 28).max(1),
        format,
        cubemap: caps2 & 0x200 != 0,
        volume: caps2 & 0x0020_0000 != 0,
    })
}

/// A PNG of the pixels, and whether it decodes back to exactly them.
pub fn png_round_trip(rgba: &kenshi_io::dds::Rgba) -> (usize, bool) {
    let mut out = Vec::new();
    let flat: Vec<u8> = rgba.pixels.iter().flatten().copied().collect();
    let img = image::RgbaImage::from_raw(rgba.width, rgba.height, flat.clone());
    let Some(img) = img else { return (0, false) };
    if img.write_to(&mut std::io::Cursor::new(&mut out), image::ImageFormat::Png).is_err() {
        return (0, false);
    }
    let same = image::load_from_memory_with_format(&out, image::ImageFormat::Png).is_ok_and(|back| back.to_rgba8().into_raw() == flat);
    (out.len(), same)
}

/// The zstd size of `data` at `level`.
pub fn zstd_len(data: &[u8], level: i32) -> usize {
    zstd::bulk::compress(data, level).map_or(data.len(), |c| c.len())
}

pub fn is_dds(p: &Path) -> bool {
    p.extension().is_some_and(|x| x.eq_ignore_ascii_case("dds"))
}
