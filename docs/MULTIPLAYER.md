# Multiplayer: what is tested

The simulation (`kenshi_game::sim::Sim`) builds without an engine and is bit-identical between
processes and machines, so two players can share one game. `okran-net` (crate `src/net`) tests
this headless, with scripted players, over TCP. It is a test tool, not yet a way to play.

## Running it

On the machine that hosts (player A), then on the other (player B), with the same options:

```bash
okran-net host --install <Kenshi folder> --seconds 600 --extra kit:Vagabond --extra "squad:Fog raid@1"
```

```bash
okran-net join <host address>:7777 --install <Kenshi folder> --seconds 600 --extra kit:Vagabond --extra "squad:Fog raid@1"
```

- `--mode lockstep` (default): both run the simulation and send only orders, plus a state hash
  every second to catch a desync.
- `--mode snapshot`: the host runs the simulation and sends everyone's state 20 times a second.
- `--realtime`, `--speed`, `--lag-ms` and `--delay` test latency; `--poke-at` plants a desync.
- `--extra` passes the simulation's start options (`kit:<squad template>` arms the player squads,
  `squad:<template>@1` spawns a squad).

## Why it stays in step

- Every hash map in the simulation uses a fixed hasher (`kenshi_io::det`). std's maps are keyed
  at random per process; with them, two processes parted 9 ticks after the first move order.
- The simulation has a fixed 1/60 s tick, one seeded random generator and no threads.

## Results (2026-10-14, vanilla Kenshi data, zone 34.46 The Hub)

| Test | Result |
|-|-|
| Two processes, one PC, 10 game minutes, armed squads and a Fog raid | 600 hash checks matched; 77 blows, 11 down, 6 dead |
| Linux server and Windows PC over the LAN, same scenario | 600 checks matched; same final state as the Windows-only run |
| A person nudged by 1 mm on one peer | desync caught at the next check |
| Lockstep traffic | 861 bytes per game second each way |
| Snapshots, 92 people, 20 per second, no delta compression | 49,975 bytes per game second |
| 80 ms each way, input delay 100 ms | no stalls |
| 150 ms each way, input delay 100 ms / 200 ms | 73.6 s of stalls in 60 game seconds / none |
| 3x speed, 80 ms each way, input delay 6 ticks / 18 ticks | 57 s of stalls / none: the delay has to grow with game speed |

## Known gaps

- Player characters without a weapon never land a blow.
- Both player squads share the player faction, so one cannot attack the other.
- Orders are direct calls in the app; multiplayer needs them as commands stamped with a tick.
- Ragdolls run in the app's frame loop and write positions back into the simulation.
- Lua scripts can call `math.random` and `os.time`, which would break lockstep.
