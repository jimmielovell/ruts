mod store_builder;
pub use store_builder::*;

use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, Ttl, deserialize_value, serialize_value};
use serde::{Serialize, de::DeserializeOwned};
use sqlx::{Executor, PgPool, Postgres, Transaction};
use std::collections::HashMap;

pub use std::time::Duration;

/// A Postgres-backed session store.
pub struct PostgresStore {
    pool: PgPool,
    table_name: String,
    cleanup_task: Option<tokio::task::JoinHandle<()>>,
}

impl Clone for PostgresStore {
    fn clone(&self) -> Self {
        Self {
            pool: self.pool.clone(),
            table_name: self.table_name.clone(),
            cleanup_task: None,
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
        let exists: bool = sqlx::query_scalar(&format!(
            "select exists(select 1 from {} where session_id = $1)",
            self.table_name
        ))
        .bind(new_session_id.as_str())
        .fetch_one(&mut **tx)
        .await?;

        if exists {
            return Ok(false);
        }

        let result = sqlx::query(&format!(
            "update {} set session_id = $1 where session_id = $2",
            self.table_name
        ))
        .bind(new_session_id.as_str())
        .bind(old_session_id.as_str())
        .execute(&mut **tx)
        .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn _remove<'e, E>(&self, executor: E, session_id: &Id, field: &str) -> Result<(), Error>
    where
        E: Executor<'e, Database = Postgres>,
    {
        let query = format!(
            "delete from {} where session_id = $1 and field = $2",
            self.table_name
        );

        sqlx::query(&query)
            .bind(session_id.as_str())
            .bind(field)
            .execute(executor)
            .await?;

        Ok(())
    }

    async fn _upsert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
        old_session_id: Option<&Id>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        if field_ttl.is_zero() {
            let mut tx = self.pool.begin().await?;

            if let Some(old_session_id) = old_session_id {
                let _ = self
                    ._rename_session_id(&mut tx, old_session_id, session_id)
                    .await?;
            }

            self._remove(&mut *tx, session_id, field).await?;
            tx.commit().await?;

            return Ok(());
        }

        let value_bytes = serialize_value(value)?;
        let hot_cache_ttl = hot_cache_ttl.map(|h| h.min(field_ttl));

        let query = format!(
            r#"
            insert into {table} (session_id, field, value, hot_cache_ttl, expires_at)
            values ($1, $2, $3, $4, now() + make_interval(secs => $5::double precision))
            on conflict (session_id, field) do update
            set
                value = excluded.value,
                expires_at = excluded.expires_at,
                hot_cache_ttl = excluded.hot_cache_ttl
            "#,
            table = self.table_name,
        );

        let mut tx = self.pool.begin().await?;

        if let Some(old_session_id) = old_session_id {
            let _ = self
                ._rename_session_id(&mut tx, old_session_id, session_id)
                .await?;
        }

        sqlx::query(&query)
            .bind(session_id.as_str())
            .bind(field)
            .bind(value_bytes)
            .bind(hot_cache_ttl.map(i64::from))
            .bind(f64::from(field_ttl))
            .execute(&mut *tx)
            .await?;

        tx.commit().await?;

        Ok(())
    }
}

impl SessionStore for PostgresStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let query = format!(
            r#"
            select value
            from {table}
            where session_id = $1
              and field = $2
              and expires_at > now()
            "#,
            table = self.table_name
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
            select field, value
            from {table}
            where session_id = $1
              and expires_at > now()
            "#,
            table = self.table_name
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
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        #[cfg(feature = "layered-store")]
        let hot_ttl = hot_cache_ttl;
        #[cfg(not(feature = "layered-store"))]
        let hot_ttl: Option<Ttl> = None;

        self._upsert(session_id, field, value, field_ttl, hot_ttl, None)
            .await
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
        #[cfg(feature = "layered-store")]
        let hot_ttl = hot_cache_ttl;
        #[cfg(not(feature = "layered-store"))]
        let hot_ttl: Option<Ttl> = None;

        self._upsert(
            new_session_id,
            field,
            value,
            field_ttl,
            hot_ttl,
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

    async fn remove(&self, session_id: &Id, field: &str) -> Result<(), Error> {
        self._remove(&self.pool, session_id, field).await
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let query = format!(
            "delete from {table} where session_id = $1",
            table = self.table_name
        );
        let result = sqlx::query(&query)
            .bind(session_id.as_str())
            .execute(&self.pool)
            .await?;

        Ok(result.rows_affected() > 0)
    }

    async fn expire_field(&self, session_id: &Id, field: &str, ttl: Ttl) -> Result<bool, Error> {
        let query = format!(
            r#"
            update {table}
            set expires_at = now() + make_interval(secs => $3::double precision)
            where session_id = $1
              and field = $2
              and expires_at > now()
            "#,
            table = self.table_name
        );

        let result = sqlx::query(&query)
            .bind(session_id.as_str())
            .bind(field)
            .bind(f64::from(ttl))
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
    ) -> Result<Option<(SessionMap, HashMap<String, Ttl>)>, Error> {
        let query = format!(
            r#"
            select
                field,
                value,
                hot_cache_ttl,
                extract(epoch from (expires_at - now()))::bigint as ttl
            from {table}
            where session_id = $1
              and expires_at > now()
            "#,
            table = self.table_name
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

        for (field, value, hot_cache_ttl, ttl) in rows {
            session_map.insert(field.clone(), value);

            let hot_ttl = hot_cache_ttl
                .filter(|t| *t >= 0)
                .unwrap_or(ttl)
                .min(ttl)
                .max(0);

            meta_map.insert(field, Ttl::new(hot_ttl)?);
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
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> Result<(), Error> {
        self._upsert(session_id, field, value, field_ttl, hot_cache_ttl, None)
            .await
    }

    async fn set_and_rename_with_meta<T: Serialize + Send + Sync>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> Result<(), Error> {
        self._upsert(
            new_session_id,
            field,
            value,
            field_ttl,
            hot_cache_ttl,
            Some(old_session_id),
        )
        .await
    }
}
