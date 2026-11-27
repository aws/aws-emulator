// SPDX-License-Identifier: CC0-1.0

//! The publisher surface of this service's state: a publisher of a domain, the association
//! that binds a registry record to one, and the values both are addressed by.
//!
//! A publisher is the domain a registry record is published under. The control plane for it
//! is four resources' worth of rules, all of which live here so that no two operations of
//! the surface word or derive the same thing differently:
//!
//! * the `VerificationStatus` a publisher's domain verification is in, and the DNS-01
//!   challenge the caller publishes to prove ownership of the domain;
//! * the `AssociationStatus` an association is in, and the curator's decision that moves it;
//! * the identifiers of both — the publisher's global (region-less) ARN and the record's
//!   public identifier, whose layout the model states in full; and
//! * how a request about either is refused.

use std::collections::BTreeMap;

use ::aws_smithy_types::DateTime;

use crate::ops::types::{DomainChallenge, Identifier};

use super::{DueTransition, ARN_SERVICE};

/// `VerificationStatus` of a publisher whose domain is not verified yet: "the registry is
/// waiting to find the DNS TXT record described by `domainChallenge`".
pub const PUBLISHER_ACTION_REQUIRED: &str = "ACTION_REQUIRED";
/// `VerificationStatus` of a publisher whose domain is verified. Only an `ACTIVE` publisher
/// accepts new registry record associations.
pub const PUBLISHER_ACTIVE: &str = "ACTIVE";
/// `VerificationStatus` of a publisher whose verified domain stopped verifying, because a
/// later check found the TXT record missing or changed.
pub const PUBLISHER_SUSPENDED: &str = "SUSPENDED";
/// `VerificationStatus` of a publisher whose verification did not succeed.
pub const PUBLISHER_FAILED: &str = "FAILED";
/// `VerificationStatus` of a publisher whose delete is in progress.
pub const PUBLISHER_DELETING: &str = "DELETING";

/// The statuses domain verification can be RESTARTED from: a publisher whose verification
/// did not succeed, and one whose verified domain stopped verifying. Every other status is
/// either a verification already settled or in flight, or a delete, so a restart asked for
/// in one of those is refused. The whole catalogue is
/// `crate::ops::types::VERIFICATION_STATUS_VALUES`.
pub const PUBLISHER_REVALIDATABLE_STATUSES: &[&str] = &[PUBLISHER_FAILED, PUBLISHER_SUSPENDED];

/// Whether domain verification can be restarted for a publisher in `status`.
pub fn publisher_accepts_revalidation(status: &str) -> bool {
    PUBLISHER_REVALIDATABLE_STATUSES.contains(&status)
}

/// `AssociationStatus` of a new association, which "starts in PENDING_APPROVAL until a
/// curator approves or rejects it".
pub const ASSOCIATION_PENDING_APPROVAL: &str = "PENDING_APPROVAL";
/// `AssociationStatus` of an association a curator approved.
pub const ASSOCIATION_APPROVED: &str = "APPROVED";
/// `AssociationStatus` of an association a curator rejected.
pub const ASSOCIATION_REJECTED: &str = "REJECTED";

/// The statuses an association accepts a curator's decision in: one waiting for a decision,
/// and the two a decision already reached — "repeating the decision the association already
/// holds changes nothing; the same approvalStatus with a different approvalStatusReason
/// updates the reason". Every other `AssociationStatus` (an association the service itself
/// suspended) is not a state that accepts one. The whole catalogue is
/// `crate::ops::types::ASSOCIATION_STATUS_VALUES`.
pub const ASSOCIATION_DECIDABLE_STATUSES: &[&str] = &[
    ASSOCIATION_PENDING_APPROVAL,
    ASSOCIATION_APPROVED,
    ASSOCIATION_REJECTED,
];

/// Whether an association in `status` accepts a curator's decision.
pub fn association_accepts_decision(status: &str) -> bool {
    ASSOCIATION_DECIDABLE_STATUSES.contains(&status)
}

