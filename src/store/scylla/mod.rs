mod store_builder;
pub use store_builder::*;

use crate::Id;
use crate::session::MappingId;
use crate::store::{Error, SessionMap, SessionStore, Ttl, deserialize_value, serialize_value};
use futures::stream::StreamExt;
use scylla::client::session::Session as ScyllaSession;
use scylla::statement::Consistency;
use scylla::statement::batch::{Batch, BatchType};
use scylla::statement::prepared::PreparedStatement;
use scylla::value::{CqlValue, Row};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;

const EXPIRE_FIELD_MAX_ATTEMPTS: u8 = 3;

fn backend_error<E: std::fmt::Display>(err: E) -> Error {
    Error::Backend(err.to_string())
}

/// Scylla rejects a TTL past 20 years rather than capping it.
const MAX_TTL_SECS: u64 = 630_720_000;

fn capped(secs: u64) -> i32 {
    secs.min(MAX_TTL_SECS) as i32
}

/// The cookie's lifetime when it has one, else `fallback`. Never zero: CQL
/// reads `using ttl 0` as "no expiry".
fn mapping_ttl(id: &Id, fallback: u64) -> i32 {
    capped(id.max_age().unwrap_or(fallback).max(1))
}

/// A ScyllaDB-backed session store.
#[derive(Clone)]
pub struct ScyllaStore {
    session: Arc<ScyllaSession>,
    select_mapping_id_stmt: PreparedStatement,
    insert_mapping_id_stmt: PreparedStatement,
    delete_all_stmt: PreparedStatement,
    select_field_stmt: PreparedStatement,
    select_field_exists_stmt: PreparedStatement,
    select_all_stmt: PreparedStatement,
    #[cfg(feature = "layered-store")]
    select_all_with_meta_stmt: PreparedStatement,
    select_field_with_meta_stmt: PreparedStatement,
    insert_with_ttl_stmt: PreparedStatement,
    expire_field_stmt: PreparedStatement,
    delete_field_stmt: PreparedStatement,
    delete_partition_stmt: PreparedStatement,
}

impl ScyllaStore {
    async fn get_mapping_id(&self, id: &Id) -> Result<Option<MappingId>, Error> {
        if let Some(mapping_id) = id.mapping_id() {
            return Ok(Some(mapping_id));
        }

        match self.db_get_mapping_id(id.as_str()).await? {
            Some((mapping_id, _)) => Ok(Some(id.set_mapping_id(mapping_id))),
            None => Ok(None),
        }
    }

    async fn get_or_create_mapping_id(&self, id: &Id) -> Result<MappingId, Error> {
        match self.get_mapping_id(id).await? {
            Some(mapping_id) => Ok(mapping_id),
            None => Ok(id.set_mapping_id(MappingId::random())),
        }
    }

    async fn db_get_mapping_id(
        &self,
        cookie_id: &str,
    ) -> Result<Option<(MappingId, Option<i32>)>, Error> {
        let mut stream = self
            .session
            .execute_iter(self.select_mapping_id_stmt.clone(), (cookie_id,))
            .await
            .map_err(backend_error)?
            .rows_stream::<(String, Option<i32>)>()
            .map_err(backend_error)?;

        let Some(row) = stream.next().await else {
            return Ok(None);
        };
        let (mapping_id, ttl) = row.map_err(backend_error)?;

        mapping_id
            .parse::<MappingId>()
            .map(|mapping_id| Some((mapping_id, ttl)))
            .map_err(|err| Error::Backend(format!("stored mapping id is malformed: {err:?}")))
    }

    async fn db_insert_mapping_id(
        &self,
        id: &Id,
        internal: MappingId,
        ttl: i32,
    ) -> Result<(), Error> {
        self.session
            .execute_unpaged(
                &self.insert_mapping_id_stmt,
                (id.as_str(), internal.as_str(), ttl),
            )
            .await
            .map_err(backend_error)?;

        Ok(())
    }

