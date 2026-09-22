use base64::Engine;
use base64::engine::general_purpose::URL_SAFE_NO_PAD;
use base64::prelude::BASE64_URL_SAFE_NO_PAD;
use parking_lot::RwLock;
use rand::Rng;
use rand::prelude::StdRng;
use serde::{Deserialize, Deserializer, Serialize, Serializer};
use std::cell::RefCell;
use std::fmt::Display;
use std::hash::{Hash, Hasher};
use std::str::FromStr;
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

#[derive(Copy, Clone, PartialEq, Eq)]
pub(crate) struct MappingId([u8; LEN]);

impl MappingId {
    #[inline]
    pub(crate) fn as_str(&self) -> &str {
        as_str_unchecked(&self.0)
    }
}

/// A session identifier.
#[derive(Clone)]
pub struct Id {
    cookie_id: [u8; LEN],
    mapping_id: Arc<RwLock<Option<[u8; LEN]>>>,
    max_age: Option<u64>,
}

impl Default for Id {
    fn default() -> Self {
        Self {
            cookie_id: random_encoded(),
            mapping_id: Arc::new(RwLock::new(None)),
            max_age: None,
        }
    }
}

impl Id {
    #[inline]
    pub fn as_str(&self) -> &str {
        as_str_unchecked(&self.cookie_id)
    }

    #[inline]
    pub(crate) fn mapping_id(&self) -> Option<MappingId> {
        self.mapping_id.read().map(MappingId)
    }

    #[inline]
    pub(crate) fn set_mapping_id(&self, storage: &Id) -> MappingId {
        MappingId(*self.mapping_id.write().get_or_insert(storage.cookie_id))
    }

    #[inline]
    pub(crate) fn clear_mapping_id(&self) {
        *self.mapping_id.write() = None;
    }

    #[inline]
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
        if s.len() != LEN {
            return Err("Invalid ID length: must be exactly 22 characters");
        }

        let mut decoded_buffer = [0u8; 16];
        if URL_SAFE_NO_PAD
            .decode_slice(s.as_bytes(), &mut decoded_buffer)
            .is_err()
        {
            return Err("Invalid ID characters: must be URL-safe Base64");
        }

        let mut public = [0u8; LEN];
        public.copy_from_slice(s.as_bytes());
        Ok(Self {
            cookie_id: public,
            mapping_id: Arc::new(RwLock::new(None)),
            max_age: None,
        })
    }
}

impl Serialize for Id {
    fn serialize<S: Serializer>(&self, serializer: S) -> Result<S::Ok, S::Error> {
        self.cookie_id.serialize(serializer)
    }
}

impl<'de> Deserialize<'de> for Id {
    fn deserialize<D: Deserializer<'de>>(deserializer: D) -> Result<Self, D::Error> {
        Ok(Self {
            cookie_id: <[u8; LEN]>::deserialize(deserializer)?,
            mapping_id: Arc::new(RwLock::new(None)),
            max_age: None,
        })
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
    use std::collections::hash_map::DefaultHasher;

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
        assert!(a.mapping_id().is_none(), "a fresh id resolves to nothing");
        assert_eq!(a.as_str().parse::<Id>().unwrap().as_str(), a.as_str());
    }

    #[test]
    fn resolution_is_visible_through_other_clones() {
        let held_by_session = Id::default();
        let handed_to_store = held_by_session.clone();
        let storage = Id::default();

        assert!(held_by_session.mapping_id().is_none());
        handed_to_store.set_mapping_id(&storage);

        assert_eq!(
            held_by_session.mapping_id().map(|i| i.as_str().to_string()),
            Some(storage.as_str().to_string()),
            "a resolution recorded by the store must be visible to the session"
        );
    }

    #[test]
    fn resolution_is_write_once() {
        let id = Id::default();
        let first = Id::default();
        let second = Id::default();

        assert_eq!(id.set_mapping_id(&first).as_str(), first.as_str());
        assert_eq!(
            id.set_mapping_id(&second).as_str(),
            first.as_str(),
            "a second resolution must not displace the first"
        );
        assert_eq!(
            id.mapping_id().map(|i| i.as_str().to_string()),
            Some(first.as_str().to_string())
        );

        id.clear_mapping_id();
        assert!(id.mapping_id().is_none());
        assert_eq!(id.set_mapping_id(&second).as_str(), second.as_str());
    }

    #[test]
    fn clearing_is_visible_through_other_clones() {
        let held_by_session = Id::default();
        let handed_to_store = held_by_session.clone();
        handed_to_store.set_mapping_id(&Id::default());
        assert!(held_by_session.mapping_id().is_some());

        handed_to_store.clear_mapping_id();
        assert!(
            held_by_session.mapping_id().is_none(),
            "invalidating a resolution must reach every clone, or a stale one \
             keeps a live handle on the data"
        );
    }

    #[test]
    fn independent_ids_do_not_share_resolution() {
        let a = Id::default();
        let b = Id::default();
        a.set_mapping_id(&Id::default());

        assert!(
            b.mapping_id().is_none(),
            "resolution must not leak between unrelated sessions"
        );

        let text = a.to_string();
        let parsed_one: Id = text.parse().unwrap();
        let parsed_two: Id = text.parse().unwrap();
        parsed_one.set_mapping_id(&Id::default());
        assert!(parsed_two.mapping_id().is_none());
    }

    #[test]
    fn concurrent_resolution_agrees_on_one_value() {
        for _ in 0..64 {
            let id = Id::default();
            let candidates: Vec<Id> = (0..8).map(|_| Id::default()).collect();

            let observed: Vec<String> = std::thread::scope(|scope| {
                let handles: Vec<_> = candidates
                    .iter()
                    .map(|candidate| {
                        let id = id.clone();
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

    #[test]
    fn attaching_a_resolution_leaves_the_public_value_alone() {
        let id = Id::default();
        let before = id.to_string();
        id.set_mapping_id(&Id::default());

        assert_eq!(
            id.to_string(),
            before,
            "resolving must not change what the client sees"
        );
    }

    #[test]
    fn identity_ignores_the_internal_value() {
        let id = Id::default();
        let clone = id.clone();
        clone.set_mapping_id(&Id::default());

        assert!(id == clone, "resolving must not change session identity");
        assert_eq!(hash_of(&id), hash_of(&clone));

        // An unresolved id parsed from the same text is still the same id.
        let parsed: Id = id.to_string().parse().unwrap();
        assert!(id == parsed);
        assert_eq!(hash_of(&id), hash_of(&parsed));
    }

    #[test]
    fn the_internal_value_cannot_arrive_from_outside() {
        let resolved = Id::default();
        resolved.set_mapping_id(&Id::default());
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
    fn serialization_matches_the_previous_wire_form() {
        let id = Id::default();
        assert_eq!(
            serde_json::to_string(&id).unwrap(),
            serde_json::to_string(id.as_str().as_bytes()).unwrap()
        );
    }
}
