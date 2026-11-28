// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralchannel-partnercentralchannel service.
//!
//! This module owns everything the handlers in `src/ops/` share: the stored form of a
//! program management account and of a relationship (a relationship is modelled as a
//! CHILD of a program management account, so its ARN hangs off the parent's), the
//! stored form of a channel handshake, the
//! client-token (idempotency) ledger, the opaque page token the paginated lists hand
//! out, and the identifier resolution every operation needs — each identifier member
//! accepts either a bare id (`pma-…` / `rs-…`) or the full ARN.
//!
//! Ids are minted here from a counter held in this store, so every id and ARN the
//! service hands out is also stored here and can be resolved again. Handlers build
//! their own output shapes from what they read back out of here, never from the
//! request that created the record.

use std::collections::{BTreeSet, HashMap, VecDeque};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use sha2::{Digest, Sha256};

use crate::ops::types::{
    AccessDeniedException, ChannelHandshakeSummary, HandshakeDetail, ProgramManagementAccountSummary,
    RelationshipDetail, RelationshipSummary, SupportPlan, Tag, ValidationException,
    ValidationExceptionField,
};

mod channel_contracts;

pub use channel_contracts::*;

/// The ARN namespace this service stamps on every ARN, from the model's
/// `aws.api#service` trait (`arnNamespace: partnercentral`).
const ARN_NAMESPACE: &str = "partnercentral";

/// The region segment to use when a request carries no SigV4 credential scope to
/// read one from (an unsigned request to the emulator).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The revision a freshly created resource carries. `revision` is this service's
/// optimistic-concurrency token ("The current revision number of the
/// relationship"), so the first stored revision is 1 and the first successful
/// update makes it 2.
pub const INITIAL_REVISION: u32 = 1;

/// The `ProgramManagementAccountId` prefix, from the pattern `^pma-[a-z0-9]{13}$`.
pub const PROGRAM_MANAGEMENT_ACCOUNT_ID_PREFIX: &str = "pma-";

/// The `RelationshipId` prefix, from the pattern `^rs-[a-z0-9]{13}$`.
pub const RELATIONSHIP_ID_PREFIX: &str = "rs-";

/// The `ChannelHandshakeId` prefix, from the pattern `^ch-[a-z0-9]{13}$`.
pub const CHANNEL_HANDSHAKE_ID_PREFIX: &str = "ch-";

/// How many `[a-z0-9]` characters follow an id prefix, from both id patterns.
const ID_SUFFIX_LEN: usize = 13;

/// The `ProgramManagementAccountStatus` a freshly created account reports, and the
/// one it settles in. The model's value set is `PENDING | ACTIVE | INACTIVE`, with
/// `PENDING` the non-terminal state a create leaves the account in.
pub const STATUS_PENDING: &str = "PENDING";
pub const STATUS_ACTIVE: &str = "ACTIVE";

/// The `Program` whose accounts the model documents a `type` as required for:
/// `type` is "required for the managed services provider program", so an account
/// created for this program without one is refused.
pub const MANAGED_SERVICES_PROVIDER_PROGRAM: &str = "MANAGED_SERVICES_PROVIDER";

/// The `HandshakeStatus` a freshly created channel handshake reports. The
/// model's value set is `PENDING | ACCEPTED | REJECTED | CANCELED | EXPIRED`,
/// and a handshake is a request to another account, so it starts `PENDING`.
///
/// `PENDING` is NOT a clock-owed state here: a handshake leaves it when the
/// other participant accepts or rejects it, or the sender cancels it — a
/// caller-driven transition, not one the clock owes. `EXPIRED` has no published
/// window, so no expiry is invented.
pub const HANDSHAKE_STATUS_PENDING: &str = "PENDING";

/// How long a newly created program management account stays `PENDING` before the
/// clock owes it `ACTIVE`.
///
/// CHOSEN span: neither the model nor the operation docs give an activation time.
/// The project's emulator contract requires a clock-owed transition to fall due
/// 250–750 ms after the call that starts it, so a caller acting immediately after
/// the create still observes the transient state. 300 ms sits inside that band.
pub const ACTIVATION_SPAN: Duration = Duration::from_millis(300);

/// `ListRelationships`' page size when the request declares none. The model gives
/// `maxResults` `@default: 20` with `@range(min: 1, max: 100)`, so the protocol
/// boundary normally fills this in; it is named here so the handler has a value
/// even for a request that somehow carries none.
pub const DEFAULT_MAX_RESULTS: i32 = 20;

/// How many program management accounts this store holds before
/// `CreateProgramManagementAccount` answers the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// CHOSEN bound: the real per-account quota is in neither the model nor the
/// operation docs, and this store lives in memory, so the count is capped rather
/// than left to grow with every request.
pub const MAX_PROGRAM_MANAGEMENT_ACCOUNTS: usize = 100;

/// How many relationships one program management account holds before
/// `CreateRelationship` answers `ServiceQuotaExceededException`. CHOSEN for the
/// same reason as [`MAX_PROGRAM_MANAGEMENT_ACCOUNTS`].
pub const MAX_RELATIONSHIPS_PER_ACCOUNT: usize = 100;

/// How many channel handshakes this store holds before
/// `CreateChannelHandshake` answers `ServiceQuotaExceededException`. CHOSEN for
/// the same reason as [`MAX_PROGRAM_MANAGEMENT_ACCOUNTS`].
pub const MAX_CHANNEL_HANDSHAKES: usize = 100;

