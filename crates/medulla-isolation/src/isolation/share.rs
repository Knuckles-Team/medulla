//! The share transition of one node (MEDULLA-IDENTITY-R023.13.2).
//!
//! [`share_transition`] is the one pure function that decides how a `ShareGrant`
//! or `ShareRevoke` changes the share keys of a node: `_shared_scope`, `_grants`
//! and `_commons_copy`. It reads nothing but the node's current share state and
//! the acting principal, so its cost is O(g) in the grantee count and it never
//! scans the graph. The committed keys are the only visibility store; row-level
//! security reads them from the node (`row_visibility`).
//!
//! Only the owner (`_owner_id`), a `kg:admin` holder or a verified product delegate
//! ([`ShareDelegation`], MEDULLA-TYPED-PACKS-R805.3) may change a scope.
//! Promoting a node to the commons records the id of its `__commons__` copy in
//! `_commons_copy`; the caller creates that copy in the same commit, and a
//! revoke of a promoted node removes it in the same commit
//! ([`commons_copy_change`] names which).

use medulla_identity_types::acl::{ShareGrant, ShareRevoke};
use medulla_identity_types::identity::ShareScope;

/// The node key of the share scope: `private`, `org` or `commons`.
pub const SHARED_SCOPE_KEY: &str = "_shared_scope";
/// The node key of the named grantees.
pub const GRANTS_KEY: &str = "_grants";
/// The node key of the `__commons__` copy of a promoted node.
pub const COMMONS_COPY_KEY: &str = "_commons_copy";
/// The node key of the owner.
pub const OWNER_ID_KEY: &str = "_owner_id";

/// The `_shared_scope` value of a node nobody else may read.
pub const SCOPE_PRIVATE: &str = "private";
/// The `_shared_scope` value of a node the owner's tenant may read.
pub const SCOPE_ORG: &str = "org";
/// The `_shared_scope` value of a node promoted to the commons.
pub const SCOPE_COMMONS: &str = "commons";

/// One share change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareOp {
    Grant(ShareGrant),
    Revoke(ShareRevoke),
}

impl ShareOp {
    fn node_id(&self) -> &str {
        match self {
            Self::Grant(grant) => &grant.node_id,
            Self::Revoke(revoke) => &revoke.node_id,
        }
    }
}

/// The share keys of one node, as committed.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareState {
    pub node_id: String,
    /// `_owner_id`; `None` for a node nobody owns.
    pub owner_id: Option<String>,
    /// `_shared_scope`.
    pub shared_scope: String,
    /// `_grants`: the named grantees, sorted.
    pub grants: Vec<String>,
    /// `_commons_copy`: the id of the node's `__commons__` copy.
    pub commons_copy: Option<String>,
}

impl ShareState {
    /// A private node with no grants and no commons copy.
    pub fn private(node_id: &str, owner_id: Option<&str>) -> Self {
        Self {
            node_id: node_id.to_string(),
            owner_id: owner_id.map(str::to_string),
            shared_scope: SCOPE_PRIVATE.to_string(),
            grants: Vec::new(),
            commons_copy: None,
        }
    }
}

/// The authority a verified data-product role lends to one share change
/// (MEDULLA-TYPED-PACKS-R805.3). The caller of [`share_transition`] sets it only after it
/// verified the principal against the product's owner or the stewardship store; a share by
/// such a delegate is accepted on that role alone and the role is named in the audit entry.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareDelegation {
    /// The principal is the owner of this data product.
    ProductOwner { product: String },
    /// The principal is an owner or steward of this product's datasets by the stewardship store.
    ProductSteward { product: String },
}

impl ShareDelegation {
    /// The audit reason text: `product_owner:<id>` or `product_steward:<id>`.
    pub fn reason(&self) -> String {
        match self {
            Self::ProductOwner { product } => format!("product_owner:{product}"),
            Self::ProductSteward { product } => format!("product_steward:{product}"),
        }
    }
}

/// Who asks for the change.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ShareActor {
    pub principal_id: String,
    /// Whether the caller holds `kg:admin`.
    pub admin: bool,
    /// The verified product role the caller acts under, when it has one.
    pub delegation: Option<ShareDelegation>,
}

/// Why an actor may change a share.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareAuthority {
    /// The actor owns the node.
    Owner,
    /// The actor holds `kg:admin`.
    Admin,
    /// The actor acts under a verified product role.
    Delegate(ShareDelegation),
}

impl ShareAuthority {
    /// The audit reason text: `owner`, `admin` or the delegation's reason.
    pub fn reason(&self) -> String {
        match self {
            Self::Owner => "owner".to_string(),
            Self::Admin => "admin".to_string(),
            Self::Delegate(delegation) => delegation.reason(),
        }
    }
}

/// The authority under which `actor` may change the share of `current`: the owner first,
/// then `kg:admin`, then a verified product role. `None` means the actor may not.
pub fn share_authority(current: &ShareState, actor: &ShareActor) -> Option<ShareAuthority> {
    if current.owner_id.as_deref() == Some(actor.principal_id.as_str()) {
        Some(ShareAuthority::Owner)
    } else if actor.admin {
        Some(ShareAuthority::Admin)
    } else {
        actor.delegation.clone().map(ShareAuthority::Delegate)
    }
}

