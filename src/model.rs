//! The yield model: what dreaming learns from recorded history without
//! guessing at outcomes that were never observed.
//!
//! Replay scores a candidate policy by asking "what would have happened if it
//! had run instead". History can only answer that where the candidate repeats
//! recorded decisions, so replay mostly measures how much a candidate resembles
//! the policy that recorded the history (see docs/validation.md).
//!
//! The yield model asks a question history can always answer. Every recorded
//! attempt is a *move*: some node was continued for the k-th time. A move has a
//! type that depends only on the tree at that moment:
//!
//! * who was continued: the `best` attempt so far, a runner-up (`next`: the
//!   2nd or 3rd best design), any other scored attempt (`rest`), or a `fail`ed
//!   one (repair);
//! * how often it had been continued before: class 0 (never), 1 (1-2 times),
//!   2 (3-6 times), 3 (7-14 times), 4 (15 or more).
//!
//! and a measured outcome: its *credit* is 1 if the attempt became the new best,
//! `gamma^d` if a descendant d generations below did, else 0. The model is the
//! credit-weighted success rate of each move type, discounted by age. Every
//! number in it is an average over attempts that were really evaluated.
//!
//! The `bandit` policy family acts on the model: it keeps one Beta posterior
//! per move type, starts from the fitted rates, updates them with the round in
//! progress, and continues the node whose move type draws the highest sampled
//! yield. Its decision is a pure function of (tree, params, seed), so a
//! recorded round replays exactly and can be extended later.

use crate::policies::{Param, Policy};
use crate::rng::{hash, Rng};
use crate::tree::{repairable, Tree, PENDING};
use std::collections::HashMap;

pub const WHO: [&str; 4] = ["best", "next", "rest", "fail"];
/// Continuation classes: a node continued 0, 1-2, 3-6, 7-14 or 15+ times before.
pub const KC: usize = 5;
/// Number of move types (4 kinds of parent x KC continuation classes).
pub const NB: usize = 4 * KC;

/// Yield assumed for each move type before any evidence: continue the best
/// attempt first, expect less from attempts that are not the best and least
/// from failed ones, and less again each time a node has already been continued.
pub fn prior(b: usize) -> f64 {
    [0.5, 0.25, 0.12, 0.1][b / KC] * 0.7f64.powi((b % KC) as i32)
}

pub const BANDIT: &str = "bandit";
/// Credit passed to the move one generation above a new best.
pub const GAMMA: f64 = 0.5;
/// Attempts after which recorded evidence counts half.
pub const HALFLIFE: f64 = 48.0;
/// Weight of the prior, in attempts per move type.
pub const STRENGTH: f64 = 4.0;
/// How strongly the move type with the highest expected yield is preferred:
/// yields are sampled from Beta(focus * a, focus * b). 1 is Thompson sampling,
/// which explores too much within the few dozen attempts of a round; "inf" is
/// greedy, which never revisits a move type that started badly.
pub const FOCUS: f64 = 16.0;

/// How many designs behind the best count as runners-up.
pub const RUNNERS: usize = 2;

pub fn kclass(k: usize) -> usize {
    ((k + 1).ilog2() as usize).min(KC - 1)
}

/// `best0`, `best1`, ..., `fail2`.
pub fn name(b: usize) -> String {
    format!("{}{}", WHO[b / KC], b % KC)
}

pub fn describe(b: usize) -> String {
    let who = ["the best attempt", "a runner-up (2nd or 3rd best)", "a lower-ranked attempt", "a failed attempt"][b / KC];
    let k = ["not continued before", "continued 1-2 times", "continued 3-6 times", "continued 7-14 times", "continued 15+ times"][b % KC];
    format!("{who}, {k}")
}

/// Weighted credit (`s`) and weighted count (`n`) per move type.
#[derive(Clone, Debug, Default, PartialEq)]
pub struct Yield {
    pub s: [f64; NB],
    pub n: [f64; NB],
}