/// The `quotaCode` the modelled `ServiceQuotaExceededException` carries. The model
/// types it as a plain required `String` and documents no value set, so the quota
/// is named after the resource it bounds.
pub const PROGRAM_MANAGEMENT_ACCOUNT_QUOTA_CODE: &str = "ProgramManagementAccountsPerAccount";
pub const RELATIONSHIP_QUOTA_CODE: &str = "RelationshipsPerProgramManagementAccount";
pub const CHANNEL_HANDSHAKE_QUOTA_CODE: &str = "ChannelHandshakesPerAccount";

/// The `resourceType` the modelled errors carry. The model types it as a plain
/// required `String`, so these are the model's own shape names.
pub const PROGRAM_MANAGEMENT_ACCOUNT_RESOURCE_TYPE: &str = "ProgramManagementAccount";
pub const RELATIONSHIP_RESOURCE_TYPE: &str = "Relationship";
pub const CHANNEL_HANDSHAKE_RESOURCE_TYPE: &str = "ChannelHandshake";

/// The `ValidationExceptionReason` a malformed request carries, from the model's
/// value set `BUSINESS_VALIDATION_FAILED | REQUEST_VALIDATION_FAILED`. A request
/// whose VALUES are unusable is `REQUEST_VALIDATION_FAILED`; one the stored state
/// does not allow right now — a resource in the wrong status, a window that has
/// closed — is `BUSINESS_VALIDATION_FAILED`.
pub const REQUEST_VALIDATION_FAILED: &str = "REQUEST_VALIDATION_FAILED";
pub const BUSINESS_VALIDATION_FAILED: &str = "BUSINESS_VALIDATION_FAILED";

/// What this service answers a caller whose account holds no active AWS Partner
/// benefit: the entitlement gate's refusal, carried by the modelled
/// `AccessDeniedException` (HTTP 403, the status its `@httpError` declares).
///
/// The gate is an account-level authorization stage rather than a per-request
/// computation, so the sentence is fixed and interpolates nothing: it names no
/// resource, no principal and no action. It is the sentence the real API answers
/// this class with; the model documents the shape only as "insufficient
/// permissions".
pub const PARTNER_ENTITLEMENT_REQUIRED: &str = "This action requires an active AWS Partner \
     benefit. Register as an AWS Partner if you haven't already, or contact AWS Partner Central \
     Support.";

/// How many client tokens the idempotency ledger remembers. Reached only by
/// distinct caller-supplied tokens; the oldest entry is evicted to make room, so
/// the ledger cannot grow without bound.
pub const MAX_IDEMPOTENCY_ENTRIES: usize = 1024;

/// How long a client token stays replayable. CHOSEN bound: the model gives no
/// lifetime for `ClientToken`, and an entry no retry will ever ask about must not
/// be kept for the life of the process.
pub const IDEMPOTENCY_TTL: Duration = Duration::from_secs(24 * 60 * 60);

/// The ARN of the program management account `id` in `catalog`, in the form the
/// model's `ProgramManagementAccountIdentifier` pattern requires
/// (`arn:aws:partnercentral:<region>:<account>:catalog/<catalog>/program-management-account/<id>`).
/// The account segment is the emulator's canonical placeholder account, built
/// through the engine's accessor rather than a literal.
pub fn program_management_account_arn(region: &str, catalog: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("catalog/{catalog}/program-management-account/{id}"),
    )
}

/// The ARN of relationship `id` under the account whose ARN is `parent_arn`: the
/// `RelationshipIdentifier` pattern makes a relationship ARN the parent's ARN with
/// `/relationship/<id>` appended.
pub fn relationship_arn(parent_arn: &str, id: &str) -> String {
    format!("{parent_arn}/relationship/{id}")
}

/// The ARN of channel handshake `id` in `catalog`. `ChannelHandshakeSummary`'s
/// `arn` is modelled as the unconstrained `Arn` shape (len 1..1011), so the
/// handshake keeps this service's catalog-scoped ARN form, built through the
/// engine's accessor rather than a literal account.
pub fn channel_handshake_arn(region: &str, catalog: &str, id: &str) -> String {
    ::engine::emulator_arn(
        ARN_NAMESPACE,
        region,
        &format!("catalog/{catalog}/channel-handshake/{id}"),
    )
}

/// Now, as the wire timestamp type. The ONE clock the handlers read, so a stored
/// timestamp is always the real store time.
pub fn now() -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(SystemTime::now())
}

/// A stable digest of `value`'s debug rendering, used both as a request
/// fingerprint for the client-token ledger and as the filter fingerprint bound
/// into a page token. The derived `Debug` of a generated input shape renders its
/// members in declaration order, so the same request always digests to the same
/// string and a different one almost never does.
pub fn fingerprint<T: std::fmt::Debug>(value: &T) -> String {
    hex::encode(Sha256::digest(format!("{value:?}").as_bytes()))
}

/// The bare resource id an identifier member names.
///
/// Every identifier member in this service is modelled as either the bare id or
/// the full ARN (`^(arn:…/program-management-account/)?pma-[a-z0-9]{13}$`), so the
/// id is the last `/`-separated segment. `None` when the segment is not an id of
/// `prefix`'s kind — which is how a caller that passed a well-formed ARN of the
/// WRONG kind is told apart from one that passed the right one.
pub fn resolve_identifier<'a>(identifier: &'a str, prefix: &str) -> Option<&'a str> {
    let last = identifier.split('/').next_back()?;
    let suffix = last.strip_prefix(prefix)?;
    (suffix.len() == ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()))
    .then_some(last)
}

