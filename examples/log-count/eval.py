import os
import subprocess

os.makedirs("build", exist_ok=True)
suffix = ".exe" if os.name == "nt" else ""
for source, output, flags in [("tests.rs", "tests", ["--test"]), ("bench.rs", "bench", ["-O"])]:
    target = f"build/{output}{suffix}"
    subprocess.run(["rustc", "--edition=2021", source, *flags, "-o", target], check=True)
    subprocess.run([os.path.abspath(target)], check=True)
