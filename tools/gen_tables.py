#!/usr/bin/env python3
"""Generate SEISMO's ak135 assets from ObsPy.

Writes, relative to the repo root:
  assets/ak135.tt         travel-time + ray-parameter tables (format in CLAUDE.md)
  assets/ak135_model.csv  ak135 velocity model: depth_km, vp_km_s, vs_km_s, density_g_cm3

and prints a block of anchor values computed directly by ObsPy, ready to paste
into crates/tables/tests.

Phase naming (verified against ObsPy 1.5.1, see docs/approximations.md):
  * ObsPy names direct waves from a buried source "p"/"s" when the ray leaves
    upward and "P"/"S" when it leaves downward. Our "P" and "S" layers hold the
    first arrival of either.
  * ObsPy does not accept "PKPab"/"PKPbc"; both outer-core branches are called
    "PKP". We split them by ray parameter: along PKP, epicentral distance has a
    minimum at the B caustic (~145 deg). Rays with larger ray parameter than
    that turn higher in the outer core (ab branch), rays with smaller ray
    parameter turn deeper, down to grazing the inner core (bc branch).
  * PKPdf is ObsPy's "PKIKP".

Usage (from a virtualenv OUTSIDE the repo with tools/requirements.txt installed):
  python tools/gen_tables.py                # full run, minutes
  python tools/gen_tables.py --probe 100    # time a single depth row and exit
"""

import argparse
import json
import math
import multiprocessing as mp
import os
import struct
import sys
import time
from pathlib import Path

import numpy as np

REPO = Path(__file__).resolve().parent.parent
ASSETS = REPO / "assets"

MAGIC = b"SEISMOTT"
FORMAT_VERSION = 1
MODEL = "ak135"

DEPTH_START, DEPTH_STEP, DEPTH_COUNT = 0.0, 5.0, 141  # 0..700 km
DIST_START, DIST_STEP, DIST_COUNT = 0.0, 0.25, 721  # 0..180 deg

# Output layers, in CLAUDE.md priority order. New phases are appended, so the earlier layers
# keep their place in the file.
PHASES = ["P", "S", "PcP", "ScS", "PKPab", "PKPbc", "PKPdf", "PP", "SS", "SKS", "Pdiff", "Sdiff", "pP", "sP"]

# ObsPy phase names each output layer takes its first arrival from.
SOURCES = {
    "P": ["P", "p"],
    "S": ["S", "s"],
    "PcP": ["PcP"],
    "ScS": ["ScS"],
    "PKPab": ["PKP"],
    "PKPbc": ["PKP"],
    "PKPdf": ["PKIKP"],
    "PP": ["PP"],
    "SS": ["SS"],
    "SKS": ["SKS"],
    "Pdiff": ["Pdiff"],
    "Sdiff": ["Sdiff"],
    # Depth phases: the ray leaves upward and reflects off the surface above the source (sP
    # leaves as S and converts to P there). ObsPy gives none for a source at the surface.
    "pP": ["pP"],
    "sP": ["sP"],
}
OBSPY_NAMES = sorted({n for names in SOURCES.values() for n in names})


def depth_axis():
    return [DEPTH_START + i * DEPTH_STEP for i in range(DEPTH_COUNT)]


def dist_axis():
    return [DIST_START + i * DIST_STEP for i in range(DIST_COUNT)]


# ---------------------------------------------------------------------------
# Worker: one depth row
# ---------------------------------------------------------------------------

_TAU_MODEL = None


def _tau_model():
    global _TAU_MODEL
    if _TAU_MODEL is None:
        from obspy.taup import TauPyModel

        _TAU_MODEL = TauPyModel(MODEL).model
    return _TAU_MODEL


def pkp_caustic_ray_param(phase):
    """Ray parameter (s/rad) at the B caustic: where PKP distance is smallest."""
    dist = np.asarray(phase.dist)
    rp = np.asarray(phase.ray_param)
    if dist.size == 0:
        return None
    return float(rp[int(np.argmin(dist))])


