// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-pcaconnectorscep-pcaconnectorscep service.
//!
//! The service's resources are the SCEP **connector** and the **challenge** passwords that
//! hang off a general-purpose connector. Every implemented operation computes its answer
//! from the records below: nothing is canned, nothing is echoed straight back out of a
//! request without being stored first, and a delete is observable by the next reader.

use std::collections::BTreeMap;
use std::hash::{BuildHasher as _, Hasher as _};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{LazyLock, OnceLock};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ::aws_smithy_schema::serde::{SerdeError, SerializableStruct, ShapeSerializer};
use ::aws_smithy_schema::{Schema, ShapeId, ShapeType, TraitMap};
use ::aws_smithy_schema_server::constraints::{validate, LengthTrait, PatternTrait, Violation};
use ::aws_smithy_schema_server::envelope;

use crate::ops::types::{
    Challenge, ChallengeMetadata, ChallengeMetadataSummary, Connector, ConnectorSummary,
    MobileDeviceManagement, OpenIdConfiguration, ResourceNotFoundException,
};

/// The refusals the resource-tagging endpoints answer a `ResourceArn` they cannot parse, and
/// an ARN naming no resource, with — the tagging layer's own error classes rather than any of
/// this service's.
mod tagging_refusal;

/// How a client ADDRESSES a connector's SCEP plane: the hosts and URLs a connector is
/// published at, and the reverse readings that say which connector — and which access path —
/// an address names.
mod addressing;
pub use addressing::*;

pub use tagging_refusal::{
    TaggingError, TaggingRefusal, TAG_BAD_REQUEST_ERROR_CODE, TAG_BAD_REQUEST_MESSAGE,
    TAG_BAD_REQUEST_STATUS, TAG_LIMIT_EXCEEDED_MESSAGE, TAG_NOT_FOUND_ERROR_CODE,
    TAG_NOT_FOUND_MESSAGE, TAG_NOT_FOUND_STATUS,
};

/// The signing name, endpoint prefix and ARN namespace of this service, from the model's
/// `aws.auth#sigv4` / `aws.api#service` traits (all three are `pca-connector-scep`).
pub const ARN_SERVICE: &str = "pca-connector-scep";

/// Region used when a request carries no SigV4 credential scope to read one from (an
/// unsigned request still routes by endpoint host).
pub const DEFAULT_REGION: &str = "us-east-1";

/// `ConnectorStatus` value a freshly created connector carries. The full enum
/// (smithy_query: shape ConnectorStatus) is CREATING, ACTIVE, DELETING, FAILED.
pub const STATUS_CREATING: &str = "CREATING";
/// The ready state: a connector whose SCEP endpoint serves and that can mint challenges.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// `ConnectorStatus` value a connector carries while `DeleteConnector`'s teardown runs: the
/// delete answers 202 and only accepts the work, so the connector is reported in this status
/// until the teardown completes and the record (with its challenges) disappears.
pub const STATUS_DELETING: &str = "DELETING";
/// The terminal status of a connector creation that did not come up. It is the one status
/// that carries a `StatusReason` — "Information about why connector creation failed, if
/// status is FAILED" (the `StatusReason` member's own doc).
pub const STATUS_FAILED: &str = "FAILED";

/// `ConnectorType` value of a connector created without `MobileDeviceManagement` — the
/// member's own doc: "If you don't supply a value, by default Connector for SCEP creates a
/// connector for general-purpose use."
pub const TYPE_GENERAL_PURPOSE: &str = "GENERAL_PURPOSE";
/// `ConnectorType` value of a connector created with an `IntuneConfiguration`.
pub const TYPE_INTUNE: &str = "INTUNE";

/// `ResourceId`/`ResourceType` vocabulary of this service's errors: the `ResourceType`
/// member's doc says it "can be either `Connector` or `Challenge`", which are the two kinds
/// of resource this service owns. Those two words are what a call ADDRESSED AT one of those
/// resources — every read and delete of this service — reports when it cannot find it
/// (see [`resource_not_found`]). A create reports the input member that carried the reference
/// it could not resolve instead ([`reference_not_found`]), because the resource that call is
/// about does not exist yet.
pub const RESOURCE_TYPE_CONNECTOR: &str = "Connector";
/// The other value `ResourceType` carries.
pub const RESOURCE_TYPE_CHALLENGE: &str = "Challenge";

/// `ValidationExceptionReason` for a call that names a connector whose status does not
/// allow it (smithy_query: shape ValidationExceptionReason).
pub const REASON_INVALID_STATE: &str = "INVALID_STATE";
/// `ValidationExceptionReason` for a call that does not apply to the connector's `Type`.
pub const REASON_INVALID_CONNECTOR_TYPE: &str = "INVALID_CONNECTOR_TYPE";

/// `ConnectorStatusReason` of a connector whose private CA could not be found (smithy_query:
/// shape ConnectorStatusReason).
pub const REASON_PRIVATECA_RESOURCE_NOT_FOUND: &str = "PRIVATECA_RESOURCE_NOT_FOUND";
/// `ConnectorStatusReason` of a connector that was not allowed to use the private CA it
/// names (smithy_query: shape ConnectorStatusReason).
pub const REASON_PRIVATECA_ACCESS_DENIED: &str = "PRIVATECA_ACCESS_DENIED";
/// `ConnectorStatusReason` of a connector whose private CA exists but was in a status it
/// could not be used from (smithy_query: shape ConnectorStatusReason).
pub const REASON_PRIVATECA_INVALID_STATE: &str = "PRIVATECA_INVALID_STATE";
/// `ConnectorStatusReason` of a connector whose `VpcEndpointId` named no VPC endpoint
/// (smithy_query: shape ConnectorStatusReason).
pub const REASON_VPC_ENDPOINT_RESOURCE_NOT_FOUND: &str = "VPC_ENDPOINT_RESOURCE_NOT_FOUND";
/// `ConnectorStatusReason` of a connector whose VPC endpoint exists but publishes no DNS
/// entry for a client to resolve it by (smithy_query: shape ConnectorStatusReason).
pub const REASON_VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND: &str = "VPC_ENDPOINT_DNS_ENTRIES_NOT_FOUND";

/// How long a connector stays in a non-terminal status before the clock settles it:
/// `CREATING` before it owes the status its creation lands in (`ACTIVE`, or `FAILED` for a
/// creation that cannot come up), and `DELETING` before the record disappears.
///
/// `CreateConnector` and `DeleteConnector` both answer HTTP 202 and the connector's own
/// `Status` enum carries `CREATING` and `DELETING`, so both are asynchronous: a resource the
/// service reports in a non-terminal status has a transition the clock owes. The span is the
/// emulator's own choice (no AWS-documented duration applies to connector provisioning or
/// teardown) and is deliberately non-zero, so a caller reading straight after the call still
/// observes the transient status — and the state errors that go with it — as it does against
/// the real service. The instant it comes due is stored on the record
/// ([`PendingTransition::due_at`]) and materialised by [`StoredConnector::settle`];
/// nothing sleeps.
pub const TRANSITION_SPAN: Duration = Duration::from_millis(500);

/// How long a `ClientToken` stays bound to the resource it created.
///
/// Documented on the member: "Client tokens for CreateChallenge time out after five
/// minutes. Therefore, if you call CreateChallenge multiple times with the same client
/// token within five minutes, Connector for SCEP recognizes that you are requesting only
/// one challenge and will only respond with one."
pub const CLIENT_TOKEN_TTL: Duration = Duration::from_secs(5 * 60);

/// `MaxResults` upper bound on both listings, from their `@range(1, 1000)`. Also the page
/// size used when a caller names none.
pub const MAX_RESULTS_LIMIT: i32 = 1000;

/// How many connectors this emulator will hold for one account in one Region.
///
/// The real service caps connectors per account per Region with a service quota, and
/// `CreateConnector` models `ServiceQuotaExceededException` for exceeding it. The number
/// here is NOT a fidelity claim about the published quota: it is the emulator's own
/// per-Region ceiling, set generously so no realistic workflow meets it.
pub const MAX_CONNECTORS: usize = 1_000;

/// How many connectors this emulator will hold AT ALL, across every Region.
///
/// The Region a connector is filed under comes from the request ([`request_region`]), so the
/// per-Region ceiling above bounds one Region's inventory but not the number of Regions a
/// caller addresses. This bounds the product, and with it the memory a caller looping on
/// create can make this process hold: it is the ceiling that keeps that growth finite, and
/// it is reached only by a caller inventing Regions, never by a workflow.
pub const MAX_CONNECTORS_TOTAL: usize = 4 * MAX_CONNECTORS;

/// How long a Region name this service will file a record under may be.
///
/// Neither ARN pattern bounds the length of its Region segment, so this is the emulator's
/// own bound on a value the request chooses, well above every published Region name.
pub const MAX_REGION_LENGTH: usize = 64;

/// The same ceiling for the challenge passwords of one connector, for the same reason.
pub const MAX_CHALLENGES_PER_CONNECTOR: usize = 1_000;

/// How many tags one resource may carry.
///
/// `Tags` carries no `@length`, so the model states no bound; AWS's tagging limit of 50 tags
/// per resource is what this enforces, which is also what keeps a caller from making one
/// create call cost unbounded memory.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// How long a tag key may be. `Tags` carries no `@length` on its key, so the bound is the
/// AWS-wide tagging one: a key is 1 to 128 characters.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// How long a tag value may be: 0 to 256 characters, the AWS-wide bound. An EMPTY value is
/// a tag a caller may set, which is why only the key has a lower bound.
pub const MAX_TAG_VALUE_LENGTH: usize = 256;

