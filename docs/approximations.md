# Approximations

Every place where SEISMO deliberately departs from the physics, and what it costs.

## Spherical Earth, ellipticity ignored

Distances are great-circle arcs on a sphere of radius 6371 km, using geographic latitude
directly on the sphere (the ObsPy `locations2degrees` convention). Travel times come from the
1D ak135 model with no ellipticity correction. The real Earth's flattening shifts teleseismic
travel times by up to about ±1 s depending on path and latitude; SEISMO ignores this.

## First arrival only per phase (PKP as three branches)

Each cell of `assets/ak135.tt` holds the **earliest** arrival of its named phase. Where a phase
has several arrivals at one distance (the P and S triplications from the 410 and 660 km
discontinuities at roughly 15–30°, multiple PP/SS branches), only the first is kept. Later
branches are not drawn.

PKP is the exception: it is stored as three phases, `PKPab`, `PKPbc` and `PKPdf`.

- ObsPy 1.5.1 does not accept `PKPab`/`PKPbc` as phase names; it calls both outer-core
  branches `PKP`. `tools/gen_tables.py` splits them by ray parameter. Along `PKP`, epicentral
  distance is smallest at the B caustic (~145° for a surface source). Rays with a larger ray
  parameter than the caustic ray turn higher in the outer core (**ab**); rays with a smaller
  one turn deeper, down to grazing the inner core (**bc**). This rule was confirmed once
  during development with ObsPy ray paths: at 146° from a surface source, the 3.80 s/deg ray
  turns at 4246 km depth and the 3.04 s/deg ray at 4625 km. `gen_tables.py` itself does not
  trace ray paths. The anchor tests re-check the split at 150° through ObsPy's public API,
  where, with both branches present, the larger ray parameter is taken as ab.
- `PKPdf` is ObsPy's `PKIKP`.

## Direct P and S include the upgoing legs (p and s)

For a buried source ObsPy names the direct ray `p`/`s` when it leaves the source upward and
`P`/`S` when it leaves downward. Close to the epicentre of a deep event only the upgoing ray
exists (for a 600 km source, out past 10°). SEISMO's `P` and `S` layers hold the first arrival
of either, so the map front has no hole near the epicentre. `Pdiff`/`Sdiff` stay separate.

## Direct-wave cutoff (core shadow)

Direct P and S stop where their rays graze the core. Beyond that, only the diffracted waves
(`Pdiff`, `Sdiff`) and core phases arrive. Phase 1 draws direct P and S only, so the map fronts
stop there. For the Tōhoku test event (29 km source):

| | ObsPy, 0.05° scan | Table as drawn |
|---|---|---|
| direct P | 99.55° | 99.5° |
| direct S | 99.95° | 99.75° |

The table value is lower because the table is sampled every 0.25°, and a bilinear lookup needs
all four surrounding cells. The commonly quoted figure for the start of the P shadow is
~103°; in ak135, as computed by ObsPy for this source depth, direct P ends near 99.5°. SEISMO
uses whatever the generated table says. Nothing in the code or tests assumes 103°.

## Bilinear lookup returns nothing at branch edges

Lookups between grid nodes are bilinear over the four surrounding cells and return no value if
any of the four is missing. Near the end of a branch, a front therefore stops up to one
distance step (0.25°) and one depth step (5 km) short of where ObsPy would put it.
Interpolation is also not smooth across branch splits and triplications, where the first
arrival switches from one branch to another inside a cell.

## Slice view: ray-traced fronts

The slice draws wavefronts inside the Earth from rays traced through the ak135 velocity model
(`sim::raytrace`), not from the table. The model is cut into shells at most 10 km thick, with
velocity a power law of radius inside each shell. Velocities are linear in depth between the
model's samples, and each shell replaces that with a power law between its end values. A test
checks the traced surface arrival times against the table within 1 s: P and S at 30°, 60° and
90°; PKIKP at 150° and 180°; PcP, ScS, SKS, PP, SS and both PKP branches at one distance each.

