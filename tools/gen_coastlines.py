#!/usr/bin/env python3
"""Generate SEISMO's bundled coastlines from Natural Earth.

Writes assets/coastlines.txt (relative to the repo root): the Natural Earth 1:10m
coastline, simplified with Douglas-Peucker in plain degrees of latitude and
longitude (the map is equirectangular, so that is distance on screen), with
islands smaller than --min-size degrees dropped. The result must stay under
50,000 points. The first lines of the file are '#' comments recording the
source, its version and checksum, the settings and the date it ran.

Natural Earth data is in the public domain. Standard library only.

Usage:
  python tools/gen_coastlines.py                  # download and write
  python tools/gen_coastlines.py --zip FILE.zip   # use an already downloaded zip
  python tools/gen_coastlines.py --tolerance 0.05 --min-size 0.2
"""

import argparse
import datetime as dt
import hashlib
import io
import struct
import sys
import urllib.request
import zipfile
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "assets" / "coastlines.txt"

SOURCE = "https://naciscdn.org/naturalearth/10m/physical/ne_10m_coastline.zip"
STEM = "ne_10m_coastline"
MAX_POINTS = 50_000


def read_polylines(shp):
    """Every part of every PolyLine record of a shapefile, as lists of (lon, lat)."""
    file_code, = struct.unpack(">i", shp[0:4])
    if file_code != 9994:
        sys.exit(f"not a shapefile (file code {file_code})")
    shape_type, = struct.unpack("<i", shp[32:36])
    if shape_type != 3:
        sys.exit(f"expected PolyLine shapes (3), found {shape_type}")
    lines = []
    pos = 100
    while pos < len(shp):
        _num, words = struct.unpack(">ii", shp[pos:pos + 8])
        body = shp[pos + 8:pos + 8 + 2 * words]
        pos += 8 + 2 * words
        kind, = struct.unpack("<i", body[0:4])
        if kind == 0:
            continue
        if kind != 3:
            sys.exit(f"unexpected shape type {kind}")
        n_parts, n_points = struct.unpack("<ii", body[36:44])
        parts = struct.unpack(f"<{n_parts}i", body[44:44 + 4 * n_parts])
        at = 44 + 4 * n_parts
        xy = struct.unpack(f"<{2 * n_points}d", body[at:at + 16 * n_points])
        points = list(zip(xy[0::2], xy[1::2]))
        bounds = list(parts) + [n_points]
        for a, b in zip(bounds, bounds[1:]):
            lines.append(points[a:b])
    return lines


def simplify(points, tol):
    """Douglas-Peucker; keeps both ends. Iterative, so long coasts do not hit the recursion limit."""
    if len(points) < 3:
        return list(points)
    keep = [False] * len(points)
    keep[0] = keep[-1] = True
    stack = [(0, len(points) - 1)]
    tol2 = tol * tol
    while stack:
        i, j = stack.pop()
        (x0, y0), (x1, y1) = points[i], points[j]
        dx, dy = x1 - x0, y1 - y0
        seg2 = dx * dx + dy * dy
        worst, at = -1.0, -1
        for k in range(i + 1, j):
            px, py = points[k]
            if seg2 == 0.0:
                d2 = (px - x0) ** 2 + (py - y0) ** 2
            else:
                cross = dx * (py - y0) - dy * (px - x0)
                d2 = cross * cross / seg2
            if d2 > worst:
                worst, at = d2, k
        if worst > tol2:
            keep[at] = True
            stack.append((i, at))
            stack.append((at, j))
    return [p for p, k in zip(points, keep) if k]


def quantise(points):
    """Round to 0.001 deg (about 100 m) and drop repeated points."""
    out = []
    for lon, lat in points:
        q = (round(lon, 3), round(lat, 3))
        if not out or out[-1] != q:
            out.append(q)
    return out


def size_deg(points):
    lons = [p[0] for p in points]
    lats = [p[1] for p in points]
    return max(max(lons) - min(lons), max(lats) - min(lats))


def fmt(v):
    s = f"{v:.3f}".rstrip("0").rstrip(".")
    return "0" if s in ("-0", "") else s


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--zip", type=Path, help="a downloaded copy of the Natural Earth zip")
    ap.add_argument("--tolerance", type=float, default=0.05,
                    help="Douglas-Peucker tolerance, degrees (default 0.05)")
    ap.add_argument("--min-size", type=float, default=0.15,
                    help="drop lines whose extent is below this, degrees (default 0.15)")
    ap.add_argument("--out", type=Path, default=OUT)
    args = ap.parse_args()

    if args.zip:
        raw = args.zip.read_bytes()
    else:
        with urllib.request.urlopen(SOURCE, timeout=120) as r:
            raw = r.read()
    sha = hashlib.sha256(raw).hexdigest()
    with zipfile.ZipFile(io.BytesIO(raw)) as z:
        shp = z.read(f"{STEM}.shp")
        version = z.read(f"{STEM}.VERSION.txt").decode("utf-8").strip()

    source = read_polylines(shp)
    n_in = sum(len(l) for l in source)
    kept = []
    for line in source:
        if len(line) < 2 or size_deg(line) < args.min_size:
            continue
        line = quantise(simplify(line, args.tolerance))
        if len(line) >= 2:
            kept.append(line)
    n_out = sum(len(l) for l in kept)
    print(f"{len(source)} lines, {n_in} points -> {len(kept)} lines, {n_out} points")
    if n_out >= MAX_POINTS:
        sys.exit(f"{n_out} points is not under {MAX_POINTS}; raise --tolerance or --min-size")

    today = dt.datetime.now(dt.timezone.utc).date().isoformat()
    with open(args.out, "w", encoding="utf-8", newline="\n") as f:
        f.write("# Coastlines for the SEISMO map: Natural Earth 1:10m coastline (public domain).\n")
        f.write(f"# Source: {SOURCE}, version {version}, SHA-256 {sha}\n")
        f.write(f"# Retrieved {today} UTC by tools/gen_coastlines.py: Douglas-Peucker tolerance "
                f"{args.tolerance} deg, lines under {args.min_size} deg dropped; "
                f"{len(source)} lines, {n_in} points -> {len(kept)} lines, {n_out} points.\n")
        f.write("# One line per polyline: space-separated lat,lon pairs in degrees.\n")
        for line in kept:
            f.write(" ".join(f"{fmt(lat)},{fmt(lon)}" for lon, lat in line))
            f.write("\n")
    print(f"wrote {args.out}")


if __name__ == "__main__":
    main()
