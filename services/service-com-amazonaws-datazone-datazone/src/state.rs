// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-datazone-datazone service.
//!
//! The DataZone emulator stores every resource it serves here: a handler reads and
//! writes this struct and NOTHING else, so a create is observable by the next
//! get/list and a delete is observable by the next call.
//!
//! # Time and clock-owed transitions
//!
//! Instants are kept as milliseconds since the Unix epoch ([`now_ms`]) and rendered
//! with [`date_time`]. A domain AWS answers `CREATING` becomes `AVAILABLE`, and one it
//! answers `DELETING` disappears, once the clock passes the instant recorded in
//! `status_due_ms` — materialised by [`ComAmazonawsDatazoneDatazoneState::settle_clock`],
//! which every handler calls first. No handler ever sleeps for it.

use std::collections::{BTreeMap, VecDeque};

/// Subscription requests: their stored record and the view every operation reports one
/// with. A concern of its own, so it lives in its own module beside this one rather than
/// growing this file; everything it declares is re-exported here, so a handler reaches
/// it as `crate::state::…` the way it reaches every other part of this store.
mod subscription_requests;
pub use subscription_requests::*;

/// Subscription targets: the stored record, and the lookup that says whether a published
/// listing has one behind it. A concern of its own, so it lives in its own module beside
/// this one; everything it declares is re-exported here, so a handler reaches it as
/// `crate::state::…` like the rest of the store.
mod subscription_targets;
pub use subscription_targets::*;

/// Subscriptions: the access an accepted subscription request materialises, and the view
/// every operation that reports one renders it with. A concern of its own, so it lives in
/// its own module beside this one; everything it declares is re-exported here, so a
/// handler reaches it as `crate::state::…` like the rest of the store.
mod subscriptions;
pub use subscriptions::*;

/// A domain's data portal: where this emulator publishes it, and the page served there. A
/// concern of its own, so it lives in its own module beside this one; everything it declares
/// is re-exported here, so a handler reaches it as `crate::state::…` like the rest of the
/// store.
mod data_portal;
pub use data_portal::*;

/// What a domain create settles about its request before it stores anything — the members
/// this service requires and the roles it would have to assume — and how it words each
/// refusal. A concern of its own, so it lives in its own module beside this one; everything
/// it declares is re-exported here, so a handler reaches it as `crate::state::…` like the
/// rest of the store.
mod domain_admission;
pub use domain_admission::*;

/// WHO is asking: the principal a request's credential speaks for, the DataZone user the
/// domain holds for it, and which projects that user owns. A concern of its own, so it lives
/// in its own module beside this one; everything it declares is re-exported here, so a
/// handler reaches it as `crate::state::…` like the rest of the store.
mod requesters;
pub use requesters::*;

/// The span the emulator takes to finish a domain lifecycle step AWS reports as
/// in-progress (`CREATING` -> `AVAILABLE`, `DELETING` -> gone).
///
/// AWS does not document how long either takes, so this is a CHOSEN span. It is
/// deliberately non-zero: a caller that polls immediately after the call must still
/// observe the in-progress status the real service reports.
pub const LIFECYCLE_SETTLE_MS: i64 = 300;

/// `DomainStatus` values this emulator uses (model enum `com.amazonaws.datazone#DomainStatus`:
/// AVAILABLE | CREATING | CREATION_FAILED | DELETED | DELETING | DELETION_FAILED).
pub const DOMAIN_STATUS_CREATING: &str = "CREATING";
pub const DOMAIN_STATUS_AVAILABLE: &str = "AVAILABLE";
pub const DOMAIN_STATUS_DELETING: &str = "DELETING";

/// The `ProjectStatus` a project AWS has finished creating carries (model enum
/// `com.amazonaws.datazone#ProjectStatus`: ACTIVE | DELETING | UPDATING | DELETE_FAILED).
pub const PROJECT_STATUS_ACTIVE: &str = "ACTIVE";

/// The one `UserDesignation` that points at a designation the caller created, rather
/// than naming a role the service defines (model enum
/// `com.amazonaws.datazone#UserDesignation`: PROJECT_OWNER | PROJECT_CONTRIBUTOR |
/// PROJECT_CATALOG_VIEWER | PROJECT_CATALOG_CONSUMER | PROJECT_CATALOG_STEWARD |
/// USER_CREATED_DESIGNATION).
pub const DESIGNATION_USER_CREATED: &str = "USER_CREATED_DESIGNATION";

/// The `UserDesignation` member that makes a project member an OWNER of the project (same
/// model enum), as opposed to one of the designations that only let a member contribute to
/// it or read its catalog. What [`Project::is_owned_by`] reads.
pub const DESIGNATION_PROJECT_OWNER: &str = "PROJECT_OWNER";

/// The `SortOrder` that reverses a listing (model enum
/// `com.amazonaws.datazone#SortOrder`: ASCENDING | DESCENDING).
pub const SORT_ORDER_DESCENDING: &str = "DESCENDING";

/// The one field a listing's `sortBy` offers — the sole member of every `SortField*`
/// enum this service models (`SortFieldAccountPool`, `SortFieldConnection` and
/// `SortFieldProject` are each just NAME).
pub const SORT_FIELD_NAME: &str = "NAME";

/// The `DomainVersion` a domain gets when the request names none (model enum
/// `com.amazonaws.datazone#DomainVersion`: V1 | V2).
pub const DEFAULT_DOMAIN_VERSION: &str = "V1";

/// The `FormTypeStatus` a metadata form type gets when the request names none (model
/// enum `com.amazonaws.datazone#FormTypeStatus`: ENABLED | DISABLED). A form type is
/// usable as soon as it is created, so the default is the usable member.
pub const FORM_TYPE_STATUS_ENABLED: &str = "ENABLED";

/// `FormTypeName`'s modelled maximum length, 1..128 (smithy_query shape="FormTypeName").
pub const FORM_TYPE_NAME_MAX_LEN: usize = 128;

/// The `@pattern` the model declares on `FormTypeName`, the shape a form type's `name`
/// and a form entry's reported `typeName` both target.
///
/// SOURCE: `smithy_query(query="shape", shape="FormTypeName")`. The negative lookahead
/// is why it is checked here at all: the vendored boundary validator compiles a
/// `@pattern` with the `regex` crate, which has no look-around, and treats a pattern it
/// cannot compile as matching (`vendor/smithy-schema-server/src/constraints.rs`), so
/// this pattern would otherwise reach the store unchecked.
pub const FORM_TYPE_NAME_PATTERN: &str =
    r"^(amazon.datazone.)?(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$";

/// [`FORM_TYPE_NAME_PATTERN`], checked directly rather than compiled.
///
/// The pattern needs look-around, which the `regex` crate does not offer, and a Smithy
/// `@pattern` is an ECMA-262 expression, where `\w` is the ASCII class `[A-Za-z0-9_]`.
/// So each alternative is read off the pattern and checked in turn:
///
/// * `^_\w*[a-zA-Z0-9]\w*$` — the NAME ITSELF starts with an underscore and some ASCII
///   alphanumeric follows it. The `amazon.datazone.` prefix belongs to the other
///   alternative only, so `amazon.datazone._0` is NOT a form type name.
/// * `^(amazon.datazone.)?(?![0-9_])\w+$` — otherwise, every character is an ASCII word
///   character once the optional literal prefix is taken off, and the first of them is
///   a letter (the look-ahead rules out a digit, and an underscore there would have
///   matched the first alternative).
fn matches_form_type_name_pattern(name: &str) -> bool {
    let is_word = |byte: u8| byte.is_ascii_alphanumeric() || byte == b'_';
    if let Some(rest) = name.strip_prefix('_') {
        if rest.bytes().all(is_word) && rest.bytes().any(|byte| byte.is_ascii_alphanumeric()) {
            return true;
        }
    }
    let bare = name.strip_prefix("amazon.datazone.").unwrap_or(name);
    bare.bytes().all(is_word)
        && bare
            .bytes()
            .next()
            .is_some_and(|byte| byte.is_ascii_alphabetic())
}

/// Whether `name` satisfies `FormTypeName`'s modelled length and pattern.
///
/// Shared by every handler that takes or reports a form type name — `CreateFormType`
/// validates its `name` with it, and `CreateAssetType` decides with it whether a form
/// entry's `typeIdentifier` is already a well-formed name — so the two cannot drift.
pub fn is_form_type_name(name: &str) -> bool {
    (1..=FORM_TYPE_NAME_MAX_LEN).contains(&name.chars().count())
        && matches_form_type_name_pattern(name)
}

/// The form type NAME a `FormTypeIdentifier` resolves to.
///
/// A DataZone form type is addressed by its name: `FormTypeIdentifier` (len 1..385,
/// pattern `^(?!\.)[\w\.]*\w$`) carries that name, while `FormEntryOutput`'s `typeName`
/// is a `FormTypeName` (len 1..128, pattern
/// `^(amazon.datazone.)?(?![0-9_])\w+$|^_\w*[a-zA-Z0-9]\w*$`) — both from
/// `smithy_query(query="shape",shape="FormEntryInput,FormEntryOutput")`. So an
/// identifier that is already a well-formed name (including one in the service's own
/// `amazon.datazone.` namespace) resolves to itself, and a qualified identifier in any
/// other namespace resolves to its unqualified last segment, which is the form type's
/// own name.
///
/// Shared by every handler that has to decide whether two form type references name the
/// same form type: `CreateAssetType` stores a form entry's resolved name, and
/// `DeleteFormType` compares a stored reference against the form type being deleted — so
/// the two cannot drift. Whether a form type of that name EXISTS is a separate question:
/// the built-in `amazon.datazone.*` types are not stored at all, and an asset type or an
/// asset may name a form type in any namespace.
pub fn form_type_reference_name(identifier: &str) -> &str {
    if is_form_type_name(identifier) {
        return identifier;
    }
    identifier.split('.').next_back().unwrap_or(identifier)
}

/// The `GlossaryStatus` members (model enum `com.amazonaws.datazone#GlossaryStatus`:
/// DISABLED | ENABLED).
pub const GLOSSARY_STATUS_ENABLED: &str = "ENABLED";
pub const GLOSSARY_STATUS_DISABLED: &str = "DISABLED";

/// The `GlossaryTermStatus` members (model enum
/// `com.amazonaws.datazone#GlossaryTermStatus`: ENABLED | DISABLED).
pub const GLOSSARY_TERM_STATUS_ENABLED: &str = "ENABLED";
pub const GLOSSARY_TERM_STATUS_DISABLED: &str = "DISABLED";

/// The `Revision` (`com.amazonaws.datazone#Revision`, len 1..64) a freshly created
/// revisioned resource carries: DataZone numbers revisions from 1, as a decimal string.
pub const FIRST_REVISION: &str = "1";

/// [`FIRST_REVISION`] as the number a revision map is keyed by: revisions are numbered
/// from 1 and [`FIRST_REVISION`] is that number rendered, so the two are defined
/// together here rather than one being parsed out of the other.
pub const FIRST_REVISION_NUMBER: u32 = 1;

/// The revision number a modelled `Revision` string addresses, when it addresses one.
///
/// Addressing is EXACT: a revision is addressed by the decimal number the call that
/// recorded it returned, so only the canonical decimal resolves and `"01"` or `"+1"`
/// name no revision. The one rule every revisioned resource in this store applies
/// ([`Asset::revision`], [`FormType::revision`], [`Listing::revision`]), so they cannot
/// read one revision string differently.
pub fn revision_number(revision: &str) -> Option<u32> {
    let number = revision.parse::<u32>().ok()?;
    (number.to_string() == revision).then_some(number)
}

/// The `ListingStatus` members (model enum `com.amazonaws.datazone#ListingStatus`:
/// CREATING | ACTIVE | INACTIVE).
pub const LISTING_STATUS_CREATING: &str = "CREATING";
pub const LISTING_STATUS_ACTIVE: &str = "ACTIVE";
pub const LISTING_STATUS_INACTIVE: &str = "INACTIVE";

/// The `ChangeAction` members (model enum `com.amazonaws.datazone#ChangeAction`:
/// PUBLISH | UNPUBLISH) — the two change sets `CreateListingChangeSet` applies.
pub const CHANGE_ACTION_PUBLISH: &str = "PUBLISH";
pub const CHANGE_ACTION_UNPUBLISH: &str = "UNPUBLISH";

/// The `EntityType` members (model enum `com.amazonaws.datazone#EntityType`:
/// ASSET | DATA_PRODUCT) — what a listing can be a record OF.
pub const ENTITY_TYPE_ASSET: &str = "ASSET";
pub const ENTITY_TYPE_DATA_PRODUCT: &str = "DATA_PRODUCT";

/// The only `DataZoneEntityType` member (model enum
/// `com.amazonaws.datazone#DataZoneEntityType`: DOMAIN_UNIT) — the kind of entity
/// `AddEntityOwner`, `ListEntityOwners` and `RemoveEntityOwner` work on, so an owner is
/// kept on a stored domain unit.
pub const DATA_ZONE_ENTITY_TYPE_DOMAIN_UNIT: &str = "DOMAIN_UNIT";

/// The `listingRevision` that names a listing's newest revision: the `@default` the
/// model puts on `GetListingInput::listingRevision` (smithy_query operation="GetListing").
pub const LISTING_REVISION_LATEST: &str = "latest";

/// `ListingName`'s modelled maximum length, 1..64 (smithy_query shape="ListingName").
pub const LISTING_NAME_MAX_LEN: usize = 64;

/// Milliseconds since the Unix epoch, now.
pub fn now_ms() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map_or(0, |d| i64::try_from(d.as_millis()).unwrap_or(i64::MAX))
}

/// Render an epoch-millisecond instant as the modelled timestamp type, at the
/// WHOLE-SECOND resolution this service's responses report an instant in.
///
/// Every timestamp this service reports targets a plain `timestamp` with no
/// `@timestampFormat` override (`CreatedAt`, `UpdatedAt`, `CreationTimestamp`, …), so
/// restJson1 puts it on the wire as epoch seconds — and DataZone reports a whole number
/// of seconds there, never a sub-second tail. The stored instant keeps its millisecond
/// precision (so orderings inside one second survive in state); only the RENDERED value
/// is truncated, towards the epoch-second the instant falls in, so a reported instant is
/// never later than the instant itself.
pub fn date_time(ms: i64) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from_secs(ms.div_euclid(1000))
}

/// One stored DataZone domain, with the projects nested under it.
#[derive(Debug, Clone)]
pub struct Domain {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub description: Option<String>,
    pub single_sign_on: Option<StoredSingleSignOn>,
    pub domain_execution_role: String,
    pub service_role: Option<String>,
    pub kms_key_identifier: Option<String>,
    pub status: String,
    pub portal_url: Option<String>,
    pub domain_version: String,
    pub root_domain_unit_id: String,
    pub managed_account_id: String,
    pub tags: BTreeMap<String, String>,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    /// When the status this domain currently reports stops being true: the instant
    /// `CREATING` becomes `AVAILABLE`, or `DELETING` becomes gone. `None` for a
    /// settled domain.
    pub status_due_ms: Option<i64>,
    /// The projects stored in this domain, so they go with it when it is deleted.
    /// Bounded by [`MAX_PROJECTS_PER_DOMAIN`].
    pub projects: BTreeMap<String, Project>,
    /// The metadata form types stored in this domain, keyed by their `FormTypeName`
    /// — the same string `GetFormType`/`DeleteFormType` address them by
    /// (`formTypeIdentifier`). Nested under the domain so they go with it when it is
    /// deleted. Bounded by [`MAX_FORM_TYPES_PER_DOMAIN`].
    pub form_types: BTreeMap<String, FormType>,
    /// The catalog assets stored in this domain, keyed by their `AssetId`, so they go
    /// with the domain when it is deleted. Bounded by [`MAX_ASSETS_PER_DOMAIN`].
    pub assets: BTreeMap<String, Asset>,
    /// The policy grants stored in this domain, keyed by `grantId`, so they go with it
    /// when it is deleted. A `BTreeMap` so every `ListPolicyGrants` iterates in one
    /// stable order and a pagination token can resume from a key. Bounded by
    /// [`MAX_POLICY_GRANTS_PER_DOMAIN`].
    pub policy_grants: BTreeMap<String, PolicyGrant>,
    /// The business glossaries stored in this domain, keyed by glossary id, so they go
    /// with the domain when it is deleted. Bounded by [`MAX_GLOSSARIES_PER_DOMAIN`].
    pub glossaries: BTreeMap<String, Glossary>,
    /// The business glossary TERMS stored in this domain, keyed by term id — the
    /// `GlossaryTermId` `GetGlossaryTerm`/`DeleteGlossaryTerm` address a term by, which
    /// names no glossary, so the terms of every glossary in the domain share one
    /// namespace and each term records the glossary it belongs to
    /// ([`GlossaryTerm::glossary_id`]). Nested under the domain so they go with it when
    /// it is deleted. Bounded by [`MAX_GLOSSARY_TERMS_PER_DOMAIN`].
    pub glossary_terms: BTreeMap<String, GlossaryTerm>,
    /// The custom asset types stored in this domain, keyed by the type's NAME — the
    /// `AssetTypeIdentifier` `GetAssetType`/`DeleteAssetType` address a type by — so a
    /// name is unique within the domain and the types go with it when it is deleted.
    /// Bounded by [`MAX_ASSET_TYPES_PER_DOMAIN`].
    pub asset_types: BTreeMap<String, AssetType>,
    /// The domain units stored in this domain, keyed by id, so they go with it when it
    /// is deleted. Always holds at least the ROOT unit `root_domain_unit_id` names,
    /// which is created with the domain itself. Bounded by
    /// [`MAX_DOMAIN_UNITS_PER_DOMAIN`].
    pub domain_units: BTreeMap<String, DomainUnit>,
    /// The ids of the domain units this domain has DELETED, oldest first.
    ///
    /// A domain unit's owners are the grants held on it, and they go with the unit, so
    /// once `DeleteDomainUnit` has removed one, nothing is held on that id any more and
    /// a call addressing it is denied rather than answered with an existence report —
    /// see [`not_permitted_or_absent_message`]. Bounded by
    /// [`MAX_DELETED_DOMAIN_UNITS_PER_DOMAIN`] through
    /// [`Domain::record_deleted_domain_unit`], so a create/delete loop cannot grow the
    /// emulator's heap without limit.
    pub deleted_domain_unit_ids: VecDeque<String>,
    /// The catalog listings stored in this domain, keyed by their `ListingId` — the
    /// identifier `GetListing`/`DeleteListing` address one by — so they go with the
    /// domain when it is deleted. Bounded by [`MAX_LISTINGS_PER_DOMAIN`].
    pub listings: BTreeMap<String, Listing>,
    /// The account pools stored in this domain, keyed by their `AccountPoolId` — the
    /// identifier `GetAccountPool` addresses one by — so they go with the domain when
    /// it is deleted. Bounded by [`MAX_ACCOUNT_POOLS_PER_DOMAIN`].
    pub account_pools: BTreeMap<String, AccountPool>,
    /// The user profiles stored in this domain, keyed by [`user_profile_key`] — the
    /// identity the request names, not the minted id — so the profile of one user is
    /// reached by the `userIdentifier` every user-profile operation addresses it with.
    /// Nested under the domain so they go with it when it is deleted. Bounded by
    /// [`MAX_USER_PROFILES_PER_DOMAIN`].
    pub user_profiles: BTreeMap<String, UserProfile>,
    /// The subscription requests made in this domain, keyed by their
    /// `SubscriptionRequestId` — the identifier `GetSubscriptionRequestDetails` addresses
    /// one by — so they go with the domain when it is deleted. A `BTreeMap` so every
    /// listing of them iterates in one stable order and a pagination token can resume
    /// from a key. Bounded by [`MAX_SUBSCRIPTION_REQUESTS_PER_DOMAIN`].
    pub subscription_requests: BTreeMap<String, SubscriptionRequest>,
    /// The subscriptions of this domain, keyed by their `SubscriptionId` — the identifier
    /// `GetSubscription` addresses one by — so they go with the domain when it is
    /// deleted. A `BTreeMap` so every listing of them iterates in one stable order and a
    /// pagination token can resume from a key.
    ///
    /// Bounded without a ceiling of its own: a subscription is materialised only by a
    /// decision that ACCEPTS a subscription request, and a request takes at most one
    /// decision ([`SubscriptionRequest::awaits_a_decision`]), so this map holds at most
    /// one entry per stored request and inherits
    /// [`MAX_SUBSCRIPTION_REQUESTS_PER_DOMAIN`].
    pub subscriptions: BTreeMap<String, Subscription>,
    /// The subscription targets of this domain, keyed by their `SubscriptionTargetId` —
    /// the identifier `GetSubscriptionTarget` addresses one by — so they go with the
    /// domain when it is deleted.
    pub subscription_targets: BTreeMap<String, SubscriptionTarget>,
}

