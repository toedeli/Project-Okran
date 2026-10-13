//! Two players over the network on the engine-free simulation, with scripted orders:
//! `okran-net host --install <Kenshi>` on one machine, `okran-net join <host:port> --install
//! <Kenshi>` on the other, the same options on both. Prints whether the peers stayed in step.

use clap::{Args, Parser, Subcommand, ValueEnum};
use kenshi_game::sim::Sim;
use kenshi_net::hash::state_hash;
use kenshi_net::link::Link;
use kenshi_net::play::{self, Options, Report};
use std::net::{TcpListener, TcpStream};
use std::path::PathBuf;
use std::time::{Duration, Instant};

#[derive(Parser)]
#[command(about = "Two players over the network on Okran's simulation, with scripted orders")]
struct Cli {
    #[command(subcommand)]
    role: Role,
}

#[derive(Subcommand)]
enum Role {
    /// Wait for the other player (player A).
    Host {
        #[arg(long, default_value_t = 7777)]
        port: u16,
        #[command(flatten)]
        common: Common,
    },
    /// Connect to the host (player B).
    Join {
        /// The host's address, such as 192.168.1.20:7777.
        addr: String,
        #[command(flatten)]
        common: Common,
    },
}

#[derive(Clone, Copy, PartialEq, ValueEnum)]
enum Mode {
    Lockstep,
    Snapshot,
}

#[derive(Args)]
struct Common {
    /// The Kenshi folder.
    #[arg(long)]
    install: PathBuf,
    #[arg(long, value_enum, default_value_t = Mode::Lockstep)]
    mode: Mode,
    /// The zone, as X.Y.
    #[arg(long, default_value = "34.46")]
    zone: String,
    /// Townsfolk to spawn.
    #[arg(long, default_value_t = 40)]
    people: usize,
    /// Characters in each player's squad.
    #[arg(long, default_value_t = 3)]
    squad: usize,
    /// Game seconds to play.
    #[arg(long, default_value_t = 300.0)]
    seconds: f32,
    /// Lockstep: ticks between giving an order and its running.
    #[arg(long, default_value_t = 6)]
    delay: u64,
    /// Lockstep: compare state hashes every this many ticks.
    #[arg(long, default_value_t = 60)]
    hash_every: u64,
    /// Snapshots: send one every this many ticks.
    #[arg(long, default_value_t = 3)]
    snap_every: u64,
    /// Added one-way latency for this peer's messages, in milliseconds.
    #[arg(long, default_value_t = 0)]
    lag_ms: u64,
    /// Run at game speed instead of as fast as possible.
    #[arg(long)]
    realtime: bool,
    #[arg(long, default_value_t = 1.0)]
    speed: f32,
    /// (testing) Nudge a person after this tick on this peer: a planted desync.
    #[arg(long)]
    poke_at: Option<u64>,
    /// More of the simulation's start options after the two player squads, such as
    /// "squad:Fog raid@1" (a raid on the town). Repeatable; the same on both peers.
    #[arg(long = "extra")]
    extras: Vec<String>,
}

static BLOWS: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);

