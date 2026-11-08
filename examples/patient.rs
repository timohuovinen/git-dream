//! An external git-dream policy written in Rust (see README, "Writing new policies").
//! Continue the best attempt; end the round after `p` calls without improvement.
//!
//!     cargo build --release --example patient
//!     # .git-dream.toml:
//!     # [policies.external.patient]
//!     # command = "/path/to/target/release/examples/patient"
//!     # grid = { p = [3, 5, 8] }
use std::io::{BufRead, Write};

fn main() {
    let mut out = std::io::stdout().lock();
    for line in std::io::stdin().lock().lines() {
        let v: serde_json::Value = serde_json::from_str(&line.unwrap()).unwrap();
        let p = v["params"]["p"].as_u64().unwrap_or(5) as usize;
        let min = v["tree"]["goal"] == "min";
        let nodes = v["tree"]["nodes"].as_array().unwrap();
        let cost = |n: &serde_json::Value| n["score"].as_f64().map(|s| if min { s } else { -s });
        let best = nodes.iter().filter(|n| cost(n).is_some()).min_by(|a, b| cost(a).unwrap().total_cmp(&cost(b).unwrap()));
        let reply = match best {
            Some(b) if nodes.len() - 1 - (b["id"].as_u64().unwrap() as usize) < p => b["id"].to_string(),
            _ => "null".into(),
        };
        writeln!(out, "{reply}").unwrap();
        out.flush().unwrap();
    }
}
