//! End-to-end tests on the examples, in throwaway git repositories.

use git_dream::engine::Engine;
use git_dream::sim::{self, Mode, ReplayWorld};
use git_dream::tree::{Tree, PENDING};
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
        if e.file_name() == "evidence" {
            continue; // what a recorded run produced is not part of the project
        }
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

/// The agent gets every attempt of every round (write-ups, results, failure
/// output); its multi-line proposal is kept in full; failures get a trailer.
#[test]
fn agent_sees_whole_history() {
    let agent = r#"command = 'cp "$DREAM_CONTEXT" "$DREAM_WORKTREE/../history.md" && ./agent.sh && printf "\nIdea: a test write-up.\nSecond line.\n" >> "$DREAM_MESSAGE_FILE"'"#;
    let eval = r#"command = 'if [ "$DREAM_NODE" -ge 2 ]; then echo "solver.awk:3: syntax error" >&2; exit 3; fi; ./bench.sh'"#;
    let r = repo("toy-command", "history", |s| s.replace("budget = 12", "budget = 4").replace("command = \"./agent.sh\"", agent).replace("command = \"./bench.sh\"", eval));
    let e = open(&r, None);
    let trees = e.rounds(2, false, None, None, None).unwrap();
    let ctx = std::fs::read_to_string(r.0.join(".git/dream/wt/history.md")).unwrap();
    for want in ["## Round 1", "## Round 2", "> Second line.", "fail (exit 3)", "solver.awk:3: syntax error", "**you are continuing this attempt**"] {
        assert!(ctx.contains(want), "history lacks {want:?}:\n{ctx}");
    }
    let edits: Vec<_> = trees.iter().flat_map(|t| &t.nodes[1..]).filter(|n| n.status != "noop").collect();
    assert!(edits.iter().all(|n| n.change.ends_with("\n\nIdea: a test write-up.\nSecond line.") && n.change.lines().count() == 4));
    // the full write-up round-trips through git
    for t in &trees {
        let lt = e.mem.load_tree("main", t.round).unwrap().unwrap();
        assert_eq!(lt.nodes.iter().map(|n| &n.change).collect::<Vec<_>>(), t.nodes.iter().map(|n| &n.change).collect::<Vec<_>>());
    }
    let failed = edits.iter().find(|n| n.status == "fail").expect("a failed attempt");
    assert!(git(&r.0, &["log", "-1", "--format=%B", &failed.sha]).contains("Dream-Failure: exit 3"));
    // the agent's files never enter a commit
    for n in &edits {
        assert!(!git(&r.0, &["ls-tree", "-r", "--name-only", &n.sha]).contains(".dream-agent"));
    }
}

