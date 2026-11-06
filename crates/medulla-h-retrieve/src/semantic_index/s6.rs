//! The served S6 completion (MEDULLA-UNIFIED-DATA-PLANE-R040.6, D110).
//!
//! The external stage worker completes S6 through `CompleteGenerationStage`
//! with an `activation` payload. The engine reads the ANN image from the blob
//! the worker uploaded under its own owner scope and verifies its length and
//! SHA-256, records the worker's S4/S5 probe results, and activates through
//! `SemanticIndexService::activate_s6`, whose promotion gate refuses
//! `PROMOTION_BLOCKED` before a regressed generation goes `Live`.

use medulla_ann_codes::semantic_ann_codes::decode_image_body;
use medulla_ann_codes::semantic_index_service::S6Completion;
use medulla_index_types::semantic_index::{
    SemanticGenerationArtifact, SemanticImageBlobRef, SemanticS6Activation, SemanticStageTransition,
};
use medulla_mutation_types::mutation_scope::MutationOutboxLease;
use medulla_protocol_types::protocol::Response;
use medulla_protocol_types::result_contract::ingestion as ingestion_results;
use medulla_types::contract::ServerErrorCode;

use super::{contracts, reply, SemanticIndexContext};

fn image_unavailable(req_id: u64, detail: &str) -> Response {
    Response::err(
        req_id,
        format!("{}: {detail}", ServerErrorCode::SemanticImageUnavailable),
    )
}

#[cfg(feature = "blob")]
async fn read_image_body(
    ctx: &SemanticIndexContext<'_>,
    image: &SemanticImageBlobRef,
) -> Result<Vec<u8>, Response> {
    use medulla_server_blob::blob::BlobStateExt;
    let cursors = ctx
        .state
        .read()
        .await
        .blob_cursors()
        .cloned()
        .ok_or_else(|| image_unavailable(ctx.req_id, "the blob substrate is disabled"))?;
    let owner = ctx.authority.owner_scope().to_string();
    let image = image.clone();
    let max = usize::try_from(medulla_index_types::semantic_index::MAX_SEMANTIC_IMAGE_BYTES)
        .unwrap_or(usize::MAX);
    let read = tokio::task::spawn_blocking(move || {
        let sha256 = medulla_types::contract::Digest256::parse(&image.sha256)?;
        medulla_server_blob::blob::engine_bodies::read_owned_body(
            cursors.store.as_ref(),
            &owner,
            &image.manifest_digest,
            sha256,
            image.length,
            max,
        )
    })
    .await;
    match read {
        Ok(Ok(body)) => Ok(body),
        Ok(Err(error)) => Err(image_unavailable(ctx.req_id, &error)),
        Err(_) => Err(image_unavailable(
            ctx.req_id,
            "the read could not be scheduled",
        )),
    }
}

#[cfg(not(feature = "blob"))]
async fn read_image_body(
    ctx: &SemanticIndexContext<'_>,
    _image: &SemanticImageBlobRef,
) -> Result<Vec<u8>, Response> {
    Err(image_unavailable(ctx.req_id, "this build serves no blobs"))
}

/// Complete S6: read and verify the image, record the probes, activate.
pub(super) async fn complete(
    ctx: &SemanticIndexContext<'_>,
    lease: MutationOutboxLease,
    transition: SemanticStageTransition,
    artifact: SemanticGenerationArtifact,
    activation: SemanticS6Activation,
) -> Response {
    let body = match read_image_body(ctx, &activation.image).await {
        Ok(body) => body,
        Err(response) => return response,
    };
    let config = medulla_server_api::semantic_model_config::semantic_model_config().cloned();
    let completed = super::worker::on_own_lease(ctx, lease, move |service, lease, now_ms| {
        if let Some(config) = config {
            service.install_model_config(config)?;
        }
        let image = decode_image_body(&body)?;
        let inputs = S6Completion {
            checkpoint: &activation.checkpoint,
            image: &image,
            probes: &activation.probes,
        };
        service.complete_s6_stage(lease, &transition, &artifact, inputs, now_ms)
    });
    reply::<ingestion_results::SemanticIndexCompleteGenerationStage, _>(
        ctx.req_id,
        completed.await.map(contracts::receipt),
    )
}
