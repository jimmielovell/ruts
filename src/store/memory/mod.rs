use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use dashmap::DashMap;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::time::{Duration, Instant};

#[derive(Debug, Clone)]
struct StoredValue {
    data: Vec<u8>,
    expires_at: Option<Instant>,
}

/// An in-memory session store implementation.
///
/// It uses a HashMap to manage session data.
#[derive(Debug, Clone)]
pub struct MemoryStore {
    data: DashMap<String, HashMap<String, StoredValue>>,
}

impl Default for MemoryStore {
    fn default() -> Self {
        Self::new()
    }
}

impl MemoryStore {
    pub fn new() -> Self {
        Self {
            data: DashMap::new(),
        }
    }

    fn cleanup_expired(&self) {
        self.data.retain(|_, fields| {
            fields.retain(|_, value| {
                value
                    .expires_at
                    .map(|expires| expires > Instant::now())
                    .unwrap_or(true)
            });
            !fields.is_empty()
        });
    }

    fn get_max_ttl(&self, session_id: &Id) -> i64 {
        self.data
            .get(&session_id.to_string())
            .and_then(|fields| {
                fields
                    .values()
                    .filter_map(|v| v.expires_at)
                    .max()
                    .map(|exp| {
                        let now = Instant::now();
                        if exp > now {
                            exp.duration_since(now).as_secs() as i64
                        } else {
                            -2
                        }
                    })
            })
            .unwrap_or(-1)
    }
}

fn expires_at(key_ttl_secs: Option<i64>, field_ttl_secs: Option<i64>) -> Option<Instant> {
    let ttl = match (key_ttl_secs, field_ttl_secs) {
        (Some(_), Some(fts)) | (None, Some(fts)) => Some(fts),
        (Some(kts), None) => Some(kts),
        (None, None) => None,
    };

    if let Some(ttl) = ttl
        && ttl > 0
    {
        Some(Instant::now() + Duration::from_secs(ttl as u64))
    } else {
        None
    }
}

impl SessionStore for MemoryStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        self.cleanup_expired();
        if let Some(fields) = self.data.get(&session_id.to_string()) {
            if let Some(value) = fields.get(field) {
                if value.expires_at.map(|e| e > Instant::now()).unwrap_or(true) {
                    return Ok(Some(deserialize_value(&value.data)?));
                }
            }
        }
        Ok(None)
    }

    /// This method is not implemented for `MemoryStore`.
    ///
    /// The `get_all` functionality is designed as an optimization for stores
    /// where a single bulk fetch is more performant than multiple individual
    /// lookups (e.g., Redis, Postgres).
    ///
    /// For `MemoryStore`, all data is already in local memory, making direct calls
    /// to `session.get()` optimally efficient. Implementing `get_all` would
    /// introduce an unnecessary intermediate allocation (`SessionMap`) without any
    /// performance gain.
    ///
    /// It is recommended to use `session.get()` for individual field access when
    /// using `MemoryStore`.
    async fn get_all(&self, _session_id: &Id) -> Result<Option<SessionMap>, Error> {
        unimplemented!(
            "`get_all` is intentionally not implemented for `MemoryStore`.
            Please use `session.get()` directly for the most efficient access."
        );
    }

    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();
        {
            let fields = self.data.entry(session_id.to_string()).or_default();
            if fields.contains_key(field) {
                return Ok(-2);
            }
        }

        let expires_at = expires_at(key_ttl_secs, field_ttl_secs);
        {
            let mut fields = self.data.entry(session_id.to_string()).or_default();
            fields.insert(
                field.to_string(),
                StoredValue {
                    data: serialize_value(value)?,
                    expires_at,
                },
            );
        }

        Ok(self.get_max_ttl(session_id))
    }

    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();

        {
            let expires_at = expires_at(key_ttl_secs, field_ttl_secs);
            let mut fields = self.data.entry(session_id.to_string()).or_default();
            fields.insert(
                field.to_string(),
                StoredValue {
                    data: serialize_value(value)?,
                    expires_at,
                },
            );
        }

        Ok(self.get_max_ttl(session_id))
    }

    async fn insert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();
        let old_key = old_session_id.to_string();
        let new_key = new_session_id.to_string();

        {
            let fields = self.data.get(&old_key).unwrap();
            if fields.contains_key(field) {
                return Ok(-2);
            }
        }

        let expires_at = expires_at(key_ttl_secs, field_ttl_secs);
        {
            let mut fields = self.data.get_mut(&old_key).unwrap();
            fields.insert(
                field.to_string(),
                StoredValue {
                    data: serialize_value(value)?,
                    expires_at,
                },
            );
        }
        {
            let (_, fields) = self.data.remove(&old_key).unwrap();
            self.data.insert(new_key, fields);
        }

        Ok(self.get_max_ttl(new_session_id))
    }

    async fn update_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();
        let old_key = old_session_id.to_string();
        let new_key = new_session_id.to_string();

        let expires_at = expires_at(key_ttl_secs, field_ttl_secs);
        {
            let mut fields = self.data.get_mut(&old_key).unwrap();
            fields.insert(
                field.to_string(),
                StoredValue {
                    data: serialize_value(value)?,
                    expires_at,
                },
            );
        }
        {
            let (_, fields) = self.data.remove(&old_key).unwrap();
            self.data.insert(new_key, fields);
        }

        Ok(self.get_max_ttl(new_session_id))
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        self.cleanup_expired();
        if self.data.contains_key(&new_session_id.to_string()) {
            return Ok(false);
        }

        if let Some((_, fields)) = self.data.remove(&old_session_id.to_string()) {
            self.data.insert(new_session_id.to_string(), fields);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        self.cleanup_expired();

        if let Some(mut fields) = self.data.get_mut(&session_id.to_string()) {
            let removed = fields.remove(field).is_some();
            if removed {
                let ttl = fields
                    .values()
                    .filter_map(|v| v.expires_at)
                    .min()
                    .map(|e| {
                        let now = Instant::now();
                        if e > now {
                            e.duration_since(now).as_secs() as i64
                        } else {
                            -2
                        }
                    })
                    .unwrap_or(-1); // no expiry
                return Ok(ttl);
            }
            return Ok(-2);
        }
        Ok(-2)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        self.cleanup_expired();

        Ok(self.data.remove(&session_id.to_string()).is_some())
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        if seconds <= 0 {
            return self.delete(session_id).await;
        }

        if let Some(mut fields) = self.data.get_mut(&session_id.to_string()) {
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

        let ttl = store
            .insert(&session_id, "user", &user, Some(30), Some(30), None)
            .await
            .unwrap();
        // TTL can be slightly less due to execution time, so we check a range.
        assert!(ttl <= 30 && ttl > 28);

        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert_eq!(retrieved.unwrap(), user);

        let updated_user = TestUser {
            id: 1,
            name: "Updated User".to_string(),
        };

        let ttl = store
            .update(&session_id, "user", &updated_user, Some(60), Some(60), None)
            .await
            .unwrap();
        assert!(ttl <= 60 && ttl > 58);

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

        store
            .insert(&session_id, "user", &user, Some(2), Some(2), None)
            .await
            .unwrap();

        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_some());

        sleep(Duration::from_secs(3)).await;

        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_none());
    }
}
