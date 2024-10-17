use std::sync::Arc;

use async_trait::async_trait;
use axum_core::extract::FromRequestParts;
use http::{request::Parts, StatusCode};
use tower_cookies::Cookies;

use crate::session::Inner;
use crate::store::SessionStore;
use crate::{Id, Session};

/// axum extractor for [`Session`].
#[async_trait]
impl<S, T> FromRequestParts<S> for Session<T>
where
    S: Sync + Send,
    T: SessionStore,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let inner_session = parts.extensions.get::<Arc<Inner<T>>>().ok_or_else(|| {
            tracing::error!("session layer not found in the request extensions");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "session not found in the request",
            )
        })?;

        // Cookies are only used if the SessionLayer has a cookie_options set.
        let cookie_options = &inner_session.cookie_options.ok_or_else(|| {
            tracing::error!("missing cookie options");
            (StatusCode::INTERNAL_SERVER_ERROR, "missing cookie options")
        })?;

        let cookies_ext = parts.extensions.get::<Cookies>().ok_or_else(|| {
            tracing::error!("cookies not found in the request extensions");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "cookies not found in the request",
            )
        })?;

        let mut cookies = inner_session.cookies.lock();
        *cookies = Some(cookies_ext.to_owned());

        if let Some(cookie) = cookies.as_ref().unwrap().get(cookie_options.name) {
            let session_id = cookie
                .value()
                .parse::<Id>()
                .map_err(|err| {
                    tracing::warn!(
                        err = %err,
                        "malformed session id"
                    )
                })
                .ok();
            *inner_session.id.write() = session_id;
        }

        Ok(Session::new(inner_session.clone()))
    }
}
