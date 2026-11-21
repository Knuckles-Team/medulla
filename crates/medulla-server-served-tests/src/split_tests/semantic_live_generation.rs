//! The served live-generation helper (MEDULLA-MEDIA-VOCAB-R001.102.2): it drives a binding's
//! generation from completed S1 to a live generation through `Method::SemanticIndex` ops, as a
//! stage worker does, and lets the engine's own S6 worker activate it.
//!
//! The caller admits the source and completes S1 for every entity of the generation first. The
//! helper then completes S2 for each entity, S3 as one generation-level lexical checkpoint that
//! each entity's completion advances, S4 as one vector coverage checkpoint (a deterministic unit
//! vector per entity, [`entity_vector`]), and S5 as one ANN checkpoint. The completion that makes
//! a checkpoint cover the whole entity set is the one that publishes the next stage; the store
//! publishes the other entities' successors (MEDULLA-UNIFIED-DATA-PLANE-R040.3). The S5
//! completion carries the S6 intent, and the engine's S6 worker (subscribed at `AdmitBinding`)
//! activates the generation inside that same request, so the live generation is read back once
//! the last completion returns.
//!
//! It is shared by the bookmark proofs and every served proof that needs a live semantic index.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use tokio::sync::RwLock;

use super::medulla_h_query_dispatch_pipeline_tests::{signed, TENANT};
use medulla_index_types::semantic_index::{
    SemanticAnnIndexManifest, SemanticBinding, SemanticDigest, SemanticExpectedEntity,
    SemanticGenerationAggregate, SemanticGenerationArtifact, SemanticGenerationCheckpoint,
    SemanticGenerationCheckpointDraft, SemanticGenerationCheckpointUpdate,
    SemanticGenerationDependency, SemanticGenerationMember, SemanticGraphProjectionManifest,
    SemanticGraphProjectionManifestDraft, SemanticIndexOp, SemanticLexicalIndexManifest,
    SemanticStage, SemanticStageArtifact, SemanticStageIntent, SemanticStageIntentDraft,
    SemanticStageLeaseEntry, SemanticStageLeasePage, SemanticStageOutcome,
    SemanticStagePredecessor, SemanticStageReceipt, SemanticStageScope, SemanticStageTransition,
    SemanticVector,
};
use medulla_server_api::state::ServerState;
use medulla_server_api::test_support::decode_raw_response as ok;

/// The consumer the helper claims its leases as.
const DRIVER: &str = "live-generation-driver";

/// Request ids are the envelope nonce; one counter keeps them unique across helper calls.
static REQUEST_ID: AtomicU64 = AtomicU64::new(910_000);

/// The deterministic unit vector the helper embeds for `source_entity_id`, so a proof can build
/// the query embedding that ranks an entity first.
#[allow(dead_code)]
pub(super) fn entity_vector(dimension: usize, source_entity_id: &str) -> Vec<f32> {
    let mut values: Vec<f32> = (0..dimension)
        .map(|axis| {
            let byte = source_entity_id
                .bytes()
                .enumerate()
                .fold(axis as u32 + 1, |acc, (at, byte)| {
                    acc.wrapping_mul(31)
                        .wrapping_add(u32::from(byte))
                        .wrapping_add(at as u32)
                });
            (byte % 997) as f32 + 1.0
        })
        .collect();
    let norm = values.iter().map(|value| value * value).sum::<f32>().sqrt();
    for value in &mut values {
        *value /= norm;
    }
    values
}

fn digest_for(tag: u8, key: &str) -> SemanticDigest {
    let mut bytes = [tag; 32];
    for (at, byte) in key.bytes().enumerate() {
        bytes[at % 32] = bytes[at % 32].wrapping_mul(31).wrapping_add(byte);
    }
    SemanticDigest::from_bytes(bytes)
}

fn completed_at(second: usize) -> String {
    format!("2026-09-10T00:{:02}:{:02}Z", (second / 60) % 60, second % 60)
}

/// The generation-level checkpoint one stage advances entity by entity.
struct Cumulative {
    stage: SemanticStage,
    expected: Vec<SemanticExpectedEntity>,
    members: Vec<SemanticGenerationMember>,
    previous: Option<SemanticGenerationCheckpoint>,
}

impl Cumulative {
    fn new(stage: SemanticStage, expected: &[SemanticExpectedEntity]) -> Self {
        Self {
            stage,
            expected: expected.to_vec(),
            members: Vec::new(),
            previous: None,
        }
    }

