use std::sync::Arc;

use async_trait::async_trait;
use axum_core::extract::FromRequestParts;
use cookie::Cookie;
use http::{request::Parts, StatusCode};
use tower_cookies::Cookies;

use crate::{Id, Inner, Session};

/// Axum Extractor for [`Session`].
#[async_trait]
impl<S> FromRequestParts<S> for Session
where
    S: Sync + Send,
{
    type Rejection = (StatusCode, &'static str);

    #[tracing::instrument(name = "session", skip(parts, _state))]
    async fn from_request_parts(parts: &mut Parts, _state: &S) -> Result<Self, Self::Rejection> {
        let inner_session = parts
            .extensions
            .get_mut::<Arc<Inner>>()
            .cloned()
            .ok_or_else(|| {
                tracing::error!("session layer not found in the request extensions");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "session layer not found in the request extensions",
                )
            })?;

        // Cookies are only used if the SessionLayer has a cookie_options set.
        // Hence, there is no overhead incurred if the SessionLayer support other variants e.g. url sessions.
        let cookie_options = inner_session.cookie_options.clone();
        if let Some(cookie_options) = cookie_options.as_ref() {
            let cookies_ext = parts.extensions.get::<Cookies>().cloned().ok_or_else(|| {
                tracing::error!("cookies not found in the request extensions");
                (
                    StatusCode::INTERNAL_SERVER_ERROR,
                    "Cookies not found in the request extensions",
                )
            })?;

            let mut cookies = inner_session.cookies.lock();
            *cookies = Some(cookies_ext);

            if let Some(cookie) = cookies
                .clone()
                .unwrap()
                .get(cookie_options.name)
                .map(Cookie::into_owned)
            {
                let session_id = cookie
                    .clone()
                    .value()
                    .parse::<Id>()
                    .map_err(|err| {
                        tracing::warn!(
                            err = %err,
                            "possibly suspicious activity: malformed session id"
                        )
                    })
                    .ok();
                let mut id = inner_session.id.lock();
                *id = session_id;
            }
        } else {
            tracing::error!("missing cookie options");
            return Err((StatusCode::INTERNAL_SERVER_ERROR, "missing cookie options"));
        }

        Ok(Session::new(inner_session.clone()))
    }
}
