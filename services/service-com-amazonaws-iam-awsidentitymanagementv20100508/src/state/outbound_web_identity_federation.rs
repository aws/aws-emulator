// SPDX-License-Identifier: CC0-1.0

//! The account's OUTBOUND web identity federation: whether the feature is on, and the
//! issuer URL the account was given when it was first turned on.
//!
//! Three operations share this one record — `EnableOutboundWebIdentityFederation` turns the
//! feature on, `GetOutboundWebIdentityFederationInfo` reports it and
//! `DisableOutboundWebIdentityFederation` turns it off — so what the feature IS belongs to
//! none of them: the record ([`OutboundWebIdentityFederation`]), the transitions over it
//! and the wording of the two refusals its state decides are stated here once, and each
//! operation only reaches them. That is what makes the three answers one account-level
//! fact rather than three readings of it.
//!
//! The feature is an account-level FLAG, so there is one record per account and not a
//! collection: `Option` is the account that has never turned it on, and a record with its
//! flag cleared is the account that turned it on and off again — which keeps the issuer the
//! enable minted, because the issuer is "a unique issuer URL for your Amazon Web Services
//! account" (`EnableOutboundWebIdentityFederationResponse$IssuerIdentifier`), a property of
//! the ACCOUNT rather than of one enablement.

#[allow(unused_imports)]
use super::*;

/// The account's outbound web identity federation, as the account holds it.
///
/// Written by the enable and the disable, read by all three operations: nothing else in
/// this store decides what those calls answer.
#[derive(Debug, Clone)]
pub struct OutboundWebIdentityFederation {
    /// "A unique issuer URL for your Amazon Web Services account that hosts the OpenID
    /// Connect (OIDC) discovery endpoints"
    /// (`GetOutboundWebIdentityFederationInfoResponse$IssuerIdentifier`), minted once by the
    /// first enable ([`outbound_web_identity_issuer`]) and reported unchanged by every later
    /// call: a client that configured a trust relationship against it must still find the
    /// same issuer after the feature has been turned off and on again.
    pub issuer_identifier: String,
    /// Whether the feature is on right now — "whether outbound identity federation is
    /// currently enabled for your Amazon Web Services account"
    /// (`GetOutboundWebIdentityFederationInfoResponse$JwtVendingEnabled`). The enable sets
    /// it, the disable clears it, and it is what decides which of the two modelled refusals
    /// a call is answered with.
    pub enabled: bool,
    /// The keys this issuer signs with, and publishes the verifying half of at its own key
    /// set endpoint ([`IssuerSigningKeys`]).
    ///
    /// Minted with the issuer and kept with it, because a token STS vended against this
    /// issuer is verified against the key its `kid` names: the disable "does not affect
    /// tokens that were issued before the feature was disabled", so the key that signed
    /// one has to stay published after the flag is cleared and a second enable must not
    /// replace it.
    pub signing_keys: IssuerSigningKeys,
}

impl OutboundWebIdentityFederation {
    /// The JSON Web Key Set this issuer publishes: the PUBLIC half of every key it signs
    /// with, as the key set endpoint serves it.
    #[must_use]
    pub fn jwks(&self) -> ::serde_json::Value {
        self.signing_keys.to_key_set()
    }

    /// The OpenID Connect discovery document this issuer publishes, built on the issuer URL
    /// the account was given ([`openid_configuration`]).
    #[must_use]
    pub fn openid_configuration(&self) -> ::serde_json::Value {
        openid_configuration(&self.issuer_identifier)
    }

    /// The key a token signed under `algorithm` is signed with, CLONED out of the store, or
    /// `None` when this issuer publishes none for it.
    ///
    /// Owned rather than borrowed ON PURPOSE: signing is per-request work, so a caller must
    /// be able to give this store's guard up BEFORE it signs — a borrow would make it hold
    /// the lock across the signature, which would serialise every other request against this
    /// store behind one token mint. The clone costs nothing: a signing key's private half is
    /// behind an [`::std::sync::Arc`], so the clone and the stored key are one key and a
    /// token signed under either verifies against the one published entry.
    #[must_use]
    pub fn signing_key(
        &self,
        algorithm: &str,
    ) -> Option<::service_common::jws_signing::SigningKey> {
        self.signing_keys.for_algorithm(algorithm).cloned()
    }
}

/// How many hexadecimal digits the opaque segment of an account's issuer URL carries.
///
/// 32 digits is 128 bits of the digest below — the width of the opaque identifier AWS puts
/// in the issuer URL of an OIDC issuer it hosts for an account's resource, so a client
/// parsing the URL sees a segment of the same shape.
const ISSUER_IDENTIFIER_HEX_DIGITS: usize = 32;

/// The issuer URL an account is given the first time it enables the feature: `endpoint`
/// followed by one opaque segment that is this account's own.
///
/// `endpoint` is the base URL THIS emulator is served at
/// (`smithy_server_runtime::OperationContext::endpoint`), because the issuer is an address
/// the caller fetches later — the discovery documents the member's own documentation names
/// live under it — and an address this emulator announces has to lead back to this
/// emulator. Nothing of AWS's own hostnames is built here.
///
/// The segment is derived from the account id, so two accounts never share an issuer and
/// the derivation carries nothing a caller sent. It is still STORED by the enable rather
/// than re-derived per read ([`OutboundWebIdentityFederation::issuer_identifier`]): the
/// endpoint an in-process call reports and the one a served request reports need not be the
/// same string, and the account's issuer must not change under a client that configured a
/// trust relationship against it.
#[must_use]
pub fn outbound_web_identity_issuer(endpoint: &str, account_id: &str) -> String {
    let mut hasher = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut hasher, account_id.as_bytes());
    let digest = ::sha2::Digest::finalize(hasher);
    // The hex coding is [`data_encoding`]'s, the crate this service already spells base32
    // and its SSH key fingerprints with ([`super::ssh_public_keys`]); only how much of the
    // digest the segment carries is decided here.
    let segment =
        ::data_encoding::HEXUPPER.encode(&digest[..ISSUER_IDENTIFIER_HEX_DIGITS / 2]);
    format!("{}/{segment}", endpoint.trim_end_matches('/'))
}

