use crate::store::{
    Error, LayeredCacheBehavior, LayeredCacheMeta, LayeredColdStore, LayeredHotStore, SessionMap,
    SessionStore,
};
use crate::Id;
use serde::{de::DeserializeOwned, Serialize, Serializer};

/// An enum for explicit control over the write strategy for a
/// specific `insert` or `update` operation.
///
/// It is passed as the `value` parameter to the session methods.
pub enum LayeredWriteStrategy<T: Send + Sync + Serialize + 'static> {
    /// Writes the value to both the hot cache and the cold persistent store.
    ///
    /// The `Option<i64>` allows you to specify a separate, often shorter,
    /// time-to-live (TTL) in seconds exclusively for the hot cache. If `None`,
    /// the hot cache will use the main expiration time.
    WriteThrough(T, Option<i64>),
    /// Writes the value *only* to the hot cache (e.g., Redis).
    /// This is useful for short-lived, ephemeral data that does not require
    /// long-term persistence.
    HotCache(T),
    /// Writes the value *only* to the cold persistent store (e.g., Postgres).
    /// This is useful for data that is not read frequently but must be saved.
    ColdCache(T),
}

impl<T: Send + Sync + Serialize + 'static> Serialize for LayeredWriteStrategy<T> {
    fn serialize<S>(&self, serializer: S) -> Result<S::Ok, S::Error>
    where
        S: Serializer,
    {
        match self {
            LayeredWriteStrategy::WriteThrough(inner, _) => inner.serialize(serializer),
            LayeredWriteStrategy::HotCache(inner) => inner.serialize(serializer),
            LayeredWriteStrategy::ColdCache(inner) => inner.serialize(serializer),
        }
    }
}

/// [`LayeredStore`], a composite store that layers a fast,
/// ephemeral "hot" cache (like Redis) on top of a slower, persistent "cold"
/// store (like Postgres). It is designed for scenarios where sessions can have
/// long lifespans but should only occupy expensive cache memory when actively
/// being used thus balancing performance and durability.
///
/// ## Core Strategies
///
/// - **Cache-Aside Reads**: On a read operation (`get`, `get_all`), the store
///   first checks the hot cache. If the data is present, it is
///   returned immediately. If not, the store queries the cold
///   store, and if the data is found, it "warms" the hot cache by populating it
///   with the data before returning it if during `insert`/`update` 
///   `LayeredWriteStrategy::WriteThrough`(default) was the strategy used.
///
/// - **Write-Through (Default)**: By default, write operations (`insert`, `update`)
///   are written to both the hot and cold stores simultaneously. This guarantees
///   data consistency.
///
/// ## Fine-Grained Write Control
///
/// The default write-through behavior can be overridden on a per-call basis
/// using the [`LayeredWriteStrategy`] enum. This gives you precise control over
/// where your session data is stored, which is especially useful for managing
/// cache resources.
///
/// The enum allows you to:
/// 1.  Write to the hot cache only.
/// 2.  Write to the cold store only.
/// 3.  Write through to both, but with a specific, shorter TTL for the hot cache.
///
/// ## Example
///
/// ```rust,no_run
/// use ruts::store::layered::LayeredWriteStrategy;
/// use ruts::Session;
/// # use ruts::store::redis::RedisStore;
/// # use ruts::store::postgres::PostgresStore;
/// # use ruts::store::layered::LayeredStore;
/// # type MySession = Session<LayeredStore<RedisStore, PostgresStore>>;
/// # #[derive(serde::Serialize)]
/// # struct User { id: i32 }
/// # async fn handler(session: MySession) {
/// # let user = User { id: 1 };
///
/// let long_term_expiry = 60  * 60 * 24 * 30; // valid for 1 month
///
/// // However, we only want it to live in the hot cache (Redis) for 1 hour.
/// let short_term_hot_cache_expiry = 60 * 60;
///
/// // The value is wrapped in the strategy enum.
/// let strategy = LayeredWriteStrategy::WriteThrough(
///     user,
///     Some(short_term_hot_cache_expiry),
/// );
///
/// // The cold store (Postgres) will get the long-term expiry,
/// // but the hot store (Redis) will be capped at the shorter TTL.
/// session.update("user", &strategy, Some(long_term_expiry)).await.unwrap();
/// # }
/// ```
#[derive(Clone, Debug)]
pub struct LayeredStore<Hot, Cold>
where
    Hot: SessionStore + LayeredHotStore,
    Cold: SessionStore,
{
    hot: Hot,
    cold: Cold,
}

impl<Hot, Cold> LayeredStore<Hot, Cold>
where
    Hot: SessionStore + LayeredHotStore,
    Cold: SessionStore + LayeredColdStore,
{
    /// Creates a new `LayeredStore`.
    ///
    /// # Arguments
    ///
    /// * `hot` - The fast cache store (e.g., `RedisStore`).
    /// * `cold` - The persistent source of truth (e.g., `PostgresStore`).
    pub fn new(hot: Hot, cold: Cold) -> Self {
        Self { hot, cold }
    }
}

