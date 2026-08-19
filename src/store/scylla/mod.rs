use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, Ttl, deserialize_value, serialize_value};
use futures::stream::StreamExt;
use scylla::client::session::Session as ScyllaSession;
use scylla::statement::prepared::PreparedStatement;
use scylla::statement::{Consistency, SerialConsistency};
use scylla::value::{CqlValue, Row};
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;

fn backend_error<E: std::fmt::Display>(e: E) -> Error {
    Error::Backend(e.to_string())
}

#[derive(Debug, Clone)]
enum ReplicationStrategy {
    Simple(u8),
    NetworkTopology(HashMap<String, u8>),
}

impl ReplicationStrategy {
    fn as_cql(&self) -> String {
        match self {
            Self::Simple(rf) => {
                format!(
                    "{{'class': 'SimpleStrategy', 'replication_factor': {}}}",
                    rf
                )
            }
            Self::NetworkTopology(dcs) => {
                let dcs_str = dcs
                    .iter()
                    .map(|(dc, rf)| format!(", '{}': {}", dc, rf))
                    .collect::<String>();
                format!("{{'class': 'NetworkTopologyStrategy'{}}}", dcs_str)
            }
        }
    }
}

#[derive(Debug)]
pub struct ScyllaStoreBuilder {
    session: Arc<ScyllaSession>,
    keyspace_name: String,
    table_name: String,
    replication_strategy: ReplicationStrategy,
    create_table: bool,
}

impl ScyllaStoreBuilder {
    pub fn new(session: Arc<ScyllaSession>) -> Self {
        Self {
            session,
            keyspace_name: "ruts".to_string(),
            table_name: "t_sessions".to_string(),
            replication_strategy: ReplicationStrategy::Simple(1),
            create_table: false,
        }
    }

    pub fn keyspace_name(mut self, name: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        validate_identifier(&name)?;
        self.keyspace_name = name;
        Ok(self)
    }

    pub fn table_name(mut self, name: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        validate_identifier(&name)?;
        self.table_name = name;
        Ok(self)
    }

    pub fn simple_strategy(mut self, replication_factor: u8) -> Self {
        self.replication_strategy = ReplicationStrategy::Simple(replication_factor);
        self
    }

    pub fn network_topology_strategy(
        mut self,
        datacenter: impl Into<String>,
        replication_factor: u8,
    ) -> Self {
        if let ReplicationStrategy::NetworkTopology(ref mut dcs) = self.replication_strategy {
            dcs.insert(datacenter.into(), replication_factor);
        } else {
            let mut dcs = HashMap::new();
            dcs.insert(datacenter.into(), replication_factor);
            self.replication_strategy = ReplicationStrategy::NetworkTopology(dcs);
        }
        self
    }

    pub fn create_table(mut self, create: bool) -> Self {
        self.create_table = create;
        self
    }

    pub async fn build(self) -> Result<ScyllaStore, Error> {
        let kv = format!("{}.{}", self.keyspace_name, self.table_name);

        if self.create_table {
            self.session
                .query_unpaged(
                    format!(
                        "create keyspace if not exists {} with replication = {}",
                        self.keyspace_name,
                        self.replication_strategy.as_cql()
                    ),
                    (),
                )
                .await
                .map_err(backend_error)?;

            self.session
                .query_unpaged(
                    format!(
                        "create table if not exists {kv} (
                            session_id text,
                            field text,
                            value blob,
                            hot_cache_ttl bigint,
                            primary key (session_id, field)
                        ) with compaction = {{'class': 'LeveledCompactionStrategy'}}"
                    ),
                    (),
                )
                .await
                .map_err(backend_error)?;
        }

        let session = self.session.clone();
        let prepare_stmt = |cql: String, serial: bool| {
            let session = session.clone();
            async move {
                let mut st = session.prepare(cql).await.map_err(backend_error)?;
                st.set_consistency(Consistency::LocalQuorum);
                if serial {
                    st.set_serial_consistency(Some(SerialConsistency::LocalSerial));
                }
                Ok::<_, Error>(st)
            }
        };

