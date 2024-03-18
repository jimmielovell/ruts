use http::{Request, Response};
use parking_lot::Mutex;
use session::Inner;
use tower::{Layer, Service};
use tower_cookies::{Cookie, Cookies};

mod session;
pub use session::{CookieOptions, Session};

mod id;
pub use id::Id;

pub mod store;
pub use store::*;

pub mod extract;

use cookie::time::Duration;
use std::future::Future;
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use pin_project_lite::pin_project;

/// Middleware to use [`Session`].
#[derive(Clone, Debug)]
pub struct SessionService<S> {
    inner: S,
    /// Session name in cookie. Defaults to `id`.
    cookie_options: Option<CookieOptions>,
    /// Session store.
    store: Arc<RedisStore>,
}

impl<S> SessionService<S> {
    /// Create a new session manager.
    pub fn new(inner: S, store: Arc<RedisStore>) -> Self {
        Self {
            inner,
            cookie_options: None,
            store,
        }
    }

    /// Set the cookie options for the session manager.
    pub fn with_cookie_options(mut self, cookie_options: CookieOptions) -> Self {
        self.cookie_options = Some(cookie_options);
        self
    }
}

impl<ReqBody, ResBody, S> Service<Request<ReqBody>> for SessionService<S>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
{
    type Response = S::Response;
    type Error = S::Error;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(&mut self, cx: &mut Context<'_>) -> Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let inner_session = Inner {
            id: Arc::new(Mutex::new(None)),
            cookies: Arc::new(Mutex::new(None)),
            cookie_options: Arc::new(self.cookie_options),
            store: Arc::clone(&self.store),
            changed: Arc::new(AtomicBool::new(false)),
            deleted: Arc::new(AtomicBool::new(false)),
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
#[derive(Clone, Debug)]
pub struct SessionLayer {
    cookie_options: Option<CookieOptions>,
    store: Arc<RedisStore>,
}

impl SessionLayer {
    /// Create a new session manager layer.
    pub fn new(store: Arc<RedisStore>) -> Self {
        Self {
            cookie_options: None,
            store,
        }
    }

    pub fn with_cookie_options(mut self, options: CookieOptions) -> Self {
        self.cookie_options = Some(options);
        self
    }
}

impl<S> Layer<S> for SessionLayer {
    type Service = SessionService<S>;

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
  pub struct ResponseFuture<F> {
    #[pin]
    future: F,
    inner_session: Arc<Inner>,
  }
}

impl<F, Body, E> Future for ResponseFuture<F>
where
    F: Future<Output = Result<Response<Body>, E>>,
{
    type Output = F::Output;

    fn poll(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<Self::Output> {
        let this = self.project();
        let res = ready!(this.future.poll(cx)?);
        let inner_session = this.inner_session;

        if inner_session.deleted.load(Ordering::Relaxed) {
            let cookie_options = inner_session.cookie_options.clone();
            if let Some(cookie_options) = cookie_options.as_ref() {
                unimplemented!()
            }
        } else if inner_session.changed.load(Ordering::Relaxed) {
            let cookie_options = inner_session.cookie_options.clone();
             if let Some(cookie_options) = cookie_options.as_ref() {
                 let cookies = inner_session.cookies.lock().clone().unwrap();
                build_cookie(inner_session.id.lock().unwrap(), cookie_options, cookies);
            }
        }

        Poll::Ready(Ok(res))
    }
}

fn build_cookie(id: Id, cookie_options: &CookieOptions, cookies: Cookies) {
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
