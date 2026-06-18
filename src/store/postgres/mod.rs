use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Executor, PgPool, Postgres, Transaction};
use std::collections::HashMap;

// Re-export Duration
pub use std::time::Duration;

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
    pub fn new(pool: PgPool) -> Self {
        Self {
            pool,
            table_name: "t_sessions".to_string(),
            create_table: false,
            schema_name: None,
            cleanup_interval: None,
        }
    }

    /// Create the session tables (and schema, if configured) on `build()`.
    ///
    /// Defaults to `false`. Enable for development or when no external
    /// migration system manages the schema.
    pub fn create_table(mut self, create: bool) -> Self {
        self.create_table = create;
        self
    }

    /// Sets a custom table name for the session store. Defaults to "t_sessions".
    pub fn table_name(mut self, table_name: impl Into<String>) -> Result<Self, Error> {
        let name = table_name.into();
        validate_identifier(&name)?;
        self.table_name = name;
        Ok(self)
    }

    /// Sets a custom schema name for the session store.
    pub fn schema_name(mut self, schema_name: impl Into<String>) -> Result<Self, Error> {
        let name = schema_name.into();
        validate_identifier(&name)?;
        self.schema_name = Some(name);
        Ok(self)
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
                    fk_session_id text not null references {expiry_table_name} (session_id) on update cascade on delete cascade,
                    field text not null,
                    value bytea not null,
                    hot_cache_ttl bigint,
                    expires_at timestamptz,
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
        let interval = self.cleanup_interval.unwrap_or(Duration::from_secs(60 * 5));

        let cleanup_task = tokio::spawn(async move {
            let mut ticker = tokio::time::interval(interval);
            ticker.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);

            let query = format!(
                r#"
                with expired_sessions as (
                    delete from {e_table}
                    where expires_at is not null and expires_at < now()
                )
                delete from {f_table}
                where expires_at is not null and expires_at < now()
                "#
            );

            loop {
                ticker.tick().await;
                let _ = sqlx::query(&query).execute(&pool).await;
            }
        });

        Ok(PostgresStore {
            pool: self.pool,
            expiry_table_name,
            fields_table_name,
            cleanup_task: Some(cleanup_task),
        })
    }
}

fn validate_identifier(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.len() > 63 {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: must be 1-63 bytes"
        )));
    }
    let mut chars = name.chars();
    let first = chars.next().unwrap();
    if !(first.is_ascii_alphabetic() || first == '_') {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: must start with a letter or underscore"
        )));
    }
    if !chars.all(|c| c.is_ascii_alphanumeric() || c == '_') {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: only ASCII alphanumerics and underscore allowed"
        )));
    }
    Ok(())
}

/// A Postgres-backed session store.
pub struct PostgresStore {
    pool: PgPool,
    expiry_table_name: String,
    fields_table_name: String,
    cleanup_task: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for PostgresStore {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            expiry_table_name: self.expiry_table_name.clone(),
            fields_table_name: self.fields_table_name.clone(),
            cleanup_task: None, // only the original owns the task
        }
    }
}

impl Drop for PostgresStore {
    fn drop(&mut self) {
        if let Some(handle) = self.cleanup_task.take() {
            handle.abort();
        }
    }
}

impl PostgresStore {
    async fn _rename_session_id(
        &self,
        tx: &mut Transaction<'_, Postgres>,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let fields_q = format!(
            "update {} set fk_session_id = $1 where fk_session_id = $2",
            self.fields_table_name
        );
        sqlx::query(&fields_q)
            .bind(new_session_id.to_string())
            .bind(old_session_id.to_string())
            .execute(&mut **tx)
            .await?;

        let expiry_q = format!(
            "update {} set session_id = $1 where session_id = $2",
            self.expiry_table_name
        );
        let result = sqlx::query(&expiry_q)
            .bind(new_session_id.to_string())
            .bind(old_session_id.to_string())
            .execute(&mut **tx)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn _remove<'e, E>(&self, executor: E, session_id: &Id, field: &str) -> Result<i64, Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
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
            .bind(session_id.as_str())
            .bind(field)
            .fetch_one(executor)
            .await?;