/// Which kind of resource an `AssociatedResourceIdentifier` names. The modelled
/// pattern accepts a program management account or a relationship, as a bare id
/// or as the full ARN.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AssociatedResourceKind {
    ProgramManagementAccount,
    Relationship,
}

/// A resolved `AssociatedResourceIdentifier`: the bare id it names and which
/// kind of resource that is.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AssociatedResource {
    pub id: String,
    pub kind: AssociatedResourceKind,
}

/// The resource an `AssociatedResourceIdentifier` names, or `None` when the
/// value is neither a program management account nor a relationship identifier.
pub fn resolve_associated_resource(identifier: &str) -> Option<AssociatedResource> {
    if let Some(id) = resolve_identifier(identifier, PROGRAM_MANAGEMENT_ACCOUNT_ID_PREFIX) {
        return Some(AssociatedResource {
            id: id.to_string(),
            kind: AssociatedResourceKind::ProgramManagementAccount,
        });
    }
    resolve_identifier(identifier, RELATIONSHIP_ID_PREFIX).map(|id| AssociatedResource {
        id: id.to_string(),
        kind: AssociatedResourceKind::Relationship,
    })
}

/// The `code` a `ValidationExceptionField` carries, naming the class of rule the
/// value broke. The model types the member as a plain required `String` and
/// documents no value set, so these are the names this service family's refusals
/// are recorded with: a value whose FORM is wrong (a pattern, an id, an ARN, an
/// opaque token) is `INVALID_STRING_FORMAT`, one outside a model value set is
/// `INVALID_ENUM_VALUE`, and anything else unusable is `INVALID_VALUE`.
pub const INVALID_VALUE: &str = "INVALID_VALUE";
pub const INVALID_STRING_FORMAT: &str = "INVALID_STRING_FORMAT";
pub const INVALID_ENUM_VALUE: &str = "INVALID_ENUM_VALUE";

/// The wrapper sentence a `ValidationException`'s `message` states: how many
/// member violations the refusal reports, and nothing else. The per-member detail
/// — which member, and which rule it broke — is the `fieldList`'s job, so the
/// count is the whole message. Fixed text but for the count, which is taken from
/// the `fieldList` it wraps.
fn validation_error_count_message(violations: usize) -> String {
    let plural = if violations == 1 { "" } else { "s" };
    format!("{violations} validation error{plural} detected.")
}

/// A `ValidationException` (HTTP 400) naming the member that was refused, the
/// class of rule it broke and the rule itself. Built here so every handler
/// refuses a request this service's own rules reject in the same shape: the
/// counted message, the modelled `reason`, and a one-entry `fieldList`.
pub fn field_validation_exception(
    member: &str,
    code: &str,
    clause: impl Into<String>,
) -> ValidationException {
    let field = ValidationExceptionField {
        name: member.to_string(),
        code: code.to_string(),
        message: clause.into(),
    };
    ValidationException {
        message: validation_error_count_message(1),
        reason: REQUEST_VALIDATION_FAILED.to_string(),
        field_list: Some(vec![field]),
    }
}

/// A `ValidationException` (HTTP 400) refusing `member` for a rule that is not
/// about its form or its value set: [`INVALID_VALUE`].
pub fn validation_exception(member: &str, message: impl Into<String>) -> ValidationException {
    field_validation_exception(member, INVALID_VALUE, message)
}

/// A `ValidationException` (HTTP 400) refusing the request because the stored
/// state does not allow it, rather than because a member's value is unusable: the
/// modelled `BUSINESS_VALIDATION_FAILED` reason, and no `fieldList`, because no
/// one member is at fault.
pub fn business_validation_exception(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: BUSINESS_VALIDATION_FAILED.to_string(),
        field_list: None,
    }
}

/// A `ValidationException` (HTTP 400) refusing `member` because the FORM of the
/// value is wrong — it is not the id, ARN, pattern or opaque token the member
/// accepts: [`INVALID_STRING_FORMAT`].
pub fn format_validation_exception(member: &str, message: impl Into<String>) -> ValidationException {
    field_validation_exception(member, INVALID_STRING_FORMAT, message)
}

/// A `ValidationException` (HTTP 400) refusing `member` because `value` is
/// outside the model's own value set. The one wording every handler uses for an
/// enum it re-checks before storing or filtering on it.
pub fn enum_validation_exception(
    member: &str,
    value: &str,
    values: &[&str],
) -> ValidationException {
    field_validation_exception(
        member,
        INVALID_ENUM_VALUE,
        format!(
            "Value '{value}' at '{member}' failed to satisfy constraint: Member must satisfy \
             enum value set: [{}]",
            values.join(", ")
        ),
    )
}

/// A program management account as stored. Every response that reports one is
/// built from this, never from the request that created it.
#[derive(Debug, Clone)]
pub struct ProgramManagementAccount {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub program: String,
    pub display_name: String,
    pub account_id: String,
    /// The modelled `type` member (`ProgramManagementAccountType`), absent for an
    /// account created without one.
    pub account_type: Option<String>,
    pub status: String,
    pub revision: u32,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub start_date: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
    /// When the clock owes this account its `PENDING` -> `ACTIVE` transition.
    /// Stored rather than slept for: [`PartnerCentralChannelState::advance_clock`]
    /// materialises it the next time any handler touches the store.
    pub active_at: SystemTime,
}

