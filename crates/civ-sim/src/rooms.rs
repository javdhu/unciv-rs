//! The rooms HTTP API's wire values: every request body and response body under civ-server's `/api` routes.
//! civ-server answers with them and a client sends and reads them, so the JSON has one definition.
//!
//! Requests deny unknown fields; responses do not, so a client tolerates fields a newer server adds.
use crate::{RuleTables, SeatChoice};
use serde::{Deserialize, Serialize};
use ts_rs::TS;

/// `POST /api/guest` and `POST /api/guest/name`: the caller's guest identity.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct GuestResponse {
    pub guest_id: String,
    /// Present once the guest chose a display name.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub name: Option<String>,
}

/// `POST /api/guest/name`: the name other members see, 1 to 32 characters once trimmed, no control characters.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct NameRequest {
    pub name: String,
}

/// `POST /api/rooms`: a new room's settings. Every field is optional on the wire; a missing one takes its
/// `Default` value.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", default, deny_unknown_fields)]
pub struct RoomRequest {
    pub width: u16,
    pub height: u16,
    pub majors: u8,
    pub city_states: u8,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub nation: Option<String>,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    pub wrap_x: bool,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max_turns: Option<u32>,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub speed: Option<String>,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub ruleset: Option<String>,
    #[ts(optional)]
    #[serde(skip_serializing_if = "Option::is_none")]
    pub victories: Option<Vec<String>>,
    pub turn_seconds: u32,
}

impl Default for RoomRequest {
    fn default() -> Self {
        Self {
            width: 32,
            height: 20,
            majors: 4,
            city_states: 2,
            nation: None,
            leader: None,
            slice: None,
            wrap_x: false,
            max_turns: None,
            speed: None,
            ruleset: None,
            victories: None,
            turn_seconds: 120,
        }
    }
}

/// `POST /api/rooms/join`: the 32-character invite the room's owner shares.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
pub struct JoinRequest {
    pub invite: String,
}

/// `POST /api/rooms/{id}/seat`: the caller's nation and leader in the lobby.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase", deny_unknown_fields)]
pub struct SeatRequest {
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub nation: Option<String>,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub leader: Option<String>,
}

/// One room as its caller sees it: the answer of create, join, `GET /api/rooms/{id}`, seat and start.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoomResponse {
    pub room_id: String,
    pub own_seat: u8,
    pub occupied_seats: usize,
    pub capacity: u8,
    pub width: u16,
    pub height: u16,
    pub city_states: u8,
    pub turn_seconds: u32,
    pub is_owner: bool,
    pub wrap_x: bool,
    /// The effective limit: explicit, or the speed's length.
    pub max_turns: u32,
    pub speed: String,
    pub ruleset: String,
    pub victories: Option<Vec<String>>,
    /// The Blitz slice, when the room plays one.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub slice: Option<String>,
    /// Each occupied seat's chosen nation and leader, if any.
    pub seats: Vec<SeatChoice>,
    /// Only the owner sees it.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub invite: Option<String>,
    pub match_id: Option<String>,
    /// Every occupied seat: who holds it and whether they are there (in the
    /// lobby, or connected once the match runs), and once the match runs,
    /// whether they have finished the turn or left it to a bot.
    pub members: Vec<MemberView>,
}

/// One occupied seat of a room.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct MemberView {
    pub seat: u8,
    pub name: Option<String>,
    pub is_owner: bool,
    /// Before the start, whether they read the room in the last few seconds;
    /// once the match runs, whether the host holds a live socket for the seat.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub connected: Option<bool>,
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ready: Option<bool>,
    /// A person plays it; false while a bot has taken over.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub controlled: Option<bool>,
    pub left: bool,
}

/// `GET /api/rooms`: the rooms a returning guest holds a seat in, lobbies and matches alike, leaving out any it
/// left.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct RoomsResponse {
    pub rooms: Vec<RoomSummary>,
}

/// One entry of [`RoomsResponse`].
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RoomSummary {
    pub room_id: String,
    pub match_id: Option<String>,
    pub own_seat: u8,
    pub is_owner: bool,
    pub occupied_seats: usize,
    pub capacity: u8,
    /// The match's turn, once it has started.
    #[ts(optional)]
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn: Option<u32>,
    pub finished: bool,
}

/// `POST /api/rooms/{id}/leave`: always `{"left": true}`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct LeaveResponse {
    pub left: bool,
}

/// `GET /api/rules`: the public rule tables and their content hash, the one a match's Welcome carries.
#[derive(Debug, Clone, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct RulesResponse {
    pub content_hash: String,
    pub tables: RuleTables,
}

/// The body of every error the `/api` routes answer with a JSON body.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, TS, schemars::JsonSchema)]
pub struct ErrorBody {
    pub error: ErrorCode,
}

/// Why an `/api` call failed, with its HTTP status.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize, TS, schemars::JsonSchema)]
#[serde(rename_all = "camelCase")]
pub enum ErrorCode {
    /// 401: no valid guest cookie.
    Unauthorized,
    /// 403
    Forbidden,
    /// 404
    NotFound,
    /// 409
    Conflict,
    /// 400
    InvalidRequest,
    /// 429
    Capacity,
    /// 409
    ConnectionReplaced,
    /// 503
    Unavailable,
}

impl ErrorCode {
    /// The wire string, as serde writes it.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Unauthorized => "unauthorized",
            Self::Forbidden => "forbidden",
            Self::NotFound => "notFound",
            Self::Conflict => "conflict",
            Self::InvalidRequest => "invalidRequest",
            Self::Capacity => "capacity",
            Self::ConnectionReplaced => "connectionReplaced",
            Self::Unavailable => "unavailable",
        }
    }
}

impl std::fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn error_codes_serialize_as_their_strings() {
        use ErrorCode::*;
        for code in [Unauthorized, Forbidden, NotFound, Conflict, InvalidRequest, Capacity, ConnectionReplaced, Unavailable] {
            let json = serde_json::to_string(&ErrorBody { error: code }).unwrap();
            assert_eq!(json, format!(r#"{{"error":"{}"}}"#, code.as_str()));
            assert_eq!(serde_json::from_str::<ErrorBody>(&json).unwrap().error, code);
        }
    }

    #[test]
    fn a_default_room_request_omits_unset_options_and_reads_back() {
        let json = serde_json::to_string(&RoomRequest::default()).unwrap();
        assert_eq!(json, r#"{"width":32,"height":20,"majors":4,"cityStates":2,"wrapX":false,"turnSeconds":120}"#);
        assert_eq!(serde_json::from_str::<RoomRequest>(&json).unwrap(), RoomRequest::default());
        assert_eq!(serde_json::from_str::<RoomRequest>("{}").unwrap(), RoomRequest::default());
        assert!(serde_json::from_str::<RoomRequest>(r#"{"seed":1}"#).is_err());
    }
}
