//! End-to-end tests on the examples, in throwaway git repositories.

use git_dream::engine::Engine;
use git_dream::sim::{self, Mode, ReplayWorld};
use git_dream::tree::Tree;
use std::path::{Path, PathBuf};
use std::process::Command;

fn git(dir: &Path, args: &[&str]) -> String {
    let o = Command::new("git").args(args).current_dir(dir).output().unwrap();
    assert!(o.status.success(), "git {args:?}: {}", String::from_utf8_lossy(&o.stderr));
    String::from_utf8_lossy(&o.stdout).trim().to_string()
}

struct Repo(PathBuf);

impl Drop for Repo {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn copy_dir(from: &Path, to: &Path) {
    std::fs::create_dir_all(to).unwrap();
    for e in std::fs::read_dir(from).unwrap() {
        let e = e.unwrap();
        let p = to.join(e.file_name());
        if e.file_type().unwrap().is_dir() {
            copy_dir(&e.path(), &p);
        } else {
            std::fs::copy(e.path(), &p).unwrap();
        }
    }
}

fn repo(example: &str, name: &str, edit: impl Fn(String) -> String) -> Repo {
    let d = std::env::temp_dir().join(format!("git-dream-test-{}-{name}", std::process::id()));
    let _ = std::fs::remove_dir_all(&d);
    copy_dir(&Path::new(env!("CARGO_MANIFEST_DIR")).join("examples").join(example), &d);
    let cfg = d.join(".git-dream.toml");
    std::fs::write(&cfg, edit(std::fs::read_to_string(&cfg).unwrap())).unwrap();
    git(&d, &["init", "-q"]);
    git(&d, &["config", "core.autocrlf", "false"]);
    git(&d, &["add", "-A"]);
    git(&d, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "init"]);
    Repo(d)
}

fn open(r: &Repo, run: Option<&str>) -> Engine {
    let mut e = Engine::open(&r.0, None, run).unwrap();
    e.quiet = true;
    e
}

type Sig = Vec<(Option<usize>, Option<usize>, String, Option<f64>, String)>;

fn sig(t: &Tree) -> Sig {
    t.nodes.iter().map(|n| (n.parent, n.k, n.design.clone(), n.score, n.status.clone())).collect()
}

/// The policy deployed in a round, replayed over that round's recorded tree,
/// must reproduce it exactly (same decisions, same attempts).
fn check_exact_replay(e: &Engine) {
    for t in e.mem.history(&e.run).unwrap() {
        let mut p = e.deployed(Some(t.round)).unwrap().unwrap();
        let mut w = ReplayWorld::new(std::slice::from_ref(&t), e.agent.space(), Mode::Strict, false);
        w.bind(&t);
        let r = sim::run_tree(&mut p, &mut w, t.root(), t.budget, &e.run, t.round, t.goal, t.round as u64, &mut |_, _| {}).unwrap();
        assert_eq!(sig(&r), sig(&t), "round {}", t.round);
        assert_eq!(w.misses, 0);
    }
}

#[test]
fn space_agent_end_to_end() {
    let r = repo("toy-space", "space", |s| s.replace("budget = 25", "budget = 12").replace("candidates = 200", "candidates = 40"));
    let head = git(&r.0, &["rev-parse", "HEAD"]);
    let e = open(&r, None);
    let trees = e.rounds(3, true, None, None, None).unwrap();
    assert_eq!(e.mem.rounds("main").unwrap(), vec![1, 2, 3]);
    assert!(trees.iter().all(|t| t.calls() <= 12));
    // memory round-trips through git
    for t in &trees {
        let lt = e.mem.load_tree("main", t.round).unwrap().unwrap();
        let k = |t: &Tree| t.nodes.iter().map(|n| (n.design.clone(), n.score, n.status.clone(), n.sha.clone())).collect::<Vec<_>>();
        assert_eq!(k(&lt), k(t));
    }
    check_exact_replay(&e);
    // the best attempt is a real commit holding its parameters
    let best = git(&r.0, &["rev-parse", "dream/main/best"]);
    let params: serde_json::Value = serde_json::from_str(&git(&r.0, &["show", "dream/main/best:.dream/params.json"])).unwrap();
    let node = trees.iter().flat_map(|t| &t.nodes).find(|n| n.sha == best).unwrap();
    assert_eq!(e.agent.space().unwrap().params_json(&node.design), params);
    assert!(git(&r.0, &["log", "-1", "--format=%B", &best]).contains("Dream-Score:"));
    assert!(git(&r.0, &["notes", "--ref=dream", "show", &best]).contains("\"score\""));
    // rounds chain into one DAG rooted at the base commit
    git(&r.0, &["merge-base", "--is-ancestor", &head, &best]);
    // one policy version per round
    assert_eq!(git(&r.0, &["log", "--format=%H", "dream/main/policy"]).lines().count(), 3);
    // the user's checkout is untouched
    assert_eq!(git(&r.0, &["rev-parse", "HEAD"]), head);
    assert_eq!(git(&r.0, &["status", "--porcelain"]), "");
    // resuming continues the numbering
    open(&r, None).rounds(1, false, None, None, None).unwrap();
    assert_eq!(e.mem.rounds("main").unwrap(), vec![1, 2, 3, 4]);
}

