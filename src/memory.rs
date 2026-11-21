//! Git as the persistent experimental memory.
//!
//! Everything lives inside the repository being explored, outside its branches,
//! so the normal history and the working tree are never touched:
//!
//! ```text
//! refs/dream/<run>/base              commit the run started from
//! refs/dream/<run>/rNN/nNNN          one commit per attempt (hidden refs = the discovery tree)
//! refs/dream/<run>/rNN/best          attempt promoted at the end of round NN
//! refs/heads/dream/<run>/best        branch -> latest promoted attempt (check it out / merge it)
//! refs/heads/dream/<run>/policy      branch: the exploration policy, one commit per version. Its tree is the
//!                                    policy: policy.json (a built-in family and its parameters, or the command
//!                                    that starts a program) and that program's files; dream.json says how the
//!                                    version was chosen
//! refs/tags/dream/<run>/policy-rNN   tag: the policy round NN used
//! refs/notes/dream                   evaluator output per attempt (git notes)
//! refs/dream/<run>.policy/...        the same layout again, one level up: the candidate policies a
//!                                    policy-development agent wrote, each a commit scored by replay
//! ```
//!
//! An attempt commit's parent is the attempt it continued; a round's root commit
//! has the previous round's promoted attempt as parent, so all rounds of a run
//! form one DAG rooted at `base`. Outcomes are commit-message trailers
//! (Dream-Score, Dream-Status, Dream-Design, ...), so `git log` alone rebuilds
//! every tree.

use crate::tree::{Goal, Node, Tree, NOOP, OK};
use std::sync::atomic::{AtomicUsize, Ordering};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const REFSPECS: [&str; 4] = ["refs/dream/*:refs/dream/*", "refs/heads/dream/*:refs/heads/dream/*", "refs/tags/dream/*:refs/tags/dream/*", "refs/notes/dream:refs/notes/dream"];

/// The policy, in the tree of a policy commit.
pub const POLICY_FILE: &str = "policy.json";
/// How that version was chosen: written by git-dream, not part of the policy.
pub const REPORT_FILE: &str = "dream.json";

/// The run that holds the candidate policies written for `run`.
pub fn policy_run(run: &str) -> String {
    format!("{run}.policy")
}

pub fn is_policy_run(run: &str) -> bool {
    run.ends_with(".policy")
}

#[derive(Clone)]
pub struct Memory {
    pub cwd: PathBuf,
    pub top: PathBuf,
    pub common: PathBuf,
    /// `.git/dream/`: temporary files.
    pub state: PathBuf,
    /// This repository's scratch worktrees (see [`worktree_root`]).
    pub worktrees: PathBuf,
    env: Vec<(String, String)>,
}

pub fn check_run(run: &str) -> Result<()> {
    let ok = !run.is_empty() && run.chars().next().unwrap().is_ascii_alphanumeric() && run.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !ok {
        bail!("bad run name {run:?} (letters, digits, . _ -)");
    }
    Ok(())
}

/// Where a repository's scratch worktrees live: `$GIT_DREAM_WORKTREES/<repo>`,
/// else `<user cache directory>/git-dream/<repo>`. Never under `.git`: coding
/// agents refuse to edit files there (Claude Code calls them sensitive files),
/// so an agent working in `.git/dream/wt` cannot change the code. `<repo>` is
/// the directory's name plus a hash of its path. Without a home directory the
/// old place, `.git/dream/wt`, is all that is left.
fn worktree_root(common: &Path) -> PathBuf {
    let dir = |var: &str| std::env::var_os(var).filter(|v| !v.is_empty()).map(PathBuf::from);
    let name = common.parent().and_then(Path::file_name).map_or("repo".into(), |n| n.to_string_lossy().into_owned());
    let key = format!("{name}-{:016x}", crate::rng::hash(&common.to_string_lossy()));
    let base = dir("GIT_DREAM_WORKTREES")
        .or_else(|| dir("XDG_CACHE_HOME").map(|d| d.join("git-dream")))
        .or_else(|| dir("LOCALAPPDATA").map(|d| d.join("git-dream")))
        .or_else(|| dir("HOME").map(|d| d.join(".cache").join("git-dream")));
    base.map_or_else(|| common.join("dream").join("wt"), |b| b.join(key))
}

fn fmt_score(s: Option<f64>) -> String {
    match s {
        None => "null".into(),
        // shortest representation that parses back to the same f64: replay is exact
        Some(x) => format!("{x:?}"),
    }
}

