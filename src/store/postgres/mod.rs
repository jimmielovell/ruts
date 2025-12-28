use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Executor, PgPool, Postgres};
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
            table_name: "t_sessions".to_string(),
            create_table,
            schema_name: None,
            cleanup_interval: None,
        }
    }

    /// Sets a custom table name for the session store. Defaults to "t_sessions".
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
        let (expiry_table_name, fields_table_name) = if let Some(schema) = &self.schema_name {
            (
                format!("\"{}\".\"{}\"", schema, self.table_name),
                format!("\"{}\".\"{}_kv\"", schema, self.table_name),
            )
        } else {
            (
                format!("\"{}\"", self.table_name),
                format!("\"{}_kv\"", self.table_name),
            )
        };

        if self.create_table {
            if let Some(schema) = &self.schema_name {
                sqlx::query(&format!("create schema if not exists \"{schema}\""))
                    .execute(&self.pool)
                    .await?;
            }

            sqlx::raw_sql(&format!(
                r#"
                create table if not exists {expiry_table_name} (
                    session_id text primary key,
                    expires_at timestamptz
                );
                create index if not exists idx_sessions_expires_at on {expiry_table_name}(expires_at);
                "#
            ))
                .execute(&self.pool)
                .await?;

            sqlx::raw_sql(&format!(
                r#"
                create table if not exists {fields_table_name} (
                    fk_session_id text not null references {expiry_table_name}(session_id) on delete cascade on update cascade,
                    field text not null,
                    value bytea not null,
                    expires_at timestamptz,
                    hot_cache_ttl bigint,
                    primary key (fk_session_id, field)
                );
                -- for looking up fields by session
                create index if not exists idx_fields_session_id on {fields_table_name}(fk_session_id);
                -- for field-level cleanup
                create index if not exists idx_fields_expires_at on {fields_table_name}(expires_at);
                "#
            ))
                .execute(&self.pool)
                .await?;
        }

        let pool = self.pool.clone();
        let e_table = expiry_table_name.clone();
        let f_table = fields_table_name.clone();
        let interval = self.cleanup_interval.unwrap_or(Duration::from_secs(300));

        tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            loop {
                ticker.tick().await;
                // Expired sessions (cascades to fields)
                let _ = sqlx::query(&format!(
                    "delete from {e_table} where expires_at is not null and expires_at < now()"
                ))
                    .execute(&pool)
                    .await;

                let _ = sqlx::query(&format!(
                    "delete from {f_table} where expires_at is not null and expires_at < now()"
                ))
                    .execute(&pool)
                    .await;
            }
        });

        Ok(PostgresStore {
            pool: self.pool,
            expiry_table_name,
            fields_table_name,
        })
    }
}

/// A Postgres-backed session store.
#[derive(Clone, Debug)]
pub struct PostgresStore {
    pool: PgPool,
    expiry_table_name: String,
    fields_table_name: String,
}

