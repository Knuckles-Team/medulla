//! Lock-free WAL snapshot reader (MEDULLA-UNIFIED-DATA-PLANE-R015.2.2.1).
//!
//! A live SQLite file in WAL mode keeps its newest committed pages in the `-wal` file.
//! [`WalSnapshot`] parses the WAL header (magic `0x377f0682` / `0x377f0683`, page size,
//! two salts), walks the frames validating salts and the running checksum, and stops at
//! the last frame whose database-size field is non-zero (a commit). Frames after that
//! commit, a torn tail included, are never visible. The page map keeps the newest frame
//! per page. Nothing here writes: both files are opened read-only.

use std::collections::BTreeMap;
use std::io::Read;
use std::path::Path;
use std::time::{Duration, Instant};

use crate::error::{Error, Result, SqliteReadRefusal};

const WAL_HEADER: usize = 32;
const FRAME_HEADER: usize = 24;
const MAGIC_LE: u32 = 0x377f_0682;
const MAGIC_BE: u32 = 0x377f_0683;

/// The two salts of a WAL header; a checkpoint-and-restart changes them.
pub type Salts = (u32, u32);

/// The committed state of a WAL at one instant.
#[derive(Debug, Clone, Default, PartialEq)]
pub struct WalSnapshot {
    pages: BTreeMap<u32, Vec<u8>>,
    salts: Option<Salts>,
    page_size: u32,
    db_pages: u32,
}

fn be32(buf: &[u8], off: usize) -> u32 {
    u32::from_be_bytes([buf[off], buf[off + 1], buf[off + 2], buf[off + 3]])
}

/// The SQLite WAL running checksum over 8-byte groups of `data`.
fn checksum(big_endian: bool, data: &[u8], seed: (u32, u32)) -> (u32, u32) {
    let word = |b: &[u8]| {
        let raw = [b[0], b[1], b[2], b[3]];
        if big_endian {
            u32::from_be_bytes(raw)
        } else {
            u32::from_le_bytes(raw)
        }
    };
    let (mut s0, mut s1) = seed;
    for chunk in data.chunks_exact(8) {
        s0 = s0.wrapping_add(word(&chunk[0..4])).wrapping_add(s1);
        s1 = s1.wrapping_add(word(&chunk[4..8])).wrapping_add(s0);
    }
    (s0, s1)
}

/// The salts of the WAL header at `wal`, or `None` for an absent, short or invalid header.
fn read_salts(wal: &Path) -> std::result::Result<Option<Salts>, std::io::Error> {
    let mut header = [0u8; WAL_HEADER];
    let mut file = match std::fs::File::open(wal) {
        Ok(file) => file,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e),
    };
    if file.read_exact(&mut header).is_err() {
        return Ok(None);
    }
    Ok(parse_header(&header).map(|h| h.salts))
}

struct Header {
    big_endian: bool,
    page_size: u32,
    salts: Salts,
    checksum: (u32, u32),
}

fn parse_header(buf: &[u8]) -> Option<Header> {
    if buf.len() < WAL_HEADER {
        return None;
    }
    let big_endian = match be32(buf, 0) {
        MAGIC_LE => false,
        MAGIC_BE => true,
        _ => return None,
    };
    let page_size = be32(buf, 8);
    if !(512..=65_536).contains(&page_size) || !page_size.is_power_of_two() {
        return None;
    }
    let stored = (be32(buf, 24), be32(buf, 28));
    if checksum(big_endian, &buf[..24], (0, 0)) != stored {
        return None;
    }
    Some(Header {
        big_endian,
        page_size,
        salts: (be32(buf, 16), be32(buf, 20)),
        checksum: stored,
    })
}

impl WalSnapshot {
    /// Parse the committed state of `wal`. A missing WAL or an invalid header is an empty
    /// snapshot (SQLite treats both as "no WAL"). The salts are re-read after the parse:
    /// a change means a checkpoint raced it, refused as `SnapshotUnstable`. `main` names
    /// the database file the WAL belongs to; it is only checked to exist-or-be-new.
    pub fn open(main: &Path, wal: &Path) -> std::result::Result<Self, SqliteReadRefusal> {
        let _ = main;
        let bytes = match std::fs::read(wal) {
            Ok(bytes) => bytes,
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(Self::default()),
            Err(e) => return Err(e.into()),
        };
        let snapshot = Self::parse(&bytes);
        if read_salts(wal)? != snapshot.salts {
            return Err(SqliteReadRefusal::SnapshotUnstable);
        }
        Ok(snapshot)
    }

