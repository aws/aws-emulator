// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-securityagent-securityagent service.
//!
//! The Application records every application operation reads and writes live here, so
//! `CreateApplication`, `GetApplication`, `ListApplications`, `UpdateApplication` and
//! `DeleteApplication` all see one copy of each resource. The private connection records
//! `CreatePrivateConnection`, `DescribePrivateConnection` and `ListPrivateConnections`
//! share live here for the same reason, and so do the target domain records:
//! `CreateTargetDomain` writes one and `BatchGetTargetDomains` reads that same record
//! back — including the outcome of the ownership check `VerifyTargetDomain` starts on
//! it, which is stored on the record and so reads the same through every operation that
//! projects it.
//!
//! The agent space records sit beside them in the same struct, so `CreateAgentSpace`
//! and `BatchGetAgentSpaces` — and every agent space operation implemented after them —
//! read ONE copy of each agent space.
//!
//! The artifacts uploaded into an agent space live here too, keyed by their own id and
//! carrying the id of the space that holds them, so `AddArtifact` writes one record and
//! `GetArtifact` — and every artifact operation implemented after it — reads back THAT
//! record rather than a second copy of it.
//!
//! The security requirement packs live here for the same reason:
//! `CreateSecurityRequirementPack`, `GetSecurityRequirementPack`,
//! `ListSecurityRequirementPacks` — and every pack operation implemented after them —
//! read and write ONE copy of each pack record.
//!
//! The security requirements a pack holds live beside the packs, keyed by the pair of
//! the owning `packId` and the requirement's own name, so
//! `BatchCreateSecurityRequirements` writes one record and
//! `BatchGetSecurityRequirements` and `ListSecurityRequirements` — and every requirement
//! operation implemented after them — read back THAT record rather than a second copy of
//! it.
//!
//! The helpers below (identity minting, the IdC application ARN, the account's network,
//! the modelled projections and the pagination cursor codec) are shared by those
//! handlers rather than duplicated per operation.

use std::collections::{BTreeMap, BTreeSet};
use std::time::{Duration, SystemTime};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AWSResources, AgentSpace, ApplicationSummary, Artifact, ArtifactMetadataItem, ArtifactSummary,
    BatchCreateSecurityRequirementResult, BatchGetSecurityRequirementResult, CodeReviewSettings,
    DnsVerification, HttpVerification, IdCConfiguration, PrivateConnectionSummary,
    SecurityRequirementPackSummary, SecurityRequirementSummary, TargetDomain,
    VerificationDetails,
};

/// The region every value this service derives from the request's region carries when
/// the request arrived without a SigV4 credential scope to read one from.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many summary entries one listing page holds when the caller asks for no page
/// size. Shared by every paginated listing of this service (`ListApplications`,
/// `ListSecurityRequirementPacks`, `ListPrivateConnections`), which all take the one
/// modelled `MaxResults` shape.
///
/// CHOSEN, not cited: `MaxResults` carries neither `@default` nor `@range` in the
/// model, so nothing pins this value. It only bounds the size of one response.
pub const DEFAULT_MAX_RESULTS: i32 = 100;

/// The most summary entries one listing page can hold, whatever the caller asks for.
///
/// CHOSEN, not cited, for the same reason as [`DEFAULT_MAX_RESULTS`]: `MaxResults`
/// carries no `@range`, so nothing is refused at the boundary and a caller could
/// otherwise ask for the whole store in one response. A larger request is clamped to
/// this ceiling and the page simply carries a `nextToken`, so no record becomes
/// unreachable — only the work and the response size one call can demand are bounded.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The most applications this emulator stores for the account before
/// `CreateApplication` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's application quota is not in the model. It is
/// set far above any plausible client's application count, so normal use never meets
/// it, while the store a caller can grow stays bounded.
pub const MAX_APPLICATIONS: usize = 1_000;

/// The most target domains this emulator stores for the account before
/// `CreateTargetDomain` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited, for the same reason as [`MAX_APPLICATIONS`]: the real service's
/// target domain quota is not in the model. It is set far above any plausible client's
/// domain count, so normal use never meets it, while the store a caller can grow stays
/// bounded.
pub const MAX_TARGET_DOMAINS: usize = 1_000;

/// The sentence a refusal of a target domain the account does not hold states.
///
/// RECORDED, not modelled: the recorded 404 for an unknown `targetDomainId` carries this
/// sentence, which is wholly FIXED — it names the resource CLASS and carries no value from
/// the request, so a refusal cannot echo a caller's own identifier back — and it is stated
/// here once, so every operation that fails to resolve a target domain answers the same
/// sentence rather than the documentation of the error class.
pub const TARGET_DOMAIN_NOT_FOUND: &str = "The specified target domain does not exist.";

/// The longest a `targetDomainName` may be.
///
/// NOT chosen: `targetDomainName` carries no `@length` in the model, but the operation
/// registers a web domain, and a DNS name is at most 253 characters in presentation
/// form (RFC 1035 §2.3.4 with RFC 1123 §2.1).
pub const MAX_DOMAIN_NAME_LEN: usize = 253;

/// The longest one dot-separated label of a `targetDomainName` may be — 63 characters
/// (RFC 1035 §2.3.4).
pub const MAX_DOMAIN_LABEL_LEN: usize = 63;

/// The `DomainVerificationMethod` that verifies ownership through a DNS TXT record
/// (source: `smithy_query(query="shape",shape="DomainVerificationMethod")`).
pub const VERIFICATION_METHOD_DNS_TXT: &str = "DNS_TXT";

/// The `DomainVerificationMethod` that verifies ownership through an HTTP route.
pub const VERIFICATION_METHOD_HTTP_ROUTE: &str = "HTTP_ROUTE";

/// The `DomainVerificationMethod` that verifies ownership by IP from a private VPC.
pub const VERIFICATION_METHOD_PRIVATE_VPC: &str = "PRIVATE_VPC";

/// Every value the `DomainVerificationMethod` enum admits, in the model's order
/// (source: `smithy_query(query="shape",shape="DomainVerificationMethod")`), so a
/// value outside the enum is refused rather than stored and read back as a method
/// with no challenge.
pub const VERIFICATION_METHOD_VALUES: [&str; 3] = [
    VERIFICATION_METHOD_DNS_TXT,
    VERIFICATION_METHOD_HTTP_ROUTE,
    VERIFICATION_METHOD_PRIVATE_VPC,
];

/// The `TargetDomainStatus` a registered target domain carries while its ownership is
/// not yet proven (source: the `PENDING` value's own doc, "Domain verification is
/// pending").
///
/// A registration does NOT leave this status on a timer: ownership is proven by
/// something the domain's owner publishes, so the status moves only once an ownership
/// CHECK has been made, which is what `VerifyTargetDomain` starts.
pub const TARGET_DOMAIN_STATUS_PENDING: &str = "PENDING";

/// The `TargetDomainStatus` an ownership check leaves a target domain in when the
/// check could not read the proof the domain's owner was asked to publish (source: the
/// `UNREACHABLE` value's own doc, "Domain is unreachable for verification").
///
/// Every challenge this service issues lives ON the domain — a record in its DNS zone,
/// a document its web server serves, an address reached from the private VPC — and
/// nothing of that is reachable from inside this emulator, which makes no outbound
/// request and holds no domain's zone. So a check this emulator makes reads no proof,
/// and the one status that states that is this one: `VERIFIED` would claim a proof
/// nothing read, `FAILED` would report a proof read and rejected, and `PENDING` would
/// hide a check that has already been made.
pub const TARGET_DOMAIN_STATUS_UNREACHABLE: &str = "UNREACHABLE";

/// How long after the `VerifyTargetDomain` call that starts it an ownership check takes
/// to settle, i.e. how long the domain stays `PENDING` before the check's outcome is
/// due.
///
/// CHOSEN, not cited: the model pins no duration for the check. It is non-zero, so a
/// caller that reads the domain straight after the call still sees the `PENDING` the
/// check runs in, and short enough that the outcome is due for any later call. The
/// outcome is materialised by the next call that reads the record (see
/// [`ComAmazonawsSecurityagentSecurityagentState::settle_target_domain_verifications`]),
/// never by a timer of its own.
pub const VERIFICATION_SETTLE_AFTER: Duration = Duration::from_secs(3);

/// The label prefixed onto the domain to name the TXT record a `DNS_TXT` target
/// domain's owner publishes.
///
/// RECORDED, not modelled: the model pins the record TYPE (`DNSRecordType` admits only
/// `TXT`) and says the details carry "the name of the DNS record to create for
/// verification", but not the label. The recorded `dnsRecordName` of a pending
/// `DNS_TXT` domain is this label prefixed onto the requested domain, and the label is
/// the actionable part of the response: a client that publishes the TXT record under
/// any other name publishes it where the real service never looks.
const DNS_CHALLENGE_LABEL: &str = "_aws_securityagent-challenge";

/// The type of that record. NOT chosen: `DNSRecordType` admits exactly one value,
/// `TXT` ("Currently, only TXT is supported").
const DNS_CHALLENGE_RECORD_TYPE: &str = "TXT";

/// The route an `HTTP_ROUTE` target domain's owner serves its verification document at.
///
/// RECORDED, not modelled: the model says the details carry "the HTTP route path where
/// the verification token must be served" without pinning the path. The recorded
/// `routePath` is this one FIXED, token-free relative path — the token travels beside
/// it in `httpRoute.token`, not inside the path — so every domain serves the challenge
/// at the same place, and a client wires its web server for a static `.json` document
/// rather than for a per-domain route.
const HTTP_CHALLENGE_ROUTE_PATH: &str = ".well-known/aws/securityagent-domain-verification.json";

/// The domain-separation prefix hashed into every verification token, so a token can
/// only ever stand for a target domain of this service.
const VERIFICATION_TOKEN_CONTEXT: &str = "securityagent-domain-verification:";

/// The most agent spaces this emulator stores for the account before
/// `CreateAgentSpace` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited, for the same reason as [`MAX_APPLICATIONS`]: the real service's
/// agent space quota is not in the model. It is set far above any plausible client's
/// agent space count, so normal use never meets it, while the store a caller can grow
/// stays bounded.
pub const MAX_AGENT_SPACES: usize = 1_000;

/// The most target domains ONE agent space may be associated with.
///
/// CHOSEN, not cited: `TargetDomainIdList` carries no `@length` in the model, so nothing
/// pins this value. It bounds BOTH the list one create stores and the work every later
/// read of that record does — each associated id is expanded into the full stored target
/// domain record in the response's `targetDomains` — so neither the store nor one
/// response can be grown without end by a single request. It is set far above any
/// plausible client's association count, so normal use never meets it.
pub const MAX_AGENT_SPACE_TARGET_DOMAINS: usize = 100;

/// The sentence a refusal of an agent space the account does not hold states.
///
/// RECORDED, not modelled: the recorded 404 for an `agentSpaceId` that resolves to no
/// agent space carries this sentence, which is wholly FIXED — it names the resource
/// CLASS under the name the service's own records use for an agent space, and carries no
/// value from the request, so a refusal never echoes a caller's own identifier back.
pub const AGENT_SPACE_NOT_FOUND: &str = "The specified agent instance does not exist.";

/// The sentence a refusal of an artifact the named agent space does not hold states,
/// for the `artifactId` the request named.
///
/// The service does not take the fixed form its other not-found sentences take
/// ([`AGENT_SPACE_NOT_FOUND`], [`TARGET_DOMAIN_NOT_FOUND`]): the recorded 404 for an
/// `artifactId` that resolves to no artifact names the id that was asked for, which is
/// the sentence's ONLY variable part — it is the caller's own input, echoed in the
/// refusal of it. Stated here once, so every operation that fails to resolve an artifact
/// answers the same sentence.
pub fn artifact_not_found(artifact_id: &str) -> String {
    format!("No artifact was found for id: {artifact_id}")
}

/// The most artifacts ONE agent space holds before `AddArtifact` answers the service's
/// `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited, for the same reason as [`MAX_AGENT_SPACES`]: the real service's
/// artifact quota is not in the model. Every artifact is a stored record holding the
/// bytes that were uploaded, so this bounds what one agent space can accumulate; it is
/// set far above any plausible client's artifact count, so normal use never meets it.
pub const MAX_ARTIFACTS_PER_AGENT_SPACE: usize = 100;

/// The most bytes one uploaded artifact may carry.
///
/// CHOSEN, not cited: `AddArtifact$artifactContent` targets the prelude `Blob`, which
/// carries no `@length`, so nothing pins this value. The bytes are kept on the stored
/// record and reported on every later read of the artifact, so without a ceiling a
/// single request could grow both the store and a response without end.
pub const MAX_ARTIFACT_CONTENT_BYTES: usize = 1 << 20;

/// The most security requirement packs this emulator stores for the account before
/// `CreateSecurityRequirementPack` answers the service's
/// `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's pack quota is not in the model. It is set far
/// above any plausible client's pack count, so normal use never meets it, while the
/// store a caller can grow stays bounded.
pub const MAX_SECURITY_REQUIREMENT_PACKS: usize = 1_000;

/// The most security requirements this emulator stores in ONE pack before
/// `BatchCreateSecurityRequirements` answers the service's
/// `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's per-pack requirement quota is not in the
/// model. It is set far above any plausible pack's requirement count, so normal use
/// never meets it, while the store one caller can grow stays bounded.
pub const MAX_SECURITY_REQUIREMENTS_PER_PACK: usize = 1_000;