fn determine_ttls(key_ttl_secs: Option<i64>, field_ttl_secs: Option<i64>) -> (Option<f64>, Option<f64>) {
    if field_ttl_secs == Some(-1) {
        return (None, None);
    }

    if key_ttl_secs == Some(-1) {
        let f_ttl = field_ttl_secs.and_then(|t| if t <= 0 { None } else { Some(t as f64) });
        return (None, f_ttl);
    }

    let k_ttl = key_ttl_secs.and_then(|t| if t <= 0 { None } else { Some(t as f64) });
    let f_ttl = match field_ttl_secs {
        Some(fts) => if fts <= 0 { None } else { Some(fts as f64) },
        None => k_ttl,
    };

    (k_ttl, f_ttl)
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
            self.expiry_table_name
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
            with
            field_delete as (
                delete from {fields}
                where fk_session_id = $1 and field = $2
                returning expires_at
            ),
            current_session as (
                select expires_at
                from {expiry}
                where session_id = $1
                for update
            ),
            session_status as (
                select count(*) as cnt
                from (select 1 from {fields} where fk_session_id = $1 limit 2) sub
            ),
            session_delete as (
                delete from {expiry} e
                using session_status ss
                where e.session_id = $1
                and ss.cnt <= 1
                returning -2::bigint as ttl
            ),
            session_update as (
                update {expiry} e
                set expires_at = (
                    select case
                        when bool_or(f.expires_at is null) then null
                        else max(f.expires_at)
                    end
                    from {fields} f
                    where f.fk_session_id = e.session_id
                    and f.field != $2
                )
                from field_delete fd, current_session cs, session_status ss
                where e.session_id = $1
                and ss.cnt > 1
                and (
                    fd.expires_at is null
                    or (cs.expires_at is not null and fd.expires_at >= cs.expires_at)
                )
                returning
                    case when e.expires_at is null then -1
                    else extract(epoch from (e.expires_at - now()))::bigint
                    end as ttl
            )
            select coalesce(
                (select ttl from session_delete),
                (select ttl from session_update),
                (select
                    case when expires_at is null then -1
                    else extract(epoch from (expires_at - now()))::bigint
                    end
                 from current_session),
                -2
            )
            "#,
            fields = self.fields_table_name,
            expiry = self.expiry_table_name
        );

        let ttl: i64 = sqlx::query_scalar(&query)
            .bind(session_id.to_string())
            .bind(field)
            .fetch_one(&self.pool)
            .await?;

        Ok(ttl)
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
        old_session_id: Option<&Id>
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        if key_ttl_secs == Some(0) {
            let target_id = old_session_id.unwrap_or(session_id);
            self.delete(target_id).await?;
            return Ok(-2);
        }

        if field_ttl_secs == Some(0) {
            let ttl = self._remove(session_id, field).await?;
            if let Some(old_id) = old_session_id {
                let _ = self._rename_session_id(&self.pool, old_id, session_id).await?;
            }
            return Ok(ttl);
        }

        let value_bytes = serialize_value(value)?;
        let (session_ttl, field_ttl) = determine_ttls(key_ttl_secs, field_ttl_secs);

        #[cfg(feature = "layered-store")]
        let hot_cache_ttl = hot_cache_ttl.min(field_ttl.map(|v| v as i64));

        #[cfg(not(feature = "layered-store"))]
        let hot_cache_ttl: Option<i64> = None;

        let rename_subquery = if old_session_id.is_some() {
            format!(
                r#"
                renamed_session as (
                    update {e_table}
                    set session_id = $1
                    where session_id = $7
                ),
                "#,
                e_table = self.expiry_table_name,
            )
        } else {
            String::from("")
        };

        let query = format!(
            r#"
            with
            {rename_subquery}
            exsert as (
                insert into {e_table} (session_id, expires_at)
                values (
                    $1,
                    now() + make_interval(secs => $5)
                )
                on conflict (session_id) do update
                set expires_at = case
                    when {e_table}.expires_at is null or excluded.expires_at is null then null
                    else greatest({e_table}.expires_at, excluded.expires_at)
                end
                returning
                    case when expires_at is null then -1
                    else extract(epoch from (expires_at - now()))::bigint
                    end
                as expires_at
            ),
            upsert as (
                insert into {f_table} (fk_session_id, field, value, hot_cache_ttl, expires_at)
                values (
                    $1, $2, $3, $4,
                    now() + make_interval(secs => $6)
                )
                on conflict (fk_session_id, field) do update
                set
                    value = excluded.value,
                    expires_at = excluded.expires_at,
                    hot_cache_ttl = excluded.hot_cache_ttl
            )
            select expires_at from exsert;
            "#,
            e_table = self.expiry_table_name,
            f_table = self.fields_table_name,
        );

        let mut qb = sqlx::query_scalar(&query)
            .bind(session_id.to_string())
            .bind(field)
            .bind(value_bytes)
            .bind(hot_cache_ttl)
            .bind(session_ttl)
            .bind(field_ttl);

        if let Some(session_id) = old_session_id {
            qb = qb.bind(session_id.to_string());
        }

        let ttl: i64 = qb.fetch_one(&self.pool).await?;

        Ok(ttl)
    }
}

