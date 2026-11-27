// SPDX-License-Identifier: CC0-1.0

//! Subscription requests: the stored record, and the one view every operation that
//! reports a subscription request renders it through.
//!
//! A subscription request is a principal's ask for access to a catalog listing
//! ("Creates a subscription request in Amazon DataZone", smithy_query
//! operation="CreateSubscriptionRequest"). What is STORED here is the ask itself — who
//! asked, which listing at which recorded revision, why, and the metadata forms, asset
//! scopes and asset permissions the request carried. Everything a response reports ABOUT
//! those references — a project's name, a listing's name and description, the published
//! entity — is resolved from the domain on every read through
//! [`subscription_request_view`], so no stored copy of them can go stale and the create
//! and the reader cannot drift apart.

use super::{
    asset_schema_columns, date_time, evaluate_asset_filter, Domain, Requester, StoredForm,
    FILTER_STATUS_INVALID, FILTER_STATUS_VALID,
};
use crate::ops::types::{
    AcceptedAssetScope, AssetPermission, AssetScope, FormOutput, SubscribedAssetListing,
    SubscribedGroup, SubscribedIamPrincipal, SubscribedListing, SubscribedListingItem,
    SubscribedPrincipal, SubscribedProductListing, SubscribedProject, SubscribedUser,
};

/// The `SubscriptionRequestStatus` members (model enum
/// `com.amazonaws.datazone#SubscriptionRequestStatus`: PENDING | ACCEPTED | REJECTED).
///
/// A request is an ASK, so it is created either awaiting a decision
/// ([`SUBSCRIPTION_REQUEST_STATUS_PENDING`]) or already resolved by the ask itself
/// ([`created_subscription_request_status`]); from then on only a decision operation —
/// `AcceptSubscriptionRequest` or `RejectSubscriptionRequest` — moves it to one of the two
/// terminal members. None of the three is in-progress, so a stored request owes the clock
/// no transition.
pub const SUBSCRIPTION_REQUEST_STATUS_PENDING: &str = "PENDING";
pub const SUBSCRIPTION_REQUEST_STATUS_ACCEPTED: &str = "ACCEPTED";
pub const SUBSCRIPTION_REQUEST_STATUS_REJECTED: &str = "REJECTED";

/// The `SubscriptionRequestStatus` a request made BY `requester` against `listings` is
/// recorded with.
///
/// An ask for access to data the asker already owns has nobody left to review it, so
/// DataZone resolves it when it is made: a request whose requester owns the project that
/// published EVERY subscribed listing ([`super::Project::is_owned_by`]) is recorded
/// [`SUBSCRIPTION_REQUEST_STATUS_ACCEPTED`], and every other request is recorded
/// [`SUBSCRIPTION_REQUEST_STATUS_PENDING`], awaiting a reviewer's decision. A request
/// resolved this way was decided by nobody, so it carries no `reviewerId` and no
/// `decisionComment`, nothing has updated it since it was made, and no `Subscription` is
/// materialised for it — only a DECISION grants access
/// ([`super::approved_subscription`]).
///
/// RECORDED behaviour: the model declares `status` required and names its three members but
/// does not state when a created request is already decided (smithy_query
/// operation="CreateSubscriptionRequest").
///
/// Whether the subscribed listing has a subscription target behind it does NOT enter into
/// it: a target is the path a GRANT is fulfilled through, not a decision
/// ([`super::SubscriptionTarget`]).
#[must_use]
pub fn created_subscription_request_status(
    domain: &Domain,
    requester: &Requester,
    listings: &[SubscribedListingRef],
) -> &'static str {
    let publisher_is_the_requesters = |reference: &SubscribedListingRef| {
        domain
            .listings
            .get(&reference.listing_id)
            .and_then(|listing| listing.revisions.get(&reference.revision))
            .and_then(|revision| domain.projects.get(&revision.owning_project_id))
            .is_some_and(|project| project.is_owned_by(requester))
    };
    // `subscribedListings` is 1..1 (smithy_query operation="CreateSubscriptionRequest"), so
    // the empty case cannot be reached from the wire; it is PENDING rather than vacuously
    // resolved, because an ask that names no listing names no data the requester owns.
    if !listings.is_empty() && listings.iter().all(publisher_is_the_requesters) {
        SUBSCRIPTION_REQUEST_STATUS_ACCEPTED
    } else {
        SUBSCRIPTION_REQUEST_STATUS_PENDING
    }
}

