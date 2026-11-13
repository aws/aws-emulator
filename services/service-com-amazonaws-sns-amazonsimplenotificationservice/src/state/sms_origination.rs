// SPDX-License-Identifier: CC0-1.0

//! Whether this account has anything to send an SMS message FROM.
//!
//! An SMS message leaves AWS from an ORIGINATION ENTITY — a short code, a toll-free
//! number, a ten-digit long code or another sender identity the account holds — and an
//! account that holds none has nothing SNS could send the message from. The inventory
//! lives in [`super::ComAmazonawsSnsAmazonsimplenotificationserviceState::origination_numbers`],
//! is acquired OUT OF BAND (no SNS API adds one, as
//! [`super::OriginationNumberRecord`] sets out) and is therefore EMPTY unless something
//! put a number there.
//!
//! This module is that one question, asked by every operation whose own work is to SEND
//! an SMS message, so the rule does not depend on which call asked.

use super::OriginationNumberRecord;

/// The capability an origination entity has to carry for SNS to send an SMS message
/// from it: `SMS`, one of the values the modelled `NumberCapability` enum accepts
/// (`crate::ops::types::NUMBER_CAPABILITY_VALUES`). A number that can only place a
/// voice call or carry MMS cannot send a text message.
pub const SMS_CAPABILITY: &str = "SMS";

/// The sentence SNS refuses a request that would have it SEND an SMS message for an
/// account holding no origination entity able to send one with.
///
/// WHOLLY FIXED text: no part of it comes from the request or from state — in
/// particular the destination number is `@sensitive` and is not echoed. Recorded
/// evidence: `CreateSMSSandboxPhoneNumber`, whose work is to send a one-time password
/// to the destination number, answers `400` / `UserError` with exactly this sentence in
/// an account with no origination entity.
pub const NO_ORIGINATION_ENTITIES: &str = "No origination entities available to send";

/// Whether `inventory` holds an origination entity SNS could send an SMS message from:
/// one whose capabilities include [`SMS_CAPABILITY`].
pub fn can_send_sms<'a, I>(inventory: I) -> bool
where
    I: IntoIterator<Item = &'a OriginationNumberRecord>,
{
    inventory.into_iter().any(|number| {
        number
            .number_capabilities
            .iter()
            .any(|capability| capability == SMS_CAPABILITY)
    })
}