def compute_row(depth_km):
    """Return (depth_km, times[phase][dist], ray_params[phase][dist]) as float32 arrays."""
    from obspy.taup.helper_classes import TauModelError
    from obspy.taup.seismic_phase import SeismicPhase

    model = _tau_model().depth_correct(depth_km)
    if depth_km != 0.0:
        model = model.split_branch(0.0)

    phases = {}
    for name in OBSPY_NAMES:
        try:
            phases[name] = SeismicPhase(name, model, 0.0)
        except TauModelError:
            phases[name] = None

    p_b = pkp_caustic_ray_param(phases["PKP"]) if phases.get("PKP") is not None else None

    times = np.full((len(PHASES), DIST_COUNT), np.nan, dtype=np.float64)
    rps = np.full((len(PHASES), DIST_COUNT), np.nan, dtype=np.float64)

    for j, deg in enumerate(dist_axis()):
        arrivals = {}
        for name, phase in phases.items():
            arrivals[name] = phase.calc_time(deg) if phase is not None else []
        for k, out in enumerate(PHASES):
            cands = [a for n in SOURCES[out] for a in arrivals[n]]
            if out == "PKPab":
                cands = [a for a in cands if p_b is not None and a.ray_param > p_b]
            elif out == "PKPbc":
                cands = [a for a in cands if p_b is not None and a.ray_param <= p_b]
            if not cands:
                continue
            first = min(cands, key=lambda a: a.time)
            times[k, j] = first.time
            rps[k, j] = first.ray_param_sec_degree
    return depth_km, times.astype(np.float32), rps.astype(np.float32)


# ---------------------------------------------------------------------------
# Writers
# ---------------------------------------------------------------------------


def write_table(path, times, rps, obspy_version):
    """times, rps: float32 arrays shaped [phase][depth][distance]."""
    header = {
        "model": MODEL,
        "obspy_version": obspy_version,
        "depth": {"start": DEPTH_START, "step": DEPTH_STEP, "count": DEPTH_COUNT},
        "distance": {"start": DIST_START, "step": DIST_STEP, "count": DIST_COUNT},
        "phases": PHASES,
    }
    hjson = json.dumps(header, indent=1).encode("utf-8")
    # Pad with spaces (valid JSON whitespace) so the float data is 4-byte aligned in the file.
    hjson += b" " * ((-(16 + len(hjson))) % 4)
    with open(path, "wb") as f:
        f.write(MAGIC)
        f.write(struct.pack("<II", FORMAT_VERSION, len(hjson)))
        f.write(hjson)
        for k in range(len(PHASES)):
            f.write(np.ascontiguousarray(times[k], dtype="<f4").tobytes())
            f.write(np.ascontiguousarray(rps[k], dtype="<f4").tobytes())


def write_model_csv(path):
    layers = _tau_model().s_mod.v_mod.layers
    rows = []
    for layer in layers:
        top = (layer["top_depth"], layer["top_p_velocity"], layer["top_s_velocity"], layer["top_density"])
        bot = (layer["bot_depth"], layer["bot_p_velocity"], layer["bot_s_velocity"], layer["bot_density"])
        for row in (top, bot):
            row = tuple(float(x) for x in row)
            if not rows or rows[-1] != row:
                rows.append(row)
    with open(path, "w", newline="\n", encoding="utf-8") as f:
        f.write("depth_km,vp_km_s,vs_km_s,density_g_cm3\n")
        for r in rows:
            f.write("{:.4f},{:.5f},{:.5f},{:.5f}\n".format(*r))
    return len(rows)


# ---------------------------------------------------------------------------
# Anchors: computed through the public ObsPy API, independently of compute_row
# ---------------------------------------------------------------------------

# (phase, depth_km, distance_deg). On-node unless noted; off-node points sit
# away from branch splits and triplications.
ANCHORS = [
    ("P", 0.0, 30.0),
    ("P", 0.0, 90.0),
    ("S", 0.0, 30.0),
    ("S", 0.0, 90.0),
    ("PKPdf", 0.0, 180.0),
    ("PKPab", 0.0, 150.0),
    ("PKPbc", 0.0, 150.0),
    ("PcP", 0.0, 40.0),
    ("ScS", 0.0, 50.0),
    ("SKS", 0.0, 100.0),
    ("Pdiff", 0.0, 110.0),
    ("PP", 0.0, 60.0),
    ("SS", 0.0, 80.0),
    ("P", 100.0, 30.0),
    ("P", 100.0, 60.0),
    ("S", 100.0, 60.0),
    ("P", 600.0, 5.0),  # upgoing "p" at depth
    ("P", 100.0, 47.13),  # off-node distance
    ("S", 0.0, 72.6),  # off-node distance
    ("P", 32.0, 40.0),  # off-node depth
    ("S", 212.5, 55.55),  # off-node depth and distance
    ("pP", 100.0, 40.0),
    ("pP", 100.0, 80.0),
    ("sP", 100.0, 60.0),
    ("pP", 600.0, 40.0),
    ("pP", 600.0, 60.0),
    ("sP", 600.0, 40.0),
    ("sP", 600.0, 80.0),
    ("pP", 0.0, 60.0),  # surface source: no depth phase
]