/// Why a share change is refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ShareRefusal {
    /// The actor is neither the owner, a `kg:admin` holder nor a verified product delegate.
    NotOwner,
    /// The operation names another node than the state it applies to.
    NodeMismatch,
}

impl ShareRefusal {
    /// The registered refusal code.
    pub fn code(&self) -> &'static str {
        match self {
            Self::NotOwner => "ACCESS_DENIED",
            Self::NodeMismatch => "INVALID_ARGUMENT",
        }
    }
}

impl std::fmt::Display for ShareRefusal {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::NotOwner => write!(
                f,
                "{}: only the owner or a kg:admin holder may change a share",
                self.code()
            ),
            Self::NodeMismatch => write!(
                f,
                "{}: the share names another node than the one it applies to",
                self.code()
            ),
        }
    }
}

/// The `__commons__` node id of the copy of `node_id`.
pub fn commons_copy_id(node_id: &str) -> String {
    format!("commons:{node_id}")
}

/// What the commons copy needs when `current` becomes `next`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum CommonsCopyChange {
    /// The copy stays as it is (or never existed).
    None,
    /// Copy the node into `__commons__` under this id.
    Create(String),
    /// Remove this copy from `__commons__`.
    Remove(String),
}

/// The change of the commons copy between two states of one node.
pub fn commons_copy_change(current: &ShareState, next: &ShareState) -> CommonsCopyChange {
    match (&current.commons_copy, &next.commons_copy) {
        (None, Some(id)) => CommonsCopyChange::Create(id.clone()),
        (Some(id), None) => CommonsCopyChange::Remove(id.clone()),
        _ => CommonsCopyChange::None,
    }
}

/// Decide the share state `op` leaves behind. A refusal changes nothing.
///
/// A grant of `Private` scope clears the grantees; of `Tenant` scope writes
/// `_shared_scope = org` with the named grantees (none means every member of the
/// owner's tenant); with `commons` writes `_shared_scope = commons` and the id of
/// the node's `__commons__` copy. A revoke clears `_grants`, sets `private` and
/// drops the copy.
pub fn share_transition(
    current: &ShareState,
    op: &ShareOp,
    actor: &ShareActor,
) -> Result<ShareState, ShareRefusal> {
    if op.node_id() != current.node_id {
        return Err(ShareRefusal::NodeMismatch);
    }
    if share_authority(current, actor).is_none() {
        return Err(ShareRefusal::NotOwner);
    }
    let mut next = ShareState {
        grants: Vec::new(),
        commons_copy: None,
        shared_scope: SCOPE_PRIVATE.to_string(),
        ..current.clone()
    };
    if let ShareOp::Grant(grant) = op {
        match (grant.scope, grant.commons) {
            (ShareScope::Tenant, true) => {
                next.shared_scope = SCOPE_COMMONS.to_string();
                next.commons_copy = Some(commons_copy_id(&current.node_id));
            }
            (ShareScope::Tenant, false) => {
                next.shared_scope = SCOPE_ORG.to_string();
                next.grants = grant.grantees.clone();
            }
            // Private clears everything; Placement is refused when the payload
            // is built, so it never reaches here.
            (ShareScope::Private, _) | (ShareScope::Placement, _) => {}
        }
    }
    Ok(next)
}

#[cfg(test)]
mod tests {
    use super::*;

    const NODE: &str = "node-1";

    fn owner() -> ShareActor {
        ShareActor {
            principal_id: "usr:owner".to_string(),
            admin: false,
            delegation: None,
        }
    }

    fn state() -> ShareState {
        ShareState::private(NODE, Some("usr:owner"))
    }

    fn grant(scope: ShareScope, grantees: &[&str]) -> ShareOp {
        ShareOp::Grant(
            ShareGrant::new(
                NODE.to_string(),
                scope,
                grantees.iter().map(|g| g.to_string()).collect(),
            )
            .unwrap(),
        )
    }

    fn commons() -> ShareOp {
        ShareOp::Grant(ShareGrant::commons(NODE.to_string()).unwrap())
    }

    fn revoke() -> ShareOp {
        ShareOp::Revoke(ShareRevoke::new(NODE.to_string()).unwrap())
    }

    // spec: MEDULLA-IDENTITY-R023.13.2
    /// Revoking a share clears every grant and returns the node to the private scope
    /// while the owner stays.
    #[test]
    fn revoke_clears_grants() {
        let shared =
            share_transition(&state(), &grant(ShareScope::Tenant, &["usr:b", "usr:a"]), &owner())
                .unwrap();
        assert_eq!(shared.shared_scope, SCOPE_ORG);
        assert_eq!(shared.grants, vec!["usr:a", "usr:b"]);
        let revoked = share_transition(&shared, &revoke(), &owner()).unwrap();
        assert_eq!(revoked.shared_scope, SCOPE_PRIVATE);
        assert!(revoked.grants.is_empty());
        assert_eq!(revoked.owner_id.as_deref(), Some("usr:owner"));
    }

