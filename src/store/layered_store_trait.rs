use crate::Id;
use crate::store::{Error, SessionMap};
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;

/// This trait acts as a private API, allowing the `LayeredStore` to store multiple
/// (field, value, cache_ttl) triplets in a single round-trip.
pub trait LayeredHotStore: Clone + Send + Sync + 'static {
    fn update_many(
        &self,
        session_id: &Id,
        pairs: &[(String, Vec<u8>, Option<i64>)],
    ) -> impl Future<Output = Result<i64, Error>> + Send;
}

/// Defines the caching behavior for a specific session field.
/// This metadata is intended to be stored in the cold store.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(i16)]
pub enum LayeredCacheBehavior {
    /// The default behavior: data is written to both hot and cold stores.
    WriteThrough = 0,
    /// The data should only exist in the cold store and should not be
    /// automatically warmed into the hot cache on read.
    ColdCacheOnly = 1,
}

impl From<i16> for LayeredCacheBehavior {
    fn from(value: i16) -> Self {
        match value {
            1 => LayeredCacheBehavior::ColdCacheOnly,
            _ => LayeredCacheBehavior::WriteThrough,
        }
    }
}

/// Metadata for caching a single session field.
pub struct LayeredCacheMeta {
    pub behavior: LayeredCacheBehavior,
    pub hot_cache_ttl: Option<i64>,
}

/// This trait acts as a private API, allowing the `LayeredStore` to save and
/// retrieve caching metadata alongside the session data, without polluting the
/// public `SessionStore` trait.
pub trait LayeredColdStore: Clone + Send + Sync + 'static {
    /// Retrieves all session fields and their corresponding caching metadata.
    fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> impl Future<Output = Result<Option<(SessionMap, HashMap<String, LayeredCacheMeta>)>, Error>>
    + Send;

    /// Inserts a session field along with its specific caching metadata.
    fn insert_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: LayeredCacheMeta,
    ) -> impl Future<Output = Result<i64, Error>> + Send;

    /// Updates a session field along with its specific caching metadata.
    fn update_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: LayeredCacheMeta,
    ) -> impl Future<Output = Result<i64, Error>> + Send;

    /// Inserts a session field with rename along with its specific caching metadata.
    fn insert_with_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: LayeredCacheMeta,
    ) -> impl Future<Output = Result<i64, Error>> + Send;

    /// Updates a session field with rename along with its specific caching metadata.
    fn update_with_rename_with_meta<T: Serialize + Send + Sync + 'static>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        key_ttl_secs: Option<i64>,
        field_ttl_secs: Option<i64>,
        meta: LayeredCacheMeta,
    ) -> impl Future<Output = Result<i64, Error>> + Send;
}