    /// Append `transition`'s entity and return the compare-and-swap update that advances the
    /// checkpoint from the previous head to the one covering every member so far.
    fn advance(
        &mut self,
        transition: &SemanticStageTransition,
        dependency: SemanticGenerationDependency,
    ) -> SemanticGenerationCheckpointUpdate {
        let intent = &transition.intent;
        let member = SemanticGenerationMember {
            source_entity_id: intent
                .scope
                .source_entity_id()
                .expect("an entity-scoped stage")
                .to_string(),
            source_revision: intent.source_revision.clone(),
            receipt_digest: transition.receipt.receipt_digest(),
            artifact_digest: transition.receipt.output_digest,
        };
        let expected_previous_completed_count = self.members.len() as u64;
        let expected_previous_checkpoint_digest =
            self.previous.as_ref().map(|head| head.checkpoint_digest);
        self.members.push(member.clone());
        let aggregate = SemanticGenerationAggregate::create(
            &intent.binding_id,
            intent.binding_digest,
            intent.generation,
            &intent.source_revision,
            self.stage,
            self.expected.clone(),
            self.members.clone(),
        )
        .expect("the cumulative aggregate");
        let successor = SemanticGenerationCheckpoint::create(SemanticGenerationCheckpointDraft {
            binding_id: intent.binding_id.clone(),
            binding_digest: intent.binding_digest,
            generation: intent.generation,
            source_revision: intent.source_revision.clone(),
            stage: self.stage,
            artifact_digest: aggregate.aggregate_artifact_digest,
            aggregate,
            dependency,
            completed_at: transition.receipt.completed_at.clone(),
        })
        .expect("the cumulative checkpoint");
        self.previous = Some(successor.clone());
        SemanticGenerationCheckpointUpdate {
            expected_previous_checkpoint_digest,
            expected_previous_completed_count,
            member,
            successor,
        }
    }

    fn head(&self) -> &SemanticGenerationCheckpoint {
        self.previous.as_ref().expect("at least one completion")
    }
}

struct Driver<'a> {
    state: &'a Arc<RwLock<ServerState>>,
    binding_id: &'a str,
}

impl Driver<'_> {
    async fn dispatch<T: serde::de::DeserializeOwned>(&self, label: &str, op: SemanticIndexOp) -> T {
        let id = REQUEST_ID.fetch_add(1, Ordering::SeqCst);
        ok(
            label,
            medulla_server_router::dispatch::request_boundary::dispatch(
                self.state,
                signed(id, op),
            )
            .await,
        )
    }

    async fn binding(&self) -> SemanticBinding {
        self.dispatch::<Option<SemanticBinding>>(
            "Binding",
            SemanticIndexOp::Binding {
                tenant_id: TENANT.to_string(),
                binding_id: self.binding_id.to_string(),
            },
        )
        .await
        .expect("the binding is admitted")
    }

    /// Every lease of `stage` the helper's consumer can take, ordered by entity id.
    async fn claim(&self, stage: SemanticStage) -> Vec<SemanticStageLeaseEntry> {
        let page: SemanticStageLeasePage = self
            .dispatch(
                "ClaimStageLeases",
                SemanticIndexOp::ClaimStageLeases {
                    tenant_id: TENANT.to_string(),
                    binding_id: self.binding_id.to_string(),
                    consumer: DRIVER.to_string(),
                    queue_class: stage.queue_class(),
                    limit: 256,
                    lease_ms: 60_000,
                },
            )
            .await;
        let mut entries: Vec<_> = page
            .entries
            .into_iter()
            .filter(|entry| entry.intent.stage == stage)
            .collect();
        entries.sort_by(|left, right| {
            left.intent
                .scope
                .source_entity_id()
                .cmp(&right.intent.scope.source_entity_id())
        });
        assert!(!entries.is_empty(), "no {stage:?} lease is claimable");
        entries
    }

    async fn complete(
        &self,
        entry: &SemanticStageLeaseEntry,
        transition: SemanticStageTransition,
        artifact: SemanticStageArtifact,
        successor: Option<SemanticStageIntent>,
    ) {
        let _: serde_json::Value = self
            .dispatch(
                "CompleteStage",
                SemanticIndexOp::CompleteStage {
                    tenant_id: TENANT.to_string(),
                    binding_id: self.binding_id.to_string(),
                    lease: Box::new(entry.lease.clone()),
                    transition: Box::new(transition),
                    artifact: Box::new(artifact),
                    successor: successor.map(Box::new),
                },
            )
            .await;
    }

    async fn complete_generation(
        &self,
        entry: &SemanticStageLeaseEntry,
        transition: SemanticStageTransition,
        artifact: SemanticGenerationArtifact,
        successor: Option<SemanticStageIntent>,
    ) {
        let _: serde_json::Value = self
            .dispatch(
                "CompleteGenerationStage",
                SemanticIndexOp::CompleteGenerationStage {
                    tenant_id: TENANT.to_string(),
                    binding_id: self.binding_id.to_string(),
                    lease: Box::new(entry.lease.clone()),
                    transition: Box::new(transition),
                    artifact: Box::new(artifact),
                    successor: successor.map(Box::new),
                },
            )
            .await;
    }
}