/// The `code` one entry of a batch security requirement response carries when the pack
/// already holds a security requirement of that name.
///
/// RECORDED, not modelled: `BatchSecurityRequirementError$code` is "The error code", an
/// unconstrained string, and the recorded
/// `securityagent.batch-create-security-requirements-duplicate-partial-failure` run
/// reports this code — the SHAPE NAME of the request-level error the operation models
/// for the same condition — for the entry whose name the pack already held. So a client
/// matches one vocabulary whether a condition failed the whole call or one entry of it.
pub const BATCH_REQUIREMENT_ERROR_CONFLICT: &str = "ConflictException";

/// The `code` one entry of a batch security requirement response carries when the pack
/// holds no security requirement of that name.
///
/// RECORDED, not modelled: `BatchSecurityRequirementError$code` is "The error code", an
/// unconstrained string, and the recorded
/// `securityagent.batch-delete-security-requirements-partial-not-found` run reports this
/// token for the name the pack did not hold. It is deliberately NOT the name of the
/// request-level `ResourceNotFoundException` shape: that name belongs to the call-level
/// 404, while this is the token a client branches on to tell "this one name is absent"
/// from a refusal of the whole call.
pub const BATCH_REQUIREMENT_ERROR_NOT_FOUND: &str = "ResourceNotFound";

/// The `code` one entry carries when the service never processed it, because ANOTHER
/// entry of the same request failed.
///
/// RECORDED, not modelled: the recorded
/// `securityagent.batch-create-security-requirements-duplicate-partial-failure` run
/// reports this code for the entry BESIDE the one that conflicted, and the recorded
/// `securityagent.batch-delete-security-requirements-partial-not-found` run reports it
/// for the name beside the one the pack did not hold. It is not the name of any modelled
/// error, because nothing about that entry failed — the request simply did not act on
/// it.
pub const BATCH_REQUIREMENT_ERROR_NOT_PROCESSED: &str = "NotProcessed";

/// The sentence one batch entry carries when the pack already holds a security
/// requirement of that name.
///
/// Stated here once so every batch security requirement operation that meets the
/// condition answers the same sentence. The only variable part is the requirement name,
/// which the entry itself names.
pub fn security_requirement_exists_message(name: &str) -> String {
    format!("Security requirement already exists for name: {name}")
}

/// The sentence one batch entry carries when the pack holds no security requirement of
/// that name, stated here once for the same reason as
/// [`security_requirement_exists_message`] so every batch security requirement operation
/// that meets the condition answers the same sentence.
///
/// RECORDED, not modelled: `BatchSecurityRequirementError$message` is "The error
/// message", an unconstrained string. The fixed text is the service's; the only variable
/// part is the requirement name, taken verbatim from the request.
pub fn security_requirement_not_found_message(name: &str) -> String {
    format!("Security requirement with name: {name} was not found")
}

/// The sentence one batch entry carries when the service never processed it. Wholly
/// FIXED: it carries no value from the request, because nothing about this entry is what
/// failed. It names ANOTHER entry of the same request rather than an earlier one,
/// because a request can fail on an entry that sits behind the ones it did not act on.
pub const SECURITY_REQUIREMENT_NOT_PROCESSED: &str =
    "Security requirement was not processed because another entry in the request failed.";

/// The sentence a refusal to write security requirements into a pack AWS manages
/// states.
///
/// `BatchCreateSecurityRequirements` documents itself as creating requirements "in a
/// customer managed pack", and `UpdateSecurityRequirementPack` documents that "for AWS
/// managed packs, only status can be updated", so the contents of a shipped pack are
/// the service's and not a caller's to write.
pub fn aws_managed_pack_conflict_message(pack_id: &str) -> String {
    format!("Security requirements cannot be modified for AWS managed pack: {pack_id}")
}

/// The `managementType` every pack this API creates carries.
///
/// Source: `CreateSecurityRequirementPack`'s own documentation — "Creates a *customer
/// managed* security requirement pack" — and the `ManagementType` enum
/// (`AWS_MANAGED | CUSTOMER_MANAGED`), whose other member describes packs AWS ships
/// rather than ones a caller creates.
pub const MANAGEMENT_TYPE_CUSTOMER_MANAGED: &str = "CUSTOMER_MANAGED";

/// The `managementType` of a pack AWS itself ships: the other member of the
/// `ManagementType` enum (`smithy_query(query="shape",shape="ManagementType")`).
pub const MANAGEMENT_TYPE_AWS_MANAGED: &str = "AWS_MANAGED";

/// The `vendorName` every AWS managed pack carries — "The vendor name for AWS managed
/// packs".
///
/// RECORDED, not modelled: the model declares the member but not its contents, and a
/// fresh account's catalog reports this vendor for each of its built-in packs.
pub const AWS_MANAGED_VENDOR_NAME: &str = "Amazon Web Services";

/// One entry of the catalog of security requirement packs AWS ships with every
/// account: the id a client references the pack by, its display name, the description
/// it is shipped with, and the status it is shipped in.
///
/// Only the status is a caller's to change — `UpdateSecurityRequirementPack` documents
/// "For AWS managed packs, only status can be updated" — so the rest of the record is
/// the service's and is seeded, never written.
#[derive(Debug, Clone, Copy)]
pub struct AwsManagedPack {
    pub pack_id: &'static str,
    pub name: &'static str,
    /// The description this pack is shipped with, which every read of it reports.
    /// Static catalog content like the name beside it, so it is part of this table
    /// rather than something a caller supplies.
    pub description: &'static str,
    pub status: &'static str,
}

/// When the AWS managed security requirement pack catalog was published, as seconds
/// since the Unix epoch: 2025-12-01T00:00:00Z.
///
/// RECORDED, not modelled: the model says nothing about which packs AWS ships or when,
/// and the `securityagent-catalog-list-security-requirement-packs` fixture reports ONE
/// instant as both `createdAt` and `updatedAt` for EVERY managed pack. That instant is a
/// property of the shipped catalog, not of this process: a pack the account was created
/// with was not published when the emulator started, and a client that caches the
/// catalog or asks "has it changed since X" must see the same instant across restarts.
/// So it is written here instead of read from the clock, and it moves only when the
/// catalog revision below it does.
pub const AWS_MANAGED_PACK_PUBLISHED_AT_EPOCH_SECONDS: u64 = 1_764_547_200;

/// [`AWS_MANAGED_PACK_PUBLISHED_AT_EPOCH_SECONDS`] as the `SystemTime` the seeded
/// records carry, so every managed pack's stamps come from one place.
pub fn aws_managed_pack_published_at() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(AWS_MANAGED_PACK_PUBLISHED_AT_EPOCH_SECONDS)
}

/// The AWS managed pack catalog a fresh account holds.
///
/// RECORDED, not modelled: the model declares the pack shapes and the `AWS_MANAGED`
/// management type but says nothing about which packs AWS ships, so this table is the
/// catalog a fresh account's `ListSecurityRequirementPacks` reports — recorded by the
/// `securityagent-catalog-list-security-requirement-packs` fixture at step `task#1`: the
/// base pack enabled by default and the three framework packs shipped disabled. Every
/// entry is read back through the same projection as a created pack, so nothing here is a
/// response the handlers synthesise.
pub const AWS_MANAGED_PACKS: [AwsManagedPack; 4] = [
    AwsManagedPack {
        pack_id: "srp-am-base",
        name: "ASA Base Pack",
        description: "Default security requirements shipped with AWS Security Agent",
        status: PACK_STATUS_ENABLED,
    },
    AwsManagedPack {
        pack_id: "srp-am-nist-csf",
        name: "NIST CSF Pack",
        description: "Security requirements derived from the NIST Cybersecurity Framework",
        status: PACK_STATUS_DISABLED,
    },
    AwsManagedPack {
        pack_id: "srp-am-pci-dss",
        name: "PCI DSS Pack",
        description: "Security requirements derived from the PCI DSS v4.0.1 standard",
        status: PACK_STATUS_DISABLED,
    },
    AwsManagedPack {
        pack_id: "srp-am-well-architected",
        name: "AWS Well-Architected Pack",
        description:
            "Security requirements derived from the AWS Well-Architected Framework Security Pillar",
        status: PACK_STATUS_DISABLED,
    },
];

/// The `status` a pack created without one carries.
///
/// Source: the `status` member's documentation on `CreateSecurityRequirementPackInput`
/// — "The status of the pack. Defaults to ENABLED if not provided."
pub const PACK_STATUS_ENABLED: &str = "ENABLED";

/// The other member of the `SecurityRequirementPackStatus` enum.
pub const PACK_STATUS_DISABLED: &str = "DISABLED";

/// Every value the `SecurityRequirementPackStatus` enum admits.
///
/// Source: `smithy_query(query="shape", shape="SecurityRequirementPackStatus")` —
/// `["ENABLED", "DISABLED"]`.
pub const PACK_STATUS_VALUES: [&str; 2] = [PACK_STATUS_ENABLED, PACK_STATUS_DISABLED];

/// The prefix every `ListApplications` pagination cursor carries, so a token this
/// service did not mint is refused instead of being read as a resume point.
const PAGE_TOKEN_PREFIX: &str = "app-v1:";

/// The prefix every `ListSecurityRequirementPacks` pagination cursor carries. Distinct
/// from [`PAGE_TOKEN_PREFIX`], so a cursor one list minted is not read as a resume
/// point by the other.
const PACK_PAGE_TOKEN_PREFIX: &str = "pack-v1:";

/// The same, for `ListPrivateConnections`: a cursor minted for one listing names a
/// resume point in another listing's key space, so each listing carries its own prefix
/// and a cursor taken from the wrong one is refused.
const PRIVATE_CONNECTION_PAGE_TOKEN_PREFIX: &str = "privconn-v1:";

/// The same, for `ListArtifacts`. Distinct from every prefix above, so a cursor another
/// listing minted is not read as a resume point in the artifact key space.
const ARTIFACT_PAGE_TOKEN_PREFIX: &str = "artifact-v1:";

/// The same, for `ListSecurityRequirements`: its cursor names the security requirement
/// NAME the next page resumes at, within the pack the request names, so it is its own
/// key space and carries its own prefix.
const REQUIREMENT_PAGE_TOKEN_PREFIX: &str = "srq-v1:";

/// What separates the pack from the requirement name inside a `ListSecurityRequirements`
/// cursor: the ASCII unit separator, which no `packId` carries.
const REQUIREMENT_CURSOR_SEPARATOR: char = '\u{1f}';

/// The IAM Identity Center instance id an `idcInstanceArn` that names none falls back
/// to, so the minted `idcApplicationArn` is always well formed.
const FALLBACK_IDC_INSTANCE_ID: &str = "ssoins-emulator";

/// The IAM Identity Center configuration stored for an application.
///
/// Mirrors the modelled [`IdCConfiguration`] shape: `idcInstanceArn` is what the
/// caller supplied, `idcApplicationArn` is the ARN this emulator minted for the
/// Identity Center application it stands up alongside the application record.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct IdcConfig {
    pub idc_application_arn: Option<String>,
    pub idc_instance_arn: Option<String>,
}

impl IdcConfig {
    /// This configuration projected onto the modelled shape the API returns.
    pub fn modelled(&self) -> IdCConfiguration {
        IdCConfiguration {
            idc_application_arn: self.idc_application_arn.clone(),
            idc_instance_arn: self.idc_instance_arn.clone(),
        }
    }
}

/// One stored application: the top-level organizational unit every agent space, code
/// review, pentest, threat model and finding hangs off.
#[derive(Debug, Clone)]
pub struct Application {
    pub application_id: String,
    pub application_name: String,
    pub domain: String,
    pub role_arn: Option<String>,
    pub default_kms_key_id: Option<String>,
    pub idc_configuration: Option<IdcConfig>,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
}

impl Application {
    /// This application projected onto the `ApplicationSummary` shape
    /// `ListApplications` returns: the stored record's own fields, never request input.
    pub fn summary(&self) -> ApplicationSummary {
        ApplicationSummary {
            application_id: self.application_id.clone(),
            application_name: self.application_name.clone(),
            domain: self.domain.clone(),
            default_kms_key_id: self.default_kms_key_id.clone(),
        }
    }
}

