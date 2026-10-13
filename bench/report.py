"""Turns results/<orm>.r<round>.json into markdown: a summary, then one table per scenario."""

import json
import statistics
import sys
from pathlib import Path

# results/<orm>.r<round>.json: timings of all rounds are pooled, memory figures
# are the median of the rounds.
runs = {}
for path in sorted(Path(sys.argv[1]).glob("*.json")):
    for scenario, data in json.loads(path.read_text()).items():
        run = runs.setdefault(path.name.split(".")[0], {}).setdefault(scenario, {"ns": [], "heap": [], "rss": []})
        run["ns"] += data["ns"]
        run["heap"].append(data["heap_peak"])
        run["rss"].append(data["rss_peak"])

orms = list(runs)
scenarios = list(next(iter(runs.values())))


def stats(orm, scenario):
    run = runs[orm][scenario]
    ns = sorted(run["ns"])
    pct = lambda q: ns[min(len(ns) - 1, int(q * len(ns)))]
    return {
        "n": len(ns),
        "mean": statistics.fmean(ns),
        "p50": pct(0.5),
        "p95": pct(0.95),
        "p99": pct(0.99),
        "heap": statistics.median(run["heap"]),
        "rss": statistics.median(run["rss"]),
    }


def mb(b):
    return f"{b / 2**20:,.1f}"


def time(ns):
    return f"{ns / 1e3:,.0f} µs" if ns < 1e6 else f"{ns / 1e6:,.1f} ms"


table = {(o, s): stats(o, s) for o in orms for s in scenarios}

out = ["## Summary: mean time per operation (fastest in bold)", ""]
out += ["| Scenario | " + " | ".join(orms) + " |", "|---|" + "---:|" * len(orms)]
for s in scenarios:
    best = min(table[o, s]["mean"] for o in orms)
    cells = [(f"**{time(table[o, s]['mean'])}**" if table[o, s]["mean"] == best else time(table[o, s]["mean"])) for o in orms]
    out.append(f"| {s} | " + " | ".join(cells) + " |")

out += ["", "## Summary: memory", "", "Heap peak: most memory in use above the start of the scenario, per allocator (Rust: global allocator; PHP: `memory_get_peak_usage()`). It misses what C client libraries take with `malloc`: libmysqlclient under diesel on MySQL, and Oracle's OCI under both Rust ORMs on Oracle. Peak RSS: whole process, highest during the scenario.", ""]
out += ["| Scenario | " + " | ".join(f"{o} heap MB" for o in orms) + " | " + " | ".join(f"{o} RSS MB" for o in orms) + " |", "|---|" + "---:|" * (2 * len(orms))]
for s in scenarios:
    out.append(f"| {s} | " + " | ".join(mb(table[o, s]["heap"]) for o in orms) + " | " + " | ".join(mb(table[o, s]["rss"]) for o in orms) + " |")

out += ["", "## Per scenario", ""]
for s in scenarios:
    unit, div = ("ms", 1e6) if min(table[o, s]["mean"] for o in orms) >= 1e6 else ("µs", 1e3)
    best = min(table[o, s]["mean"] for o in orms)
    out += [f"### {s}", "", f"| ORM | runs | mean {unit} | p50 {unit} | p95 {unit} | p99 {unit} | ops/s | vs fastest | heap peak MB | peak RSS MB |", "|---|" + "---:|" * 9]
    for o in sorted(orms, key=lambda o: table[o, s]["mean"]):
        t = table[o, s]
        f = lambda ns: f"{ns / div:,.1f}"
        out.append(f"| {o} | {t['n']} | {f(t['mean'])} | {f(t['p50'])} | {f(t['p95'])} | {f(t['p99'])} | {1e9 / t['mean']:,.1f} | {t['mean'] / best:.2f}x | {mb(t['heap'])} | {mb(t['rss'])} |")
    out.append("")
print("\n".join(out))
