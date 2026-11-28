// SPDX-License-Identifier: CC0-1.0

//! The revenue-attribution half of this service's state: the records a revenue
//! attribution and its allocations exist as, and the refusals a request that
//! names one the store does not hold earns.
//!
//! Held beside the marketplace-revenue-share records in the parent module
//! because the two planes are separate namespaces with separate identifiers
//! (`ra-…` against `mrs-…`), and every implemented revenue-attribution handler
//! reads or writes ONLY through here — so a create is observable by the next
//! get or list and an update by the next reader.
//!
//! # Revisions are held, not faked
//!
//! `GetRevenueAttribution` takes an optional `Revision` that "omit to return the
//! latest revision", and the two allocation reads take an optional
//! `RevenueAttributionRevision`, so an attribution holds its whole revision
//! history: the instant each revision was written and the mutable values it held
//! then ([`AttributionRevision`]). A read at revision `r` is answered from the
//! revision that was current then, reconstructed from state rather than stored
//! as a response literal.
//!
//! # What writes an allocation
//!
//! Allocations under an attribution are written by
//! `StartRevenueAttributionAllocationsTask`, which is still a scaffold stub
//! (`src/ops/start_revenue_attribution_allocations_task.rs`). Until it is
//! implemented an attribution holds none, so
//! `ListRevenueAttributionAllocations` answers with the empty list the store
//! really holds and `GetRevenueAttributionAllocation` answers the modelled 404 —
//! both read out of [`RevenueAttribution::allocations`], so neither changes when
//! that operation starts filling it.

use std::collections::{BTreeMap, HashMap};
use std::time::SystemTime;

use crate::ops::types::{
    ConflictException, MarketplaceProductSummary, ResourceNotFoundException, Tag,
    ValidationException, TENANCY_MODEL_VALUES,
};

use super::{
    business_validation, invalid_enum, product_code, product_id_segment, required_member,
    AllocationStatus, CatalogName, ARN_SERVICE,
};

/// The ARN resource prefix of a revenue attribution, as `RevenueAttributionId`'s
/// own pattern spells it
/// (`…:catalog/[a-zA-Z]+/revenue-attribution/ra-[a-z0-9]{13}`).
pub const REVENUE_ATTRIBUTION_RESOURCE: &str = "revenue-attribution";

/// The leading characters of every revenue attribution id this service mints
/// (`RevenueAttributionId`: `^ra-[a-z0-9]{13}$` in its short form).
pub const ATTRIBUTION_ID_PREFIX: &str = "ra-";

/// The most revenue attributions this emulator holds at once.
///
/// A bound on how far the store can grow under repeated creates: a create past
/// it is refused with the modelled `ServiceQuotaExceededException` (HTTP 402)
/// rather than accepted, so a caller cannot grow the process's memory without
/// limit. AWS publishes its own quota and it is not this number; this is the
/// emulator's own ceiling, chosen well above what a client script needs, and the
/// same one the marketplace-revenue-share plane uses
/// ([`super::MAX_REVENUE_SHARES`]).
pub const MAX_REVENUE_ATTRIBUTIONS: usize = 100;

/// The highest revision one revenue attribution reaches in this emulator, for
/// the reason [`super::MAX_SHARE_REVISION`] states for a revenue share: every
/// write appends a revision and the history is what a historical read is
/// answered from, so the history is also the store's growth vector.
pub const MAX_ATTRIBUTION_REVISION: i32 = 1_000;

/// `TenancyModel` — whether the attribution covers one customer or many.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TenancyModel {
    /// `MULTI_TENANT`.
    MultiTenant,
    /// `SINGLE_TENANT`.
    SingleTenant,
}

impl TenancyModel {
    /// The value this tenancy model serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::MultiTenant => "MULTI_TENANT",
            Self::SingleTenant => "SINGLE_TENANT",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "MULTI_TENANT" => Some(Self::MultiTenant),
            "SINGLE_TENANT" => Some(Self::SingleTenant),
            _ => None,
        }
    }
}

