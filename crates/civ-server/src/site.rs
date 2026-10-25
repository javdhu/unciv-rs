// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! The browser client's files, served beside `/api` so the page, its guest cookie and the API share one origin,
//! with the cross-origin isolation headers wasm threads need.
use crate::HostError;
use axum::{
    body::Body,
    extract::{Request, State},
    http::{header, HeaderMap, HeaderName, HeaderValue, Method},
    middleware::Next,
    response::{IntoResponse, Response},
};
use std::{
    path::{Component, Path, PathBuf},
    sync::Arc,
};

/// Answers GET and HEAD outside `/api` and `/health` from `root`. Every other request goes on to the API
/// router, Origin check included, exactly as without a static directory.
pub(crate) async fn serve(State(root): State<Arc<PathBuf>>, request: Request, next: Next) -> Response {
    let path = request.uri().path();
    let api = path == "/api" || path.starts_with("/api/") || path == "/health";
    let head = request.method() == Method::HEAD;
    if api || !(head || request.method() == Method::GET) {
        return next.run(request).await;
    }
    let mut response = match file(&root, path, request.headers(), head).await {
        Some(response) => response,
        None => HostError::NotFound.into_response(),
    };
    let headers = response.headers_mut();
    for (name, value) in [
        ("cross-origin-opener-policy", "same-origin"),
        ("cross-origin-embedder-policy", "require-corp"),
        ("cross-origin-resource-policy", "same-origin"),
        ("x-content-type-options", "nosniff"),
        ("cache-control", "no-cache"),
    ] {
        headers.insert(HeaderName::from_static(name), HeaderValue::from_static(value));
    }
    response
}

/// The response for a file under `root`, or None when the path is unsafe, missing, or not a regular file.
async fn file(root: &Path, path: &str, headers: &HeaderMap, head: bool) -> Option<Response> {
    let relative = relative(path)?;
    // Canonical per request, so a root that is a symlink swapped on deploy is followed.
    let root = tokio::fs::canonicalize(root).await.ok()?;
    let original = resolve(&root, &relative).await?;
    let mut compressed = relative.clone().into_os_string();
    compressed.push(".br");
    let brotli = if accepts_brotli(headers) {
        resolve(&root, Path::new(&compressed)).await
    } else {
        None
    };
    let served = brotli.as_ref().unwrap_or(&original);
    // HEAD reads only the length; axum drops any body for it anyway.
    let body = if head {
        None
    } else {
        Some(tokio::fs::read(served).await.ok()?)
    };
    let length = match &body {
        Some(body) => body.len() as u64,
        None => tokio::fs::metadata(served).await.ok()?.len(),
    };
    let mut response = Response::new(body.map_or_else(Body::empty, Body::from));
    let headers = response.headers_mut();
    headers.insert(header::CONTENT_TYPE, HeaderValue::from_static(mime(&relative)));
    headers.insert(header::CONTENT_LENGTH, HeaderValue::from(length));
    headers.insert(header::VARY, HeaderValue::from_static("accept-encoding"));
    if brotli.is_some() {
        headers.insert(header::CONTENT_ENCODING, HeaderValue::from_static("br"));
    }
    Some(response)
}

/// The relative file a request path names: `/` is `index.html`; otherwise every `/`-separated segment,
/// percent-decoded, must be a plain non-empty name that neither starts with `.` nor contains `..`, `/`, `\`,
/// `:` (a drive prefix) or NUL.
fn relative(path: &str) -> Option<PathBuf> {
    if path == "/" {
        return Some(PathBuf::from("index.html"));
    }
    let mut relative = PathBuf::new();
    for segment in path.strip_prefix('/')?.split('/') {
        let segment = decode(segment)?;
        if segment.is_empty()
            || segment.starts_with('.')
            || segment.contains("..")
            || segment.contains(['/', '\\', ':', '\0'])
        {
            return None;
        }
        relative.push(segment);
    }
    // Whatever the platform's path rules, nothing but plain names.
    relative
        .components()
        .all(|c| matches!(c, Component::Normal(_)))
        .then_some(relative)
}