/// The tag-key namespace Amazon Web Services keeps for the tags it sets itself, which a
/// caller may therefore not write. Matched without regard to case, so no spelling of it
/// gets a caller-set tag into the namespace.
pub const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// `ServiceCode` carried by this service's `ServiceQuotaExceededException`: the originating
/// service, which is this service's own endpoint prefix.
pub const SERVICE_CODE: &str = ARN_SERVICE;

/// `QuotaCode` for [`MAX_CONNECTORS`]. AWS's own `L-`code for the limit is not in the
/// model, so the emulator names the limit it enforces rather than inventing a code.
pub const QUOTA_CODE_CONNECTORS: &str = "ConnectorsPerAccountPerRegion";

/// `QuotaCode` for [`MAX_CHALLENGES_PER_CONNECTOR`], named for the same reason.
pub const QUOTA_CODE_CHALLENGES: &str = "ChallengesPerConnector";

/// The separator between a connector ARN and the challenge segment of a challenge ARN, from
/// `ChallengeArn`'s own pattern (`…:connector/<uuid>/challenge/<uuid>$`).
const CHALLENGE_SEGMENT: &str = "/challenge/";

/// A status change the clock owes a connector: where it lands, and when.
#[derive(Debug, Clone)]
pub struct PendingTransition {
    /// What the clock owes this connector.
    pub outcome: PendingOutcome,
    /// The instant the transition comes due.
    pub due_at: SystemTime,
}

/// Where a pending transition leaves the connector.
///
/// A connector in a non-terminal `ConnectorStatus` either settles into another status
/// (`CREATING` -> `ACTIVE`, or -> `FAILED` for a creation that cannot come up) or ceases to
/// exist (`DELETING` -> gone, taking its challenges with it), so those are the only two
/// outcomes the clock can owe.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum PendingOutcome {
    /// The connector settles into this `ConnectorStatus` value, carrying this
    /// `ConnectorStatusReason` — which only the failure status has one of, because the
    /// member reports "why connector creation failed".
    Status {
        status: String,
        reason: Option<String>,
    },
    /// The teardown completes: the record, its challenges and its tags are gone.
    Removed,
}

/// A `ClientToken` and the resource the call carrying it created.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// ARN of the connector or challenge that token created.
    pub arn: String,
    /// When the token was first seen; it stops binding [`CLIENT_TOKEN_TTL`] later.
    pub created_at: SystemTime,
    /// A rendering of the resource-defining inputs the call carried, so a retry carrying
    /// the same token can be checked to be the SAME request: a token answers for the
    /// resource it created only when the inputs still match.
    pub request: String,
}

/// A stored challenge password: the modelled `Challenge`, as state.
#[derive(Debug, Clone)]
pub struct StoredChallenge {
    pub arn: String,
    pub connector_arn: String,
    /// The SCEP challenge password, in UUID format (the `Password` member's own doc). It is
    /// `@sensitive`, so only `CreateChallenge` and `GetChallengePassword` return it — never
    /// a metadata listing.
    pub password: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags `CreateChallenge` was given. They live ON the record, so they are dropped
    /// with it and a deleted challenge can never leave a tag entry behind.
    pub tags: BTreeMap<String, String>,
}

