use crate::Id;
use crate::store::{Error, SessionMap};
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;

/// This trait acts as a private API, allowing the `LayeredStore` to store multiple
/// (field, value, cache_ttl) triplets in a single round-trip.
pub trait LayeredHotStore: Clone + Send + Sync + 'static {
    fn set_multiple(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Option<i64>)],
    ) -> impl Future<Output = Result<i64, Error>> + Send;
}

/// This trait acts as a private API, allowing the `LayeredStore` to save and
/// retrieve caching metadata alongside the session data, without polluting the
/// public `SessionStore` trait.
pub trait LayeredColdStore: Clone + Send + Sync + 'static {
    /// Retrieves all session fields and their corresponding hot_cache_ttl.
    fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> impl Future<Output = Result<Option<(SessionMap, HashMap<String, Option<i64>>)>, Error>> + Send;

    /// Updates a session field along with its specific caching metadata.
    fn set_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        hot_cache_ttl: Option<i64>,
    ) -> impl Future<Output = Result<i64, Error>> + Send;

    /// Inserts a session field with rename along with its specific caching metadata.
    fn set_and_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        hot_cache_ttl: Option<i64>,
    ) -> impl Future<Output = Result<i64, Error>> + Send;
}
