// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-codeartifact-codeartifactcontrolplaneservice service.
//!
//! CodeArtifact's primary resource pair is the domain and the repositories nested
//! inside it, so the store is a single sorted map of domains, each owning a sorted map
//! of its repositories. Sorted (`BTreeMap`) rather than hashed because every `List*`
//! operation on this service is `@paginated` over a name: a stable key order is what
//! makes an opaque `nextToken` resolve to the same place on the next call.
//!
//! The modelled response shapes are NOT stored. `DomainDescription`,
//! `DomainSummary`, `RepositoryDescription`, `RepositorySummary`, `PackageSummary` and
//! `PackageGroupDescription` are all projections of the records below
//! ([`Domain::description`] and friends), so a value that is derived —
//! `repositoryCount`, a package group's `parent` and each origin restriction's
//! `effectiveMode` above all — can never go stale against the records it summarises.

use std::collections::{BTreeMap, BTreeSet, HashMap, VecDeque};
use std::time::SystemTime;

use crate::ops::types::{
    AssetSummary, DomainDescription, DomainEntryPoint, DomainSummary, LicenseInfo,
    PackageGroupDescription, PackageGroupOriginConfiguration, PackageGroupOriginRestriction,
    PackageGroupReference, PackageGroupSummary, PackageOriginConfiguration,
    PackageOriginRestrictions, PackageSummary, PackageVersionDescription, PackageVersionOrigin,
    PackageVersionSummary, RepositoryDescription, RepositoryExternalConnectionInfo,
    RepositorySummary, ResourcePolicy, Tag, UpstreamRepositoryInfo, ValidationException,
    PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES,
};

/// The event a package version's state change announces, and how it reaches the account's
/// default event bus. A state change is RECORDED on this store
/// ([`ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::record_version_change`])
/// by whatever changed the version, and published from there once the store's lock is
/// released, so every plane that changes a version announces it the same way.
pub mod package_version_events;

/// The KMS key a domain is encrypted with: the one resolution of a create's
/// `encryptionKey` against KMS, and of the managed key a create that names none asks for.
pub mod domain_encryption;

/// This service's SigV4 signing name, which is also the service segment of every ARN
/// it mints (`arn:aws:codeartifact:...`).
pub const SERVICE_SIGNING_NAME: &str = "codeartifact";

/// The region attributed to a request that carries no SigV4 credential scope to read a
/// region out of (an anonymous or unsigned call). Matches the region the emulator's own
/// generated client config uses.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The only status a domain this emulator HOLDS can be in: CodeArtifact creates a
/// domain synchronously and `DescribeDomain` reports it `Active` from the first read, so
/// there is no transient creating state to move through. (`DomainStatus` models exactly
/// `Active` and `Deleted` — `smithy_query(query="shape",shape="DomainStatus")`.)
pub const DOMAIN_STATUS_ACTIVE: &str = "Active";

/// The terminal status of a domain that has been deleted — the other half of
/// `DomainStatus`. No domain in the store ever carries it: it belongs to the description
/// `DeleteDomain` answers with ([`Domain::deleted_description`]), which reports the state
/// the domain is in AFTER the call, not the record as it was before.
pub const DOMAIN_STATUS_DELETED: &str = "Deleted";

/// The only status an external connection this emulator holds can be in: there is one
/// external-connection status, and an associated connection carries it from the first
/// read (CodeArtifact associates a public registry synchronously, so there is no
/// transient state to move through).
///
/// Spelled as the WIRE spells it. The on-disk model is stale for this member — Smithy
/// `ExternalConnectionStatus` lists `Available`
/// (`smithy_query(query="shape",shape="ExternalConnectionStatus")`) and the member doc
/// repeats it, but the live responses of `AssociateExternalConnection` and
/// `DescribeRepository` return `AVAILABLE`, and `status` is an enum a client COMPARES
/// against what it sees from AWS, so the observed wire token is the contract here and the
/// model entry is not.
pub const EXTERNAL_CONNECTION_STATUS_AVAILABLE: &str = "AVAILABLE";

/// The public registries CodeArtifact can connect a repository to, each paired with the
/// `PackageFormat` its packages carry.
///
/// Source: the `externalConnection` member doc of `AssociateExternalConnectionRequest`
/// ("The following values are supported: public:npmjs - for the npm public repository
/// …"), read with
/// `smithy_query(query="doc",shape="AssociateExternalConnectionRequest",member="externalConnection")`;
/// the format of each is the registry's own package format from `PackageFormat`. The
/// member is modelled as a free `ExternalConnectionName` string rather than an enum, so
/// this catalog is what distinguishes a supported connection from an arbitrary one.
pub const EXTERNAL_CONNECTIONS: &[(&str, &str)] = &[
    ("public:npmjs", "npm"),
    ("public:nuget-org", "nuget"),
    ("public:pypi", "pypi"),
    ("public:maven-central", "maven"),
    ("public:maven-googleandroid", "maven"),
    ("public:maven-gradleplugins", "maven"),
    ("public:maven-commonsware", "maven"),
    ("public:maven-clojars", "maven"),
    ("public:ruby-gems-org", "ruby"),
    ("public:crates-io", "cargo"),
];

/// The `PackageFormat` of the packages a named external connection serves, or `None` when
/// the name is not a connection CodeArtifact supports — which the caller answers as an
/// invalid parameter value rather than storing a connection to a registry that does not
/// exist.
pub fn external_connection_package_format(name: &str) -> Option<&'static str> {
    EXTERNAL_CONNECTIONS
        .iter()
        .find(|(connection, _)| *connection == name)
        .map(|(_, format)| *format)
}

// ---------------------------------------------------------------------------
// Store bounds
//
// Every record below is created by a caller request and freed only by an explicit
// delete, so each growth site needs a declared ceiling: without one, a local caller can
// grow the shared in-process state without limit. These are CodeArtifact's own published
// default quotas (AWS CodeArtifact "Quotas" page: Domains per account 10, Repositories
// per domain 1,000, Upstream repositories per repository 10), so a caller that hits one
// gets the SAME answer the real service gives — `ServiceQuotaExceededException` (402),
// which both create operations already model.
// ---------------------------------------------------------------------------

/// Maximum domains one account holds ("Domains per account" quota).
pub const MAX_DOMAINS: usize = 10;

/// Maximum repositories one domain holds ("Repositories per domain" quota).
pub const MAX_REPOSITORIES_PER_DOMAIN: usize = 1_000;

/// Maximum upstream repositories one repository may list ("Upstream repositories per
/// repository" quota). `UpstreamRepositoryList` carries NO modelled length, so the
/// boundary validator cannot cap it and `CreateRepository` must.
pub const MAX_UPSTREAMS_PER_REPOSITORY: usize = 10;

/// The `associationType` of a package whose associated group's pattern names the
/// package's own path components (an "exact" match).
pub const ASSOCIATION_TYPE_STRONG: &str = "STRONG";

/// The `associationType` of a package that matched part of a component of its group's
/// pattern (a "relative" match) rather than whole components of it.
pub const ASSOCIATION_TYPE_WEAK: &str = "WEAK";

/// Maximum packages one repository holds.
///
/// Unlike the three bounds above this one is NOT a published CodeArtifact quota — AWS
/// publishes none for packages per repository — so it is a bound this emulator CHOOSES to
/// keep its in-process store finite: a package record is freed only by an explicit delete,
/// so without a ceiling a local caller could grow the shared state without limit. It is far
/// above anything a fixture creates, and a caller that reaches it is refused (the operations
/// that create a package model no quota error, so the refusal is their `ValidationException`
/// naming this limit) rather than served a truncated store.
pub const MAX_PACKAGES_PER_REPOSITORY: usize = 10_000;

/// Maximum external connections one repository may hold. `AssociateExternalConnection`'s
/// own documentation states the limit — "One external connection is allowed per
/// repository" — and it is why that operation models
/// `ServiceQuotaExceededException` (402).
pub const MAX_EXTERNAL_CONNECTIONS_PER_REPOSITORY: usize = 1;

/// Maximum bytes one published asset may carry.
///
/// NOT an AWS quota — CodeArtifact's published asset ceiling is measured in gigabytes,
/// which this in-memory store could not hold — but the emulator's OWN bound: asset bytes
/// live in the process until an explicit delete, so without a ceiling one local caller
/// can grow the shared store until the process dies. An asset over it is refused with the
/// `ServiceQuotaExceededException` (402) every publish path already models.
///
/// It bounds what is STORED, not the peak allocation of one request: the request body is
/// already buffered by the time a handler sees it, so the only bound on THAT is the
/// server's body cap.
pub const MAX_ASSET_BYTES: usize = 64 * 1024 * 1024;

/// Maximum assets one package version may hold (the emulator's own bound, for the same
/// reason as [`MAX_ASSET_BYTES`]: an unfinished version accepts assets until it closes).
pub const MAX_ASSETS_PER_PACKAGE_VERSION: usize = 100;

/// Maximum versions one `CopyPackageVersions` request may name.
///
/// This is the modelled `PackageVersionList` bound (`@length(0..100)`) that the `versions`
/// member already carries, applied to the `versionRevisions` member, which names the SAME
/// set and carries NO modelled length — so the boundary validator caps one form of the
/// request and nothing but this constant caps the other. A request past it is refused with
/// the same 400 `ValidationException` the validator answers for an over-long `versions`
/// list, before the keys are collected.
pub const MAX_VERSIONS_PER_COPY_REQUEST: usize = 100;

/// Maximum versions one package may hold (the emulator's own bound).
pub const MAX_PACKAGE_VERSIONS_PER_PACKAGE: usize = 1_000;

/// Maximum total bytes of asset content this service retains, across every domain.
///
/// [`MAX_ASSET_BYTES`] bounds ONE asset; the record-count ceilings bound only their
/// product, which is astronomically larger than the host's memory. This is the bound that
/// actually holds the process: real CodeArtifact keeps assets in S3, this emulator keeps
/// them in RAM, so the bytes it retains need a ceiling of their own. Peer services that
/// retain caller bytes declare the same thing (S3's `MAX_OBJECT_BYTES_TOTAL`,
/// mediastoredata's `MAX_STORED_BYTES`); this sits in the same band and is far above
/// anything a fixture publishes. A publish that would cross it is refused with
/// `ServiceQuotaExceededException` (402) BEFORE the asset is stored.
///
/// An `i64` rather than a `usize` because it is compared against the derived total of
/// [`ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::stored_asset_bytes`],
/// which is an `i64` to match the modelled `assetSizeBytes`.
pub const MAX_STORED_ASSET_BYTES: i64 = 256 * 1024 * 1024;

// ---------------------------------------------------------------------------
// Package-version vocabulary
//
// Each constant below is one value of a modelled enum (`PackageFormat`,
// `PackageVersionStatus`, `AllowPublish`/`AllowUpstream`, `PackageVersionOriginType`,
// `HashAlgorithm`), named here so the handlers compare against one spelling.
// ---------------------------------------------------------------------------

/// The only `PackageFormat` `PublishPackageVersion` accepts: "Only generic packages can
/// be published using this API" (the operation's own documentation).
pub const PACKAGE_FORMAT_GENERIC: &str = "generic";

/// The `PackageFormat` whose packages are published and resolved over an npm registry —
/// the repository endpoint `crate::service::npm_registry` serves — as the model spells it
/// (`smithy_query(query="shape",shape="PackageFormat")`).
pub const PACKAGE_FORMAT_NPM: &str = "npm";

/// The `PackageFormat` whose packages are published and resolved over a PyPI index — the
/// repository endpoint `crate::service::pypi_registry` serves — as the model spells it
/// (`smithy_query(query="shape",shape="PackageFormat")`).
pub const PACKAGE_FORMAT_PYPI: &str = "pypi";

/// The `PackageFormat`s whose namespace is a SCOPE, so the scope is part of how a package
/// of that format is written: "The namespace of an npm or Swift package version is its
/// scope" (`PackageVersionDescription$namespace`,
/// `smithy_query(query="doc",shape="PackageVersionDescription",member="namespace")`).
/// Used by [`package_display_name`]; every other format writes its namespace as a
/// separate coordinate (a Maven `groupId`, a generic `namespace`) or has none at all.
pub const SCOPED_PACKAGE_FORMATS: &[&str] = &["npm", "swift"];

/// `PackageVersionStatus` of a version whose assets are all uploaded.
pub const PACKAGE_VERSION_STATUS_PUBLISHED: &str = "Published";

/// `PackageVersionStatus` of a version that is still accepting assets, which is what the
/// `unfinished` flag keeps a publish in.
pub const PACKAGE_VERSION_STATUS_UNFINISHED: &str = "Unfinished";

/// `PackageVersionStatus` of a version that `UpdatePackageVersionsStatus` retired: an
/// archived version is still held, so it can be moved back to `Published` ("If you want to
/// remove a package version from your repository and be able to restore it later, set its
/// status to Archived" — `DeletePackageVersions`' own documentation).
pub const PACKAGE_VERSION_STATUS_ARCHIVED: &str = "Archived";

/// `PackageVersionStatus` of a version that is held and resolvable but kept out of the
/// listings — the third status `UpdatePackageVersionsStatus` can move a version to.
pub const PACKAGE_VERSION_STATUS_UNLISTED: &str = "Unlisted";

/// The statuses `UpdatePackageVersionsStatus` can move a version TO.
///
/// Source: that operation's own documentation — "Using UpdatePackageVersionsStatus, you can
/// update the status of package versions to Archived, Published, or Unlisted. To set the
/// status of a package version to Disposed, use DisposePackageVersions." `PackageVersionStatus`
/// itself models six values, so the three it does NOT list here (`Disposed`, `Deleted`,
/// `Unfinished`) are reachable only through the operation that owns each of them, and a
/// `targetStatus` naming one is a bad REQUEST rather than a status change.
pub const UPDATABLE_PACKAGE_VERSION_STATUSES: &[&str] = &[
    PACKAGE_VERSION_STATUS_ARCHIVED,
    PACKAGE_VERSION_STATUS_PUBLISHED,
    PACKAGE_VERSION_STATUS_UNLISTED,
];

/// `PackageVersionStatus` of a version `DisposePackageVersions` disposed: the version
/// record stays, which is why a disposed version is still readable, and the status is
/// terminal — "a disposed package version cannot be restored in your repository"
/// (`DisposePackageVersions`' own documentation). The asset deletion that operation
/// documents is work the service does out of band, so a disposed version's asset records
/// are not removed by the call that disposed it.
pub const PACKAGE_VERSION_STATUS_DISPOSED: &str = "Disposed";

/// `PackageVersionStatus` of a version a delete removed, which is what
/// `DeletePackageVersions` reports for every version it deleted ("The status of every
/// successful version will be `Deleted`" — `DeletePackageVersionsResult$successfulVersions`).
///
/// Never STORED: a deleted version is gone from the store, so this status is reported by
/// the call that removed it and by nothing that reads a stored record.
pub const PACKAGE_VERSION_STATUS_DELETED: &str = "Deleted";

/// The `PackageVersionStatus` values a request may SUPPLY as the status of a package
/// version — the statuses a stored version can carry, which is every modelled value
/// except the one no record ever holds ([`PACKAGE_VERSION_STATUS_DELETED`]).
///
/// `PackageVersionStatus` is one enum used in both directions: a response REPORTS the
/// state a version reached, while a request's `status`/`expectedStatus` states which
/// stored versions it means. The report is the wider of the two, so the input set is
/// named once here rather than re-derived per handler — see
/// [`input_package_version_status_refusal`], which every handler taking such a member
/// calls. (`PackageVersionStatus`' whole catalog is
/// `crate::ops::types::PACKAGE_VERSION_STATUS_VALUES`.)
pub const INPUT_PACKAGE_VERSION_STATUSES: &[&str] = &[
    PACKAGE_VERSION_STATUS_ARCHIVED,
    PACKAGE_VERSION_STATUS_DISPOSED,
    PACKAGE_VERSION_STATUS_PUBLISHED,
    PACKAGE_VERSION_STATUS_UNFINISHED,
    PACKAGE_VERSION_STATUS_UNLISTED,
];

/// The `AllowPublish`/`AllowUpstream` value that permits the action.
pub const ORIGIN_RESTRICTION_ALLOW: &str = "ALLOW";

/// The `AllowPublish`/`AllowUpstream` value that refuses the action.
pub const ORIGIN_RESTRICTION_BLOCK: &str = "BLOCK";

/// `PackageVersionOriginType` of a version published straight into a repository (as
/// opposed to one pulled in from an upstream or an external connection).
pub const PACKAGE_VERSION_ORIGIN_INTERNAL: &str = "INTERNAL";

/// Maximum package groups one domain holds, counting the root group every domain is
/// created with.
///
/// Unlike the ceilings above this one does NOT quote a published figure: AWS caps
/// package groups per domain, but the number is not cited here, so treat this as the
/// emulator's own declared bound on a growth site a caller controls. What matters for
/// fidelity is the REFUSAL — `ServiceQuotaExceededException` (402), which
/// `CreatePackageGroup` models — not the exact threshold.
pub const MAX_PACKAGE_GROUPS_PER_DOMAIN: usize = 100;

// ---------------------------------------------------------------------------
// Package groups
// ---------------------------------------------------------------------------

/// The pattern of the root package group. Every domain has it from the moment it is
/// created: it matches every package in the domain, it is the ancestor of every other
/// package group, and it is what the origin restrictions of a group that inherits
/// ultimately resolve against.
pub const ROOT_PACKAGE_GROUP_PATTERN: &str = "/*";

/// The `description` the service itself assigns to the root package group it creates with
/// every domain.
///
/// Fixed text, identical for every domain: it carries no part of the request, the account
/// or the clock. A caller-created group has no description until the caller gives it one,
/// but the root group is created BY the service, so its record carries this from the
/// moment the domain exists — and an `UpdatePackageGroup` on the root group replaces it
/// like any other stored description.
pub const ROOT_PACKAGE_GROUP_DESCRIPTION: &str = "Default root package group";

/// `PackageGroupOriginRestrictionMode$ALLOW`.
pub const ORIGIN_MODE_ALLOW: &str = "ALLOW";

