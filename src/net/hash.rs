//! A hash of the simulation's state, compared between peers to catch a desync: every person's
//! exact position and velocity bits, down and dead, blood and each body part's flesh, plus the
//! tick, the clock and the random generator.

use kenshi_game::sim::Sim;

pub fn state_hash(sim: &Sim) -> u64 {
    let mut h = Fnv(0xcbf2_9ce4_8422_2325);
    h.bytes(&sim.tick.to_le_bytes());
    h.bytes(&sim.t.to_bits().to_le_bytes());
    h.bytes(&sim.rng.0.to_le_bytes());
    h.bytes(&(sim.folk.len() as u64).to_le_bytes());
    for p in &sim.folk {
        for v in p.mover.position.iter().chain(&p.mover.velocity) {
            h.bytes(&v.to_bits().to_le_bytes());
        }
        h.bytes(&[p.down as u8 | (p.dead as u8) << 1]);
        if let Some(f) = sim.arena.f.get(p.id) {
            h.bytes(&f.medical.blood.to_bits().to_le_bytes());
            for part in &f.medical.parts {
                h.bytes(&part.flesh.to_bits().to_le_bytes());
            }
        }
    }
    h.0
}

/// FNV-1a: the same on every machine, unlike std's randomly keyed hasher.
struct Fnv(u64);

impl Fnv {
    fn bytes(&mut self, b: &[u8]) {
        for &x in b {
            self.0 ^= x as u64;
            self.0 = self.0.wrapping_mul(0x0000_0100_0000_01b3);
        }
    }
}