/// The `ValidationException` message `AcceptSubscriptionRequest` refuses a request that
/// is no longer PENDING with: a decision has already been taken on it, so there is none
/// left to take. Entirely fixed text — recorded from the service — so it echoes nothing
/// of the request or of the store back.
pub const SUBSCRIPTION_ALREADY_DECIDED_MESSAGE: &str =
    "The approved subscription is already revoked or unsubscribed.";

/// The `ValidationException` message `RejectSubscriptionRequest` refuses a request that
/// is not PENDING with.
///
/// Fixed text followed by ONE variable part: the status the request currently carries,
/// read from the store (so an already-accepted request and an already-rejected one each
/// name their own).
pub fn cannot_reject_message(status: &str) -> String {
    format!("Cannot reject a subscription request with status {status}")
}

impl SubscriptionRequest {
    /// Whether this request is still awaiting a decision, i.e. whether a decision
    /// operation may take one on it. The one rule both decision handlers ask, so neither
    /// can refuse on a different notion of "already decided".
    pub fn awaits_a_decision(&self) -> bool {
        self.status == SUBSCRIPTION_REQUEST_STATUS_PENDING
    }

    /// Record a decision on this request: the terminal `status`, the reviewer that took
    /// it, the comment it carried, and the instant it was taken.
    ///
    /// The one writer both decision handlers use, so an accept and a reject cannot leave
    /// a differently-shaped record behind. A decision that carries no comment leaves the
    /// stored one absent, which is what the reader reports.
    pub fn record_decision(
        &mut self,
        status: &str,
        reviewer: String,
        decision_comment: Option<String>,
        now: i64,
    ) {
        self.status = status.to_string();
        self.reviewer_id = Some(reviewer.clone());
        self.decision_comment = decision_comment;
        self.updated_by = Some(reviewer);
        self.updated_at_ms = now;
    }
}

/// The published ASSETS the stored listings of `request` record — what an `assetScopes`
/// or `assetPermissions` entry about this request may name.
///
/// Only an ASSET listing publishes an asset; a data product listing publishes the
/// product, whose own assets come from its record.
pub fn subscribed_asset_ids(domain: &Domain, request: &SubscriptionRequest) -> Vec<String> {
    request
        .listings
        .iter()
        .filter_map(|reference| domain.listings.get(&reference.listing_id))
        .filter(|listing| listing.entity_type == super::ENTITY_TYPE_ASSET)
        .map(|listing| listing.entity_id.clone())
        .collect()
}

/// Why an `assetScopes` or `assetPermissions` entry cannot be recorded.
#[derive(Debug, Clone)]
pub enum AssetReferenceRefusal {
    /// The entry names an asset none of the subscribed listings publishes, so the
    /// request would ask for something about an asset it is not about. Carries the
    /// `ValidationException` message, which names the member only.
    NotSubscribed(String),
    /// A scope names a filter that is not one of its asset's own, so it is as absent as
    /// one nothing created: a `ResourceNotFoundException` for
    /// [`super::ResourceKind::AssetFilter`].
    FilterNotFound,
}