    // spec: MEDULLA-IDENTITY-R023.13.2
    #[test]
    fn grant_writes_scope_and_grantees() {
        let org = share_transition(&state(), &grant(ShareScope::Tenant, &[]), &owner()).unwrap();
        assert_eq!(org.shared_scope, SCOPE_ORG);
        assert!(org.grants.is_empty());
        let named =
            share_transition(&state(), &grant(ShareScope::Tenant, &["usr:a"]), &owner()).unwrap();
        assert_eq!(named.grants, vec!["usr:a"]);
        let back = share_transition(&named, &grant(ShareScope::Private, &[]), &owner()).unwrap();
        assert_eq!(back.shared_scope, SCOPE_PRIVATE);
        assert!(back.grants.is_empty());
    }

    // spec: MEDULLA-IDENTITY-R023.13.2
    #[test]
    fn commons_promotion_records_the_copy_and_revoke_removes_it() {
        let promoted = share_transition(&state(), &commons(), &owner()).unwrap();
        assert_eq!(promoted.shared_scope, SCOPE_COMMONS);
        let copy = commons_copy_id(NODE);
        assert_eq!(promoted.commons_copy.as_deref(), Some(copy.as_str()));
        assert_eq!(
            commons_copy_change(&state(), &promoted),
            CommonsCopyChange::Create(copy.clone())
        );
        let revoked = share_transition(&promoted, &revoke(), &owner()).unwrap();
        assert_eq!(revoked.commons_copy, None);
        assert_eq!(
            commons_copy_change(&promoted, &revoked),
            CommonsCopyChange::Remove(copy)
        );
        // Narrowing a promotion to the tenant also removes the copy.
        let narrowed = share_transition(&promoted, &grant(ShareScope::Tenant, &[]), &owner())
            .unwrap();
        assert!(matches!(
            commons_copy_change(&promoted, &narrowed),
            CommonsCopyChange::Remove(_)
        ));
        // Promoting a promoted node again changes no copy.
        let again = share_transition(&promoted, &commons(), &owner()).unwrap();
        assert_eq!(commons_copy_change(&promoted, &again), CommonsCopyChange::None);
    }

    // spec: MEDULLA-IDENTITY-R023.13.2
    #[test]
    fn only_the_owner_or_an_admin_changes_a_scope() {
        let stranger = ShareActor {
            principal_id: "usr:other".to_string(),
            admin: false,
            delegation: None,
        };
        let refused = share_transition(&state(), &grant(ShareScope::Tenant, &[]), &stranger)
            .expect_err("a non-owner is refused");
        assert_eq!(refused, ShareRefusal::NotOwner);
        assert_eq!(refused.code(), "ACCESS_DENIED");
        let admin = ShareActor {
            principal_id: "usr:other".to_string(),
            admin: true,
            delegation: None,
        };
        assert!(share_transition(&state(), &grant(ShareScope::Tenant, &[]), &admin).is_ok());
        // A node nobody owns is changed by an admin only.
        let unowned = ShareState::private(NODE, None);
        assert!(share_transition(&unowned, &revoke(), &owner()).is_err());
        assert!(share_transition(&unowned, &revoke(), &admin).is_ok());
        // An operation on another node is refused.
        let other = ShareOp::Revoke(ShareRevoke::new("node-2".to_string()).unwrap());
        assert_eq!(
            share_transition(&state(), &other, &owner()),
            Err(ShareRefusal::NodeMismatch)
        );
    }

    // spec: MEDULLA-TYPED-PACKS-R805.3
    #[test]
    fn a_verified_product_delegate_changes_a_scope_and_names_its_reason() {
        let delegate = |delegation| ShareActor {
            principal_id: "usr:steward".to_string(),
            admin: false,
            delegation: Some(delegation),
        };
        let steward = delegate(ShareDelegation::ProductSteward {
            product: "orders".to_string(),
        });
        let shared = share_transition(&state(), &grant(ShareScope::Tenant, &["usr:a"]), &steward)
            .expect("a verified steward may share");
        assert_eq!(shared.grants, vec!["usr:a"]);
        assert_eq!(
            share_authority(&state(), &steward).map(|authority| authority.reason()),
            Some("product_steward:orders".to_string())
        );
        let owner = delegate(ShareDelegation::ProductOwner {
            product: "orders".to_string(),
        });
        assert_eq!(
            share_authority(&state(), &owner).map(|authority| authority.reason()),
            Some("product_owner:orders".to_string())
        );
        // Admin and node ownership stay first.
        let admin = ShareActor {
            admin: true,
            ..steward.clone()
        };
        assert_eq!(share_authority(&state(), &admin), Some(ShareAuthority::Admin));
        let node_owner = ShareActor {
            principal_id: "usr:owner".to_string(),
            ..steward
        };
        assert_eq!(share_authority(&state(), &node_owner), Some(ShareAuthority::Owner));
    }
}
