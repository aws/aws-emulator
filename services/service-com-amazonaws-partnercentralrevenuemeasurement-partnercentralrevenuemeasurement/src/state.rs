// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement service.
//!
//! The records here are the ONLY place a marketplace revenue share or one of its
//! allocations exists: every implemented handler in `src/ops/` mints into this
//! store or reads out of it, so a create is observable by the next get/list and
//! an update by the next reader.
//!
//! # Revisions are held, not faked
//!
//! `ListMarketplaceRevenueShareAllocations` documents "Optional share revision
//! for historical list", and `Get…`/`Get…Allocation` both take an optional
//! revision that reads the resource as it existed at that revision. A share
//! therefore holds its whole revision history — the instant each revision was
//! written and the products it held then ([`ShareRevision`]) — and every
//! allocation keeps the ordered list of its own versions
//! ([`MarketplaceRevenueShareAllocation::versions`]), each stamped with the share
//! revision at which it became current. A read at revision `r` is then answered
//! by [`MarketplaceRevenueShare::products_at`] and
//! [`MarketplaceRevenueShareAllocation::version_at`] — reconstructed from
//! state, never stored as a response literal.
//!
//! Two things this slice deliberately does NOT emulate, so a reader does not
//! mistake a modelled error for reachable behaviour: authorization (every
//! operation models `AccessDeniedException`, and the emulator holds no principals
//! to evaluate, so no handler constructs it) and throttling
//! (`ThrottlingException`, likewise). The emulator verifies request signatures in
//! the framework, before any handler runs.

use std::collections::{BTreeMap, HashMap};
use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

/// The revenue-attribution half of this store: the records that plane's
/// operations read and write, and the refusals they answer with. Held in its own
/// file because it is a separate resource with separate identifiers, while the
/// two planes share this module's catalogs, ARNs, paging and refusal builders.
pub mod attributions;

use crate::ops::types::{
    ConflictException, InternalServerException, MarketplaceRevenueShareProductDetail,
    MarketplaceRevenueShareProductEntry, MarketplaceRevenueShareProductSummary,
    ResourceNotFoundException, RevenueShareProductAccountDetail,
    RevenueShareProductAccountSummary, ServiceQuotaExceededException, Tag, ValidationException,
    ValidationExceptionField, CATALOG_NAME_VALUES, REVENUE_SHARE_PRODUCT_ACCOUNT_TYPE_VALUES,
};

/// The Region an ARN carries when the request is unsigned, so no SigV4
/// credential scope names one. AWS Partner Central's control plane is reached in
/// US East (N. Virginia).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN service namespace for every resource this service mints
/// (`OPERATIONS.md`: "ARN namespace: `partnercentral`").
pub const ARN_SERVICE: &str = "partnercentral";

/// The ARN resource prefix of a marketplace revenue share, as
/// `MarketplaceRevenueShareArn`'s own pattern spells it
/// (`…:catalog/[a-zA-Z]+/marketplace-revenue-share/mrs-[a-z0-9]{13}`).
pub const MARKETPLACE_REVENUE_SHARE_RESOURCE: &str = "marketplace-revenue-share";

/// The ARN sub-resource prefix of one allocation under a revenue share.
pub const ALLOCATION_RESOURCE: &str = "allocation";

/// The leading characters of every marketplace revenue share id this service
/// mints (`MarketplaceRevenueShareId`: `^mrs-[a-z0-9]{13}$`).
pub const SHARE_ID_PREFIX: &str = "mrs-";

/// The leading characters of every allocation id this service mints
/// (`MarketplaceRevenueShareAllocationId`: `^mrsa-[a-z0-9]{13}$`).
pub const ALLOCATION_ID_PREFIX: &str = "mrsa-";

/// The most marketplace revenue shares this emulator holds at once.
///
/// A bound on how far the store can grow under repeated creates: a create past
/// it is refused with the modelled `ServiceQuotaExceededException` (HTTP 402)
/// rather than accepted, so a caller cannot grow the process's memory without
/// limit. AWS publishes its own quota and it is not this number; this is the
/// emulator's own ceiling, chosen well above what a client script needs.
pub const MAX_REVENUE_SHARES: usize = 100;

/// The most allocations one revenue share holds at once, for the same reason.
/// `CreateMarketplaceRevenueShareAllocation` models the same quota error, so a
/// create past this ceiling is refused with `ServiceQuotaExceededException`.
pub const MAX_ALLOCATIONS_PER_SHARE: usize = 250;

/// The highest revision one revenue share reaches in this emulator.
///
/// Every write to a share or to one of its allocations appends a revision, and
/// the history is what a historical read is answered from — so the history is
/// also the store's growth vector, and a revision can carry a product set of
/// its own. A write past this ceiling is refused with the modelled
/// `ValidationException`. `RevisionToken` admits up to 19 digits, so this is the
/// emulator's own ceiling and not a modelled one.
pub const MAX_SHARE_REVISION: i32 = 1_000;

/// The most items any `@paginated` list in this service returns in one page,
/// including when `MaxResults` is absent.
///
/// `MaxResults` carries `@range(min: 1, max: 100)` on both lists, so a value
/// outside it is refused at the boundary before a handler runs. This is the
/// page a caller that names no `MaxResults` gets, and the ceiling [`page`]
/// clamps to; a caller that wants the rest follows `NextToken`.
pub const MAX_PAGE_SIZE: i32 = 100;

/// `CatalogName` — which catalog a resource lives in (`AWS` | `Sandbox`).
///
/// Part of every resource's key AND of its ARN, so the two catalogs are separate
/// namespaces: the same `ProductId` in `AWS` and in `Sandbox` is two resources.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum CatalogName {
    /// The production catalog.
    Aws,
    /// The sandbox catalog.
    Sandbox,
}

impl CatalogName {
    /// The value this catalog serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Aws => "AWS",
            Self::Sandbox => "Sandbox",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "AWS" => Some(Self::Aws),
            "Sandbox" => Some(Self::Sandbox),
            _ => None,
        }
    }
}

/// `AllocationStatus` — whether an allocation is in force (`ACTIVE`) or has been
/// soft-deleted (`INACTIVE`).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AllocationStatus {
    /// In force; counted by `TotalActiveMarketplaceRevenueShareAllocationCount`.
    Active,
    /// Soft-deleted.
    Inactive,
}

impl AllocationStatus {
    /// The value this status serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Active => "ACTIVE",
            Self::Inactive => "INACTIVE",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ACTIVE" => Some(Self::Active),
            "INACTIVE" => Some(Self::Inactive),
            _ => None,
        }
    }
}

/// `SortOrder` — the direction a list sorts in.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SortOrder {
    /// Smallest sort key first.
    Ascending,
    /// Largest sort key first.
    Descending,
}

impl SortOrder {
    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "ASCENDING" => Some(Self::Ascending),
            "DESCENDING" => Some(Self::Descending),
            _ => None,
        }
    }
}

/// `RevenueShareProductAccountType` — which list of accounts a product entry
/// carries (`DEPLOYMENT` | `ORGANIZATION`).
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum AccountType {
    /// The accounts the product is deployed into.
    Deployment,
    /// The accounts of the organization the product belongs to.
    Organization,
}