/// `PackageGroupOriginRestrictionMode$ALLOW_SPECIFIC_REPOSITORIES` — the only mode that
/// carries an allowed-repository list, and therefore the only one with a
/// `repositoriesCount`.
pub const ORIGIN_MODE_ALLOW_SPECIFIC_REPOSITORIES: &str = "ALLOW_SPECIFIC_REPOSITORIES";

/// `PackageGroupOriginRestrictionMode$INHERIT` — the mode a newly created group carries
/// for every restriction type, which resolves to an ancestor's mode.
pub const ORIGIN_MODE_INHERIT: &str = "INHERIT";

/// Per-service state for com-amazonaws-codeartifact-codeartifactcontrolplaneservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState {
    /// Every domain this emulator holds, keyed by domain name.
    ///
    /// Keyed by NAME alone, not by region: this emulator serves one logical account and
    /// its state is not partitioned per region, so a domain created through a request
    /// signed for one region is visible to a request signed for another (and keeps the
    /// ARNs minted from the creating request's region). Real CodeArtifact namespaces
    /// domains per region; a later slice that needs that distinction should widen this
    /// key to `(region, name)` rather than add a second map.
    pub domains: BTreeMap<String, Domain>,
    /// Every bearer credential `GetAuthorizationToken` has minted and not let expire,
    /// keyed by the DIGEST of the token ([`authorization_token_digest`]) rather than by
    /// the token itself, so the secret a client holds is not kept in this store.
    ///
    /// A repository endpoint authenticates the token a package manager presents against
    /// this map: the credential is this service's own, so this service is what resolves
    /// it. Bounded by [`MAX_AUTHORIZATION_TOKENS`].
    pub authorization_tokens: BTreeMap<String, AuthorizationToken>,
    /// The package-version state changes this service has committed and not yet announced,
    /// oldest first.
    ///
    /// A change is recorded here while this store's lock is held and announced after it is
    /// released ([`package_version_events`]), which is what keeps the in-process call into
    /// the event bus off the critical section. A ring bounded by
    /// [`package_version_events::MAX_PENDING_VERSION_EVENTS`] — see
    /// [`Self::record_version_change`], the one place it grows.
    pub pending_version_changes: VecDeque<package_version_events::VersionChange>,
}

impl ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState {
    /// Resolve a domain the way every domain-scoped operation addresses it: by `name`,
    /// qualified by the optional `domainOwner` the caller may pin it to.
    ///
    /// `None` means "no such domain for this caller" — either the name is not held, or
    /// it is held by an account other than the one the caller named — and each operation
    /// answers that with its own modelled not-found error. Keeping the rule here means
    /// `domainOwner` cannot be honoured by one operation and ignored by the next.
    pub fn domain(&self, name: &str, owner: Option<&str>) -> Option<&Domain> {
        self.domains
            .get(name)
            .filter(|domain| owner.is_none_or(|owner| owner == domain.owner))
    }

    /// [`Self::domain`], for an operation that mutates the domain it resolves.
    pub fn domain_mut(&mut self, name: &str, owner: Option<&str>) -> Option<&mut Domain> {
        self.domains
            .get_mut(name)
            .filter(|domain| owner.is_none_or(|owner| owner == domain.owner))
    }

    /// The total asset bytes this service currently retains, across every domain — what
    /// [`MAX_STORED_ASSET_BYTES`] bounds.
    ///
    /// Summed from the assets themselves rather than kept as a running counter, for the
    /// same reason as every other derived value here: a counter is one more place that
    /// can forget to move when a version or a repository is deleted. The sum costs the
    /// NUMBER of stored assets (bounded by the record ceilings above), never their bytes.
    pub fn stored_asset_bytes(&self) -> i64 {
        self.domains.values().fold(0i64, |total, domain| {
            total.saturating_add(domain.asset_size_bytes())
        })
    }

    /// The total CONTENT bytes this service currently retains: every asset's bytes plus
    /// every version's retained text ([`PackageVersion::retained_text_bytes`] — its
    /// metadata document and its readme) — the whole of
    /// what a publish leaves in the store, which is what [`MAX_STORED_ASSET_BYTES`] has to
    /// bound.
    ///
    /// Distinct from [`Self::stored_asset_bytes`], which is the ASSET total a domain
    /// reports as its `assetSizeBytes`: a metadata document is not an asset, so it is not
    /// part of that reported value, but it is retained caller content and so is part of
    /// what the ceiling is checked against. Derived on each call for the same reason, and
    /// costs the NUMBER of stored versions and assets (bounded by the record ceilings
    /// above), never their bytes.
    pub fn stored_content_bytes(&self) -> i64 {
        self.domains
            .values()
            .flat_map(|domain| domain.repositories.values())
            .flat_map(|repository| repository.packages.values())
            .flat_map(|package| package.versions.values())
            .fold(0i64, |total, version| {
                total
                    .saturating_add(version.asset_size_bytes())
                    .saturating_add(version.retained_text_bytes())
            })
    }

    /// Remember a bearer credential this service just minted: which domain it is scoped
    /// to, and when it stops being valid.
    ///
    /// Only the token's DIGEST is kept, so the store holds nothing a client could present
    /// as the credential. Records the clock has already expired are dropped first, which
    /// is what keeps the map inside [`MAX_AUTHORIZATION_TOKENS`] without ever giving up a
    /// credential that is still valid; at the ceiling with nothing expired, the record
    /// expiring soonest makes room, so a mint never fails for want of space.
    pub fn record_authorization_token(
        &mut self,
        token: &str,
        domain: &str,
        domain_owner: &str,
        principal: &str,
        expires_at: SystemTime,
        now: SystemTime,
    ) {
        self.authorization_tokens
            .retain(|_, held| now < held.expires_at);
        while self.authorization_tokens.len() >= MAX_AUTHORIZATION_TOKENS {
            let Some(soonest) = self
                .authorization_tokens
                .iter()
                .min_by_key(|(digest, held)| (held.expires_at, (*digest).clone()))
                .map(|(digest, _)| digest.clone())
            else {
                break;
            };
            self.authorization_tokens.remove(&soonest);
        }
        self.authorization_tokens.insert(
            authorization_token_digest(token),
            AuthorizationToken {
                domain: domain.to_string(),
                domain_owner: domain_owner.to_string(),
                principal: principal.to_string(),
                expires_at,
            },
        );
    }

    /// Remember that a package version's state changed, for the event that announces it.
    ///
    /// Called with this store's lock held, by whatever made the change — so the npm data
    /// plane and every control-plane operation that creates, moves or removes a version
    /// record it the same way. The oldest records make room once the queue is at
    /// [`package_version_events::MAX_PENDING_VERSION_EVENTS`], which only an emulator with
    /// nothing draining the queue ever reaches.
    pub fn record_version_change(&mut self, change: package_version_events::VersionChange) {
        while self.pending_version_changes.len()
            >= package_version_events::MAX_PENDING_VERSION_EVENTS
        {
            if self.pending_version_changes.pop_front().is_none() {
                break;
            }
        }
        self.pending_version_changes.push_back(change);
    }

    /// Take up to `limit` recorded changes off the queue, oldest first.
    ///
    /// Moving them out is what makes an announcement happen once: a change this returns is
    /// no longer on the queue, so a second drain cannot publish it again.
    pub fn take_pending_version_changes(
        &mut self,
        limit: usize,
    ) -> Vec<package_version_events::VersionChange> {
        let taken = self.pending_version_changes.len().min(limit);
        self.pending_version_changes.drain(..taken).collect()
    }

    /// The record of the bearer credential `token`, when this service minted it and the
    /// clock has not passed its expiry.
    ///
    /// A READ: a token past its expiry is simply not resolved, and nothing is written, so
    /// presenting one twice answers the same way both times.
    pub fn authorization_token(&self, token: &str, now: SystemTime) -> Option<&AuthorizationToken> {
        self.authorization_tokens
            .get(&authorization_token_digest(token))
            .filter(|held| now < held.expires_at)
    }
}

/// How many minted bearer credentials this service remembers at once.
///
/// A mint is a request-driven insert, so the map needs a declared ceiling. The value is a
/// chosen emulator bound, far above any test or demo workload; AWS publishes no such
/// limit.
pub const MAX_AUTHORIZATION_TOKENS: usize = 1_024;

/// One bearer credential `GetAuthorizationToken` minted: the scope it was issued for and
/// when it stops being valid. The token itself is NOT part of the record — the map is
/// keyed by its digest.
#[derive(Debug, Clone)]
pub struct AuthorizationToken {
    /// The domain the token is scoped to: "The name of the domain that is in scope for the
    /// generated authorization token" (`GetAuthorizationToken`'s `domain`).
    pub domain: String,
    /// The account that owns that domain.
    pub domain_owner: String,
    /// The principal the credential speaks for, as `sts:GetCallerIdentity` reports it for
    /// the request that minted it. Kept because a repository endpoint a token is presented
    /// to has to decide what THAT principal may do there, which is what a repository's
    /// resource policy states.
    pub principal: String,
    /// The instant the token expires, as the mint reported it to the caller.
    pub expires_at: SystemTime,
}

/// The key a minted bearer credential is remembered under: the lowercase-hex SHA-256 of
/// the token.
///
/// A digest rather than the token, so this store never holds a value that could be
/// presented as the credential; the same token always digests to the same key, which is
/// what lets a presented token be resolved without the token being kept.
#[must_use]
pub fn authorization_token_digest(token: &str) -> String {
    sha256_hex(token.as_bytes())
}

// ---------------------------------------------------------------------------
// Resource tagging
//
// Everything the ARN-addressed tag surface needs, in one place: the bound, the records'
// one tag list, the ARN resolver and the keyed merge. `TagResource` and
// `ListTagsForResource` reach the SAME list through the accessors below, so a writer and
// a reader cannot hold their own copies of a resource's tags.
// ---------------------------------------------------------------------------

/// Maximum tags one resource holds, from CodeArtifact's own published tag restrictions
/// ("Tagging resources in CodeArtifact": "Maximum number of tags per resource – 50";
/// the same block's "Maximum key length – 128" and "Maximum value length – 256" are
/// what `TagKey` and `TagValue` model).
///
/// This is an ACCUMULATED bound on the resource, not on one request: `TagList` is
/// modelled `@length(0..200)`, so the boundary validator refuses an oversized single
/// batch, and nothing but this constant stops a sequence of small batches from growing
/// one resource's tag set without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The sentence every tag-quota refusal of this service carries.
///
/// The bound is on the tag set a request would leave the resource HOLDING, and the
/// service states the refusal that way — as a statement of the rule, naming the bound
/// and nothing about the particular request. [`MAX_TAGS_PER_RESOURCE`] fills the one
/// variable part.
///
/// One owner, so `TagResource` and every create site that carries a `tags` member word
/// the refusal the same way rather than each inventing its own sentence.
pub fn tag_quota_exceeded_message() -> String {
    format!("The resultant tag set must not have more than {MAX_TAGS_PER_RESOURCE} user tags.")
}

/// The tag accessors, a second `impl` block beside the store's own so this whole surface
/// reads in one place.
impl ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState {
    /// The tags of the resource an ARN named, resolved through [`ResourceRef`].
    ///
    /// `None` means this store holds no such resource — the same rule as
    /// [`Self::domain`], extended through the domain to its repositories and package
    /// groups — which each ARN-addressed operation answers with its own modelled
    /// not-found error.
    pub fn resource_tags(&self, reference: &ResourceRef) -> Option<&[Tag]> {
        match reference {
            ResourceRef::Domain { owner, name } => {
                Some(self.domain(name, owner.as_deref())?.tags.as_slice())
            }
            ResourceRef::Repository {
                owner,
                domain,
                repository,
            } => Some(
                self.domain(domain, owner.as_deref())?
                    .repositories
                    .get(repository)?
                    .tags
                    .as_slice(),
            ),
            ResourceRef::PackageGroup {
                owner,
                domain,
                pattern,
            } => Some(
                self.domain(domain, owner.as_deref())?
                    .package_groups
                    .get(pattern)?
                    .tags
                    .as_slice(),
            ),
        }
    }

    /// [`Self::resource_tags`], for an operation that rewrites the tag set it resolves.
    pub fn resource_tags_mut(&mut self, reference: &ResourceRef) -> Option<&mut Vec<Tag>> {
        match reference {
            ResourceRef::Domain { owner, name } => {
                Some(&mut self.domain_mut(name, owner.as_deref())?.tags)
            }
            ResourceRef::Repository {
                owner,
                domain,
                repository,
            } => Some(
                &mut self
                    .domain_mut(domain, owner.as_deref())?
                    .repositories
                    .get_mut(repository)?
                    .tags,
            ),
            ResourceRef::PackageGroup {
                owner,
                domain,
                pattern,
            } => Some(
                &mut self
                    .domain_mut(domain, owner.as_deref())?
                    .package_groups
                    .get_mut(pattern)?
                    .tags,
            ),
        }
    }
}

/// A resource of THIS service that a caller addressed by ARN, as the identifiers the
/// store is keyed by.
///
/// Only the kinds this store actually holds are represented — the three that carry a tag
/// set. A CodeArtifact ARN naming anything else (a package, a package version, an asset)
/// resolves to no record here, which [`taggable_resource`] reports by answering `None`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ResourceRef {
    /// `arn:<partition>:codeartifact:<region>:<owner>:domain/<name>`.
    Domain {
        /// The account the ARN pins the resource to, or `None` when the ARN's account
        /// field is empty — the same optional qualifier the `domainOwner` member is.
        owner: Option<String>,
        name: String,
    },
    /// `arn:<partition>:codeartifact:<region>:<owner>:repository/<domain>/<name>`.
    Repository {
        owner: Option<String>,
        domain: String,
        repository: String,
    },
    /// `arn:<partition>:codeartifact:<region>:<owner>:package-group/<domain><pattern>`,
    /// the form [`package_group_arn`] mints: the pattern is a path from the domain root,
    /// so it carries its own `/` separators and its leading `/` is the one between the
    /// domain segment and the path. The segments of that path are percent-encoded in the
    /// ARN and decoded back here ([`decode_arn_resource_path`]), so `pattern` is the
    /// identifier itself.
    PackageGroup {
        owner: Option<String>,
        domain: String,
        /// The stored pattern, leading `/` included — the key `Domain::package_groups`
        /// holds a group under.
        pattern: String,
    },
}

/// The resource a PARSED ARN of this service's namespace names, read from its ACCOUNT
/// and RESOURCE fields.
///
/// Takes the `Arn` that `service_common::arn::parse_of(value, SERVICE_SIGNING_NAME)`
/// yields — the one parse of a caller-supplied ARN — so a value that is not a
/// well-formed ARN of this namespace never reaches here: that is a bad REQUEST, a
/// different answer from a resource that is not held. `None` here means the ARN is ours
/// and well formed, but its resource field names no kind this store holds.
///
/// The REGION field is deliberately not read: this store is not partitioned per region
/// (see [`ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::domains`]), so
/// an ARN resolves to the one record of that name whatever region minted it.
#[must_use]
pub fn taggable_resource(arn: &::service_common::arn::Arn<'_>) -> Option<ResourceRef> {
    // An empty account field pins the ARN to no account, so it qualifies nothing.
    let owner = (!arn.account.is_empty()).then(|| arn.account.to_string());

    // The shape of the RESOURCE field is this service's own: `<type>/<id…>`, where a
    // package group's id is a path carrying further `/` separators.
    let mut segments = arn.resource.splitn(3, '/');
    match (segments.next()?, segments.next(), segments.next()) {
        ("domain", Some(name), None) if !name.is_empty() => Some(ResourceRef::Domain {
            owner,
            name: name.to_string(),
        }),
        ("repository", Some(domain), Some(repository))
            if !domain.is_empty() && !repository.is_empty() && !repository.contains('/') =>
        {
            Some(ResourceRef::Repository {
                owner,
                domain: domain.to_string(),
                repository: repository.to_string(),
            })
        }
        // A package group's identifier is a PATH, so everything after the domain segment
        // belongs to it, `/` separators included — the leading one is what makes `/*` the
        // ancestor of `/npm/*`. The path is percent-encoded in the ARN this service mints
        // (`package_group_arn`), so it is decoded back into the pattern the store is keyed
        // by; an ARN written out by hand with literal characters decodes to itself.
        ("package-group", Some(domain), Some(path)) if !domain.is_empty() && !path.is_empty() => {
            let pattern = decode_arn_resource_path(path)?;
            Some(ResourceRef::PackageGroup {
                owner,
                domain: domain.to_string(),
                pattern: format!("/{pattern}"),
            })
        }
        _ => None,
    }
}

/// Why a caller-supplied `resourceArn` names no resource of this service.
///
/// Both tag operations model the same two refusals for the same two reasons, so the
/// reason is decided ONCE (in [`resolve_taggable`]) and each handler lifts it into its
/// own modelled error enum — the two cannot drift into disagreeing about which ARN is a
/// bad request and which is a missing resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagArnRefusal {
    /// Not a well-formed ARN of this service's namespace: it addresses nothing this API
    /// can reach, so it is a bad REQUEST (`ValidationException`, HTTP 400).
    NotThisServicesArn,
    /// A well-formed ARN of this namespace whose resource field names no kind this store
    /// holds (`ResourceNotFoundException`, HTTP 404).
    NoSuchResource,
}

/// The resource a caller-supplied `resourceArn` names, or why it names none.
///
/// The ONE place a tag operation parses an ARN: [`service_common::arn::parse_of`] decides
/// whether the value is this service's ARN at all, and [`taggable_resource`] which record
/// kind it addresses.
pub fn resolve_taggable(resource_arn: &str) -> Result<ResourceRef, TagArnRefusal> {
    let parsed = ::service_common::arn::parse_of(resource_arn, SERVICE_SIGNING_NAME)
        .ok_or(TagArnRefusal::NotThisServicesArn)?;
    taggable_resource(&parsed).ok_or(TagArnRefusal::NoSuchResource)
}

/// The `repository` value of the modelled `ResourceType` enum — the counterpart of
/// [`DOMAIN_RESOURCE_TYPE`]. (`ResourceType` models exactly `asset`, `domain`, `package`,
/// `package-version` and `repository`; see `RESOURCE_TYPE_VALUES` in `ops::types`.)
pub const REPOSITORY_RESOURCE_TYPE: &str = "repository";

