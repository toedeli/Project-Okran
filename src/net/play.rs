//! The two ways to play together, driven by scripted players so that a run can be checked.
//!
//! Lockstep: both peers run the same `Sim`. Each tick a player's orders go out for the tick
//! `delay` ticks ahead; a peer runs tick `t` once it holds both players' orders for it, applies
//! them in player order, steps, and every `hash_every` ticks sends its state hash to compare.
//!
//! Snapshots: the host runs the `Sim` with both players' orders and sends everyone's state every
//! `snap_every` ticks; the client runs no simulation and only keeps the latest state.

use crate::hash::state_hash;
use crate::link::Link;
use crate::proto::{Command, Msg, PersonState};
use kenshi_game::sim::Sim;
use std::collections::BTreeMap;
use std::io;
use std::time::{Duration, Instant};

pub struct Options {
    pub ticks: u64,
    pub delay: u64,
    pub hash_every: u64,
    pub snap_every: u64,
    /// Run at game speed (sim.dt per tick over `speed`) instead of as fast as possible.
    pub realtime: bool,
    pub speed: f32,
    /// (testing) This peer nudges a person by a millimetre after this tick: a planted desync.
    pub poke_at: Option<u64>,
}

#[derive(Debug, Default)]
pub struct Report {
    pub ticks: u64,
    pub wall: Duration,
    /// Time spent waiting for the other player's orders.
    pub stalled: Duration,
    pub sent: u64,
    pub received: u64,
    pub hashes_compared: u64,
    /// The first tick whose hashes differed.
    pub desync: Option<u64>,
    pub final_hash: u64,
    /// The other player left before the end.
    pub cut_short: bool,
}

/// The squads the players control: each player platoon's people, in platoon order.
pub fn player_squads(sim: &Sim) -> Vec<Vec<usize>> {
    let mut by: BTreeMap<usize, Vec<usize>> = BTreeMap::new();
    for p in &sim.folk {
        if let (true, Some(pl)) = (p.player, p.platoon) {
            by.entry(pl).or_default().push(p.id);
        }
    }
    by.into_values().collect()
}

/// What player `me` orders at tick `i`: a move; then player A attacks the nearest townsperson
/// and player B attacks player A's squad; then a stop. Each player only knows their own orders;
/// the other learns them over the network.
pub fn scripted(sim: &Sim, me: usize, i: u64) -> Vec<Command> {
    let squads = player_squads(sim);
    let Some(squad) = squads.get(me) else {
        return vec![];
    };
    let ids: Vec<u16> = squad.iter().map(|&i| i as u16).collect();
    let here = sim.folk[squad[0]].mover.position_units();
    let (move_at, attack_at) = if me == 0 { (30, 1800) } else { (90, 2400) };
    if i == move_at {
        let to = if me == 0 { [here[0] + 400.0, here[1], here[2]] } else { [here[0], here[1], here[2] + 400.0] };
        vec![Command::MoveSquad { ids, to }]
    } else if i == attack_at {
        let whom = if me == 0 { nearest_townsperson(sim, here) } else { squads.first().and_then(|a| a.first().copied()) };
        whom.map_or_else(Vec::new, |whom| vec![Command::Attack { ids, whom: whom as u16 }])
    } else if i == 9000 {
        vec![Command::Stop { ids }]
    } else {
        vec![]
    }
}

fn nearest_townsperson(sim: &Sim, from: [f32; 3]) -> Option<usize> {
    let d = |p: &kenshi_game::sim::Person| {
        let q = p.mover.position_units();
        (q[0] - from[0]).powi(2) + (q[2] - from[2]).powi(2)
    };
    sim.folk
        .iter()
        .filter(|p| !p.player && !p.down && !p.dead && p.inside.is_none())
        .min_by(|a, b| d(a).total_cmp(&d(b)))
        .map(|p| p.id)
}

