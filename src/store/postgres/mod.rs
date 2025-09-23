use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use cookie::time::OffsetDateTime;
use serde::{Serialize, de::DeserializeOwned};
use sqlx::postgres::PgRow;
use sqlx::{Executor, PgPool, Postgres, Row};
use std::collections::HashMap;
// Re-export Duration
pub use tokio::time::Duration;

/// A builder for creating a `PostgresStore`.
///
/// This allows for customizing the table and schema names for session storage.
#[derive(Debug)]
pub struct PostgresStoreBuilder {
    pool: PgPool,
    table_name: String,
    create_table: bool,
    schema_name: Option<String>,
    cleanup_interval: Option<Duration>,
}

impl PostgresStoreBuilder {
    /// Creates a new builder with a database pool and default settings.
    pub fn new(pool: PgPool, create_table: bool) -> Self {
        Self {
            pool,
            table_name: "sessions".to_string(),
            create_table,
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
            format!("\"{}\".\"{}\"", schema, self.table_name)
        } else {
            format!("\"{}\"", self.table_name)
        };

        if self.create_table {
            if let Some(schema) = &self.schema_name {
                // Quoted to handle special characters.
                sqlx::query(&format!("create schema if not exists \"{schema}\"",))
                    .execute(&self.pool)
                    .await?;
            }

            let create_table_and_indexes = format!(
                r#"
                create table if not exists {table} (
                    session_id text not null,
                    field text not null,
                    value bytea not null,
                    expires_at timestamptz,
                    hot_cache_ttl bigint,
                    primary key (session_id, field)
                );

                -- Index to speed up session lookups
                create index if not exists ruts_session_idx on {table}(session_id);

                -- Index to speed up TTL cleanup
                create index if not exists ruts_expires_idx on {table}(expires_at);

                -- Composite index for selective cleanups by session and expiry
                create index if not exists ruts_session_expires_idx on {table}(session_id, expires_at);
                "#,
                table = qualified_table_name
            );

            sqlx::raw_sql(&create_table_and_indexes)
                .execute(&self.pool)
                .await?;
        }

        let pool_clone = self.pool.clone();
        let delete_query = format!(
            "delete from {qualified_table_name} where expires_at is not null and expires_at < now()",
        );

        let cleanup_interval = self
            .cleanup_interval
            .unwrap_or_else(|| Duration::from_secs(300));

        tokio::spawn(async move {
            let mut interval = tokio::time::interval(cleanup_interval);
            loop {
                if let Err(err) = sqlx::query(&delete_query).execute(&pool_clone).await {
                    tracing::error!("Failed to clean up expired sessions: {err:?}");
                }
                interval.tick().await;
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
        (None, None) => None,
    };

    if let Some(ttl) = ttl
        && ttl >= 0
    {
        Some(OffsetDateTime::now_utc() + cookie::time::Duration::seconds(ttl))
    } else {
        None
    }
}

fn resolve_ttl_from_stats(row: PgRow) -> Result<i64, sqlx::Error> {
    let affected_rows = row.try_get::<i64, _>("affected_rows")?;
    let has_persistent_ttl = row.try_get::<Option<bool>, _>("has_persistent_ttl")?;
    let max_expires = row.try_get::<Option<OffsetDateTime>, _>("max_expires")?;

    if affected_rows == 0 {
        return Ok(-2);
    }
    if has_persistent_ttl == Some(true) {
        return Ok(-1);
    }
    if let Some(exp) = max_expires {
        let now = OffsetDateTime::now_utc();
        let dur = exp - now;
        let secs = dur.whole_seconds();
        if secs <= 0 { Ok(0) } else { Ok(secs) }
    } else {
        Ok(-1)
    }
}

impl PostgresStore {
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

    async fn _remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        let query = format!(
            r#"
            with removed as (
                delete from {table}
                where session_id = $1 and field = $2
            )
            select
                count(*) as affected_rows,
                bool_or(expires_at is null) as has_persistent_ttl,
                max(expires_at) as max_expires
            from {table}
            where session_id = $1 and field != $2
            "#,
            table = self.qualified_table_name,
        );
        let row = sqlx::query(&query)
            .bind(session_id.to_string())
            .bind(field)
            .fetch_one(&self.pool)
            .await?;
        Ok(resolve_ttl_from_stats(row)?)
    }

    async fn _upsert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
        query: String,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        if key_ttl_secs == Some(0) {
            self.delete(session_id).await?;
            return Ok(-2);
        }

        if field_ttl_secs == Some(0) {
            let ttl_after = self._remove(session_id, field).await?;
            return Ok(ttl_after);
        }

        let value_bytes = serialize_value(value)?;
        let expires = expires_at(key_ttl_secs, field_ttl_secs);

        let query = format!(
            r#"
            with upsert as ({query} returning expires_at),
            combined as (
                select * from upsert
                union all
                select expires_at from {table} where session_id = $1
            )
            select
                (select count(*) from upsert) as affected_rows,
                exists (select 1 from combined where expires_at is null) as has_persistent_ttl,
                max(expires_at) as max_expires
            from combined;
            "#,
            table = self.qualified_table_name
        );

        let mut qb = sqlx::query(&query)
            .bind(session_id.to_string())
            .bind(field)
            .bind(value_bytes)
            .bind(expires);

        #[cfg(feature = "layered-store")]
        if let Some(hot_cache_ttl) = hot_cache_ttl {
            qb = qb.bind(hot_cache_ttl);
        }

        let row = qb.fetch_one(&self.pool).await?;
        Ok(resolve_ttl_from_stats(row)?)
    }

    async fn _upsert_with_rename<T>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        #[cfg(feature = "layered-store")] hot_cache_ttl_secs: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
        query: String,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        if key_ttl_secs == Some(0) {
            self.delete(old_session_id).await?;
            return Ok(-2);
        }

        if field_ttl_secs == Some(0) {
            let mut tx = self.pool.begin().await?;

            sqlx::query(&format!(
                r"delete from {table} where session_id = $1 and field = $2",
                table = self.qualified_table_name
            ))
            .bind(old_session_id.to_string())
            .bind(field)
            .execute(&mut *tx)
            .await?;

            let row = sqlx::query(&format!(
                r"
                with renamed as (
                    update {table}
                    set session_id = $2
                    where session_id = $1 and field != $3
                    returning expires_at
                ),
                select
                    count(*) as affected_rows,
                    exists (select 1 from renamed where expires_at is null) as has_persistent_ttl,
                    max(expires_at) as max_expires
                from renamed
                ",
                table = self.qualified_table_name,
            ))
            .bind(old_session_id.to_string())
            .bind(new_session_id.to_string())
            .bind(field)
            .fetch_one(&mut *tx)
            .await?;

            tx.commit().await?;

            return Ok(resolve_ttl_from_stats(row)?);
        }

        let value_bytes = serialize_value(value)?;
        let expires = expires_at(key_ttl_secs, field_ttl_secs);

        let mut tx = self.pool.begin().await?;

        sqlx::query(&format!(
            r"update {table} set session_id = $1 where session_id = $2",
            table = self.qualified_table_name
        ))
        .bind(new_session_id.to_string())
        .bind(old_session_id.to_string())
        .execute(&mut *tx)
        .await?;

        let query = format!(
            r"
            with upsert as ({query} returning expires_at),
            combined as (
                select * from upsert
                union all
                select expires_at from {table} where session_id = $1
            )
            select
                (select count(*) from upsert) as affected_rows,
                exists (select 1 from combined where expires_at is null) as has_persistent_ttl,
                max(expires_at) as max_expires
            from combined;
            ",
            table = self.qualified_table_name
        );

        let mut qb = sqlx::query(&query)
            .bind(new_session_id.to_string())
            .bind(field)
            .bind(value_bytes)
            .bind(expires);

        #[cfg(feature = "layered-store")]
        if let Some(hot_cache_ttl) = hot_cache_ttl_secs {
            qb = qb.bind(hot_cache_ttl);
        }

        let row = qb.fetch_one(&mut *tx).await?;
        tx.commit().await?;

        Ok(resolve_ttl_from_stats(row)?)
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
            None => Ok(None),
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
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict do nothing
            ",
            table = self.qualified_table_name,
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
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict (session_id, field)
            do update set
                value = excluded.value,
                expires_at = excluded.expires_at
            ",
            table = self.qualified_table_name,
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
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict do nothing
            ",
            table = self.qualified_table_name,
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
        #[cfg(feature = "layered-store")] _: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at)
            values ($1, $2, $3, $4)
            on conflict (session_id, field)
            do update set
                value = excluded.value,
                expires_at = excluded.expires_at
            ",
            table = self.qualified_table_name,
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

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        self._remove(session_id, field).await
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let query = format!(
            "delete from {table} where session_id = $1",
            table = self.qualified_table_name
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
            "update {table} set expires_at = $1 where session_id = $2",
            table = self.qualified_table_name
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
    ) -> Result<Option<(SessionMap, std::collections::HashMap<String, Option<i64>>)>, Error> {
        let query = format!(
            "select field, value, expires_at, hot_cache_ttl from {table} where session_id = $1",
            table = self.qualified_table_name
        );
        let rows: Vec<(String, Vec<u8>, Option<OffsetDateTime>, Option<i64>)> =
            sqlx::query_as(&query)
                .bind(session_id.to_string())
                .fetch_all(&self.pool)
                .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut session_map = HashMap::with_capacity(rows.len());
        let mut meta_map = std::collections::HashMap::new();
        for (field, value, expires_at, hot_cache_ttl) in rows {
            if !expires_at.is_some_and(|err| err < OffsetDateTime::now_utc()) {
                session_map.insert(field.clone(), value);
                meta_map.insert(field, hot_cache_ttl);
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
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at, hot_cache_ttl)
            values ($1, $2, $3, $4, $5)
            on conflict do nothing
            ",
            table = self.qualified_table_name,
        );
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
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
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at, hot_cache_ttl)
            values ($1, $2, $3, $4, $5)
            on conflict (session_id, field)
            do update set
                value = excluded.value,
                expires_at = excluded.expires_at,
                hot_cache_ttl = excluded.hot_cache_ttl
            ",
            table = self.qualified_table_name,
        );
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
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
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at, hot_cache_ttl)
            values ($1, $2, $3, $4, $5)
            on conflict do nothing
            returning session_id
            ",
            table = self.qualified_table_name,
        );
        self._upsert_with_rename(
            old_session_id,
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
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
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        let query = format!(
            r"
            insert into {table} (session_id, field, value, expires_at, hot_cache_ttl)
            values ($1, $2, $3, $4, $5)
            on conflict (session_id, field)
            do update set
                value = excluded.value,
                expires_at = excluded.expires_at,
                hot_cache_ttl = excluded.hot_cache_ttl
            ",
            table = self.qualified_table_name,
        );
        self._upsert_with_rename(
            old_session_id,
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
            query,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde::{Deserialize, Serialize};
    use sqlx::PgPool;
    use std::sync::Arc;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Eq, Clone)]
    struct TestData {
        value: String,
    }

    async fn setup_store() -> Arc<PostgresStore> {
        let database_url =
            std::env::var("DATABASE_URL").expect("DATABASE_URL must be set for tests");
        let pool = PgPool::connect(&database_url).await.unwrap();

        // Clean up table before each test run
        sqlx::query("drop table if exists sessions")
            .execute(&pool)
            .await
            .unwrap();

        let store = PostgresStoreBuilder::new(pool.clone(), true)
            .build()
            .await
            .unwrap();
        Arc::new(store)
    }

    #[tokio::test]
    async fn test_insert_and_get() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "field1";
        let value = TestData {
            value: "hello".into(),
        };

        let ttl = store
            .insert(&session_id, field, &value, Some(60), Some(60), None)
            .await
            .unwrap();
        assert!(ttl > 55);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched, Some(value.clone()));

        // Insert again shouldn't overwrite
        store
            .insert(
                &session_id,
                field,
                &TestData { value: "x".into() },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();
        let fetched2: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched2, Some(value));
    }

    #[tokio::test]
    async fn test_update_overwrites() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "field_update";
        let value = TestData {
            value: "initial".into(),
        };
        let updated_value = TestData {
            value: "updated".into(),
        };

        store
            .insert(&session_id, field, &value, Some(60), Some(60), None)
            .await
            .unwrap();
        store
            .update(&session_id, field, &updated_value, Some(60), Some(60), None)
            .await
            .unwrap();

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched, Some(updated_value));
    }

    #[tokio::test]
    async fn test_ttl_zero_removes() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "ttl_zero";

        // Field TTL = 0 triggers removal
        let ttl = store
            .insert(
                &session_id,
                field,
                &TestData { value: "x".into() },
                Some(60),
                Some(0),
                None,
            )
            .await
            .unwrap();
        assert_eq!(ttl, -2);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_ttl_negative_persists() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "ttl_neg";

        let ttl = store
            .insert(
                &session_id,
                field,
                &TestData { value: "y".into() },
                Some(60),
                Some(-1),
                None,
            )
            .await
            .unwrap();
        assert_eq!(ttl, -1);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_some());
    }

    #[tokio::test]
    async fn test_expire_method() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "expire_field";
        store
            .insert(
                &session_id,
                field,
                &TestData {
                    value: "temp".into(),
                },
                Some(2),
                Some(2),
                None,
            )
            .await
            .unwrap();

        tokio::time::sleep(tokio::time::Duration::from_secs(3)).await;
        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_remove_and_delete() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "to_remove";

        store
            .insert(
                &session_id,
                field,
                &TestData {
                    value: "bye".into(),
                },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();
        let ttl = store.remove(&session_id, field).await.unwrap();
        assert_eq!(ttl, -2);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());

        store.delete(&session_id).await.unwrap();
        let all = store.get_all(&session_id).await.unwrap();
        assert!(all.is_none());
    }

    #[tokio::test]
    async fn test_insert_with_rename() {
        let store = setup_store().await;
        let old_id = Id::default();
        let new_id = Id::default();
        let field = "rename_field";
        let value = TestData {
            value: "rename".into(),
        };

        store
            .insert_with_rename(&old_id, &new_id, field, &value, Some(60), Some(60), None)
            .await
            .unwrap();

        assert!(
            store
                .get::<TestData>(&old_id, field)
                .await
                .unwrap()
                .is_none()
        );
        assert_eq!(
            store.get::<TestData>(&new_id, field).await.unwrap(),
            Some(value)
        );
    }

    #[tokio::test]
    async fn test_rename_to_existing_session() {
        let store = setup_store().await;
        let old_id = Id::default();
        let new_id = Id::default();
        let field_old = "f1";
        let field_new = "f2";

        store
            .insert(
                &old_id,
                field_old,
                &TestData { value: "v1".into() },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();
        store
            .insert(
                &new_id,
                field_new,
                &TestData { value: "v2".into() },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();

        store
            .update_with_rename(
                &old_id,
                &new_id,
                field_old,
                &TestData {
                    value: "v1_upd".into(),
                },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();

        assert!(
            store
                .get::<TestData>(&old_id, field_old)
                .await
                .unwrap()
                .is_none()
        );
        let fetched_new: Option<TestData> = store.get(&new_id, field_old).await.unwrap();
        assert_eq!(fetched_new.unwrap().value, "v1_upd");
    }

    #[tokio::test]
    async fn test_get_all_multiple_fields() {
        let store = setup_store().await;
        let session_id = Id::default();

        store
            .insert(
                &session_id,
                "f1",
                &TestData { value: "v1".into() },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();
        store
            .insert(
                &session_id,
                "f2",
                &TestData { value: "v2".into() },
                Some(60),
                Some(60),
                None,
            )
            .await
            .unwrap();

        let all = store.get_all(&session_id).await.unwrap().unwrap();
        assert_eq!(all.len(), 2);
    }
}
