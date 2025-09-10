use std::collections::HashMap;
use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use cookie::time::OffsetDateTime;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Executor, PgPool, Postgres};

// Re-export Duration
pub use tokio::time::Duration;

/// A builder for creating a `PostgresStore`.
///
/// This allows for customizing the table and schema names for session storage.
#[derive(Debug)]
pub struct PostgresStoreBuilder {
    pool: PgPool,
    table_name: String,
    schema_name: Option<String>,
    cleanup_interval: Option<Duration>,
}

impl PostgresStoreBuilder {
    /// Creates a new builder with a database pool and default settings.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            table_name: "sessions".to_string(),
            schema_name: None,
            cleanup_interval: None,
        }
    }

    /// Sets a custom table name for the session store. Defaults to "sessions".
    pub fn table_name(mut self, table_name: impl Into<String>) -> Self {
        self.table_name = table_name.into();
        self
    }

    /// Sets a custom schema name for the session store.
    pub fn schema_name(mut self, schema_name: impl Into<String>) -> Self {
        self.schema_name = Some(schema_name.into());
        self
    }

    /// Sets the interval for the background task that cleans up expired sessions.
    ///
    /// If this is not set, the cleanup task defaults to running every 5 minutes.
    pub fn cleanup_interval(mut self, interval: Duration) -> Self {
        self.cleanup_interval = Some(interval);
        self
    }

    /// Builds the `PostgresStore`, creating the schema and table if they don't exist.
    pub async fn build(self) -> Result<PostgresStore, sqlx::Error> {
        let qualified_table_name = if let Some(schema) = &self.schema_name {
            // Quoted to handle special characters.
            sqlx::query(&format!("create schema if not exists \"{}\"", schema))
                .execute(&self.pool)
                .await?;
            format!("\"{}\".\"{}\"", schema, self.table_name)
        } else {
            format!("\"{}\"", self.table_name)
        };

        let create_table_query = format!(
            "create table if not exists {} (
                session_id text not null,
                field text not null,
                value bytea not null,
                expires_at timestamptz,
                cache_behavior smallint,
                hot_cache_ttl integer,
                primary key (session_id, field)
            );",
            qualified_table_name
        );
        sqlx::query(&create_table_query).execute(&self.pool).await?;

        let pool_clone = self.pool.clone();
        let delete_query = format!(
            "delete from {} where expires_at is not null and expires_at < now()",
            qualified_table_name
        );

        let cleanup_interval = self
            .cleanup_interval
            .unwrap_or_else(|| Duration::from_secs(300));

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                interval.tick().await;
                if let Err(err) = sqlx::query(&delete_query).execute(&pool_clone).await {
                    tracing::error!("Failed to clean up expired sessions: {err:?}");
                }
            }
        });

        Ok(PostgresStore {
            pool: self.pool,
            qualified_table_name,
        })
    }
}

/// A Postgres-backed session store.
#[derive(Clone, Debug)]
pub struct PostgresStore {
    pool: PgPool,
    qualified_table_name: String,
}

fn expires_at(key_ttl_secs: Option<i64>, field_ttl_secs: Option<i64>) -> Option<OffsetDateTime> {
    let ttl = match (key_ttl_secs, field_ttl_secs) {
        (Some(_), Some(fts)) | (None, Some(fts)) => Some(fts),
        (Some(kts), None) => Some(kts),
        (None, None) => None
    };

    if let Some(ttl) = ttl && ttl > 0 {
        Some(OffsetDateTime::now_utc() + cookie::time::Duration::seconds(ttl))
    } else {
        None
    }
}