impl Domain {
    /// The listing that records the entity `entity_type`/`entity_id`, when that entity
    /// has been published in this domain's catalog.
    ///
    /// At most one listing records one entity: a listing IS the entity's record in the
    /// catalog, and `CreateListingChangeSet` adds a revision to that record rather than
    /// a second listing. A linear scan, deliberately: the map is bounded by
    /// [`MAX_LISTINGS_PER_DOMAIN`], so one lookup rule shared by every handler is worth
    /// more than a second index to keep in step (the same trade
    /// [`ComAmazonawsDatazoneDatazoneState::live_domain_named`] makes).
    pub fn listing_for_entity(&self, entity_type: &str, entity_id: &str) -> Option<&Listing> {
        self.listings
            .values()
            .find(|listing| listing.entity_type == entity_type && listing.entity_id == entity_id)
    }

    /// [`Self::listing_for_entity`], for a change set that writes the listing.
    pub fn listing_for_entity_mut(
        &mut self,
        entity_type: &str,
        entity_id: &str,
    ) -> Option<&mut Listing> {
        self.listings
            .values_mut()
            .find(|listing| listing.entity_type == entity_type && listing.entity_id == entity_id)
    }

    /// The DIRECT child of the domain unit `parent_id` that already carries `name`,
    /// when this domain holds one.
    ///
    /// A domain unit's name is unique among the children of ONE parent, so the same
    /// name under a different parent — or in another domain — is not a collision. This
    /// is the lookup a create refuses on, and a rename compares the clash's id with its
    /// own so renaming a unit to the name it already has is not a self-collision. The
    /// comparison is exact: `DomainUnitName` admits both cases and nothing folds them.
    ///
    /// A linear scan, deliberately: `domain_units` is bounded by
    /// [`MAX_DOMAIN_UNITS_PER_DOMAIN`], so one lookup rule shared by every handler is
    /// worth more than a second index to keep in step (the same trade
    /// [`Self::listing_for_entity`] makes). The walk is over a `BTreeMap`, so the clash
    /// reported for a parent holding several is the same one on every call.
    pub fn child_domain_unit_named(&self, parent_id: &str, name: &str) -> Option<&DomainUnit> {
        self.domain_units.values().find(|unit| {
            unit.name == name && unit.parent_domain_unit_id.as_deref() == Some(parent_id)
        })
    }

    /// The object stored in this domain that binds a metadata form to the form type
    /// `form_type_name` and so prevents its deletion, when one does.
    ///
    /// An asset type's forms carry the form type a form of that type must satisfy, and
    /// an asset's forms carry the form type the form's content is an instance of. Both
    /// name it by `FormTypeIdentifier`, so each reference is resolved to a form type
    /// name with [`form_type_reference_name`] before it is compared. Asset types are
    /// looked at first — in name order, then assets in id order — so the dependency
    /// reported is the same one on every call: the asset type that links the form type
    /// where one does, and otherwise the asset that uses it. `None` exactly when nothing
    /// in this domain depends on that form type, which is what lets `DeleteFormType`
    /// remove it. A linear scan over maps bounded by [`MAX_ASSET_TYPES_PER_DOMAIN`] and
    /// [`MAX_ASSETS_PER_DOMAIN`], the same trade [`Self::listing_for_entity`] makes.
    pub fn form_type_blocker(&self, form_type_name: &str) -> Option<DeletionBlocker> {
        let binds_it = |reference: &str| form_type_reference_name(reference) == form_type_name;
        let blocking_type = self
            .asset_types
            .values()
            .find(|asset_type| {
                asset_type
                    .forms
                    .values()
                    .any(|form| binds_it(&form.type_name))
            })
            .map(|asset_type| DeletionBlocker {
                tag: GRAPH_TAG_ASSET,
                identifier: asset_type.name.clone(),
            });
        blocking_type.or_else(|| {
            self.assets
                .values()
                .find(|asset| {
                    // EVERY recorded revision counts: a revision is a record this store
                    // still serves (`GetAsset` reads one by number), so a form type one
                    // of them binds is still in use even when the newest revision
                    // dropped the form.
                    asset
                        .revisions
                        .values()
                        .flat_map(|revision| revision.forms.iter())
                        .any(|form| form.type_name.as_deref().is_some_and(binds_it))
                })
                .map(|asset| DeletionBlocker {
                    tag: GRAPH_TAG_ASSET,
                    identifier: asset.id.clone(),
                })
        })
    }

    /// The object stored in this domain that is still OF the asset type
    /// `asset_type_name` and so prevents that type's deletion, or `None` when nothing
    /// is.
    ///
    /// "The asset type must not be in use (e.g., assigned to any asset). If used,
    /// deletion will fail." (smithy_query query="doc" shape="DeleteAssetType"): what an
    /// asset type is assigned to is an ASSET of it, which records the type it was
    /// created with ([`Asset::type_identifier`]) — the only place an asset type is
    /// referenced outside the type itself. The reference is matched EXACTLY, the way
    /// [`asset_type_revision`] resolves an asset's type and `GetAssetType` reads one: an
    /// asset type is addressed by the whole name it is stored under, and `TypeName`
    /// admits the dots a qualified name carries, so there is no shorter name to resolve
    /// a reference to. Every asset counts whichever revision it is at — an asset's type
    /// identifies the asset itself and no revision changes it
    /// (`CreateAssetRevisionInput` names no `typeIdentifier`).
    ///
    /// A walk over a `BTreeMap`, so the asset reported for a type several assets are of
    /// is the same one on every call. `None` exactly when no asset in this domain is of
    /// that type, which is what lets `DeleteAssetType` remove it. A linear scan over a
    /// map bounded by [`MAX_ASSETS_PER_DOMAIN`], the same trade
    /// [`Self::form_type_blocker`] makes.
    pub fn asset_type_blocker(&self, asset_type_name: &str) -> Option<DeletionBlocker> {
        self.assets
            .values()
            .find(|asset| asset.type_identifier == asset_type_name)
            .map(|asset| DeletionBlocker {
                tag: GRAPH_TAG_ASSET,
                identifier: asset.id.clone(),
            })
    }

    /// The user profile this domain holds for the identity a user-profile request
    /// names: the `userIdentifier` (and, for a role session, the `sessionName`) it was
    /// created with, or — when the domain holds no profile under that identity — the
    /// profile whose own minted `UserProfileId` is that identifier, since a profile is
    /// addressable by the id its create reported as well as by the identity it is for.
    ///
    /// `profile_type` is the optional `type` a reader narrows by: a profile of another
    /// type is not the one that was asked for.
    pub fn user_profile(
        &self,
        user_identifier: &str,
        session_name: Option<&str>,
        profile_type: Option<&str>,
    ) -> Option<&UserProfile> {
        let matches_type =
            |profile: &UserProfile| profile_type.is_none_or(|t| profile.profile_type == t);
        self.user_profiles
            .get(&user_profile_key(user_identifier, session_name))
            .filter(|profile| matches_type(profile))
            .or_else(|| {
                // A linear scan over a map bounded by [`MAX_USER_PROFILES_PER_DOMAIN`]
                // — the trade [`Domain::listing_for_entity`] makes: one lookup rule
                // every handler shares, rather than a second index to keep in step.
                self.user_profiles
                    .values()
                    .find(|profile| profile.id == user_identifier && matches_type(profile))
            })
    }

    /// The object stored in this domain that still hangs UNDER the domain unit
    /// `unit_id` and so prevents its deletion, named the way a deletion refusal names
    /// it ([`hierarchy_object`]), or `None` when nothing does.
    ///
    /// A domain unit is a node of the domain's ownership hierarchy, so what depends on
    /// it is what hangs under it: the child domain units whose parent it is, the
    /// projects assigned to it, and — for the domain's ROOT unit — the domain itself,
    /// which names that unit in `rootDomainUnitId` and so is never left without it.
    /// Child units are looked at first, then projects, each walk over a `BTreeMap`, so
    /// the dependency reported for a unit several objects hang under is the same one on
    /// every call. `None` exactly when nothing in this domain hangs under the unit,
    /// which is what lets `DeleteDomainUnit` remove it. Linear scans over maps bounded
    /// by [`MAX_DOMAIN_UNITS_PER_DOMAIN`] and [`MAX_PROJECTS_PER_DOMAIN`], the same
    /// trade [`Self::listing_for_entity`] makes.
    pub fn domain_unit_blocker(&self, unit_id: &str) -> Option<String> {
        if self.root_domain_unit_id == unit_id {
            return Some(hierarchy_object(HIERARCHY_KIND_DOMAIN, &self.id));
        }
        self.domain_units
            .values()
            .find(|unit| unit.parent_domain_unit_id.as_deref() == Some(unit_id))
            .map(|unit| hierarchy_object(HIERARCHY_KIND_DOMAIN_UNIT, &unit.id))
            .or_else(|| {
                self.projects
                    .values()
                    .find(|project| project.domain_unit_id.as_deref() == Some(unit_id))
                    .map(|project| hierarchy_object(HIERARCHY_KIND_PROJECT, &project.id))
            })
    }

    /// Whether any glossary TERM stored in this domain is still an entry of the glossary
    /// `glossary_id`, which is what prevents that glossary's deletion.
    ///
    /// What depends on a glossary is what is filed UNDER it: its terms, each of which
    /// records the glossary it is an entry of ([`GlossaryTerm::glossary_id`]). Nothing
    /// else stored here references a glossary — an asset's or a project's
    /// `glossaryTerms` name TERMS, which this same walk already covers. A linear scan
    /// over a map bounded by [`MAX_GLOSSARY_TERMS_PER_DOMAIN`], the same trade
    /// [`Self::domain_unit_blocker`] makes.
    pub fn glossary_has_terms(&self, glossary_id: &str) -> bool {
        self.glossary_terms
            .values()
            .any(|term| term.glossary_id == glossary_id)
    }

    /// The kind of object stored in this domain that is still linked to the glossary term
    /// `term_id` and so prevents its deletion, when one is.
    ///
    /// What is linked to a term is whatever stored object NAMES it: another term that
    /// relates to it ([`StoredTermRelations`], the `isA`/`classifies` relations a term
    /// holds to other terms of the domain), an asset whose metadata attaches it
    /// ([`AssetRevision::glossary_terms`]) and a project that attaches it
    /// ([`Project::glossary_terms`]) — the three places a `GlossaryTermId` is stored
    /// outside the term itself. EVERY recorded asset revision counts, for the reason
    /// [`Self::form_type_blocker`] states: a revision is a record this store still serves,
    /// so a term one of them attaches is still in use even when the newest revision
    /// dropped it.
    ///
    /// The kinds are looked at in a fixed order, so the link reported for a term several
    /// objects name is the same one on every call. `None` exactly when nothing in this
    /// domain names the term, which is what lets `DeleteGlossaryTerm` remove it. Linear
    /// scans over maps bounded by [`MAX_GLOSSARY_TERMS_PER_DOMAIN`],
    /// [`MAX_ASSETS_PER_DOMAIN`] and [`MAX_PROJECTS_PER_DOMAIN`], the same trade
    /// [`Self::listing_for_entity`] makes.
    pub fn glossary_term_link(&self, term_id: &str) -> Option<TermLinkKind> {
        let names_it = |candidate: &String| candidate == term_id;
        let related = self.glossary_terms.values().any(|other| {
            other.id != term_id
                && other
                    .term_relations
                    .related_term_ids()
                    .any(|related| related == term_id)
        });
        if related {
            return Some(TermLinkKind::GlossaryTerm);
        }
        let attached_to_asset = self.assets.values().any(|asset| {
            asset
                .revisions
                .values()
                .flat_map(|revision| revision.glossary_terms.iter())
                .any(names_it)
        });
        if attached_to_asset {
            return Some(TermLinkKind::Asset);
        }
        self.projects
            .values()
            .any(|project| project.glossary_terms.iter().any(names_it))
            .then_some(TermLinkKind::Project)
    }

    /// Remember that the domain unit `unit_id` was deleted from this domain.
    ///
    /// The ids are kept oldest first and bounded by
    /// [`MAX_DELETED_DOMAIN_UNITS_PER_DOMAIN`]: past the bound the oldest id is
    /// forgotten, so a create/delete loop cannot grow the store without limit.
    pub fn record_deleted_domain_unit(&mut self, unit_id: String) {
        if self.has_deleted_domain_unit(&unit_id) {
            return;
        }
        while self.deleted_domain_unit_ids.len() >= MAX_DELETED_DOMAIN_UNITS_PER_DOMAIN {
            self.deleted_domain_unit_ids.pop_front();
        }
        self.deleted_domain_unit_ids.push_back(unit_id);
    }

    /// Whether the domain unit `unit_id` was deleted from this domain.
    pub fn has_deleted_domain_unit(&self, unit_id: &str) -> bool {
        self.deleted_domain_unit_ids.iter().any(|id| id == unit_id)
    }
}

/// The kind name a DOMAIN UNIT carries where a deletion refusal names it.
///
/// SOURCE: the recorded `ConflictException` for `DeleteDomainUnit`, which names both the
/// unit being deleted and the child unit that prevented it `domainUnit <domainUnitId>`.
pub const HIERARCHY_KIND_DOMAIN_UNIT: &str = "domainUnit";

/// The kind name a PROJECT carries there: the same form, with the API's own type name
/// for the kind.
pub const HIERARCHY_KIND_PROJECT: &str = "project";

/// The kind name a DOMAIN carries there — what a domain's root unit is held by.
pub const HIERARCHY_KIND_DOMAIN: &str = "domain";

/// The `ConflictException` message DataZone answers a `DeleteGlossary` whose glossary
/// still has terms filed under it ([`Domain::glossary_has_terms`]).
///
/// Recorded behaviour, and FIXED text: the service names neither the glossary nor the
/// term, so nothing the caller sent and nothing from the store appears in it.
pub const GLOSSARY_HAS_TERMS_MESSAGE: &str =
    "Glossary can't be deleted while having glossary terms attached";

/// The `ConflictException` message DataZone answers a `DeleteGlossary` whose glossary is
/// not in DISABLED state yet — the other prerequisite the operation documents.
///
/// Worded in the form [`GLOSSARY_HAS_TERMS_MESSAGE`] takes, with the one variable part
/// read from the STORE: the `GlossaryStatus` the glossary is actually in.
pub fn glossary_not_disabled_message(status: &str) -> String {
    format!("Glossary can't be deleted while it is in {status} state")
}

/// The kind of stored object that is linked to a glossary term and so prevents its
/// deletion ([`Domain::glossary_term_link`]).
///
/// A closed set, so a refusal names a kind this service defines and never an identifier
/// the caller sent.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TermLinkKind {
    /// Another glossary term of the domain relates to this one.
    GlossaryTerm,
    /// An asset of the domain attaches this term at one of its revisions.
    Asset,
    /// A project of the domain attaches this term.
    Project,
}

impl TermLinkKind {
    /// The linked objects as a deletion refusal names them, in the plural the wording
    /// takes.
    pub const fn attached_kind(self) -> &'static str {
        match self {
            Self::GlossaryTerm => "glossary terms",
            Self::Asset => "assets",
            Self::Project => "projects",
        }
    }
}

/// The `ConflictException` message DataZone answers a `DeleteGlossaryTerm` whose term is
/// still linked to another stored object — "The term must not be linked to other assets
/// or child terms" (smithy_query doc shape="DeleteGlossaryTerm").
///
/// Worded in the form [`GLOSSARY_HAS_TERMS_MESSAGE`] takes — the service's own wording
/// for a delete a stored dependency prevents in this resource family — with the one
/// variable part read from the STORE: the kind of object that is still linked
/// ([`TermLinkKind::attached_kind`]). Neither the term nor the linked object is named, so
/// nothing the caller sent appears in it.
pub fn glossary_term_linked_message(link: TermLinkKind) -> String {
    format!(
        "Glossary term can't be deleted while having {} attached",
        link.attached_kind()
    )
}

/// One object of a domain's ownership hierarchy as a deletion refusal names it:
/// `<kind> <identifier>`, the kind followed by the object's own identifier within the
/// domain (see [`HIERARCHY_KIND_DOMAIN_UNIT`] for where the form comes from). Distinct
/// from [`graph_object`], the form the asset/form-type dependency graph names its
/// objects by.
pub fn hierarchy_object(kind: &str, identifier: &str) -> String {
    format!("{kind} {identifier}")
}

/// One stored object whose dependency on another object prevents that object's deletion
/// — what [`Domain::form_type_blocker`] reports and [`deletion_prevented_message`] names
/// in the refusal.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeletionBlocker {
    /// The tag naming this object's kind in the dependency graph: one of the
    /// `GRAPH_TAG_*` constants.
    pub tag: &'static str,
    /// The object's own identifier within its domain.
    pub identifier: String,
}

/// The dependency-graph tag a metadata form type is named by in a deletion refusal.
///
/// SOURCE: the recorded `ConflictException` wording, which names the form type being
/// deleted `type Form#<domainId>#<formTypeName>`.
pub const GRAPH_TAG_FORM_TYPE: &str = "Form";

/// The dependency-graph tag an asset-domain object — an asset type, an asset of one, or
/// a filter that is a view of an asset — is named by in a deletion refusal.
///
/// SOURCE: the same recorded wording, which names the asset type that prevented a form
/// type's deletion `type asset#<domainId>#<assetTypeName>`. The third field is the
/// object's own identifier, which is what tells one asset-domain object from another.
pub const GRAPH_TAG_ASSET: &str = "asset";

/// One object as a deletion refusal names it: `type <tag>#<domainId>#<identifier>`.
pub fn graph_object(tag: &str, domain_id: &str, identifier: &str) -> String {
    format!("type {tag}#{domain_id}#{identifier}")
}

/// The `ConflictException` message DataZone answers a delete a stored dependency
/// prevents: the object that could not be deleted, and the object that prevented it.
///
/// Built from the request and the store, never from a recorded value: the wording is
/// recorded behaviour ("The following objects could not be deleted: `<object>` deletion
/// prevented by `<blocker>`. ", ending in a period and a space), while both objects in it
/// are named by [`graph_object`] from the domain, kind and identifier the call actually
/// found.
pub fn deletion_prevented_message(object: &str, blocker: &str) -> String {
    format!("The following objects could not be deleted: {object} deletion prevented by {blocker}. ")
}

/// One stored catalog listing: an entity published in the domain's catalog.
///
/// A listing is "a record of an asset at a given time" (smithy_query
/// operation="CreateListingChangeSet"), so each PUBLISH change set records a further
/// [`ListingRevision`] — a snapshot of the published entity as it was at that instant —
/// while the listing itself keeps the identity (`id`) the catalog addresses it by.
/// An UNPUBLISH change set takes the record out of the catalog by moving `status` to
/// [`LISTING_STATUS_INACTIVE`]; it adds no snapshot, because nothing about the entity
/// was recorded.
#[derive(Debug, Clone)]
pub struct Listing {
    pub id: String,
    pub domain_id: String,
    /// Which kind of entity this listing records: one of [`ENTITY_TYPE_ASSET`] /
    /// [`ENTITY_TYPE_DATA_PRODUCT`].
    pub entity_type: String,
    /// The id of the entity this listing records, within the same domain.
    pub entity_id: String,
    /// One of [`LISTING_STATUS_CREATING`] / [`LISTING_STATUS_ACTIVE`] /
    /// [`LISTING_STATUS_INACTIVE`].
    pub status: String,
    /// When a [`LISTING_STATUS_CREATING`] listing becomes [`LISTING_STATUS_ACTIVE`].
    /// `None` for a settled listing — one that is ACTIVE or INACTIVE.
    pub status_due_ms: Option<i64>,
    /// The caller and instant of the change set that FIRST published this entity.
    pub created_by: String,
    pub created_at_ms: i64,
    /// The caller and instant of the LATEST change set applied to this listing.
    pub updated_by: String,
    pub updated_at_ms: i64,
    /// Every recorded revision, keyed by its number so the greatest key is the latest.
    /// Bounded by [`MAX_LISTING_REVISIONS`].
    pub revisions: BTreeMap<u32, ListingRevision>,
    /// The `clientToken` of the UNPUBLISH change set that last deactivated this
    /// listing, so a replay of that request is answered rather than re-applied.
    pub unpublish_client_token: Option<String>,
}

impl Listing {
    /// The greatest revision number recorded, i.e. the one a `GetListing` that asks for
    /// [`LISTING_REVISION_LATEST`] reports.
    pub fn latest_revision(&self) -> Option<&ListingRevision> {
        self.revisions.values().next_back()
    }

    /// The revision addressed by a `listingRevision` string, by the rule
    /// [`revision_number`] states.
    pub fn revision(&self, revision: &str) -> Option<&ListingRevision> {
        self.revisions.get(&revision_number(revision)?)
    }

