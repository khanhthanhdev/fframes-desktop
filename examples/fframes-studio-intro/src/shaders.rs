//! Every GPU shader of the video, compiled once by the Skia renderer.

use std::sync::LazyLock;

use fframes::Shader;

pub struct Shaders {
    pub grain: Shader,
    pub contour: Shader,
    pub grid: Shader,
}

pub static SHADERS: LazyLock<Shaders> = LazyLock::new(|| Shaders {
    grain: Shader::sksl(include_str!("shaders/grain.sksl")),
    contour: Shader::sksl(include_str!("shaders/contour.sksl")),
    grid: Shader::sksl(include_str!("shaders/grid.sksl")),
});
