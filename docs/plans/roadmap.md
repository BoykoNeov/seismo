# Roadmap: what more can be done

Written 2026-10-15, after the terminal version (commit 29c8047). A survey of what the project
could still optimise or add, grouped by kind and ordered within each group by value for the
effort. Nothing here is decided; each item is a candidate for a future batch. Items marked
**(debt)** close something the project already promised; the rest expand it.

State at the time of writing: about 19,600 lines of Rust in 7 crates; one doc
(`docs/approximations.md`); no README, no CI workflow, no license; the one deferred
done-check is "60 fps at 1920×1080 on integrated graphics" (dropped from phase 1 on
2026-10-14, still the long-term target in `CLAUDE.md`).

## 1. First: cheap, high value

### 1.1 Performance on integrated graphics (debt)

Last measurement (2026-10-15, release build, `WGPU_POWER_PREF=low`, AMD Radeon, Vulkan):
44 fps without surface-wave bands, 40 with, at the default 2400×1350 window. Below target, and
never measured at the actual 1920×1080 target because the app has no window-size flag.

- Add `--size WxH` (points or pixels, say which) so a run at 1920×1080 is reproducible.
- Measure at 1920×1080 with and without `--surface`, `--crt off`, and with 1 / 8 / 32 events.
- Profile the suspects in order: the map pass (per-pixel bilinear travel-time lookup over up
  to 32 events, plus the 3D-delay and surface-band textures), the bloom blur (half-resolution
  already; check threshold pass count), coastlines (235k line vertices rebuilt per map resize,
  ~1.5 ms on the RTX 5090 in debug: a level-of-detail pick by map width would cut it).
- Done when: 60 fps at 1920×1080 on the iGPU with the default settings and the live list, or a
  written reason why not and which setting gets there.

