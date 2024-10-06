//! Session management for web applications.

use std::{result, sync::Arc};

use std::sync::atomic::{AtomicBool, Ordering};

use cookie::SameSite;
use parking_lot::{Mutex, RwLock};
use serde::{de::DeserializeOwned, Serialize};

use thiserror::Error;
use tower_cookies::Cookies;

mod id;
use crate::store;
use crate::store::redis::RedisStore;
use crate::store::SessionStore;
pub use id::Id;

#[derive(Error, Debug)]
pub enum Error {
    #[error(transparent)]
    Store(#[from] store::Error),
    #[error("session has not been initialized")]
    UnInitialized,
}

type Result<T> = result::Result<T, Error>;

/// A parsed on-demand session store.
///
/// The default store is the [`RedisStore`<RedisPool>]
#[derive(Debug)]
pub struct Session<S: SessionStore = RedisStore> {
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

    /// Returns the cookie options for this session.
    pub fn cookie_options(&self) -> Option<CookieOptions> {
        *self.inner.cookie_options.clone()
    }

    /// Deletes the entire session from the store.
    ///
    /// Returns `true` if the session was successfully deleted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use ruts::store::redis::RedisStore;
    ///
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/delete", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         session.delete().await.unwrap();
    ///     }));
    /// ```
    #[tracing::instrument(name = "deleting session from store", skip(self))]
    pub async fn delete(&self) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        let no_of_deleted_keys = self.inner.store.delete(&id.unwrap()).await.map_err(|err| {
            tracing::error!(err = %err, "failed to delete session from store");
            err
        })?;

        if no_of_deleted_keys == 1 {
            self.deleted();
        }

        Ok(true)
    }

    /// Updates the session expiry time.
    ///
    /// A value of -1 or 0 immediately expires the session and deletes it.
    ///
    /// Returns `true` if the expiry was successfully updated.
    ///
    /// # Example
    ///
    /// ```rust
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use ruts::store::redis::RedisStore;
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/expire", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         session.expire(30).await.unwrap();
    ///     }));
    /// ```
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
            .expire(&id.unwrap(), seconds)
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

