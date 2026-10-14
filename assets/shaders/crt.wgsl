// CRT post-process: one full-screen pass over the scene only.
// Implemented: master enable, scanlines, phosphor persistence (ping-pong decay buffer).
// Uniform slots reserved, not yet implemented: bloom, barrel distortion, flicker.
//
// Outputs: location 0 = new persistence buffer (Rgba16Float),
//          location 1 = displayed image (Rgba8Unorm, display-space values, which egui shows as is).

struct Uniforms {
    // width_px, height_px, time_s (real seconds, for flicker later), master_enable (0/1)
    view: vec4<f32>,
    // scanline enable, scanline intensity [0,1], scanline period (px), unused
    scanlines: vec4<f32>,
    // persistence enable, decay factor applied to the previous buffer this frame
    // (exp(-dt_real / tau), 0 to clear), unused, unused
    persistence: vec4<f32>,
    // bloom enable, bloom intensity, unused, unused          (reserved)
    bloom: vec4<f32>,
    // barrel enable, barrel amount, unused, unused           (reserved)
    barrel: vec4<f32>,
    // flicker enable, flicker amount, unused, unused         (reserved)
    flicker: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;
@group(0) @binding(1) var scene: texture_2d<f32>;
@group(0) @binding(2) var previous: texture_2d<f32>;

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

struct FsOut {
    @location(0) persist: vec4<f32>,
    @location(1) display: vec4<f32>,
};

@fragment
fn fs_main(in: VsOut) -> FsOut {
    let p = vec2<i32>(in.pos.xy);
    let current = textureLoad(scene, p, 0).rgb;

    var out: FsOut;
    if (u.view.w < 0.5) {
        out.persist = vec4<f32>(current, 1.0);
        out.display = vec4<f32>(clamp(current, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
        return out;
    }

    var colour = current;
    if (u.persistence.x > 0.5) {
        let prev = textureLoad(previous, p, 0).rgb;
        colour = max(current, prev * u.persistence.y);
    }
    out.persist = vec4<f32>(colour, 1.0);

    if (u.scanlines.x > 0.5) {
        let period = max(u.scanlines.z, 1.0);
        let phase = in.pos.y / period * 6.28318530718;
        let dark = 0.5 + 0.5 * cos(phase);
        colour = colour * (1.0 - u.scanlines.y * dark);
    }

    out.display = vec4<f32>(clamp(colour, vec3<f32>(0.0), vec3<f32>(1.0)), 1.0);
    return out;
}
