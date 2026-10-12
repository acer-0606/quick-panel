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
            // CASE is lazy: type rejection must precede length(TEXT), and the
            // full BLOB column must only execute after its size is known safe.
            // One statement keeps validation and payload in the same snapshot.
            // A rejected row yields NULL (an error below), never a missing row.
            "SELECT revision, CASE WHEN typeof(envelope)='blob' THEN
                CASE WHEN length(envelope) BETWEEN 96 AND ?1 THEN envelope END
             END FROM account_vault WHERE id=1",
            [crate::account_vault::MAX_ENVELOPE_BYTES as i64],
            |r| {
                Ok(VaultSnapshot {
                    revision: r
                        .get::<_, i64>(0)?
                        .try_into()
                        .map_err(|_| rusqlite::Error::InvalidQuery)?,
                    envelope: r.get(1)?,
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
    /// Return the durable revision and whether this call committed a write.
    pub(crate) fn write_account_vault(
        &mut self,
        expected: u64,
        envelope: &[u8],
    ) -> Result<(u64, bool), String> {
        validate_envelope(envelope)?;
        let next = expected
            .checked_add(1)
            .filter(|v| *v <= i64::MAX as u64)
            .ok_or(STORE_ERROR)?;
        // Idempotent replay is read-only, including after a lost success receipt.
        if read(&self.connection)?.is_some_and(|s| s.revision == next && s.envelope == envelope) {
            return Ok((next, false));
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
            return Ok((next, false));
        }
        if previous.as_ref().map_or(0, |s| s.revision) != expected {
            return Err("账号保险箱已更改，请重新解锁".into());
        }
        tx.execute_batch("CREATE TABLE IF NOT EXISTS account_vault (id INTEGER PRIMARY KEY CHECK(id=1), revision INTEGER NOT NULL CHECK(revision>0), envelope BLOB NOT NULL CHECK(length(envelope)<=4194304))").map_err(|_|STORE_ERROR)?;
        tx.execute("INSERT INTO account_vault(id,revision,envelope) VALUES(1,?1,?2) ON CONFLICT(id) DO UPDATE SET revision=excluded.revision,envelope=excluded.envelope",params![next as i64,envelope]).map_err(|_|STORE_ERROR)?;
        tx.commit().map_err(|_| STORE_ERROR)?;
        Ok((next, true))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use rusqlite::{Connection, ffi};
    use std::path::Path;

    fn cold_reader(path: &Path) -> Connection {
        let db = Connection::open(path).unwrap();
        db.execute_batch("PRAGMA mmap_size=0; PRAGMA cache_size=-64;")
            .unwrap();
        db
    }
    fn memory_status(reset: bool) -> (i64, i64) {
        let mut current = 0;
        let mut highwater = 0;
        // SAFETY: these output pointers remain valid for the synchronous call.
        // The probe runs alone in a child, since SQLite's counters are global.
        let status = unsafe {
            ffi::sqlite3_status64(
                ffi::SQLITE_STATUS_MEMORY_USED,
                &mut current,
                &mut highwater,
                i32::from(reset),
            )
        };
        assert_eq!(status, ffi::SQLITE_OK);
        (current, highwater)
    }
    fn isolated_probe(kind: &str) {
        let output = std::process::Command::new(std::env::current_exe().unwrap())
            .args([
                "--exact",
                "account_vault_store::tests::sqlite_materialization_probe",
                "--ignored",
                "--nocapture",
            ])
            .env("QP_VAULT_MATERIALIZATION_KIND", kind)
            .output()
            .unwrap();
        let stdout = String::from_utf8_lossy(&output.stdout);
        let stderr = String::from_utf8_lossy(&output.stderr);
        println!("{stdout}");
        assert!(
            output.status.success(),
            "isolated {kind} probe failed: {stdout} {stderr}"
        );
    }
    #[test]
    fn oversized_blob_rejected_before_sqlite_materialization() {
        isolated_probe("BLOB");
    }
    #[test]
    fn wrong_type_text_rejected_before_sqlite_materialization() {
        isolated_probe("TEXT");
    }
    #[test]
    #[ignore = "run by the bounded-reader tests in an isolated child process"]
    fn sqlite_materialization_probe() {
        let kind = std::env::var("QP_VAULT_MATERIALIZATION_KIND").unwrap_or_else(|_| "BLOB".into());
        let expression = match kind.as_str() {
            "BLOB" => "zeroblob(?1)",
            "TEXT" => "CAST(zeroblob(?1) AS TEXT)",
            _ => panic!("unknown synthetic fixture"),
        };
        let temp = tempfile::tempdir().unwrap();
        let path = temp.path().join("malformed-vault.sqlite");
        let writer = Connection::open(&path).unwrap();
        writer.execute_batch("PRAGMA page_size=4096; CREATE TABLE account_vault(id INTEGER PRIMARY KEY,revision INTEGER,envelope BLOB);").unwrap();
        writer
            .execute(
                &format!("INSERT INTO account_vault VALUES(1,1,{expression})"),
                [32 * 1024 * 1024],
            )
            .unwrap();
        drop(writer);

        let db = cold_reader(&path);
        let baseline = memory_status(true).0;
        assert!(read(&db).is_err(), "invalid record must not become None");
        let guarded_peak = memory_status(false).1 - baseline;
        drop(db);

        // Real bundled-SQLite control: SELECT materializes the column even
        // though the Rust callback never requests it. This calibrates the
        // resource counter instead of assuming its platform/build behavior.
        let control = cold_reader(&path);
        let baseline = memory_status(true).0;
        control
            .query_row("SELECT envelope FROM account_vault WHERE id=1", [], |_| {
                Ok(())
            })
            .unwrap();
        let full_peak = memory_status(false).1 - baseline;
        let version: String = control
            .query_row("SELECT sqlite_version()", [], |r| r.get(0))
            .unwrap();
        println!(
            "bundled SQLite {version}, {kind}, payload=33554432 bytes: guarded peak growth={guarded_peak}; full-column control={full_peak}"
        );
        assert!(
            full_peak > 24 * 1024 * 1024,
            "measurement must detect full payload allocation"
        );
        assert!(
            guarded_peak < 1024 * 1024,
            "reject before SQLite materializes payload: peak growth={guarded_peak}"
        );
    }
}