pub fn apply(sim: &mut Sim, c: &Command) {
    let n = sim.folk.len();
    let ids = |v: &[u16]| v.iter().map(|&i| i as usize).filter(|&i| i < n).collect::<Vec<_>>();
    match c {
        Command::MoveSquad { ids: v, to } => sim.order_move_squad(&ids(v), *to),
        Command::Attack { ids: v, whom } if (*whom as usize) < n => sim.order_attack(&ids(v), *whom as usize),
        Command::Attack { .. } => {}
        Command::Stop { ids: v } => sim.order_stop(&ids(v)),
    }
}

/// Exchanges Hello: both peers must have loaded the same world.
pub fn handshake(link: &mut Link, world: u64) -> io::Result<()> {
    link.send(&Msg::Hello { version: crate::proto::VERSION, world });
    match link.recv()? {
        Msg::Hello { version, world: theirs } => {
            if version != crate::proto::VERSION {
                return Err(io::Error::other(format!("the other player runs protocol {version}")));
            }
            if theirs != world {
                return Err(io::Error::other(format!("different worlds: mine {world:016x}, theirs {theirs:016x}")));
            }
            Ok(())
        }
        m => Err(io::Error::other(format!("expected Hello, got {m:?}"))),
    }
}

struct Theirs {
    orders: BTreeMap<u64, Vec<Command>>,
    hashes: BTreeMap<u64, u64>,
    done: bool,
}

impl Theirs {
    fn take(&mut self, m: Msg) {
        match m {
            Msg::Orders { tick, commands } => {
                self.orders.insert(tick, commands);
            }
            Msg::Hash { tick, hash } => {
                self.hashes.insert(tick, hash);
            }
            Msg::Done => self.done = true,
            _ => {}
        }
    }
}

fn compare(mine: &mut BTreeMap<u64, u64>, theirs: &mut BTreeMap<u64, u64>, r: &mut Report) {
    let both: Vec<u64> = mine.keys().filter(|t| theirs.contains_key(t)).copied().collect();
    for t in both {
        let (a, b) = (mine.remove(&t), theirs.remove(&t));
        r.hashes_compared += 1;
        if a != b && r.desync.is_none_or(|d| t < d) {
            r.desync = Some(t);
        }
    }
}

fn pace(began: Instant, ticks: u64, tick: f32) {
    let due = began + Duration::from_secs_f64(ticks as f64 * tick as f64);
    let now = Instant::now();
    if due > now {
        std::thread::sleep(due - now);
    }
}

pub fn lockstep(sim: &mut Sim, link: &mut Link, me: usize, o: &Options) -> io::Result<Report> {
    let mut r = Report::default();
    let mut mine_orders: BTreeMap<u64, Vec<Command>> = (0..o.delay).map(|t| (t, vec![])).collect();
    let mut theirs = Theirs { orders: (0..o.delay).map(|t| (t, vec![])).collect(), hashes: BTreeMap::new(), done: false };
    let mut mine_hashes = BTreeMap::new();
    let began = Instant::now();
    'run: for i in 0..o.ticks {
        let cmds = scripted(sim, me, i);
        link.send(&Msg::Orders { tick: i + o.delay, commands: cmds.clone() });
        mine_orders.insert(i + o.delay, cmds);
        let waited = Instant::now();
        while !theirs.orders.contains_key(&i) {
            if theirs.done {
                r.cut_short = true;
                break 'run;
            }
            theirs.take(link.recv()?);
        }
        r.stalled += waited.elapsed();
        while let Some(m) = link.try_recv() {
            theirs.take(m?);
        }
        let by_player = [mine_orders.remove(&i).unwrap_or_default(), theirs.orders.remove(&i).unwrap_or_default()];
        let (first, second) = if me == 0 { (&by_player[0], &by_player[1]) } else { (&by_player[1], &by_player[0]) };
        for c in first.iter().chain(second) {
            apply(sim, c);
        }
        sim.step();
        r.ticks = i + 1;
        if o.poke_at == Some(i) {
            sim.folk[0].mover.position[0] += 0.001;
        }
        if (i + 1) % o.hash_every == 0 || i + 1 == o.ticks {
            let h = state_hash(sim);
            mine_hashes.insert(i, h);
            link.send(&Msg::Hash { tick: i, hash: h });
        }
        compare(&mut mine_hashes, &mut theirs.hashes, &mut r);
        if r.desync.is_some() {
            break;
        }
        if o.realtime {
            pace(began, i + 1, sim.dt / o.speed);
        }
    }
    r.wall = began.elapsed();
    link.send(&Msg::Done);
    while !theirs.done {
        match link.recv() {
            Ok(m) => theirs.take(m),
            Err(_) => break,
        }
        compare(&mut mine_hashes, &mut theirs.hashes, &mut r);
    }
    compare(&mut mine_hashes, &mut theirs.hashes, &mut r);
    r.final_hash = state_hash(sim);
    r.sent = link.sent;
    r.received = link.received();
    Ok(r)
}

