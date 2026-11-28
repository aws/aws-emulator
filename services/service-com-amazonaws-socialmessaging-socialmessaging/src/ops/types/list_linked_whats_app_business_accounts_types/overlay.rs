// SPDX-License-Identifier: CC0-1.0

//! `ListLinkedWhatsAppBusinessAccountsOutput`, declared from this crate's overlay model.
//!
//! AWS answers the WhatsApp Business Account listing with an account-level fact beside
//! the page — `setupEnabled`, whether WhatsApp setup is enabled for the calling account —
//! reported whatever the page holds, an empty listing included, while the published model
//! declares the output with the page and its token alone. The published snapshot is kept
//! exactly as shipped (see `smithy-model/LOCAL-ADDITIONS.md`), so this one shape is
//! generated from the small overlay model `smithy-model/local-additions.json`, which
//! restates it with that member against the snapshot's own member targets. Nothing about
//! the wire is hand-written: the shape is still declared by `#[smithy_macros::smithy]`
//! from a model.
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/local-additions.json");

/// The page `ListLinkedWhatsAppBusinessAccounts` reports, and the account-level fact it
/// reports beside it.
///
/// `setup_enabled` is read from the store
/// ([`crate::state::ComAmazonawsSocialmessagingSocialmessagingState::setup_enabled`]), not
/// derived from the page, so every page of every listing carries it.
#[smithy_macros::smithy(shape = "com.amazonaws.socialmessaging#ListLinkedWhatsAppBusinessAccountsOutput")]
#[derive(Debug, Clone, Default)]
pub struct ListLinkedWhatsAppBusinessAccountsOutput {
    pub linked_accounts: Option<Vec<LinkedWhatsAppBusinessAccountSummary>>,
    pub next_token: Option<String>,
    pub setup_enabled: Option<bool>,
}
