use crate::{ApiError, App};
use axum::{
    extract::{Request, State},
    http::{StatusCode, header, uri::Authority},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::net::IpAddr;

fn valid_authority(authority: &Authority) -> bool {
    if authority.as_str().contains('@') {
        return false;
    }
    let host = authority.host();
    let Some(suffix) = authority.as_str().strip_prefix(host) else {
        return false;
    };
    // Authority::port_u16 treats malformed or out-of-range ports as absent.
    if !suffix.is_empty()
        && !suffix
            .strip_prefix(':')
            .is_some_and(|port| port.parse::<u16>().is_ok())
    {
        return false;
    }
    true
}

fn loopback(authority: &Authority) -> bool {
    if !valid_authority(authority) {
        return false;
    }
    let host = authority.host();
    host.eq_ignore_ascii_case("localhost")
        || host
            .trim_matches(['[', ']'])
            .parse::<IpAddr>()
            .is_ok_and(|address| address.is_loopback())
}

fn same_authority(left: &Authority, right: &Authority, default_port: u16) -> bool {
    left.host().eq_ignore_ascii_case(right.host())
        && left.port_u16().unwrap_or(default_port) == right.port_u16().unwrap_or(default_port)
}

fn allowed(request: &Request, public: Option<&str>) -> bool {
    let public = public
        .and_then(|value| value.parse::<axum::http::Uri>().ok())
        .and_then(|uri| uri.authority().cloned());
    let accepted = |authority: &Authority| {
        valid_authority(authority)
            && (loopback(authority)
                || public
                    .as_ref()
                    .is_some_and(|configured| same_authority(authority, configured, 443)))
    };
    let mut hosts = request.headers().get_all(header::HOST).iter();
    let authority = match hosts.next() {
        Some(value) => value
            .to_str()
            .ok()
            .and_then(|host| host.parse::<Authority>().ok()),
        None => request.uri().authority().cloned(),
    };
    let Some(authority) = authority else {
        return false;
    };
    if hosts.next().is_some() || !accepted(&authority) {
        return false;
    }
    if let Some(uri_authority) = request.uri().authority()
        && (!accepted(uri_authority)
            || !same_authority(
                &authority,
                uri_authority,
                if request.uri().scheme_str() == Some("https") {
                    443
                } else {
                    80
                },
            ))
    {
        return false;
    }

    let mut origins = request.headers().get_all(header::ORIGIN).iter();
    let Some(value) = origins.next() else {
        return true;
    };
    if origins.next().is_some() {
        return false;
    }
    let Some(origin) = value
        .to_str()
        .ok()
        .and_then(|value| value.parse::<axum::http::Uri>().ok())
    else {
        return false;
    };
    let default_port = match origin.scheme_str() {
        Some("http") => 80,
        Some("https") => 443,
        _ => return false,
    };
    if origin
        .path_and_query()
        .is_some_and(|path| path.as_str() != "/")
    {
        return false;
    }
    origin.authority().is_some_and(|origin_authority| {
        valid_authority(origin_authority)
            && ((loopback(origin_authority)
                && same_authority(&authority, origin_authority, default_port))
                || (default_port == 443
                    && public.as_ref().is_some_and(|configured| {
                        same_authority(origin_authority, configured, 443)
                    })))
    })
}

pub(crate) async fn require_origin(
    State(app): State<App>,
    request: Request,
    next: Next,
) -> Response {
    if !allowed(&request, app.config.public_origin.as_deref()) {
        return ApiError(
            StatusCode::FORBIDDEN,
            if app.config.public_origin.is_some() {
                "Only configured hosts and matching browser origins are allowed."
            } else {
                "Only loopback hosts and matching browser origins are allowed."
            }
            .into(),
        )
        .into_response();
    }
    next.run(request).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn demo_origin_is_exact_and_does_not_trust_forwarding_headers() {
        let origin = Some("https://demo.friend.ts.net");
        for (host, source, expected) in [
            ("demo.friend.ts.net", "https://demo.friend.ts.net", true),
            ("127.0.0.1:4320", "https://demo.friend.ts.net", true),
            ("localhost:5173", "http://localhost:5173", true),
            ("evil.friend.ts.net", "https://demo.friend.ts.net", false),
            ("demo.friend.ts.net", "https://other.friend.ts.net", false),
            (
                "demo.friend.ts.net:443",
                "http://demo.friend.ts.net:443",
                false,
            ),
            (
                "demo.friend.ts.net:bad",
                "https://demo.friend.ts.net",
                false,
            ),
            (
                "demo.friend.ts.net:70000",
                "https://demo.friend.ts.net",
                false,
            ),
            (
                "demo.friend.ts.net",
                "https://demo.friend.ts.net/path",
                false,
            ),
            ("demo.friend.ts.net", "null", false),
        ] {
            let request = Request::builder()
                .uri("/api/status")
                .header("Host", host)
                .header("Origin", source)
                .header("X-Forwarded-Host", "demo.friend.ts.net")
                .body(axum::body::Body::empty())
                .unwrap();
            assert_eq!(allowed(&request, origin), expected, "{host} {source}");
        }
    }
}