/// The one `IdentifierType` the model declares — "`DOMAIN` is the only supported value",
/// which is also the member's modelled default. The catalogue is
/// `crate::ops::types::IDENTIFIER_TYPE_VALUES`.
pub const IDENTIFIER_TYPE_DOMAIN: &str = "DOMAIN";

/// How many publishers one account may hold before `CreatePublisher` answers the modelled
/// `ServiceQuotaExceededException`.
///
/// CHOSEN, not documented: AWS publishes no Publisher quota for this service, so the
/// emulator picks a generous one. It exists so a caller-driven store cannot grow without
/// bound.
pub const MAX_PUBLISHERS: usize = 1_000;

/// How many associations one publisher may hold before `AssociatePublisher` answers the
/// modelled `ServiceQuotaExceededException`. CHOSEN for the same reason as
/// [`MAX_PUBLISHERS`].
pub const MAX_ASSOCIATIONS_PER_PUBLISHER: usize = 1_000;

/// A publisher of a domain, exactly as `CreatePublisher` stored it.
///
/// A publisher is a GLOBAL resource: its ARN carries no region, and the account holds one
/// publisher per domain, so the store keys it by the domain rather than by a minted id.
#[derive(Debug)]
pub struct Publisher {
    /// The domain the publisher publishes under, as the create supplied it.
    pub domain: String,
    pub publisher_arn: String,
    pub status: String,
    pub status_reason: Option<String>,
    /// The challenge last issued for this domain. A new value is issued each time
    /// verification starts, and the earlier one no longer verifies the domain.
    pub domain_challenge: Option<DomainChallenge>,
    pub tags: BTreeMap<String, String>,
    pub created_at: DateTime,
    pub updated_at: DateTime,
    pub pending: Option<DueTransition<PublisherTransition>>,
}

impl Publisher {
    /// This publisher's `Identifier`: the union the model declares, whose `domain` member is
    /// the only one GA supports.
    pub fn identifier(&self) -> Identifier {
        Identifier::Domain(self.domain.clone())
    }

    /// This publisher's tags as the modelled map, or `None` where it carries none — an
    /// absent member rather than an empty map, so a read reports a tag set only where one
    /// was written.
    pub fn tag_map(&self) -> Option<std::collections::HashMap<String, String>> {
        (!self.tags.is_empty()).then(|| self.tags.clone().into_iter().collect())
    }
}

/// What a publisher's pending transition turns it into.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PublisherTransition {
    /// `ACTION_REQUIRED` -> `ACTIVE`: the registry found the challenge record, so the
    /// domain is verified.
    Verified,
    /// `DELETING` -> gone: the publisher is removed.
    Removed,
}

/// The key a publisher is stored and looked up under: the domain, lowercased.
///
/// A DNS name is case-insensitive, so one account cannot hold two publishers whose domains
/// differ only in case — they name the same domain. The value a read reports is the one the
/// create supplied; only the key is folded.
pub fn publisher_key(domain: &str) -> String {
    domain.to_lowercase()
}

/// The ARN of a publisher: `arn:aws:agent-registry::<account>:<domain>`.
///
/// `PublisherIdentifierArn` is the ARN of a GLOBAL resource, so the region field is empty,
/// and "the resource segment carries the identifier value, which may contain dots".
pub fn publisher_arn(domain: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, "", domain)
}