        Ok(ttl)
    }

    async fn _upsert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
        old_session_id: Option<&Id>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        if key_ttl_secs == 0 {
            self.delete(session_id).await?;
            return Ok(-2);
        }

        if field_ttl_secs == 0 {
            let mut tx = self.pool.begin().await?;

            if let Some(old_session_id) = old_session_id {
                let _ = self
                    ._rename_session_id(&mut tx, old_session_id, session_id)
                    .await?;
            }

            let ttl = self._remove(&self.pool, session_id, field).await?;
            tx.commit().await?;
            return Ok(ttl);
        }

        let value_bytes = serialize_value(value)?;

        #[cfg(feature = "layered-store")]
        let hot_cache_ttl = hot_cache_ttl.min(Some(field_ttl_secs));
        #[cfg(not(feature = "layered-store"))]
        let hot_cache_ttl: Option<i64> = None;

        let key_ttl = (key_ttl_secs != -1).then_some(key_ttl_secs as f64);
        let field_ttl = (field_ttl_secs != -1).then_some(field_ttl_secs as f64);

        let query = format!(
            r#"
            with
            exsert as (
                insert into {e_table} (session_id, expires_at)
                values ($1, now() + make_interval(secs => $5))
                on conflict (session_id) do update
                set expires_at = case
                    when {e_table}.expires_at is null or excluded.expires_at is null then null
                    else greatest({e_table}.expires_at, excluded.expires_at)
                end
                returning session_id, expires_at
            ),
            upsert as (
                insert into {f_table} (fk_session_id, field, value, hot_cache_ttl, expires_at)
                select p.session_id, $2, $3, $4, now() + make_interval(secs => $6)
                from exsert p
                on conflict (fk_session_id, field) do update
                set
                    value = excluded.value,
                    expires_at = excluded.expires_at,
                    hot_cache_ttl = excluded.hot_cache_ttl
            )
            select
                case when expires_at is null then -1
                else extract(epoch from (expires_at - now()))::bigint
                end
            from exsert
            "#,
            e_table = self.expiry_table_name,
            f_table = self.fields_table_name,
        );

        let qs = sqlx::query_scalar(&query)
            .bind(session_id.as_str())
            .bind(field)
            .bind(value_bytes)
            .bind(hot_cache_ttl)
            .bind(key_ttl)
            .bind(field_ttl);

        if let Some(old_session_id) = old_session_id {
            let mut tx = self.pool.begin().await?;
            let _ = self
                ._rename_session_id(&mut tx, old_session_id, session_id)
                .await?;
            let ttl = qs.fetch_one(&mut *tx).await?;
            tx.commit().await?;

            return Ok(ttl);
        }

        let ttl: i64 = qs.fetch_one(&self.pool).await?;

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
            .bind(session_id.as_str())
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
            .bind(session_id.as_str())
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
        key_ttl_secs: i64,
        field_ttl_secs: i64,
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
        key_ttl_secs: i64,
        field_ttl_secs: i64,
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
            Some(old_session_id),
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let mut tx = self.pool.begin().await?;
        let result = self
            ._rename_session_id(&mut tx, old_session_id, new_session_id)
            .await?;
        tx.commit().await?;

        Ok(result)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        self._remove(&self.pool, session_id, field).await
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let query = format!(
            "delete from {table} where session_id = $1",
            table = self.expiry_table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.as_str())
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
                    and (
                        target.new_expiry is null
                        or (expires_at is not null and expires_at > target.new_expiry)
                    )
            )
            select count(*) from session_update
            "#,
            expiry = self.expiry_table_name,
            fields = self.fields_table_name
        );

        let rows_affected: i64 = sqlx::query_scalar(&query)
            .bind(session_id.as_str())
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
            select 
                f.field,
                f.value, 
                f.hot_cache_ttl,
                case when f.expires_at is null then -1
                    else extract(epoch from (f.expires_at - now()))::bigint
                end as ttl
            from {fields} f
            join {expiry} e on f.fk_session_id = e.session_id
            where e.session_id = $1
              and (e.expires_at is null or e.expires_at > now())
              and (f.expires_at is null or f.expires_at > now())
            "#,
            fields = self.fields_table_name,
            expiry = self.expiry_table_name
        );

        let rows: Vec<(String, Vec<u8>, Option<i64>, i64)> = sqlx::query_as(&query)
            .bind(session_id.as_str())
            .fetch_all(&self.pool)
            .await?;

        if rows.is_empty() {
            return Ok(None);
        }

        let mut session_map = HashMap::with_capacity(rows.len());
        let mut meta_map = HashMap::new();
        for (field, value, mut hot_cache_ttl, ttl) in rows {
            session_map.insert(field.clone(), value);
            if ttl > -1 {
                hot_cache_ttl = hot_cache_ttl.or(Some(ttl));
                hot_cache_ttl = hot_cache_ttl.min(Some(ttl));
            }

            if ttl > 0 {
                meta_map.insert(field, hot_cache_ttl);
            }
        }

        if session_map.is_empty() {
            return Ok(None);
        }

        Ok(Some((SessionMap::new(session_map), meta_map)))
    }

    async fn set_with_meta<T: Serialize + Send + Sync>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
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

    async fn set_and_rename_with_meta<T: Serialize + Send + Sync>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        hot_cache_ttl_secs: Option<i64>,
    ) -> Result<i64, Error> {
        self._upsert(
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_cache_ttl_secs,
            Some(old_session_id),
        )
        .await
    }
}
