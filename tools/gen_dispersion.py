#!/usr/bin/env python3
"""Generate SEISMO's surface-wave dispersion curves from the bundled ak135 model.

Writes assets/surface_dispersion.csv (relative to the repo root): phase and group velocity of
the fundamental-mode Rayleigh and Love waves, as a function of period. The first lines of the
file are '#' comments recording how it was made.

The model is assets/ak135_model.csv (written by tools/gen_tables.py), cut into thin layers
(ak135's discontinuities kept as layer boundaries) and turned into an equivalent flat model by
the Earth-flattening transformation, so that a flat-layer code (disba, after Herrmann's
Computer Programs in Seismology) gives the spherical Earth's dispersion:

  depth   z' = a ln(a / r)
  speed   v' = v a / r
  density rho' = rho (r / a)^5      (Love; exact for SH)
          rho' = rho (r / a)^2.275  (Rayleigh; Biswas 1972, approximate)

with a = 6371 km and r the radius. Gravity is ignored. On 2026-10-15 the result was checked
against Mineos (a spherical normal-mode code with gravity, run on the same ak135 without
attenuation): phase velocity within 0.1% and group velocity within 0.25% over the whole band
(see docs/approximations.md).

The script fails if a curve is not smooth (a jump to a higher mode) or if the group velocity
does not match the one implied by the phase velocity, U = c / (1 + (T / c) dc/dT).

Usage:
  python tools/gen_dispersion.py
"""

import csv
import datetime as dt
import sys
from importlib.metadata import version
from pathlib import Path

import numpy as np
from disba import GroupDispersion, PhaseDispersion

REPO = Path(__file__).resolve().parent.parent
MODEL = REPO / "assets" / "ak135_model.csv"
OUT = REPO / "assets" / "surface_dispersion.csv"

EARTH_RADIUS_KM = 6371.0
# The flat model stops here; below is a half-space with the model's values at this depth.
BOTTOM_KM = 1500.0
# Period band (s), log-spaced: the periods a display seismogram needs. Up to 200 s the
# missing gravity changes velocities by less than 0.1% (checked against Mineos).
PERIOD_MIN_S = 8.0
PERIOD_MAX_S = 200.0
PERIOD_COUNT = 120
# Density exponents of the flattening transformation.
RHO_EXPONENT = {"love": 5.0, "rayleigh": 2.275}


def max_layer_km(depth):
    """Thin layers where the waves are most sensitive, thicker deeper down."""
    if depth < 120.0:
        return 2.0
    if depth < 700.0:
        return 5.0
    return 10.0


def read_model():
    with MODEL.open(newline="") as f:
        rows = list(csv.DictReader(f))
    return [
        (
            float(r["depth_km"]),
            float(r["vp_km_s"]),
            float(r["vs_km_s"]),
            float(r["density_g_cm3"]),
        )
        for r in rows
    ]


def spherical_layers(model):
    """(top, bottom, vp, vs, rho) of thin layers down to BOTTOM_KM, each with the model's
    values at its middle (linear between the model's points; the model's repeated depths are
    discontinuities and stay layer boundaries)."""
    layers = []
    for (z0, vp0, vs0, rho0), (z1, vp1, vs1, rho1) in zip(model, model[1:]):
        if z1 <= z0 or z0 >= BOTTOM_KM:
            continue
        z1c = min(z1, BOTTOM_KM)
        n = int(np.ceil((z1c - z0) / max_layer_km(z0) - 1e-9))
        edges = np.linspace(z0, z1c, n + 1)
        for top, bottom in zip(edges, edges[1:]):
            f = (0.5 * (top + bottom) - z0) / (z1 - z0)
            layers.append(
                (
                    top,
                    bottom,
                    vp0 + f * (vp1 - vp0),
                    vs0 + f * (vs1 - vs0),
                    rho0 + f * (rho1 - rho0),
                )
            )
    # Half-space: the model's values just below BOTTOM_KM.
    for (z0, vp0, vs0, rho0), (z1, vp1, vs1, rho1) in zip(model, model[1:]):
        if z0 <= BOTTOM_KM < z1:
            f = (BOTTOM_KM - z0) / (z1 - z0)
            half = (vp0 + f * (vp1 - vp0), vs0 + f * (vs1 - vs0), rho0 + f * (rho1 - rho0))
            break
    else:
        sys.exit(f"model does not reach {BOTTOM_KM} km")
    return layers, half


