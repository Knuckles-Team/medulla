//! Object-store namespace of the storage authority.
//!
//! `archive` holds the archive-segment seam of
//! MEDULLA-OBJECT-STORAGE-BACKEND-R006, `fetch` the read path (R002) and
//! `commit` the commit-before-ack path (R003), `counting` the action counter
//! of the comparative cost gate (R005); each is a single declaration
//! point over the shared `object_store` trait.

pub mod archive;
pub mod commit;
pub mod counting;
pub mod fetch;
