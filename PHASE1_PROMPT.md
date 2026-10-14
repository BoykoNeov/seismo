# Phase 1 prompt — scaffold, real tables, P/S rings on the map, basic CRT

Paste the block below into Claude Code from the repo root (after placing
CLAUDE.md there).

Phase 1 burns down the riskiest part first: real ak135 travel times reaching
the screen. The full CRT look (bloom, barrel, flicker) is phase 2.

---

Read CLAUDE.md first and follow it strictly.

Set up phase 1 of SEISMO: the Cargo workspace, the real ak135 table pipeline,
an eframe window showing P and S wavefronts from the real tables on a map
grid, and a basic CRT pass. Work in this order and stop after each step to run
the checks listed under "Done when".

## Step 1 — Workspace

`git init` if the folder is not a repository yet. Create the workspace exactly
as laid out in CLAUDE.md with crates `sim`, `tables`, `feed`, `render`, `app`.
Pin a recent stable toolchain in `rust-toolchain.toml`. Add `.gitignore`,
`rustfmt.toml`, and a `justfile` (or Makefile) wrapping the commands in
CLAUDE.md. Each crate gets a one-line `lib.rs`/`main.rs` doc comment stating
its responsibility and its allowed dependencies.

Dependencies to add now: `eframe` (wgpu backend, not glow), `egui`,
`bytemuck`, `glam`, `chrono` (serde feature), `serde`, `serde_json`, `anyhow`,
`thiserror`, `tracing`, `tracing-subscriber`, `crossbeam-channel`,
`directories`. `tokio` and `reqwest` go only in `feed`. Versions must be
chosen together, not each "latest": pick the latest `eframe`, then pin `wgpu`
in `render` to exactly the version that eframe's `egui-wgpu` uses (check its
Cargo.toml), so `app` and `render` share one wgpu. No bincode.

## Step 2 — `sim` and `tables`

In `sim`:
- `Event { id: String, ids: Vec<String>, kind: EventKind, origin: DateTime<Utc>,
  lat: f64, lon: f64, depth_km: f64, magnitude: f64, mag_type: String }` with
  the `EventKind` enum from CLAUDE.md. Constructor clamps negative depth to 0
  and normalises lon 180 → -180.
- `great_circle_deg(lat1, lon1, lat2, lon2) -> f64` using the atan2 form from
  CLAUDE.md, with tests: antipodes = 180, same point = 0, equator 90° apart =
  90, nearly antipodal points stay accurate, and London (51.5074, -0.1278) ↔
  New York (40.7128, -74.0060) = 50.094° ± 0.001°.
- `Simulation { events: Vec<Event> }` and `Snapshot` with
  `state_at(&self, now: DateTime<Utc>) -> Snapshot`. For now the snapshot lists,
  per event, `t = now - origin` and the current P and S wavefront distance
  (from `tables`, first distance where arrival time ≥ t), or none.
- A `SimClock` in `app` (not `sim`): absolute UTC, play/pause/scrub/speed
  (0.1×–1000×), and a `seek` that the CRT pass can observe to clear
  persistence. Tested.

In `tables`:
- Implement the file format from CLAUDE.md exactly. `TravelTimeTable::from_bytes`,
  a thin `load(path)` wrapper, `::arrival(phase, depth_km, dist_deg) ->
  Option<f64>` and `::ray_param(...)` (s/deg), bilinear, `None` if any corner
  is NaN. A `gpu_grid(phase) -> Vec<[f32; 2]>` helper that interleaves time and
  ray parameter with NaN replaced by -1.0 in both. `sim` re-exports `tables`
  so `render` uses it as `sim::tables`.
- Load `ak135_model.csv` into an `EarthModel` (depth, vp, vs, density) for the
  future ray tracer. Parse only; no ray tracing in phase 1.
- Tests with a tiny hand-built fixture (2 depths × 3 distances × 1 phase,
  written by the test in the real format): interpolation, NaN corners, wrong
  magic, wrong version.

`tools/gen_tables.py` + `tools/requirements.txt` (pin obspy):
- Writes `assets/ak135.tt` with all CLAUDE.md phases (first arrival per phase,
  PKP split into ab/bc/df — check how ObsPy names the branches and say what you
  found) and `assets/ak135_model.csv` from the same ObsPy ak135 model.
- Uses multiprocessing; it will still take minutes. Records the ObsPy version
  in the header.
- Prints a block of anchor values computed directly by ObsPy (P, S at 30°/90°,
  PKIKP at 180°, surface focus, plus a few at depth 100 km and off-node
  distances away from triplications). Hardcode these into `tables/tests` and
  check the real table against them within 0.5 s.
- Create the virtualenv outside the repo (on this machine: under
  `W:\temp\claude`).

## Step 3 — Window and render loop

In `app`: eframe app titled "SEISMO" on the wgpu backend. Log with `tracing`.
The assets are embedded with `include_bytes!`. Each frame: update clock →
build egui UI → render the scene into an offscreen texture via `render` → CRT
pass into a second texture → show it in the central panel. egui stays on top,
undistorted. Resizing must work and keep the map's 2:1 aspect ratio.

egui UI: a side panel with the sim clock (UTC time display, play/pause, speed
slider 0.1×–1000×, scrub slider, "replay from origin"), and a "CRT"
collapsible with a checkbox and intensity slider per effect present.

## Step 4 — Map pass and basic CRT pass

In `render`:
- `MapPass`: equirectangular lat/lon grid (no coastlines yet) and the epicentre
  of a hardcoded test event: 2011 Tōhoku, 38.297°N 142.373°E, 29 km, Mw 9.1,
  origin 2011-03-11T05:46:24Z. The table is uploaded as an RG32Float 2D array
  texture (R = time, G = ray parameter; one layer per phase; sentinel -1.0). The fragment shader computes
  distance to the epicentre per pixel and draws P and S wavefronts with a manual
  4-tap bilinear lookup; ring width from the ray parameter; a short analytic
  fading trail behind each front from `t - T`. P solid, S dashed. The event list
  is a uniform/storage array sized for the CLAUDE.md cap even though it holds
  one event now.
- `CrtPass`: full-screen WGSL post-process with a master enable, scanlines, and
  phosphor persistence (ping-pong texture, decay per real second, cleared on
  seek). Leave uniform slots for bloom, barrel and flicker, unimplemented.

## Done when

- `cargo test --workspace` and `cargo clippy --workspace -- -D warnings` pass,
  including the real-table anchor tests against ObsPy values.
- `cargo run -p app` opens a window showing the Tōhoku P and S fronts
  expanding at a stable 60 fps on integrated graphics. The P front stops at
  ~103° (only direct P is drawn; core phases come later) and S stops at
  ~103° too. Check by eye; read the actual cutoffs for a 29 km source from the
  generated table and write them into `docs/approximations.md`. No test
  asserts 103°. Speed follows the slider; scrubbing backwards moves the
  fronts back with no smear.
- Toggling each CRT effect visibly changes the output.
- Resizing the window does not crash or distort the aspect ratio.
- `docs/approximations.md` exists with a heading and these entries: spherical
  Earth / ellipticity ignored; first arrival only per phase (PKP as three
  branches); surface waves at constant group velocity, not dispersive.
- Commit with message "phase 1: workspace, ak135 tables, P/S rings, basic CRT".

Before writing code, list any decision you have to make that CLAUDE.md does
not cover (e.g. exact eframe/wgpu versions, how eframe exposes the wgpu
device for custom passes) and state your choice in one line each. Then proceed
without waiting.