    /// The number the NEXT recorded revision gets: one past the greatest ever recorded,
    /// so revision numbers never go backwards.
    pub fn next_revision_number(&self) -> u32 {
        self.revisions.keys().next_back().copied().unwrap_or(0) + 1
    }

    /// The revision carrying `token`, if a PUBLISH change set with that `clientToken`
    /// already recorded one of this listing's revisions.
    pub fn revision_with_token(&self, token: &str) -> Option<&ListingRevision> {
        self.revisions
            .values()
            .find(|revision| revision.client_token.as_deref() == Some(token))
    }

    /// Finish the publish this listing owed the clock, as of the instant `due_at` it was
    /// due at: the listing enters the catalog, and the catalog's own record of the
    /// published entity is recorded as its next revision.
    ///
    /// A publish records TWO revisions of a listing — recorded behaviour: the change set
    /// that starts it records the snapshot it was asked to publish and answers with that
    /// revision, and the catalog records the entity again when the publish lands, so a
    /// listing read once its publish has completed reports one revision more than the
    /// change set reported. The second revision is the SAME snapshot (the entity as the
    /// change set found it; a listing is a record of the entity at a given time), minus
    /// the change set's `clientToken`, which stays on the revision that call answered
    /// with so a replay of it is still answered with that revision.
    ///
    /// Bounded by [`MAX_LISTING_REVISIONS`], like the change set's own record: at the
    /// bound the listing still enters the catalog, keeping the revisions it has.
    pub fn complete_publish(&mut self, due_at: i64) {
        self.status = LISTING_STATUS_ACTIVE.to_string();
        self.status_due_ms = None;
        let published = self.latest_revision().cloned();
        let Some(published) = published.filter(|_| self.revisions.len() < MAX_LISTING_REVISIONS)
        else {
            return;
        };
        let revision = self.next_revision_number();
        self.revisions.insert(
            revision,
            ListingRevision {
                revision,
                created_at_ms: due_at,
                client_token: None,
                ..published
            },
        );
        self.updated_at_ms = due_at;
    }
}

/// One recorded revision of a listing: the published entity as it was when the PUBLISH
/// change set that recorded it ran.
///
/// The snapshot is taken at publish time deliberately — a listing is a record of the
/// entity AT A GIVEN TIME, so a later change to the entity does not rewrite a revision
/// the catalog already holds.
#[derive(Debug, Clone)]
pub struct ListingRevision {
    /// The revision number, rendered as the modelled `Revision` string: the first one
    /// recorded for a listing renders as [`FIRST_REVISION`].
    pub revision: u32,
    /// The revision of the ENTITY this snapshot records.
    pub entity_revision: String,
    /// The entity's name, clipped to [`LISTING_NAME_MAX_LEN`] by [`listing_name`].
    pub name: String,
    pub description: Option<String>,
    /// The entity's own type — an asset's `typeIdentifier`, reported as the listing
    /// item's `assetType`.
    pub entity_type_name: String,
    /// The project that owns the published entity.
    pub owning_project_id: String,
    /// When the published ENTITY was created, which is what a listing item's
    /// `createdAt` reports (smithy_query shape="AssetListing" member="createdAt").
    pub entity_created_at_ms: i64,
    /// The entity's metadata forms AS THEY WERE when this revision was recorded: the
    /// snapshot every form member of a listing item is projected from — the modelled
    /// `Forms` string by [`listing_forms`] and the per-form metadata by
    /// [`form_metadata`]. Empty when the entity had no form attached.
    pub forms: Vec<StoredForm>,
    /// The glossary terms attached to the published entity, resolved against the
    /// domain's stored terms when this revision was recorded
    /// ([`listing_glossary_terms`]). Empty exactly when the entity had none this store
    /// holds a term for, which is what lets a listing item report an empty list rather
    /// than nothing at all.
    pub glossary_terms: Vec<ListingGlossaryTerm>,
    /// The caller and instant of the change set that recorded this revision.
    pub created_by: String,
    pub created_at_ms: i64,
    /// That change set's `clientToken`, so a replay of it is answered with this
    /// revision rather than recording a second one.
    pub client_token: Option<String>,
}

/// An entity's name as the listing that records it reports it.
///
/// `ListingName` is 1..64 while the name of a publishable entity is longer-ranged
/// (`AssetName` is 1..256, smithy_query shape="AssetName"), so a name that does not fit
/// the listing's own modelled bound is clipped to it — on a CHARACTER boundary, so the
/// result is still valid UTF-8 — rather than reported in violation of the model.
pub fn listing_name(entity_name: &str) -> String {
    entity_name.chars().take(LISTING_NAME_MAX_LEN).collect()
}

/// An entity's metadata forms as the modelled `Forms` string a listing reports.
///
/// `Forms` is a plain string (smithy_query shape="Forms") and a listing reports the
/// forms attached to the published entity, so they are rendered as ONE JSON object
/// keyed by `formName` — the name a form is addressed by on its entity — whose value is
/// that form's content. Content that is itself JSON nests as that JSON, so the
/// rendering round-trips through a parser instead of double-encoding; content that is
/// not JSON stays a JSON string, and a form with no content renders as `null`.
///
/// `None` when the entity carries no form at all: there is then no object to report.
pub fn listing_forms(forms: &[StoredForm]) -> Option<String> {
    if forms.is_empty() {
        return None;
    }
    let mut rendered = serde_json::Map::new();
    for form in forms {
        let value = match form.content.as_deref() {
            None => serde_json::Value::Null,
            Some(content) => serde_json::from_str(content)
                .unwrap_or_else(|_| serde_json::Value::String(content.to_string())),
        };
        rendered.insert(form.form_name.clone(), value);
    }
    serde_json::to_string(&serde_json::Value::Object(rendered)).ok()
}

/// Whether a metadata form is shown collapsed, as a [`form_output`] reports it.
///
/// No request member carries a rendering configuration — a `FormInput` is
/// `formName`/`typeIdentifier`/`typeRevision`/`content` (smithy_query shape="FormInput")
/// — so every form this store holds is rendered the one way, expanded.
pub const FORM_RENDERED_COLLAPSED: bool = false;

/// One stored metadata form as the modelled `FormOutput`, shared by every operation that
/// reports an entity's forms so no two of them can drift apart.
///
/// `renderingConfig` is reported on EVERY form output — recorded behaviour, carrying the
/// one value this store has for it ([`FORM_RENDERED_COLLAPSED`]) — so a client that
/// renders metadata forms can always read whether a form is collapsed instead of finding
/// nothing there.
pub fn form_output(form: &StoredForm) -> crate::ops::types::FormOutput {
    crate::ops::types::FormOutput {
        form_name: form.form_name.clone(),
        type_name: form.type_name.clone(),
        type_revision: form.type_revision.clone(),
        content: form.content.clone(),
        rendering_config: Some(crate::ops::types::FormRenderingConfig {
            collapse: Some(FORM_RENDERED_COLLAPSED),
        }),
    }
}

/// [`form_output`] without the form's CONTENT: the metadata of one form — which form it
/// is, of which form type at which revision, and how it renders.
///
/// What a published entity's `assetFormsMetadata` reports, where the content of the same
/// forms is already reported once, as the `forms` string [`listing_forms`] renders.
pub fn form_metadata(form: &StoredForm) -> crate::ops::types::FormOutput {
    crate::ops::types::FormOutput {
        content: None,
        ..form_output(form)
    }
}

/// One glossary term a listing recorded: the term's reportable detail, snapshotted when
/// the change set that published the entity ran.
///
/// A `DetailedGlossaryTerm` is a term's NAME and short description rather than its id
/// (smithy_query shape="DetailedGlossaryTerm"), and both belong to the term's own stored
/// record, so they are resolved once — at publish — and kept, because a listing is the
/// published entity AS IT WAS.
#[derive(Debug, Clone)]
pub struct ListingGlossaryTerm {
    pub name: String,
    pub short_description: Option<String>,
}

/// The glossary terms a listing records for an entity whose revision attached
/// `term_ids`, resolved against the terms `domain` stores now.
///
/// An asset attaches terms BY ID (`GlossaryTerms` is a list of `GlossaryTermId`), and a
/// listing reports each term's name and short description, so an id this domain holds no
/// term for carries nothing to report and is left out: what the listing records is the
/// terms it could resolve, in the order the entity attached them.
pub fn listing_glossary_terms(domain: &Domain, term_ids: &[String]) -> Vec<ListingGlossaryTerm> {
    term_ids
        .iter()
        .filter_map(|id| domain.glossary_terms.get(id))
        .map(|term| ListingGlossaryTerm {
            name: term.name.clone(),
            short_description: term.short_description.clone(),
        })
        .collect()
}

/// The terms a listing recorded as the modelled `DetailedGlossaryTerms` list, in the
/// order the published entity attached them.
pub fn detailed_glossary_terms(
    terms: &[ListingGlossaryTerm],
) -> Vec<crate::ops::types::DetailedGlossaryTerm> {
    terms
        .iter()
        .map(|term| crate::ops::types::DetailedGlossaryTerm {
            name: Some(term.name.clone()),
            short_description: term.short_description.clone(),
        })
        .collect()
}

/// A recorded revision as the modelled `ListingItem` union (smithy_query
/// shape="ListingItem"), shared by every operation that reports a listing's item so the
/// forms cannot drift apart.
///
/// Which member of the union is reported follows the listing's own `entity_type`: the
/// snapshot is the same either way, and each union member names it with that entity's
/// own vocabulary (`assetId`/`assetRevision` against
/// `dataProductId`/`dataProductRevision`).
///
/// Every LIST member of an asset listing is reported as a list, empty where the
/// published entity has nothing of that kind — recorded behaviour, and what lets a
/// catalog consumer iterate zero times instead of finding nothing there. The forms are
/// reported twice over, as the `forms` string [`listing_forms`] renders and as one
/// `assetFormsMetadata` entry per form ([`form_metadata`]); the glossary terms are the
/// ones the change set resolved ([`listing_glossary_terms`]).
pub fn listing_item(
    listing: &Listing,
    revision: &ListingRevision,
) -> crate::ops::types::ListingItem {
    use crate::ops::types::{AssetListing, DataProductListing, ListingItem};

    if listing.entity_type == ENTITY_TYPE_DATA_PRODUCT {
        return ListingItem::dataProductListing(DataProductListing {
            data_product_id: Some(listing.entity_id.clone()),
            data_product_revision: Some(revision.entity_revision.clone()),
            created_at: Some(date_time(revision.entity_created_at_ms)),
            forms: listing_forms(&revision.forms),
            glossary_terms: Some(detailed_glossary_terms(&revision.glossary_terms)),
            owning_project_id: Some(revision.owning_project_id.clone()),
            // The assets of a data product listing come from the data product's own
            // record, which no store holds yet.
            items: None,
        });
    }
    ListingItem::assetListing(AssetListing {
        asset_id: Some(listing.entity_id.clone()),
        asset_revision: Some(revision.entity_revision.clone()),
        asset_type: Some(revision.entity_type_name.clone()),
        created_at: Some(date_time(revision.entity_created_at_ms)),
        forms: listing_forms(&revision.forms),
        asset_forms_metadata: Some(revision.forms.iter().map(form_metadata).collect()),
        // Time series data points are imported by `PostTimeSeriesDataPoints`, which is
        // still a scaffold, so no published entity has any — an empty list, not a
        // missing one.
        latest_time_series_data_point_forms: Some(Vec::new()),
        glossary_terms: Some(detailed_glossary_terms(&revision.glossary_terms)),
        // Governed glossary terms are attached by `AssociateGovernedTerms`, which is
        // still a scaffold, so no published entity carries any.
        governed_glossary_terms: Some(Vec::new()),
        owning_project_id: Some(revision.owning_project_id.clone()),
    })
}

/// One stored DataZone domain unit: a node of the domain's ownership hierarchy.
///
/// Every domain has exactly one ROOT unit — the one `Domain::root_domain_unit_id`
/// names, created with the domain — and every other unit names a parent, so the units
/// of one domain form a tree rooted there.
#[derive(Debug, Clone)]
pub struct DomainUnit {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    /// The unit this one hangs under. `None` only for the domain's root unit.
    pub parent_domain_unit_id: Option<String>,
    /// The unit's owners, in the order they were added: the principal that created the
    /// unit, until `AddEntityOwner` adds another.
    pub owners: Vec<DomainUnitOwner>,
    pub created_by: String,
    pub created_at_ms: i64,
    /// Who last changed the unit, and when — absent until something does.
    pub last_updated_by: Option<String>,
    pub last_updated_at_ms: Option<i64>,
}

/// The stored form of the modelled `DomainUnitOwnerProperties` union, whose members
/// are `user` and `group` (smithy_query shape="DomainUnitOwnerProperties").
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DomainUnitOwner {
    User(String),
    Group(String),
}

impl DomainUnitOwner {
    /// The principal this owner names, and the tag that tells a user from a group —
    /// the two members both owner unions declare (smithy_query
    /// shape="OwnerProperties,DomainUnitOwnerProperties").
    pub fn parts(&self) -> (&str, &'static str) {
        match self {
            Self::User(user_id) => (user_id.as_str(), "user"),
            Self::Group(group_id) => (group_id.as_str(), "group"),
        }
    }
}

/// The key one owner is identified and ordered by.
///
/// Injective in `(identifier, kind)` for the reason [`membership_key`] is: the kind tag
/// is always last and the two tags are distinct, so no two different principals key to
/// the same string. It is what makes an entity's owners a SET — `AddEntityOwner`
/// compares owners by it — and gives `ListEntityOwners` one stable total order to
/// paginate over, independent of the order the owners were added in.
pub fn owner_key(owner: &DomainUnitOwner) -> String {
    let (identifier, tag) = owner.parts();
    format!("{identifier}{PRINCIPAL_KEY_SEPARATOR}{tag}")
}

/// The owner a request's `OwnerProperties` union names, in stored form (smithy_query
/// shape="OwnerProperties": `user` carries `userIdentifier`, `group` carries
/// `groupIdentifier`). Shared by `AddEntityOwner` and `RemoveEntityOwner`, so the two
/// cannot read the same request differently.
pub fn owner_from_properties(owner: &crate::ops::types::OwnerProperties) -> DomainUnitOwner {
    use crate::ops::types::OwnerProperties;
    match owner {
        OwnerProperties::User(user) => DomainUnitOwner::User(user.user_identifier.clone()),
        OwnerProperties::Group(group) => DomainUnitOwner::Group(group.group_identifier.clone()),
    }
}

/// A stored owner as the modelled `OwnerPropertiesOutput` union (smithy_query
/// shape="OwnerPropertiesOutput": `user` carries `userId`, `group` carries `groupId`),
/// the form `ListEntityOwners` reports. The companion of [`owner_properties`], which
/// renders the same stored owner in the union `GetDomainUnit` reports.
pub fn entity_owner_output(owner: &DomainUnitOwner) -> crate::ops::types::OwnerPropertiesOutput {
    use crate::ops::types::{
        OwnerGroupPropertiesOutput, OwnerPropertiesOutput, OwnerUserPropertiesOutput,
    };
    match owner {
        DomainUnitOwner::User(user_id) => OwnerPropertiesOutput::User(OwnerUserPropertiesOutput {
            user_id: Some(user_id.clone()),
        }),
        DomainUnitOwner::Group(group_id) => {
            OwnerPropertiesOutput::Group(OwnerGroupPropertiesOutput {
                group_id: Some(group_id.clone()),
            })
        }
    }
}

/// One stored metadata form type: a name, the project that owns it, and every
/// revision created under that name.
///
/// The model gives `CreateFormType` no companion `UpdateFormType` and makes
/// `revision` a REQUIRED output of both `CreateFormType` and `GetFormType`
/// (smithy_query operation="CreateFormType,GetFormType"), so re-creating a name is
/// the only way a second revision can come about: each create under an existing name
/// appends the next revision and leaves the earlier ones readable.
#[derive(Debug, Clone)]
pub struct FormType {
    pub name: String,
    pub domain_id: String,
    /// The project named by `owningProjectIdentifier` on the create that minted
    /// revision 1; a later create under this name must name the same project.
    pub owning_project_id: String,
    /// Every revision, keyed by its number so the greatest key is the latest.
    /// Bounded by [`MAX_FORM_TYPE_REVISIONS`].
    pub revisions: BTreeMap<u32, FormTypeRevision>,
}

impl FormType {
    /// The greatest revision number stored, i.e. the one a `GetFormType` that names no
    /// `revision` reports.
    pub fn latest_revision(&self) -> Option<&FormTypeRevision> {
        self.revisions.values().next_back()
    }

    /// The revision addressed by a `revision` string, by the rule [`revision_number`]
    /// states. `None` when this form type has no such revision.
    pub fn revision(&self, revision: &str) -> Option<&FormTypeRevision> {
        self.revisions.get(&revision_number(revision)?)
    }
}

/// One revision of a stored metadata form type.
#[derive(Debug, Clone)]
pub struct FormTypeRevision {
    /// The revision number, rendered as the modelled `Revision` string: the first one
    /// minted under a name renders as [`FIRST_REVISION`].
    pub revision: u32,
    /// The `smithy` member of the modelled `Model` union — the only member it has
    /// (smithy_query shape="Model") — exactly as the create supplied it.
    pub model_smithy: String,
    pub status: String,
    pub description: Option<String>,
    pub created_at_ms: i64,
    pub created_by: String,
}

impl FormTypeRevision {
    /// This revision as the modelled `Revision` string.
    pub fn revision_string(&self) -> String {
        self.revision.to_string()
    }
}

/// One stored DataZone business glossary.
///
/// A glossary belongs to a domain and is owned by a project in that domain; the terms
/// under it are a separate resource with its own operations.
#[derive(Debug, Clone)]
pub struct Glossary {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    pub owning_project_id: String,
    /// One of [`GLOSSARY_STATUS_ENABLED`] / [`GLOSSARY_STATUS_DISABLED`].
    pub status: String,
    /// `usageRestrictions` as stored (model list `GlossaryUsageRestrictions`, 1..1 of
    /// `ASSET_GOVERNED_TERMS`): empty when the request named none.
    pub usage_restrictions: Vec<String>,
    pub created_by: String,
    pub created_at_ms: i64,
    /// Who last updated the glossary and when — `None` until something does, because
    /// `GetGlossary` reports `updatedAt`/`updatedBy` as optional members.
    pub updated_by: Option<String>,
    pub updated_at_ms: Option<i64>,
}

/// One stored DataZone business glossary term: an entry of one glossary of one domain.
///
/// A term belongs to the glossary `glossary_id` names, and its name is unique within
/// THAT glossary (smithy_query doc operation="CreateGlossaryTerm"), so two glossaries of
/// one domain may each carry a term of the same name.
#[derive(Debug, Clone)]
pub struct GlossaryTerm {
    pub id: String,
    pub domain_id: String,
    /// The glossary this term is an entry of, within the same domain.
    pub glossary_id: String,
    pub name: String,
    pub short_description: Option<String>,
    pub long_description: Option<String>,
    /// One of [`GLOSSARY_TERM_STATUS_ENABLED`] / [`GLOSSARY_TERM_STATUS_DISABLED`].
    pub status: String,
    /// The term's relations to other terms of the same domain.
    pub term_relations: StoredTermRelations,
    pub created_by: String,
    pub created_at_ms: i64,
    /// Who last updated the term and when — `None` until something does, because
    /// `GetGlossaryTerm` reports `updatedAt`/`updatedBy` as optional members.
    pub updated_by: Option<String>,
    pub updated_at_ms: Option<i64>,
}

/// The stored form of the modelled `TermRelations` structure (smithy_query
/// shape="TermRelations": `isA` and `classifies`, each a `GlossaryTerms` list of 1..10
/// term ids). Each list is empty exactly when the request named none, so the absent and
/// the empty case cannot drift apart in the store.
#[derive(Debug, Clone, Default)]
pub struct StoredTermRelations {
    pub is_a: Vec<String>,
    pub classifies: Vec<String>,
}

impl StoredTermRelations {
    /// Every term id this relation names, `isA` first then `classifies`: what a create
    /// has to resolve against the store before it keeps the relation.
    pub fn related_term_ids(&self) -> impl Iterator<Item = &str> {
        self.is_a
            .iter()
            .chain(self.classifies.iter())
            .map(String::as_str)
    }
}

/// A request's `termRelations` in stored form. Shared by every handler that takes one,
/// so none of them reads the same request differently.
pub fn stored_term_relations(
    relations: Option<crate::ops::types::TermRelations>,
) -> StoredTermRelations {
    let relations = relations.unwrap_or_default();
    StoredTermRelations {
        is_a: relations.is_a.unwrap_or_default(),
        classifies: relations.classifies.unwrap_or_default(),
    }
}

