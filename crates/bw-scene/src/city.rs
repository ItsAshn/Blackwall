//! The chamber's dots: one mesh for every process, kernel and volume column,
//! one for the RAM floor, both drawn by `dots.wgsl`.
//!
//! The meshes are rebuilt when data arrives (about once a second); all
//! animation runs in the shader, so tens of thousands of dots stay cheap.
//! The floor is rebuilt only when the city's footprint or the machine's RAM
//! changes: how much of it is lit is a shader uniform.

use crate::camera::JackIn;
use crate::explore::MachineLayer;
use crate::interior::{self, ELEMENT_ID_BASE, ElementKind, Interior};
use crate::layout::LEVEL_H;
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
use bw_model::{Health, Realm};

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
    fn vertex_shader() -> ShaderRef {
        "embedded://bw_scene/shaders/dots.wgsl".into()
    }

    fn fragment_shader() -> ShaderRef {
        "embedded://bw_scene/shaders/dots.wgsl".into()
    }

    /// Light adds to light: overlapping halos build up like neon.
    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
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
pub(crate) struct CityHandles {
    city: Handle<Mesh>,
    floor: Handle<Mesh>,
    pub(crate) mat: Handle<DotsMaterial>,
}

const KIND_PROCESS: f32 = 0.0;
const KIND_VOLUME: f32 = 2.0;
const KIND_FLOOR: f32 = 3.0;
const KIND_CRITICAL: f32 = 4.0;
const KIND_FLOW: f32 = 6.0;
const KIND_WATCH: f32 = 7.0;
const KIND_BEACON: f32 = 8.0;
const KIND_CABLE: f32 = 9.0;
const KIND_LEDGE: f32 = 10.0;
const KIND_STRATUM: f32 = 11.0;

/// How high issue beacons climb: well above the tallest tower, so a problem
/// can be seen from anywhere in the city.
const BEACON_RISE: f32 = 30.0;

/// A dot's quad is this many times its core's half-size: the rest is halo.
const GLOW: f32 = 4.0;

/// Volume columns get ids above any process column.
pub const VOLUME_ID_BASE: usize = 1_000_000;

pub(crate) struct DotMesh {
    pos: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    uv_b: Vec<[f32; 2]>,
    color: Vec<[f32; 4]>,
    idx: Vec<u32>,
    /// Where the dots are placed: positions and sizes are scaled by `scale`
    /// and moved to `origin` (an interior standing in its tower's plot).
    origin: Vec3,
    scale: f32,
}

impl Default for DotMesh {
    fn default() -> Self {
        Self::framed(Vec3::ZERO, 1.0)
    }
}

impl DotMesh {
    pub(crate) fn framed(origin: Vec3, scale: f32) -> Self {
        Self {
            pos: vec![],
            normal: vec![],
            uv: vec![],
            uv_b: vec![],
            color: vec![],
            idx: vec![],
            origin,
            scale,
        }
    }

    /// One point of light: a camera-facing quad that `dots.wgsl` turns into
    /// a hot round core inside a soft halo. All four corners share the dot's
    /// center; the normal carries the corner and the quad's half-size, and the
    /// vertex shader spreads them out to face the camera.
    pub(crate) fn dot(&mut self, c: Vec3, h: f32, uv: [f32; 2], uv_b: [f32; 2], color: [f32; 4]) {
        let base = self.pos.len() as u32;
        let size = h * GLOW * self.scale;
        let c = self.origin + c * self.scale;
        for (x, y) in [(-1.0, -1.0), (1.0, -1.0), (1.0, 1.0), (-1.0, 1.0)] {
            self.pos.push(c.to_array());
            self.normal.push([x, y, size]);
            self.uv.push(uv);
            self.uv_b.push(uv_b);
            self.color.push(color);
        }
        self.idx.extend([0, 1, 2, 0, 2, 3].map(|v| base + v));
    }

    pub(crate) fn into_mesh(mut self) -> Mesh {
        // Never upload an empty mesh: park one dot far below the floor.
        if self.pos.is_empty() {
            self.dot(
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
        .with_inserted_attribute(Mesh::ATTRIBUTE_NORMAL, self.normal)
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
            MachineLayer,
        ));
    }
    commands.insert_resource(CityHandles { city, floor, mat });
}

