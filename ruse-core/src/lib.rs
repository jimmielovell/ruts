use http::{Request, Response};
use tower::{Layer, Service};
use tower_cookies::{Cookie, Cookies};

mod session;
pub use session::{CookieOptions, Id, Session};

mod store;
pub use store::*;

pub mod extract;

use std::future::Future;
use std::pin::Pin;
use std::sync::Arc;
use std::task::{ready, Context, Poll};

use pin_project_lite::pin_project;

/// Middleware to use [`Session`].
#[derive(Clone, Debug)]
pub struct SessionService<S, Store: SessionStore> {
    inner: S,
    /// Session name in cookie. Defaults to `id`.
    cookie_options: Option<CookieOptions>,
    /// Session store.
    store: Arc<Store>,
}

impl<S, Store> SessionService<S, Store>
where
    Store: SessionStore,
{
    /// Create a new session manager.
    pub fn new(inner: S, store: Arc<Store>) -> Self {
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

impl<ReqBody, ResBody, S, Store> Service<Request<ReqBody>> for SessionService<S, Store>
where
    S: Service<Request<ReqBody>, Response = Response<ResBody>>,
    Store: SessionStore,
{
    type Error = S::Error;
    type Response = S::Response;
    type Future = ResponseFuture<S::Future>;

    fn poll_ready(
        &mut self,
        cx: &mut std::task::Context<'_>,
    ) -> std::task::Poll<Result<(), Self::Error>> {
        self.inner.poll_ready(cx)
    }

    fn call(&mut self, mut req: Request<ReqBody>) -> Self::Future {
        let mut session = Session::new(self.store.clone(), None, None);
        let cookie_options = self.cookie_options;
        let mut cookies = None;

        if let Some(cookie_options) = cookie_options.clone() {
            cookies = req.extensions().get::<Cookies>().cloned();

            if let Some(cookies) = cookies.clone() {
                if let Some(cookie) = cookies.get(cookie_options.name).map(Cookie::into_owned) {
                    let session_id = cookie.clone().value().parse::<Id>().ok();

                    if session_id.is_some() {
                        session =
                            Session::new(self.store.clone(), session_id, Some(cookie_options));
                    }
                } else {
                    session.set_cookie_options(cookie_options);
                }
            }
        }

        req.extensions_mut().insert(session.clone());

        ResponseFuture {
            future: self.inner.call(req),
            session,
            cookies,
        }
    }
}

/// Layer to apply [`SessionService`] middleware.
#[derive(Clone, Debug, Default)]
pub struct SessionLayer<Store: SessionStore> {
    cookie_options: Option<CookieOptions>,
    store: Arc<Store>,
}

impl<Store> SessionLayer<Store>
where
    Store: SessionStore,
{
    /// Create a new session manager layer.
    pub fn new(store: Store) -> Self {
        Self {
            cookie_options: None,
            store: Arc::new(store),
        }
    }

    pub fn with_cookie_options(mut self, options: CookieOptions) -> Self {
        self.cookie_options = Some(options);
        self
    }
}

impl<S, Store> Layer<S> for SessionLayer<Store>
where
    Store: SessionStore,
{
    type Service = SessionService<S, Store>;

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
    session: Session,
    cookies: Option<Cookies>,
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

        let session = this.session;

        if session.is_changed() {
            if let Some(cookie_options) = session.cookie_options() {
                let id = Id::new();
                if let Some(cookies) = this.cookies.as_mut() {
                    let cookie_builder = Cookie::build((cookie_options.name, id.to_string()))
                        .secure(cookie_options.secure)
                        .http_only(cookie_options.http_only)
                        .same_site(cookie_options.same_site)
                        .expires(cookie_options.expires);

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
            }
        }

        Poll::Ready(Ok(res))
    }
}