/// A stored term's relations as the modelled `TermRelations` structure.
///
/// Both lists are modelled 1..10, so an EMPTY one is not a legal value on the wire: a
/// relation the term does not have is reported absent, and a term with no relation at
/// all reports no `termRelations` at all. Shared by every handler that reports a term so
/// the forms cannot drift.
pub fn term_relations_output(
    relations: &StoredTermRelations,
) -> Option<crate::ops::types::TermRelations> {
    let is_a = (!relations.is_a.is_empty()).then(|| relations.is_a.clone());
    let classifies = (!relations.classifies.is_empty()).then(|| relations.classifies.clone());
    (is_a.is_some() || classifies.is_some())
        .then_some(crate::ops::types::TermRelations { is_a, classifies })
}

/// The `usageRestrictions` a glossary term reports: "The usage restriction of the
/// restricted glossary" (smithy_query doc shape="CreateGlossaryTermOutput"
/// member="usageRestrictions"), so it is the OWNING GLOSSARY's restriction rather than
/// anything a term carries of its own. `None` when that glossary has none; the list is
/// modelled 1..1, so an empty one is never reported.
pub fn term_usage_restrictions(domain: &Domain, glossary_id: &str) -> Option<Vec<String>> {
    let glossary = domain.glossaries.get(glossary_id)?;
    (!glossary.usage_restrictions.is_empty()).then(|| glossary.usage_restrictions.clone())
}

/// The stored form of the modelled `SingleSignOn` structure.
#[derive(Debug, Clone, Default)]
pub struct StoredSingleSignOn {
    pub sso_type: Option<String>,
    pub user_assignment: Option<String>,
    pub idc_instance_arn: Option<String>,
}

/// One stored DataZone project.
#[derive(Debug, Clone)]
pub struct Project {
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    pub project_status: String,
    pub created_by: String,
    pub created_at_ms: i64,
    pub last_updated_at_ms: i64,
    pub glossary_terms: Vec<String>,
    pub domain_unit_id: Option<String>,
    pub project_profile_id: Option<String>,
    /// `resourceTags` as stored: the request takes a map, the response a list of
    /// `ResourceTag`, so the key order is kept stable by storing it sorted.
    pub resource_tags: BTreeMap<String, String>,
    pub project_category: Option<String>,
    /// This project's members, keyed by [`membership_key`] so one principal holds at
    /// most one designation in the project and every listing iterates in one stable
    /// order — the member identifier's. Bounded by [`MAX_MEMBERSHIPS_PER_PROJECT`].
    ///
    /// Nested in the project, so the memberships go with it when the project (or the
    /// domain above it) is deleted.
    pub memberships: BTreeMap<String, Membership>,
}

/// Whether a membership names a user or a group: the two arms of the modelled
/// `Member` and `MemberDetails` unions (smithy_query shape="Member,MemberDetails").
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum MemberKind {
    Group,
    User,
}

impl MemberKind {
    /// This arm's tag, as it is spelled inside a stored membership key.
    pub const fn tag(self) -> &'static str {
        match self {
            Self::Group => "group",
            Self::User => "user",
        }
    }

    /// The `Member` union member a request names this arm with, for a refusal message
    /// that names the input member the caller has to fix.
    pub const fn request_member(self) -> &'static str {
        match self {
            Self::Group => "member.groupIdentifier",
            Self::User => "member.userIdentifier",
        }
    }
}

/// One stored project membership: which principal holds which designation.
///
/// `ProjectMember` carries no timestamp (smithy_query shape="ProjectMember"), so a
/// membership stores nothing beyond what a listing reports.
#[derive(Debug, Clone)]
pub struct Membership {
    pub kind: MemberKind,
    pub identifier: String,
    pub designation: String,
}

/// The separator between a principal's identifier and its kind inside a key this state
/// builds for it — a membership key ([`membership_key`]) or an owner key
/// ([`owner_key`]).
///
/// A control character, so the key orders by the identifier FIRST — the only name this
/// emulator holds for a principal, and so the order `sortBy=NAME` asks for — while
/// still telling a user apart from a group that happens to share its identifier.
const PRINCIPAL_KEY_SEPARATOR: char = '\u{1}';

/// The key one membership is stored under.
///
/// Injective in `(identifier, kind)`: the kind tag is always last and the two tags are
/// distinct, so no two different members can key to the same string.
pub fn membership_key(kind: MemberKind, identifier: &str) -> String {
    format!("{identifier}{PRINCIPAL_KEY_SEPARATOR}{}", kind.tag())
}

/// The principal a `Member` union value names.
pub fn member_ref(member: &crate::ops::types::Member) -> (MemberKind, &str) {
    match member {
        crate::ops::types::Member::userIdentifier(id) => (MemberKind::User, id.as_str()),
        crate::ops::types::Member::groupIdentifier(id) => (MemberKind::Group, id.as_str()),
    }
}

/// Resolve the principal a membership names against the profiles its domain holds,
/// answering the refusal message when the domain holds none for it.
///
/// A GROUP member must already have a group profile in the domain: the service
/// resolves the profile and refuses the membership when the domain holds none, because
/// a group profile is only created implicitly for a domain that manages its own
/// identities. Group profiles are created by `CreateGroupProfile`, which is still a
/// scaffold here, so a domain holds none and the refusal is the only reachable branch —
/// the honest answer rather than a membership for a principal the domain does not
/// know. A USER needs no profile up front: the service creates one implicitly for the
/// principal the membership names.
pub fn resolve_membership_principal(
    kind: MemberKind,
    identifier: &str,
    domain_id: &str,
) -> Result<(), String> {
    if kind == MemberKind::Group {
        return Err(format!(
            "Group profile '{identifier}' not found for domain '{domain_id}', profile creation is only valid for implicit domain"
        ));
    }
    Ok(())
}

/// Why a requested membership cannot be stored.
#[derive(Debug)]
pub enum MembershipRefusal {
    /// The request breaks a rule the model documents; the string is the refusal
    /// message, naming the member and the constraint. Answer it with the operation's
    /// own `ValidationException`.
    Invalid(String),
    /// The membership names a principal the domain holds no profile for; the string is
    /// the refusal message. Answer it with the operation's own
    /// `ResourceNotFoundException`.
    ProfileNotFound(String),
    /// The membership names a resource this emulator does not serve yet; the string
    /// says which. Answer it with the scaffold's 501, never by storing something else.
    Unserved(String),
}

/// Validate one requested membership and turn it into the key and record to store.
///
/// Shared by `CreateProjectMembership` and by `CreateProject`'s
/// `membershipAssignments`, which carry the same three members and the same rules.
pub fn membership_from_request(
    member: &crate::ops::types::Member,
    designation: &str,
    user_created_designation_id: Option<&str>,
    domain_id: &str,
) -> Result<(String, Membership), MembershipRefusal> {
    let (kind, identifier) = member_ref(member);
    // The union's arms target plain `String`, so the boundary validator accepts an
    // empty one; a membership for no principal at all is a bad request.
    if identifier.is_empty() {
        return Err(MembershipRefusal::Invalid(format!(
            "{} must name a {}, but it is empty",
            kind.request_member(),
            kind.tag()
        )));
    }
    // The principal must be one the domain can hold a membership for.
    resolve_membership_principal(kind, identifier, domain_id)
        .map_err(MembershipRefusal::ProfileNotFound)?;
    // `userCreatedDesignationId` is documented as "Required when designation is
    // USER_CREATED_DESIGNATION; must not be provided otherwise" (smithy_query
    // query="doc" shape="CreateProjectMembershipInput"
    // member="userCreatedDesignationId", and the same sentence on
    // `ProjectMembershipAssignment`).
    match (
        designation == DESIGNATION_USER_CREATED,
        user_created_designation_id,
    ) {
        (true, None) => {
            return Err(MembershipRefusal::Invalid(format!(
                "userCreatedDesignationId is required when designation is {DESIGNATION_USER_CREATED}"
            )))
        }
        (false, Some(id)) => {
            return Err(MembershipRefusal::Invalid(format!(
                "userCreatedDesignationId {id} must not be provided when designation is {designation}"
            )))
        }
        // The id names a designation resource, which only `CreateDesignation` can
        // make and that operation is still a scaffold — so no such designation can
        // exist here, and accepting the membership would claim one does.
        (true, Some(_)) => {
            return Err(MembershipRefusal::Unserved(format!(
                "designation {DESIGNATION_USER_CREATED}, which names a designation resource (CreateDesignation)"
            )))
        }
        (false, None) => {}
    }
    Ok((
        membership_key(kind, identifier),
        Membership {
            kind,
            identifier: identifier.to_string(),
            designation: designation.to_string(),
        },
    ))
}

/// One user profile stored in a domain: the identity it is for, the type and status
/// DataZone reports it with, and the details it reports under the matching member of
/// the modelled `UserProfileDetails` union.
///
/// The union is kept as the modelled type itself (the trade [`AccountPool`] makes for
/// `accountSource`), so the profile's reader reports exactly what its create stored.
/// The model gives a user profile no timestamps (smithy_query
/// operation="GetUserProfile"), so nothing beyond what a read reports is kept, and no
/// member of `UserProfileStatus` is non-terminal, so a profile owes the clock no
/// transition.
#[derive(Debug, Clone)]
pub struct UserProfile {
    /// The profile's own `UserProfileId`, minted by
    /// [`ComAmazonawsDatazoneDatazoneState::mint_user_profile_id`].
    pub id: String,
    pub domain_id: String,
    /// The `userIdentifier` the profile was created for — an IAM ARN, a principal name
    /// or an identity-store user id. The identity half of [`user_profile_key`].
    pub user_identifier: String,
    /// The `sessionName` the profile was created with, which is what tells two profiles
    /// of one IAM role apart. The session half of [`user_profile_key`].
    pub session_name: Option<String>,
    /// A `UserProfileType` member — [`USER_PROFILE_TYPE_IAM`] or
    /// [`USER_PROFILE_TYPE_SSO`].
    pub profile_type: String,
    /// A `UserProfileStatus` member — see [`created_user_profile_status`].
    pub status: String,
    pub details: crate::ops::types::UserProfileDetails,
}

/// The `UserProfileType` members (model enum `com.amazonaws.datazone#UserProfileType`:
/// IAM | SSO): which identity system the profile's user comes from.
pub const USER_PROFILE_TYPE_IAM: &str = "IAM";
pub const USER_PROFILE_TYPE_SSO: &str = "SSO";

/// The `UserType` members (model enum `com.amazonaws.datazone#UserType`: IAM_USER |
/// IAM_ROLE | SSO_USER | IAM_ROLE_SESSION) a create may name the user with.
pub const USER_TYPE_IAM_USER: &str = "IAM_USER";
pub const USER_TYPE_IAM_ROLE: &str = "IAM_ROLE";
pub const USER_TYPE_IAM_ROLE_SESSION: &str = "IAM_ROLE_SESSION";
pub const USER_TYPE_SSO_USER: &str = "SSO_USER";

/// The `UserProfileStatus` members a profile of each type is reported with (model enum
/// `com.amazonaws.datazone#UserProfileStatus`: ASSIGNED | NOT_ASSIGNED | ACTIVATED |
/// DEACTIVATED): an IAM principal's profile is ACTIVATED or DEACTIVATED, an identity
/// centre user's profile is ASSIGNED or NOT_ASSIGNED.
pub const USER_PROFILE_STATUS_ACTIVATED: &str = "ACTIVATED";
pub const USER_PROFILE_STATUS_ASSIGNED: &str = "ASSIGNED";

/// The most user profiles this emulator stores in one domain.
///
/// A CHOSEN ceiling, not a published quota: the identities come from the request, so a
/// create loop is a caller-driven growth site and has to be bounded. `CreateUserProfile`
/// models no `ServiceQuotaExceededException` (smithy_query operation="CreateUserProfile"),
/// so the refusal past it is its `ValidationException` — the rule
/// `CreateProjectMembership` follows for a bound its own model does not name.
pub const MAX_USER_PROFILES_PER_DOMAIN: usize = 500;

/// The key one user profile is stored under: the identity the request names, plus the
/// session name that tells two profiles of one IAM role apart.
///
/// Injective in `(user_identifier, session_name)`: the separator is a control character
/// no `UserIdentifier` admits (its `@pattern` is three alternatives of ASCII name, ARN
/// and hex-and-dash id characters, smithy_query shape="UserIdentifier"), so the two
/// parts of the key can never be confused for one another.
pub fn user_profile_key(user_identifier: &str, session_name: Option<&str>) -> String {
    format!(
        "{user_identifier}{PRINCIPAL_KEY_SEPARATOR}{}",
        session_name.unwrap_or_default()
    )
}

/// Whether `candidate` has the shape of a `UserProfileId` — an identity id rather than
/// a principal NAME.
///
/// SOURCE: `UserProfileId`'s `@pattern`
/// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
/// (smithy_query shape="UserProfileId"), which is also the FIRST alternative of
/// `UserIdentifier`: a request may name a user by such an id, and an id is not a
/// username.
pub fn is_user_profile_id(candidate: &str) -> bool {
    /// The hex-digit group lengths of the id, in order.
    const GROUPS: [usize; 5] = [8, 4, 4, 4, 12];
    let is_hex_run = |text: &str, len: usize| {
        text.len() == len && text.bytes().all(|byte| byte.is_ascii_hexdigit())
    };
    let mut parts: Vec<&str> = candidate.split('-').collect();
    // The optional identity-store prefix is a sixth, LOWERCASE hex group of 10 in
    // front of the five that make up the id itself.
    if parts.len() == GROUPS.len() + 1 {
        let prefix = parts.remove(0);
        if prefix.len() != 10
            || !prefix
                .bytes()
                .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
        {
            return false;
        }
    }
    parts.len() == GROUPS.len()
        && parts
            .iter()
            .zip(GROUPS)
            .all(|(part, len)| is_hex_run(part, len))
}

/// The `UserProfileType` a create reports for the user it names.
///
/// `userType` names the identity system when the request carries one: the three IAM
/// members are IAM profiles and `SSO_USER` is an SSO profile. When the request names
/// none, the identity system follows the FORM of the identifier — an identity-store id
/// ([`is_user_profile_id`]) names an identity centre user, anything else (an IAM ARN or
/// a principal name, the other two alternatives of `UserIdentifier`) an IAM principal.
pub fn user_profile_type(user_type: Option<&str>, user_identifier: &str) -> &'static str {
    match user_type {
        Some(USER_TYPE_SSO_USER) => USER_PROFILE_TYPE_SSO,
        Some(_) => USER_PROFILE_TYPE_IAM,
        None if is_user_profile_id(user_identifier) => USER_PROFILE_TYPE_SSO,
        None => USER_PROFILE_TYPE_IAM,
    }
}

/// The status a profile is created with: a profile of an IAM principal is ACTIVATED,
/// one of an identity centre user is ASSIGNED. Each is the member of its type's pair
/// that means the user HAS the domain's access the create just gave it;
/// `UpdateUserProfile` is what moves it to the other member.
pub fn created_user_profile_status(profile_type: &str) -> &'static str {
    if profile_type == USER_PROFILE_TYPE_SSO {
        USER_PROFILE_STATUS_ASSIGNED
    } else {
        USER_PROFILE_STATUS_ACTIVATED
    }
}

/// The `UserProfileDetails` a profile of `profile_type` reports for the identity the
/// request named.
///
/// The union has one member per profile type (smithy_query shape="UserProfileDetails"),
/// and each carries what the request itself names:
///
/// * `iam` — the IAM principal's ARN, which is the identifier when the request named
///   one and otherwise the ARN of the principal that `userType` names (a role for
///   `IAM_ROLE`/`IAM_ROLE_SESSION`, a user for `IAM_USER` and for a request that names
///   no type), plus the `sessionName` of a role session. `groupProfileId` links the
///   profile to a group profile, which only `CreateGroupProfile` makes, so a profile
///   created here belongs to none.
/// * `sso` — the username, which is the identifier when the request named the user by
///   NAME. An identity-store id is not a username, and this emulator holds no identity
///   store to resolve one through, so a profile created from an id reports no username
///   rather than echoing the id as one; nor are the first and last names anything the
///   request carries.
pub fn user_profile_details(
    profile_type: &str,
    user_type: Option<&str>,
    user_identifier: &str,
    session_name: Option<&str>,
) -> crate::ops::types::UserProfileDetails {
    use crate::ops::types::{IamUserProfileDetails, SsoUserProfileDetails, UserProfileDetails};
    if profile_type == USER_PROFILE_TYPE_SSO {
        return UserProfileDetails::Sso(SsoUserProfileDetails {
            username: (!is_user_profile_id(user_identifier))
                .then(|| user_identifier.to_string()),
            first_name: None,
            last_name: None,
        });
    }
    let arn = if user_identifier.starts_with("arn:") {
        user_identifier.to_string()
    } else {
        let resource = match user_type {
            Some(USER_TYPE_IAM_ROLE) | Some(USER_TYPE_IAM_ROLE_SESSION) => {
                format!("role/{user_identifier}")
            }
            _ => format!("user/{user_identifier}"),
        };
        // IAM is a global service, so its ARNs carry no region.
        ::engine::emulator_arn("iam", "", &resource)
    };
    UserProfileDetails::Iam(IamUserProfileDetails {
        arn: Some(arn),
        // The principal id is minted by IAM, which this emulator does not stand in
        // for, so the profile reports none rather than an invented one.
        principal_id: None,
        session_name: session_name.map(str::to_string),
        group_profile_id: None,
    })
}

/// One stored DataZone catalog asset: the identity the catalog addresses it by, plus
/// every recorded revision of its metadata.
///
/// An asset is versioned. `CreateAsset` records revision [`FIRST_REVISION`] and each
/// `CreateAssetRevision` records the next number, so what a revision CHANGES lives on
/// the [`AssetRevision`] and what identifies the asset — its id, the domain and project
/// it belongs to, its type, the identifier of the system it was imported from — stays
/// here, unchanged by any revision (`CreateAssetRevisionInput` names none of them,
/// smithy_query operation="CreateAssetRevision"). A revision already recorded is never
/// rewritten, so an earlier one stays readable by number after a later one is added.
#[derive(Debug, Clone)]
pub struct Asset {
    pub id: String,
    pub domain_id: String,
    pub owning_project_id: String,
    pub external_identifier: Option<String>,
    pub type_identifier: String,
    /// Every recorded revision, keyed by its number so the greatest key is the latest
    /// and the least is the first. NEVER empty: a created asset is its own first
    /// revision. Bounded by [`MAX_ASSET_REVISIONS`].
    pub revisions: BTreeMap<u32, AssetRevision>,
    /// The asset filters defined ON this asset, keyed by their `FilterId` — the
    /// identifier `GetAssetFilter`/`DeleteAssetFilter` address one by — so a filter goes
    /// with the asset it is a view of. Kept on the ASSET rather than on a revision: a
    /// filter addresses the asset (`CreateAssetFilterInput` names no revision,
    /// smithy_query operation="CreateAssetFilter"), so adding a revision neither
    /// duplicates nor drops one. A `BTreeMap` so `ListAssetFilters` iterates in one
    /// stable order and a pagination token can resume from a key. Bounded by
    /// [`MAX_FILTERS_PER_ASSET`].
    pub filters: BTreeMap<String, AssetFilter>,
}

impl Asset {
    /// The greatest revision number recorded: the revision a call that names none
    /// reads, and the one a new revision supersedes.
    pub fn latest_revision(&self) -> Option<&AssetRevision> {
        self.revisions.values().next_back()
    }

    /// The least revision number recorded — the one `CreateAsset` minted — which is
    /// what the `firstRevisionCreatedBy`/`firstRevisionCreatedAt` members report.
    pub fn first_revision(&self) -> Option<&AssetRevision> {
        self.revisions.values().next()
    }

    /// The revision addressed by a `Revision` string, by the rule [`revision_number`]
    /// states. `None` when this asset has no such revision.
    pub fn revision(&self, revision: &str) -> Option<&AssetRevision> {
        self.revisions.get(&revision_number(revision)?)
    }

    /// The number the NEXT recorded revision gets: one past the greatest ever recorded,
    /// so revision numbers never go backwards.
    pub fn next_revision_number(&self) -> u32 {
        self.revisions.keys().next_back().copied().unwrap_or(0) + 1
    }

    /// The revision carrying `token`, if a `CreateAssetRevision` with that
    /// `clientToken` already recorded one of this asset's revisions.
    pub fn revision_with_token(&self, token: &str) -> Option<&AssetRevision> {
        self.revisions
            .values()
            .find(|revision| revision.client_token.as_deref() == Some(token))
    }
}

