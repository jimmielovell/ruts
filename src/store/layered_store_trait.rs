use crate::Id;
use crate::store::{Error, SessionMap, Ttl};
use serde::Serialize;
use std::collections::HashMap;
use std::future::Future;

/// This trait acts as a private API, allowing the `LayeredStore` to store multiple
/// (field, value, cache_ttl) triplets in a single round-trip.
pub trait LayeredHotStore: Clone + Send + Sync + 'static {
    /// Caches each triplet under `session_id`.
    ///
    /// A pair whose TTL is [`Ttl::ZERO`] is skipped rather than cached: that is
    /// the cold store marking a field as one the hot tier should not hold.
    /// `LayeredStore` filters those out before calling, so skipping here only
    /// catches a stray — cheaper than failing a session read over it.
    fn set_multiple(
        &self,
        session_id: &Id,
        pairs: &[(&str, &[u8], Ttl)],
    ) -> impl Future<Output = Result<(), Error>> + Send;
}

/// This trait acts as a private API, allowing the `LayeredStore` to save and
/// retrieve caching metadata alongside the session data, without polluting the
/// public `SessionStore` trait.
pub trait LayeredColdStore: Clone + Send + Sync + 'static {
    /// Retrieves all session fields and their corresponding hot_cache_ttl.
    ///
    /// The two maps must carry the same keys: every field returned needs an
    /// entry, and a field the hot tier should not hold is reported with
    /// [`Ttl::ZERO`] rather than left out.
    fn get_all_with_meta(
        &self,
        session_id: &Id,
    ) -> impl Future<Output = Result<Option<(SessionMap, HashMap<String, Ttl>)>, Error>> + Send;

    /// Updates a session field along with its specific caching metadata.
    fn set_with_meta<T: Serialize + Send + Sync>(
        &self,
        session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> impl Future<Output = Result<(), Error>> + Send;

    /// Inserts a session field with rename along with its specific caching metadata.
    fn set_and_rename_with_meta<T: Serialize + Send + Sync>(
        &self,
        old_session_id: &Id,
        new_session_id: &Id,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        hot_cache_ttl: Option<Ttl>,
    ) -> impl Future<Output = Result<(), Error>> + Send;
}
