use crate::store::{deserialize_value, serialize_value, Error, SessionMap, SessionStore};
use crate::Id;
use dashmap::DashMap;
use serde::{de::DeserializeOwned, Serialize};
use sqlx::{Executor, PgPool, Postgres};
use time::{Duration, OffsetDateTime};

/// A builder for creating a `PostgresStore`.
///
/// This allows for customizing the table and schema names for session storage.
#[derive(Debug)]
pub struct PostgresStoreBuilder {
    pool: PgPool,
    table_name: String,
    schema_name: Option<String>,
}

impl PostgresStoreBuilder {
    /// Creates a new builder with a database pool and default settings.
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            table_name: "sessions".to_string(),
            schema_name: None,
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

    /// Builds the `PostgresStore`, creating the schema and table if they don't exist.
    pub async fn build(self) -> Result<PostgresStore, sqlx::Error> {
        let qualified_table_name = if let Some(schema) = &self.schema_name {
            // Create schema if it doesn't exist. Quoted to handle special characters.
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
                hot_cache_ttl integer,
                cache_behavior smallint not null default 0,
                primary key (session_id, field)
            );",
            qualified_table_name
        );
        sqlx::query(&create_table_query).execute(&self.pool).await?;

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

/// Helper to calculate the expiration time.
fn expires_at(seconds: i64) -> Option<OffsetDateTime> {
    if seconds > 0 {
        Some(OffsetDateTime::now_utc() + Duration::seconds(seconds))
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
        seconds: i64,
    ) -> Result<bool, Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let expires = expires_at(seconds);
        let query = format!(
            "update {} set session_id = $1, expires_at = $2 where session_id = $3",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(new_session_id.to_string())
            .bind(expires)
            .bind(old_session_id.to_string())
            .execute(executor)
            .await?;
        Ok(result.rows_affected() > 0)
    }
}

impl SessionStore for PostgresStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let mut tx = self.pool.begin().await?;
        let query = format!(
            "select value, expires_at from {} where session_id = $1 and field = $2",
            self.qualified_table_name
        );
        let result: Option<(Vec<u8>, Option<OffsetDateTime>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .bind(field)
            .fetch_optional(&mut *tx)
            .await?;

        match result {
            Some((value, expires_at)) => {
                if expires_at.is_some_and(|err| err < OffsetDateTime::now_utc()) {
                    self._remove(&mut *tx, session_id, field).await?;
                    tx.commit().await?;
                    return Ok(None);
                }
                tx.commit().await?;
                Ok(Some(deserialize_value(&value)?))
            }
            None => {
                tx.commit().await?;
                Ok(None)
            }
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        let map = DashMap::new();

        let mut tx = self.pool.begin().await?;
        let query = format!(
            "select field, value, expires_at from {} where session_id = $1",
            self.qualified_table_name
        );
        let rows: Vec<(String, Vec<u8>, Option<OffsetDateTime>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .fetch_all(&mut *tx)
            .await?;

        if rows.is_empty() {
            tx.commit().await?;
            return Ok(None);
        }

        let mut expired_fields = Vec::new();
        for (field, value, expires_at) in rows {
            if expires_at.is_some_and(|err| err < OffsetDateTime::now_utc()) {
                expired_fields.push(field);
            } else {
                map.insert(field, value);
            }
        }

        if !expired_fields.is_empty() {
            let delete_query = format!(
                "delete from {} where session_id = $1 and field = any($2)",
                self.qualified_table_name
            );
            sqlx::query(&delete_query)
                .bind(session_id.to_string())
                .bind(expired_fields)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

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
        key_seconds: i64,
        _field_seconds: Option<i64>,
    ) -> Result<bool, Error>
    where
        T: Send + Sync + Serialize,
    {
        if key_seconds <= 0 {
            self.remove(session_id, field).await?;
            return Ok(false);
        }
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let query = format!(
            "insert into {} (session_id, field, value, expires_at) values ($1, $2, $3, $4) on conflict do nothing",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
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
        if key_seconds <= 0 {
            let removed = self.remove(session_id, field).await?;
            return Ok(removed > 0);
        }
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let query = format!(
            "insert into {} (session_id, field, value, expires_at) values ($1, $2, $3, $4) on conflict (session_id, field) do update set value = excluded.value, expires_at = excluded.expires_at",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires)
            .execute(&self.pool)
            .await?;
        Ok(result.rows_affected() > 0)
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
        let mut tx = self.pool.begin().await?;
        if key_seconds <= 0 {
            self._remove(&mut *tx, old_session_id, field).await?;
            self._rename_session_id(&mut *tx, old_session_id, new_session_id, 0)
                .await?;
            tx.commit().await?;
            return Ok(false);
        }
        self._rename_session_id(&mut *tx, old_session_id, new_session_id, key_seconds)
            .await?;
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let query = format!(
            "insert into {} (session_id, field, value, expires_at) values ($1, $2, $3, $4) on conflict do nothing",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(new_session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
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
        let mut tx = self.pool.begin().await?;
        if key_seconds <= 0 {
            self._remove(&mut *tx, old_session_id, field).await?;
            self._rename_session_id(&mut *tx, old_session_id, new_session_id, 0)
                .await?;
            tx.commit().await?;
            return Ok(true);
        }
        self._rename_session_id(&mut *tx, old_session_id, new_session_id, key_seconds)
            .await?;
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let query = format!(
            "insert into {} (session_id, field, value, expires_at) values ($1, $2, $3, $4) on conflict (session_id, field) do update set value = excluded.value, expires_at = excluded.expires_at",
            self.qualified_table_name
        );
        let result = sqlx::query(&query)
            .bind(new_session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires)
            .execute(&mut *tx)
            .await?;
        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        seconds: i64,
    ) -> Result<bool, Error> {
        let mut tx = self.pool.begin().await?;
        let exists_query = format!(
            "select exists(select 1 from {} where session_id = $1)",
            self.qualified_table_name
        );
        let exists: (bool,) = sqlx::query_as(&exists_query)
            .bind(new_session_id.to_string())
            .fetch_one(&mut *tx)
            .await?;
        if exists.0 {
            return Ok(false);
        }
        let result = self
            ._rename_session_id(&mut *tx, old_session_id, new_session_id, seconds)
            .await?;
        tx.commit().await?;
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

    async fn expire(&self, session_id: &Id, seconds: i64) -> Result<bool, Error> {
        if seconds <= 0 {
            return self.delete(session_id).await;
        }
        let expires = expires_at(seconds);
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
    ) -> Result<Option<(SessionMap, std::collections::HashMap<String, crate::store::LayeredCacheMeta>)>, Error> {
        let session_map = DashMap::new();
        let mut meta_map = std::collections::HashMap::new();

        let mut tx = self.pool.begin().await?;
        let query = format!(
            "select field, value, expires_at, cache_behavior, hot_cache_ttl from {} where session_id = $1",
            self.qualified_table_name
        );
        let rows: Vec<(String, Vec<u8>, Option<OffsetDateTime>, i16, Option<i64>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .fetch_all(&mut *tx)
            .await?;

        if rows.is_empty() {
            tx.commit().await?;
            return Ok(None);
        }

        let mut expired_fields = Vec::new();
        for (field, value, expires_at, cache_behaviour, hot_cache_ttl) in rows {
            if expires_at.is_some_and(|err| err < OffsetDateTime::now_utc()) {
                expired_fields.push(field);
            } else {
                session_map.insert(field.clone(), value);
                meta_map.insert(field, crate::store::LayeredCacheMeta {
                    hot_cache_ttl,
                    behavior: cache_behaviour.into(),
                });
            }
        }

        if !expired_fields.is_empty() {
            let delete_query = format!(
                "delete from {} where session_id = $1 and field = any($2)",
                self.qualified_table_name
            );
            sqlx::query(&delete_query)
                .bind(session_id.to_string())
                .bind(expired_fields)
                .execute(&mut *tx)
                .await?;
        }
        tx.commit().await?;

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
        key_seconds: i64,
        _field_seconds: Option<i64>, // Not used in Postgres
        meta: &crate::store::LayeredCacheMeta,
    ) -> Result<bool, Error> {
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let hot_ttl: Option<i32> = meta.hot_cache_ttl.and_then(|t| t.try_into().ok());

        let result = sqlx::query(&format!(
            "insert into {} (session_id, field, value, expires_at, hot_cache_ttl, cache_behavior)
             values ($1, $2, $3, $4, $5, $6) on conflict do nothing",
            self.qualified_table_name
        ))
        .bind(session_id.to_string())
        .bind(field)
        .bind(value)
        .bind(expires)
        .bind(hot_ttl)
        .bind(meta.behavior as i16)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn update_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
        meta: &crate::store::LayeredCacheMeta,
    ) -> Result<bool, Error> {
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let hot_ttl: Option<i32> = meta.hot_cache_ttl.and_then(|t| t.try_into().ok());

        let result = sqlx::query(&format!(
            "insert into {} (session_id, field, value, expires_at, hot_cache_ttl, cache_behavior)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (session_id, field) do update set
                value = excluded.value,
                expires_at = excluded.expires_at,
                hot_cache_ttl = excluded.hot_cache_ttl,
                cache_behavior = excluded.cache_behavior",
            self.qualified_table_name
        ))
        .bind(session_id.to_string())
        .bind(field)
        .bind(value)
        .bind(expires)
        .bind(hot_ttl)
        .bind(meta.behavior as i16)
        .execute(&self.pool)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn insert_with_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
        meta: &crate::store::LayeredCacheMeta,
    ) -> Result<bool, Error> {
        let mut tx = self.pool.begin().await?;

        // First, rename any existing rows for the session.
        self._rename_session_id(&mut *tx, old_session_id, new_session_id, key_seconds).await?;

        // Now, insert the new field with its metadata.
        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let hot_ttl: Option<i32> = meta.hot_cache_ttl.and_then(|t| t.try_into().ok());

        let result = sqlx::query(&format!(
            "insert into {} (session_id, field, value, expires_at, hot_cache_ttl, cache_behavior)
             values ($1, $2, $3, $4, $5, $6) on conflict do nothing", self.qualified_table_name
        ))
            .bind(new_session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires)
            .bind(hot_ttl)
            .bind(meta.behavior as i16)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }

    async fn update_with_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_seconds: i64,
        _field_seconds: Option<i64>,
        meta: &crate::store::LayeredCacheMeta,
    ) -> Result<bool, Error> {
        let mut tx = self.pool.begin().await?;

        self._rename_session_id(&mut *tx, old_session_id, new_session_id, key_seconds).await?;

        let value = serialize_value(value)?;
        let expires = expires_at(key_seconds);
        let hot_ttl: Option<i32> = meta.hot_cache_ttl.and_then(|t| t.try_into().ok());

        let result = sqlx::query(&format!(
            "insert into {} (session_id, field, value, expires_at, hot_cache_ttl, cache_behavior)
             values ($1, $2, $3, $4, $5, $6)
             on conflict (session_id, field) do update set
                value = excluded.value,
                expires_at = excluded.expires_at,
                hot_cache_ttl = excluded.hot_cache_ttl,
                cache_behavior = excluded.cache_behavior", self.qualified_table_name
        ))
            .bind(new_session_id.to_string())
            .bind(field)
            .bind(value)
            .bind(expires)
            .bind(hot_ttl)
            .bind(meta.behavior as i16)
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;
        Ok(result.rows_affected() > 0)
    }
}
