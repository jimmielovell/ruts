use crate::store::Error;
use crate::store::scylla::{ScyllaStore, backend_error};
use scylla::client::session::Session as ScyllaSession;
use scylla::statement::{Consistency, SerialConsistency};
use std::collections::HashMap;
use std::sync::Arc;

const MAPPING_TABLE_SUFFIX: &str = "_ids";

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

    /// Sets the data table's name. The id-mapping table is named by appending
    /// `_ids`, so the name given here must leave room for that suffix.
    pub fn table_name(mut self, name: impl Into<String>) -> Result<Self, Error> {
        let name = name.into();
        validate_identifier(&name)?;
        validate_identifier(&format!("{name}{MAPPING_TABLE_SUFFIX}"))?;
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
        let data = format!("{}.{}", self.keyspace_name, self.table_name);
        let ids = format!(
            "{}.{}{}",
            self.keyspace_name, self.table_name, MAPPING_TABLE_SUFFIX
        );

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
                        "create table if not exists {ids} (
                            cookie_id text primary key,
                            mapping_id text
                        ) with compaction = {{'class': 'LeveledCompactionStrategy'}}"
                    ),
                    (),
                )
                .await
                .map_err(backend_error)?;

            self.session
                .query_unpaged(
                    format!(
                        "create table if not exists {data} (
                            mapping_id text,
                            field text,
                            value blob,
                            hot_cache_ttl bigint,
                            primary key (mapping_id, field)
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
            select_mapping_id_stmt: prepare_stmt(
                format!("select mapping_id, ttl(mapping_id) from {ids} where cookie_id = ?"),
                false,
            )
                .await?,
            insert_mapping_id_stmt: prepare_stmt(
                format!("insert into {ids} (cookie_id, mapping_id) values (?, ?) using ttl ?"),
                false,
            )
                .await?,
            select_field_stmt: prepare_stmt(
                format!("select value from {data} where mapping_id = ? and field = ?"),
                false,
            )
                .await?,
            select_field_exists_stmt: prepare_stmt(
                format!("select field from {data} where mapping_id = ? and field = ?"),
                false,
            )
                .await?,
            select_all_stmt: prepare_stmt(
                format!("select field, value from {data} where mapping_id = ?"),
                false,
            )
                .await?,
            select_field_with_meta_stmt: prepare_stmt(
                format!("select value, hot_cache_ttl from {data} where mapping_id = ? and field = ?"),
                false,
            )
                .await?,
            #[cfg(feature = "layered-store")]
            select_all_with_meta_stmt: prepare_stmt(
                format!(
                    "select field, value, hot_cache_ttl, ttl(value) from {data} where mapping_id = ?"
                ),
                false,
            )
                .await?,
            insert_with_ttl_stmt: prepare_stmt(
                format!(
                    "insert into {data} (mapping_id, field, value, hot_cache_ttl) values (?, ?, ?, ?) using ttl ?"
                ),
                false,
            )
                .await?,
            expire_field_stmt: prepare_stmt(
                format!(
                    "update {data} using ttl ? set value = ?, hot_cache_ttl = ? where mapping_id = ? and field = ? if value = ?"
                ),
                true,
            )
                .await?,
            delete_field_stmt: prepare_stmt(
                format!("delete from {data} where mapping_id = ? and field = ?"),
                false,
            )
                .await?,
            delete_all_stmt: prepare_stmt(
                format!("delete from {ids} where cookie_id = ?"),
                false,
            )
                .await?,
            delete_partition_stmt: prepare_stmt(
                format!("delete from {data} where mapping_id = ?"),
                false,
            )
                .await?,
        };

        Ok(store)
    }
}
