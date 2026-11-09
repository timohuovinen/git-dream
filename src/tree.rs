//! Discovery tree: what one round of exploration produces and what policies see.

use serde::{Deserialize, Serialize};
use std::collections::HashMap;

/// Only `OK` attempts carry a score.
pub const OK: &str = "ok";
/// The evaluator failed / rejected the attempt (build broke, tests failed, timeout, ...).
pub const FAIL: &str = "fail";
/// The agent produced nothing new.
pub const NOOP: &str = "noop";
/// Replay only: the outcome is not in recorded history.
pub const MISS: &str = "miss";

/// Failed attempts a policy may still continue ("full" = hashbench's name for an overflowing table).
pub fn repairable(status: &str) -> bool {
    status == FAIL || status == "full"
}

#[derive(Clone, Copy, Debug, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "lowercase")]
pub enum Goal {
    #[default]
    Min,
    Max,
}

impl Goal {
    pub fn cost(self, s: f64) -> f64 {
        match self {
            Goal::Min => s,
            Goal::Max => -s,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Goal::Min => "min",
            Goal::Max => "max",
        }
    }
    pub fn parse(s: &str) -> Goal {
        if s == "max" {
            Goal::Max
        } else {
            Goal::Min
        }
    }
}

#[derive(Clone, Debug, PartialEq)]
pub struct Node {
    pub id: usize,
    /// Parent node id (None for the root).
    pub parent: Option<usize>,
    /// This node is the k-th continuation of its parent.
    pub k: Option<usize>,
    /// Identity of the attempt's content: a parameter key or a git tree hash.
    pub design: String,
    pub score: Option<f64>,
    pub status: String,
    /// The agent's write-up of its change: a one-line summary, optionally
    /// followed by the idea, its evidence and why it is not a repeat.
    pub change: String,
    /// How many times this node has been continued.
    pub n_exp: usize,
    /// Git commit of this attempt.
    pub sha: String,
    /// Replay: id of the matching recorded node.
    pub rec: Option<usize>,
}

impl Node {
    pub fn new(id: usize, parent: Option<usize>, k: Option<usize>, design: impl Into<String>, score: Option<f64>, status: &str, change: impl Into<String>) -> Node {
        Node { id, parent, k, design: design.into(), score, status: status.into(), change: change.into(), n_exp: 0, sha: String::new(), rec: None }
    }
}

#[derive(Clone, Debug)]
pub struct Tree {
    pub run: String,
    pub round: u32,
    pub budget: usize,
    pub goal: Goal,
    pub nodes: Vec<Node>,
    pub policy: String,
    /// Best score after each call (index 0 = root).
    pub best_trace: Vec<Option<f64>>,
}

impl Tree {
    pub fn new(run: &str, round: u32, budget: usize, goal: Goal, policy: &str) -> Tree {
        Tree { run: run.into(), round, budget, goal, nodes: vec![], policy: policy.into(), best_trace: vec![] }
    }

    pub fn cost(&self, s: f64) -> f64 {
        self.goal.cost(s)
    }

    /// Improvement of score `s` over `reference`, relative to |reference| (> 0: s is better).
    /// For goal=min and positive scores this is (ref - s) / ref.
    pub fn rel(&self, s: f64, reference: f64) -> f64 {
        let d = if reference == 0.0 { 1.0 } else { reference.abs() };
        (self.cost(reference) - self.cost(s)) / d
    }

    pub fn calls(&self) -> usize {
        self.nodes.len().saturating_sub(1)
    }

    pub fn root(&self) -> &Node {
        &self.nodes[0]
    }

    pub fn best(&self) -> Option<&Node> {
        let mut b: Option<&Node> = None;
        for n in &self.nodes {
            if let Some(s) = n.score {
                if b.is_none_or(|x| self.cost(s) < self.cost(x.score.unwrap())) {
                    b = Some(n);
                }
            }
        }
        b
    }

    pub fn set_root(&mut self, n: Node) {
        self.best_trace = vec![n.score];
        self.nodes = vec![n];
    }

    pub fn add(&mut self, n: Node) {
        self.nodes.push(n);
        let b = self.best().and_then(|b| b.score);
        self.best_trace.push(b);
    }

    /// Calls since the best score last improved by a relative margin > eps.
    pub fn stalled(&self, eps: f64) -> usize {
        let mut r = self.best_trace[0];
        let mut since = 0;
        for s in &self.best_trace[1..] {
            match (s, r) {
                (Some(s), None) => {
                    r = Some(*s);
                    since = 0
                }
                (Some(s), Some(x)) if self.rel(*s, x) > eps => {
                    r = Some(*s);
                    since = 0
                }
                _ => since += 1,
            }
        }
        since
    }

    /// (parent id, k) -> node index.
    pub fn edges(&self) -> HashMap<(usize, usize), usize> {
        self.nodes.iter().enumerate().filter_map(|(i, n)| Some(((n.parent?, n.k?), i))).collect()
    }

    pub fn children(&self, id: usize) -> Vec<&Node> {
        self.nodes.iter().filter(|n| n.parent == Some(id)).collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn goal_max() {
        let mut t = Tree::new("r", 1, 10, Goal::Max, "");
        t.set_root(Node::new(0, None, None, "a", Some(10.0), OK, ""));
        t.add(Node::new(1, Some(0), Some(0), "b", Some(12.0), OK, ""));
        t.add(Node::new(2, Some(0), Some(1), "c", Some(8.0), OK, ""));
        assert_eq!(t.best().unwrap().id, 1);
        assert!((t.rel(12.0, 10.0) - 0.2).abs() < 1e-12);
        assert_eq!(t.stalled(0.0), 1);
    }
}
