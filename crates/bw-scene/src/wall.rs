//! The Blackwall material.

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
    pub base: Vec4,
    pub hot: Vec4,
    /// x: pressure, y: time, z: glitch on, w: intensity.
    pub state: Vec4,
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
                base: Vec4::new(0.75, 0.1, 1.0, 1.0),
                hot: Vec4::new(1.0, 0.05, 0.12, 1.0),
                state: Vec4::new(0.0, 0.0, 1.0, 1.0),
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
}