/// The `package` value of the modelled `ResourceType` enum — what a refusal reports for a
/// package COORDINATE (`(format, namespace, name)`), the other two members of the same
/// enum this service's package-scoped reads name.
pub const PACKAGE_RESOURCE_TYPE: &str = "package";

/// The `package-version` value of the modelled `ResourceType` enum, reported beside the
/// fully-qualified [`package_version_resource_id`] of the version a refusal is about.
pub const PACKAGE_VERSION_RESOURCE_TYPE: &str = "package-version";

/// The `asset` value of the modelled `ResourceType` enum, reported for one asset of a
/// package version.
pub const ASSET_RESOURCE_TYPE: &str = "asset";

/// The `resourceType` a modelled error reports for a resolved resource.
///
/// `None` for a package group: `ResourceType` declares no value for one, so the member is
/// left unset rather than filled with a spelling the model does not define.
#[must_use]
pub fn reported_resource_type(reference: &ResourceRef) -> Option<String> {
    match reference {
        ResourceRef::Domain { .. } => Some(DOMAIN_RESOURCE_TYPE.to_string()),
        ResourceRef::Repository { .. } => Some(REPOSITORY_RESOURCE_TYPE.to_string()),
        ResourceRef::PackageGroup { .. } => None,
    }
}

/// The `resourceId` a modelled error reports for a resolved resource: the IDENTIFIER of
/// the resource, never the ARN the caller addressed it by.
///
/// `ResourceNotFoundException$resourceId` is documented as "The ID of the resource", and a
/// client feeds it back into a name-scoped call (or matches it against a name), so an ARN
/// there is unusable. Each kind reports what this store keys it by: a domain its name, a
/// repository its own name inside its domain, a package group its pattern.
#[must_use]
pub fn reported_resource_id(reference: &ResourceRef) -> String {
    match reference {
        ResourceRef::Domain { name, .. } => name.clone(),
        ResourceRef::Repository { repository, .. } => repository.clone(),
        ResourceRef::PackageGroup { pattern, .. } => pattern.clone(),
    }
}

/// The `ResourceNotFoundException` message for an ARN-addressed resource this store does
/// not hold: the sentence the resource's OWN kind is refused with everywhere else in this
/// service ([`domain_not_found_message`] and friends), so a caller that addressed a domain
/// by ARN reads the same refusal as one that named it.
#[must_use]
pub fn tagged_resource_not_found_message(reference: &ResourceRef) -> String {
    match reference {
        ResourceRef::Domain { owner, name } => domain_not_found_message(name, owner.as_deref()),
        ResourceRef::Repository {
            owner,
            domain,
            repository,
        } => repository_not_found_message(domain, owner.as_deref(), repository),
        ResourceRef::PackageGroup {
            owner,
            domain,
            pattern,
        } => package_group_not_found_message(domain, owner.as_deref(), pattern),
    }
}

/// `held` with `incoming` applied, the way CodeArtifact's `TagResource` documents it
/// ("Adds or updates tags for a resource"): a key the resource already holds keeps its
/// position and takes the new value, and a key it does not hold is appended.
///
/// Tag KEYS are the identity of a tag, so the result holds one entry per distinct key —
/// including when one request repeats a key, where the last value in the request wins.
/// Values are copied verbatim: a tag value is an opaque Unicode string, never
/// normalised, trimmed or re-encoded.
#[must_use]
pub fn merged_tags(held: &[Tag], incoming: &[Tag]) -> Vec<Tag> {
    let mut merged = held.to_vec();
    for tag in incoming {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(existing) => existing.value = tag.value.clone(),
            None => merged.push(tag.clone()),
        }
    }
    merged
}

/// A stored CodeArtifact domain.
#[derive(Debug, Clone)]
pub struct Domain {
    pub name: String,
    /// The 12-digit account that owns the domain.
    pub owner: String,
    pub arn: String,
    pub status: String,
    pub created_time: SystemTime,
    pub encryption_key: Option<String>,
    pub s3_bucket_arn: String,
    pub tags: Vec<Tag>,
    pub policy: Option<ResourcePolicy>,
    /// The domain's repositories, keyed by repository name. `repositoryCount` is
    /// derived from this map's length and never stored beside it.
    pub repositories: BTreeMap<String, Repository>,
    /// The domain's package groups, keyed by their pattern — which the model calls out
    /// as the group's identifier (`CreatePackageGroupRequest$packageGroup`: "The pattern
    /// is also the identifier of the package group").
    ///
    /// A domain always holds at least [`ROOT_PACKAGE_GROUP_PATTERN`], inserted when the
    /// domain is created. The parent/child hierarchy is NOT stored here: it is derived
    /// from the patterns themselves on every read ([`Domain::package_group_parent`]), so
    /// inserting an intermediate group reparents the groups below it with no rewrite.
    pub package_groups: BTreeMap<String, PackageGroup>,
}

impl Domain {
    /// Attach `document` as this domain's resource policy, replacing any policy it already
    /// holds, and answer with the record that was stored.
    ///
    /// The revision the record carries identifies the DOCUMENT that is now attached — see
    /// [`attached_policy`].
    pub fn attach_policy(&mut self, document: String) -> ResourcePolicy {
        let policy = attached_policy(&self.arn, document);
        self.policy = Some(policy.clone());
        policy
    }

    /// The modelled `DomainDescription` for this record.
    pub fn description(&self) -> DomainDescription {
        DomainDescription {
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_time: Some(self.created_time.into()),
            encryption_key: self.encryption_key.clone(),
            repository_count: self.repository_count(),
            asset_size_bytes: self.asset_size_bytes(),
            s3_bucket_arn: Some(self.s3_bucket_arn.clone()),
        }
    }

    /// The modelled `DomainDescription` of this domain AFTER it has been deleted: the
    /// projection [`Domain::description`] builds, with `status` moved to the terminal
    /// [`DOMAIN_STATUS_DELETED`].
    ///
    /// A delete answers for the post-delete state of the resource, so the status it
    /// reports is the one the domain reached by being removed — never the `Active` the
    /// record carried while it was still held.
    pub fn deleted_description(&self) -> DomainDescription {
        DomainDescription {
            status: Some(DOMAIN_STATUS_DELETED.to_string()),
            ..self.description()
        }
    }

    /// The modelled `DomainSummary` for this record — the subset of
    /// [`Domain::description`] that `ListDomains` returns.
    pub fn summary(&self) -> DomainSummary {
        DomainSummary {
            name: Some(self.name.clone()),
            owner: Some(self.owner.clone()),
            arn: Some(self.arn.clone()),
            status: Some(self.status.clone()),
            created_time: Some(self.created_time.into()),
            encryption_key: self.encryption_key.clone(),
        }
    }

    /// `repositoryCount`, derived from the repositories actually held.
    ///
    /// Saturating rather than wrapping: the modelled member is an `i32`, and a count
    /// that overflowed it would otherwise report a negative number of repositories.
    fn repository_count(&self) -> i32 {
        i32::try_from(self.repositories.len()).unwrap_or(i32::MAX)
    }

    /// Every repository of this domain that the repository `from` reaches through its
    /// upstream chain, nearest first and each named once. `from` itself is NOT in the
    /// answer: it is the repository the chain is searched on behalf of.
    ///
    /// Breadth first, in each repository's own upstream order, because that is the order a
    /// repository's upstreams are searched in ("the order of the upstream repositories in
    /// the list determines their priority order when CodeArtifact looks for a requested
    /// package version" — `UpstreamRepository`) and a nearer copy wins over a further one.
    /// The visited set is what makes a chain that names a repository twice (or loops back)
    /// stop, so the walk is bounded by the repositories the domain holds.
    ///
    /// This is the ONE traversal every operation that resolves a package through upstreams
    /// uses — a repository endpoint serving a version it does not hold itself, and a copy
    /// asked to include the versions upstream of its source.
    pub fn upstream_chain(&self, from: &str) -> Vec<String> {
        let mut seen: BTreeSet<String> = BTreeSet::from([from.to_string()]);
        let mut frontier: VecDeque<String> = VecDeque::from([from.to_string()]);
        let mut chain: Vec<String> = Vec::new();
        while let Some(name) = frontier.pop_front() {
            let Some(repository) = self.repositories.get(&name) else {
                continue;
            };
            for upstream in &repository.upstreams {
                let Some(next) = upstream.repository_name.as_deref() else {
                    continue;
                };
                if !seen.insert(next.to_string()) {
                    continue;
                }
                chain.push(next.to_string());
                frontier.push_back(next.to_string());
            }
        }
        chain
    }

    /// The package group most closely associated with the package `path`
    /// ([`package_path`]): of the groups whose pattern matches that path, the MOST
    /// SPECIFIC one — the one matching on the longest literal, with an exact pattern
    /// beating a relative pattern that matches the same literal.
    ///
    /// This is the ASSOCIATION question (`GetAssociatedPackageGroup`), which compares a
    /// pattern against a package path character by character, so a pattern naming part
    /// of a component still matches; [`Domain::package_group_parent`] answers the
    /// HIERARCHY question between two patterns, which compares whole components.
    ///
    /// Never `None` for a domain that holds its root group, whose pattern matches every
    /// package.
    pub fn associated_package_group(&self, path: &str) -> Option<&PackageGroup> {
        self.package_groups
            .values()
            .filter(|group| pattern_matches(&group.pattern, path))
            .max_by_key(|group| pattern_specificity(&group.pattern))
    }

    /// `assetSizeBytes`: the total size of every asset the domain's repositories hold.
    ///
    /// Derived, never stored, for the same reason as [`Domain::repository_count`]: a
    /// publish or a delete changes it, and a stored copy would be the one place that
    /// forgot to move. Saturating, so a total past `i64::MAX` reports the ceiling rather
    /// than wrapping negative.
    pub fn asset_size_bytes(&self) -> i64 {
        self.repositories.values().fold(0i64, |total, repository| {
            total.saturating_add(repository.asset_size_bytes())
        })
    }

    /// The modelled `PackageGroupDescription` of the group this domain holds under
    /// `pattern`, or `None` when it holds no such group.
    ///
    /// `PackageGroupDescription` and `PackageGroupSummary` declare the SAME members
    /// (`smithy_query(query="shape",shape="PackageGroupSummary")`), so this is the one
    /// projection — [`Domain::package_group_summary`] — carried across to the other
    /// struct, rather than a second copy of it: a change to a derived member cannot land
    /// in one of them only and let a listing disagree with a read.
    pub fn package_group_description(&self, pattern: &str) -> Option<PackageGroupDescription> {
        let summary = self.package_group_summary(self.package_groups.get(pattern)?);
        Some(PackageGroupDescription {
            arn: summary.arn,
            pattern: summary.pattern,
            domain_name: summary.domain_name,
            domain_owner: summary.domain_owner,
            created_time: summary.created_time,
            contact_info: summary.contact_info,
            description: summary.description,
            origin_configuration: summary.origin_configuration,
            parent: summary.parent,
        })
    }

    /// The DIRECT parent of `pattern`: the most specific group this domain holds whose
    /// pattern covers a strict prefix of the path `pattern` covers.
    ///
    /// `None` only for the root group, which covers the whole domain and so has nothing
    /// above it.
    pub fn package_group_parent(&self, pattern: &str) -> Option<&PackageGroup> {
        let path = covered_path(pattern);
        self.package_groups
            .values()
            .filter(|candidate| covers_strictly(&candidate.pattern, &path))
            .max_by_key(|candidate| covered_path(&candidate.pattern).len())
    }

    /// Is the group under `pattern` a DIRECT child of the group under `parent_pattern`?
    ///
    /// The same relation [`Domain::package_group_parent`] reports as a group's `parent`,
    /// read the other way round, so the children a listing answers with are exactly the
    /// groups that name that listing's group as their parent. Derived from the patterns
    /// held at this call, so a group inserted in between moves the child off this parent
    /// with no rewrite.
    pub fn is_direct_sub_package_group(&self, parent_pattern: &str, pattern: &str) -> bool {
        self.package_group_parent(pattern)
            .is_some_and(|parent| parent.pattern == parent_pattern)
    }

    /// The modelled `PackageGroupSummary` of one group this domain holds.
    ///
    /// `PackageGroupSummary` carries every member of `PackageGroupDescription`
    /// (`smithy_query(query="shape",shape="PackageGroupSummary")`), so a listing reports
    /// the same derived parent and origin configuration a read of the single group does —
    /// computed here from the groups held AT THIS CALL rather than stored.
    pub fn package_group_summary(&self, group: &PackageGroup) -> PackageGroupSummary {
        PackageGroupSummary {
            arn: Some(group.arn.clone()),
            pattern: Some(group.pattern.clone()),
            domain_name: Some(group.domain_name.clone()),
            domain_owner: Some(group.domain_owner.clone()),
            created_time: Some(group.created_time.into()),
            contact_info: group.contact_info.clone(),
            description: group.description.clone(),
            origin_configuration: Some(PackageGroupOriginConfiguration {
                restrictions: Some(self.origin_restrictions(group)),
            }),
            parent: self
                .package_group_parent(&group.pattern)
                .map(PackageGroup::reference),
        }
    }

    /// One modelled restriction per `PackageGroupOriginRestrictionType`, which is the
    /// whole `originConfiguration` of a group: the type list is the model's own enum, so
    /// a type added to the model shows up here without another edit.
    fn origin_restrictions(
        &self,
        group: &PackageGroup,
    ) -> HashMap<String, PackageGroupOriginRestriction> {
        PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES
            .iter()
            .map(|restriction_type| {
                (
                    (*restriction_type).to_string(),
                    self.origin_restriction(group, restriction_type),
                )
            })
            .collect()
    }

    /// One origin restriction of `group`, resolved.
    ///
    /// `mode` is what the group itself carries. `effectiveMode` is the same value unless
    /// the group inherits, in which case it is "the value of `mode` of the first parent
    /// group which does not have a value of `INHERIT`"
    /// (`PackageGroupOriginRestriction$effectiveMode`), and `inheritedFrom` names that
    /// group.
    fn origin_restriction(
        &self,
        group: &PackageGroup,
        restriction_type: &str,
    ) -> PackageGroupOriginRestriction {
        // Walk up while the mode inherits. Each step moves to a group covering a
        // STRICTLY shorter path, so the walk cannot cycle and ends at the root.
        //
        // The walk breaks at the root, whose modes never inherit as long as the root is
        // created by `PackageGroup::root` — so `effective_mode` below is a resolved
        // mode. An `UpdatePackageGroupOriginConfiguration` that ever let the ROOT group
        // carry `INHERIT` would make this report `INHERIT` as the effective mode, and
        // must refuse that instead: the root has nothing to inherit from.
        let mut source = group;
        let mut inherited_from = None;
        while source.restriction_mode(restriction_type) == ORIGIN_MODE_INHERIT {
            match self.package_group_parent(&source.pattern) {
                Some(parent) => {
                    source = parent;
                    inherited_from = Some(parent.reference());
                }
                None => break,
            }
        }

        let effective_mode = source.restriction_mode(restriction_type).to_string();
        PackageGroupOriginRestriction {
            mode: Some(group.restriction_mode(restriction_type).to_string()),
            effective_mode: Some(effective_mode.clone()),
            inherited_from,
            // "The number of repositories in the allowed repository list"
            // (`PackageGroupOriginRestriction$repositoriesCount`) — a list only an
            // ALLOW_SPECIFIC_REPOSITORIES restriction has, counted on the group the
            // effective mode came from.
            repositories_count: (effective_mode == ORIGIN_MODE_ALLOW_SPECIFIC_REPOSITORIES).then(
                || {
                    i64::try_from(source.allowed_repositories(restriction_type).len())
                        .unwrap_or(i64::MAX)
                },
            ),
        }
    }
}

/// A stored package group.
///
/// The group's place in the hierarchy is not a field: it follows from [`Self::pattern`]
/// and the other patterns the domain holds, so it is derived where it is reported.
#[derive(Debug, Clone)]
pub struct PackageGroup {
    /// The pattern, which is also the group's identifier within the domain.
    pub pattern: String,
    pub domain_name: String,
    pub domain_owner: String,
    pub arn: String,
    pub created_time: SystemTime,
    pub contact_info: Option<String>,
    pub description: Option<String>,
    /// The group's own origin restriction per `PackageGroupOriginRestrictionType`. A
    /// type missing from the map reads as [`ORIGIN_MODE_INHERIT`].
    pub restrictions: BTreeMap<String, OriginRestriction>,
    pub tags: Vec<Tag>,
}

impl PackageGroup {
    /// The root group of a domain: it has no parent, so none of its restrictions can
    /// inherit — they are the domain's effective defaults, and CodeArtifact's root group
    /// allows publishing and both upstream kinds. The service creates this group itself,
    /// so it also carries the service-assigned [`ROOT_PACKAGE_GROUP_DESCRIPTION`].
    pub fn root(region: &str, domain: &str, owner: &str, created_time: SystemTime) -> Self {
        Self {
            pattern: ROOT_PACKAGE_GROUP_PATTERN.to_string(),
            domain_name: domain.to_string(),
            domain_owner: owner.to_string(),
            arn: package_group_arn(region, domain, ROOT_PACKAGE_GROUP_PATTERN),
            created_time,
            contact_info: None,
            description: Some(ROOT_PACKAGE_GROUP_DESCRIPTION.to_string()),
            restrictions: origin_restrictions_in_mode(ORIGIN_MODE_ALLOW),
            tags: Vec::new(),
        }
    }

    /// How this group is referred to from another group's description — as a `parent`,
    /// or as the `inheritedFrom` of a restriction.
    pub fn reference(&self) -> PackageGroupReference {
        PackageGroupReference {
            arn: Some(self.arn.clone()),
            pattern: Some(self.pattern.clone()),
        }
    }

    /// This group's OWN mode for `restriction_type` — not the effective one.
    pub fn restriction_mode(&self, restriction_type: &str) -> &str {
        self.restrictions
            .get(restriction_type)
            .map_or(ORIGIN_MODE_INHERIT, |restriction| {
                restriction.mode.as_str()
            })
    }