/// The domain a `PublisherIdentifierArn` names, or `None` when the value is not the ARN of a
/// publisher this account could hold: the resource segment of a region-less ARN of this
/// service's namespace in the account the emulator serves.
///
/// The account is part of what is resolved because a publisher is a global resource whose
/// ARN carries one: an ARN of ANOTHER account names a publisher shared with this account,
/// which this emulator holds none of, so it resolves to nothing rather than to the
/// same-named publisher of the account it does serve.
///
/// Splitting the value into its ARN fields is `service-common`'s single ARN parse, the same
/// one the tagging surface resolves its `resourceArn` through.
pub fn publisher_domain_of_arn(arn: &str) -> Option<String> {
    let parsed = ::service_common::arn::parse(arn)?;
    let partitioned = parsed.partition == "aws" || parsed.partition.starts_with("aws-");
    if parsed.vendor != ARN_SERVICE
        || !partitioned
        || parsed.account != ::engine::emulator_account_id()
        || !parsed.region.is_empty()
        || parsed.resource.is_empty()
    {
        return None;
    }
    Some(parsed.resource.to_string())
}

/// The DNS-01 challenge a caller publishes to prove ownership of `domain`: the name of the
/// TXT record to create, and the value to publish in it.
///
/// The record name is the challenge label on the domain itself, as the member documents
/// (`_acme-challenge.example.com`). The value is server-generated and a NEW one is issued
/// every time verification starts, so two challenges for one domain never share it.
pub fn domain_challenge_for(domain: &str) -> DomainChallenge {
    DomainChallenge {
        record_name: format!("{CHALLENGE_LABEL}.{domain}"),
        record_value: super::mint_challenge_value(),
    }
}

/// The DNS label a domain's challenge record sits at, as the member's own example states it.
const CHALLENGE_LABEL: &str = "_acme-challenge";

/// The public identifier an association assigns a registry record, in the layout the model
/// states for it: `urn:air:[<subdomain>.]<domain>:<namespace>:<identity>[;<kind>]`.
///
/// * the authority is the publisher's domain with the association's optional subdomain
///   folded in as a leading label;
/// * the namespace is the association's categories, in the order they were given, each its
///   own `:`-separated segment and absent altogether when none were supplied;
/// * the identity is the registry record's name; and
/// * the kind is the artifact kind of the record's `recordType` ([`record_kind`]), appended
///   after `;` and absent for a record type that has none.
///
/// It is a URN and not an ARN: the identifier is a public-facing, provider-neutral name.
pub fn public_identifier(
    domain: &str,
    subdomain: Option<&str>,
    categories: &[String],
    record_name: &str,
    record_type: &str,
) -> String {
    let authority = match subdomain {
        Some(label) => format!("{label}.{domain}"),
        None => domain.to_string(),
    };
    let mut identifier = format!("urn:air:{authority}");
    for category in categories {
        identifier.push(':');
        identifier.push_str(category);
    }
    identifier.push(':');
    identifier.push_str(record_name);
    if let Some(kind) = record_kind(record_type) {
        identifier.push(';');
        identifier.push_str(kind);
    }
    identifier
}

/// The artifact kind a public identifier carries for a record of `record_type`, or `None`
/// for a record type that carries none.
///
/// The mapping is the one the model states for the identifier's `<kind>` segment: `MCP` is
/// `mcp`, `AGENT` and `CUSTOM` are `agent`, `SKILL` is `skill`, `WEB_BOT` is `webbot`, and
/// `GATEWAY` has no kind. A `RecordType` outside that list carries none either, rather than
/// a kind invented for it. The whole catalogue is `crate::ops::types::RECORD_TYPE_VALUES`.
pub fn record_kind(record_type: &str) -> Option<&'static str> {
    match record_type {
        "MCP" => Some("mcp"),
        "AGENT" | "CUSTOM" => Some("agent"),
        "SKILL" => Some("skill"),
        "WEB_BOT" => Some("webbot"),
        _ => None,
    }
}

/// How this service states that the PUBLISHER an ARN named is not there.
///
/// The ONE place this sentence is written, so no two operations of this surface word the
/// same error class differently. Its fixed text names the resource class and the kind of
/// identifier a publisher is addressed by; the one variable part is the ARN exactly as the
/// request supplied it.
pub fn publisher_not_found_message(publisher_arn: &str) -> String {
    format!("Publisher with ARN {publisher_arn} not found.")
}

