#!/usr/bin/env python3
"""List every Rust source/test file and the additional checks actually applied."""
import json
from pathlib import Path
import re

from run import ROOT, WRAPPERS
from mutations import MUTANTS

def main():
 rows=[]
 for path in sorted([*(ROOT/"src").rglob("*.rs"),*(ROOT/"tests").rglob("*.rs")]):
  name=str(path.relative_to(ROOT))
  text=path.read_text()
  tests=len(re.findall(r"(?m)^\s*#\[(?:tokio::)?test(?:\([^\n]*\))?\]",text))
  diff=name.removeprefix("src/") in WRAPPERS
  mutants=[v[0] for v in MUTANTS if "src/"+v[1]==name]
  rows.append(dict(path=name,lines=len(text.splitlines()),declared_tests=tests,go_comparison=diff,mutations=mutants))
 out=Path(__file__).with_name("inventory.json")
 out.write_text(json.dumps(rows,indent=2)+"\n")
 md=["# Source and test inventory", "", "This inventory covers every Rust file under `src/` and `tests/`. A listed", "Go comparison applies only to the operations in `run.py`, not every function", "in that file. Test counts are declarations, not branch coverage or assertions.", "Files without additional checks rely on the existing full-suite runs; this", "does not imply an independent implementation or mutation audit of every path.", "", "| File | Test declarations | Go operations | Mutation probes |", "|---|---:|---|---|"]
 for r in rows:md.append(f"| `{r['path']}` | {r['declared_tests']} | {'yes' if r['go_comparison'] else '—'} | {', '.join(r['mutations']) or '—'} |")
 Path(__file__).with_name("INVENTORY.md").write_text("\n".join(md)+"\n")
 print(f"{len(rows)} Rust source/test files inventoried")

if __name__=="__main__":main()