/// One recorded revision of an asset: the asset's metadata as the call that recorded
/// that revision set it.
#[derive(Debug, Clone)]
pub struct AssetRevision {
    /// The revision number, rendered as the modelled `Revision` string: the revision
    /// `CreateAsset` records renders as [`FIRST_REVISION`].
    pub revision: u32,
    pub name: String,
    pub description: Option<String>,
    /// The revision of the asset TYPE this revision of the asset is bound to.
    pub type_revision: String,
    pub glossary_terms: Vec<String>,
    /// The metadata forms attached at this revision, in the order the request listed
    /// them and keyed by form name (a name appears at most once).
    pub forms: Vec<StoredForm>,
    /// The `predictionConfiguration` the request set, mirrored member for member so a
    /// read reports exactly what was written.
    pub prediction_configuration: Option<StoredPredictionConfiguration>,
    /// The caller that recorded this revision, and when — the `createdBy`/`createdAt`
    /// an asset read at this revision reports.
    pub created_by: String,
    pub created_at_ms: i64,
    /// The `clientToken` of the `CreateAssetRevision` that recorded this revision, so a
    /// replay of that request answers with it instead of recording a second one.
    /// `None` on the revision `CreateAsset` records: that token created the ASSET, so it
    /// is remembered in the store's own idempotency map ([`IdempotencyKey::asset`]).
    pub client_token: Option<String>,
}

impl AssetRevision {
    /// This revision as the modelled `Revision` string.
    pub fn revision_string(&self) -> String {
        self.revision.to_string()
    }
}

/// The metadata forms a request's form list attaches to a resource, in stored form, or
/// the message a bad list is refused with.
///
/// A form is identified on its resource by its `formName` — that is the only way a form
/// list names one — so two forms in one list cannot share a name; `member` is the input
/// member the refusal names, so each operation's message points the caller at the member
/// it sent. A form bound to a type whose revision the request did not name is bound to
/// that type's first revision, and a form that names no type has no type revision
/// either. Shared by every operation that takes a form list (`CreateAsset`,
/// `CreateAssetRevision`, `CreateSubscriptionRequest`) so they cannot read one
/// differently.
pub fn stored_forms(
    member: &str,
    forms_input: Vec<crate::ops::types::FormInput>,
) -> Result<Vec<StoredForm>, String> {
    let mut seen_form_names = std::collections::BTreeSet::new();
    for form in &forms_input {
        if !seen_form_names.insert(form.form_name.as_str()) {
            return Err(format!(
                "{member} names the form {} more than once; a form name is unique within {member}",
                form.form_name
            ));
        }
    }
    Ok(forms_input
        .into_iter()
        .map(|form| StoredForm {
            type_name: form.type_identifier.clone(),
            type_revision: form.type_revision.or_else(|| {
                form.type_identifier
                    .as_ref()
                    .map(|_| FIRST_REVISION.to_string())
            }),
            form_name: form.form_name,
            content: form.content,
        })
        .collect())
}

/// The asset-type revision an asset (or a new revision of one) is bound to, or the
/// message a `typeRevision` that names no revision of that type is refused with.
///
/// "--type-revision (if used) must match a valid revision of the asset type"
/// (smithy_query query="doc" shape="CreateAsset"): a pinned revision is checked against
/// the revision the STORED type is at, and a type no store holds — one the SERVICE
/// defines, such as `amazon.datazone.RelationalTable` — carries no revision to check
/// against, so a pin on it is taken as given. A request that pins none keeps `fallback`
/// (the revision the asset is already bound to, for a new revision of an existing
/// asset), else takes the revision the stored type is at, else the first revision.
///
/// Shared by `CreateAsset` and `CreateAssetRevision` so the two cannot read one
/// `typeRevision` differently.
pub fn asset_type_revision(
    domain: &Domain,
    type_identifier: &str,
    pinned: Option<&str>,
    fallback: Option<&str>,
) -> Result<String, String> {
    let stored = domain
        .asset_types
        .get(type_identifier)
        .map(|asset_type| asset_type.revision.as_str());
    if let (Some(pinned), Some(stored)) = (pinned, stored) {
        if pinned != stored {
            return Err(format!(
                "typeRevision {pinned} is not a revision of asset type {type_identifier}, which is at revision {stored}"
            ));
        }
    }
    Ok(pinned
        .or(fallback)
        .or(stored)
        .unwrap_or(FIRST_REVISION)
        .to_string())
}

/// One metadata form attached to an asset: the stored form of the modelled
/// `FormInput`/`FormOutput` pair.
#[derive(Debug, Clone, Default)]
pub struct StoredForm {
    pub form_name: String,
    pub type_name: Option<String>,
    pub type_revision: Option<String>,
    pub content: Option<String>,
}

/// The stored form of the modelled `PredictionConfiguration`: present-or-absent is
/// meaningful at every level, so the structure is kept rather than flattened.
#[derive(Debug, Clone, Default)]
pub struct StoredPredictionConfiguration {
    pub business_name_generation: Option<StoredBusinessNameGeneration>,
}

/// The stored form of the modelled `BusinessNameGenerationConfiguration`.
#[derive(Debug, Clone, Default)]
pub struct StoredBusinessNameGeneration {
    pub enabled: Option<bool>,
}

/// One stored asset filter: a controlled view of ONE asset — either the columns of it a
/// consumer sees (`columnConfiguration`) or the rows (`rowConfiguration`).
///
/// What is STORED is what the request set: the filter's identity, its name and
/// description, and the `configuration` as the modelled union the request carried, so a
/// read reports back exactly what was persisted rather than a lossy projection of it.
/// The derived members (`status`, `errorMessage`, `effective*`) are not stored at all:
/// they are a function of that configuration and of the asset's schema AS IT IS NOW, so
/// every operation reporting this filter derives them through
/// [`asset_filter_evaluation`] and no stored copy of them can go stale when a new asset
/// revision changes the schema.
#[derive(Debug, Clone)]
pub struct AssetFilter {
    /// The filter's own `FilterId`, minted by
    /// [`ComAmazonawsDatazoneDatazoneState::mint_id`].
    pub id: String,
    pub domain_id: String,
    /// The asset this filter is a view OF.
    pub asset_id: String,
    pub name: String,
    pub description: Option<String>,
    pub configuration: crate::ops::types::AssetFilterConfiguration,
    pub created_at_ms: i64,
    /// The `@idempotencyToken` the request that created this filter carried, so a replay
    /// of that request answers with this filter instead of adding a second one. Stored on
    /// the filter (rather than in a side map) so the token is released with it, the way
    /// [`PolicyGrant::client_token`] is.
    pub client_token: Option<String>,
}

/// The `FilterStatus` members (model enum `com.amazonaws.datazone#FilterStatus`:
/// VALID | INVALID). Both are verdicts on a filter evaluated against its asset — the
/// enum has no in-progress member — so a filter is reported with its verdict from the
/// moment it is stored and owes the clock no transition.
pub const FILTER_STATUS_VALID: &str = "VALID";
pub const FILTER_STATUS_INVALID: &str = "INVALID";

/// The most asset filters this emulator stores on one asset.
///
/// A CHOSEN ceiling, not a published quota: `CreateAssetFilter` is a caller-driven
/// growth site, so the per-asset filter map has to be bounded or a create loop grows the
/// emulator's heap without limit. The refusal past it is the
/// `ServiceQuotaExceededException` the operation already models.
pub const MAX_FILTERS_PER_ASSET: usize = 100;

/// The deepest `and`/`or` nesting this emulator evaluates in one row filter.
///
/// A CHOSEN bound, not a published quota: `RowFilterList` declares no length and
/// `RowFilter` nests into itself (smithy_query shape="RowFilter"), so a request can
/// describe a filter of any depth. Past this the request is refused with the operation's
/// `ValidationException` rather than walked, so neither rendering nor validating a
/// stored filter can run away.
pub const MAX_ROW_FILTER_DEPTH: usize = 10;

/// The most nodes (`and`/`or` groups plus expressions) this emulator evaluates in one
/// row filter.
///
/// A CHOSEN bound, and the one that makes the whole filter bounded rather than just its
/// nesting: depth alone caps no total, because the BREADTH of each `and`/`or` member
/// list is unconstrained by the model, so one shallow group can carry any number of
/// expressions. Every walk over a row filter — measuring, collecting its columns,
/// rendering it — is linear in the node count, so capping the count caps the CPU one
/// request can ask for, both at create time and on every later read of the stored
/// filter. Past it the request is refused with the operation's `ValidationException`.
pub const MAX_ROW_FILTER_NODES: usize = 1_000;

/// The most columns this emulator accepts in one column filter's `includedColumnNames`.
///
/// A CHOSEN bound: `ColumnNameList` declares no length (smithy_query
/// shape="ColumnNameList"), so the list is a caller-chosen size that every evaluation of
/// the stored filter walks. Past it the request is refused with the operation's
/// `ValidationException`.
pub const MAX_INCLUDED_COLUMN_NAMES: usize = 1_000;

/// How big a row filter is: its `and`/`or` nesting depth (1 for a bare expression) and
/// its total node count.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RowFilterSize {
    pub depth: usize,
    /// The nodes counted, which STOPS one past [`MAX_ROW_FILTER_NODES`]: a filter is
    /// refused at that point, so there is never a reason to walk the rest of it.
    pub nodes: usize,
}

/// Measure `filter`, stopping as soon as it is already too big to accept.
///
/// Walked with an explicit stack rather than by recursion, so measuring a deeply nested
/// request costs no stack, and abandoned once more than [`MAX_ROW_FILTER_NODES`] nodes
/// have been seen, so measuring a request of ANY size costs at most that many steps.
/// A handler refuses the filter when either figure exceeds its bound, before the filter
/// is stored and before anything walks it again.
pub fn measure_row_filter(filter: &crate::ops::types::RowFilter) -> RowFilterSize {
    use crate::ops::types::RowFilter;
    let mut deepest = 0;
    let mut nodes = 0;
    let mut pending = vec![(filter, 1usize)];
    while let Some((node, depth)) = pending.pop() {
        deepest = deepest.max(depth);
        nodes += 1;
        if nodes > MAX_ROW_FILTER_NODES {
            break;
        }
        match node {
            RowFilter::Expression(_) => {}
            RowFilter::And(members) | RowFilter::Or(members) => {
                // The children still to visit are themselves bounded by the node cap:
                // the walk stops one past it, so the stack cannot grow past the number
                // of children of the nodes already counted.
                for member in members.iter().take(MAX_ROW_FILTER_NODES + 1) {
                    pending.push((member, depth + 1));
                }
            }
        }
    }
    RowFilterSize {
        depth: deepest,
        nodes,
    }
}

/// Every column name `filter` references, in the order the filter names them and with
/// repeats dropped.
///
/// Linear in the filter's node count: membership is decided through a set, not by
/// rescanning what has been collected. The walk visits at most
/// [`MAX_ROW_FILTER_NODES`] nodes, the bound every handler enforces before it stores a
/// filter, so no stored filter can make this walk unbounded either.
pub fn row_filter_column_names(filter: &crate::ops::types::RowFilter) -> Vec<String> {
    use crate::ops::types::{RowFilter, RowFilterExpression as Expr};
    let mut names: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    // The members of a group are walked in order, so the names come out in the order
    // the request listed them; the stack is popped from the back, so each group's
    // members are pushed in reverse.
    let mut pending: Vec<&RowFilter> = vec![filter];
    let mut visited = 0;
    while let Some(node) = pending.pop() {
        visited += 1;
        if visited > MAX_ROW_FILTER_NODES {
            break;
        }
        match node {
            RowFilter::Expression(expression) => {
                let column = match expression {
                    Expr::equalTo(e) => &e.column_name,
                    Expr::notEqualTo(e) => &e.column_name,
                    Expr::greaterThan(e) => &e.column_name,
                    Expr::lessThan(e) => &e.column_name,
                    Expr::greaterThanOrEqualTo(e) => &e.column_name,
                    Expr::lessThanOrEqualTo(e) => &e.column_name,
                    Expr::isNull(e) => &e.column_name,
                    Expr::isNotNull(e) => &e.column_name,
                    Expr::In(e) => &e.column_name,
                    Expr::notIn(e) => &e.column_name,
                    Expr::Like(e) => &e.column_name,
                    Expr::notLike(e) => &e.column_name,
                };
                if seen.insert(column.clone()) {
                    names.push(column.clone());
                }
            }
            RowFilter::And(members) | RowFilter::Or(members) => {
                for member in members.iter().take(MAX_ROW_FILTER_NODES + 1).rev() {
                    pending.push(member);
                }
            }
        }
    }
    names
}

/// `filter` as ONE predicate, the modelled `effectiveRowFilter` string.
///
/// `effectiveRowFilter` is an unconstrained `String` (smithy_query
/// shape="CreateAssetFilterOutput" member="effectiveRowFilter") and AWS documents no
/// grammar for it, so this is a CHOSEN canonical rendering: a SQL `WHERE` predicate,
/// with each comparison as `<column> <operator> '<value>'`, an `in`/`notIn` as a
/// parenthesised value list, and a group of two or more members joined by `AND`/`OR`
/// inside parentheses. A single-member group renders as that member, and a group with
/// no member renders as nothing at all, since it restricts nothing. A value's single
/// quotes are doubled, so a value can never end the quoted literal early.
///
/// Recursion is bounded by [`MAX_ROW_FILTER_DEPTH`] and the work by
/// [`MAX_ROW_FILTER_NODES`], both of which every handler checks with
/// [`measure_row_filter`] before it stores a filter — so rendering a stored filter is
/// bounded however big a request was.
pub fn render_row_filter(filter: &crate::ops::types::RowFilter) -> String {
    use crate::ops::types::{RowFilter, RowFilterExpression as Expr};

    /// A value as a SQL string literal: single quotes doubled, the whole quoted.
    fn literal(value: &str) -> String {
        format!("'{}'", value.replace('\'', "''"))
    }
    /// A `(v1, v2, …)` value list for `in`/`notIn`.
    fn literal_list(values: &[String]) -> String {
        let rendered: Vec<String> = values.iter().map(|value| literal(value)).collect();
        format!("({})", rendered.join(", "))
    }

    match filter {
        RowFilter::Expression(expression) => match expression {
            Expr::equalTo(e) => format!("{} = {}", e.column_name, literal(&e.value)),
            Expr::notEqualTo(e) => format!("{} <> {}", e.column_name, literal(&e.value)),
            Expr::greaterThan(e) => format!("{} > {}", e.column_name, literal(&e.value)),
            Expr::lessThan(e) => format!("{} < {}", e.column_name, literal(&e.value)),
            Expr::greaterThanOrEqualTo(e) => {
                format!("{} >= {}", e.column_name, literal(&e.value))
            }
            Expr::lessThanOrEqualTo(e) => format!("{} <= {}", e.column_name, literal(&e.value)),
            Expr::isNull(e) => format!("{} IS NULL", e.column_name),
            Expr::isNotNull(e) => format!("{} IS NOT NULL", e.column_name),
            Expr::In(e) => format!("{} IN {}", e.column_name, literal_list(&e.values)),
            Expr::notIn(e) => format!("{} NOT IN {}", e.column_name, literal_list(&e.values)),
            Expr::Like(e) => format!("{} LIKE {}", e.column_name, literal(&e.value)),
            Expr::notLike(e) => format!("{} NOT LIKE {}", e.column_name, literal(&e.value)),
        },
        RowFilter::And(members) => render_group(members, "AND"),
        RowFilter::Or(members) => render_group(members, "OR"),
    }
}

/// One `and`/`or` group of [`render_row_filter`]: the members that render to something,
/// joined by `operator` and parenthesised once there is more than one of them.
fn render_group(members: &[crate::ops::types::RowFilter], operator: &str) -> String {
    let rendered: Vec<String> = members
        .iter()
        .map(render_row_filter)
        .filter(|member| !member.is_empty())
        .collect();
    match rendered.len() {
        0 => String::new(),
        1 => rendered.into_iter().next().unwrap_or_default(),
        _ => format!("({})", rendered.join(&format!(" {operator} "))),
    }
}

/// The column names an asset's own schema declares, or `None` when nothing it stores
/// declares any.
///
/// An asset's columns live in the content of a metadata form — DataZone's relational
/// table form carries a `columns` array whose entries name a `columnName` — and this
/// emulator persists that content verbatim, so the schema is read back off the stored
/// forms of the asset's LATEST revision: that is the asset as it is now, which is what a
/// filter created against it has to agree with. Entries are collected in the order the
/// forms declare them, with repeats dropped, and a column named by a bare string is
/// taken as that name, so a form that lists `["id"]` and one that lists
/// `[{"columnName": "id"}]` declare the same column.
///
/// `None` means NOT KNOWN, not empty: an asset whose forms declare no `columns` array
/// says nothing about its schema, and a filter over it is therefore not contradicted by
/// it. An asset that declares an EMPTY array declares a schema with no columns.
///
/// Linear in the number of entries the forms declare: membership is decided through a
/// set, not by rescanning what has been collected.
pub fn asset_schema_columns(asset: &Asset) -> Option<Vec<String>> {
    let mut columns: Vec<String> = Vec::new();
    let mut seen: std::collections::HashSet<String> = std::collections::HashSet::new();
    let mut declared = false;
    for form in asset
        .latest_revision()
        .map(|revision| revision.forms.as_slice())
        .unwrap_or_default()
    {
        let Some(content) = form.content.as_deref() else {
            continue;
        };
        let Ok(parsed) = serde_json::from_str::<serde_json::Value>(content) else {
            continue;
        };
        let Some(listed) = parsed.get("columns").and_then(serde_json::Value::as_array) else {
            continue;
        };
        declared = true;
        for entry in listed {
            let name = match entry {
                serde_json::Value::String(name) => Some(name.clone()),
                serde_json::Value::Object(_) => entry
                    .get("columnName")
                    .and_then(serde_json::Value::as_str)
                    .map(str::to_string),
                _ => None,
            };
            if let Some(name) = name {
                if seen.insert(name.clone()) {
                    columns.push(name);
                }
            }
        }
    }
    declared.then_some(columns)
}

/// What an asset filter's configuration, evaluated against the asset's schema, makes of
/// it: the derived members every asset-filter operation reports.
#[derive(Debug, Clone)]
pub struct AssetFilterEvaluation {
    pub status: String,
    pub error_message: Option<String>,
    pub effective_column_names: Option<Vec<String>>,
    pub effective_row_filter: Option<String>,
}

/// The derived members a stored filter is reported with, against the asset's schema as
/// it is NOW ([`asset_schema_columns`]).
///
/// Derived on every report rather than stored, so a filter's verdict follows the asset:
/// a filter naming a column the asset did not have is `INVALID`, and becomes `VALID`
/// once a later revision of that asset declares the column. Shared by every operation
/// that reports a filter, so `CreateAssetFilter` and `GetAssetFilter` cannot disagree
/// about one.
///
/// Takes the schema rather than the asset so a handler can read both out of the store,
/// RELEASE the store guard, and only then derive: the walk is bounded
/// ([`MAX_ROW_FILTER_NODES`], [`MAX_INCLUDED_COLUMN_NAMES`]) but it is not work to do
/// while every other caller of this service waits on the same lock.
///
/// "The asset must have the referenced columns available in its schema for column-based
/// filtering" (smithy_query doc shape="CreateAssetFilter"), so a filter is
/// [`FILTER_STATUS_VALID`] exactly when every column it names is available in the
/// asset's schema, and [`FILTER_STATUS_INVALID`] otherwise, with an `errorMessage`
/// naming the columns that are not. A schema of `None` is NOT KNOWN, so it contradicts
/// nothing and every named column stands.
///
/// A column filter's `effectiveColumnNames` are the included columns the asset actually
/// has — "the column names in the asset filter" (smithy_query doc
/// shape="CreateAssetFilterOutput" member="effectiveColumnNames") — and a row filter's
/// `effectiveRowFilter` is its predicate ([`render_row_filter`]). Each configuration
/// fills the member that belongs to it and leaves the other absent: a row filter selects
/// rows rather than columns, and a column filter names no rows.
pub fn evaluate_asset_filter(
    configuration: &crate::ops::types::AssetFilterConfiguration,
    schema: Option<&[String]>,
) -> AssetFilterEvaluation {
    use crate::ops::types::AssetFilterConfiguration as Config;

    // Built once per evaluation, so deciding whether a column is in the schema is a
    // hash lookup rather than a scan of the schema per named column.
    let known: Option<std::collections::HashSet<&str>> =
        schema.map(|columns| columns.iter().map(String::as_str).collect());
    let available =
        |name: &String| known.as_ref().is_none_or(|columns| columns.contains(name.as_str()));
    match configuration {
        Config::columnConfiguration(column) => {
            let mut requested: Vec<String> = Vec::new();
            let mut seen: std::collections::HashSet<&str> = std::collections::HashSet::new();
            // Bounded by the cap the handler refuses a longer list with, so no stored
            // filter makes this walk unbounded either.
            for name in column
                .included_column_names
                .iter()
                .flatten()
                .take(MAX_INCLUDED_COLUMN_NAMES)
            {
                if seen.insert(name.as_str()) {
                    requested.push(name.clone());
                }
            }
            let (effective, missing): (Vec<String>, Vec<String>) =
                requested.into_iter().partition(available);
            AssetFilterEvaluation {
                status: filter_status(&missing).to_string(),
                error_message: unavailable_columns_message(&missing),
                effective_column_names: Some(effective),
                effective_row_filter: None,
            }
        }
        Config::rowConfiguration(row) => {
            let missing: Vec<String> = row_filter_column_names(&row.row_filter)
                .into_iter()
                .filter(|name| !available(name))
                .collect();
            AssetFilterEvaluation {
                status: filter_status(&missing).to_string(),
                error_message: unavailable_columns_message(&missing),
                effective_column_names: None,
                effective_row_filter: Some(render_row_filter(&row.row_filter)),
            }
        }
    }
}

