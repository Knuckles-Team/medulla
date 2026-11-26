//! Data products, data contracts and access requests (MEDULLA-TYPED-PACKS-R805.1).
//!
//! Pure data and field validation. Contract checks, grants and served
//! admission are later children of `MEDULLA-TYPED-PACKS-R805`.

use serde::{Deserialize, Serialize};

/// Longest identifier, principal or purpose a record may carry.
pub const MAX_PRODUCT_TEXT_BYTES: usize = 1024;
/// Most members one product list may name.
pub const MAX_PRODUCT_MEMBERS: usize = 256;
/// The stable refusal code for an access request without a purpose.
pub const ACCESS_PURPOSE_MISSING: &str = "ACCESS_PURPOSE_MISSING";

/// How a consumer reaches a product.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProductPort {
    pub name: String,
    pub kind: ProductPortKind,
}

/// The closed set of access-port kinds.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ProductPortKind {
    Query,
    Api,
    Stream,
    File,
}

/// An owned, versioned set of datasets, terms and access ports.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataProduct {
    pub id: String,
    pub owner: String,
    /// Semantic version `MAJOR.MINOR.PATCH`.
    pub version: String,
    pub datasets: Vec<String>,
    pub terms: Vec<String>,
    pub ports: Vec<ProductPort>,
}

/// What a product promises its consumers.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataContract {
    pub product: String,
    /// `sha256:<64 lowercase hex>` of the promised schema.
    pub schema_digest: String,
    pub quality_rules: Vec<String>,
    /// Largest promised age of the data, in seconds.
    pub freshness_s: u64,
}

/// One column of a data resource as the schema digest reads it.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct DigestColumn {
    pub name: String,
    pub type_name: String,
    pub nullable: bool,
}

/// The schema digest of a data resource (MEDULLA-TYPED-PACKS-R805.3): `sha256:<64 hex>` over its
/// columns in canonical sorted order, each as a length-prefixed `name`, `type`, `nullable` line.
/// Catalog ingest writes it as `cat:schemaDigest`; a contract promises one.
pub fn schema_digest(columns: &[DigestColumn]) -> String {
    use sha2::{Digest, Sha256};
    let mut sorted: Vec<&DigestColumn> = columns.iter().collect();
    sorted.sort();
    let mut hash = Sha256::new();
    for column in sorted {
        let line = format!("{}\t{}\t{}", column.name, column.type_name, column.nullable);
        hash.update((line.len() as u64).to_be_bytes());
        hash.update(line.as_bytes());
    }
    format!("sha256:{}", hex::encode(hash.finalize()))
}

/// The live schema digest of a product from the `cat:schemaDigest` of each of its datasets: the
/// digest itself for one dataset, else `sha256:<hex>` over the sorted digests. A contract's
/// `schema_digest` is promised in this form.
pub fn product_schema_digest(dataset_digests: &[String]) -> String {
    use sha2::{Digest, Sha256};
    if let [one] = dataset_digests {
        return one.clone();
    }
    let mut sorted: Vec<&String> = dataset_digests.iter().collect();
    sorted.sort();
    let mut hash = Sha256::new();
    for digest in sorted {
        hash.update((digest.len() as u64).to_be_bytes());
        hash.update(digest.as_bytes());
    }
    format!("sha256:{}", hex::encode(hash.finalize()))
}

use medulla_types::access_request_machine::AccessRequestState;

/// A consumer's governed request to use a product.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct AccessRequest {
    pub product: String,
    pub principal: String,
    pub purpose: String,
    pub state: AccessRequestState,
    /// The sharing grant an approved request wrote, absent until approval.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub grant_ref: Option<String>,
}

/// One promise a data contract makes.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub enum ContractPromise {
    Schema,
    Quality { rule: String },
    Freshness,
    /// A promise that cannot be checked: `of` names it (`schema` or `freshness`) and the
    /// observation it needs is missing (no ingested schema digest, no capture cursor).
    Unverifiable { of: String },
}

/// A broken promise observed against live state.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ContractViolation {
    pub product: String,
    pub promise: ContractPromise,
    /// What was observed: a digest, a rule outcome or an age in seconds.
    pub observed: String,
    pub at_ms: u64,
}

