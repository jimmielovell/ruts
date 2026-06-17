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

/// An in-memory session store, intended for development and testing.
///
/// Backed by a [`DashMap`] for concurrent access. Data is lost when the
/// process exits; use [`RedisStore`] or [`PostgresStore`] for production.
///
/// # Behavioral divergence from production stores
///
/// `MemoryStore` tracks expiry per-field only — there is no separate
/// session-level TTL. As a consequence, the TTL returned by [`set`],
/// [`set_and_rename`], and [`remove`] is synthesized from the maximum
/// finite field expiry in the session, which can differ from what
/// [`RedisStore`] or [`PostgresStore`] would return for the same call.
///
/// Tests that assert on the *value* of returned TTLs may pass against
/// `MemoryStore` and fail against the production stores (or vice versa).
/// Prefer asserting on observable behavior — whether a field exists,
/// whether `get` returns `Some` or `None` — over the returned TTL.
///
/// [`set`]: SessionStore::set
/// [`set_and_rename`]: SessionStore::set_and_rename
/// [`remove`]: SessionStore::remove
/// [`RedisStore`]: crate::store::redis::RedisStore
/// [`PostgresStore`]: crate::store::postgres::PostgresStore
#[derive(Debug, Clone)]
pub struct MemoryStore {
    data: DashMap<String, HashMap<String, StoredValue>>,
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

    fn get_ttl(&self, session_id: &Id) -> i64 {
        if let Some(fields) = self.data.get(session_id.as_str()) {
            if fields.is_empty() {
                return -2;
            }

            let mut max_finite = None;
            let now = Instant::now();

            for val in fields.values() {
                match val.expires_at {
                    None => return -1,
                    Some(exp) => {
                        if exp > now {
                            match max_finite {
                                None => max_finite = Some(exp),
                                Some(current) if exp > current => max_finite = Some(exp),
                                _ => {}
                            }
                        }
                    }
                }
            }

            match max_finite {
                Some(exp) => exp.duration_since(now).as_secs() as i64,
                None => -2,
            }
        } else {
            -2
        }
    }
}

fn determine_expiry(key_ttl_secs: i64, field_ttl_secs: i64) -> Option<Instant> {
    let ttl = match (key_ttl_secs, field_ttl_secs) {
        (-1, -1) => return None,
        (-1, t) | (t, -1) => t,
        (a, b) => a.min(b),
    };
    (ttl > 0).then(|| Instant::now() + Duration::from_secs(ttl as u64))
}

impl SessionStore for MemoryStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        if let Some(fields) = self.data.get(session_id.as_str()) {
            if let Some(value) = fields.get(field) {
                if value.expires_at.map(|e| e > Instant::now()).unwrap_or(true) {
                    return Ok(Some(deserialize_value(&value.data)?));
                }
            }
        }
        Ok(None)
    }

    async fn get_all(&self, _session_id: &Id) -> Result<Option<SessionMap>, Error> {
        Err(Error::Backend(
            "MemoryStore does not support get_all; use get() per field instead".to_string(),
        ))
    }

    async fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        if key_ttl_secs == 0 {
            self.delete(session_id).await?;
            return Ok(-2);
        }
        if field_ttl_secs == 0 {
            return self.remove(session_id, field).await;
        }

        self.cleanup_expired();

        let expires_at = determine_expiry(key_ttl_secs, field_ttl_secs);

        let mut fields = self.data.entry(session_id.to_string()).or_default();
        fields.insert(
            field.to_string(),
            StoredValue {
                data: serialize_value(value)?,
                expires_at,
            },
        );

        drop(fields);

        Ok(self.get_ttl(session_id))
    }

    async fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        self.cleanup_expired();

        let old_key = old_session_id.as_str();
        let new_key = new_session_id.as_str();

        if old_key != new_key && self.data.contains_key(new_key) {
            return Err(Error::Backend(format!(
                "rename failed: target session {new_session_id} already exists"
            )));
        }

        if key_ttl_secs == 0 {
            self.data.remove(old_key);
            return Ok(-2);
        }

        let mut fields = self
            .data
            .remove(old_key)
            .map(|(_, f)| f)
            .unwrap_or_default();

        if field_ttl_secs == 0 {
            fields.remove(field);
        } else {
            let expires_at = determine_expiry(key_ttl_secs, field_ttl_secs);
            fields.insert(
                field.to_string(),
                StoredValue {
                    data: serialize_value(value)?,
                    expires_at,
                },
            );
        }

        if fields.is_empty() {
            return Ok(-2);
        }

        self.data.insert(new_key.to_owned(), fields);
        Ok(self.get_ttl(new_session_id))
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        self.cleanup_expired();

        let new_key = new_session_id.to_string();

        if self.data.contains_key(&new_key) {
            return Ok(false);
        }

        if let Some((_, fields)) = self.data.remove(old_session_id.as_str()) {
            self.data.insert(new_key, fields);
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        self.cleanup_expired();

        let session_id_str = session_id.as_str();

        if let Some(mut fields) = self.data.get_mut(session_id_str) {
            let removed = fields.remove(field).is_some();

            if fields.is_empty() {
                drop(fields);
                self.data.remove(session_id_str);
                return Ok(-2);
            }

            if removed {
                drop(fields);
                return Ok(self.get_ttl(session_id));
            }

            drop(fields);
            return Ok(self.get_ttl(session_id));
        }

        Ok(-2)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        self.cleanup_expired();
        Ok(self.data.remove(session_id.as_str()).is_some())
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        if seconds == 0 {
            return self.delete(session_id).await;
        }

        if let Some(mut fields) = self.data.get_mut(session_id.as_str()) {
            let new_expiry =
                (seconds > 0).then(|| Instant::now() + Duration::from_secs(seconds as u64));

            for value in fields.values_mut() {
                value.expires_at = match (new_expiry, value.expires_at) {
                    (None, _) => None,
                    (Some(_), None) => value.expires_at,
                    (Some(new), Some(cur)) if cur > new => Some(new),
                    _ => value.expires_at,
                };
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
            .set(&session_id, "user", &user, 30, 30, None)
            .await
            .unwrap();
        assert!(ttl <= 30 && ttl > 28);

        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert_eq!(retrieved.unwrap(), user);

        let updated_user = TestUser {
            id: 1,
            name: "Updated User".to_string(),
        };

        let ttl = store
            .set(&session_id, "user", &updated_user, 60, 60, None)
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
            .set(&session_id, "user", &user, 2, 2, None)
            .await
            .unwrap();

        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_some());

        sleep(Duration::from_millis(2100)).await;

        let retrieved: Option<TestUser> = store.get(&session_id, "user").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_rename_preserves_data() {
        let store = MemoryStore::new();
        let old_id = Id::default();
        let new_id = Id::default();
        let user = TestUser {
            id: 1,
            name: "A".into(),
        };

        store.set(&old_id, "f1", &user, 60, 60, None).await.unwrap();

        store
            .set_and_rename(&old_id, &new_id, "f2", &user, 60, 60, None)
            .await
            .unwrap();

        assert!(
            store
                .get::<TestUser>(&old_id, "f1")
                .await
                .unwrap()
                .is_none()
        );
        assert!(
            store
                .get::<TestUser>(&new_id, "f1")
                .await
                .unwrap()
                .is_some()
        );
        assert!(
            store
                .get::<TestUser>(&new_id, "f2")
                .await
                .unwrap()
                .is_some()
        );
    }
}
