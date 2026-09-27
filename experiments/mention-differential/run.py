#!/usr/bin/env python3
"""Run without Cargo dependencies; compile the production module directly."""
import argparse
import hashlib
import itertools
import json
import os
from pathlib import Path
import random
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent


def corpus():
    # Explicit oracles anchor the comparison; agreement alone is not correctness.
    expected = {
        "hello": "hello",
        "hi <@U100>!": "hi @Alice!",
        "<@U123|alice>": "@Bob",
        "<@UNKNOWN|alice>": "@alice",
        "<@EMPTY|alice>": "@alice",
        "<@EMPTY|>": "@EMPTY",
        "<@UNKNOWN>": "@UNKNOWN",
        "<@>": "<@>",
        "<@U123<@U100>": "<@U123<@U100>",
        "<@bad id> <@U100>": "<@bad id> @Alice",
        "<@U100": "<@U100",
        "こんにちは😀<@U100>\n<@U123>": "こんにちは😀@Alice\n@Bob",
    }
    # Unicode White_Space property, including ASCII vertical tab.
    whitespace = list(range(9, 14)) + [32, 0x85, 0xA0, 0x1680]
    whitespace += list(range(0x2000, 0x200B)) + [0x2028, 0x2029, 0x202F, 0x205F, 0x3000]
    for cp in whitespace:
        text = f"<@U{chr(cp)}100> <@U123>"
        expected[text] = f"<@U{chr(cp)}100> @Bob"
    cases = list(expected)
    atoms = ["<@", ">", "|", "U100", "U123", " ", "\n", "日", "😀"]
    for size in range(6):
        cases.extend("".join(parts) for parts in itertools.product(atoms, repeat=size))
    rng = random.Random(20260927)
    atoms += ["<", "@", "EMPTY", "alice", "\t", "\v", "\u00a0", "\u3000", "\u200b", "\x00"]
    for _ in range(15000):
        cases.append("".join(rng.choices(atoms, k=rng.randrange(1, 35))))
    for cp in whitespace:
        for uid in ["U100", "UNKNOWN", "EMPTY"]:
            for label in ["", "|alice", "|", "|a|b"]:
                cases.append(f"前<@{uid}{chr(cp)}bad{label}>後<@U123>")
    return list(dict.fromkeys(cases)), expected


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--go", default="go")
    parser.add_argument("--report", type=Path, required=True)
    args = parser.parse_args()
    source = ROOT / "src/slack/mention.rs"
    rust = '''
use std::io::{self, BufRead, Write};
mod slack {
    mod mention;
    pub fn substitute(s: &str) -> String {
        let cache = [("U123".into(), "Bob".into()), ("U100".into(), "Alice".into()),
                     ("EMPTY".into(), String::new())].into_iter().collect();
        mention::substitute_mentions(s, &cache)
    }
}
fn main() {
    let mut out = io::BufWriter::new(io::stdout().lock());
    for line in io::stdin().lock().lines() {
        let line = line.unwrap();
        let bytes: Vec<u8> = (0..line.len()).step_by(2)
            .map(|i| u8::from_str_radix(&line[i..i+2], 16).unwrap()).collect();
        let text = String::from_utf8(bytes).unwrap();
        for byte in slack::substitute(&text).bytes() { write!(out, "{byte:02x}").unwrap(); }
        writeln!(out).unwrap();
    }
}
'''
    cases, expected = corpus()
    wire = "".join(text.encode().hex() + "\n" for text in cases)
    with tempfile.TemporaryDirectory(prefix="scout-mention-") as directory:
        tmp = Path(directory)
        adapter = tmp / "adapter.rs"
        adapter.write_text(rust)
        (tmp / "slack").mkdir()
        (tmp / "slack/mention.rs").symlink_to(source)
        (tmp / "slack/mention").symlink_to(source.with_suffix(""), target_is_directory=True)
        env = dict(os.environ, GOCACHE=str(tmp / "gocache"), GOTOOLCHAIN="local")
        subprocess.run([args.go, "build", "-o", str(tmp / "reference"), str(HERE / "reference.go")], env=env, check=True)
        subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", str(adapter), "-o", str(tmp / "production")], check=True)
        subprocess.run(["rustc", "--edition=2024", "-A", "dead_code", "--test", str(adapter), "-o", str(tmp / "tests")], check=True)
        test_run = subprocess.run([str(tmp / "tests")], text=True, capture_output=True)
        print(test_run.stdout)
        test_run.check_returncode()
        outputs = []
        for binary in ["reference", "production"]:
            result = subprocess.run([str(tmp / binary)], input=wire, text=True, capture_output=True, check=True)
            lines = result.stdout.splitlines()
            assert len(lines) == len(cases), (binary, len(lines), len(cases))
            outputs.append([bytes.fromhex(line).decode() for line in lines])
    mismatches = []
    oracle_failures = []
    for text, go, rust in zip(cases, *outputs):
        if go != rust:
            mismatches.append({"input": text, "go": go, "rust": rust})
        if text in expected:
            for name, actual in [("go", go), ("rust", rust)]:
                if actual != expected[text]:
                    oracle_failures.append({"implementation": name, "input": text, "expected": expected[text], "actual": actual})
    report = {
        "seed": 20260927,
        "cases": len(cases),
        "explicit_oracles": len(expected),
        "go_version": subprocess.check_output([args.go, "version"], text=True).strip(),
        "rust_version": subprocess.check_output(["rustc", "--version"], text=True).strip(),
        "production_sha256": hashlib.sha256(source.read_bytes()).hexdigest(),
        "reference_sha256": hashlib.sha256((HERE / "reference.go").read_bytes()).hexdigest(),
        "module_tests": test_run.stdout,
        "mismatch_count": len(mismatches),
        "oracle_failures": oracle_failures,
        "mismatches": mismatches,
    }
    args.report.write_text(json.dumps(report, ensure_ascii=True, indent=2) + "\n")
    print(f"{len(cases)} cases; {len(mismatches)} differences; {len(oracle_failures)} oracle failures")
    print(f"Report: {args.report}")
    return int(bool(mismatches or oracle_failures))


if __name__ == "__main__":
    raise SystemExit(main())
