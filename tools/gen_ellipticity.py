#!/usr/bin/env python3
"""Generate SEISMO's ellipticity coefficients for ak135 from EllipticiPy.

Writes assets/ak135_ellip.bin (relative to the repo root): for every phase of assets/ak135.tt,
on a coarse grid of source depth and distance, the three coefficients sigma_0..2 (s) of the
ellipticity correction of the phase's first arrival there (the same arrival the table holds).
The correction a station sees, added to the spherical travel time, is

  dt = sum_m sigma_m(depth, distance) * P2m(colatitude) * cos(m * azimuth)

with P2m the Schmidt semi-normalised degree-2 Legendre functions, colatitude the source's
(90 deg - latitude) and azimuth from the source to the station, clockwise from north
(Dziewonski & Gilbert 1976; Kennett & Gudmundsson 1996). EllipticiPy computes the coefficients
from the ray path through the model, with the Earth's ellipticity of figure from Clairaut's
equation (Russell, Rudge, Irving and Cottaar 2022, cite when used). EllipticiPy is LGPL-3.0,
like ObsPy; it is used here as a tool and nothing of it ships with SEISMO.

File format (all little-endian):
  b"SEISMOEL" magic (8 bytes), u32 format version, u32 header length, UTF-8 JSON header
  (model, versions, citation, depth and distance axes, phase names in order), then per phase
  in header order the coefficients as f32 [depth][distance][sigma_0, sigma_1, sigma_2]; NaN
  where the phase has no arrival.

Usage (from a virtualenv OUTSIDE the repo with tools/requirements.txt installed):
  python tools/gen_ellipticity.py           # full run, a few minutes on all cores
"""

import argparse
import json
import multiprocessing as mp
import os
import struct
import sys
import time
from pathlib import Path

import numpy as np

from gen_tables import MODEL, OBSPY_NAMES, PHASES, SOURCES, pkp_caustic_ray_param

REPO = Path(__file__).resolve().parent.parent
OUT = REPO / "assets" / "ak135_ellip.bin"

MAGIC = b"SEISMOEL"
FORMAT_VERSION = 1
CITATION = (
    "Russell, S., Rudge, J. F., Irving, J. C. E. and Cottaar, S., A re-examination of "
    "ellipticity corrections for seismic phases, Geophys. J. Int., 231(3), 2095-2101, 2022, "
    "doi:10.1093/gji/ggac315."
)

# The correction changes slowly with depth and distance, so a coarse grid is enough: every
# fifth depth row and every fourth distance column of the travel-time table.
DEPTH_START, DEPTH_STEP, DEPTH_COUNT = 0.0, 25.0, 29  # 0..700 km
DIST_START, DIST_STEP, DIST_COUNT = 0.0, 1.0, 181  # 0..180 deg


def depth_axis():
    return [DEPTH_START + i * DEPTH_STEP for i in range(DEPTH_COUNT)]


def dist_axis():
    return [DIST_START + i * DIST_STEP for i in range(DIST_COUNT)]


_TAU_MODEL = None


def _tau_model():
    global _TAU_MODEL
    if _TAU_MODEL is None:
        from obspy.taup import TauPyModel

        _TAU_MODEL = TauPyModel(MODEL).model
    return _TAU_MODEL


def first_arrival(out, arrivals, p_b):
    """The arrival gen_tables.compute_row keeps for layer `out`: the earliest of its ObsPy
    phases, PKP split into ab and bc at the B caustic's ray parameter `p_b`."""
    cands = [a for n in SOURCES[out] for a in arrivals[n]]
    if out == "PKPab":
        cands = [a for a in cands if p_b is not None and a.ray_param > p_b]
    elif out == "PKPbc":
        cands = [a for a in cands if p_b is not None and a.ray_param <= p_b]
    return min(cands, key=lambda a: a.time) if cands else None


def compute_row(depth_km):
    """Return (depth_km, coefficients[phase][dist][3]) for one source depth."""
    from ellipticipy.tools import individual_ellipticity_coefficients
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

    out = np.full((len(PHASES), DIST_COUNT, 3), np.nan, dtype=np.float64)
    for j, deg in enumerate(dist_axis()):
        arrivals = {n: (p.calc_time(deg) if p is not None else []) for n, p in phases.items()}
        for k, name in enumerate(PHASES):
            first = first_arrival(name, arrivals, p_b)
            if first is None:
                continue
            first = first.phase.calc_path_from_arrival(first)
            out[k, j, :] = individual_ellipticity_coefficients(first)
    return depth_km, out.astype(np.float32)