impl AccountType {
    /// The value this account type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Deployment => "DEPLOYMENT",
            Self::Organization => "ORGANIZATION",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "DEPLOYMENT" => Some(Self::Deployment),
            "ORGANIZATION" => Some(Self::Organization),
            _ => None,
        }
    }
}

/// One typed account list under a product, as the request supplied it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductAccounts {
    /// `AccountType`.
    pub account_type: AccountType,
    /// `AccountIds`. Held verbatim and never logged:
    /// `RevenueShareProductAccountId` is `@sensitive`.
    pub account_ids: Vec<String>,
}

/// One product in a marketplace revenue share, as this emulator holds it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ProductRecord {
    /// `ProductId` — the product id the entry named, which is what the share
    /// "stores and returns" even when the entry carried a product ARN.
    pub product_id: String,
    /// The typed account lists, at most one per type, in the order the request
    /// listed them.
    ///
    /// Empty when the entry carried none: the model documents an empty list as
    /// the same as omitting the member and omitting it as "have the service
    /// determine the accounts", and the accounts of an AWS Marketplace product
    /// live in the Marketplace catalog, which this emulator does not have — so
    /// no account is invented for it.
    pub accounts: Vec<ProductAccounts>,
}

/// One revision of a marketplace revenue share: when it was written, and the
/// products the share held from then on.
///
/// Every write to the share or to one of its allocations appends one of these,
/// so a historical read (`Revision` on the get,
/// `MarketplaceRevenueShareRevision` on the allocation reads) is answered from
/// the revision that was current then rather than from the newest values.
#[derive(Debug, Clone)]
pub struct ShareRevision {
    /// When this revision was written.
    pub written_at: SystemTime,
    /// The products the share held at this revision. Empty once the share is
    /// closed.
    ///
    /// SHARED with the revisions either side of it: a write to one of the
    /// share's allocations appends a revision without touching the products and
    /// holds the same set rather than a copy of it, so the stored product
    /// history grows only when `UpdateMarketplaceRevenueShare` replaces the set.
    pub products: Arc<Vec<ProductRecord>>,
}

/// One version of an allocation: the mutable half of the record, plus the share
/// revision at which this version became the current one.
#[derive(Debug, Clone)]
pub struct AllocationVersion {
    /// The parent share revision this version was written at. A read at share
    /// revision `r` sees the last version whose `revision` is `<= r`.
    pub revision: i32,
    /// `EffectiveFrom` — the first day of a month.
    pub effective_from: String,
    /// `EffectiveUntil` — the last day of a month.
    pub effective_until: String,
    /// `RevenueSharePercent`, verbatim as the caller wrote it.
    pub revenue_share_percent: String,
    /// `Status`.
    pub status: AllocationStatus,
    /// `LastModifiedDate` for this version.
    pub modified_date: SystemTime,
}

/// A marketplace revenue share allocation, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct MarketplaceRevenueShareAllocation {
    /// `MarketplaceRevenueShareAllocationId` — minted by this service.
    pub allocation_id: String,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// The version this allocation was created with.
    ///
    /// Held SEPARATELY from the later ones so "an allocation always has at
    /// least one version" is a fact of the type rather than an assertion a
    /// reader has to trust: every accessor below is total, and no path through
    /// this module can unwrap an empty history.
    pub created_version: AllocationVersion,
    /// Every version written after the create, oldest first.
    pub later_versions: Vec<AllocationVersion>,
}

impl MarketplaceRevenueShareAllocation {
    /// Every version of this allocation, oldest first.
    pub fn versions(&self) -> impl DoubleEndedIterator<Item = &AllocationVersion> {
        ::std::iter::once(&self.created_version).chain(self.later_versions.iter())
    }

    /// This allocation's current version.
    pub fn current(&self) -> &AllocationVersion {
        self.later_versions.last().unwrap_or(&self.created_version)
    }

    /// The version this allocation was created with.
    pub fn created(&self) -> &AllocationVersion {
        &self.created_version
    }

    /// Record a new current version.
    pub fn push_version(&mut self, version: AllocationVersion) {
        self.later_versions.push(version);
    }

    /// This allocation as it existed at share revision `revision`: the last
    /// version written at or before it, or `None` when the allocation did not
    /// exist yet.
    pub fn version_at(&self, revision: i32) -> Option<&AllocationVersion> {
        self.versions()
            .rev()
            .find(|version| version.revision <= revision)
    }

    /// The share revision this allocation was created at.
    pub fn created_revision(&self) -> i32 {
        self.created_version.revision
    }
}

/// A marketplace revenue share, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct MarketplaceRevenueShare {
    /// `Id` — the `^mrs-[a-z0-9]{13}$` identifier this service minted for it.
    pub share_id: String,
    /// `Arn`.
    pub arn: String,
    /// `Catalog`.
    pub catalog: CatalogName,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// Every revision, oldest first: `revisions[r - 1]` is revision `r` and the
    /// last one is current. Never empty — the create writes revision 1 — so
    /// [`MarketplaceRevenueShare::revision`] is always at least 1.
    pub revisions: Vec<ShareRevision>,
    /// The tags the create request carried.
    pub tags: Vec<Tag>,
    /// Every allocation under this share, keyed by id. A `BTreeMap` over
    /// zero-padded ids, so iteration order is creation order and a list page
    /// resumes deterministically.
    pub allocations: BTreeMap<String, MarketplaceRevenueShareAllocation>,
}

impl MarketplaceRevenueShare {
    /// The current revision; `1` right after the create.
    pub fn revision(&self) -> i32 {
        i32::try_from(self.revisions.len()).unwrap_or(i32::MAX)
    }

    /// The revision `revision` names, or `None` for one this share never
    /// reached.
    pub fn revision_at(&self, revision: i32) -> Option<&ShareRevision> {
        usize::try_from(revision)
            .ok()
            .and_then(|index| index.checked_sub(1))
            .and_then(|index| self.revisions.get(index))
    }

    /// `LastModifiedDate` — when the current revision was written.
    pub fn last_modified_date(&self) -> SystemTime {
        self.revisions
            .last()
            .map(|revision| revision.written_at)
            .unwrap_or(self.created_date)
    }

    /// When `revision` was written, falling back to the creation instant for a
    /// revision this share never reached.
    pub fn modified_at(&self, revision: i32) -> SystemTime {
        self.revision_at(revision)
            .map(|revision| revision.written_at)
            .unwrap_or(self.created_date)
    }

    /// The products the share held at `revision`, and none at a revision it
    /// never reached.
    pub fn products_at(&self, revision: i32) -> &[ProductRecord] {
        self.revision_at(revision)
            .map(|revision| revision.products.as_slice())
            .unwrap_or_default()
    }

    /// The products the share holds now.
    pub fn products(&self) -> &[ProductRecord] {
        self.products_at(self.revision())
    }

    /// Whether the share is closed: its current revision holds no product.
    /// Closing is one-way, so a share that reaches this stays here.
    pub fn is_closed(&self) -> bool {
        self.products().is_empty()
    }

    /// Record a new revision written at `now`, holding `products`, and answer
    /// its number.
    pub fn push_revision(&mut self, now: SystemTime, products: Vec<ProductRecord>) -> i32 {
        self.revisions.push(ShareRevision {
            written_at: now,
            products: Arc::new(products),
        });
        self.revision()
    }

