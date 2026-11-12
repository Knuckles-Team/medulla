//! Receipt writer shared by the benchmark receipts (MEDULLA-DURABLE-KERNEL-R042).
//! A receipt is one JSON object written to `<root>/<release>/<case>.json`.

use std::io;
use std::path::{Path, PathBuf};

/// Write `receipt` to `<root>/<release>/<case>.json` and return the path. The
/// release and case names are path segments: anything that would leave `root`
/// is refused.
pub fn write_receipt(
    root: &Path,
    release: &str,
    case: &str,
    receipt: &serde_json::Value,
) -> io::Result<PathBuf> {
    for segment in [release, case] {
        if segment.is_empty() || segment.contains(['/', '\\']) || segment == ".." || segment == "."
        {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                format!("receipt path segment {segment:?} is not a plain name"),
            ));
        }
    }
    let dir = root.join(release);
    std::fs::create_dir_all(&dir)?;
    let path = dir.join(format!("{case}.json"));
    let text = serde_json::to_string_pretty(receipt).map_err(io::Error::other)?;
    std::fs::write(&path, text)?;
    Ok(path)
}
