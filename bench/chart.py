"""Draws the README chart (assets/bench.svg) from the summary tables in RESULTS-<db>.md."""

import re
from pathlib import Path

HERE = Path(__file__).parent
SCENARIOS = ["find_by_pk", "insert_single", "update_single", "fetch_10000_rows", "eager_1000_users_10000_posts", "bulk_insert_10000"]
ORMS = [("rustique", "rustique", "#E2672A"), ("diesel", "diesel", "#2DD4BF"), ("eloquent-opcache-jit", "Eloquent (OPcache JIT)", "#8B949E")]
FONT = "system-ui,-apple-system,'Segoe UI',Roboto,Helvetica,Arial,sans-serif"
LABEL_W, BAR_W, BAR_H, GAP, ROW_GAP = 250, 520, 12, 4, 18


def summary(db):
    """Mean time per scenario and ORM in µs, plus the original cell text."""
    lines = (HERE / f"RESULTS-{db}.md").read_text().split("## Summary: memory")[0].splitlines()
    rows = [[c.strip().strip("*") for c in line.strip("|").split("|")] for line in lines if line.startswith("|")]
    header, body = rows[0], rows[2:]
    out = {}
    for row in body:
        for orm, cell in zip(header[1:], row[1:]):
            value, unit = re.match(r"([\d,.]+) (ms|µs)", cell).groups()
            out[row[0], orm] = (float(value.replace(",", "")) * (1000 if unit == "ms" else 1), cell)
    return out


def text(x, y, s, size=14, fill="#8B949E", weight="400", anchor="start"):
    return f'<text x="{x}" y="{y}" font-family="{FONT}" font-size="{size}" font-weight="{weight}" fill="{fill}" text-anchor="{anchor}">{s}</text>'


svg, y = [], 30
for x, (_, name, color) in zip((0, 130, 240), ORMS):
    svg += [f'<rect x="{x}" y="{y - 11}" width="12" height="12" rx="2" fill="{color}"/>', text(x + 18, y, name)]
y += 20
for db, title in (("mysql", "MySQL 8.4"), ("oracle", "Oracle XE 21c")):
    data = summary(db)
    y += 30
    svg.append(text(0, y, f"{title}: mean time per operation, shorter is faster", 18, "#C2410C", "700"))
    y += 18
    for s in SCENARIOS:
        slowest = max(data[s, orm][0] for orm, _, _ in ORMS)
        svg.append(text(LABEL_W - 12, y + (3 * BAR_H + 2 * GAP) / 2 + 5, s, anchor="end"))
        for orm, _, color in ORMS:
            us, cell = data[s, orm]
            w = max(2, BAR_W * us / slowest)
            svg += [f'<rect x="{LABEL_W}" y="{y}" width="{w:.1f}" height="{BAR_H}" rx="2" fill="{color}"/>', text(f"{LABEL_W + w + 6:.1f}", y + BAR_H - 1, cell, 12)]
            y += BAR_H + GAP
        y += ROW_GAP

width = LABEL_W + BAR_W + 90
print(f'<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 {width} {y}" width="{width}" height="{y}" role="img" aria-label="Benchmark: rustique vs diesel vs Eloquent on MySQL and Oracle">')
print("\n".join(svg))
print("</svg>")
