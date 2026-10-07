// Every dot in the city: process columns, kernel columns, volume columns and
// the floor lattice, in one mesh. Color carries health only (blue → violet →
// red); brightness and rising pulses carry CPU; dot count carries memory.
//
// Per-vertex data (see city.rs):
//   uv   = (cpu 0..1 | volume used flag | floor jitter, height fraction 0..1)
//   uv_b = (column id, kind)
//   color = (rgb health color, birth time)
// Must compile on every wgpu backend, including GL (PLAN §3.3).

#import bevy_pbr::forward_io::VertexOutput

struct DotParams {
    // x: time, y: pressure, z: selected column id (-1 none), w: hovered id
    a: vec4<f32>,
    // x: issues-only, y: intensity
    b: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: DotParams;

const KIND_PROCESS: f32 = 0.0;
const KIND_KERNEL: f32 = 1.0;
const KIND_VOLUME: f32 = 2.0;
const KIND_FLOOR: f32 = 3.0;
const KIND_CRITICAL: f32 = 4.0;

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 127.1 + 3.7) * 43758.5453);
}

fn is_kind(k: f32, want: f32) -> bool {
    return abs(k - want) < 0.5;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    var uv = vec2<f32>(0.0);
    var ub = vec2<f32>(-10.0, 0.0);
    var col = vec4<f32>(1.0, 1.0, 1.0, 0.0);
#ifdef VERTEX_UVS_A
    uv = in.uv;
#endif
#ifdef VERTEX_UVS_B
    ub = in.uv_b;
#endif
#ifdef VERTEX_COLORS
    col = in.color;
#endif
    let t = params.a.x;
    let id = ub.x;
    let kind = ub.y;
    let phase = hash11(id);

    var b = 1.0;
    if (is_kind(kind, KIND_FLOOR)) {
        // A slow wave rolling toward the Wall.
        let wave = pow(sin(in.world_position.z * 0.35 + t * 1.1 + uv.x * 0.6) * 0.5 + 0.5, 6.0);
        b = 0.18 + 0.5 * wave + 0.2 * uv.x;
    } else if (is_kind(kind, KIND_VOLUME)) {
        b = select(0.1, 1.5, uv.x > 0.5);
    } else {
        // Columns grow upward when a process is born.
        let grow = clamp((t - col.a) / 1.2, 0.0, 1.0);
        if (uv.y > grow + 0.001) {
            discard;
        }
        let cpu = uv.x;
        let f = fract(uv.y * 1.5 - t * (0.12 + cpu * 1.4) + phase);
        let pulse = pow(f, 14.0) * (0.6 + cpu * 5.0);
        b = 0.3 + cpu * 1.6 + pulse;
        if (is_kind(kind, KIND_CRITICAL)) {
            b = b * (0.5 + 1.2 * step(0.5, fract(t * 1.3 + phase)));
        } else if (params.b.x > 0.5) {
            b = b * 0.1;
        }
        if (is_kind(kind, KIND_KERNEL)) {
            b = b * 0.75;
        }
    }

    var rgb = col.rgb * b * params.b.y;
    let process = !is_kind(kind, KIND_FLOOR) && !is_kind(kind, KIND_VOLUME);
    if (process && abs(id - params.a.z) < 0.5) {
        rgb = mix(rgb, vec3<f32>(2.2, 2.6, 3.2), 0.65);
    } else if (process && abs(id - params.a.w) < 0.5) {
        rgb = rgb * 2.2 + vec3<f32>(0.2);
    }
    return vec4<f32>(rgb, 1.0);
}