impl<Hot, Cold> SessionStore for LayeredStore<Hot, Cold>
where
    Hot: SessionStore + LayeredHotStore,
    Cold: SessionStore + LayeredColdStore,
{
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        match self.hot.get(session_id, field).await? {
            Some(value) => Ok(Some(value)),
            None => match self.cold.get_all_with_meta(session_id).await? {
                Some((session_map, meta_map)) => {
                    let pairs_to_cache: Vec<(String, Vec<u8>, Option<i64>)> = session_map
                        .iter()
                        .filter_map(|entry| {
                            let meta = meta_map.get(entry.key());
                            let should_cache = meta
                                .is_none_or(|m| m.behavior != LayeredCacheBehavior::ColdCacheOnly);

                            if should_cache {
                                let hot_ttl = meta.and_then(|m| m.hot_cache_ttl);
                                Some((entry.key().to_owned(), entry.value().to_owned(), hot_ttl))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !pairs_to_cache.is_empty() {
                        self.hot.update_many(session_id, &pairs_to_cache).await?;
                    }

                    session_map.get(field)
                }
                None => Ok(None),
            },
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        match self.hot.get_all(session_id).await? {
            Some(session_map) => Ok(Some(session_map)),
            None => match self.cold.get_all_with_meta(session_id).await? {
                Some((session_map, meta_map)) => {
                    let pairs_to_cache: Vec<(String, Vec<u8>, Option<i64>)> = session_map
                        .iter()
                        .filter_map(|entry| {
                            let meta = meta_map.get(entry.key());
                            let should_cache = meta
                                .is_none_or(|m| m.behavior != LayeredCacheBehavior::ColdCacheOnly);

                            if should_cache {
                                let hot_ttl = meta.and_then(|m| m.hot_cache_ttl);
                                Some((entry.key().to_owned(), entry.value().to_owned(), hot_ttl))
                            } else {
                                None
                            }
                        })
                        .collect();

                    if !pairs_to_cache.is_empty() {
                        self.hot.update_many(session_id, &pairs_to_cache).await?;
                    }

                    Ok(Some(session_map))
                }
                None => Ok(None),
            },
        }
    }

    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = &None;
        let mut meta = LayeredCacheMeta {
            behavior: LayeredCacheBehavior::ColdCacheOnly,
            hot_cache_ttl: None,
        };

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            match strategy {
                LayeredWriteStrategy::HotCache(inner_value) => {
                    return self
                        .hot
                        .insert(session_id, field, inner_value, key_seconds, field_seconds)
                        .await;
                }
                LayeredWriteStrategy::ColdCache(inner_value) => {
                    let meta = LayeredCacheMeta {
                        behavior: LayeredCacheBehavior::ColdCacheOnly,
                        hot_cache_ttl: None,
                    };
                    return self
                        .cold
                        .insert_with_meta(
                            session_id,
                            field,
                            inner_value,
                            key_seconds,
                            field_seconds,
                            &meta,
                        )
                        .await;
                }
                LayeredWriteStrategy::WriteThrough(inner_value, secs) => {
                    meta.hot_cache_ttl = *secs;
                    value = inner_value;
                    hot_cache_ttl = secs;
                }
            };
        }

        let hot_cache_ttl = hot_cache_ttl
            .unwrap_or(key_seconds)
            .min(key_seconds)
            .min(field_seconds.unwrap_or(key_seconds));

        let (hot_result, cold_result) = tokio::try_join!(
            self.hot
                .insert(session_id, field, value, hot_cache_ttl, Some(hot_cache_ttl)),
            self.cold
                .insert_with_meta(session_id, field, value, key_seconds, field_seconds, &meta),
        )?;

        Ok(hot_result && cold_result)
    }

    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = &None;
        let mut meta = LayeredCacheMeta {
            behavior: LayeredCacheBehavior::ColdCacheOnly,
            hot_cache_ttl: None,
        };

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            match strategy {
                LayeredWriteStrategy::HotCache(inner_value) => {
                    return self
                        .hot
                        .update(session_id, field, inner_value, key_seconds, field_seconds)
                        .await;
                }
                LayeredWriteStrategy::ColdCache(inner_value) => {
                    let meta = LayeredCacheMeta {
                        behavior: LayeredCacheBehavior::ColdCacheOnly,
                        hot_cache_ttl: None,
                    };
                    return self
                        .cold
                        .update_with_meta(session_id, field, inner_value, key_seconds, field_seconds, &meta)
                        .await
                }
                LayeredWriteStrategy::WriteThrough(inner_value, secs) => {
                    meta.hot_cache_ttl = *secs;
                    value = inner_value;
                    hot_cache_ttl = secs;
                }
            };
        }

        let hot_cache_ttl = hot_cache_ttl
            .unwrap_or(key_seconds)
            .min(key_seconds)
            .min(field_seconds.unwrap_or(key_seconds));

        let (hot_result, cold_result) = tokio::try_join!(
            self.hot
                .update(session_id, field, value, hot_cache_ttl, Some(hot_cache_ttl)),
            self.cold
                .update_with_meta(session_id, field, value, key_seconds, field_seconds, &meta),
        )?;

        Ok(hot_result && cold_result)
    }

    async fn insert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = &None;
        let mut meta = LayeredCacheMeta {
            behavior: LayeredCacheBehavior::ColdCacheOnly,
            hot_cache_ttl: None,
        };

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            match strategy {
                LayeredWriteStrategy::HotCache(inner_value) => {
                    return self
                        .hot
                        .insert_with_rename(
                            old_session_id,
                            new_session_id,
                            field,
                            inner_value,
                            key_seconds,
                            field_seconds,
                        )
                        .await;
                }
                LayeredWriteStrategy::ColdCache(inner_value) => {
                    let meta = LayeredCacheMeta {
                        behavior: LayeredCacheBehavior::ColdCacheOnly,
                        hot_cache_ttl: None,
                    };
                    return self
                        .cold
                        .insert_with_rename_with_meta(
                            old_session_id,
                            new_session_id,
                            field,
                            inner_value,
                            key_seconds,
                            field_seconds,
                            &meta
                        )
                        .await
                }
                LayeredWriteStrategy::WriteThrough(inner_value, secs) => {
                    meta.hot_cache_ttl = *secs;
                    value = inner_value;
                    hot_cache_ttl = secs;
                }
            };
        }

        let hot_cache_ttl = hot_cache_ttl
            .unwrap_or(key_seconds)
            .min(key_seconds)
            .min(field_seconds.unwrap_or(key_seconds));

        let (hot_result, cold_result) = tokio::try_join!(
            self.hot.insert_with_rename(
                old_session_id,
                new_session_id,
                field,
                value,
                hot_cache_ttl,
                Some(hot_cache_ttl),
            ),
            self.cold.insert_with_rename_with_meta(
                old_session_id,
                new_session_id,
                field,
                value,
                key_seconds,
                field_seconds,
                &meta
            ),
        )?;

        Ok(hot_result && cold_result)
    }

    async fn update_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let mut value = value;
        let mut hot_cache_ttl = &None;
        let mut meta = LayeredCacheMeta {
            behavior: LayeredCacheBehavior::ColdCacheOnly,
            hot_cache_ttl: None,
        };

        if let Some(strategy) =
            (value as &dyn std::any::Any).downcast_ref::<LayeredWriteStrategy<T>>()
        {
            match strategy {
                LayeredWriteStrategy::HotCache(inner_value) => {
                    return self
                        .hot
                        .update_with_rename(
                            old_session_id,
                            new_session_id,
                            field,
                            inner_value,
                            key_seconds,
                            field_seconds,
                        )
                        .await;
                }
                LayeredWriteStrategy::ColdCache(inner_value) => {
                    let meta = LayeredCacheMeta {
                        behavior: LayeredCacheBehavior::ColdCacheOnly,
                        hot_cache_ttl: None,
                    };
                    return self
                        .cold
                        .update_with_rename_with_meta(
                            old_session_id,
                            new_session_id,
                            field,
                            inner_value,
                            key_seconds,
                            field_seconds,
                            &meta
                        )
                        .await
                }
                LayeredWriteStrategy::WriteThrough(inner_value, secs) => {
                    meta.hot_cache_ttl = *secs;
                    value = inner_value;
                    hot_cache_ttl = secs;
                }
            };
        }

        let hot_cache_ttl = hot_cache_ttl
            .unwrap_or(key_seconds)
            .min(key_seconds)
            .min(field_seconds.unwrap_or(key_seconds));

        let (hot_result, cold_result) = tokio::try_join!(
            self.hot.update_with_rename(
                old_session_id,
                new_session_id,
                field,
                value,
                hot_cache_ttl,
                Some(hot_cache_ttl),
            ),
            self.cold.update_with_rename_with_meta(
                old_session_id,
                new_session_id,
                field,
                value,
                key_seconds,
                field_seconds,
                &meta
            ),
        )?;

        Ok(hot_result && cold_result)
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> Result<bool, Error> {
        let (hot_result, cold_result) = tokio::try_join!(
            self.hot
                .rename_session_id(old_session_id, new_session_id, seconds),
            self.cold
                .rename_session_id(old_session_id, new_session_id, seconds),
        )?;
        Ok(hot_result && cold_result)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error> {
        let (hot_result, cold_result) = tokio::try_join!(
            self.hot.remove(session_id, field),
            self.cold.remove(session_id, field),
        )?;

        if hot_result == 0 || cold_result == 0 {
            return Ok(0);
        }

        Ok(1)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let (hot_deleted, cold_deleted) =
            tokio::try_join!(self.hot.delete(session_id), self.cold.delete(session_id),)?;
        Ok(hot_deleted && cold_deleted)
    }

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        let (hot_expired, cold_expired) = tokio::try_join!(
            self.hot.expire(session_id, seconds),
            self.cold.expire(session_id, seconds),
        )?;
        Ok(hot_expired && cold_expired)
    }
}
