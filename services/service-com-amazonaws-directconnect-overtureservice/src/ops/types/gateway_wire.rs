// SPDX-License-Identifier: CC0-1.0

//! NOT COMPILED, and holds nothing: no module declares this file.
//!
//! It was added by an earlier round of a fix to carry two members a Direct Connect gateway
//! record was then put on the wire with — `directConnectGatewayArn` and
//! `directConnectGatewayType` — which the model did not declare, because
//! `#[smithy_macros::smithy(shape = ...)]` admits exactly the model's member list. The
//! model this crate ships declares NEITHER member, so neither belongs on a gateway this
//! service answers with, and the one projection of a stored gateway
//! (`ComAmazonawsDirectconnectOvertureserviceState::gateway_output`) fills only the
//! members the model declares; nothing is left for this file to hold, and `types/mod.rs`
//! does not declare it.
//!
//! The file itself could not be removed from the fixer's checkout, which has no delete.