impl Yield {
    pub fn add(&mut self, other: &Yield, w: f64) {
        for b in 0..NB {
            self.s[b] += w * other.s[b];
            self.n[b] += w * other.n[b];
        }
    }

    /// Posterior Beta(alpha, beta) of move type `b` with `m` prior pseudo-attempts.
    pub fn posterior(&self, b: usize, m: f64) -> (f64, f64) {
        let s = self.s[b].clamp(0.0, self.n[b].max(0.0));
        (m * prior(b) + s, m * (1.0 - prior(b)) + (self.n[b] - s).max(0.0))
    }

    pub fn mean(&self, b: usize, m: f64) -> f64 {
        let (a, c) = self.posterior(b, m);
        a / (a + c)
    }
}

/// Scored designs seen so far, ranked by their first measurement.
#[derive(Default)]
struct Ranking<'a> {
    cost: HashMap<&'a str, f64>,
    sorted: Vec<f64>,
}

impl<'a> Ranking<'a> {
    fn see(&mut self, design: &'a str, cost: f64) {
        if !self.cost.contains_key(design) {
            self.cost.insert(design, cost);
            let at = self.sorted.partition_point(|x| *x < cost);
            self.sorted.insert(at, cost);
        }
    }

    /// 1 (runner-up) or 2 (the rest) for a scored design that is not the best.
    fn who(&self, design: &str) -> usize {
        let c = self.cost.get(design).copied().unwrap_or(f64::INFINITY);
        if self.sorted.partition_point(|x| *x < c) <= RUNNERS { 1 } else { 2 }
    }
}

/// The move type and credit of every recorded attempt of `t` (index = node id - 1).
/// A new best must improve on the best so far by a relative margin > `eps`.
///
/// A move is typed by what was known when it was chosen. With attempts running
/// at once, results arrive after later attempts were chosen ([`Node::visible`]),
/// so results are taken in the order they became known, not in id order.
pub fn moves(t: &Tree, gamma: f64, eps: f64) -> Vec<(usize, f64)> {
    let n = t.nodes.len();
    let mut out: Vec<(usize, f64)> = Vec::with_capacity(n.saturating_sub(1));
    let mut best: Option<usize> = None;
    let mut rank = Ranking::default();
    if let Some(s) = t.nodes.first().and_then(|r| r.score) {
        best = Some(0);
        rank.see(&t.nodes[0].design, t.cost(s));
    }
    let mut known: Vec<usize> = (1..n).collect();
    known.sort_by_key(|&j| (t.nodes[j].visible(), j));
    let mut next = 0;
    for i in 1..=n {
        // results known before attempt i was chosen (after the last attempt: all the rest)
        while next < known.len() && (i == n || t.nodes[known[next]].visible() <= i) {
            let j = known[next];
            next += 1;
            let c = &t.nodes[j];
            let Some(s) = c.score else { continue };
            rank.see(&c.design, t.cost(s));
            let (better, improved) = match best.and_then(|b| t.nodes[b].score) {
                Some(bs) => (t.cost(s) < t.cost(bs), t.rel(s, bs) > eps),
                None => (true, true),
            };
            if improved {
                // credit the attempt and, discounted, the moves that led to it
                let (mut a, mut w) = (j, 1.0);
                while a > 0 && w > 1e-6 {
                    let e = &mut out[a - 1];
                    e.1 = e.1.max(w);
                    a = t.nodes[a].parent.filter(|&q| q < a).unwrap_or(0);
                    w *= gamma;
                }
            }
            if better {
                best = Some(j);
            }
        }
        if i == n {
            break;
        }
        let c = &t.nodes[i];
        let Some(p) = c.parent.filter(|&p| p < i) else {
            out.push((2 * KC, 0.0));
            continue;
        };
        let pn = &t.nodes[p];
        let who = if pn.score.is_none() {
            3
        } else if best.is_some_and(|b| t.nodes[b].design == pn.design) {
            0
        } else {
            rank.who(&pn.design)
        };
        out.push((who * KC + kclass(c.k.unwrap_or(0)), 0.0));
    }
    out
}

