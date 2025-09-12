//! Session management for web applications.

use parking_lot::RwLock;
use serde::{Serialize, de::DeserializeOwned};
use std::sync::OnceLock;
use std::sync::atomic::{AtomicI64, AtomicU8, Ordering};
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
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use serde::Deserialize;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// #[derive(Clone, Deserialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
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

    /// Inserts a value into the session store under the given field.
    ///
    /// The behavior of `field_ttl_secs` determines how this field affects session persistence:
    ///
    /// - **-1**: Marks this field as persistent. The session key itself will also be persisted,
    ///   making the associated cookie persistent. This does **not** alter the TTL of other fields
    ///   in the session.
    /// - **0**: Removes this field from the store. The session behaves as if `remove` was called
    ///   on this field.
    /// - **> 0**: Sets a TTL (in seconds) for this field. The session TTL is updated according to:
    ///   - If the session key is already persistent, its TTL remains unchanged.
    ///   - If the field TTL is less than the current session TTL, the session TTL remains unchanged.
    ///   - If the field TTL is greater than the current session TTL, the session TTL is updated
    ///     to match the field TTL.
    ///
    /// Returns `true` if the field-value pair was successfully inserted or updated, and `false` if
    /// the operation resulted in deletion (e.g., TTL = 0 for a non-existent session).
    ///
    /// # Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use serde::Serialize;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// async fn example_handler(session: Session<MemoryStore>) {
    ///     let user = User { id: 34895634, name: "John Doe".to_string() };
    ///     // Insert the field with a TTL of 5 seconds
    ///     session.insert("app", &user, Some(5)).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(
        name = "session-store: inserting field-value",
        skip(self, field, value, field_ttl_secs)
    )]
    pub async fn insert<T>(
        &self,
        field: &str,
        value: &T,
        field_ttl_secs: Option<i64>,
    ) -> Result<bool>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let current_id = self.inner.get_or_set_id();
        let pending_id = self.inner.take_pending_id();

        let key_ttl_secs = self.max_age();
        if let Some(ttl) = field_ttl_secs
            && ttl > key_ttl_secs
        {
            Some(ttl)
        } else {
            Some(key_ttl_secs)
        };

        let max_age = match pending_id {
            Some(new_id) => {
                let max_age = self.inner
                    .store
                    .insert_with_rename(&current_id, &new_id, field, value, Some(key_ttl_secs), field_ttl_secs)
                    .await
                    .map_err(|err| {
                        tracing::error!(err = %err, "failed to insert field-value with rename to session store");
                        err
                    })?;
                if max_age > -2 {
                    *self.inner.id.write() = Some(new_id);
                }
                max_age
            }
            None => self
                .inner
                .store
                .insert(
                    &current_id,
                    field,
                    value,
                    Some(key_ttl_secs),
                    field_ttl_secs,
                )
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to insert field-value to session store");
                    err
                })?,
        };

        if max_age > -2 {
            self.inner.set_changed();
            self.set_expiration(max_age);
        }

        Ok(max_age > -2)
    }

    /// Updates a value in the session store.
    ///
    /// If the key doesn't exist, it will be inserted.
    ///
    /// - **-1**: Marks this field as persistent. The session key itself will also be persisted,
    ///   making the associated cookie persistent. This does **not** alter the TTL of other fields
    ///   in the session.
    /// - **0**: Removes this field from the store. The session behaves as if `remove` was called
    ///   on this field.
    /// - **> 0**: Sets a TTL (in seconds) for this field. The session TTL is updated according to:
    ///   - If the session key is already persistent, its TTL remains unchanged.
    ///   - If the field TTL is less than the current session TTL, the session TTL remains unchanged.
    ///   - If the field TTL is greater than the current session TTL, the session TTL is updated
    ///     to match the field TTL.
    ///
    /// Returns `true` if the field-value pair was successfully inserted or updated, and `false` if
    /// the operation resulted in deletion (e.g., TTL = 0 for a non-existent session).
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use serde::Serialize;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// #[derive(Serialize)]
    /// struct User {
    ///     id: i64,
    ///     name: String,
    /// }
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
    ///     let user = User {id: 21342365, name: String::from("Jane Doe")};
    ///
    ///     let updated = session.update("app", &user, Some(5)).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(
        name = "session-store: updating field",
        skip(self, field, value, field_ttl_secs)
    )]
    pub async fn update<T>(
        &self,
        field: &str,
        value: &T,
        field_ttl_secs: Option<i64>,
    ) -> Result<bool>
    where
        T: Send + Sync + Serialize + 'static,
    {
        let current_id = self.inner.get_or_set_id();
        let pending_id = self.inner.take_pending_id();

        let key_ttl_secs = self.max_age();
        if let Some(ttl) = field_ttl_secs
            && ttl > key_ttl_secs
        {
            Some(ttl)
        } else {
            Some(key_ttl_secs)
        };

        let max_age = match pending_id {
            Some(new_id) => {
                let max_age = self.inner
                    .store
                    .update_with_rename(&current_id, &new_id, field, value, Some(key_ttl_secs), field_ttl_secs)
                    .await
                    .map_err(|err| {
                        tracing::error!(err = %err, "failed to update field-value with rename in session store");
                        err
                    })?;

                if max_age > -2 {
                    *self.inner.id.write() = Some(new_id);
                }
                max_age
            }
            None => self
                .inner
                .store
                .update(
                    &current_id,
                    field,
                    value,
                    Some(key_ttl_secs),
                    field_ttl_secs,
                )
                .await
                .map_err(|err| {
                    tracing::error!(err = %err, "failed to update field in session store");
                    err
                })?,
        };

        if max_age > -2 {
            self.inner.set_changed();
            self.set_expiration(max_age);
        }
        Ok(max_age > -2)
    }

    /// Removes a field along with its value from the session store.
    ///
    /// Returns `true` if the field was successfully removed.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
    ///     let removed = session.remove("user").await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "session-store: removing field", skip(self, field))]
    pub async fn remove(&self, field: &str) -> Result<bool> {
        let id = self.id();
        if id.is_none() {
            tracing::error!("session not initialized");
            return Err(Error::UnInitialized);
        }

        let max_age = self
            .inner
            .store
            .remove(&id.unwrap(), field)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to remove field from session store");
                err
            })?;

        if max_age == -2 {
            self.inner.set_deleted();
        } else if max_age > -2 {
            self.inner.set_changed();
            self.set_expiration(max_age);
        }

        Ok(max_age > -2)
    }

    /// Deletes the entire session from the store.
    ///
    /// Returns `true` if the session was successfully deleted.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
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
    /// - A value of -1 persists the session.
    /// - A value of 0 immediately expires the session and deletes it.
    ///
    /// Returns `true` if the expiry was successfully updated.
    ///
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
    ///     session.expire(30).await.unwrap();
    /// }
    /// ```
    #[tracing::instrument(name = "updating session expiry", skip(self, ttl_secs))]
    pub async fn expire(&self, ttl_secs: i64) -> Result<bool> {
        if ttl_secs == -1 || ttl_secs == 0 {
            return self.delete().await;
        }

        let id = self.id();
        if id.is_none() {
            tracing::error!("session not initialized");
            return Err(Error::UnInitialized);
        }

        self.set_expiration(ttl_secs);
        let expired = self
            .inner
            .store
            .expire(&id.unwrap(), ttl_secs)
            .await
            .map_err(|err| {
                tracing::error!(err = %err, "failed to update session expiry");
                err
            })?;

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
    /// ## Example
    ///
    /// ```rust
    /// use ruts::{Session};
    /// use fred::clients::Client;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
    ///     let id = session.regenerate().await.unwrap();
    /// }
    /// ```
    ///
    /// **Note**: This does not renew the session expiry.
    #[tracing::instrument(name = "regenerating session id", skip(self))]
    pub async fn regenerate(&self) -> Result<Option<Id>> {
        let old_id = self.id();
        let new_id = Id::default();
        let renamed = self
            .inner
            .store
            .rename_session_id(&old_id.unwrap(), &new_id)
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
    /// ## Example
    ///
    /// ```rust,no_run
    /// use ruts::Session;
    /// use fred::clients::Client;
    /// use ruts::store::memory::MemoryStore;
    ///
    /// async fn some_handler_could_be_axum(session: Session<MemoryStore>) {
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
            cookie_max_age: AtomicI64::new(cookie_max_age.unwrap_or(-1)),
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

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::Arc;
    use serde::Deserialize;
    use crate::store::memory::MemoryStore;

    #[derive(Debug, Serialize, Deserialize, PartialEq, Clone)]
    struct TestUser {
        pub id: i64,
        pub name: String,
    }

    fn create_test_user() -> TestUser {
        TestUser {
            id: 1,
            name: "Test User".to_string(),
        }
    }

    fn create_inner<S: SessionStore>(
        store: Arc<S>,
        cookie_name: Option<&'static str>,
        cookie_max_age: Option<i64>,
    ) -> Arc<Inner<S>> {
        Arc::new(Inner::new(store, cookie_name, cookie_max_age))
    }

    #[tokio::test]
    async fn test_session_basic_operations() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_user();

        // Test initial state
        let initial_get: Option<TestUser> = session.get("test").await.unwrap();
        assert!(initial_get.is_none());

        // Test insert
        let inserted = session.insert("test", &test_data, None).await.unwrap();
        assert!(inserted);

        // Test get after insert
        let retrieved: Option<TestUser> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), test_data);

        // Test update
        let mut updated_data = test_data.clone();
        updated_data.name = "Updated User".to_string();
        let updated = session.update("test", &updated_data, None).await.unwrap();
        assert!(updated);

        // Verify update
        let retrieved: Option<TestUser> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), updated_data);

        // Test delete
        let deleted = session.delete().await.unwrap();
        assert!(deleted);

        // Verify deletion
        let retrieved: Option<TestUser> = session.get("test").await.unwrap();
        assert!(retrieved.is_none());
    }

    #[tokio::test]
    async fn test_prepare_regenerate_with_update() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store.clone(), Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_user();

        // Insert initial data
        session.insert("test", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // Prepare new ID and update
        let prepared_id = session.prepare_regenerate();
        let mut updated_data = test_data.clone();
        updated_data.name = "Updated User".to_string();
        let updated = session.update("test", &updated_data, None).await.unwrap();
        assert!(updated);

        // Verify ID changed and data updated
        let current_id = session.id().unwrap();
        assert_eq!(current_id, prepared_id);
        assert_ne!(current_id, original_id);

        let retrieved: Option<TestUser> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), updated_data);

        // Verify old session is gone by directly checking the store
        let result: Option<TestUser> = store.get(&original_id, "test").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_prepare_regenerate_with_insert() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store.clone(), Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_user();

        // Insert initial data
        session.insert("test1", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // Prepare new ID and insert new field
        let prepared_id = session.prepare_regenerate();
        let mut new_data = test_data.clone();
        new_data.name = "New User".to_string();
        let inserted = session.insert("test2", &new_data, None).await.unwrap();
        assert!(inserted);

        // Verify ID changed and both fields exist
        let current_id = session.id().unwrap();
        assert_eq!(current_id, prepared_id);
        assert_ne!(current_id, original_id);

        let retrieved1: Option<TestUser> = session.get("test1").await.unwrap();
        let retrieved2: Option<TestUser> = session.get("test2").await.unwrap();
        assert_eq!(retrieved1.unwrap(), test_data);
        assert_eq!(retrieved2.unwrap(), new_data);

        // Verify old session is gone by directly checking the store
        let result: Option<TestUser> = store.get(&original_id, "test1").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_multiple_prepare_regenerate() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store.clone(), Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_user();

        // Insert initial data
        session.insert("test", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // First prepare_regenerate
        let first_prepared_id = session.prepare_regenerate();

        // Second prepare_regenerate before any operation
        let second_prepared_id = session.prepare_regenerate();
        assert_ne!(first_prepared_id, second_prepared_id);

        // Update - should use the last prepared ID
        let mut updated_data = test_data.clone();
        updated_data.name = "Updated User".to_string();
        session.update("test", &updated_data, None).await.unwrap();

        // Verify the last prepared ID was used
        let current_id = session.id().unwrap();
        assert_eq!(current_id, second_prepared_id);
        assert_ne!(current_id, first_prepared_id);
        assert_ne!(current_id, original_id);

        // Verify original session is gone by directly checking the store
        let result: Option<TestUser> = store.get(&original_id, "test").await.unwrap();
        assert!(result.is_none());
    }

    #[tokio::test]
    async fn test_session_regeneration() {
        let store = Arc::new(MemoryStore::new());
        let inner = create_inner(store, Some("test_sess"), Some(3600));
        let session = Session::new(inner);
        let test_data = create_test_user();

        // Insert initial data
        session.insert("test", &test_data, None).await.unwrap();
        let original_id = session.id().unwrap();

        // Regenerate session
        let new_id = session.regenerate().await.unwrap();
        assert!(new_id.is_some());
        assert_ne!(original_id, new_id.unwrap());

        // Verify data persistence
        let retrieved: Option<TestUser> = session.get("test").await.unwrap();
        assert_eq!(retrieved.unwrap(), test_data);
    }
}
