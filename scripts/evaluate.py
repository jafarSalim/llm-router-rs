"""Tunes the two thresholds on half of the labeled prompts and reports results on the other half.

Calibration = prompts with an odd number in their id, test = even. The objective rewards agreement
with the label and punishes sending a hard prompt to Haiku three times as much as any other miss.
Costs assume typical answer lengths per label (easy 150, medium 600, hard 1,500 output tokens)."""
import json, subprocess, sys, pathlib, itertools
ROOT = pathlib.Path(__file__).resolve().parents[1]
BIN = ROOT / "target" / "release" / "llm-router"
T = ["haiku", "sonnet", "opus"]
PRICE = {"haiku": (0.10, 0.50), "sonnet": (2.0, 10.0), "opus": (4.0, 20.0)}  # USD / M tokens, 2026-11-09
OUT_TOK = {"haiku": 150, "sonnet": 600, "opus": 1500}

data = [json.loads(l) for l in open(ROOT / "data" / "prompts.jsonl")]
routes = {}
for line in subprocess.run([str(BIN), "route", str(ROOT / "data" / "prompts.jsonl")], capture_output=True, text=True, check=True).stdout.splitlines():
    d = json.loads(line); routes[d["id"]] = d
for d in data:
    d["score"] = routes[d["id"]]["score"]; d["in_tok"] = max(1, round(len(d["prompt"]) / 4))
cal = [d for d in data if int(d["id"][1:]) % 2 == 1]
test = [d for d in data if int(d["id"][1:]) % 2 == 0]


def tier(s, a, b):
    return "opus" if s >= b else "sonnet" if s >= a else "haiku"


def objective(ds, a, b):
    ok = sum(tier(d["score"], a, b) == d["label"] for d in ds)
    severe = sum(d["label"] == "opus" and tier(d["score"], a, b) == "haiku" for d in ds)
    return ok - 3 * severe


grid = [x / 10 for x in range(-10, 61)]
best = max(((a, b) for a in grid for b in grid if b > a), key=lambda ab: (objective(cal, *ab), -ab[1]))
a, b = best


def report(ds):
    conf = {l: {t: 0 for t in T} for l in T}
    cost = {"router": 0.0, "always_opus": 0.0, "always_sonnet": 0.0, "oracle": 0.0}
    for d in ds:
        t = tier(d["score"], a, b); conf[d["label"]][t] += 1
        o = OUT_TOK[d["label"]]
        c = lambda m: (d["in_tok"] * PRICE[m][0] + o * PRICE[m][1]) / 1e6
        cost["router"] += c(t); cost["always_opus"] += c("opus"); cost["always_sonnet"] += c("sonnet"); cost["oracle"] += c(d["label"])
    n = len(ds)
    return dict(n=n, accuracy=sum(conf[t][t] for t in T) / n, confusion=conf,
                under_routed=sum(conf["opus"]["haiku"] + conf["opus"]["sonnet"] + conf["sonnet"]["haiku"] for _ in [0]) / n,
                over_routed=(conf["haiku"]["sonnet"] + conf["haiku"]["opus"] + conf["sonnet"]["opus"]) / n,
                severe=conf["opus"]["haiku"], cost_per_1000=({k: 1000 * v / n for k, v in cost.items()}))


MIXES = {"Chat app": (0.70, 0.25, 0.05), "Coding assistant": (0.30, 0.50, 0.20), "Research assistant": (0.10, 0.40, 0.50)}


def mix_costs(ds):
    """Cost per 1,000 requests for a traffic mix, from per-label averages on the given prompts."""
    out = {}
    for name, w in MIXES.items():
        row = {}
        for strat in ("router", "always_opus", "always_sonnet", "oracle"):
            tot = 0.0
            for lab, share in zip(T, w):
                sub = [d for d in ds if d["label"] == lab]
                r = report(sub)
                tot += share * r["cost_per_1000"][strat]
            row[strat] = tot
        out[name] = dict(mix=w, **row)
    return out


res = dict(thresholds=dict(sonnet=a, opus=b), calibration=report(cal), test=report(test), all=report(data),
           mixes=mix_costs(test), scores={l: sorted(d["score"] for d in data if d["label"] == l) for l in T})
json.dump(res, open(ROOT / "results" / "eval.json", "w"), indent=1)
print("thresholds", a, b)
for k, v in res["mixes"].items():
    print(k, {x: (round(y, 2) if isinstance(y, float) else y) for x, y in v.items()})
for k in ("calibration", "test", "all"):
    r = res[k]; print(k, f"acc {r['accuracy']:.3f} under {r['under_routed']:.3f} over {r['over_routed']:.3f} severe {r['severe']}", r["confusion"], {x: round(y, 2) for x, y in r["cost_per_1000"].items()})
