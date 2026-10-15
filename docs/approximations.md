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

## Depth phases pP and sP

`pP` leaves a buried source upward as P, reflects off the surface above it and then travels
down and back up as P; `sP` leaves upward as S and converts to P at that surface point. The
time between P and pP (or sP) grows with the source's depth, which is how seismologists read
depth off a seismogram. The `pP` and `sP` table layers hold ObsPy's first arrival of each,
like the other phases. A source at the surface has none (ObsPy gives none, and the echo would
coincide with P). Since the lookup needs all four surrounding table cells, a source shallower
than the first depth row (5 km) has none either; there the echo would trail P by under about
2 s, which the traces cannot show anyway. Explosions, landslides and impacts are never that
deep, so they never show depth phases. An sP ray leaving close to horizontal has a ray parameter no P wave at the
surface can carry, so it has no sP; ObsPy and the tracer agree on that edge.

- **Shown on the seismograms and in the slice, not on the map.** The map draws P and S only.
- **1D times.** The 3D-mantle delay is computed along the direct P and S rays; the depth phases
  take a different path, so their times stay 1D (in the slice, their rays are bent through the
  3D mantle like the others).
- **Amplitudes** come from their own ray tubes (see Seismograms), with a fixed factor for the
  surface bounce (pP 0.7, sP 0.5). The real factors depend on the angle, flip pP's polarity,
  and are dwarfed by the source's radiation pattern, which SEISMO does not model; so the sizes
  of pP and sP next to P are display choices. sP takes the source's S weight (it leaves as S),
  pP the P weight.

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
90°; PKIKP at 150° and 180°; PcP, ScS, SKS, PP, SS and both PKP branches at one distance each;
pP and sP from 100 and 600 km sources at 40°, 60° and 80°.

What the slice leaves out:

- **Only eleven phases:** P, S, PcP, ScS, PKP, PKIKP, SKS, PP, SS and the depth phases pP and
  sP, P and S including the rays that leave a buried source upward. Rays that cannot get through the velocity jump at the
  410 or 660 bounce off its top (total reflection) and stay part of P and S, as in ObsPy. There
  are no partial reflections (a ray that gets through leaves no reflected copy), no reflections
  off the underside of a layer, no surface reflections other than PP, SS, pP and sP, no
  conversions other than those named (no PcS, no SKP, no S in the inner core), and no sS or
  pS.
- **No diffraction, no head waves.** Fronts end where the rays end. There is no Pdiff or Sdiff
  creeping along the core, so beyond about 100° the slice shows only the core phases and the surface
  reflections PP and SS, and there is no Pn running along the Moho.
- **Shared paths drawn once.** Before they reach the core, PcP, PKP and PKIKP rays follow the
  same path down. That part is drawn as one front (the PcP fan), and PKP and PKIKP start at the
  core-mantle boundary. Likewise PP and SS start where P and S reach the surface, and pP and sP
  where the upgoing p and s rays do (sP's path is drawn as S up to there). Labels sit on
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

Rayleigh and Love waves travel at a constant group velocity (Rayleigh 3.5 km/s, Love 4.4 km/s,
`sim::seismogram::SurfaceWaves`). Real surface waves are dispersive: their speed depends on
period, so a real wave train spreads out with distance. SEISMO's seismograms show each one as a
single packet of fixed length (see below); the map does not draw surface waves yet.

## Map fronts: line width and trail

The map draws each front as a line of fixed pixel width, centred where the tabulated arrival
time equals the time since origin. The line width is converted from time to degrees using the
tabulated ray parameter (dT/dΔ). Where the ray parameter approaches 0 (directly above a buried
source, and for PKIKP near 180°), it is clamped to 0.5 s/deg for this conversion only. The
fading trail behind a front is a display effect (exponential in time since arrival), not a
model of the coda.

## Map coastlines: simplified

The coastlines are Natural Earth's 1:10m coastline, simplified by `tools/gen_coastlines.py`
(Douglas–Peucker at 0.05° in plain latitude and longitude, lines smaller than 0.15° across
dropped, points rounded to 0.001°), from about 411,000 points to about 41,000. At the map's
usual size a pixel is about 0.3°, so the simplification is below a pixel; small islands are
missing. Settings, source version and checksum are in the file's header. They are drawn on the
same spherical-Earth latitude and longitude as everything else.

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
- Checked against the authors' own code (2026-10-15): their map programs (`depmaphj_jr`,
  `raw2xyz_jr` from `SP12RTS_plotting.tar.gz`), compiled unchanged and run on the bundled files
  at all 21 spline knots and halfway between them, both dlnVs and dlnVp on a 2° grid. Within
  ±60° latitude SEISMO's exact sum agrees with their output to about one unit of its last
  printed digit (worst 0.00006 percentage points); the overall size ratio is 1.000000 at every
  depth. Nearer the poles their single-precision build differs by up to 0.002 points; rebuilt
  in double precision it matches SEISMO there too (0.00005). At the grid's own nodes the runtime
  grid differs from their values by at most 0.06 points (near 130 km depth, where dlnVs reaches
  3.6% and changes fast with depth; 25 km linear steps in radius); between nodes the
  latitude/longitude interpolation adds to that, and a test bounds the total at 0.1 points. A
  test keeps 32 of their values.
