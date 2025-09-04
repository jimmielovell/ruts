//! Session management for web applications.

use parking_lot::RwLock;
use serde::{de::DeserializeOwned, Serialize};
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
use std::sync::OnceLock;
use std::{result, sync::Arc};

use thiserror::Error;
use tower_cookies::Cookies;

mod cookie_options;
mod id;

use crate::store;
use crate::store::{SessionMap, SessionStore};
pub use cookie_options::CookieOptions;
pub use id::Id;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error("Session has not been initialized")]
    UnInitialized,
}

type Result<T> = result::Result<T, Error>;

/// A parsed on-demand session store.
#[derive(Clone, Debug)]
pub struct Session<S: SessionStore> {
    inner: Arc<Inner<S>>,
}

impl<S> Session<S>
where
    S: SessionStore,
{
    /// Creates a new `Session` instance.
    pub fn new(inner: Arc<Inner<S>>) -> Self {
        Self { inner }
    }

    /// Retrieves the value of a field from the session store.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use serde::Deserialize;
    /// use ruts::store::redis::RedisStore;
    ///
    /// #[derive(Clone, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[derive(Clone, Deserialize)]
    /// enum Theme {
    ///     Light,
    ///     Dark,
    /// }
    ///
    /// #[derive(Clone, Deserialize)]
    /// struct AppSession {
    ///     user: User,
    ///     theme: Option<Theme>,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     session.get::<AppSession>("app").await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: getting value for field", skip(self, field))]
    pub async fn get<T>(&self, field: &str) -> Result<Option<T>>
    where
        T: Clone + Send + Sync + DeserializeOwned,
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

    //// Retrieves all fields from the session store as a `SessionMap`.
    ///
    /// This method performs one bulk query to the store and returns a wrapper
    /// that allows for lazy, on-demand deserialization of each field.
    #[tracing::instrument(
        name = "session-store: getting values for all fields for session id",
        skip(self)
    )]
    pub async fn get_all(&self) -> Result<Option<SessionMap>> {
        match self.id() {
            Some(id) => self
                .inner
                .store
                .get_all(&id)
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to get all values from session store");
                    err.into()
                }),
            None => {
                tracing::debug!("session has not been initialized");
                Ok(None)
            }
        }
    }

    /// Inserts a value into the session store.
    ///
    /// Returns `true` if the value was successfully inserted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use serde::Serialize;
    /// use ruts::store::redis::RedisStore;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[derive(Serialize)]
    /// enum Theme {
    ///     Light,
    ///     Dark,
    /// }
    ///
    /// #[derive(Serialize)]
    /// struct AppSession {
    ///     user: User,
    ///     theme: Option<Theme>,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     let app = AppSession {
    ///             user: User {
    ///             id: 34895634,
    ///             name: String::from("John Doe"),
    ///         },
    ///         theme: Some(Theme::Dark),
    ///     };
    ///
    ///     session.insert("app", &app, Some(5)).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(
        name = "session-store: inserting field-value",
        skip(self, field, value, field_expire)
    )]
    pub async fn insert<T>(&self, field: &str, value: &T, field_expire: Option<i64>) -> Result<bool>
    where
        T: Send + Sync + Serialize,
    {
        let current_id = self.inner.get_or_set_id();
        let pending_id = self.inner.take_pending_id();

        let inserted = match pending_id {
            Some(new_id) => {
                let inserted = self.inner
                    .store
                    .insert_with_rename(&current_id, &new_id, field, value, self.max_age(), field_expire)
                    .await
                    .map_err(|err| {
                        tracing::error!(err = %err, "failed to insert field-value with rename to session store");
                        err
                    })?;
                if inserted {
                    *self.inner.id.write() = Some(new_id);
                }
                inserted
            }
            None => self
                .inner
                .store
                .insert(&current_id, field, value, self.max_age(), field_expire)
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to insert field-value to session store");
                    err
                })?,
        };

        if inserted {
            self.inner.set_changed();
        }

        Ok(inserted)
    }

    /// Updates a value in the session store.
    ///
    /// If the key doesn't exist, it will be inserted.
    ///
    /// Returns `true` if the value was successfully updated or inserted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use serde::Serialize;
    /// use ruts::store::redis::RedisStore;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[derive(Serialize)]
    /// enum Theme {
    ///     Light,
    ///     Dark,
    /// }
    ///
    /// #[derive(Serialize)]
    /// struct AppSession {
    ///     user: User,
    ///     theme: Option<Theme>,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     let app = AppSession {
    ///         user: User {
    ///             id: 21342365,
    ///             name: String::from("Jane Doe"),
    ///         },
    ///         theme: Some(Theme::Light),
    ///     };
    ///
    ///     let updated = session.update("app", &app, Some(5)).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(
        name = "session-store: updating field",
        skip(self, field, value, field_expire)
    )]
    pub async fn update<T>(&self, field: &str, value: &T, field_expire: Option<i64>) -> Result<bool>
    where
        T: Send + Sync + Serialize,
    {
        let current_id = self.inner.get_or_set_id();
        let pending_id = self.inner.take_pending_id();

        let updated = match pending_id {
            Some(new_id) => {
                let updated = self.inner
                    .store
                    .update_with_rename(&current_id, &new_id, field, value, self.max_age(), field_expire)
                    .await
                    .map_err(|err| {
                        tracing::error!(err = %err, "failed to update field-value with rename in session store");
                        err
                    })?;

                if updated {
                    *self.inner.id.write() = Some(new_id);
                }

                updated
            }
            None => self
                .inner
                .store
                .update(&current_id, field, value, self.max_age(), field_expire)
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to update field in session store");
                    err
                })?,
        };

        if updated {
            self.inner.set_changed();
        }

        Ok(updated)
    }

    /// Removes a field along with its value from the session store.
    ///
    /// Returns `true` if the field was successfully removed.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::redis::RedisStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     let removed = session.remove("app").await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: removing field", skip(self, field))]
    pub async fn remove(&self, field: &str) -> Result<i8> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("session not initialized");
            return Err(Error::UnInitialized);
        }

        let removed = self
            .inner
            .store
            .remove(&id.unwrap(), field)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to remove field from session store");
                err
            })?;

        Ok(removed)
    }

    /// Deletes the entire session from the store.
    ///
    /// Returns `true` if the session was successfully deleted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::redis::RedisStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     let deleted = session.delete().await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: deleting session", skip(self))]
    pub async fn delete(&self) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("session not initialized");
            return Err(Error::UnInitialized);
        }

        let deleted = self.inner.store.delete(&id.unwrap()).await.map_err(|err| {
            tracing::error!(err = %err, "failed to delete session from store");
            err
        })?;

        if deleted {
            self.inner.set_deleted();
        }

        Ok(deleted)
    }

    /// Updates the cookie's max-age and session expiry time in the store.
    ///
    /// A value of -1 or 0 immediately expires the session and deletes it.
    ///
    /// Returns `true` if the expiry was successfully updated.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::redis::RedisStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     session.expire(30).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "updating session expiry", skip(self, seconds))]
    pub async fn expire(&self, seconds: i64) -> Result<bool> {
        if seconds == -1 || seconds == 0 {
            return self.delete().await;
        }

        let id = self.id();
        if id.is_none() {
            tracing::error!("session not initialized");
            return Err(Error::UnInitialized);
        }

        self.set_expiration(seconds);
        let expired = self
            .regenerate()
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to update session expiry");
                err
            })?
            .is_some();

        if expired {
            self.inner.set_changed();
        }

        Ok(expired)
    }

    /// Updates the cookie max-age.
    ///
    /// Any subsequent call to `insert`, `update` or `regenerate` within this request cycle
    /// will use this value.
    pub fn set_expiration(&self, seconds: i64) {
        self.inner.cookie_max_age.store(seconds, Ordering::SeqCst);
    }

    /// Regenerates the session with a new ID.
    ///
    /// Returns the new session ID if successful.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::redis::RedisStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     let id = session.regenerate().await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "regenerating session id", skip(self))]
    pub async fn regenerate(&self) -> Result<Option<Id>> {
        let old_id = self.id();
        let new_id = Id::default();
        let renamed = self
            .inner
            .store
            .rename_session_id(&old_id.unwrap(), &new_id, self.max_age())
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
    /// The new ID will be used to rename the current session (if it exists) when the next
    /// insert or update operation is performed.
    ///
    /// # Example
    ///
    /// ```rust
    /// use ruts::Session;
    /// use fred::clients::Client;
    /// use ruts::store::redis::RedisStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<RedisStore<Client>>) {
    ///     let new_id = session.prepare_regenerate();
    ///     // The next update/insert operation will use this new ID
    ///     session.update("field", &"value", None).await.unwrap();
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

    fn max_age(&self) -> i64 {
        self.inner.cookie_max_age.load(Ordering::SeqCst)
    }
}

