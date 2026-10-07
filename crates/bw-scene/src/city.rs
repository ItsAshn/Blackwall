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
use crate::layout::{self as lay, Column, LEVEL_H, WINDOW};
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

/// How high issue beacons climb: well above the tallest tower, so a problem
/// can be seen from anywhere in the city.
const BEACON_RISE: f32 = 30.0;

/// How long an exited process takes to dissolve.
const DISSOLVE_SECS: f32 = 1.2;
/// Upper bound on floor dots; above it each dot stands for more RAM.
const MAX_FLOOR_DOTS: u64 = 12_000;

/// A dot's quad is this many times its core's half-size: the rest is halo.
const GLOW: f32 = 4.0;

/// Volume columns get ids above any process column.
pub const VOLUME_ID_BASE: usize = 1_000_000;

#[derive(Default)]
pub(crate) struct DotMesh {
    pos: Vec<[f32; 3]>,
    normal: Vec<[f32; 3]>,
    uv: Vec<[f32; 2]>,
    uv_b: Vec<[f32; 2]>,
    color: Vec<[f32; 4]>,
    idx: Vec<u32>,
}

impl DotMesh {
    /// One point of light: a camera-facing quad that `dots.wgsl` turns into
    /// a hot round core inside a soft halo. All four corners share the dot's
    /// center; the normal carries the corner and the quad's half-size, and the
    /// vertex shader spreads them out to face the camera.
    pub(crate) fn dot(&mut self, c: Vec3, h: f32, uv: [f32; 2], uv_b: [f32; 2], color: [f32; 4]) {
        let base = self.pos.len() as u32;
        let size = h * GLOW;
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

/// What a column looked like, kept so it can dissolve after its process exits.
#[derive(Clone)]
struct Ghost {
    column: Column,
    rgb: [f32; 3],
    died: f32,
}

/// A tower as a brutalist facade: windows on its four faces only, one row
/// per level. The shader lights a share of the windows equal to the CPU; the
/// rest stay barely visible, so the mass (memory) still reads. At each memory
/// ledge (10 MB, 100 MB, 1 GB…) a ledge line runs round the tower and it
/// steps in by a window, alternating sides; the roof gets a ledge too.
pub(crate) fn tower_dots(
    m: &mut DotMesh,
    id: usize,
    c: &Column,
    rgb: [f32; 3],
    kind: f32,
    cpu: f32,
    time: f32,
) {
    let half = if c.realm == Realm::Kernel {
        0.035
    } else {
        0.042
    };
    let col = [rgb[0], rgb[1], rgb[2], time];
    let ledges: Vec<u32> = lay::ledges(c.levels).collect();
    // The block still standing: offset and size in windows.
    let (mut ox, mut oz, mut ex, mut ez) = (0, 0, c.wx as i32, c.wz as i32);
    let first = c.base - Vec3::new(c.half().x, 0.0, c.half().y)
        + Vec3::new(WINDOW / 2.0, 0.0, WINDOW / 2.0);
    let ledge = |m: &mut DotMesh, ox: i32, oz: i32, ex: i32, ez: i32, level: u32| {
        let y = level as f32 * LEVEL_H - 0.05;
        let lo = first
            + Vec3::new(
                (ox as f32 - 0.5) * WINDOW - 0.04,
                y,
                (oz as f32 - 0.5) * WINDOW - 0.04,
            );
        let hi = first
            + Vec3::new(
                ((ox + ex) as f32 - 0.5) * WINDOW + 0.04,
                y,
                ((oz + ez) as f32 - 0.5) * WINDOW + 0.04,
            );
        let hf = level as f32 / c.levels as f32;
        let path = [
            lo,
            Vec3::new(hi.x, lo.y, lo.z),
            Vec3::new(hi.x, lo.y, hi.z),
            Vec3::new(lo.x, lo.y, hi.z),
            lo,
        ];
        interior::along(&path, 0.1, |p, _| {
            m.dot(p, half * 0.55, [0.0, hf], [id as f32, KIND_LEDGE], col)
        });
    };
    let mut step = 0;
    for level in 0..c.levels {
        if ledges.contains(&level) {
            ledge(m, ox, oz, ex, ez, level);
            // Step in by one window: x on even ledges, z on odd ones.
            if step % 2 == 0 {
                if ex > 2 {
                    ex -= 1;
                    ox += step / 2 % 2;
                }
            } else if ez > 2 {
                ez -= 1;
                oz += step / 2 % 2;
            }
            step += 1;
        }
        let hfrac = (level as f32 + 0.5) / c.levels as f32;
        for ix in 0..ex {
            for iz in 0..ez {
                if ix > 0 && ix < ex - 1 && iz > 0 && iz < ez - 1 {
                    continue;
                }
                let p = first
                    + Vec3::new(
                        (ox + ix) as f32 * WINDOW,
                        level as f32 * LEVEL_H + half,
                        (oz + iz) as f32 * WINDOW,
                    );
                m.dot(p, half, [cpu, hfrac], [id as f32, kind], col);
            }
        }
    }
    ledge(m, ox, oz, ex, ez, c.levels);
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
        // The share of windows lit: CPU, linear, so it can be read.
        (p.cpu_pct / 100.0).clamp(0.0, 1.0),
    )
}

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
    mut last_seen: Local<HashMap<ProcKey, Ghost>>,
    mut ghosts: Local<Vec<Ghost>>,
) {
    if m.generation == *last_gen && !settings.is_changed() {
        return;
    }
    let fresh = m.generation != *last_gen;
    *last_gen = m.generation;
    let now = time.elapsed_secs();
    let first = born.0.is_empty();
    // On the first frame the city resolves out of black row by row from the
    // Wall outward, after the jack-in's fall through the rain.
    let start = if first { now + jack.city_delay() } else { now };

    // Processes that exited since the last update start dissolving.
    if fresh {
        for (k, g) in last_seen.drain() {
            if !m.snapshot.processes.contains_key(&k) {
                ghosts.push(Ghost { died: now, ..g });
            }
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
        tower_dots(&mut dm, id, c, rgb, kind, cpu, b);
        let health = p.health();
        if health != Health::Healthy && c.realm == Realm::User {
            beacon(&mut dm, id, c.top(), BEACON_RISE, health, b);
        }
        // Family: a cable from the parent's tower to this one.
        if let Some(pc) = p
            .parent
            .and_then(|pk| sl.layout.column(&pk))
            .filter(|pc| settings.show_links && pc.realm == Realm::User && c.realm == Realm::User)
        {
            let h = pc.height().min(c.height()) * 0.85;
            cable(
                &mut dm,
                id,
                pc.base + Vec3::Y * h,
                c.base + Vec3::Y * h,
                health_color(Health::Healthy, Realm::User),
                b,
            );
        }
        // Disk IO: a conduit along the floor to the storage it lands on.
        let io = (p.io_read_bytes + p.io_write_bytes) as f32;
        if settings.show_streams
            && c.realm == Realm::User
            && !m.snapshot.volumes.is_empty()
            && (io > 4096.0 || p.state == bw_model::ProcState::DiskWait)
        {
            // Down the tower's face, along its alley, then up the city's
            // west edge in a trunk of parallel lanes to the volumes.
            let v = sl.layout.volume_base(0);
            let y = 0.03;
            let lane = ((p.key.pid % 9) as f32 - 4.0) * 0.06;
            let face = c.base + Vec3::new(lane * 0.5, 0.0, c.half().y + 0.06);
            let alley = face.z + 0.42 + lane * 0.3;
            let trunk = sl.layout.min.x - 0.6 + lane;
            let path = [
                face + Vec3::Y * c.height() * 0.5,
                face + Vec3::Y * y,
                Vec3::new(face.x, y, alley),
                Vec3::new(trunk, y, alley),
                Vec3::new(trunk, y, v.z + lane),
                Vec3::new(v.x, y, v.z + lane),
            ];
            let rate = ((io / 4096.0).max(1.0).log10() / 4.0).clamp(0.0, 1.0);
            conduit(&mut dm, id, &path, rate, rgb, b);
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
        tower_dots(
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
        dm.dot(
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
    // Under reduced motion time stops, far enough in that every column has grown.
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
    // Fog from the eye, scaled to how far out the camera stands: close in,
    // the city crowds round and its far side is lost in the dark.
    let cd = cam.dist;
    mat.params.d = if ex.inside().is_some() {
        Vec4::new(cd * 1.1 + 16.0, cd * 2.4 + 45.0, 0.0, 0.0)
    } else {
        Vec4::new(cd * 0.8 + 4.0, cd * 1.9 + 16.0, 0.0, 0.0)
    };
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
    snap: &bw_model::Snapshot,
    born: f32,
    _now: f32,
) -> Mesh {
    let mut m = DotMesh::default();
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
            ElementKind::Satellite { child } => {
                if let Some(c) = snap.processes.get(child) {
                    let base = (e.min + e.max) * 0.5;
                    let column = Column::for_process(c, Vec3::new(base.x, 0.0, base.z));
                    let (crgb, ckind, cpu) = process_look(c, c.realm);
                    tower_dots(&mut m, id, &column, crgb, ckind, cpu, b);
                }
                if let [a, z] = e.path[..] {
                    cable(&mut m, id, a, z, rgb, b);
                }
            }
        }
        // Anything wrong raises a beacon over the whole interior.
        if e.health != Health::Healthy {
            let from = Vec3::new(e.anchor.x, e.max.y, e.anchor.z);
            beacon(&mut m, id, from, it.height + 20.0 - from.y, e.health, b);
        }
    }
    m.into_mesh()
}
