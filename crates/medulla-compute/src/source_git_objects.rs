//! The git object codec of source changesets (MEDULLA-SCM-R010): object ids,
//! trees, commits and version-2 packfiles, in both object formats.
//!
//! Pure functions over bytes. Import and export live above this module
//! (`medulla-h-ingest` `source_changeset::git`); the codec reads a pack a git
//! client sends (which may hold `OFS_DELTA` and `REF_DELTA` entries), writes
//! undeltified packs, and recomputes every object id so a pack that does not
//! match its ids is refused.

use std::collections::BTreeMap;
use std::io::Write;

use flate2::write::ZlibEncoder;
use flate2::{Compression, Decompress, FlushDecompress, Status};
use medulla_source_types::source_changeset::GitObjectFormat;
use sha2::Digest;

/// The mode of a tree entry that names a subdirectory, as git writes it.
pub const MODE_TREE: u32 = 0o40000;
/// Regular file.
pub const MODE_FILE: u32 = 0o100644;
/// Executable file.
pub const MODE_EXECUTABLE: u32 = 0o100755;
/// Symbolic link; the blob holds the link target.
pub const MODE_SYMLINK: u32 = 0o120000;
/// Submodule; the stored blob holds the hex commit id of the submodule.
pub const MODE_SUBMODULE: u32 = 0o160000;

/// The kind of a git object.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum GitObjectKind {
    Commit,
    Tree,
    Blob,
    Tag,
}

impl GitObjectKind {
    /// The name git hashes in the object header.
    pub fn name(self) -> &'static str {
        match self {
            Self::Commit => "commit",
            Self::Tree => "tree",
            Self::Blob => "blob",
            Self::Tag => "tag",
        }
    }

    fn pack_type(self) -> u8 {
        match self {
            Self::Commit => 1,
            Self::Tree => 2,
            Self::Blob => 3,
            Self::Tag => 4,
        }
    }

    fn from_pack_type(code: u8) -> Option<Self> {
        match code {
            1 => Some(Self::Commit),
            2 => Some(Self::Tree),
            3 => Some(Self::Blob),
            4 => Some(Self::Tag),
            _ => None,
        }
    }
}

/// Why a git object could not be built or read.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum GitObjectError {
    /// A tree entry mode outside `40000`, `100644`, `100755`, `120000`, `160000`.
    ModeUnsupported(u32),
    /// Bytes that are not a well-formed object or pack; the text says why.
    Malformed(String),
    /// The writer failed.
    Io(String),
}

impl std::fmt::Display for GitObjectError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::ModeUnsupported(mode) => write!(f, "tree entry mode {mode:o} is not supported"),
            Self::Malformed(text) => write!(f, "{text}"),
            Self::Io(text) => write!(f, "{text}"),
        }
    }
}

impl std::error::Error for GitObjectError {}

fn malformed(text: impl Into<String>) -> GitObjectError {
    GitObjectError::Malformed(text.into())
}

/// Bytes of one object id in `format`.
pub fn id_len(format: GitObjectFormat) -> usize {
    match format {
        GitObjectFormat::Sha1 => 20,
        GitObjectFormat::Sha256 => 32,
    }
}

fn digest(format: GitObjectFormat, parts: &[&[u8]]) -> Vec<u8> {
    match format {
        GitObjectFormat::Sha1 => {
            let mut hasher = sha1::Sha1::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().to_vec()
        }
        GitObjectFormat::Sha256 => {
            let mut hasher = sha2::Sha256::new();
            for part in parts {
                hasher.update(part);
            }
            hasher.finalize().to_vec()
        }
    }
}

/// The id of an object: the hex of SHA-1 or SHA-256 over `"<kind> <len>\0" + body`.
pub fn object_id(format: GitObjectFormat, kind: GitObjectKind, body: &[u8]) -> String {
    let header = format!("{} {}\0", kind.name(), body.len());
    hex::encode(digest(format, &[header.as_bytes(), body]))
}

/// One entry of a tree: `mode` is the numeric mode (`MODE_TREE` for a
/// directory) and `id` the hex object id.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct GitTreeEntry {
    pub name: String,
    pub mode: u32,
    pub id: String,
}