- **Strength 1 is the published model, not the real Earth.** Like every global tomography
  model, SP12RTS is damped and limited to degree 12, so its fast and slow regions are smoother
  and usually weaker than the real ones; the authors also publish less- and more-damped
  versions. "Real" in the strength control means "as published"; the true Earth's 3D delays are
  probably somewhat larger, so settings a little above 1× are not necessarily unphysical.

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
- **Checked against the 1D tracer.** With no perturbation the bent rays land within 0.05° of
  the closed-form 1D rays, and on time within 0.1 s at the same landing point, for all eleven
  phases from sources at 29 and 600 km (most within 0.02 s; the largest differences are for
  shallow P and S rays and the surface-reflected PP and SS). Knife-edge rays, where a
  1e-5 rad change of take-off angle moves the landing point by more than 0.05° (rays turning
  just above a discontinuity or reflecting off it), are left out of that check. The bent fans
  cover at least the 1D fans' distance ranges. At real strength a bent ray's time, moved to the
  1D ray's landing point, matches the map's 1D time + delay within 0.3 s, so map and slice
  agree.
- **Grazing the core.** Rays that only just enter the core (the far end of PKP, towards 180°)
  are very sensitive to the rock at the bottom of the mantle. Where it is faster above the
  exit point than above the entry, ray theory reflects them back into the core and that end of
  the branch is lost for that azimuth. Real waves diffract around such edges; the slice does
  not model that.
- **Above 1× the views differ in method.** The map scales the first-order delay linearly; the
  slice traces rays through a model scaled by the same factor, so at 10× or 20× the rays bend
  much more than the linear scaling assumes and the two views no longer match exactly. The
  velocity factor 1 + s·dlnV is kept at or above 0.2.
- **Fewer rays.** Bent fans are seeded with at most 300 take-off angles from each 1D fan, plus
  its edges and probes just beyond them, then refined like the 1D fans (neighbours landing or
  turning more than 60 km apart, edges bisected), up to 2000 rays per fan. Where the
  exaggerated model makes rays miss their phase (a P ray reaching the core, a PKP ray reaching
  the inner core), the ray is dropped and the front shows a gap.
- **Computed per cut.** Bent fans depend on the event, the azimuth and the strength; they are
  rebuilt on a worker thread when any of these changes (about 2–3 s), and the 1D fans are drawn
  until they are ready.

## Seismograms: display-plausible, not waveform-accurate

The virtual stations' traces are built from a list of arrivals, each drawn as a wavelet. They
look like seismograms and put every wave at the right time, but their shapes are invented and
their sizes only roughly physical. The UI says so.

- **Stations.** The bundled list is the open stations of the Global Seismographic Network (FDSN
  networks IU and II), thinned so that no two are closer than 12°, generated by
  `tools/gen_stations.py` (query and date in the file's header). Elevation is ignored (spherical
  Earth). Users can add their own stations anywhere.
- **Times.** Body-wave times are the table's (first arrival of each named phase; PKP as three
  branches), plus the 3D-mantle delay for P and S (not pP or sP), sampled from the same grid with the same
  interpolation as the map, so a trace and a map ring agree. Other phases stay 1D. Surface waves
  arrive at path length ÷ group velocity, the short way round (R1, L1) and the long way (R2, L2).
- **Amplitudes from ray tubes.** For each body phase the ray tracer lands a fan of rays (take-off
  steps of 0.25°, refined until neighbours land within 0.2°). Between neighbouring rays the
  amplitude is √(sin i / (|dΔ/di| · sin Δ · cos i₀)): energy leaving between take-off angles i
  and i + di spread over the band of Earth's surface between Δ and Δ + dΔ, with i₀ the angle of
  incidence at the station. A test checks this against straight rays in a uniform mantle
  (amplitude ∝ 1/path length) within 1%. Where a phase has several branches at a station, the
  branch nearest the table's time is used.
