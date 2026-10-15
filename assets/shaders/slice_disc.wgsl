// Slice pass, background: the half-disc cross-section of the Earth along a great circle, with
// the surface, layer boundaries, region fills, the 3D mantle's fast and slow rock,
// distance ticks and the source.
// The epicentre is at the left end of the flat edge, the antipode at the right end; epicentral
// distance delta runs over the arc. Each feature is painted in its theme colour at its level
// (display space) and the per-channel maximum is kept.

struct Uniforms {
    // viewport width_px, height_px, disc centre x_px, centre y_px (y down)
    view: vec4<f32>,
    // disc radius_px, source radius (fraction of Earth radius), CMB fraction, ICB fraction
    disc: vec4<f32>,
    // boundary radii as fractions of the Earth radius; < 0 = unused
    rings_a: vec4<f32>,
    rings_b: vec4<f32>,
    // boundary line brightness, region fill brightness, unused, unused
    style: vec4<f32>,
    // theme colours, rgb (display space) + unused
    background: vec4<f32>,
    // region fills: mantle, outer core, inner core, and their level weights (x, y, z)
    fill_mantle: vec4<f32>,
    fill_outer: vec4<f32>,
    fill_inner: vec4<f32>,
    fill_weights: vec4<f32>,
    boundary_colour: vec4<f32>,
    // the surface, ticks and the flat edge
    chrome_colour: vec4<f32>,
    source_colour: vec4<f32>,
    // rock shading colours, and its levels: base, fast gain, slow gain (see `Theme`)
    rock_fast: vec4<f32>,
    rock_slow: vec4<f32>,
    rock_levels: vec4<f32>,
    // rock shading on (0/1), display gain, Moho fraction (top of the shading; < 0 = surface),
    // bottom of the grid as a fraction of the Earth radius
    rock: vec4<f32>,
    // rock grid: delta step (deg), delta count, radius count (radius rows run evenly from the
    // bottom fraction to the surface), unused
    rock_axis: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// R32Float dlnV over the shown half: width = epicentral distance, height = radius.
@group(0) @binding(1) var rock: texture_2d<f32>;

// Manual bilinear lookup of the rock grid at radius fraction `r` and distance `delta` (deg).
fn rock_lookup(r: f32, delta: f32) -> f32 {
    let nd = u.rock_axis.y;
    let nr = u.rock_axis.z;
    let fu = clamp(delta / u.rock_axis.x, 0.0, nd - 1.0);
    let fv = clamp((r - u.rock.w) / (1.0 - u.rock.w) * (nr - 1.0), 0.0, nr - 1.0);
    let i = min(floor(fu), nd - 2.0);
    let j = min(floor(fv), nr - 2.0);
    let fx = fu - i;
    let fy = fv - j;
    let x = i32(i);
    let y = i32(j);
    let c00 = textureLoad(rock, vec2<i32>(x, y), 0).r;
    let c10 = textureLoad(rock, vec2<i32>(x + 1, y), 0).r;
    let c01 = textureLoad(rock, vec2<i32>(x, y + 1), 0).r;
    let c11 = textureLoad(rock, vec2<i32>(x + 1, y + 1), 0).r;
    return mix(mix(c00, c10, fx), mix(c01, c11, fx), fy);
}

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) uv: vec2<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    // One full-screen triangle (covers the viewport).
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    out.uv = vec2<f32>(xy.x, 1.0 - xy.y);
    return out;
}

const PI: f32 = 3.14159265358979;

// 1 on a line at distance `d` px, fading over one pixel beyond half width `w`.
fn line(d: f32, w: f32) -> f32 {
    return 1.0 - smoothstep(w, w + 1.0, abs(d));
}

fn ring(r_px: f32, frac: f32, w: f32) -> f32 {
    if (frac < 0.0) {
        return 0.0;
    }
    return line(r_px - frac * u.disc.x, w);
}