#[test]
fn history_all_pools_runs() {
    let agent = r#"command = 'cp "$DREAM_CONTEXT" "$DREAM_WORKTREE/../history-$DREAM_RUN.md" && ./agent.sh'"#;
    let eval = r#"command = 'if [ "$DREAM_RUN" = a ] && [ "$DREAM_NODE" -ge 2 ]; then echo "only-in-run-a" >&2; exit 3; fi; ./bench.sh'"#;
    let r = repo("toy-command", "pool", |s| s.replace("budget = 12", "budget = 3").replace("command = \"./agent.sh\"", agent).replace("command = \"./bench.sh\"", eval));
    open(&r, Some("a")).rounds(1, false, None, None, None).unwrap();
    // default: runs don't see each other
    open(&r, Some("b")).rounds(1, false, None, None, None).unwrap();
    let wt = r.0.join(".git/dream/wt");
    let b = std::fs::read_to_string(wt.join("history-b.md")).unwrap();
    assert!(!b.contains("only-in-run-a") && b.contains("## Round 1"), "{b}");
    // history = "all": run b reads run a's attempts, labelled by run, before its own
    let cfg = r.0.join(".git-dream.toml");
    std::fs::write(&cfg, std::fs::read_to_string(&cfg).unwrap().replace("[dream]\n", "[dream]\nhistory = \"all\"\n")).unwrap();
    open(&r, Some("b")).rounds(1, false, None, None, None).unwrap();
    let b = std::fs::read_to_string(wt.join("history-b.md")).unwrap();
    for want in ["only-in-run-a", "other run `a` in", "## Run a, round 1", "### a r01 n002", "### b r02 n000"] {
        assert!(b.contains(want), "history lacks {want:?}:\n{b}");
    }
    assert!(b.find("## Run a, round 1") < b.find("## Run b, round 1"));
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
        s.replace("budget = 25", "budget = 8").replace("candidates = 200", "candidates = 10") + "\n[policies]\ninitial = { family = \"refine_best\" }\nfamilies = [\"first_k\"]\n[policies.external.first_k]\ncommand = \"./first_k.sh\"\nsource = \"first_k.sh\"\ngrid = { k = [1, 2, 3] }\n"
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

fn loose_objects(dir: &Path) -> usize {
    git(dir, &["count-objects", "-v"]).lines().find_map(|l| l.strip_prefix("count: ")).unwrap().parse().unwrap()
}

#[test]
fn rounds_are_packed() {
    let r = repo("toy-space", "pack", |s| s.replace("budget = 25", "budget = 6") + "\n[storage]\ngc_every = 2\n");
    let e = open(&r, None);
    e.rounds(1, false, None, None, None).unwrap();
    assert_eq!(loose_objects(&r.0), 0, "round 1 should be repacked");
    let packs = || std::fs::read_dir(r.0.join(".git/objects/pack")).unwrap().filter(|f| f.as_ref().unwrap().path().extension().is_some_and(|x| x == "pack")).count();
    assert!(packs() >= 1);
    e.rounds(1, false, None, None, None).unwrap();
    assert_eq!(loose_objects(&r.0), 0);
    assert_eq!(packs(), 1, "round 2 should run git gc and merge the packs");
    // switched off: attempts stay loose
    let r = repo("toy-space", "nopack", |s| s.replace("budget = 25", "budget = 6") + "\n[storage]\nrepack = false\ngc_every = 0\n");
    open(&r, None).rounds(1, false, None, None, None).unwrap();
    assert!(loose_objects(&r.0) > 0);
}

#[test]
fn exploration_floor_replays_exactly_and_gate_waits_for_history() {
    let r = repo("toy-space", "floor", |s| s.replace("budget = 25", "budget = 6\nmin_calls = 3") + "\n[policies]\ninitial = { family = \"ucb\", params = { patience = 0 } }\n");
    let e = open(&r, None);
    let trees = e.rounds(2, true, None, Some(10), None).unwrap();
    assert!(trees.iter().all(|t| t.calls() == 3));
    check_exact_replay(&e);
    assert_eq!(e.mem.policy_doc("main", Some(2)).unwrap()["accepted"], false);
    let o = Command::new(env!("CARGO_BIN_EXE_git-dream")).args(["replay", "--holdout", "1"]).current_dir(&r.0).output().unwrap();
    assert!(o.status.success(), "{}", String::from_utf8_lossy(&o.stderr));
    assert!(String::from_utf8_lossy(&o.stdout).contains("held-out:"));
}

#[test]
fn confirmation_rejects_failures_and_carries_fresh_score() {
    let r = repo("toy-space", "confirm", |s| s.replace("budget = 25", "budget = 2").replace("command = \"./bench.sh\"", "command = './confirm.sh'").replace("timeout = 30", "timeout = 30\nconfirm_top = 1\nconfirm_reps = 1"));
    // Counter is deliberately outside materialized files: the second measurement
    // of a candidate fails. The root remains valid, with a changed fresh score.
    std::fs::write(r.0.join("confirm.sh"), r#"#!/bin/sh
f="$DREAM_WORKTREE/../count-$DREAM_ROUND-$DREAM_NODE"
n=0
[ ! -f "$f" ] || n=$(cat "$f")
n=$((n+1)); echo "$n" > "$f"
if [ "$DREAM_NODE" = 0 ]; then echo $((100+n));
elif [ "$n" = 1 ]; then echo 1;
else echo rejected >&2; exit 3; fi
"#).unwrap();
    git(&r.0, &["add", "confirm.sh"]);
    git(&r.0, &["update-index", "--chmod=+x", "confirm.sh"]);
    git(&r.0, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qm", "evaluator"]);
    let e = open(&r, None);
    let t = e.explore(None, None).unwrap();
    assert_eq!(e.mem.rev("dream/main/best").unwrap(), t.root().sha);
    let rejected = e.mem.read_note(&t.nodes[1].sha).unwrap();
    assert_eq!(rejected["confirmation"]["score"], serde_json::Value::Null);
    assert!(rejected["confirmation"]["evaluations"][0]["stderr"].as_str().unwrap().contains("rejected"));
    let next = e.explore(None, Some(1)).unwrap();
    assert_eq!(next.root().score, Some(102.0));
}

#[test]
fn invalid_external_policy_is_an_error_not_a_cheap_stopping_rule() {
    let r = repo("toy-space", "invalid-policy", |s| s + "\n[policies]\ninitial = {family = \"broken\"}\n[policies.external.broken]\ncommand = \"echo bogus\"\n");
    let err = open(&r, None).explore(None, Some(1)).unwrap_err();
    assert!(format!("{err:#}").contains("replied"));
}

#[test]
fn invalid_config_is_rejected_before_creating_history() {
    for (i, edit) in [
        ("budget = 25", "budget = 0"),
        ("lam = 0.02", "lam = nan"),
        ("timeout = 30", "timeout = inf"),
        ("mode = \"cache\"", "mode = \"cache\"\nnormalize = \"typo\""),
    ].iter().enumerate() {
        let r = repo("toy-space", &format!("invalid-{i}"), |s| s.replace(edit.0, edit.1));
        assert!(Engine::open(&r.0, None, None).is_err());
        assert!(git(&r.0, &["for-each-ref", "refs/dream/"]).is_empty());
    }
}

#[test]
fn protected_evaluator_edits_are_preserved_but_rejected() {
    let r = repo("toy-command", "protected", |s| s.replace("command = \"./agent.sh\"", "command = 'echo hacked > bench.sh'").replace("[evaluate]", "[evaluate]\nprotected = [\"bench.sh\"]"));
    let e = open(&r, None);
    let t = e.explore(None, Some(1)).unwrap();
    assert_eq!(t.nodes[1].status, "fail");
    assert_eq!(t.nodes[1].score, None);
    assert_eq!(git(&r.0, &["show", &format!("{}:bench.sh", t.nodes[1].sha)]), "hacked");
    assert_eq!(e.mem.read_note(&t.nodes[1].sha).unwrap()["exit"], "protected paths changed");
}

#[test]
fn external_policy_deadline_is_an_error() {
    let r = repo("toy-space", "policy-timeout", |s| s + "\n[policies]\ninitial = {family = \"stuck\"}\n[policies.external.stuck]\ncommand = \"sleep 5\"\ntimeout = 0.05\n");
    let start = std::time::Instant::now();
    let err = open(&r, None).explore(None, Some(1)).unwrap_err();
    assert!(format!("{err:#}").contains("deadline"));
    assert!(start.elapsed().as_secs_f64() < 3.0);
}

#[test]
fn incompatible_replay_objectives_are_rejected() {
    let r = repo("toy-space", "incompatible", |s| s.replace("budget = 25", "budget = 2"));
    open(&r, None).explore(None, None).unwrap();
    let cfg = r.0.join(".git-dream.toml");
    std::fs::write(&cfg, std::fs::read_to_string(&cfg).unwrap().replace("goal = \"min\"", "goal = \"max\"")).unwrap();
    let e = open(&r, None);
    let err = e.replay(&e.current_policy().unwrap(), None, None).unwrap_err();
    assert!(format!("{err:#}").contains("incompatible agent/evaluator"));
}

#[test]
fn reuse_survives_reopen_but_does_not_disable_root_remeasurement() {
    let r = repo("toy-space", "reuse", |_| r#"
[agent]
knobs = { x = [1] }
[evaluate]
command = "echo 7"
reuse = true
remeasure_root = true
[explore]
budget = 1
"#.into());
    let first = open(&r, None).explore(None, None).unwrap();
    assert!(open(&r, None).mem.read_note(&first.nodes[1].sha).unwrap().get("reused").is_some());
    let e = open(&r, None);
    let cached = e.evaluate(&e.mem.tree_of(&first.root().sha).unwrap(), &first.root().design, 2, 1, true).unwrap();
    assert!(cached.note.get("reused").is_some());
    let second = e.explore(None, None).unwrap();
    assert!(e.mem.read_note(&second.root().sha).unwrap().get("seconds").is_some());
}

#[test]
fn held_out_regression_blocks_training_winner() {
    let r = repo("toy-space", "gate", |s| {
        s.replace("budget = 25", "budget = 5")
            .replace("command = \"./bench.sh\"", "command = 'if [ $DREAM_NODE = 0 ]; then echo 10; elif [ $DREAM_ROUND = 2 ] && [ $DREAM_NODE -ge 2 ]; then echo 1; else echo 9; fi'")
            + "\n[policies]\ninitial = { family = \"refine_best\" }\nfamilies = [\"first_k\"]\n[policies.external.first_k]\ncommand = \"sh first_k.sh\"\ngrid = { k = [1] }\n"
    });
    std::fs::write(r.0.join("first_k.sh"), r#"
while read -r line; do
  c=$(printf '%s' "$line" | sed -n 's/.*"calls":\([0-9]*\).*/\1/p')
  if [ "$c" = 0 ]; then echo 0; else echo null; fi
done
"#).unwrap();
    let e = open(&r, None);
    e.rounds(2, false, None, None, None).unwrap();
    let ranking = e.dream(None, Some(5), None, None, true).unwrap();
    assert_eq!(ranking[0].0.family, "first_k", "candidate should win on training only");
    let doc = e.mem.policy_doc("main", Some(3)).unwrap();
    assert_eq!(doc["accepted"], false);
    assert_eq!(doc["policy"]["family"], "refine_best");
    assert!(doc["validation"]["candidate_J"].as_f64().unwrap() < doc["validation"]["current_J"].as_f64().unwrap());
}

#[test]
fn run_killed_mid_round_resumes_from_recorded_attempts() {
    let r = repo("toy-space", "resume", |s| s.replace("budget = 25", "budget = 6"));
    let e = open(&r, None);
    e.rounds(2, false, None, None, None).unwrap();
    // Simulate a kill after round 2's third attempt: later attempts and the promotion never happened.
    for i in 4..=6 {
        let _ = git(&r.0, &["update-ref", "-d", &format!("refs/dream/main/r02/n{i:03}")]);
    }
    git(&r.0, &["update-ref", "-d", "refs/dream/main/r02/best"]);
    let e = open(&r, None);
    let trees = e.rounds(1, true, None, None, None).unwrap();
    assert_eq!(trees[0].round, 3);
    assert_eq!(e.mem.rounds("main").unwrap(), vec![1, 2, 3]);
    let r2 = e.mem.load_tree("main", 2).unwrap().unwrap();
    assert_eq!(r2.calls(), 3);
    let best = git(&r.0, &["rev-parse", "refs/dream/main/r02/best"]);
    assert_eq!(best, r2.best().unwrap().sha, "interrupted round promotes its best recorded attempt");
    assert_eq!(git(&r.0, &["rev-parse", &format!("{}^", trees[0].root().sha)]), best, "round 3 continues from it");
}

/// The default policy learns from measured outcomes; a round can be extended
/// (or finished after a kill) and is then identical to one that never stopped;
/// a new round starts from the yield model fitted over recorded history.
#[test]
fn bandit_extends_rounds_and_dreams_a_yield_model() {
    let r = repo("toy-space", "bandit", |s| s.replace("budget = 25", "budget = 6"));
    let e = open(&r, Some("a"));
    assert_eq!(e.initial_policy().unwrap().family, "bandit");
    // three steps of 6 in one round == one round of 18
    let steps = e.run(3, true, None, None, None, true, false).unwrap();
    assert_eq!(e.mem.rounds("a").unwrap(), vec![1]);
    assert_eq!(steps.iter().map(|t| t.calls()).collect::<Vec<_>>(), vec![6, 12, 18]);
    let whole = open(&r, Some("b")).explore(Some(1), Some(18)).unwrap();
    assert_eq!(sig(&steps[2]), sig(&whole));
    assert_eq!(sig(&e.mem.load_tree("a", 1).unwrap().unwrap()), sig(&whole));
    check_exact_replay(&e);
    assert!(sig(&whole).iter().any(|n| n.0.is_some_and(|p| p != 0) || n.1.is_some_and(|k| k > 0)), "more than a chain");

    // a killed round (attempts lost after n012, never promoted) is finished, not abandoned
    for i in 13..=18 {
        git(&r.0, &["update-ref", "-d", &format!("refs/dream/a/r01/n{i:03}")]);
    }
    git(&r.0, &["update-ref", "-d", "refs/dream/a/r01/best"]);
    let e = open(&r, Some("a"));
    assert_eq!(sig(&e.extend(Some(6)).unwrap()), sig(&whole));
    assert_eq!(git(&r.0, &["rev-parse", "refs/dream/a/r01/best"]), e.mem.load_tree("a", 1).unwrap().unwrap().best().unwrap().sha);

    // a new round: the policy carries the measured yield of each kind of move
    let t2 = e.run(1, true, None, None, None, true, true).unwrap().remove(0);
    assert_eq!((t2.round, t2.calls()), (2, 6));
    let doc = e.mem.policy_doc("a", Some(2)).unwrap();
    assert_eq!(doc["method"], "model");
    let weight: f64 = doc["model"].as_array().unwrap().iter().map(|m| m["weight"].as_f64().unwrap()).sum();
    assert!(weight > 10.0 && weight <= 18.0, "discounted weight of 18 recorded attempts: {weight}");
    assert!(doc["policy"]["params"].as_object().unwrap().keys().any(|k| k.starts_with("n_best")));
    check_exact_replay(&e);

    // the binary: extend by flag, and show the model without deploying it
    let bin = env!("CARGO_BIN_EXE_git-dream");
    let run = |args: &[&str]| {
        let o = Command::new(bin).args(args).current_dir(&r.0).output().unwrap();
        assert!(o.status.success(), "{args:?}: {}", String::from_utf8_lossy(&o.stderr));
        String::from_utf8_lossy(&o.stdout).into_owned()
    };
    assert!(run(&["run", "--run", "a", "--extend", "--budget", "2"]).contains("extend r2"));
    assert_eq!(e.mem.load_tree("a", 2).unwrap().unwrap().calls(), 8);
    assert!(run(&["dream", "--run", "a", "--dry-run"]).contains("yield model over 26 recorded attempts"));
    assert!(e.mem.policy_doc("a", Some(3)).is_none());
    // extension fast-forwards any policy through its recorded attempts, including one with random state
    let r2 = repo("toy-space", "extend-softmax", |s| s.replace("budget = 25", "budget = 5") + "\n[policies]\ninitial = { family = \"softmax\", params = { T = 0.3 } }\n");
    let e2 = open(&r2, Some("a"));
    let parts = e2.run(2, false, None, None, None, true, false).unwrap();
    assert_eq!(sig(&parts[1]), sig(&open(&r2, Some("b")).explore(Some(1), Some(10)).unwrap()));
}

/// Every line a script appended to the test's log, in order.
fn log_lines(r: &Repo) -> Vec<String> {
    std::fs::read_to_string(r.0.join("log")).unwrap_or_default().lines().map(String::from).collect()
}

/// Wrap the toy evaluator: note in `log` (outside the worktrees) when it starts and ends.
fn logged_eval(r: &Repo, sleep: &str) -> String {
    let log = r.0.join("log");
    format!("command = \"echo e+ $DREAM_LANE >> {0}; sleep {sleep}; x=$(./bench.sh); s=$?; echo e- >> {0}; echo $x; exit $s\"", log.display())
}

/// A score that does not depend on machine load may be measured for several
/// attempts at once. Which results a later choice was made without is
/// recorded, so the round can still be extended.
#[test]
fn parallel_evaluations_overlap_and_the_round_extends() {
    let r = repo("toy-space", "parallel-space", |s| s);
    let cfg = std::fs::read_to_string(r.0.join(".git-dream.toml")).unwrap();
    let cfg = cfg.replace("command = \"./bench.sh\"", &logged_eval(&r, "0.3")).replace("budget = 25", "budget = 8\nparallel = 4\nevaluations = \"parallel\"");
    std::fs::write(r.0.join(".git-dream.toml"), cfg).unwrap();
    git(&r.0, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "config"]);
    let e = open(&r, None);
    assert_eq!(e.lanes(), 4);
    let t = e.explore(Some(1), None).unwrap();
    assert_eq!(t.calls(), 8);
    assert!(t.nodes.iter().all(|n| n.status != PENDING));
    let log = log_lines(&r);
    assert_eq!((&log[0][..2], log[1].as_str()), ("e+", "e-"), "the root is measured by itself");
    assert!(log.windows(4).any(|w| w.iter().all(|l| l.starts_with("e+"))), "four evaluations at once: {log:?}");
    let lanes: std::collections::BTreeSet<&str> = log.iter().filter_map(|l| l.strip_prefix("e+ ")).collect();
    assert_eq!(lanes.into_iter().collect::<Vec<_>>(), ["0", "1", "2", "3"], "each in its own worktree");
    // the first four were chosen together: none continues another, none knew another's result
    assert!(t.nodes[1..5].iter().all(|n| n.parent == Some(0) && n.visible() >= 5));
    assert!(git(&r.0, &["log", "-1", "--format=%B", "refs/dream/main/r01/n001"]).contains("Dream-Seen: "));
    let stored = e.mem.load_tree("main", 1).unwrap().unwrap();
    assert_eq!(sig(&stored), sig(&t));
    assert_eq!(stored.nodes.iter().map(|n| n.visible()).collect::<Vec<_>>(), t.nodes.iter().map(|n| n.visible()).collect::<Vec<_>>());
    // extending drives the policy through the round again, showing it what it saw then
    let more = open(&r, None).extend(Some(5)).unwrap();
    assert_eq!(more.calls(), 13);
    assert_eq!(sig(&more)[..9], sig(&t)[..]);
    // one attempt at a time is still available, and a new round learns from the parallel one
    let mut one = open(&r, None);
    one.cfg.explore.parallel = 1;
    assert_eq!(one.extend(Some(2)).unwrap().calls(), 15);
    let t2 = one.run(1, true, None, None, None, true, true).unwrap().remove(0);
    assert_eq!((t2.round, t2.calls()), (2, 8));
    assert_eq!(one.mem.policy_doc("main", Some(2)).unwrap()["method"], "model");
    // with a load-dependent score (the default), a parameter search gains nothing from lanes
    let mut alone = open(&r, None);
    alone.cfg.explore.evaluations = "alone".into();
    assert_eq!(alone.lanes(), 1);
}

/// Agents work at once; with the default `evaluations = "alone"` every result
/// is then measured while nothing else runs.
#[test]
fn parallel_agents_work_together_and_are_measured_alone() {
    let r = repo("toy-command", "parallel-command", |s| s);
    let log = r.0.join("log");
    let cfg = std::fs::read_to_string(r.0.join(".git-dream.toml")).unwrap();
    let cfg = cfg
        .replace("command = \"./agent.sh\"", &format!("command = \"echo a+ >> {0}; sleep 0.3; ./agent.sh; echo a- >> {0}\"", log.display()))
        .replace("command = \"./bench.sh\"", &logged_eval(&r, "0.05"))
        .replace("budget = 12", "budget = 6\nparallel = 3");
    std::fs::write(r.0.join(".git-dream.toml"), cfg).unwrap();
    git(&r.0, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "config"]);
    let e = open(&r, None);
    let t = e.explore(Some(1), None).unwrap();
    assert_eq!(t.calls(), 6);
    let log = log_lines(&r);
    assert!(log.windows(3).any(|w| w.iter().all(|l| l == "a+")), "three agents at once: {log:?}");
    for (i, l) in log.iter().enumerate() {
        if l.starts_with("e+") {
            assert_eq!(log[i + 1], "e-", "nothing runs during an evaluation: {log:?}");
        }
    }
    // two waves of three: the second was chosen knowing all of the first
    assert_eq!(t.nodes.iter().skip(1).map(|n| n.visible()).collect::<Vec<_>>(), [4, 4, 4, 7, 7, 7]);
    for n in &t.nodes[1..] {
        assert_eq!(git(&r.0, &["rev-parse", &format!("{}^", n.sha)]), t.nodes[n.parent.unwrap()].sha);
    }
    assert_eq!(open(&r, None).extend(Some(3)).unwrap().calls(), 9);
}

/// A round killed while several attempts were running has, in git, every
/// attempt that was started. The unfinished ones are run again.
#[test]
fn killed_parallel_round_reruns_unfinished_attempts() {
    let r = repo("toy-space", "parallel-kill", |s| s);
    let cfg = std::fs::read_to_string(r.0.join(".git-dream.toml")).unwrap();
    let cfg = cfg.replace("command = \"./bench.sh\"", "command = \"sleep 0.5; ./bench.sh\"").replace("budget = 25", "budget = 12\nparallel = 4\nevaluations = \"parallel\"");
    std::fs::write(r.0.join(".git-dream.toml"), cfg).unwrap();
    git(&r.0, &["-c", "user.name=t", "-c", "user.email=t@t", "commit", "-qam", "config"]);
    let mut child = Command::new(env!("CARGO_BIN_EXE_git-dream")).args(["explore"]).current_dir(&r.0).stdout(std::process::Stdio::null()).spawn().unwrap();
    let started = std::time::Instant::now();
    while Command::new("git").args(["rev-parse", "-q", "--verify", "refs/dream/main/r01/n006"]).current_dir(&r.0).output().unwrap().stdout.is_empty() {
        assert!(started.elapsed().as_secs() < 60 && child.try_wait().unwrap().is_none(), "the round never got to its sixth attempt");
        std::thread::sleep(std::time::Duration::from_millis(20));
    }
    child.kill().unwrap();
    child.wait().unwrap();
    std::thread::sleep(std::time::Duration::from_millis(800)); // evaluators it left behind
    let e = open(&r, None);
    let rec = e.mem.load_tree("main", 1).unwrap().unwrap();
    let unfinished = rec.nodes.iter().filter(|n| n.status == PENDING).count();
    assert!(rec.calls() >= 6 && (1..=4).contains(&unfinished), "{} started, {unfinished} unfinished", rec.calls());
    // history with unfinished attempts can be shown and learnt from
    let show = Command::new(env!("CARGO_BIN_EXE_git-dream")).args(["show"]).current_dir(&r.0).output().unwrap();
    assert!(show.status.success() && String::from_utf8_lossy(&show.stdout).contains("pending"));
    e.dream(None, None, None, None, false).unwrap();
    let t = e.extend(Some(2)).unwrap();
    assert_eq!(t.calls(), rec.calls() + 2);
    assert!(t.nodes.iter().all(|n| n.status != PENDING));
    for (a, b) in rec.nodes.iter().zip(&t.nodes) {
        assert_eq!((a.parent, a.k), (b.parent, b.k), "attempt {}", a.id);
        if a.status != PENDING {
            assert_eq!((a.score, &a.sha), (b.score, &b.sha), "finished attempts are kept");
        }
    }
    assert_eq!(sig(&e.mem.load_tree("main", 1).unwrap().unwrap()), sig(&t));
}

/// An agent is told which continuations of its attempt are being made right
/// now by others; attempts without a result do not appear as history.
#[test]
fn agents_are_told_about_attempts_running_beside_them() {
    use git_dream::engine::{agent_context, history_context};
    use git_dream::tree::{Goal, Node, OK};
    let mut t = Tree::new("main", 1, 9, Goal::Min, "");
    t.set_root(Node::new(0, None, None, "a", Some(10.0), OK, "base"));
    t.add(Node::new(1, Some(0), Some(0), "b", Some(9.0), OK, "first idea"));
    let serial = (agent_context(&t, 0, 1), history_context(&[&t], 0, 1, &Default::default()));
    // this attempt (n2, continuation #2 of the root) and one started beside it (n3)
    t.add(Node::pending(2, 0, 1));
    assert_eq!((agent_context(&t, 0, 1), history_context(&[&t], 0, 1, &Default::default())), serial, "an attempt running by itself is told what it always was");
    t.add(Node::pending(3, 0, 2));
    let ctx = agent_context(&t, 0, 1);
    assert!(ctx.contains("## Already tried from this attempt\n- n001 ok 9: first idea") && ctx.contains("1 other agent started from this same attempt"), "{ctx}");
    let hist = history_context(&[&t], 0, 1, &Default::default());
    assert!(hist.contains("(1 attempt, best n001 9, 1 more without a result yet)") && !hist.contains("n002") && !hist.contains("n003"), "{hist}");
}
