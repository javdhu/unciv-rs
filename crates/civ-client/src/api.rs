// This Source Code Form is subject to the terms of the Mozilla Public
// License, v. 2.0. If a copy of the MPL was not distributed with this
// file, You can obtain one at https://mozilla.org/MPL/2.0/.

//! civ-server's rooms API, typed with `civ_sim::rooms` (docs/ROOMS-SPEC.md, component 1): what each call
//! sends and how its reply reads. The transport is `net::Http`; calls return at once and answers arrive later.

use crate::net::{Http, HttpReply};
use civ_sim::rooms::{
    ErrorBody, GuestResponse, JoinRequest, LeaveResponse, NameRequest, RoomRequest, RoomResponse, RoomsResponse,
    RulesResponse, SeatRequest,
};

#[derive(Clone, Debug)]
pub enum Call {
    Guest,
    Rename(String),
    Rooms,
    Create(RoomRequest),
    Join(String),
    Room(String),
    Seat(String, SeatRequest),
    Start(String),
    Leave(String),
    Rules,
}

#[derive(Clone, Debug)]
pub enum Answer {
    Guest(GuestResponse),
    Rooms(RoomsResponse),
    Room(RoomResponse),
    Left,
    Rules(Box<RulesResponse>),
}

/// Why a call failed: the server's code (`unauthorized`, `conflict`, ...), or that it could not be reached.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum ApiError {
    Server { status: u16, code: String },
    Unreachable(String),
    /// A reply this client cannot read.
    Garbled(String),
}

impl std::fmt::Display for ApiError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            ApiError::Server { code, .. } => f.write_str(match code.as_str() {
                "unauthorized" => "the server does not know this guest",
                "forbidden" => "not allowed",
                "notFound" => "no such room",
                "conflict" => "the room is full, started, or that nation is taken",
                "invalidRequest" => "the server refused the request",
                "capacity" => "the server is full",
                "unavailable" => "the server is busy; try again",
                other => other,
            }),
            ApiError::Unreachable(_) => f.write_str("the server could not be reached"),
            ApiError::Garbled(e) => write!(f, "unreadable reply: {e}"),
        }
    }
}

impl Call {
    /// Method, path and JSON body.
    pub fn request(&self) -> (&'static str, String, Option<String>) {
        macro_rules! json {
            ($body:expr) => {
                Some(serde_json::to_string($body).expect("request bodies serialize"))
            };
        }
        match self {
            Call::Guest => ("POST", "/api/guest".into(), None),
            Call::Rename(name) => ("POST", "/api/guest/name".into(), json!(&NameRequest { name: name.clone() })),
            Call::Rooms => ("GET", "/api/rooms".into(), None),
            Call::Create(settings) => ("POST", "/api/rooms".into(), json!(settings)),
            Call::Join(invite) => ("POST", "/api/rooms/join".into(), json!(&JoinRequest { invite: invite.clone() })),
            Call::Room(id) => ("GET", format!("/api/rooms/{id}"), None),
            Call::Seat(id, seat) => ("POST", format!("/api/rooms/{id}/seat"), json!(seat)),
            Call::Start(id) => ("POST", format!("/api/rooms/{id}/start"), None),
            Call::Leave(id) => ("POST", format!("/api/rooms/{id}/leave"), None),
            Call::Rules => ("GET", "/api/rules".into(), None),
        }
    }

    pub fn send(&self, http: &Http, id: u64) {
        let (method, path, body) = self.request();
        http.send(id, method, &path, body);
    }

    /// Read the reply to this call.
    pub fn read(&self, reply: &HttpReply) -> Result<Answer, ApiError> {
        if reply.status == 0 {
            return Err(ApiError::Unreachable(reply.body.clone()));
        }
        if !(200..300).contains(&reply.status) {
            let code = serde_json::from_str::<ErrorBody>(&reply.body)
                .ok()
                .map(|e| e.error.as_str().to_string())
                .unwrap_or_else(|| reply.status.to_string());
            return Err(ApiError::Server { status: reply.status, code });
        }
        let garbled = |e: serde_json::Error| ApiError::Garbled(e.to_string());
        Ok(match self {
            Call::Guest | Call::Rename(_) => Answer::Guest(serde_json::from_str(&reply.body).map_err(garbled)?),
            Call::Rooms => Answer::Rooms(serde_json::from_str(&reply.body).map_err(garbled)?),
            Call::Rules => Answer::Rules(Box::new(serde_json::from_str(&reply.body).map_err(garbled)?)),
            Call::Leave(_) => {
                let _: LeaveResponse = serde_json::from_str(&reply.body).map_err(garbled)?;
                Answer::Left
            }
            Call::Create(_) | Call::Join(_) | Call::Room(_) | Call::Seat(..) | Call::Start(_) => {
                Answer::Room(serde_json::from_str(&reply.body).map_err(garbled)?)
            }
        })
    }
}
