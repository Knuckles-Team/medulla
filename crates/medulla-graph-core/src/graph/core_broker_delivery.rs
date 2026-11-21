use super::*;

impl GraphCore {
    /// Resolve a consumer's `delivery_tag` to the id of the message it fences, for the tag-addressed
    /// ack (MEDULLA-CQ-ORCH-R810.4). The tag must be the message's current tag, held by `consumer`,
    /// on a `claimed` message or one the consumer already acked; a stale generation retires its
    /// lookup node. This reads and fences only: the ack itself is decided by the one ack route,
    /// `medulla_core::broker::broker_ack`, so the lease check is the same by id and by tag.
    #[cfg(feature = "broker")]
    pub fn broker_resolve_ack_tag(&self, delivery_tag: i64, consumer: &str) -> Option<String> {
        if !Self::broker_tag_addressable(delivery_tag, consumer) {
            return None;
        }
        let lookup_id = crate::broker_keys::dtag_lookup_node_id(delivery_tag);
        let mut txn = self.txn();
        let lookup = txn.node_row_map(&lookup_id)?;
        if lookup.get("type").and_then(serde_json::Value::as_str)
            != Some(crate::broker_keys::DTAG_LOOKUP_TYPE)
        {
            txn.remove_node(lookup_id);
            return None;
        }
        if lookup
            .get("owner_consumer")
            .and_then(serde_json::Value::as_str)
            != Some(consumer)
        {
            return None;
        }
        let node_id = Self::broker_lookup_str(&lookup, "node_id");
        if node_id.is_empty() {
            txn.remove_node(lookup_id);
            return None;
        }
        let Some(properties) = txn.node_row_map(&node_id) else {
            txn.remove_node(lookup_id);
            return None;
        };
        let status = properties.get("status").and_then(serde_json::Value::as_str);
        let current_tag = properties
            .get("delivery_tag")
            .and_then(serde_json::Value::as_i64)
            == Some(delivery_tag);
        if !current_tag || !matches!(status, Some("claimed") | Some("acked")) {
            txn.remove_node(lookup_id);
            return None;
        }
        let owned = properties
            .get("owner_consumer")
            .and_then(serde_json::Value::as_str)
            == Some(consumer);
        owned.then_some(node_id)
    }

    /// Extend a still-live claimed delivery lease for its current owner. Every
    /// comparison and the write occur under one guard; caller-supplied time keeps
    /// replay deterministic.
    #[cfg(feature = "broker")]
    pub fn broker_renew_delivery_tag(
        &self,
        delivery_tag: i64,
        consumer: &str,
        now_ms: u64,
        lease_ms: u64,
    ) -> bool {
        if !Self::broker_tag_addressable(delivery_tag, consumer) || lease_ms == 0 {
            return false;
        }
        let Some(renewed_until) = now_ms.checked_add(lease_ms) else {
            return false;
        };
        let mut txn = self.txn();
        let BrokerTagOwner::Owned {
            lookup_id, node_id, ..
        } = Self::broker_resolve_tag_owner(&mut txn, delivery_tag, consumer)
        else {
            return false;
        };
        let Some(mut properties) =
            Self::broker_take_live_claim(&mut txn, &lookup_id, &node_id, delivery_tag, consumer)
        else {
            return false;
        };
        if !Self::broker_lease_extends(&properties, now_ms, renewed_until) {
            return false;
        }
        properties.insert("lease_until".into(), serde_json::Value::from(renewed_until));
        let value = serde_json::Value::Object(properties);
        let Ok(blob) = rmp_serde::to_vec_named(&value) else {
            return false;
        };
        txn.add_node(node_id, blob);
        true
    }

    /// Atomically fence and end a tag-addressed delivery. Requeue is applied in
    /// the same transaction; a terminal transition removes the original and returns
    /// its immutable properties so the broker layer can perform DLX routing in the
    /// surrounding staged durable mutation.
    #[cfg(feature = "broker")]
    pub fn broker_nack_delivery_tag(
        &self,
        delivery_tag: i64,
        consumer: &str,
        requeue: bool,
    ) -> BrokerNackTransition {
        if !Self::broker_tag_addressable(delivery_tag, consumer) {
            return BrokerNackTransition::Absent;
        }
        let mut txn = self.txn();
        let BrokerTagOwner::Owned {
            lookup_id,
            node_id,
            queue,
        } = Self::broker_resolve_tag_owner(&mut txn, delivery_tag, consumer)
        else {
            return BrokerNackTransition::Absent;
        };
        if node_id.is_empty() || queue.is_empty() {
            txn.remove_node(lookup_id);
            return BrokerNackTransition::Absent;
        }
        let Some(properties) =
            Self::broker_take_live_claim(&mut txn, &lookup_id, &node_id, delivery_tag, consumer)
        else {
            return BrokerNackTransition::Absent;
        };
        let Some(delivery_count) = properties
            .get("delivery_count")
            .and_then(serde_json::Value::as_i64)
            .filter(|count| *count >= 0)
        else {
            return BrokerNackTransition::Absent;
        };
        // The policy read is a side-effect-free lookup, so skipping it when the
        // caller is not requeueing is unobservable.
        if requeue && Self::broker_under_max_delivery(&txn, &queue, delivery_count) {
            return Self::broker_requeue(&mut txn, lookup_id, node_id, properties);
        }

        let value = serde_json::Value::Object(properties);
        txn.remove_node(lookup_id);
        txn.remove_node(node_id.clone());
        BrokerNackTransition::Terminal {
            node_id,
            queue,
            properties: value,
        }
    }

    /// Is a delivery tag addressable at all? Both tag-addressed transitions reject
    /// a non-positive tag and an empty consumer before touching the graph.
    #[cfg(feature = "broker")]
    pub(super) fn broker_tag_addressable(delivery_tag: i64, consumer: &str) -> bool {
        delivery_tag > 0 && !consumer.trim().is_empty()
    }
}