const SESSION_STATE_CHANGED: u8 = 0b01;
const SESSION_STATE_DELETED: u8 = 0b10;
const DEFAULT_COOKIE_MAX_AGE: i64 = 10 * 60;

#[derive(Debug)]
pub struct Inner<T: SessionStore> {
    pub state: AtomicU8,
    pub id: RwLock<Option<Id>>,
    pub pending_id: RwLock<Option<Id>>,
    pub cookie_max_age: AtomicI64,
    pub cookie_name: Option<&'static str>,
    pub cookies: OnceLock<Cookies>,
    pub store: Arc<T>,
}

impl<T: SessionStore> Inner<T> {
    pub fn new(
        store: Arc<T>,
        cookie_name: Option<&'static str>,
        cookie_max_age: Option<i64>,
    ) -> Self {
        Self {
            state: AtomicU8::new(0),
            id: RwLock::new(None),
            pending_id: RwLock::new(None),
            cookie_max_age: AtomicI64::new(cookie_max_age.unwrap_or(DEFAULT_COOKIE_MAX_AGE)),
            cookie_name,
            cookies: OnceLock::new(),
            store,
        }
    }

    pub fn is_changed(&self) -> bool {
        self.state.load(Ordering::SeqCst) & SESSION_STATE_CHANGED != 0
    }

    pub fn is_deleted(&self) -> bool {
        self.state.load(Ordering::SeqCst) & SESSION_STATE_DELETED != 0
    }

    pub fn get_id(&self) -> Option<Id> {
        *self.id.read()
    }

    pub fn get_or_set_id(&self) -> Id {
        *self.id.write().get_or_insert(Id::default())
    }

    pub fn set_id(&self, id: Option<Id>) {
        *self.id.write() = id;
    }

    pub fn set_pending_id(&self, id: Option<Id>) {
        *self.pending_id.write() = id;
    }

    pub fn take_pending_id(&self) -> Option<Id> {
        self.pending_id.write().take()
    }

    pub fn set_changed(&self) {
        self.state.fetch_or(SESSION_STATE_CHANGED, Ordering::SeqCst);
    }

    pub fn set_deleted(&self) {
        self.state.fetch_or(SESSION_STATE_DELETED, Ordering::SeqCst);
    }

    pub fn get_cookies(&self) -> Option<&Cookies> {
        self.cookies.get()
    }

    pub fn set_cookies_if_empty(&self, cookies: Cookies) -> bool {
        self.cookies.set(cookies).is_ok()
    }
}
