mod common;

use ruts::store::scylla::ScyllaStoreBuilder;
use scylla::client::session_builder::SessionBuilder;
use std::sync::Arc;

async fn setup_store() -> Arc<ruts::store::scylla::ScyllaStore> {
    let uri = std::env::var("SCYLLA_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string());
    let session = SessionBuilder::new().known_node(uri).build().await.unwrap();
    let session = Arc::new(session);

    for table in ["ruts_test.t_test", "ruts_test.t_test_ids"] {
        let _ = session
            .query_unpaged(format!("drop table if exists {table}"), &[])
            .await;
    }

    let store = ScyllaStoreBuilder::new(session.clone())
        .keyspace_name("ruts_test")
        .unwrap()
        .table_name("t_test")
        .unwrap()
        .create_table(true)
        .build()
        .await
        .unwrap();

    Arc::new(store)
}

define_session_store_tests!(setup_store);
define_session_store_timing_tests!(setup_store);
define_layered_cold_store_tests!(setup_store);