/// One stored agent space: "a dedicated workspace for securing a specific
/// application" (source: the `AgentSpace` shape doc), carrying the members the caller
/// supplied plus the identity and the timestamps the service mints.
///
/// The modelled shapes are stored as they arrived rather than flattened into local
/// mirrors, so a read projects the stored record and never reconstructs one from
/// request text.
#[derive(Debug, Clone)]
pub struct AgentSpaceRecord {
    pub agent_space_id: String,
    /// The agent space's ARN, minted at create time from the region the create was
    /// signed for — stored rather than recomputed per read, so every read of the record
    /// reports the one ARN the create answered, whatever region a later read arrives in.
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub aws_resources: Option<AWSResources>,
    pub target_domain_ids: Option<Vec<String>>,
    pub code_review_settings: Option<CodeReviewSettings>,
    pub kms_key_id: Option<String>,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented. `tags` is not a member of any agent
    /// space response shape, so it is never projected below.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl AgentSpaceRecord {
    /// This agent space projected onto the [`AgentSpace`] shape every agent space
    /// response carries: the stored record's own fields, never request input.
    ///
    /// `target_domains` is the store's target domain map, so each id the agent space is
    /// associated with is echoed RESOLVED — the stored record of that domain, carrying
    /// the challenge it is waiting on. An id no target domain record answers
    /// contributes no entry: the association is reported for the domains that exist,
    /// never as a fabricated record. Each DISTINCT id contributes at most one entry, and
    /// the stored list is capped at create time, so this projection's size is bounded by
    /// [`MAX_AGENT_SPACE_TARGET_DOMAINS`] however long a list the caller sent.
    ///
    /// `id`, `ownerId` and `arn` are recorded-but-unmodelled members (see
    /// `crate::ops::types::wire_shape`): the id again under the name the service's own
    /// record uses, the account every resource of this emulator belongs to, and the
    /// ARN minted at create time.
    pub fn modelled(&self, target_domains: &BTreeMap<String, TargetDomainRecord>) -> AgentSpace {
        // A list of no ids is the same state as no list at all: the agent space is
        // associated with nothing. An echo of the record reports that state by the
        // ABSENCE of both association members rather than by an empty array, so a
        // caller that distinguishes "not returned" from "returned empty" is never told
        // that an unset association is an authoritative empty one. (The READ path is the
        // documented exception: see [`Self::read_view`].)
        let associated = self
            .target_domain_ids
            .as_deref()
            .filter(|ids| !ids.is_empty());
        AgentSpace {
            agent_space_id: self.agent_space_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            aws_resources: self.aws_resources.clone(),
            target_domain_ids: associated.map(<[String]>::to_vec),
            code_review_settings: self.code_review_settings.clone(),
            kms_key_id: self.kms_key_id.clone(),
            created_at: Some(timestamp(self.created_at)),
            updated_at: Some(timestamp(self.updated_at)),
            id: self.agent_space_id.clone(),
            owner_id: ::engine::emulator_account_id().to_string(),
            arn: self.arn.clone(),
            target_domains: associated.map(|ids| {
                // Each DISTINCT id contributes at most one record, in the order it was
                // first named, so a list that repeats an id does not multiply the
                // answer. With the stored list capped at create time
                // ([`MAX_AGENT_SPACE_TARGET_DOMAINS`]) this expansion is bounded by that
                // cap whatever the request asked for.
                let mut answered = BTreeSet::new();
                ids.iter()
                    .filter(|id| answered.insert(id.as_str()))
                    .filter_map(|id| target_domains.get(id))
                    .map(TargetDomainRecord::modelled)
                    .collect()
            }),
        }
    }

    /// This agent space as a READ of the store reports it: [`Self::modelled`]'s
    /// projection with both of its collection members PRESENT rather than absent — a
    /// record that holds no association carries them as EMPTY lists.
    ///
    /// RECORDED, not modelled: a read of the store reports the record's collections, so
    /// `targetDomainIds` and the resolved `targetDomains` beside it are arrays on every
    /// agent space a read answers and a caller iterating either one loops zero times
    /// instead of dereferencing a missing member. The create and update echo paths are
    /// NOT this: there an association the record does not hold — no list, or a list of no
    /// ids — stays ABSENT, which is why the normalisation lives here and not in
    /// [`Self::modelled`].
    pub fn read_view(&self, target_domains: &BTreeMap<String, TargetDomainRecord>) -> AgentSpace {
        let mut view = self.modelled(target_domains);
        view.target_domain_ids = Some(view.target_domain_ids.unwrap_or_default());
        view.target_domains = Some(view.target_domains.unwrap_or_default());
        view
    }
}

/// The ARN of one agent space: `arn:aws:securityagent:<region>:<account>:agent-instance/<id>`.
///
/// The namespace is the service's own `arnNamespace`, `securityagent`
/// (`smithy_query(query="service_info")`); the account segment is the emulator's
/// canonical account, through [`::engine::emulator_arn`]. RECORDED, not modelled: the
/// resource type is `agent-instance` — the name the service's own records use for an
/// agent space, the same one the batch read reports its list of them under.
pub fn agent_space_arn(region: &str, agent_space_id: &str) -> String {
    ::engine::emulator_arn(
        AGENT_SPACE_ARN_SERVICE,
        region,
        &format!("{AGENT_SPACE_ARN_KIND}/{agent_space_id}"),
    )
}

/// The ARN namespace (and SigV4 signing name) of this service
/// (`smithy_query(query="service_info")`: `arnNamespace` `securityagent`).
pub const AGENT_SPACE_ARN_SERVICE: &str = "securityagent";

/// The ARN resource type an agent space carries; see [`agent_space_arn`].
pub const AGENT_SPACE_ARN_KIND: &str = "agent-instance";

/// A stored instant on the wire, as the modelled timestamp members of an agent space
/// carry it (`smithy_query(query="shape",shape="AgentSpace")`).
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// One stored artifact: the file `AddArtifact` uploaded into an agent space, plus the
/// identity and the timestamps the service minted for it.
///
/// An artifact belongs to exactly ONE agent space — every operation that names an
/// artifact names the space beside it — so the owning id is part of the record and a
/// read resolves the PAIR rather than the artifact id alone.
#[derive(Debug, Clone)]
pub struct ArtifactRecord {
    pub artifact_id: String,
    /// The agent space the artifact was uploaded to.
    pub agent_space_id: String,
    pub file_name: String,
    /// The `ArtifactType` the upload declared. One of the enum's members: the boundary
    /// validator refuses every other value before the handler runs.
    pub artifact_type: String,
    /// The bytes the upload carried, stored exactly as they arrived, so a read reports
    /// the content that was uploaded rather than a value rebuilt from anything else.
    pub content: Vec<u8>,
    /// When the artifact was last written — the instant it was uploaded, until something
    /// updates it. The only timestamp any artifact shape of this service carries
    /// (`smithy_query(query="shape",shape="ArtifactMetadataItem,ArtifactSummary")` and
    /// `GetArtifact`'s output: `updatedAt` and nothing beside it).
    pub updated_at: SystemTime,
}

impl ArtifactRecord {
    /// This artifact projected onto the [`Artifact`] shape a read of it answers: the
    /// stored content and the stored type, never request text.
    pub fn modelled(&self) -> Artifact {
        Artifact {
            contents: self.contents(),
            r#type: self.artifact_type.clone(),
        }
    }

    /// This artifact projected onto the [`ArtifactMetadataItem`] shape a metadata read
    /// answers: the pair of ids the record resolved to, the file name it was stored
    /// under and the instant it was last written — the artifact's metadata, which is
    /// every member of that shape and carries none of its content.
    ///
    /// Stated here beside [`Self::modelled`] so a metadata read and a full read of one
    /// artifact report the SAME stored record rather than two projections of it.
    pub fn metadata(&self) -> ArtifactMetadataItem {
        ArtifactMetadataItem {
            agent_space_id: self.agent_space_id.clone(),
            artifact_id: self.artifact_id.clone(),
            file_name: self.file_name.clone(),
            updated_at: timestamp(self.updated_at),
        }
    }

    /// This artifact projected onto the [`ArtifactSummary`] shape a listing of the agent
    /// space's artifacts answers: the three members the shape declares
    /// (`smithy_query(query="shape",shape="ArtifactSummary")` — `artifactId`, `fileName`
    /// and `artifactType`, each `@required`), read off this stored record. A summary
    /// carries no content: the bytes are reported by a read of the artifact itself.
    pub fn summary(&self) -> ArtifactSummary {
        ArtifactSummary {
            artifact_id: self.artifact_id.clone(),
            file_name: self.file_name.clone(),
            artifact_type: self.artifact_type.clone(),
        }
    }

    /// The stored bytes as the `contents` string a read reports: their base64 form, the
    /// same encoding the blob itself travels in on the wire.
    ///
    /// `Artifact$contents` is a string while the upload member (`AddArtifact`'s
    /// `artifactContent`) is a blob, and the artifact types the service accepts include
    /// binary documents (PNG, JPEG, PDF, DOC, DOCX) whose bytes encode no text at all,
    /// so the content a read reports is the base64 of the stored bytes — the one
    /// rendering that carries ANY uploaded file through a string member unchanged, and
    /// the one a caller reverses to get the bytes it uploaded back.
    pub fn contents(&self) -> String {
        ::aws_smithy_types::base64::encode(self.content.as_slice())
    }
}