impl StoredChallenge {
    /// The modelled `Challenge`, as `CreateChallenge` returns it — password included.
    pub fn to_challenge(&self) -> Challenge {
        Challenge {
            arn: Some(self.arn.clone()),
            connector_arn: Some(self.connector_arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
            password: Some(self.password.clone()),
        }
    }

    /// The modelled `ChallengeMetadataSummary`, as `ListChallengeMetadata` returns it.
    /// `ChallengeMetadataSummary` models no password member, so metadata never carries one.
    pub fn to_metadata_summary(&self) -> ChallengeMetadataSummary {
        ChallengeMetadataSummary {
            arn: Some(self.arn.clone()),
            connector_arn: Some(self.connector_arn.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// The modelled `ChallengeMetadata`, as `GetChallengeMetadata` returns it. Its members
    /// are the same four `ChallengeMetadataSummary` carries, and neither shape models a
    /// password, so the single read and the listing report one record identically.
    pub fn to_metadata(&self) -> ChallengeMetadata {
        let summary = self.to_metadata_summary();
        ChallengeMetadata {
            arn: summary.arn,
            connector_arn: summary.connector_arn,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// A stored connector: everything `CreateConnector` was given, plus what the service minted.
#[derive(Debug, Clone)]
pub struct StoredConnector {
    pub arn: String,
    pub certificate_authority_arn: String,
    /// A `ConnectorType` value: [`TYPE_GENERAL_PURPOSE`] or [`TYPE_INTUNE`].
    pub connector_type: String,
    /// Echo of the stored MDM settings; `None` for a general-purpose connector, whose
    /// `MobileDeviceManagement` the model documents as empty.
    pub mobile_device_management: Option<MobileDeviceManagement>,
    /// OIDC parameters, minted for an Intune connector only.
    pub open_id_configuration: Option<OpenIdConfiguration>,
    /// The VPC endpoint the connector is reachable through, when the create named one.
    pub vpc_endpoint_id: Option<String>,
    /// A `ConnectorStatus` value.
    pub status: String,
    /// A `ConnectorStatusReason` value — only set when `status` is `FAILED`, per the
    /// `StatusReason` member's doc.
    pub status_reason: Option<String>,
    /// The transition the clock owes, or `None` in a settled status.
    pub pending: Option<PendingTransition>,
    /// The connector's HTTPS SCEP URL — derived from the access path the create chose, so a
    /// connector restricted to one VPC endpoint reports a different host from a public one.
    pub endpoint: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// The tags `CreateConnector` was given. They live ON the record, so they are dropped
    /// with it and a deleted connector can never leave a tag entry behind.
    pub tags: BTreeMap<String, String>,
    /// This connector's challenge passwords, keyed by challenge ARN and ordered by it, so
    /// `ListChallengeMetadata` paginates deterministically.
    pub challenges: BTreeMap<String, StoredChallenge>,
}

impl StoredConnector {
    /// Materialise the transition the clock already owes this connector, answering whether
    /// the record survives it (`false` once a teardown has come due, so the caller drops it).
    ///
    /// This is the ONLY write a reporting operation makes: the change was owed at this
    /// instant whether or not anybody called.
    fn settle(&mut self, now: SystemTime) -> bool {
        let Some(pending) = self.pending.clone() else {
            return true;
        };
        if now < pending.due_at {
            return true;
        }
        match pending.outcome {
            PendingOutcome::Status { status, reason } => {
                self.status = status;
                // The change happened when the clock made it due, not when someone looked.
                self.updated_at = pending.due_at.max(self.updated_at);
                self.status_reason = reason;
                self.pending = None;
                true
            }
            // The teardown is complete, so there is no record left to report.
            PendingOutcome::Removed => false,
        }
    }

    /// Accept a delete: the connector moves into `DELETING` and the clock owes its removal.
    ///
    /// `DeleteConnector` answers 202 with an empty body — it only accepts the work — so this
    /// is correct whatever non-terminal status the connector is in: a pending `ACTIVE` is
    /// replaced by the teardown rather than being waited for. A connector already tearing
    /// down keeps the due instant it was given, so repeating the `@idempotent` delete can
    /// neither extend the teardown nor restart it.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        if self.is_deleting() {
            return;
        }
        self.status = STATUS_DELETING.to_string();
        // `StatusReason` says why a connector FAILED; one being torn down carries none.
        self.status_reason = None;
        self.updated_at = now;
        self.pending = Some(PendingTransition {
            outcome: PendingOutcome::Removed,
            due_at: now + TRANSITION_SPAN,
        });
    }

    /// True while a transition is still owed — the connector is mid-flight.
    pub fn is_in_transition(&self) -> bool {
        self.pending.is_some()
    }

    /// True once the connector is in the one status that lets a caller use it.
    pub fn is_active(&self) -> bool {
        self.status == STATUS_ACTIVE
    }

    /// True while the connector's teardown is running: it is still reported, but nothing can
    /// be built on it and it is about to disappear.
    pub fn is_deleting(&self) -> bool {
        self.status == STATUS_DELETING
    }

    /// True when the connector is of the `ConnectorType` whose challenge passwords this
    /// service mints itself, as opposed to one whose passwords its mobile device management
    /// holds.
    pub fn is_general_purpose(&self) -> bool {
        self.connector_type == TYPE_GENERAL_PURPOSE
    }

    /// The modelled `ConnectorSummary`, as `ListConnectors` returns it. This is the ONE place
    /// the stored record is projected onto the connector output shapes.
    pub fn to_summary(&self) -> ConnectorSummary {
        ConnectorSummary {
            arn: Some(self.arn.clone()),
            certificate_authority_arn: Some(self.certificate_authority_arn.clone()),
            r#type: Some(self.connector_type.clone()),
            mobile_device_management: self.mobile_device_management.clone(),
            open_id_configuration: self.open_id_configuration.clone(),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            endpoint: Some(self.endpoint.clone()),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// The modelled `Connector`, as `GetConnector` returns it. Its members are the same ten
    /// `ConnectorSummary` carries, so it is built FROM [`Self::to_summary`] rather than
    /// re-listing them: the single read and the listing report one record identically, and a
    /// member newly reported or newly derived reaches both at once.
    pub fn to_connector(&self) -> Connector {
        let summary = self.to_summary();
        Connector {
            arn: summary.arn,
            certificate_authority_arn: summary.certificate_authority_arn,
            r#type: summary.r#type,
            mobile_device_management: summary.mobile_device_management,
            open_id_configuration: summary.open_id_configuration,
            status: summary.status,
            status_reason: summary.status_reason,
            endpoint: summary.endpoint,
            created_at: summary.created_at,
            updated_at: summary.updated_at,
        }
    }
}

/// Per-service state for com-amazonaws-pcaconnectorscep-pcaconnectorscep. A handler reaches it through [`service_state`].
///
/// This service is regional: a resource exists in exactly one Region — the one named by its
/// own ARN — so every record below is reached through an accessor that takes the Region the
/// request was signed for ([`request_region`]) and answers only for records of that Region.
/// A handler never indexes the maps directly, which is what keeps one Region's endpoint from
/// reporting, resolving or deleting another Region's resources.
#[derive(Debug, Default)]
pub struct ComAmazonawsPcaconnectorscepPcaconnectorscepState {
    /// Every connector of every Region, keyed by its ARN and ordered by it, so a listing
    /// paginates deterministically. The ARN carries the Region the connector exists in, so a
    /// Region's own inventory is the entries whose ARN names it
    /// ([`ComAmazonawsPcaconnectorscepPcaconnectorscepState::connectors_in`]). A connector
    /// owns its challenges and both own their tags, so deleting a connector drops all three
    /// and leaves nothing behind.
    pub connectors: BTreeMap<String, StoredConnector>,
    /// `CreateConnector`'s (Region, `ClientToken`) -> the connector ARN it created. The
    /// Region is half the key because a token identifies a retry of a call made against ONE
    /// Region's endpoint: the same token sent to another Region asks for a connector there.
    /// Read and written through [`token_replay`] and [`remember_token`], which is where the
    /// rule both creates apply to a reused token lives.
    pub connector_tokens: IdempotencyTable,
    /// `CreateChallenge`'s (Region, `ClientToken`) -> the challenge ARN it created, reached
    /// through the same two helpers.
    pub challenge_tokens: IdempotencyTable,
}

impl ComAmazonawsPcaconnectorscepPcaconnectorscepState {
    /// Materialise everything the clock already owes: each connector's status transition or
    /// completed teardown, and the expiry of every `ClientToken` past its five-minute window.
    ///
    /// Called at the top of each handler, reporting ones included: they may write only a
    /// change that was already due at this instant.
    pub fn settle(&mut self, now: SystemTime) {
        // A connector whose teardown has come due is gone, challenges and tags with it.
        self.connectors.retain(|_, connector| connector.settle(now));
        let live = |record: &IdempotencyRecord| record.created_at + CLIENT_TOKEN_TTL > now;
        self.connector_tokens.retain(|_, record| live(record));
        self.challenge_tokens.retain(|_, record| live(record));
    }

    /// The connectors that exist in `region`, in ARN order: the stored records whose ARN
    /// names that Region, which is the whole inventory a call signed for it can see.
    ///
    /// Every ARN this service mints starts with the same prefix for a given Region
    /// ([`connector_arn_prefix`]), and the map is keyed by ARN, so one Region's records are a
    /// contiguous range: this walks that range and stops at its end rather than scanning the
    /// other Regions' records.
    pub fn connectors_in<'a>(
        &'a self,
        region: &str,
    ) -> impl Iterator<Item = (&'a String, &'a StoredConnector)> {
        let prefix = connector_arn_prefix(region);
        self.connectors
            .range(prefix.clone()..)
            .take_while(move |(arn, _)| arn.starts_with(&prefix))
    }

    /// The connector `connector_arn` names, as a call signed for `region` sees it: `None`
    /// when no record is stored under that ARN, and equally when the ARN names a connector
    /// of another Region, because that connector does not exist here.
    pub fn connector(&self, region: &str, connector_arn: &str) -> Option<&StoredConnector> {
        self.connectors
            .get(connector_arn)
            .filter(|_| arn_region(connector_arn) == Some(region))
    }

    /// The mutable twin of [`Self::connector`], refusing a foreign-Region ARN the same way,
    /// so no call can change — or tear down — a resource of a Region it was not signed for.
    pub fn connector_mut(
        &mut self,
        region: &str,
        connector_arn: &str,
    ) -> Option<&mut StoredConnector> {
        if arn_region(connector_arn) != Some(region) {
            return None;
        }
        self.connectors.get_mut(connector_arn)
    }

    /// The challenge stored under `challenge_arn`, found through the connector its ARN names
    /// — so a challenge is reachable only in the Region that connector exists in.
    pub fn challenge(&self, region: &str, challenge_arn: &str) -> Option<&StoredChallenge> {
        let connector_arn = connector_arn_of_challenge(challenge_arn)?;
        self.connector(region, connector_arn)?
            .challenges
            .get(challenge_arn)
    }

    /// Remove the challenge `challenge_arn` names, answering whether it was there — in
    /// `region`, which is the only Region its owning connector exists in.
    pub fn remove_challenge(&mut self, region: &str, challenge_arn: &str) -> bool {
        let Some(connector_arn) = connector_arn_of_challenge(challenge_arn) else {
            return false;
        };
        self.connector_mut(region, connector_arn)
            .is_some_and(|connector| connector.challenges.remove(challenge_arn).is_some())
    }

    /// The connector a `ResourceArn` of either kind hangs off: the connector itself for a
    /// connector ARN, and the owning connector for a challenge ARN — the record whose
    /// lifecycle governs both, because a connector owns its challenges and takes them with
    /// it ([`Self::connectors`]).
    ///
    /// `Some` as soon as that connector exists in `region`, even when the challenge segment
    /// names no stored challenge, so a caller that needs the resource itself to exist asks
    /// for it ([`Self::tags_of`], [`Self::challenge`]) rather than inferring it from here.
    pub fn owning_connector(&self, region: &str, resource_arn: &str) -> Option<&StoredConnector> {
        let connector_arn = connector_arn_of_challenge(resource_arn).unwrap_or(resource_arn);
        self.connector(region, connector_arn)
    }

    /// The tags of the connector or challenge `resource_arn` names, or `None` when this
    /// service holds no resource under that ARN.
    ///
    /// Which of the two kinds an ARN names follows from its shape ([`resource_type_of`]), and
    /// the tags come off that record — both kinds carry their own map — so a tag listing is
    /// computed from the resource itself and disappears with it.
    pub fn tags_of(
        &self,
        region: &str,
        resource_arn: &str,
    ) -> Option<&BTreeMap<String, String>> {
        if connector_arn_of_challenge(resource_arn).is_some() {
            self.challenge(region, resource_arn)
                .map(|challenge| &challenge.tags)
        } else {
            self.connector(region, resource_arn)
                .map(|connector| &connector.tags)
        }
    }

    /// The connector or challenge `resource_arn` names, to WRITE its tags, or `None` when this
    /// service holds no resource under that ARN.
    ///
    /// The mutable twin of [`tags_of`], reaching the same single map on the same record, so a
    /// tag written through here is the one a listing reads back and is dropped with the
    /// resource rather than lingering in a second table. It hands back the record's
    /// `updated_at` beside that map ([`TaggableMut`]), because a tag write changes the record
    /// and every write path here maintains the instant the record last changed.
    pub fn taggable_mut(&mut self, region: &str, resource_arn: &str) -> Option<TaggableMut<'_>> {
        match connector_arn_of_challenge(resource_arn) {
            Some(connector_arn) => self
                .connector_mut(region, connector_arn)?
                .challenges
                .get_mut(resource_arn)
                .map(|challenge| TaggableMut {
                    tags: &mut challenge.tags,
                    updated_at: &mut challenge.updated_at,
                }),
            None => self
                .connector_mut(region, resource_arn)
                .map(|connector| TaggableMut {
                    tags: &mut connector.tags,
                    updated_at: &mut connector.updated_at,
                }),
        }
    }

    /// Why a tagging WRITE on `resource_arn` cannot be served from the state the resource is
    /// in, as the sentence that says so — or `None` when the resource's state admits the
    /// change.
    ///
    /// The ONE place this service decides and words that precondition, for every operation
    /// that changes a resource's tags (`TagResource` over the entries it merges, and
    /// `UntagResource` over the keys it drops), so the sibling writes cannot answer one state
    /// with two sentences. A handler maps the sentence into its own error channel and adds
    /// nothing to it.
    ///
    /// A connector whose `DeleteConnector` has been accepted is a record this service only
    /// REPORTS until its teardown completes: a tag change written onto it — or onto a
    /// challenge hanging off it, which the teardown takes with it — would be dropped with the
    /// record, so it is refused for the status the record is in. The refusal is for a resource
    /// that EXISTS: an ARN this service holds no record under is the tagging layer's own
    /// not-found refusal instead ([`TaggingError::not_found`]), so the owning connector's
    /// status is consulted only once the resource's tag map is known to be there.
    pub fn tag_write_refusal(&self, region: &str, resource_arn: &str) -> Option<String> {
        self.tags_of(region, resource_arn)?;
        let connector = self
            .owning_connector(region, resource_arn)
            .filter(|connector| connector.is_deleting())?;
        Some(tags_cannot_be_changed_message(
            resource_arn,
            &connector.arn,
            &connector.status,
        ))
    }
}

/// One of this service's taggable records, borrowed to write its tags: the single tag map the
/// resource carries, together with the instant that record last changed.
///
/// A connector and a challenge each keep their own tag map and their own `updated_at`, so one
/// borrow carries both halves of a tag write — the map the write lands on, and the timestamp
/// the write moves. The timestamp is reachable only through [`Self::merge_tags`], so a tag
/// write cannot change the record and leave the instant it reports behind.
#[derive(Debug)]
pub struct TaggableMut<'a> {
    /// The resource's own tags: the very map [`ComAmazonawsPcaconnectorscepPcaconnectorscepState::tags_of`]
    /// reads back.
    pub tags: &'a mut BTreeMap<String, String>,
    /// The instant the record was last changed, as the resource reports it.
    updated_at: &'a mut SystemTime,
}

impl TaggableMut<'_> {
    /// Merge `tags` into the record's own map and, when that leaves the record different from
    /// what it was, record `now` as the instant it changed. Answers whether anything changed.
    ///
    /// A tag is a key with ONE value, so a key the resource already carries takes the new
    /// value instead of being duplicated and every other tag it carries stays. A merge that
    /// writes only values the record already had leaves the record exactly as it was, so there
    /// is no change for the timestamp to report.
    pub fn merge_tags(
        &mut self,
        tags: impl IntoIterator<Item = (String, String)>,
        now: SystemTime,
    ) -> bool {
        let mut changed = false;
        for (key, value) in tags {
            match self.tags.get_mut(&key) {
                Some(existing) if *existing == value => {}
                Some(existing) => {
                    *existing = value;
                    changed = true;
                }
                None => {
                    self.tags.insert(key, value);
                    changed = true;
                }
            }
        }
        if changed {
            *self.updated_at = now;
        }
        changed
    }

    /// Drop the tags `keys` names from the record's own map and, when that leaves the record
    /// different from what it was, record `now` as the instant it changed. Answers whether
    /// anything changed.
    ///
    /// A tag is identified by its key alone, so a key the resource carries takes its value with
    /// it and every other tag it carries stays. A key the resource does not carry leaves the
    /// record exactly as it was — removing a tag that is not there is the same end state as
    /// removing it twice — so there is no change for the timestamp to report.
    pub fn remove_tags<K: AsRef<str>>(
        &mut self,
        keys: impl IntoIterator<Item = K>,
        now: SystemTime,
    ) -> bool {
        let mut changed = false;
        for key in keys {
            if self.tags.remove(key.as_ref()).is_some() {
                changed = true;
            }
        }
        if changed {
            *self.updated_at = now;
        }
        changed
    }
}

/// The key a `ClientToken` seen in `region` is remembered under, so one token stands for one
/// call against one Region's endpoint.
pub fn token_key(region: &str, client_token: &str) -> (String, String) {
    (region.to_string(), client_token.to_string())
}

/// One create's `ClientToken` table: (Region, `ClientToken`) -> the record the call carrying
/// that token created ([`ComAmazonawsPcaconnectorscepPcaconnectorscepState::connector_tokens`],
/// [`ComAmazonawsPcaconnectorscepPcaconnectorscepState::challenge_tokens`]).
pub type IdempotencyTable = BTreeMap<(String, String), IdempotencyRecord>;

/// What a create must do with the `ClientToken` its request carried, as [`token_replay`]
/// decides it.
#[derive(Debug)]
pub enum TokenReplay<T> {
    /// The token is bound to a resource that can still be answered with, and this call is the
    /// one the token stands for: answer with that resource and mint nothing.
    Answer(T),
    /// The token is bound to a resource that can still be answered with, but this call is NOT
    /// the one it stands for, so answering with that resource would answer for a request
    /// nobody made. The ARN is the resource the token already created, which is what the
    /// refusal reports.
    Conflict { arn: String },
    /// The token binds nothing that can be answered with — it is unused, expired, or the
    /// resource it created can no longer be handed out — so the call is served exactly as a
    /// first one, and files this token itself ([`remember_token`]).
    Fresh,
}

/// What the `@idempotencyToken` rule makes of the `ClientToken` a create carried: look the
/// (Region, `ClientToken`) record up in that create's own table, ask `answerable` whether the
/// resource it created can still be answered with, refuse a token reused for a DIFFERENT
/// request, and otherwise answer with that resource.
///
/// The ONE place this service applies that rule, for every operation that mints a resource
/// under an `@idempotencyToken` — `CreateConnector` over the connectors it mints and
/// `CreateChallenge` over the challenge passwords it mints — so the sibling creates cannot
/// decide one reused token two ways. Each caller supplies only what genuinely differs: its own
/// table, its own fingerprint of the resource-defining inputs
/// ([`connector_request`], [`challenge_request`]), the liveness predicate that says whether
/// its resource can still be handed out, and the error payload it words a [`TokenReplay::Conflict`]
/// with.
///
/// `answerable` is handed the ARN the token is bound to and answers with what the caller would
/// return for it, or `None` when that resource can no longer be answered with — a retry is not
/// exempt from the preconditions of the call it stands for, so a token whose resource is gone,
/// or is in a state that could not serve a fresh create, binds nothing and the call falls
/// through to [`TokenReplay::Fresh`]. It is the caller's own read, so it reports exactly what a
/// first call would.
///
/// A record past [`CLIENT_TOKEN_TTL`] is not here to be found: it is dropped by
/// [`ComAmazonawsPcaconnectorscepPcaconnectorscepState::settle`] at the top of the handler, so
/// an expired token is a token nobody has used.
pub fn token_replay<T>(
    tokens: &IdempotencyTable,
    region: &str,
    client_token: Option<&str>,
    request: &str,
    answerable: impl FnOnce(&str) -> Option<T>,
) -> TokenReplay<T> {
    let Some(record) = client_token.and_then(|token| tokens.get(&token_key(region, token))) else {
        return TokenReplay::Fresh;
    };
    let Some(answer) = answerable(&record.arn) else {
        return TokenReplay::Fresh;
    };
    if record.request != request {
        return TokenReplay::Conflict {
            arn: record.arn.clone(),
        };
    }
    TokenReplay::Answer(answer)
}

/// File the `ClientToken` a create carried against the resource it just created, so the next
/// call carrying that token is answered by [`token_replay`] rather than minting a second
/// resource.
///
/// The write half of the same rule, in the same one place: a call that carried no token files
/// nothing — there is nothing to recognise a retry by — and one that did is remembered under
/// [`token_key`], with the fingerprint of the inputs it was made with, which is what tells a
/// retry from a token reused for another request.
pub fn remember_token(
    tokens: &mut IdempotencyTable,
    region: &str,
    client_token: Option<String>,
    arn: &str,
    now: SystemTime,
    request: String,
) {
    let Some(token) = client_token else {
        return;
    };
    tokens.insert(
        token_key(region, &token),
        IdempotencyRecord {
            arn: arn.to_string(),
            created_at: now,
            request,
        },
    );
}

/// The Region segment of an ARN: field four of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`, which is where both
/// `ConnectorArn`'s and `ChallengeArn`'s own patterns put it. `None` for a string that
/// carries no non-empty Region segment there.
pub fn arn_region(arn: &str) -> Option<&str> {
    arn.split(':').nth(3).filter(|region| !region.is_empty())
}

/// The partition segment of an ARN: field two of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`. `None` for a string that
/// carries no non-empty partition segment there.
pub fn arn_partition(arn: &str) -> Option<&str> {
    arn.split(':').nth(1).filter(|partition| !partition.is_empty())
}

/// The account segment of an ARN: field five of
/// `arn:<partition>:<service>:<region>:<account>:<resource>`. `None` for a string that
/// carries no non-empty account segment there.
pub fn arn_account(arn: &str) -> Option<&str> {
    arn.split(':').nth(4).filter(|account| !account.is_empty())
}

/// Why the private CA an ARN names cannot be linked by a connector created in `region`, as
/// the `ConnectorStatusReason` that says so — or `None` when the ARN names a CA this
/// connector could reach.
///
/// This is the FIRST reading `CreateConnector` makes of a `CertificateAuthorityArn`, and the
/// one that decides whether Private CA is asked about it at all: a CA this endpoint and
/// principal do not resolve is one no read of this endpoint reaches, so what a reachable
/// endpoint would answer about that ARN is the answer to a different question.
///
/// A connector talks to its private CA as the account it is created in, from the Region it
/// is created in: a private CA is a regional resource of one account, and neither an
/// endpoint of another Region nor a principal of another account resolves it. So the three
/// ARN segments that place a resource — partition, Region and account — must be the ones
/// this connector's OWN ARN carries ([`connector_arn_prefix`], which is where this service's
/// partition and account come from, so there is one source for both):
///
/// * a CA in another partition or another Region is not a CA this Region's endpoint can
///   resolve, so it is the one that cannot be found;
/// * a CA in another account exists, but nothing grants this connector the use of it, so
///   that is the reason about permission rather than about existence.
///
/// `VpcEndpointId` gets no equivalent check HERE: it carries neither a Region nor an account
/// segment, so nothing in the request itself says whether the endpoint it names is reachable
/// — that question is Amazon EC2's to answer, and it is asked there
/// ([`crate::service::vpc_endpoint`]).
pub fn private_ca_failure_reason(
    region: &str,
    certificate_authority_arn: &str,
) -> Option<&'static str> {
    let own = connector_arn_prefix(region);
    if arn_partition(certificate_authority_arn) != arn_partition(&own)
        || arn_region(certificate_authority_arn) != arn_region(&own)
    {
        return Some(REASON_PRIVATECA_RESOURCE_NOT_FOUND);
    }
    if arn_account(certificate_authority_arn) != arn_account(&own) {
        return Some(REASON_PRIVATECA_ACCESS_DENIED);
    }
    None
}

/// Which of this service's two resource kinds an ARN names, as the `ResourceType` member of
/// its errors spells it ("either `Connector` or `Challenge`").
///
/// Decided by the ARN's own shape: a challenge ARN is its connector's ARN plus the challenge
/// segment ([`connector_arn_of_challenge`]), so anything without that segment names a
/// connector.
pub fn resource_type_of(resource_arn: &str) -> &'static str {
    if connector_arn_of_challenge(resource_arn).is_some() {
        RESOURCE_TYPE_CHALLENGE
    } else {
        RESOURCE_TYPE_CONNECTOR
    }
}

/// The ONE `ResourceNotFoundException` this service answers with, for every operation that
/// models it.
///
/// All three members of the shape are required (smithy_query: shape
/// ResourceNotFoundException), and the `Message` is not free text: it is the `ResourceType`
/// word, the fixed words `" was not found: "`, the `ResourceId`, and a terminating `.`. The
/// two variable parts are the error's other two members, so a caller reads the same two
/// facts out of the sentence and out of the fields.
///
/// `resource_type` is what this call DISCRIMINATES the failed resolution on, and the word its
/// sentence opens with. Which word that is follows from what the call does:
///
/// * a call addressed AT a resource — every read and delete of this service — could not find
///   the resource it was asked about, so it reports that resource's KIND, in the two words the
///   member's own doc states its domain as ("either `Connector` or `Challenge`"), as
///   [`resource_type_of`] reads them off the ARN;
/// * a CREATE could not resolve a resource its input only REFERENCES — the resource the call
///   is about does not exist yet — so it reports the INPUT MEMBER that carried that reference
///   ([`reference_not_found`], which is the one place this service words that case).
pub fn resource_not_found(resource_type: &str, resource_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: format!("{resource_type} was not found: {resource_id}."),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// The `ResourceNotFoundException` a CREATE of this service answers a reference it cannot
/// resolve with: `input_member` is the NAME of the input member that carried the unresolvable
/// ARN, and it is both what the error discriminates on (`ResourceType`) and the word its
/// `Message` opens with, over the ARN the request supplied (`ResourceId`).
///
/// The one place this service words that case, for every operation that creates a resource
/// from a reference to another — `CreateConnector`'s `CertificateAuthorityArn` and
/// `CreateChallenge`'s `ConnectorArn` — so two creates cannot report one failed resolution two
/// ways. What a create could not resolve is the parameter it was handed rather than the
/// resource the call is addressed at: the resource it would create does not exist yet, so the
/// caller's next move is to fix that input, and that is what the error names.
///
/// A read or a delete of the SAME reference is addressed at the resource itself and reports
/// its kind instead ([`resource_not_found`]).
pub fn reference_not_found(input_member: &str, resource_arn: &str) -> ResourceNotFoundException {
    resource_not_found(input_member, resource_arn)
}

/// The opening of every sentence this service refuses a PRECONDITION with: a request whose
/// input is well-formed but names resources whose state does not admit the call.
///
/// The clause that follows says which precondition failed and over which resource; the
/// builders below are the clauses this service states, so a caller reads one scaffold
/// whichever precondition refused the call.
const VALIDATION_ERROR_PREFIX: &str = "Validation error detected: ";

/// One precondition refusal's whole sentence: the scaffold above, the `clause` that says which
/// precondition failed and over which resource, and the terminating `.` that closes it.
///
/// Every builder below states only its own clause and reaches the scaffold through here, so a
/// clause cannot be worded into a sentence that opens or closes differently from its siblings'
/// — whichever precondition, and whichever operation, refused the call.
fn precondition_message(clause: impl ::std::fmt::Display) -> String {
    format!("{VALIDATION_ERROR_PREFIX}{clause}.")
}

/// The sentence a call is refused with when a resource it names, or references, is not in the
/// state that lets it be used: the prefix, the fixed clause `"Resource(s) are not active: "`,
/// the ARN of that resource, and a terminating `.`.
///
/// The one place this service words that refusal, for every operation that has such a
/// precondition — `CreateConnector` over the `CertificateAuthorityArn` it links, and
/// `CreateChallenge` over the connector it mints a password under — so the two cannot state
/// one precondition in two sentences.
pub fn resources_not_active_message(resource_arn: &str) -> String {
    precondition_message(format_args!("Resource(s) are not active: {resource_arn}"))
}

/// The sentence a call is refused with when the connector it names is of a `ConnectorType`
/// that call is not for: the precondition scaffold ([`precondition_message`]) around a clause
/// naming the connector, the type its record carries, and the one type whose challenge
/// passwords this service mints.
///
/// The variable parts are the ARN the request named and the `ConnectorType` the record is of,
/// read from state rather than from the request's shape; the required type is this service's
/// own constant, so the sentence and the precondition it states cannot name two different
/// types.
///
/// Stated here with its sibling clauses rather than in the handler that applies it, so a
/// precondition refusal of this service opens and closes the same way whichever precondition,
/// and whichever operation, refused the call.
pub fn challenge_connector_type_message(connector_arn: &str, connector_type: &str) -> String {
    precondition_message(format_args!(
        "Connector {connector_arn} has type {connector_type}; challenge passwords can only be \
         created for {TYPE_GENERAL_PURPOSE} connectors"
    ))
}

/// The sentence a tag change is refused with when the connector the resource hangs off has
/// been accepted for teardown: the precondition scaffold ([`precondition_message`]) around a
/// clause naming the resource, the connector whose status governs it, and the status that
/// record is in.
///
/// Two forms, one per resource kind, because the connector IS the resource for a connector ARN
/// and is the owner for a challenge ARN ([`resource_type_of`]) — so the sentence names the
/// challenge and its connector separately only where they are two resources.
///
/// Reached through [`ComAmazonawsPcaconnectorscepPcaconnectorscepState::tag_write_refusal`],
/// which is what decides the refusal applies, so the decision and the wording stay together.
fn tags_cannot_be_changed_message(
    resource_arn: &str,
    connector_arn: &str,
    status: &str,
) -> String {
    if resource_arn == connector_arn {
        precondition_message(format_args!(
            "Connector {connector_arn} has status {status}; its tags cannot be changed"
        ))
    } else {
        precondition_message(format_args!(
            "{} {resource_arn} belongs to connector {connector_arn}, which has status {status}; \
             its tags cannot be changed",
            resource_type_of(resource_arn)
        ))
    }
}

/// The sentence a call is refused with when the resource it names is in a state the call
/// cannot be served from at all: the prefix, the fixed clause
/// `"Resource cannot be in state "`, that state, the ARN, and a terminating `.`.
///
/// The variable parts are the `ConnectorStatus` the record is in and the ARN the request
/// named, both read from state rather than from the request's shape.
pub fn resource_cannot_be_in_state_message(status: &str, resource_arn: &str) -> String {
    precondition_message(format_args!(
        "Resource cannot be in state {status}: {resource_arn}"
    ))
}

/// The Region segment every ARN of this service carries, as both `ConnectorArn`'s and
/// `ChallengeArn`'s own `@pattern` write it (smithy_query: shape ConnectorArn, ChallengeArn):
/// two or more lower-case words joined by `-`, then a number with no leading zero.
///
/// The service's ONE spelling of that rule. It is a macro rather than a `const` so the same
/// expression can be `concat!`ed into the whole-ARN pattern below AND anchored on its own for
/// a single Region name ([`WHOLE_REGION_PATTERN`]) — `concat!` takes literals, so a `const`
/// would have to be re-spelled at one of the two places and the two could then drift.
macro_rules! region_segment_pattern {
    () => {
        r"[a-z]+(-[a-z]+)+-[1-9]\d*"
    };
}

/// The form a `ResourceArn` naming one of this service's resources takes: `ConnectorArn`'s
/// own `@pattern` with `ChallengeArn`'s challenge segment appended OPTIONALLY.
///
/// One expression covers both kinds because the challenge pattern IS the connector pattern
/// plus that segment (smithy_query: shape ConnectorArn, ChallengeArn), so the two cannot
/// drift apart here. Written exactly as the model writes them, because it is also the
/// requirement a refusal quotes back.
pub const RESOURCE_ARN_PATTERN: &str = concat!(
    r"^arn:aws(-[a-z]+)*:pca-connector-scep:",
    region_segment_pattern!(),
    r":\d{12}",
    r":connector\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}",
    r"(\/challenge\/[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12})?$"
);

/// Shortest a resource ARN of this service may be, from the `@length(5, 200)` that both
/// `ConnectorArn` and `ChallengeArn` declare.
pub const MIN_RESOURCE_ARN_LENGTH: i64 = 5;
/// Longest a resource ARN of this service may be, from the same `@length`.
pub const MAX_RESOURCE_ARN_LENGTH: i64 = 200;

/// `ResourceArn`'s constraints: the `@length` above and [`RESOURCE_ARN_PATTERN`], as the
/// trait values the framework's own constraint walk reads.
static RESOURCE_ARN_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(LengthTrait::new(
        Some(MIN_RESOURCE_ARN_LENGTH),
        Some(MAX_RESOURCE_ARN_LENGTH),
    )));
    traits.insert(Box::new(PatternTrait::new(RESOURCE_ARN_PATTERN)));
    traits
});

