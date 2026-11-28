// SPDX-License-Identifier: CC0-1.0

//! The shapes `list_linked_whats_app_business_accounts` reaches, as the published model
//! declares them.
//!
//! Every shape the operation reaches except its output, which this service answers with
//! the account-level `setupEnabled` flag the published model does not declare and which
//! is therefore declared in [`super::overlay`].
#![allow(unused_imports)]

use super::super::*;

smithy_macros::smithy_model!("smithy-model/com-amazonaws-socialmessaging-socialmessaging.json");

#[smithy_macros::smithy(shape = "com.amazonaws.socialmessaging#ListLinkedWhatsAppBusinessAccountsInput")]
#[derive(Debug, Clone, Default)]
pub struct ListLinkedWhatsAppBusinessAccountsInput {
    pub next_token: Option<String>,
    pub max_results: Option<i32>,
}

#[smithy_macros::smithy(shape = "com.amazonaws.socialmessaging#LinkedWhatsAppBusinessAccountSummary")]
#[derive(Debug, Clone)]
pub struct LinkedWhatsAppBusinessAccountSummary {
    pub arn: String,
    pub id: String,
    pub waba_id: String,
    pub registration_status: String,
    pub link_date: ::aws_smithy_types::DateTime,
    pub waba_name: String,
    pub event_destinations: Vec<WhatsAppBusinessAccountEventDestination>,
    pub marketing_messages_onboarding_status: Option<String>,
    pub direct_send_enforcement_status: Option<String>,
    pub direct_send_enforcement_status_expires_at: Option<::aws_smithy_types::DateTime>,
    pub dataset_id: Option<String>,
}
