use std::sync::Arc;

use axum_core::extract::FromRequestParts;
use http::{request::Parts, StatusCode};
use tower_cookies::Cookies;

use crate::session::Inner;
use crate::store::SessionStore;
use crate::{Id, Session};

/// axum extractor for [`Session`].
impl<S, T> FromRequestParts<S> for Session<T>
where
    S: Sync + Send,
    T: SessionStore,
{
    type Rejection = (StatusCode, &'static str);

    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let session_inner = parts.extensions.get::<Arc<Inner<T>>>().ok_or_else(|| {
            tracing::error!("session layer not found in the request extensions");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Session not found in the request",
            )
        })?;

        // Cookies are only used if the SessionLayer has a cookie_options set.
        let cookie_name = session_inner.cookie_name.ok_or_else(|| {
            (StatusCode::INTERNAL_SERVER_ERROR, "Missing cookie options")
        })?;

        let cookies_ext = parts.extensions.get::<Cookies>().ok_or_else(|| {
            tracing::error!("cookies not found in the request extensions");
            (
                StatusCode::INTERNAL_SERVER_ERROR,
                "Cookies not found in the request",
            )
        })?;

        session_inner.set_cookies_if_empty(cookies_ext.to_owned());

        if let Some(cookie) = cookies_ext.get(cookie_name) {
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
            session_inner.set_id(session_id);
        }

        Ok(Session::new(session_inner.clone()))
    }
}
