//! Native migration-definition storage adapters.

mod directory;
mod embedded;

pub use directory::DirectoryMigrationStore;
pub use embedded::EmbeddedMigrationStore;
pub(crate) use embedded::validate_embedded_directory;
