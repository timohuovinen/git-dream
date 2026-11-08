//! Dream-RSI evaluator: the fixed, objective scorer for hash-table designs.
//!
//! Reads one design key per line on stdin, e.g. `W64-b2-L3-r0.5-o0.02`, and
//! prints one JSON object per line on stdout.
//!
//! Design knobs (see `dream/dream.py` for the discrete grid):
//!   W  SIMD group width (16 = SSE2, 64 = AVX-512 / one cache line of ctrl bytes)
//!   b  SIMD groups per level bucket (bucket β = b·W slots)
//!   L  number of funnel levels (0 = pure SwissTable-style linear group probing)
//!   r  geometric level-size ratio |A_{i+1}| / |A_i|
//!   o  overflow-region fraction of all slots
//!
//! Workload, per load factor α in `--loads` (default 0.95,0.98):
//!   fill a fixed-capacity table with `slots` u64->u64 entries to α  (insert ns)
//!   1M successful lookups of random inserted keys, random order   (hit ns)
//!   1M unsuccessful lookups of fresh keys                         (miss ns)
//! score = mean over α of (insert + hit + miss) / 3   [ns per op, lower is better]
//! Lookup phases are repeated `--reps` times and the minimum is kept.
//! If any insert reports `Full`, the design is invalid at that load.

use hashbench::Map;
use hashbench::funnel::{Funnel, FunnelConfig};
use hashbench::group::{G16, G64, Group};
use hashbench::hash::SplitMix;
use hashbench::wrappers::Brown;
use std::hint::black_box;
use std::io::{BufRead, Write};
use std::time::Instant;

#[derive(Clone, Debug)]
struct Design {
    w: usize,
    b: usize,
    l: usize,
    r: f64,
    o: f64,
}

fn parse(key: &str) -> Result<Design, String> {
    let mut d = Design { w: 16, b: 1, l: 0, r: 0.5, o: 0.05 };
    for part in key.trim().split('-') {
        let (k, v) = part.split_at(1);
        let bad = |e: &dyn std::fmt::Display| format!("bad {k}={v}: {e}");
        match k {
            "W" => d.w = v.parse().map_err(|e| bad(&e))?,
            "b" => d.b = v.parse().map_err(|e| bad(&e))?,
            "L" => d.l = v.parse().map_err(|e| bad(&e))?,
            "r" => d.r = v.parse().map_err(|e| bad(&e))?,
            "o" => d.o = v.parse().map_err(|e| bad(&e))?,
            _ => return Err(format!("unknown knob {part:?}")),
        }
    }
    if d.w != 16 && d.w != 64 {
        return Err("W must be 16 or 64".into());
    }
    Ok(d)
}

struct Phase {
    ins: f64,
    hit: f64,
    miss: f64,
    hit_probes: f64,
    miss_probes: f64,
    max_probes: u32,
}

const LOOKUPS: usize = 1 << 20;

fn time_lookups<M: Map>(m: &M, keys: &[u64], reps: usize) -> f64 {
    let mut best = f64::INFINITY;
    for _ in 0..reps {
        let t = Instant::now();
        let mut acc = 0u64;
        for &k in keys {
            acc = acc.wrapping_add(m.get(black_box(k)).unwrap_or(1));
        }
        black_box(acc);
        best = best.min(t.elapsed().as_nanos() as f64 / keys.len() as f64);
    }
    best
}

/// Fill `reps` fresh tables from `mk` (keeping the fastest fill), then time
/// lookups on the last one.
fn run_map<M: Map>(mk: impl Fn() -> M, load: f64, reps: usize, seed: u64, target_n: Option<usize>) -> Option<Phase> {
    let mut m = mk();
    let n = target_n.unwrap_or((m.slots() as f64 * load) as usize);
    let mut rng = SplitMix(seed);
    let keys: Vec<u64> = (0..n).map(|_| rng.next_u64()).collect();
    let mut ins = f64::INFINITY;
    for rep in 0..reps {
        if rep > 0 {
            m = mk();
        }
        let t = Instant::now();
        for &k in &keys {
            if m.insert(k, k).is_err() {
                return None;
            }
        }
        ins = ins.min(t.elapsed().as_nanos() as f64 / n as f64);
    }
    let hits: Vec<u64> = (0..LOOKUPS).map(|_| keys[(rng.next_u64() % n as u64) as usize]).collect();
    let misses: Vec<u64> = (0..LOOKUPS).map(|_| rng.next_u64()).collect();
    let hit = time_lookups(&m, &hits, reps);
    let miss = time_lookups(&m, &misses, reps);
    let sample = 20_000;
    let mut max_probes = 0;
    let mut hp = 0u64;
    let mut mp = 0u64;
    for i in 0..sample {
        let a = m.probe_count(hits[i]);
        let b = m.probe_count(misses[i]);
        max_probes = max_probes.max(a).max(b);
        hp += a as u64;
        mp += b as u64;
    }
    Some(Phase { ins, hit, miss, hit_probes: hp as f64 / sample as f64, miss_probes: mp as f64 / sample as f64, max_probes })
}

