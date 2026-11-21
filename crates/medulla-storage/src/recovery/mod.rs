//! Adoption, backup, restore, and recovery validation for one owner file.

pub(crate) mod adopt;
pub(crate) mod archive;
#[cfg(feature = "object-archive")]
pub(crate) mod archiver;
pub(crate) mod authority;
pub(crate) mod backup;
pub(crate) mod evidence;
pub(crate) mod semantic_merge;
pub(crate) mod validate;