/// The `FilterStatus` a filter missing `missing` columns from the asset's schema carries.
fn filter_status(missing: &[String]) -> &'static str {
    if missing.is_empty() {
        FILTER_STATUS_VALID
    } else {
        FILTER_STATUS_INVALID
    }
}

/// The `errorMessage` an [`FILTER_STATUS_INVALID`] filter reports — "the error message
/// that is displayed if the asset filter is not created successfully" (smithy_query doc
/// shape="CreateAssetFilterOutput" member="errorMessage").
///
/// Built from the request and the asset's stored schema: it names the columns the filter
/// asked for that the asset does not have, which is exactly why the filter is invalid.
/// `None` when nothing is missing, so a valid filter carries no error message.
fn unavailable_columns_message(missing: &[String]) -> Option<String> {
    if missing.is_empty() {
        return None;
    }
    Some(format!(
        "The following columns are not available in the asset's schema: {}",
        missing.join(", ")
    ))
}

/// One stored policy grant: an authorization policy of one `policyType`, held by one
/// principal, on one entity (`entityType` + `entityIdentifier`) inside a domain.
///
/// `principal` and `detail` are kept as the modelled unions the request carried, so
/// `ListPolicyGrants` reports back exactly what `AddPolicyGrant` persisted rather than
/// a lossy projection of it.
#[derive(Debug, Clone)]
pub struct PolicyGrant {
    /// The grant's own id, minted by [`ComAmazonawsDatazoneDatazoneState::mint_grant_id`].
    pub grant_id: String,
    /// `TargetEntityType`: ASSET_TYPE | DOMAIN_UNIT | ENVIRONMENT_BLUEPRINT_CONFIGURATION
    /// | ENVIRONMENT_PROFILE (model enum `com.amazonaws.datazone#TargetEntityType`).
    pub entity_type: String,
    /// The id of the entity the grant is attached to.
    pub entity_identifier: String,
    /// `ManagedPolicyType` (model enum `com.amazonaws.datazone#ManagedPolicyType`).
    pub policy_type: String,
    pub principal: crate::ops::types::PolicyGrantPrincipal,
    pub detail: crate::ops::types::PolicyGrantDetail,
    pub created_at_ms: i64,
    pub created_by: String,
    /// The `@idempotencyToken` the request that created this grant carried, so a replay
    /// of that request answers with this grant instead of adding a second one. Stored on
    /// the grant (rather than in a side map) so the token is released with the grant.
    pub client_token: Option<String>,
}

/// The canonical key of a `PolicyGrantPrincipal`: two principals are the same principal
/// exactly when their keys are equal.
///
/// `RemovePolicyGrant` identifies the grant to drop by its principal, and the modelled
/// unions and structures carry no `PartialEq`, so the comparison is made explicit here:
/// every member of every variant is rendered, separated by a byte no identifier can
/// contain (`\x1f`), so no two different principals can collide.
pub fn principal_key(principal: &crate::ops::types::PolicyGrantPrincipal) -> String {
    use crate::ops::types::{
        DomainUnitGrantFilter, GroupPolicyGrantPrincipal, PolicyGrantPrincipal, ProjectGrantFilter,
        UserPolicyGrantPrincipal,
    };
    const SEP: char = '\x1f';
    match principal {
        PolicyGrantPrincipal::User(UserPolicyGrantPrincipal::userIdentifier(id)) => {
            format!("user{SEP}userIdentifier{SEP}{id}")
        }
        PolicyGrantPrincipal::User(UserPolicyGrantPrincipal::allUsersGrantFilter(_)) => {
            format!("user{SEP}allUsersGrantFilter")
        }
        PolicyGrantPrincipal::Group(GroupPolicyGrantPrincipal::groupIdentifier(id)) => {
            format!("group{SEP}groupIdentifier{SEP}{id}")
        }
        PolicyGrantPrincipal::Project(project) => {
            let filter = match project.project_grant_filter.as_ref() {
                Some(ProjectGrantFilter::domainUnitFilter(f)) => format!(
                    "domainUnitFilter{SEP}{}{SEP}{}",
                    f.domain_unit,
                    f.include_child_domain_units.unwrap_or(false)
                ),
                None => String::new(),
            };
            format!(
                "project{SEP}{}{SEP}{}{SEP}{}{SEP}{filter}",
                project.project_designation,
                project.user_created_designation_id.as_deref().unwrap_or(""),
                project.project_identifier.as_deref().unwrap_or(""),
            )
        }
        PolicyGrantPrincipal::domainUnit(unit) => {
            let filter = match unit.domain_unit_grant_filter.as_ref() {
                Some(DomainUnitGrantFilter::allDomainUnitsGrantFilter(_)) => {
                    "allDomainUnitsGrantFilter"
                }
                None => "",
            };
            format!(
                "domainUnit{SEP}{}{SEP}{}{SEP}{filter}",
                unit.domain_unit_designation,
                unit.domain_unit_identifier.as_deref().unwrap_or(""),
            )
        }
    }
}

/// The `ManagedPolicyType` a `PolicyGrantDetail` member describes.
///
/// The model declares the two in one-to-one correspondence: every `ManagedPolicyType`
/// value has the `PolicyGrantDetail` member of the same name (smithy_query
/// shape="ManagedPolicyType" / shape="PolicyGrantDetail"), so a request whose `detail`
/// is not the member its `policyType` names describes a different policy from the one it
/// asks to grant.
pub fn policy_type_of_detail(detail: &crate::ops::types::PolicyGrantDetail) -> &'static str {
    use crate::ops::types::PolicyGrantDetail as D;
    match detail {
        D::createDomainUnit(_) => "CREATE_DOMAIN_UNIT",
        D::overrideDomainUnitOwners(_) => "OVERRIDE_DOMAIN_UNIT_OWNERS",
        D::addToProjectMemberPool(_) => "ADD_TO_PROJECT_MEMBER_POOL",
        D::overrideProjectOwners(_) => "OVERRIDE_PROJECT_OWNERS",
        D::createGlossary(_) => "CREATE_GLOSSARY",
        D::createFormType(_) => "CREATE_FORM_TYPE",
        D::createAssetType(_) => "CREATE_ASSET_TYPE",
        D::createProject(_) => "CREATE_PROJECT",
        D::createEnvironmentProfile(_) => "CREATE_ENVIRONMENT_PROFILE",
        D::delegateCreateEnvironmentProfile(_) => "DELEGATE_CREATE_ENVIRONMENT_PROFILE",
        D::createEnvironment(_) => "CREATE_ENVIRONMENT",
        D::createEnvironmentFromBlueprint(_) => "CREATE_ENVIRONMENT_FROM_BLUEPRINT",
        D::createProjectFromProjectProfile(_) => "CREATE_PROJECT_FROM_PROJECT_PROFILE",
        D::useAssetType(_) => "USE_ASSET_TYPE",
    }
}

/// One stored DataZone asset type, at one revision.
///
/// An asset type is a named, revisioned schema: the metadata forms an asset of that
/// type carries. `CreateAssetType` mints revision [`FIRST_REVISION`] and the model
/// declares no update operation for it, so a stored type keeps the revision it was
/// created at.
#[derive(Debug, Clone)]
pub struct AssetType {
    pub name: String,
    pub domain_id: String,
    pub revision: String,
    pub description: Option<String>,
    /// The metadata forms attached to the type, keyed by form name. A `BTreeMap` so the
    /// forms are reported in one stable order whatever order the request listed them in;
    /// the modelled `FormsOutputMap` is bounded at 10 entries, which the request-level
    /// constraint on `formsInput` already enforces.
    pub forms: BTreeMap<String, AssetTypeForm>,
    pub owning_project_id: String,
    pub created_at_ms: i64,
    pub created_by: String,
    pub updated_at_ms: i64,
    pub updated_by: String,
}

/// One metadata form attached to an asset type: the stored form of the modelled
/// `FormEntryOutput` (`typeName`, `typeRevision`, `required`).
#[derive(Debug, Clone)]
pub struct AssetTypeForm {
    pub type_name: String,
    pub type_revision: String,
    pub required: Option<bool>,
}

/// The most domains this emulator stores.
///
/// A ceiling, not a guess at AWS's published quota: it exists so a create loop cannot
/// grow the emulator's heap — or the per-request [`ComAmazonawsDatazoneDatazoneState::settle_clock`]
/// scan — without bound. `CreateDomain` models `ServiceQuotaExceededException`, which
/// is the refusal once the store is full.
pub const MAX_DOMAINS: usize = 100;

/// The most projects this emulator stores in one domain, for the same reason and with
/// the same refusal (`CreateProject` models `ServiceQuotaExceededException` too).
pub const MAX_PROJECTS_PER_DOMAIN: usize = 500;

/// The most metadata form types this emulator stores in one domain, for the same
/// reason and with the same refusal (`CreateFormType` models
/// `ServiceQuotaExceededException` too).
pub const MAX_FORM_TYPES_PER_DOMAIN: usize = 500;

/// The most revisions this emulator keeps for one metadata form type. Re-creating a
/// name appends a revision, so this is the bound on that caller-driven growth; the
/// refusal past it is `CreateFormType`'s `ServiceQuotaExceededException`.
pub const MAX_FORM_TYPE_REVISIONS: usize = 100;

/// The most domain units this emulator stores in one domain, for the same reason and
/// with the same refusal (`CreateDomainUnit` models `ServiceQuotaExceededException`).
/// It also bounds the depth of the hierarchy, since every unit but the root hangs
/// under a parent that is itself stored.
pub const MAX_DOMAIN_UNITS_PER_DOMAIN: usize = 100;

/// The most deleted-domain-unit ids this emulator remembers per domain
/// ([`Domain::deleted_domain_unit_ids`]). Every id it holds is one a unit the store no
/// longer has once carried, so the whole hierarchy a domain can hold at once fits —
/// past that the oldest is forgotten and a call addressing it falls back to the
/// existence answer.
pub const MAX_DELETED_DOMAIN_UNITS_PER_DOMAIN: usize = MAX_DOMAIN_UNITS_PER_DOMAIN;

/// The most owners this emulator keeps on one entity, for the same reason and with the
/// same refusal (`AddEntityOwner` models `ServiceQuotaExceededException` too): adding
/// owners is caller-driven growth of a stored entity.
pub const MAX_OWNERS_PER_ENTITY: usize = 100;

/// The most assets this emulator stores in one domain, for the same reason and with the
/// same refusal (`CreateAsset` models `ServiceQuotaExceededException` too).
pub const MAX_ASSETS_PER_DOMAIN: usize = 500;

/// The most revisions this emulator records for one asset. Each `CreateAssetRevision`
/// appends one, so this is the bound on that caller-driven growth; `CreateAssetRevision`
/// models no `ServiceQuotaExceededException` (smithy_query
/// operation="CreateAssetRevision"), so the refusal past it is its `ValidationException`
/// — the rule `CreateProjectMembership` follows for a bound its own model does not name.
pub const MAX_ASSET_REVISIONS: usize = 100;

/// The most policy grants this emulator stores in one domain, for the same reason and
/// with the same refusal (`AddPolicyGrant` models `ServiceQuotaExceededException` too).
pub const MAX_POLICY_GRANTS_PER_DOMAIN: usize = 500;

/// The most members this emulator stores in one project, for the same reason: a
/// ceiling so an add loop cannot grow the store without bound. The refusal is each
/// operation's own — `ServiceQuotaExceededException` where it is modelled
/// (`CreateProject`), `ValidationException` where it is not
/// (`CreateProjectMembership`).
pub const MAX_MEMBERSHIPS_PER_PROJECT: usize = 200;

/// The most business glossaries this emulator stores in one domain, for the same reason
/// and with the same refusal (`CreateGlossary` models `ServiceQuotaExceededException`).
pub const MAX_GLOSSARIES_PER_DOMAIN: usize = 500;

/// The most business glossary terms this emulator stores in one domain, for the same
/// reason and with the same refusal (`CreateGlossaryTerm` models
/// `ServiceQuotaExceededException` too).
pub const MAX_GLOSSARY_TERMS_PER_DOMAIN: usize = 500;

/// The most asset types this emulator stores in one domain, for the same reason and
/// with the same refusal (`CreateAssetType` models `ServiceQuotaExceededException` too).
pub const MAX_ASSET_TYPES_PER_DOMAIN: usize = 500;

/// The most catalog listings this emulator stores in one domain, for the same reason
/// and with the same refusal (`CreateListingChangeSet` models
/// `ServiceQuotaExceededException` too). It also bounds the
/// [`Domain::listing_for_entity`] scan and the per-request
/// [`ComAmazonawsDatazoneDatazoneState::settle_clock`] walk over listings.
pub const MAX_LISTINGS_PER_DOMAIN: usize = 500;

/// The most revisions this emulator records for one listing. Publishing an entity that
/// already has a listing records a further revision, so this is the bound on that
/// caller-driven growth; the refusal past it is `CreateListingChangeSet`'s
/// `ServiceQuotaExceededException`.
pub const MAX_LISTING_REVISIONS: usize = 100;

/// The most account pools this emulator stores in one domain, for the same reason and
/// with the same refusal (`CreateAccountPool` models `ServiceQuotaExceededException`
/// too): a CHOSEN ceiling, not a published quota.
pub const MAX_ACCOUNT_POOLS_PER_DOMAIN: usize = 500;

/// The most tags this emulator keeps on one resource.
///
/// A CHOSEN ceiling, not a published quota: `TagResource` is a caller-driven growth
/// site (the keys come from the request), so the per-resource tag set has to be
/// bounded or a tag loop grows the emulator's heap without limit. The refusal once a
/// request would cross it is `ValidationException`, the client-input error
/// `TagResource` models.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The ARN namespace this service's resources live in: `arnNamespace` on the model's
/// `aws.api#service` trait (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "datazone";

/// Whether `candidate` has the shape of an ARN of THIS service —
/// `arn:<partition>:datazone:<region>:<account>:<resource>`.
///
/// `resourceArn` is modelled as a plain `String` with no pattern, so the boundary
/// validator passes any text at all and the handler is where a string that cannot
/// name a DataZone resource is recognised. Region and account may be empty (an ARN
/// is allowed to omit either), the partition and the resource part may not.
pub fn is_service_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    fields.next() == Some("arn")
        && fields.next().is_some_and(|partition| !partition.is_empty())
        && fields.next() == Some(ARN_NAMESPACE)
        && fields.next().is_some()
        && fields.next().is_some()
        && fields.next().is_some_and(|resource| !resource.is_empty())
}

/// The hard ceiling on remembered `@idempotencyToken`s.
///
/// The token keys are CALLER-supplied, so the map is a caller-driven growth site and
/// must be bounded: [`ComAmazonawsDatazoneDatazoneState::bound_idempotency`] first
/// releases every token whose resource is gone (such a token can never be replayed into
/// anything), then evicts the oldest entries beyond this ceiling. At most one token is
/// recorded per stored resource, and a token evicted here only costs its request the
/// idempotent replay — the resource it created stays stored and readable.
pub const MAX_IDEMPOTENCY_TOKENS: usize = MAX_DOMAINS * 16;

/// Which kind of resource a remembered `@idempotencyToken` created.
///
/// Part of the map KEY, not just the value: two operations can be handed the same
/// caller-chosen token string, and a `CreateGlossary` repeat must not consume
/// (or overwrite) the record a `CreateDomain` left under that token.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum IdempotentResource {
    Domain,
    DomainUnit,
    Glossary,
    GlossaryTerm,
    Asset,
    SubscriptionRequest,
}

/// The key one remembered `@idempotencyToken` is filed under.
///
/// TOTAL scoping, so no two creates can collide on one caller-chosen string: the
/// operation's resource kind, the domain the create is scoped to, and the token.
#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord)]
pub struct IdempotencyKey {
    kind: IdempotentResource,
    /// The domain the create happens IN — empty for a create that is scoped to the
    /// service rather than to a domain (a domain itself).
    domain_id: String,
    token: String,
}

