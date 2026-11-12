"""Router overhead: the same requests sent straight to a local mock of the Messages API and through
the router (cache misses), plus repeated requests (cache hits). Writes results/latency.json."""
import json, pathlib, statistics as st, subprocess, threading, time, urllib.request
from http.server import BaseHTTPRequestHandler, ThreadingHTTPServer

ROOT = pathlib.Path(__file__).resolve().parents[1]


class Mock(BaseHTTPRequestHandler):
    def do_POST(self):
        body = json.loads(self.rfile.read(int(self.headers["content-length"])))
        out = json.dumps({"id": "m", "type": "message", "role": "assistant", "model": body["model"], "content": [{"type": "text", "text": "ok"}],
                          "usage": {"input_tokens": 200, "output_tokens": 50}}).encode()
        self.send_response(200); self.send_header("content-type", "application/json"); self.send_header("content-length", str(len(out))); self.end_headers(); self.wfile.write(out)

    def log_message(self, *a):
        pass


srv = ThreadingHTTPServer(("127.0.0.1", 0), Mock)
threading.Thread(target=srv.serve_forever, daemon=True).start()
up = f"http://127.0.0.1:{srv.server_port}"
proc = subprocess.Popen([str(ROOT / "target/release/llm-router"), "serve", "--listen", "127.0.0.1:18787", "--upstream", up], stderr=subprocess.DEVNULL)
time.sleep(0.8)


def post(base, i, model="auto"):
    data = json.dumps({"model": model, "max_tokens": 50, "messages": [{"role": "user", "content": f"Request {i}: explain how OFDM handles multipath fading."}]}).encode()
    t = time.perf_counter()
    with urllib.request.urlopen(urllib.request.Request(base + "/v1/messages", data=data, headers={"content-type": "application/json"})) as r:
        r.read()
    return (time.perf_counter() - t) * 1000


N = 400
for i in range(30):
    post(up, i, "claude-sonnet-5-5"); post("http://127.0.0.1:18787", 10_000 + i)
direct = [post(up, i, "claude-sonnet-5-5") for i in range(N)]
routed = [post("http://127.0.0.1:18787", i) for i in range(N)]
hits = [post("http://127.0.0.1:18787", 7) for _ in range(N)]
proc.terminate(); srv.shutdown()
q = lambda v, p: sorted(v)[int(p * (len(v) - 1))]
res = {k: dict(p50=q(v, 0.5), p90=q(v, 0.9), p99=q(v, 0.99), mean=st.mean(v)) for k, v in [("direct", direct), ("routed_miss", routed), ("routed_cache_hit", hits)]}
res["overhead_p50_ms"] = res["routed_miss"]["p50"] - res["direct"]["p50"]
res["samples"] = {"direct": direct, "routed_miss": routed, "routed_cache_hit": hits}
json.dump(res, open(ROOT / "results" / "latency.json", "w"))
print({k: {a: round(b, 3) for a, b in v.items()} if isinstance(v, dict) and "p50" in v else v for k, v in res.items() if k != "samples"})
