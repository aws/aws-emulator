// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralselling-awspartnercentralselling service.
//!
//! Besides the state struct this module holds the few helpers the Opportunity
//! handlers share — catalog validation, the modelled-error constructors, and the
//! millisecond clock the `DateTime` members are built from. They live here rather
//! than in one operation's file because six operations need them and `src/lib.rs`
//! is generated, so a new module file is not an option.
//!
//! The CustomerAccount parts live in child modules: [`customer_accounts`] holds the
//! stored record and what the CustomerAccount operations share, and
//! [`account_search`] the `Filter`, sort, grouping and paging rules the two reporting
//! ones evaluate.

mod account_search;
mod customer_accounts;

pub use account_search::*;
pub use customer_accounts::*;

use std::collections::{HashMap, VecDeque};

use crate::ops::types::{
    AssigneeContact, Contact, Customer, LifeCycle, Marketing, Project, RelatedEntityIdentifiers,
    ResourceNotFoundException, SoftwareRevenue, Tag, ValidationException, ValidationExceptionError,
};

/// The catalogs this API accepts.
///
/// Source: the `Catalog` member doc, which every Opportunity operation repeats —
/// "This field takes a string value from a predefined list: AWS or Sandbox"
/// (`smithy_query(query="doc",shape="CreateOpportunityRequest",member="Catalog")`).
/// The model itself carries only the coarse `^[a-zA-Z]+$` pattern because the
/// value space is checked at runtime, so this allowlist is the service's check.
pub const CATALOGS: [&str; 2] = ["AWS", "Sandbox"];

/// `ReviewStatus` an opportunity carries right after creation. Source: the
/// `CreateOpportunity` doc — "Creating an opportunity sets Lifecycle.ReviewStatus
/// to Pending Submission".
pub const REVIEW_STATUS_PENDING_SUBMISSION: &str = "Pending Submission";
/// `ReviewStatus` a submitted opportunity carries. Source: the `ReviewStatus` enum
/// ("Submitted": "Submitted for validation, and Amazon Web Services hasn't
/// reviewed it (read-only)").
pub const REVIEW_STATUS_SUBMITTED: &str = "Submitted";

/// The `ReviewStatus` values whose doc marks the opportunity read-only, so an
/// `UpdateOpportunity` against one is refused. Source: the `ReviewStatus` member
/// doc on `LifeCycle` — "Submitted … (read-only)", "In Review … (read-only)",
/// "Rejected: Disqualified (read-only)"; "Pending Submission" and "Approved" are
/// marked editable. The enum spells the middle one `In review`.
pub const READ_ONLY_REVIEW_STATUSES: [&str; 3] = ["Submitted", "In review", "Rejected"];

/// Modelled maximum of `PartnerOpportunityTeamMembersList` (`len ..10`), the
/// shape `OpportunityTeam` targets on both the create input and the get output.
pub const MAX_OPPORTUNITY_TEAM_MEMBERS: usize = 10;

/// Modelled maximum of `NextStepsHistories` (`len ..50`), the shape
/// `LifeCycle.NextStepsHistory` targets.
pub const MAX_NEXT_STEPS_HISTORY: usize = 50;

/// Default `MaxResults` for `ListOpportunities`. Source: the `MaxResults` member
/// doc — "Default: 20".
pub const DEFAULT_LIST_MAX_RESULTS: i32 = 20;

/// The most opportunities this emulator holds at once.
///
/// A bound on how far [`ComAmazonawsPartnercentralsellingAwspartnercentralsellingState::opportunities`]
/// can grow under repeated creates: a create past it is refused with the modelled
/// `ThrottlingException` ("Review the provided quotas and adapt your usage") rather
/// than accepted, so a caller cannot grow this process's memory — or the cost of the
/// sort every later `ListOpportunities` runs — without limit. AWS publishes its own
/// opportunity quota and it is not this number; this is the emulator's own ceiling,
/// chosen well above what a client script needs.
pub const MAX_OPPORTUNITIES: usize = 200;

/// The most `ClientToken` -> `Id` entries this service remembers.
///
/// Tokens are caller-supplied (`^.{1,255}$`), so the ledger is caller-driven growth
/// that holds no resource of its own; past this many entries the oldest is evicted
/// ([`CreateReplays::record`]). An evicted token is indistinguishable from one whose
/// idempotency window has passed: the next create carrying it mints a new
/// opportunity. It is deliberately below [`MAX_OPPORTUNITIES`], so the ledger is
/// bounded by its own rule and not merely by the store's ceiling.
pub const MAX_CREATE_REPLAYS: usize = 128;

