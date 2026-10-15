# SEISMO — Global Earthquake Simulator

Desktop app in Rust. Simulates seismic wave propagation through a 1D Earth
(ak135, optionally perturbed by a 3D mantle model) for user-entered or live USGS events, rendered as a world map, a
vertical great-circle slice, and virtual seismograph traces, with a retro
green-phosphor CRT look.

## Workspace layout

```
seismo/               # = repo root
  Cargo.toml          # workspace
  crates/
    sim/              # pure simulation core + ray tracer. NO rendering, NO async, NO I/O.
    tables/           # travel-time table + ak135 velocity model + SP12RTS 3D mantle: format, parsing, interpolation
    feed/             # USGS fetch/cache/parse (tokio + reqwest + serde)
    render/           # wgpu passes: map, slice, traces, CRT post-process
    app/              # eframe (wgpu backend) + egui shell, owns the clock, wires everything together
  assets/
    ak135.tt          # travel-time tables (generated, committed)
    ak135_model.csv   # ak135 velocity model: depth_km, vp_km_s, vs_km_s, density_g_cm3 (generated, committed)
    sp12rts/          # SP12RTS 3D mantle model (authors' .sph files, unchanged; README has citation + checksums)
    stations.csv      # bundled real seismograph stations (GSN, IU + II; generated, committed, provenance in header)
    coastlines.txt    # Natural Earth 1:10m coastline, simplified (generated, committed, provenance in header)
    surface_dispersion.csv # Rayleigh + Love phase/group velocity vs period from ak135 (generated, committed, provenance in header)
    shaders/          # WGSL
  tools/
    gen_tables.py     # ObsPy script that produces both ak135 assets
    gen_stations.py   # FDSN station query that produces stations.csv (stdlib only)
    gen_coastlines.py # Natural Earth download + simplification that produces coastlines.txt (stdlib only)
    gen_dispersion.py # disba (Earth-flattened ak135) script that produces surface_dispersion.csv
    requirements.txt  # pinned obspy and disba versions
  docs/
    approximations.md
  CLAUDE.md
```

Dependency direction is strict: `app -> render -> sim`, `app -> feed -> sim`,
`sim -> tables`. `sim` re-exports `tables` (`pub use tables;`), so `render`
reaches table data as `sim::tables` without its own dependency on it. `sim`
and `tables` must compile with no GPU, no network, and
no std::thread, so they can be unit-tested in CI and reused by a future TUI.
`tables` parses from a byte slice (`from_bytes`); a path-based `load` is only a
thin wrapper. The app embeds the assets with `include_bytes!` so it starts with
no dependence on the working directory.

## Units and conventions (never deviate)

- Distance: kilometres. Angular distance: degrees of arc (0–180).
- Time: seconds as `f64`. Absolute times: `chrono::DateTime<Utc>`. Never local time.
- Coordinates: latitude/longitude in degrees, `f64`, lat in [-90, 90], lon in [-180, 180).
  Incoming lon = 180.0 is normalised to -180.0.
- Depth: km below the surface, positive down. Earth radius 6371.0 km.
  Negative depths (USGS reports some above sea level) are clamped to 0.
- Magnitude: moment magnitude Mw unless the source says otherwise; keep the
  source's magnitude type alongside the value.
- Azimuth: degrees clockwise from north.
- Great-circle distance: spherical Earth, geographic latitude used directly on
  the sphere (same convention as ObsPy `locations2degrees`), computed with the
  atan2 form so it is stable from 0° to 180°. Ellipticity is ignored (see
  `docs/approximations.md`).
- Sim time: the clock is absolute UTC. Per-event time `t = now - origin` in
  `f64` seconds, and may be negative (event not yet occurred). The GPU only
  ever receives per-event `t` (as f32), never absolute times.

## Architecture rules

- `sim` is deterministic: same inputs + same time → identical output. No RNG
  without an explicit seed passed in.
- The core query is `Simulation::state_at(now: DateTime<Utc>) -> Snapshot`.
  Rendering never advances a clock itself; `app` owns the clock and may scrub,
  pause, or run at any speed.