fn config<G: Group>(d: &Design, slots: usize) -> FunnelConfig {
    if d.l == 0 { FunnelConfig::swiss::<G>(slots) } else { FunnelConfig::simd_groups::<G>(slots, d.l, d.r, d.o, d.b) }
}

fn eval<G: Group>(d: &Design, slots: usize, loads: &[f64], reps: usize) -> String {
    let mut parts = Vec::new();
    let mut total = 0.0;
    let mut ok = true;
    let mut bytes = 0;
    let mut levels = 0;
    for (i, &a) in loads.iter().enumerate() {
        let m = Funnel::<G>::new(config::<G>(d, slots));
        bytes = m.heap_bytes();
        levels = m.num_levels();
        drop(m);
        match run_map(|| Funnel::<G>::new(config::<G>(d, slots)), a, reps, 1000 + i as u64, None) {
            Some(p) => {
                total += (p.ins + p.hit + p.miss) / 3.0;
                parts.push(format!(
                    "{{\"load\":{a},\"ins\":{:.2},\"hit\":{:.2},\"miss\":{:.2},\"hit_probes\":{:.3},\"miss_probes\":{:.3},\"max_probes\":{}}}",
                    p.ins, p.hit, p.miss, p.hit_probes, p.miss_probes, p.max_probes
                ));
            }
            None => {
                ok = false;
                parts.push(format!("{{\"load\":{a},\"full\":true}}"));
            }
        }
    }
    let score = if ok { format!("{:.3}", total / loads.len() as f64) } else { "null".into() };
    format!(
        "\"status\":\"{}\",\"score\":{score},\"bytes\":{bytes},\"levels\":{levels},\"phases\":[{}]",
        if ok { "ok" } else { "full" },
        parts.join(",")
    )
}

fn main() {
    let mut slots = 1usize << 21;
    let mut loads = vec![0.95, 0.98];
    let mut reps = 3;
    let mut reference = false;
    let args: Vec<String> = std::env::args().collect();
    let mut i = 1;
    while i < args.len() {
        match args[i].as_str() {
            "--slots" => {
                slots = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--loads" => {
                loads = args[i + 1].split(',').map(|x| x.parse().unwrap()).collect();
                i += 1;
            }
            "--reps" => {
                reps = args[i + 1].parse().unwrap();
                i += 1;
            }
            "--reference" => reference = true,
            a => panic!("unknown arg {a}"),
        }
        i += 1;
    }
    let out = std::io::stdout();
    let mut out = out.lock();
    if reference {
        // hashbrown cannot be held at 95%+ load (it grows at 87.5%), so it gets the
        // same number of keys and chooses its own capacity: a context number only.
        for (i, &a) in loads.iter().enumerate() {
            let n = (slots as f64 * a) as usize;
            let p = run_map(|| Brown::with_capacity(0), a, reps, 1000 + i as u64, Some(n)).unwrap();
            let mut m = Brown::with_capacity(0);
            for k in 0..n as u64 {
                m.insert(k, k).unwrap();
            }
            writeln!(
                out,
                "{{\"design\":\"hashbrown\",\"keys\":{n},\"bytes\":{},\"ins\":{:.2},\"hit\":{:.2},\"miss\":{:.2},\"op\":{:.3}}}",
                m.heap_bytes(),
                p.ins,
                p.hit,
                p.miss,
                (p.ins + p.hit + p.miss) / 3.0
            )
            .unwrap();
        }
        return;
    }
    for line in std::io::stdin().lock().lines() {
        let line = line.unwrap();
        let key = line.trim();
        if key.is_empty() {
            continue;
        }
        let t = Instant::now();
        let body = match parse(key) {
            Ok(d) if d.w == 16 => eval::<G16>(&d, slots, &loads, reps),
            Ok(d) => eval::<G64>(&d, slots, &loads, reps),
            Err(e) => format!("\"status\":\"error\",\"score\":null,\"error\":{e:?}"),
        };
        writeln!(out, "{{\"design\":\"{key}\",{body},\"eval_s\":{:.3}}}", t.elapsed().as_secs_f64()).unwrap();
        out.flush().unwrap();
    }
}