// A feature of colour `c` at brightness `level` over the background; above 1 it overshoots for
// the bloom.
fn paint(c: vec4<f32>, level: f32) -> vec3<f32> {
    return mix(u.background.rgb, c.rgb, clamp(level, 0.0, 1.0)) * max(1.0, level);
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let px = in.uv * u.view.xy;
    let rel = vec2<f32>(px.x - u.view.z, u.view.w - px.y); // y up
    let r_px = length(rel);
    let radius = u.disc.x;
    let r = r_px / radius;
    // Angle from the +x axis; delta = 0 at the left (source), 180 at the right (antipode).
    let delta = 180.0 - degrees(atan2(max(rel.y, 0.0), rel.x));

    var colour = u.background.rgb;
    let upper = rel.y >= -0.5;
    if (upper && r <= 1.0) {
        // Region fills: mantle, liquid outer core, inner core.
        var fill = u.fill_mantle;
        var weight = u.fill_weights.x;
        if (r < u.disc.z) {
            fill = u.fill_outer;
            weight = u.fill_weights.y;
        }
        if (r < u.disc.w) {
            fill = u.fill_inner;
            weight = u.fill_weights.z;
        }
        colour = max(colour, paint(fill, u.style.y * weight));
        // Fast and slow rock; +-1% spans the range.
        let top = select(1.0, u.rock.z, u.rock.z > 0.0);
        if (u.rock.x > 0.5 && r >= u.rock.w && r <= top) {
            let x = clamp(rock_lookup(r, delta) / 0.01, -1.0, 1.0);
            let fast = u.rock_levels.x + u.rock_levels.y * x;
            let slow = u.rock_levels.z * -x;
            colour = max(colour, paint(u.rock_fast, u.rock.y * max(fast, 0.0)));
            colour = max(colour, paint(u.rock_slow, u.rock.y * max(slow, 0.0)));
        }
    }
    if (upper) {
        // Boundaries; the core boundaries a little brighter.
        var b = 0.0;
        b = max(b, ring(r_px, u.rings_a.x, 0.4));
        b = max(b, ring(r_px, u.rings_a.y, 0.4));
        b = max(b, ring(r_px, u.rings_a.z, 0.4));
        b = max(b, ring(r_px, u.rings_a.w, 0.4));
        b = max(b, ring(r_px, u.rings_b.x, 0.4));
        b = max(b, ring(r_px, u.rings_b.y, 0.4));
        b = max(b, ring(r_px, u.rings_b.z, 0.4));
        b = max(b, ring(r_px, u.rings_b.w, 0.4));
        let core = max(ring(r_px, u.disc.z, 0.6), ring(r_px, u.disc.w, 0.6));
        colour = max(colour, paint(u.boundary_colour, u.style.x * max(0.6 * b, core)));
        // Surface.
        colour = max(colour, paint(u.chrome_colour, 0.75 * line(r_px - radius, 0.8)));
        // Distance ticks outside the surface: every 10 deg, longer every 30 deg.
        let step10 = abs(delta - 10.0 * round(delta / 10.0)) * PI / 180.0 * radius;
        let step30 = abs(delta - 30.0 * round(delta / 30.0)) * PI / 180.0 * radius;
        let out_px = r_px - radius;
        let t10 = line(step10, 0.4) * step(0.0, out_px) * step(out_px, 5.0);
        let t30 = line(step30, 0.6) * step(0.0, out_px) * step(out_px, 10.0);
        colour = max(colour, paint(u.chrome_colour, 0.6 * max(t10, t30)));
    }
    // The flat edge (a diameter through the centre): the vertical below the source on the
    // left, below the antipode on the right.
    if (abs(rel.x) <= radius) {
        colour = max(colour, paint(u.chrome_colour, 0.3 * line(rel.y, 0.4)));
    }
    // Source: a bright dot with a ring.
    let src = vec2<f32>(-u.disc.y * radius, 0.0);
    let ds = length(rel - src);
    let source = max(1.4 * (1.0 - smoothstep(2.5, 3.5, ds)), line(ds - 7.0, 0.5));
    colour = max(colour, paint(u.source_colour, source));

    return vec4<f32>(colour, 1.0);
}