impl ProgramManagementAccount {
    /// This account as the `ProgramManagementAccountSummary`
    /// `ListProgramManagementAccounts` returns. Every member the summary shape
    /// declares is reported, each from the stored record, so a list page says
    /// the same thing about an account as the create that minted it.
    pub fn to_summary(&self) -> ProgramManagementAccountSummary {
        ProgramManagementAccountSummary {
            id: Some(self.id.clone()),
            revision: Some(self.revision.to_string()),
            catalog: Some(self.catalog.clone()),
            program: Some(self.program.clone()),
            display_name: Some(self.display_name.clone()),
            account_id: Some(self.account_id.clone()),
            arn: Some(self.arn.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            start_date: Some(self.start_date),
            status: Some(self.status.clone()),
            r#type: self.account_type.clone(),
        }
    }
}

/// A relationship as stored, the child of exactly one program management account.
#[derive(Debug, Clone)]
pub struct Relationship {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub program_management_account_id: String,
    pub association_type: String,
    /// The account associated in this relationship. Optional in the model, so a
    /// relationship created without one stores none and reports none.
    pub associated_account_id: Option<String>,
    pub display_name: String,
    pub resale_account_model: Option<String>,
    pub sector: String,
    /// The `AccountScope` of a managed services relationship, as the request gave
    /// it; absent for a relationship created without one.
    pub account_scope: Option<String>,
    pub requested_support_plan: Option<SupportPlan>,
    pub revision: u32,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub start_date: ::aws_smithy_types::DateTime,
    /// When this relationship ends: the end date of the channel contract in
    /// effect for it, so it is absent until one is.
    pub end_date: Option<::aws_smithy_types::DateTime>,
    /// The channel contract currently in effect for this relationship — the one
    /// whose signature has been accepted. Absent while no contract is in effect.
    pub channel_contract_id: Option<String>,
    pub tags: Vec<Tag>,
}

impl Relationship {
    /// This relationship as the `RelationshipDetail` `GetRelationship` returns.
    pub fn to_detail(&self) -> RelationshipDetail {
        RelationshipDetail {
            arn: Some(self.arn.clone()),
            id: Some(self.id.clone()),
            revision: Some(self.revision.to_string()),
            catalog: Some(self.catalog.clone()),
            association_type: Some(self.association_type.clone()),
            program_management_account_id: Some(self.program_management_account_id.clone()),
            associated_account_id: self.associated_account_id.clone(),
            display_name: Some(self.display_name.clone()),
            resale_account_model: self.resale_account_model.clone(),
            sector: Some(self.sector.clone()),
            account_scope: self.account_scope.clone(),
            channel_contract_id: self.channel_contract_id.clone(),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            start_date: Some(self.start_date),
            end_date: self.end_date,
        }
    }

    /// This relationship as the `RelationshipSummary` `ListRelationships` returns.
    /// The summary shape carries neither `resaleAccountModel` nor the support plan
    /// — the model does not declare them on it — so those are reachable only
    /// through `GetRelationship`.
    pub fn to_summary(&self) -> RelationshipSummary {
        RelationshipSummary {
            arn: Some(self.arn.clone()),
            id: Some(self.id.clone()),
            revision: Some(self.revision.to_string()),
            catalog: Some(self.catalog.clone()),
            association_type: Some(self.association_type.clone()),
            program_management_account_id: Some(self.program_management_account_id.clone()),
            associated_account_id: self.associated_account_id.clone(),
            display_name: Some(self.display_name.clone()),
            sector: Some(self.sector.clone()),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            start_date: Some(self.start_date),
        }
    }
}

/// A channel handshake as stored: a request one account makes to another about
/// an associated resource (a program management account or a relationship).
/// Every response that reports one is built from this, never from the request
/// that created it.
#[derive(Debug, Clone)]
pub struct ChannelHandshake {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub handshake_type: String,
    /// The account that owns the handshake record: the one that created it.
    pub owner_account_id: String,
    pub sender_account_id: String,
    /// The counterparty the handshake is addressed to: the account the associated
    /// resource names. Absent when that resource names none — a relationship
    /// created without an `associatedAccountId` — because there is no stored
    /// account to report and none is invented.
    pub receiver_account_id: Option<String>,
    /// The bare id of the associated resource, from the model's
    /// `AssociatedResourceId` (`^(pma|rs)-[a-z0-9]{13}$`).
    pub associated_resource_id: String,
    /// The type-specific detail, as the modelled `HandshakeDetail` union.
    pub detail: Option<HandshakeDetail>,
    pub status: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub tags: Vec<Tag>,
}

impl ChannelHandshake {
    /// This handshake as the `ChannelHandshakeSummary` `ListChannelHandshakes`
    /// returns.
    ///
    /// `senderDisplayName` is left absent: the model types it as the sender's
    /// `PartnerProfileDisplayName` and this service models no partner profile —
    /// no operation creates or updates one — so there is no stored value to
    /// report and none is invented.
    pub fn to_summary(&self) -> ChannelHandshakeSummary {
        ChannelHandshakeSummary {
            id: Some(self.id.clone()),
            arn: Some(self.arn.clone()),
            catalog: Some(self.catalog.clone()),
            handshake_type: Some(self.handshake_type.clone()),
            owner_account_id: Some(self.owner_account_id.clone()),
            sender_account_id: Some(self.sender_account_id.clone()),
            sender_display_name: None,
            receiver_account_id: self.receiver_account_id.clone(),
            associated_resource_id: Some(self.associated_resource_id.clone()),
            detail: self.detail.clone(),
            created_at: Some(self.created_at),
            updated_at: Some(self.updated_at),
            status: Some(self.status.clone()),
        }
    }

