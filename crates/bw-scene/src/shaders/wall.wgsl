// The Blackwall: a curtain of falling rain made of square dots, after the
// Blackwall in Cyberpunk 2077 and the rain of the Matrix. Each lane carries
// trails led by a white-hot head and fading into black. Kernel pressure adds
// lanes, lengthens and speeds the trails, and burns them from magenta to red;
// above ~65% the rain tears sideways. Additive, so the kernel's districts
// show through the gaps. Nothing here is decoration: no haze, no backdrop.
// Must compile on every wgpu backend, including GL (PLAN §3.3).

#import bevy_pbr::forward_io::VertexOutput

struct WallParams {
    calm: vec4<f32>,
    hot: vec4<f32>,
    head: vec4<f32>,
    // x: pressure 0..1, y: time (s), z: glitch enabled (0/1), w: intensity
    state: vec4<f32>,
    // x: fall-speed boost (jack-in), y: wall height (world units)
    extra: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: WallParams;

const PITCH: f32 = 0.18;

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 91.7 + 1.3) * 43758.5453);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let pressure = params.state.x;
    let t = params.state.y;
    let glitch_on = params.state.z;
    let intensity = params.state.w;
    let boost = params.extra.x;
    let height = params.extra.y;

    var x = in.world_position.x;
    let y = in.world_position.y;

    // Tearing: horizontal slices jump sideways under high pressure.
    let slice = floor(y * 1.5 + floor(t * 10.0) * 3.1);
    let burst = step(0.94 - max(pressure - 0.65, 0.0) * 1.2, hash11(floor(t * 5.0) * 7.0)) * step(0.65, pressure);
    x += (hash11(slice + floor(t * 10.0)) - 0.5) * burst * (1.0 + pressure * 3.0) * glitch_on;

    // Lanes: closer together under pressure; a few stay dark at rest.
    let gap = 0.32 - pressure * 0.12;
    let lane = floor(x / gap);
    let lx = fract(x / gap);
    let h = hash11(lane);
    let lit = step(0.12 - pressure * 0.12, hash11(lane + 0.5));

    // Square dots on a grid of lanes × PITCH.
    let cell = floor(y / PITCH);
    let fy = fract(y / PITCH);
    let dot = (1.0 - smoothstep(0.09, 0.16, abs(lx - 0.5))) * (1.0 - smoothstep(0.24, 0.34, abs(fy - 0.5)));

    // Up to three trails per lane, falling.
    let speed = (2.0 + 6.0 * h) * (1.0 + 3.0 * pressure) * boost / PITCH;
    let len = 10.0 + 26.0 * pressure * hash11(lane + 2.2) + 12.0 * h;
    let period = height / PITCH + len;
    var trail = 0.0;
    var head = 0.0;
    for (var k = 0; k < 3; k = k + 1) {
        let head_cell = period * (1.0 - fract(t * speed / period + h * 7.0 + f32(k) / 3.0)) - len * 0.5;
        let d = cell - floor(head_cell);
        if (d >= 0.0 && d < len) {
            if (d < 1.0) {
                head = 1.0;
            } else {
                trail = max(trail, pow(1.0 - d / len, 1.4) * (0.45 + 0.55 * h));
            }
        }
    }

    // Fade into the void above and at the curtain's ends.
    var fade = smoothstep(height, height * 0.55, y) * smoothstep(-0.2, 0.3, y);
#ifdef VERTEX_UVS_A
    fade *= smoothstep(0.0, 0.12, in.uv.x) * smoothstep(1.0, 0.88, in.uv.x);
#endif

    let tone = mix(params.calm.rgb, params.hot.rgb, smoothstep(0.35, 0.9, pressure));
    let rgb = (tone * trail * (1.4 + pressure * 1.6) + params.head.rgb * head * 2.4) * dot * lit * fade * intensity;
    // AlphaMode::Add is premultiplied blending: alpha 0 makes it purely additive.
    return vec4<f32>(rgb, 0.0);
}