/// One stored security requirement pack: the named collection of security
/// requirements `CreateSecurityRequirementPack` mints and every other pack operation
/// reads.
///
/// `management_type`, `vendor_name` and `import_status` are the service's to choose,
/// not the caller's: a pack this API creates is customer managed, so it carries no
/// vendor, and it holds no import until `ImportSecurityRequirements` starts one.
#[derive(Debug, Clone)]
pub struct SecurityRequirementPack {
    pub pack_id: String,
    pub name: String,
    pub description: Option<String>,
    /// `None` for a customer managed pack: the member documents it as "The vendor name
    /// for AWS managed packs".
    pub vendor_name: Option<String>,
    /// One of the `ManagementType` enum values; always
    /// [`MANAGEMENT_TYPE_CUSTOMER_MANAGED`] for a pack created through this API.
    pub management_type: String,
    /// One of [`PACK_STATUS_VALUES`].
    pub status: String,
    /// The `SecurityRequirementPackImportStatus` of the pack's last import, or `None`
    /// while no import has been started for it.
    pub import_status: Option<String>,
    pub kms_key_id: Option<String>,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl SecurityRequirementPack {
    /// One entry of the seeded AWS managed catalog as a stored record: the catalog's own
    /// id, name, description and shipped status, the AWS vendor, and the catalog's
    /// publication instant as both stamps — nothing has updated a pack the account was
    /// created with, and when it was published is not when this process started.
    pub fn aws_managed(entry: &AwsManagedPack) -> Self {
        let published_at = aws_managed_pack_published_at();
        Self {
            pack_id: entry.pack_id.to_string(),
            name: entry.name.to_string(),
            description: Some(entry.description.to_string()),
            vendor_name: Some(AWS_MANAGED_VENDOR_NAME.to_string()),
            management_type: MANAGEMENT_TYPE_AWS_MANAGED.to_string(),
            status: entry.status.to_string(),
            // No import has been started for a shipped pack.
            import_status: None,
            kms_key_id: None,
            tags: BTreeMap::new(),
            created_at: published_at,
            updated_at: published_at,
        }
    }

    /// This pack projected onto the `SecurityRequirementPackSummary` shape
    /// `ListSecurityRequirementPacks` returns: the stored record's own fields, never
    /// request input.
    pub fn summary(&self) -> SecurityRequirementPackSummary {
        SecurityRequirementPackSummary {
            pack_id: self.pack_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            vendor_name: self.vendor_name.clone(),
            management_type: self.management_type.clone(),
            status: self.status.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The sentence every operation that fails to resolve a `packId` answers, for the pack
/// the request named.
///
/// RECORDED, not modelled: the pack reads answer this wording for an id the account
/// holds no pack for, and it is built here once so every operation that resolves a
/// `packId` — the pack reads and the requirement listing inside a pack — states the
/// same sentence rather than the documentation of the error class.
pub fn pack_not_found_message(pack_id: &str) -> String {
    format!("No security requirement pack found for ID: {pack_id}")
}

/// One stored security requirement: one rule inside a security requirement pack, as
/// `BatchCreateSecurityRequirements` writes it and every other security requirement
/// operation reads it.
///
/// A requirement belongs to exactly ONE pack — every operation that names a requirement
/// names the pack beside it, and the batch operations address requirements by
/// `securityRequirementNames` alone — so the owning `packId` is part of the record and
/// the NAME is the requirement's identity WITHIN that pack, never across packs.
#[derive(Debug, Clone)]
pub struct SecurityRequirement {
    pub pack_id: String,
    pub name: String,
    pub description: String,
    pub domain: String,
    pub evaluation: String,
    /// Optional on the create entry ("remediation" carries no `@required`), so a
    /// requirement created without one carries none on every later read.
    pub remediation: Option<String>,
    pub created_at: SystemTime,
    /// Moves only when a later write changes the record; equal to `created_at` until
    /// then.
    pub updated_at: SystemTime,
}

impl SecurityRequirement {
    /// The record one `CreateSecurityRequirementEntry` becomes: the entry's own fields,
    /// the pack it was created in, and `now` as both stamps — nothing has updated a
    /// requirement at the instant it is created.
    pub fn created(
        pack_id: &str,
        name: String,
        description: String,
        domain: String,
        evaluation: String,
        remediation: Option<String>,
        now: SystemTime,
    ) -> Self {
        Self {
            pack_id: pack_id.to_string(),
            name,
            description,
            domain,
            evaluation,
            remediation,
            created_at: now,
            updated_at: now,
        }
    }

    /// Apply one update entry's SUPPLIED members to this stored record, so every later
    /// read of this pack and name answers what was written.
    ///
    /// The members of an update entry are optional, so a member the request carries
    /// replaces the stored value and a member it does not carry leaves that field as it
    /// was — an update of one member is not a clear of the others. The name is the
    /// requirement's identity ("an immutable identifier ... cannot be changed once the
    /// requirement is created") and so is not writable here. `updated_at` is when the
    /// record last changed, so it moves for an entry that wrote something and stays put
    /// for one that carried no writable member at all.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        domain: Option<String>,
        evaluation: Option<String>,
        remediation: Option<String>,
        now: SystemTime,
    ) {
        let mut mutated = false;
        if let Some(description) = description {
            self.description = description;
            mutated = true;
        }
        if let Some(domain) = domain {
            self.domain = domain;
            mutated = true;
        }
        if let Some(evaluation) = evaluation {
            self.evaluation = evaluation;
            mutated = true;
        }
        if let Some(remediation) = remediation {
            self.remediation = Some(remediation);
            mutated = true;
        }
        if mutated {
            self.updated_at = now;
        }
    }

    /// This requirement projected onto the `SecurityRequirementSummary` shape
    /// `ListSecurityRequirements` returns: the STORED record's own fields, never request
    /// input. The summary carries the pack, the name, the description and the stamps;
    /// the domain, the evaluation and the remediation are the batch reads' to report.
    pub fn summary(&self) -> SecurityRequirementSummary {
        SecurityRequirementSummary {
            pack_id: self.pack_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// This requirement projected onto the `BatchCreateSecurityRequirementResult`
    /// shape: the STORED record's own fields, so the create answers what a later read
    /// answers rather than echoing the request.
    pub fn create_result(&self) -> BatchCreateSecurityRequirementResult {
        BatchCreateSecurityRequirementResult {
            pack_id: self.pack_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            domain: self.domain.clone(),
            evaluation: self.evaluation.clone(),
            remediation: self.remediation.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }

    /// The same record projected onto `BatchGetSecurityRequirementResult`, which
    /// carries the same members: one projection per shape, both off the one stored
    /// record.
    pub fn get_result(&self) -> BatchGetSecurityRequirementResult {
        BatchGetSecurityRequirementResult {
            pack_id: self.pack_id.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            domain: self.domain.clone(),
            evaluation: self.evaluation.clone(),
            remediation: self.remediation.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.updated_at),
        }
    }
}

/// The identity minted for a new application: the three values that are the service's
/// to choose rather than the caller's to supply.
#[derive(Debug, Clone)]
pub struct ApplicationIdentity {
    pub application_id: String,
    pub application_name: String,
    pub domain: String,
    /// The monotonic sequence number behind this identity, used to mint the IAM
    /// Identity Center application ARN that goes with it.
    pub sequence: u64,
}

// ---------------------------------------------------------------------------
// Private connections
// ---------------------------------------------------------------------------

/// The two `PrivateConnectionType` values
/// (`smithy_query(query="shape",shape="PrivateConnectionType")`).
pub const TYPE_SERVICE_MANAGED: &str = "SERVICE_MANAGED";
/// See [`TYPE_SERVICE_MANAGED`].
pub const TYPE_SELF_MANAGED: &str = "SELF_MANAGED";

/// The `PrivateConnectionStatus` a connection carries while the clock still owes it its
/// `ACTIVE` status, and the one it carries from then on
/// (`smithy_query(query="shape",shape="PrivateConnectionStatus")`).
pub const STATUS_CREATE_IN_PROGRESS: &str = "CREATE_IN_PROGRESS";
/// See [`STATUS_CREATE_IN_PROGRESS`].
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// How long after its create call a private connection still reports
/// `CREATE_IN_PROGRESS`.
///
/// A created private connection is answered in the non-terminal
/// `CREATE_IN_PROGRESS` status and reaches `ACTIVE` on its own, so the transition is
/// one the CLOCK owes. AWS documents no duration for it, so this is the emulator's
/// CHOSEN span, inside the 250–750 ms window this project fixes for such a
/// transition. It is never zero, so a caller that reads straight back still observes
/// the transient status; nothing sleeps for it — the due instant is stored and
/// compared on read.
pub const CREATE_TRANSITION: Duration = Duration::from_millis(500);

/// The `dnsResolution` a service-managed create that names none takes.
///
/// Cited: `ServiceManagedInput$dnsResolution` — "The DNS resolution mode for the
/// resource gateway. Defaults to PUBLIC when not set."
pub const DEFAULT_DNS_RESOLUTION: &str = "PUBLIC";

/// The most private connections this emulator stores for the account before
/// `CreatePrivateConnection` answers the service's `ServiceQuotaExceededException`.
///
/// CHOSEN, not cited: the real service's private connection quota is not in the model.
/// It is set far above any plausible client's connection count, so normal use never
/// meets it, while the store a caller can grow stays bounded.
pub const MAX_PRIVATE_CONNECTIONS: usize = 100;

/// The sentence a refusal of a private connection the account does not hold states.
///
/// RECORDED, not modelled: `ResourceNotFoundException` declares only a `message`, and the
/// shape's documentation ("The specified resource was not found. …") is the doc of the
/// error CLASS, not the sentence the service answers. The recorded 404 for a private
/// connection carries this sentence instead, whole: the trailing `null` is how the service
/// renders the INTERNAL record identifier it names the resource by, which a lookup that
/// resolved no record has none of. Nothing from the request reaches it — a refusal echoes
/// back no name a caller sent — so every operation that fails to resolve a private
/// connection answers the one sentence.
pub const PRIVATE_CONNECTION_NOT_FOUND: &str = "Private Connection not found: null";

/// The ARN namespace (and SigV4 signing name) of VPC Lattice, the service that owns the
/// resource gateways and resource configurations a private connection is built on.
pub const VPC_LATTICE_ARN_SERVICE: &str = "vpc-lattice";

/// The ARN resource type of a VPC Lattice resource gateway, as
/// `com.amazonaws.vpclattice#ResourceGatewayArn`'s pattern spells it
/// (`…:resourcegateway/rgw-…`).
pub const KIND_RESOURCE_GATEWAY: &str = "resourcegateway";
/// The ARN resource type of a VPC Lattice resource configuration, as
/// `com.amazonaws.vpclattice#ResourceConfigurationArn`'s pattern spells it
/// (`…:resourceconfiguration/rcfg-…`).
pub const KIND_RESOURCE_CONFIGURATION: &str = "resourceconfiguration";

/// The id prefix of a VPC Lattice resource gateway
/// (`com.amazonaws.vpclattice#ResourceGatewayId`: `^rgw-[0-9a-z]{17}$`).
pub const RESOURCE_GATEWAY_ID_PREFIX: &str = "rgw-";
/// The id prefix of a VPC Lattice resource configuration
/// (`com.amazonaws.vpclattice#ResourceConfigurationId`: `^rcfg-[0-9a-z]{17}$`).
pub const RESOURCE_CONFIGURATION_ID_PREFIX: &str = "rcfg-";
/// How many `[0-9a-z]` characters follow the prefix of either id, from those same two
/// patterns.
pub const LATTICE_ID_SUFFIX_LEN: usize = 17;

/// The lowercase base-36 alphabet those id patterns admit (`[0-9a-z]`).
const BASE36_LOWER: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";

/// How many of a VPC Lattice identifier's [`LATTICE_ID_SUFFIX_LEN`] base-36 digits
/// carry the never-reused number the account's mint drew for that resource, the rest
/// carrying a digest of the mint's namespace. Ten digits is 36^10 resources before the
/// low part could repeat — far past the connections the quota admits — and leaves the
/// remaining digits to tell one mint from another. See
/// [`IdentifierMint::next_lattice_suffix`], which is the one place the split is applied.
const LATTICE_ID_SEQUENCE_DIGITS: u32 = 10;

/// The digits above them, which carry the namespace digest.
const LATTICE_ID_NAMESPACE_DIGITS: u32 = LATTICE_ID_SUFFIX_LEN as u32 - LATTICE_ID_SEQUENCE_DIGITS;

/// One stored private connection: the record every private connection operation reads.
#[derive(Debug, Clone)]
pub struct PrivateConnection {
    /// The caller-chosen name, unique within the account — this record's key.
    pub name: String,
    /// `PrivateConnectionType`: [`TYPE_SERVICE_MANAGED`] or [`TYPE_SELF_MANAGED`].
    pub connection_type: String,
    /// The VPC Lattice resource gateway this connection reaches the provider through.
    /// Present for a service-managed connection, whose gateway the service creates;
    /// absent for a self-managed one, where the gateway is the caller's own and only
    /// the resource configuration is named to the service.
    pub resource_gateway_id: Option<String>,
    /// The VPC Lattice resource configuration: the one the service created beside the
    /// gateway for a service-managed connection, or the one the caller named for a
    /// self-managed one, as the account holds it (the bare id, not the request's form).
    pub resource_configuration_id: Option<String>,
    /// `hostAddress` as the service-managed create supplied it.
    pub host_address: Option<String>,
    /// The VPC the service-managed resource gateway was created in.
    pub vpc_id: Option<String>,
    /// The subnets that gateway spans, persisted so a later read reports the placement
    /// this connection was created with.
    pub subnet_ids: Vec<String>,
    /// The security groups attached to that gateway.
    pub security_group_ids: Vec<String>,
    /// `ipAddressType` as supplied, `None` when the create named none.
    pub ip_address_type: Option<String>,
    /// `ipv4AddressesPerEni` as supplied.
    pub ipv4_addresses_per_eni: Option<i32>,
    /// The TCP port ranges a consumer may reach the resource on, as supplied.
    pub port_ranges: Vec<String>,
    /// `ResourceConfigDnsResolution`, defaulted per [`DEFAULT_DNS_RESOLUTION`] for a
    /// service-managed connection; `None` for a self-managed one, whose resource
    /// configuration the caller owns and whose DNS mode this service does not choose.
    pub dns_resolution: Option<String>,
    /// The tags supplied at create time.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// The instant the clock owes this connection its `ACTIVE` status.
    pub active_at: SystemTime,
}

impl PrivateConnection {
    /// `PrivateConnectionStatus` at `now`: the transient create status until the stored
    /// due instant passes, [`STATUS_ACTIVE`] from then on. A pure function of stored
    /// state and the clock, so reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            STATUS_ACTIVE
        } else {
            STATUS_CREATE_IN_PROGRESS
        }
    }

    /// This connection projected onto the `PrivateConnectionSummary` shape, which
    /// carries every member `CreatePrivateConnection`, `DescribePrivateConnection` and
    /// `ListPrivateConnections` report — so the three answer ONE projection of the
    /// stored record rather than three hand-written ones.
    pub fn summary(&self, now: SystemTime) -> PrivateConnectionSummary {
        PrivateConnectionSummary {
            name: self.name.clone(),
            r#type: self.connection_type.clone(),
            status: self.status(now).to_string(),
            resource_gateway_id: self.resource_gateway_id.clone(),
            host_address: self.host_address.clone(),
            vpc_id: self.vpc_id.clone(),
            resource_configuration_id: self.resource_configuration_id.clone(),
            // The certificate a create supplies is the CALLER's own PEM chain, and the
            // expiry AWS reports here is that certificate's `notAfter`. This emulator
            // reads the chain's PEM envelope but does not decode the X.509 inside it,
            // so it reports no expiry rather than a value it would have had to invent;
            // a value that is not a PEM certificate chain is still refused at create
            // time.
            certificate_expiry_time: None,
            dns_resolution: self.dns_resolution.clone(),
            // No stored connection has failed: this emulator moves a connection from
            // `CREATE_IN_PROGRESS` to `ACTIVE` and never to a failed status, so there
            // is no failure to describe. The member stays absent rather than carrying
            // a message for a failure that did not happen.
            failure_message: None,
            // AWS reports no tag map for an untagged resource, so an empty map is
            // absent rather than present and empty.
            tags: (!self.tags.is_empty())
                .then(|| self.tags.clone().into_iter().collect()),
        }
    }
}

/// One registered target domain: a web domain that must be verified before a pentest
/// may target it.
#[derive(Debug, Clone)]
pub struct TargetDomainRecord {
    pub target_domain_id: String,
    /// The domain name the caller registered, as supplied.
    pub domain_name: String,
    /// The `DomainVerificationMethod` ownership is to be proven with.
    pub verification_method: String,
    /// The record's `TargetDomainStatus`.
    pub verification_status: String,
    pub verification_status_reason: Option<String>,
    /// The token this service minted for the owner to publish. Stored, not recomputed
    /// per read, so every projection of the record carries the same token.
    pub verification_token: String,
    /// The tags supplied at create time, persisted so the tagging operations have
    /// state to read once they are implemented.
    pub tags: BTreeMap<String, String>,
    pub created_at: SystemTime,
    /// When this record last changed. At registration it is the creation instant; a
    /// verification call moves it, because the verification attempt it makes is part of
    /// the record. `VerifyTargetDomain` is the one operation whose output carries it
    /// (`updatedAt`); the `TargetDomain` shape the reads project has no such member
    /// (`smithy_query(query="shape",shape="TargetDomain")`).
    pub updated_at: SystemTime,
    /// When ownership was proven; `None` while the record is not `VERIFIED`.
    pub verified_at: Option<SystemTime>,
    /// When the ownership check a `VerifyTargetDomain` call started is due to settle,
    /// or `None` when no check is in flight. Stored rather than slept on: the next call
    /// that reads this record materialises the outcome the clock has made due.
    pub verification_due_at: Option<SystemTime>,
}

impl TargetDomainRecord {
    /// A newly registered target domain: `PENDING`, carrying the challenge its chosen
    /// verification method requires and no verification time yet.
    pub fn pending(
        identity: TargetDomainIdentity,
        domain_name: String,
        verification_method: String,
        tags: BTreeMap<String, String>,
    ) -> Self {
        let created_at = SystemTime::now();
        let mut record = Self {
            target_domain_id: identity.target_domain_id,
            domain_name,
            verification_method,
            verification_status: TARGET_DOMAIN_STATUS_PENDING.to_string(),
            verification_status_reason: None,
            verification_token: identity.verification_token,
            tags,
            created_at,
            // Nothing has changed the record since it was registered.
            updated_at: created_at,
            verified_at: None,
            // A registration starts no ownership check: `VerifyTargetDomain` does.
            verification_due_at: None,
        };
        // The reason names the challenge, which is built from the minted token, so it
        // is computed once the rest of the record exists.
        record.verification_status_reason = Some(record.pending_status_reason());
        record
    }

    /// The name of the TXT record a `DNS_TXT` target domain's owner publishes.
    pub fn dns_record_name(&self) -> String {
        format!("{DNS_CHALLENGE_LABEL}.{}", self.domain_name)
    }

    /// The path an `HTTP_ROUTE` target domain's owner serves the token at — the one
    /// fixed route, the same for every domain (see [`HTTP_CHALLENGE_ROUTE_PATH`]).
    pub fn http_route_path(&self) -> String {
        HTTP_CHALLENGE_ROUTE_PATH.to_string()
    }

    /// Why this record is still `PENDING`: what the domain's owner has to do before a
    /// verification call can find the proof.
    ///
    /// RECORDED, not modelled: one CONSTANT sentence per verification method — the
    /// reason names the method's challenge, not this request's domain or token, so it
    /// carries nothing the caller sent. The `DNS_TXT` and `HTTP_ROUTE` sentences are the
    /// recorded ones.
    fn pending_status_reason(&self) -> String {
        match self.verification_method.as_str() {
            VERIFICATION_METHOD_DNS_TXT => "Target domain is pending ownership verification. \
                 Add the DNS TXT record to the DNS resolver associated with your domain \
                 before attempting verification"
                .to_string(),
            VERIFICATION_METHOD_HTTP_ROUTE => "Target domain is pending ownership \
                 verification. Add the HTTP route path and token directly under your \
                 configured domain before attempting verification"
                .to_string(),
            // The remaining enum member, `PRIVATE_VPC`, proves ownership by IP from
            // inside the VPC that reaches the domain, so there is nothing to publish
            // on the domain itself. No other value reaches a stored record:
            // `verification_method_violation` refuses anything outside the enum before
            // the record is created. CHOSEN, not recorded: the corpus carries no
            // `PRIVATE_VPC` registration, so this sentence is this emulator's, worded
            // like the two above.
            _ => "Target domain is pending ownership verification. Reach the domain by IP \
                  from the private VPC associated with it before attempting verification"
                .to_string(),
        }
    }

    /// The challenge this record's verification method requires, as the modelled
    /// [`VerificationDetails`] shape.
    ///
    /// Exactly one of the per-method members is present, and only for a method that
    /// has a challenge to publish: `PRIVATE_VPC` is verified by IP from the VPC, so it
    /// carries the method alone.
    pub fn verification_details(&self) -> VerificationDetails {
        let mut details = VerificationDetails {
            method: Some(self.verification_method.clone()),
            dns_txt: None,
            http_route: None,
        };
        match self.verification_method.as_str() {
            VERIFICATION_METHOD_DNS_TXT => {
                details.dns_txt = Some(DnsVerification {
                    token: Some(self.verification_token.clone()),
                    dns_record_name: Some(self.dns_record_name()),
                    dns_record_type: Some(DNS_CHALLENGE_RECORD_TYPE.to_string()),
                })
            }
            VERIFICATION_METHOD_HTTP_ROUTE => {
                details.http_route = Some(HttpVerification {
                    token: Some(self.verification_token.clone()),
                    route_path: Some(self.http_route_path()),
                })
            }
            // `PRIVATE_VPC` (the only other enum member a stored record can carry)
            // publishes no challenge, so it keeps the method alone.
            _ => {}
        }
        details
    }

    /// Why this record is `UNREACHABLE`: the part of the challenge the ownership check
    /// could not read, and what to do about it.
    ///
    /// CHOSEN, not recorded: one constant sentence per verification method, worded like
    /// the pending ones above — it names the method's challenge, so it carries nothing
    /// the caller sent.
    fn unreachable_status_reason(&self) -> String {
        match self.verification_method.as_str() {
            VERIFICATION_METHOD_DNS_TXT => "Target domain ownership verification could \
                 not read the DNS TXT record for the domain. Add the DNS TXT record to \
                 the DNS resolver associated with your domain, then verify the target \
                 domain again"
                .to_string(),
            VERIFICATION_METHOD_HTTP_ROUTE => "Target domain ownership verification \
                 could not read the token from the HTTP route path under the domain. Add \
                 the HTTP route path and token directly under your configured domain, \
                 then verify the target domain again"
                .to_string(),
            // `PRIVATE_VPC`, the remaining enum member a stored record can carry, is
            // proven by reaching the domain by IP from inside the VPC.
            _ => "Target domain ownership verification could not reach the domain by IP \
                  from the private VPC associated with it. Make the domain reachable \
                  from the private VPC, then verify the target domain again"
                .to_string(),
        }
    }

    /// Start this record's ownership check at `at`, or report the outcome of the check
    /// already made on it, and answer with the `TargetDomainStatus` the record then
    /// holds.
    ///
    /// A check is started only for a domain that is `PENDING` with none already in
    /// flight: the outcome of a check that has settled stands, so a repeat call reports
    /// it rather than returning the record to `PENDING` and hiding a check that has
    /// already been made, and a call made while one is in flight does not push its due
    /// instant back.
    ///
    /// The started check does not settle within this call — see
    /// [`VERIFICATION_SETTLE_AFTER`] — so the caller sees the `PENDING` it runs in,
    /// exactly as it would while the real service reads the domain.
    pub fn attempt_verification(&mut self, at: SystemTime) -> &str {
        // An outcome the clock already owes is reported by this very call rather than
        // by the next one.
        self.settle_verification(at);
        if self.verification_status == TARGET_DOMAIN_STATUS_PENDING
            && self.verification_due_at.is_none()
        {
            self.verification_due_at = Some(at + VERIFICATION_SETTLE_AFTER);
            self.updated_at = at;
        }
        &self.verification_status
    }

    /// Materialise the outcome of this record's ownership check once the clock has made
    /// it due by `now`; a record with no check in flight, or one whose check is not due
    /// yet, is left exactly as it was.
    ///
    /// The check reads the token this record carries where the record's verification
    /// method says the domain's OWNER publishes it, so a check made from inside this
    /// emulator reads no proof and the domain becomes
    /// [`TARGET_DOMAIN_STATUS_UNREACHABLE`] with no verification time. The record
    /// changed at the instant the outcome was DUE, not at the instant some later call
    /// happened to materialise it, so that is what `updated_at` carries.
    pub fn settle_verification(&mut self, now: SystemTime) {
        let Some(due) = self.verification_due_at else {
            return;
        };
        if now < due {
            return;
        }
        let reason = self.unreachable_status_reason();
        self.verification_due_at = None;
        self.verification_status = TARGET_DOMAIN_STATUS_UNREACHABLE.to_string();
        self.verification_status_reason = Some(reason);
        self.updated_at = due;
    }

    /// This target domain projected onto the [`TargetDomain`] shape the reads return:
    /// the stored record's own fields, never request input.
    ///
    /// `status` carries the same `TargetDomainStatus` as the modelled
    /// `verificationStatus`: the recorded wire reports the domain's status under both
    /// names, and one stored field feeds both, so they cannot disagree.
    pub fn modelled(&self) -> TargetDomain {
        TargetDomain {
            target_domain_id: self.target_domain_id.clone(),
            domain_name: self.domain_name.clone(),
            verification_status: Some(self.verification_status.clone()),
            verification_status_reason: self.verification_status_reason.clone(),
            verification_details: Some(self.verification_details()),
            created_at: Some(timestamp(self.created_at)),
            verified_at: self.verified_at.map(timestamp),
            status: Some(self.verification_status.clone()),
        }
    }
}

/// The identity minted for a new private connection: the VPC Lattice resources the
/// service creates for it, which are the service's to choose rather than the caller's
/// to supply.
#[derive(Debug, Clone)]
pub struct PrivateConnectionIdentity {
    /// The resource gateway id, in VPC Lattice's own `rgw-<17 of [0-9a-z]>` form.
    pub resource_gateway_id: String,
    /// The resource configuration id, in VPC Lattice's own `rcfg-<17 of [0-9a-z]>` form.
    pub resource_configuration_id: String,
}

/// The identity minted for a new target domain: the two values that are the service's
/// to choose rather than the caller's to supply.
#[derive(Debug, Clone)]
pub struct TargetDomainIdentity {
    pub target_domain_id: String,
    pub verification_token: String,
}

// ---------------------------------------------------------------------------
// The form each resource class's identifier carries
// ---------------------------------------------------------------------------

/// The prefix every `agentSpaceId` this service hands out carries.
///
/// RECORDED, not modelled: `AgentSpaceId` carries neither a `@pattern` nor a `@length`,
/// but every agent space id the recording holds — the one a create answered with, and
/// the deliberately unknown id a fixture probes a refusal with — begins with these
/// characters, followed by a body of that one space's own.
pub const AGENT_SPACE_ID_PREFIX: &str = "as-";

/// The prefix every `packId` of this service carries, whoever manages the pack.
///
/// RECORDED, not modelled: `SecurityRequirementPackId` carries no `@pattern` or
/// `@length`, and every pack id the recording holds begins with these characters — the
/// managed catalog's four ([`AWS_MANAGED_PACKS`]) are the ones it holds literally. So one
/// class prefix serves every pack id rather than the catalog having one namespace and a
/// created pack another.
pub const PACK_ID_CLASS_PREFIX: &str = "srp-";

/// The marker a pack id carries between its class prefix and its body: which
/// `ManagementType` the pack is of.
///
/// RECORDED, not modelled: the catalog's ids are `srp-am-<name>` — the class prefix, the
/// AWS-managed marker, then that pack's own body — so the marker is part of the form, and
/// a pack this API creates is `CUSTOMER_MANAGED` ([`MANAGEMENT_TYPE_CUSTOMER_MANAGED`])
/// and carries the customer-managed marker instead.
pub const PACK_ID_AWS_MANAGED_MARKER: &str = "am-";

/// See [`PACK_ID_AWS_MANAGED_MARKER`].
pub const PACK_ID_CUSTOMER_MANAGED_MARKER: &str = "cm-";

/// The prefix every `packId` this API hands out for a pack a caller created carries: the
/// class prefix and the customer-managed marker, ahead of that one pack's own body.
pub const SECURITY_REQUIREMENT_PACK_ID_PREFIX: &str = "srp-cm-";

/// The prefix every `targetDomainId` this service hands out carries.
///
/// RECORDED, not modelled: `TargetDomainId` carries no `@pattern`, and what the recording
/// shows is the FORM the service's resource classes take — a short lowercase class
/// abbreviation, a hyphen, then a body freshly generated for that one resource
/// ([`AGENT_SPACE_ID_PREFIX`], [`SECURITY_REQUIREMENT_PACK_ID_PREFIX`]) — which is what a
/// client telling one of this service's ids from another relies on.
pub const TARGET_DOMAIN_ID_PREFIX: &str = "td-";

/// What the body after one of those prefixes is drawn over, so the uuid body of an
/// identifier cannot collide with the digest of a verification token drawn from the same
/// mint.
const IDENTIFIER_BODY_CONTEXT: &str = "securityagent-resource-identifier:";

/// How many bytes a uuid carries, which is how much of the digest behind an identifier's
/// body is laid out as one.
const UUID_BYTES: usize = 16;

/// The ONE generator every identifier and every verification token this service mints
/// is drawn from.
///
/// Two values go into each minted value, and each answers one half of what the API
/// promises its callers:
///
/// * a counter this mint NEVER reuses, shared by every resource type, so two resources
///   of this account are never handed one identifier — not two agent spaces, and not an
///   agent space and a pack;
/// * a namespace drawn once per mint from entropy the system seeded
///   ([`system_entropy`]), so what one mint hands out is not what another hands out,
///   and a value is not the ordinal of the call that created it.
///
/// So every minted value is opaque, is unique within the account across resource types,
/// and carries nothing from the request that asked for it. What a value looks like is
/// the RESOURCE CLASS's, not this mint's: a class whose identifier form the service
/// hands out is known renders the mint's draw in that form (a prefix the class is read
/// by, then the draw as this resource's own body), which is why the renderings below sit
/// beside each other here rather than in the handlers.
#[derive(Debug)]
pub struct IdentifierMint {
    /// The namespace every value this mint hands out carries, drawn once per mint.
    namespace: u64,
    /// How many values this mint has handed out, which is what makes the next one
    /// unique rather than merely unlikely to repeat.
    minted: u64,
}

impl Default for IdentifierMint {
    /// A mint with a fresh namespace and nothing handed out yet.
    fn default() -> Self {
        Self {
            namespace: system_entropy(),
            minted: 0,
        }
    }
}

impl IdentifierMint {
    /// The next number this mint has not handed out.
    fn next_sequence(&mut self) -> u64 {
        self.minted = self.minted.wrapping_add(1);
        self.minted
    }

    /// A fresh identifier of this mint, with the sequence number behind it for a
    /// resource that also names a second value derived from the same identity.
    ///
    /// 32 lowercase hex characters: the mint's namespace, then this resource's own
    /// number. This is the form for a resource class the service is not known to hand a
    /// prefixed identifier out for: no identifier shape of this service carries a
    /// `@pattern` or a `@length` in the model, so nothing pins a form there, and what is
    /// promised — "unique identifier" — is the uniqueness above. A class whose form the
    /// recording does show draws [`Self::next_uuid`] and renders it behind that class's
    /// prefix instead.
    pub fn next_identifier(&mut self) -> MintedIdentifier {
        let sequence = self.next_sequence();
        MintedIdentifier {
            id: format!("{:016x}{sequence:016x}", self.namespace),
            sequence,
        }
    }

    /// A fresh draw of this mint as the hyphenated lowercase uuid that is the BODY of a
    /// prefixed identifier: the per-resource half of `<class prefix><body>`.
    ///
    /// The 16 bytes the uuid lays out are the SHA-256 of this draw's own never-reused
    /// number, the mint's seeded namespace and a context of its own, so the body is
    /// unique per draw (the number), is not the ordinal of the call that asked for it
    /// (the namespace), carries nothing the request supplied, and cannot coincide with a
    /// verification token drawn from the same mint (the context). The uuid LAYOUT — its
    /// version and variant bits and its `8-4-4-4-12` groups — comes from the shared
    /// renderer, so it is not spelled out again here.
    pub fn next_uuid(&mut self) -> String {
        let sequence = self.next_sequence();
        let mut digest = Sha256::new();
        digest.update(IDENTIFIER_BODY_CONTEXT.as_bytes());
        digest.update(self.namespace.to_be_bytes());
        digest.update(sequence.to_be_bytes());
        let digest = digest.finalize();
        let mut body = [0u8; UUID_BYTES];
        body.copy_from_slice(&digest[..UUID_BYTES]);
        ::service_common::uuid_from_random_bytes(body)
    }

    /// A fresh secret of this mint for the challenge `context`: 64 lowercase hex
    /// characters, the hex-encoded SHA-256 of the context, the mint's namespace, a
    /// number this mint never reuses and the instant it was drawn.
    ///
    /// Unique per call because of the number, opaque because of the namespace's seeded
    /// bits, and carrying nothing the caller sent — which is what a proof-of-control
    /// challenge is: a value this service handed to the owner of ONE resource, so no
    /// two resources are ever told to publish one value. EMULATOR-GRADE: a digest, not
    /// an unforgeable secret.
    pub fn next_secret(&mut self, context: &str) -> String {
        let sequence = self.next_sequence();
        let drawn_at = SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map_or(0, |since_epoch| since_epoch.as_nanos());
        let mut digest = Sha256::new();
        digest.update(context.as_bytes());
        digest.update(self.namespace.to_be_bytes());
        digest.update(sequence.to_be_bytes());
        digest.update(drawn_at.to_be_bytes());
        hex::encode(digest.finalize())
    }

    /// A fresh identifier of this mint in the base-36 suffix form VPC Lattice's own id
    /// patterns declare ([`base36_id`]), for the resources this service creates THERE
    /// rather than in its own namespace.
    ///
    /// That form holds fewer bits than a namespace and a sequence number together, so
    /// the two are not shifted into it — that would keep only the namespace's low bits.
    /// The low [`LATTICE_ID_SEQUENCE_DIGITS`] digits carry the mint's own never-reused
    /// number, so two resources of one mint cannot be handed one id, and the digits
    /// above them carry a digest of the WHOLE namespace, so two mints do not agree on
    /// them.
    pub fn next_lattice_suffix(&mut self) -> String {
        let sequence = self.next_sequence();
        let span = 36u128.pow(LATTICE_ID_SEQUENCE_DIGITS);
        let digest = Sha256::digest(self.namespace.to_be_bytes());
        let namespace = digest
            .iter()
            .take(8)
            .fold(0u128, |value, byte| (value << 8) | u128::from(*byte))
            % 36u128.pow(LATTICE_ID_NAMESPACE_DIGITS);
        base36_id(namespace * span + (u128::from(sequence) % span))
    }
}

/// One identifier [`IdentifierMint::next_identifier`] handed out.
#[derive(Debug, Clone)]
pub struct MintedIdentifier {
    /// The identifier itself, as every response and every store key carries it.
    pub id: String,
    /// The mint's own number behind it, for a resource whose identity carries a second
    /// value derived from the same draw.
    pub sequence: u64,
}

/// Per-service state for com-amazonaws-securityagent-securityagent. A handler reaches it through [`service_state`].
///
/// `Default` is hand-written rather than derived because the account does not start
/// empty: the pack store is SEEDED with the AWS managed catalog ([`AWS_MANAGED_PACKS`]),
/// which a fresh account already holds. The engine builds this state with `Default` the
/// first time a handler asks for it, so that is the one place the seed can live.
#[derive(Debug)]
pub struct ComAmazonawsSecurityagentSecurityagentState {
    /// Every stored application, keyed by its `applicationId`. A `BTreeMap` so
    /// `ListApplications` walks the applications in one stable order and a cursor
    /// taken on one page resumes at the right place on the next — including after a
    /// neighbouring record was deleted.
    pub applications: BTreeMap<String, Application>,
    /// Every stored private connection, keyed by its name — which the API itself makes
    /// the identifier ("A unique name for the private connection within your
    /// account"). A `BTreeMap` for the same reason as `applications`.
    pub private_connections: BTreeMap<String, PrivateConnection>,
    /// Every registered target domain, keyed by its `targetDomainId`. A `BTreeMap` for
    /// the same reason as [`Self::applications`]: one stable order for the reads that
    /// walk the whole set.
    pub target_domains: BTreeMap<String, TargetDomainRecord>,
    /// Every stored agent space, keyed by its `agentSpaceId`. A `BTreeMap` for the
    /// same reason as [`Self::applications`]: one stable order for a list read to walk
    /// and to resume a cursor in.
    pub agent_spaces: BTreeMap<String, AgentSpaceRecord>,
    /// Every stored security requirement pack, keyed by its `packId`. A `BTreeMap` for
    /// the same reason as [`Self::applications`]: one stable order for
    /// `ListSecurityRequirementPacks` to walk and to resume a cursor in.
    pub security_requirement_packs: BTreeMap<String, SecurityRequirementPack>,
    /// Every stored security requirement, keyed by the PAIR `(packId, name)` that
    /// identifies it: a requirement lives in exactly one pack and its name is unique
    /// only within that pack, so two packs can each hold a requirement of one name and
    /// neither read sees the other's. A `BTreeMap` so the pairs of one pack are a
    /// contiguous, name-ordered range — one stable order for a read that walks a pack's
    /// requirements and for a cursor to resume in.
    pub security_requirements: BTreeMap<(String, String), SecurityRequirement>,
    /// Every stored artifact, keyed by its `artifactId`. A `BTreeMap` for the same
    /// reason as [`Self::applications`]: the ids come from the counter below, so one
    /// stable order — the upload order — for a read that walks the artifacts of an
    /// agent space.
    pub artifacts: BTreeMap<String, ArtifactRecord>,
    /// The one generator every identifier and verification token this account's
    /// resources are minted from — whatever their type — so no two of them are ever
    /// handed the same value. See [`IdentifierMint`].
    pub identifiers: IdentifierMint,
}

impl Default for ComAmazonawsSecurityagentSecurityagentState {
    /// The state of a fresh account: every store empty EXCEPT the pack store, which
    /// already holds the AWS managed catalog.
    ///
    /// The seeded packs are stored records like any other, so
    /// `ListSecurityRequirementPacks` and `GetSecurityRequirementPack` project them
    /// through exactly the same path as a created pack, and
    /// `UpdateSecurityRequirementPack` writes their status in place.
    fn default() -> Self {
        Self {
            applications: BTreeMap::new(),
            private_connections: BTreeMap::new(),
            target_domains: BTreeMap::new(),
            agent_spaces: BTreeMap::new(),
            security_requirement_packs: AWS_MANAGED_PACKS
                .iter()
                .map(|entry| {
                    (
                        entry.pack_id.to_string(),
                        SecurityRequirementPack::aws_managed(entry),
                    )
                })
                .collect(),
            security_requirements: BTreeMap::new(),
            artifacts: BTreeMap::new(),
            identifiers: IdentifierMint::default(),
        }
    }
}

impl ComAmazonawsSecurityagentSecurityagentState {
    /// How many of the stored packs a caller created, i.e. how many count against the
    /// account's pack quota. The seeded AWS managed catalog is the service's own and is
    /// not part of what a caller may create, so it is not counted.
    pub fn customer_managed_pack_count(&self) -> usize {
        self.security_requirement_packs
            .values()
            .filter(|pack| pack.management_type == MANAGEMENT_TYPE_CUSTOMER_MANAGED)
            .count()
    }

    /// Mint the identity of a new application.
    ///
    /// The id is a fresh identifier of the account's one mint ([`IdentifierMint`]), so
    /// it is unique across every resource this account holds and `ListApplications`'
    /// ordering is the creation order. The name and the domain are derived from that
    /// same identity, so a later read returns a real stored value rather than a
    /// placeholder, and the domain names the region the request was signed for.
    pub fn mint_application_identity(&mut self, region: &str) -> ApplicationIdentity {
        let minted = self.identifiers.next_identifier();
        ApplicationIdentity {
            application_name: format!("application-{}", minted.sequence),
            domain: format!("{}.securityagent.{region}.on.aws", minted.id),
            application_id: minted.id,
            sequence: minted.sequence,
        }
    }

    /// Mint the VPC Lattice resources a service-managed private connection is created
    /// with: the resource gateway the service stands up in the caller's VPC, and the
    /// resource configuration it creates beside it.
    ///
    /// Both ids are drawn from the account's one mint — once each, so the gateway and
    /// the configuration are two distinct resources — and rendered in the base-36 form
    /// VPC Lattice's own id patterns declare.
    pub fn mint_private_connection_identity(&mut self) -> PrivateConnectionIdentity {
        PrivateConnectionIdentity {
            resource_gateway_id: format!(
                "{RESOURCE_GATEWAY_ID_PREFIX}{}",
                self.identifiers.next_lattice_suffix()
            ),
            resource_configuration_id: format!(
                "{RESOURCE_CONFIGURATION_ID_PREFIX}{}",
                self.identifiers.next_lattice_suffix()
            ),
        }
    }

    /// The VPC Lattice resource configuration `identifier` names in `region`, as the
    /// account holds it (its bare id), or `None` when the account holds none under that
    /// identifier — which a create answers with `ResourceNotFoundException`.
    ///
    /// `SelfManagedInput$resourceConfigurationId` is "The identifier or ARN of the
    /// resource configuration", so both forms resolve: the bare id, and the ARN
    /// `arn:aws:vpc-lattice:<region>:<account>:resourceconfiguration/<id>`
    /// (`com.amazonaws.vpclattice#ResourceConfigurationArn`). Rather than taking an ARN
    /// apart, each id the account holds is rendered as the ARN it has through
    /// [`lattice_arn`] — the one ARN BUILDER this emulator mints resources with — and
    /// the identifier has to equal one of the two forms. So an ARN naming another
    /// partition, vendor, Region, account or resource type simply is not an ARN of any
    /// resource configuration this request can reach, with no second ARN grammar
    /// written down to disagree with the builder's.
    ///
    /// The resource configurations the emulated account holds are the ones it CREATED:
    /// one per service-managed private connection, whose gateway and configuration are
    /// the service's to create and are persisted on the record. VPC Lattice's own
    /// `CreateResourceConfiguration` still answers 501 and keeps no store, so it
    /// contributes none yet; when it is implemented, THIS is the one place that has to
    /// start consulting that store beside these records.
    pub fn resolve_resource_configuration(
        &self,
        region: &str,
        identifier: &str,
    ) -> Option<String> {
        self.private_connections
            .values()
            .filter_map(|connection| connection.resource_configuration_id.as_deref())
            .find(|id| {
                identifier == *id
                    || identifier
                        == lattice_arn(region, KIND_RESOURCE_CONFIGURATION, id)
            })
            .map(str::to_string)
    }

    /// Settle every target domain ownership check the clock has made due by `now`.
    ///
    /// Called by each handler that projects a stored target domain — the batch read,
    /// the agent space reads that expand the domains a space is associated with, and
    /// `VerifyTargetDomain` itself — so a check's outcome surfaces on the first call
    /// after it fell due, whichever call that is, and every one of them reports the
    /// same record. This materialises a change the CLOCK already owed: it is the only
    /// write a reporting operation makes, and the record it leaves is the one a read
    /// made at the due instant would have found.
    pub fn settle_target_domain_verifications(&mut self, now: SystemTime) {
        for record in self.target_domains.values_mut() {
            record.settle_verification(now);
        }
    }

    /// Mint the identity of a new target domain: its id and the ownership-verification
    /// token its owner is told to publish.
    ///
    /// BOTH are fresh draws of the account's one mint ([`IdentifierMint`]), never
    /// derived from each other or from request text. So the id is unique across every
    /// resource this account holds, and the token is this ONE domain's proof of
    /// control: no other domain, and no other verification method, is ever told to
    /// publish it.
    ///
    /// The id is rendered in the target domain class's own form
    /// ([`TARGET_DOMAIN_ID_PREFIX`] and a body of this domain's own), so a client that
    /// routes on, validates or stores it can tell it from an agent space's or a pack's
    /// id. The token is not an identifier and carries no class prefix: it is the
    /// challenge value its owner publishes.
    pub fn mint_target_domain_identity(&mut self) -> TargetDomainIdentity {
        TargetDomainIdentity {
            target_domain_id: format!(
                "{TARGET_DOMAIN_ID_PREFIX}{}",
                self.identifiers.next_uuid()
            ),
            verification_token: self.identifiers.next_secret(VERIFICATION_TOKEN_CONTEXT),
        }
    }

    /// Mint the `agentSpaceId` of a new agent space.
    ///
    /// The agent space class's own form ([`AGENT_SPACE_ID_PREFIX`]) over a fresh draw of
    /// the account's one mint, so the id is unique across every resource this account
    /// holds while a client reading it can tell which class of this service's resources
    /// it names. The body is opaque to the caller — `AgentSpaceId` carries neither a
    /// `@pattern` nor a `@length` in the model and nothing of the request reaches it.
    /// The ARN this id goes into is built from it ([`agent_space_arn`]), so the two
    /// agree and are distinct per space.
    pub fn mint_agent_space_id(&mut self) -> String {
        format!("{AGENT_SPACE_ID_PREFIX}{}", self.identifiers.next_uuid())
    }

    /// Mint the `packId` of a new security requirement pack.
    ///
    /// The pack class's own form for a pack this API creates
    /// ([`SECURITY_REQUIREMENT_PACK_ID_PREFIX`]: the class prefix and the
    /// customer-managed marker) over a fresh draw of the account's one mint, taken while
    /// the store is locked, so two calls — including two concurrent calls carrying the
    /// same `name` — get two distinct ids and neither overwrites the other, and a pack id
    /// can never be an agent space's or a target domain's id. A created pack is therefore
    /// read by an id of the same CLASS as the managed catalog's
    /// ([`AWS_MANAGED_PACKS`]), marked for the management type it has, rather than out of
    /// a second namespace — which is what lets a client handle every pack id uniformly.
    pub fn mint_security_requirement_pack_id(&mut self) -> String {
        format!(
            "{SECURITY_REQUIREMENT_PACK_ID_PREFIX}{}",
            self.identifiers.next_uuid()
        )
    }

    /// The security requirement `name` names IN the pack `pack_id`, or `None` when that
    /// pack holds no requirement of that name.
    ///
    /// A requirement is reachable only through the pack that holds it, so a name that
    /// names a requirement of ANOTHER pack resolves to nothing here — a read of it is
    /// answered exactly as a read of a name nothing created is.
    pub fn security_requirement(&self, pack_id: &str, name: &str) -> Option<&SecurityRequirement> {
        self.security_requirements
            .get(&(pack_id.to_string(), name.to_string()))
    }

    /// The same record, mutably: the one record a write addressed to that pack and name
    /// changes, so an update and a read can never disagree about which record a name
    /// names.
    pub fn security_requirement_mut(
        &mut self,
        pack_id: &str,
        name: &str,
    ) -> Option<&mut SecurityRequirement> {
        self.security_requirements
            .get_mut(&(pack_id.to_string(), name.to_string()))
    }

    /// Every security requirement the pack `pack_id` holds, in name order.
    ///
    /// The keys of one pack are a contiguous range of the store, so this walks that
    /// range rather than the whole store.
    pub fn security_requirements_in<'a>(
        &'a self,
        pack_id: &'a str,
    ) -> impl Iterator<Item = &'a SecurityRequirement> {
        self.security_requirements_from(pack_id, None)
    }

    /// The same walk, starting at the requirement `resume_from` names rather than at the
    /// pack's first one: what one page of `ListSecurityRequirements` resumes at.
    ///
    /// One range serves both, so a paged read and a whole-pack read cannot disagree
    /// about a pack's contents or their order.
    pub fn security_requirements_from<'a>(
        &'a self,
        pack_id: &'a str,
        resume_from: Option<&str>,
    ) -> impl Iterator<Item = &'a SecurityRequirement> {
        let start = (
            pack_id.to_string(),
            resume_from.unwrap_or_default().to_string(),
        );
        self.security_requirements
            .range(start..)
            .take_while(move |((pack, _), _)| pack == pack_id)
            .map(|(_, requirement)| requirement)
    }

    /// How many security requirements the pack `pack_id` holds, i.e. how many count
    /// against that pack's requirement quota ([`MAX_SECURITY_REQUIREMENTS_PER_PACK`]).
    pub fn security_requirement_count(&self, pack_id: &str) -> usize {
        self.security_requirements_in(pack_id).count()
    }

    /// Store `requirement` under the pair that identifies it, so every later read of
    /// that pack and name resolves THIS record.
    pub fn insert_security_requirement(&mut self, requirement: SecurityRequirement) {
        self.security_requirements.insert(
            (requirement.pack_id.clone(), requirement.name.clone()),
            requirement,
        );
    }

    /// Drop the security requirement `name` names IN the pack `pack_id`, handing back
    /// the record that was removed, or `None` when that pack held no requirement of that
    /// name.
    ///
    /// The counterpart of [`Self::insert_security_requirement`], keyed by the same pair,
    /// so a removal and a read can never disagree about which record a name names: after
    /// this returns `Some`, every later read of that pack and name resolves nothing.
    pub fn remove_security_requirement(
        &mut self,
        pack_id: &str,
        name: &str,
    ) -> Option<SecurityRequirement> {
        self.security_requirements
            .remove(&(pack_id.to_string(), name.to_string()))
    }

    /// Mint the `artifactId` of a newly uploaded artifact.
    ///
    /// A fresh draw of the account's one mint, taken while the store is locked, so two
    /// uploads — including two carrying the same file name and bytes — are two distinct
    /// artifacts and neither overwrites the other. `ArtifactId` carries no `@pattern`
    /// or `@length` in the model, so the form is this emulator's to choose.
    pub fn mint_artifact_id(&mut self) -> String {
        self.identifiers.next_identifier().id
    }

    /// How many artifacts the agent space `agent_space_id` holds, i.e. how many count
    /// against that space's artifact quota ([`MAX_ARTIFACTS_PER_AGENT_SPACE`]).
    pub fn artifact_count(&self, agent_space_id: &str) -> usize {
        self.artifacts
            .values()
            .filter(|artifact| artifact.agent_space_id == agent_space_id)
            .count()
    }

    /// The artifact `artifact_id` names IN the agent space `agent_space_id`, or `None`
    /// when that space holds no such artifact.
    ///
    /// An artifact is reachable only through the space that holds it, so an id that
    /// names an artifact of ANOTHER agent space resolves to nothing here — a read of it
    /// is refused exactly as a read of an id nothing minted is.
    pub fn artifact_in(
        &self,
        agent_space_id: &str,
        artifact_id: &str,
    ) -> Option<&ArtifactRecord> {
        self.artifacts
            .get(artifact_id)
            .filter(|artifact| artifact.agent_space_id == agent_space_id)
    }

    /// Drop the artifact `artifact_id` of the agent space `agent_space_id` from the
    /// store, answering the record that was removed, or `None` when that space holds no
    /// such artifact — in which case NOTHING is removed.
    ///
    /// The pair resolves exactly as [`Self::artifact_in`] resolves it, so a delete that
    /// names an artifact of ANOTHER agent space removes nothing, the same way a read of
    /// it reports nothing.
    pub fn remove_artifact_in(
        &mut self,
        agent_space_id: &str,
        artifact_id: &str,
    ) -> Option<ArtifactRecord> {
        self.artifact_in(agent_space_id, artifact_id)?;
        self.artifacts.remove(artifact_id)
    }
}

