//! `AgentComponent.edit_skill_source`: a lossless SKILL.md edit (MEDULLA-TYPED-PACKS-R100.100).
//!
//! The base revision's verified source bytes are patched by byte range with
//! `apply_skill_patches`; nothing is reparsed or reserialized. The edited source must still
//! pass the SKILL.md front-matter checks of the import (R019, R029), and is then published
//! as the next revision through the one publish path, which stages the body, commits the
//! revision, the head and the body holder in one owner write, and replays by the request's
//! idempotency key. A refusal happens before the publish and writes nothing.

use std::sync::Arc;

use tokio::sync::RwLock;

use medulla_agent_types::agent_component::{SkillSourceEditRequest, SkillSourcePatch};
use medulla_persistence_records::agent_library::AgentLibraryStore;
use medulla_protocol_types::protocol::Response;
use medulla_server_api::authority_context::VerifiedRequestContext;
use medulla_server_api::state::ServerState;

/// The pure part of the edit: patch `base`, then check the result is still a skill source.
///
/// `MALFORMED_BODY` when the edited bytes no longer pass the restricted front-matter profile
/// under the name the base revision declared.
pub(crate) fn edit_skill_source(
    base: &[u8],
    source_digest: &str,
    patches: &[SkillSourcePatch],
) -> Result<medulla_catalog_types::skill_edit::EditedSkill, String> {
    use super::connector_pack::import::front_matter::front_matter_value;
    use super::connector_pack::import::validation::valid_skill_frontmatter;

    let edited = medulla_catalog_types::skill_edit::apply_skill_patches(
        base,
        source_digest,
        patches,
    )?;
    let name = front_matter_value(base, "name").ok_or_else(|| {
        "MALFORMED_BODY: the base skill source declares no front-matter name".to_string()
    })?;
    if !valid_skill_frontmatter(&edited.bytes, &name) {
        return Err(
            "MALFORMED_BODY: the edited skill front matter is absent, unsafe, or does not name \
             the skill"
                .to_string(),
        );
    }
    Ok(edited)
}

pub(crate) async fn serve_skill_source_edit(
    state: &Arc<RwLock<ServerState>>,
    store: &AgentLibraryStore,
    req_id: u64,
    verified: &VerifiedRequestContext,
    request: SkillSourceEditRequest,
) -> Response {
    if request.context.tenant_id != verified.tenant() {
        return Response::err(
            req_id,
            "ACCESS_DENIED: skill edit tenant must match verified request tenant",
        );
    }
    #[cfg(not(feature = "blob"))]
    {
        let _ = (state, store);
        Response::err(req_id, "AgentComponent.edit_skill_source requires the blob feature")
    }
    #[cfg(feature = "blob")]
    {
        use medulla_agent_types::agent_component::{
            AgentComponentKind, AgentComponentPublishRequest, ComponentBody,
        };

        let base = request.context.expected_revision.unwrap_or(0);
        let (pointer, source) = match super::component_content::read_verified_body(
            state,
            store,
            &request.context.tenant_id,
            &request.component_id,
            Some(base),
        )
        .await
        {
            Ok(value) => value,
            Err(error) => return Response::err(req_id, error),
        };
        if pointer.entry.kind != AgentComponentKind::Skill {
            return Response::err(
                req_id,
                "INVALID_ARGUMENT: a skill edit names a skill component",
            );
        }
        let edited = match edit_skill_source(&source, &request.source_digest, &request.patches) {
            Ok(edited) => edited,
            Err(error) => return Response::err(req_id, error),
        };
        let SkillSourceEditRequest { context, .. } = request;
        let Ok(text) = String::from_utf8(edited.bytes) else {
            return Response::err(req_id, "SKILL_SPAN_UNKNOWN: the edited source is not UTF-8");
        };
        let mut component = super::component_content::draft_of(pointer.entry);
        component.content_digest = edited.digest;
        let publish = AgentComponentPublishRequest {
            context,
            component,
            evaluation_receipt_digest: None,
            body: Some(ComponentBody::SkillSource(text)),
        };
        super::agent::component_publish(state, store, req_id, verified, Box::new(publish)).await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use medulla_catalog_types::skill_edit::skill_digest;

    const SOURCE: &str =
        "---\nname: demo\ndescription: First line\n# keep: this comment\nx-custom:   spaced\n---\n\
         1. step one\n2. step two\n";

    fn edit(
        source: &str,
        digest: &str,
        patches: &[SkillSourcePatch],
    ) -> Result<medulla_catalog_types::skill_edit::EditedSkill, String> {
        edit_skill_source(source.as_bytes(), digest, patches)
    }

    fn replace(source: &str, from: &str, to: &str) -> SkillSourcePatch {
        let start = source.find(from).expect("anchor");
        SkillSourcePatch {
            range_start: start as u64,
            range_end: (start + from.len()) as u64,
            replacement: to.to_string(),
        }
    }

    // spec: MEDULLA-TYPED-PACKS-R100.100
    #[test]
    fn an_edit_keeps_every_untouched_byte_and_still_validates() {
        let patches = vec![
            replace(SOURCE, "First line", "Second line"),
            replace(SOURCE, "step two", "step 2"),
        ];
        let edited = edit(SOURCE, &skill_digest(SOURCE.as_bytes()), &patches).expect("edit");
        let text = String::from_utf8(edited.bytes).expect("utf8");
        assert_eq!(
            text,
            SOURCE
                .replace("First line", "Second line")
                .replace("step two", "step 2")
        );
        assert!(text.contains("# keep: this comment\nx-custom:   spaced\n"));
        assert_eq!(edited.digest, skill_digest(text.as_bytes()));
    }

    // spec: MEDULLA-TYPED-PACKS-R100.100
    #[test]
    fn an_edit_that_breaks_the_front_matter_refuses_malformed_body() {
        let patches = vec![replace(SOURCE, "name: demo", "name: other")];
        let error = edit(SOURCE, &skill_digest(SOURCE.as_bytes()), &patches)
            .expect_err("renamed skill refuses");
        assert!(error.starts_with("MALFORMED_BODY"), "{error}");
        let patches = vec![replace(SOURCE, "description: First line", "description: &a x")];
        let error = edit(SOURCE, &skill_digest(SOURCE.as_bytes()), &patches)
            .expect_err("anchor syntax refuses");
        assert!(error.starts_with("MALFORMED_BODY"), "{error}");
    }

    // spec: MEDULLA-TYPED-PACKS-R100.100
    #[test]
    fn a_stale_digest_refuses_before_any_validation() {
        let patches = vec![replace(SOURCE, "step one", "step 1")];
        let error = edit(SOURCE, &skill_digest(b"other bytes"), &patches).expect_err("stale");
        assert!(error.starts_with("SKILL_SOURCE_STALE"), "{error}");
    }
}
