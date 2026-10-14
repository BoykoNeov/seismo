// Map pass: equirectangular world grid, epicentres, and per-pixel wavefronts from the
// travel-time table, nudged per event by 3D-mantle delays. All values written are display-space intensities (gamma-encoded green),
// possibly > 1 for later bloom.

const MAX_EVENTS: u32 = 32u;
const MISSING: f32 = -1.0;
// Ray parameters below this (s/deg) are treated as this when converting time to distance:
// p reaches 0 at the epicentre of a buried source and for PKIKP at 180 deg.
const MIN_RAY_PARAM: f32 = 0.5;

struct MapEvent {
    // lat_deg, lon_deg, depth_km, t_s (seconds since origin; < 0 = not yet)
    a: vec4<f32>,
    // magnitude, first delay layer (P; S is the next; < 0 = none), max |delay| (s), unused
    b: vec4<f32>,
};

struct Uniforms {
    // width_px, height_px, event_count, unused
    view: vec4<f32>,
    // depth_start, depth_step, depth_count, unused
    depth_axis: vec4<f32>,
    // dist_start, dist_step, dist_count, unused
    dist_axis: vec4<f32>,
    // P layer, S layer, (later phases), as f32 indices; < 0 = absent
    layers: vec4<f32>,
    // front line width (px), trail length (s), trail strength, grid brightness
    style: vec4<f32>,
    // phosphor colour rgb (display space), background level
    phosphor: vec4<f32>,
    // background rgb, unused
    background: vec4<f32>,
    // slice great circle: epicentre lat_deg, lon_deg, azimuth_deg, enabled (0/1)
    slice: vec4<f32>,
    // delay grid: dist_step_deg, dist_count, az_step_deg, az_count
    delay_axis: vec4<f32>,
    // 3D-mantle strength (0 = 1D, 1 = real), unused, unused, unused
    mantle: vec4<f32>,
    events: array<MapEvent, MAX_EVENTS>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// RG32Float, R = arrival time (s), G = ray parameter (s/deg); one layer per phase;
// width = distance, height = depth; MISSING where there is no arrival.
@group(0) @binding(1) var tt: texture_2d_array<f32>;
// R32Float, delay (s, at strength 1) of each event's P and S; width = distance, height =
// azimuth (clockwise from north, wrapping); 0 where the phase has no ray.
@group(0) @binding(2) var delays: texture_2d_array<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One full-screen triangle.
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return out;
}

const PI: f32 = 3.14159265358979;
const DEG: f32 = 0.0174532925199433;

// Great-circle distance (deg) and azimuth from point 1 to point 2 (radians), atan2 form.
fn distance_azimuth(lat1: f32, lon1: f32, lat2: f32, lon2: f32) -> vec2<f32> {
    let p1 = lat1 * DEG;
    let p2 = lat2 * DEG;
    let dl = (lon2 - lon1) * DEG;
    let y = sqrt(pow(cos(p2) * sin(dl), 2.0) + pow(cos(p1) * sin(p2) - sin(p1) * cos(p2) * cos(dl), 2.0));
    let x = sin(p1) * sin(p2) + cos(p1) * cos(p2) * cos(dl);
    let az = atan2(sin(dl) * cos(p2), cos(p1) * sin(p2) - sin(p1) * cos(p2) * cos(dl));
    return vec2<f32>(atan2(y, x) / DEG, az);
}

fn tex(layer: i32, x: i32, y: i32) -> vec2<f32> {
    return textureLoad(tt, vec2<i32>(x, y), layer, 0).rg;
}