/// 64 bits of entropy this process did not choose, telling one [`IdentifierMint`] apart
/// from another.
///
/// `std::collections::hash_map::RandomState::new` "is initialized with random keys"
/// (its own documentation) — the system seeds them — and a fresh one per call carries
/// fresh keys, so what it yields cannot be derived from a request or from an earlier
/// draw. EMULATOR-GRADE, by design: it names a mint and opens a challenge token's
/// digest, it is never a key, a secret or a signature, and the UNIQUENESS of a minted
/// value comes from the mint's own never-reused counter rather than from here. The same
/// derivation stands behind the domain verification tokens in
/// `services/common/src/ses_identities.rs`, so no second entropy idiom is introduced.
fn system_entropy() -> u64 {
    use std::collections::hash_map::RandomState;
    use std::hash::{BuildHasher, Hasher};
    RandomState::new().build_hasher().finish()
}

/// `sequence` as the `<17 of [0-9a-z]>` suffix VPC Lattice's resource-gateway and
/// resource-configuration id patterns declare: base 36, lowercase, zero-padded to the
/// pattern's exact length.
fn base36_id(mut value: u128) -> String {
    let mut digits = [b'0'; LATTICE_ID_SUFFIX_LEN];
    for slot in digits.iter_mut().rev() {
        *slot = BASE36_LOWER[(value % 36) as usize];
        value /= 36;
    }
    String::from_utf8_lossy(&digits).into_owned()
}

