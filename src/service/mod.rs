//! Session management middleware for tower applications.
//!
//! This module provides [`SessionLayer`] for integrating
//! session management into tower applications.

use http::{Request, Response};
use parking_lot::{Mutex, RwLock};
use tower::{Layer, Service};
use tower_cookies::{Cookie, Cookies};

use crate::store::redis::RedisStore;
use crate::store::SessionStore;
use crate::{session::Inner, CookieOptions, Id};
use cookie::time::Duration;
use pin_project_lite::pin_project;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll};

/// A Tower Middleware to use [`Session`].
#[derive(Clone, Debug)]
pub struct SessionService<S, T: SessionStore = RedisStore> {
    inner: S,
    cookie_options: Option<CookieOptions>,
    store: Arc<T>,
}

impl<S, T> SessionService<S, T>
where
    T: SessionStore,
{
    fn new(inner: S, store: Arc<T>) -> Self {
        Self {
            inner,
            cookie_options: None,
            store,
        }
    }

    fn with_cookie_options(mut self, cookie_options: CookieOptions) -> Self {
        self.cookie_options = Some(cookie_options);
        self
    }
}

impl<ReqBody, ResBody, S, T> Service<Request<ReqBody>> for SessionService<S, T>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    T: SessionStore,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future, T>;

    #[inline]
    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let inner_session = Inner {
            id: RwLock::new(None),
            cookies: Mutex::new(None),
            cookie_options: Arc::new(self.cookie_options),
            store: Arc::clone(&self.store),
            changed: AtomicBool::new(false),
            deleted: AtomicBool::new(false),
        };
        let inner_session = Arc::new(inner_session);
        req.extensions_mut().insert(inner_session.clone());

        ResponseFuture {
            future: self.inner.call(req),
            inner_session,
        }
    }
}

/// Layer to apply [`SessionService`] middleware.
///
/// # Example
///
/// ```rust
/// use fred::clients::RedisClient;
/// use ruts::{CookieOptions, Session, SessionLayer};
/// use ruts::store::redis::RedisStore;
/// use std::sync::Arc;
/// use tower_cookies::CookieManagerLayer;
///
/// let cookie_options = CookieOptions::build()
///         .name("test_sess")
///         .http_only(true)
///         .same_site(cookie::SameSite::Lax)
///         .secure(true)
///         .max_age(1 * 60)
///         .path("/");
///
///     let client = RedisClient::default();
///     // Initialize the client
///
///     let store = RedisStore::new(Arc::new(client));
///     let session_layer = SessionLayer::new(Arc::new(store))
///         .with_cookie_options(cookie_options);
/// ```
///
#[derive(Clone, Debug)]
pub struct SessionLayer<T: SessionStore> {
    cookie_options: Option<CookieOptions>,
    store: Arc<T>,
}

impl<T> SessionLayer<T>
where
    T: SessionStore,
{
    /// Create a new session manager layer.
    pub fn new(store: Arc<T>) -> Self {
        Self {
            cookie_options: None,
            store,
        }
    }

    /// Set the cookie options for the session manager.
    pub fn with_cookie_options(mut self, options: CookieOptions) -> Self {
        self.cookie_options = Some(options);
        self
    }
}

impl<S, T> Layer<S> for SessionLayer<T>
where
    T: SessionStore,
{
    type Service = SessionService<S, T>;

    fn layer(&self, inner: S) -> Self::Service {
        let service = SessionService::new(inner, self.store.clone());

        if let Some(cookie_options) = self.cookie_options {
            service.with_cookie_options(cookie_options)
        } else {
            service
        }
    }
}

pin_project! {
  /// Response future for SessionManager
  #[derive(Debug)]
  pub struct ResponseFuture<F, T: SessionStore> {
    #[pin]
    future: F,
    inner_session: Arc<Inner<T>>,
  }
}

impl<F, Body, E, T> Future for ResponseFuture<F, T>
where
    F: Future<Output = Result<Response<Body>, E>>,
    T: SessionStore,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let res = ready!(this.future.poll(cx)?);
        let inner_session = this.inner_session;

        if inner_session.deleted.load(Ordering::Relaxed) {
            if let Some(cookie_options) = &*inner_session.cookie_options {
                if let Some(cookies) = inner_session.cookies.lock().as_ref() {
                    let cookie = Cookie::build(cookie_options.name);
                    cookies.remove(cookie.build());
                }
            }
        } else if inner_session.changed.load(Ordering::Relaxed) {
            if let Some(cookie_options) = &*inner_session.cookie_options {
                if let Some(cookies) = inner_session.cookies.lock().as_ref() {
                    if let Some(id) = inner_session.id.read().as_ref() {
                        build_cookie(id, cookie_options, cookies);
                    }
                }
            }
        }

        Poll::Ready(Ok(res))
    }
}

fn build_cookie(id: &Id, cookie_options: &CookieOptions, cookies: &Cookies) {
    let cookie_builder = Cookie::build((cookie_options.name, id.to_string()))
        .secure(cookie_options.secure)
        .http_only(cookie_options.http_only)
        .same_site(cookie_options.same_site)
        .max_age(Duration::seconds(cookie_options.max_age));

    let cookie_builder = if let Some(domain) = cookie_options.domain {
        cookie_builder.domain(domain)
    } else {
        cookie_builder
    };

    let cookie_builder = if let Some(path) = cookie_options.path {
        cookie_builder.path(path)
    } else {
        cookie_builder
    };

    cookies.add(cookie_builder.build());
}
