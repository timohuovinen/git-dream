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
//! refs/heads/dream/<run>/policy      branch: one commit per round with the policy it used
//! refs/tags/dream/<run>/policy-rNN   tag of each policy version
//! refs/notes/dream                   evaluator output per attempt (git notes)
//! ```
//!
//! An attempt commit's parent is the attempt it continued; a round's root commit
//! has the previous round's promoted attempt as parent, so all rounds of a run
//! form one DAG rooted at `base`. Outcomes are commit-message trailers
//! (Dream-Score, Dream-Status, Dream-Design, ...), so `git log` alone rebuilds
//! every tree.

use crate::tree::{Goal, Node, Tree, NOOP, OK};
use anyhow::{anyhow, bail, Context, Result};
use serde_json::Value;
use std::collections::HashMap;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub const REFSPECS: [&str; 4] = ["refs/dream/*:refs/dream/*", "refs/heads/dream/*:refs/heads/dream/*", "refs/tags/dream/*:refs/tags/dream/*", "refs/notes/dream:refs/notes/dream"];

pub struct Memory {
    pub cwd: PathBuf,
    pub top: PathBuf,
    pub common: PathBuf,
    /// `.git/dream/`: scratch worktrees and temporary files.
    pub state: PathBuf,
    env: Vec<(String, String)>,
}

