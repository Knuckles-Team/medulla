use super::*;

impl GraphCore {
    #[cfg(feature = "broker")]
    pub fn broker_ack_delivery_tag(&self, delivery_tag: i64, consumer: &str) -> bool {
        if delivery_tag <= 0 || consumer.trim().is_empty() {
            return false;
        }
        let lookup_id = crate::broker_keys::dtag_lookup_node_id(delivery_tag);
        let mut txn = self.txn();
        let Some(lookup) = txn.node_row_map(&lookup_id) else {
            return false;
        };
        if lookup.get("type").and_then(serde_json::Value::as_str)
            != Some(crate::broker_keys::DTAG_LOOKUP_TYPE)
        {
            txn.remove_node(lookup_id);
            return false;
        }
        if lookup
            .get("owner_consumer")
            .and_then(serde_json::Value::as_str)
            != Some(consumer)
        {
            return false;
        }
        let node_id = lookup
            .get("node_id")
            .and_then(serde_json::Value::as_str)
            .unwrap_or("")
            .to_string();
        if node_id.is_empty() {
            txn.remove_node(lookup_id);
            return false;
        }
        let Some(mut properties) = txn.node_row_map(&node_id) else {
            txn.remove_node(lookup_id);
            return false;
        };
        let status = properties.get("status").and_then(serde_json::Value::as_str);
        let current_tag = properties
            .get("delivery_tag")
            .and_then(serde_json::Value::as_i64)
            == Some(delivery_tag);
        let owned = properties
            .get("owner_consumer")
            .and_then(serde_json::Value::as_str)
            == Some(consumer);
        if status == Some("acked") && current_tag && owned {
            // The repeated ack by the claimant: nothing is written (MEDULLA-CQ-ORCH-R810.4).
            return true;
        }
        if status != Some("claimed") || !current_tag {
            txn.remove_node(lookup_id);
            return false;
        }
        if !owned {
            return false;
        }
        // The message stays `acked` and its lookup node stays until the sweep purges both.
        // A tag ack carries no clock, so the ack time is the claim time of this delivery.
        let acked_at = properties
            .get("claimed_at")
            .and_then(serde_json::Value::as_u64)
            .unwrap_or(0);
        properties.insert("status".into(), serde_json::Value::String("acked".into()));
        properties.insert("acked_at_ms".into(), serde_json::Value::from(acked_at));
        properties.insert("lease_until".into(), serde_json::Value::Null);
        let value = serde_json::Value::Object(properties);
        let Ok(blob) = rmp_serde::to_vec_named(&value) else {
            return false;
        };
        txn.add_node(node_id, blob);
        true
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