    fn build_db_rename_id_batch(
        &self,
        old: &Id,
        new: &Id,
        mapping_id: MappingId,
        mapping_row_ttl: i32,
    ) -> (Batch, Vec<Vec<Option<CqlValue>>>) {
        let mut batch = Batch::new(BatchType::Logged);
        batch.set_consistency(Consistency::LocalQuorum);

        batch.append_statement(self.insert_mapping_id_stmt.clone());
        batch.append_statement(self.delete_all_stmt.clone());

        let values = vec![
            vec![
                Some(CqlValue::Text(new.as_str().to_string())),
                Some(CqlValue::Text(mapping_id.as_str().to_string())),
                Some(CqlValue::Int(mapping_row_ttl)),
            ],
            vec![Some(CqlValue::Text(old.as_str().to_string()))],
        ];

        (batch, values)
    }

    async fn db_write<T>(
        &self,
        id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        if field_ttl.is_zero() {
            let Some(mapping_id) = self.get_mapping_id(id).await? else {
                return Ok(());
            };

            return self.db_delete_field(mapping_id.as_str(), field).await;
        }

        let mapping_id = self.get_or_create_mapping_id(id).await?;

        if let Err(err) = tokio::try_join!(
            self.db_insert_mapping_id(id, mapping_id, mapping_ttl(id, field_ttl.into())),
            self.db_insert_field_value(mapping_id, field, value, field_ttl, hot_cache_ttl),
        ) {
            id.clear_mapping_id();
            return Err(err);
        }

        Ok(())
    }

    async fn db_rename_and_write<T>(
        &self,
        old: &Id,
        new: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        let (existing, b) = tokio::try_join!(
            self.db_get_mapping_id(old.as_str()),
            self.db_get_mapping_id(new.as_str())
        )?;
        let rotate = old != new && b.is_none();

        let Some((mapping_id, remaining_ttl)) = existing else {
            old.clear_mapping_id();
            let target = if rotate { new } else { old };

            return self
                .db_write(target, field, value, field_ttl, hot_cache_ttl)
                .await;
        };

        if !rotate {
            old.set_mapping_id(mapping_id);

            return self
                .db_write(old, field, value, field_ttl, hot_cache_ttl)
                .await;
        }

        let carried = remaining_ttl.unwrap_or(0).max(i32::from(field_ttl)) as u64;
        let (mut batch, mut values) =
            self.build_db_rename_id_batch(old, new, mapping_id, mapping_ttl(new, carried));

        if field_ttl.is_zero() {
            batch.append_statement(self.delete_field_stmt.clone());
            values.push(vec![
                Some(CqlValue::Text(mapping_id.as_str().to_string())),
                Some(CqlValue::Text(field.to_string())),
            ]);
        } else {
            batch.append_statement(self.insert_with_ttl_stmt.clone());
            values.push(vec![
                Some(CqlValue::Text(mapping_id.as_str().to_string())),
                Some(CqlValue::Text(field.to_string())),
                Some(CqlValue::Blob(serialize_value(value)?)),
                hot_cache_ttl.map(|hot| CqlValue::BigInt(i64::from(hot.min(field_ttl)))),
                Some(CqlValue::Int(capped(field_ttl.into()))),
            ]);
        }

        self.session
            .batch(&batch, values)
            .await
            .map_err(backend_error)?;

        new.set_mapping_id(mapping_id);
        old.clear_mapping_id();

        Ok(())
    }

    #[cfg(feature = "layered-store")]
    async fn db_select_all_fields_values_ttls(
        &self,
        internal: &str,
    ) -> Result<Vec<(String, Vec<u8>, Option<i64>, Option<i32>)>, Error> {
        let mut stream = self
            .session
            .execute_iter(self.select_all_with_meta_stmt.clone(), (internal,))
            .await
            .map_err(backend_error)?
            .rows_stream::<(String, Vec<u8>, Option<i64>, Option<i32>)>()
            .map_err(backend_error)?;

        let mut rows = Vec::new();
        while let Some(row) = stream.next().await {
            rows.push(row.map_err(backend_error)?);
        }

        Ok(rows)
    }

    async fn db_field_exists(&self, mapping_id: &str, field: &str) -> Result<bool, Error> {
        let mut stream = self
            .session
            .execute_iter(self.select_field_exists_stmt.clone(), (mapping_id, field))
            .await
            .map_err(backend_error)?
            .rows_stream::<(String,)>()
            .map_err(backend_error)?;

        Ok(stream
            .next()
            .await
            .transpose()
            .map_err(backend_error)?
            .is_some())
    }