    /// Parse the committed prefix of a WAL image.
    fn parse(bytes: &[u8]) -> Self {
        let Some(header) = parse_header(bytes) else {
            return Self::default();
        };
        let page_size = header.page_size as usize;
        let mut committed = BTreeMap::new();
        let mut pending: Vec<(u32, Vec<u8>)> = Vec::new();
        let mut db_pages = 0u32;
        let mut running = header.checksum;
        let mut offset = WAL_HEADER;
        while let Some(frame) = bytes.get(offset..offset + FRAME_HEADER + page_size) {
            let (head, data) = frame.split_at(FRAME_HEADER);
            let page_no = be32(head, 0);
            if page_no == 0 || (be32(head, 8), be32(head, 12)) != header.salts {
                break;
            }
            let sum = checksum(header.big_endian, &head[..8], running);
            let sum = checksum(header.big_endian, data, sum);
            if sum != (be32(head, 16), be32(head, 20)) {
                break;
            }
            running = sum;
            pending.push((page_no, data.to_vec()));
            let commit_size = be32(head, 4);
            if commit_size != 0 {
                committed.extend(pending.drain(..));
                db_pages = commit_size;
            }
            offset += FRAME_HEADER + page_size;
        }
        committed.retain(|page_no, _| *page_no <= db_pages);
        WalSnapshot {
            pages: committed,
            salts: Some(header.salts),
            page_size: header.page_size,
            db_pages,
        }
    }

    /// The header salts this snapshot was read under (`None` when there is no valid WAL).
    pub fn salts(&self) -> Option<Salts> {
        self.salts
    }

    /// The database size in pages at the last commit (0 when the WAL holds no commit).
    pub fn database_pages(&self) -> u32 {
        self.db_pages
    }

    /// The newest committed image of `page_no`, if the WAL holds one.
    pub fn page(&self, page_no: u32) -> Option<&[u8]> {
        self.pages.get(&page_no).map(Vec::as_slice)
    }

    /// Lay the WAL pages over `main` bytes: the main image is cut or zero-extended to the
    /// committed database size, then each WAL page replaces its slot.
    pub(crate) fn overlay(&self, mut main: Vec<u8>) -> Vec<u8> {
        if self.db_pages == 0 {
            return main;
        }
        let size = self.page_size as usize;
        main.resize(self.db_pages as usize * size, 0);
        for (page_no, data) in &self.pages {
            let start = (*page_no as usize - 1) * size;
            main[start..start + size].copy_from_slice(data);
        }
        main
    }
}

/// Read `main` + `wal` as one consistent snapshot. The order is what makes it consistent
/// without a lock: the WAL salts are read first, then the main file, then the WAL is
/// parsed. A checkpoint copies pages into the main file but only a later writer restarts
/// the WAL (new salts), so equal salts before and after mean every page this WAL
/// generation touched is overlaid by its newest committed frame and the main file only
/// supplies pages the generation never touched. A salt change means a checkpoint raced
/// the read: it retries until `busy_timeout` elapses, then refuses with `SnapshotUnstable`.
/// `observe(attempt)` runs after the main file is read (a seam for tests to race it).
pub(crate) fn read_stable(
    main: &Path,
    wal: &Path,
    busy_timeout: Duration,
    observe: &mut dyn FnMut(u32),
) -> Result<Vec<u8>> {
    let started = Instant::now();
    let mut attempt = 0u32;
    loop {
        let before = read_salts(wal).map_err(SqliteReadRefusal::from)?;
        let main_bytes = std::fs::read(main)?;
        observe(attempt);
        match WalSnapshot::open(main, wal) {
            Ok(snapshot) if snapshot.salts == before => return Ok(snapshot.overlay(main_bytes)),
            Ok(_) | Err(SqliteReadRefusal::SnapshotUnstable) => {}
            Err(other) => return Err(other.into()),
        }
        if started.elapsed() >= busy_timeout {
            return Err(Error::Refused(SqliteReadRefusal::SnapshotUnstable));
        }
        attempt += 1;
        std::thread::sleep(Duration::from_millis(1));
    }
}
