// CRT post-process over the scene only, in four full-screen passes:
//   fs_persist  (full res)  phosphor persistence: max(scene, previous * decay), scene space
//   fs_bloom_h  (half res)  bright parts of the persisted image, 2x2 averaged, blurred across
//   fs_bloom_v  (half res)  the same blurred down
//   fs_display  (full res)  barrel distortion, + bloom, scanlines, flicker -> displayed image
// The master switch and every effect's enable flag are applied on the CPU: a disabled effect
// arrives here as a neutral value (decay 0, bloom 0, barrel 0, flicker gain 1).
// Each pass binds only its own slots; binding numbers are unique across the module.

struct Uniforms {
    // width_px, height_px (scene and display), half-res width_px, half-res height_px
    view: vec4<f32>,
    // scanline enable, scanline intensity [0,1], scanline period (px), unused
    scanlines: vec4<f32>,
    // persistence enable, decay factor applied to the previous buffer this frame
    // (exp(-dt_real / tau), 0 to clear), unused, unused
    persistence: vec4<f32>,
    // bloom intensity (0 = off), bright threshold, blur sigma (half-res px), unused
    bloom: vec4<f32>,
    // barrel amount k (0 = off), unused, unused, unused
    barrel: vec4<f32>,
    // flicker brightness gain this frame (1 = off), unused, unused, unused
    flicker: vec4<f32>,
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

fn gauss(i: f32) -> f32 {
    let s = max(u.bloom.z, 0.5);
    return exp(-0.5 * i * i / (s * s));
}

const BLOOM_TAPS: i32 = 10;

@fragment
fn fs_bloom_h(in: VsOut) -> @location(0) vec4<f32> {
    // A half-res pixel centre sits on the corner of 2x2 full-res pixels, so one bilinear
    // sample averages them; neighbours are 2 full-res pixels apart.
    let full = u.view.xy;
    let uv = in.pos.xy * 2.0 / full;
    let step = vec2<f32>(2.0 / full.x, 0.0);
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = -BLOOM_TAPS; i <= BLOOM_TAPS; i++) {
        let w = gauss(f32(i));
        let c = textureSampleLevel(persisted, linear, uv + step * f32(i), 0.0).rgb;
        sum += w * max(c - vec3<f32>(u.bloom.y), vec3<f32>(0.0));
        weight += w;
    }
    return vec4<f32>(sum / weight, 1.0);
}

@fragment
fn fs_bloom_v(in: VsOut) -> @location(0) vec4<f32> {
    let p = vec2<i32>(in.pos.xy);
    let last = i32(u.view.w) - 1;
    var sum = vec3<f32>(0.0);
    var weight = 0.0;
    for (var i = -BLOOM_TAPS; i <= BLOOM_TAPS; i++) {
        let w = gauss(f32(i));
        let q = vec2<i32>(p.x, clamp(p.y + i, 0, last));
        sum += w * textureLoad(bloom_h, q, 0).rgb;
        weight += w;
    }
    return vec4<f32>(sum / weight, 1.0);
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

@fragment
fn fs_display(in: VsOut) -> @location(0) vec4<f32> {
    let size = u.view.xy;
    let uv = barrel(in.pos.xy / size);
    if (any(uv < vec2<f32>(0.0)) || any(uv > vec2<f32>(1.0))) {
        // Outside the curved glass.
        return vec4<f32>(0.0, 0.0, 0.0, 1.0);
    }

    var colour = textureSampleLevel(persisted, linear, uv, 0.0).rgb;
    if (u.bloom.x > 0.0) {
        colour += u.bloom.x * textureSampleLevel(bloom, linear, uv, 0.0).rgb;
    }

    if (u.scanlines.x > 0.5) {
        // In scene pixels, so the lines curve with the glass.
        let period = max(u.scanlines.z, 1.0);
        let phase = uv.y * size.y / period * 6.28318530718;
        let dark = 0.5 + 0.5 * cos(phase);
        colour = colour * (1.0 - u.scanlines.y * dark);
    }

    colour = colour * u.flicker.x;
    return vec4<f32>(clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
}