    /// Record a new revision that leaves the products as they are — what a
    /// write to one of this share's allocations does to the share itself. The
    /// new revision SHARES the product set rather than copying it.
    pub fn bump_revision(&mut self, now: SystemTime) -> i32 {
        let products = self
            .revisions
            .last()
            .map(|revision| Arc::clone(&revision.products))
            .unwrap_or_default();
        self.revisions.push(ShareRevision {
            written_at: now,
            products,
        });
        self.revision()
    }

    /// Every allocation that existed at `revision`, paired with the version that
    /// was current then, in allocation-id (creation) order.
    pub fn allocations_at(
        &self,
        revision: i32,
    ) -> impl Iterator<Item = (&MarketplaceRevenueShareAllocation, &AllocationVersion)> {
        self.allocations
            .values()
            .filter_map(move |allocation| Some((allocation, allocation.version_at(revision)?)))
    }

    /// `TotalMarketplaceRevenueShareAllocationCount` and
    /// `TotalActiveMarketplaceRevenueShareAllocationCount` at `revision`, both
    /// counted from the stored allocations rather than held as a field.
    pub fn allocation_counts_at(&self, revision: i32) -> (i32, i32) {
        let mut total: i32 = 0;
        let mut active: i32 = 0;
        for (_, version) in self.allocations_at(revision) {
            total = total.saturating_add(1);
            if version.status == AllocationStatus::Active {
                active = active.saturating_add(1);
            }
        }
        (total, active)
    }

    /// `EffectiveFrom` and `EffectiveUntil` as the share reports them at
    /// `revision`: "derived from the earliest ACTIVE allocation start date" and
    /// from "the latest ACTIVE allocation end date", and both absent when no
    /// ACTIVE allocation remains.
    pub fn effective_window_at(&self, revision: i32) -> (Option<String>, Option<String>) {
        let mut from: Option<&str> = None;
        let mut until: Option<&str> = None;
        for (_, version) in self.allocations_at(revision) {
            if version.status != AllocationStatus::Active {
                continue;
            }
            // `YYYY-MM-DD` orders chronologically as a string.
            from = Some(match from {
                Some(held) if held <= version.effective_from.as_str() => held,
                _ => version.effective_from.as_str(),
            });
            until = Some(match until {
                Some(held) if held >= version.effective_until.as_str() => held,
                _ => version.effective_until.as_str(),
            });
        }
        (from.map(str::to_string), until.map(str::to_string))
    }

    /// Whether this record is the one `CreateMarketplaceRevenueShare` would mint
    /// for these parameters — the comparison an idempotent replay makes before
    /// answering with it, so a `ClientToken` reused with DIFFERENT parameters
    /// conflicts instead of quietly answering with the first call's share.
    ///
    /// The products compared are the ones the CREATE wrote (revision 1), not
    /// the ones the share holds now: a later `UpdateMarketplaceRevenueShare`
    /// does not turn a faithful replay of the create into a conflict.
    pub fn matches_create(
        &self,
        catalog: CatalogName,
        products: &[ProductRecord],
        tags: Option<&Vec<Tag>>,
    ) -> bool {
        self.catalog == catalog
            && self.products_at(1) == products
            && tag_pairs(Some(&self.tags)) == tag_pairs(tags)
    }
}

/// A tag list as a comparable value, and empty when the member is absent.
pub fn tag_pairs(tags: Option<&Vec<Tag>>) -> Vec<(String, String)> {
    let mut pairs: Vec<(String, String)> = tags
        .map(|tags| {
            tags.iter()
                .map(|tag| (tag.key.clone(), tag.value.clone()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort();
    pairs
}

/// Per-service state for com-amazonaws-partnercentralrevenuemeasurement-partnercentralrevenuemeasurement. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState {
    /// Every revenue share, keyed by `(Catalog, Id)` — a `BTreeMap`, so every
    /// list page is in a stable, resumable order and the two catalogs are
    /// contiguous ranges.
    pub marketplace_revenue_shares: BTreeMap<(CatalogName, String), MarketplaceRevenueShare>,
    /// `ClientToken` -> the share key it created, so an `@idempotent` create
    /// replayed with the same token answers with the share it already minted.
    pub share_idempotency: HashMap<String, (CatalogName, String)>,
    /// `ClientToken` -> `(Catalog, share id, the share revision the update
    /// produced)`, so an `@idempotent` `UpdateMarketplaceRevenueShare` replayed
    /// with the same token answers with the revision it wrote instead of
    /// failing the optimistic concurrency check against a revision it has
    /// itself already moved.
    pub share_update_idempotency: HashMap<String, (CatalogName, String, i32)>,
    /// `ClientToken` -> `(Catalog, share id, allocation id)`, for the same
    /// reason. Kept separate from `share_idempotency` because the two creates are
    /// different operations: one token reused across both must not make a
    /// `CreateMarketplaceRevenueShareAllocation` answer with a share.
    pub allocation_idempotency: HashMap<String, (CatalogName, String, String)>,
    /// `ClientToken` -> `(Catalog, share id, allocation id, share revision the
    /// update produced)`, so an `@idempotent` update replayed with the same token
    /// answers with the version it wrote instead of failing the optimistic
    /// concurrency check against a revision it has itself already moved.
    pub allocation_update_idempotency: HashMap<String, (CatalogName, String, String, i32)>,
    /// Every revenue attribution, keyed by `(Catalog, Id)` — a `BTreeMap`, for
    /// the same reasons as the shares above: stable, resumable list pages and
    /// two catalogs that are separate namespaces.
    pub revenue_attributions: BTreeMap<(CatalogName, String), attributions::RevenueAttribution>,
    /// `ClientToken` -> the attribution key it created, so an `@idempotent`
    /// `CreateRevenueAttribution` replayed with the same token answers with the
    /// attribution it already minted. Kept separate from the share maps because
    /// the creates are different operations: one token reused across both must
    /// not make a create of one answer with the other.
    pub attribution_idempotency: HashMap<String, (CatalogName, String)>,
    /// `ClientToken` -> `(Catalog, attribution id, the revision the update
    /// produced)`, so an `@idempotent` `UpdateRevenueAttribution` replayed with
    /// the same token answers with the revision it wrote instead of failing the
    /// optimistic concurrency check against a revision it has itself already
    /// moved.
    pub attribution_update_idempotency: HashMap<String, (CatalogName, String, i32)>,
    /// Source of the sequence number behind every minted share and allocation
    /// id. Monotonic, so no two resources in one store ever share an id.
    pub next_sequence: u64,
}

impl ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState {
    /// The next sequence number: unique within this store for the life of the
    /// process.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The share `(catalog, share_id)` names, if the store holds it.
    pub fn share(&self, catalog: CatalogName, share_id: &str) -> Option<&MarketplaceRevenueShare> {
        self.marketplace_revenue_shares
            .get(&(catalog, share_id.to_string()))
    }

    /// The share `(catalog, share_id)` names, for mutation.
    pub fn share_mut(
        &mut self,
        catalog: CatalogName,
        share_id: &str,
    ) -> Option<&mut MarketplaceRevenueShare> {
        self.marketplace_revenue_shares
            .get_mut(&(catalog, share_id.to_string()))
    }

    /// The revenue attribution `(catalog, attribution_id)` names, if the store
    /// holds it.
    pub fn attribution(
        &self,
        catalog: CatalogName,
        attribution_id: &str,
    ) -> Option<&attributions::RevenueAttribution> {
        self.revenue_attributions
            .get(&(catalog, attribution_id.to_string()))
    }

    /// The revenue attribution `(catalog, attribution_id)` names, for mutation.
    pub fn attribution_mut(
        &mut self,
        catalog: CatalogName,
        attribution_id: &str,
    ) -> Option<&mut attributions::RevenueAttribution> {
        self.revenue_attributions
            .get_mut(&(catalog, attribution_id.to_string()))
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsPartnercentralrevenuemeasurementPartnercentralrevenuemeasurementState>)
}

/// The Region an ARN this request mints belongs in: the one the request's SigV4
/// credential scope names, else [`DEFAULT_REGION`].
///
/// A scope region that is not an ARN region segment (`[a-z0-9-]+`, from
/// `ResourceARN`'s own pattern) is not used, so a crafted `Authorization` header
/// can never produce a stored `Arn` that violates that pattern.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .chars()
                    .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '-')
        })
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The tracing target every audit record this service emits carries.
pub const AUDIT_TARGET: &str = "emulator::audit";