/// One stored Opportunity: everything a later read of it must be able to answer.
///
/// Timestamps are kept as epoch milliseconds, the precision the `date-time`
/// (RFC 3339) wire format a client reads them back at preserves, so a value a
/// caller echoes into `UpdateOpportunity.LastModifiedDate` compares equal to the
/// stored one.
#[derive(Debug, Clone)]
pub struct StoredOpportunity {
    pub id: String,
    pub arn: String,
    pub catalog: String,
    pub partner_opportunity_identifier: Option<String>,
    pub customer: Option<Customer>,
    pub project: Option<Project>,
    pub life_cycle: Option<LifeCycle>,
    pub opportunity_type: Option<String>,
    pub origin: Option<String>,
    pub marketing: Option<Marketing>,
    pub software_revenue: Option<SoftwareRevenue>,
    pub primary_needs_from_aws: Option<Vec<String>>,
    pub national_security: Option<String>,
    pub opportunity_team: Option<Vec<Contact>>,
    pub assignee: Option<AssigneeContact>,
    pub involvement_type: Option<String>,
    pub visibility: Option<String>,
    pub related_entity_identifiers: RelatedEntityIdentifiers,
    pub tags: Vec<Tag>,
    pub created_date_ms: i64,
    pub last_modified_date_ms: i64,
}

impl StoredOpportunity {
    /// The opportunity's `ReviewStatus`, or `None` when it carries no lifecycle.
    pub fn review_status(&self) -> Option<&str> {
        self.life_cycle
            .as_ref()
            .and_then(|life_cycle| life_cycle.review_status.as_deref())
    }

    /// Move `LastModifiedDate` to now, never backwards and never to the value it
    /// already held: the member doubles as `UpdateOpportunity`'s concurrency
    /// token, so two writes inside one millisecond must still be distinguishable.
    pub fn touch(&mut self) {
        self.last_modified_date_ms = (self.last_modified_date_ms + 1).max(now_ms());
    }
}

/// What a `CreateOpportunity` `ClientToken` resolves to on replay.
///
/// The fingerprint is what makes the token's two documented outcomes separable:
/// the same token with the same body returns the same `Id`, while the same token
/// with a different body is a conflict. It is a fixed-size digest, so remembering a
/// token costs the same whatever the request body's size.
#[derive(Debug, Clone)]
pub struct CreateReplay {
    pub opportunity_id: String,
    pub request_fingerprint: [u8; 32],
}

/// A bounded (catalog, client token) -> `CreateReplay` ledger.
///
/// Insertion-ordered and capped at [`MAX_CREATE_REPLAYS`]: recording the (cap + 1)-th
/// token forgets the oldest one rather than growing the map.
#[derive(Debug, Default)]
pub struct CreateReplays {
    by_key: HashMap<(String, String), CreateReplay>,
    oldest_first: VecDeque<(String, String)>,
}

impl CreateReplays {
    /// What a previous call recorded under `key`, if it is still remembered.
    pub fn get(&self, key: &(String, String)) -> Option<&CreateReplay> {
        self.by_key.get(key)
    }