    async fn db_delete_field(&self, mapping_id: &str, field: &str) -> Result<(), Error> {
        self.session
            .execute_unpaged(&self.delete_field_stmt, (mapping_id, field))
            .await
            .map_err(backend_error)?;

        Ok(())
    }

    async fn db_select_field_value_ttl(
        &self,
        mapping_id: &str,
        field: &str,
    ) -> Result<Option<(Vec<u8>, Option<i64>)>, Error> {
        let mut stream = self
            .session
            .execute_iter(
                self.select_field_with_meta_stmt.clone(),
                (mapping_id, field),
            )
            .await
            .map_err(backend_error)?
            .rows_stream::<(Vec<u8>, Option<i64>)>()
            .map_err(backend_error)?;

        match stream.next().await {
            Some(row) => Ok(Some(row.map_err(backend_error)?)),
            None => Ok(None),
        }
    }

    async fn db_insert_field_value<T>(
        &self,
        mapping_id: MappingId,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> Result<(), Error>
    where
        T: Send + Sync + Serialize,
    {
        let value_bytes = serialize_value(value)?;
        let hot_cache_ttl = hot_cache_ttl.map(|h| h.min(field_ttl));

        self.session
            .execute_unpaged(
                &self.insert_with_ttl_stmt,
                (
                    mapping_id.as_str(),
                    field,
                    value_bytes,
                    hot_cache_ttl.map(i64::from),
                    capped(field_ttl.into()),
                ),
            )
            .await
            .map_err(backend_error)?;

        Ok(())
    }
}

impl SessionStore for ScyllaStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let Some(mapping_id) = self.get_mapping_id(session_id).await? else {
            return Ok(None);
        };

        let mut stream = self
            .session
            .execute_iter(self.select_field_stmt.clone(), (mapping_id.as_str(), field))
            .await
            .map_err(backend_error)?
            .rows_stream::<(Vec<u8>,)>()
            .map_err(backend_error)?;