/// Emit the structured audit record a state-changing operation owes: the
/// operation, the resource it changed, how the call ended, the Region it was
/// signed for and the access key id the caller presented. The subscriber stamps
/// the time.
///
/// Only identifiers are recorded. The signature, the secret, the request body
/// and every other header stay out of it, so a record can never be the thing
/// that leaks a credential.
pub fn audit(parts: &::http::request::Parts, operation: &str, resource: &str, outcome: &str) {
    ::tracing::info!(
        target: AUDIT_TARGET,
        operation,
        resource,
        outcome,
        region = %request_region(parts),
        access_key_id = %::engine::extract_akid(&parts.headers),
        "PartnerCentral Revenue Measurement state change"
    );
}

/// The ARN of a marketplace revenue share, through the one ARN builder the
/// framework vends (so the account segment is the canonical emulator account).
pub fn share_arn(region: &str, catalog: CatalogName, share_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "catalog/{}/{MARKETPLACE_REVENUE_SHARE_RESOURCE}/{share_id}",
            catalog.as_wire()
        ),
    )
}

/// The ARN of one allocation: its parent share's ARN plus the allocation
/// sub-resource.
///
/// No output shape in this service carries an allocation ARN, so this names the
/// allocation in the AUDIT record a state-changing operation emits — one string
/// that says which allocation under which share in which catalog changed.
pub fn allocation_arn(
    region: &str,
    catalog: CatalogName,
    share_id: &str,
    allocation_id: &str,
) -> String {
    format!(
        "{}/{ALLOCATION_RESOURCE}/{allocation_id}",
        share_arn(region, catalog, share_id)
    )
}

/// A marketplace revenue share id for sequence number `sequence`, matching
/// `MarketplaceRevenueShareId`'s `^mrs-[a-z0-9]{13}$`. Zero-padded to exactly
/// 13 digits, so id order is creation order and a list page resumes
/// deterministically.
pub fn share_id(sequence: u64) -> String {
    format!("{SHARE_ID_PREFIX}{:013}", sequence % 10_000_000_000_000)
}

/// An allocation id for sequence number `sequence`, matching
/// `MarketplaceRevenueShareAllocationId`'s `^mrsa-[a-z0-9]{13}$`. Zero-padded
/// to exactly 13 digits, for the same reason.
///
/// The modulo keeps the id exactly 13 characters however far the sequence runs;
/// two ids could only collide past 10^13 writes, which
/// [`MAX_REVENUE_SHARES`] times [`MAX_ALLOCATIONS_PER_SHARE`] puts far out of
/// reach of any store this emulator holds.
pub fn allocation_id(sequence: u64) -> String {
    format!("{ALLOCATION_ID_PREFIX}{:013}", sequence % 10_000_000_000_000)
}

/// The share id a `MarketplaceRevenueShareIdentifier` names: the identifier
/// itself in short form (`mrs-…`), or the resource segment of the ARN form.
///
/// The member's `@pattern` admits exactly those two forms and is enforced at
/// the boundary, so this only has to pick the id out. The ARN form's Region and
/// catalog segments are NOT read back: `Catalog` is its own `@required` member
/// and the account is this emulator's, so the id is what identifies the share.
pub fn parse_share_identifier(value: &str) -> Option<String> {
    let candidate = match value.rsplit_once('/') {
        Some((_, last)) => last,
        None => value,
    };
    candidate
        .starts_with(SHARE_ID_PREFIX)
        .then(|| candidate.to_string())
}

/// The share id a `MarketplaceRevenueShareIdentifier` names, or the modelled
/// 404 an identifier that names no share in this catalog earns. A value neither
/// form fits names nothing either, so it is the same 404 rather than a second
/// kind of refusal.
pub fn resolve_share_identifier(
    catalog: CatalogName,
    identifier: &str,
) -> Result<String, ResourceNotFoundException> {
    parse_share_identifier(identifier).ok_or_else(|| share_not_found(catalog, identifier))
}

// A request that carries no `ClientToken` gets NO idempotency entry: an absent
// `@idempotencyToken` member means "no idempotency key", so this service mints
// no substitute for it. A server-minted token recorded in the same map a caller
// is matched against would make a value the client never sent replayable by a
// client that guessed it.

// `ProductName` is NOT held: the display name of an AWS Marketplace product
// lives in the Marketplace catalog, which this emulator does not have, and the
// member is optional on every output that carries it. Every handler therefore
// leaves it absent rather than answering with generated prose that would read as
// looked up. `ProductCode` IS answered, for the reason [`product_code`] states.

/// `MarketplaceProductId`'s own length bounds, which are what the share
/// ANSWERS a product id within (`len 3..256`).
const PRODUCT_ID_LENGTH: ::std::ops::RangeInclusive<usize> = 3..=256;

/// `ProductCode`'s own maximum length (`len 1..64`).
const PRODUCT_CODE_MAX_LENGTH: usize = 64;

/// Whether `value` is a `MarketplaceProductId`:
/// `^[a-zA-Z0-9][.a-zA-Z0-9/-]{1,254}[a-zA-Z0-9]$`, `len 3..256`.
fn is_product_id(value: &str) -> bool {
    PRODUCT_ID_LENGTH.contains(&value.chars().count())
        && value.starts_with(|c: char| c.is_ascii_alphanumeric())
        && value.ends_with(|c: char| c.is_ascii_alphanumeric())
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || matches!(c, '.' | '-' | '/'))
}

/// The product id segment of a product identifier: the value itself, or the
/// last segment of a product ARN — the model documents both forms wherever a
/// product is named ("product IDs or product ARNs").
pub fn product_id_segment(value: &str) -> &str {
    if value.starts_with("arn:") {
        value.rsplit(['/', ':']).next().unwrap_or(value)
    } else {
        value
    }
}

