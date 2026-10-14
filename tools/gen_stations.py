#!/usr/bin/env python3
"""Generate SEISMO's bundled station list from the FDSN station web service.

Writes assets/stations.csv (relative to the repo root): the open stations of the
Global Seismographic Network (FDSN networks IU and II), thinned so that no two
kept stations are closer than --min-sep degrees of arc. The first lines of the
file are '#' comments recording the query and the date it ran.

Coordinates are the service's, unchanged. Standard library only; no ObsPy.

Usage:
  python tools/gen_stations.py              # default thinning
  python tools/gen_stations.py --min-sep 8
"""

import argparse
import csv
import datetime as dt
import io
import math
import sys
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "assets" / "stations.csv"

SERVICE = "https://service.earthscope.org/fdsnws/station/1/query"
NETWORKS = "IU,II"


def great_circle_deg(lat1, lon1, lat2, lon2):
    """Same convention as sim::great_circle_deg: spherical, atan2 form."""
    p1, p2 = math.radians(lat1), math.radians(lat2)
    dl = math.radians(lon2 - lon1)
    y = math.hypot(
        math.cos(p2) * math.sin(dl),
        math.cos(p1) * math.sin(p2) - math.sin(p1) * math.cos(p2) * math.cos(dl),
    )
    x = math.sin(p1) * math.sin(p2) + math.cos(p1) * math.cos(p2) * math.cos(dl)
    return math.degrees(math.atan2(y, x))


def fetch(open_after):
    url = (
        f"{SERVICE}?network={NETWORKS}&level=station&format=text"
        f"&endafter={open_after}&nodata=404"
    )
    with urllib.request.urlopen(url, timeout=60) as r:
        text = r.read().decode("utf-8")
    return url, text


def parse(text):
    """Rows of the FDSN text format, one per station (the latest epoch wins)."""
    rows = {}
    for line in text.splitlines():
        if not line.strip() or line.startswith("#"):
            continue
        net, sta, lat, lon, elev, name, start, _end = line.split("|")
        key = (net, sta)
        if key not in rows or start > rows[key]["start"]:
            rows[key] = {
                "network": net,
                "station": sta,
                "lat": float(lat),
                "lon": float(lon),
                "elev": float(elev),
                "name": name.strip(),
                "start": start,
            }
    return list(rows.values())


def thin(rows, min_sep):
    """Greedy, deterministic: IU before II, then by station code; drop a station closer
    than `min_sep` degrees to one already kept."""
    kept = []
    for r in sorted(rows, key=lambda r: (r["network"] != "IU", r["station"])):
        if all(
            great_circle_deg(r["lat"], r["lon"], k["lat"], k["lon"]) >= min_sep
            for k in kept
        ):
            kept.append(r)
    return sorted(kept, key=lambda r: (r["network"], r["station"]))


def main():
    ap = argparse.ArgumentParser()
    ap.add_argument("--min-sep", type=float, default=12.0, help="degrees of arc")
    args = ap.parse_args()

    today = dt.datetime.now(dt.timezone.utc).date()
    url, text = fetch(today.isoformat())
    rows = parse(text)
    kept = thin(rows, args.min_sep)

    out = io.StringIO()
    out.write("# Global Seismographic Network (FDSN networks IU and II), open stations.\n")
    out.write(f"# Source: {url}\n")
    out.write(f"# Retrieved {today.isoformat()} UTC by tools/gen_stations.py; ")
    out.write(f"{len(rows)} stations, thinned to {len(kept)} at >= {args.min_sep:g} deg.\n")
    w = csv.writer(out, lineterminator="\n")
    w.writerow(["network", "station", "lat", "lon", "elevation_m", "name"])
    for r in kept:
        w.writerow([r["network"], r["station"], r["lat"], r["lon"], r["elev"], r["name"]])
    OUT.write_text(out.getvalue(), encoding="utf-8", newline="\n")
    print(f"{len(rows)} stations, kept {len(kept)} -> {OUT}", file=sys.stderr)


if __name__ == "__main__":
    main()
