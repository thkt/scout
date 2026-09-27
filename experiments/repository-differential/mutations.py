#!/usr/bin/env python3
"""Deliberately break isolated source snapshots to challenge existing tests.

This is a representative mutation sample, not an exhaustive mutation score.
Compilation failures and infrastructure errors never count as detected bugs.
"""
import argparse
import json
import os
import re
from pathlib import Path
import shutil
import subprocess
import tempfile

ROOT=Path(__file__).resolve().parents[2]
# id, source, old text, replacement, existing test-name filter
MUTANTS=[
 ("inline_pipe", "markdown.rs", "'|' | '[' | ']' | '(' | ')' =>", "'[' | ']' | '(' | ')' =>", "markdown::"),
 ("yaml_null", "yaml.rs", "'\\0' => {}", "'\\0' => out.push(c),", "yaml::"),
 ("charset_single_byte", "charset.rs", ".contains(&encoding)", ".contains(&encoding) || encoding == encoding_rs::WINDOWS_1252", "charset::"),
 ("retry_boundary", "retry.rs", "s <= MAX_RETRY_AFTER_SECS", "s < MAX_RETRY_AFTER_SECS", "retry::"),
 ("body_boundary", "body_limit.rs", "if body.len() > cap", "if body.len() >= cap", "body_limit::"),
 ("token_priority", "token_source.rs", '["GITHUB_TOKEN", "GH_TOKEN"]', '["GH_TOKEN", "GITHUB_TOKEN"]', "token_source::"),
 ("redacted_trim", "redacted.rs", "let trimmed = s.trim();", "let trimmed = s;", "redacted::"),
 ("sigint_code", "signals.rs", "Self::Sigint => 130,", "Self::Sigint => 129,", "signals::"),
 ("lang_auto", "search/lang.rs", "Lang::Auto => None,", 'Lang::Auto => Some("en"),', "search::lang::"),
 ("research_depth", "search/engine.rs", ".take(depth)", ".take(depth + 1)", "search::engine::"),
 ("research_failure_order", "search/engine.rs", "indexed_failures.sort_by_key(|(idx, _)| *idx);", "indexed_failures.sort_by_key(|(idx, _)| std::cmp::Reverse(*idx));", "search::engine::"),
 ("github_binary", "github/encoding.rs", "bytes.contains(&0)", "bytes.is_empty() && bytes.contains(&0)", "github::encoding::"),
 ("github_range_zero", "github/helpers.rs", "if start == 0 {", "if start == usize::MAX {", "github::helpers::"),
 ("timeout_minimum", "tools/config.rs", "const TIMEOUT_MIN_SECS: u64 = 1;", "const TIMEOUT_MIN_SECS: u64 = 0;", "tools::config::"),
 ("stdin_trim", "tools/params.rs", ".map(|s| s.trim().to_owned())", ".map(|s| s.to_owned())", "tools::"),
 ("slack_empty_channel", "slack/url.rs", "if segments[1].is_empty() {", "if false {", "slack::url::"),
 ("slack_whitespace", "slack/mention.rs", "c.is_whitespace() || c == '<'", "c.is_ascii_whitespace() || c == '<'", "slack::mention::"),
 ("http_not_found", "classify.rs", "404 => Self::new(ErrorCode::NotFound),", "404 => Self::new(ErrorCode::DataError),", "classify"),
 ("retryability", "envelope.rs", "matches!(self, Self::TempFailure | Self::Timeout)", "matches!(self, Self::TempFailure)", "errors::"),
 ("html_paragraph_break", "fetch/converter.rs", 'let content_html = close_self_closed_raw_text_tags(&article.content_html);', 'let mutated_html = article.content_html.replace("<br>", " "); let content_html = close_self_closed_raw_text_tags(&mutated_html);', "fetch::converter::"),
 ("content_type_case", "fetch/download.rs", "let normalized = mime.to_ascii_lowercase();", "let normalized = mime.to_owned();", "fetch::download::content_type_tests::"),
]

def main():
 p=argparse.ArgumentParser();p.add_argument("--report",type=Path,required=True);p.add_argument("--only");args=p.parse_args()
 results=[]
 with tempfile.TemporaryDirectory(prefix="scout-test-mutations-") as directory:
  tmp=Path(directory)
  shutil.copytree(ROOT/"src",tmp/"src")
  for name in ["Cargo.toml","Cargo.lock"]:shutil.copy2(ROOT/name,tmp/name)
  env=dict(os.environ,SCOUT_NETWORK_TESTS="1")
  for name,file,old,new,filter_ in MUTANTS:
   if args.only and name not in args.only.split(","):continue
   path=tmp/"src"/file;base=path.read_text()
   if base.count(old)!=1:raise ValueError((name,"mutation must match exactly once",base.count(old)))
   path.write_text(base.replace(old,new))
   try:
    result=subprocess.run(["cargo","test","--offline","--locked","--manifest-path",str(tmp/"Cargo.toml"),"--target-dir",str(ROOT/"target/repository-mutations"),"--lib",filter_],env=env,text=True,capture_output=True,timeout=600)
    text=result.stdout+result.stderr
    passed=re.search(r"test result: ok\. (\d+) passed",text)
    if result.returncode==0 and passed and int(passed[1])>0: status="survived"
    elif "test result: FAILED" in text:status="detected"
    else:status="invalid_or_infrastructure_failure"
    failed=[line for line in text.splitlines() if line.startswith("test ") and line.endswith("FAILED")]
    summaries=[line for line in text.splitlines() if line.startswith("test result:")]
    record=dict(id=name,source=file,filter=filter_,status=status,failed_tests=failed,summaries=summaries)
    if status=="invalid_or_infrastructure_failure":record["diagnostic"]=text[-5000:]
   except subprocess.TimeoutExpired:
    record=dict(id=name,source=file,status="timeout")
   finally:path.write_text(base)
   results.append(record)
   args.report.write_text(json.dumps(results,indent=2)+"\n")
   print(name,record["status"],flush=True)
 return int(any(r["status"]!="detected" for r in results))

if __name__=="__main__":raise SystemExit(main())
