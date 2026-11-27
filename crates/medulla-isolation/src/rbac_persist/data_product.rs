//! MEDULLA-TYPED-PACKS-R805.3: durable data products, their contracts and access requests.
//!
//! One JSON row each in the owner table `rbac_data_product`, keyed
//! `product:<tenant>:<id>`, `contract:<tenant>:<id>` and
//! `request:<tenant>:<product>:<principal>`. A product and its contract are written in one
//! authority commit; an access request is written whole, its `state` only ever set by the
//! caller through `AccessRequestMachine` (`medulla_types::access_request_machine`). The store
//! decides no admission: the served handler verifies the caller against the product's roles
//! before it writes the decision.

use medulla_catalog_types::data_product::{AccessRequest, DataContract, DataProduct};
use redb::{ReadableTable, TableDefinition};
use sha2::{Digest, Sha256};

use super::durable_write::commit_authority_mutation;
use super::reservation::require_no_open_reservation;
use super::{RbacPersistError, RbacStore, RBAC_TABLE};

/// One row per product, contract or access request, JSON bytes.
const PRODUCT_TABLE: TableDefinition<&str, &[u8]> = TableDefinition::new("rbac_data_product");

fn redb(error: impl ToString) -> RbacPersistError {
    RbacPersistError::Redb(error.to_string())
}

fn invalid(field: &str) -> RbacPersistError {
    RbacPersistError::Redb(format!("INVALID_FIELD: {field}"))
}

/// A key part must not contain the key separator, so two records never share a key.
fn key_part(field: &str, value: &str) -> Result<(), RbacPersistError> {
    if value.is_empty() || value.contains(':') {
        return Err(invalid(field));
    }
    Ok(())
}

fn product_key(tenant: &str, id: &str) -> String {
    format!("product:{tenant}:{id}")
}

fn contract_key(tenant: &str, id: &str) -> String {
    format!("contract:{tenant}:{id}")
}

fn request_key(tenant: &str, product: &str, principal: &str) -> String {
    format!("request:{tenant}:{product}:{principal}")
}

fn batch_digest(kind: &str, key: &str, row: &[u8]) -> String {
    let mut hash = Sha256::new();
    hash.update(kind.as_bytes());
    hash.update([0]);
    hash.update(key.as_bytes());
    hash.update([0]);
    hash.update(row);
    hex::encode(hash.finalize())
}

impl RbacStore {
    fn data_product_row(&self, key: &str) -> Result<Option<Vec<u8>>, RbacPersistError> {
        let read = self.scoped_read()?;
        let table = read
            .open_owner_table(PRODUCT_TABLE)
            .map_err(RbacPersistError::Redb)?;
        let row = table.get(key).map_err(redb)?;
        Ok(row.map(|row| row.value().to_vec()))
    }

    /// The product and the contract of `tenant` named `id`, when both are stored.
    pub fn data_product(
        &self,
        tenant: &str,
        id: &str,
    ) -> Result<Option<(DataProduct, DataContract)>, RbacPersistError> {
        let Some(product) = self.data_product_row(&product_key(tenant, id))? else {
            return Ok(None);
        };
        let Some(contract) = self.data_product_row(&contract_key(tenant, id))? else {
            return Ok(None);
        };
        Ok(Some((
            serde_json::from_slice(&product)?,
            serde_json::from_slice(&contract)?,
        )))
    }