/// The product id a `MarketplaceRevenueShareProductEntry` names: the entry's
/// own value, or the last segment of a product ARN — the model documents the
/// member as "the AWS Marketplace product ID or product ARN" and that "the
/// share stores and returns the product ID".
///
/// The entry's own `@pattern` (`^[\w\-:/.]+$`) admits characters the product id
/// this service answers with does not (`MarketplaceProductId`), so a value that
/// names no product id is refused here rather than stored and answered as one.
pub fn resolve_product_id(member: &str, value: &str) -> Result<String, ValidationException> {
    let candidate = product_id_segment(value);
    if is_product_id(candidate) {
        return Ok(candidate.to_string());
    }
    Err(field_validation(
        "REQUEST_VALIDATION_FAILED",
        "INVALID_STRING_FORMAT",
        member,
        format!("{value:?} names no AWS Marketplace product id"),
    ))
}

/// The AWS Marketplace product code of `product_id`: its alphanumeric
/// characters, which is the one token this emulator has that `ProductCode`'s
/// `^[A-Za-z0-9]+$` admits.
///
/// Why this member is answered while `ProductName` is not. `ProductCode` is an
/// opaque alphanumeric token, and the emulator has exactly one such token that
/// is unique per product, stable across every read, derived from a value the
/// caller itself supplied, and never regenerated per request: the product id's
/// own alphanumerics. So the code is that token, and a `ProductCodes` filter is
/// a real filter over state. `ProductName`, by contrast, is human-readable
/// PROSE: there is no token to derive it from, only text to invent, so that
/// member is left absent (see the note above).
///
/// The divergence this leaves is stated plainly: on real AWS the product code is
/// a different string from the product id, assigned by AWS Marketplace, and this
/// emulator holds no Marketplace catalog to read it out of.
pub fn product_code(product_id: &str) -> Option<String> {
    let code: String = product_id
        .chars()
        .filter(char::is_ascii_alphanumeric)
        .take(PRODUCT_CODE_MAX_LENGTH)
        .collect();
    (!code.is_empty()).then_some(code)
}

/// The products a `Products` member asks the share to hold, or the modelled 400
/// the request earns.
///
/// `Products` is "the complete desired set of products", so one product may
/// appear once; each product's `Accounts` carries "at most one entry per type",
/// and "each entry must carry at least one account" with "an empty list \[\] the
/// same as omitting the field". Every refusal names the member path it is
/// about, so a caller can fix the request from the message alone.
pub fn resolve_products(
    entries: &[MarketplaceRevenueShareProductEntry],
) -> Result<Vec<ProductRecord>, ValidationException> {
    let mut products: Vec<ProductRecord> = Vec::with_capacity(entries.len());
    for (index, entry) in entries.iter().enumerate() {
        let member = format!("Products[{index}].ProductId");
        let product_id = resolve_product_id(&member, &entry.product_id)?;
        if products
            .iter()
            .any(|held| held.product_id == product_id)
        {
            return Err(field_validation(
                "REQUEST_VALIDATION_FAILED",
                "DUPLICATE_VALUE",
                &member,
                format!("product {product_id} is listed more than once"),
            ));
        }
        let mut accounts: Vec<ProductAccounts> = Vec::new();
        for (slot, supplied) in entry.accounts.iter().flatten().enumerate() {
            let member = format!("Products[{index}].Accounts[{slot}].AccountType");
            let Some(account_type) = AccountType::from_wire(&supplied.account_type) else {
                return Err(invalid_enum(
                    &member,
                    REVENUE_SHARE_PRODUCT_ACCOUNT_TYPE_VALUES,
                ));
            };
            if accounts.iter().any(|held| held.account_type == account_type) {
                return Err(field_validation(
                    "REQUEST_VALIDATION_FAILED",
                    "DUPLICATE_VALUE",
                    &member,
                    format!(
                        "{} is listed more than once for product {product_id}",
                        account_type.as_wire()
                    ),
                ));
            }
            if supplied.account_ids.is_empty() {
                return Err(invalid_value(
                    &format!("Products[{index}].Accounts[{slot}].AccountIds"),
                    "must carry at least one account",
                ));
            }
            accounts.push(ProductAccounts {
                account_type,
                account_ids: supplied.account_ids.clone(),
            });
        }
        products.push(ProductRecord {
            product_id,
            accounts,
        });
    }
    Ok(products)
}

/// The most accounts one marketplace revenue share's product set names, summed
/// over every product in it.
///
/// `Products` is `len 0..25`, each product's `Accounts` is `len 0..2` and each
/// `AccountIds` is `len 1..1000`, and all three are refused at the boundary —
/// so this is the product of those modelled maxima and no request the boundary
/// admits reaches it. It is here so that the two operations which STORE a
/// product set check, at the write itself, how much the store is about to grow
/// by: one explicit ceiling in front of the allocation rather than three
/// separate lengths a reader has to multiply. A set past it is refused with the
/// modelled `ServiceQuotaExceededException`.
pub const MAX_ACCOUNTS_PER_SHARE: usize = 50_000;

/// How many accounts `products` names in total — what a write compares against
/// [`MAX_ACCOUNTS_PER_SHARE`] before storing the set.
pub fn account_total(products: &[ProductRecord]) -> usize {
    products
        .iter()
        .map(|product| {
            product
                .accounts
                .iter()
                .map(|accounts| accounts.account_ids.len())
                .sum::<usize>()
        })
        .sum()
}

/// `Products` as `MarketplaceRevenueShareProductSummary` carries it: the stored
/// products with "per-type account counts but no account IDs".
pub fn product_summaries(products: &[ProductRecord]) -> Vec<MarketplaceRevenueShareProductSummary> {
    products
        .iter()
        .map(|product| MarketplaceRevenueShareProductSummary {
            product_id: product.product_id.clone(),
            product_code: product_code(&product.product_id),
            // Absent: see the note on `ProductName` above.
            product_name: None,
            accounts: account_summaries(product),
        })
        .collect()
}

/// `Products` as `MarketplaceRevenueShareProductDetail` carries it: the stored
/// products with "its count and full list of IDs" per type.
pub fn product_details(products: &[ProductRecord]) -> Vec<MarketplaceRevenueShareProductDetail> {
    products
        .iter()
        .map(|product| MarketplaceRevenueShareProductDetail {
            product_id: product.product_id.clone(),
            product_code: product_code(&product.product_id),
            // Absent: see the note on `ProductName` above.
            product_name: None,
            accounts: (!product.accounts.is_empty()).then(|| {
                product
                    .accounts
                    .iter()
                    .map(|accounts| RevenueShareProductAccountDetail {
                        account_type: accounts.account_type.as_wire().to_string(),
                        count: account_count(accounts),
                        account_ids: accounts.account_ids.clone(),
                    })
                    .collect()
            }),
        })
        .collect()
}

/// The per-type counts of one product's accounts, and no member at all for a
/// product whose entry carried no account list: an absent `Accounts` says the
/// service holds no account list for the product, which a list of zero counts
/// would not.
fn account_summaries(product: &ProductRecord) -> Option<Vec<RevenueShareProductAccountSummary>> {
    (!product.accounts.is_empty()).then(|| {
        product
            .accounts
            .iter()
            .map(|accounts| RevenueShareProductAccountSummary {
                account_type: accounts.account_type.as_wire().to_string(),
                count: account_count(accounts),
            })
            .collect()
    })
}

/// `Count` for one typed account list, counted from the stored ids rather than
/// held as a field.
fn account_count(accounts: &ProductAccounts) -> i32 {
    i32::try_from(accounts.account_ids.len()).unwrap_or(i32::MAX)
}