/// The ARN a VPC Lattice resource of `kind` and id `id` has in `region`, built with the
/// ONE ARN builder this emulator mints resources with (`engine::emulator_arn`, which
/// carries the emulator's own partition and account). The form matches
/// `com.amazonaws.vpclattice#ResourceConfigurationArn` /
/// `#ResourceGatewayArn`: `arn:aws:vpc-lattice:<region>:<account>:<kind>/<id>`.
pub fn lattice_arn(region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(VPC_LATTICE_ARN_SERVICE, region, &format!("{kind}/{id}"))
}

// ---------------------------------------------------------------------------
// The account's network
// ---------------------------------------------------------------------------

/// How many hex characters an EC2 subnet identifier carries in its long form
/// (`subnet-0bb1c79de3example`), which is the form [`region_subnets`] mints.
const SUBNET_ID_HEX_LEN: usize = 17;

/// How many digest BYTES go into an EC2 VPC identifier's short form (`vpc-1a2b3c4d`):
/// two hex characters each, so four make the 8 the form carries.
const VPC_ID_DIGEST_BYTES: usize = 4;

/// The Availability Zone suffixes a Region's default subnets sit in. A Region has at
/// least three Availability Zones, and a default VPC has one default subnet in each.
const AVAILABILITY_ZONE_SUFFIXES: [&str; 3] = ["a", "b", "c"];

