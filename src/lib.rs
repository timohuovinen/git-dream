//! git-dream: Git-native experimental search with replay diagnostics.
//!
//! * [`tree`]     discovery trees (what a round produces, what policies see)
//! * [`policies`] exploration policies (built-in families + external commands)
//! * [`agents`]   discovery agents (parameter space / command) and the evaluator
//! * [`memory`]   the git layout: commits, trailers, notes, hidden refs
//! * [`sim`]      running policies against worlds; replay and dreaming
//! * [`engine`]   configuration, the live world and the round loop

pub mod agents;
pub mod engine;
pub mod memory;
pub mod policies;
pub mod rng;
pub mod sim;
pub mod tree;
