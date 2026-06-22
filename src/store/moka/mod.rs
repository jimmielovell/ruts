use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, Ttl, deserialize_value, serialize_value};
use moka::future::Cache;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::RwLock;

#[derive(Debug, Clone)]
struct StoredValue {
    data: Vec<u8>,
    expires_at: Instant,
}

/// A highly concurrent, thread-safe in-memory session store backed by `moka`.
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
}

impl SessionStore for MokaStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        if let Some(fields_lock) = self.data.get(session_id.as_str()).await {
            let fields = fields_lock.read().await;
            if let Some(value) = fields.get(field) {
                if value.expires_at > Instant::now() {
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
                if v.expires_at > now {
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
        field_ttl_secs: Ttl,
        #[cfg(feature = "layered-store")] _: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        let data_bytes = serialize_value(value)?;
        let fields_lock = self
            .data
            .get_with(session_id.to_string(), async {
                Arc::new(RwLock::new(HashMap::new()))
            })
            .await;

        let mut fields = fields_lock.write().await;
        let now = Instant::now();

        fields.retain(|_, v| v.expires_at > now);

        fields.insert(
            field.to_string(),
            StoredValue {
                data: data_bytes,
                expires_at: now + Duration::from_secs(field_ttl_secs.into()),
            },
        );

        Ok(())
    }

    async fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] _: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
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

        let fields_lock = if let Some(lock) = self.data.get(old_key).await {
            self.data.invalidate(old_key).await;
            lock
        } else {
            Arc::new(RwLock::new(HashMap::new()))
        };

        let mut fields = fields_lock.write().await;
        let now = Instant::now();

        fields.retain(|_, v| v.expires_at > now);

        fields.insert(
            field.to_string(),
            StoredValue {
                data: serialize_value(value)?,
                expires_at: now + Duration::from_secs(field_ttl.into()),
            },
        );

        let is_empty = fields.is_empty();
        drop(fields);

        if !is_empty {
            self.data.insert(new_key.to_string(), fields_lock).await;
        }

        Ok(())
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

    async fn remove(&self, session_id: &Id, field: &str) -> Result<(), Error> {
        let session_id_str = session_id.as_str();

        if let Some(fields_lock) = self.data.get(session_id_str).await {
            let mut fields = fields_lock.write().await;
            let now = Instant::now();

            fields.retain(|_, v| v.expires_at > now);
            fields.remove(field);

            if fields.is_empty() {
                drop(fields);
                self.data.invalidate(session_id_str).await;
            }
        }

        Ok(())
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

    async fn expire_field(&self, session_id: &Id, field: &str, ttl: Ttl) -> Result<bool, Error> {
        if let Some(fields_lock) = self.data.get(session_id.as_str()).await {
            let mut fields = fields_lock.write().await;
            let now = Instant::now();

            if let Some(value) = fields.get_mut(field) {
                if value.expires_at > now {
                    value.expires_at = now + Duration::from_secs(ttl.into());
                    return Ok(true);
                }
            }
        }

        Ok(false)
    }
}

#[cfg(feature = "layered-store")]
impl crate::store::LayeredHotStore for MokaStore {
    async fn set_multiple(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Ttl)],
    ) -> Result<(), Error> {
        let session_id_str = session_id.as_str();

        let fields_lock = self
            .data
            .get_with(session_id_str.to_string(), async {
                Arc::new(RwLock::new(HashMap::new()))
            })
            .await;

        let mut fields = fields_lock.write().await;
        let now = Instant::now();

        fields.retain(|_, v| v.expires_at > now);

        for (field, data, field_ttl) in pairs {
            fields.insert(
                field.to_string(),
                StoredValue {
                    data: data.to_vec(),
                    expires_at: now + Duration::from_secs((*field_ttl).into()),
                },
            );
        }

        Ok(())
    }
}