        match stream.next().await {
            Some(row) => {
                let (data,) = row.map_err(backend_error)?;
                Ok(Some(deserialize_value(&data)?))
            }
            None => Ok(None),
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        let Some(mapping_id) = self.get_mapping_id(session_id).await? else {
            return Ok(None);
        };

        let mut stream = self
            .session
            .execute_iter(self.select_all_stmt.clone(), (mapping_id.as_str(),))
            .await
            .map_err(backend_error)?
            .rows_stream::<(String, Vec<u8>)>()
            .map_err(backend_error)?;

        let mut map = HashMap::new();
        while let Some(row) = stream.next().await {
            let (field, value) = row.map_err(backend_error)?;
            map.insert(field, value);
        }

        if map.is_empty() {
            return Ok(None);
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

        self.db_write(session_id, field, value, field_ttl, hot_ttl)
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

        self.db_rename_and_write(
            old_session_id,
            new_session_id,
            field,
            value,
            field_ttl,
            hot_ttl,
        )
        .await
    }

    async fn rename_session_id(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
    ) -> Result<bool, Error> {
        let Some((mapping_id, remaining)) = self.db_get_mapping_id(old_session_id.as_str()).await?
        else {
            old_session_id.clear_mapping_id();
            return Ok(false);
        };

        if old_session_id == new_session_id {
            return Ok(true);
        }

        if self
            .db_get_mapping_id(new_session_id.as_str())
            .await?
            .is_some()
        {
            return Ok(false);
        }

        let (batch, values) = self.build_db_rename_id_batch(
            old_session_id,
            new_session_id,
            mapping_id,
            mapping_ttl(new_session_id, remaining.unwrap_or(0) as u64),
        );

        self.session
            .batch(&batch, values)
            .await
            .map_err(backend_error)?;

        new_session_id.set_mapping_id(mapping_id);
        old_session_id.clear_mapping_id();

        Ok(true)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<bool, Error> {
        let Some(mapping_id) = self.get_mapping_id(session_id).await? else {
            return Ok(false);
        };

        if !self.db_field_exists(mapping_id.as_str(), field).await? {
            return Ok(false);
        }

        self.db_delete_field(mapping_id.as_str(), field).await?;

        Ok(true)
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let Some((mapping_id, _)) = self.db_get_mapping_id(session_id.as_str()).await? else {
            session_id.clear_mapping_id();
            return Ok(false);
        };

        let mut batch = Batch::new(BatchType::Logged);
        batch.set_consistency(Consistency::LocalQuorum);
        batch.append_statement(self.delete_all_stmt.clone());
        batch.append_statement(self.delete_partition_stmt.clone());

        let values = vec![
            vec![Some(CqlValue::Text(session_id.as_str().to_string()))],
            vec![Some(CqlValue::Text(mapping_id.as_str().to_string()))],
        ];

        self.session
            .batch(&batch, values)
            .await
            .map_err(backend_error)?;

        session_id.clear_mapping_id();

        Ok(true)
    }

    async fn expire_field(&self, session_id: &Id, field: &str, ttl: Ttl) -> Result<bool, Error> {
        let Some(mapping_id) = self.get_mapping_id(session_id).await? else {
            return Ok(false);
        };

        if ttl.is_zero() {
            return self.remove(session_id, field).await;
        }

        for _ in 0..EXPIRE_FIELD_MAX_ATTEMPTS {
            let Some((value, hot_cache_ttl)) = self
                .db_select_field_value_ttl(mapping_id.as_str(), field)
                .await?
            else {
                return Ok(false);
            };

            let qr = self
                .session
                .execute_unpaged(
                    &self.expire_field_stmt,
                    (
                        capped(ttl.into()),
                        value.as_slice(),
                        hot_cache_ttl,
                        mapping_id.as_str(),
                        field,
                        value.as_slice(),
                    ),
                )
                .await
                .map_err(backend_error)?;

            let row = qr
                .into_rows_result()
                .map_err(backend_error)?
                .first_row::<Row>()
                .map_err(backend_error)?;
            let is_lwt_applied = match row.columns.first() {
                Some(Some(CqlValue::Boolean(applied))) => *applied,
                _ => false,
            };

            if is_lwt_applied {
                self.db_insert_mapping_id(
                    session_id,
                    mapping_id,
                    mapping_ttl(session_id, ttl.into()),
                )
                .await?;
                return Ok(true);
            }
        }

        Err(Error::Backend(format!(
            "expire_field gave up after {EXPIRE_FIELD_MAX_ATTEMPTS} attempts: field {field:?} \
             is being rewritten concurrently"
        )))
    }
}

#[cfg(feature = "layered-store")]
impl crate::store::LayeredColdStore for ScyllaStore {
    async fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> Result<Option<(SessionMap, HashMap<String, Ttl>)>, Error> {
        let Some(mapping_id) = self.get_mapping_id(session_id).await? else {
            return Ok(None);
        };
        let rows = self
            .db_select_all_fields_values_ttls(mapping_id.as_str())
            .await?;

        let mut session_map = HashMap::new();
        let mut meta_map = HashMap::new();
        for (field, value, hot_cache_ttl, ttl) in rows {
            session_map.insert(field.clone(), value);

            // A row with no TTL never expires, so it never caps the hot TTL.
            let ttl = ttl.map_or(i64::from(i32::MAX), i64::from);

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
        self.db_write(session_id, field, value, field_ttl, hot_cache_ttl)
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
        self.db_rename_and_write(
            old_session_id,
            new_session_id,
            field,
            value,
            field_ttl,
            hot_cache_ttl,
        )
        .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn a_mapping_row_gets_a_lifetime_the_clause_can_express() {
        let immediate = Id::default().with_max_age(Some(0));
        assert_eq!(mapping_ttl(&immediate, 60), 1);

        let forever = Id::default().with_max_age(Some(u64::MAX));
        assert_eq!(mapping_ttl(&forever, 60), MAX_TTL_SECS as i32);

        let ordinary = Id::default().with_max_age(Some(600));
        assert_eq!(mapping_ttl(&ordinary, 60), 600);
    }

    #[test]
    fn a_session_cookie_borrows_the_fallback() {
        let session_cookie = Id::default();

        assert_eq!(mapping_ttl(&session_cookie, 60), 60);
        assert_eq!(mapping_ttl(&session_cookie, 0), 1);
        assert_eq!(mapping_ttl(&session_cookie, u64::MAX), MAX_TTL_SECS as i32);
    }

    #[test]
    fn a_field_ttl_is_held_to_what_scylla_accepts() {
        assert_eq!(capped(60), 60);
        assert_eq!(capped(0), 0);
        assert_eq!(capped(i32::MAX as u64), MAX_TTL_SECS as i32);
    }
}
