#!/usr/bin/env python3
"""Independent process-level fixtures, with no credentials or remote requests."""
import argparse
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer
import json
from pathlib import Path
import subprocess
import threading

ROOT=Path(__file__).resolve().parents[2]

def main():
 p=argparse.ArgumentParser();p.add_argument("--report",type=Path,required=True);args=p.parse_args()
 scenario={};requests=[]
 class Handler(BaseHTTPRequestHandler):
  def log_message(self,*args):pass
  def do_GET(self):
   requests.append(self.path)
   self.send_response(scenario["status"])
   self.send_header("Content-Type",scenario["type"])
   body=scenario["body"].encode()
   self.send_header("Content-Length",str(len(body)))
   self.end_headers();self.wfile.write(body)
 server=ThreadingHTTPServer(("127.0.0.1",0),Handler)
 thread=threading.Thread(target=server.serve_forever,daemon=True);thread.start()
 # Intentionally do not inherit service tokens, proxy settings, or user GH config.
 env={"PATH":"/usr/bin:/bin","LANG":"en_US.UTF-8","SCOUT_MAX_RETRIES":"0","SCOUT_FETCH_TIMEOUT_SECS":"5","HTTP_PROXY":f"http://127.0.0.1:{server.server_port}"}
 results=[]
 def run(name,argv,code,contains=None,error=None,network=False,forbidden=None):
  requests.clear()
  result=subprocess.run([str(ROOT/"target/debug/scout"),"--json",*argv],env=env,stdin=subprocess.DEVNULL,text=True,capture_output=True,timeout=15)
  parsed=[]
  for line in (result.stdout if result.returncode==0 else result.stderr).splitlines():
   try:parsed.append(json.loads(line))
   except json.JSONDecodeError:pass
  ok=result.returncode==code and len(parsed)==1
  if error:ok=ok and parsed[0].get("error",{}).get("code")==error
  if contains:ok=ok and contains in parsed[0].get("data",{}).get("markdown","")
  if forbidden:ok=ok and forbidden not in parsed[0].get("data",{}).get("markdown","")
  if network:ok=ok and len(requests)==1
  else:ok=ok and not requests
  record=dict(name=name,passed=ok,exit=result.returncode,expected_exit=code,requests=len(requests))
  if not ok:record.update(stdout=result.stdout,stderr=result.stderr,expected_contains=contains)
  results.append(record)
 try:
  scenario.update(status=200,type="text/html",body="<p>fixture body</p>")
  for mime in ["text/html","Text/HTML","TEXT/PLAIN","APPLICATION/XML","Application/Rss+Xml","Application/Xhtml+Xml"]:
   scenario["type"]=mime
   run("media_type:"+mime,["fetch","http://fixture.example/","--raw"],0,contains="fixture body",network=True)
  scenario["type"]="text/html"
  for status,exit_,code in [(400,65,"DATA_ERROR"),(401,64,"USAGE_ERROR"),(403,64,"USAGE_ERROR"),(404,66,"NOT_FOUND"),(408,75,"TEMP_FAILURE"),(422,65,"DATA_ERROR"),(429,75,"TEMP_FAILURE"),(500,75,"TEMP_FAILURE"),(503,75,"TEMP_FAILURE"),(599,75,"TEMP_FAILURE")]:
   scenario["status"]=status
   run("http_status:"+str(status),["fetch","http://fixture.example/"],exit_,error=code,network=True)
  scenario["status"]=200
  for name,html,expected in [("paragraph","<p>alpha\n beta</p>","alpha beta"),("hard_break","<p>alpha<br>beta</p>","alpha  \nbeta"),("pre","<pre>alpha\nbeta</pre>","alpha\nbeta"),("unicode","<p>日本語😀</p>","日本語😀"),("title","<title>A &amp; B</title><p>body</p>",'title: "A & B"'),("suppression","<script>HIDDEN</script><p>"+"visible paragraph. "*80+"</p>","visible")]:
   scenario["body"]=html
   run("html:"+name,["fetch","http://fixture.example/","--raw"],0,contains=expected,network=True,forbidden="HIDDEN" if name=="suppression" else None)
  for command in ["search","research","fetch","repo-tree","repo-read","repo-overview"]:
   run("missing_input:"+command,[command],64,error="USAGE_ERROR")
  for url in ["http://127.0.0.1/","http://[::1]/","file:///etc/passwd"]:
   run("blocked_url:"+url,["fetch",url],65,error="DATA_ERROR")
 finally:
  server.shutdown();server.server_close();thread.join()
 args.report.write_text(json.dumps(results,ensure_ascii=False,indent=2)+"\n")
 print(f"{sum(r['passed'] for r in results)}/{len(results)} process fixtures passed")
 return int(not all(r["passed"] for r in results))

if __name__=="__main__":raise SystemExit(main())
