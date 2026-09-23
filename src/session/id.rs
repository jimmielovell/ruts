use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
#[cfg(feature = "scylla-store")]
use parking_lot::RwLock;
use rand::Rng;
use rand::prelude::StdRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
#[cfg(feature = "scylla-store")]
use std::sync::Arc;
use std::{fmt, str};

thread_local! {
    static RNG: RefCell<StdRng> = RefCell::new(rand::make_rng());
}

/// Encoded length of an id: 128 bits as unpadded base64url.
const LEN: usize = 22;

fn random_encoded() -> [u8; LEN] {
    let mut raw = [0u8; 16];
    RNG.with(|rng| rng.borrow_mut().fill_bytes(&mut raw));

    let mut encoded = [0u8; LEN];
    let _ = BASE64_URL_SAFE_NO_PAD.encode_slice(raw, &mut encoded);
    encoded
}

fn as_str_unchecked(bytes: &[u8; LEN]) -> &str {
    str::from_utf8(bytes).expect("Encoded Id is valid UTF-8")
}

/// Only base64url characters survive this, and those are all ASCII, which is
/// what lets [`Id::as_str`] convert without checking.
fn validate_encoded(bytes: &[u8]) -> Result<[u8; LEN], &'static str> {
    let encoded: [u8; LEN] = bytes
        .try_into()
        .map_err(|_| "Invalid ID length: must be exactly 22 characters")?;

    let mut decoded = [0u8; 16];
    if URL_SAFE_NO_PAD.decode_slice(encoded, &mut decoded).is_err() {
        return Err("Invalid ID characters: must be URL-safe Base64");
    }

    Ok(encoded)
}

/// Where a session's data actually lives, for a store that indirects.
#[cfg(feature = "scylla-store")]
#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct MappingId([u8; LEN]);

#[cfg(feature = "scylla-store")]
impl MappingId {
    #[inline]
    pub(crate) fn random() -> Self {
        Self(random_encoded())
    }

    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        as_str_unchecked(&self.0)
    }
}

#[cfg(feature = "scylla-store")]
impl FromStr for MappingId {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(validate_encoded(s.as_bytes())?))
    }
}

/// A session identifier.
#[derive(Clone)]
pub struct Id {
    cookie_id: [u8; LEN],
    #[cfg(feature = "scylla-store")]
    mapping_id: Arc<RwLock<Option<[u8; LEN]>>>,
    max_age: Option<u64>,
}

impl Default for Id {
    fn default() -> Self {
        Self::new(random_encoded())
    }
}

impl Id {
    fn new(cookie_id: [u8; LEN]) -> Self {
        Self {
            cookie_id,
            #[cfg(feature = "scylla-store")]
            mapping_id: Arc::new(RwLock::new(None)),
            max_age: None,
        }
    }

    #[inline]
    pub fn as_str(&self) -> &str {
        as_str_unchecked(&self.cookie_id)
    }

    #[inline]
    #[cfg(feature = "scylla-store")]
    pub(crate) fn mapping_id(&self) -> Option<MappingId> {
        self.mapping_id.read().map(MappingId)
    }

    #[inline]
    #[cfg(feature = "scylla-store")]
    pub(crate) fn set_mapping_id(&self, mapping_id: MappingId) -> MappingId {
        MappingId(*self.mapping_id.write().get_or_insert(mapping_id.0))
    }

    #[inline]
    #[cfg(feature = "scylla-store")]
    pub(crate) fn clear_mapping_id(&self) {
        *self.mapping_id.write() = None;
    }

    #[inline]
    #[cfg(feature = "scylla-store")]
    pub(crate) fn max_age(&self) -> Option<u64> {
        self.max_age
    }

    #[inline]
    pub(crate) fn with_max_age(mut self, max_age: Option<u64>) -> Self {
        self.max_age = max_age;
        self
    }
}

