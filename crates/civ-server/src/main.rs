// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

use civ_server::{router, Config, Host};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
    time::Duration,
};

/// Open sockets may outlive a graceful drain; stop waiting well inside Fly's 10 s kill_timeout.
const DRAIN_LIMIT: Duration = Duration::from_secs(5);

/// Ctrl-C locally, SIGTERM from Fly.io, Docker and systemd.
async fn stop_requested() {
    #[cfg(unix)]
    {
        use tokio::signal::unix::{signal, SignalKind};
        let mut terminate = signal(SignalKind::terminate()).expect("SIGTERM handler");
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = terminate.recv() => {}
        }
    }
    #[cfg(not(unix))]
    let _ = tokio::signal::ctrl_c().await;
}

#[tokio::main]
async fn main() -> Result<(), Box<dyn std::error::Error>> {
    let address: SocketAddr = std::env::var("CIV_BIND")
        .unwrap_or_else(|_| "127.0.0.1:3001".into())
        .parse()?;
    let origins = std::env::var("CIV_ALLOWED_ORIGINS")
        .ok()
        .map(|s| s.split(',').map(|s| s.trim().to_string()).collect());
    let secure_cookie = std::env::var("CIV_SECURE_COOKIE").as_deref() == Ok("1");
    if address.ip() != IpAddr::V4(Ipv4Addr::LOCALHOST) && origins.is_none() {
        return Err("non-default bind requires explicit CIV_ALLOWED_ORIGINS".into());
    }
    let static_dir = std::env::var("CIV_STATIC_DIR")
        .ok()
        .filter(|s| !s.is_empty())
        .map(PathBuf::from);
    if let Some(dir) = &static_dir {
        if !dir.is_dir() {
            return Err(format!("CIV_STATIC_DIR {} is not a directory", dir.display()).into());
        }
        eprintln!("Serving files from {}", dir.display());
    }
    let mut config = Config {
        secure_cookie,
        static_dir,
        ..Config::default()
    };
    if let Some(origins) = origins {
        config.allowed_origins = origins;
    }
    let database = PathBuf::from(
        std::env::var("CIV_DATABASE").unwrap_or_else(|_| "civ-server.sqlite3".into()),
    );
    let host = Host::open(database, config)?;
    let listener = tokio::net::TcpListener::bind(address).await?;
    eprintln!(
        "Private-room host listening on http://{}",
        listener.local_addr()?
    );
    let clock = tokio::spawn(host.clone().run_clock());
    let (stop, stopped) = tokio::sync::watch::channel(false);
    let server = axum::serve(listener, router(host.clone())).with_graceful_shutdown({
        let mut stopped = stopped.clone();
        async move {
            let _ = stopped.wait_for(|&s| s).await;
        }
    });
    let mut server = tokio::spawn(async move { server.await });
    let result = tokio::select! {
        result = &mut server => result?,
        () = stop_requested() => {
            eprintln!("Stopping: draining connections");
            let _ = stop.send(true);
            // WebSockets stay open until their clients leave, so the drain is bounded.
            match tokio::time::timeout(DRAIN_LIMIT, &mut server).await {
                Ok(result) => result?,
                Err(_) => {
                    server.abort();
                    Ok(())
                }
            }
        }
    };
    clock.abort();
    let _ = clock.await;
    host.flush().await?;
    eprintln!("Stopped; store flushed");
    result?;
    Ok(())
}