/// A beam of light climbing from `from` into the dark: issues are visible
/// from anywhere. Critical beams are brighter and faster than watch beams.
pub(crate) fn beacon(m: &mut DotMesh, id: usize, from: Vec3, rise: f32, health: Health, time: f32) {
    let rgb = health_color(health, Realm::User);
    let crit = if health == Health::Critical { 1.0 } else { 0.0 };
    let n = (rise / 0.09) as usize;
    for i in 0..n {
        let u = i as f32 / n as f32;
        m.dot(
            from + Vec3::Y * (0.25 + u * rise),
            0.038,
            [crit, u],
            [id as f32, KIND_BEACON],
            [rgb[0], rgb[1], rgb[2], time],
        );
    }
}

/// A sagging cable of dots from `a` to `b`, with slow pulses running from a
/// to b: a relationship (parent → child) that is always there.
pub(crate) fn cable(m: &mut DotMesh, id: usize, a: Vec3, b: Vec3, rgb: [f32; 3], time: f32) {
    let len = a.distance(b);
    let sag = 0.12 * len + 0.15;
    let n = (len / 0.11).max(2.0) as usize;
    let pulses = (len / 3.0).max(1.0);
    for i in 0..=n {
        let u = i as f32 / n as f32;
        let p = a.lerp(b, u) - Vec3::Y * sag * 4.0 * u * (1.0 - u);
        m.dot(
            p,
            0.022,
            [pulses, u],
            [id as f32, KIND_CABLE],
            [rgb[0], rgb[1], rgb[2], time],
        );
    }
}

/// A conduit of flowing dots along a polyline; `rate` 0..1 sets how dense
/// the pulses run (IO volume).
pub(crate) fn conduit(
    m: &mut DotMesh,
    id: usize,
    path: &[Vec3],
    rate: f32,
    rgb: [f32; 3],
    time: f32,
) {
    let len: f32 = path.windows(2).map(|w| w[0].distance(w[1])).sum();
    let pulses = (len / 2.5 * (0.3 + rate * 2.2)).max(1.0);
    interior::along(path, 0.09, |p, u| {
        m.dot(
            p,
            0.026,
            [pulses, u],
            [id as f32, KIND_FLOW],
            [rgb[0], rgb[1], rgb[2], time],
        )
    });
}