/// `TenancyModel` as this service holds it, or the modelled 400 the wire value
/// earns.
///
/// `TenancyModel` is an enum SHAPE: the boundary validator does not check its
/// membership, and the deserializer materializes an absent enum member as the
/// empty string, so both cases are a handler's to refuse — exactly as
/// [`super::parse_catalog`] does for `Catalog`.
pub fn parse_tenancy_model(value: &str) -> Result<TenancyModel, ValidationException> {
    if value.is_empty() {
        return Err(required_member("TenancyModel"));
    }
    TenancyModel::from_wire(value)
        .ok_or_else(|| invalid_enum("TenancyModel", TENANCY_MODEL_VALUES))
}

/// `EntityType` — what an allocation attributes revenue to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EntityType {
    /// `OFFER`.
    Offer,
    /// `OPPORTUNITY`.
    Opportunity,
}

impl EntityType {
    /// The value this entity type serializes as.
    pub fn as_wire(self) -> &'static str {
        match self {
            Self::Offer => "OFFER",
            Self::Opportunity => "OPPORTUNITY",
        }
    }

    /// Parse a wire value, or `None` when it is not one the model declares.
    pub fn from_wire(value: &str) -> Option<Self> {
        match value {
            "OFFER" => Some(Self::Offer),
            "OPPORTUNITY" => Some(Self::Opportunity),
            _ => None,
        }
    }
}

/// One revision of a revenue attribution: when it was written, and the mutable
/// values the attribution held from then on.
///
/// Every write appends one of these, so a historical read (`Revision` on the
/// get, `RevenueAttributionRevision` on the allocation reads) is answered from
/// the revision that was current then rather than from the newest values.
#[derive(Debug, Clone)]
pub struct AttributionRevision {
    /// When this revision was written.
    pub written_at: SystemTime,
    /// `Description` as the attribution held it at this revision.
    pub description: Option<String>,
    /// `Context` as the attribution held it at this revision. A `BTreeMap`, so
    /// the map a read answers with is in one stable order whatever order the
    /// request wrote the keys in.
    pub context: BTreeMap<String, String>,
}

/// One version of an allocation under an attribution: the mutable half of the
/// record, plus the attribution revision at which this version became current.
#[derive(Debug, Clone)]
pub struct AttributionAllocationVersion {
    /// The parent attribution revision this version was written at. A read at
    /// revision `r` sees the last version whose `revision` is `<= r`.
    pub revision: i32,
    /// `RevenueSharePercent`, verbatim as the caller wrote it.
    pub revenue_share_percent: String,
    /// `EffectiveFrom` (`YYYY-MM-DD`).
    pub effective_from: String,
    /// `EffectiveUntil` (`YYYY-MM-DD`).
    pub effective_until: String,
    /// `Status`.
    pub status: AllocationStatus,
    /// `LastModifiedDate` for this version.
    pub modified_date: SystemTime,
}

/// One allocation under a revenue attribution, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct AttributionAllocation {
    /// `RevenueAttributionAllocationId` — minted by this service.
    pub allocation_id: String,
    /// `EntityType`.
    pub entity_type: EntityType,
    /// `EntityIdentifier`.
    pub entity_identifier: String,
    /// `EntityName`, absent when the service holds no display name for the
    /// entity.
    pub entity_name: Option<String>,
    /// `CustomerAwsAccountId`. Held verbatim and never logged: the member is
    /// `@sensitive`.
    pub customer_aws_account_id: String,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// The version this allocation was created with. Held SEPARATELY from the
    /// later ones so "an allocation always has at least one version" is a fact
    /// of the type rather than an assertion a reader has to trust.
    pub created_version: AttributionAllocationVersion,
    /// Every version written after the create, oldest first.
    pub later_versions: Vec<AttributionAllocationVersion>,
}

impl AttributionAllocation {
    /// Every version of this allocation, oldest first.
    pub fn versions(&self) -> impl DoubleEndedIterator<Item = &AttributionAllocationVersion> {
        ::std::iter::once(&self.created_version).chain(self.later_versions.iter())
    }

    /// This allocation as it existed at attribution revision `revision`: the
    /// last version written at or before it, or `None` when the allocation did
    /// not exist yet.
    pub fn version_at(&self, revision: i32) -> Option<&AttributionAllocationVersion> {
        self.versions()
            .rev()
            .find(|version| version.revision <= revision)
    }
}

