use std::{result, sync::Arc};

use std::sync::atomic::{AtomicBool, Ordering};

use cookie::SameSite;
use parking_lot::Mutex;
use serde::{de::DeserializeOwned, Serialize};

use thiserror::Error;
use tower_cookies::Cookies;

mod id;
use crate::store;
use crate::store::SessionStore;
pub use id::Id;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error("Session not initialized")]
    UnInitialized,
}

type Result<T> = result::Result<T, Error>;

/// A parsed on-demand session store.
#[derive(Debug)]
pub struct Session<S: SessionStore> {
    inner: Arc<Inner<S>>,
}

impl<S> Session<S>
where
    S: SessionStore,
{
    pub fn new(inner: Arc<Inner<S>>) -> Self {
        Self { inner }
    }

    pub fn cookie_options(&self) -> Option<CookieOptions> {
        *self.inner.cookie_options.clone()
    }

    /// Delete the entire session from the store.
    #[tracing::instrument(name = "deleting session from store", skip(self))]
    pub async fn delete(&self) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        let deleted = self.inner.store.delete(id.unwrap()).await.map_err(|err| {
            tracing::error!(err = %err, "failed to delete session from store");
            err
        })?;

        if deleted {
            self.deleted();
        }

        Ok(deleted)
    }

    /// Update the session expiry.
    /// A value of -1 or 0 immediately expires the session and is deleted.
    #[tracing::instrument(name = "updating session expiry", skip(self, seconds))]
    pub async fn expire(&self, seconds: i64) -> Result<bool> {
        if seconds == -1 || seconds == 0 {
            return self.delete().await;
        }

        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        let expired = self
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

    /// Get a value from the store for a session.
    #[tracing::instrument(name = "getting value for session-field from store", skip(self, field))]
    pub async fn get<T>(&self, field: &str) -> Result<Option<T>>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        Ok(self
            .inner
            .store
            .get(id.unwrap(), field)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to get session from store");
                err
            })?)
    }

    /// Get all values from the session store.
    #[tracing::instrument(name = "getting session from store", skip(self))]
    pub async fn get_all<T>(&self) -> Result<Option<T>>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        Ok(self.inner.store.get_all(id.unwrap()).await.map_err(|err| {
            tracing::error!(err = %err, "failed to get session from store");
            err
        })?)
    }

    /// Get the session id.
    /// The returned value will be `None` if there has not been any data insertion in this request cycle or any previous ones.
    pub fn id(&self) -> Option<Id> {
        self.inner.id.lock().clone()
    }

    /// Set a value in the store for a session field.
    #[tracing::instrument(name = "inserting session to store", skip(self, field, value))]
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

        if inserted {
            self.changed();
        }

        Ok(inserted)
    }

    /// Regenerate a new session id.
    #[tracing::instrument(name = "regenerating session-id", skip(self))]
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

    /// Remove the session field along with its value from the session.
    /// If there is not any other field remaining in the session, it is deleted.
    #[tracing::instrument(name = "removing session-field from store", skip(self, field))]
    pub async fn remove(&self, field: &str) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        let no_of_remaining_keys =
            self.inner
                .store
                .remove(id.unwrap(), field)
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to remove session-field from store");
                    err
                })?;

        if no_of_remaining_keys == -1 {
            tracing::error!("failed to remove session-field from store");
            return Ok(false);
        } else if no_of_remaining_keys == 0 {
            // Mark the session cookie deletion
            self.deleted();
        }

        Ok(true)
    }

    /// Update the session field value with the new value.
    /// This inserts the session field if it does not exist.
    #[tracing::instrument(name = "updating session in store", skip(self, field, value))]
    pub async fn update<T>(&self, field: &str, value: T) -> Result<bool>
    where
        T: Send + Sync + Serialize,
    {
        let id = self.id_or_gen();

        let updated = self
            .inner
            .store
            .update(id.unwrap(), field, &value)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to update session to store");
                err
            })?;

        if updated {
            self.changed();
        }

        Ok(updated)
    }

    fn changed(&self) {
        self.inner.changed.store(true, Ordering::Relaxed);
    }

    fn deleted(&self) {
        self.inner.deleted.store(true, Ordering::Relaxed);
    }

    fn id_or_gen(&self) -> Option<Id> {
        let mut id = self.inner.id.lock();
        if id.is_none() {
            *id = Some(Id::default());
        }

        id.clone()
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
pub struct Inner<T: SessionStore> {
    pub id: Arc<Mutex<Option<Id>>>,
    // data: Arc<DashMap<String, serde_json::Value>>,
    // set when a new value is inserted or removed
    pub changed: AtomicBool,
    // set when the session is deleted
    pub deleted: AtomicBool,
    pub cookie_options: Arc<Option<CookieOptions>>,
    pub cookies: Arc<Mutex<Option<Cookies>>>,
    pub store: Arc<T>,
}
