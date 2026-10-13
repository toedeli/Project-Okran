//! Hash maps and sets whose iteration order is the same in every process and on every machine
//! (std's are keyed at random per process). The simulation uses these so that peers in lockstep,
//! running the same orders, stay bit-identical.

pub type HashMap<K, V> = std::collections::HashMap<K, V, rustc_hash::FxBuildHasher>;
pub type HashSet<T> = std::collections::HashSet<T, rustc_hash::FxBuildHasher>;
