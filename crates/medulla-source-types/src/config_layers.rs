//! The layered configuration read (MEDULLA-IDENTITY-R740).
//!
//! A configuration layer is a graph node of class `ConfigLayer`: one `company` layer per tenant,
//! one `team` layer per team and one `user` layer per principal. The engine reads the layers of the
//! verified caller: the company layer, the layer of every team the caller is a member of, and the
//! caller's own user layer. The caller never names a team or a user, so a read cannot ask for
//! another principal's layer, and a layer that is absent, foreign or hidden is left out the same
//! way (no existence oracle). Pure data and decoding; the router reads the nodes.

use serde::{Deserialize, Serialize};
use serde_json::Value;

/// The graph class of a layer node.
pub const CONFIG_LAYER_TYPE: &str = "ConfigLayer";

/// Which layer a node holds, snake_case on the wire. The order is the order the layers apply in:
/// company first, then team, then user.
#[derive(Clone, Copy, Debug, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(rename_all = "snake_case")]
pub enum ConfigLayerKind {
    /// The tenant's company layer.
    Company,
    /// The layer of one team.
    Team,
    /// The layer of one principal.
    User,
}

impl ConfigLayerKind {
    /// The snake_case text stored in the `layer` property and used in the node id.
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Company => "company",
            Self::Team => "team",
            Self::User => "user",
        }
    }
}

/// `ConfigLayers` request: the layers of the verified caller. It names nothing: the tenant, the
/// teams and the user all come from the verified request context and the graph.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConfigLayersRequest {}

/// One layer the caller may read.
#[derive(Clone, Debug, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConfigLayerRecord {
    /// Company, team or user.
    pub layer: ConfigLayerKind,
    /// The team id of a team layer, the principal persistence id of a user layer, empty for the
    /// company layer.
    pub scope: String,
    /// The graph node the layer is stored as.
    pub node_id: String,
    /// The revision the layer was last written at; 0 when the node carries none.
    pub revision: u64,
    /// The settings of the layer, a JSON object.
    pub settings: Value,
}

/// What `ConfigLayers` answers: the readable layers in application order (company, the member
/// teams by id, user). A tenant with no layer answers an empty list.
#[derive(Clone, Debug, Default, PartialEq, Eq, Serialize, Deserialize)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
#[serde(deny_unknown_fields)]
pub struct ConfigLayersAnswer {
    /// The layers.
    pub layers: Vec<ConfigLayerRecord>,
}

/// The node id of one layer: namespaced by the tenant with its length in front, so no tenant and
/// scope pair spells another's and a foreign tenant's layer is never the caller's node.
pub fn config_layer_node_id(tenant: &str, layer: ConfigLayerKind, scope: &str) -> String {
    format!(
        "config-layer:{}:{tenant}:{}:{scope}",
        tenant.len(),
        layer.as_str()
    )
}

/// Decode a stored layer node. `None` when the node is not a layer of `tenant` of the expected
/// kind and scope, whatever else it holds, so a foreign or mismatched node reads as absent.
/// `settings` may be a JSON object or the text of one; anything else is not a layer.
pub fn config_layer_record(
    tenant: &str,
    layer: ConfigLayerKind,
    scope: &str,
    node: &Value,
) -> Option<ConfigLayerRecord> {
    let text = |key: &str| node.get(key).and_then(Value::as_str);
    let is_layer = text("type") == Some(CONFIG_LAYER_TYPE)
        && text("tenant_id") == Some(tenant)
        && text("layer") == Some(layer.as_str())
        && text("scope").unwrap_or_default() == scope;
    if !is_layer {
        return None;
    }
    let settings = match node.get("settings")? {
        object @ Value::Object(_) => object.clone(),
        Value::String(raw) => match serde_json::from_str::<Value>(raw).ok()? {
            object @ Value::Object(_) => object,
            _ => return None,
        },
        _ => return None,
    };
    Some(ConfigLayerRecord {
        layer,
        scope: scope.to_owned(),
        node_id: config_layer_node_id(tenant, layer, scope),
        revision: node.get("revision").and_then(Value::as_u64).unwrap_or(0),
        settings,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn node(tenant: &str, layer: &str, scope: &str) -> Value {
        json!({
            "type": "ConfigLayer", "tenant_id": tenant, "layer": layer, "scope": scope,
            "revision": 3, "settings": {"skills": ["a"]}
        })
    }

    // spec: MEDULLA-IDENTITY-R740
    #[test]
    fn a_layer_decodes_only_for_its_tenant_kind_and_scope() {
        let company = node("t", "company", "");
        let record = config_layer_record("t", ConfigLayerKind::Company, "", &company).expect("a layer");
        assert_eq!(record.revision, 3);
        assert_eq!(record.node_id, config_layer_node_id("t", ConfigLayerKind::Company, ""));
        assert_eq!(record.settings, json!({"skills": ["a"]}));
        // Another tenant, another kind and another scope all read as absent.
        assert!(config_layer_record("u", ConfigLayerKind::Company, "", &company).is_none());
        assert!(config_layer_record("t", ConfigLayerKind::Team, "", &company).is_none());
        assert!(config_layer_record("t", ConfigLayerKind::Team, "x", &node("t", "team", "y")).is_none());
        // Settings that are not an object are not a layer; an object in text form is.
        let mut bad = node("t", "user", "p");
        bad["settings"] = json!([1]);
        assert!(config_layer_record("t", ConfigLayerKind::User, "p", &bad).is_none());
        let mut text = node("t", "user", "p");
        text["settings"] = json!("{\"hooks\":[]}");
        assert!(config_layer_record("t", ConfigLayerKind::User, "p", &text).is_some());
    }

    // spec: MEDULLA-IDENTITY-R740
    #[test]
    fn node_ids_never_collide_across_tenants() {
        let one = config_layer_node_id("a:1", ConfigLayerKind::Team, "x");
        let two = config_layer_node_id("a", ConfigLayerKind::Team, "1:team:x");
        assert_ne!(one, two);
        assert!(ConfigLayerKind::Company < ConfigLayerKind::Team && ConfigLayerKind::Team < ConfigLayerKind::User);
    }
}