/// The `resourceArn` member schema that walk is driven over: its name gives the violation
/// path, in the lowerCamel spelling every clause uses, and its trait map carries the
/// constraints.
static M_RESOURCE_ARN: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.pcaconnectorscep#TaggedResource$resourceArn",
        "com.amazonaws.pcaconnectorscep",
        "resourceArn",
    ),
    ShapeType::String,
    "resourceArn",
    0,
)
.with_traits(&RESOURCE_ARN_TRAITS);

/// The root of the shadow input the walk runs over. Its member list is empty on purpose: the
/// walk visits what [`ConstrainedString`] WRITES, and a root struct schema carries no member
/// name, so it contributes no path segment.
static SHADOW_INPUT_SCHEMA: Schema = Schema::new_struct(
    ShapeId::from_static(
        "com.amazonaws.pcaconnectorscep#ConstrainedString",
        "com.amazonaws.pcaconnectorscep",
        "ConstrainedString",
    ),
    ShapeType::Structure,
    &[],
);

/// One string value, written for the validating walk the way the codec writes a string
/// member: the member schema says which constraints the walk then reads.
struct ConstrainedString<'a> {
    member: &'static Schema,
    value: &'a str,
}

impl SerializableStruct for ConstrainedString<'_> {
    fn serialize_members(&self, s: &mut dyn ShapeSerializer) -> Result<(), SerdeError> {
        s.write_string(self.member, self.value)
    }
}

