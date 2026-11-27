//! The build identity of the running engine (MEDULLA-OBSERVE-R009.1).
//!
//! `build.rs` embeds the git SHA, the enabled feature list and the profile; the
//! version is [`crate::VERSION`]. The executable's build id is read once, when
//! [`BUILD_INFO`] is first used: the ELF GNU build-id note of the running
//! executable on Linux, and `None` on a target whose binary format is not read here.

use std::sync::LazyLock;

/// What a deployed engine says about the binary it runs.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct BuildInfo {
    /// The engine release version.
    pub version: &'static str,
    /// The commit the binary was built from, or `unknown` outside a checkout.
    pub git_sha: &'static str,
    /// The executable's build id as lowercase hex, when the binary carries one.
    pub build_id: Option<String>,
    /// The cargo profile the binary was built with.
    pub profile: &'static str,
    /// The enabled cargo features, ascending.
    pub features: Vec<&'static str>,
}

/// The build identity, read once at first use.
pub static BUILD_INFO: LazyLock<BuildInfo> = LazyLock::new(BuildInfo::current);

impl BuildInfo {
    fn current() -> Self {
        let features = env!("MEDULLA_BUILD_FEATURES");
        Self {
            version: crate::VERSION,
            git_sha: env!("MEDULLA_BUILD_GIT_SHA"),
            build_id: std::fs::read("/proc/self/exe")
                .ok()
                .and_then(|image| elf_build_id(&image)),
            profile: env!("MEDULLA_BUILD_PROFILE"),
            features: features.split(',').filter(|name| !name.is_empty()).collect(),
        }
    }
}

fn read_u16(bytes: &[u8], at: usize) -> Option<u16> {
    Some(u16::from_le_bytes(bytes.get(at..at + 2)?.try_into().ok()?))
}

fn read_u32(bytes: &[u8], at: usize) -> Option<u32> {
    Some(u32::from_le_bytes(bytes.get(at..at + 4)?.try_into().ok()?))
}

fn read_u64(bytes: &[u8], at: usize) -> Option<u64> {
    Some(u64::from_le_bytes(bytes.get(at..at + 8)?.try_into().ok()?))
}

/// The GNU build id of a little-endian ELF64 image, as lowercase hex.
///
/// Walks the program headers for `PT_NOTE` segments and takes the `NT_GNU_BUILD_ID` note
/// owned by `GNU`. Any structure that does not parse answers `None`.
pub fn elf_build_id(image: &[u8]) -> Option<String> {
    const PT_NOTE: u32 = 4;
    const NT_GNU_BUILD_ID: u32 = 3;
    if image.get(..4)? != b"\x7fELF" || *image.get(4)? != 2 || *image.get(5)? != 1 {
        return None;
    }
    let ph_offset = usize::try_from(read_u64(image, 0x20)?).ok()?;
    let ph_size = usize::from(read_u16(image, 0x36)?);
    let ph_count = usize::from(read_u16(image, 0x38)?);
    for index in 0..ph_count {
        let header = ph_offset.checked_add(index.checked_mul(ph_size)?)?;
        if read_u32(image, header)? != PT_NOTE {
            continue;
        }
        let offset = usize::try_from(read_u64(image, header + 0x08)?).ok()?;
        let size = usize::try_from(read_u64(image, header + 0x20)?).ok()?;
        let notes = image.get(offset..offset.checked_add(size)?)?;
        let mut at = 0;
        while at + 12 <= notes.len() {
            let name_len = usize::try_from(read_u32(notes, at)?).ok()?;
            let desc_len = usize::try_from(read_u32(notes, at + 4)?).ok()?;
            let kind = read_u32(notes, at + 8)?;
            let name_at = at + 12;
            let desc_at = name_at.checked_add(name_len.checked_add(3)? & !3)?;
            let next = desc_at.checked_add(desc_len.checked_add(3)? & !3)?;
            if kind == NT_GNU_BUILD_ID && notes.get(name_at..name_at + name_len)? == b"GNU\0" {
                let id = notes.get(desc_at..desc_at + desc_len)?;
                return Some(id.iter().map(|byte| format!("{byte:02x}")).collect());
            }
            at = next;
        }
    }
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    // spec: MEDULLA-OBSERVE-R009.1
    // Proves the build identity carries the version, the git SHA, the profile and an ascending
    // feature list.
    #[test]
    fn build_info_has_version_sha_and_features() {
        let info = &*BUILD_INFO;
        assert_eq!(info.version, crate::VERSION);
        assert!(!info.git_sha.is_empty());
        assert!(
            info.git_sha == "unknown" || info.git_sha.bytes().all(|byte| byte.is_ascii_hexdigit()),
            "the sha is a commit or `unknown`: {}",
            info.git_sha
        );
        assert!(matches!(info.profile, "debug" | "release") || !info.profile.is_empty());
        assert!(info.features.windows(2).all(|pair| pair[0] < pair[1]), "features ascend");
    }

    // spec: MEDULLA-OBSERVE-R009.1
    // Proves the executable's GNU build id is read from the note and a foreign image reads none.
    #[test]
    fn elf_build_id_reads_the_gnu_note_and_refuses_other_images() {
        // A minimal ELF64 header with one PT_NOTE program header and one build-id note.
        let note = {
            let mut note = Vec::new();
            note.extend(4_u32.to_le_bytes());
            note.extend(4_u32.to_le_bytes());
            note.extend(3_u32.to_le_bytes());
            note.extend(b"GNU\0");
            note.extend([0xde, 0xad, 0xbe, 0xef]);
            note
        };
        let mut image = vec![0_u8; 0x40 + 0x38];
        image[..4].copy_from_slice(b"\x7fELF");
        image[4] = 2;
        image[5] = 1;
        image[0x20..0x28].copy_from_slice(&0x40_u64.to_le_bytes());
        image[0x36..0x38].copy_from_slice(&0x38_u16.to_le_bytes());
        image[0x38..0x3a].copy_from_slice(&1_u16.to_le_bytes());
        let note_at = image.len() as u64;
        image[0x40..0x44].copy_from_slice(&4_u32.to_le_bytes());
        image[0x48..0x50].copy_from_slice(&note_at.to_le_bytes());
        image[0x60..0x68].copy_from_slice(&(note.len() as u64).to_le_bytes());
        image.extend(&note);
        assert_eq!(elf_build_id(&image).as_deref(), Some("deadbeef"));
        assert_eq!(elf_build_id(b"MZ not an elf"), None);
        assert_eq!(elf_build_id(&image[..0x30]), None);
    }
}