What the slice leaves out:

- **Only nine phases:** P, S, PcP, ScS, PKP, PKIKP, SKS, PP and SS, P and S including the rays
  that leave a buried source upward. Rays that cannot get through the velocity jump at the
  410 or 660 bounce off its top (total reflection) and stay part of P and S, as in ObsPy. There
  are no partial reflections (a ray that gets through leaves no reflected copy), no reflections
  off the underside of a layer, no surface reflections other than PP and SS, no conversions
  other than those named (no PcS, no SKP, no S in the inner core), and no depth phases (pP, sP).
- **No diffraction, no head waves.** Fronts end where the rays end. There is no Pdiff or Sdiff
  creeping along the core, so beyond about 100° the slice shows only the core phases and the surface
  reflections PP and SS, and there is no Pn running along the Moho.
- **Shared paths drawn once.** Before they reach the core, PcP, PKP and PKIKP rays follow the
  same path down. That part is drawn as one front (the PcP fan), and PKP and PKIKP start at the
  core-mantle boundary. Likewise PP and SS start where P and S reach the surface. Labels sit on
  the leg that gives a phase its name.
- **Fronts join neighbouring rays with straight lines.** Rays are refined until neighbours
  land and turn within 60 km of each other. A front is broken where neighbours are more than
  400 km apart. Cusps and folds at triplications and caustics are real, but are only as sharp
  as the ray spacing allows.
- **Mirror image.** The 1D model is the same in every direction, so the slice shows half of the
  great circle. The azimuth only moves the cut on the map.
- **Brightness is not amplitude.** Line brightness is fixed per phase for legibility. The
  fainter fronts behind each front are copies at earlier times, a display trail, not a coda.

## Surface waves at constant group velocity, not dispersive

(Not drawn yet.) Rayleigh and Love waves will be rings expanding at a constant group velocity
(Rayleigh ~3.5 km/s, Love ~4.4 km/s, configurable in `sim`). Real surface waves are dispersive:
their speed depends on period, so a real wave train spreads out with distance. SEISMO draws a
single non-dispersive ring.

## Map fronts: line width and trail

The map draws each front as a line of fixed pixel width, centred where the tabulated arrival
time equals the time since origin. The line width is converted from time to degrees using the
tabulated ray parameter (dT/dΔ). Where the ray parameter approaches 0 (directly above a buried
source, and for PKIKP near 180°), it is clamped to 0.5 s/deg for this conversion only. The
fading trail behind a front is a display effect (exponential in time since arrival), not a
model of the coda.

## 3D mantle: SP12RTS on top of ak135

(Model loaded; not drawn yet.) The 3D structure is SP12RTS (Koelemeijer, Ritsema, Deuss & van
Heijst 2016, *Geophys. J. Int.* 204(2), 1024–1039, doi:10.1093/gji/ggv481), bundled unchanged
in `assets/sp12rts/` with its citation. It gives the shear and compressional velocity
perturbations, dlnVs and dlnVp, as spherical harmonics up to degree 12 (no detail finer than
about 1500 km) on 21 radial splines.

- **Different base model.** SP12RTS perturbations are relative to PREM. SEISMO applies them as
  fractions of the ak135 velocities. The two base models differ by about 1% in places, which is
  small next to the perturbations themselves but not zero.
- **Degree 0 ignored.** The authors remove the radial average; SEISMO drops whatever is left of
  it, so the perturbations average to zero at every depth and only lateral differences remain.
- **Mantle only.** The model runs from its own Moho (24.4 km deep, PREM's) to the core-mantle
  boundary (radius 3480 km). The crust and the core stay 1D.
- **Gridded for speed.** The exact harmonic sum is sampled once onto a 2° × 2° × 25 km grid and
  looked up trilinearly; the grid differs from the exact sum by less than 0.1% of velocity.
- Checked against the paper: peak dlnVs of about 3.5% near 100 km depth and about 1.5% just
  above the core, as the paper states.