impl IdempotencyKey {
    /// The key a `CreateDomain` token is filed under. A domain create is scoped to the
    /// service, so the key carries no domain.
    pub fn domain(token: &str) -> Self {
        Self {
            kind: IdempotentResource::Domain,
            domain_id: String::new(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateDomainUnit` token is filed under: the same token in another
    /// domain is a different key, so each domain keeps its own record.
    pub fn domain_unit(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::DomainUnit,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateGlossary` token is filed under: the same token in another
    /// domain is a different key, so each domain keeps its own record.
    pub fn glossary(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::Glossary,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateGlossaryTerm` token is filed under, scoped the same way: a term
    /// is created IN a domain, so the same token in another domain is a different key.
    pub fn glossary_term(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::GlossaryTerm,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateAsset` token is filed under, scoped the same way: an asset is
    /// created IN a domain, so the same token in another domain is a different key.
    pub fn asset(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::Asset,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }

    /// The key a `CreateSubscriptionRequest` token is filed under, scoped the same way:
    /// a subscription request is made IN a domain, so the same token in another domain
    /// is a different key.
    pub fn subscription_request(domain_id: &str, token: &str) -> Self {
        Self {
            kind: IdempotentResource::SubscriptionRequest,
            domain_id: domain_id.to_string(),
            token: token.to_string(),
        }
    }
}

/// What one remembered `@idempotencyToken` points at, plus when it was remembered so
/// the oldest can be evicted first.
#[derive(Debug, Clone)]
pub struct IdempotencyRecord {
    /// The id of the resource the first request carrying the token created, inside the
    /// scope its [`IdempotencyKey`] names.
    pub resource_id: String,
    pub recorded_at_ms: i64,
}

/// Per-service state for com-amazonaws-datazone-datazone. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsDatazoneDatazoneState {
    /// Every live domain, keyed by its `dzd_…` id. A `BTreeMap` so every listing
    /// iterates in one stable order and a pagination token can resume from a key.
    /// Bounded by [`MAX_DOMAINS`].
    pub domains: BTreeMap<String, Domain>,
    /// [`IdempotencyKey`] -> the resource the first request carrying that token created,
    /// so a repeated `@idempotencyToken` request returns that resource instead of
    /// creating a second one.
    ///
    /// Bounded: see [`MAX_IDEMPOTENCY_TOKENS`]. Write it through
    /// [`Self::record_idempotency`], which applies the bound, rather than inserting
    /// directly.
    idempotency: BTreeMap<IdempotencyKey, IdempotencyRecord>,
    /// Monotonic counter behind every minted id, so no two resources in one process
    /// ever share an id even when minted in the same millisecond.
    id_seq: u64,
}

impl ComAmazonawsDatazoneDatazoneState {
    /// The domain a `clientToken` already created, if that domain is still stored.
    pub fn idempotent_domain(&self, token: &str) -> Option<&Domain> {
        let record = self.idempotency.get(&IdempotencyKey::domain(token))?;
        self.domains.get(&record.resource_id)
    }

    /// The domain unit a `clientToken` already created in `domain_id`, if it is still
    /// stored. The domain is part of the key, so the same token against a different
    /// domain creates that domain's own unit and leaves this record alone.
    pub fn idempotent_domain_unit(&self, domain_id: &str, token: &str) -> Option<&DomainUnit> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::domain_unit(domain_id, token))?;
        self.domains
            .get(domain_id)?
            .domain_units
            .get(&record.resource_id)
    }

    /// The glossary a `clientToken` already created in `domain_id`, if it is still
    /// stored. The domain is part of the key, so the same token against a different
    /// domain creates that domain's own glossary and leaves this record alone.
    pub fn idempotent_glossary(&self, domain_id: &str, token: &str) -> Option<&Glossary> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::glossary(domain_id, token))?;
        self.domains
            .get(domain_id)?
            .glossaries
            .get(&record.resource_id)
    }

    /// The glossary term a `clientToken` already created in `domain_id`, if it is still
    /// stored. Scoped to the domain the same way a glossary token is.
    pub fn idempotent_glossary_term(&self, domain_id: &str, token: &str) -> Option<&GlossaryTerm> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::glossary_term(domain_id, token))?;
        self.domains
            .get(domain_id)?
            .glossary_terms
            .get(&record.resource_id)
    }

    /// The asset a `clientToken` already created in `domain_id`, if it is still stored.
    /// Scoped to the domain the same way a glossary token is.
    pub fn idempotent_asset(&self, domain_id: &str, token: &str) -> Option<&Asset> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::asset(domain_id, token))?;
        self.domains.get(domain_id)?.assets.get(&record.resource_id)
    }

    /// The subscription request a `clientToken` already made in `domain_id`, if it is
    /// still stored. Scoped to the domain the same way a glossary token is.
    pub fn idempotent_subscription_request(
        &self,
        domain_id: &str,
        token: &str,
    ) -> Option<&SubscriptionRequest> {
        let record = self
            .idempotency
            .get(&IdempotencyKey::subscription_request(domain_id, token))?;
        self.domains
            .get(domain_id)?
            .subscription_requests
            .get(&record.resource_id)
    }

    /// Whether a remembered token still resolves to something this store holds. One
    /// lookup per level, because the key carries the owning domain.
    fn record_resolves(&self, key: &IdempotencyKey, record: &IdempotencyRecord) -> bool {
        match key.kind {
            IdempotentResource::Domain => self.domains.contains_key(&record.resource_id),
            IdempotentResource::DomainUnit => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.domain_units.contains_key(&record.resource_id)),
            IdempotentResource::Glossary => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.glossaries.contains_key(&record.resource_id)),
            IdempotentResource::GlossaryTerm => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.glossary_terms.contains_key(&record.resource_id)),
            IdempotentResource::Asset => self
                .domains
                .get(&key.domain_id)
                .is_some_and(|domain| domain.assets.contains_key(&record.resource_id)),
            IdempotentResource::SubscriptionRequest => {
                self.domains.get(&key.domain_id).is_some_and(|domain| {
                    domain
                        .subscription_requests
                        .contains_key(&record.resource_id)
                })
            }
        }
    }

    /// Remember that the token `key` names created `resource_id`, and re-apply the
    /// bound.
    ///
    /// Call it AFTER the resource is stored: a token naming a resource that is not in
    /// the store is released immediately by the bound below.
    pub fn record_idempotency(&mut self, key: IdempotencyKey, resource_id: String, now: i64) {
        self.idempotency.insert(
            key,
            IdempotencyRecord {
                resource_id,
                recorded_at_ms: now,
            },
        );
        self.bound_idempotency();
    }

    /// Keep the remembered-token map bounded: release every token whose resource is no
    /// longer stored, then evict the oldest entries beyond [`MAX_IDEMPOTENCY_TOKENS`].
    ///
    /// Reached from [`Self::record_idempotency`] and from [`Self::settle_clock`] (which
    /// every handler calls), so a create/delete loop with fresh tokens releases each
    /// token as its resource disappears and can never grow the store without limit.
    fn bound_idempotency(&mut self) {
        let released: Vec<IdempotencyKey> = self
            .idempotency
            .iter()
            .filter(|(key, record)| !self.record_resolves(key, record))
            .map(|(key, _)| key.clone())
            .collect();
        for key in released {
            self.idempotency.remove(&key);
        }
        let Some(excess) = self
            .idempotency
            .len()
            .checked_sub(MAX_IDEMPOTENCY_TOKENS)
            .filter(|excess| *excess > 0)
        else {
            return;
        };
        let mut by_age: Vec<(i64, IdempotencyKey)> = self
            .idempotency
            .iter()
            .map(|(key, record)| (record.recorded_at_ms, key.clone()))
            .collect();
        by_age.sort();
        for (_, key) in by_age.into_iter().take(excess) {
            self.idempotency.remove(&key);
        }
    }
    /// Mint an opaque id suffix: lowercase base-36, so it satisfies both the
    /// `^[a-zA-Z0-9_-]{1,36}$` (project, domain) and `^[a-z0-9_\-]+$` (domain unit)
    /// patterns the model declares. Unique within a process, and never reused.
    pub fn mint_id(&mut self, now: i64) -> String {
        self.id_seq += 1;
        format!(
            "{}{}",
            base36(u64::try_from(now).unwrap_or(0)),
            base36(self.id_seq)
        )
    }

    /// Mint a `GrantIdentifier`: exactly 10 characters, which is what the model's
    /// `^[A-Za-z0-9+/]{10}$` pattern (smithy_query shape="GrantIdentifier") admits.
    ///
    /// The value encoded is the mint instant in the high digits and the per-process
    /// counter in the low ones, so two grants minted in the same millisecond still get
    /// different ids. Only the 62 alphanumeric characters of the pattern's class are
    /// used, so an id is safe to carry in a URI as well as in a body.
    pub fn mint_grant_id(&mut self, now: i64) -> String {
        /// A subset of the pattern's character class, in ASCII order.
        const ALPHABET: &[u8; 62] =
            b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZabcdefghijklmnopqrstuvwxyz";
        const LEN: usize = 10;

        self.id_seq += 1;
        // 62^10 is about 8.4e17, so the encoded value must stay below that: 1e11
        // milliseconds of clock (about 3 years of distinct instants) times 1e6
        // counter values is 1e17.
        let mut value = u64::try_from(now).unwrap_or(0) % 100_000_000_000 * 1_000_000
            + self.id_seq % 1_000_000;
        let mut out = [ALPHABET[0]; LEN];
        for slot in out.iter_mut().rev() {
            *slot = ALPHABET[usize::try_from(value % 62).unwrap_or(0)];
            value /= 62;
        }
        // The buffer holds one ASCII byte per digit by construction.
        String::from_utf8(out.to_vec()).expect("the alphabet is ASCII")
    }

    /// Mint a `UserProfileId`: five groups of hex digits, `8-4-4-4-12`, which is what
    /// the model's
    /// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
    /// pattern (smithy_query shape="UserProfileId") admits. The optional leading group
    /// belongs to an id an identity store minted, so an id minted HERE omits it.
    ///
    /// The 32 digits are the mint instant in the high half and the per-process counter
    /// in the low one, so two profiles minted in the same millisecond still get
    /// different ids, and no id is ever reused.
    pub fn mint_user_profile_id(&mut self, now: i64) -> String {
        self.id_seq += 1;
        let digits = format!(
            "{:016x}{:016x}",
            u64::try_from(now).unwrap_or(0),
            self.id_seq
        );
        // 32 ASCII hex digits by construction, so every group below is in bounds.
        format!(
            "{}-{}-{}-{}-{}",
            &digits[0..8],
            &digits[8..12],
            &digits[12..16],
            &digits[16..20],
            &digits[20..32]
        )
    }

    /// Materialise every domain lifecycle step the clock already owes at `now`: a
    /// `CREATING` domain whose span has elapsed becomes `AVAILABLE`, a `DELETING` one
    /// disappears together with its projects, and a `CREATING` listing whose span has
    /// elapsed becomes `ACTIVE`.
    ///
    /// This is the ONLY write a reporting operation makes, and it writes exactly what
    /// was already owed at this instant whether or not anybody called.
    pub fn settle_clock(&mut self, now: i64) {
        let due: Vec<String> = self
            .domains
            .iter()
            .filter(|(_, d)| d.status_due_ms.is_some_and(|due| due <= now))
            .map(|(id, _)| id.clone())
            .collect();
        for id in due {
            let Some(domain) = self.domains.get_mut(&id) else {
                continue;
            };
            let due_at = domain.status_due_ms.unwrap_or(now);
            match domain.status.as_str() {
                DOMAIN_STATUS_CREATING => {
                    domain.status = DOMAIN_STATUS_AVAILABLE.to_string();
                    domain.status_due_ms = None;
                    domain.last_updated_at_ms = due_at;
                }
                DOMAIN_STATUS_DELETING => {
                    // The deletion the earlier DeleteDomain call started has
                    // completed: the domain and the projects under it are gone.
                    self.domains.remove(&id);
                }
                // No other status carries a due instant.
                _ => domain.status_due_ms = None,
            }
        }
        // The publish a `CreateListingChangeSet` started has finished: the listing is
        // in the catalog, and the catalog's own record of the published entity is the
        // next revision of it ([`Listing::complete_publish`]). Bounded by
        // MAX_DOMAINS * MAX_LISTINGS_PER_DOMAIN, and run after the domain pass so a
        // domain that has just disappeared is not walked.
        for domain in self.domains.values_mut() {
            for listing in domain.listings.values_mut() {
                if let Some(due_at) = listing.status_due_ms.filter(|due| *due <= now) {
                    listing.complete_publish(due_at);
                }
            }
        }
        // A token naming a domain that has just disappeared can never be replayed
        // into anything, so it is released with the domain.
        self.bound_idempotency();
    }

    /// Whether a request scoped to the domain `domain_identifier` is authorized.
    ///
    /// A DataZone permission is held ON a domain, and the service decides a
    /// domain-scoped request by authorization BEFORE it resolves the domain or any
    /// resource inside it. So a caller holds nothing on an identifier this store serves
    /// no domain for, whatever that identifier looks like, and such a request is denied
    /// rather than answered with an existence report — see [`not_permitted_message`]
    /// for the refusal every domain-scoped operation builds from this.
    ///
    /// The status of a domain the store DOES hold is not part of this question: a
    /// request scoped to a stored domain is authorized, and what its own resource
    /// lookups then find (including a domain on its way out, which
    /// [`Self::live_domain`] excludes) is reported as existence.
    pub fn authorizes_domain(&self, domain_identifier: &str) -> bool {
        self.domains.contains_key(domain_identifier)
    }

    /// The domain `id`, when it exists and is not already being deleted.
    pub fn live_domain(&self, id: &str) -> Option<&Domain> {
        self.domains
            .get(id)
            .filter(|d| d.status != DOMAIN_STATUS_DELETING)
    }

    /// The domain unit `unit_id` stored in domain `domain_id`, when both exist.
    pub fn domain_unit(&self, domain_id: &str, unit_id: &str) -> Option<&DomainUnit> {
        self.domains.get(domain_id)?.domain_units.get(unit_id)
    }

    /// The asset `asset_id` stored in the live domain `domain_id`, when both exist.
    ///
    /// The domain liveness notion is [`Self::live_domain`]'s: a domain on its way out no
    /// longer takes a new asset filter, and its assets are about to go with it. Shared by
    /// every asset-filter handler so they resolve the pair by one rule.
    pub fn live_asset(&self, domain_id: &str, asset_id: &str) -> Option<&Asset> {
        self.live_domain(domain_id)?.assets.get(asset_id)
    }

    /// [`Self::live_asset`], for a call that writes the asset (adding a filter to it).
    pub fn live_asset_mut(&mut self, domain_id: &str, asset_id: &str) -> Option<&mut Asset> {
        self.domains
            .get_mut(domain_id)
            .filter(|domain| domain.status != DOMAIN_STATUS_DELETING)?
            .assets
            .get_mut(asset_id)
    }

    /// The entity an entity-owner call names, when it is stored.
    ///
    /// `DataZoneEntityType` has exactly one member, `DOMAIN_UNIT` (smithy_query
    /// shape="DataZoneEntityType"), so the entity owners are kept on is the domain unit
    /// `entity_id` of domain `domain_id`. `None` when `entity_type` is not that member
    /// or either resource is not stored, so `AddEntityOwner`, `ListEntityOwners` and
    /// `RemoveEntityOwner` all resolve the pair by one rule.
    pub fn owned_entity(
        &self,
        domain_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Option<&DomainUnit> {
        if entity_type != DATA_ZONE_ENTITY_TYPE_DOMAIN_UNIT {
            return None;
        }
        self.domain_unit(domain_id, entity_id)
    }

    /// [`Self::owned_entity`], for a call that writes the entity's owners.
    pub fn owned_entity_mut(
        &mut self,
        domain_id: &str,
        entity_type: &str,
        entity_id: &str,
    ) -> Option<&mut DomainUnit> {
        if entity_type != DATA_ZONE_ENTITY_TYPE_DOMAIN_UNIT {
            return None;
        }
        self.domains
            .get_mut(domain_id)?
            .domain_units
            .get_mut(entity_id)
    }

    /// The live domain that already carries `name`, if the account has one.
    ///
    /// A domain name is unique within the account, so this is what `CreateDomain`
    /// refuses on. Every domain in this store belongs to the one account the emulator
    /// serves (`Domain::managed_account_id`), so the whole store IS the account's
    /// namespace. A domain already being deleted is on its way out and no longer holds
    /// its name — the same liveness notion [`Self::live_domain`] applies.
    ///
    /// A linear scan, deliberately: the store is bounded by [`MAX_DOMAINS`] (the same
    /// bound `settle_clock`'s per-request scan relies on), so one liveness rule shared
    /// with [`Self::live_domain`] is worth more than a second index to keep in step.
    pub fn live_domain_named(&self, name: &str) -> Option<&Domain> {
        self.domains
            .values()
            .find(|d| d.name == name && d.status != DOMAIN_STATUS_DELETING)
    }

    /// The tag set of the resource `arn` names, when this emulator stores that
    /// resource.
    ///
    /// The lookup is by the ARN the store itself minted for the resource, so every
    /// stored resource that carries one is reachable by exactly the ARN its own
    /// create/get reports — and a resource in a non-terminal status is still stored,
    /// so it is still taggable until its delete completes. That is deliberately the
    /// reach of `GetDomain`, which reports a `DELETING` domain and its tags rather
    /// than 404ing on it, so the tagging operations and the resource's own reader
    /// agree on which resources exist; [`Self::live_domain`] is the narrower lookup,
    /// for operations that must not hang a NEW resource off a dying domain.
    ///
    /// Only the domain is reachable today, because it is the only resource this store
    /// keeps an ARN for: [`Project`] and [`AssetType`] mint none, so no DataZone
    /// resource ARN can name one. (A project's `resource_tags` field is the modelled
    /// `resourceTags` member — project metadata, not the AWS resource tags these
    /// operations manage.) A later resource that stores an `arn` and a tag set joins
    /// this same lookup rather than getting a tag map of its own.
    pub fn resource_tags(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        self.domains
            .values()
            .find(|domain| domain.arn == arn)
            .map(|domain| &domain.tags)
    }

    /// The same lookup as [`Self::resource_tags`], for a tag write.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        self.domains
            .values_mut()
            .find(|domain| domain.arn == arn)
            .map(|domain| &mut domain.tags)
    }
}

/// The ROOT domain unit a domain is created with.
///
/// `CreateDomain` reports a `rootDomainUnitId`, so the unit it names exists from the
/// moment the domain does: it is the one unit in the domain with no parent, it is
/// owned by the principal that created the domain, and it is the parent every other
/// unit's chain of ancestors ends at.
pub fn root_domain_unit(domain: &Domain, created_by: &str, now: i64) -> DomainUnit {
    DomainUnit {
        id: domain.root_domain_unit_id.clone(),
        domain_id: domain.id.clone(),
        name: domain_unit_name_from(&domain.name, &domain.id),
        description: None,
        parent_domain_unit_id: None,
        owners: vec![DomainUnitOwner::User(created_by.to_string())],
        created_by: created_by.to_string(),
        created_at_ms: now,
        last_updated_by: None,
        last_updated_at_ms: None,
    }
}

/// A `DomainUnitName` for the root unit of a domain called `domain_name`.
///
/// The root unit carries the domain's own name, but `DomainName` declares no pattern
/// while `DomainUnitName` is `^[\w -]+$` with length 1..128 (smithy_query
/// shape="DomainUnitName"), so a character outside that class becomes `_`, the result
/// is clipped to 128 characters, and a name that is empty once clipped falls back to
/// the domain id — itself inside the class.
///
/// The class kept here is deliberately NARROWER than the modelled one: `\w` admits
/// Unicode word characters, and this keeps only ASCII alphanumerics, `_`, `-` and the
/// space, so every name it produces satisfies the model while a non-ASCII letter is
/// replaced rather than passed through.
fn domain_unit_name_from(domain_name: &str, domain_id: &str) -> String {
    let name: String = domain_name
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || c == '_' || c == '-' || c == ' ' {
                c
            } else {
                '_'
            }
        })
        .take(128)
        .collect();
    if name.is_empty() {
        domain_id.to_string()
    } else {
        name
    }
}

/// One account pool created in a domain.
///
/// `accountSource` is kept as the modelled union itself (the trade
/// [`AssetFilter::configuration`] makes): either a static list of accounts or a custom
/// handler, reported back by the pool's reader exactly as it was stored. The model
/// gives an account pool no status member (smithy_query operation="GetAccountPool"), so
/// a pool is reported in full from the moment it is stored and owes the clock no
/// transition.
#[derive(Debug, Clone)]
pub struct AccountPool {
    /// The pool's own `AccountPoolId`, minted by
    /// [`ComAmazonawsDatazoneDatazoneState::mint_id`].
    pub id: String,
    pub domain_id: String,
    pub name: String,
    pub description: Option<String>,
    /// A `ResolutionStrategy` member — see [`RESOLUTION_STRATEGY_MANUAL`].
    pub resolution_strategy: String,
    pub account_source: crate::ops::types::AccountSource,
    /// The domain unit the pool belongs to. `CreateAccountPool` takes none, so a new
    /// pool belongs to the domain's ROOT unit ([`Domain::root_domain_unit_id`]) — the
    /// unit a resource created without an explicit one hangs under.
    pub domain_unit_id: String,
    pub created_by: String,
    pub created_at_ms: i64,
    /// Moves with every update of the pool; equal to [`Self::created_at_ms`] until
    /// something updates it.
    pub last_updated_at_ms: i64,
    /// The principal of the last update, absent until something updates the pool.
    pub updated_by: Option<String>,
}

/// The one `ResolutionStrategy` member (model enum
/// `com.amazonaws.datazone#ResolutionStrategy`: MANUAL) — the mechanism that resolves
/// an account selection from a pool.
pub const RESOLUTION_STRATEGY_MANUAL: &str = "MANUAL";

/// `AccountInfoList`'s modelled length, 1..25 (smithy_query shape="AccountInfoList").
pub const ACCOUNTS_PER_SOURCE: std::ops::RangeInclusive<usize> = 1..=25;

/// `AwsRegionList`'s modelled length, 1..3 (smithy_query shape="AwsRegionList").
pub const REGIONS_PER_ACCOUNT: std::ops::RangeInclusive<usize> = 1..=3;

/// `AwsAccountName`'s modelled length, 1..256 (smithy_query shape="AccountInfo").
pub const AWS_ACCOUNT_NAME_LEN: std::ops::RangeInclusive<usize> = 1..=256;

/// Whether `account_source` satisfies the constraints the model declares INSIDE the
/// `AccountSource` union, answering the member and the constraint it fails otherwise.
///
/// Every handler that takes an `accountSource` checks it through this, so the two forms
/// of the union are judged by one rule.
///
/// The reason it is checked here at all: the vendored boundary validator walks a
/// structure's members (`vendor/smithy-schema-server/src/constraints.rs`) but not the
/// active member of a UNION, so the `@length` and `@pattern` traits the model declares
/// under `AccountSource` would otherwise reach the store unchecked. The refusal is the
/// `ValidationException` these operations model, and its message names the offending
/// member and the constraint only — never the value the caller sent (`AccountPoolName`
/// and `AwsAccountName` are `@sensitive`).
pub fn validate_account_source(
    account_source: &crate::ops::types::AccountSource,
) -> Result<(), String> {
    match account_source {
        crate::ops::types::AccountSource::Accounts(accounts) => {
            if !ACCOUNTS_PER_SOURCE.contains(&accounts.len()) {
                return Err(format!(
                    "accountSource.accounts must hold between {} and {} accounts",
                    ACCOUNTS_PER_SOURCE.start(),
                    ACCOUNTS_PER_SOURCE.end()
                ));
            }
            for (index, account) in accounts.iter().enumerate() {
                // `AwsAccountId` is `^\d{12}$` (smithy_query shape="AwsAccountId").
                if !is_aws_account_id(&account.aws_account_id) {
                    return Err(format!(
                        "accountSource.accounts[{index}].awsAccountId must be twelve digits"
                    ));
                }
                if !REGIONS_PER_ACCOUNT.contains(&account.supported_regions.len()) {
                    return Err(format!(
                        "accountSource.accounts[{index}].supportedRegions must hold between {} and {} regions",
                        REGIONS_PER_ACCOUNT.start(),
                        REGIONS_PER_ACCOUNT.end()
                    ));
                }
                for (slot, region) in account.supported_regions.iter().enumerate() {
                    if !is_aws_region(region) {
                        return Err(format!(
                            "accountSource.accounts[{index}].supportedRegions[{slot}] must be an Amazon Web Services Region identifier"
                        ));
                    }
                }
                if account
                    .aws_account_name
                    .as_deref()
                    .is_some_and(|name| !AWS_ACCOUNT_NAME_LEN.contains(&name.chars().count()))
                {
                    return Err(format!(
                        "accountSource.accounts[{index}].awsAccountName must be between {} and {} characters",
                        AWS_ACCOUNT_NAME_LEN.start(),
                        AWS_ACCOUNT_NAME_LEN.end()
                    ));
                }
            }
            Ok(())
        }
        crate::ops::types::AccountSource::customAccountPoolHandler(handler) => {
            if !is_lambda_function_arn(&handler.lambda_function_arn) {
                return Err(
                    "accountSource.customAccountPoolHandler.lambdaFunctionArn must be the ARN of a Lambda function"
                        .to_string(),
                );
            }
            if handler
                .lambda_execution_role_arn
                .as_deref()
                .is_some_and(|arn| !is_iam_role_arn(arn))
            {
                return Err(
                    "accountSource.customAccountPoolHandler.lambdaExecutionRoleArn must be the ARN of an IAM role"
                        .to_string(),
                );
            }
            Ok(())
        }
    }
}

