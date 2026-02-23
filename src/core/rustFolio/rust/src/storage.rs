// storage.rs — redb table definitions and persistent storage operations.
//
// Tables:
//   RECORDS:      &str → &[u8]   (Ref.id → serialized persistent Dict)
//   META:         &str → &[u8]   (system key/value pairs)
//   HISTORY:      &[u8] → &[u8]  (composite key: ref_id + timestamp → value)
//   HISTORY_META: &str → &[u8]   (per-point history metadata)

use redb::{Database, ReadableTable, TableDefinition};
use std::path::Path;
use crate::error::{FolioError, Result};
use crate::types::*;
use crate::types_ser::*;
use crate::types_de::*;

const RECORDS:      TableDefinition<&str, &[u8]> = TableDefinition::new("records");
const META:         TableDefinition<&str, &[u8]> = TableDefinition::new("meta");

pub struct Storage {
    pub db: Database,
}

impl Storage {
    /// Open or create the redb database.
    pub fn open(path: &Path) -> Result<Self> {
        // Create parent directory if needed
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let db = Database::create(path)?;
        // Initialize tables
        let tx = db.begin_write()?;
        tx.open_table(RECORDS)?;
        tx.open_table(META)?;
        tx.commit()?;
        Ok(Storage { db })
    }

    /// Read curVer from META table.
    pub fn read_cur_ver(&self) -> Result<u64> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(META)?;
        match table.get("curVer")? {
            None       => Ok(0),
            Some(v)    => {
                let bytes = v.value();
                if bytes.len() < 8 {
                    return Ok(0);
                }
                Ok(u64::from_be_bytes(bytes[..8].try_into().unwrap()))
            }
        }
    }

    /// Write curVer to META table (within a write transaction).
    pub fn write_cur_ver_tx(tx: &redb::WriteTransaction, ver: u64) -> Result<()> {
        let mut table = tx.open_table(META)?;
        table.insert("curVer", ver.to_be_bytes().as_ref())?;
        Ok(())
    }

    /// Load all records from RECORDS table as (id, Dict) pairs.
    pub fn load_all_records(&self) -> Result<Vec<(String, Dict)>> {
        let tx = self.db.begin_read()?;
        let table = tx.open_table(RECORDS)?;
        let mut records = Vec::new();
        for entry in table.iter()? {
            let (k, v) = entry?;
            let id = k.value().to_string();
            let bytes = v.value();
            let mut pos = 0usize;
            let dict = read_dict(bytes, &mut pos)?;
            records.push((id, dict));
        }
        Ok(records)
    }

    /// Write a batch of persistent record changes atomically.
    /// `changes` is a list of (id, Option<Dict>):
    ///   - Some(dict) = upsert
    ///   - None = delete
    /// Returns the new curVer.
    pub fn commit_records(
        &self,
        changes: &[(String, Option<Dict>)],
        old_ver: u64,
    ) -> Result<u64> {
        let new_ver = old_ver + 1;
        let tx = self.db.begin_write()?;
        {
            let mut table = tx.open_table(RECORDS)?;
            for (id, dict_opt) in changes {
                match dict_opt {
                    Some(dict) => {
                        let mut buf = Vec::new();
                        write_dict(&mut buf, dict);
                        table.insert(id.as_str(), buf.as_slice())?;
                    }
                    None => {
                        table.remove(id.as_str())?;
                    }
                }
            }
        }
        Self::write_cur_ver_tx(&tx, new_ver)?;
        tx.commit()?;
        Ok(new_ver)
    }
}