    /// Remember `replay` under `key`, evicting the oldest entries to stay within
    /// [`MAX_CREATE_REPLAYS`].
    pub fn record(&mut self, key: (String, String), replay: CreateReplay) {
        if self.by_key.insert(key.clone(), replay).is_none() {
            self.oldest_first.push_back(key);
        }
        while self.oldest_first.len() > MAX_CREATE_REPLAYS {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many tokens are remembered right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether no token is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }
}

/// Per-service state for com-amazonaws-partnercentralselling-awspartnercentralselling. A handler reaches it through [`service_state`].
///
/// Both maps are bounded. `opportunities` is capped at [`MAX_OPPORTUNITIES`] — a
/// create that would exceed the ceiling is refused with the modelled
/// `ThrottlingException` — and the idempotency ledger is capped and evicted on its
/// own terms; see [`CreateReplays`]. Each record's own lists are capped too
/// ([`MAX_OPPORTUNITY_TEAM_MEMBERS`], [`MAX_NEXT_STEPS_HISTORY`]), so neither the
/// number of records nor the size of one grows without limit under repeated calls.
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralsellingAwspartnercentralsellingState {
    /// Every stored opportunity, keyed by (catalog, id): the catalog is part of
    /// the identity, so the same id in another catalog is another resource.
    pub opportunities: HashMap<(String, String), StoredOpportunity>,
    /// Monotonic counter the `^O[0-9]{1,19}$` ids are minted from.
    pub next_opportunity_seq: u64,
    /// `CreateOpportunity` idempotency, keyed by (catalog, client token).
    pub create_replays: CreateReplays,
    /// Every stored customer account, keyed by (catalog, id) for the same reason.
    pub customer_accounts: HashMap<(String, String), StoredCustomerAccount>,
    /// Monotonic counter the `^cust-acc-[0-9a-z]{13}$` ids are minted from.
    pub next_customer_account_seq: u64,
}

impl ComAmazonawsPartnercentralsellingAwspartnercentralsellingState {
    /// Mint the next `^O[0-9]{1,19}$` opportunity id.
    pub fn mint_opportunity_id(&mut self) -> String {
        self.next_opportunity_seq += 1;
        format!("O{}", self.next_opportunity_seq)
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralsellingAwspartnercentralsellingState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralsellingAwspartnercentralsellingState>)
}

/// Wall-clock now, as epoch milliseconds.
pub fn now_ms() -> i64 {
    ::std::time::SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since| since.as_millis() as i64)
        .unwrap_or_default()
}

/// A stored millisecond timestamp as the modelled `DateTime` a response carries.
pub fn date_time(millis: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_millis(millis)
}

/// A `DateTime` a request carried, as epoch milliseconds — the precision the stored
/// timestamps keep, so an echoed value compares equal to its origin.
///
/// Rounded, not truncated: a timestamp that travels as a fractional epoch second
/// comes back as a float whose nanosecond expansion can land just under the
/// millisecond it encodes, and rounding recovers the millisecond the caller was given.
///
/// Saturating throughout: the seconds a caller may send span the whole `i64`, so both
/// the scaling and the rounding term clamp at the bounds rather than overflowing — a
/// timestamp at the end of time is compared as that end, and no request can panic or
/// wrap its way to a negative instant here.
pub fn millis_of(value: &::aws_smithy_types::DateTime) -> i64 {
    let millis = i64::from((value.subsec_nanos() + 500_000) / 1_000_000);
    value.secs().saturating_mul(1_000).saturating_add(millis)
}

/// The region to build ARNs in: the one the caller signed for, defaulting to the
/// emulator's own default when the request carries no SigV4 scope.
pub fn request_region(parts: &::smithy_server_runtime::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// The ARN of an opportunity. The service's `arnNamespace` is `partnercentral`
/// (`smithy_query(query="service_info")`), and the resource path carries the
/// catalog because (catalog, id) is the resource's identity.
pub fn opportunity_arn(region: &str, catalog: &str, id: &str) -> String {
    ::engine::emulator_arn(
        "partnercentral",
        region,
        &format!("catalog/{catalog}/opportunity/{id}"),
    )
}

/// Refuse a `Catalog` outside [`CATALOGS`].
pub fn check_catalog(catalog: &str) -> Result<(), ValidationException> {
    if CATALOGS.contains(&catalog) {
        return Ok(());
    }
    Err(field_validation(
        "REQUEST_VALIDATION_FAILED",
        "Catalog",
        "INVALID_ENUM_VALUE",
        format!(
            "Catalog must be one of [{}], got `{catalog}`.",
            CATALOGS.join(", ")
        ),
    ))
}

/// A modelled `ValidationException` about one member of the request.
pub fn field_validation(
    reason: &str,
    field_name: &str,
    code: &str,
    message: impl Into<String>,
) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: message.clone(),
        reason: reason.to_string(),
        error_list: Some(vec![ValidationExceptionError {
            field_name: Some(field_name.to_string()),
            message,
            code: code.to_string(),
        }]),
    }
}

/// A modelled `ValidationException` for an action the opportunity's current
/// state does not permit.
pub fn invalid_resource_state(field_name: &str, message: impl Into<String>) -> ValidationException {
    field_validation(
        "BUSINESS_VALIDATION_FAILED",
        field_name,
        "INVALID_RESOURCE_STATE",
        message,
    )
}

/// A modelled `ResourceNotFoundException` for an opportunity that is not in the
/// requested catalog.
pub fn opportunity_not_found(catalog: &str, identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Opportunity `{identifier}` was not found in catalog `{catalog}`."
        )),
    }
}
