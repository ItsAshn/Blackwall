// Towers as solid walls of data (see towers.rs).
//
// Near-black faces hide what stands behind them; a faint edge outlines each
// tower so the skyline reads at any distance. The faces are a dense grid of
// data cells: a CPU share of them lit. Hover or choose a tower and the cells
// come up, showing what the wall is made of. Kernel towers use a finer,
// denser grid laid in heavy courses. Far towers sink into black (fog from
// the eye) and the screen's edges darken (vignette), so depth reads.
//
//   uv      = (along the face, height) in world units; roof: (x, z) from its corner
//   uv_b    = (tower id, realm: 0 user, 1 kernel, 2 kernel memory, +10 dying)
//   tangent = (face width, tower height | roof depth, roof flag, born | died time)
//   color   = (health rgb, cpu 0..1)

#import bevy_pbr::mesh_functions::get_world_from_local
#import bevy_pbr::mesh_view_bindings::view

struct TowerParams {
    // x: time, y: selected id, z: hovered id, w: open id
    a: vec4<f32>,
    // x: fog start, y: fog end, z: issues only, w: reduced motion
    b: vec4<f32>,
    // x: vignette strength, y: cutaway distance (0 = off)
    c: vec4<f32>,
};

@group(#{MATERIAL_BIND_GROUP}) @binding(0) var<uniform> params: TowerParams;

struct Vertex {
    @builtin(instance_index) instance_index: u32,
    @location(0) position: vec3<f32>,
    @location(1) normal: vec3<f32>,
    @location(2) uv: vec2<f32>,
    @location(3) uv_b: vec2<f32>,
    @location(4) tangent: vec4<f32>,
    @location(5) color: vec4<f32>,
};

struct VertexOutput {
    @builtin(position) clip: vec4<f32>,
    @location(0) world: vec3<f32>,
    @location(1) uv: vec2<f32>,
    @location(2) uv_b: vec2<f32>,
    @location(3) tangent: vec4<f32>,
    @location(4) color: vec4<f32>,
};

@vertex
fn vertex(v: Vertex) -> VertexOutput {
    var out: VertexOutput;
    let world = get_world_from_local(v.instance_index) * vec4<f32>(v.position, 1.0);
    out.clip = view.clip_from_world * world;
    out.world = world.xyz;
    out.uv = v.uv;
    out.uv_b = v.uv_b;
    out.tangent = v.tangent;
    out.color = v.color;
    return out;
}

fn hash12(p: vec2<f32>) -> f32 {
    return fract(sin(dot(p, vec2<f32>(127.1, 311.7))) * 43758.5453);
}

fn near(a: f32, b: f32) -> bool {
    return abs(a - b) < 0.5;
}

@fragment
fn fragment(in: VertexOutput) -> @location(0) vec4<f32> {
    let t = params.a.x;
    let id = in.uv_b.x;
    var realm = in.uv_b.y;
    let dying = realm > 5.0;
    if (dying) {
        realm = realm - 10.0;
    }
    let roof = in.tangent.z > 0.5;
    let w = in.tangent.x;
    let h = in.tangent.y;
    let cpu = in.color.a;
    let health = in.color.rgb;

    // Rising out of the floor when born; dissolving from the top when dead.
    let hf = select(in.uv.y / max(h, 1e-3), 1.0, roof);
    if (dying) {
        let age = (t - in.tangent.w) / 1.2;
        if (age > 1.0 || hf > 1.0 - age) {
            discard;
        }
    } else {
        let grow = clamp((t - in.tangent.w) / 1.4, 0.0, 1.0);
        if (hf > grow + 0.001 && !roof) {
            discard;
        }
        if (roof && grow < 1.0) {
            discard;
        }
    }

    let chosen = near(id, params.a.y);
    let hovered = near(id, params.a.z);
    let open = near(id, params.a.w);
    let reveal = select(0.0, 1.0, chosen || hovered);

    // Edges: distance to the face's border, in world units.
    let u = in.uv.x;
    let v = in.uv.y;
    var edge = min(u, w - u);
    if (roof) {
        edge = min(edge, min(v, h - v));
    } else {
        edge = min(edge, h - v);
    }
    let fw = max(fwidth(u), fwidth(v));
    let rim = 1.0 - smoothstep(0.0, 0.025 + fw * 1.5, edge);

    // An open tower (we are inside it) keeps only its outline, and so does
    // anything standing between the eye and what is chosen: a cutaway.
    let eye_d = distance(in.world, view.world_position);
    let cut = params.c.y > 0.0 && eye_d < params.c.y && !chosen;
    if ((open || cut) && rim < 0.05) {
        discard;
    }

    // The data cells: user towers a 0.07 grid, kernel a dense 0.04 grid in
    // courses (every sixth row dark).
    let kernel = realm > 0.5;
    let pitch = select(0.07, 0.04, kernel);
    let g = in.uv / pitch;
    let cell = floor(g);
    let f = fract(g) - vec2<f32>(0.5);
    let r = select(0.17, 0.3, kernel);
    let gw = fwidth(g.x) + fwidth(g.y);
    var dot = 1.0 - smoothstep(r - gw * 0.5, r + gw * 0.5, length(f));
    // Far away the grid is finer than a pixel: fade it to its average.
    dot = mix(dot, r * r * 3.1, smoothstep(0.35, 0.9, gw));
    var course = 1.0;
    if (kernel && !roof && (i32(cell.y) % 6) == 0) {
        course = 0.25;
    }
    let hc = hash12(cell + vec2<f32>(id * 7.13, select(0.0, 91.0, roof)));
    let epoch = floor(t / (6.0 + hc * 10.0) + hc * 17.0);
    let lit = hash12(vec2<f32>(hc * 413.0, epoch)) < 0.02 + cpu * 0.98;

    var cells = select(0.022, 0.13, lit);
    cells = mix(cells, select(0.3, 1.2, lit), reveal);
    var rgb = vec3<f32>(0.0035) + health * (dot * cells * course);
    // The heavy kernel material: a little body of its own.
    if (kernel) {
        rgb = rgb + health * 0.012;
    }

    // Outline: faint from afar, bright when chosen; issues blink.
    var rim_b = mix(0.16, 0.9, reveal);
    let is_issue = health.r > 0.5 && health.g < 0.2;
    if (is_issue && params.b.w < 0.5) {
        rim_b = rim_b * (0.5 + 1.2 * step(0.5, fract(t * 1.3 + hc)));
    }
    if (open) {
        rim_b = 0.5;
    }
    if (cut) {
        rim_b = 0.12;
    }
    rgb = mix(rgb, health * rim_b * 1.6, rim);
    if (params.b.z > 0.5 && !is_issue) {
        rgb = rgb * 0.15;
    }

    // Depth: fog from the eye into black.
    let dist = distance(in.world, view.world_position);
    let fog = 1.0 - smoothstep(params.b.x, params.b.y, dist);
    // Vignette: the frame's edges sink.
    let sp = (in.clip.xy - view.viewport.xy) / view.viewport.zw - vec2<f32>(0.5);
    let vig = 1.0 - params.c.x * smoothstep(0.32, 0.78, length(sp * vec2<f32>(1.25, 1.0)));
    return vec4<f32>(rgb * fog * vig, 1.0);
}
