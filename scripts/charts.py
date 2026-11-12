"""Synthwave-styled SVG charts for the README, drawn from results/eval.json and results/latency.json."""
import json, pathlib, random
ROOT = pathlib.Path(__file__).resolve().parents[1]
E = json.load(open(ROOT / "results/eval.json")); L = json.load(open(ROOT / "results/latency.json"))
BG1, BG2, INK, DIM, GRID = "#1a0b2e", "#2d1150", "#fdf0ff", "#b79ad6", "#4a2a72"
COL = {"haiku": "#00f0ff", "sonnet": "#ff9f1c", "opus": "#ff2a6d", "label": "#c77dff"}
F = 'font-family="Segoe UI, Helvetica, Arial, sans-serif"'
NAME = {"haiku": "Haiku 5.5", "sonnet": "Sonnet 5.5", "opus": "Opus 5.5"}
LAB = {"haiku": "easy", "sonnet": "medium", "opus": "hard"}


def head(w, h, title, sub):
    return f'''<svg xmlns="http://www.w3.org/2000/svg" width="{w}" height="{h}" viewBox="0 0 {w} {h}">
<defs><linearGradient id="bg" x1="0" y1="0" x2="0" y2="1"><stop offset="0" stop-color="{BG1}"/><stop offset="1" stop-color="{BG2}"/></linearGradient></defs>
<rect width="{w}" height="{h}" rx="16" fill="url(#bg)"/>
<text x="32" y="46" {F} font-size="22" font-weight="800" fill="{INK}">{title}</text>
<text x="32" y="70" {F} font-size="13" fill="{DIM}">{sub}</text>'''


def save(name, s):
    (ROOT / "results" / name).write_text(s + "</svg>")


# 1. Sankey: label -> routed tier on the held-out half
conf = E["test"]["confusion"]; n = E["test"]["n"]
w, h = 980, 520; top, bot = 110, 470; x0, x1 = 230, 750; gap = 14
scale = (bot - top - 2 * gap) / n
s = head(w, h, "Where each request went", f"held-out test half: {n} hand-labeled prompts · thresholds tuned on the other half")
ly, ry = {}, {}
y = top
for l in ["haiku", "sonnet", "opus"]:
    hh = sum(conf[l].values()) * scale; ly[l] = [y, y]; s += f'<rect x="{x0 - 16}" y="{y}" width="16" height="{hh}" rx="3" fill="{COL["label"]}"/><text x="{x0 - 26}" y="{y + hh / 2 + 5}" {F} font-size="15" fill="{INK}" text-anchor="end">{LAB[l]} · {sum(conf[l].values())}</text>'; y += hh + gap
y = top
for t in ["haiku", "sonnet", "opus"]:
    hh = sum(conf[l][t] for l in conf) * scale; ry[t] = [y, y]; s += f'<rect x="{x1}" y="{y}" width="16" height="{hh}" rx="3" fill="{COL[t]}"/><text x="{x1 + 28}" y="{y + hh / 2 + 5}" {F} font-size="15" fill="{COL[t]}" font-weight="700">{NAME[t]} · {sum(conf[l][t] for l in conf)}</text>'; y += hh + gap
k = 0
for l in ["haiku", "sonnet", "opus"]:
    for t in ["haiku", "sonnet", "opus"]:
        v = conf[l][t]
        if not v:
            continue
        hh = v * scale; a, b = ly[l][1], ry[t][1]; ly[l][1] += hh; ry[t][1] += hh
        mx = (x0 + x1) / 2
        d = f"M{x0},{a} C{mx},{a} {mx},{b} {x1},{b} L{x1},{b + hh} C{mx},{b + hh} {mx},{a + hh} {x0},{a + hh} Z"
        s += f'<path d="{d}" fill="{COL[t]}" opacity="0.42"><animate attributeName="opacity" values="0;0.42" dur="0.8s" begin="{0.15 * k:.2f}s" fill="freeze"/></path>'
        s += f'<path d="M{x0},{a + hh / 2} C{mx},{a + hh / 2} {mx},{b + hh / 2} {x1},{b + hh / 2}" fill="none" stroke="{COL[t]}" stroke-width="2" stroke-dasharray="4 12" opacity="0.9"><animate attributeName="stroke-dashoffset" from="160" to="0" dur="2.5s" repeatCount="indefinite"/></path>'
        s += f'<text x="{mx}" y="{(a + b) / 2 + hh / 2 + 4}" {F} font-size="12" fill="{INK}" text-anchor="middle">{v}</text>'
        k += 1
save("sankey.svg", s)

# 2. Score strip plot with thresholds
w, h = 980, 400; x0, x1 = 150, 940
lo, hi = -2, 7
sx = lambda v: x0 + (x1 - x0) * (v - lo) / (hi - lo)
s = head(w, h, "Difficulty scores and the two thresholds", "each dot is one prompt · dashed lines: tuned thresholds")
for g in range(lo, hi + 1):
    s += f'<line x1="{sx(g)}" y1="96" x2="{sx(g)}" y2="{h - 40}" stroke="{GRID}"/><text x="{sx(g)}" y="{h - 18}" {F} font-size="12" fill="{DIM}" text-anchor="middle">{g}</text>'
