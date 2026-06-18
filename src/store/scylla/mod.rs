use crate::Id;
use crate::store::{Error, SessionMap, SessionStore, deserialize_value, serialize_value};
use futures::stream::StreamExt;
use scylla::client::session::Session as ScyllaSession;
use scylla::statement::Consistency;
use scylla::statement::prepared::PreparedStatement;
use serde::{Serialize, de::DeserializeOwned};
use std::collections::HashMap;
use std::sync::Arc;

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

    /// Configures the keyspace to use SimpleStrategy with the specified replication factor.
    pub fn simple_strategy(mut self, replication_factor: u8) -> Self {
        self.replication_strategy = ReplicationStrategy::Simple(replication_factor);
        self
    }

    /// Configures the keyspace to use NetworkTopologyStrategy.
    /// Can be chained multiple times to add multiple datacenters.
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
        let full_table_name = format!("{}.{}", self.keyspace_name, self.table_name);

        if self.create_table {
            let ks_query = format!(
                "create keyspace if not exists {} with replication = {}",
                self.keyspace_name,
                self.replication_strategy.as_cql()
            );
            self.session
                .query_unpaged(ks_query, &[])
                .await
                .map_err(|err| Error::Backend(err.to_string()))?;

            let table_query = format!(
                r#"
                create table if not exists {} (
                    session_id text,
                    field text,
                    value blob,
                    hot_cache_ttl bigint,
                    primary key (session_id, field)
                ) with compaction = {{
                    'class': 'TimeWindowCompactionStrategy',
                    'compaction_window_unit': 'HOURS',
                    'compaction_window_size': 1
                }}
                "#,
                full_table_name
            );
            self.session
                .query_unpaged(table_query, &[])
                .await
                .map_err(|err| Error::Backend(err.to_string()))?;
        }

        let mut get_ttl_stmt = self
            .session
            .prepare(format!(
                "select ttl(value) from {} where session_id = ?",
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        get_ttl_stmt.set_consistency(Consistency::LocalQuorum);

        let mut insert_no_ttl_stmt = self
            .session
            .prepare(format!(
                "insert into {} (session_id, field, value, hot_cache_ttl) values (?, ?, ?, ?)",
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        insert_no_ttl_stmt.set_consistency(Consistency::LocalQuorum);

        let mut insert_with_ttl_stmt = self
            .session
            .prepare(format!(
                r#"
                insert into {} (session_id, field, value, hot_cache_ttl)
                values (?, ?, ?, ?)
                using ttl ?
                "#,
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        insert_with_ttl_stmt.set_consistency(Consistency::LocalQuorum);

        let mut get_stmt = self
            .session
            .prepare(format!(
                "select value from {} where session_id = ? and field = ?",
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        get_stmt.set_consistency(Consistency::LocalQuorum);

        let mut get_all_stmt = self
            .session
            .prepare(format!(
                "select field, value from {} where session_id = ?",
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        get_all_stmt.set_consistency(Consistency::LocalQuorum);

        let mut get_all_meta_stmt = self
            .session
            .prepare(format!(
                r#"
                select field, value, hot_cache_ttl, ttl(value)
                from {}
                where session_id = ?
                "#,
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        get_all_meta_stmt.set_consistency(Consistency::LocalQuorum);

        let mut remove_stmt = self
            .session
            .prepare(format!(
                "delete from {} where session_id = ? and field = ?",
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        remove_stmt.set_consistency(Consistency::LocalQuorum);

        let mut delete_stmt = self
            .session
            .prepare(format!(
                "delete from {} where session_id = ?",
                full_table_name
            ))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;
        delete_stmt.set_consistency(Consistency::LocalQuorum);

        Ok(ScyllaStore {
            session: self.session,
            table_name: full_table_name,
            get_ttl_stmt,
            insert_no_ttl_stmt,
            insert_with_ttl_stmt,
            get_stmt,
            get_all_stmt,
            get_all_meta_stmt,
            remove_stmt,
            delete_stmt,
        })
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

#[derive(Clone)]
pub struct ScyllaStore {
    session: Arc<ScyllaSession>,
    table_name: String,
    get_ttl_stmt: PreparedStatement,
    insert_no_ttl_stmt: PreparedStatement,
    insert_with_ttl_stmt: PreparedStatement,
    get_stmt: PreparedStatement,
    get_all_stmt: PreparedStatement,
    get_all_meta_stmt: PreparedStatement,
    remove_stmt: PreparedStatement,
    delete_stmt: PreparedStatement,
}

impl ScyllaStore {
    async fn get_session_ttl(&self, session_id: &Id) -> Result<i64, Error> {
        let mut rows_stream = self
            .session
            .execute_iter(self.get_ttl_stmt.clone(), (session_id.as_str(),))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?
            .rows_stream::<(Option<i32>,)>()
            .map_err(|err| Error::Backend(err.to_string()))?;

        let mut max_ttl: i64 = -2;
        let mut has_rows = false;
        let mut has_persistent = false;

        while let Some(next_row_res) = rows_stream.next().await {
            has_rows = true;
            let (ttl,) = next_row_res.map_err(|err| Error::Backend(err.to_string()))?;
            match ttl {
                Some(ttl) => max_ttl = max_ttl.max(ttl as i64),
                None => has_persistent = true,
            }
        }

        if !has_rows {
            Ok(-2)
        } else if has_persistent {
            Ok(-1)
        } else {
            Ok(max_ttl)
        }
    }

    async fn _upsert<T>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        hot_cache_ttl: Option<i64>,
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
            if let Some(old_id) = old_session_id {
                self.rename_session_id(old_id, session_id).await?;
            }
            let ttl = self.remove(session_id, field).await?;
            return Ok(ttl);
        }

        if let Some(old_id) = old_session_id {
            self.rename_session_id(old_id, session_id).await?;
        }

        let value_bytes = serialize_value(value)?;
        let mut computed_hot_cache_ttl = hot_cache_ttl;

        if field_ttl_secs > 0 {
            computed_hot_cache_ttl = computed_hot_cache_ttl.map(|h| h.min(field_ttl_secs));
        }

        if field_ttl_secs == -1 {
            self.session
                .execute_unpaged(
                    &self.insert_no_ttl_stmt,
                    (
                        session_id.as_str(),
                        field,
                        value_bytes,
                        computed_hot_cache_ttl,
                    ),
                )
                .await
                .map_err(|err| Error::Backend(err.to_string()))?;
        } else {
            let ttl_i32 = field_ttl_secs.clamp(1, i32::MAX as i64) as i32;
            self.session
                .execute_unpaged(
                    &self.insert_with_ttl_stmt,
                    (
                        session_id.as_str(),
                        field,
                        value_bytes,
                        computed_hot_cache_ttl,
                        ttl_i32,
                    ),
                )
                .await
                .map_err(|err| Error::Backend(err.to_string()))?;
        }

        self.get_session_ttl(session_id).await
    }
}

impl SessionStore for ScyllaStore {
    async fn get<T>(&self, session_id: &Id, field: &str) -> Result<Option<T>, Error>
    where
        T: Send + Sync + DeserializeOwned,
    {
        let mut rows_stream = self
            .session
            .execute_iter(self.get_stmt.clone(), (session_id.as_str(), field))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?
            .rows_stream::<(Vec<u8>,)>()
            .map_err(|err| Error::Backend(err.to_string()))?;

        if let Some(next_row_res) = rows_stream.next().await {
            let (data,) = next_row_res.map_err(|err| Error::Backend(err.to_string()))?;
            Ok(Some(deserialize_value(&data)?))
        } else {
            Ok(None)
        }
    }

    async fn get_all(&self, session_id: &Id) -> Result<Option<SessionMap>, Error> {
        let mut rows_stream = self
            .session
            .execute_iter(self.get_all_stmt.clone(), (session_id.as_str(),))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?
            .rows_stream::<(String, Vec<u8>)>()
            .map_err(|err| Error::Backend(err.to_string()))?;

        let mut map = HashMap::new();
        while let Some(next_row_res) = rows_stream.next().await {
            let (field, value) = next_row_res.map_err(|err| Error::Backend(err.to_string()))?;
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
        key_ttl_secs: i64,
        field_ttl_secs: i64,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        #[cfg(feature = "layered-store")]
        let hot_ttl = hot_cache_ttl;
        #[cfg(not(feature = "layered-store"))]
        let hot_ttl: Option<i64> = None;

        self._upsert(
            session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
            hot_ttl,
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
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<i64>,
        #[cfg(not(feature = "layered-store"))] _: Option<std::marker::PhantomData<()>>,
    ) -> Result<i64, Error>
    where
        T: Send + Sync + Serialize,
    {
        #[cfg(feature = "layered-store")]
        let hot_ttl = hot_cache_ttl;
        #[cfg(not(feature = "layered-store"))]
        let hot_ttl: Option<i64> = None;

        self._upsert(
            new_session_id,
            field,
            value,
            key_ttl_secs,
            field_ttl_secs,
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
        let mut rows_stream = self
            .session
            .execute_iter(self.get_all_meta_stmt.clone(), (old_session_id.as_str(),))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?
            .rows_stream::<(String, Vec<u8>, Option<i64>, Option<i32>)>()
            .map_err(|err| Error::Backend(err.to_string()))?;

        let mut rows = Vec::new();
        while let Some(next_row_res) = rows_stream.next().await {
            rows.push(next_row_res.map_err(|err| Error::Backend(err.to_string()))?);
        }

        let affected = !rows.is_empty();

        let futures = rows
            .into_iter()
            .map(|(field, value, hot_cache_ttl, ttl)| async move {
                if let Some(ttl_val) = ttl {
                    self.session
                        .execute_unpaged(
                            &self.insert_with_ttl_stmt,
                            (
                                new_session_id.as_str(),
                                field,
                                value,
                                hot_cache_ttl,
                                ttl_val,
                            ),
                        )
                        .await
                } else {
                    self.session
                        .execute_unpaged(
                            &self.insert_no_ttl_stmt,
                            (new_session_id.as_str(), field, value, hot_cache_ttl),
                        )
                        .await
                }
            });

        if affected {
            futures::future::try_join_all(futures)
                .await
                .map_err(|err| Error::Backend(err.to_string()))?;
            self.delete(old_session_id).await?;
        }

        Ok(affected)
    }

    async fn remove(&self, session_id: &Id, field: &str) -> Result<i64, Error> {
        self.session
            .execute_unpaged(&self.remove_stmt, (session_id.as_str(), field))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;

        self.get_session_ttl(session_id).await
    }

    async fn delete(&self, session_id: &Id) -> Result<bool, Error> {
        self.session
            .execute_unpaged(&self.delete_stmt, (session_id.as_str(),))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?;

        Ok(true)
    }

    async fn expire(&self, session_id: &Id, ttl_secs: i64) -> Result<bool, Error> {
        if ttl_secs == 0 {
            return self.delete(session_id).await;
        }

        let mut rows_stream = self
            .session
            .execute_iter(self.get_all_meta_stmt.clone(), (session_id.as_str(),))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?
            .rows_stream::<(String, Vec<u8>, Option<i64>, Option<i32>)>()
            .map_err(|err| Error::Backend(err.to_string()))?;

        let mut rows = Vec::new();
        while let Some(next_row_res) = rows_stream.next().await {
            rows.push(next_row_res.map_err(|err| Error::Backend(err.to_string()))?);
        }

        let affected = !rows.is_empty();

        let futures = rows
            .into_iter()
            .map(|(field, value, hot_cache_ttl, _)| async move {
                if ttl_secs > 0 {
                    self.session
                        .execute_unpaged(
                            &self.insert_with_ttl_stmt,
                            (
                                session_id.as_str(),
                                field,
                                value,
                                hot_cache_ttl,
                                ttl_secs as i32,
                            ),
                        )
                        .await
                } else {
                    self.session
                        .execute_unpaged(
                            &self.insert_no_ttl_stmt,
                            (session_id.as_str(), field, value, hot_cache_ttl),
                        )
                        .await
                }
            });

        if affected {
            futures::future::try_join_all(futures)
                .await
                .map_err(|err| Error::Backend(err.to_string()))?;
        }

        Ok(affected)
    }
}

#[cfg(feature = "layered-store")]
impl crate::store::LayeredColdStore for ScyllaStore {
    async fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> Result<Option<(SessionMap, HashMap<String, Option<i64>>)>, Error> {
        let mut rows_stream = self
            .session
            .execute_iter(self.get_all_meta_stmt.clone(), (session_id.as_str(),))
            .await
            .map_err(|err| Error::Backend(err.to_string()))?
            .rows_stream::<(String, Vec<u8>, Option<i64>, Option<i32>)>()
            .map_err(|err| Error::Backend(err.to_string()))?;

        let mut session_map = HashMap::new();
        let mut meta_map = HashMap::new();

        while let Some(next_row_res) = rows_stream.next().await {
            let (field, value, mut hot_cache_ttl, ttl) =
                next_row_res.map_err(|err| Error::Backend(err.to_string()))?;

            session_map.insert(field.clone(), value);

            let ttl_i64 = ttl.map(|t| t as i64).unwrap_or(-1);

            if ttl_i64 > -1 {
                hot_cache_ttl = hot_cache_ttl.or(Some(ttl_i64));
                hot_cache_ttl = hot_cache_ttl.min(Some(ttl_i64));
            }

            if ttl_i64 > 0 {
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