    /// Every product and contract of `tenant`, in id order. A product whose contract row is
    /// missing is left out, as [`Self::data_product`] leaves it out.
    pub fn data_products(
        &self,
        tenant: &str,
    ) -> Result<Vec<(DataProduct, DataContract)>, RbacPersistError> {
        key_part("tenant", tenant)?;
        let prefix = product_key(tenant, "");
        let read = self.scoped_read()?;
        let table = read
            .open_owner_table(PRODUCT_TABLE)
            .map_err(RbacPersistError::Redb)?;
        let mut products: Vec<DataProduct> = Vec::new();
        for entry in table.range::<&str>(prefix.as_str()..).map_err(redb)? {
            let (key, row) = entry.map_err(redb)?;
            if !key.value().starts_with(prefix.as_str()) {
                break;
            }
            products.push(serde_json::from_slice(row.value())?);
        }
        let mut out = Vec::with_capacity(products.len());
        for product in products {
            let key = contract_key(tenant, &product.id);
            if let Some(row) = table.get(key.as_str()).map_err(redb)? {
                out.push((product, serde_json::from_slice(row.value())?));
            }
        }
        Ok(out)
    }

    /// Store `product` and its `contract` in one commit, replacing any earlier pair of the same
    /// id. Refuses with `INVALID_FIELD` when a record fails validation, the contract names
    /// another product or an id carries `:`; with `RBAC_RESERVED` while a reservation holds the
    /// authorization image.
    pub fn put_data_product(
        &self,
        tenant: &str,
        product: &DataProduct,
        contract: &DataContract,
    ) -> Result<(), RbacPersistError> {
        key_part("tenant", tenant)?;
        key_part("id", &product.id)?;
        product.validate().map_err(|error| invalid(error.field))?;
        contract.validate().map_err(|error| invalid(error.field))?;
        if contract.product != product.id {
            return Err(invalid("product"));
        }
        let rows = [
            (
                product_key(tenant, &product.id),
                serde_json::to_vec(product)?,
            ),
            (
                contract_key(tenant, &product.id),
                serde_json::to_vec(contract)?,
            ),
        ];
        let digest = batch_digest("product", &rows[0].0, &rows[0].1);
        commit_authority_mutation(
            self,
            self.current_version()?,
            &digest,
            None,
            |owner_write| {
                require_no_open_reservation(
                    &owner_write
                        .open_table(RBAC_TABLE)
                        .map_err(RbacPersistError::Redb)?,
                )?;
                let mut table = owner_write
                    .open_table(PRODUCT_TABLE)
                    .map_err(RbacPersistError::Redb)?;
                for (key, row) in &rows {
                    table.insert(key.as_str(), row.as_slice()).map_err(redb)?;
                }
                Ok(())
            },
        )?;
        Ok(())
    }

    /// The access request `principal` holds against `product`, when one is stored.
    pub fn access_request(
        &self,
        tenant: &str,
        product: &str,
        principal: &str,
    ) -> Result<Option<AccessRequest>, RbacPersistError> {
        self.data_product_row(&request_key(tenant, product, principal))?
            .map(|row| serde_json::from_slice(&row).map_err(RbacPersistError::from))
            .transpose()
    }

    /// Store `request` whole under its product and principal, replacing the earlier row. The
    /// caller moved `state` through `AccessRequestMachine`. Refuses with `INVALID_FIELD` when the
    /// request fails validation or an id carries `:` in the product part.
    pub fn put_access_request(
        &self,
        tenant: &str,
        request: &AccessRequest,
    ) -> Result<(), RbacPersistError> {
        key_part("tenant", tenant)?;
        key_part("product", &request.product)?;
        request.validate().map_err(|error| invalid(error.field))?;
        let key = request_key(tenant, &request.product, &request.principal);
        let row = serde_json::to_vec(request)?;
        let digest = batch_digest("request", &key, &row);
        commit_authority_mutation(
            self,
            self.current_version()?,
            &digest,
            None,
            |owner_write| {
                require_no_open_reservation(
                    &owner_write
                        .open_table(RBAC_TABLE)
                        .map_err(RbacPersistError::Redb)?,
                )?;
                owner_write
                    .open_table(PRODUCT_TABLE)
                    .map_err(RbacPersistError::Redb)?
                    .insert(key.as_str(), row.as_slice())
                    .map_err(redb)?;
                Ok(())
            },
        )?;
        Ok(())
    }
}
