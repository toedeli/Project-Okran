//! Hooks through which code outside the simulation, such as a Lua mod, changes its results. Each
//! hook is one function per process, set once at start-up (the kenshi-mods crate sets them), so
//! the simulation itself never depends on a scripting language.

use kenshi_char::Damage;
use std::sync::OnceLock;

/// One resolved hit: after armour and toughness, before the body part takes it.
pub struct HitEvent<'a> {
    /// The part's index in the victim's body.
    pub part: usize,
    /// The part's kind: 0 torso, 1 leg, 2 arm, 3 head.
    pub kind: i32,
    /// The victim's toughness.
    pub toughness: f32,
    pub damage: &'a mut Damage,
}

pub type HitHook = Box<dyn Fn(&mut HitEvent) + Send + Sync>;

static HIT: OnceLock<HitHook> = OnceLock::new();

/// Sets the hit hook; false when one is set already.
pub fn set_hit_hook(hook: HitHook) -> bool {
    HIT.set(hook).is_ok()
}

pub(crate) fn on_hit(event: &mut HitEvent) {
    if let Some(hook) = HIT.get() {
        hook(event);
    }
}
