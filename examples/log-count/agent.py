"""Replay adapter for candidates authored in the implementation session.
This is deliberately not presented as an autonomous LLM invocation.
"""
import os
from pathlib import Path

current = Path("count.rs").read_text()
if "HashMap" in current:
    message = "No further candidate supplied in this session"
else:
    control = os.environ["DREAM_K"] == "0"
    name = "control" if control else "optimized"
    Path("count.rs").write_text(Path(f"candidates/{name}.rs").read_text())
    message = (
        "Negative control: overwrite duplicate keys (must fail correctness)"
        if control else
        "Aggregate borrowed keys in a hash map before sorting\n\n"
        "The baseline scans all distinct keys per input row. Replace the linear "
        "lookup with HashMap::entry, then allocate the sorted result once per key. "
        "Keep parsing and duplicate aggregation semantics unchanged."
    )
Path(os.environ["DREAM_MESSAGE_FILE"]).write_text(message + "\n")
