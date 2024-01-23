use std::{
    str::FromStr,
    sync::{atomic::AtomicBool, Arc},
};

use cookie::SameSite;
use dashmap::DashMap;
use parking_lot::Mutex;
use rand::{distributions::Standard, thread_rng, Rng};

use crate::store::SessionStore;

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
        } else {j
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
#[derive(Clone, Debug)]
pub struct Session {
    inner: Arc<Inner>,
    store: Arc<dyn SessionStore>,
}

impl Session {
    pub fn new(
        store: Arc<impl SessionStore>,
        id: Option<Id>,
        cookie_options: Option<CookieOptions>,
    ) -> Self {
        let inner = Inner {
            id,
            cookie_options,
            ..Default::default()
        };
        Self {
            inner: Arc::new(Mutex::new(inner)),
            store,
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

    /// Set the session cookie options. Mainly used internally.
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

#[derive(Clone, Copy, Debug)]
pub struct CookieOptions {
    pub http_only: bool,
    pub name: &'static str,
    pub domain: Option<&'static str>,
    pub path: Option<&'static str>,
    pub same_site: SameSite,
    pub secure: bool,
    pub expires: cookie::Expiration,
    pub max_age: Option<cookie::time::Duration>,
}

impl Default for CookieOptions {
    fn default() -> Self {
        Self {
            http_only: true,
            name: "id",
            domain: None,
            path: None,
            same_site: SameSite::Lax,
            secure: true,
            expires: cookie::Expiration::Session,
            max_age: None,
        }
    }
}

impl CookieOptions {
    pub fn build() -> Self {
        Self::default()
    }

    pub fn name(mut self, name: &'static str) -> Self {
        self.name = name;
        self
    }

    pub fn http_only(mut self, http_only: bool) -> Self {
        self.http_only = http_only;
        self
    }

    pub fn same_site(mut self, same_site: SameSite) -> Self {
        self.same_site = same_site;
        self
    }

    pub fn secure(mut self, secure: bool) -> Self {
        self.secure = secure;
        self
    }

    pub fn domain(mut self, domain: &'static str) -> Self {
        self.domain = Some(domain);
        self
    }

    pub fn path(mut self, path: &'static str) -> Self {
        self.path = Some(path);
        self
    }

    pub fn expires(mut self, expires: cookie::Expiration) -> Self {
        self.expires = expires;
        self
    }
}

#[derive(Debug, Default)]
struct Inner {
    id: Option<Id>,
    data: Option<DashMap<String, String>>,
    changed: AtomicBool,
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
