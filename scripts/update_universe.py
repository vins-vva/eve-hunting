"""Regenerates data/universe.tsv (k-space systems and stargates) from the SDE published by Fuzzwork.

    python scripts/update_universe.py

Output format (tabs):
    S  <system_id>  <name>  <region>  <security>
    J  <from_id>  <to_id>          (once per gate pair, from < to)
Only k-space (ids 30000000-30999999) is kept: wormholes have no gates.
"""
import csv
import io
import pathlib
import urllib.request

BASE = "https://www.fuzzwork.co.uk/dump/latest/csv/"
OUT = pathlib.Path(__file__).resolve().parent.parent / "data" / "universe.tsv"


def rows(name):
    req = urllib.request.Request(BASE + name + ".csv", headers={"User-Agent": "eve-scouting"})
    with urllib.request.urlopen(req, timeout=60) as r:
        text = r.read().decode("utf-8-sig")
    return list(csv.DictReader(io.StringIO(text)))


def kspace(i):
    return 30000000 <= int(i) < 31000000


def main():
    regions = {r["regionID"]: r["regionName"] for r in rows("mapRegions")}
    lines = []
    for s in rows("mapSolarSystems"):
        if kspace(s["solarSystemID"]):
            sec = float(s["security"] or 0)
            lines.append(f"S\t{s['solarSystemID']}\t{s['solarSystemName']}\t{regions.get(s['regionID'], '?')}\t{sec:.4f}")
    pairs = set()
    for j in rows("mapSolarSystemJumps"):
        a, b = int(j["fromSolarSystemID"]), int(j["toSolarSystemID"])
        if kspace(a) and kspace(b):
            pairs.add((min(a, b), max(a, b)))
    lines += [f"J\t{a}\t{b}" for a, b in sorted(pairs)]
    OUT.parent.mkdir(exist_ok=True)
    OUT.write_text("\n".join(lines) + "\n", encoding="utf-8", newline="\n")
    print(f"{OUT}: {sum(l[0] == 'S' for l in lines)} systems, {len(pairs)} gates")


if __name__ == "__main__":
    main()
