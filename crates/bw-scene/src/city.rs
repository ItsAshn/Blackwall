//! The chamber's dots: one mesh for every process, kernel and volume column,
//! one for the RAM floor, both drawn by `dots.wgsl`.
//!
//! The meshes are rebuilt when data arrives (about once a second); all
//! animation runs in the shader, so tens of thousands of dots stay cheap.
//! The floor is rebuilt only when the city's footprint or the machine's RAM
//! changes: how much of it is lit is a shader uniform.

use crate::camera::JackIn;
use crate::layout::{Column, LEVEL_H};
use crate::palette::{self, linear};
use crate::*;
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::Indices;
use bevy::reflect::TypePath;
use bevy::render::render_resource::{AsBindGroup, PrimitiveTopology, ShaderType};
use bevy::shader::ShaderRef;
use bw_model::{Health, Process, Realm};

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/dots.wgsl");
    app.add_plugins(MaterialPlugin::<DotsMaterial>::default())
        .init_resource::<Born>()
        .init_resource::<FloorInfo>()
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (rebuild_city, rebuild_floor, update_params)
                .chain()
                .in_set(SceneSet::Visuals),
        );
}

/// Health colors (linear). Color means health and nothing else.
pub fn health_color(h: Health, realm: Realm) -> [f32; 3] {
    linear(match (h, realm) {
        (Health::Critical, _) => palette::HEALTH_ISSUE,
        (Health::Warning, _) => palette::HEALTH_WATCH,
        (Health::Healthy, Realm::Kernel) => palette::KERNEL,
        (Health::Healthy, Realm::User) => palette::HEALTH_OK,
    })
}

#[derive(ShaderType, Clone, Debug, Default)]
pub struct DotParams {
    pub a: Vec4,
    pub b: Vec4,
    pub c: Vec4,
    pub d: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug, Default)]
pub struct DotsMaterial {
    #[uniform(0)]
    pub params: DotParams,
}

impl Material for DotsMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://bw_scene/shaders/dots.wgsl".into()
    }
}

/// What one floor dot stands for, for the HUD legend.
#[derive(Resource, Debug, Default, Clone, Copy)]
pub struct FloorInfo {
    /// Megabytes of RAM per floor dot (64, doubled until the dot count fits).
    pub mb_per_dot: u64,
    pub dots: usize,
}

/// When each process was first seen (seconds), for the grow-in effect.
#[derive(Resource, Default)]
struct Born(HashMap<ProcKey, f32>);

#[derive(Resource)]
struct CityHandles {
    city: Handle<Mesh>,
    floor: Handle<Mesh>,
    mat: Handle<DotsMaterial>,
}

const KIND_PROCESS: f32 = 0.0;
const KIND_KERNEL: f32 = 1.0;
const KIND_VOLUME: f32 = 2.0;
const KIND_FLOOR: f32 = 3.0;
const KIND_CRITICAL: f32 = 4.0;
const KIND_DYING: f32 = 5.0;

/// How long an exited process takes to dissolve.
const DISSOLVE_SECS: f32 = 1.2;
/// Upper bound on floor dots; above it each dot stands for more RAM.
const MAX_FLOOR_DOTS: u64 = 12_000;

/// Volume columns get ids above any process column.
pub const VOLUME_ID_BASE: usize = 1_000_000;

#[derive(Default)]
struct DotMesh {
    pos: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    uv_b: Vec<[f32; 2]>,
    color: Vec<[f32; 4]>,
    idx: Vec<u32>,
}

impl DotMesh {
    /// A small axis-aligned cube: the dots are square, like the reference.
    fn cube(&mut self, c: Vec3, h: f32, uv: [f32; 2], uv_b: [f32; 2], color: [f32; 4]) {
        let base = self.pos.len() as u32;
        for i in 0..8 {
            let o = Vec3::new(
                if i & 1 == 0 { -h } else { h },
                if i & 2 == 0 { -h } else { h },
                if i & 4 == 0 { -h } else { h },
            );
            self.pos.push((c + o).to_array());
            self.uv.push(uv);
            self.uv_b.push(uv_b);
            self.color.push(color);
        }
        const F: [[u32; 4]; 6] = [
            [0, 2, 3, 1],
            [4, 5, 7, 6],
            [0, 1, 5, 4],
            [2, 6, 7, 3],
            [0, 4, 6, 2],
            [1, 3, 7, 5],
        ];
        for f in F {
            self.idx
                .extend([f[0], f[1], f[2], f[0], f[2], f[3]].map(|v| base + v));
        }
    }