// Manual 4-tap bilinear lookup of (time, ray parameter). Either component is MISSING when any
// of its four corners is missing or the point is off the grid.
fn tt_lookup(layer: i32, depth_km: f32, dist_deg: f32) -> vec2<f32> {
    let nz = u.depth_axis.z;
    let nx = u.dist_axis.z;
    let fu = (dist_deg - u.dist_axis.x) / u.dist_axis.y;
    let fv = (depth_km - u.depth_axis.x) / u.depth_axis.y;
    if (layer < 0 || fu < 0.0 || fu > nx - 1.0 || fv < 0.0 || fv > nz - 1.0) {
        return vec2<f32>(MISSING);
    }
    let i = min(floor(fu), nx - 2.0);
    let j = min(floor(fv), nz - 2.0);
    let fx = fu - i;
    let fy = fv - j;
    let x = i32(i);
    let y = i32(j);
    let c00 = tex(layer, x, y);
    let c10 = tex(layer, x + 1, y);
    let c01 = tex(layer, x, y + 1);
    let c11 = tex(layer, x + 1, y + 1);
    let v = mix(mix(c00, c10, fx), mix(c01, c11, fx), fy);
    let lo = min(min(c00, c10), min(c01, c11));
    // The sentinel is -1 and real values are >= 0, so any corner below -0.5 is missing.
    return select(v, vec2<f32>(MISSING), lo < vec2<f32>(-0.5));
}

// Manual bilinear lookup of a delay (s), wrapping in azimuth. `az` in radians from
// `distance_azimuth`.
fn delay_lookup(layer: i32, dist_deg: f32, az: f32) -> f32 {
    let nd = u.delay_axis.y;
    let na = u.delay_axis.w;
    let fu = clamp(dist_deg / u.delay_axis.x, 0.0, nd - 1.0);
    let az_deg = select(az / DEG, az / DEG + 360.0, az < 0.0);
    let fv = clamp(az_deg / u.delay_axis.z, 0.0, na);
    let i = min(floor(fu), nd - 2.0);
    let j = min(floor(fv), na - 1.0);
    let fx = fu - i;
    let fy = fv - j;
    let x = i32(i);
    let y0 = i32(j);
    let y1 = (y0 + 1) % i32(na);
    let c00 = textureLoad(delays, vec2<i32>(x, y0), layer, 0).r;
    let c10 = textureLoad(delays, vec2<i32>(x + 1, y0), layer, 0).r;
    let c01 = textureLoad(delays, vec2<i32>(x, y1), layer, 0).r;
    let c11 = textureLoad(delays, vec2<i32>(x + 1, y1), layer, 0).r;
    return mix(mix(c00, c10, fx), mix(c01, c11, fx), fy);
}

// Brightness of one phase's front and trail at this pixel.
// `dlayer` >= 0 nudges the arrival time by that delay layer, scaled by the mantle strength;
// `max_delay` bounds it, so the lookup is skipped where it cannot matter.
// `dash` > 0 makes the front dashed with that period (deg of arc along the ring).
fn phase_glow(layer: i32, dlayer: i32, max_delay: f32, depth: f32, dist: f32, az: f32, t: f32,
              px_deg: f32, dash: f32) -> f32 {
    let tp = tt_lookup(layer, depth, dist);
    if (tp.x < 0.0 || tp.y < 0.0 || t < 0.0) {
        return 0.0;
    }
    let p = max(tp.y, MIN_RAY_PARAM);
    let half_w = 0.5 * u.style.x * px_deg;
    var arrival = tp.x;
    let strength = u.mantle.x;
    if (dlayer >= 0 && strength != 0.0) {
        // Past ~8 trail lengths behind the front the trail is invisible either way.
        let margin = abs(strength) * max_delay + (half_w + px_deg) * p;
        let dt0 = t - arrival;
        if (dt0 > -margin && dt0 < margin + 8.0 * u.style.y) {
            arrival = arrival + strength * delay_lookup(dlayer, dist, az);
        }
    }
    // Seconds since the front passed this point; converted to degrees behind the front with
    // the ray parameter (dT/dDelta), so the line has a fixed width in screen pixels.
    let dt = t - arrival;
    let behind_deg = dt / p;
    var front = 1.0 - smoothstep(half_w, half_w + px_deg, abs(behind_deg));
    if (dash > 0.0) {
        let along = az / DEG * sin(dist * DEG);
        let on = step(fract(along / dash), 0.6);
        front = front * on;
    }
    var trail = 0.0;
    if (dt > 0.0) {
        trail = u.style.z * exp(-dt / max(u.style.y, 1e-3));
    }
    return max(front, trail);
}

