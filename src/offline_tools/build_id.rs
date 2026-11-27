//! `medulla-server --build-id` — print the build identity of the running executable
//! (MEDULLA-CONTRACT-R060.5).
//!
//! The value is the lowercase-hex identity the toolchain embeds: the ELF
//! `NT_GNU_BUILD_ID` note, the Mach-O `LC_UUID`, or the PE CodeView GUID (16 bytes in
//! GUID field order). It is the same value `scripts/read_build_id.py` returns for the
//! same file (MEDULLA-CONTRACT-R060.2); `tests/server_build_id_flag.rs` keeps the two
//! readers equal. A build with no identity prints `BUILD_ID_MISSING` on stderr and exits
//! 1. The flag is handled before `Args::parse`, so no server setup runs.

use object::read::Object;

fn hex(bytes: &[u8]) -> String {
    use std::fmt::Write as _;
    bytes
        .iter()
        .fold(String::with_capacity(bytes.len() * 2), |mut out, byte| {
            let _ = write!(out, "{byte:02x}");
            out
        })
}

/// The build identity of the executable this process runs, or `None` when it carries none.
fn running_build_id() -> Option<String> {
    let path = std::env::current_exe().ok()?;
    let bytes = std::fs::read(path).ok()?;
    let file = object::File::parse(&*bytes).ok()?;
    if let Ok(Some(id)) = file.build_id() {
        return Some(hex(id));
    }
    if let Ok(Some(uuid)) = file.mach_uuid() {
        return Some(hex(&uuid));
    }
    if let Ok(Some(info)) = file.pdb_info() {
        return Some(hex(&info.guid()));
    }
    None
}

/// Print the identity and exit 0, or print `BUILD_ID_MISSING` and exit 1.
pub(crate) fn run() -> ! {
    match running_build_id() {
        Some(id) => {
            println!("{id}");
            std::process::exit(0);
        }
        None => {
            eprintln!("BUILD_ID_MISSING");
            std::process::exit(1);
        }
    }
}
