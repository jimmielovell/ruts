use std::{str::FromStr, sync::Arc};

use async_trait::async_trait;
use axum_core::extract::FromRequestParts;
use dashmap::DashMap;
use http::{request::Parts, StatusCode};
use parking_lot::Mutex;
use rand::{distributions::Standard, thread_rng, Rng};

/// A parsed on-demand session id.
#[derive(Clone, Debug)]
pub struct Id(String);

impl Id {
    pub fn new() -> Self {
        let id: Vec<u8> = thread_rng().sample_iter(Standard).take(128).collect();
        let hash = blake3::hash(&id);
        Self(hash.to_string())
    }

    pub fn parse(id: &str) -> Option<Self> {
        if id.len() == 128 {
            Some(Self(id.to_owned()))
        } else {
            None
        }
    }
}

impl ToString for Id {
    fn to_string(&self) -> String {
        self.0.clone()
    }
}

impl FromStr for Id {
    type Err = ();

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(Self(s.to_owned()))
    }
}

/// A parsed on-demand session store.
#[derive(Clone, Debug, Default)]
pub struct Session {
    inner: Arc<Mutex<Inner>>,
}

impl Session {
    pub fn new(id: Option<Id>, cookie_options: Option<CookieOptions>) -> Self {
        let inner = Inner {
            id,
            cookie_options,
            ..Default::default()
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
        }
    }

    pub fn insert(&self, key: &str, value: &str) {
        let mut inner = self.inner.lock();
        inner.data().insert(key.to_owned(), value.to_owned());

        inner.changed = true;
    }

    pub fn get(&self, key: &str) -> Option<String> {
        let mut inner = self.inner.lock();
        let value = inner.data().get(key).map(|v| v.value().to_owned());
        value
    }

    pub fn remove(&self, key: &str) {
        let mut inner = self.inner.lock();
        inner.data().remove(key);
        inner.changed = true;
    }

    pub fn delete(&self) {
        let mut inner = self.inner.lock();
        inner.data = None;
        inner.changed = true;
    }

    pub fn is_changed(&self) -> bool {
        let inner = self.inner.lock();
        inner.changed
    }

    pub fn id(&self) -> Option<Id> {
        let inner = self.inner.lock();
        inner.id.clone()
    }

    pub fn set_cookie_options(&self, cookie_options: CookieOptions) -> Self {
        let mut inner = self.inner.lock();
        inner.cookie_options = Some(cookie_options);
        self.clone()
    }

    pub fn cookie_options(&self) -> Option<CookieOptions> {
        let inner = self.inner.lock();
        inner.cookie_options.clone()
    }
}

#[async_trait]
impl<S> FromRequestParts<S> for Session
where
    S: Sync + Send,
{
    type Rejection = (http::StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        parts.extensions.get::<Session>().cloned().ok_or((
            StatusCode::INTERNAL_SERVER_ERROR,
            "Can't extract session. Is `SessionLayer` enabled?",
        ))
    }
}

#[derive(Clone, Copy, Debug)]
pub struct CookieOptions {
    pub name: &'static str,
    pub domain: Option<&'static str>,
    pub path: Option<&'static str>,
    pub secure: bool,
    pub http_only: bool,
}

impl Default for CookieOptions {
    fn default() -> Self {
        Self {
            name: "id",
            domain: None,
            path: None,
            secure: true,
            http_only: true,
        }
    }
}

impl CookieOptions {
    pub fn new(
        name: &'static str,
        domain: Option<&'static str>,
        path: Option<&'static str>,
        secure: bool,
        http_only: bool,
    ) -> Self {
        Self {
            name,
            domain,
            path,
            secure,
            http_only,
        }
    }
}

#[derive(Debug, Default)]
struct Inner {
    id: Option<Id>,
    data: Option<DashMap<String, String>>,
    changed: bool,
    cookie_options: Option<CookieOptions>,
}

impl Inner {
    fn data(&mut self) -> &mut DashMap<String, String> {
        if self.data.is_none() {
            let mut data = DashMap::new();
            // Load data from session store
            if let Some(id) = &self.id {
                // Load data from session store
            }
            self.data = Some(data);
        }
        self.data.as_mut().unwrap()
    }
}