/// Check the `assetScopes` and `assetPermissions` a create or a decision carries against
/// the assets `subscribed_assets` the subscribed listings publish.
///
/// Each entry narrows what is asked for on ONE asset, so it has to name an asset those
/// listings publish, and a scope's `filterIds` have to name filters OF that asset — so a
/// stored scope's filters always resolve when it is reported back. The one rule every
/// operation that takes these members applies, so a create and a decision cannot admit
/// different entries.
pub fn check_asset_references(
    domain: &Domain,
    subscribed_assets: &[String],
    asset_scopes: &[AcceptedAssetScope],
    asset_permissions: &[AssetPermission],
) -> Result<(), AssetReferenceRefusal> {
    let publishes = |asset_id: &str| subscribed_assets.iter().any(|id| id == asset_id);
    for (index, scope) in asset_scopes.iter().enumerate() {
        if !publishes(&scope.asset_id) {
            return Err(AssetReferenceRefusal::NotSubscribed(format!(
                "assetScopes[{index}].assetId must name an asset one of the subscribedListings publishes"
            )));
        }
        let asset = domain.assets.get(&scope.asset_id);
        if scope
            .filter_ids
            .iter()
            .any(|id| asset.is_none_or(|asset| !asset.filters.contains_key(id)))
        {
            return Err(AssetReferenceRefusal::FilterNotFound);
        }
    }
    for (index, permission) in asset_permissions.iter().enumerate() {
        if !publishes(&permission.asset_id) {
            return Err(AssetReferenceRefusal::NotSubscribed(format!(
                "assetPermissions[{index}].assetId must name an asset one of the subscribedListings publishes"
            )));
        }
    }
    Ok(())
}

/// The most subscription requests this emulator stores in one domain.
///
/// A CHOSEN ceiling, not a published quota: `CreateSubscriptionRequest` is a
/// caller-driven growth site, so the per-domain map has to be bounded or a create loop
/// grows the emulator's heap without limit. The refusal past it is the
/// `ServiceQuotaExceededException` the operation already models — the rule
/// `CreateGlossaryTerm` follows for its own bound.
pub const MAX_SUBSCRIPTION_REQUESTS_PER_DOMAIN: usize = 500;

/// The principal a subscription request was made FOR, as the request named it: one arm
/// of the modelled `SubscribedPrincipalInput` union (smithy_query
/// shape="SubscribedPrincipalInput") plus the identifier it carried.
///
/// Only the IDENTITY is stored. What the response reports for it — a project's name, a
/// user profile's details — belongs to that principal's own record and is resolved on
/// every read by [`subscribed_principal`], so a renamed project is reported under its
/// new name.
#[derive(Debug, Clone)]
pub enum SubscribedPrincipalRef {
    Project(String),
    User(String),
    Group(String),
    Iam(String),
}

/// The listing a subscription request was made against: its `ListingId` and the
/// revision of it the catalog held when the request was made.
///
/// The revision is pinned deliberately — a listing is "a record of an asset at a given
/// time" (smithy_query operation="CreateListingChangeSet"), so a later change set that
/// records a further revision does not rewrite what this request asked for. A recorded
/// revision is never rewritten either, so the pair reads back exactly as it was stored.
#[derive(Debug, Clone)]
pub struct SubscribedListingRef {
    pub listing_id: String,
    pub revision: u32,
}

/// One stored subscription request.
#[derive(Debug, Clone)]
pub struct SubscriptionRequest {
    /// The request's own `SubscriptionRequestId`, minted by
    /// [`super::ComAmazonawsDatazoneDatazoneState::mint_id`].
    pub id: String,
    pub domain_id: String,
    /// One of [`SUBSCRIPTION_REQUEST_STATUS_PENDING`] /
    /// [`SUBSCRIPTION_REQUEST_STATUS_ACCEPTED`] /
    /// [`SUBSCRIPTION_REQUEST_STATUS_REJECTED`].
    pub status: String,
    pub request_reason: String,
    /// The principals the request was made for. `SubscribedPrincipalInputs` is 1..1
    /// (smithy_query operation="CreateSubscriptionRequest"), so this holds exactly one
    /// today; it is a list because the modelled member is one.
    pub principals: Vec<SubscribedPrincipalRef>,
    /// The listings the request was made against — `SubscribedListingInputs` is 1..1,
    /// the same way.
    pub listings: Vec<SubscribedListingRef>,
    /// The `metadataForms` the request carried, in the stored form every metadata form
    /// in this service takes ([`StoredForm`]), so one rendering serves them all.
    pub metadata_forms: Vec<StoredForm>,
    /// The `assetScopes` the request carried, kept as the modelled shapes themselves
    /// (the trade [`super::AccountPool`] makes for `accountSource`), so the reader
    /// reports back exactly what was asked for.
    pub asset_scopes: Vec<AcceptedAssetScope>,
    /// The `assetPermissions` the request carried, kept the same way.
    pub asset_permissions: Vec<AssetPermission>,
    pub created_by: String,
    pub created_at_ms: i64,
    /// The caller and instant of the last change to the request. `updated_at_ms` equals
    /// [`Self::created_at_ms`] and `updated_by` is absent until something updates it,
    /// because `updatedAt` is a REQUIRED output member while `updatedBy` is not
    /// (smithy_query operation="CreateSubscriptionRequest").
    pub updated_by: Option<String>,
    pub updated_at_ms: i64,
    /// The reviewer and comment of the decision taken on this request, absent while it
    /// is still PENDING: nobody has decided it yet.
    pub reviewer_id: Option<String>,
    pub decision_comment: Option<String>,
    /// The subscription the requested access already exists under, when the principal
    /// holds one for the same listing.
    pub existing_subscription_id: Option<String>,
}