random.seed(1)
for i, l in enumerate(["haiku", "sonnet", "opus"]):
    yc = 140 + i * 80
    s += f'<text x="{x0 - 16}" y="{yc + 5}" {F} font-size="15" fill="{INK}" text-anchor="end">{LAB[l]}</text>'
    for j, v in enumerate(E["scores"][l]):
        s += f'<circle cx="{sx(v):.1f}" cy="{yc + random.uniform(-22, 22):.1f}" r="6" fill="{COL[l]}" opacity="0.85"><animate attributeName="r" values="0;6" dur="0.5s" begin="{0.02 * j:.2f}s" fill="freeze"/></circle>'
for name, v, c, ty in [("Sonnet", E["thresholds"]["sonnet"], COL["sonnet"], 104), ("Opus", E["thresholds"]["opus"], COL["opus"], 122)]:
    s += f'<line x1="{sx(v)}" y1="92" x2="{sx(v)}" y2="{h - 40}" stroke="{c}" stroke-width="2.5" stroke-dasharray="7 5"/><text x="{sx(v) + 6}" y="{ty}" {F} font-size="13" font-weight="700" fill="{c}">≥ {v} → {name}</text>'
save("scores.svg", s)

# 3. Cost per 1,000 requests by traffic mix
w, h = 980, 470; x0, y0, y1 = 90, 400, 110
M = E["mixes"]; strategies = [("always_opus", "Always Opus", "#ff2a6d"), ("router", "Router", "#00f0ff"), ("oracle", "Perfect labels", "#c77dff"), ("always_sonnet", "Always Sonnet", "#ff9f1c")]
vmax = max(v[k] for v in M.values() for k, _, _ in strategies) * 1.15
sy = lambda v: y0 - (y0 - y1) * v / vmax
s = head(w, h, "Cost per 1,000 requests, by kind of traffic (USD)", "per-label costs from the test half, weighted by each mix (easy / medium / hard)")
for g in range(0, int(vmax) + 1, 5):
    s += f'<line x1="{x0}" y1="{sy(g)}" x2="{w - 30}" y2="{sy(g)}" stroke="{GRID}"/><text x="{x0 - 10}" y="{sy(g) + 4}" {F} font-size="12" fill="{DIM}" text-anchor="end">${g}</text>'
gw = (w - 30 - x0) / len(M)
for i, (name, v) in enumerate(M.items()):
    gx = x0 + i * gw + 30; bw = (gw - 60) / 4
    for j, (k, lab, c) in enumerate(strategies):
        bx = gx + j * bw; val = v[k]
        s += f'<rect x="{bx + 3}" y="{sy(val)}" width="{bw - 6}" height="{y0 - sy(val)}" rx="4" fill="{c}"><animate attributeName="height" from="0" to="{y0 - sy(val)}" dur="1s" fill="freeze"/><animate attributeName="y" from="{y0}" to="{sy(val)}" dur="1s" fill="freeze"/></rect>'
        s += f'<text x="{bx + bw / 2}" y="{sy(val) - 6}" {F} font-size="11" fill="{INK}" text-anchor="middle">{val:.2f}</text>'
    mix = "/".join(f"{int(100 * x)}" for x in v["mix"])
    s += f'<text x="{gx + 2 * bw}" y="{y0 + 22}" {F} font-size="14" font-weight="700" fill="{INK}" text-anchor="middle">{name}</text><text x="{gx + 2 * bw}" y="{y0 + 40}" {F} font-size="12" fill="{DIM}" text-anchor="middle">{mix} % easy / medium / hard</text>'
lx = x0
for k, lab, c in strategies:
    s += f'<rect x="{lx}" y="84" width="12" height="12" rx="2" fill="{c}"/><text x="{lx + 18}" y="95" {F} font-size="13" fill="{INK}">{lab}</text>'; lx += 150
save("cost_mix.svg", s)

# 4. Latency
w, h = 980, 330; x0, x1 = 230, 920
vmax = 1.3
sx = lambda v: x0 + (x1 - x0) * v / vmax
s = head(w, h, "What the router adds to each request (ms)", "400 requests each against a local mock of the Messages API · bar = p50 to p99, dot = median")
for g in [0, 0.25, 0.5, 0.75, 1.0, 1.25]:
    s += f'<line x1="{sx(g)}" y1="96" x2="{sx(g)}" y2="{h - 40}" stroke="{GRID}"/><text x="{sx(g)}" y="{h - 18}" {F} font-size="12" fill="{DIM}" text-anchor="middle">{g}</text>'
for i, (k, lab, c) in enumerate([("direct", "Direct to API", "#b79ad6"), ("routed_miss", "Through router", "#00f0ff"), ("routed_cache_hit", "Router cache hit", "#ff9f1c")]):
    y = 130 + i * 55; v = L[k]
    s += f'<text x="{x0 - 16}" y="{y + 5}" {F} font-size="15" fill="{INK}" text-anchor="end">{lab}</text>'
    s += f'<rect x="{sx(v["p50"])}" y="{y - 9}" width="{sx(v["p99"]) - sx(v["p50"])}" height="18" rx="9" fill="{c}" opacity="0.35"/>'
    s += f'<circle cx="{sx(v["p50"])}" cy="{y}" r="9" fill="{c}"/><text x="{sx(v["p99"]) + 10}" y="{y + 5}" {F} font-size="13" fill="{c}">p50 {v["p50"]:.2f} · p99 {v["p99"]:.2f}</text>'
save("latency.svg", s)
print("ok")