fn main() -> std::process::ExitCode {
    let cli = Cli::parse();
    // counts the blows that land; reads only, so the peers stay in step
    kenshi_combat::hooks::set_hit_hook(Box::new(|_| {
        BLOWS.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    }));
    let (me, stream, c) = match cli.role {
        Role::Host { port, common } => {
            let listener = TcpListener::bind(("0.0.0.0", port)).expect("listen");
            eprintln!("okran-net: waiting for player B on port {port}");
            let (s, from) = listener.accept().expect("accept");
            eprintln!("okran-net: player B joined from {from}");
            (0, s, common)
        }
        Role::Join { addr, common } => {
            let tried = Instant::now();
            let s = loop {
                match TcpStream::connect(&addr) {
                    Ok(s) => break s,
                    Err(e) if tried.elapsed() < Duration::from_secs(60) => {
                        eprintln!("okran-net: {addr}: {e}; trying again");
                        std::thread::sleep(Duration::from_millis(500));
                    }
                    Err(e) => panic!("{addr}: {e}"),
                }
            };
            (1, s, common)
        }
    };
    let mut link = Link::new(stream, Duration::from_millis(c.lag_ms)).expect("link");
    let (zx, zy) = c.zone.split_once('.').map(|(x, y)| (x.parse().expect("zone X"), y.parse().expect("zone Y"))).expect("zone X.Y");
    let mut extras = vec![format!("player:{}", c.squad), format!("player:{}", c.squad)];
    extras.extend(c.extras.iter().cloned());
    let loading = Instant::now();
    let mut sim = (me == 0 || c.mode == Mode::Lockstep).then(|| Sim::load(&c.install, zx, zy, c.people, &extras));
    let load_time = loading.elapsed();
    let world = sim.as_ref().map_or(0, state_hash);
    if c.mode == Mode::Lockstep
        && let Err(e) = play::handshake(&mut link, world)
    {
        eprintln!("okran-net: {e}");
        return std::process::ExitCode::FAILURE;
    }
    let ticks = sim.as_ref().map_or(0, |s| (c.seconds / s.dt).round() as u64);
    let o = Options {
        ticks,
        delay: c.delay,
        hash_every: c.hash_every,
        snap_every: c.snap_every,
        realtime: c.realtime,
        speed: c.speed,
        poke_at: c.poke_at,
    };
    let result = match (c.mode, sim.as_mut()) {
        (Mode::Lockstep, Some(sim)) => play::lockstep(sim, &mut link, me, &o),
        (Mode::Snapshot, Some(sim)) => play::snapshot_host(sim, &mut link, &o),
        (Mode::Snapshot, None) => play::snapshot_client(&mut link),
        (Mode::Lockstep, None) => unreachable!("lockstep loads the world on both peers"),
    };
    link.close();
    match result {
        Ok(r) => {
            report(me, c.mode, &r, load_time, sim.as_ref().map(|s| s.folk.len()), sim.as_ref().map_or(1.0 / 60.0, |s| s.dt));
            if let Some(s) = &sim {
                let down = s.folk.iter().filter(|p| p.down && !p.dead).count();
                let dead = s.folk.iter().filter(|p| p.dead).count();
                let blows = BLOWS.load(std::sync::atomic::Ordering::Relaxed);
                println!("  {blows} blows landed; at the end {down} people down, {dead} dead");
            }
            if r.desync.is_some() || r.cut_short {
                std::process::ExitCode::FAILURE
            } else {
                std::process::ExitCode::SUCCESS
            }
        }
        Err(e) => {
            eprintln!("okran-net: {e}");
            std::process::ExitCode::FAILURE
        }
    }
}

fn report(me: usize, mode: Mode, r: &Report, load: Duration, people: Option<usize>, dt: f32) {
    let player = ["A (host)", "B (joined)"][me];
    let game_secs = r.ticks as f64 * dt as f64;
    let mode = if mode == Mode::Lockstep { "lockstep" } else { "snapshots" };
    println!("player {player}, {mode}");
    if let Some(n) = people {
        println!("  world: {n} people, loaded in {:.1} s", load.as_secs_f64());
    }
    println!(
        "  played {} ticks ({game_secs:.0} game seconds) in {:.1} s; waited on the other player {:.2} s",
        r.ticks,
        r.wall.as_secs_f64(),
        r.stalled.as_secs_f64()
    );
    if game_secs > 0.0 {
        println!(
            "  sent {} bytes ({:.0} bytes per game second), received {} bytes ({:.0} per game second)",
            r.sent,
            r.sent as f64 / game_secs,
            r.received,
            r.received as f64 / game_secs
        );
    }
    match (mode, r.desync, r.cut_short) {
        (_, Some(t), _) => println!("  DESYNC at tick {t} ({} hash checks before it)", r.hashes_compared.saturating_sub(1)),
        (_, None, true) => println!("  the other player left early"),
        ("snapshots", None, false) if me == 0 => println!("  final state {:016x}", r.final_hash),
        ("snapshots", None, false) => println!("  the last snapshot matched the host's final state {:016x}", r.final_hash),
        (_, None, false) => println!("  in step: {} hash checks matched; final state {:016x}", r.hashes_compared, r.final_hash),
    }
}
