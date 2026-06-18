mod common;

use ruts::store::moka::MokaStoreBuilder;
use std::sync::Arc;

async fn setup_store() -> Arc<ruts::store::moka::MokaStore> {
    Arc::new(MokaStoreBuilder::new().build())
}

define_session_store_tests!(setup_store);
define_layered_hot_store_tests!(setup_store);