/// Every constraint `member` declares that `value` breaks, as the framework's own constraint
/// walk reports them: one entry per constraint broken, length before pattern.
///
/// The ONE place this crate evaluates a string constraint, so a value checked here is checked
/// — and, where the answer is a message, worded — exactly as the request boundary checks the
/// same string on a constrained member.
fn string_violations(member: &'static Schema, value: &str) -> Vec<Violation> {
    validate(&ConstrainedString { member, value }, &SHADOW_INPUT_SCHEMA)
}

/// How many fields an ARN is built from: `arn:<partition>:<service>:<region>:<account>:<resource>`.
const ARN_FIELD_COUNT: usize = 6;

/// Whether `value` is syntactically an ARN, which is the ONE thing the resource-tagging layer
/// parses a `ResourceArn` for before it resolves anything.
///
/// An ARN is six colon-separated fields — `arn:<partition>:<service>:<region>:<account-id>:<resource>`
/// — of which the literal `arn`, the partition, the service and the resource are always
/// filled; a region and an account id are empty in the ARNs of global and account-less
/// resources, so neither is required here. The resource field keeps any further colons,
/// because a resource id may carry them.
///
/// Nothing here is about THIS service: a value that parses names some resource somewhere, so a
/// call naming it is a lookup that may find nothing, and a value that does not parse is a
/// request the layer refuses before any service sees it.
fn parses_as_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.splitn(ARN_FIELD_COUNT, ':').collect();
    let [prefix, partition, service, _region, _account, resource] = fields.as_slice() else {
        return false;
    };
    *prefix == "arn" && !partition.is_empty() && !service.is_empty() && !resource.is_empty()
}

