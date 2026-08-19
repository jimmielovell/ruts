//! Session management for web applications.

mod cookie_options;
pub use cookie_options::CookieOptions;

mod id;
pub use id::Id;

use crate::store;
use crate::store::{SessionMap, SessionStore, Ttl};
use parking_lot::RwLock;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicU8, Ordering};
use std::{result, sync::Arc};

use thiserror::Error;
use tower_cookies::Cookies;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error("Session has not been initialized")]
    UnInitialized,
}

type Result<T> = result::Result<T, Error>;

/// A parsed on-demand session store.
#[derive(Clone)]
pub struct Session<S: SessionStore> {
    inner: Arc<Inner<S>>,
}

impl<S> Session<S>
where
    S: SessionStore,
{
    /// Creates a new `Session` instance.
    pub(crate) fn new(inner: Arc<Inner<S>>) -> Self {
        Self { inner }
    }

    /// Retrieves the value of a field from the session store.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use serde::Deserialize;
    /// use ruts::store::moka::MokaStore;
    ///
    /// #[derive(Clone, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     session.get::<User>("user").await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: getting value for field", skip(self, field))]
    pub async fn get<T>(&self, field: &str) -> Result<Option<T>>
    where
        T: Send + Sync + DeserializeOwned,
    {
        match self.id() {
            Some(id) => self.inner.store.get(&id, field).await.map_err(|err| {
                tracing::error!(err = %err, "failed to get value for field from session store");
                err.into()
            }),
            None => {
                tracing::debug!("session not initialized");
                Ok(None)
            }
        }
    }

    /// Retrieves all fields from the session store as a `SessionMap`.
    ///
    /// This method performs one bulk query to the store and returns a wrapper
    /// that allows for lazy, on-demand deserialization of each field.
    #[tracing::instrument(
        name = "session-store: getting values for all fields for session id",
        skip(self)
    )]
    pub async fn get_all(&self) -> Result<Option<SessionMap>> {
        match self.id() {
            Some(id) => self.inner.store.get_all(&id).await.map_err(|err| {
                tracing::error!(err = %err, "failed to get all values from session store");
                err.into()
            }),
            None => {
                tracing::debug!("session has not been initialized");
                Ok(None)
            }
        }
    }

    /// Sets a value in the session store.
    ///
    /// If the field doesn't exist, it will be inserted. Requires a strictly
    /// positive `field_ttl_secs`.
    ///
    /// This does **not** change the cookie's `Max-Age`: the cookie lifetime is
    /// owned by [`CookieOptions::max_age`] and only changed explicitly via
    /// [`Session::set_expiration`]. A field's TTL is its own concern.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use serde::Serialize;
    /// use ruts::store::moka::MokaStore;
    /// use ruts::store::Ttl;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     let user = User {id: 21342365, name: String::from("Jane Doe")};
    ///
    ///     session.set("app", &user, Ttl::new(3600).unwrap(), None).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(
        name = "session-store: updating field",
        skip(self, field, value, field_ttl, hot_cache_ttl)
    )]
    pub async fn set<T>(
        &self,
        field: &str,
        value: &T,
        field_ttl: Ttl,
        #[cfg(feature = "layered-store")] hot_cache_ttl: Option<Ttl>,
        #[cfg(not(feature = "layered-store"))] hot_cache_ttl: Option<std::marker::PhantomData<()>>,
    ) -> Result<()>
    where
        T: Send + Sync + Serialize,
    {
        let current_id = self.inner.get_or_set_id();
        let pending_id = self.inner.take_pending_id();

        match pending_id {
            Some(new_id) => {
                self.inner
                    .store
                    .set_and_rename(&current_id, &new_id, field, value, field_ttl, hot_cache_ttl)
                    .await
                    .map_err(|err| {
                        tracing::error!(
                            err = %err,
                            "failed to update field-value with rename in session store"
                        );
                        err
                    })?;

                *self.inner.id.write() = Some(new_id);
            }
            None => {
                self.inner
                    .store
                    .set(&current_id, field, value, field_ttl, hot_cache_ttl)
                    .await
                    .map_err(|err| {
                        tracing::error!(err = %err, "failed to update field in session store");
                        err
                    })?;
            }
        };

        self.inner.set_changed();

        Ok(())
    }

    /// Removes a field along with its value from the session store.
    ///
    /// If this was the last live field, the session ceases to exist at the
    /// store. The session cookie is reissued (the server remains authoritative;
    /// a presented id with no live fields is simply treated as a fresh session).
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use ruts::store::moka::MokaStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     session.remove("user").await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: removing field", skip(self, field))]
    pub async fn remove(&self, field: &str) -> Result<()> {
        let id = self.id().ok_or_else(|| {
            tracing::error!("session not initialized");
            Error::UnInitialized
        })?;

        self.inner.store.remove(&id, field).await.map_err(|err| {
            tracing::error!(err = %err, "failed to remove field from session store");
            err
        })?;

        self.inner.set_changed();

        Ok(())
    }

    /// Deletes the entire session from the store.
    ///
    /// Returns `true` if the session was successfully deleted. The middleware
    /// emits a clearing cookie (`Max-Age=0`) on the response.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use ruts::store::moka::MokaStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     let deleted = session.delete().await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: deleting session", skip(self))]
    pub async fn delete(&self) -> Result<bool> {
        let id = self.id().ok_or_else(|| {
            tracing::error!("session not initialized");
            Error::UnInitialized
        })?;

        let deleted = self.inner.store.delete(&id).await.map_err(|err| {
            tracing::error!(err = %err, "failed to delete session from store");
            err
        })?;

        if deleted {
            self.inner.set_deleted();
        }
        Ok(deleted)
    }

    /// Extends the TTL of a specific `field` belonging to the session.
    ///
    /// Returns `true` if the field existed and was active, `false` if it was
    /// missing or expired. Requires a strictly positive `ttl_secs`.
    ///
    /// This re-TTLs the named field only; it does not touch other fields and
    /// does not change the cookie's `Max-Age`. On success the cookie is
    /// reissued at the configured lifetime.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use ruts::store::Ttl;
    /// use ruts::store::moka::MokaStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     session.expire_field("user", Ttl::new(3600).unwrap()).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "updating field expiry", skip(self, ttl))]
    pub async fn expire_field(&self, field: &str, ttl: Ttl) -> Result<bool> {
        let id = self.id().ok_or_else(|| {
            tracing::error!("session not initialized");
            Error::UnInitialized
        })?;

        let expired = self
            .inner
            .store
            .expire_field(&id, field, ttl)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to update field expiry");
                err
            })?;

        if expired {
            self.inner.set_changed();
        }

        Ok(expired)
    }

    /// Overrides the cookie's `Max-Age` for this request cycle.
    ///
    /// The response cookie built by the middleware will use this value instead
    /// of [`CookieOptions::max_age`]. This is the only way a field operation's
    /// caller influences cookie lifetime — it is never derived implicitly.
    pub fn set_expiration(&self, seconds: u64) {
        *self.inner.cookie_max_age.write() = Some(seconds);
    }

    /// Regenerates the session with a new ID.
    ///
    /// Returns the new session ID if successful.
    ///
    /// ## Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use ruts::store::moka::MokaStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     let id = session.regenerate().await.unwrap();
    /// }
    /// ```
    ///
    /// **Note**: This does not renew any field's expiry.
    #[tracing::instrument(name = "regenerating session id", skip(self))]
    pub async fn regenerate(&self) -> Result<Option<Id>> {
        let old_id = self.id().ok_or_else(|| {
            tracing::error!("session not initialized");
            Error::UnInitialized
        })?;

        let new_id = Id::default();
        let renamed = self
            .inner
            .store
            .rename_session_id(&old_id, &new_id)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to regenerate session id: {err:?}");
                err
            })?;

        if renamed {
            *self.inner.id.write() = Some(new_id);
            self.inner.set_changed();
            return Ok(Some(new_id));
        }

        Ok(None)
    }

    /// Prepares a new session ID to be used in the next store operation.
    /// The new ID will be used to rename the current session (if it exists) when
    /// the next set operation is performed.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::Session;
    /// use ruts::store::Ttl;
    /// use ruts::store::moka::MokaStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MokaStore>) {
    ///     let new_id = session.prepare_regenerate();
    ///     // The next set operation will use this new ID
    ///     session.set("field", &"value", Ttl::new(3600).unwrap(), None).await.unwrap();
    /// }
    /// ```
    pub fn prepare_regenerate(&self) -> Id {
        if self.id().is_none() {
            self.inner.get_or_set_id()
        } else {
            let new_id = Id::default();
            self.inner.set_pending_id(Some(new_id));
            new_id
        }
    }

    /// Returns the session ID, if it exists.
    pub fn id(&self) -> Option<Id> {
        self.inner.get_id()
    }

    /// Returns the cookie max_age, if it exists.
    pub fn cookie_max_age(&self) -> Option<u64> {
        self.inner.get_cookie_max_age()
    }
}

