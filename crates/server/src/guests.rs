use crate::{ApiError, App};
use axum::{
    extract::{MatchedPath, Request, State},
    http::{Method, StatusCode, header},
    middleware::Next,
    response::{IntoResponse, Response},
};
use sha2::{Digest, Sha256};
use uuid::Uuid;

const COOKIE: &str = "__Host-deep_guest";

#[derive(Clone)]
pub(crate) struct Guest(pub Option<String>);

fn token(request: &Request) -> Option<String> {
    let mut found = None;
    for header in request.headers().get_all(header::COOKIE) {
        for cookie in header.to_str().ok()?.split(';') {
            let Some((name, value)) = cookie.trim().split_once('=') else {
                continue;
            };
            if name == COOKIE {
                if found.is_some()
                    || value.len() != 64
                    || !value.bytes().all(|byte| byte.is_ascii_hexdigit())
                {
                    return None;
                }
                found = Some(value.to_owned());
            }
        }
    }
    found
}

fn resource(path: &str) -> Option<&'static str> {
    match path {
        "/api/workspaces/{id}"
        | "/api/workspaces/{id}/branch"
        | "/api/workspaces/{id}/messages/{run_id}"
        | "/api/workspaces/{id}/runs"
        | "/api/workspaces/{id}/assets"
        | "/api/workspaces/{id}/asset-proposals"
        | "/api/workspaces/{id}/proposals"
        | "/api/workspaces/{id}/image-assets"
        | "/api/workspaces/{id}/project-export"
        | "/api/workspaces/{id}/executions" => Some("workspaces"),
        "/api/runs/{id}" => Some("runs"),
        "/api/assets/{id}" => Some("assets"),
        "/api/image-assets/{id}" | "/api/image-assets/{id}/content" => Some("image_assets"),
        "/api/image-jobs/{id}" | "/api/image-jobs/{id}/reconcile" => Some("image_jobs"),
        "/api/executions/{id}" | "/api/executions/{id}/proposal" => Some("executions"),
        "/api/proposals/{id}" | "/api/proposals/{id}/apply" | "/api/proposals/{id}/reject" => {
            Some("proposals")
        }
        _ => None,
    }
}

async fn authorize(app: &App, request: &mut Request) -> Result<Option<String>, ApiError> {
    if app.config.public_origin.is_none() {
        request.extensions_mut().insert(Guest(None));
        return Ok(None);
    }
    let api = request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    if !api {
        return Ok(None);
    }
    // Lax cookies accompany top-level GET navigation. Do not let another site
    // invoke an API operation through a link, including worker readiness/recovery.
    if request
        .headers()
        .get("sec-fetch-site")
        .is_some_and(|site| site != "same-origin" && site != "none")
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Open Deep directly to use the demo.".into(),
        ));
    }
    let path = request
        .extensions()
        .get::<MatchedPath>()
        .map(|path| path.as_str())
        .unwrap_or("")
        .to_owned();
    if path == "/api/worker"
        && request
            .headers()
            .get("x-deep-request")
            .is_none_or(|value| value != "1")
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Worker checks require an application request.".into(),
        ));
    }
    if request.method() != Method::GET
        && request.method() != Method::HEAD
        && !request.headers().contains_key(header::ORIGIN)
    {
        return Err(ApiError(
            StatusCode::FORBIDDEN,
            "Demo actions require the matching browser Origin.".into(),
        ));
    }
    let bootstrap = path == "/api/session" && request.method() == Method::POST;
    let mut guest = None;
    if let Some(token) = token(request) {
        let id = format!("{:x}", Sha256::digest(token.as_bytes()));
        if app.store.touch_guest(id.clone()).await? {
            guest = Some(id);
        }
    }
    let mut cookie = None;
    if guest.is_none() && bootstrap {
        let token = format!("{}{}", Uuid::new_v4().simple(), Uuid::new_v4().simple());
        let id = format!("{:x}", Sha256::digest(token.as_bytes()));
        app.store.create_guest(id.clone()).await?;
        guest = Some(id);
        cookie = Some(format!(
            "{COOKIE}={token}; Path=/; HttpOnly; Secure; SameSite=Lax; Max-Age=2592000"
        ));
    }
    let guest = guest.ok_or_else(|| {
        ApiError(
            StatusCode::UNAUTHORIZED,
            "This browser session expired. Reload Deep to start a new guest session.".into(),
        )
    })?;
    match path.as_str() {
        "/api/session" | "/api/status" | "/api/worker" | "/api/workspaces" => {}
        _ => {
            let table = resource(&path)
                .ok_or_else(|| ApiError(StatusCode::NOT_FOUND, "API resource not found.".into()))?;
            let id = request.uri().path().split('/').nth(3).unwrap_or("");
            if !app
                .store
                .guest_owns(guest.clone(), table, id.into())
                .await?
            {
                return Err(ApiError(
                    StatusCode::NOT_FOUND,
                    "API resource not found.".into(),
                ));
            }
        }
    }
    request.extensions_mut().insert(Guest(Some(guest)));
    Ok(cookie)
}

pub(crate) async fn require_guest(
    State(app): State<App>,
    mut request: Request,
    next: Next,
) -> Response {
    let private = request.uri().path() == "/api" || request.uri().path().starts_with("/api/");
    let (cookie, mut response) = match authorize(&app, &mut request).await {
        Ok(cookie) => (cookie, next.run(request).await),
        Err(error) => (None, error.into_response()),
    };
    if app.config.public_origin.is_some() && private {
        response
            .headers_mut()
            .insert(header::CACHE_CONTROL, "private, no-store".parse().unwrap());
    }
    if let Some(cookie) = cookie {
        response
            .headers_mut()
            .append(header::SET_COOKIE, cookie.parse().unwrap());
    }
    response
}