impl PostgresStore {
    async fn _remove<'e, E>(&self, executor: E, session_id: &Id, field: &str) -> Result<i8, Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let query = format!(
            "delete from {} where session_id = $1 and field = $2",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.to_string())
            .bind(field)
            .execute(executor)
            .await?;

        Ok(result.rows_affected() as i8)
    }

    async fn _rename_session_id<'e, E>(
        &self,
        executor: E,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let query = format!(
            "update {} set session_id = $1 where session_id = $2",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(new_session_id.to_string())
            .bind(old_session_id.to_string())
            .execute(executor)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn _get_expiry<'e, E>(&self, executor: E, session_id: &Id) -> Result<Option<i64>, Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let query = format!(
            "select max(expires_at) from {} where session_id = $1",
            self.qualified_table_name
        );
        let result: Option<Option<OffsetDateTime>> = sqlx::query_scalar(&query)
            .bind(session_id.to_string())
            .fetch_one(executor)
            .await?;

        if let Some(Some(expires_at)) = result {
            let duration = expires_at - OffsetDateTime::now_utc();
            return Ok(Some(duration.whole_seconds()));
        }

        Ok(None)
    }

    async fn _upsert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")]
        meta: Option<crate::store::LayeredCacheMeta>,
        #[cfg(not(feature = "layered-store"))]
        meta: Option<std::marker::PhantomData<T>>,
        query: String,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        if let Some(ttl) = key_ttl_secs && ttl <= 0 {
            self.delete(session_id).await?;
            return Ok(None);
        }

        if let Some(ttl) = field_ttl_secs && ttl <= 0 {
            self.remove(session_id, field).await?;
            return Ok(None);
        }

        let value = serialize_value(value)?;
        let expires = expires_at(key_ttl_secs, field_ttl_secs);

        let query = format!(r"
            with upsert as ({query} returning session_id)
            select
              case
                when count(*) filter (where expires_at is null) > 0 then null
                else greatest(ceil(extract(epoch from max(expires_at) - now())), 0)::bigint
              end as ttl
            from {}
            where session_id = (select session_id from upsert)
        ", self.qualified_table_name);
        let mut builder = sqlx::query_scalar(&query)
            .bind(session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires);

        #[cfg(feature = "layered-store")]
        if let Some(meta) = meta {
            builder = builder.bind(meta.behavior as i16).bind(meta.hot_cache_ttl);
        }

        let new_max_age: Option<i64> = builder
            .fetch_optional(&self.pool)
            .await?;

        Ok(new_max_age)
    }

    async fn _upsert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")]
        meta: Option<crate::store::LayeredCacheMeta>,
        #[cfg(not(feature = "layered-store"))]
        meta: Option<std::marker::PhantomData<T>>,
        query: String,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        if let Some(ttl) = key_ttl_secs && ttl <= 0 {
            self.delete(old_session_id).await?;
            return Ok(None);
        }
        
        let mut tx = self.pool.begin().await?;

        if let Some(ttl) = field_ttl_secs && ttl <= 0 {
            self._remove(&mut *tx, old_session_id, field).await?;
            self._rename_session_id(&mut *tx, old_session_id, new_session_id)
                .await?;
            tx.commit().await?;

            return Ok(None);
        }

        self._rename_session_id(&mut *tx, old_session_id, new_session_id)
            .await?;

        let value = serialize_value(value)?;
        let expires = expires_at(key_ttl_secs, field_ttl_secs);
        
        let query = format!(r"
            with upsert as ({query} returning session_id)
            select
              case
                when count(*) filter (where expires_at is null) > 0 then null
                else greatest(ceil(extract(epoch from max(expires_at) - now())), 0)::bigint
              end as ttl
            from {}
            where session_id = (select session_id from upsert)
        ", self.qualified_table_name);
        let mut builder = sqlx::query_scalar(&query)
            .bind(new_session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires);

        #[cfg(feature = "layered-store")]
        if let Some(meta) = meta {
            builder = builder.bind(meta.behavior as i16).bind(meta.hot_cache_ttl);
        }

        let new_max_age: Option<i64> = builder
            .fetch_optional(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(new_max_age)
    }
}

