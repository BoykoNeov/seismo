#!/usr/bin/env python3
"""Generate SEISMO's bundled crust model from CRUST1.0.

Writes assets/crust1.bin (relative to the repo root): CRUST1.0 (Laske, Masters, Ma and
Pasyanos 2013), a 1x1 degree global model of the crust, re-encoded compactly and without
loss. Each cell keeps its nine layer boundaries (heights above sea level) and the P and S
speeds of its eight layers (water, ice, three sediment layers, three crystalline crust
layers). Densities and the speeds below the Moho are left out: SEISMO keeps ak135's mantle.
Layers of zero thickness carry no speeds, which lets the cells share a small table of distinct
speed profiles.

File format (all little-endian):
  b"SEISMOCR" magic (8 bytes), u32 format version, u32 header length, UTF-8 JSON header
  (source, citation, axes, layer names, units, profile count), then the profiles
  (count x 8 layers x (u16 vp, u16 vs), in 0.01 km/s), then the cells, north to south and
  west to east (180 x 360 x (u16 profile index, 9 x i16 boundary height in 0.01 km)).

CRUST1.0 is free to download from the authors' page with a request to cite it; no license is
stated. Standard library only.

Usage:
  python tools/gen_crust.py                       # download and write
  python tools/gen_crust.py --tar crust1.0.tar.gz # use an already downloaded archive
"""

import argparse
import datetime as dt
import hashlib
import io
import json
import struct
import sys
import tarfile
import urllib.request
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "assets" / "crust1.bin"

SOURCE = "https://igppweb.ucsd.edu/~gabi/crust1/crust1.0.tar.gz"
PAGE = "https://igppweb.ucsd.edu/~gabi/crust1.html"
CITATION = (
    "Laske, G., Masters, G., Ma, Z. and Pasyanos, M., Update on CRUST1.0 - A 1-degree Global "
    "Model of Earth's Crust, Geophys. Res. Abstracts, 15, Abstract EGU2013-2658, 2013."
)
MAGIC = b"SEISMOCR"
FORMAT_VERSION = 1
N_LAT, N_LON, N_BND, N_LAYER = 180, 360, 9, 8
LAYERS = [
    "water",
    "ice",
    "upper sediments",
    "middle sediments",
    "lower sediments",
    "upper crust",
    "middle crust",
    "lower crust",
]


def hundredths(text):
    """A CRUST1.0 value (two decimals) as an exact integer number of hundredths."""
    x = float(text)
    n = round(x * 100)
    if abs(x * 100 - n) > 1e-6:
        sys.exit(f"value {text} has more than two decimals")
    return n


def read_member(tar, name):
    rows = []
    for line in tar.extractfile(name).read().decode("ascii").splitlines():
        if line.strip():
            rows.append([hundredths(v) for v in line.split()])
    if len(rows) != N_LAT * N_LON or any(len(r) != N_BND for r in rows):
        sys.exit(f"{name}: expected {N_LAT * N_LON} rows of {N_BND} values")
    return rows


def main():
    ap = argparse.ArgumentParser(description=__doc__.split("\n")[0])
    ap.add_argument("--tar", type=Path, help="an already downloaded crust1.0.tar.gz")
    ap.add_argument("--out", type=Path, default=OUT)
    args = ap.parse_args()

    if args.tar:
        raw = args.tar.read_bytes()
    else:
        print(f"downloading {SOURCE}")
        with urllib.request.urlopen(SOURCE) as r:
            raw = r.read()
    sha = hashlib.sha256(raw).hexdigest()
    with tarfile.open(fileobj=io.BytesIO(raw), mode="r:gz") as tar:
        bnds = read_member(tar, "crust1.bnds")
        vp = read_member(tar, "crust1.vp")
        vs = read_member(tar, "crust1.vs")

    profiles = {}
    cells = []
    for b, p, s in zip(bnds, vp, vs):
        if any(b[i] < b[i + 1] for i in range(N_LAYER)):
            sys.exit(f"boundaries not in order: {b}")
        key = tuple(
            (p[i], s[i]) if b[i] > b[i + 1] else (0, 0) for i in range(N_LAYER)
        )
        for i, (pv, sv) in enumerate(key):
            if b[i] > b[i + 1] and (pv <= 0 or (i > 0 and sv <= 0)):
                sys.exit(f"layer {LAYERS[i]} has thickness but no speed: {p} {s}")
        index = profiles.setdefault(key, len(profiles))
        cells.append((index, b))
    if len(profiles) > 0xFFFF:
        sys.exit("too many profiles for u16 indices")

    header = {
        "model": "CRUST1.0",
        "source": SOURCE,
        "page": PAGE,
        "sha256": sha,
        "retrieved": dt.datetime.now(dt.timezone.utc).strftime("%Y-%m-%d"),
        "citation": CITATION,
        "generator": "tools/gen_crust.py",
        "lat": {"start": 89.5, "step": -1.0, "count": N_LAT},
        "lon": {"start": -179.5, "step": 1.0, "count": N_LON},
        "layers": LAYERS,
        "boundary_unit": "0.01 km above sea level; tops of the 8 layers, then the Moho",
        "speed_unit": "0.01 km/s",
        "profiles": len(profiles),
    }
    hjson = json.dumps(header, indent=1).encode("utf-8")
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with open(args.out, "wb") as f:
        f.write(MAGIC)
        f.write(struct.pack("<II", FORMAT_VERSION, len(hjson)))
        f.write(hjson)
        for key in profiles:  # dicts keep insertion order: index order
            for pv, sv in key:
                f.write(struct.pack("<HH", pv, sv))
        for index, b in cells:
            f.write(struct.pack("<H9h", index, *b))
    print(f"wrote {args.out}: {len(profiles)} profiles, {len(cells)} cells, "
          f"{args.out.stat().st_size} bytes, source sha256 {sha}")

    # Anchors for the Rust tests, read straight from the source files.
    for name, lat, lon in [("Tibet", 32.5, 88.5), ("mid-Pacific", 0.5, -150.5),
                           ("Hawaii (HON)", 21.5, -157.5)]:
        row = int(90 - lat)
        col = int(180 + lon)
        k = row * N_LON + col
        print(f"{name} ({lat}, {lon}): bnds {[v / 100 for v in bnds[k]]} "
              f"vp {[v / 100 for v in vp[k][:8]]} vs {[v / 100 for v in vs[k][:8]]}")


if __name__ == "__main__":
    main()