const SESSION_STATE_CHANGED: u8 = 1;
const SESSION_STATE_DELETED: u8 = 2;

#[cfg(feature = "signed")]
use tower_cookies::Key;

pub(crate) struct Inner<T: SessionStore> {
    pub(crate) state: AtomicU8,
    pub(crate) id: RwLock<Option<Id>>,
    pub(crate) pending_id: RwLock<Option<Id>>,
    /// Cookie `Max-Age`: `Some(seconds)` persistent, `None` session cookie.
    pub(crate) cookie_max_age: RwLock<Option<u64>>,
    pub(crate) cookie_name: Option<&'static str>,
    pub(crate) cookies: OnceLock<Cookies>,
    pub(crate) store: Arc<T>,
    #[cfg(feature = "signed")]
    pub(crate) signing_key: Option<Arc<Key>>,
}

impl<T: SessionStore> Inner<T> {
    pub(crate) fn new(
        store: Arc<T>,
        cookie_name: Option<&'static str>,
        cookie_max_age: Option<u64>,
        #[cfg(feature = "signed")] signing_key: Option<Arc<Key>>,
    ) -> Self {
        Self {
            state: AtomicU8::new(0),
            id: RwLock::new(None),
            pending_id: RwLock::new(None),
            cookie_max_age: RwLock::new(cookie_max_age),
            cookie_name,
            cookies: OnceLock::new(),
            store,
            #[cfg(feature = "signed")]
            signing_key,
        }
    }