fn mode_allowed(mode: u32) -> bool {
    matches!(
        mode,
        MODE_TREE | MODE_FILE | MODE_EXECUTABLE | MODE_SYMLINK | MODE_SUBMODULE
    )
}

/// Build a tree object from `entries`: sorted bytewise on the name with `/`
/// appended to directory names, the directory mode written `40000`. Returns the
/// tree id and its body.
pub fn build_tree(
    format: GitObjectFormat,
    entries: &[GitTreeEntry],
) -> Result<(String, Vec<u8>), GitObjectError> {
    let mut sorted: Vec<(Vec<u8>, &GitTreeEntry)> = Vec::with_capacity(entries.len());
    for entry in entries {
        if !mode_allowed(entry.mode) {
            return Err(GitObjectError::ModeUnsupported(entry.mode));
        }
        if entry.name.is_empty() || entry.name.contains('/') || entry.name.contains('\0') {
            return Err(malformed(format!("tree entry name {:?}", entry.name)));
        }
        let mut key = entry.name.as_bytes().to_vec();
        if entry.mode == MODE_TREE {
            key.push(b'/');
        }
        sorted.push((key, entry));
    }
    sorted.sort_by(|a, b| a.0.cmp(&b.0));
    let mut body = Vec::new();
    for (_, entry) in sorted {
        let id = hex::decode(&entry.id).map_err(|_| malformed("tree entry id is not hex"))?;
        if id.len() != id_len(format) {
            return Err(malformed("tree entry id length differs from the object format"));
        }
        body.extend_from_slice(format!("{:o} ", entry.mode).as_bytes());
        body.extend_from_slice(entry.name.as_bytes());
        body.push(0);
        body.extend_from_slice(&id);
    }
    Ok((object_id(format, GitObjectKind::Tree, &body), body))
}

/// Parse a tree body into its entries, in stored order.
pub fn parse_tree(
    format: GitObjectFormat,
    body: &[u8],
) -> Result<Vec<GitTreeEntry>, GitObjectError> {
    let width = id_len(format);
    let mut entries = Vec::new();
    let mut pos = 0usize;
    while pos < body.len() {
        let space = body[pos..]
            .iter()
            .position(|byte| *byte == b' ')
            .ok_or_else(|| malformed("tree entry has no mode"))?;
        let mode_text = std::str::from_utf8(&body[pos..pos + space])
            .map_err(|_| malformed("tree entry mode is not text"))?;
        let mode = u32::from_str_radix(mode_text, 8)
            .map_err(|_| malformed("tree entry mode is not octal"))?;
        if !mode_allowed(mode) {
            return Err(GitObjectError::ModeUnsupported(mode));
        }
        pos += space + 1;
        let nul = body[pos..]
            .iter()
            .position(|byte| *byte == 0)
            .ok_or_else(|| malformed("tree entry has no name end"))?;
        let name = std::str::from_utf8(&body[pos..pos + nul])
            .map_err(|_| malformed("tree entry name is not UTF-8"))?
            .to_string();
        pos += nul + 1;
        if body.len() < pos + width {
            return Err(malformed("tree entry id is truncated"));
        }
        let id = hex::encode(&body[pos..pos + width]);
        pos += width;
        entries.push(GitTreeEntry { name, mode, id });
    }
    Ok(entries)
}

/// Build a commit body: `"tree <tree_id>\n" + raw_headers + "\n" + message`.
pub fn build_commit(tree_id: &str, raw_headers: &str, message: &str) -> Vec<u8> {
    format!("tree {tree_id}\n{raw_headers}\n{message}").into_bytes()
}

/// A parsed commit: `raw_headers` is every header line after `tree <id>\n` up
/// to the blank line, verbatim.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ParsedCommit {
    pub tree_id: String,
    pub parents: Vec<String>,
    pub raw_headers: String,
    pub committer_time_s: i64,
    pub message: String,
}

