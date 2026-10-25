// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! Local, private-room host. The only network game state is a seat-filtered projection.
mod host;
mod http;
mod site;
mod store;

pub use host::{Config, Host, HostError};
pub use http::router;

pub(crate) fn now_ms() -> u64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis()
        .min(u128::from(u64::MAX)) as u64
}

pub(crate) fn opaque_id() -> String {
    uuid::Uuid::new_v4().simple().to_string()
}

pub(crate) fn token_hash(token: &str) -> String {
    use sha2::{Digest, Sha256};
    format!("{:x}", Sha256::digest(token.as_bytes()))
}
