//! An external git-dream policy: `w` independent chains from the round's root, each `d` attempts deep.
//!
//! This is the fixed exploration policy of the Dream-RSI paper (W workspaces, each refining its own
//! latest attempt): the tree is the root with `w` branches, and only the root and the ends of
//! branches are continued. Recorded this way, a round is wide: a policy that follows some of the
//! branches, in another order or less deep, replays without leaving the record.
//!
//!     cargo build --release --example chains
//!     # .git-dream.toml:
//!     # [policies.external.chains]
//!     # command = "/path/to/target/release/examples/chains"
//!     # grid = { w = [1, 2, 4, 8], d = [1, 2, 3, 6, 12] }
use std::io::{BufRead, Write};

fn main() {
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let w = v["params"]["w"].as_u64().unwrap_or(4);
        let d = v["params"]["d"].as_u64().unwrap_or(u64::MAX);
        let nodes = v["tree"]["nodes"].as_array().unwrap();
        let mut depth = vec![0u64; nodes.len()];
        let mut reply = None;
        if nodes[0]["n_exp"].as_u64().unwrap_or(0) < w {
            reply = Some(0);
        } else {
            for (i, n) in nodes.iter().enumerate().skip(1) {
                depth[i] = depth[n["parent"].as_u64().unwrap() as usize] + 1;
                // the end of a branch: finished, never continued, and not yet `d` deep
                if n["n_exp"] == 0 && n["status"] != "pending" && depth[i] < d {
                    reply = Some(i);
                    break;
                }
            }
        }
        writeln!(out, "{}", reply.map_or("null".to_string(), |i| i.to_string())).unwrap();
        out.flush().unwrap();
    }
}