/// Parse a commit body. Headers and message must be UTF-8.
pub fn parse_commit(body: &[u8]) -> Result<ParsedCommit, GitObjectError> {
    let text = std::str::from_utf8(body).map_err(|_| malformed("commit is not UTF-8"))?;
    let split = text
        .find("\n\n")
        .ok_or_else(|| malformed("commit has no blank line after its headers"))?;
    let headers = &text[..split + 1];
    let message = text[split + 2..].to_string();
    let (first, raw_headers) = headers
        .split_once('\n')
        .ok_or_else(|| malformed("commit has no headers"))?;
    let tree_id = first
        .strip_prefix("tree ")
        .ok_or_else(|| malformed("commit does not start with a tree header"))?
        .to_string();
    let mut parents = Vec::new();
    let mut committer_time_s = None;
    for line in raw_headers.lines() {
        if let Some(parent) = line.strip_prefix("parent ") {
            parents.push(parent.to_string());
        } else if let Some(who) = line.strip_prefix("committer ") {
            let after = who
                .rsplit_once('>')
                .map(|(_, rest)| rest)
                .ok_or_else(|| malformed("committer header has no email"))?;
            let seconds = after
                .split_whitespace()
                .next()
                .and_then(|token| token.parse::<i64>().ok())
                .ok_or_else(|| malformed("committer header has no time"))?;
            committer_time_s = Some(seconds);
        }
    }
    Ok(ParsedCommit {
        tree_id,
        parents,
        raw_headers: raw_headers.to_string(),
        committer_time_s: committer_time_s.ok_or_else(|| malformed("commit has no committer"))?,
        message,
    })
}

const PACK_MAGIC: &[u8; 4] = b"PACK";
const OFS_DELTA: u8 = 6;
const REF_DELTA: u8 = 7;