**Verdict (2026-10-15): not 60 fps; measured and stopped there at the user's call.**
Tools added: `--size WxH` (window inner size in physical pixels, controls included),
`--bench <s>` (no vsync; frame-time mean/median/p95 after a warm-up, then the scene alone
offscreen and each pass's CPU encode and GPU time; `--bench-warmup`, `--bench-out <file>`),
`--map-cap <n>` and `--paused`. At a 1920×1080 window the scene is 1530×1080 px (map
1080×540). Release build, `--offline`, busy replay `--event us6000u2hv --start-offset 300`
(6 events on the map, 5 with 3D delays), AMD Radeon iGPU (Ryzen 7 7800X3D, 2 CUs), Vulkan:

| Setup | fps | Scene GPU (ms) |
|---|---|---|
| iGPU, no events on the map | 48–55 | about 14.5 |
| iGPU, busy replay, default | 34–43 | about 15.6 |
| iGPU, busy replay, `--crt off` | 48 | about 11 |
| RTX 5090, busy replay, default | 70 | about 0.5 |

GPU time per pass on the iGPU: map about 1.3 ms plus about 0.9 ms per event; post-process
about 7 ms (bloom about 4 of it, persistence copy and display about 2.4, the other CRT effects
about 0.5); slice about 2 ms GPU plus 2 ms CPU building its lines; traces under 0.5 ms.

Two separate limits, so no single setting reaches 60 fps on this iGPU:
- **GPU:** the busy scene alone is close to the 16.7 ms budget before egui and presenting.
  `--crt off` removes about 5 ms; the map's cost grows with the number of events.
- **CPU:** about 10 ms per frame on any GPU (the RTX 5090 run is capped at 70 fps by it):
  egui, the slice's line geometry rebuilt every frame, trace columns. This is the next target
  if the work resumes: cache the slice vertices between frames when nothing moved, and profile
  the rest of `ui`.
- **Caveat:** the displays hang off the RTX 5090, so every iGPU frame is copied across cards
  before it is shown; a laptop whose iGPU drives the screen would not pay that. Frame times
  here look like CPU plus GPU rather than the larger of the two, which fits that copy but is
  not proven.

Already changed (picture unchanged, at most 1/255 in a few dozen bytes): the map shader takes
each event's sin/cos of latitude and longitude from the CPU and computes the pixel's once, so
the per-event loop has no trigonometry but two `atan2` (and no `pow` of negative numbers); the
bloom's Gaussian weights come from the CPU. No measurable speed-up within the run-to-run noise
(fps swings 34–43 between identical runs; compare per-pass GPU medians, not fps). Not tried:
splitting bloom into threshold-downsample plus two bilinear-paired blur passes (about half the
texture reads, changes pixels slightly), skipping the persistence copy in the modern look.

### 1.2 Split `crates/app/src/main.rs`

Over 2,000 lines in one file: clock, controls, event list, made-event form, station picking,
screenshots, the worker. It works; this is only about keeping it changeable. Candidate split:
`controls.rs`, `event_list.rs`, `make_form.rs`, `stations.rs`, `capture.rs`, with `main.rs`
left as the eframe shell and the frame loop. No behaviour change; the UI Automation and
`--capture-ui` screenshot tests must pass unchanged.

### 1.3 Repo health (debt)

- README: what it is, a screenshot of each look and of the terminal version, how to build,
  the command-line flags, the physics sources (ak135, SP12RTS, Natural Earth, GSN) with
  citations.
- CI: a GitHub Actions workflow running `cargo test --workspace` and
  `cargo clippy --workspace -- -D warnings` on Linux and Windows. `sim` and `tables` were
  designed to run headless for exactly this, and nothing runs them yet. The GPU crates compile
  without a GPU; only the screenshot tests need one and should be skipped in CI.
- License: still undecided. Do not add one without the user choosing (see memory note).

## 2. Physics that could be made more honest

Each of these is a deliberate shortcut listed in `docs/approximations.md`. Fixing one means
updating that doc and adding a `sim`/`tables` test.

### 2.1 Radiation pattern

Today every source radiates equally in all directions. USGS publishes a focal mechanism (the
fault's orientation: strike, dip, rake) for most M5.5+ events as a product on the event's
detail page. Using it, the P and S amplitudes on each station's trace would depend on the
direction from the source, which is the main reason real seismograms differ from station to
station. Work: parse the `focal-mechanism` / `moment-tensor` product in `feed`, carry it on the
event, apply the standard double-couple pattern (Aki & Richards) in `sim::seismogram` for P,
SV and SH; fall back to the current isotropic weights when absent. Medium effort.

### 2.2 Ellipticity correction

The Earth is flattened; ignoring it shifts teleseismic times by up to about ±1 s. The standard
correction (Kennett & Gudmundsson 1996, as in ObsPy/TauP `ellipticity`) is a small table per
phase. Work: generate the coefficients in `tools/gen_tables.py`, apply in `tables` lookup given
source latitude, distance and azimuth. Small effort; the map shader would need the three
coefficients per phase as uniforms or a tiny texture.

### 2.3 Later arrivals (triplications and multiple branches)

Each table cell keeps only the first arrival of its phase, so the P and S triplications at
15–30° (two or three pulses in a row from the 410 and 660 km boundaries) and the later PP/SS
branches never show on the traces. Work: let `gen_tables.py` store up to N arrivals per cell
(format version bump), `tables` returns a small list, traces and slice draw all of them; the
map keeps drawing the first. Moderate effort, mostly in the table format and its tests.

### 2.4 More phases

PcS, ScP, SKKS, PKKP, PKiKP, SP, PS. Each is cheap once named in `gen_tables.py` (phase order
is append-only so earlier layers keep their place). They show on the traces and in the slice;
the map stays P, S and surface bands. Check ObsPy's naming for each before assuming.

### 2.5 A real crust

ak135 has a flat 35 km crust everywhere. CRUST1.0 (1° grid, ~1 MB as a compact asset) would
give correct local-station times and let the slice show thin oceanic against thick continental
crust. Work: a new asset plus generator script, a crustal time correction per station and per
source in `sim`, slice shading of the Moho depth. Moderate effort; the map would keep the 1D
crust (per-pixel crust corrections are a texture of their own, defer).

## 3. Features that expand what the app is for

### 3.1 Real recorded waveforms beside the synthetic ones — recommended

IRIS's FDSN `dataselect` service returns actual seismograms (miniSEED) for the bundled GSN
stations for any time window. Drawing the real trace under SEISMO's synthetic one turns the
traces from "display-plausible" into a comparison the user can learn from, and is the largest
step up in what the app is worth. Work: miniSEED decoding (a small crate exists, or write the
Steim1/2 decoder: ~300 lines), a fetch on the feed thread with the same cache and offline
rules as USGS, a second line per trace in `render::traces` and the TUI, instrument response
ignored (say so in the UI and in `docs/approximations.md`). Medium effort.

### 3.2 Sound

A trace sped up about 1000× is audible; "listen to the earthquake" is a known outreach trick
and fits the CRT look. Work: resample the selected trace, play through `cpal` or `rodio`
(one new dependency in `app` only), a play button and a speed choice. Small effort.

### 3.3 Export and reproducible views

PNG of the current frame already exists for screenshots; add a GIF/PNG-sequence of the map
over a time span, CSV of a trace, and a "copy command line" button that produces the
`--event`/`--after`/`--look`/`--size` invocation that reproduces the view.

### 3.4 Globe projection

An orthographic (globe-looking) map as an alternative to equirectangular, which distorts
wavefronts near the poles. The shader already computes arrival time per pixel from
(lat, lon); only the pixel → (lat, lon) mapping and the coastline projection change. Moderate
effort; CPU code must still compute nothing per pixel.

### 3.5 3D mantle in the terminal version

`seismo-tui` is 1D only. The 3D-delay computation already runs on a worker thread in `app`;
moving that worker into `session` would let the TUI nudge its map fronts and bend its slice
rays the same way. Mostly plumbing. The `sim` crate stays thread-free.

### 3.6 Replay the day / aftershock view

A mode that runs the last 24 h of USGS events at high speed from the start of the day, and a
list filter for aftershocks of the selected event (within a radius and after its origin).

### 3.7 Tsunami arrival rings

The impact kind carries a tsunami flag with no model. For offshore M7+ events, long-wave speed
√(g·depth) over bathymetry gives arrival rings. Needs a bathymetry asset (ETOPO at 1° or
0.5°, a few MB) and a shortest-path (fast-marching) solver on the worker thread. Larger
effort; the first real new physics outside `tables`/`sim`'s current scope, so it would go in
`sim` with its own test and approximations entry.

## 4. Quality of life

- Station search by name or code; "nearest N stations to the epicentre" auto-selection.
- A settings file (beside `made.json`) so look, slider positions, chosen stations and the
  minimum magnitude persist between runs. Today only made events and kept past events do.
- Accessibility: accesskit is enabled but nothing has been checked with a screen reader;
  keyboard-only control of the clock (play/pause, step, speed) and the event list.
- A `--size` flag (see 1.1) also serves screenshots and the README.

## Suggested next batch

1. ~~Section 1.1, the performance pass with `--size`, so the 60 fps target gets a real
   verdict.~~ Done 2026-10-15: measured, not 60 fps; see the verdict there.
2. Section 1.3, README and CI (license stays the user's call).
3. Section 3.1, real waveforms from IRIS.

The first two close open debts; the third is the biggest gain in what the app is for. Items
in section 2 are independent of each other and of section 3 and can be slotted in any order;
2.2 (ellipticity) and 2.4 (more phases) are the smallest.
