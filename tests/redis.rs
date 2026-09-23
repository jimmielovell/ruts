mod common;

use fred::clients::Client;
use fred::interfaces::ClientLike;
use ruts::store::redis::RedisStore;
use std::sync::Arc;

async fn setup_store() -> Arc<RedisStore<Client>> {
    // Configurable because this suite calls FLUSHALL: pointing it at the wrong
    // instance wipes that instance.
    let client = match std::env::var("REDIS_URL") {
        Ok(url) => {
            let config = fred::types::config::Config::from_url(&url).unwrap();
            Client::new(config, None, None, None)
        }
        Err(_) => Client::default(),
    };
    let _ = client.connect();
    client.wait_for_connect().await.unwrap();

    let _: Result<(), fred::error::Error> = client.flushall(false).await;

    Arc::new(RedisStore::new(Arc::new(client)).await.unwrap())
}

define_session_store_tests!(setup_store);
define_layered_hot_store_tests!(setup_store);