/// A subnet of the account's network in one Region: the identifier a request names, the
/// VPC it belongs to and the Availability Zone it sits in.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SubnetRecord {
    pub subnet_id: String,
    pub vpc_id: String,
    pub availability_zone: String,
}

/// The id of the account's default VPC in `region`.
///
/// AWS gives an account a default VPC in every Region and this emulator models no
/// other, so the id is DERIVED from the Region name — stable, so every read reports the
/// same VPC — in EC2's short `vpc-<8 hex>` form.
pub fn default_vpc_id(region: &str) -> String {
    format!(
        "vpc-{}",
        hex::encode(&Sha256::digest(region.as_bytes())[..VPC_ID_DIGEST_BYTES])
    )
}

/// The subnets that exist in `region`: the default VPC's default subnet in each
/// Availability Zone of that Region.
///
/// THE SAME derivation, constant for constant, as the one
/// `services/service-com-amazonaws-fsx-awssimbaapiservice-v20180301/src/state.rs` and
/// `services/service-com-amazonaws-dax-amazondaxv3/src/state.rs` apply, all three
/// written against the contract `services/common/src/network.rs` states, so no two
/// services answer "does this subnet exist" differently and a client that places an FSx
/// file system and a private connection's resource gateway in "the same" subnet is
/// talking to one account. The derivation itself moves into that shared module once one
/// change may carry every such service crate's manifest; keep them in step until then,
/// and when EC2's subnet operations stop answering 501, [`resolve_subnet`] is the one
/// place here that has to start consulting the stored subnets beside these derived ones.
pub fn region_subnets(region: &str) -> Vec<SubnetRecord> {
    let vpc_id = default_vpc_id(region);
    AVAILABILITY_ZONE_SUFFIXES
        .iter()
        .map(|suffix| {
            let availability_zone = format!("{region}{suffix}");
            let digest = hex::encode(Sha256::digest(availability_zone.as_bytes()));
            SubnetRecord {
                subnet_id: format!("subnet-{}", &digest[..SUBNET_ID_HEX_LEN]),
                vpc_id: vpc_id.clone(),
                availability_zone,
            }
        })
        .collect()
}

/// The subnet `subnet_id` names in `region`, or `None` when no subnet of that Region
/// carries that identifier — which is what lets a request naming it be refused instead
/// of stored.
pub fn resolve_subnet(region: &str, subnet_id: &str) -> Option<SubnetRecord> {
    region_subnets(region)
        .into_iter()
        .find(|subnet| subnet.subnet_id == subnet_id)
}