- Surface arrival times come from `tables`, plus a per-event 3D-mantle delay
  computed in `sim` along the 1D rays through the 3D model (first order,
  −∫ dlnV dt; rays not bent), scaled by a user strength (0 = 1D, 1 = the published model,
  default 1; above 1 the UI says EXAGGERATED). The delay is never "3D minus
  table": the tracer/table tolerance is as large as the signal. Wavefronts and ray paths
  *inside* the Earth (slice view, seismogram amplitudes) come from the ray
  tracer in `sim`, which shoots rays through the ak135 velocity model loaded by
  `tables`. A test checks ray-traced surface arrival times against the tables
  (P and S at 30°/60°/90°, within 1.0 s). Surface waves are the one exception
  to both: dispersive, from `assets/surface_dispersion.csv` (fundamental-mode
  Rayleigh and Love phase and group velocity, 8–200 s, Earth-flattened ak135,
  checked against Mineos). Seismogram trains are summed over period in
  `sim::surface` (phase velocity for the crests, group velocity for the energy,
  PREM Q, fixed source spectrum, faded in before the fastest energy); the map
  draws where those trains are (see Map); 1D only.
- Map: the shader receives the travel-time table as an RG32Float 2D array
  texture (R = arrival time, G = ray parameter; one array layer per phase,
  width = distance, height = depth) and evaluates arrival time per
  pixel with a manual 4-tap bilinear lookup (`textureLoad`, no hardware
  filtering — 32-bit float filtering is optional on integrated GPUs). Ring width in
  degrees comes from the ray parameter (dT/dΔ). 3D-mantle delays arrive as an
  R32Float 2D array texture (width = distance, height = azimuth, wrapping; two
  layers per GPU slot: P, S), slots assigned per event by `app` (never by the
  re-sorted map list), computed on a worker thread; 1D rings show until ready.
  Surface waves are bands from one R32Float 2D array texture shared by every
  event (width = apparent speed path/t in deg/s, height = path 0–360° in 2.5°
  steps, layer 0 Rayleigh, 1 Love; each row the train's envelope scaled to peak
  1), built once at start by `sim::surface` from the same synthesis as the
  traces; R1/L1 and the dimmer R2/L2; Rayleigh a smooth glow, Love pixel-grid
  dots. CPU code computes nothing per-pixel.
- Map events are culled before upload, capped (default 32): first events whose
  body waves are still on the globe, largest magnitude first; then, in the room
  left, events with only surface waves left (until the slowest train has been
  once round), largest first. Only the first group gets 3D-delay slots.
  Surface bands only for M5+ (`sim::SURFACE_MAP_MIN_MAG`, decided 2026-10-15);
  smaller events leave with their body waves.
- Slice: `sim` computes a ray fan per (source depth, phase) — per ray, not per
  pixel — cached, since a 1D model depends only on source depth. With the 3D
  mantle on, fans are bent through it in the plane of the cut
  (`sim::raytrace::bend`, seeded from the 1D fans) and cached per (event,
  azimuth, strength), built on the app's worker (fans in parallel; `sim` itself
  stays thread-free); 1D fans show until ready. The wavefront
  at time t is the line through the points at time t along neighbouring rays.
  The slice shows one half of the great circle (epicentre → antipode along the
  azimuth, slider 0–360°): forward fans up to the antipode plus back fans past
  it. With the 3D mantle on, a per-cut dlnV grid (1° × 25 km, computed on the
  worker, never per pixel) shades the mantle; greens only.
- The feed runs on a tokio runtime in its own thread and talks to the app via
  `crossbeam_channel`. The UI thread is synchronous.
- Every new feature touching physics gets a unit test in `sim` or `tables`.

## Physics reference