/// The moves of one tree, counted. An attempt that is still running counts as
/// a move that has not paid: while several attempts are in flight, the policy
/// is steered away from choosing the same kind of move for all of them.
pub fn tally(t: &Tree, gamma: f64, eps: f64) -> Yield {
    let mut y = Yield::default();
    for (b, credit) in moves(t, gamma, eps) {
        y.s[b] += credit;
        y.n[b] += 1.0;
    }
    y
}

/// Fit the model over recorded rounds. An attempt followed by `a` later
/// attempts of its run weighs `0.5^(a / halflife)`: search gets harder as it
/// progresses, so old rounds say less about the next one.
pub fn fit(history: &[Tree], gamma: f64, eps: f64, halflife: f64) -> Yield {
    let mut runs: std::collections::BTreeMap<&str, Vec<&Tree>> = std::collections::BTreeMap::new();
    for t in history {
        runs.entry(&t.run).or_default().push(t);
    }
    let mut y = Yield::default();
    for rounds in runs.values_mut() {
        rounds.sort_by_key(|t| std::cmp::Reverse(t.round));
        let mut later = 0usize;
        for t in rounds.iter() {
            let mv = moves(t, gamma, eps);
            let n = mv.len();
            for (i, (b, credit)) in mv.into_iter().enumerate() {
                // an attempt that never finished (a killed run) says nothing
                if t.nodes[i + 1].status == PENDING {
                    continue;
                }
                let age = (later + n - 1 - i) as f64;
                let w = if halflife.is_finite() && halflife > 0.0 { 0.5f64.powf(age / halflife) } else { 1.0 };
                y.s[b] += w * credit;
                y.n[b] += w;
            }
            later += n;
        }
    }
    y
}

/// The node the policy would continue for each move type available in `t`:
/// identical designs act as one (the copy continued furthest is continued
/// again), and each move type offers its most promising attempt.
pub fn options(t: &Tree) -> Vec<(usize, usize)> {
    struct Group {
        node: usize,
        n_exp: usize,
        cost: Option<f64>,
        repairable: bool,
    }
    let mut order: Vec<&str> = vec![];
    let mut groups: HashMap<&str, Group> = HashMap::new();
    let mut rank = Ranking::default();
    for n in &t.nodes {
        let g = groups.entry(n.design.as_str()).or_insert_with(|| {
            order.push(n.design.as_str());
            Group { node: n.id, n_exp: n.n_exp, cost: None, repairable: false }
        });
        if n.n_exp > g.n_exp {
            g.node = n.id;
            g.n_exp = n.n_exp;
        }
        if let Some(s) = n.score {
            g.cost.get_or_insert(t.cost(s));
            rank.see(&n.design, t.cost(s));
        } else if repairable(&n.status) {
            g.repairable = true;
        }
    }
    let best_design = t.best().map(|b| b.design.as_str());
    // per move type: (cost of the attempt, node id)
    let mut pick: [Option<(f64, usize)>; NB] = [None; NB];
    for d in order {
        let g = &groups[d];
        let (who, cost) = if let Some(c) = g.cost {
            (if Some(d) == best_design { 0 } else { rank.who(d) }, c)
        } else if g.repairable {
            // a failed attempt is worth what the attempt it broke was worth
            let mut a = &t.nodes[g.node];
            while a.score.is_none() {
                match a.parent {
                    Some(p) => a = &t.nodes[p],
                    None => break,
                }
            }
            match a.score {
                Some(s) => (3, t.cost(s)),
                None => continue,
            }
        } else {
            continue;
        };
        let b = who * KC + kclass(g.n_exp);
        if pick[b].is_none_or(|(c, _)| cost <= c) {
            pick[b] = Some((cost, g.node)); // ties -> newest design
        }
    }
    (0..NB).filter_map(|b| pick[b].map(|(_, id)| (b, id))).collect()
}

