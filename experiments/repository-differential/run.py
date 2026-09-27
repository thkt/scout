#!/usr/bin/env python3
"""Compare Go contract models with wrappers around a temporary Rust source copy."""
import argparse
import base64
import collections
import hashlib
import ipaddress
import json
import os
from pathlib import Path
import random
import re
import shutil
import subprocess
import tempfile

HERE = Path(__file__).resolve().parent
ROOT = HERE.parent.parent

# Only test-only access adapters are appended. Production source is unchanged.
WRAPPERS = {
    "markdown.rs": '''match op {
        "inline" => j!(escape_md_inline(a)), "heading" => j!(sanitize_heading(a)),
        "truncate" => j!(truncate_with_note(a, n as usize)), "fence" => j!(fence_delimiter(a)),
        _ => panic!("unknown op") }''',
    "yaml.rs": '''match op { "yaml" => j!(escape_yaml(a)), "markers" => j!(neutralize_yaml_markers(a)),
        "yaml_field" => { let mut s=String::new(); write_yaml_str(&mut s,"title",a);j!(s) }, _=>panic!("unknown op") }''',
    "github/helpers.rs": '''match op { "encode_path"=>j!(encode_path(a)), "repo"=>j!(parse_repo(a).ok()),
        "ref"=>j!(validate_ref(a).is_ok()), "path"=>j!(validate_path(a).is_ok()), "range"=>j!(parse_line_range(a).ok()), _=>panic!("unknown op") }''',
    "github/encoding.rs": '''j!(decode_base64(a).ok().map(|bytes| bytes.iter().map(|b| format!("{b:02x}")).collect::<String>()))''',
    "github/format.rs": '''j!(format_size(n))''',
    "fetch/download.rs": '''j!(check_content_type(a).is_ok())''',
    "fetch/ssrf.rs": '''j!(is_private_ip(a.parse().unwrap()))''',
    "retry.rs": '''if op=="retry_cap" { j!(retry_after_within_cap(Some(n))) } else {
        struct At(u64);impl crate::clock::Clock for At {fn now_secs(&self)->u64{self.0}}
        let mut headers=HeaderMap::new();
        match reqwest::header::HeaderValue::from_str(a) {Ok(v)=>{headers.insert(RETRY_AFTER,v);j!(parse_retry_after(&headers,&At(n)))}, Err(_)=>j!(null)}
    }''',
    "classify.rs": '''let k=Classification::from_http_status(n as u16).kind;j!([k,k.exit_code(),k.is_retryable()])''',
    "tools/config.rs": '''let key=v["b"].as_str().unwrap();
        let result=RuntimeConfig::from_env_with(|k| if k==key {Ok(a.to_owned())}else{Err(std::env::VarError::NotPresent)});
        j!(result.ok().map(|c| match key {"SCOUT_MAX_RETRIES"=>u64::from(c.max_retries),"SCOUT_FETCH_TIMEOUT_SECS"=>c.fetch_timeout.as_secs(),
        "SCOUT_RESEARCH_TIMEOUT_SECS"=>c.research_timeout.as_secs(),"SCOUT_SLACK_TIMEOUT_SECS"=>c.slack_timeout.as_secs(),_=>c.github_timeout.as_secs()}))''',
    "slack/mention.rs": '''let cache=[("U123".into(),"Bob".into()),("U100".into(),"Alice".into()),("EMPTY".into(),String::new())].into_iter().collect();j!(substitute_mentions(a,&cache))''',
}
DISPATCH = {
    "markdown": ["inline", "heading", "truncate", "fence"],
    "yaml": ["yaml", "yaml_field", "markers"],
    "github::helpers": ["encode_path", "repo", "ref", "path", "range"],
    "github::encoding": ["base64"], "github::format": ["size"],
    "fetch::download": ["content_type"], "fetch::ssrf": ["ip"],
    "retry": ["retry", "retry_cap"], "classify": ["status"],
    "tools::config": ["config"], "slack::mention": ["mention"],
}