impl PartialEq for Id {
    fn eq(&self, other: &Self) -> bool {
        self.cookie_id == other.cookie_id
    }
}

impl Eq for Id {}

impl Hash for Id {
    fn hash<H: Hasher>(&self, state: &mut H) {
        self.cookie_id.hash(state);
    }
}

impl Display for Id {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

impl FromStr for Id {
    type Err = &'static str;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self::new(validate_encoded(s.as_bytes())?))
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.cookie_id.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        let cookie_id = <[u8; LEN]>::deserialize(deserializer)?;
        validate_encoded(&cookie_id)
            .map(Self::new)
            .map_err(serde::de::Error::custom)
    }
}

#[cfg(feature = "redis-store")]
impl From<&Id> for fred::types::Key {
    fn from(value: &Id) -> Self {
        value.as_str().into()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[cfg(feature = "scylla-store")]
    use std::collections::hash_map::DefaultHasher;

    #[cfg(feature = "scylla-store")]
    fn hash_of(id: &Id) -> u64 {
        let mut hasher = DefaultHasher::new();
        id.hash(&mut hasher);
        hasher.finish()
    }

    #[test]
    fn default_ids_are_well_formed_and_distinct() {
        let a = Id::default();
        let b = Id::default();

        assert_eq!(a.as_str().len(), LEN);
        assert_ne!(a.as_str(), b.as_str());
        #[cfg(feature = "scylla-store")]
        assert!(a.mapping_id().is_none(), "a fresh id resolves to nothing");
        assert_eq!(a.as_str().parse::<Id>().unwrap().as_str(), a.as_str());
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn resolution_is_visible_through_other_clones() {
        let held_by_session = Id::default();
        let handed_to_store = held_by_session.clone();
        let storage = MappingId::random();

        assert!(held_by_session.mapping_id().is_none());
        handed_to_store.set_mapping_id(storage);

        assert_eq!(
            held_by_session.mapping_id().map(|i| i.as_str().to_string()),
            Some(storage.as_str().to_string()),
            "a resolution recorded by the store must be visible to the session"
        );
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn resolution_is_write_once() {
        let id = Id::default();
        let first = MappingId::random();
        let second = MappingId::random();

        assert_eq!(id.set_mapping_id(first).as_str(), first.as_str());
        assert_eq!(
            id.set_mapping_id(second).as_str(),
            first.as_str(),
            "a second resolution must not displace the first"
        );
        assert_eq!(
            id.mapping_id().map(|i| i.as_str().to_string()),
            Some(first.as_str().to_string())
        );

        id.clear_mapping_id();
        assert!(id.mapping_id().is_none());
        assert_eq!(id.set_mapping_id(second).as_str(), second.as_str());
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn clearing_is_visible_through_other_clones() {
        let held_by_session = Id::default();
        let handed_to_store = held_by_session.clone();
        handed_to_store.set_mapping_id(MappingId::random());
        assert!(held_by_session.mapping_id().is_some());

        handed_to_store.clear_mapping_id();
        assert!(
            held_by_session.mapping_id().is_none(),
            "invalidating a resolution must reach every clone, or a stale one \
             keeps a live handle on the data"
        );
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn independent_ids_do_not_share_resolution() {
        let a = Id::default();
        let b = Id::default();
        a.set_mapping_id(MappingId::random());

        assert!(
            b.mapping_id().is_none(),
            "resolution must not leak between unrelated sessions"
        );

        let text = a.to_string();
        let parsed_one: Id = text.parse().unwrap();
        let parsed_two: Id = text.parse().unwrap();
        parsed_one.set_mapping_id(MappingId::random());
        assert!(parsed_two.mapping_id().is_none());
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn concurrent_resolution_agrees_on_one_value() {
        for _ in 0..64 {
            let id = Id::default();
            let candidates: Vec<MappingId> = (0..8).map(|_| MappingId::random()).collect();

            let observed: Vec<String> = std::thread::scope(|scope| {
                let handles: Vec<_> = candidates
                    .iter()
                    .map(|candidate| {
                        let id = id.clone();
                        let candidate = *candidate;
                        scope.spawn(move || id.set_mapping_id(candidate).as_str().to_string())
                    })
                    .collect();
                handles.into_iter().map(|h| h.join().unwrap()).collect()
            });

            let winner = id
                .mapping_id()
                .expect("one writer must have won")
                .as_str()
                .to_string();
            assert!(
                observed.iter().all(|seen| *seen == winner),
                "every concurrent caller must observe the same resolution"
            );
            assert!(
                candidates.iter().any(|c| c.as_str() == winner),
                "the winning value must be one that was actually offered"
            );
        }
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn attaching_a_resolution_leaves_the_public_value_alone() {
        let id = Id::default();
        let before = id.to_string();
        id.set_mapping_id(MappingId::random());

        assert_eq!(
            id.to_string(),
            before,
            "resolving must not change what the client sees"
        );
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn identity_ignores_the_internal_value() {
        let id = Id::default();
        let clone = id.clone();
        clone.set_mapping_id(MappingId::random());

        assert!(id == clone, "resolving must not change session identity");
        assert_eq!(hash_of(&id), hash_of(&clone));

        // An unresolved id parsed from the same text is still the same id.
        let parsed: Id = id.to_string().parse().unwrap();
        assert!(id == parsed);
        assert_eq!(hash_of(&id), hash_of(&parsed));
    }

    #[cfg(feature = "scylla-store")]
    #[test]
    fn the_internal_value_cannot_arrive_from_outside() {
        let resolved = Id::default();
        resolved.set_mapping_id(MappingId::random());
        assert!(resolved.mapping_id().is_some());

        let reparsed: Id = resolved.to_string().parse().unwrap();
        assert_eq!(reparsed.as_str(), resolved.as_str());
        assert!(
            reparsed.mapping_id().is_none(),
            "a client must not be able to supply a storage location"
        );

        let round_tripped: Id =
            serde_json::from_str(&serde_json::to_string(&resolved).unwrap()).unwrap();
        assert_eq!(round_tripped.as_str(), resolved.as_str());
        assert!(
            round_tripped.mapping_id().is_none(),
            "a storage location must not survive serialization"
        );
    }

    #[test]
    fn rejects_malformed_ids() {
        assert!("".parse::<Id>().is_err());
        assert!("too-short".parse::<Id>().is_err());
        assert!(format!("{}x", Id::default()).parse::<Id>().is_err());

        let mut bad = Id::default().to_string().into_bytes();
        bad[0] = b'!';
        assert!(str::from_utf8(&bad).unwrap().parse::<Id>().is_err());
    }

    #[test]
    fn deserializing_rejects_what_could_not_be_printed() {
        // The right length, but not an encoded id.
        let mut bytes = vec![b'A'; LEN];
        bytes[3] = 0xFF;
        assert!(
            serde_json::from_str::<Id>(&serde_json::to_string(&bytes).unwrap()).is_err(),
            "an id that cannot be printed must not be constructible"
        );

        // A byte outside the base64url alphabet, and the wrong length, are
        // turned away at the same door `FromStr` uses.
        bytes[3] = b'!';
        assert!(serde_json::from_str::<Id>(&serde_json::to_string(&bytes).unwrap()).is_err());
        assert!(serde_json::from_str::<Id>("[65,65,65]").is_err());

        // A real id still round-trips.
        let id = Id::default();
        let round_tripped: Id = serde_json::from_str(&serde_json::to_string(&id).unwrap()).unwrap();
        assert_eq!(round_tripped.as_str(), id.as_str());
    }

    #[test]
    fn serialization_matches_the_previous_wire_form() {
        let id = Id::default();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            serde_json::to_string(id.as_str().as_bytes()).unwrap()
        );
    }
}