fn normal(rng: &mut Rng) -> f64 {
    let u = rng.f64().max(1e-300);
    (-2.0 * u.ln()).sqrt() * (std::f64::consts::TAU * rng.f64()).cos()
}

/// Gamma(shape, 1) (Marsaglia & Tsang).
fn gamma_sample(rng: &mut Rng, shape: f64) -> f64 {
    if shape < 1.0 {
        let u = rng.f64().max(1e-300);
        return gamma_sample(rng, shape + 1.0) * u.powf(1.0 / shape);
    }
    let d = shape - 1.0 / 3.0;
    let c = 1.0 / (9.0 * d).sqrt();
    loop {
        let x = normal(rng);
        let v = (1.0 + c * x).powi(3);
        if v <= 0.0 {
            continue;
        }
        let u = rng.f64().max(1e-300);
        if u.ln() < 0.5 * x * x + d - d * v + d * v.ln() {
            return d * v;
        }
    }
}

/// The discounted history a `bandit` policy carries in its spec (`s_*`, `n_*`).
pub fn learned(p: &Policy) -> Yield {
    let mut y = Yield::default();
    for b in 0..NB {
        y.n[b] = p.p(&format!("n_{}", name(b)), 0.0).max(0.0);
        y.s[b] = p.p(&format!("s_{}", name(b)), 0.0).clamp(0.0, y.n[b]);
    }
    y
}

/// Dreaming for a `bandit` policy: refit its yield model over `history`.
/// Returns None for other families (they are tuned by replay instead).
pub fn learn(p: &Policy, history: &[Tree]) -> Option<(Policy, Yield)> {
    if p.family != BANDIT {
        return None;
    }
    let y = fit(history, p.p("gamma", GAMMA), p.p("eps", 0.0), p.p("halflife", HALFLIFE));
    let mut q = p.clone();
    let round = |x: f64| (x * 1e4).round() / 1e4;
    for b in 0..NB {
        for (key, v) in [("s", y.s[b]), ("n", y.n[b])] {
            let k = format!("{key}_{}", name(b));
            if round(v) > 0.0 {
                q.set(&k, Param::Num(round(v)));
            } else {
                q.unset(&k);
            }
        }
    }
    Some((q, y))
}

/// Which node a `bandit` policy continues next (None: end the round).
pub fn select(p: &Policy, seed: u64, t: &Tree) -> Option<usize> {
    let m = p.p("m", STRENGTH);
    let mut y = tally(t, p.p("gamma", GAMMA), p.p("eps", 0.0));
    y.add(&learned(p), 1.0);
    let opts = options(t);
    let stop = p.p("stop", 0.0);
    if opts.is_empty() || (stop > 0.0 && opts.iter().all(|(b, _)| y.mean(*b, m) < stop)) {
        return None;
    }
    // a pure function of (tree, params, seed): replay reproduces live rounds
    let mut rng = Rng::new(hash(&format!("bandit|{seed}|{}", t.calls())));
    let focus = p.p("focus", FOCUS).max(1e-3);
    let mut top: Option<(f64, usize)> = None;
    for (b, id) in opts {
        let (a, c) = y.posterior(b, m);
        let x = if focus.is_finite() { beta_sample(&mut rng, focus * a, focus * c) } else { a / (a + c) };
        if top.is_none_or(|(v, _)| x > v) {
            top = Some((x, id));
        }
    }
    top.map(|(_, id)| id)
}

pub fn beta_sample(rng: &mut Rng, a: f64, b: f64) -> f64 {
    let (a, b) = (a.max(1e-3), b.max(1e-3));
    let x = gamma_sample(rng, a);
    let y = gamma_sample(rng, b);
    if x + y > 0.0 {
        x / (x + y)
    } else {
        a / (a + b)
    }
}