impl Memory {
    pub fn open(cwd: &Path) -> Result<Memory> {
        let mut m = Memory { cwd: cwd.into(), top: PathBuf::new(), common: PathBuf::new(), state: PathBuf::new(), worktrees: PathBuf::new(), env: vec![("GIT_TERMINAL_PROMPT".into(), "0".into())] };
        m.top = PathBuf::from(m.git(&["rev-parse", "--show-toplevel"]).context("not inside a git repository")?);
        m.common = PathBuf::from(m.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
        m.state = m.common.join("dream");
        m.worktrees = worktree_root(&m.common);
        if m.run(&["var", "GIT_COMMITTER_IDENT"], None, None, &[]).is_err() {
            for k in ["AUTHOR", "COMMITTER"] {
                m.env.push((format!("GIT_{k}_NAME"), "git-dream".into()));
                m.env.push((format!("GIT_{k}_EMAIL"), "git-dream@localhost".into()));
            }
        }
        Ok(m)
    }

    pub fn run(&self, args: &[&str], input: Option<&str>, cwd: Option<&Path>, env: &[(String, String)]) -> Result<String> {
        let mut c = Command::new("git");
        c.args(args).current_dir(cwd.unwrap_or(&self.cwd)).stdin(if input.is_some() { Stdio::piped() } else { Stdio::null() }).stdout(Stdio::piped()).stderr(Stdio::piped());
        for (k, v) in self.env.iter().chain(env) {
            c.env(k, v);
        }
        let mut child = c.spawn().context("running git")?;
        if let Some(i) = input {
            let mut stdin = child.stdin.take().unwrap();
            let data = i.as_bytes().to_vec();
            std::thread::spawn(move || {
                let _ = stdin.write_all(&data);
            });
        }
        let out = child.wait_with_output()?;
        if !out.status.success() {
            bail!("git {}: {}", args.join(" "), String::from_utf8_lossy(&out.stderr).trim());
        }
        Ok(String::from_utf8_lossy(&out.stdout).trim().to_string())
    }

    pub fn git(&self, args: &[&str]) -> Result<String> {
        self.run(args, None, None, &[])
    }

    pub fn git_in(&self, args: &[&str], input: &str) -> Result<String> {
        self.run(args, Some(input), None, &[])
    }

    pub fn rev(&self, r: &str) -> Option<String> {
        self.git(&["rev-parse", "-q", "--verify", r]).ok().filter(|s| !s.is_empty())
    }

    pub fn tree_of(&self, commit: &str) -> Result<String> {
        self.rev(&format!("{commit}^{{tree}}")).ok_or_else(|| anyhow!("no tree for {commit}"))
    }

    // ---- objects ----------------------------------------------------------------
    pub fn blob(&self, text: &str) -> Result<String> {
        self.git_in(&["hash-object", "-w", "--stdin"], text)
    }

    /// Tree of commit/tree `base` (None = empty) with `files` (path, text) added or replaced.
    pub fn tree_with_files(&self, base: Option<&str>, files: &[(String, String)]) -> Result<String> {
        std::fs::create_dir_all(&self.state)?;
        // unique per call: attempts are built on several threads at once
        static NEXT: AtomicUsize = AtomicUsize::new(0);
        let idx = self.state.join(format!("index-{}-{}", std::process::id(), NEXT.fetch_add(1, Ordering::Relaxed)));
        let env = vec![("GIT_INDEX_FILE".to_string(), idx.to_string_lossy().into_owned())];
        let r = (|| {
            match base {
                Some(b) => self.run(&["read-tree", b], None, None, &env)?,
                None => self.run(&["read-tree", "--empty"], None, None, &env)?,
            };
            for (path, text) in files {
                let info = format!("100644,{},{path}", self.blob(text)?);
                self.run(&["update-index", "--add", "--cacheinfo", &info], None, None, &env)?;
            }
            self.run(&["write-tree"], None, None, &env)
        })();
        let _ = std::fs::remove_file(&idx);
        r
    }

    pub fn commit(&self, tree: &str, parents: &[&str], message: &str) -> Result<String> {
        let mut args = vec!["commit-tree", tree, "-F", "-"];
        for p in parents {
            args.extend(["-p", p]);
        }
        self.git_in(&args, message)
    }

    pub fn note(&self, sha: &str, obj: &Value) -> Result<()> {
        self.git_in(&["notes", "--ref=dream", "add", "-f", "-F", "-", sha], &(serde_json::to_string_pretty(obj)? + "\n"))?;
        Ok(())
    }

    pub fn read_note(&self, sha: &str) -> Option<Value> {
        self.git(&["notes", "--ref=dream", "show", sha]).ok().and_then(|s| serde_json::from_str(&s).ok())
    }

    /// The notes of many commits with one git call (commits without a note are left out).
    pub fn notes_of(&self, shas: &[&str]) -> Result<HashMap<String, Value>> {
        if shas.is_empty() {
            return Ok(HashMap::new());
        }
        let log = self.git_in(&["log", "--no-walk=unsorted", "--stdin", "--notes=dream", "--format=%H%x1f%N%x1e"], &(shas.join("\n") + "\n"))?;
        Ok(log
            .split('\x1e')
            .filter_map(|rec| {
                let (sha, note) = rec.trim().split_once('\x1f')?;
                Some((sha.to_string(), serde_json::from_str(note.trim()).ok()?))
            })
            .collect())
    }

    pub fn set_ref(&self, r: &str, sha: &str) -> Result<()> {
        self.git(&["update-ref", r, sha])?;
        Ok(())
    }

    // ---- runs -------------------------------------------------------------------
    /// Base commit of `run`, fixed the first time the run is used.
    pub fn base(&self, run: &str) -> Result<String> {
        check_run(run)?;
        let r = format!("refs/dream/{run}/base");
        if let Some(s) = self.rev(&r) {
            return Ok(s);
        }
        let s = self.rev("HEAD^{commit}").ok_or_else(|| anyhow!("the repository needs at least one commit"))?;
        self.set_ref(&r, &s)?;
        Ok(s)
    }

    fn refnames(&self, prefix: &str) -> Result<Vec<String>> {
        Ok(self.git(&["for-each-ref", "--format=%(refname)", prefix])?.lines().map(String::from).collect())
    }

    /// The runs of this repository (not the `<run>.policy` runs that hold their candidate policies).
    pub fn runs(&self) -> Result<Vec<String>> {
        let mut v: Vec<String> = self.refnames("refs/dream/")?.iter().filter_map(|r| r.strip_prefix("refs/dream/")?.strip_suffix("/base").map(String::from)).filter(|r| !r.contains('/') && !is_policy_run(r)).collect();
        v.sort();
        Ok(v)
    }

    pub fn rounds(&self, run: &str) -> Result<Vec<u32>> {
        let pre = format!("refs/dream/{run}/r");
        let mut v: Vec<u32> = self.refnames(&format!("refs/dream/{run}/"))?.iter().filter_map(|r| r.strip_prefix(&pre)?.split_once("/n")?.0.parse().ok()).collect();
        v.sort();
        v.dedup();
        Ok(v)
    }

    // ---- attempts -----------------------------------------------------------------
    pub fn commit_attempt(&self, t: &Tree, n: &Node, tree: &str, parent_sha: &str, note: Option<&Value>) -> Result<String> {
        let p = n.parent.map(|i| &t.nodes[i]);
        let change = n.change.trim();
        let first = change.lines().next().unwrap_or("");
        let subject = match p {
            None => format!("dream({} r{}): root {}", t.run, t.round, if first.is_empty() { &n.design } else { first }),
            Some(_) => format!("dream({} r{} n{:03}): {}", t.run, t.round, n.id, if first.is_empty() { "change" } else { first }),
        };
        let g = |s: Option<f64>| s.map_or("-".to_string(), |x| format!("{x:.6}"));
        let mut body = vec![format!("design: {}", n.design)];
        let mut rel = String::new();
        if let Some(p) = p {
            body.push(format!("parent: n{:03} score {}", p.id, g(p.score)));
            if let (Some(s), Some(ps)) = (n.score, p.score) {
                rel = format!(" ({:+.2}% better than parent)", t.rel(s, ps) * 100.0);
            }
        }
        body.push(format!("result: {}{}{rel}", n.status, n.score.map_or(String::new(), |s| format!(" score {s}"))));
        if change.lines().count() > 1 {
            body.push(String::new());
            body.push(change.to_string());
        }
        let opt = |o: Option<usize>| o.map_or(String::new(), |x| x.to_string());
        let mut trailers = vec![
            ("Run", t.run.clone()),
            ("Round", t.round.to_string()),
            ("Node", n.id.to_string()),
            ("Parent-Node", opt(n.parent)),
            ("Expansion", opt(n.k)),
            ("Design", n.design.clone()),
            ("Policy", t.policy.clone()),
            ("Budget", t.budget.to_string()),
            ("Goal", t.goal.as_str().into()),
            ("Status", n.status.clone()),
            ("Score", fmt_score(n.score)),
        ];
        // Only when the result arrived after later attempts had been chosen
        // (attempts running at once): which choices were made without it.
        if n.seen > n.id + 1 {
            trailers.push(("Seen", n.seen.to_string()));
        }
        // How the evaluation failed (measured, not a diagnosis: the evidence is in the note).
        if n.status != OK && n.status != NOOP {
            if let Some(f) = note.and_then(crate::agents::failure) {
                trailers.push(("Failure", f));
            }
        }
        let tr: Vec<String> = trailers.iter().map(|(k, v)| format!("Dream-{k}: {v}")).collect();
        let msg = format!("{subject}\n\n{}\n\n{}\n", body.join("\n"), tr.join("\n"));
        let sha = self.commit(tree, &[parent_sha], &msg)?;
        self.set_ref(&format!("refs/dream/{}/r{:02}/n{:03}", t.run, t.round, n.id), &sha)?;
        if let Some(nt) = note {
            self.note(&sha, nt)?;
        }
        Ok(sha)
    }

    /// Rebuild a discovery tree purely from git (refs + commit trailers).
    pub fn load_tree(&self, run: &str, round: u32) -> Result<Option<Tree>> {
        let out = self.git(&["for-each-ref", "--format=%(refname) %(objectname)", &format!("refs/dream/{run}/r{round:02}/")])?;
        let shas: Vec<&str> = out.lines().filter_map(|l| l.split_once(' ')).filter(|(r, _)| r.rsplit('/').next().is_some_and(|x| x.starts_with('n'))).map(|(_, s)| s).collect();
        if shas.is_empty() {
            return Ok(None);
        }
        // The outcome is read with git's own trailer parser: only the last
        // paragraph of a message is its trailers, so a line of an agent's
        // write-up that looks like one stays text.
        let log = self.git_in(&["log", "--no-walk=unsorted", "--stdin", "--format=%H%x1f%(trailers:only,unfold)%x1f%B%x1e"], &(shas.join("\n") + "\n"))?;
        let mut nodes = vec![];
        let mut meta: HashMap<String, String> = HashMap::new();
        for rec in log.split('\x1e') {
            let rec = rec.trim();
            if rec.is_empty() {
                continue;
            }
            let mut fields = rec.splitn(3, '\x1f');
            let (Some(sha), Some(trailers), Some(body)) = (fields.next(), fields.next(), fields.next()) else { bail!("bad log record") };
            let body = body.trim();
            let tr: HashMap<String, String> = trailers.lines().filter_map(|l| l.strip_prefix("Dream-")?.split_once(':')).map(|(k, v)| (k.to_string(), v.trim().to_string())).collect();
            let get = |k: &str| tr.get(k).cloned().ok_or_else(|| anyhow!("commit {sha} lacks Dream-{k}"));
            let subject = body.lines().next().unwrap_or("");
            // A multi-line change sits between the design/result paragraph and the trailers.
            let paras: Vec<&str> = body.split("\n\n").collect();
            let change = if paras.len() > 3 {
                paras[2..paras.len() - 1].join("\n\n")
            } else {
                let s = subject.split_once("): ").map_or("", |x| x.1);
                let root = tr.get("Parent-Node").is_none_or(|p| p.is_empty());
                if root { s.strip_prefix("root ").unwrap_or(s) } else { s }.to_string()
            };
            let num = |k: &str| -> Result<Option<usize>> { Ok(get(k).ok().filter(|s| !s.is_empty()).map(|s| s.parse()).transpose()?) };
            let score = get("Score")?;
            let mut n = Node::new(
                get("Node")?.parse()?,
                num("Parent-Node")?,
                num("Expansion")?,
                get("Design")?,
                if score == "null" { None } else { Some(score.parse()?) },
                &get("Status")?,
                change,
            );
            n.sha = sha.trim().to_string();
            n.seen = num("Seen")?.unwrap_or(0);
            // an extended round records a larger budget on its later attempts: keep the newest
            if nodes.iter().all(|m: &Node| m.id < n.id) {
                meta = tr;
            }
            nodes.push(n);
        }
        nodes.sort_by_key(|n| n.id);
        if nodes.iter().enumerate().any(|(i, n)| n.id != i) {
            bail!("round {round} of run {run:?} has missing attempts");
        }
        let mut t = Tree::new(run, round, meta.get("Budget").and_then(|b| b.parse().ok()).unwrap_or(0), Goal::parse(meta.get("Goal").map_or("min", |s| s)), meta.get("Policy").map_or("", |s| s));
        for i in 0..nodes.len() {
            anyhow::ensure!(nodes[i].score.is_none_or(f64::is_finite), "non-finite score in round {round} node {i}");
            anyhow::ensure!((i == 0) == nodes[i].parent.is_none(), "invalid root/parent in round {round} node {i}");
            if let Some(p) = nodes[i].parent {
                anyhow::ensure!(p < i, "invalid parent {p} in round {round} node {i}");
                nodes[p].n_exp += 1;
            }
        }
        let mut it = nodes.into_iter();
        t.set_root(it.next().unwrap());
        for n in it {
            t.add(n);
        }
        Ok(Some(t))
    }

    pub fn history(&self, run: &str) -> Result<Vec<Tree>> {
        let mut v = vec![];
        for r in self.rounds(run)? {
            if let Some(t) = self.load_tree(run, r)? {
                v.push(t);
            }
        }
        Ok(v)
    }

    // ---- policies -------------------------------------------------------------------
    /// A new version of the policy of `run`, for `round`: a commit on the policy
    /// branch whose tree is the policy. `spec` becomes `policy.json`, `report`
    /// (how it was chosen) `dream.json`, on top of `tree`: the tree a
    /// policy-development attempt left (`from`, recorded as a second parent, so
    /// `git log --graph` shows where the version came from), else the previous
    /// version's tree, so a policy program's files stay.
    pub fn commit_policy(&self, run: &str, round: u32, spec: &Value, report: &Value, files: &[(String, String)], from: Option<&str>) -> Result<String> {
        let branch = format!("refs/heads/dream/{run}/policy");
        let parent = self.rev(&branch);
        let mut all = vec![(POLICY_FILE.to_string(), serde_json::to_string_pretty(spec)? + "\n"), (REPORT_FILE.to_string(), serde_json::to_string_pretty(report)? + "\n")];
        all.extend(files.iter().cloned());
        let tree = self.tree_with_files(from.or(parent.as_deref()), &all)?;
        let repr = report["repr"].as_str().unwrap_or("");
        let why = report["why"].as_str().unwrap_or("");
        let msg = format!("policy({run} r{round:02}): {repr}\n\n{why}\n\nDream-Run: {run}\nDream-Round: {round}\nDream-Policy: {repr}\n");
        let parents: Vec<&str> = parent.as_deref().into_iter().chain(from).collect();
        let sha = self.commit(&tree, &parents, &msg)?;
        self.set_ref(&branch, &sha)?;
        self.set_ref(&format!("refs/tags/dream/{run}/policy-r{round:02}"), &sha)?;
        Ok(sha)
    }

    /// The policy commit of `run`: the one `round` used, or the newest.
    pub fn policy_commit(&self, run: &str, round: Option<u32>) -> Option<String> {
        self.rev(&match round {
            None => format!("refs/heads/dream/{run}/policy^{{commit}}"),
            Some(n) => format!("refs/tags/dream/{run}/policy-r{n:02}^{{commit}}"),
        })
    }

    /// How a policy version was chosen (`dream.json`), with the policy itself
    /// (`policy.json`) as its "policy". Versions written before the two were
    /// separate files hold both in `policy.json`.
    pub fn policy_doc(&self, run: &str, round: Option<u32>) -> Option<Value> {
        let commit = self.policy_commit(run, round)?;
        let file = |name: &str| self.git(&["show", &format!("{commit}:{name}")]).ok().and_then(|s| serde_json::from_str::<Value>(&s).ok());
        let spec = file(POLICY_FILE)?;
        if spec.get("policy").is_some() {
            return Some(spec);
        }
        let mut doc = file(REPORT_FILE).filter(Value::is_object).unwrap_or_else(|| serde_json::json!({}));
        doc["policy"] = spec;
        Some(doc)
    }

    // ---- scratch worktree ----------------------------------------------------------------
    /// A private worktree (outside the repository) where attempts are materialised,
    /// built and evaluated. Ignored build outputs survive between attempts.
    /// Attempts running at once each use their own (`lane` 0, 1, ...).
    pub fn worktree(&self, run: &str, lane: usize) -> Result<PathBuf> {
        let path = self.worktree_path(run, lane);
        self.worktree_at(&path, || self.base(run))?;
        Ok(path)
    }

    /// The policy version `commit` of `run`, checked out where its program can
    /// be started: one more scratch worktree, `wt/<run>.policy@live`.
    pub fn policy_checkout(&self, run: &str, commit: &str) -> Result<PathBuf> {
        let path = self.worktrees.join(format!("{}@live", policy_run(run)));
        self.worktree_at(&path, || Ok(commit.to_string()))?;
        self.materialize(&path, &self.tree_of(commit)?)?;
        Ok(path)
    }

    /// Make sure `path` is a (detached) worktree of this repository; a new one starts at `commit`.
    fn worktree_at(&self, path: &Path, commit: impl FnOnce() -> Result<String>) -> Result<()> {
        let listed = self.git(&["worktree", "list", "--porcelain"])?;
        let want = format!("worktree {}", path.display());
        if !listed.lines().any(|l| l == want) {
            if path.exists() {
                std::fs::remove_dir_all(path)?;
            }
            self.git(&["worktree", "prune"])?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            self.git(&["worktree", "add", "-q", "--detach", &path.to_string_lossy(), &commit()?])?;
        }
        Ok(())
    }

    /// `wt/<run>` for lane 0, `wt/<run>@<lane>` for the others ('@' cannot occur in a run name).
    fn worktree_path(&self, run: &str, lane: usize) -> PathBuf {
        self.worktrees.join(if lane == 0 { run.to_string() } else { format!("{run}@{lane}") })
    }

    /// Make worktree `wt` (index + files) exactly `tree`; keep ignored files.
    pub fn materialize(&self, wt: &Path, tree: &str) -> Result<()> {
        self.run(&["read-tree", "-u", "--reset", tree], None, Some(wt), &[])?;
        self.run(&["clean", "-fdq"], None, Some(wt), &[])?;
        Ok(())
    }

    /// Tree of everything in worktree `wt` (respecting .gitignore).
    pub fn snapshot(&self, wt: &Path) -> Result<String> {
        self.run(&["add", "-A"], None, Some(wt), &[])?;
        self.run(&["write-tree"], None, Some(wt), &[])
    }

    // ---- maintenance -------------------------------------------------------------------------
    /// Attempts are written with plumbing (hash-object, commit-tree, update-ref),
    /// which never triggers git's housekeeping, so every version of a file would
    /// stay a full loose object. Ask git to do what it does after a commit:
    /// pack once its own thresholds are reached (`gc.auto`, `maintenance.*`).
    pub fn maintain(&self) -> Result<()> {
        self.git(&["maintenance", "run", "--auto", "--quiet"])?;
        Ok(())
    }

    /// Delete every ref and scratch worktree of `run`, and of the run holding its candidate policies.
    pub fn delete_run(&self, run: &str) -> Result<usize> {
        check_run(run)?;
        let nested = if is_policy_run(run) { 0 } else { self.delete_run(&policy_run(run))? };
        let mut refs = vec![];
        for pre in [format!("refs/dream/{run}/"), format!("refs/heads/dream/{run}/"), format!("refs/tags/dream/{run}/")] {
            refs.extend(self.refnames(&pre)?);
        }
        if !refs.is_empty() {
            let input: String = refs.iter().map(|r| format!("delete {r}\n")).collect();
            self.git_in(&["update-ref", "--stdin"], &input)?;
        }
        let mut paths = vec![self.worktree_path(run, 0)];
        if let Ok(dir) = std::fs::read_dir(&self.worktrees) {
            let lanes = format!("{run}@");
            paths.extend(dir.flatten().filter(|e| e.file_name().to_string_lossy().starts_with(&lanes)).map(|e| e.path()));
        }
        for path in paths.iter().filter(|p| p.exists()) {
            let _ = self.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
            let _ = std::fs::remove_dir_all(path);
        }
        self.git(&["worktree", "prune"])?;
        Ok(refs.len() + nested)
    }
}