fn transition_of(
    intent: &SemanticStageIntent,
    output_digest: SemanticDigest,
    second: usize,
) -> SemanticStageTransition {
    SemanticStageTransition {
        intent: intent.clone(),
        receipt: SemanticStageReceipt {
            intent_digest: intent.intent_digest,
            output_digest,
            cursor: format!("cursor:{second}"),
            completed_at: completed_at(second),
            outcome: SemanticStageOutcome::Completed,
        },
        generation_checkpoint: None,
    }
}

fn successor_of(
    intent: &SemanticStageIntent,
    stage: SemanticStage,
    scope: SemanticStageScope,
    predecessor: SemanticStagePredecessor,
    input_digest: SemanticDigest,
) -> SemanticStageIntent {
    SemanticStageIntent::create(SemanticStageIntentDraft {
        binding_id: intent.binding_id.clone(),
        binding_digest: intent.binding_digest,
        generation: intent.generation,
        scope,
        source_revision: intent.source_revision.clone(),
        stage,
        predecessor,
        input_digest,
    })
    .expect("the successor intent")
}

/// Drive the binding's generation from completed S1 to live and return the live generation.
// Consumed by the bookmark served proofs (MEDULLA-MEDIA-VOCAB-R001.102.2), which are not built yet.
#[allow(dead_code)]
pub(super) async fn drive_generation_to_live(
    state: &Arc<RwLock<ServerState>>,
    binding_id: &str,
) -> u64 {
    let driver = Driver { state, binding_id };
    let _: serde_json::Value = driver
        .dispatch(
            "SubscribeStageConsumer",
            SemanticIndexOp::SubscribeStageConsumer {
                tenant_id: TENANT.to_string(),
                binding_id: binding_id.to_string(),
                consumer: DRIVER.to_string(),
            },
        )
        .await;
    let binding = driver.binding().await;
    let mut second = 0usize;
    let mut tick = || {
        second += 1;
        second
    };

    // S2: one graph projection per entity; the entity set of the generation is fixed here.
    let mut expected = Vec::new();
    for entry in driver.claim(SemanticStage::GraphProjection).await {
        let intent = &entry.intent;
        let entity = intent.scope.source_entity_id().expect("entity scope").to_string();
        expected.push(SemanticExpectedEntity {
            source_entity_id: entity.clone(),
            source_revision: intent.source_revision.clone(),
        });
        let completed = transition_of(intent, digest_for(13, &entity), tick());
        let manifest = SemanticGraphProjectionManifest::create(SemanticGraphProjectionManifestDraft {
            binding_id: intent.binding_id.clone(),
            binding_digest: intent.binding_digest,
            generation: intent.generation,
            source_entity_id: entity.clone(),
            source_revision: intent.source_revision.clone(),
            source_manifest_digest: intent.input_digest,
            projection_entity_digest: digest_for(14, &entity),
            projection_content_digest: completed.receipt.output_digest,
            completed_receipt_digest: completed.receipt.receipt_digest(),
            completed_at: completed.receipt.completed_at.clone(),
        })
        .expect("the graph projection manifest");
        driver
            .complete(
                &entry,
                completed,
                SemanticStageArtifact::GraphProjectionManifest {
                    manifest: Box::new(manifest),
                },
                None,
            )
            .await;
    }

    // S3: every entity appends to the one lexical checkpoint; the last one covers the set and
    // makes the store derive the S4 intents.
    let mut lexical = Cumulative::new(SemanticStage::LexicalIndex, &expected);
    for entry in driver.claim(SemanticStage::LexicalIndex).await {
        let intent = &entry.intent;
        let entity = intent.scope.source_entity_id().expect("entity scope").to_string();
        let mut completed = transition_of(intent, digest_for(15, &entity), tick());
        let update = lexical.advance(&completed, SemanticGenerationDependency::None);
        let checkpoint = &update.successor;
        let manifest = SemanticLexicalIndexManifest {
            binding_id: intent.binding_id.clone(),
            binding_digest: intent.binding_digest,
            generation: intent.generation,
            source_revision: intent.source_revision.clone(),
            identity: binding.lexical_index_identity.clone(),
            artifact_digest: checkpoint.artifact_digest,
            row_count: checkpoint.aggregate.completed_entity_count as _,
            completed_receipt_digest: checkpoint.aggregate.aggregate_receipt_digest,
            completed_at: checkpoint.completed_at.clone(),
        };
        completed.generation_checkpoint = Some(Box::new(update));
        driver
            .complete_generation(
                &entry,
                completed,
                SemanticGenerationArtifact::LexicalIndexManifest {
                    manifest: Box::new(manifest),
                },
                None,
            )
            .await;
    }
    let lexical_digest = lexical.head().checkpoint_digest;

    // S4: one deterministic vector per entity into the vector coverage checkpoint; the
    // completion that covers the set carries the S5 intent naming that checkpoint.
    let mut coverage = Cumulative::new(SemanticStage::Vector, &expected);
    let entries = driver.claim(SemanticStage::Vector).await;
    let last = entries.len() - 1;
    for (at, entry) in entries.iter().enumerate() {
        let intent = &entry.intent;
        let entity = intent.scope.source_entity_id().expect("entity scope").to_string();
        let vector = SemanticVector::create(
            &binding,
            entity.clone(),
            intent.source_revision.clone(),
            entity_vector(binding.dimension as usize, &entity),
        )
        .expect("the entity vector");
        let completed = transition_of(intent, vector.values_digest, tick());
        let checkpoint = coverage
            .advance(
                &completed,
                SemanticGenerationDependency::Checkpoint {
                    stage: SemanticStage::LexicalIndex,
                    checkpoint_digest: lexical_digest,
                },
            )
            .successor;
        let successor = (at == last).then(|| {
            successor_of(
                intent,
                SemanticStage::AnnIndex,
                SemanticStageScope::Entity {
                    source_entity_id: entity.clone(),
                },
                SemanticStagePredecessor::GenerationCoverage {
                    checkpoint: Box::new(checkpoint),
                },
                completed.receipt.output_digest,
            )
        });
        driver
            .complete(
                entry,
                completed,
                SemanticStageArtifact::Vector {
                    vector: Box::new(vector),
                },
                successor,
            )
            .await;
    }
    let coverage_digest = coverage.head().checkpoint_digest;

    // S5: every entity appends to the one ANN checkpoint; the last carries the S6 intent, and
    // the engine's S6 worker activates the generation inside that request.
    let mut ann = Cumulative::new(SemanticStage::AnnIndex, &expected);
    let entries = driver.claim(SemanticStage::AnnIndex).await;
    let last = entries.len() - 1;
    for (at, entry) in entries.iter().enumerate() {
        let intent = &entry.intent;
        let entity = intent.scope.source_entity_id().expect("entity scope").to_string();
        let mut completed = transition_of(intent, digest_for(17, &entity), tick());
        let update = ann.advance(
            &completed,
            SemanticGenerationDependency::Checkpoint {
                stage: SemanticStage::Vector,
                checkpoint_digest: coverage_digest,
            },
        );
        let checkpoint = update.successor.clone();
        let manifest = SemanticAnnIndexManifest {
            binding_id: intent.binding_id.clone(),
            binding_digest: intent.binding_digest,
            generation: intent.generation,
            source_revision: intent.source_revision.clone(),
            identity: binding.ann_index_identity.clone(),
            artifact_digest: checkpoint.artifact_digest,
            vector_count: checkpoint.aggregate.completed_entity_count as _,
            completed_receipt_digest: checkpoint.aggregate.aggregate_receipt_digest,
            completed_at: checkpoint.completed_at.clone(),
        };
        let successor = (at == last).then(|| {
            successor_of(
                intent,
                SemanticStage::ReconcileAndActivate,
                SemanticStageScope::Generation,
                SemanticStagePredecessor::Activation {
                    lexical_checkpoint_digest: lexical_digest,
                    ann_checkpoint_digest: checkpoint.checkpoint_digest,
                },
                completed.receipt.output_digest,
            )
        });
        completed.generation_checkpoint = Some(Box::new(update));
        driver
            .complete_generation(
                entry,
                completed,
                SemanticGenerationArtifact::AnnIndexManifest {
                    manifest: Box::new(manifest),
                },
                successor,
            )
            .await;
    }

    driver
        .dispatch::<Option<u64>>(
            "LiveGeneration",
            SemanticIndexOp::LiveGeneration {
                tenant_id: TENANT.to_string(),
                binding_id: binding_id.to_string(),
            },
        )
        .await
        .expect("the engine S6 worker activated the generation")
}
