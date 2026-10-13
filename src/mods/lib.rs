//! Lua mods. Every `*.lua` file in a folder runs once at start-up in its own environment: it sees
//! a sandboxed standard library (see [`sandbox`]), and its globals stay its own. A script hooks the game by defining a
//! function named after a hook; when several scripts do, they run in file-name order, each seeing
//! the last one's result. A script that fails to load or to run is reported and dropped, so a
//! broken mod never stops the game.
//!
//! Hooks:
//! - `on_hit(hit)`: one resolved hit, after armour and toughness, before the body part takes it.
//!   `hit.cut`, `hit.blunt`, `hit.pierce`, `hit.stun` and `hit.bleed` can be changed (below zero
//!   counts as zero); `hit.part` (the part's index), `hit.kind` ("torso", "leg", "arm" or
//!   "head") and `hit.toughness` (the victim's) are there to read.

use kenshi_combat::hooks::{HitEvent, set_hit_hook};
use mlua::{Function, HookTriggers, Lua, LuaOptions, StdLib, Table, VmState};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::Mutex;
use std::sync::atomic::{AtomicU32, Ordering};

/// Instructions a script may run in one go (loading, or one hook call), in thousands.
const BUDGET_THOUSANDS: u32 = 1_000;

/// A Lua with only what a game script needs: tables, strings, maths, utf8 and coroutines. There is
/// no file, OS, package or debug library, no loading of files or bytecode, no clock and no
/// unseeded randomness, so a mod cannot touch the machine and every player's game runs a script
/// the same way. Memory is capped, and a script that runs too long is stopped with an error.
pub fn sandbox() -> mlua::Result<(Lua, Arc<AtomicU32>)> {
    let lua = Lua::new_with(StdLib::TABLE | StdLib::STRING | StdLib::MATH | StdLib::UTF8 | StdLib::COROUTINE, LuaOptions::default())?;
    lua.set_memory_limit(64 << 20)?;
    let g = lua.globals();
    for name in ["dofile", "loadfile", "collectgarbage"] {
        g.set(name, mlua::Nil)?;
    }
    let math: Table = g.get("math")?;
    math.set("random", mlua::Nil)?;
    math.set("randomseed", mlua::Nil)?;
    lua.load(r#"local raw = load; load = function(chunk, name, _, env) return raw(chunk, name, "t", env) end"#).exec()?;
    let spent = Arc::new(AtomicU32::new(0));
    let counter = spent.clone();
    lua.set_hook(HookTriggers::new().every_nth_instruction(1000), move |_, _| {
        if counter.fetch_add(1, Ordering::Relaxed) >= BUDGET_THOUSANDS {
            return Err(mlua::Error::runtime("the script ran too long"));
        }
        Ok(VmState::Continue)
    })?;
    Ok((lua, spent))
}

const KINDS: [&str; 4] = ["torso", "leg", "arm", "head"];

/// A script's hook function, dropped after its first error.
struct Hook {
    script: String,
    function: Option<Function>,
}

/// Loads the scripts in `dir` and sets the hooks they define. Returns one line per script for the
/// log; a missing folder or an empty one means no mods.
pub fn install(dir: &Path) -> Vec<String> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .filter_map(|e| e.ok().map(|e| e.path()))
        .filter(|p| p.extension().is_some_and(|x| x.eq_ignore_ascii_case("lua")))
        .collect();
    files.sort();

    let mut log = Vec::new();
    let (lua, spent) = match sandbox() {
        Ok(l) => l,
        Err(e) => return vec![format!("lua mod: no scripting: {e}")],
    };
    let mut on_hit = Vec::new();
    for path in files {
        let script = path.file_name().map_or_else(String::new, |n| n.to_string_lossy().into_owned());
        spent.store(0, Ordering::Relaxed);
        match load(&lua, &path, &script) {
            Ok(env) => match env.get::<Option<Function>>("on_hit") {
                Ok(Some(function)) => {
                    log.push(format!("lua mod: {script} (on_hit)"));
                    on_hit.push(Hook { script, function: Some(function) });
                }
                _ => log.push(format!("lua mod: {script} (no hooks)")),
            },
            Err(e) => log.push(format!("lua mod: {script} failed to load: {e}")),
        }
    }
    if !on_hit.is_empty() {
        let hooks = Mutex::new((lua, on_hit));
        if !set_hit_hook(Box::new(move |hit| {
            spent.store(0, Ordering::Relaxed);
            run_on_hit(&hooks, hit)
        })) {
            log.push("lua mod: on_hit was set already; these scripts' on_hit is not used".into());
        }
    }
    log
}

fn load(lua: &Lua, path: &Path, script: &str) -> mlua::Result<Table> {
    let source = std::fs::read_to_string(path).map_err(mlua::Error::external)?;
    let env = lua.create_table()?;
    let meta = lua.create_table()?;
    meta.set("__index", lua.globals())?;
    env.set_metatable(Some(meta))?;
    lua.load(source).set_name(script).set_environment(env.clone()).exec()?;
    Ok(env)
}

fn run_on_hit(hooks: &Mutex<(Lua, Vec<Hook>)>, hit: &mut HitEvent) {
    let Ok(mut guard) = hooks.lock() else {
        return;
    };
    let (lua, list) = &mut *guard;
    for hook in list.iter_mut() {
        let Some(function) = &hook.function else {
            continue;
        };
        if let Err(e) = call_on_hit(lua, function, hit) {
            eprintln!("lua mod: {} on_hit failed and is off from now on: {e}", hook.script);
            hook.function = None;
        }
    }
}

