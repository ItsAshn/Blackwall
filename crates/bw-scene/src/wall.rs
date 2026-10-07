//! The Blackwall material: falling rain (see `shaders/wall.wgsl`).

use crate::palette;
use bevy::{
    asset::embedded_asset,
    prelude::*,
    reflect::TypePath,
    render::render_resource::{AsBindGroup, ShaderType},
    shader::ShaderRef,
};

pub(crate) fn plugin(app: &mut App) {
    embedded_asset!(app, "shaders/wall.wgsl");
    app.add_plugins(MaterialPlugin::<WallMaterial>::default());
}

#[derive(ShaderType, Clone, Debug)]
pub struct WallParams {
    pub calm: Vec4,
    pub hot: Vec4,
    pub head: Vec4,
    /// x: pressure, y: time, z: glitch on, w: intensity.
    pub state: Vec4,
    /// x: fall-speed boost (arrival), y: wall height, z: gate count.
    pub extra: Vec4,
    /// Gate openings: (x, z, half width, height).
    pub gates: [Vec4; 24],
}

#[derive(Asset, TypePath, AsBindGroup, Clone, Debug)]
pub struct WallMaterial {
    #[uniform(0)]
    pub params: WallParams,
}

impl Default for WallMaterial {
    fn default() -> Self {
        Self {
            params: WallParams {
                calm: palette::linear4(palette::WALL_CALM, 1.0),
                hot: palette::linear4(palette::WALL_HOT, 1.0),
                head: palette::linear4(palette::WALL_HEAD, 1.0),
                state: Vec4::new(0.0, 0.0, 1.0, 1.0),
                extra: Vec4::new(1.0, 34.0, 0.0, 0.0),
                gates: [Vec4::ZERO; 24],
            },
        }
    }
}

impl Material for WallMaterial {
    fn fragment_shader() -> ShaderRef {
        "embedded://bw_scene/shaders/wall.wgsl".into()
    }

    fn alpha_mode(&self) -> AlphaMode {
        AlphaMode::Add
    }

    fn specialize(
        _: &bevy::pbr::MaterialPipeline,
        descriptor: &mut bevy::render::render_resource::RenderPipelineDescriptor,
        _: &bevy::mesh::MeshVertexBufferLayoutRef,
        _: bevy::pbr::MaterialPipelineKey<Self>,
    ) -> Result<(), bevy::render::render_resource::SpecializedMeshPipelineError> {
        // Seen from inside and out.
        descriptor.primitive.cull_mode = None;
        Ok(())
    }
}