/// Why `name` is not a domain name this service can register as a target domain, or
/// `None` when it is one.
///
/// `targetDomainName` is a bare `String` in the model — no `@length`, no `@pattern` —
/// so this rule is a CHOSEN refusal of input the model admits: what the operation
/// registers is "a web domain", i.e. a DNS name, and a value that cannot name a domain
/// could never be verified, so it is refused at the door instead of being stored.
///
/// A name is one when, ignoring the optional single trailing dot of the rooted form,
/// it is at most [`MAX_DOMAIN_NAME_LEN`] bytes and made of non-empty dot-separated
/// labels of at most [`MAX_DOMAIN_LABEL_LEN`] bytes drawn from letters, digits and
/// hyphens, none of them starting or ending with a hyphen (RFC 1035 §2.3.1/§2.3.4 with
/// RFC 1123 §2.1, which is also why an underscore label — legal in a DNS zone but not
/// in a hostname — is not a web domain). Both bounds are measured in bytes, which is
/// the unit the RFCs state them in and, for a name this rule accepts, the same as
/// characters. The returned text is the constraint the value missed, for the refusal's
/// `fieldList` entry.
pub fn target_domain_name_violation(name: &str) -> Option<String> {
    if name.is_empty() {
        return Some("must not be empty".to_string());
    }
    // The rooted form `example.com.` names the same domain as `example.com`, so the
    // one trailing dot is not part of the name being checked.
    let name = name.strip_suffix('.').unwrap_or(name);
    if name.len() > MAX_DOMAIN_NAME_LEN {
        return Some(format!(
            "must have length less than or equal to {MAX_DOMAIN_NAME_LEN}"
        ));
    }
    for label in name.split('.') {
        if label.is_empty() {
            return Some("must not carry an empty label".to_string());
        }
        if !label.bytes().all(|b| b.is_ascii_alphanumeric() || b == b'-') {
            return Some(
                "must be a domain name built from letters, digits, hyphens and dots".to_string(),
            );
        }
        if label.len() > MAX_DOMAIN_LABEL_LEN {
            return Some(format!(
                "must carry labels of at most {MAX_DOMAIN_LABEL_LEN} characters"
            ));
        }
        if label.starts_with('-') || label.ends_with('-') {
            return Some("must not carry a label that starts or ends with a hyphen".to_string());
        }
    }
    None
}

/// Why `method` is not a `DomainVerificationMethod` this service can act on, or `None`
/// when it is one of [`VERIFICATION_METHOD_VALUES`].
///
/// The model declares the member as an enum, so a conforming client never sends
/// anything else; checking it here means an unmodelled value is refused rather than
/// stored and projected back as a method with no challenge to publish.
pub fn verification_method_violation(method: &str) -> Option<String> {
    (!VERIFICATION_METHOD_VALUES.contains(&method)).then(|| {
        format!(
            "must satisfy enum value set: [{}]",
            VERIFICATION_METHOD_VALUES.join(", ")
        )
    })
}

/// The grammar a `resourceConfigurationId` has to satisfy: a bare VPC Lattice resource
/// configuration id, or the ARN of one. This is the text a refusal REPORTS;
/// [`resource_configuration_id_violation`] is what enforces it, alternative for
/// alternative and character class for character class.
///
/// RECORDED, not modelled: `ResourceConfigurationId` carries no `@pattern` in this
/// service's model, so nothing refuses a malformed value at the boundary — and the real
/// service refuses one at validation, reporting exactly this pattern. It is the union of
/// VPC Lattice's own two patterns (`com.amazonaws.vpclattice#ResourceConfigurationId`
/// `^rcfg-[0-9a-z]{17}$` and `com.amazonaws.vpclattice#ResourceConfigurationArn`), which
/// is why [`RESOURCE_CONFIGURATION_ID_PREFIX`] and [`LATTICE_ID_SUFFIX_LEN`] — the parts
/// this emulator MINTS ids from — are the same two bounds spelled out here.
///
/// The account segment is `\d{12}`, twelve ASCII digits, which is what an AWS account id
/// is. It is spelled with the `\d` shorthand because a quoted pattern is the service's
/// own pattern SOURCE text: a client repairs its request from the expression it reads
/// out of the refusal, so an equivalent respelling of a character class (`[0-9]{12}`
/// here) would advertise an expression the service does not publish.
pub const RESOURCE_CONFIGURATION_ID_PATTERN: &str = concat!(
    r"((rcfg-[0-9a-z]{17})|",
    r"(arn:[a-z0-9\-]+:vpc-lattice:[a-zA-Z0-9\-]+:\d{12}:",
    r"resourceconfiguration/rcfg-[0-9a-z]{17}))",
);

/// Whether `value` is `prefix` followed by exactly [`LATTICE_ID_SUFFIX_LEN`] characters
/// of `[0-9a-z]` — the bare id form of a VPC Lattice resource, which is also the form
/// [`base36_id`] mints.
fn lattice_id(value: &str, prefix: &str) -> bool {
    value.strip_prefix(prefix).is_some_and(|suffix| {
        suffix.len() == LATTICE_ID_SUFFIX_LEN
            && suffix
                .bytes()
                .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase())
    })
}

/// Whether `value` is the ARN of a VPC Lattice resource configuration:
/// `arn:<partition>:vpc-lattice:<region>:<account>:resourceconfiguration/<id>`, with the
/// partition drawn from `[a-z0-9-]`, the region from `[a-zA-Z0-9-]`, the account twelve
/// ASCII digits, and the resource id in the bare form [`lattice_id`] accepts. Each
/// segment is the corresponding alternative of [`RESOURCE_CONFIGURATION_ID_PATTERN`].
///
/// Written out rather than run through a regex engine: the grammar is two fixed
/// alternatives of six fixed segments, not a language, so there is nothing to parse and
/// no dependency this one check has to pull into the workspace.
fn resource_configuration_arn(value: &str) -> bool {
    let fields: Vec<&str> = value.split(':').collect();
    let [arn, partition, service, region, account, resource] = fields.as_slice() else {
        return false;
    };
    let Some((kind, id)) = resource.split_once('/') else {
        return false;
    };
    *arn == "arn"
        && !partition.is_empty()
        && partition
            .bytes()
            .all(|b| b.is_ascii_digit() || b.is_ascii_lowercase() || b == b'-')
        && *service == VPC_LATTICE_ARN_SERVICE
        && !region.is_empty()
        && region
            .bytes()
            .all(|b| b.is_ascii_alphanumeric() || b == b'-')
        && account.len() == 12
        && account.bytes().all(|b| b.is_ascii_digit())
        && kind == KIND_RESOURCE_CONFIGURATION
        && lattice_id(id, RESOURCE_CONFIGURATION_ID_PREFIX)
}

/// Why `identifier` is not a VPC Lattice resource configuration identifier at all, or
/// `None` when it is one.
///
/// This is a SYNTACTIC check, decided from the value alone: it says nothing about
/// whether the account holds the resource, which is
/// [`ComAmazonawsSecurityagentSecurityagentState::resolve_resource_configuration`]'s
/// question. The returned text is the constraint the value missed, for the refusal's
/// `fieldList` entry.
///
/// The WHOLE value has to be an identifier — a well-formed id inside other text names no
/// resource — which is why neither alternative below accepts a prefix or a suffix around
/// itself, and why the reported pattern is read as anchored.
pub fn resource_configuration_id_violation(identifier: &str) -> Option<String> {
    let matches = lattice_id(identifier, RESOURCE_CONFIGURATION_ID_PREFIX)
        || resource_configuration_arn(identifier);
    (!matches).then(|| {
        format!("must satisfy regular expression pattern: {RESOURCE_CONFIGURATION_ID_PATTERN}")
    })
}

/// The ARN of the IAM Identity Center application this emulator stands up for an
/// application whose `idcInstanceArn` was supplied.
///
/// The instance segment is taken from the supplied instance ARN's own resource id
/// (`arn:aws:sso:::instance/<instance-id>`) so the two ARNs agree; an instance ARN
/// that names no usable id falls back to [`FALLBACK_IDC_INSTANCE_ID`]. The account
/// segment is the emulator's canonical account via `engine::emulator_arn`, and
/// `sso` ARNs carry no region, so the region passed is empty.
pub fn idc_application_arn(idc_instance_arn: &str, sequence: u64) -> String {
    let instance_id = idc_instance_arn
        .rsplit_once('/')
        .map(|(_, segment)| {
            segment
                .chars()
                .filter(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || *c == '-')
                .take(64)
                .collect::<String>()
        })
        .filter(|segment| !segment.is_empty())
        .unwrap_or_else(|| FALLBACK_IDC_INSTANCE_ID.to_string());
    ::engine::emulator_arn(
        "sso",
        "",
        &format!("application/{instance_id}/apl-{sequence:016x}"),
    )
}

/// Encode the record id a page resumes at as an opaque `nextToken` under `prefix`.
///
/// The cursor names the record to resume AT, not an offset, so deleting a record
/// between two pages cannot make the next page skip or repeat one.
fn encode_cursor(prefix: &str, record_id: &str) -> String {
    format!("{prefix}{}", hex::encode(record_id))
}

/// The record id a `nextToken` minted under `prefix` resumes at, or `None` when the
/// token is not one this service minted for that list.
fn decode_cursor(prefix: &str, token: &str) -> Option<String> {
    let body = token.strip_prefix(prefix)?;
    let bytes = hex::decode(body).ok()?;
    let record_id = String::from_utf8(bytes).ok()?;
    (!record_id.is_empty()).then_some(record_id)
}

/// Encode the `applicationId` a `ListApplications` page resumes at as an opaque
/// `nextToken`.
pub fn encode_page_token(application_id: &str) -> String {
    encode_cursor(PAGE_TOKEN_PREFIX, application_id)
}

/// The `applicationId` a `nextToken` resumes at, or `None` when the token is not one
/// this service minted (the caller answers that with a `ValidationException`).
pub fn decode_page_token(token: &str) -> Option<String> {
    decode_cursor(PAGE_TOKEN_PREFIX, token)
}

/// Encode the `packId` a `ListSecurityRequirementPacks` page resumes at as an opaque
/// `nextToken`.
pub fn encode_pack_page_token(pack_id: &str) -> String {
    encode_cursor(PACK_PAGE_TOKEN_PREFIX, pack_id)
}

/// The `packId` a `ListSecurityRequirementPacks` `nextToken` resumes at, or `None`
/// when the token is not one this service minted for that list (the caller answers
/// that with a `ValidationException`).
pub fn decode_pack_page_token(token: &str) -> Option<String> {
    decode_cursor(PACK_PAGE_TOKEN_PREFIX, token)
}

/// Encode the private connection name a `ListPrivateConnections` page resumes at as an
/// opaque `nextToken`, on [`encode_page_token`]'s resume-AT rule.
pub fn encode_connection_page_token(name: &str) -> String {
    encode_cursor(PRIVATE_CONNECTION_PAGE_TOKEN_PREFIX, name)
}

/// The private connection name a `ListPrivateConnections` `nextToken` resumes at, or
/// `None` when the token is not one this listing minted.
pub fn decode_connection_page_token(token: &str) -> Option<String> {
    decode_cursor(PRIVATE_CONNECTION_PAGE_TOKEN_PREFIX, token)
}

/// Encode the `artifactId` a `ListArtifacts` page resumes at as an opaque `nextToken`,
/// on [`encode_page_token`]'s resume-AT rule.
pub fn encode_artifact_page_token(artifact_id: &str) -> String {
    encode_cursor(ARTIFACT_PAGE_TOKEN_PREFIX, artifact_id)
}

/// The `artifactId` a `ListArtifacts` `nextToken` resumes at, or `None` when the token is
/// not one this listing minted.
pub fn decode_artifact_page_token(token: &str) -> Option<String> {
    decode_cursor(ARTIFACT_PAGE_TOKEN_PREFIX, token)
}

/// Encode the pack and the security requirement name a `ListSecurityRequirements` page
/// resumes at as an opaque `nextToken`, on [`encode_page_token`]'s resume-AT rule.
///
/// A requirement name is a resume point only INSIDE one pack — the same name names a
/// different requirement in another pack — so the cursor carries the pack it was minted
/// for and [`decode_requirement_page_token`] refuses it against any other.
pub fn encode_requirement_page_token(pack_id: &str, name: &str) -> String {
    encode_cursor(
        REQUIREMENT_PAGE_TOKEN_PREFIX,
        &format!("{pack_id}{REQUIREMENT_CURSOR_SEPARATOR}{name}"),
    )
}

/// The security requirement name a `ListSecurityRequirements` `nextToken` resumes at
/// inside `pack_id`, or `None` when the token is not one this listing minted FOR THAT
/// PACK.
pub fn decode_requirement_page_token(token: &str, pack_id: &str) -> Option<String> {
    let body = decode_cursor(REQUIREMENT_PAGE_TOKEN_PREFIX, token)?;
    let (minted_for, name) = body.split_once(REQUIREMENT_CURSOR_SEPARATOR)?;
    (minted_for == pack_id && !name.is_empty()).then(|| name.to_string())
}

/// The region this request was signed for, from its SigV4 credential scope;
/// [`DEFAULT_REGION`] when it carries none.
///
/// The scope is caller-controlled text and the value lands inside a derived domain
/// name and inside an ARN, so anything that is not a plausible region label is refused
/// here and the default used instead: a generated domain or ARN can never carry
/// arbitrary request text.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 64
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsSecurityagentSecurityagentState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsSecurityagentSecurityagentState>)
}