/// Why a tagging operation will not look a resource up under `resource_arn`.
///
/// The two refusals are different layers' and must not be collapsed: see
/// [`resource_arn_rejection`].
#[derive(Debug)]
pub enum ResourceArnRejection {
    /// The value is not an ARN at all, so the resource-tagging layer's own ARN parse failed:
    /// its [`TaggingError::malformed_resource_arn`] refusal, worded by that layer and naming
    /// nothing of the request.
    Malformed,
    /// A well-formed ARN of THIS service that is not one of the resource ARNs it publishes, so
    /// the service's own check of the value failed, with the sentence it states.
    NotAResourceOfThisService(String),
}

impl ResourceArnRejection {
    /// This rejection as the operation's error, given how that operation words a refusal its
    /// own model covers (`validation`). Keeps the two layers' refusals apart in the one place
    /// all three tagging operations call.
    pub fn into_error<E>(
        self,
        validation: impl FnOnce(String) -> TaggingError<E>,
    ) -> TaggingError<E> {
        match self {
            Self::Malformed => TaggingError::malformed_resource_arn(),
            Self::NotAResourceOfThisService(why) => validation(why),
        }
    }
}

/// Why `resource_arn` is not a value a tagging operation will look a resource up under, or
/// `None` when it is.
///
/// The tagging operations take `ResourceArn` as a bare `String` with no `@pattern` and no
/// `@length` (smithy_query: operation ListTagsForResource / TagResource / UntagResource), so
/// nothing is refused for them at the request boundary and this is the only place the check
/// can live — while the same ARNs reached as `ConnectorArn` or `ChallengeArn` labels are
/// refused there, against the very patterns [`RESOURCE_ARN_PATTERN`] is built from.
///
/// Three things are therefore separated, because two different layers answer them.
///
/// A value that is not an ARN at all never reaches this service: the resource-tagging layer
/// that fronts `/tags/{ResourceArn}` parses it first and refuses it with its OWN error class
/// and one fixed sentence ([`ResourceArnRejection::Malformed`]), reporting neither the value,
/// the member nor any expression — which is the RECORDED answer to such a request.
///
/// An ARN of THIS service that is not one of its published resource ARNs did parse, so it is
/// this service's own check of the value that fails. The check and the wording there are the
/// FRAMEWORK's: the constraint walk ([`aws_smithy_schema_server::constraints::validate`]) runs
/// over the member schema above and renders one clause per constraint broken, length before
/// pattern, in its `N validation error(s) detected` envelope with its own elision of the
/// echoed value. That envelope is the conventional AWS one, and it is NOT the sentence this
/// service states for a patterned label whose value fell outside its `@pattern`: that one is
/// reworded from the refusal the model's own constraints produced ([`crate::service::refusal`])
/// on the strength of the recorded answer to such a request. Nothing recorded shows how this
/// member's own check is worded, so it keeps what the rest of the tree states for the class.
///
/// An ARN of another service parsed and names a resource elsewhere, so it is left to the
/// lookup to miss and be reported by the tagging layer's not-found refusal.
pub fn resource_arn_rejection(resource_arn: &str) -> Option<ResourceArnRejection> {
    if !parses_as_arn(resource_arn) {
        return Some(ResourceArnRejection::Malformed);
    }
    if resource_arn.split(':').nth(2) != Some(ARN_SERVICE) {
        return None;
    }
    envelope(&string_violations(&M_RESOURCE_ARN, resource_arn))
        .map(ResourceArnRejection::NotAResourceOfThisService)
}

/// Whether `character` is one a tag key or a tag value may carry.
///
/// AWS's tag alphabet is letters, numbers, spaces and the characters `_ . : / = + - @`, and
/// it is not restricted to ASCII: "letters" and "numbers" of any script are allowed, which
/// is what [`char::is_alphanumeric`] answers. "Spaces" are the Unicode separators — a tab or
/// a newline is a control character rather than a space, and [`char::is_whitespace`] answers
/// for those too, so they are excluded here.
fn is_tag_character(character: char) -> bool {
    character.is_alphanumeric()
        || (character.is_whitespace() && !character.is_control())
        || matches!(character, '_' | '.' | ':' | '/' | '=' | '+' | '-' | '@')
}

/// Why this one tag KEY is not a key a caller may name, as the sentence to refuse the request
/// with and naming `member` as the member it came in on, or `None` when it is.
///
/// The service's ONE statement of what a tag key is — its length, its alphabet and the
/// namespace Amazon Web Services keeps for itself — so every operation that takes tag keys,
/// as the halves of `Tags` or as `TagKeys` on its own, holds a caller to the same rule. The
/// length is checked before the characters, so the text a message quotes back is always
/// within the bound the quote came from.
pub fn tag_key_rejection(member: &str, key: &str) -> Option<String> {
    let key_length = key.chars().count();
    if key_length == 0 {
        return Some(format!(
            "{member} carries an empty tag key; a tag key is 1 to {MAX_TAG_KEY_LENGTH} characters"
        ));
    }
    if key_length > MAX_TAG_KEY_LENGTH {
        return Some(format!(
            "{member} carries a tag key of {key_length} characters; a tag key is 1 to \
             {MAX_TAG_KEY_LENGTH} characters"
        ));
    }
    if let Some(character) = key.chars().find(|character| !is_tag_character(*character)) {
        return Some(format!(
            "the key of tag `{key}` carries {character:?}; a tag key may use only letters, \
             numbers, spaces and the characters _ . : / = + - @"
        ));
    }
    if key
        .get(..RESERVED_TAG_KEY_PREFIX.len())
        .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
    {
        return Some(format!(
            "tag key `{key}` uses the `{RESERVED_TAG_KEY_PREFIX}` prefix, which is reserved \
             for the tags Amazon Web Services sets itself"
        ));
    }
    None
}

