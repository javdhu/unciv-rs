//! `CIV_STATIC_DIR`: the browser client's files on the API's origin, and nothing outside the directory.
use civ_server::{router, Config, Host};
use serde_json::{json, Value};
use std::{
    io::{Read, Write},
    path::{Path, PathBuf},
};

const WASM: &[u8] = b"\0asm\x01\0\0\0";
const ISOLATION: [(&str, &str); 5] = [
    ("cross-origin-opener-policy", "same-origin"),
    ("cross-origin-embedder-policy", "require-corp"),
    ("cross-origin-resource-policy", "same-origin"),
    ("x-content-type-options", "nosniff"),
    ("cache-control", "no-cache"),
];

struct Server {
    address: String,
    task: tokio::task::JoinHandle<()>,
}
impl Drop for Server {
    fn drop(&mut self) {
        self.task.abort();
    }
}
async fn start(database: &Path, static_dir: Option<PathBuf>) -> Server {
    let listener = tokio::net::TcpListener::bind("127.0.0.1:0").await.unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let config = Config { allowed_origins: vec![format!("http://{address}")], static_dir, ..Config::default() };
    let app = router(Host::open(database, config).unwrap());
    let task = tokio::spawn(async move {
        axum::serve(listener, app).await.unwrap();
    });
    Server { address, task }
}

struct Reply {
    status: u16,
    headers: Vec<(String, String)>,
    body: Vec<u8>,
}
impl Reply {
    fn header(&self, name: &str) -> Option<&str> {
        self.headers.iter().find(|(n, _)| n == name).map(|(_, v)| v.as_str())
    }
    fn json(&self) -> Value {
        serde_json::from_slice(&self.body).unwrap()
    }
}

/// One HTTP/1.1 request with the target sent exactly as written: an HTTP client library would normalise `..`
/// and `%2e%2e` away before they reached the server.
async fn raw(server: &Server, method: &str, target: &str, headers: &[(&str, &str)]) -> Reply {
    let address = server.address.clone();
    let mut request = format!("{method} {target} HTTP/1.1\r\nHost: {address}\r\nConnection: close\r\n");
    for (name, value) in headers {
        request.push_str(&format!("{name}: {value}\r\n"));
    }
    request.push_str("Content-Length: 0\r\n\r\n");
    tokio::task::spawn_blocking(move || {
        let mut stream = std::net::TcpStream::connect(address).unwrap();
        stream.write_all(request.as_bytes()).unwrap();
        let mut bytes = Vec::new();
        stream.read_to_end(&mut bytes).unwrap();
        let split = bytes.windows(4).position(|w| w == b"\r\n\r\n").unwrap();
        let head = std::str::from_utf8(&bytes[..split]).unwrap();
        let mut lines = head.split("\r\n");
        let status = lines.next().unwrap().split(' ').nth(1).unwrap().parse().unwrap();
        let headers = lines
            .map(|line| {
                let (name, value) = line.split_once(':').unwrap();
                (name.trim().to_ascii_lowercase(), value.trim().to_owned())
            })
            .collect();
        Reply { status, headers, body: bytes[split + 4..].to_vec() }
    })
    .await
    .unwrap()
}
async fn get(server: &Server, target: &str) -> Reply {
    raw(server, "GET", target, &[]).await
}

/// `site/` holds index.html, pkg/app_bg.wasm and app.js with its brotli twin; `secret.txt` sits beside it.
fn site(dir: &Path) -> PathBuf {
    let root = dir.join("site");
    std::fs::create_dir_all(root.join("pkg")).unwrap();
    std::fs::write(root.join("index.html"), "<!doctype html><title>civ</title>").unwrap();
    std::fs::write(root.join("pkg/app_bg.wasm"), WASM).unwrap();
    std::fs::write(root.join("app.js"), "console.log('plain')").unwrap();
    std::fs::write(root.join("app.js.br"), b"\x8b\x02\x80brotli\x03").unwrap();
    std::fs::write(dir.join("secret.txt"), "outside the root").unwrap();
    root
}

