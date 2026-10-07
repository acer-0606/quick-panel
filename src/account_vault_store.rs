//! The actor sees only a validated envelope; no passwords, accounts or scenes.
use crate::{
    account_vault::{VaultSnapshot, validate_envelope},
    repository::Repository,
};
use rusqlite::{OptionalExtension, params};
const STORE_ERROR: &str = "账号保险箱存储失败";
fn read(connection: &rusqlite::Connection) -> Result<Option<VaultSnapshot>, String> {
    let exists:bool=connection.query_row("SELECT EXISTS(SELECT 1 FROM sqlite_master WHERE type='table' AND name='account_vault')",[],|r|r.get(0)).map_err(|_|STORE_ERROR)?;
    if !exists {
        return Ok(None);
    }
    let snapshot = connection
        .query_row(
            "SELECT revision,length(envelope),envelope FROM account_vault WHERE id=1",
            [],
            |r| {
                let length: i64 = r.get(1)?;
                if !(96..=crate::account_vault::MAX_ENVELOPE_BYTES as i64).contains(&length) {
                    return Err(rusqlite::Error::InvalidQuery);
                }
                Ok(VaultSnapshot {
                    revision: r
                        .get::<_, i64>(0)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    envelope: r.get(2)?,
                })
            },
        )
        .optional()
        .map_err(|_| STORE_ERROR)?;
    if let Some(s) = &snapshot {
        if s.revision == 0 {
            return Err(STORE_ERROR.into());
        }
        validate_envelope(&s.envelope)?;
    }
    Ok(snapshot)
}
impl Repository {
    pub(crate) fn read_account_vault(&self) -> Result<Option<VaultSnapshot>, String> {
        read(&self.connection)
    }
    pub(crate) fn write_account_vault(
        &mut self,
        expected: u64,
        envelope: &[u8],
    ) -> Result<u64, String> {
        validate_envelope(envelope)?;
        let next = expected
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or(STORE_ERROR)?;
        // Idempotent replay is read-only, including after a lost success receipt.
        if read(&self.connection)?.is_some_and(|s| s.revision == next && s.envelope == envelope) {
            return Ok(next);
        }
        let tx = self
            .connection
            .transaction_with_behavior(rusqlite::TransactionBehavior::Immediate)
            .map_err(|_| STORE_ERROR)?;
        let previous = read(&tx)?;
        if previous
            .as_ref()
            .is_some_and(|s| s.revision == next && s.envelope == envelope)
        {
            return Ok(next);
        }
        if previous.as_ref().map_or(0, |s| s.revision) != expected {
            return Err("账号保险箱已更改，请重新解锁".into());
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS account_vault (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL CHECK(revision>0), envelope BLOB NOT NULL CHECK(length(envelope)<=4194304))").map_err(|_|STORE_ERROR)?;
        tx.execute("INSERT INTO account_vault(id,revision,envelope) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,envelope=excluded.envelope",params![next as i64,envelope]).map_err(|_|STORE_ERROR)?;
        tx.commit().map_err(|_| STORE_ERROR)?;
        Ok(next)
    }
}
