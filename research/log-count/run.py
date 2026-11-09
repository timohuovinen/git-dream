#!/usr/bin/env python3
"""Reproduce the session-authored candidate experiment in an isolated repository."""
import argparse
from pathlib import Path
import shutil
import subprocess
import tempfile

parser = argparse.ArgumentParser()
parser.add_argument("--output", type=Path, required=True)
args = parser.parse_args()
root = Path(__file__).resolve().parents[2]
output = args.output.resolve()
output.mkdir(parents=True, exist_ok=False)
binary = root / "target/debug/git-dream"
if not binary.exists():
    binary = binary.with_suffix(".exe")
with tempfile.TemporaryDirectory(prefix="git-dream-log-count-") as temp:
    repo = Path(temp)
    shutil.copytree(root / "examples/log-count", repo, dirs_exist_ok=True)
    def run(*argv):
        return subprocess.run(argv, cwd=repo, check=True, capture_output=True, text=True).stdout
    run("git", "init", "-q")
    run("git", "config", "user.name", "timohuovinen")
    run("git", "config", "user.email", "timohuovinen@users.noreply.github.com")
    run("git", "add", "-A")
    run("git", "commit", "-qm", "Baseline log aggregation fixture and independent evaluator")
    (output / "run.log").write_text(run(str(binary), "run", "--no-dream"))
    (output / "environment.txt").write_text(run("rustc", "--version") + run("python3", "--version") + run("git", "--version"))
    run("git", "bundle", "create", str(output / "history.bundle"), "--all")
    (output / "refs.txt").write_text(run("git", "show-ref"))
print(output)
