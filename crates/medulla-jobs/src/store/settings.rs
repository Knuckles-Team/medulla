//! Engine settings rows kept in the job store (MEDULLA-DURABLE-KERNEL-R076.2).
//!
//! A setting is one text row under a `setting:` key of the store's key/value
//! ledger table. It is written as a ledgered maintenance batch, so it survives a
//! restart and is replicated and replayed like every other job-store write. The
//! `setting:` prefix keeps these rows apart from idempotency claims.

use super::*;

const SETTING_KEY_PREFIX: &str = "setting:";

fn setting_key(key: &str) -> Result<String> {
    let ledger_key = format!("{SETTING_KEY_PREFIX}{key}");
    if key.is_empty() || !valid_identifier(&ledger_key) {
        return Err(codec_err("job-store setting key is invalid"));
    }
    Ok(ledger_key)
}

impl JobStore {
    /// Store `value` under `key`, replacing any earlier value.
    pub fn put_setting(&self, key: &str, value: &str) -> Result<()> {
        let ledger_key = setting_key(key)?;
        if value.len() > MAX_JOB_STRING_BYTES {
            return Err(codec_err("job-store setting exceeds the storage limit"));
        }
        self.maintain("setting-put", |wtx| {
            let mut table = wtx.open_table(IDEMPOTENCY_LEDGER).map_err(redb_err)?;
            table.insert(ledger_key.as_str(), value).map_err(redb_err)?;
            Ok(())
        })
    }

    /// The value stored under `key`, if any.
    pub fn get_setting(&self, key: &str) -> Result<Option<String>> {
        let ledger_key = setting_key(key)?;
        self.first_wins_owner(
            IDEMPOTENCY_LEDGER,
            &ledger_key,
            "job-store setting key is invalid",
        )
    }
}
