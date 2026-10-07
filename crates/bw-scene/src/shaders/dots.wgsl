// Every dot in the chamber: process columns, kernel columns, volume columns
// and the RAM floor, in one mesh. Black is nothing; light is data.
//   color: health only (blue → violet → red)
//   brightness + rising pulses: CPU; the brightest dots burn to a white core
//   dot count: memory
//
// Per-vertex data (see city.rs):
//   uv    = (cpu 0..1 | volume used flag | floor rank 0..1, height fraction 0..1)
//   uv_b  = (column id, kind)
//   color = (linear rgb, birth time | death time)
// Must compile on every wgpu backend, including GL (PLAN §3.3).

#import bevy_pbr::forward_io::VertexOutput

struct DotParams {
    // x: time, y: pressure, z: selected column id (-1 none), w: hovered id
    a: vec4<f32>,
    // x: issues-only, y: intensity, z: RAM in use 0..1, w: reduced motion
    b: vec4<f32>,
    // rgb: dot-off (linear), w: unused
    c: vec4<f32>,
    // x: fog start, y: fog end (world units from the city's center), zw: unused
    d: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: DotParams;

const KIND_PROCESS: f32 = 0.0;
const KIND_KERNEL: f32 = 1.0;
const KIND_VOLUME: f32 = 2.0;
const KIND_FLOOR: f32 = 3.0;
const KIND_CRITICAL: f32 = 4.0;
const KIND_DYING: f32 = 5.0;
const KIND_FLOW: f32 = 6.0;
const KIND_WATCH: f32 = 7.0;

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 127.1 + 3.7) * 43758.5453);
}

fn is_kind(k: f32, want: f32) -> bool {
    return abs(k - want) < 0.5;
}

// Light above 1.0 burns toward a white core; bloom adds the halo.
fn burn(rgb: vec3<f32>, b: f32) -> vec3<f32> {
    let lit = rgb * b;
    return mix(lit, vec3<f32>(b), clamp((b - 1.0) * 0.35, 0.0, 0.7));
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
    let reduced = params.b.w > 0.5;
    let id = ub.x;
    let kind = ub.y;
    let phase = hash11(id);

    var rgb: vec3<f32>;
    if (is_kind(kind, KIND_FLOOR)) {
        // RAM: dots ranked from the city outward; the first `used` share are lit.
        if (uv.x < params.b.z) {
            rgb = col.rgb * 0.32;
        } else {
            rgb = params.c.rgb;
        }
    } else if (is_kind(kind, KIND_VOLUME)) {
        if (uv.x > 0.5) {
            rgb = burn(col.rgb, 1.3);
        } else {
            rgb = params.c.rgb;
        }
    } else {
        var b: f32;
        if (is_kind(kind, KIND_DYING)) {
            // Dissolving upward into the dark: the column empties from the floor up.
            let age = (t - col.a) / 1.2;
            if (age > 1.0 || uv.y < age) {
                discard;
            }
            b = 0.6 * (1.0 - age);
        } else {
            // Rising out of the floor when born.
            let grow = clamp((t - col.a) / 1.2, 0.0, 1.0);
            if (uv.y > grow + 0.001) {
                discard;
            }
            let cpu = uv.x;
            if (is_kind(kind, KIND_FLOW)) {
                // A conduit: dots flowing along its path.
                b = 0.22 + pow(fract(uv.y * 14.0 - t * 0.7 + phase), 10.0) * 2.4;
            } else {
                let f = fract(uv.y * 1.5 - t * (0.12 + cpu * 1.4) + phase);
                let pulse = pow(f, 14.0) * (0.6 + cpu * 5.0);
                b = 0.3 + cpu * 1.6 + pulse;
            }
            if (is_kind(kind, KIND_WATCH) && !reduced) {
                // Worth watching: a slow breath, so it can be spotted from afar.
                b = b * (0.7 + 0.8 * (sin(t * 3.0 + phase * 6.0) * 0.5 + 0.5));
            }
            if (is_kind(kind, KIND_CRITICAL)) {
                // Issues blink at 1.3 Hz; under reduced motion they hold fully lit.
                if (reduced) {
                    b = b * 1.4;
                } else {
                    b = b * (0.35 + 1.25 * step(0.5, fract(t * 1.3 + phase)));
                }
            } else if (params.b.x > 0.5 && !is_kind(kind, KIND_WATCH)) {
                b = b * 0.08;
            }
            if (is_kind(kind, KIND_KERNEL)) {
                b = b * 0.75;
            }
        }
        rgb = burn(col.rgb, b);
        if (abs(id - params.a.z) < 0.5) {
            // Chosen: blazing, but it keeps its health hue (red stays red).
            let lift = b * 1.8 + 0.6;
            rgb = mix(col.rgb * lift, vec3<f32>(lift), 0.18);
        } else if (abs(id - params.a.w) < 0.5) {
            rgb = burn(col.rgb, b * 2.2 + 0.4);
        }
    }

    // Distance fog into the void: nothing has an edge or a horizon.
    let r = length(in.world_position.xz);
    let fog = 1.0 - smoothstep(params.d.x, params.d.y, r);
    return vec4<f32>(rgb * params.b.y * fog, 1.0);
}