/// Whether `candidate` is an account id: `AwsAccountId`'s modelled `^\d{12}$`
/// (smithy_query shape="AwsAccountId"). A Smithy `@pattern` is ECMA-262, where `\d` is
/// the ASCII digit class.
fn is_aws_account_id(candidate: &str) -> bool {
    candidate.len() == 12 && candidate.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `candidate` is a Region identifier: `AwsRegion`'s modelled
/// `^[a-z]{2}-[a-z]{4,10}-\d$` (smithy_query shape="AwsRegion"), read off the pattern
/// field by field — two lowercase letters, 4..10 lowercase letters, one digit.
fn is_aws_region(candidate: &str) -> bool {
    let lowercase = |field: &str, len: std::ops::RangeInclusive<usize>| {
        len.contains(&field.len()) && field.bytes().all(|byte| byte.is_ascii_lowercase())
    };
    let mut fields = candidate.split('-');
    let (Some(area), Some(direction), Some(ordinal), None) =
        (fields.next(), fields.next(), fields.next(), fields.next())
    else {
        return false;
    };
    lowercase(area, 2..=2)
        && lowercase(direction, 4..=10)
        && ordinal.len() == 1
        && ordinal.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `candidate` is the ARN of a Lambda function, as `LambdaFunctionArn`'s
/// modelled pattern describes one (smithy_query shape="CustomAccountPoolHandler"):
/// `arn`, one of the three partitions the pattern names, the `lambda` service, a Region
/// (the pattern admits a `-gov-` one here), a 12-digit account, then `function` and the
/// function's name, optionally followed by one qualifier and/or a `$`-prefixed version.
fn is_lambda_function_arn(candidate: &str) -> bool {
    let mut fields = candidate.split(':');
    if fields.next() != Some("arn")
        || !matches!(fields.next(), Some("aws" | "aws-cn" | "aws-us-gov"))
        || fields.next() != Some("lambda")
        || !fields.next().is_some_and(is_lambda_arn_region)
        || !fields.next().is_some_and(is_aws_account_id)
        || fields.next() != Some("function")
    {
        return false;
    }
    let name_and_qualifier: Vec<&str> = fields.collect();
    if !(1..=2).contains(&name_and_qualifier.len()) {
        return false;
    }
    // `[a-zA-Z0-9-_]+` per field, and the LAST field may carry the pattern's trailing
    // `(?:\$[\w-]+)?` version suffix.
    let last = name_and_qualifier.len() - 1;
    name_and_qualifier
        .iter()
        .enumerate()
        .all(|(index, field)| {
            let (field, version) = match field.split_once('$') {
                Some((field, version)) if index == last => (field, Some(version)),
                Some(_) => return false,
                None => (*field, None),
            };
            !field.is_empty()
                && field
                    .bytes()
                    .all(|byte| byte.is_ascii_alphanumeric() || byte == b'-' || byte == b'_')
                && version.is_none_or(|version| {
                    !version.is_empty()
                        && version
                            .bytes()
                            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_' || byte == b'-')
                })
        })
}

/// Whether `candidate` is the Region field of a Lambda function ARN:
/// `[a-z]{2}(?:-gov)?-[a-z]+-\d{1,}`, the sub-expression `LambdaFunctionArn`'s pattern
/// admits there — WIDER than [`is_aws_region`], which is `AwsRegion`'s own pattern.
fn is_lambda_arn_region(candidate: &str) -> bool {
    let mut fields: Vec<&str> = candidate.split('-').collect();
    if fields.len() == 4 {
        // The optional `-gov` infix, which the pattern allows only there.
        if fields[1] != "gov" {
            return false;
        }
        fields.remove(1);
    }
    let [area, direction, ordinal] = fields[..] else {
        return false;
    };
    area.len() == 2
        && area.bytes().all(|byte| byte.is_ascii_lowercase())
        && !direction.is_empty()
        && direction.bytes().all(|byte| byte.is_ascii_lowercase())
        && !ordinal.is_empty()
        && ordinal.bytes().all(|byte| byte.is_ascii_digit())
}

/// Whether `candidate` is the ARN of an IAM role, as `LambdaExecutionRoleArn`'s
/// modelled pattern describes one (smithy_query shape="CustomAccountPoolHandler"):
/// `^arn:aws[^:]*:iam::\d{12}:(role|role/service-role)/[\w+=,.@-]*$`.
fn is_iam_role_arn(candidate: &str) -> bool {
    let mut fields = candidate.splitn(6, ':');
    if fields.next() != Some("arn")
        || !fields.next().is_some_and(|partition| partition.starts_with("aws"))
        || fields.next() != Some("iam")
        || fields.next() != Some("")
        || !fields.next().is_some_and(is_aws_account_id)
    {
        return false;
    }
    let Some(resource) = fields.next() else {
        return false;
    };
    // The longer prefix first: the path of a service role carries the only `/` the
    // pattern's trailing class does not admit.
    let Some(name) = resource
        .strip_prefix("role/service-role/")
        .or_else(|| resource.strip_prefix("role/"))
    else {
        return false;
    };
    name.bytes().all(|byte| {
        byte.is_ascii_alphanumeric() || b"_+=,.@-".contains(&byte)
    })
}

/// A stored owner as the modelled `DomainUnitOwnerProperties` union
/// (smithy_query shape="DomainUnitOwnerProperties"), shared by every operation that
/// reports a domain unit's owners so the two forms cannot drift apart.
pub fn owner_properties(
    owner: &DomainUnitOwner,
) -> crate::ops::types::DomainUnitOwnerProperties {
    use crate::ops::types::{
        DomainUnitGroupProperties, DomainUnitOwnerProperties, DomainUnitUserProperties,
    };
    match owner {
        DomainUnitOwner::User(user_id) => {
            DomainUnitOwnerProperties::User(DomainUnitUserProperties {
                user_id: Some(user_id.clone()),
            })
        }
        DomainUnitOwner::Group(group_id) => {
            DomainUnitOwnerProperties::Group(DomainUnitGroupProperties {
                group_id: Some(group_id.clone()),
            })
        }
    }
}

/// The ids of `unit_id`'s ancestors in `domain`: its parent first, then that unit's
/// parent, up to and including the domain's root unit.
///
/// Empty for the root unit, which has no ancestor. Only ids with a stored record are
/// reported, and the walk is bounded by the number of stored units, so neither a
/// dangling parent pointer nor a cycle can make it report a unit that is not there or
/// run forever.
pub fn ancestor_domain_unit_ids(domain: &Domain, unit_id: &str) -> Vec<String> {
    let mut ancestors: Vec<String> = Vec::new();
    let mut cursor = domain
        .domain_units
        .get(unit_id)
        .and_then(|unit| unit.parent_domain_unit_id.clone());
    while let Some(id) = cursor {
        let Some(parent) = domain.domain_units.get(&id) else {
            break;
        };
        if ancestors.contains(&id) || ancestors.len() >= domain.domain_units.len() {
            break;
        }
        cursor = parent.parent_domain_unit_id.clone();
        ancestors.push(id);
    }
    ancestors
}

/// The resource kinds this service names in its not-found messages.
///
/// A closed set, so a handler cannot invent a third spelling of a kind or interpolate
/// a caller-supplied identifier into the message.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    Domain,
    DomainUnit,
    Project,
    AccountPool,
    AssetType,
    FormType,
    Glossary,
    GlossaryTerm,
    Asset,
    AssetFilter,
    DataProduct,
    Listing,
    UserProfile,
    GroupProfile,
    SubscriptionRequest,
    Subscription,
}

impl ResourceKind {
    /// The kind as DataZone names it in a message.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Domain => "Domain",
            Self::DomainUnit => "Domain unit",
            Self::Project => "Project",
            Self::AccountPool => "Account Pool",
            Self::AssetType => "Asset type",
            Self::FormType => "Form type",
            Self::Glossary => "Glossary",
            Self::GlossaryTerm => "Glossary term",
            Self::Asset => "Asset",
            Self::AssetFilter => "Asset filter",
            Self::DataProduct => "Data product",
            Self::Listing => "Listing",
            Self::UserProfile => "User profile",
            Self::GroupProfile => "Group profile",
            Self::SubscriptionRequest => "Subscription request",
            Self::Subscription => "Subscription",
        }
    }

    /// The `ResourceNotFoundException` message DataZone answers for a resource of this
    /// kind that it cannot find: the KIND only, never the caller-supplied identifier
    /// (recorded behaviour: "Project not found", "Account Pool not found"). Build every
    /// not-found message in this service's handlers through this, so none of them can
    /// echo the request back.
    pub fn not_found_message(self) -> String {
        match self {
            // Recorded behaviour: some kinds DataZone cannot find are reported with a
            // sentence of their own rather than the "<Kind> not found" form. Still a
            // fixed text naming only the kind, so it echoes nothing back either.
            Self::FormType => FORM_TYPE_NOT_FOUND_MESSAGE.to_string(),
            Self::Asset => ASSET_NOT_FOUND_MESSAGE.to_string(),
            Self::Glossary => GLOSSARY_NOT_FOUND_MESSAGE.to_string(),
            Self::GlossaryTerm => GLOSSARY_TERM_NOT_FOUND_MESSAGE.to_string(),
            _ => format!("{} not found", self.as_str()),
        }
    }
}

/// The `ResourceNotFoundException` message DataZone answers for a metadata form type it
/// cannot find — [`ResourceKind::FormType`]'s not-found text, recorded from the service.
/// Every handler that addresses a form type (`GetFormType`, `DeleteFormType`) answers
/// the same text, so the two cannot drift.
pub const FORM_TYPE_NOT_FOUND_MESSAGE: &str =
    "The given FormType doesn't exist. Try creating FormType before accessing it.";

/// The `ResourceNotFoundException` message DataZone answers for a catalog asset it
/// cannot find — [`ResourceKind::Asset`]'s not-found text, recorded from the service and
/// worded the same way a missing form type is ([`FORM_TYPE_NOT_FOUND_MESSAGE`]): the
/// resource is named by its API type name, and nothing the caller sent appears in it.
/// Every handler that addresses an asset (`GetAsset`, `DeleteAsset`,
/// `CreateAssetFilter`) answers this one text, so they cannot drift.
pub const ASSET_NOT_FOUND_MESSAGE: &str =
    "The given Asset doesn't exist. Try creating Asset before accessing it.";

/// The `ResourceNotFoundException` message DataZone answers for a business glossary it
/// cannot find — [`ResourceKind::Glossary`]'s not-found text, recorded from the service
/// and worded the way a missing form type's and a missing asset's are
/// ([`FORM_TYPE_NOT_FOUND_MESSAGE`]): the resource is named by its API type name, and
/// nothing the caller sent appears in it. Every handler that addresses a glossary
/// (`GetGlossary`, `UpdateGlossary`, `DeleteGlossary`, `CreateGlossaryTerm`) answers this
/// one text, so they cannot drift.
pub const GLOSSARY_NOT_FOUND_MESSAGE: &str =
    "The given Glossary doesn't exist. Try creating Glossary before accessing it.";

/// The same, for a business glossary TERM — [`ResourceKind::GlossaryTerm`]'s not-found
/// text, in that same recorded form with the term's own API type name. Every handler that
/// addresses a term (`GetGlossaryTerm`, `UpdateGlossaryTerm`, `DeleteGlossaryTerm`)
/// answers it.
pub const GLOSSARY_TERM_NOT_FOUND_MESSAGE: &str =
    "The given GlossaryTerm doesn't exist. Try creating GlossaryTerm before accessing it.";

impl std::fmt::Display for ResourceKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(self.as_str())
    }
}

/// The `ResourceNotFoundException` message answered for a DOMAIN the request RESOLVES and
/// this store holds no record of — the domain a resource is created IN, named by the
/// `domainIdentifier` the request's path carried.
///
/// Recorded behaviour, and fixed text apart from that ONE substitution: the identifier is
/// taken verbatim from the request, and nothing else about the request, the caller or the
/// store appears in it. Distinct from [`ResourceKind::not_found_message`], which names the
/// kind alone and is what the operations that resolve a resource INSIDE a domain answer;
/// this one belongs to the operations whose miss is the domain itself.
pub fn domain_not_found_message(domain_identifier: &str) -> String {
    format!("Domain {domain_identifier} not found")
}

/// The `AccessDeniedException` message DataZone answers a request it denies INSIDE a
/// domain — one carrying a `domainIdentifier` and addressing a resource under it.
///
/// The fixed text plus the OPERATION NAME of the request — `Operation::NAME`, the name
/// the model binds, which is why the only argument is that name: nothing from the
/// request's path, body or state appears in it, so the denial reveals nothing about
/// what the caller addressed. Every domain-scoped operation that denies a request words
/// it through this, so the wording cannot drift between them. (An operation whose own
/// identifier IS the domain uses [`DOMAIN_SCOPE_DENIED_MESSAGE`] instead.)
pub fn not_permitted_message(operation: &str) -> String {
    format!("User is not permitted to perform operation: {operation}")
}

/// The `AccessDeniedException` message DataZone answers when the identifier the request
/// carries IS the domain it is scoped to, rather than a resource inside one.
///
/// Recorded behaviour: that denial's body carries no members at all, so the `message`
/// the model marks required is EMPTY — the service says nothing about a domain the
/// caller holds nothing on, not even which operation was refused. Deliberately distinct
/// from [`not_permitted_message`], which is the recorded wording for a denial inside a
/// domain, and shared so the domain-level operations cannot drift from one another.
pub const DOMAIN_SCOPE_DENIED_MESSAGE: &str = "";

/// The `AccessDeniedException` message DataZone answers a call addressing a domain unit
/// it has DELETED: [`not_permitted_message`] plus the clause that makes the one answer
/// cover both halves of a question the service will not separate.
///
/// Recorded behaviour, and fixed text apart from the OPERATION NAME — the same single
/// substitution [`not_permitted_message`] takes, so nothing the caller sent appears in
/// it and the answer is the same on every repeated call.
pub fn not_permitted_or_absent_message(operation: &str) -> String {
    format!(
        "{}, or the requested resource does not exist",
        not_permitted_message(operation)
    )
}

/// The `ConflictException` message `CreateDomain` answers when the account already has
/// a domain with the requested name. The requirement is the 409 refusal and the code;
/// this text is AWS's own recorded wording, and like the not-found text above it names
/// no caller-supplied value.
pub const DUPLICATE_DOMAIN_NAME_MESSAGE: &str =
    "Domain name already exists under this account, please use another one";

/// Lowercase base-36 encoding of `value` — the alphabet every DataZone id pattern
/// accepts.
fn base36(mut value: u64) -> String {
    const ALPHABET: &[u8; 36] = b"0123456789abcdefghijklmnopqrstuvwxyz";
    if value == 0 {
        return "0".to_string();
    }
    let mut out = Vec::new();
    while value > 0 {
        out.push(ALPHABET[(value % 36) as usize]);
        value /= 36;
    }
    out.reverse();
    String::from_utf8(out).unwrap_or_default()
}

// ---------------------------------------------------------------------------
// Request-derived values and pagination, shared by this service's handlers.
//
// `src/ops/mod.rs` is generated (it `include!`s the generated module list) and so is
// `src/lib.rs`, so a helper several operations need lives in this file — the service's
// own hand-editable module — rather than in a module neither generated file can name.
// ---------------------------------------------------------------------------

/// The region the caller signed for, defaulting to the region a generated emulator
/// presents when the request carries no SigV4 scope (an anonymous call).
pub fn caller_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The region an anonymous request is served in: AWS SDK clients default to
/// `us-east-1`, so an unsigned call is reported there.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The IAM identity this emulator authenticates every caller as: the ARN of the one IAM
/// user it publishes — the very principal its `sts:GetCallerIdentity` answers.
///
/// Read from the tree's single definition of that principal
/// ([`::service_common::caller_principal`]) rather than composed here, so the IAM service
/// that HOLDS the user, the STS that reports it as the caller and this service all name one
/// record. It is the `userIdentifier` the caller's own user profile is held under
/// ([`caller_user_profile`]), so the IAM principal behind the DataZone user a response
/// names is one the identity service knows. Deliberately NOT derived from the request's
/// access key id: an access key id pasted in where an IAM user NAME belongs names no
/// principal that exists.
pub fn caller_identity_arn() -> String {
    ::service_common::caller_principal::ambient_user_arn()
}

/// The `UserProfileId` of the DataZone USER this emulator authenticates every caller as.
///
/// Fixed, like the account id and the user name of the IAM identity it belongs to
/// ([`caller_identity_arn`]): this emulator serves exactly one caller identity, so that
/// user's id is the same in every domain and over every call, and every domain is created
/// holding a profile under it ([`caller_user_profile`]) so the id a response reports
/// resolves through `GetUserProfile`.
///
/// Hex digits in the groups `8-4-4-4-12` that `UserProfileId`'s `@pattern`
/// `^([0-9a-f]{10}-|)[A-Fa-f0-9]{8}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{4}-[A-Fa-f0-9]{12}$`
/// admits (smithy_query shape="UserProfileId"); the optional leading group belongs to an
/// id an identity store minted, so an id this emulator owns omits it.
pub const CALLER_USER_PROFILE_ID: &str = "e398d0a2-f1b6-4c5d-8e7a-0b9c1d2e3f40";

/// The DataZone user every call is attributed to, and so the value of every "who did
/// this" member this service reports (`createdBy`, `updatedBy`, `firstRevisionCreatedBy`,
/// an owner, a reviewer: "The Amazon DataZone user that created this asset in the
/// catalog").
///
/// A DataZone user is identified by the id of its USER PROFILE rather than by the IAM
/// principal behind it, so that id is what an audit member carries: the handle a reader
/// looks the user up by (`GetUserProfile`) and the one a project membership names. Never
/// anything the request body carried, and never the caller's access key id.
pub fn caller_user_id() -> String {
    CALLER_USER_PROFILE_ID.to_string()
}

/// The user profile a new domain is created holding: the DataZone user this emulator
/// authenticates every caller as, which is the user that created the domain and owns its
/// root domain unit.
///
/// Answers the `user_profiles` entry — its [`user_profile_key`] and the profile — so a
/// domain holds one from the instant it exists and the id every audit member in it reports
/// ([`caller_user_id`]) is one `GetUserProfile` resolves, addressed either by that id or
/// by the IAM ARN behind it. The profile is of an IAM principal, so it is `ACTIVATED`
/// ([`created_user_profile_status`]) and reports its ARN under the union's `iam` member,
/// exactly as a profile `CreateUserProfile` stores for the same identity would.
pub fn caller_user_profile(domain_id: &str) -> (String, UserProfile) {
    let arn = caller_identity_arn();
    (
        user_profile_key(&arn, None),
        UserProfile {
            id: CALLER_USER_PROFILE_ID.to_string(),
            domain_id: domain_id.to_string(),
            user_identifier: arn.clone(),
            session_name: None,
            profile_type: USER_PROFILE_TYPE_IAM.to_string(),
            status: created_user_profile_status(USER_PROFILE_TYPE_IAM).to_string(),
            details: user_profile_details(USER_PROFILE_TYPE_IAM, None, &arn, None),
        },
    )
}

/// Encode the key a page ended on as the opaque `nextToken` a caller passes back.
///
/// Opaque to the client (hex of the key's bytes) but a pure function of the key, so
/// the next page resumes exactly after it however the stored map has changed.
pub fn encode_page_token(last_key: &str) -> String {
    hex::encode(last_key.as_bytes())
}

/// The key a `nextToken` resumes after, or `None` when the token is not one this
/// service issued.
pub fn decode_page_token(token: &str) -> Option<String> {
    String::from_utf8(hex::decode(token).ok()?).ok()
}

/// The ordering key a listing sorted by [`SORT_FIELD_NAME`] uses: the resource's name,
/// then its id.
///
/// A name is not unique, so the id after it is what makes the key a TOTAL order — two
/// resources sharing a name still have one stable position each, and a page's token
/// (which carries this key) resumes after exactly one of them. The separator is a NUL,
/// which no name shape this service stores admits, so the key cannot be ambiguous
/// between the two parts.
pub fn name_sort_key(name: &str, id: &str) -> String {
    format!("{name}\u{0}{id}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsDatazoneDatazoneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsDatazoneDatazoneState>)
}