/// Why this one tag may not be stored, as the sentence to refuse the request with, or `None`
/// when the pair is one a caller may set.
///
/// The key is checked before the value ([`tag_key_rejection`]), and each half's length before
/// its characters, so the text a message quotes back is always within the bound the quote came
/// from.
pub fn tag_entry_rejection(key: &str, value: &str) -> Option<String> {
    if let Some(why) = tag_key_rejection("Tags", key) {
        return Some(why);
    }
    let value_length = value.chars().count();
    if value_length > MAX_TAG_VALUE_LENGTH {
        return Some(format!(
            "the value of tag `{key}` is {value_length} characters; a tag value is at most \
             {MAX_TAG_VALUE_LENGTH} characters"
        ));
    }
    if let Some(character) = value.chars().find(|character| !is_tag_character(*character)) {
        return Some(format!(
            "the value of tag `{key}` carries {character:?}; a tag value may use only letters, \
             numbers, spaces and the characters _ . : / = + - @"
        ));
    }
    None
}

/// Why the `Tags` of a request may not be stored, or `None` when every entry is one a caller
/// may set.
///
/// `Tags` carries no modelled `@length` or `@pattern` on the map or on either half of an
/// entry, so nothing is refused at the request boundary and every tagging rule this service
/// holds a caller to lives here: the per-resource count ([`MAX_TAGS_PER_RESOURCE`]) and then
/// each entry ([`tag_entry_rejection`]).
///
/// The count is answered first, so an oversized map is refused for being oversized rather
/// than for whichever of its entries happened to be looked at; the entries are then walked
/// in key order, so the same map is always refused with the same sentence however the
/// request's own map iterates.
pub fn tags_rejection<'a>(tags: impl IntoIterator<Item = (&'a str, &'a str)>) -> Option<String> {
    let entries: Vec<(&str, &str)> = tags.into_iter().collect();
    if entries.len() > MAX_TAGS_PER_RESOURCE {
        return Some(format!(
            "Tags carries {} entries; a resource may have at most {MAX_TAGS_PER_RESOURCE}",
            entries.len()
        ));
    }
    tag_entries_rejection(entries)
}

/// Why the ENTRIES of a request's `Tags` may not be stored, without the count ceiling, or
/// `None` when every entry is one a caller may set.
///
/// The half of [`tags_rejection`] that is about each pair ([`tag_entry_rejection`]) rather
/// than about how many there are, for the caller that answers the count itself: the tagging
/// endpoint's ceiling is on the tag set the RESOURCE ends up carrying, which the request's own
/// entry count does not settle, and it is refused with the tagging layer's own class rather
/// than with this service's input error.
///
/// The entries are walked in key order, so the same map is always refused with the same
/// sentence however the request's own map iterates.
pub fn tag_entries_rejection<'a>(
    tags: impl IntoIterator<Item = (&'a str, &'a str)>,
) -> Option<String> {
    let mut entries: Vec<(&str, &str)> = tags.into_iter().collect();
    entries.sort_unstable();
    entries
        .into_iter()
        .find_map(|(key, value)| tag_entry_rejection(key, value))
}

/// Why the `TagKeys` of a request names something that is not a tag key, or `None` when every
/// entry is a key a caller may name.
///
/// `TagKeys` is a `list<String>` with no modelled `@length` and no `@pattern`, on the list or
/// on its member (smithy_query: operation UntagResource), so nothing is refused at the request
/// boundary and the rule is the same one every other tagging operation holds a caller to
/// ([`tag_key_rejection`]): a string outside it is not a tag key at all, and the reserved
/// namespace holds the tags Amazon Web Services sets itself, which a caller may neither write
/// nor remove.
///
/// No count ceiling applies: [`MAX_TAGS_PER_RESOURCE`] bounds what a resource CARRIES, and
/// naming a key the resource does not have removes nothing. The keys are walked in key order,
/// so the same list is always refused with the same sentence however the request happened to
/// order it.
pub fn tag_keys_rejection<'a>(keys: impl IntoIterator<Item = &'a str>) -> Option<String> {
    let mut keys: Vec<&str> = keys.into_iter().collect();
    keys.sort_unstable();
    keys.into_iter()
        .find_map(|key| tag_key_rejection("TagKeys", key))
}

/// The region this request was signed for, read from its SigV4 credential scope.
///
/// The record store is partitioned by this value, so only a Region-shaped one is taken from
/// the request ([`is_region_shaped`]): a scope carrying anything else is served in
/// [`DEFAULT_REGION`], which keeps the Region a resource is filed under one that its ARN can
/// actually name.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| is_region_shaped(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// [`region_segment_pattern`] anchored to a WHOLE value, so one Region name on its own is
/// read by the very expression an ARN embeds it as.
const WHOLE_REGION_PATTERN: &str = concat!("^", region_segment_pattern!(), "$");

/// A Region name's only constraint: [`WHOLE_REGION_PATTERN`]. How long the name may be is a
/// separate bound ([`MAX_REGION_LENGTH`]) — the emulator's own rather than one either ARN
/// pattern states — so it is not a trait here.
static REGION_TRAITS: LazyLock<TraitMap> = LazyLock::new(|| {
    let mut traits = TraitMap::new();
    traits.insert(Box::new(PatternTrait::new(WHOLE_REGION_PATTERN)));
    traits
});

/// The `region` member schema the constraint walk is driven over for one Region name.
static M_REGION: Schema = Schema::new_member(
    ShapeId::from_static(
        "com.amazonaws.pcaconnectorscep#ConstrainedString$region",
        "com.amazonaws.pcaconnectorscep",
        "region",
    ),
    ShapeType::String,
    "region",
    0,
)
.with_traits(&REGION_TRAITS);

/// Whether `value` has the shape every ARN of this service requires of its Region segment,
/// within [`MAX_REGION_LENGTH`].
///
/// The shape is not restated here: the check runs [`region_segment_pattern`] — the same
/// expression [`RESOURCE_ARN_PATTERN`] embeds — through the same constraint walk
/// ([`string_violations`]) that refuses a `ResourceArn`. So the Region a record is filed under
/// is, by construction, one the ARN built from it can name and that ARN can be read back by.
/// The length bound is the separate bound it is, and the only thing checked beside the
/// pattern.
pub fn is_region_shaped(value: &str) -> bool {
    value.len() <= MAX_REGION_LENGTH && string_violations(&M_REGION, value).is_empty()
}

/// A fresh identifier in the UUID form every id in this service's ARNs and its challenge
/// `Password` carry (`[0-9a-f]{8}(-[0-9a-f]{4}){3}-[0-9a-f]{12}`).
///
/// Unpredictable AND unique. Two independent contributions go into the digest: the
/// per-process root seed ([`id_seed`]) and a value hashed through a `RandomState` built for
/// THIS call, which `std` seeds from the operating system — it is what makes `HashMap`
/// resistant to collision attacks — so a caller holding one challenge password cannot derive
/// the next one. The counter and the clock also go in, so two ids minted in the same
/// nanosecond, in the same or different threads, still differ.
///
/// Those digest bytes are then handed to the `uuid` crate, which owns both of the remaining
/// rules: it writes the version and variant nibbles a random-bytes UUID carries, and renders
/// the canonical hyphenated lowercase form. So the result is a well-formed UUID and not
/// merely 32 hex characters, and neither rule is restated here to drift from the library's.
pub fn new_uuid() -> String {
    static COUNTER: AtomicU64 = AtomicU64::new(0);

    let counter = COUNTER.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|since| since.as_nanos())
        .unwrap_or_default();

    let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write(&counter.to_le_bytes());
    hasher.write(&nanos.to_le_bytes());
    let per_call = hasher.finish();

    let digest = <::sha2::Sha256 as ::sha2::Digest>::digest(format!(
        "{}:{per_call}:{counter}:{nanos}",
        id_seed()
    ));

    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    ::uuid::Builder::from_random_bytes(bytes)
        .into_uuid()
        .hyphenated()
        .to_string()
}

/// The per-process root seed [`new_uuid`] mixes into every identifier.
///
/// Taken once through an operating-system-seeded `RandomState`, so every identifier in the
/// process shares one unpredictable root, with no RNG crate and no entropy of this crate's
/// own invention.
fn id_seed() -> u64 {
    static SEED: OnceLock<u64> = OnceLock::new();

    *SEED.get_or_init(|| {
        let mut hasher = ::std::collections::hash_map::RandomState::new().build_hasher();
        hasher.write(ARN_SERVICE.as_bytes());
        hasher.finish()
    })
}

/// The ARN of a connector: `arn:aws:pca-connector-scep:<region>:<account>:connector/<uuid>`,
/// which is what `ConnectorArn`'s own pattern requires.
pub fn connector_arn(region: &str, connector_id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("connector/{connector_id}"))
}

/// The prefix every connector ARN of `region` begins with — the same ARN
/// [`connector_arn`] builds, up to and including the resource segment, with no id. Because
/// the store is keyed by ARN, this is the lower bound of that Region's contiguous range of
/// records.
pub fn connector_arn_prefix(region: &str) -> String {
    connector_arn(region, "")
}