    /// Retrieves a value from the session store.
    ///
    /// # Example
    ///
    /// ```rust
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use serde::{Deserialize, Serialize};
    /// use ruts::store::redis::RedisStore;
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// enum Theme {
    ///     Light,
    ///     #[default]
    ///     Dark,
    /// }
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// struct AppSession {
    ///     user: Option<User>,
    ///     theme: Option<Theme>,
    /// }
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/get", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         session.get::<AppSession>("app").await.unwrap();
    ///     }));
    /// ```
    #[tracing::instrument(name = "getting value for session-key from store", skip(self, key))]
    pub async fn get<T>(&self, key: &str) -> Result<Option<T>>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        match self.id() {
            Some(id) => self
                .inner
                .store
                .get(&id, key)
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to get session from store");
                    err.into()
                }),
            None => {
                tracing::debug!("session not initialized");
                Ok(None)
            }
        }
    }

    /// Retrieves all values from the session store.
    #[tracing::instrument(name = "getting session from store", skip(self))]
    pub async fn get_all<T>(&self) -> Result<Option<T>>
    where
        T: Clone + Send + Sync + DeserializeOwned,
    {
        match self.id() {
            Some(id) => self
                .inner
                .store
                .get_all(&id)
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to get session from store");
                    err.into()
                }),
            None => {
                tracing::debug!("session not initialized");
                Ok(None)
            }
        }
    }

    /// Returns the session ID, if it exists.
    pub fn id(&self) -> Option<Id> {
        *self.inner.id.read()
    }

    /// Inserts a value into the session store.
    ///
    /// Returns `true` if the value was successfully inserted.
    ///
    /// # Example
    ///
    /// ```rust
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use serde::{Deserialize, Serialize};
    /// use ruts::store::redis::RedisStore;
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// enum Theme {
    ///     Light,
    ///     #[default]
    ///     Dark,
    /// }
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// struct AppSession {
    ///     user: Option<User>,
    ///     theme: Option<Theme>,
    /// }
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/set", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         let app = AppSession {
    ///             user: Some(User {
    ///                 id: 34895634,
    ///                 name: String::from("John Doe"),
    ///             }),
    ///             theme: Some(Theme::Dark),
    ///         };
    ///
    ///         session.insert("app", app).await.unwrap();
    ///     }));
    /// ```
    #[tracing::instrument(name = "inserting session to store", skip(self, key, value))]
    pub async fn insert<T>(&self, key: &str, value: T) -> Result<bool>
    where
        T: Send + Sync + Serialize,
    {
        let id = self.id_or_gen();
        let inserted = self
            .inner
            .store
            .insert(&id, key, &value, self.max_age())
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

    /// Regenerates the session with a new ID.
    ///
    /// Returns the new session ID if successful.
    ///
    /// # Example
    ///
    /// ```rust
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use ruts::store::redis::RedisStore;
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/regenerate", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         let id = session.regenerate().await.unwrap();
    ///     }));
    /// ```
    #[tracing::instrument(name = "regenerating the session id", skip(self))]
    pub async fn regenerate(&self) -> Result<Option<Id>> {
        let old_id = self.id();
        let new_id = Id::default();
        let renamed = self
            .inner
            .store
            .update_key(&old_id.unwrap(), &new_id, self.max_age())
            .await?;

        if renamed {
            *self.inner.id.write() = Some(new_id);
            self.changed();
            return Ok(Some(new_id));
        }

        Ok(None)
    }

    /// Removes a key-value pair from the session store.
    ///
    /// Returns `true` if the pair was successfully removed.
    ///
    /// # Example
    ///
    /// ```rust
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use ruts::store::redis::RedisStore;
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/remove", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         session.remove("app").await.unwrap();
    ///     }));
    /// ```
    #[tracing::instrument(name = "removing session-key from store", skip(self, key))]
    pub async fn remove(&self, key: &str) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("the session has not been initialized");
            return Err(Error::UnInitialized);
        }

        let removed = self
            .inner
            .store
            .remove(&id.unwrap(), key)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to remove key from session store");
                err
            })?;

        Ok(removed)
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
    /// use axum::{Router, routing::get};
    /// use ruts::{Session};
    /// use fred::clients::RedisClient;
    /// use serde::{Deserialize, Serialize};
    /// use ruts::store::redis::RedisStore;
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// enum Theme {
    ///     Light,
    ///     #[default]
    ///     Dark,
    /// }
    ///
    /// #[derive(Clone, Debug, Default, Serialize, Deserialize)]
    /// struct AppSession {
    ///     user: Option<User>,
    ///     theme: Option<Theme>,
    /// }
    ///
    /// let _: Router<()> = Router::new()
    ///     .route("/update", get(|session: Session<RedisStore<RedisClient>>| async move {
    ///         let app = AppSession {
    ///             user: Some(User {
    ///                 id: 21342365,
    ///                 name: String::from("Jane Doe"),
    ///             }),
    ///             theme: Some(Theme::Light),
    ///         };
    ///
    ///         session.update("app-2", app).await.unwrap();
    ///     }));
    /// ```
    #[tracing::instrument(name = "updating session in store", skip(self, key, value))]
    pub async fn update<T>(&self, key: &str, value: T) -> Result<bool>
    where
        T: Send + Sync + Serialize,
    {
        let id = self.id_or_gen();
        let updated = self
            .inner
            .store
            .update(&id, key, &value, self.max_age())
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

    fn id_or_gen(&self) -> Id {
        let mut id_guard = self.inner.id.write();
        let id = id_guard.get_or_insert(Id::default());
        *id
    }

    fn max_age(&self) -> i64 {
        self.inner
            .cookie_options
            .as_ref()
            .as_ref()
            .map(|options| options.max_age)
            .unwrap_or(0)
    }
}

/// Configuration options for session cookies.
///
/// # Example
///
/// ```rust
/// use ruts::CookieOptions;
///
/// let cookie_options = CookieOptions::build()
///         .name("test_sess")
///         .http_only(true)
///         .same_site(cookie::SameSite::Lax)
///         .secure(true)
///         .max_age(1 * 60)
///         .path("/");
/// ```
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
    /// Creates a new `CookieOptions` with default values.
    pub fn build() -> Self {
        Self::default()
    }

    /// Sets the name of the cookie.
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
    pub id: RwLock<Option<Id>>,
    // set when a new value is inserted or removed
    pub changed: AtomicBool,
    // set when the session is deleted
    pub deleted: AtomicBool,
    pub cookie_options: Arc<Option<CookieOptions>>,
    pub cookies: Mutex<Option<Cookies>>,
    pub store: Arc<T>,
}