    /// The repositories this group's `restriction_type` allows, which is meaningful only
    /// in [`ORIGIN_MODE_ALLOW_SPECIFIC_REPOSITORIES`].
    pub fn allowed_repositories(&self, restriction_type: &str) -> &BTreeSet<String> {
        /// Borrowed when the group carries no restriction of that type at all, so the
        /// caller never has to handle a missing list.
        static NONE: BTreeSet<String> = BTreeSet::new();

        self.restrictions
            .get(restriction_type)
            .map_or(&NONE, |restriction| &restriction.allowed_repositories)
    }

    /// This group's restriction of `restriction_type`, for a writer: the stored entry,
    /// materialised in [`ORIGIN_MODE_INHERIT`] when the group carries none.
    ///
    /// A missing entry already READS as `INHERIT` with no allowed repositories
    /// ([`PackageGroup::restriction_mode`], [`PackageGroup::allowed_repositories`]), so
    /// materialising it changes nothing any reader reports.
    pub fn restriction_mut(&mut self, restriction_type: &str) -> &mut OriginRestriction {
        self.restrictions
            .entry(restriction_type.to_string())
            .or_insert_with(|| OriginRestriction {
                mode: ORIGIN_MODE_INHERIT.to_string(),
                allowed_repositories: BTreeSet::new(),
            })
    }
}

/// One stored origin restriction of a package group.
#[derive(Debug, Clone)]
pub struct OriginRestriction {
    /// A `PackageGroupOriginRestrictionMode` value.
    pub mode: String,
    /// The repositories named by `ALLOW_SPECIFIC_REPOSITORIES`, which
    /// `UpdatePackageGroupOriginConfiguration` adds to and removes from. A set, because
    /// a repository is either allowed or not and the allowed list is reported as a
    /// count.
    pub allowed_repositories: BTreeSet<String>,
}

/// Every `PackageGroupOriginRestrictionType` in `mode`, with no allowed repositories:
/// the restriction map a group is created with — [`ORIGIN_MODE_ALLOW`] for the root
/// group, [`ORIGIN_MODE_INHERIT`] for every group created under it.
pub fn origin_restrictions_in_mode(mode: &str) -> BTreeMap<String, OriginRestriction> {
    PACKAGE_GROUP_ORIGIN_RESTRICTION_TYPE_VALUES
        .iter()
        .map(|restriction_type| {
            (
                (*restriction_type).to_string(),
                OriginRestriction {
                    mode: mode.to_string(),
                    allowed_repositories: BTreeSet::new(),
                },
            )
        })
        .collect()
}

/// The package groups a freshly created domain holds: just the root group.
pub fn initial_package_groups(
    region: &str,
    domain: &str,
    owner: &str,
    created_time: SystemTime,
) -> BTreeMap<String, PackageGroup> {
    BTreeMap::from([(
        ROOT_PACKAGE_GROUP_PATTERN.to_string(),
        PackageGroup::root(region, domain, owner, created_time),
    )])
}

/// The path a package group pattern covers, as segments, with the pattern's terminator
/// removed: `/*` covers nothing (the whole domain), `/npm/*` covers `npm`, and
/// `/npm/scope/pkg$` covers `npm`, `scope`, `pkg`.
///
/// A pattern is a `/`-separated path whose final element either is the wildcard `*` (the
/// group covers everything below the path before it) or names one package, optionally
/// marked exact with a trailing `$`. Only that terminator is interpreted; the segments
/// themselves are compared literally, which is all the hierarchy needs.
fn covered_path(pattern: &str) -> Vec<&str> {
    let mut segments: Vec<&str> = pattern.split('/').filter(|part| !part.is_empty()).collect();
    if segments.last().is_some_and(|last| *last == "*") {
        segments.pop();
    } else if let Some(last) = segments.last_mut() {
        let named = *last;
        *last = named.strip_suffix('$').unwrap_or(named);
    }
    segments
}

/// Whether `pattern` covers a STRICT prefix of `path` — the relation that makes the
/// group with that pattern an ANCESTOR of the group covering `path`.
///
/// Only a wildcard pattern covers anything below the path it names, so a pattern that
/// names a single package is nobody's ancestor.
fn covers_strictly(pattern: &str, path: &[&str]) -> bool {
    if !pattern.ends_with('*') {
        return false;
    }
    let ancestor = covered_path(pattern);
    ancestor.len() < path.len() && path.starts_with(&ancestor)
}

/// Split a package group pattern into the literal it matches a PACKAGE PATH on, and
/// whether that match is EXACT.
///
/// This is the association reading of a pattern (`GetAssociatedPackageGroup`), beside
/// [`covered_path`]'s hierarchy reading of the same string: a package path is matched
/// character by character, so a pattern naming part of a component still matches it.
///
/// A pattern ends in the terminator that decides how it matches (see "Package group
/// definition syntax and matching behavior" in the CodeArtifact User Guide): `$` matches
/// that one package path and nothing under it — an exact match — while `*` and `~` match
/// every package path beginning with the literal before them. A pattern carrying no
/// terminator names a single path, so it is read as exact.
fn pattern_body(pattern: &str) -> (&str, bool) {
    if let Some(body) = pattern.strip_suffix('$') {
        (body, true)
    } else if let Some(body) = pattern
        .strip_suffix('*')
        .or_else(|| pattern.strip_suffix('~'))
    {
        (body, false)
    } else {
        (pattern, true)
    }
}

/// Does `pattern` match the package `path` ([`package_path`])?
fn pattern_matches(pattern: &str, path: &str) -> bool {
    let (body, exact) = pattern_body(pattern);
    if exact {
        path == body
    } else {
        path.starts_with(body)
    }
}

/// Is `pattern`'s match of `path` an EXACT match — the one reported as
/// [`ASSOCIATION_TYPE_STRONG`] — rather than a RELATIVE one
/// ([`ASSOCIATION_TYPE_WEAK`])?
///
/// A package path is a sequence of `/`-separated components (the format, the optional
/// namespace, the name). The match is exact when the pattern's literal covers whole
/// components of the path — it is the entire path, or it stops where a component ends —
/// because the group then names the package's own format, namespace and name. It is
/// relative when the literal stops INSIDE a component, which is a prefix match on a
/// namespace or a name (what a `~`-terminated pattern does for a sub-namespace): the
/// group names something the package is only relative to.
pub fn match_is_exact(pattern: &str, path: &str) -> bool {
    let (body, exact) = pattern_body(pattern);
    if exact {
        return path == body;
    }
    match path.strip_prefix(body) {
        Some(rest) => rest.is_empty() || rest.starts_with('/') || body.ends_with('/'),
        None => false,
    }
}

/// How specific `pattern` is, for picking the most closely associated group: the length
/// of the literal it matches on, and — between two patterns matching the same literal —
/// the exact one, which matches strictly fewer packages.
///
/// Two patterns can still tie: `/npm/*` and `/npm/~` match the same literal the same
/// way, so they are EQUIVALENT as matchers and which one is reported is decided by the
/// group map's key order, not by a rule of the service. Nothing below depends on which
/// of two equivalent patterns wins; a caller that wants one to win creates the more
/// specific pattern.
fn pattern_specificity(pattern: &str) -> (usize, bool) {
    let (body, exact) = pattern_body(pattern);
    (body.len(), exact)
}

/// The path a package is matched against a package group pattern on:
/// `/<format>/<namespace>/<name>`, with the namespace segment omitted for a package that
/// has none (the formats whose versions carry no namespace component).
pub fn package_path(format: &str, namespace: Option<&str>, package: &str) -> String {
    match namespace {
        Some(namespace) => format!("/{format}/{namespace}/{package}"),
        None => format!("/{format}/{package}"),
    }
}

/// The package formats whose packages are addressed WITH a namespace, so an operation
/// that resolves a package by format must be given one: a Maven package's namespace is
/// its `groupId`, a Swift package's is its scope, and a generic package's is its
/// namespace (`GetAssociatedPackageGroup`'s own `namespace` doc). The remaining formats'
/// versions carry no corresponding component.
pub const NAMESPACE_REQUIRED_FORMATS: &[&str] = &["generic", "maven", "swift"];

/// A stored repository inside a domain.
///
/// Later slices hang the package-VERSION maps off [`Package`]; the fields here are the
/// ones the repository lifecycle and the package listing need.
#[derive(Debug, Clone)]
pub struct Repository {
    pub name: String,
    pub domain_name: String,
    pub domain_owner: String,
    pub administrator_account: String,
    pub arn: String,
    pub description: Option<String>,
    /// Upstream repositories in the caller's priority order, which CodeArtifact
    /// preserves — so this is an ordered `Vec`, not a set.
    pub upstreams: Vec<UpstreamRepositoryInfo>,
    pub external_connections: Vec<RepositoryExternalConnectionInfo>,
    pub created_time: SystemTime,
    pub tags: Vec<Tag>,
    pub policy: Option<ResourcePolicy>,
    /// The packages held in this repository, keyed by [`package_key`]. Sorted for the
    /// same reason as the maps above: `ListPackages` is `@paginated`, and a stable key
    /// order is what makes its opaque `nextToken` resolve to the same place next call.
    pub packages: BTreeMap<String, Package>,
}

impl Repository {
    /// Attach `document` as this repository's resource policy, replacing any policy it
    /// already holds, and answer with the record that was stored.
    ///
    /// Both policy-bearing records attach a policy through this one rule
    /// ([`Domain::attach_policy`]), so a revision minted on a domain and one minted on a
    /// repository cannot follow different schemes.
    pub fn attach_policy(&mut self, document: String) -> ResourcePolicy {
        let policy = attached_policy(&self.arn, document);
        self.policy = Some(policy.clone());
        policy
    }
    /// The modelled `RepositoryDescription` for this record.
    pub fn description(&self) -> RepositoryDescription {
        RepositoryDescription {
            name: Some(self.name.clone()),
            administrator_account: Some(self.administrator_account.clone()),
            domain_name: Some(self.domain_name.clone()),
            domain_owner: Some(self.domain_owner.clone()),
            arn: Some(self.arn.clone()),
            description: self.description.clone(),
            upstreams: Some(self.upstreams.clone()),
            external_connections: Some(self.external_connections.clone()),
            created_time: Some(self.created_time.into()),
        }
    }

    /// The modelled `RepositorySummary` for this record — the subset of
    /// [`Repository::description`] that the repository `List*` operations return.
    pub fn summary(&self) -> RepositorySummary {
        RepositorySummary {
            name: Some(self.name.clone()),
            administrator_account: Some(self.administrator_account.clone()),
            domain_name: Some(self.domain_name.clone()),
            domain_owner: Some(self.domain_owner.clone()),
            arn: Some(self.arn.clone()),
            description: self.description.clone(),
            created_time: Some(self.created_time.into()),
        }
    }

    /// The internal id this service knows this repository by, which its own refusals name
    /// it by ([`repository_id`]).
    pub fn id(&self) -> String {
        repository_id(&self.domain_owner, &self.domain_name, &self.name)
    }

    /// Resolve a package the way every package-scoped operation addresses it: by the
    /// `(format, namespace, package)` triple, never by name alone.
    pub fn package(&self, format: &str, namespace: Option<&str>, package: &str) -> Option<&Package> {
        self.packages.get(&package_key(format, namespace, package))
    }

    /// The total size of every asset this repository holds, summed from the assets
    /// themselves (see [`Domain::asset_size_bytes`] on why it is derived).
    pub fn asset_size_bytes(&self) -> i64 {
        self.packages.values().fold(0i64, |total, package| {
            total.saturating_add(package.asset_size_bytes())
        })
    }
}

/// The key a package is held under inside its repository.
///
/// A package's identity is the `(format, namespace, package)` triple — the same name in
/// two formats, or under two namespaces, is two different packages — so the key carries
/// all three. `/` is the separator because no component can contain one: `PackageFormat`
/// is an enum of lowercase words, and both `PackageNamespace` and `PackageName` are
/// modelled `^[^#/\s]+$`. A package with no namespace keys as `<format>//<package>`,
/// which no namespaced package can collide with for that same reason.
pub fn package_key(format: &str, namespace: Option<&str>, package: &str) -> String {
    format!("{format}/{}/{package}", namespace.unwrap_or_default())
}

/// `displayName`: the stored package's name written the way its FORMAT writes it.
///
/// The rule and its one documented variation come from the member itself: "The name of
/// the package that is displayed. The `displayName` varies depending on the package
/// version's format. For example, if an npm package is named `ui`, is in the namespace
/// `vue`, and has the format `npm`, then the `displayName` is `@vue/ui`"
/// (`smithy_query(query="doc",shape="PackageVersionDescription",member="displayName")`).
///
/// So the display name folds the namespace into the name exactly for the formats whose
/// namespace is a SCOPE ([`SCOPED_PACKAGE_FORMATS`]) — written `@<scope>/<name>`, the one
/// spelling the doc gives. Every other format writes its namespace as a separate
/// coordinate (a Maven `groupId`, a generic `namespace`) or carries none, so its display
/// name is the package name.
///
/// DERIVED from the coordinate the version is stored under and therefore always
/// available: nothing here is read out of package metadata, which is also why the
/// metadata-sourced members beside it (`summary`, `homePage`, `sourceCodeRepository`)
/// stay absent. A leading `@` already on the scope is not doubled, so a caller that
/// published `@vue` as the namespace and one that published `vue` get the same rendering.
pub fn package_display_name(format: &str, namespace: Option<&str>, package: &str) -> String {
    match namespace {
        Some(scope)
            if !scope.is_empty() && SCOPED_PACKAGE_FORMATS.contains(&format) =>
        {
            format!("@{}/{package}", scope.strip_prefix('@').unwrap_or(scope))
        }
        _ => package.to_string(),
    }
}

/// The distribution tag a package manager resolves "the latest version" through, and the
/// one `defaultDisplayVersion` names for a format that carries tags ("For npm packages,
/// it's the version referenced by the latest tag").
pub const LATEST_DIST_TAG: &str = "latest";

/// How many distribution tags one package carries at most.
///
/// A tag is a request-driven insert whose KEY the caller chooses (npm's
/// `PUT /-/package/<package>/dist-tags/<tag>`, and the `dist-tags` object of a publish
/// document), so the map needs a declared ceiling like every other per-request insert in
/// this store. The value is a chosen emulator bound, far above the handful of channels a
/// package is published on (`latest`, `next`, `beta`, a release line per major version);
/// AWS publishes no such limit.
pub const MAX_DIST_TAGS_PER_PACKAGE: usize = 100;

/// The asset an npm package version's content is stored as.
///
/// CodeArtifact keeps an npm version's tarball under ONE asset name, fixed for the format
/// rather than derived from the package name or the version — which is what lets
/// `GetPackageVersionAsset` address any npm version's content by the same `asset` value.
/// The name npm's own tarball URL publishes is derived from the version instead (see
/// `crate::service::npm_registry`), so the two are deliberately different strings.
pub const NPM_TARBALL_ASSET: &str = "package.tgz";

/// A stored package inside a repository.
///
/// A package is identified by the triple (`format`, `namespace`, `name`) — the same triple
/// every package-scoped operation addresses it with — and carries the origin restrictions
/// that decide whether new versions may be published into, or ingested from an upstream
/// into, this repository, plus the versions published into it. A package can legitimately
/// hold no versions at all: `PutPackageOriginConfiguration` is documented to create
/// exactly that when called for a package the repository does not yet hold.
#[derive(Debug, Clone)]
pub struct Package {
    pub format: String,
    /// `None` for a format whose versions carry no namespace component (the model's
    /// `PackageSummary$namespace` doc lists Python, NuGet, Ruby and Cargo).
    pub namespace: Option<String>,
    pub name: String,
    /// `publish`/`upstream` origin control, as `PutPackageOriginConfiguration` sets it.
    pub origin_restrictions: PackageOriginRestrictions,
    /// The package's versions, keyed by version string.
    pub versions: BTreeMap<String, PackageVersion>,
    /// The package's distribution tags — the named pointers a package manager resolves a
    /// request for "the latest version" through — keyed by tag name.
    ///
    /// Only a format whose package manager HAS tags ever fills this: npm's publish
    /// document carries a `dist-tags` object, and `latest` is the tag its clients resolve
    /// by default. A version published through `PublishPackageVersion` carries no
    /// metadata, so a generic package's tags stay empty.
    pub dist_tags: BTreeMap<String, String>,
}

impl Package {
    /// A package record created by something OTHER than a direct publish, carrying the
    /// unrestricted origin configuration: both publishing into it and ingesting a version
    /// into it from elsewhere are allowed until a `PutPackageOriginConfiguration` narrows
    /// it. This is the record `PutPackageOriginConfiguration` creates for a package the
    /// repository does not hold (its restrictions are then overwritten with the ones the
    /// caller asked for) and the record a copy creates in the destination repository.
    pub fn new(format: &str, namespace: Option<&str>, name: &str) -> Self {
        Self {
            format: format.to_string(),
            namespace: namespace.map(str::to_string),
            name: name.to_string(),
            origin_restrictions: PackageOriginRestrictions {
                publish: ORIGIN_RESTRICTION_ALLOW.to_string(),
                upstream: ORIGIN_RESTRICTION_ALLOW.to_string(),
            },
            versions: BTreeMap::new(),
            dist_tags: BTreeMap::new(),
        }
    }

    /// A package record created because a caller PUBLISHED a version directly into the
    /// repository: publishing stays allowed and upstream ingestion is blocked.
    ///
    /// CodeArtifact sets a package's origin controls when it creates the package, from HOW
    /// the package came to exist: a package created by publishing a version directly gets
    /// `publish: ALLOW` with `upstream: BLOCK`, which is what the controls are for — a
    /// package name that exists in a repository because someone published it there must not
    /// also be resolvable from an upstream or external source (the dependency-substitution
    /// defence). `PutPackageOriginConfiguration` is what moves it afterwards.
    ///
    /// Only the CREATE path chooses this: a publish into a package the repository already
    /// holds leaves that package's configuration exactly as it is.
    pub fn published(format: &str, namespace: Option<&str>, name: &str) -> Self {
        Self {
            origin_restrictions: PackageOriginRestrictions {
                publish: ORIGIN_RESTRICTION_ALLOW.to_string(),
                upstream: ORIGIN_RESTRICTION_BLOCK.to_string(),
            },
            ..Self::new(format, namespace, name)
        }
    }