/// How this service states that a publisher already exists for a domain a create asked for:
/// "your account can have one publisher per domain". The variable part is the domain as the
/// request supplied it.
pub fn duplicate_publisher_message(domain: &str) -> String {
    format!("A publisher for domain '{domain}' already exists in this account")
}

/// How this service refuses an association for a publisher that is not verified yet: the
/// publisher's ARN, the status it is in, and the status it has to be in. "Only an `ACTIVE`
/// publisher accepts new registry record associations."
pub fn publisher_not_active_message(publisher_arn: &str, status: &str) -> String {
    format!(
        "Publisher {publisher_arn} is in status {status}; a registry record can be associated \
         once it is {PUBLISHER_ACTIVE}"
    )
}

/// How this service refuses a delete of a publisher that still holds associations: the
/// publisher's ARN, how many associations it holds, and what the caller has to do about it.
pub fn publisher_has_associations_message(publisher_arn: &str, associations: usize) -> String {
    format!(
        "Cannot delete publisher '{publisher_arn}' because it has {associations} registry \
         record associations. Disassociate every registry record from it before deleting the \
         publisher."
    )
}

/// How this service refuses a change to a publisher whose delete is already in flight: the
/// publisher's ARN and the status that delete left it in.
pub fn publisher_deleting_message(publisher_arn: &str) -> String {
    format!(
        "Publisher '{publisher_arn}' is in status {PUBLISHER_DELETING}; a delete is already in \
         progress"
    )
}

/// How this service refuses a restart of verification for a publisher that is not in a
/// status it can be restarted from: the publisher's ARN, the status it is in, and the
/// statuses a restart is made from, in the order [`PUBLISHER_REVALIDATABLE_STATUSES`] states
/// them.
pub fn publisher_not_revalidatable_message(publisher_arn: &str, status: &str) -> String {
    format!(
        "Cannot restart domain verification for publisher {publisher_arn} in status {status}. \
         Verification restarts from: {}",
        PUBLISHER_REVALIDATABLE_STATUSES.join(", ")
    )
}

/// How this service refuses an association for a registry record that is not approved: the
/// record's ARN, the status it is in, and the status it has to be in. "The registry record
/// must be APPROVED."
pub fn record_not_approved_message(record_arn: &str, status: &str) -> String {
    format!(
        "Registry record {record_arn} is in status {status}; it can be associated with a \
         publisher once it is {}",
        super::RECORD_APPROVED
    )
}

/// How this service refuses a second association for a registry record: "a registry record
/// can have only one publisher association". The variable parts are the record's ARN and the
/// publisher it is already associated with.
pub fn record_already_associated_message(record_arn: &str, publisher_arn: &str) -> String {
    format!(
        "Registry record {record_arn} is already associated with publisher {publisher_arn}; a \
         registry record can have only one publisher association"
    )
}

/// How this service refuses an association whose public identifier another registry record
/// already holds: the identifier, and the record holding it.
pub fn public_identifier_taken_message(public_identifier: &str, record_arn: &str) -> String {
    format!(
        "Public identifier {public_identifier} is already held by registry record {record_arn}"
    )
}

/// How this service refuses a curator's decision on an association that is not in a state
/// that accepts one: the record the association binds, the status it is in, and the statuses
/// a decision is accepted in, in the order [`ASSOCIATION_DECIDABLE_STATUSES`] states them.
pub fn association_not_decidable_message(record_arn: &str, status: &str) -> String {
    format!(
        "Cannot record an approval decision for the publisher association of registry record \
         {record_arn} in status {status}. A decision is accepted in: {}",
        ASSOCIATION_DECIDABLE_STATUSES.join(", ")
    )
}

/// How this service states that the caller has no publisher association for a registry
/// record, which is what an operation on somebody else's association — or on one that is not
/// there — is refused with. The one variable part is the record's ARN as the request supplied
/// it.
pub fn no_association_message(record_arn: &str) -> String {
    format!("No publisher association for registry record {record_arn} in this account.")
}
