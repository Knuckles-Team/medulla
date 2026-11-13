//! Durable learned capability rewards (MEDULLA-DECISION-ENGINE-R172.5.1).
//!
//! One `capability_reward` row per tenant and designated entity, written
//! through the Agent Library control-plane write path (the mutation kernel) so a
//! reward survives restart. The reward update rule itself is
//! `apply_capability_outcome` in `medulla-decision-types`; this file only keeps
//! its result. A write that carries an older `updated_revision` than the
//! stored row is refused, so a durable reward is never replaced by a staler one.

use medulla_decision_types::decision::capability_designation::CapabilityReward;
use medulla_storage::{CAPABILITY_REWARD, DECISION_ARTIFACTS};
use redb::ReadableTable;

use super::AgentLibraryStore;

/// The control-plane event name of a reward write.
const REWARD_EVENT: &str = "capability_reward";

/// Refusal code of a reward write older than the stored row.
pub const CAPABILITY_REWARD_STALE: &str = "CAPABILITY_REWARD_STALE";

fn encode(reward: &CapabilityReward) -> Result<Vec<u8>, String> {
    serde_json::to_vec(reward).map_err(|error| error.to_string())
}

fn decode(bytes: &[u8]) -> Result<CapabilityReward, String> {
    serde_json::from_slice(bytes)
        .map_err(|error| format!("CORRUPT_CAPABILITY_REWARD: invalid reward row: {error}"))
}

impl AgentLibraryStore {
    /// Write the reward of one entity for its tenant. The row is replaced when
    /// its revision is not older than the stored one.
    pub fn put_capability_reward(&self, reward: &CapabilityReward) -> Result<(), String> {
        reward.validate()?;
        let bytes = encode(reward)?;
        let tenant = reward.tenant_id.as_str();
        let entity = reward.entity_id.as_str();
        self.maintain_control_rows(tenant, REWARD_EVENT, |owner| {
            let mut table = owner.open_table(CAPABILITY_REWARD)?;
            let stored = table
                .get((tenant, entity))
                .map_err(|error| error.to_string())?
                .map(|row| decode(row.value()))
                .transpose()?;
            if let Some(stored) = stored {
                if stored.updated_revision > reward.updated_revision {
                    return Err(format!(
                        "{CAPABILITY_REWARD_STALE}: {entity} holds revision {} and the write carries {}",
                        stored.updated_revision, reward.updated_revision
                    ));
                }
            }
            table
                .insert((tenant, entity), bytes.as_slice())
                .map_err(|error| error.to_string())?;
            Ok(())
        })
    }

    /// Apply one outcome to the reward of `entity_id` and store its trace under
    /// `trace_key`, in ONE owner transaction, so a concurrent outcome never
    /// reads a stale prior and a crash never leaves a trace without its reward.
    ///
    /// `step` receives the stored reward (`None` before any outcome) and the
    /// trace already stored under `trace_key`. It answers the new reward and
    /// the trace bytes to store, or `None` when the stored trace is the same
    /// outcome (a replay: nothing is written). The answer is the reward in
    /// force and the trace now stored, and whether it was a replay.
    pub fn apply_capability_outcome_row<F>(
        &self,
        tenant_id: &str,
        entity_id: &str,
        trace_key: &str,
        step: F,
    ) -> Result<(CapabilityReward, Vec<u8>, bool), String>
    where
        F: FnOnce(
            Option<CapabilityReward>,
            Option<&[u8]>,
        ) -> Result<Option<(CapabilityReward, Vec<u8>)>, String>,
    {
        let mut applied: Option<(CapabilityReward, Vec<u8>, bool)> = None;
        self.maintain_control_rows(tenant_id, REWARD_EVENT, |owner| {
            let mut rewards = owner.open_table(CAPABILITY_REWARD)?;
            let mut artifacts = owner.open_table(DECISION_ARTIFACTS)?;
            let stored = rewards
                .get((tenant_id, entity_id))
                .map_err(|error| error.to_string())?
                .map(|row| decode(row.value()))
                .transpose()?;
            let existing = artifacts
                .get((tenant_id, trace_key))
                .map_err(|error| error.to_string())?
                .map(|row| row.value().to_vec());
            match step(stored.clone(), existing.as_deref())? {
                None => {
                    let reward = stored.ok_or_else(|| {
                        format!(
                            "CORRUPT_CAPABILITY_REWARD: trace {trace_key} has no reward row for {entity_id}"
                        )
                    })?;
                    let trace = existing.unwrap_or_default();
                    applied = Some((reward, trace, true));
                }
                Some((reward, trace)) => {
                    reward.validate()?;
                    let bytes = encode(&reward)?;
                    rewards
                        .insert((tenant_id, entity_id), bytes.as_slice())
                        .map_err(|error| error.to_string())?;
                    artifacts
                        .insert((tenant_id, trace_key), trace.as_slice())
                        .map_err(|error| error.to_string())?;
                    applied = Some((reward, trace, false));
                }
            }
            Ok(())
        })?;
        applied.ok_or_else(|| {
            "capability outcome write was answered as a mutation replay without applying"
                .to_string()
        })
    }

    /// The reward of one entity for `tenant_id`, when it has outcomes.
    pub fn capability_reward(
        &self,
        tenant_id: &str,
        entity_id: &str,
    ) -> Result<Option<CapabilityReward>, String> {
        let read = self.read()?;
        let table = read.open_owner_table(CAPABILITY_REWARD)?;
        table
            .get((tenant_id, entity_id))
            .map_err(|error| error.to_string())?
            .map(|row| decode(row.value()))
            .transpose()
    }

    /// Every reward of `tenant_id`, in entity order. Never another tenant's.
    pub fn capability_rewards(&self, tenant_id: &str) -> Result<Vec<CapabilityReward>, String> {
        let read = self.read()?;
        let table = read.open_owner_table(CAPABILITY_REWARD)?;
        let mut rewards = Vec::new();
        for row in table
            .range((tenant_id, "")..)
            .map_err(|error| error.to_string())?
        {
            let (key, value) = row.map_err(|error| error.to_string())?;
            if key.value().0 != tenant_id {
                break;
            }
            rewards.push(decode(value.value())?);
        }
        Ok(rewards)
    }
}
