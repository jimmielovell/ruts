mod common;

use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::store::redis::RedisStore;
use std::sync::Arc;

async fn setup_store() -> Arc<RedisStore<Client>> {
    let client = Client::default();
    let _ = client.connect();
    client.wait_for_connect().await.unwrap();

    let _: Result<(), fred::error::Error> = client.flushall(false).await;

    Arc::new(RedisStore::new(Arc::new(client)).await.unwrap())
}

define_session_store_tests!(setup_store);
define_layered_hot_store_tests!(setup_store);
