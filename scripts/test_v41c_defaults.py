"""V41c: Benchmark new Aggressive defaults [0.2-0.8] with rescue [0.8,1.0] when zones > 4.

Compares old defaults [0.2-0.5] vs new defaults [0.2-0.8] + rescue [0.8,1.0] when zones>4.
"""

import json
import os
import urllib.request

ROOT = os.path.abspath(os.path.join(os.path.dirname(__file__), ".."))
OUT_DIR = os.path.join(ROOT, "ai_docs", "tmp", "v41c_defaults")
os.makedirs(OUT_DIR, exist_ok=True)

PORT = os.environ.get("FREECUT_PORT", "8093")
BASE_URL = f"http://127.0.0.1:{PORT}"
SEEDS = int(os.environ.get("FREECUT_SEEDS", "30"))

with open(os.path.join(ROOT, "tests", "fixtures", "multisheet_varied_4sheets.json")) as f:
    base_req = json.load(f)

base_req["params"]["time_limit_ms"] = 8000
base_req["params"]["restarts"] = 5
base_req["params"]["layout_mode"] = "guillotine"
base_req["params"]["include_svg"] = False
base_req["params"]["retry_strategy"] = "smart"
base_req["params"]["max_retry_attempts"] = 3
base_req["params"]["partition"] = {"enabled": True, "sheet_budget_ms": 15000}

CONFIGS = [
    {
        "tag": "old_defaults_4penalties",
        "pp": {"enabled": True, "preset": "aggressive", "zone_penalties": [0.2, 0.3, 0.4, 0.5]},
    },
    {
        "tag": "new_defaults_6plus_rescue",
        "pp": {"enabled": True, "preset": "aggressive"},
    },
]


def call_optimize(req):
    data = json.dumps(req).encode("utf-8")
    r = urllib.request.Request(BASE_URL + "/v1/optimize", data=data, headers={"Content-Type": "application/json"}, method="POST")
    try:
        with urllib.request.urlopen(r, timeout=600) as resp:
            return json.loads(resp.read().decode("utf-8"))
    except urllib.error.HTTPError as e:
        body = e.read().decode("utf-8", errors="replace")[:200]
        print(f"  HTTP {e.code}: {body}", flush=True)
        return None


def response_metrics(resp):
    if not resp or "solutions" not in resp:
        return None
    utils = []
    per_sheet_zones = []
    for sol in resp["solutions"]:
        trim = sol.get("trim_mm", {})
        w = sol["width_mm"] - trim.get("left", 0) - trim.get("right", 0)
        h = sol["height_mm"] - trim.get("top", 0) - trim.get("bottom", 0)
        used = sum(p["width_mm"] * p["height_mm"] for p in sol.get("placements", []))
        util = used / (w * h) * 100 if w > 0 and h > 0 else 0
        utils.append(util)
        cell = 10
        nx, ny = int(w // cell), int(h // cell)
        if nx <= 0 or ny <= 0:
            per_sheet_zones.append(0)
            continue
        occ = [[False] * nx for _ in range(ny)]
        for p in sol.get("placements", []):
            px = p["x_mm"] - trim.get("left", 0)
            py = p["y_mm"] - trim.get("top", 0)
            for j in range(max(0, int(py // cell)), min(ny, int((py + p["height_mm"] - 0.01) // cell) + 1)):
                for i in range(max(0, int(px // cell)), min(nx, int((px + p["width_mm"] - 0.01) // cell) + 1)):
                    occ[j][i] = True
        seen = [[False] * nx for _ in range(ny)]
        zones = 0
        for j in range(ny):
            for i in range(nx):
                if occ[j][i] or seen[j][i]:
                    continue
                stack = [(i, j)]
                seen[j][i] = True
                cells = 0
                while stack:
                    ci, cj = stack.pop()
                    cells += 1
                    for di, dj in [(-1, 0), (1, 0), (0, -1), (0, 1)]:
                        ni, nj = ci + di, cj + dj
                        if 0 <= ni < nx and 0 <= nj < ny and not occ[nj][ni] and not seen[nj][ni]:
                            seen[nj][ni] = True
                            stack.append((ni, nj))
                if cells >= 50:
                    zones += 1
        per_sheet_zones.append(zones)
    utils.sort(reverse=True)
    lead = sum(utils[:-1]) / (len(utils) - 1) if len(utils) > 1 else (utils[0] if utils else 0)
    return {
        "sheets": len(resp["solutions"]),
        "lead": round(lead, 2),
        "total_zones": sum(per_sheet_zones),
        "per_sheet_zones": per_sheet_zones,
        "max_per_sheet": max(per_sheet_zones) if per_sheet_zones else 0,
    }


def main():
    all_results = {}
    for cfg in CONFIGS:
        tag = cfg["tag"]
        print(f"\n=== {tag} ({SEEDS} seeds) ===", flush=True)
        rows = []
        for seed in range(1, SEEDS + 1):
            req = json.loads(json.dumps(base_req))
            req["params"]["seed"] = seed
            req["params"]["profile_pool"] = cfg["pp"]
            resp = call_optimize(req)
            m = response_metrics(resp)
            if m is None:
                print(f"  s{seed}: FAIL", flush=True)
                continue
            rows.append(m)
            sheets_zones = "/".join(str(z) for z in m["per_sheet_zones"])
            print(f"  s{seed}: {m['sheets']}s lead={m['lead']:.2f}% zones={m['total_zones']} per_sheet=[{sheets_zones}]", flush=True)

        all_results[tag] = rows
        if rows:
            n = len(rows)
            avg_lead = sum(r["lead"] for r in rows) / n
            avg_zones = sum(r["total_zones"] for r in rows) / n
            avg_max = sum(r["max_per_sheet"] for r in rows) / n
            le4 = sum(1 for r in rows if r["total_zones"] <= 4)
            le5 = sum(1 for r in rows if r["total_zones"] <= 5)
            print(f"\n  SUMMARY({tag}): lead={avg_lead:.2f}% zones={avg_zones:.1f} max/sheet={avg_max:.2f} le4={le4}/{n} le5={le5}/{n}", flush=True)

    with open(os.path.join(OUT_DIR, "v41c_defaults_results.json"), "w", encoding="utf-8") as f:
        json.dump(all_results, f, indent=2, ensure_ascii=False)

    if len(CONFIGS) >= 2:
        b = all_results.get(CONFIGS[0]["tag"], [])
        e = all_results.get(CONFIGS[1]["tag"], [])
        n = min(len(b), len(e))
        if n > 0:
            better_z = sum(1 for i in range(n) if e[i]["total_zones"] < b[i]["total_zones"])
            same_z = sum(1 for i in range(n) if e[i]["total_zones"] == b[i]["total_zones"])
            worse_z = sum(1 for i in range(n) if e[i]["total_zones"] > b[i]["total_zones"])
            lead_delta = sum(e[i]["lead"] - b[i]["lead"] for i in range(n)) / n
            print(f"\n=== COMPARISON ===", flush=True)
            print(f"zones: better={better_z} same={same_z} worse={worse_z}/{n} avg_lead_delta={lead_delta:+.2f}pp", flush=True)

    print(f"\nSaved to {OUT_DIR}", flush=True)


if __name__ == "__main__":
    main()