/// The light layer over the solid towers: beacons over issues, family
/// cables between towers, disk-IO conduits to the volumes, the volumes.
#[allow(clippy::too_many_arguments)]
fn rebuild_city(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    time: Res<Time>,
    jack: Res<JackIn>,
    handles: Res<CityHandles>,
    settings: Res<SceneSettings>,
    mut born: ResMut<Born>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last_gen: Local<u64>,
) {
    if m.generation == *last_gen && !settings.is_changed() {
        return;
    }
    *last_gen = m.generation;
    let now = time.elapsed_secs();
    let first = born.0.is_empty();
    let start = if first {
        now + jack.city_delay() + 1.0
    } else {
        now
    };
    born.0.retain(|k, _| m.snapshot.processes.contains_key(k));

    let lay = &sl.layout;
    let mut dm = DotMesh::default();
    for (id, c) in lay.columns.iter().enumerate() {
        let Some(p) = m.snapshot.processes.get(&c.key) else {
            continue;
        };
        let b = *born
            .0
            .entry(c.key)
            .or_insert(if first { start } else { now });
        let health = p.health();
        let rgb = health_color(health, c.realm);
        if health != Health::Healthy {
            beacon(&mut dm, id, c.top(), BEACON_RISE, health, b);
        }
        // Family: a cable from the parent's roof to the child's.
        if settings.show_links
            && let Some(pc) = p.parent.and_then(|pk| lay.column(&pk))
        {
            let h = pc.height().min(c.height());
            cable(
                &mut dm,
                id,
                pc.base() + Vec3::Y * h,
                c.base() + Vec3::Y * h,
                health_color(Health::Healthy, Realm::User),
                b,
            );
        }
        // Disk IO: down the tower's face and along the floor to storage.
        let io = (p.io_read_bytes + p.io_write_bytes) as f32;
        if settings.show_streams
            && !m.snapshot.volumes.is_empty()
            && (io > 4096.0 || p.state == bw_model::ProcState::DiskWait)
        {
            let v = lay.volume_base(0);
            let y = 0.03;
            let lane = ((p.key.pid % 9) as f32 - 4.0) * 0.07;
            let foot = Vec3::new(c.base().x + lane * 0.3, y, c.max.y + 0.04);
            let path = [
                foot + Vec3::Y * c.height() * 0.5,
                foot,
                Vec3::new(foot.x, y, lay.max.y + 0.9 + lane),
                Vec3::new(v.x + lane, y, lay.max.y + 0.9 + lane),
                Vec3::new(v.x + lane, y, v.z),
            ];
            let rate = ((io / 4096.0).max(1.0).log10() / 4.0).clamp(0.0, 1.0);
            conduit(&mut dm, id, &path, rate, rgb, b);
        }
    }
    // Volumes: dense 3×3 columns outside the RAM square; lit dots are used
    // space, the rest dot-off.
    for (i, v) in m.snapshot.volumes.iter().enumerate() {
        let base = lay.volume_base(i);
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
                    dm.dot(
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

/// The floor is the machine's RAM: a square of dots under the whole map.
/// Dots under memory in use are faintly lit; free memory is unlit ground.
fn rebuild_floor(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    handles: Res<CityHandles>,
    mut info: ResMut<FloorInfo>,
    mut meshes: ResMut<Assets<Mesh>>,
    mut last: Local<u64>,
) {
    if m.generation == *last || sl.layout.extent <= 0.0 {
        return;
    }
    *last = m.generation;
    let lay = &sl.layout;
    let side = lay.extent * 2.0;
    let n = 110usize;
    let pitch = side / n as f32;
    let free: Vec<&crate::layout::Block> = lay
        .blocks
        .iter()
        .filter(|b| b.kind == crate::layout::BlockKind::Free)
        .collect();
    let ok = health_color(Health::Healthy, Realm::User);
    let mut dm = DotMesh::default();
    for ix in 0..n {
        for iz in 0..n {
            let p = lay.min + Vec2::new(ix as f32 + 0.5, iz as f32 + 0.5) * pitch;
            let is_free = free
                .iter()
                .any(|b| p.cmpge(b.min).all() && p.cmplt(b.max).all());
            dm.dot(
                Vec3::new(p.x, -0.02, p.y),
                0.02,
                [if is_free { 0.99 } else { 0.0 }, 0.0],
                [-5.0, KIND_FLOOR],
                [ok[0], ok[1], ok[2], 0.0],
            );
        }
    }
    let total = m.snapshot.system.mem_total;
    *info = FloorInfo {
        mb_per_dot: total / (n * n) as u64 / 1_048_576,
        dots: n * n,
    };
    if let Some(mut mesh) = meshes.get_mut(&handles.floor) {
        *mesh = dm.into_mesh();
    }
}

#[allow(clippy::too_many_arguments)]
fn update_params(
    m: Res<Machine>,
    sl: Res<SceneLayout>,
    sel: Res<Selection>,
    settings: Res<SceneSettings>,
    quality: Res<Quality>,
    time: Res<Time>,
    handles: Res<CityHandles>,
    ex: Res<crate::explore::Explore>,
    cam: Res<crate::camera::OrbitCam>,
    mut mats: ResMut<Assets<DotsMaterial>>,
) {
    let id = |k: Option<ProcKey>| {
        k.and_then(|k| sl.layout.column_index(&k))
            .map_or(-1.0, |i| i as f32)
    };
    let Some(mut mat) = mats.get_mut(&handles.mat) else {
        return;
    };
    // Under reduced motion time stops, far enough in that everything has grown.
    let t = if settings.reduced_motion {
        1e5
    } else {
        time.elapsed_secs()
    };
    let s = &m.snapshot.system;
    let ram = s.mem_used as f32 / s.mem_total.max(1) as f32;
    mat.params.a = if ex.inside().is_some() {
        Vec4::new(
            t,
            s.kernel_pressure,
            ex.element_id(ex.element),
            ex.element_id(ex.hovered),
        )
    } else {
        Vec4::new(t, s.kernel_pressure, id(sel.key), id(sel.hovered))
    };
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
    // Fog from the eye, and a near fade scaled to the zoom.
    let (fs, fe) = crate::camera::fog(&cam);
    let near = cam.dist * 0.04;
    mat.params.d = Vec4::new(fs, fe, near, near * 4.0);
}

/// The builder for dot meshes, shared with the interior.
pub(crate) type DotMeshBuilder = DotMesh;

fn element_kind(h: Health) -> f32 {
    match h {
        Health::Critical => KIND_CRITICAL,
        Health::Warning => KIND_WATCH,
        Health::Healthy => KIND_PROCESS,
    }
}

/// The world inside one process, as dots (see `interior.rs`).
pub(crate) fn interior_mesh(
    it: &Interior,
    d: &bw_model::ProcessDetail,
    born: f32,
    _now: f32,
    origin: Vec3,
    scale: f32,
) -> Mesh {
    let mut m = DotMesh::framed(origin, scale);
    let per_face = (2.0 * interior::CORE_R / interior::CORE_WINDOW).round() as usize;
    for (i, e) in it.elements.iter().enumerate() {
        let id = ELEMENT_ID_BASE + i;
        let rgb = interior::health_rgb(e.health);
        let kind = element_kind(e.health);
        let b = born + i as f32 * 0.003;
        let col = [rgb[0], rgb[1], rgb[2], b];
        match &e.kind {
            ElementKind::Stratum { region } => {
                let r = &d.regions[*region];
                let levels = ((e.max.y - e.min.y) / LEVEL_H).round().max(1.0) as usize;
                // The bottom row of each slab stays dark: a seam between slabs.
                for lv in 1..levels {
                    let y = e.min.y + lv as f32 * LEVEL_H + 0.05;
                    for f in 0..4 {
                        let (out, side) = interior::face(f);
                        for k in 0..per_face {
                            if !interior::stratum_keep(r.kind, f * per_face + k, lv) {
                                continue;
                            }
                            let off = (k as f32 + 0.5) * interior::CORE_WINDOW - interior::CORE_R;
                            let pos = out * interior::CORE_R + side * off + Vec3::Y * y;
                            let kind = if e.health == Health::Healthy {
                                KIND_STRATUM
                            } else {
                                kind
                            };
                            m.dot(
                                pos,
                                0.045,
                                [0.5, lv as f32 / levels as f32],
                                [id as f32, kind],
                                col,
                            );
                        }
                    }
                }
                // A ledge caps the slab.
                let y = e.max.y - 0.02;
                let r = interior::CORE_R + 0.12;
                let ring = [
                    Vec3::new(-r, y, -r),
                    Vec3::new(r, y, -r),
                    Vec3::new(r, y, r),
                    Vec3::new(-r, y, r),
                    Vec3::new(-r, y, -r),
                ];
                interior::along(&ring, 0.08, |p, _| {
                    m.dot(p, 0.026, [0.0, 1.0], [id as f32, KIND_LEDGE], col)
                });
            }
            ElementKind::Floor { thread } => {
                // A cantilevered slab: its outline at two heights, windows
                // lit by the thread's CPU.
                let t = &d.threads[*thread];
                let cpu = (t.cpu_pct / 100.0).clamp(0.0, 1.0);
                let (lo, hi) = (e.min + Vec3::splat(0.12), e.max - Vec3::splat(0.12));
                // The deck: a grid of windows, so the slab has mass.
                let (nx, nz) = (
                    ((hi.x - lo.x) / 0.3).round().max(1.0) as usize,
                    ((hi.z - lo.z) / 0.3).round().max(1.0) as usize,
                );
                for ix in 1..nx {
                    for iz in 1..nz {
                        let p = Vec3::new(
                            lo.x + (hi.x - lo.x) * ix as f32 / nx as f32,
                            hi.y,
                            lo.z + (hi.z - lo.z) * iz as f32 / nz as f32,
                        );
                        m.dot(p, 0.04, [cpu, 0.5], [id as f32, kind], col);
                    }
                }
                for (row, y) in [lo.y, hi.y].into_iter().enumerate() {
                    let rect = [
                        Vec3::new(lo.x, y, lo.z),
                        Vec3::new(hi.x, y, lo.z),
                        Vec3::new(hi.x, y, hi.z),
                        Vec3::new(lo.x, y, hi.z),
                        Vec3::new(lo.x, y, lo.z),
                    ];
                    interior::along(&rect, 0.2, |p, u| {
                        m.dot(
                            p,
                            0.045,
                            [cpu, row as f32 * 0.5 + u * 0.5],
                            [id as f32, kind],
                            col,
                        )
                    });
                }
            }
            ElementKind::Conduit { .. } => {
                conduit(&mut m, id, &e.path, 0.6, rgb, b);
            }
            // Children are towers of their own on the map.
            ElementKind::Satellite { .. } => {}
        }
        // Anything wrong raises a beacon over the whole interior.
        if e.health != Health::Healthy {
            let from = Vec3::new(e.anchor.x, e.max.y, e.anchor.z);
            beacon(&mut m, id, from, it.height + 20.0 - from.y, e.health, b);
        }
    }
    m.into_mesh()
}
