//! Paired bootstrap intervals across seeds; never pool unlike objective scales.
//!
//!   cargo run --release --locked --example summarize -- results.jsonl
//!
//! benchmarks/evidence/compare/summary.md was first produced by a Python script. The
//! resampling here draws the same stream (MT19937 seeded with 42, indices taken
//! the way CPython's `random.choices` takes them), so the same input still gives
//! the same table.
use anyhow::{bail, ensure, Context, Result};
use serde_json::Value;
use std::collections::BTreeMap;

const RESAMPLES: usize = 5000;

struct Mt {
    s: [u32; 624],
    i: usize,
}

impl Mt {
    /// CPython's `random.Random(seed)` for a seed below 2^32 (`init_by_array` with one key).
    fn new(seed: u32) -> Self {
        let mut s = [0u32; 624];
        s[0] = 19_650_218;
        for i in 1..624 {
            s[i] = 1_812_433_253u32.wrapping_mul(s[i - 1] ^ (s[i - 1] >> 30)).wrapping_add(i as u32);
        }
        let mut i = 1;
        for _ in 0..624 {
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_664_525)).wrapping_add(seed);
            i += 1;
            if i == 624 {
                s[0] = s[623];
                i = 1;
            }
        }
        for _ in 0..623 {
            s[i] = (s[i] ^ (s[i - 1] ^ (s[i - 1] >> 30)).wrapping_mul(1_566_083_941)).wrapping_sub(i as u32);
            i += 1;
            if i == 624 {
                s[0] = s[623];
                i = 1;
            }
        }
        s[0] = 0x8000_0000;
        Mt { s, i: 624 }
    }

    fn next_u32(&mut self) -> u32 {
        if self.i == 624 {
            for k in 0..624 {
                let y = (self.s[k] & 0x8000_0000) | (self.s[(k + 1) % 624] & 0x7fff_ffff);
                self.s[k] = self.s[(k + 397) % 624] ^ (y >> 1) ^ if y & 1 == 1 { 0x9908_b0df } else { 0 };
            }
            self.i = 0;
        }
        let mut y = self.s[self.i];
        self.i += 1;
        y ^= y >> 11;
        y ^= (y << 7) & 0x9d2c_5680;
        y ^= (y << 15) & 0xefc6_0000;
        y ^ (y >> 18)
    }

    /// Uniform in [0, 1), 53 bits.
    fn f64(&mut self) -> f64 {
        let (a, b) = (self.next_u32() >> 5, self.next_u32() >> 6);
        (a as f64 * 67_108_864.0 + b as f64) / 9_007_199_254_740_992.0
    }
}

fn mean(v: &[f64]) -> f64 {
    v.iter().sum::<f64>() / v.len() as f64
}

fn main() -> Result<()> {
    let path = std::env::args().nth(1).context("usage: summarize RESULTS.jsonl")?;
    let text = std::fs::read_to_string(&path).with_context(|| format!("reading {path}"))?;
    // (task, method) -> seed -> (regret, evaluations)
    let mut groups: BTreeMap<(String, String), BTreeMap<i64, (f64, f64)>> = BTreeMap::new();
    for (n, line) in text.lines().enumerate().filter(|(_, l)| !l.trim().is_empty()) {
        let at = || format!("{path}:{}", n + 1);
        let row: Value = serde_json::from_str(line).with_context(at)?;
        if row["type"] != "result" {
            continue;
        }
        let field = |k: &str| row[k].as_f64().with_context(|| format!("{}: no numeric {k:?}", at()));
        let key = (row["task"].as_str().with_context(at)?.to_string(), row["method"].as_str().with_context(at)?.to_string());
        let seed = row["seed"].as_i64().with_context(at)?;
        if groups.entry(key.clone()).or_default().insert(seed, (field("regret")?, field("evaluations")?)).is_some() {
            bail!("duplicate seed for {key:?}");
        }
    }

    println!("| Task | Method | Mean regret ↓ | Paired regret difference vs refine (95% bootstrap CI) | Mean evaluations |");
    println!("|---|---|---:|---:|---:|");
    for ((task, method), results) in &groups {
        let baseline = groups.get(&(task.clone(), "refine_best".to_string())).with_context(|| format!("{task}: no refine_best arm"))?;
        ensure!(results.keys().eq(baseline.keys()), "unpaired seeds for {task}/{method}");
        let diff: Vec<f64> = results.iter().map(|(s, r)| r.0 - baseline[s].0).collect();
        let mut rng = Mt::new(42);
        let mut means: Vec<f64> = (0..RESAMPLES)
            .map(|_| mean(&(0..diff.len()).map(|_| diff[(rng.f64() * diff.len() as f64) as usize]).collect::<Vec<_>>()))
            .collect();
        means.sort_by(f64::total_cmp);
        let (lo, hi) = (means[125], means[4874]);
        let regret = mean(&results.values().map(|r| r.0).collect::<Vec<_>>());
        let cost = mean(&results.values().map(|r| r.1).collect::<Vec<_>>());
        println!("| {task} | {method} | {regret:.3} | {:+.3} [{lo:+.3}, {hi:+.3}] | {cost:.1} |", mean(&diff));
    }
    Ok(())
}