/// `Revision` / `MarketplaceRevenueShareRevision` as the wire carries it: a
/// `RevisionToken` matching `^[1-9][0-9]*$`.
pub fn revision_token(revision: i32) -> String {
    revision.to_string()
}

/// Parse a `RevisionToken`. The `@pattern` is checked at the boundary, so this
/// only has to refuse a value too large for the counter.
pub fn parse_revision_token(value: &str) -> Option<i32> {
    value.parse::<i32>().ok().filter(|revision| *revision >= 1)
}

// ---------------------------------------------------------------------------
// Calendar questions
//
// `AllocationEffectiveDateString` is `^\d{4}-\d{2}-\d{2}$`, which the boundary
// validator enforces — but the pattern admits `2024-13-45`, and the model
// documents that `EffectiveFrom` "must be the first day of a month" and
// `EffectiveUntil` "must be the last day of a month". Those are calendar
// questions, and a date-time type answers them: this module does no calendar
// arithmetic of its own. The type is `aws_smithy_types::DateTime`, the one this
// crate already carries every modelled `Timestamp` in (see [`wire_timestamp`])
// — so one date type across the service and no dependency added for it. Dates
// in `YYYY-MM-DD` also compare chronologically as strings, which is what every
// date filter and the "on or after today" rule below rely on.
// ---------------------------------------------------------------------------

/// The RFC 3339 instant an `AllocationEffectiveDateString` names: that day at
/// midnight UTC. It is the protocol's own timestamp format, which is what makes
/// the parse below the date type's rather than this module's.
const DATE_TIME_FORMAT: ::aws_smithy_types::date_time::Format =
    ::aws_smithy_types::date_time::Format::DateTime;

/// One day, in seconds.
const ONE_DAY: i64 = 86_400;

/// Parse an `AllocationEffectiveDateString`, or `None` when it names no day
/// this service can answer a calendar question about — including `2024-02-30`,
/// `2023-02-29` and `2024-13-01`, which the `@pattern` admits and the date type
/// refuses, and a year outside the range RFC 3339 represents (`0000-01-01`
/// parses to an instant the formatter will not render).
///
/// The render is part of the parse ON PURPOSE: the calendar questions below are
/// answered from the RENDERED date, so a value the formatter cannot render is
/// refused here, where the caller earns the modelled `ValidationException`,
/// rather than deeper down where only a panic would be left.
pub fn parse_date(value: &str) -> Option<::aws_smithy_types::DateTime> {
    // The member is a DATE, so it is read as that day at midnight UTC. The
    // length check is what keeps a longer string from reaching the parser
    // through the concatenation; the calendar itself is the parser's.
    if value.len() != 10 {
        return None;
    }
    let date =
        ::aws_smithy_types::DateTime::from_str(&format!("{value}T00:00:00Z"), DATE_TIME_FORMAT)
            .ok()?;
    format_date(date)?;
    Some(date)
}

/// `instant`'s UTC date as `YYYY-MM-DD`: the date type renders the instant and
/// this takes the date part — or `None` when the instant has no RFC 3339
/// rendering at all, which is every instant outside years `0001..=9999`.
///
/// Fallible rather than asserted: the instants this is asked about are derived
/// from request members (`EffectiveFrom`, `EffectiveUntil`), whose `@pattern`
/// admits a year the formatter refuses, so an unrenderable instant is a request
/// to refuse and never a reason to unwind a handler.
pub fn format_date(instant: ::aws_smithy_types::DateTime) -> Option<String> {
    let rendered = instant.fmt(DATE_TIME_FORMAT).ok()?;
    Some(rendered.chars().take(10).collect())
}

/// Whether `date` is the first day of its month. A date with no rendering is
/// not the first day of anything — [`parse_date`] has already refused it.
pub fn is_first_day_of_month(date: ::aws_smithy_types::DateTime) -> bool {
    format_date(date).is_some_and(|rendered| rendered.ends_with("-01"))
}

/// Whether `date` is the last day of its month: one day later falls in another
/// month. Month lengths and the leap rule are the date type's — this only adds
/// a day's worth of seconds and compares the rendered `YYYY-MM`.
pub fn is_last_day_of_month(date: ::aws_smithy_types::DateTime) -> bool {
    let Some(this) = format_date(date) else {
        return false;
    };
    let Some(next) = date
        .secs()
        .checked_add(ONE_DAY)
        .map(::aws_smithy_types::DateTime::from_secs)
    else {
        return false;
    };
    match format_date(next) {
        Some(next) => month_of(&next) != month_of(&this),
        // `date` renders and the day after it does not, so `date` is the last
        // day RFC 3339 represents (`9999-12-31`) — and the last day of its
        // month, since the range ends on a month boundary. Answered here
        // instead of unwinding: the value came from a request member.
        None => true,
    }
}

/// The `YYYY-MM` of a `YYYY-MM-DD` date string.
fn month_of(date: &str) -> &str {
    date.get(0..7).unwrap_or(date)
}

/// `instant`'s UTC date as `YYYY-MM-DD`, so it compares with an
/// `AllocationEffectiveDateString` directly — or `None` when the instant lies
/// outside the years RFC 3339 renders (see [`format_date`]). The one caller
/// asks about the emulator's own clock, so it answers the modelled
/// [`clock_unavailable`] 500 rather than guessing a date.
pub fn utc_date_string(instant: SystemTime) -> Option<String> {
    format_date(::aws_smithy_types::DateTime::from(instant))
}

/// `RevenueSharePercent` in hundredths of a percent, or `None` when the string
/// is not one. The `@pattern` (`^\d{1,3}(\.\d{1,2})?$`) is enforced at the
/// boundary; this is what lets the handler refuse a percentage above 100.
pub fn percent_hundredths(value: &str) -> Option<u32> {
    let (whole, fraction) = match value.split_once('.') {
        Some((whole, fraction)) => (whole, fraction),
        None => (value, ""),
    };
    if whole.is_empty() || whole.len() > 3 || !whole.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    if fraction.len() > 2 || !fraction.bytes().all(|b| b.is_ascii_digit()) {
        return None;
    }
    let whole: u32 = whole.parse().ok()?;
    let fraction: u32 = match fraction.len() {
        0 => 0,
        1 => fraction.parse::<u32>().ok()? * 10,
        _ => fraction.parse().ok()?,
    };
    Some(whole * 100 + fraction)
}

/// One hundred percent, in the units [`percent_hundredths`] answers in: the
/// largest share of revenue there is.
pub const MAX_PERCENT_HUNDREDTHS: u32 = 10_000;

// ---------------------------------------------------------------------------
// Modelled failures
// ---------------------------------------------------------------------------

/// The modelled 500 for a request that could not reach this service's store.
pub fn state_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the PartnerCentral Revenue Measurement state store is unavailable".to_string(),
    }
}

/// The modelled 500 for an emulator clock that names no date the calendar
/// rules in this module can compare a request's dates against.
pub fn clock_unavailable() -> InternalServerException {
    InternalServerException {
        message: "the service clock is outside the range of dates this service compares against"
            .to_string(),
    }
}

/// The modelled 404 for an identifier that names nothing in the store.
pub fn not_found(message: impl Into<String>) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: message.into(),
        reason: "RESOURCE_NOT_FOUND".to_string(),
    }
}