        let store = ScyllaStore {
            session: self.session,
            get_stmt: prepare_stmt(
                format!("select value from {kv} where session_id = ? and field = ?"),
                false,
            )
                .await?,
            get_all_stmt: prepare_stmt(
                format!("select field, value from {kv} where session_id = ?"),
                false,
            )
                .await?,
            get_all_meta_stmt: prepare_stmt(
                format!("select field, value, hot_cache_ttl, ttl(value) from {kv} where session_id = ?"),
                false,
            )
                .await?,
            get_field_meta_stmt: prepare_stmt(
                format!("select value, hot_cache_ttl from {kv} where session_id = ? and field = ?"),
                false,
            )
                .await?,
            exists_stmt: prepare_stmt(
                format!("select field from {kv} where session_id = ? limit 1"),
                false,
            )
                .await?,
            insert_with_ttl_stmt: prepare_stmt(
                format!("insert into {kv} (session_id, field, value, hot_cache_ttl) values (?, ?, ?, ?) using ttl ?"),
                false,
            )
                .await?,
            expire_field_stmt: prepare_stmt(
                format!("update {kv} using ttl ? set value = ?, hot_cache_ttl = ? where session_id = ? and field = ? if exists"),
                true,
            )
                .await?,
            remove_stmt: prepare_stmt(
                format!("delete from {kv} where session_id = ? and field = ?"),
                false,
            )
                .await?,
            del_partition_stmt: prepare_stmt(
                format!("delete from {kv} where session_id = ?"),
                false,
            )
                .await?,
        };

        Ok(store)
    }
}

