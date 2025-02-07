use parking_lot::RwLock;
use serde::{de::DeserializeOwned, Serialize};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};

use crate::store::{Error, SessionStore};
use crate::Id;

pub(crate) fn serialize_value<T: Serialize>(value: &T) -> Result<Vec<u8>, Error> {
    rmp_serde::to_vec(value).map_err(|e| Error::Encode(e.to_string()))
}

pub(crate) fn deserialize_value<T: DeserializeOwned>(value: &[u8]) -> Result<T, Error> {
    rmp_serde::from_slice(value).map_err(|e| Error::Decode(e.to_string()))
}

#[derive(Debug, Clone)]
struct StoredValue {
    data: Vec<u8>,
    expires_at: Option<Instant>,
}

/// An in-memory session store implementation.
///
/// It uses a HashMap to manage session data and supports
/// serialization/deserialization using [MessagePack](https://crates.io/crates/rmp-serde).
///
/// ### Note
///
/// Do not use this in a production environment.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    data: Arc<RwLock<HashMap<String, HashMap<String, StoredValue>>>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            data: Arc::new(RwLock::new(HashMap::new())),
        }
    }

    fn cleanup_expired(&self) {
        let mut data = self.data.write();
        data.retain(|_, fields| {
            fields.retain(|_, value| {
                value
                    .expires_at
                    .map(|expires| expires > Instant::now())
                    .unwrap_or(true)
            });
            !fields.is_empty()
        });
    }
}

impl SessionStore for MemoryStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        self.cleanup_expired();

        let data = self.data.read();
        if let Some(fields) = data.get(&session_id.to_string()) {
            if let Some(value) = fields.get(field) {
                if value.expires_at.map(|e| e > Instant::now()).unwrap_or(true) {
                    return Ok(Some(deserialize_value(&value.data)?));
                }
            }
        }
        Ok(None)
    }

    async fn get_all<T>(&self, session_id: &Id) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        self.cleanup_expired();

        let data = self.data.read();
        if let Some(fields) = data.get(&session_id.to_string()) {
            if let Some(value) = fields.get("__all") {
                if value.expires_at.map(|e| e > Instant::now()).unwrap_or(true) {
                    return Ok(Some(deserialize_value(&value.data)?));
                }
            }
        }
        Ok(None)
    }

    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();

        let mut data = self.data.write();
        let fields = data.entry(session_id.to_string()).or_default();

        if fields.contains_key(field) {
            return Ok(false);
        }

        let expires_at = if key_seconds > 0 {
            Some(Instant::now() + Duration::from_secs(key_seconds as u64))
        } else {
            None
        };

        fields.insert(
            field.to_string(),
            StoredValue {
                data: serialize_value(value)?,
                expires_at,
            },
        );

        Ok(true)
    }

    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();

        let mut data = self.data.write();
        let fields = data.entry(session_id.to_string()).or_default();

        let expires_at = if key_seconds > 0 {
            Some(Instant::now() + Duration::from_secs(key_seconds as u64))
        } else {
            None
        };

        fields.insert(
            field.to_string(),
            StoredValue {
                data: serialize_value(value)?,
                expires_at,
            },
        );

        Ok(true)
    }

    async fn insert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();

        let mut data = self.data.write();

        // Check if old session exists and new session doesn't
        if !data.contains_key(&old_session_id.to_string()) || data.contains_key(&new_session_id.to_string()) {
            return Ok(false);
        }

        // Get the fields map, return false if field exists
        let fields = data.get_mut(&old_session_id.to_string()).unwrap();
        if fields.contains_key(field) {
            return Ok(false);
        }

        // Calculate expiration
        let expires_at = if key_seconds > 0 {
            Some(Instant::now() + Duration::from_secs(key_seconds as u64))
        } else {
            None
        };

        // Insert the new field
        fields.insert(
            field.to_string(),
            StoredValue {
                data: serialize_value(value)?,
                expires_at,
            },
        );

        // Move the map to the new session ID
        let fields = data.remove(&old_session_id.to_string()).unwrap();
        data.insert(new_session_id.to_string(), fields);

        Ok(true)
    }

    async fn update_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();

        let mut data = self.data.write();

        // Check if old session exists and new session doesn't
        if !data.contains_key(&old_session_id.to_string()) || data.contains_key(&new_session_id.to_string()) {
            return Ok(false);
        }

        // Calculate expiration
        let expires_at = if key_seconds > 0 {
            Some(Instant::now() + Duration::from_secs(key_seconds as u64))
        } else {
            None
        };

        // Update the field
        let fields = data.get_mut(&old_session_id.to_string()).unwrap();
        fields.insert(
            field.to_string(),
            StoredValue {
                data: serialize_value(value)?,
                expires_at,
            },
        );

        // Move the map to the new session ID
        let fields = data.remove(&old_session_id.to_string()).unwrap();
        data.insert(new_session_id.to_string(), fields);

        Ok(true)
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> Result<bool, Error> {
        self.cleanup_expired();

        let mut data = self.data.write();

        if data.contains_key(&new_session_id.to_string()) {
            return Ok(false);
        }

        if let Some(mut fields) = data.remove(&old_session_id.to_string()) {
            // Update expiration for all fields
            if seconds > 0 {
                let new_expires_at = Instant::now() + Duration::from_secs(seconds as u64);
                for value in fields.values_mut() {
                    value.expires_at = Some(new_expires_at);
                }
            }

            data.insert(new_session_id.to_string(), fields);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error> {
        self.cleanup_expired();

        let mut data = self.data.write();
        if let Some(fields) = data.get_mut(&session_id.to_string()) {
            fields.remove(field);
            Ok(if fields.is_empty() { 0 } else { 1 })
        } else {
            Ok(0)
        }
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        self.cleanup_expired();

        let mut data = self.data.write();
        Ok(data.remove(&session_id.to_string()).is_some())
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        if seconds <= 0 {
            return self.delete(session_id).await;
        }

        let mut data = self.data.write();
        if let Some(fields) = data.get_mut(&session_id.to_string()) {
            let expires_at = Instant::now() + Duration::from_secs(seconds as u64);
            for value in fields.values_mut() {
                value.expires_at = Some(expires_at);
            }
            Ok(true)
        } else {
            Ok(false)
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use tokio::time::sleep;

    #[derive(Debug, Clone, Serialize, Deserialize, PartialEq)]
    struct TestUser {
        id: i32,
        name: String,
    }

    #[tokio::test]
    async fn test_basic_operations() {
        let store = MemoryStore::new();
        let session_id = Id::default();
        let user = TestUser {
            id: 1,
            name: "Test User".to_string(),
        };

        // Test insert
        assert!(store
            .insert(&session_id, "user", &user, 60, None)
            .await
            .unwrap());

        // Test get
        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert_eq!(retrieved.unwrap(), user);

        // Test update
        let updated_user = TestUser {
            id: 1,
            name: "Updated User".to_string(),
        };
        assert!(store
            .update(&session_id, "user", &updated_user, 60, None)
            .await
            .unwrap());

        // Test delete
        assert!(store.delete(&session_id).await.unwrap());
        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_expiration() {
        let store = MemoryStore::new();
        let session_id = Id::default();
        let user = TestUser {
            id: 1,
            name: "Test User".to_string(),
        };

        // Insert with 1 second expiration
        assert!(store
            .insert(&session_id, "user", &user, 1, None)
            .await
            .unwrap());

        // Should exist immediately
        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_some());

        // Wait for expiration
        sleep(Duration::from_secs(2)).await;

        // Should be gone after expiration
        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_none());
    }
}
