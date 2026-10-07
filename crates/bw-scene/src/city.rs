//! The city's dots: one mesh for every process, kernel and volume column,
//! one for the floor lattice, both drawn by `dots.wgsl` (PLAN §6, v3 look).
//!
//! The meshes are rebuilt when data arrives (about once a second); all
//! animation runs in the shader, so tens of thousands of dots stay cheap.

use crate::layout::{CELL, Column, LEVEL_H};
use crate::quality::{Quality, Tier};
use crate::*;
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::Indices;
use bevy::reflect::TypePath;
use bevy::render::render_resource::PrimitiveTopology;
use bevy::render::render_resource::{AsBindGroup, ShaderType};
use bevy::shader::ShaderRef;
use bw_model::{Health, Process, Realm};

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/dots.wgsl");
    app.add_plugins(MaterialPlugin::<DotsMaterial>::default())
        .init_resource::<Born>()
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
    match (h, realm) {
        (Health::Critical, _) => [1.0, 0.04, 0.06],
        (Health::Warning, _) => [0.7, 0.12, 1.0],
        (Health::Healthy, Realm::Kernel) => [0.28, 0.18, 1.0],
        (Health::Healthy, Realm::User) => [0.08, 0.38, 1.0],
    }
}

#[derive(ShaderType, Clone, Debug, Default)]
pub struct DotParams {
    pub a: Vec4,
    pub b: Vec4,
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
                [-9.0, 3.0],
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

fn column_dots(m: &mut DotMesh, id: usize, c: &Column, p: &Process, born: f32) {
    let health = p.health();
    let kind = match (health, c.realm) {
        (Health::Critical, _) => KIND_CRITICAL,
        (_, Realm::Kernel) => KIND_KERNEL,
        _ => KIND_PROCESS,
    };
    let rgb = health_color(health, c.realm);
    let cpu = (p.cpu_pct / 100.0).clamp(0.0, 1.0).sqrt();
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
                    [rgb[0], rgb[1], rgb[2], born],
                );
            }
        }
    }
}

#[allow(clippy::too_many_arguments)]
fn rebuild_city(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    time: Res<Time>,
    handles: Res<CityHandles>,
    mut born: ResMut<Born>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last_gen: Local<u64>,
) {
    if m.generation == *last_gen {
        return;
    }
    *last_gen = m.generation;
    let now = time.elapsed_secs();
    let first = born.0.is_empty();
    born.0.retain(|k, _| m.snapshot.processes.contains_key(k));
    let mut dm = DotMesh::default();
    for (id, c) in sl.layout.columns.iter().enumerate() {
        let Some(p) = m.snapshot.processes.get(&c.key) else {
            continue;
        };
        // Everything present at start grows in together, staggered by distance from the Wall.
        let b = *born.0.entry(c.key).or_insert(if first {
            now + (c.base.z - sl.layout.min.y) * 0.04
        } else {
            now
        });
        column_dots(&mut dm, id, c, p, b);
    }
    // Volumes: tall 3×3 columns; lit dots are used space, colored by health.
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
                        fx as f32 * 0.22,
                        level as f32 * LEVEL_H + 0.07,
                        fz as f32 * 0.22,
                    );
                    let lit = if level < used { 1.0 } else { 0.0 };
                    dm.cube(
                        base + off,
                        0.05,
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

fn rebuild_floor(
    sl: Res<SceneLayout>,
    quality: Res<Quality>,
    handles: Res<CityHandles>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last: Local<Option<(Vec2, Vec2, Tier)>>,
) {
    let key = (sl.layout.min, sl.layout.max, quality.tier);
    if *last == Some(key) {
        return;
    }
    *last = Some(key);
    let step = if quality.tier == Tier::Low {
        CELL
    } else {
        CELL * 0.5
    };
    let (x0, x1) = (sl.layout.min.x - 14.0, sl.layout.max.x + 14.0);
    let (z0, z1) = (sl.layout.wall_z() + 0.5, sl.layout.max.y + 16.0);
    let mut dm = DotMesh::default();
    let mut seed = 0x51_7CC1_B727_220Au64;
    let mut z = z0;
    while z < z1 {
        let mut x = x0;
        while x < x1 {
            seed ^= seed << 13;
            seed ^= seed >> 7;
            seed ^= seed << 17;
            let j = (seed >> 40) as f32 / (1u64 << 24) as f32;
            // Offset by half a cell so floor dots sit between columns.
            dm.cube(
                Vec3::new(x + step * 0.5, -0.02, z + step * 0.5),
                0.022,
                [j, 0.0],
                [-5.0, KIND_FLOOR],
                [0.12, 0.16, 0.5, 0.0],
            );
            x += step;
        }
        z += step;
    }
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
    if let Some(mut mat) = mats.get_mut(&handles.mat) {
        // Under reduced motion time stands still, far enough in that every column has grown in.
        let t = if settings.reduced_motion {
            1e5
        } else {
            time.elapsed_secs()
        };
        mat.params.a = Vec4::new(
            t,
            m.snapshot.system.kernel_pressure,
            id(sel.key),
            id(sel.hovered),
        );
        let intensity = if quality.tier == Tier::Low { 1.2 } else { 1.0 };
        mat.params.b = Vec4::new(
            if settings.issues_only { 1.0 } else { 0.0 },
            intensity,
            0.0,
            0.0,
        );
    }
}
