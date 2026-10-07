//! Towers as solid walls: one opaque mesh of boxes, one per process plot,
//! drawn by `towers.wgsl`.
//!
//! From afar a tower is a near-black block with a faint edge, so towers in
//! front hide towers behind and the skyline stays legible. Its faces are a
//! dense texture of data cells (a lit share for CPU); hovering or choosing
//! a tower brings the cells up, showing it is made of data. Kernel towers
//! are a heavier material: a finer, denser cell grid in banded courses.

use crate::camera::JackIn;
use crate::layout::{BlockKind, Column};
use crate::palette::{self, linear};
use crate::*;
use bevy::asset::{RenderAssetUsages, embedded_asset};
use bevy::camera::visibility::NoFrustumCulling;
use bevy::mesh::{Indices, MeshVertexBufferLayoutRef};
use bevy::pbr::{MaterialPipeline, MaterialPipelineKey};
use bevy::reflect::TypePath;
use bevy::render::render_resource::{
    AsBindGroup, PrimitiveTopology, RenderPipelineDescriptor, ShaderType,
    SpecializedMeshPipelineError,
};
use bevy::shader::ShaderRef;
use bw_model::Realm;

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/towers.wgsl");
    app.add_plugins(MaterialPlugin::<TowerMaterial>::default())
        .add_systems(Startup, setup)
        .add_systems(
            Update,
            (rebuild, update_params).chain().in_set(SceneSet::Visuals),
        );
}

/// Realm codes in the shader.
const USER: f32 = 0.0;
const KERNEL: f32 = 1.0;
/// The kernel's own memory: one heavy slab.
const KERNEL_MEMORY: f32 = 2.0;
/// Added to the realm code while a tower dissolves.
const DYING: f32 = 10.0;

/// Ids for blocks (not processes), above any column index.
pub(crate) const BLOCK_ID_BASE: usize = 2_000_000;
const DISSOLVE_SECS: f32 = 1.2;

#[derive(ShaderType, Clone, Debug, Default)]
pub struct TowerParams {
    /// x: time, y: selected id, z: hovered id, w: open id (shows edges only).
    pub a: Vec4,
    /// x: fog start, y: fog end (from the eye), z: issues only, w: reduced motion.
    pub b: Vec4,
    /// x: vignette strength.
    pub c: Vec4,
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug, Default)]
pub struct TowerMaterial {
    #[uniform(0)]
    pub params: TowerParams,
}

impl Material for TowerMaterial {
    fn vertex_shader() -> ShaderRef {
        "embedded://bw_scene/shaders/towers.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://bw_scene/shaders/towers.wgsl".into()
    }

    fn specialize(
        _: &MaterialPipeline,
        descriptor: &mut RenderPipelineDescriptor,
        _: &MeshVertexBufferLayoutRef,
        _: MaterialPipelineKey<Self>,
    ) -> Result<(), SpecializedMeshPipelineError> {
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}

#[derive(Resource)]
pub(crate) struct TowerHandles {
    mesh: Handle<Mesh>,
    pub(crate) mat: Handle<TowerMaterial>,
}

#[derive(Default)]
struct BoxMesh {
    pos: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    uv_b: Vec<[f32; 2]>,
    tangent: Vec<[f32; 4]>,
    color: Vec<[f32; 4]>,
    idx: Vec<u32>,
}

/// One tower to draw.
struct Spec {
    min: Vec2,
    max: Vec2,
    h: f32,
    id: f32,
    realm: f32,
    rgb: [f32; 3],
    cpu: f32,
    time: f32,
}

impl BoxMesh {
    /// Four walls and a roof. Each face carries its own coordinates in
    /// world units (along the face, height) and its size, so the shader can
    /// draw cells and edges at a constant scale.
    fn tower(&mut self, s: &Spec) {
        let (a, b) = (s.min, s.max);
        let corners = [
            Vec3::new(a.x, 0.0, b.y),
            Vec3::new(b.x, 0.0, b.y),
            Vec3::new(b.x, 0.0, a.y),
            Vec3::new(a.x, 0.0, a.y),
        ];
        let col = [s.rgb[0], s.rgb[1], s.rgb[2], s.cpu];
        for i in 0..4 {
            let (p, q) = (corners[i], corners[(i + 1) % 4]);
            let w = p.distance(q);
            let n = (q - p).cross(Vec3::Y).normalize_or_zero();
            let base = self.pos.len() as u32;
            for (pt, u, v) in [
                (p, 0.0, 0.0),
                (q, w, 0.0),
                (q + Vec3::Y * s.h, w, s.h),
                (p + Vec3::Y * s.h, 0.0, s.h),
            ] {
                self.pos.push(pt.to_array());
                self.normal.push(n.to_array());
                self.uv.push([u, v]);
                self.uv_b.push([s.id, s.realm]);
                self.tangent.push([w, s.h, 0.0, s.time]);
                self.color.push(col);
            }
            self.idx.extend([0, 1, 2, 0, 2, 3].map(|v| base + v));
        }
        let base = self.pos.len() as u32;
        let size = b - a;
        for (x, z) in [(a.x, a.y), (b.x, a.y), (b.x, b.y), (a.x, b.y)] {
            self.pos.push([x, s.h, z]);
            self.normal.push([0.0, 1.0, 0.0]);
            self.uv.push([x - a.x, z - a.y]);
            self.uv_b.push([s.id, s.realm]);
            self.tangent.push([size.x, size.y, 1.0, s.time]);
            self.color.push(col);
        }
        self.idx.extend([0, 2, 1, 0, 3, 2].map(|v| base + v));
    }

