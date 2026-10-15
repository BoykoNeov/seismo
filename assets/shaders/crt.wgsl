// CRT post-process over the scene only, in four full-screen passes:
//   fs_persist  (full res)  phosphor persistence: max(scene, previous * decay), scene space
//   fs_bloom_h  (half res)  bright parts of the persisted image, 2x2 averaged, blurred across
//   fs_bloom_v  (half res)  the same blurred down
//   fs_display  (full res)  warm-up, barrel distortion, + bloom, hot spots, scanlines, noise,
//                           hum bar, vignette, rounded corners, flicker -> displayed image
// The master switch and every effect's enable flag are applied on the CPU: a disabled effect
// arrives here as a neutral value (decay 0, bloom 0, barrel 0, flicker gain 1, warm-up 1,
// every amount 0).
// Each pass binds only its own slots; binding numbers are unique across the module.

struct Uniforms {
    // width_px, height_px (scene and display), half-res width_px, half-res height_px
    view: vec4<f32>,
    // scanline enable, scanline intensity [0,1], scanline period (px), unused
    scanlines: vec4<f32>,
    // persistence enable, decay factor applied to the previous buffer this frame
    // (exp(-dt_real / tau), 0 to clear), unused, unused
    persistence: vec4<f32>,
    // bloom intensity (0 = off), bright threshold, blur sigma (half-res px; the weights come
    // from it, in bloom_weights), unused
    bloom: vec4<f32>,
    // barrel amount k (0 = off), unused, unused, unused
    barrel: vec4<f32>,
    // flicker brightness gain this frame (1 = off), unused, unused, unused
    flicker: vec4<f32>,
    // vignette amount (0 = off), corner radius (px, 0 = square), noise amount (0 = off),
    // hot-spot whitening (0 = off)
    glass: vec4<f32>,
    // hum bar centre (display v, may be off-screen), hum bar gain (0 = off),
    // warm-up progress [0, 1] (1 = on and steady), scanline fill by bright beam [0, 1]
    signal: vec4<f32>,
    // noise colour (the phosphor), noise frame seed
    tint: vec4<f32>,
    // bloom blur weights for taps 0..=BLOOM_TAPS, four to a row, normalised over the whole
    // symmetric kernel (from the CPU, so no exp per tap)
    bloom_weights: array<vec4<f32>, 3>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
// fs_persist
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var previous: texture_2d<f32>;
// fs_bloom_h and fs_display
@group(0) @binding(3) var persisted: texture_2d<f32>;
@group(0) @binding(4) var linear: sampler;
// fs_bloom_v
@group(0) @binding(5) var bloom_h: texture_2d<f32>;
// fs_display
@group(0) @binding(6) var bloom: texture_2d<f32>;

struct VsOut {
    @builtin(position) pos: vec4<f32>,
};

@vertex
fn vs_main(@builtin(vertex_index) i: u32) -> VsOut {
    let xy = vec2<f32>(f32((i << 1u) & 2u), f32(i & 2u));
    var out: VsOut;
    out.pos = vec4<f32>(xy * 2.0 - 1.0, 0.0, 1.0);
    return out;
}

@fragment
fn fs_persist(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let current = textureLoad(scene, p, 0).rgb;
    if (u.persistence.x < 0.5) {
        return vec4<f32>(current, 1.0);
    }
    let prev = textureLoad(previous, p, 0).rgb;
    return vec4<f32>(max(current, prev * u.persistence.y), 1.0);
}

// Must match `BLOOM_TAPS` in crt.rs.
const BLOOM_TAPS: i32 = 10;

fn gauss(i: i32) -> f32 {
    let k = abs(i);
    return u.bloom_weights[k / 4][k % 4];
}

@fragment
fn fs_bloom_h(in: VsOut) -> @location(0) vec4<f32> {
    // A half-res pixel centre sits on the corner of 2x2 full-res pixels, so one bilinear
    // sample averages them; neighbours are 2 full-res pixels apart.
    let full = u.view.xy;
    let uv = in.pos.xy * 2.0 / full;
    let step = vec2<f32>(2.0 / full.x, 0.0);
    var sum = vec3<f32>(0.0);
    for (var i = -BLOOM_TAPS; i <= BLOOM_TAPS; i++) {
        let c = textureSampleLevel(persisted, linear, uv + step * f32(i), 0.0).rgb;
        sum += gauss(i) * max(c - vec3<f32>(u.bloom.y), vec3<f32>(0.0));
    }
    return vec4<f32>(sum, 1.0);
}

@fragment
fn fs_bloom_v(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let last = i32(u.view.w) - 1;
    var sum = vec3<f32>(0.0);
    for (var i = -BLOOM_TAPS; i <= BLOOM_TAPS; i++) {
        let q = vec2<i32>(p.x, clamp(p.y + i, 0, last));
        sum += gauss(i) * textureLoad(bloom_h, q, 0).rgb;
    }
    return vec4<f32>(sum, 1.0);
}

// Display uv -> scene uv. Must match `CrtSettings::display_to_scene` in crt.rs: with c the
// position in [-1, 1] on each axis, c' = c (1 + k |c|^2) / (1 + k), so the middles of the edges
// stay put and the corners bulge out of view.
fn barrel(uv: vec2<f32>) -> vec2<f32> {
    let k = u.barrel.x;
    let c = uv * 2.0 - 1.0;
    let d = c * (1.0 + k * dot(c, c)) / (1.0 + k);
    return d * 0.5 + 0.5;
}

// A small integer hash (lowbias32) of `x` to [0, 1).
fn hash(x: u32) -> f32 {
    var h = x;
    h ^= h >> 16u;
    h *= 0x7feb352du;
    h ^= h >> 15u;
    h *= 0x846ca68bu;
    h ^= h >> 16u;
    return f32(h >> 8u) / 16777216.0;
}

// Warm-up: the picture opens from a bright horizontal line (the vertical deflection coming up),
// then widens. Display uv -> display uv of the steady picture; outside the picture if either
// coordinate leaves [0, 1].
fn warm_up(uv: vec2<f32>) -> vec2<f32> {
    let w = u.signal.z;
    if (w >= 1.0) {
        return uv;
    }
    // Vertical size rises fast, the width catches up a little later.
    let ease = 1.0 - (1.0 - w) * (1.0 - w);
    let ys = max(ease, 0.004);
    let xs = 0.55 + 0.45 * smoothstep(0.1, 0.9, w);
    return (uv - 0.5) / vec2<f32>(xs, ys) + 0.5;
}

// Signed distance (px) outside a rectangle `size` with corners of radius `r`, at display pixel `p`.
fn corner_distance(p: vec2<f32>, size: vec2<f32>, r: f32) -> f32 {
    let q = abs(p - size * 0.5) - (size * 0.5 - vec2<f32>(r));
    return length(max(q, vec2<f32>(0.0))) - r;
}

@fragment
fn fs_display(in: VsOut) -> @location(0) vec4<f32> {
    let size = u.view.xy;
    let screen = in.pos.xy / size;
    let steady = warm_up(screen);
    let uv = barrel(steady);
    if (any(steady < vec2<f32>(0.0)) || any(steady > vec2<f32>(1.0))
        || any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        // Outside the curved glass (or the picture is still opening).
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    var colour = textureSampleLevel(persisted, linear, uv, 0.0).rgb;
    if (u.bloom.x > 0.0) {
        colour += u.bloom.x * textureSampleLevel(bloom, linear, uv, 0.0).rgb;
    }

    // Beam brightness here, for the effects that depend on it.
    let peak = max(colour.r, max(colour.g, colour.b));

    // Hot spots: the phosphor saturates, so the overbright core of a line goes pale.
    if (u.glass.w > 0.0 && peak > 1.0) {
        colour = mix(colour, vec3<f32>(peak), clamp((peak - 1.0) * u.glass.w, 0.0, 1.0));
    }

    if (u.scanlines.x > 0.5) {
        // In scene pixels, so the lines curve with the glass. A bright beam is a fat beam:
        // it fills the gap between its lines.
        let period = max(u.scanlines.z, 1.0);
        let phase = uv.y * size.y / period * 6.28318530718;
        let dark = 0.5 + 0.5 * cos(phase);
        let depth = u.scanlines.y * (1.0 - u.signal.w * clamp(peak, 0.0, 1.0));
        colour = colour * (1.0 - depth * dark);
    }

    // Signal noise: a faint grain, mostly where the beam is lit, in the phosphor's colour.
    if (u.glass.z > 0.0) {
        let p = vec2<u32>(in.pos.xy);
        let n = hash(p.x * 1973u + p.y * 9277u + u32(u.tint.w) * 26699u) - 0.5;
        colour += u.tint.rgb * (u.glass.z * n * (0.25 + 0.75 * clamp(peak, 0.0, 1.0)));
    }

    // Hum bar: a faint band of brightness rolling down the picture.
    if (u.signal.y > 0.0) {
        let d = (screen.y - u.signal.x) / 0.08;
        colour = colour * (1.0 + u.signal.y * exp(-0.5 * d * d));
    }

    // The tube is dimmer towards its edges, and its glass has rounded corners.
    if (u.glass.x > 0.0) {
        let c = screen * 2.0 - 1.0;
        colour = colour * (1.0 - u.glass.x * smoothstep(0.35, 1.45, length(c)));
    }
    if (u.glass.y > 0.0) {
        let outside = corner_distance(in.pos.xy, size, u.glass.y);
        colour = colour * (1.0 - smoothstep(-1.5, 0.5, outside));
    }

    // The collapsed picture is a hot line: all the beam's energy on a few rows.
    colour = colour * u.flicker.x * (1.0 + 2.0 * (1.0 - u.signal.z));
    return vec4<f32>(clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