fn snapshot_of(sim: &Sim) -> Vec<PersonState> {
    sim.folk
        .iter()
        .map(|p| PersonState {
            id: p.id as u16,
            position: p.mover.position,
            velocity: p.mover.velocity,
            flags: p.down as u8 | (p.dead as u8) << 1,
        })
        .collect()
}

/// A hash of a snapshot's people, so host and client can check they hold the same state.
pub fn snapshot_hash(people: &[PersonState]) -> u64 {
    let mut h: u64 = 0xcbf2_9ce4_8422_2325;
    let mut eat = |b: &[u8]| {
        for &x in b {
            h ^= x as u64;
            h = h.wrapping_mul(0x0000_0100_0000_01b3);
        }
    };
    for p in people {
        eat(&p.id.to_le_bytes());
        for v in p.position.iter().chain(&p.velocity) {
            eat(&v.to_bits().to_le_bytes());
        }
        eat(&[p.flags]);
    }
    h
}

/// The server: runs the simulation with both players' scripted orders and sends snapshots.
/// (harness) The client's orders are scripted here, as if they had arrived on time.
pub fn snapshot_host(sim: &mut Sim, link: &mut Link, o: &Options) -> io::Result<Report> {
    let mut r = Report::default();
    let began = Instant::now();
    for i in 0..o.ticks {
        for me in 0..2 {
            for c in scripted(sim, me, i) {
                apply(sim, &c);
            }
        }
        sim.step();
        r.ticks = i + 1;
        if (i + 1) % o.snap_every == 0 || i + 1 == o.ticks {
            link.send(&Msg::Snapshot { tick: i, people: snapshot_of(sim) });
        }
        if o.realtime {
            pace(began, i + 1, sim.dt / o.speed);
        }
    }
    r.wall = began.elapsed();
    let last = snapshot_of(sim);
    r.final_hash = snapshot_hash(&last);
    link.send(&Msg::Hash { tick: r.ticks, hash: r.final_hash });
    link.send(&Msg::Done);
    while let Ok(m) = link.recv() {
        if m == Msg::Done {
            break;
        }
    }
    r.sent = link.sent;
    r.received = link.received();
    Ok(r)
}

/// The client: keeps the latest snapshot and checks it against the host's last hash.
pub fn snapshot_client(link: &mut Link) -> io::Result<Report> {
    let mut r = Report::default();
    let began = Instant::now();
    let mut latest: Vec<PersonState> = vec![];
    loop {
        match link.recv()? {
            Msg::Snapshot { tick, people } => {
                r.ticks = tick + 1;
                latest = people;
            }
            Msg::Hash { hash, .. } => {
                r.hashes_compared = 1;
                if snapshot_hash(&latest) != hash {
                    r.desync = Some(r.ticks);
                }
            }
            Msg::Done => break,
            _ => {}
        }
    }
    link.send(&Msg::Done);
    r.wall = began.elapsed();
    r.final_hash = snapshot_hash(&latest);
    r.sent = link.sent;
    r.received = link.received();
    Ok(r)
}