/// Central interval of Beta(a, b) by sampling (reporting only).
pub fn interval(a: f64, b: f64, level: f64) -> (f64, f64) {
    let mut rng = Rng::new(0x5eed);
    let mut v: Vec<f64> = (0..2000).map(|_| beta_sample(&mut rng, a, b)).collect();
    v.sort_by(f64::total_cmp);
    let tail = (1.0 - level) / 2.0;
    (v[(tail * 1999.0) as usize], v[((1.0 - tail) * 1999.0) as usize])
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tree::{Goal, Node, FAIL, OK};

    fn tree(spec: &[(usize, Option<f64>)]) -> Tree {
        let mut t = Tree::new("r", 1, 99, Goal::Min, "");
        t.set_root(Node::new(0, None, None, "d0", Some(10.0), OK, ""));
        for (i, (p, s)) in spec.iter().enumerate() {
            let k = t.nodes[*p].n_exp;
            t.nodes[*p].n_exp += 1;
            t.add(Node::new(i + 1, Some(*p), Some(k), format!("d{}", i + 1), *s, if s.is_some() { OK } else { FAIL }, ""));
        }
        t
    }

    #[test]
    fn moves_are_typed_and_credited() {
        // n1: best0, new best. n2: best0 (continues n1), worse. n3: other0 (continues n2), new best.
        // n4: best0 fails. n5: fail0 repairs it into a new best. n6..n8: the root (continued 1, 2, 3 times before).
        let t = tree(&[(0, Some(9.0)), (1, Some(9.5)), (2, Some(8.0)), (3, None), (4, Some(7.0)), (0, Some(20.0)), (0, Some(20.0)), (0, Some(20.0))]);
        let m = moves(&t, 0.5, 0.0);
        let types: Vec<String> = m.iter().map(|(b, _)| name(*b)).collect();
        // the root is a runner-up while at most two designs beat it (n6: 9, 8, 7 do, so it is `rest`)
        assert_eq!(types, ["best0", "best0", "next0", "best0", "fail0", "rest1", "rest1", "rest2"]);
        let credit: Vec<f64> = m.iter().map(|(_, c)| *c).collect();
        // n2 led to n3 (0.5); n4 led to n5 (0.5); n3 is an ancestor of n5 but already has 1.
        assert_eq!(credit, [1.0, 0.5, 1.0, 0.5, 1.0, 0.0, 0.0, 0.0]);
        let y = tally(&t, 0.5, 0.0);
        assert_eq!((y.n[0], y.s[0]), (3.0, 2.0));
        // a margin: n1 (10 -> 9) no longer counts as an improvement above 20%
        assert_eq!(moves(&t, 0.5, 0.2)[0].1, 0.0);
    }

    #[test]
    fn moves_use_what_was_known_when_they_were_chosen() {
        // n1 and n2 both continue the root; n1 becomes the new best.
        let serial = tree(&[(0, Some(9.0)), (0, Some(9.5))]);
        assert_eq!(moves(&serial, 0.5, 0.0), [(0, 1.0), (KC + 1, 0.0)], "one at a time: n2 continues a runner-up");
        // Started together: when n2 was chosen the root was still the best, and
        // n2's result arrived first, so it too was a new best when it arrived.
        let mut both = serial.clone();
        both.nodes[1].seen = 3;
        both.nodes[2].seen = 3;
        assert_eq!(moves(&both, 0.5, 0.0), [(0, 1.0), (1, 0.0)], "n1's result (id order within a wave) is counted first");
        both.nodes[1].seen = 4;
        assert_eq!(moves(&both, 0.5, 0.0), [(0, 1.0), (1, 1.0)]);
        // a running attempt counts as an unpaid move in the round, and as nothing in history
        let mut t = serial.clone();
        t.nodes[0].n_exp += 1;
        t.add(Node::pending(3, 0, 2));
        assert_eq!(tally(&t, 0.5, 0.0).n[KC + 1], 2.0);
        assert_eq!(fit(&[t], 0.5, 0.0, f64::INFINITY).n[KC + 1], 1.0);
    }

    #[test]
    fn fit_discounts_old_attempts() {
        let mut a = tree(&[(0, Some(9.0)), (1, Some(8.0))]);
        let mut b = tree(&[(0, None), (0, None)]);
        a.round = 1;
        b.round = 2;
        let y = fit(&[b.clone(), a.clone()], 0.5, 0.0, 2.0);
        // round 2 (newest): best0 at age 1, best1 at age 0; round 1: best0 at ages 3 and 2, both credited
        assert!((y.n[0] - (0.5f64.powf(0.5) + 0.5f64.powf(1.5) + 0.5)).abs() < 1e-12);
        assert!((y.s[0] - (0.5f64.powf(1.5) + 0.5)).abs() < 1e-12);
        assert_eq!((y.n[1], y.s[1]), (1.0, 0.0));
        assert_eq!(fit(&[a, b], 0.5, 0.0, f64::INFINITY).n[0], 3.0);
    }

    #[test]
    fn options_dedup_designs() {
        let mut t = tree(&[(0, Some(9.0)), (0, Some(12.0)), (1, None)]);
        // a copy of the best design that has been continued further acts for it
        let k = t.nodes[2].n_exp;
        t.nodes[2].n_exp += 3;
        t.add(Node::new(4, Some(2), Some(k), "d1", Some(9.2), OK, ""));
        t.nodes[4].n_exp = 4;
        // best -> n4 (the copy continued furthest: 4 times, class 2); the runners-up root
        // (2 times, class 1) and n2 (3 times, class 2); fail -> n3
        assert_eq!(options(&t), vec![(2, 4), (KC + 1, 0), (KC + 2, 2), (3 * KC, 3)]);
    }

    #[test]
    fn bandit_is_pure_and_learns() {
        let reg = crate::policies::Registry::builtin();
        let mut p = reg.make(&serde_json::json!({"family": "bandit"})).unwrap();
        // the best attempt has failed 20 times: its move type is spent, others take over
        let mut spec: Vec<(usize, Option<f64>)> = vec![(0, Some(9.0))];
        spec.extend((0..20).map(|_| (1, Some(9.5))));
        let t = tree(&spec);
        p.reset(7);
        let a = p.select(&t).unwrap().unwrap();
        assert_eq!(p.select(&t).unwrap(), Some(a), "same tree, same decision");
        let picks: std::collections::HashSet<usize> = (0..50).map(|s| { p.reset(s); p.select(&t).unwrap().unwrap() }).collect();
        assert!(!picks.contains(&1), "the exhausted best is not continued: {picks:?}");
        // history that says continuing the exhausted best pays brings it back
        let mut y = Yield::default();
        (y.s[4], y.n[4]) = (40.0, 40.0);
        let mut q = p.clone();
        q.set("s_best4", Param::Num(40.0));
        q.set("n_best4", Param::Num(40.0));
        assert_eq!(learned(&q), y);
        q.reset(7);
        assert_eq!(q.select(&t).unwrap(), Some(1));
        // stop: no move type is expected to pay
        p.set("stop", Param::Num(0.9));
        assert_eq!(p.select(&t).unwrap(), None);
        // learn() writes the fitted model into the spec, and it round-trips
        let (r, fitted) = learn(&reg.make(&serde_json::json!({"family": "bandit"})).unwrap(), &[t]).unwrap();
        assert!(fitted.n[0] > 0.0 && r.repr().contains("n_best0="));
        assert_eq!(reg.make(&r.spec()).unwrap().repr(), r.repr());
        assert!(learn(&reg.make(&serde_json::json!({"family": "refine_best"})).unwrap(), &[]).is_none());
    }

    #[test]
    fn beta_samples_match_the_mean() {
        let mut rng = Rng::new(3);
        for (a, b) in [(0.4, 1.6), (1.0, 1.0), (7.5, 2.5), (0.05, 30.0)] {
            let m = (0..20000).map(|_| beta_sample(&mut rng, a, b)).sum::<f64>() / 20000.0;
            assert!((m - a / (a + b)).abs() < 0.01, "Beta({a},{b}) mean {m}");
        }
    }
}