impl SessionStore for PostgresStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let query = format!(
            "select value, expires_at from {} where session_id = $1 and field = $2",
            self.qualified_table_name
        );
        let result: Option<(Vec<u8>, Option<OffsetDateTime>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .bind(field)
            .fetch_optional(&self.pool)
            .await?;

        match result {
            Some((value, expires_at)) => {
                if expires_at.is_some_and(|err| err < OffsetDateTime::now_utc()) {
                    return Ok(None);
                }
                Ok(Some(deserialize_value(&value)?))
            }
            None => {
                Ok(None)
            }
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        let query = format!(
            "select field, value, expires_at from {} where session_id = $1",
            self.qualified_table_name
        );
        let rows: Vec<(String, Vec<u8>, Option<OffsetDateTime>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .fetch_all(&self.pool)
            .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut map = HashMap::with_capacity(rows.len());
        for (field, value, expires_at) in rows {
            if !expires_at.is_some_and(|exp| exp < OffsetDateTime::now_utc()) {
                map.insert(field, value);
            }
        }

        if map.is_empty() {
            return Ok(None);
        }

        Ok(Some(SessionMap::new(map)))
    }

    async fn insert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict do nothing
            ",
            self.qualified_table_name,
        );
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            None,
            query,
        )
        .await
    }

    async fn update<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict (session_id, field)
            do update set value = excluded.value, expires_at = excluded.expires_at
            ",
            self.qualified_table_name,
        );
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            None,
            query,
        )
        .await
    }

    async fn insert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict do nothing
            ",
            self.qualified_table_name,
        );
        self._upsert_with_rename(
            old_session_id,
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            None,
            query,
        )
        .await
    }

    async fn update_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
    ) -> Result<Option<i64>, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict (session_id, field)
            do update set value = excluded.value, expires_at = excluded.expires_at
            ",
            self.qualified_table_name,
        );
        self._upsert_with_rename(
            old_session_id,
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            None,
            query,
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let result = self
            ._rename_session_id(&self.pool, old_session_id, new_session_id)
            .await?;
        Ok(result)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i8, Error> {
        self._remove(&self.pool, session_id, field).await
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let query = format!(
            "delete from {} where session_id = $1",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn expire(&self, session_id: &Id, key_ttl_secs: i64) -> Result<bool, Error> {
        if key_ttl_secs <= 0 {
            return self.delete(session_id).await;
        }

        let expires = expires_at(Some(key_ttl_secs), None);
        let query = format!(
            "update {} set expires_at = $1 where session_id = $2",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(expires)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }
}

#[cfg(feature = "layered-store")]
impl crate::store::LayeredColdStore for PostgresStore {
    async fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> Result<
        Option<(
            SessionMap,
            std::collections::HashMap<String, crate::store::LayeredCacheMeta>,
        )>,
        Error,
    > {
        let query = format!(
            "select field, value, expires_at, cache_behavior, hot_cache_ttl from {} where session_id = $1",
            self.qualified_table_name
        );
        let rows: Vec<(String, Vec<u8>, Option<OffsetDateTime>, i16, Option<i64>)> =
            sqlx::query_as(&query)
                .bind(session_id.to_string())
                .fetch_all(&self.pool)
                .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut session_map = HashMap::with_capacity(rows.len());
        let mut meta_map = std::collections::HashMap::new();
        for (field, value, expires_at, cache_behaviour, hot_cache_ttl) in rows {
            if !expires_at.is_some_and(|err| err < OffsetDateTime::now_utc()) {
                session_map.insert(field.clone(), value);
                meta_map.insert(
                    field,
                    crate::store::LayeredCacheMeta {
                        hot_cache_ttl,
                        behavior: cache_behaviour.into(),
                    },
                );
            }
        }

        if session_map.is_empty() {
            return Ok(None);
        }

        Ok(Some((SessionMap::new(session_map), meta_map)))
    }

    async fn insert_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>, // Not used in Postgres
        meta: crate::store::LayeredCacheMeta,
    ) -> Result<Option<i64>, Error> {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at, cache_behavior, hot_cache_ttl)
            values ($1, $2, $3, $4, $5, $6)
            on conflict do nothing
            ",
            self.qualified_table_name,
        );
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            Some(meta),
            query,
        )
        .await
    }

    async fn update_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: crate::store::LayeredCacheMeta,
    ) -> Result<Option<i64>, Error> {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at, cache_behavior, hot_cache_ttl)
            values ($1, $2, $3, $4, $5, $6)
            on conflict (session_id, field)
            do update set
                value = excluded.value,
                expires_at = excluded.expires_at,
                cache_behavior = excluded.cache_behavior,
                hot_cache_ttl = excluded.hot_cache_ttl
            ",
            self.qualified_table_name,
        );
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            Some(meta),
            query,
        )
        .await
    }

    async fn insert_with_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: crate::store::LayeredCacheMeta,
    ) -> Result<Option<i64>, Error> {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at, cache_behavior, hot_cache_ttl)
            values ($1, $2, $3, $4, $5, $6)
            on conflict do nothing
            returning session_id
            ",
            self.qualified_table_name,
        );
        self._upsert_with_rename(
            old_session_id,
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            Some(meta),
            query,
        )
        .await
    }

    async fn update_with_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: crate::store::LayeredCacheMeta,
    ) -> Result<Option<i64>, Error> {
        let query = format!(
            r"
            insert into {} (session_id, field, value, expires_at, cache_behavior, hot_cache_ttl)
            values ($1, $2, $3, $4, $5, $6)
            on conflict (session_id, field)
            do update set
                value = excluded.value,
                expires_at = excluded.expires_at,
                cache_behavior = excluded.cache_behavior,
                hot_cache_ttl = excluded.hot_cache_ttl
            ",
            self.qualified_table_name,
        );
        self._upsert_with_rename(
            old_session_id,
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            Some(meta),
            query,
        )
        .await
    }
}