/// Whether `segment` has the FORM of the opaque last segment of an account's issuer URL:
/// [`ISSUER_IDENTIFIER_HEX_DIGITS`] uppercase hexadecimal digits.
///
/// The form, not one account's value — it is what tells a request addressed to an issuer
/// THIS emulator hosts from one addressed to anything else, before any account's record is
/// read. Stated beside the derivation that produces it so the two cannot disagree.
#[must_use]
pub fn is_issuer_identifier_segment(segment: &str) -> bool {
    segment.len() == ISSUER_IDENTIFIER_HEX_DIGITS
        && segment
            .bytes()
            .all(|digit| digit.is_ascii_digit() || (b'A'..=b'F').contains(&digit))
}

/// How AWS words `FeatureDisabledException` for a call that needs the feature ON while the
/// account has it off: the account's outbound identity federation is disabled.
///
/// The only variable part is the account the call was served for, which is the account this
/// store stands for and never a value a caller sent — the three operations carry no input at
/// all.
#[must_use]
pub fn outbound_federation_disabled_message() -> String {
    format!(
        "Outbound identity federation is disabled for account {}",
        ::engine::emulator_account_id()
    )
}

/// How AWS words that same refusal for the DISABLE of an account that has the feature off
/// already: the account cannot disable it twice — "you cannot disable the feature multiple
/// times" (`FeatureDisabledException`). Its variable part is the account, as
/// [`outbound_federation_disabled_message`]'s is.
#[must_use]
pub fn outbound_federation_already_disabled_message() -> String {
    format!(
        "Outbound identity federation is already disabled for account {}",
        ::engine::emulator_account_id()
    )
}

/// How AWS words `FeatureEnabledException` for the ENABLE of an account that has the feature
/// on already: the account cannot enable it twice — "you cannot enable the feature multiple
/// times" (`FeatureEnabledException`). Its variable part is the account, as
/// [`outbound_federation_disabled_message`]'s is.
#[must_use]
pub fn outbound_federation_already_enabled_message() -> String {
    format!(
        "Outbound identity federation is already enabled for account {}",
        ::engine::emulator_account_id()
    )
}

impl ComAmazonawsIamAwsidentitymanagementv20100508State {
    /// The account's outbound web identity federation while the feature is ON, and `None`
    /// whenever it is off — an account that never enabled it and one that disabled it again
    /// are the same answer, because both have the feature off.
    ///
    /// The one reading of "the feature is enabled", so the report and the two refusals
    /// cannot disagree about which state the account is in. A READ: nothing is written.
    #[must_use]
    pub fn served_outbound_web_identity_federation(
        &self,
    ) -> Option<&OutboundWebIdentityFederation> {
        self.outbound_web_identity_federation
            .as_ref()
            .filter(|held| held.enabled)
    }

    /// Turn the feature ON for this account and answer the issuer URL it is served under,
    /// or `None` when the feature was already on.
    ///
    /// Minting the issuer is part of enabling — the operation "also generates a unique
    /// issuer URL for your Amazon Web Services account" — so the URL is minted here the
    /// first time and kept from then on: an account that turns the feature off and on again
    /// is served the issuer it already had, with the keys it already published.
    ///
    /// `endpoint` is where this emulator is served, which is what that URL is built on
    /// ([`outbound_web_identity_issuer`]). `keys` is the key material an account enabling
    /// the feature for the FIRST time publishes, generated by the caller OUTSIDE this
    /// mutation because generating one is CPU-bound ([`IssuerSigningKeys::generate`]); for an
    /// account that already holds an issuer it is dropped unused, so the keys a relying party
    /// fetched keep verifying the tokens it holds.
    ///
    /// `None` is what makes the one refusal this transition has exact under concurrency:
    /// whichever of two calls takes the store's lock first turns the feature on, and the
    /// other is told it was already enabled rather than both being served.
    pub fn enable_outbound_web_identity_federation(
        &mut self,
        endpoint: &str,
        keys: IssuerSigningKeys,
    ) -> Option<String> {
        if self.served_outbound_web_identity_federation().is_some() {
            return None;
        }
        let held = self
            .outbound_web_identity_federation
            .get_or_insert(OutboundWebIdentityFederation {
                issuer_identifier: outbound_web_identity_issuer(
                    endpoint,
                    ::engine::emulator_account_id(),
                ),
                enabled: false,
                // The issuer is minted here, and so is the key material it publishes: the
                // operation "also generates a unique issuer URL for your Amazon Web
                // Services account", and an issuer that announces discovery endpoints has
                // to have keys to publish at them.
                signing_keys: keys,
            });
        held.enabled = true;
        Some(held.issuer_identifier.clone())
    }

    /// Turn the feature OFF for this account, leaving the issuer it was given on the
    /// record.
    ///
    /// The disable "does not affect tokens that were issued before the feature was
    /// disabled", so it clears the flag and nothing else: the issuer stays, and a later
    /// enable reports that same issuer rather than a second one.
    pub fn disable_outbound_web_identity_federation(&mut self) {
        if let Some(held) = self.outbound_web_identity_federation.as_mut() {
            held.enabled = false;
        }
    }
}