    /// The `servicePeriodType` this handshake's detail carries, for the
    /// type-specific `servicePeriodTypes` filter. `None` for a handshake whose
    /// detail is of another type or carries no service period type.
    pub fn service_period_type(&self) -> Option<&str> {
        match self.detail.as_ref()? {
            HandshakeDetail::startServicePeriodHandshakeDetail(detail) => {
                detail.service_period_type.as_deref()
            }
            HandshakeDetail::revokeServicePeriodHandshakeDetail(detail) => {
                detail.service_period_type.as_deref()
            }
            HandshakeDetail::programManagementAccountHandshakeDetail(_) => None,
        }
    }

    /// The `program` this handshake's detail carries, for the type-specific
    /// `programs` filter.
    pub fn program(&self) -> Option<&str> {
        match self.detail.as_ref()? {
            HandshakeDetail::programManagementAccountHandshakeDetail(detail) => {
                detail.program.as_deref()
            }
            HandshakeDetail::startServicePeriodHandshakeDetail(_)
            | HandshakeDetail::revokeServicePeriodHandshakeDetail(_) => None,
        }
    }
}

/// The channel handshakes of this service, keyed by id and kept in creation
/// order so a paginated list walks a stable sequence.
#[derive(Debug, Default)]
pub struct ChannelHandshakeStore {
    order: Vec<String>,
    by_id: HashMap<String, ChannelHandshake>,
}

impl ChannelHandshakeStore {
    pub fn insert(&mut self, handshake: ChannelHandshake) {
        self.order.push(handshake.id.clone());
        self.by_id.insert(handshake.id.clone(), handshake);
    }

    pub fn get(&self, id: &str) -> Option<&ChannelHandshake> {
        self.by_id.get(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    /// Every stored handshake, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &ChannelHandshake> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }
}

/// The program management accounts of this service, keyed by id and kept in
/// creation order so a paginated list walks a stable sequence. `insert` and
/// `remove` are the only writers, which is what keeps the order and the map
/// consistent.
#[derive(Debug, Default)]
pub struct ProgramManagementAccountStore {
    order: Vec<String>,
    by_id: HashMap<String, ProgramManagementAccount>,
}

impl ProgramManagementAccountStore {
    pub fn insert(&mut self, account: ProgramManagementAccount) {
        self.order.push(account.id.clone());
        self.by_id.insert(account.id.clone(), account);
    }

    pub fn get(&self, id: &str) -> Option<&ProgramManagementAccount> {
        self.by_id.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut ProgramManagementAccount> {
        self.by_id.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) -> Option<ProgramManagementAccount> {
        let account = self.by_id.remove(id)?;
        self.order.retain(|stored| stored != id);
        Some(account)
    }

    /// Every stored account, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &ProgramManagementAccount> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    pub fn len(&self) -> usize {
        self.by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_id.is_empty()
    }

    /// The account whose (catalog, program, accountId) triple is already taken, if
    /// any. That triple is what makes a second create a `ConflictException`: one
    /// AWS account can hold only one program management account per program in a
    /// catalog.
    pub fn find_duplicate(
        &self,
        catalog: &str,
        program: &str,
        account_id: &str,
    ) -> Option<&ProgramManagementAccount> {
        self.iter_in_order().find(|account| {
            account.catalog == catalog
                && account.program == program
                && account.account_id == account_id
        })
    }
}

/// The relationships of this service, keyed by id and kept in creation order.
#[derive(Debug, Default)]
pub struct RelationshipStore {
    order: Vec<String>,
    by_id: HashMap<String, Relationship>,
}

impl RelationshipStore {
    pub fn insert(&mut self, relationship: Relationship) {
        self.order.push(relationship.id.clone());
        self.by_id.insert(relationship.id.clone(), relationship);
    }

    pub fn get(&self, id: &str) -> Option<&Relationship> {
        self.by_id.get(id)
    }

    pub fn get_mut(&mut self, id: &str) -> Option<&mut Relationship> {
        self.by_id.get_mut(id)
    }

    pub fn contains(&self, id: &str) -> bool {
        self.by_id.contains_key(id)
    }

    pub fn remove(&mut self, id: &str) -> Option<Relationship> {
        let relationship = self.by_id.remove(id)?;
        self.order.retain(|stored| stored != id);
        Some(relationship)
    }

    /// Every stored relationship, in creation order.
    pub fn iter_in_order(&self) -> impl Iterator<Item = &Relationship> {
        self.order.iter().filter_map(|id| self.by_id.get(id))
    }

    /// How many relationships hang off one program management account, for the
    /// [`MAX_RELATIONSHIPS_PER_ACCOUNT`] quota.
    pub fn count_for_parent(&self, program_management_account_id: &str) -> usize {
        self.by_id
            .values()
            .filter(|relationship| {
                relationship.program_management_account_id == program_management_account_id
            })
            .count()
    }
}

/// What a client token already in the ledger says about the request carrying it.
#[derive(Debug)]
pub enum TokenOutcome {
    /// The token has not been seen: do the work.
    Fresh,
    /// The same token with the same parameters for the same operation: replay the
    /// earlier result, recomputed from the store.
    Replay(IdempotencyRecord),
    /// The same token and operation with DIFFERENT parameters.
    ParameterMismatch,
    /// The same token already used by a different operation.
    TokenInUse,
}

/// One entry of the client-token ledger. It records WHICH resource the call acted
/// on rather than a serialized body, so a replay is recomputed from the store and
/// can never report a resource that is no longer there.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    pub operation: &'static str,
    pub request_fingerprint: String,
    pub resource_id: Option<String>,
    /// When the token was first used, for the [`IDEMPOTENCY_TTL`] window.
    pub recorded_at: SystemTime,
}

/// Per-service state for com-amazonaws-partnercentralchannel-partnercentralchannel. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralchannelPartnercentralchannelState {
    /// Every program management account, in creation order.
    pub program_management_accounts: ProgramManagementAccountStore,
    /// Every relationship, in creation order.
    pub relationships: RelationshipStore,
    /// Every channel handshake, in creation order.
    pub channel_handshakes: ChannelHandshakeStore,
    /// Every channel contract, in creation order.
    pub channel_contracts: ChannelContractStore,
    /// `clientToken` -> what the call that first used it did. Bounded by
    /// [`MAX_IDEMPOTENCY_ENTRIES`] and [`IDEMPOTENCY_TTL`].
    idempotency: HashMap<String, IdempotencyRecord>,
    /// The tokens in `idempotency`, oldest first, so the ledger can evict.
    idempotency_order: VecDeque<String>,
    /// Monotonic counter mixed into a minted id, so two resources created in the
    /// same nanosecond still get different ids.
    id_sequence: u64,
    /// The AWS accounts that hold an active AWS Partner benefit — the whole input
    /// to the entitlement gate ([`Self::entitlement_refusal`]).
    ///
    /// EMPTY at state init, and deliberately so: holding a partner benefit is a
    /// property of an account that this API cannot establish — it models no
    /// partner registration — so no account is entitled until something outside
    /// the API records one. Seeding an entitled account here would make the gate
    /// unobservable.
    partner_accounts: BTreeSet<String>,
}

impl ComAmazonawsPartnercentralchannelPartnercentralchannelState {
    /// Materialise every transition the clock has already made due: a program
    /// management account past its [`ACTIVATION_SPAN`] leaves `PENDING` for
    /// `ACTIVE`. Called by every handler before it reads or writes, so a
    /// reporting operation reports exactly what was owed at the same instant even
    /// if nobody had called.
    pub fn advance_clock(&mut self) {
        let now = SystemTime::now();
        let due: Vec<String> = self
            .program_management_accounts
            .iter_in_order()
            .filter(|account| account.status == STATUS_PENDING && account.active_at <= now)
            .map(|account| account.id.clone())
            .collect();
        for id in due {
            if let Some(account) = self.program_management_accounts.get_mut(&id) {
                account.status = STATUS_ACTIVE.to_string();
            }
        }
    }

