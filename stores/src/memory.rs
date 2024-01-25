use async_trait::async_trait;
use dashmap::DashMap;

use ruse_core::{Record, SessionStore, StoreError};

#[derive(Clone, Debug)]
pub struct MemoryStore {
    records: DashMap<String, String>,
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            records: DashMap::new(),
        }
    }
}

#[async_trait]
impl SessionStore for MemoryStore {
    async fn load(&self, id: &str) -> Option<Record> {
        self.records.get(id).map(|data| Record {
            id: id.to_string(),
            data: data.value().clone(),
        })
    }

    async fn save(&self, record: Record) -> Result<(), StoreError> {
        self.records.insert(record.id, record.data);
        Ok(())
    }

    async fn delete(&self, id: &str) -> Result<(), StoreError> {
        self.records.remove(id);
        Ok(())
    }
}