/// The modelled 404 for a revenue share this catalog does not hold. The
/// message names the IDENTIFIER the request sent, which is what the caller can
/// act on, in either of the two forms the member admits.
pub fn share_not_found(catalog: CatalogName, identifier: &str) -> ResourceNotFoundException {
    not_found(format!(
        "no marketplace revenue share {identifier} in catalog {}",
        catalog.as_wire()
    ))
}

/// The modelled 404 for an allocation this share does not hold.
pub fn allocation_not_found(share_id: &str, allocation_id: &str) -> ResourceNotFoundException {
    not_found(format!(
        "no marketplace revenue share allocation {allocation_id} under marketplace revenue share \
         {share_id}"
    ))
}

/// The modelled 404 for a share revision the store never reached.
///
/// `requested` is the caller's token text, not a parsed number: `RevisionToken`
/// admits up to 19 digits, which no `i32` holds, so re-rendering a parse would
/// report a revision the caller never sent.
pub fn revision_not_found(
    member: &str,
    share_id: &str,
    requested: &str,
    latest: i32,
) -> ResourceNotFoundException {
    not_found(format!(
        "{member} {requested} does not exist for marketplace revenue share {share_id}; \
         its latest revision is {latest}"
    ))
}

/// The modelled 409 for a request that conflicts with the stored resource,
/// carrying one of `ConflictExceptionReason`'s values.
pub fn conflict(reason: &str, message: impl Into<String>) -> ConflictException {
    ConflictException {
        message: message.into(),
        reason: reason.to_string(),
    }
}

/// The modelled 409 for a `ClientToken` replayed with parameters that do not
/// match the resource it already wrote.
///
/// The message names the TOKEN's reuse and not the resource behind it: that
/// resource is the one the FIRST request wrote, which this request has not been
/// shown to be allowed to see, so its ARN is not disclosed here.
pub fn client_token_conflict() -> ConflictException {
    conflict(
        "CONFLICT_CLIENT_TOKEN",
        "the ClientToken of this request was already used for a request with different \
         parameters",
    )
}

/// The modelled 409 for the optimistic concurrency check a revision member
/// carries: the caller's revision is not the share's current one, so the share
/// moved under it. `member` is the one the request sent it in — `Revision` on
/// the share update, `MarketplaceRevenueShareRevision` on an allocation write.
pub fn revision_mismatch(
    member: &str,
    share_id: &str,
    supplied: &str,
    current: i32,
) -> ConflictException {
    conflict(
        "REVISION_MISMATCH",
        format!(
            "{member} {supplied} is not the current revision of marketplace revenue share \
             {share_id}, which is {current}"
        ),
    )
}

/// The modelled 400 for a member this service refuses, naming the member and the
/// rule it broke — the text is built from the request, so a caller can fix the
/// request from it alone.
pub fn field_validation(
    reason: &str,
    code: &str,
    member: &str,
    message: impl Into<String>,
) -> ValidationException {
    let message = message.into();
    ValidationException {
        message: format!("{member}: {message}"),
        reason: reason.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: member.to_string(),
            message,
            code: code.to_string(),
        }]),
    }
}

/// The modelled 400 for a member whose value the request could not carry
/// correctly (`REQUEST_VALIDATION_FAILED`).
pub fn invalid_value(member: &str, message: impl Into<String>) -> ValidationException {
    field_validation(
        "REQUEST_VALIDATION_FAILED",
        "INVALID_VALUE",
        member,
        message,
    )
}

/// The modelled 400 for a request that is wrong as a WHOLE rather than in one
/// member — a combination of members that cannot hold together. It carries no
/// `fieldList`, because naming one member of the set would point a caller at a
/// member that is not at fault.
pub fn request_validation(message: impl Into<String>) -> ValidationException {
    ValidationException {
        message: message.into(),
        reason: "REQUEST_VALIDATION_FAILED".to_string(),
        field_list: None,
    }
}

/// The modelled 400 for an enum member carrying a value the model does not
/// declare.
pub fn invalid_enum(member: &str, allowed: &[&str]) -> ValidationException {
    field_validation(
        "REQUEST_VALIDATION_FAILED",
        "INVALID_ENUM_VALUE",
        member,
        format!("must be one of {}", allowed.join(", ")),
    )
}

/// The modelled 400 for a `@required` enum member the request left empty. The
/// deserializer materializes an absent enum member as the empty string, so this
/// is the refusal the handler owes it.
pub fn required_member(member: &str) -> ValidationException {
    field_validation(
        "REQUEST_VALIDATION_FAILED",
        "REQUIRED_FIELD_MISSING",
        member,
        "must be present",
    )
}

/// The modelled 400 for a request that is well-formed but asks for something the
/// resource's own rules forbid (`BUSINESS_VALIDATION_FAILED`).
pub fn business_validation(member: &str, message: impl Into<String>) -> ValidationException {
    field_validation(
        "BUSINESS_VALIDATION_FAILED",
        "ACTION_NOT_PERMITTED",
        member,
        message,
    )
}

/// The modelled 402 for a create this store has no room for, naming the ceiling
/// it would pass.
///
/// `ServiceQuotaExceededExceptionReason` declares only
/// `ATTRIBUTION_LIMIT_EXCEEDED` and `TAG_LIMIT_EXCEEDED`; the former is the only
/// one that names a resource-count limit at all, so it is the reason a resource
/// ceiling is reported with even though the resource here is a revenue share
/// rather than a revenue attribution. The wire message states the ceiling and
/// the resource and nothing else.
pub fn quota_exceeded(resource: &str, limit: usize) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!("this emulator holds at most {limit} {resource} resources"),
        reason: "ATTRIBUTION_LIMIT_EXCEEDED".to_string(),
    }
}

// ---------------------------------------------------------------------------
// Reading an allocation out, and paging
// ---------------------------------------------------------------------------

/// One allocation as every allocation-shaped response carries it. The four
/// operations that answer with an allocation declare four distinct output shapes
/// with identical members, so they share this one view built from state and each
/// maps it into its own shape.
#[derive(Debug, Clone)]
pub struct AllocationView {
    /// `MarketplaceRevenueShareAllocationId`.
    pub allocation_id: String,
    /// `MarketplaceRevenueShareIdentifier` — the parent share's `mrs-…` id,
    /// which is the short form every allocation output declares.
    pub share_id: String,
    /// The parent share's ARN. No allocation output carries it; it is what the
    /// audit record names the changed resource with.
    pub share_arn: String,
    /// `EffectiveFrom`.
    pub effective_from: String,
    /// `EffectiveUntil`.
    pub effective_until: String,
    /// `RevenueSharePercent`.
    pub revenue_share_percent: String,
    /// `Status`.
    pub status: String,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// `LastModifiedDate`.
    pub last_modified_date: SystemTime,
    /// `MarketplaceRevenueShareRevision` — the parent share revision this view
    /// was read at, which is the revision this version became current at.
    pub marketplace_revenue_share_revision: String,
    /// `MarketplaceRevenueShareLatestRevision` — the parent share's LATEST
    /// revision, which is not the revision the allocation was read at.
    pub marketplace_revenue_share_latest_revision: String,
}