/// Percent-decodes one segment; a malformed escape or a result that is not UTF-8 is None.
fn decode(segment: &str) -> Option<String> {
    let bytes = segment.as_bytes();
    let mut out = Vec::with_capacity(bytes.len());
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i] == b'%' {
            let hex = bytes.get(i + 1..i + 3)?;
            if !hex.iter().all(u8::is_ascii_hexdigit) {
                return None;
            }
            out.push(u8::from_str_radix(std::str::from_utf8(hex).ok()?, 16).ok()?);
            i += 3;
        } else {
            out.push(bytes[i]);
            i += 1;
        }
    }
    String::from_utf8(out).ok()
}

/// The canonical file `relative` names under the canonical `root`, if it is a regular file inside the root once
/// every symlink is resolved.
async fn resolve(root: &Path, relative: &Path) -> Option<PathBuf> {
    let file = tokio::fs::canonicalize(root.join(relative)).await.ok()?;
    let inside = file.starts_with(root) && tokio::fs::metadata(&file).await.ok()?.is_file();
    inside.then_some(file)
}

/// Whether `Accept-Encoding` lists `br` with a nonzero quality.
fn accepts_brotli(headers: &HeaderMap) -> bool {
    headers
        .get_all(header::ACCEPT_ENCODING)
        .iter()
        .filter_map(|value| value.to_str().ok())
        .flat_map(|value| value.split(','))
        .any(|coding| {
            let mut parts = coding.split(';').map(str::trim);
            parts.next().is_some_and(|name| name.eq_ignore_ascii_case("br"))
                && parts.all(|param| match param.split_once('=') {
                    Some((key, q)) if key.trim().eq_ignore_ascii_case("q") => q.trim().parse::<f32>().is_ok_and(|q| q > 0.0),
                    _ => true,
                })
        })
}

fn mime(file: &Path) -> &'static str {
    let extension = file.extension().and_then(|e| e.to_str()).unwrap_or_default().to_ascii_lowercase();
    match extension.as_str() {
        "html" => "text/html; charset=utf-8",
        "js" | "mjs" => "text/javascript; charset=utf-8",
        "wasm" => "application/wasm",
        "css" => "text/css; charset=utf-8",
        "json" => "application/json",
        "png" => "image/png",
        "jpg" | "jpeg" => "image/jpeg",
        "svg" => "image/svg+xml",
        "ico" => "image/x-icon",
        "txt" => "text/plain; charset=utf-8",
        "woff2" => "font/woff2",
        _ => "application/octet-stream",
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_plain_names_resolve() {
        assert_eq!(relative("/"), Some(PathBuf::from("index.html")));
        assert_eq!(relative("/pkg/client_bg.wasm"), Some(PathBuf::from("pkg/client_bg.wasm")));
        assert_eq!(relative("/a%20b.txt"), Some(PathBuf::from("a b.txt")));
        for unsafe_path in [
            "", "//", "/a/", "/a//b", "/..", "/../x", "/a/../../x", "/%2e%2e/x", "/%2E%2E/x", "/.", "/./x", "/.env",
            "/a%2f..%2fx", "/a%5c..%5cx", "/a\\b", "/C:/x", "/c%3a/x", "/a%00b", "/%zz", "/%2", "/%+f", "/%ff",
            "/x..y",
        ] {
            assert_eq!(relative(unsafe_path), None, "{unsafe_path}");
        }
    }

    #[test]
    fn brotli_needs_a_nonzero_quality() {
        let accepts = |value: &str| {
            let mut headers = HeaderMap::new();
            headers.insert(header::ACCEPT_ENCODING, HeaderValue::from_str(value).unwrap());
            accepts_brotli(&headers)
        };
        assert!(accepts("gzip, deflate, br, zstd"));
        assert!(accepts("BR;q=0.5"));
        assert!(!accepts("gzip"));
        assert!(!accepts("br;q=0"));
        assert!(!accepts("brotli"));
        assert!(!accepts_brotli(&HeaderMap::new()));
    }
}
