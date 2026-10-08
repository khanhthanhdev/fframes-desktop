/// Cache limits shared by Skia frame previews and video exports.
///
/// Limits apply independently to each worker. Geometry uses two generations, so
/// its estimated retained bytes can reach twice `geometry_bytes`. Hash-map and
/// allocator overhead are additional. Cached geometry is allocated as it is used.
///
/// ```
/// use fframes_skia_renderer::{SkiaCacheConfig, SkiaPipelineConfig};
///
/// let pipeline = SkiaPipelineConfig {
///     cache: SkiaCacheConfig {
///         text_capacity: 100_000,
///         geometry_capacity: 100_000,
///         geometry_bytes: 100_000 * 512,
///     },
///     ..Default::default()
/// };
/// ```
#[derive(Debug, Clone, Copy)]
pub struct SkiaCacheConfig {
    /// Maximum positioned SVG text layouts retained by each converter.
    /// Zero disables SVG text caching. The default is 10 layouts.
    pub text_capacity: usize,
    /// Maximum dynamic path geometries retained per generation.
    /// Zero disables dynamic geometry caching. The byte limit also applies.
    pub geometry_capacity: usize,
    /// Estimated bytes of dynamic geometry retained per generation.
    /// Includes source geometry and its Skia conversion. The default is 4 MiB.
    pub geometry_bytes: usize,
}

impl Default for SkiaCacheConfig {
    fn default() -> Self {
        Self {
            text_capacity: 10,
            geometry_capacity: usize::MAX,
            geometry_bytes: 4 * 1024 * 1024,
        }
    }
}