/// The view of `allocation` at `version`, under `share`.
pub fn allocation_view(
    share: &MarketplaceRevenueShare,
    allocation: &MarketplaceRevenueShareAllocation,
    version: &AllocationVersion,
) -> AllocationView {
    AllocationView {
        allocation_id: allocation.allocation_id.clone(),
        share_id: share.share_id.clone(),
        share_arn: share.arn.clone(),
        effective_from: version.effective_from.clone(),
        effective_until: version.effective_until.clone(),
        revenue_share_percent: version.revenue_share_percent.clone(),
        status: version.status.as_wire().to_string(),
        created_date: allocation.created_date,
        last_modified_date: version.modified_date,
        marketplace_revenue_share_revision: revision_token(version.revision),
        marketplace_revenue_share_latest_revision: revision_token(share.revision()),
    }
}

/// A timestamp as the wire carries it.
pub fn wire_timestamp(instant: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(instant)
}

/// `instant` as whole seconds and nanoseconds since the Unix epoch, which is a
/// total order a list can sort timestamps by.
pub fn epoch_parts(instant: SystemTime) -> (i64, u32) {
    match instant.duration_since(UNIX_EPOCH) {
        Ok(elapsed) => (
            i64::try_from(elapsed.as_secs()).unwrap_or(i64::MAX),
            elapsed.subsec_nanos(),
        ),
        Err(_) => (0, 0),
    }
}

/// `instant` as whole seconds since the Unix epoch — the precision the two
/// creation-time list filters compare at.
///
/// A `Timestamp` travels this protocol as an epoch value whose fraction is a
/// float, so a sub-second bound a caller echoes back from a previous response
/// does not reconstruct bit-for-bit. Whole seconds do, so comparing at second
/// granularity is what makes a bound a caller READ OUT of a `CreatedDate` mean
/// exactly that instant rather than a nanosecond either side of it.
pub fn epoch_whole_seconds(instant: SystemTime) -> i64 {
    epoch_parts(instant).0
}

/// A modelled `Timestamp` in the same whole seconds [`epoch_whole_seconds`]
/// answers in.
pub fn wire_whole_seconds(at: ::aws_smithy_types::DateTime) -> i64 {
    at.secs()
}

/// One page of a `@paginated` list over `cursors`, which are the sorted
/// resource identifiers of every item the filters kept.
///
/// `NextToken` is the identifier the next page starts at: the only cursor this
/// store has that survives concurrent writes, and a non-whitespace string, so it
/// satisfies `NextToken`'s `^[\S]+$`. A token naming no kept item was never
/// issued for this query, so this answers `None` and the caller is told so rather
/// than silently served the whole collection from the start. `MaxResults` is
/// clamped to [`MAX_PAGE_SIZE`], and an absent one asks for a full page rather
/// than for everything. A `MaxResults` at or below zero is refused by the
/// handler, with the modelled `ValidationException`, before this is reached; the
/// lower end of the clamp here is defence in depth and never the refusal.
pub fn page(
    cursors: &[String],
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<Page> {
    let start = match next_token {
        Some(token) => cursors.iter().position(|cursor| cursor == token)?,
        None => 0,
    };
    let limit = max_results.map_or(MAX_PAGE_SIZE, |value| value.clamp(1, MAX_PAGE_SIZE)) as usize;
    let end = start.saturating_add(limit).min(cursors.len());
    Some(Page {
        start,
        end,
        next_token: cursors.get(end).cloned(),
    })
}

/// Where one page of a list begins and ends in the sorted, filtered items, and
/// the `NextToken` that resumes after it.
#[derive(Debug, Clone)]
pub struct Page {
    /// Index of the first item on this page.
    pub start: usize,
    /// Index one past the last item on this page.
    pub end: usize,
    /// `NextToken`, absent on the last page.
    pub next_token: Option<String>,
}

// ---------------------------------------------------------------------------
// The allocation date and percentage rules the model documents
// ---------------------------------------------------------------------------

/// `EffectiveFrom` as a calendar date.
///
/// The model documents "Must be the first day of a month", and the `@pattern`
/// the boundary enforces still admits a day no calendar has (`2024-02-30`), so
/// both are checked here.
pub fn validate_effective_from(
    member: &str,
    value: &str,
) -> Result<::aws_smithy_types::DateTime, ValidationException> {
    let date = parse_date(value).ok_or_else(|| {
        invalid_value(
            member,
            format!("{value:?} is not a calendar date in YYYY-MM-DD between 0001 and 9999"),
        )
    })?;
    if !is_first_day_of_month(date) {
        return Err(business_validation(
            member,
            format!("{value:?} must be the first day of a month"),
        ));
    }
    Ok(date)
}

/// `EffectiveUntil` as a calendar date.
///
/// The model documents "Must be the last day of a month (YYYY-MM-DD)".
pub fn validate_effective_until(
    member: &str,
    value: &str,
) -> Result<::aws_smithy_types::DateTime, ValidationException> {
    let date = parse_date(value).ok_or_else(|| {
        invalid_value(
            member,
            format!("{value:?} is not a calendar date in YYYY-MM-DD between 0001 and 9999"),
        )
    })?;
    if !is_last_day_of_month(date) {
        return Err(business_validation(
            member,
            format!("{value:?} must be the last day of a month"),
        ));
    }
    Ok(date)
}

/// The allocation's effective interval must not end before it starts. The model
/// states no bound relating the two members, so this is the emulator's own
/// interval check, reported as a business validation.
pub fn validate_interval(from: &str, until: &str) -> Result<(), ValidationException> {
    // `YYYY-MM-DD` orders chronologically as a string, and both members have
    // already been parsed as calendar dates by the callers.
    if until < from {
        return Err(business_validation(
            "EffectiveUntil",
            format!("{until:?} is before EffectiveFrom {from:?}"),
        ));
    }
    Ok(())
}

/// `RevenueSharePercent` must be a share of revenue, so it cannot exceed 100.
/// The `@pattern` admits three integer digits, so the handler owns this bound.
pub fn validate_percent(member: &str, value: &str) -> Result<(), ValidationException> {
    let hundredths = percent_hundredths(value)
        .ok_or_else(|| invalid_value(member, format!("{value:?} is not a percentage")))?;
    if hundredths > MAX_PERCENT_HUNDREDTHS {
        return Err(business_validation(
            member,
            format!("{value:?} is more than 100 percent"),
        ));
    }
    Ok(())
}

/// `Catalog` as this service holds it, or the modelled 400 the wire value earns.
///
/// `CatalogName` is an enum SHAPE: the boundary validator does not check its
/// membership, and the deserializer materializes an absent enum member as the
/// empty string, so both cases are a handler's to refuse.
pub fn parse_catalog(value: &str) -> Result<CatalogName, ValidationException> {
    if value.is_empty() {
        return Err(required_member("Catalog"));
    }
    CatalogName::from_wire(value).ok_or_else(|| invalid_enum("Catalog", CATALOG_NAME_VALUES))
}

/// The modelled 400 for a `MaxResults` that asks for a page of no items.
pub fn invalid_max_results(requested: i32) -> ValidationException {
    invalid_value(
        "MaxResults",
        format!("{requested} is not a page size; it must be at least 1"),
    )
}

/// The modelled 400 for a `NextToken` this query never issued.
pub fn invalid_next_token() -> ValidationException {
    invalid_value(
        "NextToken",
        "is not a pagination token this query issued; restart the list without it",
    )
}