    /// Whether `account` holds an active AWS Partner benefit, which is what the
    /// entitlement gate admits a request on.
    pub fn has_active_partner_benefit(&self, account: &str) -> bool {
        self.partner_accounts.contains(account)
    }

    /// Record an active AWS Partner benefit for `account`, so the gate admits its
    /// requests. `true` when this call is what recorded it.
    ///
    /// No modelled operation reaches this: the API has none that registers a
    /// partner, so the entitlement arrives from outside the API — which in this
    /// emulator means an in-process caller holding the shared engine (a test
    /// standing in for an entitled partner account). Nothing on the wire can
    /// grant itself the entitlement.
    pub fn grant_partner_entitlement(&mut self, account: &str) -> bool {
        self.partner_accounts.insert(account.to_string())
    }

    /// The entitlement gate: `Some(..)` when the one account this emulator
    /// presents holds no active AWS Partner benefit, so the request is refused
    /// with the modelled `AccessDeniedException` rather than served.
    ///
    /// A handler applies this AFTER the modelled shape constraints — so a
    /// malformed request is a `ValidationException` whether the caller is
    /// entitled or not — and BEFORE any resource lookup, so an un-entitled caller
    /// never learns whether a resource exists.
    pub fn entitlement_refusal(&self) -> Option<AccessDeniedException> {
        (!self.has_active_partner_benefit(&caller_account_id())).then(|| AccessDeniedException {
            message: PARTNER_ENTITLEMENT_REQUIRED.to_string(),
            reason: None,
        })
    }