def write_file(path, coeffs, versions):
    """coeffs: float32 array shaped [phase][depth][distance][3]."""
    header = {
        "model": MODEL,
        **versions,
        "citation": CITATION,
        "depth": {"start": DEPTH_START, "step": DEPTH_STEP, "count": DEPTH_COUNT},
        "distance": {"start": DIST_START, "step": DIST_STEP, "count": DIST_COUNT},
        "phases": PHASES,
        "values": "sigma_0, sigma_1, sigma_2 (s) of the first arrival; NaN where none",
    }
    hjson = json.dumps(header, indent=1).encode("utf-8")
    # Pad with spaces (valid JSON whitespace) so the float data is 4-byte aligned in the file.
    hjson += b" " * ((-(16 + len(hjson))) % 4)
    with open(path, "wb") as f:
        f.write(MAGIC)
        f.write(struct.pack("<II", FORMAT_VERSION, len(hjson)))
        f.write(hjson)
        for k in range(len(PHASES)):
            f.write(np.ascontiguousarray(coeffs[k], dtype="<f4").tobytes())


# ---------------------------------------------------------------------------
# Anchors: EllipticiPy's own corrections, for crates/tables/tests and crates/sim
# ---------------------------------------------------------------------------

# (phase, depth_km, distance_deg, source_latitude, azimuth_deg). The first ones are grid nodes,
# the rest between nodes (away from branch ends, where the coefficients are smooth).
ANCHORS = [
    ("P", 0.0, 60.0, 45.0, 30.0),
    ("S", 100.0, 80.0, -30.0, 120.0),
    ("PKPdf", 300.0, 150.0, -20.0, 200.0),
    ("PKPab", 0.0, 170.0, 70.0, 0.0),
    ("ScS", 50.0, 40.0, 10.0, 300.0),
    ("pP", 200.0, 50.0, 60.0, 90.0),
    ("P", 110.0, 62.5, 35.5, 45.0),
    ("SKS", 37.0, 101.3, -52.0, 250.0),
    ("PcP", 612.0, 33.7, 5.0, 170.0),
]


def anchor_value(model, phase, depth, dist, lat, az):
    """EllipticiPy's correction for the first arrival, from ObsPy's own ray path."""
    from ellipticipy import ellipticity_correction
    from obspy.taup.seismic_phase import SeismicPhase

    names = SOURCES[phase]
    arrs = model.get_ray_paths(source_depth_in_km=depth, distance_in_degree=dist, phase_list=names)
    arrs = [a for a in arrs if a.name in names]
    if phase in ("PKPab", "PKPbc"):
        tm = model.model.depth_correct(depth)
        if depth != 0.0:
            tm = tm.split_branch(0.0)
        p_b = pkp_caustic_ray_param(SeismicPhase("PKP", tm, 0.0))
        arrs = [a for a in arrs if (a.ray_param > p_b) == (phase == "PKPab")]
    if not arrs:
        return None
    first = min(arrs, key=lambda a: a.time)
    return ellipticity_correction(first, azimuth=az, source_latitude=lat)


def print_anchors():
    from obspy.taup import TauPyModel

    model = TauPyModel(MODEL)
    print("\n// ---- anchors from EllipticiPy (paste into crates/sim/src/ellipticity.rs) ----")
    print("// (phase, depth_km, distance_deg, source_latitude, azimuth_deg, correction_s)")
    for phase, depth, dist, lat, az in ANCHORS:
        v = anchor_value(model, phase, depth, dist, lat, az)
        if v is None:
            print(f"// {phase} {depth} {dist}: no arrival")
            continue
        print(f'    ("{phase}", {depth:.1f}, {dist:.1f}, {lat:.1f}, {az:.1f}, {v:.4f}),')


def main():
    ap = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    ap.add_argument("--workers", type=int, default=max(1, (os.cpu_count() or 2) - 1))
    ap.add_argument("--anchors", action="store_true", help="print the anchors only")
    args = ap.parse_args()

    import obspy
    from importlib.metadata import version

    if args.anchors:
        print_anchors()
        return

    t0 = time.time()
    depths = depth_axis()
    coeffs = np.full((len(PHASES), DEPTH_COUNT, DIST_COUNT, 3), np.nan, dtype=np.float32)
    with mp.Pool(args.workers) as pool:
        for n, (d, row) in enumerate(pool.imap_unordered(compute_row, depths), 1):
            coeffs[:, depths.index(d), :, :] = row
            print(f"\r{n}/{len(depths)} depth rows ({time.time() - t0:.0f} s)", end="", file=sys.stderr, flush=True)
    print(file=sys.stderr)

    versions = {"obspy_version": obspy.__version__, "ellipticipy_version": version("ellipticipy")}
    write_file(OUT, coeffs, versions)
    print(f"wrote {OUT} ({OUT.stat().st_size} bytes), {time.time() - t0:.0f} s, {versions}")
    for k, name in enumerate(PHASES):
        c = coeffs[k]
        ok = np.isfinite(c[..., 0])
        print(f"  {name:6s} cells: {int(ok.sum()):5d}  |sigma| max: {np.nanmax(np.abs(c)) if ok.any() else 0:.2f} s")

    print_anchors()


if __name__ == "__main__":
    main()