/// A stored subscription request as every operation that reports one answers it.
///
/// `CreateSubscriptionRequestOutput` and `GetSubscriptionRequestDetailsOutput` declare
/// the same members (smithy_query
/// operation="CreateSubscriptionRequest,GetSubscriptionRequestDetails"), so the view is
/// built once here and each handler maps it onto its own generated output struct: the
/// create and the read cannot then report one stored request differently.
#[derive(Debug, Clone)]
pub struct SubscriptionRequestView {
    pub id: String,
    pub created_by: String,
    pub updated_by: Option<String>,
    pub domain_id: String,
    pub status: String,
    pub created_at: ::aws_smithy_types::DateTime,
    pub updated_at: ::aws_smithy_types::DateTime,
    pub request_reason: String,
    pub subscribed_principals: Vec<SubscribedPrincipal>,
    pub subscribed_listings: Vec<SubscribedListing>,
    pub reviewer_id: Option<String>,
    pub decision_comment: Option<String>,
    pub existing_subscription_id: Option<String>,
    pub metadata_forms: Option<Vec<FormOutput>>,
}

/// The stored request `request` of `domain`, as a response reports it.
///
/// `None` when a listing the request records is no longer in the domain's catalog: the
/// modelled `subscribedListings` is 1..1 and required, so a request whose listing the
/// store lost cannot be reported at all, and the handler answers a server fault rather
/// than a response missing a member the model requires. Nothing removes a stored listing
/// today (`DeleteListing` is a scaffold, and a domain's listings go with the domain,
/// which takes its subscription requests with it), so the case is unreachable — it is
/// handled rather than assumed away.
pub fn subscription_request_view(
    domain: &Domain,
    request: &SubscriptionRequest,
) -> Option<SubscriptionRequestView> {
    let mut subscribed_listings = Vec::with_capacity(request.listings.len());
    for reference in &request.listings {
        subscribed_listings.push(subscribed_listing(
            domain,
            &request.asset_scopes,
            &request.asset_permissions,
            reference,
        )?);
    }
    Some(SubscriptionRequestView {
        id: request.id.clone(),
        created_by: request.created_by.clone(),
        updated_by: request.updated_by.clone(),
        domain_id: request.domain_id.clone(),
        status: request.status.clone(),
        created_at: date_time(request.created_at_ms),
        updated_at: date_time(request.updated_at_ms),
        request_reason: request.request_reason.clone(),
        subscribed_principals: request
            .principals
            .iter()
            .map(|principal| subscribed_principal(domain, principal))
            .collect(),
        subscribed_listings,
        reviewer_id: request.reviewer_id.clone(),
        decision_comment: request.decision_comment.clone(),
        existing_subscription_id: request.existing_subscription_id.clone(),
        metadata_forms: (!request.metadata_forms.is_empty())
            .then(|| request.metadata_forms.iter().map(super::form_output).collect()),
    })
}