/// The ARN of a challenge: the owning connector's ARN plus `/challenge/<uuid>`, which is
/// what `ChallengeArn`'s own pattern requires.
pub fn challenge_arn(connector_arn: &str, challenge_id: &str) -> String {
    format!("{connector_arn}{CHALLENGE_SEGMENT}{challenge_id}")
}

/// The connector ARN embedded in a challenge ARN, or `None` when the ARN carries no
/// challenge segment.
pub fn connector_arn_of_challenge(challenge_arn: &str) -> Option<&str> {
    challenge_arn
        .split_once(CHALLENGE_SEGMENT)
        .map(|(connector, _)| connector)
}

/// The audience an Intune connector publishes.
///
/// It is the single audience Microsoft Entra workload identity federation accepts in a
/// token presented for token exchange, so it is a property of the federation this
/// integration configures rather than of any one connector: every Intune connector
/// publishes it, and nothing a caller sends can change it.
pub const OIDC_AUDIENCE: &str = "api://AzureADTokenExchange";

/// Host label of the OIDC issuer this service publishes, in front of the service's own
/// regional domain — the identity plane an Entra federated credential trusts, which is a
/// different host from the connector's SCEP data plane ([`connector_endpoint`]).
const OIDC_HOST_LABEL: &str = "oidc";

/// The issuer URL an Intune connector publishes.
///
/// An OIDC issuer URL, so it is an HTTPS origin under this service's own regional domain
/// whose discovery document and keys a relying party fetches. It is reached by the Entra
/// tenant over the public internet, so it never carries the connector's VPC endpoint: the
/// access path chosen for the SCEP data plane says nothing about where the issuer serves.
/// The connector is named by the issuer's path, so two connectors publish two issuers,
/// each with its own key namespace, and the URL is stable for the life of the connector.
pub fn oidc_issuer(region: &str, connector_id: &str) -> String {
    format!(
        "https://{OIDC_HOST_LABEL}.{ARN_SERVICE}.{region}.{PUBLIC_DNS_SUFFIX}/{connector_id}"
    )
}

/// The subject an Intune connector publishes: the identifier this service mints for the
/// connector INSIDE its issuer's namespace, so it names a resource of the issuer rather
/// than repeating an ARN, and is distinct for two connectors.
pub fn oidc_subject(connector_id: &str) -> String {
    format!("connector/{connector_id}")
}

/// The OIDC parameters an Intune connector publishes for the caller to copy into their
/// Microsoft Entra app registration's federated credential.
///
/// All three are minted by the service: the issuer and the subject from the connector's own
/// identity and Region ([`oidc_issuer`], [`oidc_subject`]), the audience from the
/// federation protocol ([`OIDC_AUDIENCE`]). None of them is taken from the request — an
/// `IntuneConfiguration` carries the caller's Entra tenant id and primary domain, which
/// are what the caller already has and are neither an OIDC audience nor an issuer.
pub fn open_id_configuration(region: &str, connector_id: &str) -> OpenIdConfiguration {
    OpenIdConfiguration {
        issuer: Some(oidc_issuer(region, connector_id)),
        subject: Some(oidc_subject(connector_id)),
        audience: Some(OIDC_AUDIENCE.to_string()),
    }
}

/// The resource-defining inputs of a `CreateConnector` call, rendered so two calls can be
/// compared.
///
/// `Tags` is deliberately NOT part of it: tags are mutable after the create (through
/// `TagResource`), so they do not define which connector a request asked for.
pub fn connector_request(
    certificate_authority_arn: &str,
    mobile_device_management: Option<&MobileDeviceManagement>,
    vpc_endpoint_id: Option<&str>,
) -> String {
    let mdm = match mobile_device_management {
        Some(MobileDeviceManagement::Intune(intune)) => format!(
            "Intune({},{})",
            intune.azure_application_id, intune.domain
        ),
        None => "none".to_string(),
    };
    format!(
        "ca={certificate_authority_arn}\nmdm={mdm}\nvpce={}",
        vpc_endpoint_id.unwrap_or("none")
    )
}

/// The resource-defining input of a `CreateChallenge` call: the connector it asked for.
pub fn challenge_request(connector_arn: &str) -> String {
    format!("connector={connector_arn}")
}

/// One page of a `@paginated` listing: the items to return, plus the cursor that resumes
/// exactly where the page ended (`None` when the page is the last one).
#[derive(Debug, Clone)]
pub struct Page<T> {
    pub items: Vec<T>,
    pub next_token: Option<String>,
}

/// A `NextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Prefix the cursor plaintext carries, so a token this service never minted cannot decode
/// into a usable position.
const CURSOR_PREFIX: &str = "pca-connector-scep-after:";

/// Separates the cursor's scope from the key it names. A newline cannot occur in a scope or
/// in an ARN, so the split is unambiguous.
const CURSOR_SEPARATOR: char = '\n';

/// Padding appended when the hex cursor does not fill whole four-character groups, so the
/// token still matches `NextToken`'s pattern.
const CURSOR_PADDING: &str = "==";

/// Cursor-scope prefix of `ListConnectors`.
pub const CURSOR_SCOPE_CONNECTORS: &str = "connectors";

/// Cursor scope of `ListConnectors`, which lists the connectors of ONE Region: the scope
/// names that Region, so a token minted against another Region's endpoint — or by the
/// challenge listing — names no position here and is refused rather than resuming a walk it
/// was not taken from.
pub fn cursor_scope_connectors(region: &str) -> String {
    format!("{CURSOR_SCOPE_CONNECTORS}/{region}")
}

/// Cursor scope of `ListChallengeMetadata`, which lists ONE connector's challenges: the
/// scope names that connector, so a token minted for another connector — or for the
/// connector listing — names no position here and is refused rather than silently skipping
/// entries.
pub fn cursor_scope_challenges(connector_arn: &str) -> String {
    format!("challenges/{connector_arn}")
}

/// Encode the resume cursor that follows the record keyed `key` within `scope`.
///
/// Opaque to the caller and matching `NextToken`'s own pattern
/// (`^(?:[A-Za-z0-9_-]{4})*(?:[A-Za-z0-9_-]{2}==|[A-Za-z0-9_-]{3}=)?$`): hex characters are
/// all in that alphabet, and a hex string's length is always even, so it either fills whole
/// four-character groups or leaves two — which the pattern's `{2}==` tail accepts. The cursor
/// names a POSITION in the key ordering rather than an offset, so resuming never repeats or
/// skips a record when one is created or deleted between pages.
///
/// It is self-describing — scope included — so a listing validates a token by decoding it
/// and never has to record the tokens it issued, which matters because a listing is a
/// reporting operation: remembering an issued token would make a read mutate state.
pub fn encode_cursor(scope: &str, key: &str) -> String {
    let encoded = ::hex::encode(format!("{CURSOR_PREFIX}{scope}{CURSOR_SEPARATOR}{key}"));
    if encoded.len() % 4 == 0 {
        encoded
    } else {
        format!("{encoded}{CURSOR_PADDING}")
    }
}

/// Decode a cursor minted by [`encode_cursor`] for the SAME `scope`; `None` starts at the
/// beginning. A token of another scope is a [`MalformedCursor`], not a position.
pub fn decode_cursor(
    scope: &str,
    next_token: Option<&str>,
) -> Result<Option<String>, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(None);
    };
    // Hex never contains `=`, so stripping the padding cannot remove cursor content.
    let body = token.strip_suffix(CURSOR_PADDING).unwrap_or(token);
    let bytes = ::hex::decode(body).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    let body = text.strip_prefix(CURSOR_PREFIX).ok_or(MalformedCursor)?;
    let (token_scope, key) = body.split_once(CURSOR_SEPARATOR).ok_or(MalformedCursor)?;
    if token_scope != scope {
        return Err(MalformedCursor);
    }
    Ok(Some(key.to_string()))
}

/// Take the page of `ordered` (sorted by key) that follows `after`, at most `max_results`
/// long. The cursor it mints carries `scope`, so it only resumes the listing that issued it.
///
/// `MaxResults` is `@range(1, 1000)`, so the boundary validator has already refused anything
/// outside it; an absent one takes a full page.
pub fn page_of<T>(
    scope: &str,
    ordered: impl IntoIterator<Item = (String, T)>,
    after: Option<&str>,
    max_results: Option<i32>,
) -> Page<T> {
    debug_assert!(
        max_results.is_none_or(|value| (1..=MAX_RESULTS_LIMIT).contains(&value)),
        "MaxResults is @range(1, {MAX_RESULTS_LIMIT}) and is refused at the boundary, so a \
         handler can only ever see a value inside it; got {max_results:?}"
    );
    let limit = max_results
        .unwrap_or(MAX_RESULTS_LIMIT)
        .clamp(1, MAX_RESULTS_LIMIT) as usize;
    let mut items = Vec::new();
    let mut last_key = String::new();
    let mut more_follow = false;
    for (key, item) in ordered
        .into_iter()
        .filter(|(key, _)| after.is_none_or(|after| key.as_str() > after))
    {
        if items.len() == limit {
            // One item beyond the page proves a next page exists; the cursor names where
            // this page ended, so resuming never repeats or skips an item.
            more_follow = true;
            break;
        }
        last_key = key;
        items.push(item);
    }
    Page {
        items,
        next_token: more_follow.then(|| encode_cursor(scope, &last_key)),
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPcaconnectorscepPcaconnectorscepState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPcaconnectorscepPcaconnectorscepState>)
}