    /// A fresh id of `prefix`'s kind, matching the model's `^<prefix>[a-z0-9]{13}$`:
    /// the leading hex of a digest over this store's counter and the current clock
    /// (hex digits are `[0-9a-f]`, a subset of `[a-z0-9]`), retried in the
    /// astronomically unlikely event of a collision with a live resource.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        loop {
            self.id_sequence += 1;
            let nanos = SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| elapsed.as_nanos())
                .unwrap_or_default();
            let digest = Sha256::digest(format!("{prefix}{}:{nanos}", self.id_sequence).as_bytes());
            let suffix: String = hex::encode(digest).chars().take(ID_SUFFIX_LEN).collect();
            let candidate = format!("{prefix}{suffix}");
            let taken = self.program_management_accounts.contains(&candidate)
                || self.relationships.contains(&candidate)
                || self.channel_handshakes.contains(&candidate)
                || self.channel_contracts.contains(&candidate);
            if !taken {
                return candidate;
            }
        }
    }

    /// What the ledger says about `token` for `operation` with
    /// `request_fingerprint`. An absent token is always [`TokenOutcome::Fresh`]: a
    /// caller that sends no token asks for no replay.
    pub fn classify_token(
        &self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
    ) -> TokenOutcome {
        let Some(token) = token else {
            return TokenOutcome::Fresh;
        };
        match self.idempotency.get(token) {
            None => TokenOutcome::Fresh,
            // Past its replay window: the token is as good as unseen.
            Some(record)
                if record
                    .recorded_at
                    .elapsed()
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL) =>
            {
                TokenOutcome::Fresh
            }
            Some(record) if record.operation != operation => TokenOutcome::TokenInUse,
            Some(record) if record.request_fingerprint != request_fingerprint => {
                TokenOutcome::ParameterMismatch
            }
            Some(record) => TokenOutcome::Replay(record.clone()),
        }
    }

    /// Record that `token` was used by `operation` for `request_fingerprint`. A
    /// no-op when the caller sent no token.
    pub fn record_token(
        &mut self,
        token: Option<&str>,
        operation: &'static str,
        request_fingerprint: &str,
        resource_id: Option<String>,
    ) {
        let Some(token) = token else { return };
        let now = SystemTime::now();
        // Drop what no retry can still ask about, then make room for this entry.
        self.evict_expired_tokens(now);
        if self
            .idempotency
            .insert(
                token.to_string(),
                IdempotencyRecord {
                    operation,
                    request_fingerprint: request_fingerprint.to_string(),
                    resource_id,
                    recorded_at: now,
                },
            )
            .is_none()
        {
            self.idempotency_order.push_back(token.to_string());
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_ENTRIES {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token past its [`IDEMPOTENCY_TTL`] window.
    fn evict_expired_tokens(&mut self, now: SystemTime) {
        while let Some(oldest) = self.idempotency_order.front() {
            let expired = self.idempotency.get(oldest).is_none_or(|record| {
                now.duration_since(record.recorded_at)
                    .is_ok_and(|age| age > IDEMPOTENCY_TTL)
            });
            if !expired {
                return;
            }
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }
}

/// The equality filters `ListProgramManagementAccounts` accepts: each is `None`
/// when the request omitted it (no constraint) and `Some(values)` otherwise.
/// Built by the handler and carried here so the matching rule and the token
/// fingerprint read the same thing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProgramManagementAccountFilters {
    pub catalog: String,
    pub account_ids: Option<Vec<String>>,
    pub display_names: Option<Vec<String>>,
    pub programs: Option<Vec<String>>,
    pub statuses: Option<Vec<String>>,
    /// The modelled `type` member: one `ProgramManagementAccountType` value, not
    /// a list, so it matches only an account stored with that type.
    pub account_type: Option<String>,
}

impl ProgramManagementAccountFilters {
    /// Does `account` satisfy every filter?
    ///
    /// The catalog scopes the list (an account lives in exactly one catalog), and
    /// each remaining filter matches when the account's corresponding field
    /// equals any of that filter's values; several filters are ANDed. An omitted
    /// filter constrains nothing, and a filter on a member the account does not
    /// carry never matches.
    pub fn matches(&self, account: &ProgramManagementAccount) -> bool {
        fn any_equals(values: &Option<Vec<String>>, field: &str) -> bool {
            values
                .as_ref()
                .is_none_or(|values| values.iter().any(|value| value == field))
        }

        account.catalog == self.catalog
            && any_equals(&self.account_ids, &account.account_id)
            && any_equals(&self.display_names, &account.display_name)
            && any_equals(&self.programs, &account.program)
            && any_equals(&self.statuses, &account.status)
            && self
                .account_type
                .as_ref()
                .is_none_or(|wanted| account.account_type.as_ref() == Some(wanted))
    }
}

/// The equality filters `ListRelationships` accepts, already resolved: each is
/// `None` when the request omitted it (no constraint) and `Some(values)`
/// otherwise. Built by the handler and carried here so the matching rule and the
/// token fingerprint read the same thing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct RelationshipFilters {
    pub catalog: String,
    pub associated_account_ids: Option<Vec<String>>,
    pub association_types: Option<Vec<String>>,
    pub display_names: Option<Vec<String>>,
    /// Already resolved from identifiers (bare id or ARN) to bare `pma-…` ids.
    pub program_management_account_ids: Option<Vec<String>>,
}

impl RelationshipFilters {
    /// Does `relationship` satisfy every filter?
    ///
    /// The catalog scopes the list (a relationship lives in exactly one catalog),
    /// and each remaining filter matches when the relationship's corresponding
    /// field equals any of that filter's values; several filters are ANDed. An
    /// omitted filter constrains nothing, and a filter on a member the
    /// relationship does not carry never matches.
    pub fn matches(&self, relationship: &Relationship) -> bool {
        fn any_equals(values: &Option<Vec<String>>, field: &str) -> bool {
            values
                .as_ref()
                .is_none_or(|values| values.iter().any(|value| value == field))
        }
        fn any_equals_opt(values: &Option<Vec<String>>, field: Option<&str>) -> bool {
            match values {
                None => true,
                Some(values) => field.is_some_and(|field| values.iter().any(|v| v == field)),
            }
        }

        relationship.catalog == self.catalog
            && any_equals_opt(
                &self.associated_account_ids,
                relationship.associated_account_id.as_deref(),
            )
            && any_equals(&self.association_types, &relationship.association_type)
            && any_equals(&self.display_names, &relationship.display_name)
            && any_equals(
                &self.program_management_account_ids,
                &relationship.program_management_account_id,
            )
    }
}

/// The filters `ListChannelHandshakes` accepts, already resolved: each is
/// `None` when the request omitted it (no constraint) and `Some(values)`
/// otherwise. Built by the handler and carried here so the matching rule and
/// the token fingerprint read the same thing.
#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ChannelHandshakeFilters {
    pub catalog: String,
    pub handshake_type: String,
    /// `SENDER` or `RECEIVER`: which side of the handshake the calling account
    /// must be on.
    pub participant_type: String,
    /// The account making the call, compared against the side
    /// `participant_type` selects.
    pub caller_account_id: String,
    pub statuses: Option<Vec<String>>,
    /// Already resolved from identifiers (bare id or ARN) to bare
    /// `pma-…`/`rs-…` ids.
    pub associated_resource_ids: Option<Vec<String>>,
    /// From the `startServicePeriodTypeFilters`/`revokeServicePeriodTypeFilters`
    /// variant of `handshakeTypeFilters`.
    pub service_period_types: Option<Vec<String>>,
    /// From the `programManagementAccountTypeFilters` variant of
    /// `handshakeTypeFilters`.
    pub programs: Option<Vec<String>>,
}