- **Caps where ray theory fails.** At caustics (|dΔ/di| → 0, e.g. PKP near 145°), near the
  epicentre and antipode (sin Δ → 0) and at grazing incidence the formula diverges. SEISMO caps
  |dΔ/di| at 0.1 or more, sin Δ at sin 1° or more and cos i₀ at 0.1 or more, so focusing is shown
  but stays finite.
- **What the amplitudes leave out.** No radiation pattern (every source radiates the same in all
  directions), no reflection or transmission coefficients (a fixed factor per phase stands in:
  PcP 0.2, ScS 0.5, PKPab/bc 0.4, PKPdf 0.3, PP 0.6, SS 0.6, SKS 0.4, pP 0.7, sP 0.5), no
  free-surface effect,
  no body-wave attenuation, no site effects. Magnitude scales every amplitude by 10^M.
- **Diffracted waves.** The tracer does not follow Pdiff and Sdiff. Past the direct wave's
  shadow edge (the farthest downgoing ray), they get the direct wave's amplitude 0.5° inside the
  edge, falling by a factor e every 8° (Pdiff) or 5° (Sdiff).
- **Surface-wave size.** Amplitude ∝ 1/√sin Δ (spreading over a sphere), × e^(−depth/60 km)
  (deep sources excite little surface wave), × e^(−path/15 000 km) (a stand-in for
  attenuation), with a gain chosen so a shallow earthquake's Rayleigh wave at 60° is about seven
  times its direct P on the vertical.
- **Source kind.** Each event kind has fixed relative P, S and surface-wave weights
  (earthquake 1 : 3 : 1; nuclear test 1 : 0.3 : 0.3; volcanic tremor 0.5 : 1 : 1.5;
  landslide 0.2 : 0.6 : 2; impact 0.5 : 0.5 : 3). Display choices, not source physics.
- **Components.** Each trace shows one way of shaking. Vertical: P-type arrivals and Rayleigh
  at full weight, SKS at 0.6, other S at 0.35, no Love. Transverse: S (SH) and Love at full
  weight, nothing from P, SKS or Rayleigh. Fixed weights, not a polarisation calculation.
- **Wavelets.** Body waves are Ricker wavelets of 1 s (P-type) or 2 s (S-type) period, each
  followed by a scattered coda: seeded pseudo-random wiggles, 0.4× the arrival, dying away over
  20 s (P) or 40 s (S). Surface waves are a 20 s carrier under a Gaussian envelope 30 s wide
  (one standard deviation). A faint background noise (0.3% of the trace's largest wave, about a
  6 s period) runs throughout. The coda and noise are fixed by a seed made from the event and
  station names, so a trace is the same every time.
- **Drawing.** Traces are sampled every 0.125 s and drawn as the smallest and largest value per
  screen column, so a wavelet much shorter than a column still shows at full height. Each trace
  is scaled to its own largest wave by default; the shared-scale option uses the largest wave of
  all shown traces instead, so distant stations look smaller.
- **Other events on a trace.** A trace shows the selected event's waves plus those of other
  listed events whose waves reach the station inside the window, but only up to 12 of them
  (largest first), and none more than 2 magnitude units smaller than the selected event (their
  waves would be under 1% of its size). An event's waves join a trace once its ray tubes are
  built, a moment after it appears.

## USGS events: source kinds and the small-event filter

- **Source kind from the USGS type** (decided by the user, 2026-10-14). USGS labels events
  with a free-text `type`. SEISMO maps it by likeness: every kind of blast (explosion, quarry,
  mining, chemical, nuclear, accidental, experimental) uses the explosion profile (the
  `NuclearTest` kind); landslide, rockslide, avalanche, collapse and mine collapse use the
  landslide profile; volcanic eruption the volcanic profile; meteorite the impact profile.
  Everything else (ice quake, rock burst, induced or triggered event, sonic boom, other event,
  unknown) is drawn as an earthquake. The UI always shows the USGS label, never the kind name.
- **Small events hidden by default.** The list, map and traces show events of magnitude 2.5
  and up (a slider changes it). USGS reports hundreds of magnitude 1–2 events a day; their waves
  are far too weak to cross the globe, yet the map would draw them at full size.
- **Magnitude types mixed.** USGS gives each event in its own magnitude type (ml, md, mb, mww,
  …). SEISMO uses the number as if it were moment magnitude for amplitudes and the map ring
  ordering, and shows the type beside it.
- **Deletions.** An event USGS deletes is noticed only while it is less than about 23 hours
  old (it disappears from the past-day feed). Older events, which come only from the month's
  significant events, are never removed in a session.