def corpus():
    rng = random.Random(20260927)
    cases = []
    def add(op, a="", b="", n=0):
        cases.append(dict(op=op, a=a, b=b, n=n))
    strings = ["", "\r\n", "---", "...", "--- x", "---\t", "\n---\n...\n", "😀日", "\\[x](https://x)"]
    strings += [chr(i) for i in range(128)]
    alphabet = list("ab []()|\\`~#-./;=\"\r\n\t\x00") + ["日", "😀", "\u00a0", "\u3000", "<@U100>", "<@bad id>"]
    strings += ["".join(rng.choices(alphabet, k=rng.randrange(1, 100))) for _ in range(2500)]
    for s in strings:
        for op in ["inline", "heading", "fence", "yaml", "yaml_field", "markers", "encode_path", "path", "mention"]:
            add(op, s)
        for n in [0, 1, 2, 3, 10, len(s.encode()), len(s.encode())//2]:
            add("truncate", s, n=n)
    for s in ["a"*449, "日"*150, "😀"*113, "\\"*451, "\""*451]:
        add("yaml_field", s)
    for i in range(10000):
        add("size", n=rng.randrange(100_000_001))
    for n in [0,1,1023,1024,1025,1048575,1048576,1048577,2**53-1,2**64-1]:
        add("size", n=n)
    names = ["a", "repo", "repo.git", ".", "..", "...", "-a", "_a", "x?y", "日", "", "a b", "a%2Fz"]
    for owner in names:
        for repo in names:
            for prefix in ["", "https://github.com/", "http://github.com/"]:
                for suffix in ["", "/", "/tree/main", ".git"]:
                    add("repo", prefix+owner+"/"+repo+suffix)
    refs = ["", "main", "HEAD", "@", "@{-1}", "a/b", "/main", "main/", "a//b", ".a", "a/.b", "a.lock/b", "a.lock", "foo.", "refs/heads/main"]
    refs += ["a"+chr(i)+"b" for i in range(128)]
    refs += ["".join(rng.choices(list("ab./@{~^:?*[\\_- "), k=rng.randrange(1,16))) for _ in range(1500)]
    for s in refs:
        add("ref", s)
    nums = ["", "0", "1", "10", "600", "601", "+1", "-1", " 1 ", "\u30001\u3000", "1.0", "01", "18446744073709551615", "18446744073709551616"]
    for a in nums:
        add("range", a)
        for b in nums:
            add("range", a+"-"+b)
        for key in ["SCOUT_MAX_RETRIES", "SCOUT_FETCH_TIMEOUT_SECS", "SCOUT_RESEARCH_TIMEOUT_SECS", "SCOUT_SLACK_TIMEOUT_SECS", "SCOUT_GITHUB_TIMEOUT_SECS"]:
            add("config", a, key)
    for size in range(80):
        s = base64.b64encode(rng.randbytes(size)).decode()
        for altered in [s, s.rstrip("="), s+"=", s+"!", "\u3000"+s[:4]+"\r\n"+s[4:]]:
            add("base64", altered)
    for s in ["Zh==", "Zg==", "Zm9=", "Zm8=", "Z===", "====", "aGVsbG8"]:
        add("base64", s)
    for mime in ["text/html", "text/plain", "application/xml", "application/rss+xml", "application/atom+xml", "application/xhtml+xml", "image/svg+xml", "application/json", "application/pdf", ""]:
        for s in [mime, mime.upper(), mime.title(), " "+mime+" "]:
            for suffix in ["", "; charset=UTF-8"]:
                add("content_type", s+suffix)
    for prefix in ["0.0.0.0/8","10.0.0.0/8","127.0.0.0/8","169.254.0.0/16","172.16.0.0/12","192.168.0.0/16","100.64.0.0/10","255.255.255.255/32","::/128","::1/128","fe80::/10","fc00::/7"]:
        network=ipaddress.ip_network(prefix)
        for n in [int(network.network_address)-1,int(network.network_address),int(network.network_address)+1,int(network.broadcast_address),int(network.broadcast_address)+1]:
            if 0<=n<2**network.max_prefixlen:
                addr=ipaddress.ip_address(n) if network.version==4 else ipaddress.IPv6Address(n)
                add("ip",str(addr))
                if network.version==4:
                    add("ip","::ffff:"+str(addr));add("ip","::"+str(addr))
    for _ in range(5000):
        add("ip",str(ipaddress.IPv4Address(rng.getrandbits(32))))
        add("ip",str(ipaddress.IPv6Address(rng.getrandbits(128))))
    for s in nums+["Sun, 06 Nov 1994 08:49:37 GMT", "Sunday, 06-Nov-94 08:49:37 GMT", "Sun Nov  6 08:49:37 1994", "Thu, 01 Jan 1970 00:00:00 GMT", "junk"]:
        for now in [0, 784111777, 1800000000]:
            add("retry",s,n=now)
    for n in range(1000):
        add("status",n=n);add("retry_cap",n=n)
    # Preserve insertion order while removing duplicate parameter combinations.
    return list({json.dumps(v,sort_keys=True):v for v in cases}.values())


def prepare(snapshot):
    shutil.copytree(ROOT/"src",snapshot/"src")
    source_hashes={str(path.relative_to(snapshot)):hashlib.sha256(path.read_bytes()).hexdigest() for path in sorted((snapshot/"src").rglob("*.rs"))}
    for name in ["Cargo.toml","Cargo.lock"]:
        shutil.copy2(ROOT/name,snapshot/name)
    for path in (snapshot/"src").rglob("*.rs"):
        original=path.read_text()
        exposed=re.sub(r"(?m)^mod (\w+);",r"pub(crate) mod \1;",original)
        path.write_text(exposed)
    for name,body in WRAPPERS.items():
        with (snapshot/"src"/name).open("a") as f:
            f.write('\n#[cfg(test)]\n#[allow(unused_variables)]\npub(crate) fn differential(v: &serde_json::Value) -> serde_json::Value {\nuse serde_json::json as j;\nlet op=v["op"].as_str().unwrap();let a=v["a"].as_str().unwrap();let n=v["n"].as_u64().unwrap();\n'+body+'\n}\n')
    arms="\n".join(" | ".join(json.dumps(op) for op in ops)+f" => {module}::differential(&v)," for module,ops in DISPATCH.items())
    with (snapshot/"src/lib.rs").open("a") as f:
        f.write('''
#[cfg(test)]
#[test]
fn repository_differential_driver() {
    use std::io::{BufRead, Write};
    let input=std::fs::File::open(std::env::var("SCOUT_DIFF_INPUT").unwrap()).unwrap();
    let output=std::fs::File::create(std::env::var("SCOUT_DIFF_OUTPUT").unwrap()).unwrap();
    let mut out=std::io::BufWriter::new(output);
    for line in std::io::BufReader::new(input).lines() {
        let v:serde_json::Value=serde_json::from_str(&line.unwrap()).unwrap();
        let result=match v["op"].as_str().unwrap() {
''' + arms + '''
            _=>panic!("unknown op"),
        };
        writeln!(out,"{}",serde_json::to_string(&result).unwrap()).unwrap();
    }
}
''')
    return source_hashes


def main():
    p=argparse.ArgumentParser()
    p.add_argument("--go",default="go")
    p.add_argument("--report",type=Path,required=True)
    args=p.parse_args()
    cases=corpus()
    with tempfile.TemporaryDirectory(prefix="scout-repository-diff-") as directory:
        tmp=Path(directory)
        snapshot=tmp/"snapshot";snapshot.mkdir()
        source_hashes=prepare(snapshot)
        wire="".join(json.dumps(v,ensure_ascii=True)+"\n" for v in cases)
        (tmp/"input.jsonl").write_text(wire)
        env=dict(os.environ,SCOUT_DIFF_INPUT=str(tmp/"input.jsonl"),SCOUT_DIFF_OUTPUT=str(tmp/"rust.jsonl"),GOCACHE=str(tmp/"gocache"),GOTOOLCHAIN="local")
        subprocess.run([args.go,"build","-o",str(tmp/"reference"),str(HERE/"reference.go")],env=env,check=True)
        ref=subprocess.run([str(tmp/"reference")],input=wire,text=True,capture_output=True,check=True)
        build=subprocess.run(["cargo","test","--offline","--locked","--manifest-path",str(snapshot/"Cargo.toml"),"--target-dir",str(ROOT/"target/repository-differential"),"--lib","repository_differential_driver","--","--exact"],env=env,text=True,capture_output=True)
        if build.returncode:
            print(build.stdout);print(build.stderr);build.check_returncode()
        expected=[json.loads(s) for s in ref.stdout.splitlines()]
        actual=[json.loads(s) for s in (tmp/"rust.jsonl").read_text().splitlines()]
        assert len(actual)==len(expected)==len(cases)
    counts=collections.Counter(v["op"] for v in cases)
    differences=collections.defaultdict(list)
    for v,go,rust in zip(cases,expected,actual):
        if go!=rust:
            differences[v["op"]].append({"input":v,"go":go,"rust":rust})
    report={"seed":20260927,"cases":len(cases),"cases_by_operation":dict(counts),"mismatch_counts":{k:len(v) for k,v in differences.items()},"examples":{k:v[:20] for k,v in differences.items()},"differences":dict(differences),
        "corpus_sha256":hashlib.sha256(wire.encode()).hexdigest(),"go_reference_sha256":hashlib.sha256((HERE/"reference.go").read_bytes()).hexdigest(),
        "source_sha256":source_hashes,
        "go_version":subprocess.check_output([args.go,"version"],text=True).strip(),"rust_version":subprocess.check_output(["rustc","--version"],text=True).strip()}
    args.report.write_text(json.dumps(report,ensure_ascii=True,indent=2)+"\n")
    print(f"{len(cases)} cases across {len(counts)} operations; mismatches: {report['mismatch_counts']}")
    return int(bool(differences))


if __name__=="__main__":
    raise SystemExit(main())