    fn into_mesh(mut self) -> Mesh {
        // Never upload an empty mesh: park one dot far below the floor.
        if self.pos.is_empty() {
            self.cube(
                Vec3::new(0.0, -1000.0, 0.0),
                0.01,
                [0.0, 0.0],
                [-9.0, KIND_FLOOR],
                [0.0; 4],
            );
        }
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.uv_b)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.color)
        .with_inserted_indices(Indices::U32(self.idx))
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<DotsMaterial>>,
) {
    let city = meshes.add(DotMesh::default().into_mesh());
    let floor = meshes.add(DotMesh::default().into_mesh());
    let mat = mats.add(DotsMaterial::default());
    for m in [&city, &floor] {
        commands.spawn((
            Mesh3d(m.clone()),
            MeshMaterial3d(mat.clone()),
            Transform::default(),
            NoFrustumCulling,
        ));
    }
    commands.insert_resource(CityHandles { city, floor, mat });
}

/// What a column looked like, kept so it can dissolve after its process exits.
#[derive(Clone)]
struct Ghost {
    column: Column,
    rgb: [f32; 3],
    died: f32,
}

fn column_dots(
    m: &mut DotMesh,
    id: usize,
    c: &Column,
    rgb: [f32; 3],
    kind: f32,
    cpu: f32,
    time: f32,
) {
    let fp = c.footprint as i32;
    let spacing = 0.2;
    let half = if c.realm == Realm::Kernel {
        0.035
    } else {
        0.042
    };
    for level in 0..c.levels {
        let hfrac = (level as f32 + 0.5) / c.levels as f32;
        for fx in 0..fp {
            for fz in 0..fp {
                let off = Vec3::new(
                    (fx as f32 - (fp - 1) as f32 / 2.0) * spacing,
                    level as f32 * LEVEL_H + half,
                    (fz as f32 - (fp - 1) as f32 / 2.0) * spacing,
                );
                m.cube(
                    c.base + off,
                    half,
                    [cpu, hfrac],
                    [id as f32, kind],
                    [rgb[0], rgb[1], rgb[2], time],
                );
            }
        }
    }
}

fn process_look(p: &Process, realm: Realm) -> ([f32; 3], f32, f32) {
    let health = p.health();
    let kind = match (health, realm) {
        (Health::Critical, _) => KIND_CRITICAL,
        (_, Realm::Kernel) => KIND_KERNEL,
        _ => KIND_PROCESS,
    };
    (
        health_color(health, realm),
        kind,
        (p.cpu_pct / 100.0).clamp(0.0, 1.0).sqrt(),
    )
}

#[allow(clippy::too_many_arguments)]
fn rebuild_city(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    time: Res<Time>,
    jack: Res<JackIn>,
    handles: Res<CityHandles>,
    mut born: ResMut<Born>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last_gen: Local<u64>,
    mut last_seen: Local<HashMap<ProcKey, Ghost>>,
    mut ghosts: Local<Vec<Ghost>>,
) {
    if m.generation == *last_gen {
        return;
    }
    *last_gen = m.generation;
    let now = time.elapsed_secs();
    let first = born.0.is_empty();
    // On the first frame the city resolves out of black row by row from the
    // Wall outward, after the jack-in's fall through the rain.
    let start = if first { now + jack.city_delay() } else { now };

    // Processes that exited since the last update start dissolving.
    for (k, g) in last_seen.drain() {
        if !m.snapshot.processes.contains_key(&k) {
            ghosts.push(Ghost { died: now, ..g });
        }
    }
    ghosts.retain(|g| now - g.died < DISSOLVE_SECS);
    born.0.retain(|k, _| m.snapshot.processes.contains_key(k));

    let mut dm = DotMesh::default();
    for (id, c) in sl.layout.columns.iter().enumerate() {
        let Some(p) = m.snapshot.processes.get(&c.key) else {
            continue;
        };
        let b = *born.0.entry(c.key).or_insert(if first {
            start + (c.base.z - sl.layout.min.y).max(0.0) * 0.05
        } else {
            now
        });
        let (rgb, kind, cpu) = process_look(p, c.realm);
        column_dots(&mut dm, id, c, rgb, kind, cpu, b);
        last_seen.insert(
            c.key,
            Ghost {
                column: c.clone(),
                rgb,
                died: 0.0,
            },
        );
    }
    for g in ghosts.iter() {
        column_dots(
            &mut dm,
            2 * VOLUME_ID_BASE,
            &g.column,
            g.rgb,
            KIND_DYING,
            0.0,
            g.died,
        );
    }
    // Volumes: tall 3×3 columns; lit dots are used space, the rest dot-off.
    for (i, v) in m.snapshot.volumes.iter().enumerate() {
        let base = sl.layout.volume_base(i);
        let levels = (8.0 + (v.total_bytes as f32 / (1u64 << 30) as f32).max(1.0).log2() * 2.0)
            .round() as u32;
        let used = (v.used_pct() / 100.0 * levels as f32).round() as u32;
        let rgb = health_color(v.health(), Realm::User);
        for level in 0..levels {
            for fx in -1..=1 {
                for fz in -1..=1 {
                    let off = Vec3::new(
                        fx as f32 * 0.2,
                        level as f32 * LEVEL_H + 0.05,
                        fz as f32 * 0.2,
                    );
                    let lit = if level < used { 1.0 } else { 0.0 };
                    dm.cube(
                        base + off,
                        0.045,
                        [lit, 0.0],
                        [(VOLUME_ID_BASE + i) as f32, KIND_VOLUME],
                        [rgb[0], rgb[1], rgb[2], 0.0],
                    );
                }
            }
        }
    }
    if let Some(mut mesh) = meshes.get_mut(&handles.city) {
        *mesh = dm.into_mesh();
    }
}

