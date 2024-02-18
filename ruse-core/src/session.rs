use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};

use cookie::SameSite;
use dashmap::DashMap;
use parking_lot::RwLock;

use crate::{Id, InnerSessionUtil};

/// A parsed on-demand session store.
#[derive(Debug)]
pub struct Session<T: Clone + Send + Sync> {
    inner: Inner<T>,
    utils: Arc<InnerSessionUtil>,
}

impl<T> Session<T>
where
    T: Clone + Default + Send + Sync,
{
    pub fn new(utils: Arc<InnerSessionUtil>, id: Option<Id>) -> Self {
        let inner = Inner {
            id: Arc::new(RwLock::new(id)),
            data: Arc::new(DashMap::new()),
            ..Default::default()
        };
        Self { utils, inner }
    }

    pub fn insert(&self, key: &str, value: T) {
        self.inner.data().insert(key.to_owned(), value);
        self.inner.changed.store(true, Ordering::Relaxed);
    }

    pub fn get(&self, key: &str) -> Option<T> {
        let value = self.inner.data().get(key).map(|v| v.value().to_owned());
        value
    }

    pub fn remove(&self, key: &str) {
        self.inner.data().remove(key);
        self.inner.changed.store(true, Ordering::Relaxed);
    }

    pub fn delete(&self) {
        self.inner.data.clear();
        self.inner.changed.store(true, Ordering::Relaxed);
    }

    pub fn is_changed(&self) -> bool {
        self.inner.changed.load(Ordering::Relaxed)
    }

    pub fn set_id(&self, id: Id) {
        let mut _id = self.inner.id.write();
        *_id = Some(id);
    }

    pub fn id(&self) -> Option<Id> {
        self.inner.id.read().clone()
    }

    pub fn cookie_options(&self) -> Option<CookieOptions> {
        self.utils.cookie_options
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
struct Inner<T> {
    id: Arc<RwLock<Option<Id>>>,
    data: Arc<DashMap<String, T>>,
    changed: AtomicBool,
}

impl<T> Inner<T> {
    fn data(&self) -> &DashMap<String, T> {
        if self.data.is_empty() {
            // let data = DashMap::new();
            // Load data from session store
            let id = self.id.read().clone();
            if let Some(id) = id {
                // Load data from session store
            }
            // self.data = Some(data);
        }
        &self.data
    }
}
