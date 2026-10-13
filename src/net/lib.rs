//! Multiplayer over the engine-free simulation, over TCP: deterministic lockstep (every peer runs
//! the same `Sim`; only the players' orders and state hashes cross the network) or server
//! snapshots (one peer runs the `Sim` and sends everyone's state). `okran-net` runs either with
//! scripted players and reports whether the peers stayed in step.

pub mod hash;
pub mod link;
pub mod play;
pub mod proto;
