//! Object-store namespace of the storage authority.
//!
//! `archive` holds the archive-segment seam of
//! MEDULLA-OBJECT-STORAGE-BACKEND-R006; read and commit paths land in later
//! rows and keep this module as their single declaration point.

pub mod archive;
pub mod fetch;