    /// A package record created because a version was INGESTED into the repository from an
    /// upstream rather than published into it: publishing is blocked and upstream
    /// ingestion stays allowed.
    ///
    /// The mirror image of [`Self::published`], chosen for the same reason: the controls
    /// follow from how the package came to exist. A package name that exists in a
    /// repository because it was resolved from an upstream must not also be publishable
    /// directly into it, or a publisher could substitute its own content for the
    /// dependency a consumer already resolves from upstream.
    pub fn ingested(format: &str, namespace: Option<&str>, name: &str) -> Self {
        Self {
            origin_restrictions: PackageOriginRestrictions {
                publish: ORIGIN_RESTRICTION_BLOCK.to_string(),
                upstream: ORIGIN_RESTRICTION_ALLOW.to_string(),
            },
            ..Self::new(format, namespace, name)
        }
    }

    /// The modelled `PackageSummary` for this record — what the package `List*`
    /// operations return.
    pub fn summary(&self) -> PackageSummary {
        PackageSummary {
            format: Some(self.format.clone()),
            namespace: self.namespace.clone(),
            package: Some(self.name.clone()),
            origin_configuration: Some(self.origin_configuration()),
        }
    }

    /// The modelled `PackageOriginConfiguration` for this record.
    pub fn origin_configuration(&self) -> PackageOriginConfiguration {
        PackageOriginConfiguration {
            restrictions: Some(self.origin_restrictions.clone()),
        }
    }

    /// Whether origin control permits publishing a version into this package.
    pub fn publish_allowed(&self) -> bool {
        self.origin_restrictions.publish != ORIGIN_RESTRICTION_BLOCK
    }

    /// Whether origin control permits a version of this package being INGESTED into the
    /// repository from elsewhere — an upstream repository or an external connection — as
    /// `PutPackageOriginConfiguration`'s `upstream` restriction sets it ("the package origin
    /// configuration that determines if new versions of the package can be added to the
    /// repository from an external connection or upstream source").
    ///
    /// The companion of [`Self::publish_allowed`]: the two restrictions decide the two
    /// ways a version comes to exist in a repository, so they are read the same way. A
    /// package the repository does not hold at all carries no restriction, so ingesting its
    /// first version is permitted; this answers for a record that exists.
    pub fn upstream_allowed(&self) -> bool {
        self.origin_restrictions.upstream != ORIGIN_RESTRICTION_BLOCK
    }

    /// `defaultDisplayVersion`: the version of this package a client should DISPLAY, or
    /// `None` when the package holds none to display.
    ///
    /// The member's own documentation gives the rule per format: "For Maven and PyPI
    /// packages, it's the most recently published package version. For npm packages, it's
    /// the version referenced by the latest tag. If the latest tag is not set, it's the
    /// most recently published package version"
    /// (`smithy_query(query="doc",shape="ListPackageVersionsResult",member="defaultDisplayVersion")`).
    /// A package whose format carries distribution tags therefore reports the version its
    /// [`LATEST_DIST_TAG`] names, and every other package — and any package whose latest
    /// tag names no displayable version — falls to the most recently published one.
    ///
    /// Only a version in [`PACKAGE_VERSION_STATUS_PUBLISHED`] is a candidate, because that
    /// is the one status in which a version is both listed and resolvable: an `Unlisted`
    /// version is by its own definition not displayed, and an `Archived`, `Disposed` or
    /// `Unfinished` one is not a version a client can resolve. A package holding no such
    /// version therefore reports no display version rather than naming one a client could
    /// not fetch.
    ///
    /// Ties are broken by the greater version string, so the answer is a function of the
    /// stored records and not of the order they were walked in.
    pub fn default_display_version(&self) -> Option<String> {
        let tagged = self
            .dist_tags
            .get(LATEST_DIST_TAG)
            .and_then(|version| self.versions.get(version))
            .filter(|version| version.status == PACKAGE_VERSION_STATUS_PUBLISHED)
            .map(|version| version.version.clone());
        if tagged.is_some() {
            return tagged;
        }
        self.versions
            .values()
            .filter(|version| version.status == PACKAGE_VERSION_STATUS_PUBLISHED)
            .max_by(|left, right| {
                left.published_time
                    .cmp(&right.published_time)
                    .then_with(|| left.version.cmp(&right.version))
            })
            .map(|version| version.version.clone())
    }

    /// The total size of every asset this package's versions hold.
    pub fn asset_size_bytes(&self) -> i64 {
        self.versions.values().fold(0i64, |total, version| {
            total.saturating_add(version.asset_size_bytes())
        })
    }
}

/// A stored package version: its status, when it was published, where it entered the
/// domain, and the assets published into it.
#[derive(Debug, Clone)]
pub struct PackageVersion {
    pub version: String,
    /// One of `PackageVersionStatus`; a version is only ever stored in a status it can be
    /// observed in, so `Deleted` never appears here (a deleted version is gone).
    pub status: String,
    pub published_time: SystemTime,
    /// One of `PackageVersionOriginType`.
    pub origin_type: String,
    /// The repository the version entered the domain through, reported as the origin's
    /// `domainEntryPoint`.
    pub entry_point_repository: String,
    /// The version's assets, keyed by asset name.
    pub assets: BTreeMap<String, Asset>,
    /// The version's own METADATA document, as its package manager published it — an npm
    /// `package.json`, a Maven `pom.xml` — or `None` for a version published through
    /// `PublishPackageVersion`, which takes generic assets and no metadata.
    ///
    /// Kept because CodeArtifact reports a version's summary, home page, licenses, source
    /// repository and declared dependencies out of exactly this document rather than
    /// inventing them ("CodeArtifact extracts the dependencies for a package version from
    /// the metadata file for the package format"). It is not an ASSET: the format decides
    /// what a version's assets are, and for npm that is the tarball alone.
    pub manifest: Option<String>,
    /// The version's README, as the text its package manager published it as: the `readme`
    /// an npm publish document declares (or the `package/README*` file inside the published
    /// tarball), and the long description a PyPI upload carries.
    ///
    /// `GetPackageVersionReadme` "gets the readme file or descriptive text for a package
    /// version", so the text is what the publisher sent, kept verbatim and decoded as the
    /// UTF-8 it arrived as — a multi-byte readme round-trips unchanged. `None` for a version
    /// whose publish carried none, which is "this version has no readme" rather than an
    /// empty one.
    ///
    /// Extracted at INGEST rather than on each read: the text is a property of what was
    /// published, and deriving it per read would make a readme fetch re-read a version's
    /// whole tarball.
    pub readme: Option<String>,
    /// Which change of this version the next event announces: `1` for the change that
    /// created it, advanced by every later change to it.
    ///
    /// Reported as the event's `sequenceNumber`, which orders the announcements of ONE
    /// version. It is a property of the record rather than of the store, so two versions'
    /// numbers are independent and a version that is removed takes its counter with it.
    pub sequence_number: i64,
}

impl PackageVersion {
    /// `versionRevision`: an opaque token that identifies this version's CONTENT.
    ///
    /// Derived from everything a revision has to move with — the version, its metadata
    /// document and each asset's name and content digest — so two reads of an unchanged
    /// version report the same revision, adding an asset reports a new one, and a version
    /// restored to identical content reports the identical token. The version's STATUS is
    /// deliberately not among them: a revision identifies what the version holds, and an
    /// operation that only relabels it (a status move, a disposal) leaves the content — and
    /// therefore the token clients pin in `versionRevisions` — exactly as the publish
    /// produced it.
    /// Base64 of a SHA-256 digest, which is 44 characters: inside the modelled
    /// `PackageVersionRevision` length (1..50) and `^\S+$` pattern.
    ///
    /// Costs the NUMBER of assets, not their bytes: each asset's digest was computed once
    /// when the asset was accepted (see [`Asset::new`]), so no read ever hashes stored
    /// content again.
    pub fn revision(&self) -> String {
        use base64::Engine as _;

        let mut fields: Vec<&str> = vec![self.version.as_str()];
        // The metadata document is part of the version's content too: a version restored
        // with the same assets but a different manifest is not the same version.
        if let Some(manifest) = &self.manifest {
            fields.push(manifest.as_str());
        }
        for (name, asset) in &self.assets {
            fields.push(name.as_str());
            fields.push(asset.sha256_hex());
        }
        base64::engine::general_purpose::STANDARD.encode(identity_digest(&fields))
    }

    /// The modelled `PackageVersionDescription` for this record, inside `package`.
    ///
    /// `displayName` is DERIVED from the coordinate the version is stored under, by its
    /// format's own display rule ([`package_display_name`]), so every described version
    /// reports one.
    ///
    /// `summary`, `homePage`, `sourceCodeRepository` and `licenses` are the
    /// METADATA-SOURCED members: CodeArtifact reads them out of the package's own metadata
    /// file (a `package.json`, a `pom.xml`), so they are reported exactly where the version
    /// carries one ([`Self::manifest`]) and it declares them. A version published through
    /// `PublishPackageVersion` carries no such file, and a member the file does not declare
    /// stays absent — absent is precisely "this package version has no such value", which
    /// inventing a string would not be.
    ///
    /// `licenses` is reported as the list it is, and is EMPTY rather than absent for a
    /// version whose metadata names no license: a client that iterates `licenses` must see
    /// "no licenses", not "no information".
    pub fn description(&self, package: &Package) -> PackageVersionDescription {
        let metadata = self
            .manifest
            .as_deref()
            .and_then(crate::service::npm_manifest::read)
            .filter(|_| package.format == PACKAGE_FORMAT_NPM);
        let licenses = metadata
            .as_ref()
            .and_then(|metadata| metadata.license.clone())
            .map(|name| {
                vec![LicenseInfo {
                    name: Some(name),
                    url: None,
                }]
            })
            .unwrap_or_default();
        PackageVersionDescription {
            format: Some(package.format.clone()),
            namespace: package.namespace.clone(),
            package_name: Some(package.name.clone()),
            display_name: Some(package_display_name(
                &package.format,
                package.namespace.as_deref(),
                &package.name,
            )),
            version: Some(self.version.clone()),
            summary: metadata
                .as_ref()
                .and_then(|metadata| metadata.description.clone()),
            home_page: metadata
                .as_ref()
                .and_then(|metadata| metadata.homepage.clone()),
            source_code_repository: metadata
                .as_ref()
                .and_then(|metadata| metadata.repository_url.clone()),
            published_time: Some(self.published_time.into()),
            licenses: Some(licenses),
            revision: Some(self.revision()),
            status: Some(self.status.clone()),
            origin: Some(PackageVersionOrigin {
                domain_entry_point: Some(DomainEntryPoint {
                    repository_name: Some(self.entry_point_repository.clone()),
                    // Set only for a version pulled through an external connection.
                    external_connection_name: None,
                }),
                origin_type: Some(self.origin_type.clone()),
            }),
        }
    }

    /// The modelled `PackageVersionSummary` for this record — what `ListPackageVersions`
    /// returns, the listing projection of the same record [`Self::description`] reports.
    ///
    /// Every member is derived from the stored version, so a summary can never disagree
    /// with the description of the same version: the `revision` is the same derived token
    /// ([`Self::revision`]), the `publishedTime` is the same stored instant
    /// [`Self::description`] reports, and the `origin` is the same entry point and origin
    /// type. The coordinate members a description carries (`format`, `namespace`,
    /// `packageName`) are NOT repeated per summary — the listing reports the package's
    /// coordinate once, beside the page.
    pub fn summary(&self) -> PackageVersionSummary {
        PackageVersionSummary {
            version: self.version.clone(),
            revision: Some(self.revision()),
            status: self.status.clone(),
            // The instant the version was published, projected into the listing from the
            // same stored field the description reports: a listing a client sorts or
            // filters by publish time carries it per entry, so resolving the newest
            // version costs no per-version read.
            published_time: Some(self.published_time.into()),
            origin: Some(PackageVersionOrigin {
                domain_entry_point: Some(DomainEntryPoint {
                    repository_name: Some(self.entry_point_repository.clone()),
                    // Set only for a version pulled through an external connection.
                    external_connection_name: None,
                }),
                origin_type: Some(self.origin_type.clone()),
            }),
        }
    }

    /// The total size of this version's assets.
    pub fn asset_size_bytes(&self) -> i64 {
        self.assets.values().fold(0i64, |total, asset| {
            total.saturating_add(asset.size_bytes())
        })
    }

    /// Count one further change to this version, answering the number the event
    /// announcing that change carries ([`Self::sequence_number`]).
    pub fn advance_sequence(&mut self) -> i64 {
        self.sequence_number = self.sequence_number.saturating_add(1);
        self.sequence_number
    }

    /// The size of this version's metadata document, or `0` for a version that carries
    /// none. Retained caller content, so it counts towards
    /// [`ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState::stored_content_bytes`]
    /// even though it is not an asset.
    pub fn manifest_size_bytes(&self) -> i64 {
        self.manifest
            .as_ref()
            .and_then(|manifest| i64::try_from(manifest.len()).ok())
            .unwrap_or_default()
    }

    /// The size of every TEXT this version retains beside its assets: its metadata document
    /// and its readme. Retained caller content, like the document itself, so it counts
    /// towards what this emulator may hold.
    pub fn retained_text_bytes(&self) -> i64 {
        let readme = self
            .readme
            .as_ref()
            .and_then(|readme| i64::try_from(readme.len()).ok())
            .unwrap_or_default();
        self.manifest_size_bytes().saturating_add(readme)
    }
}

/// The `PackageVersionSortType` value that orders a version listing by when each version
/// was published. The enum models exactly this one value
/// (`smithy_query(query="shape",shape="PackageVersionSortType")`), so a listing is ordered
/// either by it or by the stored version order.
pub const PACKAGE_VERSION_SORT_PUBLISHED_TIME: &str = "PUBLISHED_TIME";

/// How many decimal digits [`package_version_order_key`] pads an inverted instant to:
/// enough for every value of the `u64` nanosecond counter it inverts, so the padded
/// numbers compare as numbers do when compared as text.
const INVERTED_INSTANT_DIGITS: usize = 20;

/// The key a package version is ORDERED (and therefore paginated) under in a
/// `ListPackageVersions` page, for the `sortBy` the request asked for.
///
/// `None` is the stored order: the version string, which is the key the version is held
/// under. [`PACKAGE_VERSION_SORT_PUBLISHED_TIME`] orders the MOST RECENTLY PUBLISHED
/// version first — the direction the operation's own `defaultDisplayVersion` names ("the
/// most recently published package version") — by inverting the publish instant into a
/// fixed-width decimal, so text order is reverse time order. The version string is
/// appended to keep the key unique when two versions carry the same instant, so no page
/// can drop or repeat a version whatever the clock resolution was.
pub fn package_version_order_key(version: &PackageVersion, sort_by: Option<&str>) -> String {
    match sort_by {
        Some(PACKAGE_VERSION_SORT_PUBLISHED_TIME) => {
            let nanos = version
                .published_time
                .duration_since(SystemTime::UNIX_EPOCH)
                .map_or(0, |since| since.as_nanos());
            let inverted = u128::from(u64::MAX).saturating_sub(nanos);
            format!(
                "{inverted:0width$}/{}",
                version.version,
                width = INVERTED_INSTANT_DIGITS
            )
        }
        _ => version.version.clone(),
    }
}

/// A stored asset: its name, the exact bytes that were published, and their hashes.
///
/// The bytes are kept verbatim — `GetPackageVersionAsset` has to answer with what
/// `PublishPackageVersion` was given, byte for byte. Their hashes are computed ONCE, when
/// the asset is accepted, and stored beside them:
///
/// * a hash can never disagree with the content it describes, because the fields are
///   private and [`Asset::new`] is the only way to make one — an asset is replaced
///   wholesale, never edited in place, so a stored digest cannot go stale; and
/// * no request pays for the bytes already in the store. Hashing on every read would
///   make a few-hundred-byte `DescribePackageVersion` cost a pass over every byte the
///   version holds (its revision digests each asset), so the work one caller can ask for
///   would be the PRODUCT of the asset ceiling and the asset size rather than the size of
///   the request they sent.
#[derive(Debug, Clone)]
pub struct Asset {
    name: String,
    content: Vec<u8>,
    /// Keyed by the model's `HashAlgorithm` wire values, computed from `content`.
    hashes: HashMap<String, String>,
}

impl Asset {
    /// Accept `content` as the asset named `name`, computing its hashes now — one pass
    /// per algorithm over the bytes of THIS request, and never again for this asset.
    pub fn new(name: &str, content: Vec<u8>) -> Self {
        use md5::Digest as _;

        // One entry per value of the model's `HashAlgorithm` enum, lowercase hex, so a
        // client can look up the algorithm it verifies with.
        let hashes = HashMap::from([
            (
                HASH_ALGORITHM_MD5.to_string(),
                hex::encode(md5::Md5::digest(&content)),
            ),
            (
                HASH_ALGORITHM_SHA1.to_string(),
                hex::encode(sha1::Sha1::digest(&content)),
            ),
            (HASH_ALGORITHM_SHA256.to_string(), sha256_hex(&content)),
            (
                HASH_ALGORITHM_SHA512.to_string(),
                hex::encode(sha2::Sha512::digest(&content)),
            ),
        ]);
        Self {
            name: name.to_string(),
            content,
            hashes,
        }
    }

    /// The modelled `AssetSummary` for this record.
    pub fn summary(&self) -> AssetSummary {
        AssetSummary {
            name: self.name.clone(),
            size: Some(self.size_bytes()),
            hashes: Some(self.hashes.clone()),
        }
    }

    /// The asset's name.
    ///
    /// Read path for `GetPackageVersionAsset`, which is still a stub with its own issue:
    /// this and [`Asset::content`] are how that operation will reach a stored asset
    /// without the fields being public.
    pub fn name(&self) -> &str {
        &self.name
    }

    /// The asset's bytes, exactly as they were published.
    pub fn content(&self) -> &[u8] {
        &self.content
    }

