// SPDX-License-Identifier: CC0-1.0

//! `Connection`: the connection structure every reader of a connection reports, as AWS
//! reports it.
//!
//! This is the ONE `Connection` type of this crate — the shape `GetConnection` and
//! `ListConnections` both render from the single projection
//! [`crate::state::StoredConnection::to_modeled`] — so what it declares is what every
//! reader of a connection reports.
//!
//! It is declared here rather than beside the other shared shapes because AWS answers
//! every connection read with one member the published model snapshot does not declare,
//! and that snapshot is kept as shipped (see `smithy-model/LOCAL-ADDITIONS.md`):
//!
//! * `OwnershipType` — whether the account reading the connection owns it or had it
//!   shared in. Recorded in the wire fixtures, where a connection created in the calling
//!   account is read back with `OwnershipType` `Owned`. The snapshot declares neither the
//!   member nor a shape of that name.
//!
//! So the type is generated, like every other modelled shape, from a model file — the
//! small overlay `smithy-model/local-additions.json`, which restates the members the
//! snapshot declares, with their own targets so every value set, length and pattern is
//! captured exactly as the snapshot spells it, and adds the one it does not. Nothing
//! about the wire is hand-written: the member names, the shape id and the constraints all
//! come from that file.
#![allow(unused_imports)]

use super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// A connection as this service reports it.
///
/// `ownership_type` is the member the snapshot does not declare: an open string, because
/// nothing publishes its value set — this service reports
/// [`crate::state::CONNECTION_OWNERSHIP_OWNED`] for a connection the calling account
/// owns, read off the stored row like every other member.
#[smithy_macros::smithy(shape = "com.amazonaws.codeconnections#Connection")]
#[derive(Debug, Clone, Default)]
pub struct Connection {
    pub connection_name: Option<String>,
    pub connection_arn: Option<String>,
    pub provider_type: Option<String>,
    pub owner_account_id: Option<String>,
    pub connection_status: Option<String>,
    pub host_arn: Option<String>,
    pub provider_endpoint: Option<String>,
    pub ownership_type: Option<String>,
}