fn call_on_hit(lua: &Lua, function: &Function, hit: &mut HitEvent) -> mlua::Result<()> {
    let t = lua.create_table()?;
    let d = &mut *hit.damage;
    t.set("cut", d.cut)?;
    t.set("blunt", d.blunt)?;
    t.set("pierce", d.pierce)?;
    t.set("stun", d.stun)?;
    t.set("bleed", d.bleed)?;
    t.set("part", hit.part)?;
    t.set("kind", KINDS.get(hit.kind as usize).copied().unwrap_or("torso"))?;
    t.set("toughness", hit.toughness)?;
    function.call::<()>(&t)?;
    let get = |k: &str| t.get::<f32>(k).map(|v| v.max(0.0));
    d.cut = get("cut")?;
    d.blunt = get("blunt")?;
    d.pierce = get("pierce")?;
    d.stun = get("stun")?;
    d.bleed = get("bleed")?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use kenshi_char::Damage;

    fn hook_from(source: &str) -> (Lua, Function) {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("t.lua");
        std::fs::write(&path, source).unwrap();
        let (lua, _) = sandbox().unwrap();
        let env = load(&lua, &path, "t.lua").unwrap();
        let f = env.get::<Function>("on_hit").unwrap();
        (lua, f)
    }

    #[test]
    fn a_script_changes_the_hit_it_is_given() {
        let (lua, f) = hook_from("function on_hit(hit) if hit.kind == 'head' then hit.cut = hit.cut * 3 end hit.blunt = -5 end");
        let mut d = Damage { cut: 10.0, blunt: 4.0, pierce: 1.0, stun: 2.0, bleed: 1.0 };
        call_on_hit(&lua, &f, &mut HitEvent { part: 0, kind: 3, toughness: 5.0, damage: &mut d }).unwrap();
        assert_eq!((d.cut, d.blunt, d.pierce, d.stun), (30.0, 0.0, 1.0, 2.0));
        let mut d = Damage { cut: 10.0, ..Damage::default() };
        call_on_hit(&lua, &f, &mut HitEvent { part: 1, kind: 0, toughness: 5.0, damage: &mut d }).unwrap();
        assert_eq!(d.cut, 10.0);
    }

    #[test]
    fn a_script_sees_only_its_own_globals() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.lua"), "shared = 1").unwrap();
        std::fs::write(dir.path().join("b.lua"), "function on_hit(hit) hit.cut = shared or 7 end").unwrap();
        let (lua, _) = sandbox().unwrap();
        load(&lua, &dir.path().join("a.lua"), "a.lua").unwrap();
        let b = load(&lua, &dir.path().join("b.lua"), "b.lua").unwrap();
        let f = b.get::<Function>("on_hit").unwrap();
        let mut d = Damage::default();
        call_on_hit(&lua, &f, &mut HitEvent { part: 0, kind: 0, toughness: 0.0, damage: &mut d }).unwrap();
        assert_eq!(d.cut, 7.0);
    }

    #[test]
    fn broken_scripts_are_reported_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        std::fs::write(dir.path().join("a.lua"), "function on_hit(hit").unwrap();
        std::fs::write(dir.path().join("b.lua"), "x = 1").unwrap();
        std::fs::write(dir.path().join("notes.txt"), "not a script").unwrap();
        let log = install(dir.path());
        assert_eq!(log.len(), 2, "{log:?}");
        assert!(log[0].starts_with("lua mod: a.lua failed to load"), "{}", log[0]);
        assert_eq!(log[1], "lua mod: b.lua (no hooks)");
        assert!(install(&dir.path().join("missing")).is_empty());
    }

    #[test]
    fn the_sandbox_has_no_way_out() {
        let (lua, _) = sandbox().unwrap();
        for nil in ["io", "os", "package", "debug", "require", "dofile", "loadfile", "math.random", "math.randomseed"] {
            let v: mlua::Value = lua.load(format!("return {nil}")).eval().unwrap_or(mlua::Nil);
            assert!(v.is_nil(), "{nil} is reachable");
        }
        let bytecode = lua.load("return load(string.dump(function() return 1 end))").eval::<mlua::Value>();
        assert!(bytecode.is_err() || bytecode.unwrap().is_nil(), "bytecode loads");
        assert_eq!(lua.load("return load('return 2')()").eval::<i32>().unwrap(), 2);
    }

    #[test]
    fn a_script_that_never_ends_is_stopped() {
        let (lua, spent) = sandbox().unwrap();
        spent.store(0, Ordering::Relaxed);
        let err = lua.load("while true do end").exec().unwrap_err().to_string();
        assert!(err.contains("ran too long"), "{err}");
    }

    #[test]
    fn a_runtime_error_is_an_error_not_a_panic() {
        let (lua, f) = hook_from("function on_hit(hit) hit.cut = hit.nothing.here end");
        let mut d = Damage { cut: 3.0, ..Damage::default() };
        assert!(call_on_hit(&lua, &f, &mut HitEvent { part: 0, kind: 0, toughness: 0.0, damage: &mut d }).is_err());
    }
}
