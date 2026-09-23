use civ_server::{router, Config, Host};
use std::{
    net::{IpAddr, Ipv4Addr, SocketAddr},
    path::PathBuf,
};

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
    let mut config = Config {
        secure_cookie,
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
    let result = axum::serve(listener, router(host))
        .with_graceful_shutdown(async {
            let _ = tokio::signal::ctrl_c().await;
        })
        .await;
    clock.abort();
    result?;
    Ok(())
}
