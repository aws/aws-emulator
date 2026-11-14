// SPDX-License-Identifier: CC0-1.0

//! What the ACCOUNT does with a message THIS surface has just accepted for one of its own
//! recipients — the hand-off to Amazon SES email receiving that every send of this surface makes.
//!
//! Email receiving is one facility of the ACCOUNT rather than of an API: the account's receipt
//! rule sets, which of them is active and the actions a matching rule carries out are what
//! `CreateReceiptRuleSet`, `CreateReceiptRule` and `SetActiveReceiptRuleSet` of the CLASSIC
//! surface write, and they live in that surface's store. So a message this surface accepts for a
//! recipient the account holds an identity for is handed to that surface's receive path
//! ([`::service_com_amazonaws_ses_simpleemailservice::state::inbound_delivery`]) rather than a
//! second copy of either the rule sets or the actions being kept here.
//!
//! One module, because more than one send of this surface accepts a message: a single send and a
//! bulk send reach the receive path the same way, so neither can drift from the other over what a
//! received message is.

/// A message a send has ACCEPTED, as the account's receive path reads it.
#[derive(Debug, Clone, Copy)]
pub struct AcceptedSend<'a> {
    /// The Region the send was signed for, which is where the account's receipt rule set and the
    /// destinations its actions name live.
    pub region: &'a str,
    /// The id the message was accepted under, which is what the object a saving action writes is
    /// keyed by.
    pub message_id: &'a str,
    /// The sending identity's address.
    pub sender: &'a str,
    /// The envelope recipients, which is what a receipt rule's conditions are compared against.
    pub recipients: &'a [String],
    /// The message's RFC 5322 bytes.
    pub message: &'a [u8],
    /// The instant the message was accepted.
    pub accepted_at: ::std::time::SystemTime,
}

/// Hand `accepted` to the account's own receive path: what the active receipt rule set does with
/// a message addressed to a recipient the account holds an identity for.
///
/// Nothing happens for an engine that carries no store for the classic surface, and nothing
/// happens when no rule of the active set applies — a message the account does not receive is
/// simply sent. The identity records are settled against the accepting instant first, which
/// materialises only the verification moves the clock already owed them, so what the rules are
/// evaluated against is the identity list a caller reading it at that instant is reported.
///
/// Called once the accepting request has decided everything it answers with, and with no lock of
/// this service held: a message is RECEIVED behind its acceptance, and nothing a caller is told
/// depends on what the rules do with it.
pub fn received_by_the_account<Eng: ::std::marker::Send + ::std::marker::Sync + 'static>(
    engine: &Eng,
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    accepted: &AcceptedSend<'_>,
) {
    use ::service_com_amazonaws_ses_simpleemailservice::state as receiving;

    let Some(state) = receiving::service_state(engine) else {
        return;
    };
    let received = {
        let mut state = state.lock();
        state.settle_identities(accepted.accepted_at);
        receiving::inbound_delivery::accepted(
            &mut state,
            accepted.region,
            accepted.message_id,
            accepted.sender,
            accepted.recipients,
            accepted.message,
            accepted.accepted_at,
        )
    };
    receiving::inbound_delivery::deliver_accepted(ctx, accepted.region, &received);
}

/// The RFC 5322 document Amazon SES assembles for a message a send handed it as PARTS — "you
/// specify the sender, the recipient, and the message body, and Amazon SES assembles the message
/// for you" — which is what the account's receive path saves.
///
/// Composed by the ONE composer of the account's email service
/// ([`::service_com_amazonaws_ses_simpleemailservice::state::composed_message`]) rather than by a
/// second one here, so a message this surface assembles and one the classic surface assembles are
/// the same document.
#[must_use]
pub fn assembled_message(parts: &AssembledParts<'_>) -> Vec<u8> {
    use ::service_com_amazonaws_ses_simpleemailservice::state::composed_message as composing;

    composing::compose(&composing::Composition {
        message_id: Some(&composing::message_id_header(
            parts.message_id,
            parts.region,
        )),
        from: Some(parts.from),
        to: parts.to,
        cc: parts.cc,
        reply_to: parts.reply_to,
        subject: parts.subject,
        headers: parts.headers,
        text: parts.text,
        html: parts.html,
        composed_at: parts.composed_at,
    })
    .into_bytes()
}

/// The parts ONE message is assembled from ([`assembled_message`]).
#[derive(Debug, Clone, Copy)]
pub struct AssembledParts<'a> {
    /// The Region the message was accepted in, which its `Message-ID` is scoped to.
    pub region: &'a str,
    /// The id the message was accepted under.
    pub message_id: &'a str,
    /// The sending identity's address.
    pub from: &'a str,
    /// The `To` addresses.
    pub to: &'a [String],
    /// The `Cc` addresses.
    pub cc: &'a [String],
    /// The `Reply-To` addresses.
    pub reply_to: &'a [String],
    /// The subject.
    pub subject: Option<&'a str>,
    /// The header fields the request asked for.
    pub headers: &'a [(String, String)],
    /// The text body part.
    pub text: Option<&'a str>,
    /// The HTML body part.
    pub html: Option<&'a str>,
    /// The instant the message was composed.
    pub composed_at: ::std::time::SystemTime,
}
