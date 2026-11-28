#[cfg(feature = "redb")]
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod cluster;
#[cfg(all(test, feature = "redb", feature = "blob"))]
mod component_content_tests;
#[cfg(all(test, feature = "redb"))]
mod package_validation_tests;
// Declared in every build -- the dispatch arm that reaches it carries no cfg;
// each op refuses by name in a build without its durable substrates.
// `pub(crate)` so the projection worker shares the admin re-projection path.
#[allow(missing_docs)] // docs pending: reached across the crate split
pub mod connector_pack;

#[cfg(feature = "redb")]
pub use cluster::try_handle;
pub use connector_pack::handle_connector_pack;
pub use connector_pack::release_approval::activate_held_release;
