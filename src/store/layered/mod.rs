use crate::Id;
use crate::store::{Error, LayeredColdStore, LayeredHotStore, SessionMap, SessionStore, Ttl};
use serde::{Serialize, de::DeserializeOwned};

/// [`LayeredStore`], a composite store that layers a fast,
/// ephemeral "hot" cache (like Redis) on top of a slower, persistent "cold"
/// store (like Postgres or Scylla). It is designed for scenarios where sessions can have
/// long lifespans but should only occupy expensive cache when actively
/// being used thus balancing performance and durability.
///
/// ## Example
///
/// ```rust,no_run
/// # #[cfg(all(feature = "layered-store", feature = "redis-store", feature = "postgres-store"))]
/// # mod docs {
/// # use ruts::Session;
/// # use ruts::store::redis::RedisStore;
/// # use ruts::store::postgres::PostgresStore;
/// # use ruts::store::layered::LayeredStore;
/// # use ruts::store::Ttl;
/// # type MySession = Session<LayeredStore<RedisStore, PostgresStore>>;
/// # #[derive(serde::Serialize)]
/// # struct User { id: i32 }
/// # async fn handler(session: MySession) {
/// # let user = User { id: 1 };
///
/// let long_term_expiry = Ttl::new(60 * 60 * 24 * 30).unwrap(); // valid for 1 month
///
/// // However, we only want it to live in the hot cache (Redis) for 1 hour.
/// let short_term_hot_cache_expiry = Ttl::new(60 * 60).unwrap();
///
/// // The cold store (Postgres) will get the long-term expiry,
/// // but the hot store (Redis) will be capped at the shorter TTL.
/// session.set("user", &user, long_term_expiry, Some(short_term_hot_cache_expiry))
///     .await
///     .unwrap();
/// # }
/// # }
/// # fn main() {}
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
    /// * `cold` - The persistent source of truth (e.g., `PostgresStore` or `ScyllaStore`).
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
        T: Send + Sync + DeserializeOwned,
    {
        match self.hot.get(session_id, field).await? {
            Some(value) => Ok(Some(value)),
            None => match self.cold.get_all_with_meta(session_id).await? {
                Some((session_map, hot_cache_ttl_map)) => {
                    let pairs_to_cache: Vec<(&str, &[u8], Ttl)> = session_map
                        .iter()
                        .map(|(key, value)| {
                            let hot_cache_ttl = hot_cache_ttl_map.get(key).unwrap().to_owned();
                            (key.as_str(), value.as_slice(), hot_cache_ttl)
                        })
                        .collect();

                    if !pairs_to_cache.is_empty() {
                        self.hot.set_multiple(session_id, &pairs_to_cache).await?;
                    }

                    session_map.get(field)
                }
                None => Ok(None),
            },
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        match self.cold.get_all_with_meta(session_id).await? {
            Some((session_map, hot_cache_ttl_map)) => {
                let pairs_to_cache: Vec<(&str, &[u8], Ttl)> = session_map
                    .iter()
                    .map(|(key, value)| {
                        let hot_cache_ttl = hot_cache_ttl_map.get(key).unwrap().to_owned();
                        (key.as_str(), value.as_slice(), hot_cache_ttl)
                    })
                    .collect();

                if !pairs_to_cache.is_empty() {
                    self.hot.set_multiple(session_id, &pairs_to_cache).await?;
                }

                Ok(Some(session_map))
            }
            None => Ok(None),
        }
    }

    async fn set<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        let hot_cache_ttl = hot_cache_ttl.unwrap_or(field_ttl);

        tokio::try_join!(
            self.hot
                .set(session_id, field, value, hot_cache_ttl, Some(hot_cache_ttl)),
            self.cold
                .set_with_meta(session_id, field, value, field_ttl, Some(hot_cache_ttl)),
        )?;

        Ok(())
    }

    async fn set_and_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        let hot_cache_ttl = hot_cache_ttl.unwrap_or(field_ttl);
        tokio::try_join!(
            self.hot.set_and_rename(
                old_session_id,
                new_session_id,
                field,
                value,
                hot_cache_ttl,
                Some(hot_cache_ttl)
            ),
            self.cold.set_and_rename_with_meta(
                old_session_id,
                new_session_id,
                field,
                value,
                field_ttl,
                Some(hot_cache_ttl)
            ),
        )?;

        Ok(())
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let (hot_result, cold_result) = tokio::try_join!(
            self.hot.rename_session_id(old_session_id, new_session_id),
            self.cold.rename_session_id(old_session_id, new_session_id),
        )?;
        Ok(hot_result && cold_result)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<(), Error> {
        tokio::try_join!(
            self.hot.remove(session_id, field),
            self.cold.remove(session_id, field),
        )?;

        Ok(())
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let (hot_deleted, cold_deleted) =
            tokio::try_join!(self.hot.delete(session_id), self.cold.delete(session_id),)?;

        Ok(hot_deleted || cold_deleted)
    }

    async fn expire_field(&self, session_id: &Id, field: &str, ttl: Ttl) -> Result<bool, Error> {
        let (hot_expired, cold_expired) = tokio::try_join!(
            self.hot.expire_field(session_id, field, ttl),
            self.cold.expire_field(session_id, field, ttl),
        )?;

        Ok(hot_expired || cold_expired)
    }
}