impl ChannelHandshakeFilters {
    /// Does `handshake` satisfy every filter?
    ///
    /// The catalog and the handshake type scope the list (both are required
    /// members, and a handshake has exactly one of each) and `participantType`
    /// selects the side the caller must be on; each remaining filter matches
    /// when the handshake's corresponding field equals any of that filter's
    /// values, and several filters are ANDed. An omitted filter constrains
    /// nothing, and a type-specific filter never matches a handshake whose
    /// detail carries no such field.
    pub fn matches(&self, handshake: &ChannelHandshake) -> bool {
        fn any_equals(values: &Option<Vec<String>>, field: &str) -> bool {
            values
                .as_ref()
                .is_none_or(|values| values.iter().any(|value| value == field))
        }
        fn any_equals_opt(values: &Option<Vec<String>>, field: Option<&str>) -> bool {
            match values {
                None => true,
                Some(values) => field.is_some_and(|field| values.iter().any(|v| v == field)),
            }
        }

        let participant_account = match self.participant_type.as_str() {
            PARTICIPANT_TYPE_SENDER => Some(&handshake.sender_account_id),
            PARTICIPANT_TYPE_RECEIVER => handshake.receiver_account_id.as_ref(),
            // Not a modelled `ParticipantType`: no handshake has such a side, so
            // nothing matches. The handler refuses such a value before it reaches
            // here, so this is the belt-and-braces arm rather than the live path.
            _ => return false,
        };

        handshake.catalog == self.catalog
            && handshake.handshake_type == self.handshake_type
            && participant_account == Some(&self.caller_account_id)
            && any_equals(&self.statuses, &handshake.status)
            && any_equals(
                &self.associated_resource_ids,
                &handshake.associated_resource_id,
            )
            && any_equals_opt(&self.service_period_types, handshake.service_period_type())
            && any_equals_opt(&self.programs, handshake.program())
    }
}

/// The `ParticipantType` values, from the model's value set `SENDER | RECEIVER`.
pub const PARTICIPANT_TYPE_SENDER: &str = "SENDER";
pub const PARTICIPANT_TYPE_RECEIVER: &str = "RECEIVER";

/// Where a page of a paginated list resumed from: how far into the filtered
/// sequence the previous page stopped, and a fingerprint of the filters it was
/// taken with, so resuming with different filters is refused instead of being
/// answered with a page of a different result set.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PageCursor {
    pub offset: usize,
    pub filter_fingerprint: String,
}

/// Encode a cursor as the opaque `nextToken` the model describes (`NextToken`,
/// len 1..2048, pattern `^[^\x00-\x1F\x7F\x20]*$`). Hex of
/// `<offset>:<fingerprint>` satisfies that pattern and carries the cursor itself,
/// so a page token needs no server-side entry and a read never writes.
pub fn encode_page_token(cursor: &PageCursor) -> String {
    hex::encode(format!("{}:{}", cursor.offset, cursor.filter_fingerprint))
}

/// Decode a `nextToken` produced by [`encode_page_token`]. `None` when the token
/// was not produced by this service.
pub fn decode_page_token(token: &str) -> Option<PageCursor> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    let (offset, fingerprint) = decoded.split_once(':')?;
    Some(PageCursor {
        offset: offset.parse().ok()?,
        filter_fingerprint: fingerprint.to_string(),
    })
}

/// The region a request was signed for, from its SigV4 credential scope.
///
/// Falls back to [`DEFAULT_REGION`] for an unsigned request, and for a scope whose
/// region segment could not appear in an ARN this service mints (the modelled ARN
/// patterns allow `[a-z0-9-]+` there).
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The account id the request is made by: the emulator's single canonical
/// account (every ARN and account field it emits carries that one account), read
/// through the engine's accessor rather than a literal. It is the sender and the
/// owner of a handshake this emulator creates, and the side
/// `ListChannelHandshakes`' `participantType` is resolved against.
pub fn caller_account_id() -> String {
    ::engine::emulator_account_id().to_string()
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralchannelPartnercentralchannelState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralchannelPartnercentralchannelState>)
}
