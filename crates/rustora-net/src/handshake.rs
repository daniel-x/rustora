// SPDX-License-Identifier: GPL-3.0-or-later
// Copyright (c) The rustora authors. Ported from OpenRA, Copyright (c) The OpenRA Developers and Contributors.

//! The connection handshake (`OpenRA.Game/Network/Handshake.cs`).

use rustora_core::fieldloader::load;
use rustora_core::miniyaml::{MiniYaml, MiniYamlNode, nodes_to_string};
use rustora_derive::FieldLoad;

use crate::NetError;
use crate::session::Client;

/// `ProtocolVersion.Handshake`, the first value a server sends on a new connection.
pub const HANDSHAKE_PROTOCOL: i32 = 7;
/// `ProtocolVersion.Orders`.
pub const ORDERS_PROTOCOL: i32 = 21;

fn invalid(e: impl std::fmt::Display) -> NetError {
    NetError::Invalid(format!("invalid handshake: {e}"))
}

fn field(key: &str, value: &Option<String>) -> MiniYamlNode {
    MiniYamlNode::with_value_str(key, value.clone().unwrap_or_default())
}

#[derive(Clone, Debug, Default, FieldLoad)]
pub struct HandshakeRequest {
    #[field(name = "Mod")]
    pub mod_id: Option<String>,
    pub version: Option<String>,
    pub auth_token: Option<String>,
}

impl HandshakeRequest {
    pub fn serialize(&self) -> String {
        let fields = vec![
            field("Mod", &self.mod_id),
            field("Version", &self.version),
            field("AuthToken", &self.auth_token),
        ];
        nodes_to_string(&[MiniYamlNode::new("Handshake", MiniYaml::new(None, fields))])
    }

    pub fn deserialize(data: &str, name: &str) -> Result<HandshakeRequest, NetError> {
        let nodes = MiniYaml::from_str(data, name, true).map_err(invalid)?;
        let first = nodes.first().ok_or_else(|| invalid("empty"))?;
        load(&first.value).map_err(invalid)
    }
}

#[derive(Clone, Debug, FieldLoad)]
pub struct HandshakeResponse {
    #[field(name = "Mod")]
    pub mod_id: Option<String>,
    pub version: Option<String>,
    pub password: Option<String>,
    pub orders_protocol: i32,
    pub fingerprint: Option<String>,
    pub auth_signature: Option<String>,
    #[field(ignore)]
    pub client: Client,
}

impl Default for HandshakeResponse {
    /// `OrdersProtocol` defaults to 7 so that servers detect clients from before the field existed.
    fn default() -> Self {
        HandshakeResponse {
            mod_id: None,
            version: None,
            password: None,
            orders_protocol: HANDSHAKE_PROTOCOL,
            fingerprint: None,
            auth_signature: None,
            client: Client::default(),
        }
    }
}

impl HandshakeResponse {
    pub fn serialize(&self) -> String {
        let fields = vec![
            field("Mod", &self.mod_id),
            field("Version", &self.version),
            field("Password", &self.password),
            field("Fingerprint", &self.fingerprint),
            field("AuthSignature", &self.auth_signature),
            MiniYamlNode::with_value_str("OrdersProtocol", self.orders_protocol.to_string()),
        ];
        nodes_to_string(&[
            MiniYamlNode::new("Handshake", MiniYaml::new(None, fields)),
            MiniYamlNode::new("Client", self.client.serialize().value),
        ])
    }

    pub fn deserialize(data: &str, name: &str) -> Result<HandshakeResponse, NetError> {
        let mut handshake = HandshakeResponse::default();
        for n in MiniYaml::from_str(data, name, true).map_err(invalid)? {
            match n.key.as_str() {
                "Handshake" => {
                    let client = std::mem::take(&mut handshake.client);
                    handshake = load(&n.value).map_err(invalid)?;
                    handshake.client = client;
                }
                "Client" => handshake.client = load(&n.value).map_err(invalid)?,
                _ => {}
            }
        }
        Ok(handshake)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn response_round_trip() {
        let r = HandshakeResponse {
            mod_id: Some("ra".into()),
            version: Some("release-20250330".into()),
            orders_protocol: ORDERS_PROTOCOL,
            client: Client {
                name: Some("Commander".into()),
                ..Default::default()
            },
            ..Default::default()
        };
        let back = HandshakeResponse::deserialize(&r.serialize(), "test").unwrap();
        assert_eq!(back.mod_id.as_deref(), Some("ra"));
        assert_eq!(back.orders_protocol, ORDERS_PROTOCOL);
        assert_eq!(back.client.name.as_deref(), Some("Commander"));
    }

    #[test]
    fn missing_protocol_is_old_client() {
        let back = HandshakeResponse::deserialize("Handshake:\n\tMod: ra\n", "test").unwrap();
        assert_eq!(back.orders_protocol, HANDSHAKE_PROTOCOL);
    }
}