/// A data product the usage suggests (MEDULLA-TYPED-PACKS-R805.7): datasets that several
/// principals queried together in one period.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct ProductProposal {
    /// The clustered datasets, sorted.
    pub datasets: Vec<String>,
    /// Glossary terms linked to the datasets, sorted and without repeats.
    pub terms: Vec<String>,
    /// The principal with the most queries over the datasets (lowest id on a tie).
    pub owner_candidate: String,
    /// The principals that each queried every dataset, sorted.
    pub principals: Vec<String>,
    /// The usage period the queries fall in.
    pub period: String,
}

/// The receipt of a registered data product.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
#[cfg_attr(feature = "contract-schema", derive(schemars::JsonSchema))]
pub struct DataProductRegistered {
    pub product: String,
    pub version: String,
}

/// A record that failed validation.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DataProductError {
    pub code: &'static str,
    pub field: &'static str,
}

impl std::fmt::Display for DataProductError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        write!(f, "{}: {}", self.code, self.field)
    }
}

impl std::error::Error for DataProductError {}

fn invalid(field: &'static str) -> DataProductError {
    DataProductError {
        code: "INVALID_FIELD",
        field,
    }
}

fn text(field: &'static str, value: &str) -> Result<(), DataProductError> {
    if value.trim().is_empty()
        || value.trim() != value
        || value.len() > MAX_PRODUCT_TEXT_BYTES
        || value.chars().any(char::is_control)
    {
        return Err(invalid(field));
    }
    Ok(())
}

fn members<'a>(
    field: &'static str,
    values: impl ExactSizeIterator<Item = &'a str>,
) -> Result<(), DataProductError> {
    if values.len() > MAX_PRODUCT_MEMBERS {
        return Err(invalid(field));
    }
    let mut seen = std::collections::BTreeSet::new();
    for value in values {
        text(field, value)?;
        if !seen.insert(value) {
            return Err(invalid(field));
        }
    }
    Ok(())
}

/// Parse `MAJOR.MINOR.PATCH` into its three numbers.
pub fn parse_product_version(version: &str) -> Option<(u64, u64, u64)> {
    let mut parts = version.split('.').map(|part| {
        (!part.is_empty() && part.bytes().all(|b| b.is_ascii_digit()))
            .then(|| part.parse::<u64>().ok())
            .flatten()
    });
    let parsed = (parts.next()??, parts.next()??, parts.next()??);
    parts.next().is_none().then_some(parsed)
}

impl DataProduct {
    pub fn validate(&self) -> Result<(), DataProductError> {
        text("id", &self.id)?;
        text("owner", &self.owner)?;
        parse_product_version(&self.version).ok_or_else(|| invalid("version"))?;
        if self.datasets.is_empty() {
            return Err(invalid("datasets"));
        }
        members("datasets", self.datasets.iter().map(String::as_str))?;
        members("terms", self.terms.iter().map(String::as_str))?;
        members("ports", self.ports.iter().map(|port| port.name.as_str()))
    }
}

impl DataContract {
    pub fn validate(&self) -> Result<(), DataProductError> {
        text("product", &self.product)?;
        let hex = self
            .schema_digest
            .strip_prefix("sha256:")
            .unwrap_or_default();
        if hex.len() != 64 || !hex.bytes().all(|b| matches!(b, b'0'..=b'9' | b'a'..=b'f')) {
            return Err(invalid("schema_digest"));
        }
        members(
            "quality_rules",
            self.quality_rules.iter().map(String::as_str),
        )?;
        if self.freshness_s == 0 {
            return Err(invalid("freshness_s"));
        }
        Ok(())
    }
}

impl AccessRequest {
    pub fn validate(&self) -> Result<(), DataProductError> {
        text("product", &self.product)?;
        text("principal", &self.principal)?;
        if self.purpose.trim().is_empty() {
            return Err(DataProductError {
                code: ACCESS_PURPOSE_MISSING,
                field: "purpose",
            });
        }
        text("purpose", &self.purpose)
    }
}