/// The floor is the machine's RAM: one dot per `mb_per_dot`, the dots
/// nearest the city first, so used memory lights up from the city outward.
fn rebuild_floor(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    handles: Res<CityHandles>,
    mut info: ResMut<FloorInfo>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last: Local<Option<(Vec2, Vec2, u64)>>,
) {
    let total = m.snapshot.system.mem_total;
    let key = (sl.layout.min, sl.layout.max, total);
    if total == 0 || *last == Some(key) {
        return;
    }
    *last = Some(key);

    let mut mb = 64u64;
    while total / (mb << 20) > MAX_FLOOR_DOTS {
        mb *= 2;
    }
    let n = (total / (mb << 20)).max(16) as usize;
    // Spread the dots over the city's footprint plus a margin, at whatever
    // pitch makes them fit, then keep the n nearest the center.
    let (min, max) = (
        sl.layout.min - Vec2::splat(4.0),
        sl.layout.max + Vec2::splat(6.0),
    );
    let area = (max - min).x * (max - min).y;
    let pitch = (area / n as f32).sqrt().clamp(0.22, 3.0);
    let mut pts = Vec::new();
    let mut z = min.y;
    while z <= max.y {
        let mut x = min.x;
        while x <= max.x {
            pts.push(Vec2::new(x + pitch * 0.5, z + pitch * 0.5));
            x += pitch;
        }
        z += pitch;
    }
    // Pad outward if the footprint ran short.
    let mut ring = 1.0;
    while pts.len() < n {
        let r = (max - min).max_element() * 0.5 + ring * pitch;
        let steps = (std::f32::consts::TAU * r / pitch) as usize;
        pts.extend(
            (0..steps)
                .map(|i| Vec2::from_angle(i as f32 / steps as f32 * std::f32::consts::TAU) * r),
        );
        ring += 1.0;
    }
    let center = (sl.layout.min + sl.layout.max) * 0.5;
    pts.sort_by(|a, b| {
        a.distance_squared(center)
            .total_cmp(&b.distance_squared(center))
    });
    pts.truncate(n);

    let ok = health_color(Health::Healthy, Realm::User);
    let mut dm = DotMesh::default();
    for (i, p) in pts.iter().enumerate() {
        let rank = (i as f32 + 0.5) / n as f32;
        dm.cube(
            Vec3::new(p.x, -0.02, p.y),
            0.022,
            [rank, 0.0],
            [-5.0, KIND_FLOOR],
            [ok[0], ok[1], ok[2], 0.0],
        );
    }
    *info = FloorInfo {
        mb_per_dot: mb,
        dots: n,
    };
    if let Some(mut mesh) = meshes.get_mut(&handles.floor) {
        *mesh = dm.into_mesh();
    }
}

fn update_params(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    sel: Res<Selection>,
    settings: Res<SceneSettings>,
    quality: Res<Quality>,
    time: Res<Time>,
    handles: Res<CityHandles>,
    mut mats: ResMut<Assets<DotsMaterial>>,
) {
    let id = |k: Option<ProcKey>| {
        k.and_then(|k| sl.layout.column_index(&k))
            .map_or(-1.0, |i| i as f32)
    };
    let Some(mut mat) = mats.get_mut(&handles.mat) else {
        return;
    };
    // Under reduced motion time stops, far enough in that every column has grown.
    let t = if settings.reduced_motion {
        1e5
    } else {
        time.elapsed_secs()
    };
    let s = &m.snapshot.system;
    let ram = s.mem_used as f32 / s.mem_total.max(1) as f32;
    mat.params.a = Vec4::new(t, s.kernel_pressure, id(sel.key), id(sel.hovered));
    let intensity = if quality.tier == crate::Tier::Low {
        1.2
    } else {
        1.0
    };
    mat.params.b = Vec4::new(
        if settings.issues_only { 1.0 } else { 0.0 },
        intensity,
        ram,
        if settings.reduced_motion { 1.0 } else { 0.0 },
    );
    mat.params.c = palette::linear4(palette::DOT_OFF, 0.0);
    let reach = sl.layout.extent;
    mat.params.d = Vec4::new(reach + 8.0, reach * 1.8 + 26.0, 0.0, 0.0);
}