fn grid_line(coord: f32, spacing: f32, px: f32) -> f32 {
    let d = abs(fract(coord / spacing + 0.5) - 0.5) * spacing;
    return 1.0 - smoothstep(0.0, px, d);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let lon = -180.0 + 360.0 * in.uv.x;
    let lat = 90.0 - 180.0 * in.uv.y;
    // Size of one pixel in degrees of longitude/latitude on the map.
    let px = vec2<f32>(360.0 / u.view.x, 180.0 / u.view.y);

    var level = 0.0;

    // Graticule: 30 deg majors, 10 deg minors, equator brighter.
    let major = max(grid_line(lon, 30.0, px.x), grid_line(lat, 30.0, px.y));
    let minor = max(grid_line(lon, 10.0, px.x), grid_line(lat, 10.0, px.y));
    let equator = 1.0 - smoothstep(0.0, px.y * 1.5, abs(lat));
    level = max(level, u.style.w * max(max(0.35 * minor, 0.7 * major), equator));

    let n = u32(u.view.z);
    for (var k = 0u; k < min(n, MAX_EVENTS); k = k + 1u) {
        let e = u.events[k];
        let da = distance_azimuth(e.a.x, e.a.y, lat, lon);
        let dist = da.x;
        let az = da.y;
        // A pixel's size in degrees of arc, roughly, at this point on the map.
        let px_deg = max(px.y, px.x * cos(lat * DEG));
        let dl = i32(e.b.y);
        let dl_s = select(-1, dl + 1, dl >= 0);
        let p = phase_glow(i32(u.layers.x), dl, e.b.z, e.a.z, dist, az, e.a.w, px_deg, 0.0);
        let s = phase_glow(i32(u.layers.y), dl_s, e.b.z, e.a.z, dist, az, e.a.w, px_deg, 3.0);
        level = max(level, max(p, 0.8 * s));
        // Epicentre: bright dot plus a ring.
        let r = dist / px_deg;
        let dot = 1.0 - smoothstep(2.0, 3.0, r);
        let ring = 1.0 - smoothstep(0.5, 1.5, abs(r - 7.0));
        level = max(level, max(1.4 * dot, ring));
    }

    // The slice's great circle, epicentre to antipode: dotted, brighter every 30 deg.
    if (u.slice.w > 0.5) {
        let da = distance_azimuth(u.slice.x, u.slice.y, lat, lon);
        let px_deg = max(px.y, px.x * cos(lat * DEG));
        let rel = da.y - u.slice.z * DEG;
        let cross = abs(asin(clamp(sin(da.x * DEG) * sin(rel), -1.0, 1.0))) / DEG;
        if (cos(rel) >= 0.0 || da.x > 179.0) {
            let on_line = 1.0 - smoothstep(0.5 * px_deg, 1.5 * px_deg, cross);
            let dots = step(fract(da.x / 1.5), 0.5);
            let to_tick = abs(da.x - 30.0 * round(da.x / 30.0));
            let tick = (1.0 - smoothstep(2.0 * px_deg, 3.0 * px_deg, to_tick))
                * (1.0 - smoothstep(3.0 * px_deg, 4.0 * px_deg, cross));
            level = max(level, max(0.55 * on_line * dots, 0.9 * tick));
        }
    }

    let colour = mix(u.background.rgb, u.phosphor.rgb, clamp(level, 0.0, 1.0)) * max(1.0, level);
    return vec4<f32>(colour, 1.0);
}
