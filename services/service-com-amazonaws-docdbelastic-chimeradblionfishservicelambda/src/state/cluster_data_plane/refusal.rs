// SPDX-License-Identifier: CC0-1.0

//! How an elastic cluster's endpoint REFUSES a command, in the form its clients read.
//!
//! The engine answers a command it will not serve with a reply document carrying `ok: 0`, an
//! `errmsg` and a numeric `code` — never with a closed connection and never with a success. A
//! driver maps that code to an exception type, so a wrong code is a wrong exception in the
//! client, and the sentence is what a person debugging reads.
//!
//! An elastic cluster words some of those refusals differently from an Amazon DocumentDB
//! cluster of instances, and some of them carry no `codeName` at all, so an elastic refusal is
//! built by this endpoint and never taken from the document engine's own builders. The ones
//! raised by the command layer itself — the handshake, the authorization gate, a command that
//! could not be read — are here; the ones that come with a rule of their own are built beside
//! that rule, with the reading of it ([`super::sharding`]). A refusal the shared command layer
//! raised keeps its own code, name and sentence ([`From<WireFault>`]), because that layer
//! answers for the document engine and not for this endpoint's own rules.

use ::bson::Document;
use ::service_com_amazonaws_docdb_amazonrdsv19::state::cluster_data_plane as document_engine;

use document_engine::fault::WireFault;

/// One refusal, as the reply document it becomes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Refusal {
    /// The numeric `code` a driver branches on.
    pub code: i32,
    /// The `codeName` the reply carries, for a code this engine publishes a name for, and
    /// `None` for one it answers with the code alone.
    pub code_name: Option<&'static str>,
    /// The `errmsg` sentence.
    pub message: String,
}

impl Refusal {
    /// The reply document this refusal is answered with.
    #[must_use]
    pub fn to_reply(&self) -> Document {
        let mut reply = ::bson::doc! {
            "ok": 0.0_f64,
            "errmsg": self.message.clone(),
            "code": self.code,
        };
        if let Some(name) = self.code_name {
            reply.insert("codeName".to_string(), name);
        }
        reply
    }
}

/// A refusal the shared command layer raised, carried on unchanged: that layer answers for the
/// document engine, so its code, its name and its sentence are the answer.
impl From<WireFault> for Refusal {
    fn from(fault: WireFault) -> Self {
        Self {
            code: fault.code,
            code_name: Some(fault.code_name),
            message: fault.message,
        }
    }
}

/// A command sent by a session that has not authenticated.
///
/// The cluster authorizes nothing for a connection that presented no credential, which is what
/// this says: `AuthorizationFailed` and the engine's own sentence, with nothing about which
/// command it was or who the client might have been.
#[must_use]
pub fn authorization_failed() -> Refusal {
    Refusal {
        code: 13,
        code_name: Some("AuthorizationFailed"),
        message: "Authorization failed.".to_string(),
    }
}

/// A `saslStart` naming a mechanism this cluster does not offer. The sentence names the
/// mechanism the request asked for, which is the only variable part.
#[must_use]
pub fn invalid_auth_mechanism(mechanism: &str) -> Refusal {
    Refusal {
        code: 300,
        code_name: None,
        message: format!("Invalid auth mechanism {mechanism}"),
    }
}

/// An exchange begun for a user this cluster does not have: the administrator is the one user
/// an elastic cluster authenticates, so any other name is a user that does not exist.
#[must_use]
pub fn user_does_not_exist() -> Refusal {
    Refusal {
        code: 18,
        code_name: None,
        message: "Cascade Authentication failed. User does not exist".to_string(),
    }
}

/// An exchange whose proof did not hold — the password presented is not this cluster's. The
/// sentence says nothing about which part did not hold.
#[must_use]
pub fn authentication_failed() -> Refusal {
    Refusal {
        code: 18,
        code_name: None,
        message: "Authentication failed.".to_string(),
    }
}

/// A command that could not be read as the command it names.
#[must_use]
pub fn failed_to_parse(message: impl Into<String>) -> Refusal {
    Refusal::from(document_engine::fault::failed_to_parse(message))
}

/// A command that is well formed but cannot be served in the state the cluster is in.
#[must_use]
pub fn illegal_operation(message: impl Into<String>) -> Refusal {
    Refusal::from(document_engine::fault::illegal_operation(message))
}

/// A command this endpoint does not serve.
#[must_use]
pub fn command_not_found(command: &str) -> Refusal {
    Refusal::from(document_engine::fault::command_not_found(command))
}

/// A command naming something that is not a `<database>.<collection>` namespace at all.
#[must_use]
pub fn invalid_namespace(namespace: &str) -> Refusal {
    Refusal {
        code: 73,
        code_name: Some("InvalidNamespace"),
        message: format!("Invalid namespace specified '{namespace}'"),
    }
}
