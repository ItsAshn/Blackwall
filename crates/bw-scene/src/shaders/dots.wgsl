// Every dot in the chamber: process columns, kernel columns, volume columns
// and the RAM floor, in one mesh. Black is nothing; light is data.
//   color: health only (blue → violet → red)
//   brightness + rising pulses: CPU; the brightest dots burn to a white core
//   dot count: memory
//
// Each dot is a camera-facing quad (four corners at the dot's center, spread
// out here) drawn as a hot round core in a soft halo, blended additively.
//
// Per-vertex data (see city.rs):
//   normal = (corner x, corner y, quad half-size)
//   uv    = (cpu 0..1 | volume used flag | floor rank 0..1, height fraction 0..1)
//   uv_b  = (column id, kind)
//   color = (linear rgb, birth time | death time)
// Must compile on every wgpu backend, including GL (PLAN §3.3).

#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::view

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(5) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world_position: vec3<f32>,
    @location(1) corner: vec2<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(4) color: vec4<f32>,
};

struct DotParams {
    // x: time, y: pressure, z: selected column id (-1 none), w: hovered id
    a: vec4<f32>,
    // x: issues-only, y: intensity, z: RAM in use 0..1, w: reduced motion
    b: vec4<f32>,
    // rgb: dot-off (linear), w: unused
    c: vec4<f32>,
    // x: fog start, y: fog end (world units from the eye), zw: unused
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
const KIND_BEACON: f32 = 8.0;
const KIND_CABLE: f32 = 9.0;
const KIND_LEDGE: f32 = 10.0;
const KIND_STRATUM: f32 = 11.0;

fn hash11(x: f32) -> f32 {
    return fract(sin(x * 127.1 + 3.7) * 43758.5453);
}

fn hash31(p: vec3<f32>) -> f32 {
    return fract(sin(dot(p, vec3<f32>(127.1, 311.7, 74.7))) * 43758.5453);
}

fn is_kind(k: f32, want: f32) -> bool {
    return abs(k - want) < 0.5;
}

// Light above 1.0 burns toward a white core; bloom adds the halo.
fn burn(rgb: vec3<f32>, b: f32) -> vec3<f32> {
    let lit = rgb * b;
    return mix(lit, vec3<f32>(b), clamp((b - 1.0) * 0.35, 0.0, 0.7));
}

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let center = (get_world_from_local(v.instance_index) * vec4<f32>(v.position, 1.0)).xyz;
    // Billboard in view space, so every dot is round from any angle.
    let in_view = view.view_from_world * vec4<f32>(center, 1.0);
    // Dots right at the eye would fill the screen: cap their size by depth.
    let depth = max(-in_view.z, 0.01);
    let spread = vec4<f32>(v.normal.xy * min(v.normal.z, depth * 0.03), 0.0, 0.0);
    out.clip = view.clip_from_view * (in_view + spread);
    out.world_position = center;
    out.corner = v.normal.xy;
    out.uv = v.uv;
    out.uv_b = v.uv_b;
    out.color = v.color;
    return out;
}

// A point of light: a small disc burning white at its heart, inside a halo
// that falls off softly into the black. 1.0 is the quad's edge.
fn glow(corner: vec2<f32>) -> vec2<f32> {
    let d = length(corner);
    let core = (1.0 - smoothstep(0.08, 0.17, d)) + exp(-d * d * 260.0) * 0.8;
    let halo = exp(-d * d * 14.0) * 0.1 + exp(-d * d * 60.0) * 0.55;
    return vec2<f32>(core, halo * (1.0 - smoothstep(0.85, 1.0, d)));
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let uv = in.uv;
    let ub = in.uv_b;
    let col = in.color;
    let g = glow(in.corner);
    if (g.x + g.y < 0.004) {
        discard;
    }
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
            if (is_kind(kind, KIND_FLOW)) {
                // A conduit: pulses flowing along it; uv.x pulses per path.
                b = 0.08 + pow(fract(uv.y * uv.x - t * 0.5 + phase), 12.0) * 1.5;
            } else if (is_kind(kind, KIND_CABLE)) {
                // A family cable: dim, with rare slow pulses parent → child.
                b = 0.1 + pow(fract(uv.y * uv.x - t * 0.15 + phase), 16.0) * 0.9;
            } else if (is_kind(kind, KIND_LEDGE)) {
                b = 0.1;
            } else if (is_kind(kind, KIND_STRATUM)) {
                b = 0.42;
            } else if (is_kind(kind, KIND_BEACON)) {
                // An issue's beam: bright, climbing, fading as it rises.
                let crit = uv.x;
                let pulse = pow(fract(uv.y * 5.0 - t * (0.35 + crit * 0.5) + phase), 6.0);
                b = (0.9 + crit * 0.8 + pulse * (2.0 + crit * 2.0)) * pow(1.0 - uv.y, 1.5);
            } else {
                // Windows: CPU is the share lit (linear, so it reads); each
                // window keeps its state for 5-14 s, then may switch.
                var share = 0.03 + uv.x * 0.97;
                if (is_kind(kind, KIND_CRITICAL)) {
                    share = max(share, 0.45);
                }
                let h = hash31(in.world_position * 7.31);
                let epoch = floor(t / (5.0 + h * 9.0) + h * 13.0);
                if (hash11(h * 917.0 + epoch * 1.37) < share) {
                    b = 0.8 + h * 0.3;
                } else {
                    // Dark windows still show the mass: memory.
                    b = 0.045;
                }
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
            } else if (params.b.x > 0.5 && !is_kind(kind, KIND_WATCH) && !is_kind(kind, KIND_BEACON)) {
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

    // Fog into the void, by distance from the eye: the city crowds in and
    // its far side sinks into black. Nothing has an edge or a horizon.
    let r = distance(in.world_position, view.world_position);
    let fog = (1.0 - smoothstep(params.d.x, params.d.y, r)) * smoothstep(0.6, 2.5, r);
    // The core keeps the dot's light (and burns whiter); the halo carries its
    // hue out into the dark. Alpha 0: additive (premultiplied) blending.
    let core = mix(rgb, vec3<f32>(max(rgb.r, max(rgb.g, rgb.b))), 0.25) * g.x * 1.6;
    let light = core + rgb * g.y;
    return vec4<f32>(light * params.b.y * fog, 0.0);
}