impl SessionStore for PostgresStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let query = format!(
            r#"
            select f.value
            from {fields} f
            join {expiry} e on f.fk_session_id = e.session_id
            where e.session_id = $1
              and f.field = $2
              and (e.expires_at is null or e.expires_at > now())
              and (f.expires_at is null or f.expires_at > now())
            "#,
            fields = self.fields_table_name,
            expiry = self.expiry_table_name
        );

        let result: Option<(Vec<u8>,)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .bind(field)
            .fetch_optional(&self.pool)
            .await?;

        match result {
            Some((data,)) => Ok(Some(deserialize_value(&data)?)),
            None => Ok(None),
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        let query = format!(
            r#"
            select f.field, f.value
            from {fields} f
            join {expiry} e on f.fk_session_id = e.session_id
            where e.session_id = $1
              and (e.expires_at is null or e.expires_at > now())
              and (f.expires_at is null or f.expires_at > now())
            "#,
            fields = self.fields_table_name,
            expiry = self.expiry_table_name
        );

        let rows: Vec<(String, Vec<u8>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .fetch_all(&self.pool)
            .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut map = HashMap::with_capacity(rows.len());
        for (field, value) in rows {
            map.insert(field, value);
        }

        Ok(Some(SessionMap::new(map)))
    }

    async fn set<T>(
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
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            None,
            None,
        )
            .await
    }

    async fn set_and_rename<T>(
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
        self._upsert(
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            None,
            Some(old_session_id)
        ).await
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
            table = self.expiry_table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.to_string())
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn expire(&self, session_id: &Id, ttl_secs: i64) -> Result<bool, Error> {
        if ttl_secs == 0 {
            return self.delete(session_id).await;
        }

        let ttl_secs_f64 = ttl_secs as f64;

        let query = format!(
            r#"
            with
            target as (
                select case
                    when $2 < 0 then null
                    else (now() + make_interval(secs => $2))
                end as new_expiry
            ),
            session_update as (
                update {expiry}
                set expires_at = target.new_expiry
                from target
                where session_id = $1
                and (expires_at is null or expires_at > now())
                returning 1
            ),
            field_update as (
                update {fields}
                set expires_at = target.new_expiry
                from target, session_update
                where fk_session_id = $1
                and (expires_at is null or expires_at > target.new_expiry)
            )
            select count(*) from session_update
            "#,
            expiry = self.expiry_table_name,
            fields = self.fields_table_name
        );

        let rows_affected: i64 = sqlx::query_scalar(&query)
            .bind(session_id.to_string())
            .bind(ttl_secs_f64)
            .fetch_one(&self.pool)
            .await?;

        Ok(rows_affected > 0)
    }
}

#[cfg(feature = "layered-store")]
impl crate::store::LayeredColdStore for PostgresStore {
    async fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> Result<Option<(SessionMap, HashMap<String, Option<i64>>)>, Error> {
        let query = format!(
            r#"
            select f.field, f.value, f.hot_cache_ttl
            from {fields} f
            join {expiry} e on f.fk_session_id = e.session_id
            where e.session_id = $1
              and (e.expires_at is null or e.expires_at > now())
              and (f.expires_at is null or f.expires_at > now())
            "#,
            fields = self.fields_table_name,
            expiry = self.expiry_table_name
        );

        let rows: Vec<(String, Vec<u8>, Option<i64>)> = sqlx::query_as(&query)
            .bind(session_id.to_string())
            .fetch_all(&self.pool)
            .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut session_map = HashMap::with_capacity(rows.len());
        let mut meta_map = HashMap::new();
        for (field, value, hot_cache_ttl) in rows {
            session_map.insert(field.clone(), value);
            meta_map.insert(field, hot_cache_ttl);
        }

        if session_map.is_empty() {
            return Ok(None);
        }

        Ok(Some((SessionMap::new(session_map), meta_map)))
    }