fn validate_identifier(name: &str) -> Result<(), Error> {
    if name.is_empty() || name.len() > 48 {
        return Err(Error::Backend(format!(
            "invalid identifier {name:?}: must be 1-48 bytes"
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

/// A ScyllaDB-backed session store.
///
/// One table, one row per field, each row carrying its own native TTL. A
/// session exists exactly as long as it has at least one live field row; there
/// is no session-level marker or lifetime. Reads hit the single table and rely
/// on Scylla to reap expired rows. TTLs are always finite and positive; a field
/// is removed only via [`remove`](ScyllaStore::remove).
#[derive(Clone)]
pub struct ScyllaStore {
    session: Arc<ScyllaSession>,
    get_stmt: PreparedStatement,
    get_all_stmt: PreparedStatement,
    get_all_meta_stmt: PreparedStatement,
    get_field_meta_stmt: PreparedStatement,
    exists_stmt: PreparedStatement,
    insert_with_ttl_stmt: PreparedStatement,
    expire_field_stmt: PreparedStatement,
    remove_stmt: PreparedStatement,
    del_partition_stmt: PreparedStatement,
}

impl ScyllaStore {
    /// Reads the `[applied]` flag of a lightweight transaction.
    fn lwt_applied(qr: scylla::response::query_result::QueryResult) -> Result<bool, Error> {
        let row = qr
            .into_rows_result()
            .map_err(backend_error)?
            .first_row::<Row>()
            .map_err(backend_error)?;
        match row.columns.first() {
            Some(Some(CqlValue::Boolean(applied))) => Ok(*applied),
            _ => Ok(false),
        }
    }

    /// Best-effort existence check on a session (one row is enough). Used as the
    /// rename collision guard and to report `delete`'s affected flag; it is
    /// inherently TOCTOU and not a substitute for an atomic guarantee.
    async fn session_exists(&self, sid: &str) -> Result<bool, Error> {
        let mut stream = self
            .session
            .execute_iter(self.exists_stmt.clone(), (sid,))
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

    /// Copies every field of `old` onto `new` (preserving remaining TTLs) and
    /// deletes `old`. Returns `false` if `old` has no live fields. Returns
    /// `Err` if `new` already exists (session-fixation guard, best-effort).
    /// The copy/delete is not atomic across partitions.
    async fn rename_inner(&self, old: &Id, new: &Id) -> Result<bool, Error> {
        if old == new {
            return self.session_exists(new.as_str()).await;
        }
        let old_sid = old.as_str();
        let new_sid = new.as_str();

        let rows = self.read_all_meta(old_sid).await?;
        if rows.is_empty() {
            return Ok(false);
        }

        if self.session_exists(new_sid).await? {
            return Ok(false);
        }

        let copies = rows.into_iter().map(|(field, value, hot, ttl)| async move {
            self.session
                .execute_unpaged(
                    &self.insert_with_ttl_stmt,
                    (new_sid, field, value, hot, ttl),
                )
                .await
                .map_err(backend_error)
        });
        futures::future::try_join_all(copies).await?;

        self.session
            .execute_unpaged(&self.del_partition_stmt, (old_sid,))
            .await
            .map_err(backend_error)?;
        Ok(true)
    }

    async fn read_all_meta(
        &self,
        sid: &str,
    ) -> Result<Vec<(String, Vec<u8>, Option<i64>, i32)>, Error> {
        let mut stream = self
            .session
            .execute_iter(self.get_all_meta_stmt.clone(), (sid,))
            .await
            .map_err(backend_error)?
            .rows_stream::<(String, Vec<u8>, Option<i64>, i32)>()
            .map_err(backend_error)?;
        let mut rows = Vec::new();
        while let Some(row) = stream.next().await {
            rows.push(row.map_err(backend_error)?);
        }
        Ok(rows)
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
        if let Some(old) = old_session_id {
            self.rename_inner(old, session_id).await?;
        }

        let value_bytes = serialize_value(value)?;
        let hot_cache_ttl = hot_cache_ttl.map(|h| h.min(field_ttl));

        self.session
            .execute_unpaged(
                &self.insert_with_ttl_stmt,
                (
                    session_id.as_str(),
                    field,
                    value_bytes,
                    hot_cache_ttl.map(i64::from),
                    i32::from(field_ttl),
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
        let mut stream = self
            .session
            .execute_iter(self.get_stmt.clone(), (session_id.as_str(), field))
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
        let mut stream = self
            .session
            .execute_iter(self.get_all_stmt.clone(), (session_id.as_str(),))
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
        self.rename_inner(old_session_id, new_session_id).await
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<(), Error> {
        self.session
            .execute_unpaged(&self.remove_stmt, (session_id.as_str(), field))
            .await
            .map_err(backend_error)?;
        Ok(())
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        let sid = session_id.as_str();
        let existed = self.session_exists(sid).await?;
        self.session
            .execute_unpaged(&self.del_partition_stmt, (sid,))
            .await
            .map_err(backend_error)?;
        Ok(existed)
    }

    async fn expire_field(&self, session_id: &Id, field: &str, ttl: Ttl) -> Result<bool, Error> {
        let sid = session_id.as_str();

        // Read the current value/hot-ttl so we can re-apply the TTL to the value
        // cell (CQL has no in-place TTL refresh).
        let mut stream = self
            .session
            .execute_iter(self.get_field_meta_stmt.clone(), (sid, field))
            .await
            .map_err(backend_error)?
            .rows_stream::<(Vec<u8>, Option<i64>)>()
            .map_err(backend_error)?;

        let Some(row) = stream.next().await else {
            return Ok(false);
        };
        let (value, hot_cache_ttl) = row.map_err(backend_error)?;

        // IF EXISTS guards the window between read and write: if the field
        // lapsed in between, the conditional update does not resurrect it.
        let qr = self
            .session
            .execute_unpaged(
                &self.expire_field_stmt,
                (i32::from(ttl), value, hot_cache_ttl, sid, field),
            )
            .await
            .map_err(backend_error)?;
        Self::lwt_applied(qr)
    }
}

#[cfg(feature = "layered-store")]
impl crate::store::LayeredColdStore for ScyllaStore {
    async fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> Result<Option<(SessionMap, HashMap<String, Ttl>)>, Error> {
        let rows = self.read_all_meta(session_id.as_str()).await?;

        let mut session_map = HashMap::new();
        let mut meta_map = HashMap::new();
        for (field, value, hot_cache_ttl, ttl) in rows {
            session_map.insert(field.clone(), value);

            // Clamp to 1s: `ttl(value)` reports 0 for a field with under a
            // second left, and `Ttl` rejects 0 — which would fail the whole
            // session read over one nearly-expired field.
            let hot = hot_cache_ttl
                .filter(|t| *t >= 0)
                .unwrap_or(ttl as i64)
                .min(ttl as i64)
                .max(1);
            meta_map.insert(field, Ttl::new(hot)?);
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
