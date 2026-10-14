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