    fn into_mesh(mut self) -> Mesh {
        if self.pos.is_empty() {
            self.tower(&Spec {
                min: Vec2::splat(-0.01),
                max: Vec2::splat(0.01),
                h: 0.01,
                id: -9.0,
                realm: USER,
                rgb: [0.0; 3],
                cpu: 0.0,
                time: 0.0,
            });
        }
        Mesh::new(
            PrimitiveTopology::TriangleList,
            RenderAssetUsages::default(),
        )
        .with_inserted_attribute(Mesh::ATTRIBUTE_POSITION, self.pos)
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normal)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_0, self.uv)
        .with_inserted_attribute(Mesh::ATTRIBUTE_UV_1, self.uv_b)
        .with_inserted_attribute(Mesh::ATTRIBUTE_TANGENT, self.tangent)
        .with_inserted_attribute(Mesh::ATTRIBUTE_COLOR, self.color)
        .with_inserted_indices(Indices::U32(self.idx))
    }
}

fn setup(
    mut commands: Commands,
    mut meshes: ResMut<Assets<Mesh>>,
    mut mats: ResMut<Assets<TowerMaterial>>,
) {
    let mesh = meshes.add(BoxMesh::default().into_mesh());
    let mat = mats.add(TowerMaterial::default());
    commands.spawn((
        Mesh3d(mesh.clone()),
        MeshMaterial3d(mat.clone()),
        Transform::default(),
        NoFrustumCulling,
    ));
    commands.insert_resource(TowerHandles { mesh, mat });
}

/// Memory in one block of a plot.
const BLOCK_MB: u64 = 128;

/// The blocks standing on a plot: a near-square grid of `ceil(MB / 128)`
/// cells (at most 64), each a little apart, heights varying a little
/// around the process's height so the cluster reads as one building of
/// many parts. The tallest is the plot's height.
pub(crate) fn blocks(c: &Column, mem: u64) -> Vec<(Vec2, Vec2, f32)> {
    let n = (mem / (BLOCK_MB << 20) + 1).clamp(1, 64) as usize;
    let size = c.max - c.min;
    // Columns along the longer side, so blocks stay near square.
    let cols = ((n as f32 * size.x / size.y.max(1e-3)).sqrt().round() as usize).clamp(1, n);
    let rows = n.div_ceil(cols);
    let cell = Vec2::new(size.x / cols as f32, size.y / rows as f32);
    let gap = (cell.min_element() * 0.08).min(0.06);
    let seed = c.key.pid as f32 * 0.618;
    (0..n)
        .map(|i| {
            let (r, k) = (i / cols, i % cols);
            let min = c.min + Vec2::new(k as f32, r as f32) * cell + Vec2::splat(gap * 0.5);
            let max = min + cell - Vec2::splat(gap);
            let jitter = if n == 1 {
                1.0
            } else {
                0.72 + 0.28
                    * ((i as f32 * 12.9898 + seed).sin() * 43_758.547)
                        .fract()
                        .abs()
            };
            (min, max, c.height() * jitter)
        })
        .collect()
}

/// A tower kept after its process exited, so it can dissolve.
#[derive(Clone)]
struct Ghost {
    column: Column,
    rgb: [f32; 3],
    died: f32,
}