/// A revenue attribution, as this emulator holds it.
#[derive(Debug, Clone)]
pub struct RevenueAttribution {
    /// `Id` — the `^ra-[a-z0-9]{13}$` identifier this service minted for it.
    pub attribution_id: String,
    /// `Arn`.
    pub arn: String,
    /// `Catalog`.
    pub catalog: CatalogName,
    /// `CreatedDate`.
    pub created_date: SystemTime,
    /// `Name`, as the create request named it. Immutable: no operation this
    /// service models carries a new one.
    pub name: String,
    /// `TenancyModel`, likewise immutable.
    pub tenancy_model: TenancyModel,
    /// The AWS Marketplace product id the create named, if any — the id itself
    /// even when the request carried a product ARN, which is the form every
    /// output answers with.
    pub product_id: Option<String>,
    /// The tags the create request carried.
    pub tags: Vec<Tag>,
    /// Every revision, oldest first: `revisions[r - 1]` is revision `r` and the
    /// last one is current. Never empty — the create writes revision 1 — so
    /// [`RevenueAttribution::revision`] is always at least 1.
    pub revisions: Vec<AttributionRevision>,
    /// Every allocation under this attribution, keyed by id. A `BTreeMap` over
    /// zero-padded ids, so iteration order is creation order and a list page
    /// resumes deterministically.
    pub allocations: BTreeMap<String, AttributionAllocation>,
}

impl RevenueAttribution {
    /// The current revision; `1` right after the create.
    pub fn revision(&self) -> i32 {
        i32::try_from(self.revisions.len()).unwrap_or(i32::MAX)
    }

