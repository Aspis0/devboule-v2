//! Findings persist; a dismissal survives a rescan.
//!
//! SQLite rather than a JSON file: a crash mid-write of a JSON rewrite can
//! lose the dismissals, and rusqlite 0.40.2 is already the workspace pin
//! (Cargo allows only one `links = "sqlite3"`). Two tables, nothing else.

use std::path::Path;

use rusqlite::Connection;

use crate::error::Error;
use crate::finding::{Finding, FindingId};

pub struct Ledger {
    conn: Connection,
}

impl Ledger {
    pub fn open(path: &Path) -> Result<Self, Error> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let conn = Connection::open(path)?;
        conn.busy_timeout(std::time::Duration::from_secs(5))?;
        conn.execute_batch(
            "CREATE TABLE IF NOT EXISTS findings (
                id TEXT PRIMARY KEY,
                payload TEXT NOT NULL
             );
             CREATE TABLE IF NOT EXISTS dismissed (
                id TEXT PRIMARY KEY
             );",
        )?;
        Ok(Self { conn })
    }

    pub fn record_scan(&self, findings: &[Finding]) -> Result<(), Error> {
        let tx = self.conn.unchecked_transaction()?;
        tx.execute("DELETE FROM findings", [])?;
        {
            let mut insert =
                tx.prepare("INSERT OR REPLACE INTO findings (id, payload) VALUES (?1, ?2)")?;
            for finding in findings {
                insert.execute(rusqlite::params![
                    finding.id().as_str(),
                    crate::exchange::encode_ledger(finding),
                ])?;
            }
        }
        tx.commit()?;
        Ok(())
    }

    pub fn dismiss(&self, id: &FindingId) -> Result<(), Error> {
        self.conn.execute(
            "INSERT OR IGNORE INTO dismissed (id) VALUES (?1)",
            [id.as_str()],
        )?;
        Ok(())
    }

    pub fn active(&self) -> Result<Vec<Finding>, Error> {
        let mut statement = self
            .conn
            .prepare("SELECT payload FROM findings WHERE id NOT IN (SELECT id FROM dismissed)")?;
        let rows = statement.query_map([], |row| row.get::<_, String>(0))?;
        let mut findings = Vec::new();
        for row in rows {
            if let Some(finding) = crate::exchange::decode_ledger(&row?) {
                findings.push(finding);
            }
        }
        Ok(findings)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::finding::{Finding, Severity};
    use crate::tokens;
    use std::path::PathBuf;

    fn a_finding(root: &Path, file: &str, excerpt: &str, line: usize) -> Finding {
        let path = root.join(file);
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent).expect("mkdir");
        }
        let mut body = String::new();
        for _ in 1..line {
            body.push('\n');
        }
        body.push_str(excerpt);
        body.push('\n');
        std::fs::write(&path, body).expect("write");
        Finding::grounded(
            root,
            crate::finding::Draft {
                file: PathBuf::from(file),
                start_line: line,
                end_line: line,
                rule: "secret",
                severity: Severity::Inferno,
                source: "secrets",
                title: "A secret-looking token",
                raw_excerpt: excerpt,
            },
        )
        .expect("grounded")
    }

    #[test]
    fn a_dismissal_survives_a_rescan() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open(&temp.path().join("augur.sqlite")).expect("open");
        let finding = a_finding(temp.path(), "src/auth.rs", &tokens::aws_example(), 1);
        ledger
            .record_scan(std::slice::from_ref(&finding))
            .expect("record");
        ledger.dismiss(finding.id()).expect("dismiss");

        let shifted = a_finding(temp.path(), "src/auth.rs", &tokens::aws_example(), 4);
        assert_eq!(
            finding.id(),
            shifted.id(),
            "the id must survive the line shift"
        );
        ledger.record_scan(&[shifted]).expect("rescan");

        let active = ledger.active().expect("active");
        assert!(
            active.is_empty(),
            "a dismissed finding came back after a rescan: {active:?}"
        );
    }

    #[test]
    fn a_new_secret_is_not_covered_by_an_old_dismissal() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open(&temp.path().join("augur.sqlite")).expect("open");
        let first = a_finding(temp.path(), "src/auth.rs", &tokens::aws_example(), 1);
        ledger
            .record_scan(std::slice::from_ref(&first))
            .expect("record");
        ledger.dismiss(first.id()).expect("dismiss");

        let second = a_finding(temp.path(), "src/auth.rs", &tokens::aws_changed(), 1);
        ledger
            .record_scan(std::slice::from_ref(&second))
            .expect("rescan");
        let active = ledger.active().expect("active");
        assert_eq!(active.len(), 1, "the new secret was swallowed: {active:?}");
        assert_eq!(active[0].id(), second.id());
    }

    #[test]
    fn the_ledger_stores_a_payload_not_finding_fields() {
        let temp = tempfile::tempdir().expect("tempdir");
        let path = temp.path().join("augur.sqlite");
        let ledger = Ledger::open(&path).expect("open");
        let finding = a_finding(temp.path(), "src/auth.rs", &tokens::aws_example(), 1);
        ledger
            .record_scan(std::slice::from_ref(&finding))
            .expect("record");
        let names: Vec<String> = ledger
            .conn
            .prepare("PRAGMA table_info(findings)")
            .expect("pragma")
            .query_map([], |row| row.get::<_, String>(1))
            .expect("rows")
            .map(|name| name.expect("name"))
            .collect();
        assert!(
            names.iter().any(|name| name == "payload"),
            "storage should be a blob so the on-disk shape can change in one module: {names:?}"
        );
        assert!(
            !names
                .iter()
                .any(|name| name == "start_line" || name == "evidence"),
            "Finding field names leaked into the table: {names:?}"
        );
    }

    #[test]
    fn two_findings_with_the_same_id_do_not_wipe_the_ledger() {
        let temp = tempfile::tempdir().expect("tempdir");
        let ledger = Ledger::open(&temp.path().join("augur.sqlite")).expect("open");
        let first = a_finding(temp.path(), "src/auth.rs", &tokens::aws_example(), 1);
        let twin = a_finding(temp.path(), "src/auth.rs", &tokens::aws_example(), 4);
        assert_eq!(first.id(), twin.id());
        ledger
            .record_scan(&[first.clone(), twin])
            .expect("duplicate ids must not abort after DELETE");
        let active = ledger.active().expect("active");
        assert_eq!(active.len(), 1, "identity is unique: {active:?}");
        assert_eq!(active[0].id(), first.id());
    }
}
