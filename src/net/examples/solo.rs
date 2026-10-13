//! The lockstep world run alone, both players' scripted orders applied locally, printing every
//! tenth tick's state hash and one hash per person: run it twice and diff to find where two
//! processes part. `cargo run --release -p kenshi-net --example solo -- <install> [ticks] [orders]`

use kenshi_game::sim::Sim;
use kenshi_net::hash::state_hash;
use kenshi_net::play::{apply, scripted};

fn main() {
    let a: Vec<String> = std::env::args().skip(1).collect();
    let ticks: u64 = a.get(1).map_or(120, |n| n.parse().unwrap());
    let orders = a.get(2).is_none_or(|s| s != "no-orders");
    let mut sim = Sim::load(std::path::Path::new(&a[0]), 34, 46, 40, &["player:3".into(), "player:3".into()]);
    println!("load {:016x}", state_hash(&sim));
    for i in 0..ticks {
        if orders {
            for me in 0..2 {
                for c in scripted(&sim, me, i) {
                    apply(&mut sim, &c);
                }
            }
        }
        sim.step();
        // KENSHI_TRACE: player A's squad every second from the attack order on
        if std::env::var("KENSHI_TRACE").is_ok() && i >= 1795 && i % 60 == 0 {
            let squads = kenshi_net::play::player_squads(&sim);
            for &id in &squads[0] {
                let (p, f) = (&sim.folk[id], &sim.arena.f[id]);
                let goal = p.ai.current.as_ref().map(|c| c.task);
                let target = f.focus.map(|t| (t, sim.folk[t].mover.position_units(), sim.arena.f[t].team, sim.folk[t].inside));
                eprintln!("t{i} {id} at {:?} team {} active {} goal {goal:?} foes {:?} focus {target:?}", p.mover.position_units().map(|v| v.round()), f.team, f.active, f.foes);
            }
        }
        if (i + 1) % 10 == 0 {
            println!("tick {i} {:016x}", state_hash(&sim));
            for p in &sim.folk {
                let f = &sim.arena.f[p.id];
                println!(
                    "  {} pos {:08x} {:08x} {:08x} vel {:08x} {:08x} {:08x} down {} blood {:08x} task {:?}",
                    p.id,
                    p.mover.position[0].to_bits(),
                    p.mover.position[1].to_bits(),
                    p.mover.position[2].to_bits(),
                    p.mover.velocity[0].to_bits(),
                    p.mover.velocity[1].to_bits(),
                    p.mover.velocity[2].to_bits(),
                    p.down,
                    f.medical.blood.to_bits(),
                    p.ai.queue.first().map(|s| s.task)
                );
            }
        }
    }
}