def flatten(layers, half, wave):
    """Flat-model (thickness, vp, vs, rho) arrays, the last row a half-space (thickness is
    ignored there)."""
    a = EARTH_RADIUS_KM
    n = RHO_EXPONENT[wave]
    zf = lambda z: a * np.log(a / (a - z))  # noqa: E731
    th, vp, vs, rho = [], [], [], []
    for top, bottom, p, s, d in layers:
        r = a - 0.5 * (top + bottom)
        th.append(zf(bottom) - zf(top))
        vp.append(p * a / r)
        vs.append(s * a / r)
        rho.append(d * (r / a) ** n)
    r = a - BOTTOM_KM
    p, s, d = half
    th.append(1.0)
    vp.append(p * a / r)
    vs.append(s * a / r)
    rho.append(d * (r / a) ** n)
    return [np.array(x) for x in (th, vp, vs, rho)]


def curves(layers, half, wave, periods):
    th, vp, vs, rho = flatten(layers, half, wave)
    c = PhaseDispersion(th, vp, vs, rho, dc=0.0005)(periods, mode=0, wave=wave)
    u = GroupDispersion(th, vp, vs, rho, dc=0.0005, dt=0.005)(periods, mode=0, wave=wave)
    if len(c.period) != len(periods) or len(u.period) != len(periods):
        sys.exit(f"{wave}: disba returned {len(c.period)}/{len(u.period)} of {len(periods)} periods")
    return c.velocity, u.velocity


def check(wave, periods, c, u):
    """Fail on a mode jump (a kink) or on group velocity disagreeing with phase velocity."""
    lt = np.log(periods)
    # Second difference of c against log period: a smooth curve changes slowly.
    d2 = np.abs(np.diff(c, 2))
    if d2.max() > 0.01:
        k = int(d2.argmax()) + 1
        sys.exit(f"{wave}: phase velocity not smooth near {periods[k]:.1f} s (jump {d2.max():.4f})")
    # U = c / (1 + (T/c) dc/dT) = c / (1 + (1/c) dc/dlnT)
    dc = np.gradient(c, lt)
    u_from_c = c / (1.0 + dc / c)
    rel = np.abs(u_from_c - u) / u
    # The ends use one-sided differences; check the interior tightly.
    worst = rel[2:-2].max()
    if worst > 0.002:
        k = int(rel[2:-2].argmax()) + 2
        sys.exit(f"{wave}: group velocity off the phase velocity's by {worst:.4f} at {periods[k]:.1f} s")
    return worst


def main():
    model = read_model()
    layers, half = spherical_layers(model)
    periods = np.geomspace(PERIOD_MIN_S, PERIOD_MAX_S, PERIOD_COUNT)
    out = {}
    for wave in ("rayleigh", "love"):
        c, u = curves(layers, half, wave, periods)
        worst = check(wave, periods, c, u)
        out[wave] = (c, u)
        print(f"{wave}: group vs phase-derived worst {worst:.5f}")
        for t in (10, 20, 30, 50, 100, 150, 200):
            k = int(np.abs(periods - t).argmin())
            print(f"  T {periods[k]:7.2f} s  c {c[k]:.4f}  U {u[k]:.4f} km/s")

    today = dt.datetime.now(dt.timezone.utc).date().isoformat()
    with OUT.open("w", newline="") as f:
        f.write(
            "# Fundamental-mode surface-wave dispersion of ak135 (assets/ak135_model.csv), "
            "phase and group velocity (km/s).\n"
            f"# Earth-flattened ({len(layers)} layers to {BOTTOM_KM:.0f} km + half-space), "
            f"computed with disba {version('disba')} on {today} UTC by tools/gen_dispersion.py.\n"
            "# Spherical, no gravity, no attenuation, no ocean (ak135's crust is continental).\n"
        )
        w = csv.writer(f, lineterminator="\n")
        w.writerow(
            [
                "period_s",
                "rayleigh_phase_km_s",
                "rayleigh_group_km_s",
                "love_phase_km_s",
                "love_group_km_s",
            ]
        )
        (rc, ru), (lc, lu) = out["rayleigh"], out["love"]
        for k, t in enumerate(periods):
            w.writerow([f"{t:.4f}", f"{rc[k]:.5f}", f"{ru[k]:.5f}", f"{lc[k]:.5f}", f"{lu[k]:.5f}"])
    print(f"wrote {OUT}")


if __name__ == "__main__":
    main()
