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
- **Half the circle at a time.** The slice shows the half of the great circle from the
  epicentre along the chosen azimuth to the antipode; the other half is at azimuth + 180°. That
  half holds the rays leaving along the azimuth until they pass the antipode, and the rays
  leaving the opposite way once they have passed it. With the 1D rays these are the same rays,
  so in 1D every azimuth looks the same.
- **Rock shading.** With the 3D mantle on, the mantle part of the slice is shaded by the model's
  shear (or, if chosen, compressional) perturbation along the cut: brighter is faster, ±1% spans
  the shading, sampled every 1° along the cut and every 25 km in depth. The shading is a display
  of the model at its real amplitude; the strength setting does not scale it.
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

The 3D structure is SP12RTS (Koelemeijer, Ritsema, Deuss & van
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

## Map: 3D delays along the 1D rays, not bent rays

The map's fronts are the table's arrival times plus a delay from the 3D mantle, scaled by the
strength setting (0 = 1D, 1 = real, up to 20× exaggerated; the screen says EXAGGERATED above
1×).

- **First order, unbent rays.** Each ak135 ray is laid out along an azimuth from the source and
  its delay is −∫ dlnV dt along that fixed path. Bending the ray through the structure would
  change the time only at second order (Fermat's principle), a small fraction of a second at
  real strength. When exaggerated, the delay is simply scaled; a model really that strong would
  bend rays noticeably and the true times would differ.
- **Measured size.** For the Tōhoku event at real strength the delay varies with direction by
  about 2–3 s for P and 4–11 s for S, moving the fronts by about 0.2°–1.2°: one to six pixels on
  a full-HD map.
- **Coarse grid.** Delays are computed every 1° of distance and every 5° of azimuth and
  interpolated. The model has no structure finer than about 15°, so this loses little.
- **First arrival.** Where a phase has several branches (the 410/660 triplications), the delay
  of the earliest branch is used, matching the table.
- **Edges.** Where the 1D rays do not reach (beyond the direct-wave cutoff near 100°) the delay
  is 0, so the 3D model never moves the end of a front.
- **Crust and core are 1D.** Perturbations start below ak135's Moho (35 km) and stop at the
  core-mantle boundary.
- The P and S distances shown in the side panel are the 1D values.

## Slice: rays bent through the 3D mantle, in the plane of the cut

With the 3D mantle on, the slice's rays are traced through v = v₁(r)·(1 + s·dlnV), where v₁ is
ak135 (velocity linear in depth between its samples), dlnV is SP12RTS along the cut and s the
strength. Each ray is integrated with fixed-step RK4 (4 s, 0.5 s near turning points), ending
steps exactly on every model sample depth and velocity discontinuity; at discontinuities the
ray refracts by Snell's law, reflects, or is rejected by the same per-phase rules as the 1D
tracer.

- **In the plane only.** Rays stay in the vertical plane of the cut; the part of the velocity
  gradient across the plane is ignored. Real rays drift a little sideways; at real strength the
  drift is small.
- **Checked against the 1D tracer.** With no perturbation the bent rays land within 0.01° and
  0.1 s of the closed-form 1D rays for all nine phases (0.1 s is the closed-form tracer's own
  shell approximation, largest for SS). At real strength a bent ray's time, moved to the 1D
  ray's landing point, matches the map's 1D time + delay within 0.3 s, so map and slice agree.
- **Above 1× the views differ in method.** The map scales the first-order delay linearly; the
  slice traces rays through a model scaled by the same factor, so at 10× or 20× the rays bend
  much more than the linear scaling assumes and the two views no longer match exactly. The
  velocity factor 1 + s·dlnV is kept at or above 0.2.
- **Fewer rays.** Bent fans are seeded with at most 300 take-off angles from each 1D fan, then
  refined where neighbouring rays land more than 60 km apart, up to 1500 rays per fan. Where the
  exaggerated model makes rays miss their phase (a P ray reaching the core, a PKP ray reaching
  the inner core), the ray is dropped and the front shows a gap.
- **Computed per cut.** Bent fans depend on the event, the azimuth and the strength; they are
  rebuilt on a worker thread when any of these changes (about 2–3 s), and the 1D fans are drawn
  until they are ready.
