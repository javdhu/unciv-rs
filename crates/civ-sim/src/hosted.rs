//! Wasm adapter for the shared session; IndexedDB commit stays in its Worker host.
use crate::{js, protocol, session, tables};
use serde::Serialize;
use wasm_bindgen::prelude::*;

#[wasm_bindgen]
pub struct HostedGame {
    session: session::Session,
    prepared: Option<session::CommandTransition>,
}

#[derive(Serialize)]
#[serde(rename_all = "camelCase")]
struct PreparedReply {
    receipt: protocol::CommandReceipt,
    updates: Vec<protocol::ServerMessage>,
}

#[wasm_bindgen]
impl HostedGame {
    #[wasm_bindgen(constructor)]
    pub fn new(settings: JsValue, epoch: String) -> Result<HostedGame, JsError> {
        console_error_panic_hook::set_once();
        let settings =
            serde_wasm_bindgen::from_value(settings).map_err(|e| JsError::new(&e.to_string()))?;
        let session = session::Session::new(settings, epoch, vec![0])
            .map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Self {
            session,
            prepared: None,
        })
    }
    pub fn restore(snapshot: &[u8]) -> Result<HostedGame, JsError> {
        let session =
            session::Session::restore(snapshot).map_err(|e| JsError::new(&e.to_string()))?;
        Ok(Self {
            session,
            prepared: None,
        })
    }
    pub fn welcome(&self) -> Result<JsValue, JsError> {
        let update = self
            .session
            .current_update(0)
            .map_err(|e| JsError::new(&e.to_string()))?;
        js(&protocol::ServerMessage::Welcome {
            version: protocol::PROTOCOL_VERSION,
            content_hash: self.session.content_hash(),
            update,
        })
    }
    pub fn rules(&self) -> Result<JsValue, JsError> {
        js(&tables(self.session.rules()))
    }
    pub fn query(&self, query: JsValue) -> Result<JsValue, JsError> {
        let query =
            serde_wasm_bindgen::from_value(query).map_err(|e| JsError::new(&e.to_string()))?;
        js(&self
            .session
            .query(0, query)
            .map_err(|e| JsError::new(&e.to_string()))?)
    }
    /// There may be only one staged write; its snapshot must be persisted before commit.
    pub fn prepare(&mut self, command: JsValue) -> Result<JsValue, JsError> {
        if self.prepared.is_some() {
            return Err(JsError::new("commit or discard the pending command first"));
        }
        let command =
            serde_wasm_bindgen::from_value(command).map_err(|e| JsError::new(&e.to_string()))?;
        let transition = self
            .session
            .stage_command(0, command)
            .map_err(|e| JsError::new(&e.to_string()))?;
        let reply = js(&PreparedReply {
            receipt: transition.receipt.clone(),
            updates: transition
                .updates
                .iter()
                .filter(|(seat, _)| *seat == 0)
                .map(|(_, u)| self.session.update_message(0, u.clone()))
                .collect::<Result<_, _>>()
                .map_err(|e| JsError::new(&e.to_string()))?,
        })?;
        self.prepared = Some(transition);
        Ok(reply)
    }
    #[wasm_bindgen(js_name = pendingSnapshot)]
    pub fn pending_snapshot(&self) -> Result<Vec<u8>, JsError> {
        self.prepared
            .as_ref()
            .ok_or_else(|| JsError::new("no staged command"))?
            .next
            .save()
            .map_err(|e| JsError::new(&e.to_string()))
    }
    pub fn commit(&mut self) -> Result<(), JsError> {
        let prepared = self
            .prepared
            .take()
            .ok_or_else(|| JsError::new("no staged command"))?;
        self.session = prepared.next;
        Ok(())
    }
    pub fn discard(&mut self) {
        self.prepared = None;
    }
    pub fn snapshot(&self) -> Result<Vec<u8>, JsError> {
        self.session
            .save()
            .map_err(|e| JsError::new(&e.to_string()))
    }
}