    async fn set_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
            None,
        )
            .await
    }

    async fn set_and_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        self._upsert(
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
            Some(old_session_id)
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

        sqlx::query("drop table if exists t_sessions cascade")
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
    async fn test_set_and_get() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "field1";
        let value = TestData {
            value: "hello".into(),
        };

        let ttl = store
            .set(&session_id, field, &value, Some(60), Some(60), None)
            .await
            .unwrap();
        assert!(ttl > 55);

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert_eq!(fetched, Some(value.clone()));
        
        store
            .set(
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
        assert_eq!(fetched2, Some(TestData { value: "x".into() }));
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
            .set(&session_id, field, &value, Some(60), Some(60), None)
            .await
            .unwrap();
        store
            .set(&session_id, field, &updated_value, Some(60), Some(60), None)
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
        
        let ttl = store
            .set(
                &session_id,
                field,
                &TestData { value: "x".into() },
                Some(60),
                Some(0),
                None,
            )
            .await
            .unwrap();
        assert_eq!(ttl, -2); // Session empty -> deleted

        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_ttl_negative_persists() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "ttl_neg";

        let ttl = store
            .set(
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
            .set(
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

        tokio::time::sleep(Duration::from_secs(3)).await;
        let fetched: Option<TestData> = store.get(&session_id, field).await.unwrap();
        assert!(fetched.is_none());
    }

    #[tokio::test]
    async fn test_expire_caps_long_lived_fields() {
        let store = setup_store().await;
        let session_id = Id::default();

        store.set(
            &session_id,
            "long",
            &TestData { value: "val".into() },
            Some(3600),
            Some(3600),
            None
        ).await.unwrap();

        store.expire(&session_id, 1).await.unwrap();

        tokio::time::sleep(Duration::from_secs(2)).await;

        let fetched: Option<TestData> = store.get(&session_id, "long").await.unwrap();
        assert!(fetched.is_none(), "Field should have been capped by session expire");
    }

    #[tokio::test]
    async fn test_remove_downgrades_session_expiry() {
        let store = setup_store().await;
        let session_id = Id::default();

        store.set(
            &session_id,
            "A",
            &TestData { value: "a".into() },
            Some(100),
            Some(100),
            None
        ).await.unwrap();

        store.set(
            &session_id,
            "B",
            &TestData { value: "b".into() },
            Some(10),
            Some(10),
            None
        ).await.unwrap();
        
        let ttl = store.remove(&session_id, "A").await.unwrap();

        assert!(ttl > 0 && ttl <= 10, "TTL should have downgraded to match Field B (approx 10s), got {}", ttl);
    }

    #[tokio::test]
    async fn test_remove_and_delete() {
        let store = setup_store().await;
        let session_id = Id::default();
        let field = "to_remove";

        store
            .set(
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
    async fn test_set_with_rename() {
        let store = setup_store().await;
        let old_id = Id::default();
        let new_id = Id::default();
        let field = "rename_field";
        let value = TestData {
            value: "rename".into(),
        };

        store
            .set_and_rename(&old_id, &new_id, field, &value, Some(60), Some(60), None)
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
    async fn test_rename_collision_fails() {
        let store = setup_store().await;
        let old_id = Id::default();
        let new_id = Id::default();
        
        store.set(&old_id, "f1", &TestData { value: "v1".into() }, Some(60), Some(60), None).await.unwrap();
        store.set(&new_id, "f2", &TestData { value: "v2".into() }, Some(60), Some(60), None).await.unwrap();
        
        let result = store
            .set_and_rename(
                &old_id,
                &new_id,
                "f1",
                &TestData { value: "v1_upd".into() },
                Some(60),
                Some(60),
                None,
            )
            .await;

        assert!(result.is_err(), "Rename to existing session ID should fail to prevent session fixation");
    }

    #[tokio::test]
    async fn test_get_all_multiple_fields() {
        let store = setup_store().await;
        let session_id = Id::default();

        store
            .set(
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
            .set(
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
