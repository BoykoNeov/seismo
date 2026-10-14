// Slice pass, background: the half-disc cross-section of the Earth along a great circle, with
// the surface, layer boundaries, faint region fills, distance ticks and the source.
// The epicentre is at the left end of the flat edge, the antipode at the right end; epicentral
// distance delta runs over the arc. All values are display-space intensities.

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
    // phosphor colour rgb (display space), unused
    phosphor: vec4<f32>,
    // background rgb, unused
    background: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

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

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let px = in.uv * u.view.xy;
    let rel = vec2<f32>(px.x - u.view.z, u.view.w - px.y); // y up
    let r_px = length(rel);
    let radius = u.disc.x;
    let r = r_px / radius;
    // Angle from the +x axis; delta = 0 at the left (source), 180 at the right (antipode).
    let delta = 180.0 - degrees(atan2(max(rel.y, 0.0), rel.x));

    var level = 0.0;
    let upper = rel.y >= -0.5;
    if (upper && r <= 1.0) {
        // Faint region fills: mantle, liquid outer core (brightest), inner core.
        var fill = 0.35;
        if (r < u.disc.z) {
            fill = 1.0;
        }
        if (r < u.disc.w) {
            fill = 0.6;
        }
        level = u.style.y * fill;
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
        level = max(level, u.style.x * max(0.6 * b, core));
        // Surface.
        level = max(level, 0.75 * line(r_px - radius, 0.8));
        // Distance ticks outside the surface: every 10 deg, longer every 30 deg.
        let step10 = abs(delta - 10.0 * round(delta / 10.0)) * PI / 180.0 * radius;
        let step30 = abs(delta - 30.0 * round(delta / 30.0)) * PI / 180.0 * radius;
        let out_px = r_px - radius;
        let t10 = line(step10, 0.4) * step(0.0, out_px) * step(out_px, 5.0);
        let t30 = line(step30, 0.6) * step(0.0, out_px) * step(out_px, 10.0);
        level = max(level, 0.6 * max(t10, t30));
    }
    // The flat edge (a diameter through the centre): the vertical below the source on the
    // left, below the antipode on the right.
    if (abs(rel.x) <= radius) {
        level = max(level, 0.3 * line(rel.y, 0.4));
    }
    // Source: a bright dot with a ring.
    let src = vec2<f32>(-u.disc.y * radius, 0.0);
    let ds = length(rel - src);
    level = max(level, max(1.4 * (1.0 - smoothstep(2.5, 3.5, ds)), line(ds - 7.0, 0.5)));

    let colour = mix(u.background.rgb, u.phosphor.rgb, clamp(level, 0.0, 1.0)) * max(1.0, level);
    return vec4<f32>(colour, 1.0);
}