def anchor_value(model, phase, depth, dist):
    names = SOURCES[phase]
    arrs = model.get_travel_times(source_depth_in_km=depth, distance_in_degree=dist, phase_list=names)
    arrs = [a for a in arrs if a.name in names]
    if phase in ("PKPab", "PKPbc"):
        # Independent check of the split: with both branches present, ab has
        # the larger ray parameter.
        if len(arrs) < 2:
            return None
        arrs = sorted(arrs, key=lambda a: a.ray_param)
        arrs = [arrs[-1]] if phase == "PKPab" else [arrs[0]]
    if not arrs:
        return None
    first = min(arrs, key=lambda a: a.time)
    return first.time, first.ray_param_sec_degree


def direct_cutoff(model, phase, depth):
    """Largest distance (0.05 deg scan from 90 deg) with a direct P/S arrival."""
    last = None
    deg = 90.0
    while deg <= 120.0:
        arrs = model.get_travel_times(source_depth_in_km=depth, distance_in_degree=deg, phase_list=SOURCES[phase])
        if arrs:
            last = deg
        deg = round(deg + 0.05, 4)
    return last


def print_anchors():
    from obspy.taup import TauPyModel

    model = TauPyModel(MODEL)
    print("\n// ---- anchors from ObsPy (paste into crates/tables/tests/real_table.rs) ----")
    print("// (phase, depth_km, distance_deg, time_s, ray_param_s_per_deg)")
    for phase, depth, dist in ANCHORS:
        v = anchor_value(model, phase, depth, dist)
        if v is None:
            print(f"// {phase} {depth} {dist}: no arrival")
            continue
        print(f'    ("{phase}", {depth:.2f}, {dist:.2f}, {v[0]:.3f}, {v[1]:.4f}),')
    for phase in ("P", "S"):
        c = direct_cutoff(model, phase, 29.0)
        print(f"// direct {phase} cutoff for a 29 km source (ObsPy, 0.05 deg scan): {c}")


# ---------------------------------------------------------------------------


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--probe", type=float, help="time one depth row (km) and exit")
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    args = ap.parse_args()

    import obspy

    if args.probe is not None:
        t0 = time.time()
        d, t, r = compute_row(args.probe)
        print(f"depth {d} km: {time.time() - t0:.1f} s")
        for k, name in enumerate(PHASES):
            j150 = int(round((150.0 - DIST_START) / DIST_STEP))
            print(f"  {name:6s} valid={np.isfinite(t[k]).sum():4d}  t(150deg)={t[k, j150]:.2f}")
        return

    ASSETS.mkdir(exist_ok=True)
    t0 = time.time()
    depths = depth_axis()
    times = np.full((len(PHASES), DEPTH_COUNT, DIST_COUNT), np.nan, dtype=np.float32)
    rps = np.full_like(times, np.nan)
    with mp.Pool(args.workers) as pool:
        for n, (d, t, r) in enumerate(pool.imap_unordered(compute_row, depths), 1):
            i = depths.index(d)
            times[:, i, :] = t
            rps[:, i, :] = r
            print(f"\r{n}/{len(depths)} depth rows ({time.time() - t0:.0f} s)", end="", file=sys.stderr, flush=True)
    print(file=sys.stderr)

    table_path = ASSETS / "ak135.tt"
    write_table(table_path, times, rps, obspy.__version__)
    n_rows = write_model_csv(ASSETS / "ak135_model.csv")
    print(f"wrote {table_path} ({table_path.stat().st_size} bytes), ak135_model.csv ({n_rows} rows)")
    print(f"ObsPy {obspy.__version__}, {time.time() - t0:.0f} s")
    for k, name in enumerate(PHASES):
        print(f"  {name:6s} cells with an arrival: {int(np.isfinite(times[k]).sum())}")

    print_anchors()


if __name__ == "__main__":
    main()