Earth model: ak135, optionally perturbed in the mantle by SP12RTS (Koelemeijer et al.
2016; shear and compressional dlnV, spherical harmonics to degree 12, 21 radial splines from
the model's Moho to its CMB, relative to PREM; degree 0 ignored; crust and core stay 1D).
The bundled files must stay byte-identical to the authors' download and keep their citation.
Key depths (km): Moho 35, 410, 660, CMB 2891.5
(radius 3479.5), ICB 5153.5 (radius 1217.5).

Table phases, in priority order for implementation: P, S, PcP, ScS, PKPab,
PKPbc, PKIKP (= PKPdf), PP, SS, SKS, Pdiff, Sdiff, then the depth phases pP, sP
(appended, so earlier layers keep their place; none from a source shallower than 5 km;
on the traces and in the slice, not on the map; 1D times, no 3D delay).
Shadow zones: direct P absent ~103°–142°; direct S absent beyond ~103° (core
is liquid). The exact cutoff depends on source depth and is whatever the
generated table says; tests never hardcode 103°.

Branches: each table cell holds the FIRST arrival of its named phase (so the
P/S triplications at ~15–30° from the 410/660 show only the earliest branch).
PKP is the exception and is stored as three separate branches; `gen_tables.py`
splits them by turning region (ab/bc in the outer core, df in the inner core) —
verify how ObsPy names them rather than assuming.

Tables: depth 0–700 km in 5 km steps × distance 0–180° in 0.25° steps ×
phase → arrival time (s) and ray parameter (s/deg, ObsPy
`ray_param_sec_degree`). Missing arrival = NaN on disk and `None` from the CPU
API. When uploaded to the GPU, NaN in both grids is replaced by the sentinel
-1.0 (WGSL has no reliable NaN test). Bilinear interpolation returns
`None` if any of the four corners is missing; tests must cover NaN edges.

Table file format (`ak135.tt`), all little-endian:
`b"SEISMOTT"` magic (8 bytes), `u32` format version, `u32` header length,
UTF-8 JSON header (model name, ObsPy version, depth axis start/step/count,
distance axis start/step/count, phase names in order), then per phase in
header order: the time grid then the ray-parameter grid, each `f32`
row-major `[depth][distance]`. Python writes it with numpy, Rust reads it with
`bytemuck`. Not bincode.

Sanity anchors (surface focus, approximate and NOT yet verified — the
authoritative values are printed by `tools/gen_tables.py` directly from ObsPy
and hardcoded into `tables/tests`): P at 30° ≈ 6.2 min, P at 90° ≈ 13.0 min,
S at 30° ≈ 11 min, S at 90° ≈ 23 min, PKIKP at 180° ≈ 20 min. A test that
disagrees with ObsPy by > 0.5 s fails. Compare at grid nodes, and off-node only
away from branch splits and triplications, where interpolation is not smooth.

Stations (decided 2026-10-14): both a bundled list of real stations
(`assets/stations.csv`) and user-placed ones (click the map). A few chosen
stations at a time are shown as stacked traces, sorted by distance, with a
vertical/transverse component switch and a per-trace or shared amplitude scale.

Seismogram synthesis: each arrival is a wavelet (Ricker) scaled by magnitude,
geometric spreading, and depth; traces are display-plausible, not
waveform-accurate. Say so in the UI. Geometric spreading comes from the ray
tracer (ray-tube width), not from double-differencing the table, which is too
noisy.

## Event types

`EventKind` enum: Earthquake, NuclearTest, VolcanicTremor, Landslide, Impact.
Each has a `SourceProfile` giving relative P/S/surface amplitude and depth
limits. Nuclear: shallow, P-dominant, weak S and surface waves. Impact: surface,
strong surface waves, optional tsunami flag (flag only — no tsunami model).
Keep the raw USGS `type` string on every feed event.

## Feed

- Sources: USGS GeoJSON summary feeds (`all_hour`, `all_day`,
  `significant_month`) polled every 60 s; FDSN event API for historical
  queries. Respect `If-Modified-Since`.
- Cache raw JSON to the platform data dir (`directories` crate). The app must
  start and function fully offline.
- Dedupe on the full USGS `ids` list (an event can carry several network ids
  and its preferred `id` can change when events merge), not on `id` alone.
  The version with the newer `updated` field wins. Handle events USGS deletes.
- USGS `type` → `EventKind` (decided 2026-10-14, `feed::kind_for`): blasts of every
  kind → `NuclearTest` (the explosion profile); slides, avalanches, collapses → `Landslide`;
  volcanic eruption → `VolcanicTremor`; meteorite → `Impact`; everything else →
  `Earthquake`. The UI shows the raw USGS type, never the kind name.
- Selection (decided 2026-10-14): until the user picks an event, the app picks one (the
  largest of the last 3 hours, once USGS has answered) and switches to each newly listed
  M6+ event. A user pick ends that. Selecting from the event list never moves the clock.
- Past-event search (decided 2026-10-15, `feed::search`): FDSN by date range + minimum
  magnitude + result cap (largest first), or one event by USGS code; no place-name search
  (FDSN has none). Results show in their own list, never on the map, and go through the same
  id dedupe as the live feeds (one handle per event). Picking a result keeps it (listed from
  then on, whatever its magnitude, saved to `saved.geojson` beside the feed cache and loaded at
  start, so it works offline; a Forget button drops it) and replays it from its origin — the
  one exception to "selecting never moves the clock". Unpicked results are not saved. Kept
  past events never count for the app's own pick or the M6+ follow. A result that is a
  built-in example selects the built-in instead. Searches run on the feed thread between
  polls; offline they fail at once with a message.
- Default minimum magnitude shown: 2.5, user-adjustable. Events are identified in the app by
  a stable handle (kept through new versions and merges) plus a revision that changes with
  the physics; caches key on both, never on list position.
- Incoming events carry their real origin time; the sim clock is UTC so waves
  from a quake 10 minutes ago appear already in flight. USGS typically lists a
  quake 5–20+ minutes after origin, so the UI offers "replay from origin" for
  any event.

## Rendering and look

- One palette: phosphor green `#33ff33` on `#050805`, with two dimmer greens for
  secondary data. No other hues. Accents are brightness, not colour. Phases are
  told apart by line style (solid/dashed/dotted) and labels as well as
  brightness.
- CRT pass is a single full-screen post-process over the scene only:
  scanlines, bloom (half-resolution blur), slight barrel distortion, phosphor
  persistence (decay buffer), subtle flicker. Every effect has a uniform so it
  can be disabled for debugging. egui controls are drawn on top, undistorted;
  mouse picking on the scene undoes the barrel distortion.
- Persistence decays per real second (frame-rate independent) and is cleared on
  scrub/seek. Wave trails themselves are drawn analytically from `t - T` so
  they are correct at any speed and when scrubbing backwards; the decay buffer
  is only the CRT look.
- Map: equirectangular, coastlines from a bundled simplified Natural Earth
  dataset (10m → simplified to < 50k vertices).
- Slice: great circle through epicenter at a chosen azimuth or toward a
  selected station. Draw Moho, 410, 660, CMB, ICB. Ray paths for selected
  phases.
- Target 60 fps at 1920×1080 on integrated graphics.

## Commands

```
cargo build --workspace
cargo test --workspace              # must pass before any commit
cargo clippy --workspace -- -D warnings
cargo run -p app
pip install -r tools/requirements.txt   # in a virtualenv outside the repo
python tools/gen_tables.py              # regenerates both ak135 assets (slow: minutes)
python tools/gen_dispersion.py          # regenerates surface_dispersion.csv (needs ak135_model.csv)
```

## Open questions (ask before deciding)

- None open. (Surface-wave dispersion was decided 2026-10-15: dispersive, on traces and map.)

## Do not

- Do not add velocity constants, travel-time formulas, or Earth-layer depths
  anywhere outside `tables` and `sim`.
- Do not introduce Bevy, an ECS, or a second UI framework.
- Do not block the UI thread on network I/O.
- Do not use local time anywhere.
- Do not rewrite the eframe/wgpu scaffold once it works; extend it.
- Do not silently approximate physics. If a phase or feature is approximated,
  document it in `docs/approximations.md`.