pub fn check_run(run: &str) -> Result<()> {
    let ok = !run.is_empty() && run.chars().next().unwrap().is_ascii_alphanumeric() && run.chars().all(|c| c.is_ascii_alphanumeric() || "._-".contains(c));
    if !ok {
        bail!("bad run name {run:?} (letters, digits, . _ -)");
    }
    Ok(())
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
        let mut m = Memory { cwd: cwd.into(), top: PathBuf::new(), common: PathBuf::new(), state: PathBuf::new(), env: vec![("GIT_TERMINAL_PROMPT".into(), "0".into())] };
        m.top = PathBuf::from(m.git(&["rev-parse", "--show-toplevel"]).context("not inside a git repository")?);
        m.common = PathBuf::from(m.git(&["rev-parse", "--path-format=absolute", "--git-common-dir"])?);
        m.state = m.common.join("dream");
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
        let idx = self.state.join(format!("index-{}", std::process::id()));
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

    pub fn commit(&self, tree: &str, parent: Option<&str>, message: &str) -> Result<String> {
        let mut args = vec!["commit-tree", tree, "-F", "-"];
        if let Some(p) = parent {
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

    pub fn runs(&self) -> Result<Vec<String>> {
        let mut v: Vec<String> = self.refnames("refs/dream/")?.iter().filter_map(|r| r.strip_prefix("refs/dream/")?.strip_suffix("/base").map(String::from)).filter(|r| !r.contains('/')).collect();
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
        // How the evaluation failed (measured, not a diagnosis: the evidence is in the note).
        if n.status != OK && n.status != NOOP {
            if let Some(f) = note.and_then(crate::agents::failure) {
                trailers.push(("Failure", f));
            }
        }
        let tr: Vec<String> = trailers.iter().map(|(k, v)| format!("Dream-{k}: {v}")).collect();
        let msg = format!("{subject}\n\n{}\n\n{}\n", body.join("\n"), tr.join("\n"));
        let sha = self.commit(tree, Some(parent_sha), &msg)?;
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
        let log = self.git_in(&["log", "--no-walk=unsorted", "--stdin", "--format=%H%x1f%B%x1e"], &(shas.join("\n") + "\n"))?;
        let mut nodes = vec![];
        let mut meta: HashMap<String, String> = HashMap::new();
        for rec in log.split('\x1e') {
            let rec = rec.trim();
            if rec.is_empty() {
                continue;
            }
            let (sha, body) = rec.split_once('\x1f').ok_or_else(|| anyhow!("bad log record"))?;
            let tr: HashMap<String, String> = body.lines().filter_map(|l| l.strip_prefix("Dream-")?.split_once(':')).map(|(k, v)| (k.to_string(), v.trim().to_string())).collect();
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
            nodes.push(n);
            meta = tr;
        }
        nodes.sort_by_key(|n| n.id);
        if nodes.iter().enumerate().any(|(i, n)| n.id != i) {
            bail!("round {round} of run {run:?} has missing attempts");
        }
        let mut t = Tree::new(run, round, meta.get("Budget").and_then(|b| b.parse().ok()).unwrap_or(0), Goal::parse(meta.get("Goal").map_or("min", |s| s)), meta.get("Policy").map_or("", |s| s));
        for i in 0..nodes.len() {
            if let Some(p) = nodes[i].parent {
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
    pub fn commit_policy(&self, run: &str, round: u32, doc: &Value, files: &[(String, String)]) -> Result<String> {
        let branch = format!("refs/heads/dream/{run}/policy");
        let parent = self.rev(&branch);
        let mut all = vec![("policy.json".to_string(), serde_json::to_string_pretty(doc)? + "\n")];
        all.extend(files.iter().cloned());
        let tree = self.tree_with_files(None, &all)?;
        let repr = doc["repr"].as_str().unwrap_or("");
        let why = doc["why"].as_str().unwrap_or("");
        let msg = format!("policy({run} r{round:02}): {repr}\n\n{why}\n\nDream-Run: {run}\nDream-Round: {round}\nDream-Policy: {repr}\n");
        let sha = self.commit(&tree, parent.as_deref(), &msg)?;
        self.set_ref(&branch, &sha)?;
        self.set_ref(&format!("refs/tags/dream/{run}/policy-r{round:02}"), &sha)?;
        Ok(sha)
    }

    pub fn policy_doc(&self, run: &str, round: Option<u32>) -> Option<Value> {
        let r = match round {
            None => format!("refs/heads/dream/{run}/policy"),
            Some(n) => format!("refs/tags/dream/{run}/policy-r{n:02}"),
        };
        self.git(&["show", &format!("{r}:policy.json")]).ok().and_then(|s| serde_json::from_str(&s).ok())
    }

    // ---- scratch worktree ----------------------------------------------------------------
    /// A private worktree (inside .git/dream/) where attempts are materialised,
    /// built and evaluated. Ignored build outputs survive between attempts.
    pub fn worktree(&self, run: &str) -> Result<PathBuf> {
        let path = self.state.join("wt").join(run);
        let listed = self.git(&["worktree", "list", "--porcelain"])?;
        let want = format!("worktree {}", path.display());
        if !listed.lines().any(|l| l == want) {
            if path.exists() {
                std::fs::remove_dir_all(&path)?;
            }
            self.git(&["worktree", "prune"])?;
            std::fs::create_dir_all(path.parent().unwrap())?;
            let base = self.base(run)?;
            self.git(&["worktree", "add", "-q", "--detach", &path.to_string_lossy(), &base])?;
        }
        Ok(path)
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
    /// Move loose objects into one new pack (deltas within it) and delete them.
    /// Plumbing never triggers auto-gc, so without this every attempt keeps a
    /// full compressed copy of each file it changed.
    pub fn repack(&self) -> Result<()> {
        self.git(&["repack", "-d", "-q"])?;
        Ok(())
    }

    /// Full `git gc`: merges the per-round packs (deltas across rounds).
    pub fn gc(&self) -> Result<()> {
        self.git(&["gc", "-q"])?;
        Ok(())
    }

    pub fn delete_run(&self, run: &str) -> Result<usize> {
        check_run(run)?;
        let mut refs = vec![];
        for pre in [format!("refs/dream/{run}/"), format!("refs/heads/dream/{run}/"), format!("refs/tags/dream/{run}/")] {
            refs.extend(self.refnames(&pre)?);
        }
        if !refs.is_empty() {
            let input: String = refs.iter().map(|r| format!("delete {r}\n")).collect();
            self.git_in(&["update-ref", "--stdin"], &input)?;
        }
        let path = self.state.join("wt").join(run);
        if path.exists() {
            let _ = self.git(&["worktree", "remove", "--force", &path.to_string_lossy()]);
            let _ = std::fs::remove_dir_all(&path);
        }
        self.git(&["worktree", "prune"])?;
        Ok(refs.len())
    }
}
