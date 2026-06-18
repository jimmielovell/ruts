use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use moka::future::Cache;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct StoredValue {
    data: Vec<u8>,
    expires_at: Option<Instant>,
}

/// A highly concurrent, thread-safe in-moka session store backed by `moka`.
///
/// Ideal for production use cases where a local, fast cache is required,
/// or as a `HotStore` in a `LayeredStore` topology.
#[derive(Clone)]
pub struct MokaStore {
    data: Cache<String, Arc<RwLock<HashMap<String, StoredValue>>>>,
}

pub struct MokaStoreBuilder {
    max_capacity: u64,
    time_to_live: Option<Duration>,
    time_to_idle: Option<Duration>,
}

impl Default for MokaStoreBuilder {
    fn default() -> Self {
        Self::new()
    }
}

impl MokaStoreBuilder {
    pub fn new() -> Self {
        Self {
            max_capacity: 10_000,
            time_to_live: None,
            time_to_idle: None,
        }
    }

    /// Sets the maximum capacity of the cache.
    pub fn max_capacity(mut self, capacity: u64) -> Self {
        self.max_capacity = capacity;
        self
    }

    /// Sets the time to live for cache entries.
    pub fn time_to_live(mut self, ttl: Duration) -> Self {
        self.time_to_live = Some(ttl);
        self
    }

    /// Sets the time to idle for cache entries.
    pub fn time_to_idle(mut self, tti: Duration) -> Self {
        self.time_to_idle = Some(tti);
        self
    }

    pub fn build(self) -> MokaStore {
        let mut builder = Cache::builder().max_capacity(self.max_capacity);

        if let Some(ttl) = self.time_to_live {
            builder = builder.time_to_live(ttl);
        }
        if let Some(tti) = self.time_to_idle {
            builder = builder.time_to_idle(tti);
        }

        MokaStore {
            data: builder.build(),
        }
    }
}

impl MokaStore {
    pub fn builder() -> MokaStoreBuilder {
        MokaStoreBuilder::new()
    }

    async fn get_ttl(&self, session_id: &Id) -> i64 {
        if let Some(fields_lock) = self.data.get(session_id.as_str()).await {
            let fields = fields_lock.read().await;
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

impl SessionStore for MokaStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        if let Some(fields_lock) = self.data.get(session_id.as_str()).await {
            let fields = fields_lock.read().await;
            if let Some(value) = fields.get(field) {
                if value.expires_at.map(|e| e > Instant::now()).unwrap_or(true) {
                    return Ok(Some(deserialize_value(&value.data)?));
                }
            }
        }
        Ok(None)
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        if let Some(fields_lock) = self.data.get(session_id.as_str()).await {
            let fields = fields_lock.read().await;
            if fields.is_empty() {
                return Ok(None);
            }

            let now = Instant::now();
            let mut map = HashMap::new();

            for (k, v) in fields.iter() {
                if v.expires_at.map(|e| e > now).unwrap_or(true) {
                    map.insert(k.clone(), v.data.clone());
                }
            }

            if map.is_empty() {
                return Ok(None);
            }

            return Ok(Some(SessionMap::new(map)));
        }
        Ok(None)
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

        let expires_at = determine_expiry(key_ttl_secs, field_ttl_secs);
        let data_bytes = serialize_value(value)?;

        let fields_lock = self
            .data
            .get_with(session_id.to_string(), async {
                Arc::new(RwLock::new(HashMap::new()))
            })
            .await;

        let mut fields = fields_lock.write().await;

        let now = Instant::now();
        fields.retain(|_, v| v.expires_at.map(|e| e > now).unwrap_or(true));

        fields.insert(
            field.to_string(),
            StoredValue {
                data: data_bytes,
                expires_at,
            },
        );

        drop(fields);

        Ok(self.get_ttl(session_id).await)
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
        let old_key = old_session_id.as_str();
        let new_key = new_session_id.as_str();

        if old_key != new_key && self.data.contains_key(new_key) {
            return Err(Error::Backend(format!(
                "rename failed: target session {new_session_id} already exists"
            )));
        }

        if key_ttl_secs == 0 {
            self.data.invalidate(old_key).await;
            return Ok(-2);
        }

        let fields_lock = if let Some(lock) = self.data.get(old_key).await {
            self.data.invalidate(old_key).await;
            lock
        } else {
            Arc::new(RwLock::new(HashMap::new()))
        };

        let mut fields = fields_lock.write().await;

        let now = Instant::now();
        fields.retain(|_, v| v.expires_at.map(|e| e > now).unwrap_or(true));

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

        let is_empty = fields.is_empty();
        drop(fields);

        if is_empty {
            return Ok(-2);
        }

        self.data.insert(new_key.to_string(), fields_lock).await;
        Ok(self.get_ttl(new_session_id).await)
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let new_key = new_session_id.as_str();

        if self.data.contains_key(new_key) {
            return Ok(false);
        }

        let old_key = old_session_id.as_str();
        if let Some(fields_lock) = self.data.get(old_key).await {
            self.data.invalidate(old_key).await;
            self.data.insert(new_key.to_string(), fields_lock).await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        let session_id_str = session_id.as_str();

        if let Some(fields_lock) = self.data.get(session_id_str).await {
            let mut fields = fields_lock.write().await;

            let now = Instant::now();
            fields.retain(|_, v| v.expires_at.map(|e| e > now).unwrap_or(true));

            let removed = fields.remove(field).is_some();

            if fields.is_empty() {
                drop(fields);
                self.data.invalidate(session_id_str).await;
                return Ok(-2);
            }

            drop(fields);
            if removed {
                return Ok(self.get_ttl(session_id).await);
            }
            return Ok(self.get_ttl(session_id).await);
        }

        Ok(-2)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let session_id_str = session_id.as_str();
        if self.data.contains_key(session_id_str) {
            self.data.invalidate(session_id_str).await;
            Ok(true)
        } else {
            Ok(false)
        }
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        if seconds == 0 {
            return self.delete(session_id).await;
        }

        if let Some(fields_lock) = self.data.get(session_id.as_str()).await {
            let mut fields = fields_lock.write().await;
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

#[cfg(feature = "layered-store")]
impl crate::store::LayeredHotStore for MokaStore {
    async fn set_multiple(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Option<i64>)],
    ) -> Result<i64, Error> {
        let session_id_str = session_id.as_str();

        let fields_lock = self
            .data
            .get_with(session_id_str.to_string(), async {
                Arc::new(RwLock::new(HashMap::new()))
            })
            .await;

        let mut fields = fields_lock.write().await;

        let now = Instant::now();
        fields.retain(|_, v| v.expires_at.map(|e| e > now).unwrap_or(true));

        for (field, data, cache_ttl) in pairs {
            let expires_at =
                cache_ttl.and_then(|ttl| (ttl > 0).then(|| now + Duration::from_secs(ttl as u64)));

            fields.insert(
                field.to_string(),
                StoredValue {
                    data: data.to_vec(),
                    expires_at,
                },
            );
        }

        drop(fields);
        Ok(self.get_ttl(session_id).await)
    }
}