#[allow(clippy::too_many_arguments)]
fn rebuild(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    time: Res<Time>,
    jack: Res<JackIn>,
    settings: Res<SceneSettings>,
    handles: Res<TowerHandles>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut born: Local<HashMap<ProcKey, f32>>,
    mut last_gen: Local<u64>,
    mut last_seen: Local<HashMap<ProcKey, Ghost>>,
    mut ghosts: Local<Vec<Ghost>>,
) {
    if m.generation == *last_gen && !settings.is_changed() {
        return;
    }
    let fresh = m.generation != *last_gen;
    *last_gen = m.generation;
    let now = time.elapsed_secs();
    let first = born.is_empty();
    let start = if first { now + jack.city_delay() } else { now };
    if fresh {
        for (k, g) in last_seen.drain() {
            if !m.snapshot.processes.contains_key(&k) {
                ghosts.push(Ghost { died: now, ..g });
            }
        }
    }
    ghosts.retain(|g| now - g.died < DISSOLVE_SECS);
    born.retain(|k, _| m.snapshot.processes.contains_key(k));

    let lay = &sl.layout;
    let mut bm = BoxMesh::default();
    for (id, c) in lay.columns.iter().enumerate() {
        let Some(p) = m.snapshot.processes.get(&c.key) else {
            continue;
        };
        if c.realm == Realm::Kernel && !settings.show_kernel {
            continue;
        }
        // On the first data the city rises from the center outward.
        let b = *born.entry(c.key).or_insert(if first {
            start + c.base().length() / lay.extent.max(1.0) * 1.2
        } else {
            now
        });
        let rgb = crate::city::health_color(p.health(), c.realm);
        // A plot holds one block per 128 MB: a big process is a crowded
        // cluster you can count, a small one a single tower.
        for (min, max, h) in blocks(c, p.mem_bytes) {
            bm.tower(&Spec {
                min,
                max,
                h,
                id: id as f32,
                realm: if c.realm == Realm::Kernel {
                    KERNEL
                } else {
                    USER
                },
                rgb,
                cpu: (p.cpu_pct / 100.0).clamp(0.0, 1.0),
                time: b,
            });
        }
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
        bm.tower(&Spec {
            min: g.column.min,
            max: g.column.max,
            h: g.column.height(),
            id: -5.0,
            realm: USER + DYING,
            rgb: g.rgb,
            cpu: 0.0,
            time: g.died,
        });
    }
    // The kernel's own memory: a low, heavy slab.
    for (i, bl) in lay.blocks.iter().enumerate() {
        if bl.kind == BlockKind::KernelMemory && settings.show_kernel {
            let (a, b) = (bl.min + Vec2::splat(0.05), bl.max - Vec2::splat(0.05));
            bm.tower(&Spec {
                min: a,
                max: b,
                h: 0.9 + m.snapshot.system.kernel_pressure / 100.0 * 2.0,
                id: (BLOCK_ID_BASE + i) as f32,
                realm: KERNEL_MEMORY,
                rgb: linear(palette::KERNEL),
                cpu: m.snapshot.system.kernel_pressure / 100.0,
                time: if first { start } else { 0.0 },
            });
        }
    }
    if let Some(mut mesh) = meshes.get_mut(&handles.mesh) {
        *mesh = bm.into_mesh();
    }
}

#[allow(clippy::too_many_arguments)]
fn update_params(
    sl: Res<SceneLayout>,
    sel: Res<Selection>,
    settings: Res<SceneSettings>,
    time: Res<Time>,
    cam: Res<crate::camera::OrbitCam>,
    ex: Res<crate::explore::Explore>,
    handles: Res<TowerHandles>,
    mut mats: ResMut<Assets<TowerMaterial>>,
) {
    let id = |k: Option<ProcKey>| {
        k.and_then(|k| sl.layout.column_index(&k))
            .map_or(-1.0, |i| i as f32)
    };
    let Some(mut mat) = mats.get_mut(&handles.mat) else {
        return;
    };
    let t = if settings.reduced_motion {
        1e5
    } else {
        time.elapsed_secs()
    };
    mat.params.a = Vec4::new(t, id(sel.key), id(sel.hovered), id(ex.inside()));
    let (fs, fe) = crate::camera::fog(&cam);
    mat.params.b = Vec4::new(
        fs,
        fe,
        if settings.issues_only { 1.0 } else { 0.0 },
        if settings.reduced_motion { 1.0 } else { 0.0 },
    );
    // Cutaway: towers nearer than the chosen one show only their outline.
    let cut = if sel.key.is_some() {
        cam.eye().distance(cam.focus) * 0.8
    } else {
        0.0
    };
    mat.params.c = Vec4::new(1.0, cut, 0.0, 0.0);
}