/// A recorded principal as the modelled `SubscribedPrincipal` union.
///
/// The arm follows the arm the request named, and each arm's reportable members are
/// resolved against `domain` as it is NOW: a project is reported with the name its
/// stored record carries and a user with the details its stored profile carries, so a
/// rename is visible to the next read. A principal whose record the domain no longer
/// holds is reported with its identifier alone — the only member the model marks
/// required is none of them (smithy_query shape="SubscribedProject,SubscribedUser") —
/// rather than with a name this store cannot stand behind.
///
/// A GROUP carries no resolvable name: group profiles are created by
/// `CreateGroupProfile`, which is still a scaffold, so no domain holds one (the same
/// reason [`super::resolve_membership_principal`] refuses a group member). An IAM
/// principal is a resource of another service, so its ARN is the whole of what DataZone
/// reports for it.
pub fn subscribed_principal(
    domain: &Domain,
    principal: &SubscribedPrincipalRef,
) -> SubscribedPrincipal {
    match principal {
        SubscribedPrincipalRef::Project(id) => SubscribedPrincipal::Project(SubscribedProject {
            id: Some(id.clone()),
            name: domain.projects.get(id).map(|project| project.name.clone()),
        }),
        SubscribedPrincipalRef::User(id) => SubscribedPrincipal::User(SubscribedUser {
            id: Some(id.clone()),
            details: domain
                .user_profiles
                .values()
                .find(|profile| &profile.id == id)
                .map(|profile| profile.details.clone()),
        }),
        SubscribedPrincipalRef::Group(id) => SubscribedPrincipal::Group(SubscribedGroup {
            id: Some(id.clone()),
            name: None,
        }),
        SubscribedPrincipalRef::Iam(arn) => SubscribedPrincipal::Iam(SubscribedIamPrincipal {
            principal_arn: Some(arn.clone()),
        }),
    }
}

/// A recorded listing reference as the modelled `SubscribedListing`.
///
/// Every member is read off the listing revision the request pinned, so what the
/// response reports is the catalog record as it was when the request was made, with one
/// exception: `ownerProjectName` is the name the owning project carries NOW, because the
/// name belongs to the project's own record. `None` when the listing or the pinned
/// revision is no longer stored — see [`subscription_request_view`].
///
/// `asset_scopes` and `asset_permissions` are what was asked for ON the published assets,
/// taken from the request the reference belongs to. They are passed in rather than read
/// off a request, so the SUBSCRIPTION a decision materialises reports its subscribed
/// listing through this same renderer ([`super::subscription_view`]) and the two cannot
/// describe one catalog record differently.
pub(super) fn subscribed_listing(
    domain: &Domain,
    asset_scopes: &[AcceptedAssetScope],
    asset_permissions: &[AssetPermission],
    reference: &SubscribedListingRef,
) -> Option<SubscribedListing> {
    let listing = domain.listings.get(&reference.listing_id)?;
    let revision = listing.revisions.get(&reference.revision)?;
    Some(SubscribedListing {
        id: listing.id.clone(),
        revision: Some(revision.revision.to_string()),
        name: revision.name.clone(),
        // `description` is REQUIRED on `SubscribedListing` (smithy_query
        // shape="SubscribedListing") while a publishable entity's description is
        // optional, so a listing recorded from an entity that had none reports the
        // empty description rather than omitting a member the model requires.
        description: revision.description.clone().unwrap_or_default(),
        item: subscribed_listing_item(domain, asset_scopes, asset_permissions, listing, revision),
        owner_project_id: revision.owning_project_id.clone(),
        owner_project_name: domain
            .projects
            .get(&revision.owning_project_id)
            .map(|project| project.name.clone()),
        }
    )
}

