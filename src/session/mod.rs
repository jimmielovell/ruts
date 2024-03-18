use std::{result, sync::Arc};

use std::sync::atomic::{AtomicBool, Ordering};

use cookie::SameSite;
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};

use thiserror::Error;
use tower_cookies::Cookies;

mod id;
pub use id::Id;

use crate::redis::{RedisStore, RedisStoreError};

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] RedisStoreError),
    #[error("Session not initialized")]
    UnInitialized,
}

type Result<T> = result::Result<T, Error>;

/// A parsed on-demand session store.
#[derive(Debug)]
pub struct Session {
    inner: Arc<Inner>,
}

impl Session {
    pub fn new(inner: Arc<Inner>) -> Self {
        Self { inner }
    }

    pub async fn insert<T>(&self, field: &str, value: T) -> Result<bool>
        where
            T: Send + Sync + Serialize,
    {
        let id = self.id_or_gen();
        let cookie_options = self.inner.cookie_options.unwrap();
        let inserted = self
            .inner
            .store
            .insert(id.unwrap(), field, &value, cookie_options.max_age)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to save session to store");
                err
            })?;
        self.changed();

        Ok(inserted)
    }

    pub async fn get<T>(&self, field: &str) -> Result<Option<T>>
        where
            T: Clone + Send + Sync + DeserializeOwned,
    {
        let id = self.id();
        if id.is_none() {
            return Err(Error::UnInitialized);
        }

        let value = self
            .inner
            .store
            .get(id.unwrap(), field)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to load session from store");
                err
            })?;

        Ok(value)
    }

    pub async fn update<T>(&self, field: &str, value: T) -> Result<bool>
        where
            T: Send + Sync + Serialize,
    {
        let id = self.id();
        if id.is_none() {
            return Err(Error::UnInitialized);
        }

        let updated = self
            .inner
            .store
            .update(id.unwrap(), field, &value)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to update session to store");
                err
            })?;

        Ok(updated)
    }

    pub async fn remove(&self, field: &str) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            return Err(Error::UnInitialized);
        }

        let removed = self
            .inner
            .store
            .remove(id.unwrap(), field)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to remove session from store");
                err
            })?;

        Ok(removed)
    }

    pub async fn delete(&self) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            return Err(Error::UnInitialized);
        }

        let deleted = self.inner.store.delete(id.unwrap()).await.map_err(|err| {
            tracing::error!(err = %err, "failed to delete session from store");
            err
        })?;
        self.deleted();

        Ok(deleted)
    }

    pub async fn expire(&self, seconds: i64) -> Result<bool> {
        if seconds == -1 || seconds == 0 {
            return self.delete().await;
        }

        let id = self.id();
        if id.is_none() {
            return Err(Error::UnInitialized);
        }

        let mut expired = self
            .inner
            .store
            .expire(id.unwrap(), seconds)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to expire session");
                err
            })?;

        if expired {
            // TODO: Update this instance cookieoptions
            self.regenerate().await?;
        }

        Ok(true)
    }

    pub async fn regenerate(&self) -> Result<Option<Id>> {
        let mut id = self.inner.id.lock();

        let new_id = Id::default();
        let old_id = id.clone().unwrap();
        let renamed = self.inner.store.update_key(old_id, new_id).await?;

        if renamed {
            *id = Some(new_id);
            self.changed();
            return Ok(Some(new_id.clone()));
        }

        Ok(None)
    }

    pub fn id(&self) -> Option<Id> {
        self.inner.id.lock().clone()
    }

    pub fn id_or_gen(&self) -> Option<Id> {
        let mut id = self.inner.id.lock();
        if id.is_none() {
            *id = Some(Id::default());
        }

        id.clone()
    }

    fn changed(&self) {
        self.inner.changed.store(true, Ordering::Relaxed);
    }

    fn deleted(&self) {
        self.inner.deleted.store(true, Ordering::Relaxed);
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
    pub max_age: i64,
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
            max_age: 10 * 60,
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

    pub fn max_age(mut self, seconds: i64) -> Self {
        self.max_age = seconds;
        self
    }
}

#[derive(Debug)]
pub struct Inner {
    pub id: Arc<Mutex<Option<Id>>>,
    // data: Arc<DashMap<String, serde_json::Value>>,
    // set when a new value is inserted or removed
    pub changed: Arc<AtomicBool>,
    // set when the session is deleted
    pub deleted: Arc<AtomicBool>,
    pub cookie_options: Arc<Option<CookieOptions>>,
    pub cookies: Arc<Mutex<Option<Cookies>>>,
    pub store: Arc<RedisStore>,
}
