//! Reading one scalar out of a `SKILL.md` front matter (rule G9's restricted
//! subset: `key: scalar` lines between two `---` fences).
//!
//! Validation of the whole block is `validation::valid_skill_frontmatter`;
//! this only reads a value from a block that has passed it.

/// The front-matter block of a `SKILL.md`, without its fences.
fn block(body: &[u8]) -> Option<&str> {
    let text = std::str::from_utf8(body).ok()?;
    let rest = text
        .strip_prefix("---\n")
        .or_else(|| text.strip_prefix("---\r\n"))?;
    let end = rest.find("\n---\n").or_else(|| rest.find("\r\n---\r\n"))?;
    Some(&rest[..end])
}

/// The whitespace-collapsed value of top-level `key`, when present and
/// non-empty.
pub(super) fn front_matter_value(body: &[u8], key: &str) -> Option<String> {
    block(body)?.lines().find_map(|line| {
        let (name, value) = line.split_once(':')?;
        (name.trim() == key)
            .then(|| {
                value
                    .trim()
                    .trim_matches(&['\'', '"'][..])
                    .split_whitespace()
                    .collect::<Vec<_>>()
                    .join(" ")
            })
            .filter(|value| !value.is_empty())
    })
}

/// The skill types a `SKILL.md` may declare in its `skill_type` key.
pub(super) const SKILL_TYPES: [&str; 4] = ["skill", "workflow", "graph", "mcp_skill"];

/// The declared skill type: `Ok(None)` when absent, `Err` when it names a
/// type outside [`SKILL_TYPES`].
pub(super) fn declared_skill_type(body: &[u8]) -> Result<Option<&'static str>, ()> {
    match front_matter_value(body, "skill_type") {
        None => Ok(None),
        Some(value) => SKILL_TYPES
            .iter()
            .copied()
            .find(|known| *known == value)
            .map(Some)
            .ok_or(()),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    const SKILL: &[u8] =
        b"---\nname: demo\ndescription:  A   demo skill.\nskill_type: workflow\n---\n# Demo\n";

    #[test]
    fn values_are_read_from_the_front_matter_only() {
        assert_eq!(
            front_matter_value(SKILL, "description").as_deref(),
            Some("A demo skill.")
        );
        assert_eq!(front_matter_value(SKILL, "missing"), None);
        assert_eq!(front_matter_value(b"# no front matter\n", "name"), None);
    }

    // spec: MEDULLA-TYPED-PACKS-R700.7
    #[test]
    fn the_skill_type_is_closed() {
        assert_eq!(declared_skill_type(SKILL), Ok(Some("workflow")));
        assert_eq!(
            declared_skill_type(b"---\nname: demo\ndescription: d\n---\n"),
            Ok(None)
        );
        assert_eq!(
            declared_skill_type(b"---\nname: demo\ndescription: d\nskill_type: macro\n---\n"),
            Err(())
        );
        // The legacy `type` key is not read.
        assert_eq!(
            declared_skill_type(b"---\nname: demo\ndescription: d\ntype: workflow\n---\n"),
            Ok(None)
        );
    }
}