/// The published entity a subscribed listing records, as the modelled
/// `SubscribedListingItem` union.
///
/// Which arm is reported follows the listing's own entity type, the rule
/// [`super::listing_item`] applies to the `ListingItem` union: the snapshot is the same
/// either way and each arm names it with that entity's own vocabulary.
///
/// The ASSET arm also carries what the request asked for ON that asset: the
/// `assetScopes` entry naming it ([`accepted_asset_scope`]) and the `assetPermissions`
/// entry naming it, each reported only for the asset it names, so a scope asked for on
/// one asset is never reported against another.
///
/// `glossaryTerms` is the terms the listing revision recorded, resolved when the entity
/// was published ([`super::listing_glossary_terms`]): the same snapshot
/// [`super::listing_item`] reports, so the two readers cannot disagree about a published
/// entity's terms. It is left absent where the revision recorded none, since nothing in
/// the recordings shows this shape reporting an empty list.
fn subscribed_listing_item(
    domain: &Domain,
    asset_scopes: &[AcceptedAssetScope],
    asset_permissions: &[AssetPermission],
    listing: &super::Listing,
    revision: &super::ListingRevision,
) -> SubscribedListingItem {
    if listing.entity_type == super::ENTITY_TYPE_DATA_PRODUCT {
        return SubscribedListingItem::productListing(SubscribedProductListing {
            entity_id: Some(listing.entity_id.clone()),
            entity_revision: Some(revision.entity_revision.clone()),
            glossary_terms: None,
            name: Some(revision.name.clone()),
            description: revision.description.clone(),
            // The assets of a data product listing come from the data product's own
            // record, which no store holds yet (`CreateDataProduct` is a scaffold).
            asset_listings: None,
        });
    }
    SubscribedListingItem::assetListing(SubscribedAssetListing {
        entity_id: Some(listing.entity_id.clone()),
        entity_revision: Some(revision.entity_revision.clone()),
        entity_type: Some(revision.entity_type_name.clone()),
        forms: super::listing_forms(&revision.forms),
        glossary_terms: (!revision.glossary_terms.is_empty())
            .then(|| super::detailed_glossary_terms(&revision.glossary_terms)),
        asset_scope: asset_scopes
            .iter()
            .find(|scope| scope.asset_id == listing.entity_id)
            .map(|scope| accepted_asset_scope(domain, scope)),
        permissions: asset_permissions
            .iter()
            .find(|permission| permission.asset_id == listing.entity_id)
            .map(|permission| permission.permissions.clone()),
    })
}

/// An `assetScopes` entry as the modelled `AssetScope` it is reported back as.
///
/// `assetId` and `filterIds` are what the request asked for. `status` is a verdict on
/// that scope, so it is DERIVED on every read from the filters the scope names, each
/// evaluated against its asset's schema as it is now
/// ([`super::evaluate_asset_filter`]): the scope is [`FILTER_STATUS_VALID`] exactly when
/// every filter it names is a valid view of that asset, and [`FILTER_STATUS_INVALID`]
/// otherwise, carrying the first filter's own `errorMessage` — the `FilterStatus`
/// vocabulary (smithy_query shape="FilterStatus") this service already reports a
/// filtered view of an asset with. A scope naming a filter the asset does not have is
/// refused when the request is made, so a stored scope's filters resolve.
///
/// `scopeName` is left absent: nothing in the request or in the store names the scope.
pub fn accepted_asset_scope(domain: &Domain, scope: &AcceptedAssetScope) -> AssetScope {
    let asset = domain.assets.get(&scope.asset_id);
    let schema = asset.and_then(asset_schema_columns);
    let mut error_message = None;
    let mut valid = true;
    for filter_id in &scope.filter_ids {
        let Some(filter) = asset.and_then(|asset| asset.filters.get(filter_id)) else {
            continue;
        };
        let evaluation = evaluate_asset_filter(&filter.configuration, schema.as_deref());
        if evaluation.status != FILTER_STATUS_VALID {
            valid = false;
            error_message = error_message.or(evaluation.error_message);
        }
    }
    AssetScope {
        asset_id: scope.asset_id.clone(),
        filter_ids: scope.filter_ids.clone(),
        status: if valid {
            FILTER_STATUS_VALID.to_string()
        } else {
            FILTER_STATUS_INVALID.to_string()
        },
        scope_name: None,
        error_message,
    }
}