fn assert_isolated(reply: &Reply, what: &str) {
    for (name, value) in ISOLATION {
        assert_eq!(reply.header(name), Some(value), "{what}: {name}");
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn serves_index_and_wasm_with_isolation_headers() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(&dir.path().join("game.sqlite"), Some(site(dir.path()))).await;

    let index = get(&server, "/").await;
    assert_eq!(index.status, 200);
    assert_eq!(index.header("content-type"), Some("text/html; charset=utf-8"));
    assert_eq!(index.body, b"<!doctype html><title>civ</title>");
    assert_isolated(&index, "/");
    assert_eq!(get(&server, "/index.html").await.body, index.body);

    let wasm = get(&server, "/pkg/app_bg.wasm").await;
    assert_eq!(wasm.status, 200);
    assert_eq!(wasm.header("content-type"), Some("application/wasm"));
    assert_eq!(wasm.header("content-length"), Some(WASM.len().to_string().as_str()));
    assert_eq!(wasm.header("content-encoding"), None);
    assert_eq!(wasm.body, WASM);
    assert_isolated(&wasm, "wasm");

    let head = raw(&server, "HEAD", "/pkg/app_bg.wasm", &[]).await;
    assert_eq!(head.status, 200);
    assert_eq!(head.header("content-length"), Some(WASM.len().to_string().as_str()));
    assert_eq!(head.header("content-type"), Some("application/wasm"));
    assert!(head.body.is_empty());
    assert_isolated(&head, "HEAD");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn brotli_twin_is_served_only_to_clients_that_accept_it() {
    let dir = tempfile::tempdir().unwrap();
    let server = start(&dir.path().join("game.sqlite"), Some(site(dir.path()))).await;

    let brotli = raw(&server, "GET", "/app.js", &[("Accept-Encoding", "gzip, deflate, br")]).await;
    assert_eq!(brotli.status, 200);
    assert_eq!(brotli.header("content-encoding"), Some("br"));
    assert_eq!(brotli.header("content-type"), Some("text/javascript; charset=utf-8"));
    assert_eq!(brotli.header("vary"), Some("accept-encoding"));
    assert_eq!(brotli.body, b"\x8b\x02\x80brotli\x03");
    assert_isolated(&brotli, "brotli");

    for accept in [None, Some("gzip"), Some("br;q=0")] {
        let headers: Vec<_> = accept.map(|a| ("Accept-Encoding", a)).into_iter().collect();
        let plain = raw(&server, "GET", "/app.js", &headers).await;
        assert_eq!(plain.status, 200, "{accept:?}");
        assert_eq!(plain.header("content-encoding"), None, "{accept:?}");
        assert_eq!(plain.header("vary"), Some("accept-encoding"), "{accept:?}");
        assert_eq!(plain.body, b"console.log('plain')", "{accept:?}");
    }

    // A file without a twin is served plain to a client that accepts brotli.
    let wasm = raw(&server, "GET", "/pkg/app_bg.wasm", &[("Accept-Encoding", "br")]).await;
    assert_eq!((wasm.header("content-encoding"), wasm.body.as_slice()), (None, WASM));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nothing_outside_the_root_and_no_directory_is_served() {
    let dir = tempfile::tempdir().unwrap();
    let root = site(dir.path());
    std::fs::create_dir(root.join("a")).unwrap();
    std::fs::write(root.join(".env"), "hidden").unwrap();
    #[cfg(unix)]
    {
        std::os::unix::fs::symlink(dir.path().join("secret.txt"), root.join("escape.txt")).unwrap();
        std::os::unix::fs::symlink(dir.path(), root.join("out")).unwrap();
        std::os::unix::fs::symlink(root.join("index.html"), root.join("inner.html")).unwrap();
    }
    let server = start(&dir.path().join("game.sqlite"), Some(root)).await;

    for target in [
        "/../secret.txt",
        "/%2e%2e/secret.txt",
        "/%2E%2E/secret.txt",
        "/a/../../secret.txt",
        "/a/%2e%2e/%2e%2e/secret.txt",
        "/a%2f..%2f..%2fsecret.txt",
        "/a%5c..%5c..%5csecret.txt",
        "/./index.html",
        "//index.html",
        "/.env",
        "/C:/secret.txt",
        "/index.html%00",
        "/escape.txt",
        "/out/secret.txt",
        "/missing.js",
        "/a",
        "/a/",
        "/pkg",
        "/pkg/",
    ] {
        let reply = get(&server, target).await;
        assert_eq!(reply.status, 404, "{target}");
        assert_eq!(reply.json(), json!({"error":"notFound"}), "{target}");
        assert_isolated(&reply, target);
    }
    // The canonical check does not refuse a link that stays inside the root.
    #[cfg(unix)]
    assert_eq!(get(&server, "/inner.html").await.body, b"<!doctype html><title>civ</title>");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn api_and_health_are_unchanged_with_static_files_on() {
    let dir = tempfile::tempdir().unwrap();
    let root = site(dir.path());
    // Files that would shadow the API if the prefix check failed.
    std::fs::create_dir_all(root.join("api")).unwrap();
    std::fs::write(root.join("api/rules"), "shadow").unwrap();
    std::fs::write(root.join("health"), "shadow").unwrap();
    let server = start(&dir.path().join("game.sqlite"), Some(root)).await;
    let origin = format!("http://{}", server.address);

    let health = get(&server, "/health").await;
    assert_eq!((health.status, health.body.as_slice()), (200, b"ok".as_slice()));
    assert_eq!(health.header("cache-control"), Some("no-store"));
    assert_eq!(health.header("cross-origin-opener-policy"), None);

    let rules = get(&server, "/api/rules").await;
    assert_eq!(rules.status, 200);
    assert!(rules.json()["contentHash"].is_string());
    assert_eq!(rules.header("cache-control"), Some("no-store"));

    // The Origin check still guards mutations, the socket, and unknown API paths.
    let forbidden = raw(&server, "POST", "/api/guest", &[]).await;
    assert_eq!((forbidden.status, forbidden.json()), (403, json!({"error":"forbidden"})));
    let guest = raw(&server, "POST", "/api/guest", &[("Origin", &origin)]).await;
    assert_eq!(guest.status, 200);
    assert!(guest.json()["guestId"].is_string());
    assert_eq!(raw(&server, "GET", "/api/matches/x/ws", &[]).await.status, 403);
    let unknown = get(&server, "/api/nope").await;
    assert_eq!(unknown.status, 404);
    assert_eq!(unknown.header("cache-control"), Some("no-store"));
    // A method other than GET and HEAD on a file path goes to the API router, as without static files.
    assert_eq!(raw(&server, "POST", "/index.html", &[]).await.status, 403);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_a_static_dir_nothing_is_served() {
    let dir = tempfile::tempdir().unwrap();
    site(dir.path());
    let server = start(&dir.path().join("game.sqlite"), None).await;
    for target in ["/", "/index.html"] {
        let reply = get(&server, target).await;
        assert_eq!(reply.status, 404, "{target}");
        assert_eq!(reply.header("cross-origin-opener-policy"), None);
        assert_eq!(reply.header("cache-control"), Some("no-store"), "the API router's fallback, as before");
    }
    assert_eq!(get(&server, "/health").await.body, b"ok");
}
