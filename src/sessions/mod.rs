use http::{Request, Response};
use tower::{Layer, Service};
use tower_cookies::{Cookie, Cookies};

mod session;
pub use session::{CookieOptions, Id, Session};

use std::future::Future;
use std::pin::Pin;
use std::task::{ready, Context, Poll};

use pin_project_lite::pin_project;

/// Middleware to use [`Session`].
#[derive(Clone, Debug)]
pub struct SessionService<S> {
    inner: S,
    /// Session name in cookie. Defaults to `id`.
    cookie_options: Option<CookieOptions>,
}

impl<S> SessionService<S> {
    /// Create a new session manager.
    pub fn new(inner: S) -> Self {
        Self {
            inner,
            cookie_options: None,
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
        let mut session = Session::default();
        let cookie_options = self.cookie_options;
        let mut cookies = None;

        if let Some(cookie_options) = cookie_options.clone() {
            cookies = req.extensions().get::<Cookies>().cloned();

            if let Some(cookies) = cookies.clone() {
                if let Some(cookie) = cookies.get(cookie_options.name).map(Cookie::into_owned) {
                    let session_id = cookie.clone().value().parse::<Id>().ok();

                    if session_id.is_some() {
                        session = Session::new(session_id, Some(cookie_options));
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
pub struct SessionLayer {
    cookie_options: Option<CookieOptions>,
}

impl SessionLayer {
    /// Create a new session manager layer.
    pub fn new() -> Self {
        Self {
            cookie_options: None,
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
        let service = SessionService::new(inner);

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
                        .http_only(cookie_options.http_only);

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
