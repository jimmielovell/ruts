mod common;

use ruts::store::scylla::ScyllaStoreBuilder;
use scylla::client::session_builder::SessionBuilder;
use std::sync::Arc;

async fn setup_store() -> Arc<ruts::store::scylla::ScyllaStore> {
    let uri = std::env::var("SCYLLA_URI").unwrap_or_else(|_| "127.0.0.1:9042".to_string());
    let session = SessionBuilder::new().known_node(uri).build().await.unwrap();
    let session = Arc::new(session);

    let store = ScyllaStoreBuilder::new(session.clone())
        .keyspace_name("ruts_test")
        .unwrap()
        .table_name("t_test")
        .unwrap()
        .create_table(true)
        .build()
        .await
        .unwrap();

    // Truncate before returning cleanly for tests
    let _ = session
        .query_unpaged("truncate table ruts_test.t_test", &[])
        .await;

    Arc::new(store)
}

define_session_store_tests!(setup_store);
define_layered_cold_store_tests!(setup_store);