    /// The revision `revision` names, or `None` for one this attribution never
    /// reached.
    pub fn revision_at(&self, revision: i32) -> Option<&AttributionRevision> {
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
    /// revision this attribution never reached.
    pub fn modified_at(&self, revision: i32) -> SystemTime {
        self.revision_at(revision)
            .map(|revision| revision.written_at)
            .unwrap_or(self.created_date)
    }

    /// `Description` as the attribution held it at `revision`.
    pub fn description_at(&self, revision: i32) -> Option<String> {
        self.revision_at(revision)
            .and_then(|revision| revision.description.clone())
    }

    /// `Context` as the attribution held it at `revision`, and no member at all
    /// for a revision whose map is empty: an absent `Context` and an empty one
    /// are the same thing to this service, and the model documents an empty map
    /// on the update as "clear it".
    pub fn context_at(&self, revision: i32) -> Option<HashMap<String, String>> {
        let held = self.revision_at(revision)?;
        (!held.context.is_empty()).then(|| {
            held.context
                .iter()
                .map(|(key, value)| (key.clone(), value.clone()))
                .collect()
        })
    }

    /// The context map as it stands at the current revision, which is what a
    /// context filter is evaluated against and what an update that omits
    /// `Context` carries forward.
    pub fn context(&self) -> BTreeMap<String, String> {
        self.revisions
            .last()
            .map(|revision| revision.context.clone())
            .unwrap_or_default()
    }

    /// `MarketplaceProduct` as every attribution-shaped response carries it:
    /// absent when the create named no product.
    ///
    /// `ProductName` is left absent and `ProductCode` derived exactly as the
    /// marketplace-revenue-share plane does ([`super::product_code`]) — this
    /// emulator holds no AWS Marketplace catalog to read a display name out of.
    pub fn marketplace_product(&self) -> Option<MarketplaceProductSummary> {
        let product_id = self.product_id.clone()?;
        Some(MarketplaceProductSummary {
            product_code: product_code(&product_id),
            product_name: None,
            product_id: Some(product_id),
        })
    }

    /// Every allocation that existed at `revision`, paired with the version that
    /// was current then, in allocation-id (creation) order.
    pub fn allocations_at(
        &self,
        revision: i32,
    ) -> impl Iterator<Item = (&AttributionAllocation, &AttributionAllocationVersion)> {
        self.allocations
            .values()
            .filter_map(move |allocation| Some((allocation, allocation.version_at(revision)?)))
    }

    /// `TotalRevenueAttributionAllocationCount` and
    /// `TotalActiveRevenueAttributionAllocationCount` at `revision`, both
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

    /// `EffectiveFrom` and `EffectiveUntil` as the attribution reports them at
    /// `revision`: the model documents each as "derived from the earliest
    /// allocation start date" and "the latest allocation end date", so both are
    /// absent while the attribution holds no allocation.
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

    /// Whether revision `revision` of this attribution is the one an update
    /// carrying `description` and `context` wrote — the comparison an idempotent
    /// replay makes before answering with it, so a `ClientToken` reused with
    /// DIFFERENT values conflicts instead of quietly answering with the first
    /// call's revision.
    ///
    /// An absent `context` asks for the stored map unchanged, so it matches when
    /// that revision holds the same map as the one before it.
    pub fn matches_update(
        &self,
        revision: i32,
        description: Option<&String>,
        context: Option<&HashMap<String, String>>,
    ) -> bool {
        let Some(held) = self.revision_at(revision) else {
            return false;
        };
        if held.description.as_ref() != description {
            return false;
        }
        match context {
            None => {
                self.revision_at(revision.saturating_sub(1))
                    .map(|previous| &previous.context)
                    == Some(&held.context)
            }
            Some(wanted) => {
                held.context.len() == wanted.len()
                    && wanted
                        .iter()
                        .all(|(key, value)| held.context.get(key) == Some(value))
            }
        }
    }

    /// Record a new revision written at `now`, holding `description` and
    /// `context`, and answer its number.
    pub fn push_revision(
        &mut self,
        now: SystemTime,
        description: Option<String>,
        context: BTreeMap<String, String>,
    ) -> i32 {
        self.revisions.push(AttributionRevision {
            written_at: now,
            description,
            context,
        });
        self.revision()
    }

    /// Whether this record is the one `CreateRevenueAttribution` would mint for
    /// these parameters — the comparison an idempotent replay makes before
    /// answering with it, so a `ClientToken` reused with DIFFERENT parameters
    /// conflicts instead of quietly answering with the first call's attribution.
    ///
    /// The values compared are the ones the CREATE wrote (revision 1), not the
    /// ones the attribution holds now: a later `UpdateRevenueAttribution` does
    /// not turn a faithful replay of the create into a conflict.
    pub fn matches_create(&self, wanted: &AttributionCreate<'_>) -> bool {
        let created = self.revision_at(1);
        self.catalog == wanted.catalog
            && self.name == wanted.name
            && self.tenancy_model == wanted.tenancy_model
            && self.product_id.as_ref() == wanted.product_id
            && created.and_then(|revision| revision.description.as_ref()) == wanted.description
            && created.map(|revision| &revision.context) == Some(wanted.context)
            && super::tag_pairs(Some(&self.tags)) == super::tag_pairs(wanted.tags)
    }
}

/// The revenue attribution a `CreateRevenueAttribution` request asks for: the
/// values the create would mint, borrowed from the request.
///
/// One value rather than seven arguments, so the idempotent replay comparison
/// ([`RevenueAttribution::matches_create`]) and the record the create writes
/// cannot drift apart over which members identify a create.
#[derive(Debug)]
pub struct AttributionCreate<'a> {
    /// `Catalog`.
    pub catalog: CatalogName,
    /// `Name`.
    pub name: &'a str,
    /// `TenancyModel`.
    pub tenancy_model: TenancyModel,
    /// The product id `ProductIdentifier` named, in the id form the attribution
    /// stores.
    pub product_id: Option<&'a String>,
    /// `Description`.
    pub description: Option<&'a String>,
    /// `Context`.
    pub context: &'a BTreeMap<String, String>,
    /// `Tags`.
    pub tags: Option<&'a Vec<Tag>>,
}

/// A revenue attribution id for sequence number `sequence`, matching
/// `RevenueAttributionId`'s short form `^ra-[a-z0-9]{13}$`. Zero-padded to
/// exactly 13 digits, so id order is creation order and a list page resumes
/// deterministically — the same minting rule as [`super::share_id`].
pub fn attribution_id(sequence: u64) -> String {
    format!("{ATTRIBUTION_ID_PREFIX}{:013}", sequence % 10_000_000_000_000)
}

/// The ARN of a revenue attribution, through the one ARN builder the framework
/// vends (so the account segment is the canonical emulator account).
pub fn attribution_arn(region: &str, catalog: CatalogName, attribution_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!(
            "catalog/{}/{REVENUE_ATTRIBUTION_RESOURCE}/{attribution_id}",
            catalog.as_wire()
        ),
    )
}

