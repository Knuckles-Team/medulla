//! Print the owner-store format history rendered from the layout lineage
//! registry. Nothing is written or committed (MEDULLA-SDLC-R020.1).

fn main() {
    print!("{}", medulla_storage::render_owner_store_formats());
}
