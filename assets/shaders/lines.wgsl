// Lines for the slice, traces and map markers: anti-aliased quads (one quad per polyline
// segment, built on the CPU per line, not per pixel), each in its own colour. Blended with max
// so overlaps do not brighten.

struct Uniforms {
    // viewport width_px, height_px, unused, unused
    view: vec4<f32>,
    // background rgb (display space), unused
    background: vec4<f32>,
};

@group(0) @binding(0) var<uniform> u: Uniforms;

struct VsIn {
    // viewport pixels, y down
    @location(0) pos: vec2<f32>,
    // distance along the polyline (px), signed distance across it (px)
    @location(1) along_across: vec2<f32>,
    // half width (px), intensity, dash pattern (0 solid, 1 dashed, 2 dotted), unused
    @location(2) params: vec4<f32>,
    // line colour rgb (display space), unused
    @location(3) colour: vec4<f32>,
};

struct VsOut {
    @builtin(position) pos: vec4<f32>,
    @location(0) along_across: vec2<f32>,
    @location(1) params: vec4<f32>,
    @location(2) colour: vec4<f32>,
};

@vertex
fn vs_main(v: VsIn) -> VsOut {
    var out: VsOut;
    let ndc = vec2<f32>(v.pos.x / u.view.x * 2.0 - 1.0, 1.0 - v.pos.y / u.view.y * 2.0);
    out.pos = vec4<f32>(ndc, 0.0, 1.0);
    out.along_across = v.along_across;
    out.params = v.params;
    out.colour = v.colour;
    return out;
}

@fragment
fn fs_main(in: VsOut) -> @location(0) vec4<f32> {
    let half_w = in.params.x;
    let cover = 1.0 - smoothstep(half_w - 0.5, half_w + 0.5, abs(in.along_across.y));
    var dash = 1.0;
    if (in.params.z > 1.5) {
        dash = step(fract(in.along_across.x / 5.0), 0.4);
    } else if (in.params.z > 0.5) {
        dash = step(fract(in.along_across.x / 10.0), 0.6);
    }
    let level = in.params.y * cover * dash;
    let colour = mix(u.background.rgb, in.colour.rgb, clamp(level, 0.0, 1.0)) * max(1.0, level);
    return vec4<f32>(colour, 1.0);
}