/// The attribution id a `RevenueAttributionIdentifier` names: the identifier
/// itself in short form (`ra-…`), or the resource segment of the ARN form.
///
/// The member's `@pattern` admits exactly those two forms and is enforced at the
/// boundary, so this only has to pick the id out. The ARN form's Region and
/// catalog segments are NOT read back: `Catalog` is its own `@required` member
/// and the account is this emulator's, so the id is what identifies the
/// attribution.
pub fn parse_attribution_identifier(value: &str) -> Option<String> {
    let candidate = match value.rsplit_once('/') {
        Some((_, last)) => last,
        None => value,
    };
    candidate
        .starts_with(ATTRIBUTION_ID_PREFIX)
        .then(|| candidate.to_string())
}

/// The modelled 404 for a revenue attribution this catalog does not hold, as the
/// operations that ADDRESS one by its own `Identifier` word it.
///
/// The message names the identifier the request sent, which is what the caller
/// can act on, in either of the two forms the member admits.
pub fn attribution_not_found(identifier: &str) -> ResourceNotFoundException {
    super::not_found(format!(
        "We couldn't find revenue attribution '{identifier}'. Verify the ID and try again."
    ))
}

/// The modelled 404 for a revenue attribution a request REFERENCES rather than
/// addresses — one named in `ListRevenueAttributions`'s `Identifiers` list.
pub fn referenced_attribution_not_found(identifier: &str) -> ResourceNotFoundException {
    super::not_found(format!(
        "Revenue attribution {identifier} doesn't exist. Verify the ID and try again."
    ))
}

/// The modelled 404 for an allocation this attribution does not hold.
pub fn attribution_allocation_not_found(allocation_id: &str) -> ResourceNotFoundException {
    super::not_found(format!(
        "We couldn't find revenue attribution allocation '{allocation_id}'. Verify the ID and try \
         again."
    ))
}

/// The modelled 404 for an attribution revision the store never reached.
///
/// `requested` is the caller's token text, not a parsed number: `RevisionToken`
/// admits up to 19 digits, which no `i32` holds, so re-rendering a parse would
/// report a revision the caller never sent.
pub fn attribution_revision_not_found(
    identifier: &str,
    requested: &str,
) -> ResourceNotFoundException {
    super::not_found(format!(
        "We couldn't find revision {requested} of revenue attribution '{identifier}'. Verify the \
         revision and try again."
    ))
}

/// The modelled 409 for the optimistic concurrency check `Revision` carries on
/// an attribution write: the caller's revision is not the attribution's current
/// one, so the attribution moved under it.
pub fn attribution_revision_mismatch(
    identifier: &str,
    supplied: &str,
    current: i32,
) -> ConflictException {
    super::conflict(
        "REVISION_MISMATCH",
        format!(
            "Revision {supplied} is not the current revision of revenue attribution \
             '{identifier}', which is {current}."
        ),
    )
}

/// The modelled 400 for a write that would carry an attribution past
/// [`MAX_ATTRIBUTION_REVISION`].
pub fn attribution_revision_ceiling(identifier: &str) -> ValidationException {
    business_validation(
        "Revision",
        format!(
            "revenue attribution '{identifier}' has reached this emulator's ceiling of \
             {MAX_ATTRIBUTION_REVISION} revisions"
        ),
    )
}

/// The product id a `ProductIdentifier` names: the value itself, or the last
/// segment of a product ARN — the model documents both forms ("a product entity
/// ID (e.g., prod-abc123def4567) or a product ARN").
///
/// The segment split is [`super::product_id_segment`], the same reading the
/// marketplace-revenue-share plane gives a product identifier, so one service
/// has one answer for what names a product.
pub fn resolve_product_identifier(value: &str) -> String {
    product_id_segment(value).to_string()
}

/// Whether the stored `context` satisfies one `ContextFilter`: the key is
/// present and, when the filter lists values, its stored value is one of them —
/// "matches when the stored value equals any of these values. When omitted,
/// matches when the key is present."
pub fn context_filter_matches(
    context: &BTreeMap<String, String>,
    key: &str,
    values: Option<&Vec<String>>,
) -> bool {
    let Some(held) = context.get(key) else {
        return false;
    };
    match values {
        None => true,
        Some(values) if values.is_empty() => true,
        Some(values) => values.iter().any(|value| value == held),
    }
}
