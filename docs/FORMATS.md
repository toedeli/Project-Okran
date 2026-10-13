# Kenshi's formats, made modern

`okran-convert` (crate `src/convert`) reads the player's own Kenshi install and writes modern,
editable files on their machine; nothing of Kenshi is shipped. Each command checks its result
against the original or an independent reader.

| Command | What it does |
|-|-|
| `data-unpack <file> <folder>` | A `.base`, `.mod`, zone or save file as `header.json` plus `records/<n> <TYPE> <name>.json` |
| `data-pack <folder> <file>` | The folder back into the file |
| `data-check <files or folders> [--ext base,mod]` | Unpack, pack and compare bytes for every file found |
| `model <mesh> <out.glb> --install <Kenshi>` | One Ogre mesh as glTF, skinned with every animation of its skeleton; `--expect` writes joint positions to check an importer |
| `models <folder> --install <Kenshi> [--write <dir>]` | Every mesh under a folder as glTF |
| `textures <folder>` | Every DDS: format, decode, zstd size, lossless PNG check on a sample |
| `world <Kenshi> [--heightmap out.png]` | The heightmap as 16-bit PNG; every Havok tagfile, Scythe `.phs` and PhysX NxuStream file through the readers |

## Results (2026-10-14, a copy of the game folder and a 377-mod install)

The game-folder copy is not pristine (it holds 110 more DXT5 textures than a clean install), so
counts are this install's, not Kenshi's exact file list.

| Format | Becomes | Result |
|-|-|-|
| `.base`, `.mod` | JSON, one file per record | 4 core files (59,558 records) and 377 mods (73,926 records) byte-identical |
| Zones, levels, saves | JSON | 709 of 715 zone and level files and a save (129 files, 16,081 records) byte-identical; `zone.15.15`, `zone.16.17`, `zone.19.19`, `zone.19.23`, `zone.8.17` and `roads.level` use FCS versions 10 and 13, not read yet |
| `.translation`, `.bod2`, `.body` | JSON | 8 translations (176,647 records) and 77 of 102 body files byte-identical; 25 body files use FCS versions 10 and 13 |
| `.mesh`, `.skeleton` | glTF 2.0 | 3,277 game and 4,950 mod meshes; `human_male` (30 bones, 174 animations) in Blender 5.1.1 puts joints within 1.3e-5 units of `kenshi_anim`; 50 sampled files import with exact triangle and bone counts |
| `.dds` | PNG to edit; zstd or KTX2 to load | 2,017 game and 3,754 mod textures decode; PNG pixel-identical; zstd level 6 takes the game's 8,127 MB to 4,464 MB |
| Wwise `.bnk` | Ogg Vorbis (`kenshi-audio`, `decode_all` example) | 24 banks, 2,686 sounds, 5.1 hours, none failed |
| `fullmap.tif` | 16-bit PNG | 16,385 x 16,385, pixel-identical, 537 MB to 206 MB |
| Havok `.hkt` | read (`kenshi_io::hkt`) | 3,995 of 3,995 |
| Scythe `.phs` | read (`kenshi_io::phs`) | 85 of 86; `meshes/crate.phs` is cut short |
| PhysX NxuStream `.xml` | read (`kenshi_io::nxu`) | 1,081 of 1,136 with shapes; the other 55 have none (mostly triggers) |
| PhysX NxuStream `.bin` | not read | 857 files; no reader yet |

## Notes for whoever extends this

- Floats in the JSON are the shortest decimal that reads back to the same bits through an f64
  parse (serde_json with `float_roundtrip`); non-finite ones are `"f32:<hex>"`.
- A section whose keys repeat is written as `[key, value]` pairs, so order and duplicates survive.
- In v17 files a record's `lead` (its byte length) is recomputed on pack, so edits stay valid (a
  name made 46 characters longer packs with `lead` 431 to 477 and passes the v17 check); in
  v16 files `lead` holds other values the game does not check, and is kept as read.
- glTF keyframes fold Ogre's bind-relative keys into local transforms as `kenshi_anim::Rig::sample`
  does; neighbouring rotations are kept on one side so engines interpolate the short way.
- Not written yet: LOD levels (1,389 vanilla meshes have them), face poses, materials and textures
  in the glTF, cubemap faces past the first, a KTX2 writer, FCS versions 10 and 13, a binary
  NxuStream reader, and an open navmesh format (rebuild with Recast rather than convert Havok).
- Bevy can already load KTX2 with the original BC blocks and zstd (`bevy_image` `ktx2` and
  `zstd_rust` features); Okran enables only `dds`.
- Converting on the player's machine is the line: converted copies of Kenshi's files are still
  Lo-Fi's and must never be shipped. Load fonts from the install; some carry personal-use licences.