/// Write a version-2 pack of `objects`: header `PACK`, version 2, the object
/// count; each object undeltified, a type-and-size header then a zlib stream;
/// the trailer is the pack hash in the object format.
pub fn write_pack<W: Write>(
    format: GitObjectFormat,
    objects: &[(GitObjectKind, Vec<u8>)],
    mut out: W,
) -> Result<(), GitObjectError> {
    let count = u32::try_from(objects.len()).map_err(|_| malformed("too many pack objects"))?;
    let mut pack: Vec<u8> = Vec::new();
    pack.extend_from_slice(PACK_MAGIC);
    pack.extend_from_slice(&2u32.to_be_bytes());
    pack.extend_from_slice(&count.to_be_bytes());
    for (kind, body) in objects {
        let mut size = body.len() as u64;
        let mut byte = (kind.pack_type() << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size > 0 {
            pack.push(byte | 0x80);
            byte = (size & 0x7f) as u8;
            size >>= 7;
        }
        pack.push(byte);
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder
            .write_all(body)
            .map_err(|error| GitObjectError::Io(error.to_string()))?;
        let deflated = encoder
            .finish()
            .map_err(|error| GitObjectError::Io(error.to_string()))?;
        pack.extend_from_slice(&deflated);
    }
    let trailer = digest(format, &[&pack]);
    pack.extend_from_slice(&trailer);
    out.write_all(&pack)
        .map_err(|error| GitObjectError::Io(error.to_string()))
}

/// What one pack entry holds before delta resolution.
enum Raw {
    Base(GitObjectKind),
    Ofs(usize),
    Ref(String),
}

struct RawEntry {
    offset: usize,
    raw: Raw,
    data: Vec<u8>,
}

/// Inflate the zlib stream at `input`, expecting `size` bytes. Returns the
/// bytes and how many input bytes the stream used.
fn inflate(input: &[u8], size: usize) -> Result<(Vec<u8>, usize), GitObjectError> {
    let mut stream = Decompress::new(true);
    let mut out: Vec<u8> = Vec::with_capacity(size.min(1 << 20) + 1);
    loop {
        let in_before = stream.total_in();
        let out_before = out.len();
        let status = stream
            .decompress_vec(&input[in_before as usize..], &mut out, FlushDecompress::Finish)
            .map_err(|error| malformed(format!("pack object stream: {error}")))?;
        if out.len() > size {
            return Err(malformed("pack object is longer than its header says"));
        }
        match status {
            Status::StreamEnd => break,
            Status::Ok | Status::BufError => {
                let progressed = stream.total_in() != in_before || out.len() != out_before;
                if out.len() == out.capacity() {
                    out.reserve(out.len().max(4096));
                } else if !progressed {
                    return Err(malformed("pack object stream is truncated"));
                }
            }
        }
    }
    if out.len() != size {
        return Err(malformed("pack object is shorter than its header says"));
    }
    Ok((out, stream.total_in() as usize))
}

fn read_varint(delta: &[u8], pos: &mut usize) -> Result<usize, GitObjectError> {
    let mut value = 0usize;
    let mut shift = 0u32;
    loop {
        let byte = *delta
            .get(*pos)
            .ok_or_else(|| malformed("delta is truncated"))?;
        *pos += 1;
        if shift > 56 {
            return Err(malformed("delta size is too large"));
        }
        value |= ((byte & 0x7f) as usize) << shift;
        shift += 7;
        if byte & 0x80 == 0 {
            return Ok(value);
        }
    }
}

fn apply_delta(base: &[u8], delta: &[u8]) -> Result<Vec<u8>, GitObjectError> {
    let mut pos = 0usize;
    let source = read_varint(delta, &mut pos)?;
    let target = read_varint(delta, &mut pos)?;
    if source != base.len() {
        return Err(malformed("delta base size differs from the base object"));
    }
    let mut out = Vec::with_capacity(target.min(1 << 24));
    while pos < delta.len() {
        let command = delta[pos];
        pos += 1;
        if command & 0x80 != 0 {
            let mut offset = 0usize;
            let mut length = 0usize;
            for bit in 0..4 {
                if command & (1 << bit) != 0 {
                    let byte = *delta
                        .get(pos)
                        .ok_or_else(|| malformed("delta copy is truncated"))?;
                    pos += 1;
                    offset |= (byte as usize) << (8 * bit);
                }
            }
            for bit in 0..3 {
                if command & (0x10 << bit) != 0 {
                    let byte = *delta
                        .get(pos)
                        .ok_or_else(|| malformed("delta copy is truncated"))?;
                    pos += 1;
                    length |= (byte as usize) << (8 * bit);
                }
            }
            if length == 0 {
                length = 0x10000;
            }
            let end = offset
                .checked_add(length)
                .filter(|end| *end <= base.len())
                .ok_or_else(|| malformed("delta copy is outside its base"))?;
            out.extend_from_slice(&base[offset..end]);
        } else if command != 0 {
            let end = pos + command as usize;
            if end > delta.len() {
                return Err(malformed("delta insert is truncated"));
            }
            out.extend_from_slice(&delta[pos..end]);
            pos = end;
        } else {
            return Err(malformed("delta has a reserved command"));
        }
    }
    if out.len() != target {
        return Err(malformed("delta result differs from its declared size"));
    }
    Ok(out)
}

/// Read a version-2 pack: resolves `OFS_DELTA` and `REF_DELTA`, recomputes every
/// object id and checks the trailer. Returns the objects by id. A delta whose
/// base is not in the pack is refused.
#[allow(clippy::type_complexity)]
pub fn read_pack(
    format: GitObjectFormat,
    bytes: &[u8],
) -> Result<BTreeMap<String, (GitObjectKind, Vec<u8>)>, GitObjectError> {
    let width = id_len(format);
    if bytes.len() < 12 + width || &bytes[..4] != PACK_MAGIC {
        return Err(malformed("not a git pack"));
    }
    if bytes[4..8] != 2u32.to_be_bytes() {
        return Err(malformed("only pack version 2 is read"));
    }
    let body_end = bytes.len() - width;
    if digest(format, &[&bytes[..body_end]]) != bytes[body_end..] {
        return Err(malformed("pack trailer does not match its content"));
    }
    let count = u32::from_be_bytes([bytes[8], bytes[9], bytes[10], bytes[11]]) as usize;
    let mut pos = 12usize;
    let mut raws: Vec<RawEntry> = Vec::new();
    for _ in 0..count {
        let offset = pos;
        let first = *bytes
            .get(pos)
            .filter(|_| pos < body_end)
            .ok_or_else(|| malformed("pack is truncated"))?;
        pos += 1;
        let code = (first >> 4) & 0x07;
        let mut size = (first & 0x0f) as usize;
        let mut shift = 4u32;
        let mut byte = first;
        while byte & 0x80 != 0 {
            byte = *bytes
                .get(pos)
                .filter(|_| pos < body_end)
                .ok_or_else(|| malformed("pack is truncated"))?;
            pos += 1;
            if shift > 56 {
                return Err(malformed("pack object size is too large"));
            }
            size |= ((byte & 0x7f) as usize) << shift;
            shift += 7;
        }
        let raw = match code {
            OFS_DELTA => {
                let mut byte = *bytes
                    .get(pos)
                    .filter(|_| pos < body_end)
                    .ok_or_else(|| malformed("pack is truncated"))?;
                pos += 1;
                let mut back = (byte & 0x7f) as usize;
                while byte & 0x80 != 0 {
                    byte = *bytes
                        .get(pos)
                        .filter(|_| pos < body_end)
                        .ok_or_else(|| malformed("pack is truncated"))?;
                    pos += 1;
                    back = back
                        .checked_add(1)
                        .and_then(|value| value.checked_mul(128))
                        .map(|value| value + (byte & 0x7f) as usize)
                        .ok_or_else(|| malformed("pack delta offset is too large"))?;
                }
                let base = offset
                    .checked_sub(back)
                    .ok_or_else(|| malformed("pack delta offset is before the pack"))?;
                Raw::Ofs(base)
            }
            REF_DELTA => {
                if pos + width > body_end {
                    return Err(malformed("pack is truncated"));
                }
                let id = hex::encode(&bytes[pos..pos + width]);
                pos += width;
                Raw::Ref(id)
            }
            other => Raw::Base(
                GitObjectKind::from_pack_type(other)
                    .ok_or_else(|| malformed("pack object has an unknown type"))?,
            ),
        };
        let (data, used) = inflate(&bytes[pos..body_end], size)?;
        pos += used;
        raws.push(RawEntry { offset, raw, data });
    }
    if pos != body_end {
        return Err(malformed("pack has bytes after its last object"));
    }

    let mut by_offset: BTreeMap<usize, (GitObjectKind, Vec<u8>)> = BTreeMap::new();
    let mut by_id: BTreeMap<String, (GitObjectKind, Vec<u8>)> = BTreeMap::new();
    let mut pending: Vec<RawEntry> = Vec::new();
    for entry in raws {
        match entry.raw {
            Raw::Base(kind) => {
                by_id.insert(object_id(format, kind, &entry.data), (kind, entry.data.clone()));
                by_offset.insert(entry.offset, (kind, entry.data));
            }
            _ => pending.push(entry),
        }
    }
    while !pending.is_empty() {
        let before = pending.len();
        let mut next: Vec<RawEntry> = Vec::new();
        for entry in pending {
            let base = match &entry.raw {
                Raw::Ofs(base_offset) => by_offset.get(base_offset),
                Raw::Ref(base_id) => by_id.get(base_id),
                Raw::Base(_) => None,
            };
            match base {
                Some((kind, base_body)) => {
                    let kind = *kind;
                    let resolved = apply_delta(base_body, &entry.data)?;
                    by_id.insert(object_id(format, kind, &resolved), (kind, resolved.clone()));
                    by_offset.insert(entry.offset, (kind, resolved));
                }
                None => next.push(entry),
            }
        }
        if next.len() == before {
            return Err(malformed("a pack delta names a base that is not in the pack"));
        }
        pending = next;
    }
    Ok(by_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    const SHA1: GitObjectFormat = GitObjectFormat::Sha1;
    const SHA256: GitObjectFormat = GitObjectFormat::Sha256;

    fn zlib(data: &[u8]) -> Vec<u8> {
        let mut encoder = ZlibEncoder::new(Vec::new(), Compression::default());
        encoder.write_all(data).unwrap();
        encoder.finish().unwrap()
    }

    fn entry_header(code: u8, size: usize) -> Vec<u8> {
        let mut out = Vec::new();
        let mut size = size;
        let mut byte = (code << 4) | (size & 0x0f) as u8;
        size >>= 4;
        while size > 0 {
            out.push(byte | 0x80);
            byte = (size & 0x7f) as u8;
            size >>= 7;
        }
        out.push(byte);
        out
    }

    fn seal(format: GitObjectFormat, mut pack: Vec<u8>) -> Vec<u8> {
        let trailer = digest(format, &[&pack]);
        pack.extend_from_slice(&trailer);
        pack
    }

    /// The commit and tree git wrote for the fixture repository (one file
    /// `a.txt` holding `hello world\n`, one directory `d` holding `b.txt` with
    /// `x\n`, commit time 1700000000), per object format.
    fn vectors(format: GitObjectFormat) -> [&'static str; 6] {
        match format {
            GitObjectFormat::Sha1 => [
                "4b825dc642cb6eb9a060e54bf8d69288fbee4904",
                "3b18e512dba79e4c8300dd08aeb37f8e728b8dad",
                "add4794d9c94872b96c1697c056fb82ae0d72880",
                "282065596af94646853a6ceb9631aef70600126a",
                "8d49570894c89453ccc67d6389ae600491315b2a",
                "e69de29bb2d1d6434b8b29ae775ad8c2e48c5391",
            ],
            GitObjectFormat::Sha256 => [
                "6ef19b41225c5369f1c104d45d8d85efa9b057b53b14b4b9b939dd74decc5321",
                "0bd69098bd9b9cc5934a610ab65da429b525361147faa7b5b922919e9a23143d",
                "6101b778c5bc3277f57f668fe2b7bf7b279ff2789791e2ac8f6ab8a3f175ff6a",
                "675bb97c9070e7ca281960b2b8fb84a2d7c76394c8dbb8e00c653b669b1d6ae1",
                "88d2b1db040aefbcc7df2c35fc73c97c8c4d71012ef8e04f33383d77b290fddd",
                "473a0f4c3be8a93681a267e3b1e9a7dcda1185436fe141f7749120a303721813",
            ],
        }
    }

    /// Verifies MEDULLA-SCM-R010: the codec reproduces the ids git wrote for an
    /// empty tree, a blob, a nested tree, a root tree and a commit in both object
    /// formats, and a pack of undeltified objects, `OFS_DELTA` and `REF_DELTA`
    /// entries reads back to the same objects.
    // spec: MEDULLA-SCM-R010
    #[test]
    fn pack_round_trips_and_ids_match_git_vectors() {
        for format in [SHA1, SHA256] {
            let [empty_tree, blob_id, d_tree, root_tree, commit_id, empty_blob] = vectors(format);
            assert_eq!(build_tree(format, &[]).unwrap().0, empty_tree);
            assert_eq!(object_id(format, GitObjectKind::Tree, b""), empty_tree);
            assert_eq!(object_id(format, GitObjectKind::Blob, b""), empty_blob);
            assert_eq!(
                object_id(format, GitObjectKind::Blob, b"hello world\n"),
                blob_id
            );
            let x_id = object_id(format, GitObjectKind::Blob, b"x\n");
            let (built_d, d_body) = build_tree(
                format,
                &[GitTreeEntry {
                    name: "b.txt".into(),
                    mode: MODE_FILE,
                    id: x_id,
                }],
            )
            .unwrap();
            assert_eq!(built_d, d_tree);
            // Directory names sort with a trailing slash; the mode is `40000`.
            let (built_root, root_body) = build_tree(
                format,
                &[
                    GitTreeEntry {
                        name: "d".into(),
                        mode: MODE_TREE,
                        id: d_tree.into(),
                    },
                    GitTreeEntry {
                        name: "a.txt".into(),
                        mode: MODE_FILE,
                        id: blob_id.into(),
                    },
                ],
            )
            .unwrap();
            assert_eq!(built_root, root_tree);
            let parsed = parse_tree(format, &root_body).unwrap();
            assert_eq!(parsed[0].name, "a.txt");
            assert_eq!(parsed[1].mode, MODE_TREE);
            assert_eq!(
                build_tree(format, &parsed).unwrap(),
                (built_root.clone(), root_body.clone())
            );

            let raw_headers = "author A <a@x> 1700000000 +0000\ncommitter A <a@x> 1700000000 +0000\n";
            let commit = build_commit(root_tree, raw_headers, "first\n");
            assert_eq!(object_id(format, GitObjectKind::Commit, &commit), commit_id);
            let parsed_commit = parse_commit(&commit).unwrap();
            assert_eq!(parsed_commit.tree_id, root_tree);
            assert!(parsed_commit.parents.is_empty());
            assert_eq!(parsed_commit.raw_headers, raw_headers);
            assert_eq!(parsed_commit.committer_time_s, 1_700_000_000);
            assert_eq!(parsed_commit.message, "first\n");

            // An undeltified pack round trips.
            let objects = vec![
                (GitObjectKind::Blob, b"hello world\n".to_vec()),
                (GitObjectKind::Blob, b"x\n".to_vec()),
                (GitObjectKind::Tree, d_body),
                (GitObjectKind::Tree, root_body),
                (GitObjectKind::Commit, commit),
                (GitObjectKind::Blob, Vec::new()),
            ];
            let mut pack = Vec::new();
            write_pack(format, &objects, &mut pack).unwrap();
            let read = read_pack(format, &pack).unwrap();
            assert_eq!(read.len(), 6);
            assert_eq!(read[commit_id].0, GitObjectKind::Commit);
            assert_eq!(read[empty_blob].1, Vec::<u8>::new());
            // A flipped byte fails the trailer.
            let mut broken = pack.clone();
            broken[14] ^= 0xff;
            assert!(read_pack(format, &broken).is_err());

            // A pack holding one base, an OFS_DELTA against it and a REF_DELTA
            // against it: the delta copies the 12 base bytes and inserts "!\n".
            let base = b"hello world\n";
            let target = b"hello world\n!\n";
            let delta: Vec<u8> = vec![12, 14, 0x90, 12, 2, b'!', b'\n'];
            let mut raw: Vec<u8> = Vec::new();
            raw.extend_from_slice(b"PACK");
            raw.extend_from_slice(&2u32.to_be_bytes());
            raw.extend_from_slice(&3u32.to_be_bytes());
            let base_offset = raw.len();
            raw.extend(entry_header(3, base.len()));
            raw.extend(zlib(base));
            let ofs_offset = raw.len();
            raw.extend(entry_header(OFS_DELTA, delta.len()));
            let back = ofs_offset - base_offset;
            assert!(back < 128);
            raw.push(back as u8);
            raw.extend(zlib(&delta));
            raw.extend(entry_header(REF_DELTA, delta.len()));
            raw.extend(hex::decode(blob_id).unwrap());
            raw.extend(zlib(&delta));
            let read = read_pack(format, &seal(format, raw)).unwrap();
            assert_eq!(read.len(), 2);
            let derived = object_id(format, GitObjectKind::Blob, target);
            assert_eq!(read[&derived].1, target.to_vec());
            assert_eq!(read[blob_id].1, base.to_vec());
        }
    }

    // spec: MEDULLA-SCM-R010
    #[test]
    fn modes_outside_the_four_file_modes_are_refused() {
        let entry = GitTreeEntry {
            name: "f".into(),
            mode: 0o100664,
            id: "0".repeat(40),
        };
        assert_eq!(
            build_tree(SHA1, &[entry]).unwrap_err(),
            GitObjectError::ModeUnsupported(0o100664)
        );
        let mut body = b"100664 f\0".to_vec();
        body.extend([0u8; 20]);
        assert_eq!(
            parse_tree(SHA1, &body).unwrap_err(),
            GitObjectError::ModeUnsupported(0o100664)
        );
    }
}