    /// The asset's size, saturating at `i64::MAX` because the modelled member is an
    /// `i64` while a length is a `usize`.
    pub fn size_bytes(&self) -> i64 {
        i64::try_from(self.content.len()).unwrap_or(i64::MAX)
    }

    /// The asset's stored SHA-256, lowercase hex — the digest a caller declares in
    /// `assetSHA256`, the one two assets are compared by, and the one a version's
    /// revision is derived from. Always present: [`Asset::new`] computes it.
    pub fn sha256_hex(&self) -> &str {
        self.hashes
            .get(HASH_ALGORITHM_SHA256)
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// The asset's stored SHA-1, lowercase hex — the digest an npm client reads as a
    /// tarball's `shasum`. Always present: [`Asset::new`] computes it.
    pub fn sha1_hex(&self) -> &str {
        self.hashes
            .get(HASH_ALGORITHM_SHA1)
            .map(String::as_str)
            .unwrap_or_default()
    }

    /// The asset's stored SHA-512, base64 of the digest BYTES — the spelling a
    /// Subresource-Integrity value (`sha512-…`) carries, which is how an npm client states
    /// a tarball's integrity. Always present: [`Asset::new`] computes it.
    pub fn sha512_base64(&self) -> String {
        use base64::Engine as _;

        let digest = self
            .hashes
            .get(HASH_ALGORITHM_SHA512)
            .map(String::as_str)
            .unwrap_or_default();
        base64::engine::general_purpose::STANDARD.encode(hex::decode(digest).unwrap_or_default())
    }
}

/// The `HashAlgorithm` value the `assetSHA256` integrity check and the stored revision
/// both read.
const HASH_ALGORITHM_SHA256: &str = "SHA-256";

/// The model's other three `HashAlgorithm` values, each named once so the keys of an
/// asset's `hashes` map cannot drift from the enum.
const HASH_ALGORITHM_MD5: &str = "MD5";
const HASH_ALGORITHM_SHA1: &str = "SHA-1";
const HASH_ALGORITHM_SHA512: &str = "SHA-512";

/// The lowercase-hex SHA-256 of `content`, in the spelling the modelled `SHA256` shape
/// accepts (`^[0-9a-f]+$`, exactly 64 characters). Private: a handler reads an asset's
/// digest through [`Asset::sha256_hex`], which is the stored one.
fn sha256_hex(content: &[u8]) -> String {
    use sha2::Digest as _;

    hex::encode(sha2::Sha256::digest(content))
}

/// The cursor scope of `ListPackages` for one repository. Scoped PER REPOSITORY, so a
/// cursor minted while listing one repository's packages cannot resolve as a key in
/// another repository's listing (or in a domain or repository listing).
pub fn package_cursor_scope(domain: &str, repository: &str) -> String {
    format!("packages/{domain}/{repository}")
}

/// The cursor scope of `ListPackageVersionAssets` for one package version. Scoped per
/// repository AND per package-version coordinate ([`package_key`] carries the format and
/// the namespace), so a cursor minted while listing one version's assets cannot resolve as
/// an asset name in another version's listing, nor as a key in any other listing.
pub fn asset_cursor_scope(
    domain: &str,
    repository: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> String {
    format!(
        "assets/{domain}/{repository}/{}/{version}",
        package_key(format, namespace, package)
    )
}

/// The cursor scope of `ListPackageVersions` for one package, in one ordering. Scoped per
/// repository AND per package coordinate ([`package_key`] carries the format and the
/// namespace) AND per `sortBy`, because the cursor names a place in the ORDER the page was
/// built in: a cursor minted while listing in publish-time order must not resolve as a key
/// in the stored version order, nor in another package's or repository's listing.
pub fn package_version_cursor_scope(
    domain: &str,
    repository: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
    sort_by: Option<&str>,
) -> String {
    format!(
        "package-versions/{domain}/{repository}/{}/{}",
        package_key(format, namespace, package),
        sort_by.unwrap_or("STORED_ORDER")
    )
}

/// The cursor scope of `ListAllowedRepositoriesForGroup` for one package group's
/// restriction of one type. Scoped per group AND per restriction type, because each
/// restriction carries its OWN allowed-repository list ([`OriginRestriction`]): a cursor
/// minted while listing one restriction's list must not resolve as a key in another's.
pub fn allowed_repository_cursor_scope(
    domain: &str,
    package_group: &str,
    restriction_type: &str,
) -> String {
    format!("allowed-repositories/{domain}/{restriction_type}/{package_group}")
}

// ---------------------------------------------------------------------------
// Resource policies
//
// A domain and a repository each hold at most one resource policy ([`Domain::policy`] /
// [`Repository::policy`]), and the whole `Put*PermissionsPolicy` /
// `Get*PermissionsPolicy` / `Delete*PermissionsPolicy` family reads and writes exactly
// that field. The revision rules below therefore live here rather than once per handler,
// so a revision minted by one operation is the revision the next one accepts.
// ---------------------------------------------------------------------------

/// The `ResourceType` enum member naming a domain, reported as `resourceType` beside
/// `resourceId` on a domain-scoped policy failure and named in the messages below.
/// `ResourceType` models exactly `domain`, `repository`, `package`, `package-version` and
/// `asset` (`smithy_query(query="shape",shape="ResourceType")`).
pub const DOMAIN_RESOURCE_TYPE: &str = "domain";

/// The not-found message a resource-policy read reports when the resource it resolved holds
/// no policy: the kinds of resource that address the policy, and the names the request
/// addressed them by, each in one parenthesised, comma-separated list.
///
/// A policy hangs off a resource that takes ONE name on a domain (`(domain): (<domain>)`)
/// and TWO on a repository, which is addressed inside its domain
/// (`(domain, repo): (<domain>, <repository>)`) — both recorded. Shared by every policy
/// read so two of them cannot report an unset policy differently.
pub fn no_policy_message(resource_kinds: &[&str], resources: &[&str]) -> String {
    format!(
        "No policy found for ({}): ({})",
        resource_kinds.join(", "),
        resources.join(", ")
    )
}

/// The resource kind the recorded no-policy message names a repository by — the word AWS
/// uses in that message, which is NOT the `ResourceType` enum member
/// [`REPOSITORY_RESOURCE_TYPE`] reported beside it as `resourceType`.
pub const REPOSITORY_POLICY_RESOURCE_KIND: &str = "repo";

/// The conflict message a resource-policy write or delete reports when the `policyRevision`
/// the caller supplied is not the revision currently attached — the optimistic-locking
/// refusal [`policy_revision_matches`] decides.
///
/// ONE sentence, naming no resource: the recorded refusal says only that the revision the
/// caller held is not the one the service holds. Shared by the domain and the repository
/// families, and by the write and the delete, so no two of them word the same refusal
/// differently.
pub const POLICY_REVISION_CONFLICT_MESSAGE: &str = "Provided policy version conflicts with the server";

/// The refusal a resource-policy write owes a `policyDocument` that is not a policy, or
/// `None` when the document is one.
///
/// `policyDocument` is documented as "A valid displayable JSON Aspen policy string" and
/// the modelled pattern (`^[\P{C}\s]+$`) only forbids control characters, so JSON
/// well-formedness is the service's own rule — checked here, once, for every resource a
/// policy can hang off, so the modelled `reason` cannot differ between the domain and the
/// repository families. The document is parsed ONLY to check it: its structure is never
/// rewritten, and the only thing dropped on the way into the store is the whitespace
/// between its tokens ([`stored_policy_document`]).
///
/// Every document the policy parser cannot read as a policy is refused with the SAME
/// sentence ([`INVALID_POLICY_DOCUMENT_MESSAGE`]): what the parser choked on is the
/// service's own business, so the refusal states neither the position it stopped at nor
/// the JSON type the document turned out to be. The modelled `reason` is the one thing
/// that separates the two ways a document can fail to be a policy: a value no parser
/// could read at all (`CANNOT_PARSE`) against one that read as JSON but is not a policy
/// object, which is the submitted FIELD failing a rule of this service.
pub fn policy_document_refusal(document: &str) -> Option<ValidationException> {
    match serde_json::from_str::<serde_json::Value>(document) {
        Ok(serde_json::Value::Object(_)) => None,
        Ok(_) => Some(ValidationException {
            message: INVALID_POLICY_DOCUMENT_MESSAGE.to_string(),
            reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
        }),
        Err(_) => Some(ValidationException {
            message: INVALID_POLICY_DOCUMENT_MESSAGE.to_string(),
            reason: Some(VALIDATION_REASON_CANNOT_PARSE.to_string()),
        }),
    }
}

/// The `ValidationException` message a resource-policy write states for a `policyDocument`
/// that is not a policy document.
///
/// ENTIRELY fixed text: it names neither the member it was read from nor anything about
/// the document, so the sentence is the same for every document the policy parser rejects
/// and leaks nothing about how the document is parsed here.
pub const INVALID_POLICY_DOCUMENT_MESSAGE: &str = "Policy document isn't a valid policy document";

/// The `ValidationExceptionReason` a request member carries when its value could not be
/// read at all — the other half of [`VALIDATION_REASON_FIELD_FAILED`], which is a value
/// that was read and then failed a rule. (`ValidationExceptionReason` models exactly
/// `CANNOT_PARSE`, `ENCRYPTION_KEY_ERROR`, `FIELD_VALIDATION_FAILED`, `OTHER` and
/// `UNKNOWN_OPERATION`; see `VALIDATION_EXCEPTION_REASON_VALUES` in `ops::types`.)
pub const VALIDATION_REASON_CANNOT_PARSE: &str = "CANNOT_PARSE";

/// Whether a caller-supplied `policyRevision` authorises writing over the policy a
/// resource currently holds (`current`).
///
/// `policyRevision` is modelled as optional and documented as optimistic locking ("This
/// revision is used for optimistic locking, which prevents others from overwriting your
/// changes"), so: a caller that supplies one is asserting which revision it read, and the
/// write proceeds only while that is still the revision in the store; a caller that
/// supplies none is locking nothing and the write proceeds unconditionally. A resource
/// holding no policy carries no revision, so a supplied revision can never match it.
pub fn policy_revision_matches(current: Option<&ResourcePolicy>, supplied: Option<&str>) -> bool {
    match supplied {
        None => true,
        Some(supplied) => current.and_then(|policy| policy.revision.as_deref()) == Some(supplied),
    }
}

/// The policy to store on the resource `resource_arn` names: `document` in the form a
/// policy is stored in ([`stored_policy_document`]), under the revision that document's
/// own bytes mint ([`policy_revision`]).
///
/// The revision is derived from the STORED form, so the token a write mints identifies the
/// document a later read answers with and not a rendering of it nobody can observe.
pub fn attached_policy(resource_arn: &str, document: String) -> ResourcePolicy {
    let document = stored_policy_document(&document);
    ResourcePolicy {
        revision: Some(policy_revision(&document)),
        resource_arn: Some(resource_arn.to_string()),
        document: Some(document),
    }
}

/// The form a submitted `policyDocument` is STORED and reported in: CodeArtifact's own
/// rendering of the policy the caller wrote.
///
/// A resource policy is an Aspen (IAM) policy document, and CodeArtifact reports it as it
/// renders it, not as the request spelled it. Two differences are what the recorded
/// `Put`/`Get`/`Delete` responses of both the domain and the repository policy show:
///
/// 1. the whitespace BETWEEN the document's JSON tokens is gone — no space after a `:` or
///    a `,`, no indentation, whatever spacing the request used; and
/// 2. an Aspen element whose value the policy language types as "String or Array of
///    strings" ([`SINGLE_ELEMENT_COLLAPSING_ELEMENTS`]) is rendered as the STRING when the
///    caller submitted a one-element array of strings — a one-action statement comes back
///    with a scalar `Action`.
///
/// Everything else about the document is the caller's data and survives untouched: member
/// order, statement count, the `Statement` array itself (a one-statement policy is reported
/// as a one-element array, never as a bare object), the type and spelling of every other
/// value, and every byte inside a string literal — whitespace, escapes and non-ASCII
/// characters included. That is why this walks the document's own bytes rather than
/// re-serialising a parsed value, which would also rewrite the member order, the escapes
/// and the number spellings the caller chose.
///
/// Both rewrites are one pass over the document's TOKENS, so a member name is only ever
/// read where a name can appear (a string followed by `:`) and the contents of a string
/// literal — which may hold anything, braces, quotes and whitespace included — are copied
/// through whole. The `minifier` crate's `json::minify` is deliberately NOT used for the
/// first rewrite: as of 0.4.0 it stops minifying at the first escaped quote, so
/// `{ "a" : "x\"y" , "b" : 1 }` comes back still spaced from `"b"` on, and an escaped quote
/// is legal in any `Sid` or condition value.
///
/// Applied at the ONE place a policy is attached ([`attached_policy`]), so the domain and
/// the repository families cannot store a policy in two different forms, and the write,
/// the read and the delete of one policy all report the same bytes.
///
/// A document that is not valid JSON never reaches here: it is refused by
/// [`policy_document_refusal`] before anything is stored.
pub fn stored_policy_document(document: &str) -> String {
    let bytes = document.as_bytes();
    let mut out = String::with_capacity(bytes.len());
    // The Aspen element whose value starts at the current position: set by a string token
    // that a `:` follows, and dropped as soon as that value is reached or anything else
    // intervenes.
    let mut element: Option<&str> = None;
    let mut at = 0;
    while at < bytes.len() {
        match bytes[at] {
            // Whitespace between tokens is dropped — and only between tokens: whitespace
            // inside a string literal is part of the literal, which the arm below copies
            // whole. It does not end an element name's reach, so `"Action" : [ "x" ]` is
            // still the `Action` element.
            b' ' | b'\t' | b'\n' | b'\r' => at += 1,
            b'"' => {
                let end = string_token_end(bytes, at);
                out.push_str(&document[at..end]);
                // A string is an element NAME exactly when a `:` follows it — possibly
                // across whitespace, which is why the scan skips it here too. A literal
                // that never closed (only reachable for a document that is not valid JSON,
                // which never gets this far) names nothing.
                let closed = end > at + 1 && bytes[end - 1] == b'"';
                let after = skip_json_whitespace(bytes, end);
                element = (closed && bytes.get(after) == Some(&b':'))
                    .then(|| &document[at + 1..end - 1]);
                at = end;
            }
            b'[' if element
                .is_some_and(|name| SINGLE_ELEMENT_COLLAPSING_ELEMENTS.contains(&name)) =>
            {
                // A one-element array of one string — `["value"]`, however it is spaced —
                // is rendered as that string. Anything else (no element, several, or one
                // that is not a string) is left to the loop, which copies it as it is.
                let first = skip_json_whitespace(bytes, at + 1);
                let single = (bytes.get(first) == Some(&b'"'))
                    .then(|| string_token_end(bytes, first))
                    .map(|end| (end, skip_json_whitespace(bytes, end)))
                    .filter(|(_, close)| bytes.get(*close) == Some(&b']'));
                match single {
                    Some((end, close)) => {
                        out.push_str(&document[first..end]);
                        at = close + 1;
                    }
                    None => {
                        out.push('[');
                        at += 1;
                    }
                }
                element = None;
            }
            _ => {
                // Every other token is copied byte for byte. The run stops at the bytes the
                // arms above own, each of which is ASCII, so it always ends on a character
                // boundary even when a non-ASCII byte sequence sits inside it.
                let start = at;
                at += 1;
                while at < bytes.len()
                    && !matches!(
                        bytes[at],
                        b'"' | b'[' | b':' | b' ' | b'\t' | b'\n' | b'\r'
                    )
                {
                    at += 1;
                }
                let token = &document[start..at];
                out.push_str(token);
                // `:` is what introduces an element's value; every other token ends the
                // name's reach.
                if token != ":" {
                    element = None;
                }
            }
        }
    }
    out
}

/// The Aspen policy elements whose value the policy language types as "String or Array of
/// strings", so that a one-element array and the bare string say the same thing: the
/// statement's `Action`/`NotAction` and `Resource`/`NotResource`, and the principal lists
/// inside `Principal`/`NotPrincipal` (`AWS`, `Service`, `Federated`, `CanonicalUser`).
/// CodeArtifact renders every one of them in the string form when the array holds exactly
/// one string, which the recorded repository policy shows for `Action`.
///
/// `Statement` is deliberately NOT in this list: the recorded documents keep a
/// single-statement policy as a one-element array, so the array form is what a stored
/// policy reports there.
const SINGLE_ELEMENT_COLLAPSING_ELEMENTS: &[&str] = &[
    "Action",
    "NotAction",
    "Resource",
    "NotResource",
    "AWS",
    "Service",
    "Federated",
    "CanonicalUser",
];

/// The index of the first byte from `start` on that is not JSON whitespace — the four
/// characters JSON allows between tokens (space, tab, carriage return, line feed).
fn skip_json_whitespace(bytes: &[u8], start: usize) -> usize {
    let mut at = start;
    while matches!(bytes.get(at), Some(b' ' | b'\t' | b'\n' | b'\r')) {
        at += 1;
    }
    at
}

/// The index just past the JSON string literal that starts at `start` (which must be its
/// opening `"`), honouring `\` escapes so an escaped quote does not end the literal.
///
/// A policy document is parsed before it is stored ([`policy_document_refusal`]), so its
/// literals are well-formed; an unterminated one would run to the end of the document,
/// which this reports as the whole remaining input rather than panicking.
fn string_token_end(bytes: &[u8], start: usize) -> usize {
    let mut at = start + 1;
    while at < bytes.len() {
        match bytes[at] {
            b'\\' => at += 2,
            b'"' => return at + 1,
            _ => at += 1,
        }
    }
    bytes.len()
}

/// The `ResourcePolicy` a policy delete answers with for the resource `resource_arn`
/// names: `removed` is the record the resource held, or `None` when it held no policy.
///
/// Removing a policy that is not there is the delete's goal already met, not a failure, so
/// the answer is the bare `ResourcePolicy` ENVELOPE — `resourceArn` identifying the
/// resource the request targeted, with no `revision` and no `document`, both of which the
/// model leaves optional. Shared by the domain and the repository policy delete so the two
/// cannot report "there was nothing to delete" differently.
pub fn deleted_policy(resource_arn: &str, removed: Option<ResourcePolicy>) -> ResourcePolicy {
    removed.unwrap_or_else(|| ResourcePolicy {
        resource_arn: Some(resource_arn.to_string()),
        revision: None,
        document: None,
    })
}

/// The revision a resource policy carries once it is written: the DIGEST of the document
/// stored under it — base64 (standard alphabet) of its SHA-256, which is 44 characters,
/// inside `PolicyRevision`'s modelled 1..100 length and matching its `^\S+$` pattern.
///
/// A revision identifies the DOCUMENT, not the write: the token is a function of the
/// stored bytes alone, so it is stable across every read of the attached policy (nothing
/// per-call and nothing counted goes into it), two writes of the same document report the
/// same token, and a write of other bytes reports another one. That is what makes it
/// usable as the `policyRevision` precondition: a revision a caller holds matches exactly
/// while the document it read is still the document attached, which is the question
/// optimistic locking asks.
///
/// Digested over the document's own bytes, with nothing prepended or appended, so the
/// token reported for a document is the same on whichever resource it is attached to.
fn policy_revision(document: &str) -> String {
    use base64::Engine as _;
    use sha2::Digest as _;

    base64::engine::general_purpose::STANDARD.encode(sha2::Sha256::digest(document.as_bytes()))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsCodeartifactCodeartifactcontrolplaneserviceState>)
}

/// The region this request is addressed to: the region field of its SigV4 credential
/// scope, or [`DEFAULT_REGION`] when the request carries none.
///
/// Every ARN this service mints is regional, and the region is a property of the
/// REQUEST (the endpoint the client signed for), not of the stored record — so it is
/// read here rather than guessed.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The ARN of a domain: `arn:aws:codeartifact:<region>:<owner>:domain/<name>`.
pub fn domain_arn(region: &str, domain: &str) -> String {
    ::engine::emulator_arn(SERVICE_SIGNING_NAME, region, &format!("domain/{domain}"))
}

/// The ARN of a repository:
/// `arn:aws:codeartifact:<region>:<owner>:repository/<domain>/<repository>`.
pub fn repository_arn(region: &str, domain: &str, repository: &str) -> String {
    ::engine::emulator_arn(
        SERVICE_SIGNING_NAME,
        region,
        &format!("repository/{domain}/{repository}"),
    )
}

/// The ARN of a PACKAGE inside a repository:
/// `arn:aws:codeartifact:<region>:<owner>:package/<domain>/<repository>/<format>/<namespace>/<name>`.
///
/// The package resource type as the IAM Service Authorization Reference spells it for
/// CodeArtifact — the ARN a statement names one package by, and therefore both the resource a
/// package-scoped action is judged against and the resource a package-version event announces
/// ([`package_version_events::VersionChange::package_arn`]). The namespace is a SEGMENT of
/// the path, so a format whose packages carry none leaves it empty rather than dropping it:
/// the path is positional, and the empty segment is what distinguishes "no namespace" from a
/// namespace of that name.
pub fn package_arn(
    region: &str,
    domain: &str,
    repository: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
) -> String {
    ::engine::emulator_arn(
        SERVICE_SIGNING_NAME,
        region,
        &format!(
            "{PACKAGE_RESOURCE_TYPE}/{domain}/{repository}/{format}/{}/{package}",
            namespace.unwrap_or_default()
        ),
    )
}

// ---------------------------------------------------------------------------
// Refusal messages
//
// A CodeArtifact refusal that names a resource IDENTIFIES it: the resource kind, its
// name quoted, the parent domain when the resource is nested in one, and the 12-digit
// account that owns the domain. The account is the point — CodeArtifact domains are
// shared across accounts through `domainOwner`, so a caller has to be able to tell
// "missing in my account" from "missing in the owner's account" from the message alone.
//
// They live here, beside the store the handlers resolve against, so every operation that
// refuses the same way words it the same way: without this, each handler spells its own
// sentence and the implemented ones drift from each other.
// ---------------------------------------------------------------------------

/// The account a request addressed a domain to: the `domainOwner` it pinned the domain
/// to, or — when the request named none — the account this emulator serves, which owns
/// every domain it holds.
///
/// This is the account a refusal reports, including a refusal for a domain that is NOT
/// held: it is the account the caller asked about, which is what makes a cross-account
/// miss distinguishable from a miss in the caller's own account.
pub fn addressed_domain_owner(domain_owner: Option<&str>) -> &str {
    match domain_owner {
        Some(owner) => owner,
        None => ::engine::emulator_account_id(),
    }
}

/// The `ResourceNotFoundException` message for a domain the store does not hold for this
/// caller.
pub fn domain_not_found_message(domain: &str, domain_owner: Option<&str>) -> String {
    format!(
        "Domain not found. Domain '{domain}' owned by account '{}' does not exist.",
        addressed_domain_owner(domain_owner)
    )
}

/// The `ResourceNotFoundException` message for a repository the addressed domain does not
/// hold — whether the caller named it directly or as another repository's upstream.
pub fn repository_not_found_message(
    domain: &str,
    domain_owner: Option<&str>,
    repository: &str,
) -> String {
    format!(
        "Repository not found. Repository '{repository}' in domain '{domain}' owned by \
         account '{}' does not exist.",
        addressed_domain_owner(domain_owner)
    )
}

/// The `ConflictException` message for a create whose domain name is already held.
///
/// `owner` is the account that owns the COLLIDING domain — read off the stored record,
/// not off the request, because that is the fact the caller cannot otherwise see.
pub fn domain_already_exists_message(domain: &str, owner: &str) -> String {
    format!("Domain '{domain}' owned by '{owner}' already exists.")
}

/// The `ConflictException` message for a create whose repository name is already held in
/// the addressed domain.
///
/// The sentence names the two things a repository name is unique WITHIN — the name itself
/// and the domain holding it — unquoted, and nothing else: a repository name is taken per
/// domain, so the account owning that domain is not part of what the conflict reports.
pub fn repository_already_exists_message(domain: &str, repository: &str) -> String {
    format!("Repository {repository} already exists in domain {domain}.")
}

/// The `ConflictException` message for `DeleteDomain` on a domain that still holds
/// repositories.
///
/// This refusal states the PRECONDITION and interpolates nothing: it is the same sentence
/// whatever domain was addressed and however many repositories remain, so it reports no
/// count of a domain the caller may not be able to list.
pub const DOMAIN_NOT_EMPTY_MESSAGE: &str = "Domain must contain 0 repositories before deletion";

/// The `ResourceNotFoundException` message for an `externalConnection` that is not a
/// registry this service offers ([`EXTERNAL_CONNECTIONS`]).
///
/// The resource KIND, the identifier the request supplied, and the statement that it was
/// not found — nothing more. The identifier is the connection name AND NOTHING ELSE: the
/// registries this service can connect to are a service-wide catalog addressed by name
/// alone ([`EXTERNAL_CONNECTIONS`]), not a per-repository collection, so the thing that was
/// looked for and not found is identified without naming a repository, a domain or the
/// account owning one — unlike a repository, which exists only inside a domain
/// ([`repository_not_found_message`]), or a domain, which exists only inside an account
/// ([`domain_not_found_message`]), both of whose sentences do name what holds them. No part
/// of this sentence is read off the store: a caller gets back only what it sent.
pub fn external_connection_not_found_message(external_connection: &str) -> String {
    format!("External connection '{external_connection}' not found.")
}

/// The `ResourceNotFoundException` message for a package group the addressed domain does
/// not hold, as CodeArtifact itself words it — the resource kind capitalised, its "not
/// found" clause doubled, the group's identifying PATTERN single-quoted, the domain and the
/// account bare, and no sentence-final period.
///
/// Stated by every package-group operation that resolves one, so the sentence a caller
/// matches is the same whichever of them missed.
pub fn package_group_not_found_message(
    domain: &str,
    domain_owner: Option<&str>,
    package_group: &str,
) -> String {
    format!(
        "Package Group not found not found. Package Group '{package_group}' in domain \
         {domain} owned by account {} does not exist",
        addressed_domain_owner(domain_owner)
    )
}

/// The `ResourceNotFoundException` message every VERSION-addressing operation states for a
/// COORDINATE the addressed repository holds no version of.
///
/// A package version is addressed by its whole `(format, namespace, package, version)`
/// coordinate and a package exists in a repository only through its versions, so this is
/// the sentence for "nothing is published at this coordinate": it names the format, the
/// package, its namespace and the repository the version was looked for in. The namespace
/// clause appears only where the request carried one, since a format whose versions carry
/// no namespace component has none to name.
///
/// Stated by every operation that addresses a package's VERSIONS — whether it addresses one
/// version (`DescribePackageVersion`, `GetPackageVersionAsset`,
/// `ListPackageVersionAssets`, `ListPackageVersionDependencies`), a batch of them
/// (`CopyPackageVersions`, `DeletePackageVersions`, `DisposePackageVersions`,
/// `UpdatePackageVersionsStatus`) or all of them (`ListPackageVersions`) — because each of
/// them was asked about versions IN one repository, and the repository is what distinguishes
/// "absent from this repository" from "absent from the domain" when several hold the same
/// package name. Distinct from [`package_not_found_message`], the sentence the
/// PACKAGE-scoped operations state for a package name the repository does not hold at all.
pub fn package_coordinate_not_found_message(
    format: &str,
    namespace: Option<&str>,
    package: &str,
    repository: &str,
) -> String {
    match namespace {
        Some(namespace) => format!(
            "The {format} package '{package}' with namespace '{namespace}' does not exist in \
             repository '{repository}'."
        ),
        None => format!(
            "The {format} package '{package}' does not exist in repository '{repository}'."
        ),
    }
}

/// The `resourceId` an error reports for a PACKAGE VERSION: its fully-qualified identity
/// inside the repository, `<namespace>/<package>/<version>`.
///
/// A version string alone does not identify anything — two conflicts on different packages
/// would report the same `1.0.0` — so the id carries the whole coordinate a client would
/// have to feed back to address this version. The namespace segment is present exactly
/// when the request carried one; no component can contain `/` (`PackageNamespace`,
/// `PackageName` and `PackageVersion` are all modelled `^[^#/\s]+$`), so the identity
/// parses back unambiguously.
#[must_use]
pub fn package_version_resource_id(
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> String {
    match namespace {
        Some(namespace) => format!("{namespace}/{package}/{version}"),
        None => format!("{package}/{version}"),
    }
}

/// The `ResourceNotFoundException` message the PACKAGE-scoped operations state for a
/// package the addressed repository does not hold.
///
/// The sentence of the operations whose subject is the PACKAGE itself (`DescribePackage`),
/// as opposed to the version-addressing ones, which state
/// [`package_coordinate_not_found_message`].
///
/// A package is addressed by its whole `(format, namespace, name)` triple, so the refusal
/// states that whole triple: a caller that addressed the right name under the wrong format
/// — or without the namespace the package carries — addressed a DIFFERENT package, and a
/// sentence naming only the name could not tell it which of the three was wrong. The
/// namespace clause appears exactly where the request carried one, since a format whose
/// packages carry no namespace component has none to name.
pub fn package_not_found_message(format: &str, namespace: Option<&str>, package: &str) -> String {
    match namespace {
        Some(namespace) => format!(
            "Package not found in repository. Package = {{format={format}, \
             namespace={namespace}, name={package}}}"
        ),
        None => {
            format!("Package not found in repository. Package = {{format={format}, name={package}}}")
        }
    }
}

/// The `ResourceNotFoundException` message for a version the addressed package does not
/// hold, and the `errorMessage` of a `NOT_FOUND` entry in a per-version answer map.
///
/// A version-absent refusal reports TWO things: the repository it looked in, by the
/// internal id this service knows that repository by ([`repository_id`]) rather than by the
/// name the request used, and the whole coordinate it resolved, rendered as the
/// `PackageVersionCoordinateEntity` of [`package_version_coordinate`]. The coordinate is
/// the part that tells a caller WHICH version was looked for — format and namespace
/// included — so a wrong-namespace or wrong-format lookup is distinguishable from a
/// genuinely absent version.
pub fn package_version_not_found_message(
    repository_id: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> String {
    format!(
        "Package version not found in repo. repoId = '{repository_id}', packageCoordinate = \
         '{}'.",
        package_version_coordinate(format, namespace, package, version)
    )
}

/// The whole coordinate a package VERSION was addressed by, as this service renders one
/// inside a refusal: `PackageVersionCoordinateEntity{name=…, type=…, namespace=…,
/// version=…, displayName=…}`.
///
/// The `type` is the `PackageFormat` upper-cased, and the `namespace` component is present
/// exactly where the coordinate carries one — a format whose versions have no namespace
/// component (the model's `PackageSummary$namespace` doc lists Python, NuGet, Ruby and
/// Cargo) has none to render, so the entity leaves it out rather than rendering an empty
/// one.
fn package_version_coordinate(
    format: &str,
    namespace: Option<&str>,
    package: &str,
    version: &str,
) -> String {
    let display_name = coordinate_display_name(namespace, package, version);
    let format = format.to_uppercase();
    match namespace {
        Some(namespace) => format!(
            "PackageVersionCoordinateEntity{{name={package}, type={format}, \
             namespace={namespace}, version={version}, displayName={display_name}}}"
        ),
        None => format!(
            "PackageVersionCoordinateEntity{{name={package}, type={format}, version={version}, \
             displayName={display_name}}}"
        ),
    }
}

/// The `displayName` of one package-version coordinate: the version written the way a
/// coordinate of its shape is written.
///
/// A coordinate that carries a namespace is written as the path
/// `<namespace>/<package>/<version>` — the same `/`-joined identity a refusal reports as
/// its `resourceId` ([`package_version_resource_id`]), which no component can contain a
/// separator of (`PackageNamespace`, `PackageName` and `PackageVersion` are all modelled
/// `^[^#/\s]+$`). A coordinate that carries none is written as the version-pinned form
/// `<package>==<version>`, which is how a package manager whose versions have no namespace
/// component writes one pinned version.
fn coordinate_display_name(namespace: Option<&str>, package: &str, version: &str) -> String {
    match namespace {
        Some(namespace) => format!("{namespace}/{package}/{version}"),
        None => format!("{package}=={version}"),
    }
}

/// The `ConflictException` message for a create whose package-group pattern is already
/// held in the addressed domain, as CodeArtifact itself words it — the resource kind
/// capitalised, the identifying PATTERN single-quoted, the domain and the owning account
/// bare, and no sentence-final period.
///
/// A package-group pattern is unique per domain, and a domain is shared across accounts
/// through `domainOwner`, so the sentence names the three things the collision is scoped
/// by: the pattern, the domain holding it and the account owning that domain.
pub fn package_group_already_exists_message(
    domain: &str,
    owner: &str,
    package_group: &str,
) -> String {
    format!("Package Group '{package_group}' already exists in domain {domain} owned by account {owner}")
}

/// The `ValidationException` message `DeletePackageGroup` states for the domain's ROOT
/// package group.
///
/// ENTIRELY fixed text, the root pattern included: the root group is the one group this
/// API cannot delete, and `/*` is that group's own identifier
/// ([`ROOT_PACKAGE_GROUP_PATTERN`]) rather than a value read off the request.
pub const ROOT_PACKAGE_GROUP_NOT_DELETABLE_MESSAGE: &str =
    "The root package group, '/*', cannot be deleted.";

/// The `ResourceNotFoundException` message an ASSET-addressing read states for an asset
/// name the addressed package version does not hold.
///
/// An asset is addressed by the package-version coordinate holding it plus its own name,
/// so the refusal reports both: the repository it looked in by the NAME the request
/// addressed it by, the whole coordinate it resolved rendered as the
/// `PackageVersionCoordinateEntity` of [`package_version_coordinate`] (format upper-cased
/// and namespace included), and the asset name that missed. The coordinate is the part
/// that tells a caller WHICH version was resolved, so a wrong-format or wrong-namespace
/// lookup is distinguishable from an asset the right version genuinely does not hold.
/// Distinct from [`package_version_not_found_message`], which is the sentence for a
/// coordinate holding no version at all.
pub fn asset_not_found_message(
    repository: &str,
    format: &str,
    namespace: Option<&str>,
    package: &str,
    version: &str,
    asset: &str,
) -> String {
    format!(
        "Asset not found in repo. RepositoryName = '{repository}', packageCoordinate = '{}', \
         assetName = '{asset}'.",
        package_version_coordinate(format, namespace, package, version)
    )
}

/// The `ValidationException` a caller-supplied `resourceArn` owes when it addresses no
/// resource of this service's namespace at all.
///
/// TWO sentences, because a client distinguishes two different mistakes and so does AWS:
///
/// * a value that is not written as an ARN — fewer than the six colon-separated fields an
///   ARN has ([`service_common::arn::has_arn_fields`]) — never reaches a service's own
///   namespace check, so it is refused by the ARN PARSER, in the parser's own sentence
///   about ARN SYNTAX; and
/// * a value that IS written as an ARN but names another vendor (or is otherwise not one
///   of this service's ARNs) is refused for naming nothing this API can address.
///
/// Collapsing the two would make a syntactically broken string and a well-formed ARN of
/// another service indistinguishable. Stated here, once, for every operation that takes a
/// caller-supplied `resourceArn` ([`resolve_taggable`] is the lookup they share), so the
/// three tagging operations cannot word the same refusal differently.
#[must_use]
pub fn unaddressable_arn_refusal(resource_arn: &str) -> ValidationException {
    let message = if ::service_common::arn::has_arn_fields(resource_arn) {
        format!("resourceArn {resource_arn} is not a CodeArtifact resource ARN.")
    } else {
        format!(
            "Invalid ARN: {resource_arn} can't be parsed because it doesn't have enough \
             separators."
        )
    };
    ValidationException {
        message,
        reason: Some(VALIDATION_REASON_CANNOT_PARSE.to_string()),
    }
}

/// The `ValidationExceptionReason` a refused member carries: the request reached the
/// service and one of its fields failed a rule the model cannot express.
pub const VALIDATION_REASON_FIELD_FAILED: &str = "FIELD_VALIDATION_FAILED";

/// The `ValidationException` message for ONE input member that failed a constraint this
/// service enforces itself.
///
/// AWS states every member-level constraint failure in one envelope — `N validation
/// error(s) detected: Value '<value>' at '<member path>' failed to satisfy constraint:
/// <requirement>` — and does not change envelope because the constraint happens to be one
/// the Smithy model cannot express: clients and operators parse that form to attribute a
/// failure to a specific member. The framework renders exactly this envelope for a
/// MODELLED constraint (`aws_smithy_schema_server::envelope` over its own `Violation`s,
/// which this service's `override_refusal` delegates to), but its `ViolationKind` is
/// `#[non_exhaustive]` and spells only the modelled requirements, so a service-enforced
/// requirement is composed here instead — in the one place every handler reaches for it,
/// so the two cannot word the same refusal differently.
pub fn constraint_violation_message(member: &str, value: &str, requirement: &str) -> String {
    format!(
        "1 validation error detected: Value '{value}' at '{member}' failed to satisfy \
         constraint: {requirement}"
    )
}

/// The requirement a `PackageGroupPattern` that is not a path from the domain root failed.
///
/// `PackageGroupPattern` is modelled with length bounds only (len 2..520), so the boundary
/// validator admits any string of that length; a package group's identifier is a PATH from
/// the domain root, which is what makes `/*` the ancestor of `/npm/*`, so the leading `/`
/// is a constraint this service enforces.
const PACKAGE_GROUP_PATTERN_REQUIREMENT: &str = "Pattern must start with '/'.";

/// The refusal a caller-supplied `packageGroup` owes when it is not a path from the domain
/// root, or `None` when it is one.
///
/// Decided here rather than per handler: every operation that takes a `packageGroup`
/// accepts the same identifier, so a value no package group can ever have is the same bad
/// REQUEST at each of them — never a lookup that happens to miss.
pub fn package_group_pattern_refusal(pattern: &str) -> Option<ValidationException> {
    if pattern.starts_with('/') {
        return None;
    }
    Some(ValidationException {
        message: constraint_violation_message(
            "packageGroup",
            pattern,
            PACKAGE_GROUP_PATTERN_REQUIREMENT,
        ),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
    })
}

/// The refusal a caller-supplied package-version status owes when it is not a status a
/// request can carry, or `None` when it is one.
///
/// The boundary validator admits the whole `PackageVersionStatus` enum, because the enum
/// is also what responses report; a value outside
/// [`INPUT_PACKAGE_VERSION_STATUSES`] names a state no stored version is ever in, so a
/// request supplying it is a bad REQUEST — never a filter that matches nothing or a
/// precondition that fails per version. Decided here rather than per handler: every
/// operation that takes a status as input takes the same set.
pub fn input_package_version_status_refusal(status: &str) -> Option<ValidationException> {
    if INPUT_PACKAGE_VERSION_STATUSES.contains(&status) {
        return None;
    }
    Some(ValidationException {
        // The value as it ARRIVED, in the model's own casing, ahead of the fixed text.
        message: format!("{status} is not a valid input status"),
        reason: Some(VALIDATION_REASON_FIELD_FAILED.to_string()),
    })
}

/// The characters an ARN resource path carries LITERALLY: RFC 3986's unreserved set
/// (`ALPHA / DIGIT / "-" / "." / "_" / "~"`), which is also the set AWS's own URI
/// encoder leaves alone. Every other byte of an identifier placed in a resource path is
/// percent-encoded, so nothing a caller chose can be read as an ARN delimiter, a URI
/// delimiter or an IAM wildcard.
///
/// `/` is NOT in the set because it is handled a level up, in
/// [`encode_arn_resource_path`]: it is the path's own separator, not a character of a
/// segment.
const ARN_PATH_SEGMENT: &::percent_encoding::AsciiSet = &::percent_encoding::NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// One segment of an ARN resource path, percent-encoded.
///
/// The escapes are written with LOWER-case hex digits (`%2a`, `%24`), the form AWS's
/// CodeArtifact ARNs carry; `percent-encoding` writes them upper-case, so the two digits
/// of each escape — and only those — are lowered on the way out. Nothing else in the
/// encoder's output can follow a `%`: every `%` in it begins an escape, because a literal
/// `%` of the input is itself encoded (`%25`).
fn encode_arn_path_segment(segment: &str) -> String {
    let mut encoded = String::with_capacity(segment.len());
    let mut hex_digits_left = 0_u8;
    for character in ::percent_encoding::utf8_percent_encode(segment, ARN_PATH_SEGMENT)
        .flat_map(str::chars)
    {
        if character == '%' {
            hex_digits_left = 2;
            encoded.push(character);
        } else if hex_digits_left > 0 {
            hex_digits_left -= 1;
            encoded.push(character.to_ascii_lowercase());
        } else {
            encoded.push(character);
        }
    }
    encoded
}

/// A `/`-separated ARN resource path with every SEGMENT percent-encoded
/// ([`encode_arn_path_segment`]) and the separators kept literal.
///
/// An ARN's resource field is a path, so `/` has to stay readable as the separator it is
/// — an ARN parser and an IAM resource element both split on it. Everything inside a
/// segment is an identifier the caller chose, and is encoded.
#[must_use]
pub fn encode_arn_resource_path(path: &str) -> String {
    path.split('/')
        .map(encode_arn_path_segment)
        .collect::<Vec<String>>()
        .join("/")
}

/// The identifiers an ARN resource path carries, with the percent-encoding
/// [`encode_arn_resource_path`] applied removed again, or `None` when the escapes do not
/// decode to UTF-8.
///
/// The inverse of minting: a client carries an ARN this service minted back into the API
/// (`TagResource`, `ListTagsForResource`), so the resource path has to resolve to the
/// identifier it was built from. Decoding is the identity on a path that carries no
/// escape, so an ARN written out by hand with literal characters resolves to the same
/// resource.
#[must_use]
pub fn decode_arn_resource_path(path: &str) -> Option<String> {
    ::percent_encoding::percent_decode_str(path)
        .decode_utf8()
        .ok()
        .map(|decoded| decoded.into_owned())
}

/// The ARN of a package group:
/// `arn:aws:codeartifact:<region>:<owner>:package-group/<domain><pattern>`.
///
/// The pattern is a path that already begins with `/`, so it joins the domain segment
/// directly — the form CodeArtifact's `package-group` resource ARN takes. A pattern
/// handed in without the leading `/` gets one here, so the ARN always has exactly one
/// separator between the domain and the pattern.
///
/// The pattern is the one identifier of this service that is not restricted to characters
/// an ARN carries literally — `PackageGroupPattern` admits every non-control,
/// non-whitespace character, wildcards and sigils included — so it goes through
/// [`encode_arn_resource_path`]. (The domain name is passed through the same encoder for
/// uniformity; `DomainName` is `^[a-z][a-z0-9\-]{0,48}[a-z0-9]$`, which the encoder leaves
/// untouched.)
pub fn package_group_arn(region: &str, domain: &str, pattern: &str) -> String {
    let path = pattern.strip_prefix('/').unwrap_or(pattern);
    ::engine::emulator_arn(
        SERVICE_SIGNING_NAME,
        region,
        &format!(
            "package-group/{}",
            encode_arn_resource_path(&format!("{domain}/{path}"))
        ),
    )
}

/// The `EndpointType` a request that names none is served: AWS documents the IPv4
/// endpoint as a repository's default, with `dualstack` the opt-in alternative
/// (`EndpointType` models exactly `dualstack | ipv4`).
pub const DEFAULT_ENDPOINT_TYPE: &str = "ipv4";

/// The `EndpointType` value that selects the dual-stack (IPv4/IPv6) host.
pub const ENDPOINT_TYPE_DUALSTACK: &str = "dualstack";

/// The host AWS publishes a repository endpoint at:
/// `<domain>-<domainOwner>.d.codeartifact.<region>.<suffix>`, with `amazonaws.com` for the
/// IPv4 endpoint and `on.aws` for the dual-stack one, which is what `endpointType` selects
/// (AWS CodeArtifact user guide, e.g. the npm endpoint
/// `https://my_domain-111122223333.d.codeartifact.us-west-2.amazonaws.com/npm/my_repo/`).
///
/// It is built entirely from the repository's identity — the domain and its owning account
/// name the first label, the region names the regional endpoint the request was addressed
/// to. This emulator cannot ANSWER on that name, so the endpoint it announces carries it in
/// the path instead ([`crate::service::repository_data_plane::announced_repository_endpoint`]),
/// and the data plane resolves it there and as the request's own host.
pub fn published_endpoint_host(
    region: &str,
    domain: &str,
    domain_owner: &str,
    endpoint_type: &str,
) -> String {
    let suffix = if endpoint_type == ENDPOINT_TYPE_DUALSTACK {
        "on.aws"
    } else {
        "amazonaws.com"
    };
    format!("{domain}-{domain_owner}.d.codeartifact.{region}.{suffix}")
}

/// The two path segments a repository's endpoint for one package format is served under,
/// with the trailing slash the documented endpoint carries: `/<format>/<repository>/`
/// (AWS CodeArtifact user guide, e.g. the npm endpoint `…/npm/my_repo/`).
pub fn repository_endpoint_path(format: &str, repository: &str) -> String {
    format!("/{format}/{repository}/")
}

/// The ARN of the S3 bucket a domain stores its package assets in: the asset bucket of
/// the account that OWNS the domain, in the Region the domain lives in.
///
/// CodeArtifact keeps those assets in one bucket per account and Region, named
/// `assets-<domain-owner>-<region>`, so the ARN carries exactly those two suffix
/// components and nothing per-domain or per-call: every domain an account owns in one
/// Region reports the same bucket, and the value a domain reports is the same string for
/// its whole lifetime. An S3 ARN carries NO region or account segment of its own, so this
/// one cannot go through `engine::emulator_arn` (which always writes both) and is
/// formatted here instead; the partition is the same `aws` that helper uses, so every ARN
/// this service emits agrees on it.
pub fn asset_bucket_arn(region: &str, domain_owner: &str) -> String {
    format!("arn:aws:s3:::assets-{domain_owner}-{region}")
}

/// The internal id this service knows one repository by — the `repoId` a refusal that
/// could not resolve a package version reports.
///
/// Not a modelled member: no operation returns it, and nothing addresses a repository by
/// it; it is the id the service's own refusals name a repository by, beside the name the
/// request used. Derived from the repository's identity — the account owning its domain,
/// the domain, and the repository name — so the same repository reports the same id on
/// every read, two repositories never report one id, and no refusal carries a per-call
/// random value.
pub fn repository_id(domain_owner: &str, domain: &str, repository: &str) -> String {
    opaque_id(&identity_digest(&[
        "repository-id",
        domain_owner,
        domain,
        repository,
    ]))
}

/// An opaque resource id derived from `digest`: its first 16 bytes written in the
/// 8-4-4-4-12 hex grouping AWS writes an internal resource id in.
fn opaque_id(digest: &[u8; 32]) -> String {
    let hex = hex::encode(&digest[..16]);
    format!(
        "{}-{}-{}-{}-{}",
        &hex[0..8],
        &hex[8..12],
        &hex[12..16],
        &hex[16..20],
        &hex[20..32]
    )
}

/// A stable 32-byte digest over a resource's identifying fields.
///
/// Each field is terminated with a NUL so two different field splits (`["ab", "c"]` vs
/// `["a", "bc"]`) cannot digest to the same value.
fn identity_digest(fields: &[&str]) -> [u8; 32] {
    use sha2::Digest;

    let mut hasher = sha2::Sha256::new();
    for field in fields {
        hasher.update(field.as_bytes());
        hasher.update([0u8]);
    }
    hasher.finalize().into()
}

/// The marker every pagination cursor this service mints carries, so a token the service
/// did not issue (or one minted by an older cursor format) is REFUSED rather than read as
/// a key: without it, any hex string that happens to decode to UTF-8 would be accepted.
/// The trailing digit is the cursor format version, to be bumped if the payload changes.
const TOKEN_MARKER: &str = "ca1";

/// The cursor scope of `ListDomains`.
pub const DOMAIN_CURSOR_SCOPE: &str = "domains";

/// The cursor scope of `ListRepositoriesInDomain` for one domain. Scoped PER DOMAIN, so a
/// cursor minted while listing one domain's repositories cannot resolve as a key in
/// another domain's listing (or in the domain listing).
pub fn repository_cursor_scope(domain: &str) -> String {
    format!("repositories/{domain}")
}

/// The cursor scope of `ListPackageGroups` for one domain. Scoped PER DOMAIN, so a cursor
/// minted while listing one domain's package groups cannot resolve as a key in another
/// domain's listing (or in any other listing). The `prefix` filter is NOT part of the
/// scope: like every other filtered listing on this service, the cursor names a place in
/// the domain's key order, so narrowing the filter on a later page resumes at the same key.
pub fn package_group_cursor_scope(domain: &str) -> String {
    format!("package-groups/{domain}")
}

/// The cursor scope of `ListSubPackageGroups` for one domain's one package group. Scoped
/// per domain AND per parent pattern, because each group has its own set of children: a
/// cursor minted while listing one group's children must not resolve as a key in
/// another's (or in any other listing).
pub fn sub_package_group_cursor_scope(domain: &str, package_group: &str) -> String {
    format!("sub-package-groups/{domain}/{package_group}")
}

/// Encode a pagination cursor: the key of the LAST item returned on this page, inside the
/// listing `scope` it names a place in.
///
/// The token is opaque to the client (hex of a marked, scoped payload, so it satisfies
/// the modelled `^\S+$` pattern and needs no escaping in a query string) and carries no
/// offset — it is resolved against the live sorted key order by [`page_of`], so a token
/// stays valid across calls that insert or remove other keys.
pub fn encode_token(scope: &str, last_key: &str) -> String {
    hex::encode(format!("{TOKEN_MARKER}:{scope}:{last_key}").as_bytes())
}

/// Decode a cursor [`encode_token`] produced FOR THIS `scope`. `None` when the token was
/// not produced by this service, or was minted by a different listing — which a handler
/// answers as an invalid `nextToken` rather than silently resolving it as a key here.
pub fn decode_token(scope: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(hex::decode(token).ok()?).ok()?;
    decoded
        .strip_prefix(&format!("{TOKEN_MARKER}:{scope}:"))
        .map(str::to_string)
}

/// One page of the entries of `items` that `matches` accepts, in key order: everything
/// strictly after the `after` cursor (when one was supplied), capped at `max_results` and
/// projected through `project`.
///
/// The cursor is resolved with `BTreeMap::range`, so a page costs the keys it actually
/// returns rather than a projection of the whole store, and `project` runs only on the
/// entries that end up on the page.
///
/// Returns the page and the `nextToken` to report — `None` on the last page, which is
/// what tells a paginating client to stop.
pub fn page_of<V, T, M, P>(
    items: &BTreeMap<String, V>,
    scope: &str,
    after: Option<&str>,
    max_results: Option<i32>,
    matches: M,
    project: P,
) -> (Vec<T>, Option<String>)
where
    M: Fn(&str, &V) -> bool,
    P: Fn(&V) -> T,
{
    use std::ops::Bound;

    let limit = page_limit(max_results);

    let start = match after {
        Some(cursor) => Bound::Excluded(cursor.to_string()),
        None => Bound::Unbounded,
    };
    let mut matching = items
        .range((start, Bound::Unbounded))
        .filter(|(key, value)| matches(key.as_str(), value))
        .peekable();

    let mut page = Vec::new();
    let mut last_key = None;
    while page.len() < limit {
        match matching.next() {
            Some((key, value)) => {
                last_key = Some(key.clone());
                page.push(project(value));
            }
            None => break,
        }
    }

    // A `nextToken` is reported only when something is actually left, so the final page
    // omits it instead of handing back a token that resolves to an empty page.
    let next_token = match (matching.peek().is_some(), last_key) {
        (true, Some(key)) => Some(encode_token(scope, &key)),
        _ => None,
    };
    (page, next_token)
}

/// One page of a stored SET of names, in sort order: everything strictly after the `after`
/// cursor (when one was supplied), capped at `max_results`.
///
/// The same contract as [`page_of`] — the cursor is the last name returned, resolved
/// against the live order with `BTreeSet::range`, and the `nextToken` is reported only
/// when a name is actually left — for the listings whose records ARE their key
/// ([`OriginRestriction::allowed_repositories`]), where there is nothing to project.
pub fn page_of_names(
    names: &BTreeSet<String>,
    scope: &str,
    after: Option<&str>,
    max_results: Option<i32>,
) -> (Vec<String>, Option<String>) {
    use std::ops::Bound;

    let limit = page_limit(max_results);

    let start = match after {
        Some(cursor) => Bound::Excluded(cursor.to_string()),
        None => Bound::Unbounded,
    };
    let mut remaining = names.range((start, Bound::Unbounded)).peekable();

    let mut page: Vec<String> = Vec::new();
    while page.len() < limit {
        match remaining.next() {
            Some(name) => page.push(name.clone()),
            None => break,
        }
    }

    let next_token = match (remaining.peek().is_some(), page.last()) {
        (true, Some(last)) => Some(encode_token(scope, last)),
        _ => None,
    };
    (page, next_token)
}

/// How many records one page carries.
///
/// CodeArtifact's `maxResults` is modelled 1..=1000 on every paginated operation here, so
/// the boundary validator has already refused anything outside that range; an absent value
/// means "the service's own page size", which is the maximum.
fn page_limit(max_results: Option<i32>) -> usize {
    const DEFAULT_PAGE_SIZE: usize = 1000;

    // The floor below is a floor, not a policy: a `maxResults` under the modelled minimum
    // is refused at the boundary and must never reach here, and a page size of zero would
    // return an empty page with no cursor forever. Assert it in debug builds so a model
    // or validator change shows up as a test failure rather than as silent coercion.
    debug_assert!(
        max_results.is_none_or(|n| n >= 1),
        "maxResults below the modelled minimum reached page_limit: {max_results:?}"
    );
    max_results
        .and_then(|n| usize::try_from(n).ok())
        .unwrap_or(DEFAULT_PAGE_SIZE)
        .max(1)
}