#[test]
fn command_agent_end_to_end() {
    let r = repo("toy-command", "command", |s| s.replace("budget = 12", "budget = 5"));
    let e = open(&r, None);
    let trees = e.rounds(2, true, None, None, None).unwrap();
    check_exact_replay(&e);
    for n in &trees[0].nodes[1..] {
        // an attempt's design is its git tree; its parent commit is the attempt it continued
        assert_eq!(n.design, git(&r.0, &["rev-parse", &format!("{}^{{tree}}", n.sha)]));
        assert_eq!(git(&r.0, &["rev-parse", &format!("{}^", n.sha)]), trees[0].nodes[n.parent.unwrap()].sha);
    }
    // a stochastic agent: off-path replay always misses, even in cache mode
    let p = e.registry.make(&serde_json::json!({"family": "ucb", "params": {"c": 0.5, "kmax": 1, "patience": "inf", "eps": 0}})).unwrap();
    let rr = e.replay(&p, Some(Mode::Cache), Some(5)).unwrap();
    assert!(rr.miss_rate > 0.0);
}

#[test]
fn noop_attempts() {
    let r = repo("toy-command", "noop", |s| s.replace("command = \"./agent.sh\"", "command = \"test $DREAM_K = 0 || ./agent.sh\"").replace("budget = 12", "budget = 4"));
    let t = open(&r, None).rounds(1, false, None, None, None).unwrap().remove(0);
    assert!(t.nodes.iter().any(|n| n.status == "noop"));
}

#[test]
fn runs_are_independent() {
    let r = repo("toy-space", "runs", |s| s.replace("budget = 25", "budget = 6"));
    open(&r, Some("a")).rounds(1, true, None, None, None).unwrap();
    open(&r, Some("b")).rounds(1, false, None, None, None).unwrap();
    let e = open(&r, Some("a"));
    assert_eq!(e.mem.runs().unwrap(), vec!["a", "b"]);
    e.mem.delete_run("b").unwrap();
    assert_eq!(e.mem.runs().unwrap(), vec!["a"]);
}

#[test]
fn external_policy_and_cli() {
    // A policy in any language: continue the root while calls < k, then stop.
    let r = repo("toy-space", "external", |s| {
        s.replace("budget = 25", "budget = 8").replace("candidates = 200", "candidates = 10") + "\n[policies]\nfamilies = [\"first_k\"]\n[policies.external.first_k]\ncommand = \"./first_k.sh\"\nsource = \"first_k.sh\"\ngrid = { k = [1, 2, 3] }\n"
    });
    let script = r#"#!/bin/sh
while read -r line; do
  k=$(printf '%s' "$line" | sed -n 's/^{"params":{"k":\([0-9]*\)}.*/\1/p')
  c=$(printf '%s' "$line" | sed -n 's/.*"calls":\([0-9]*\).*/\1/p')
  if [ "$c" -lt "$k" ]; then echo 0; else echo null; fi
done
"#;
    let sp = r.0.join("first_k.sh");
    std::fs::write(&sp, script).unwrap();
    git(&r.0, &["update-index", "--chmod=+x", "--add", "first_k.sh"]);
    #[cfg(unix)]
    Command::new("chmod").arg("+x").arg(&sp).status().unwrap();
    let e = open(&r, None);
    e.rounds(1, true, None, None, None).unwrap();
    let ranking = e.dream(None, Some(10), None, None, true).unwrap();
    let fams: std::collections::BTreeSet<String> = ranking.iter().map(|(p, _)| p.family.clone()).collect();
    assert_eq!(fams, ["first_k".to_string(), "refine_best".to_string()].into());
    let k1 = ranking.iter().find(|(p, _)| p.repr() == "first_k(k=1)").unwrap();
    assert_eq!(k1.1.calls, 1);
    assert!(git(&r.0, &["show", "dream/main/policy:first_k.sh"]).contains("calls"));

    // the binary: status / show / replay / policies / init
    let bin = env!("CARGO_BIN_EXE_git-dream");
    let run = |args: &[&str]| {
        let o = Command::new(bin).args(args).current_dir(&r.0).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    assert!(run(&["status"]).contains("r01"));
    assert!(run(&["show", "1"]).contains("<- promoted"));
    assert!(run(&["replay", "first_k:k=2", "-v"]).contains("first_k(k=2)"));
    assert!(run(&["policies"]).contains("repair"));
    std::fs::remove_file(r.0.join(".git-dream.toml")).unwrap();
    run(&["init", "--kind", "command"]);
    assert!(std::fs::read_to_string(r.0.join(".git-dream.toml")).unwrap().contains("kind = \"command\""));
}
