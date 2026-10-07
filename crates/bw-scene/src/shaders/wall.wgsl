// The Blackwall: a curtain of vertical light streaks made of dots, after the
// Blackwall in Cyberpunk 2077. Calm, it is violet-blue; under kernel pressure
// the streaks speed up, crowd together, glitch and burn red (red always means
// "issue"). Additive, so the kernel's districts show through it.
// Must compile on every wgpu backend, including GL (PLAN §3.3).

#import bevy_pbr::forward_io::VertexOutput

struct WallParams {
    base: vec4<f32>,
    hot: vec4<f32>,
    // x: pressure 0..1, y: time (s), z: glitch enabled (0/1), w: intensity
    state: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: WallParams;

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 91.7 + 1.3) * 43758.5453);
}

fn noise2(p: vec2<f32>) -> f32 {
    let i = floor(p);
    let f = fract(p);
    let u = f * f * (3.0 - 2.0 * f);
    let a = hash11(i.x + i.y * 57.0);
    let b = hash11(i.x + 1.0 + i.y * 57.0);
    let c = hash11(i.x + (i.y + 1.0) * 57.0);
    let d = hash11(i.x + 1.0 + (i.y + 1.0) * 57.0);
    return mix(mix(a, b, u.x), mix(c, d, u.x), u.y);
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let pressure = params.state.x;
    let t = params.state.y;
    let glitch_on = params.state.z;
    let intensity = params.state.w;

    var x = in.world_position.x;
    let y = in.world_position.y;

    // Glitch: horizontal tearing, more frequent under pressure.
    let slice = floor(y * 1.5 + floor(t * 10.0) * 3.1);
    let burst = step(0.94 - pressure * 0.3, hash11(floor(t * 5.0) * 7.0));
    x += (hash11(slice + floor(t * 10.0)) - 0.5) * burst * (0.5 + pressure * 3.0) * glitch_on;

    // Lanes: one streak per lane, each with its own brightness and speed.
    let density = 3.0 + pressure * 2.0;
    let lane = floor(x * density);
    let h = hash11(lane);
    let lx = fract(x * density);
    let line = 1.0 - smoothstep(0.04, 0.16, abs(lx - 0.5));

    // Each streak is a dotted line flowing down; under pressure it races.
    let speed = (0.6 + h * 1.6) * (1.0 + pressure * 3.0);
    let dots = step(0.45, fract(y * 5.0 + t * speed * 2.0));
    let dash = pow(fract(y * 0.05 * (0.6 + h) + t * speed * 0.08 + h * 13.0), 2.5);
    let lit = step(0.3 - pressure * 0.25, hash11(lane + 0.5));

    // Large, slow luminous patches, like light moving behind the curtain.
    let haze = noise2(vec2<f32>(x * 0.06, y * 0.05 - t * 0.04)) * noise2(vec2<f32>(x * 0.13 + 4.0, y * 0.09 + t * 0.03));

    // Streaks are brightest near the floor and fade upward.
    var fade = smoothstep(34.0, 6.0, y) * smoothstep(-0.5, 1.0, y);
#ifdef VERTEX_UVS_A
    fade *= smoothstep(0.0, 0.15, in.uv.x) * smoothstep(1.0, 0.85, in.uv.x);
#endif

    let tint = mix(params.base.rgb, vec3<f32>(0.2, 0.25, 1.0), h * 0.6);
    let col = mix(tint, params.hot.rgb, smoothstep(0.35, 0.9, pressure));
    let streak = line * lit * (0.15 + 0.85 * dash) * (0.55 + 0.45 * dots) * (0.5 + h);
    let rgb = col * (streak * (1.2 + pressure * 2.0) + haze * 0.5 * line + haze * 0.06) * fade * intensity;
    // AlphaMode::Add is premultiplied blending: alpha 0 makes it purely additive.
    return vec4<f32>(rgb, 0.0);
}