    pub(crate) fn is_changed(&self) -> bool {
        self.state.load(Ordering::Relaxed) == SESSION_STATE_CHANGED
    }

    pub(crate) fn is_deleted(&self) -> bool {
        self.state.load(Ordering::Relaxed) == SESSION_STATE_DELETED
    }

    pub(crate) fn get_id(&self) -> Option<Id> {
        *self.id.read()
    }

    pub(crate) fn get_or_set_id(&self) -> Id {
        *self.id.write().get_or_insert(Id::default())
    }

    pub(crate) fn set_id(&self, id: Option<Id>) {
        *self.id.write() = id;
    }

    pub(crate) fn set_pending_id(&self, id: Option<Id>) {
        *self.pending_id.write() = id;
    }

    pub(crate) fn take_pending_id(&self) -> Option<Id> {
        self.pending_id.write().take()
    }

    pub(crate) fn set_changed(&self) {
        self.state.store(SESSION_STATE_CHANGED, Ordering::Relaxed);
    }

    pub(crate) fn set_deleted(&self) {
        self.state.store(SESSION_STATE_DELETED, Ordering::Relaxed);
    }

    pub(crate) fn get_cookies(&self) -> Option<&Cookies> {
        self.cookies.get()
    }

    pub(crate) fn set_cookies_if_empty(&self, cookies: Cookies) -> bool {
        self.cookies.set(cookies).is_ok()
    }

    pub(crate) fn get_cookie_max_age(&self) -> Option<u64> {
        *self.cookie_max_age.read()
    }
}
