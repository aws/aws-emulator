// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-cloudfront-cloudfront2020-05-31 service.
//!
//! CloudFront's distribution surface is CUSTOMER-owned: every distribution a caller can
//! see was created by `CreateDistribution`, so this store starts EMPTY and every record
//! in it was put there by a mutating call. The store is the single source of truth for
//! each distribution's id, ARN, domain name, status, version token (`ETag`), its
//! configuration as submitted, the tag set associated with it, and the invalidations
//! created under it — so a create or update followed by a get, a list, a tag read or a
//! delete answers from the one record rather than from the request that is being served.
//!
//! The same holds for every other resource kind this store carries — a key value store, an
//! Anycast static IP list, a
//! response headers policy, an origin request policy, a cache policy, an origin access
//! control, a CloudFront function, a connection function, a public key, a key group, a
//! real-time log
//! configuration: each record is put
//! there by the call that creates it and is what every later read of that resource answers
//! from. A resource policy is carried the same way, except that it is keyed by the ARN of
//! the resource it is attached to rather than by an id of its own: that ARN is the only
//! identifier the resource-policy surface addresses a policy by.
//!
//! None of a response headers policy, an origin request policy, a cache policy, an
//! origin access control, a public key, a key group or a real-time log configuration has a
//! status
//! member or a propagation deadline, so
//! nothing about one is owed by the clock; neither is anything about a function or a
//! connection function, each of which reaches its `LIVE` stage through a publish CALL
//! rather than by waiting.
//!
//! Several things in here are owed by the CLOCK rather than by a call:
//!
//! * a distribution CloudFront has just created or reconfigured is `InProgress` until
//!   its configuration has propagated, and is `Deployed` after
//!   ([`PROPAGATION_DELAY`]);
//! * a streaming (RTMP) distribution is `InProgress` and `Deployed` on the same rule, for
//!   the same reason ([`PROPAGATION_DELAY`]);
//! * an invalidation is `InProgress` until its batch finishes, and `Completed` after — and
//!   the objects its paths name leave the edge cache of the distribution it was created
//!   under at that same instant, which is what makes the refresh an invalidation buys
//!   observable only once it reports `Completed`;
//! * a key value store CloudFront has just created is `PROVISIONING` until it has been
//!   provisioned, and `READY` after ([`KVS_PROVISIONING_DELAY`]);
//! * a distribution tenant CloudFront has just created is `InProgress` until its
//!   configuration has propagated, and `Deployed` after ([`PROPAGATION_DELAY`]);
//! * a connection group CloudFront has just created is `InProgress` until its routing
//!   endpoint has propagated, and `Deployed` after ([`PROPAGATION_DELAY`]);
//! * an Anycast static IP list CloudFront has just created is `Deploying` until its
//!   allocated addresses have been deployed, and `Deployed` after ([`PROPAGATION_DELAY`]),
//!   and each of its IPAM CIDR configurations is provisioned on the same deadline;
//! * a trust store CloudFront has just created is `pending` until the CA certificates
//!   bundle it was built from has propagated to the edge locations, and `active` after
//!   ([`PROPAGATION_DELAY`]);
//! * a VPC origin CloudFront has just created is `Deploying` until its endpoint has been
//!   deployed to the edge locations, and `Deployed` after ([`PROPAGATION_DELAY`]).
//!
//! Each of them is materialised by
//! [`ComAmazonawsCloudfrontCloudfront20200531State::settle`],
//! which every handler calls before it reads: the transition was owed at the same
//! instant whether or not anybody called, so applying it on a read never changes what a
//! later caller sees.

use std::borrow::Cow;
use std::collections::{BTreeMap, BTreeSet, HashMap};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

// NOTE: the three error shapes this module's message constructors below build
// (`DistributionAlreadyExists`, `PreconditionFailed`, `InvalidArgument`) are named at
// their use sites through their full `crate::ops::types::` path rather than added to
// this list, which every operation added to this service edits.
use crate::ops::types::{
    ActiveTrustedKeyGroups, ActiveTrustedSigners, Aliases, AllowedMethods, AnycastIpList,
    AnycastIpListSummary, CacheBehavior, CacheBehaviors,
    CachePolicy, CachePolicyAlreadyExists, CachePolicyConfig,
    CachedMethods, CloudFrontOriginAccessIdentity, CloudFrontOriginAccessIdentityConfig,
    CloudFrontOriginAccessIdentitySummary,
    CNAMEAlreadyExists, ConnectionGroup, ConnectionGroupSummary, ContentTypeProfile,
    ContinuousDeploymentPolicy,
    ContinuousDeploymentPolicyConfig,
    ContinuousDeploymentSingleHeaderConfig, ContinuousDeploymentSingleWeightConfig,
    CustomErrorResponse, CustomErrorResponses, CustomOriginConfig,
    Customizations, DefaultCacheBehavior, Distribution, DistributionConfig, DistributionSummary,
    DistributionTenant, DistributionTenantSummary, DomainConflict, DomainResult,
    EntityAlreadyExists,
    EntityNotFound, FieldLevelEncryption, FieldLevelEncryptionConfig,
    FieldLevelEncryptionProfile, FieldLevelEncryptionProfileConfig,
    FieldLevelEncryptionProfileSummary, FieldLevelEncryptionSummary,
    ForwardedValues,
    FunctionAssociations, GeoRestriction, Invalidation, InvalidationBatch, InvalidationSummary,
    InvalidTagging, IpamCidrConfig, IpamConfig, KGKeyPairIds, KeyGroup, KeyGroupConfig,
    KeyPairIds, KeyValueStore,
    LambdaFunctionAssociations,
    LoggingConfig, ManagedCertificateDetails, ManagedCertificateRequest, MonitoringSubscription,
    NoSuchCachePolicy,
    NoSuchCloudFrontOriginAccessIdentity, NoSuchDistribution, NoSuchInvalidation,
    NoSuchMonitoringSubscription, NoSuchOriginAccessControl, NoSuchOriginRequestPolicy,
    NoSuchResource,
    NoSuchResponseHeadersPolicy, Origin, OriginAccessControl, OriginAccessControlConfig,
    OriginAccessControlSummary, OriginRequestPolicy, OriginRequestPolicyAlreadyExists,
    OriginRequestPolicyConfig, OriginSslProtocols, Parameter, ParameterDefinition, Paths,
    PublicKey, PublicKeyConfig, PublicKeySummary, QueryArgProfile, QueryArgProfileConfig,
    QueryArgProfiles, ResponseHeadersPolicy,
    ResponseHeadersPolicyAlreadyExists, ResponseHeadersPolicyConfig, Restrictions,
    SessionStickinessConfig, Signer, StreamingDistribution, StreamingDistributionConfig,
    StreamingDistributionSummary, StreamingLoggingConfig, StringSchemaConfig, Tag, Tags,
    TrafficConfig, TrustedKeyGroups, TrustedSigners, ValidationTokenDetail, ViewerCertificate,
    WebAclCustomization,
};

/// The CloudFront FUNCTION slice of this store: the function record, its two stages,
/// their quotas, the ARN and path a function is addressed by, and the modelled errors the
/// function operations answer with. Declared as a SUBMODULE so that surface grows without
/// touching the parts of this file the other resource kinds own; `pub use` keeps every
/// name reachable as `crate::state::<name>`, which is where each handler names it.
pub mod functions;
pub use functions::*;

/// RUNNING a function's code: the engine the code a caller stored is compiled and executed
/// in, the bounds one run is held to, and what a run reports back (the event object the
/// function returned, the lines it logged, the failure it raised). A SUBMODULE for the same
/// reason [`functions`] is one — it is the one place this service executes caller-supplied
/// code, and nothing in it reads or writes the store; `pub use` keeps every name reachable as
/// `crate::state::<name>`.
pub mod function_runtime;
pub use function_runtime::*;

/// The CONNECTION FUNCTION slice of this store: the connection function record, its two
/// stages, their quotas, the id, ARN and path one is addressed by, and the modelled errors
/// the connection function operations answer with. A SUBMODULE for the same reason
/// [`functions`] is one; `pub use` keeps every name reachable as `crate::state::<name>`.
pub mod connection_functions;
pub use connection_functions::*;

/// The REAL-TIME LOG CONFIGURATION slice of this store: the configuration record, the ARN
/// one is addressed by, the bound a create is refused at, the rules a submitted
/// configuration satisfies, and the modelled errors the real-time log configuration
/// operations answer with. A SUBMODULE for the same reason [`functions`] is one; `pub use`
/// keeps every name reachable as `crate::state::<name>`.
pub mod realtime_log_configs;
pub use realtime_log_configs::*;

/// The TRUST STORE slice of this store: the trust store record, the id and ARN one is
/// addressed by, the lookups every read goes through, and the modelled errors the trust
/// store operations answer with. A SUBMODULE for the same reason [`functions`] is one;
/// `pub use` keeps every name reachable as `crate::state::<name>`.
pub mod trust_stores;
pub use trust_stores::*;

/// Reading the CA CERTIFICATES BUNDLE a trust store is built from, in the one object store
/// this emulator holds: a trust store's certificate count is a property of that object's
/// bytes, so it is read there rather than invented here. A SUBMODULE for the same reason
/// [`functions`] is one; `pub use` keeps every name reachable as `crate::state::<name>`.
pub mod ca_certificates_bundle;
pub use ca_certificates_bundle::*;

/// The VPC ORIGIN slice of this store: the VPC origin record, the id one is addressed by,
/// the lookup every read goes through, the bound a create is refused at, and the modelled
/// errors the VPC origin operations answer with. A SUBMODULE for the same reason
/// [`functions`] is one; `pub use` keeps every name reachable as `crate::state::<name>`.
pub mod vpc_origins;
pub use vpc_origins::*;

/// `Distribution$Status` while a configuration is still propagating to the edge
/// locations. The model documents the terminal value — "When the status is `Deployed`,
/// the distribution's information is fully propagated to all CloudFront edge locations"
/// — so a distribution whose configuration has just changed is not yet in it.
pub const STATUS_IN_PROGRESS: &str = "InProgress";

/// `Distribution$Status` once the configuration has propagated: "When the status is
/// `Deployed`, the distribution's information is fully propagated to all CloudFront edge
/// locations."
pub const STATUS_DEPLOYED: &str = "Deployed";

/// `Invalidation$Status` once the batch has finished: "When the invalidation batch is
/// finished, the status is `Completed`."
pub const STATUS_COMPLETED: &str = "Completed";

/// The message CloudFront answers `DistributionNotDisabled` with, on the wire.
///
/// One CONSTANT per error code rather than a sentence built from the request: this
/// condition is a property of the distribution's state, so nothing about the request
/// belongs in the text.
///
/// The value below is the WIRE text — what the recorded service answers this code with,
/// and what this emulator therefore serves. It deliberately differs from the sentence the
/// MODEL's documentation of the error shape uses for the same rule ("The specified
/// CloudFront distribution is not disabled. You must disable the distribution before you
/// can delete it."): that one documents the condition, this one is served. Do not
/// "correct" this constant to the documentation sentence.
pub const DISTRIBUTION_NOT_DISABLED_MESSAGE: &str =
    "The distribution you are trying to delete has not been disabled.";

/// How long after the call that starts it a propagation or an invalidation batch is
/// owed.
///
/// CloudFront publishes no deadline for either, so this span is the EMULATOR's choice
/// rather than a claim about the service: it is long enough that a caller acting
/// straight after a create, an update or an invalidation still observes the transient
/// `InProgress` status (and the state rules that go with it), and short enough that a
/// caller that waits observes the terminal one.
pub const PROPAGATION_DELAY: Duration = Duration::from_millis(500);

/// The number of summaries `ListDistributions` serves when a request names no
/// `MaxItems`, and the ceiling it serves at whatever a request asks for.
///
/// CloudFront's `ListDistributions` pages at 100 items by default; `MaxItems` carries no
/// `@range` in the model, so the ceiling is this emulator's own page bound rather than a
/// published limit. ONE constant is both, so the default can never drift past the
/// ceiling. It bounds ONE response — a caller that wants more pages the rest with
/// `Marker` — so no single request can serve the whole store while holding the state
/// lock.
pub const MAX_ITEMS: i32 = 100;

/// The number of origin access controls `ListOriginAccessControls` serves when a request
/// names no `MaxItems`, and the ceiling it serves that listing at.
///
/// A listing answers with the default page size of ITS OWN operation rather than one
/// number shared by the whole service: `MaxItems` is an optional query parameter on every
/// listing ([`page_size_at`]), so the service supplies the value a request leaves out, and
/// the value it supplies is the one that operation pages at. A client that pages by the
/// `MaxItems` the response echoes reads this number, so a listing that reports another
/// operation's default hands it the wrong page size. This is the number the origin access
/// control listing pages at.
pub const MAX_ITEMS_ORIGIN_ACCESS_CONTROLS: i32 = 200;

/// The number of origin access identities `ListCloudFrontOriginAccessIdentities` serves
/// when a request names no `MaxItems`, and the ceiling it serves that listing at — the
/// origin access identity listing's own default, by the rule
/// [`MAX_ITEMS_ORIGIN_ACCESS_CONTROLS`] states.
pub const MAX_ITEMS_CLOUD_FRONT_ORIGIN_ACCESS_IDENTITIES: i32 = 200;

/// The number of real-time log configurations `ListRealtimeLogConfigs` serves when a
/// request names no `MaxItems`, and the ceiling it serves that listing at — the real-time
/// log configuration listing's own default, by the rule
/// [`MAX_ITEMS_ORIGIN_ACCESS_CONTROLS`] states.
pub const MAX_ITEMS_REALTIME_LOG_CONFIGS: i32 = 150;

/// The number of summaries one `ListDistributions` response serves: the `MaxItems` the
/// request named, or [`MAX_ITEMS`] when it named none, bounded above by [`MAX_ITEMS`].
///
/// `None` when the request named a count no page can hold, which the caller answers with
/// the modelled `InvalidArgument`.
pub fn page_size(max_items: Option<i32>) -> Option<usize> {
    page_size_at(max_items, MAX_ITEMS)
}

/// The number of items one page of a listing whose own default page size is `default`
/// serves: the `MaxItems` the request named, or `default` when it named none, bounded above
/// by that same default — the default and the ceiling are one number per listing, so the
/// size served can never drift past the size the response echoes, and a request asking for
/// more than the ceiling is served the ceiling rather than the number it asked for.
///
/// This is the ONE place this service decides a page size, which is what keeps a refusal
/// made at the request boundary (`crate::service::envelope`) and one made by a list handler
/// from disagreeing about which values a page size can take, and what bounds the size of
/// every response this service serves: it lives here, beside the ceilings it clamps to.
///
/// `MaxItems` is "the maximum number of items you want in the response", so it is a BOUND
/// on the page rather than a demand for one: a request that asks for none of them asks for
/// an empty page, which the listing serves, reporting it as truncated while items remain —
/// the page walk already follows from the size, so a caller whose own limit is already
/// satisfied reads the end of the listing rather than a refusal.
///
/// `None` only when the request named a count no page can hold — a NEGATIVE one, which
/// bounds nothing — which the caller answers with the modelled `InvalidArgument`.
pub fn page_size_at(max_items: Option<i32>, default: i32) -> Option<usize> {
    let requested = max_items.unwrap_or(default);
    if requested < 0 {
        return None;
    }
    // The clamp: whatever the request asked for, the size served is at most this listing's
    // own ceiling, so one response cannot grow with the number a caller writes.
    usize::try_from(requested.min(default)).ok()
}

// ---------------------------------------------------------------------------
// Which requests this service ADMITS: the preconditions decided from a submitted
// configuration, or from the state of the resource a request would associate one with,
// before anything is stored.
// ---------------------------------------------------------------------------

/// The sentence CloudFront refuses an association with a multi-tenant distribution that is
/// not enabled with, on the wire.
///
/// A property of the DISTRIBUTION's own configuration rather than of the request, so nothing
/// from the request belongs in it and every refusal of it reads the same.
pub const MULTI_TENANT_DISTRIBUTION_DISABLED_MESSAGE: &str =
    "The specified multi-tenant distribution is disabled.";

/// The modelled `InvalidAssociation` (HTTP 409) for a call that would associate a
/// distribution tenant with a multi-tenant distribution whose configuration is disabled.
///
/// A disabled distribution serves no traffic, so it serves no tenant of its own either:
/// "the specified CloudFront resource can't be associated", which is what the 409 states,
/// and it is decided from the distribution's stored configuration BEFORE the tenant's own
/// parameters are read — a tenant parameter the caller could fix is no answer to a
/// distribution that cannot carry the tenant at all.
pub fn multi_tenant_distribution_disabled() -> crate::ops::types::InvalidAssociation {
    crate::ops::types::InvalidAssociation {
        message: Some(MULTI_TENANT_DISTRIBUTION_DISABLED_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses an update whose distribution configuration leaves out the
/// price class with, on the wire.
///
/// The missing member is named by the sentence and nothing else of the request is, so every
/// refusal of it reads the same.
pub const ILLEGAL_UPDATE_MISSING_PRICE_CLASS_MESSAGE: &str =
    "Price class is missing for the resource";

/// The modelled `IllegalUpdate` (HTTP 400) for an update whose `DistributionConfig` states
/// no `PriceClass`.
///
/// An update submits the configuration the distribution is to hold WHOLE — "the distribution
/// configuration information for your distribution", read back member for member by
/// `GetDistributionConfig` — so a member the resource carries and the submitted document
/// does not is a modification CloudFront does not allow rather than a value it supplies.
/// `PriceClass` "only supports standard distributions. You can't specify this field for
/// multi-tenant distributions", so it is a member of exactly the configurations that serve
/// traffic directly.
pub fn illegal_update_missing_price_class() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_MISSING_PRICE_CLASS_MESSAGE.to_string()),
    }
}

/// The sentence CloudFront refuses an update that would change a distribution's
/// `ConnectionMode` with, on the wire.
///
/// The member the rule is about is named by the sentence and nothing else of the request is,
/// so every refusal of it reads the same.
pub const ILLEGAL_UPDATE_CONNECTION_MODE_MESSAGE: &str =
    "You can't update the ConnectionMode field for this distribution.";

/// The modelled `IllegalUpdate` (HTTP 400) for an update that would move a distribution's
/// `ConnectionMode` onto another value.
///
/// `ConnectionMode` "specifies whether the connection mode is through a standard
/// distribution (direct) or a multi-tenant distribution with distribution tenants
/// (tenant-only)", which is what the distribution IS rather than a setting of how it serves:
/// a standard distribution serves its own traffic under its own domain, while a multi-tenant
/// one serves the tenants bound to it and has no traffic domain of its own. So the mode is
/// fixed when the distribution is created, exactly as its caller reference is
/// ([`illegal_update_distribution_caller_reference`] states that rule), and an update
/// declaring a different one ([`connection_mode_declared`]) is a modification CloudFront
/// does not allow.
pub fn illegal_update_connection_mode() -> crate::ops::types::IllegalUpdate {
    crate::ops::types::IllegalUpdate {
        message: Some(ILLEGAL_UPDATE_CONNECTION_MODE_MESSAGE.to_string()),
    }
}

/// The event types a CloudFront FUNCTION association may name.
///
/// `FunctionAssociation$EventType` states the rule: "The event type of the function, either
/// `viewer-request` or `viewer-response`. You cannot use origin-facing event types
/// (`origin-request` and `origin-response`) with a CloudFront function." — so the set a
/// function association admits is the VIEWER half of the model's `EventType` enum, whose
/// other half belongs to the Lambda@Edge associations beside it.
pub const FUNCTION_ASSOCIATION_EVENT_TYPES: &[&str] = &["viewer-request", "viewer-response"];

/// The validation sentence a configuration whose CloudFront function association names an
/// event type no function runs on is refused with, or `None` when every association it
/// carries names a viewer event.
///
/// The sentence names the member path of the FIRST association that breaks the rule, read in
/// the order the configuration states them — the default cache behaviour, then the ordered
/// ones — the value that association submitted, and the set the member admits
/// ([`FUNCTION_ASSOCIATION_EVENT_TYPES`]): the same sentence, in the same shape, the request
/// boundary states for a value outside a set the SCHEMA carries, since what this reports is
/// the same kind of defect in the same member. Which is also why it is answered under the
/// one code an unreadable request document is answered with rather than under a code of its
/// own.
///
/// Every value it reads comes from the submitted document through `Option` combinators and
/// an iterator: nothing is indexed, sliced, parsed or unwrapped, and the only arithmetic is
/// the one-based position of a list entry.
///
/// Checked on the way IN, on every path that stores a `DistributionConfig`, so no stored
/// behaviour ever carries a function association CloudFront could not invoke.
pub fn function_association_event_refusal(config: &DistributionConfig) -> Option<String> {
    for (label, behavior) in labelled_behavior_views(config) {
        let associations = behavior
            .function_associations
            .and_then(|wrapper| wrapper.items.as_deref())
            .unwrap_or_default();
        for (position, association) in associations.iter().enumerate() {
            if FUNCTION_ASSOCIATION_EVENT_TYPES.contains(&association.event_type.as_str()) {
                continue;
            }
            // The member path the validation sentence states is the request document's own,
            // written as the boundary writes one: the lowerCamelCase member names of the
            // path down to the offending member, with each list entry addressed by its
            // ONE-BASED position and the `member` the entry travels in.
            return Some(format!(
                "1 validation errors detected: Value '{}' at \
                 'distributionConfig.{label}.functionAssociations.items.{}.member.eventType' \
                 failed to satisfy constraint: Member must satisfy enum value set: [{}]",
                association.event_type,
                position + 1,
                FUNCTION_ASSOCIATION_EVENT_TYPES.join(", ")
            ));
        }
    }
    None
}

/// Every cache behaviour of `config` beside the member path the request document addresses
/// it by: the default cache behaviour first, then each ordered one at its one-based
/// position.
fn labelled_behavior_views(
    config: &DistributionConfig,
) -> Vec<(String, configuration::BehaviorLists<'_>)> {
    let mut out = vec![(
        "defaultCacheBehavior".to_string(),
        configuration::BehaviorLists::from(&config.default_cache_behavior),
    )];
    if let Some(behaviors) = &config.cache_behaviors {
        for (position, behavior) in behaviors
            .items
            .as_deref()
            .unwrap_or_default()
            .iter()
            .enumerate()
        {
            out.push((
                format!("cacheBehaviors.items.{}.member", position + 1),
                configuration::BehaviorLists::from(behavior),
            ));
        }
    }
    out
}

/// The most distributions this emulator holds.
///
/// The store lives for the process's lifetime and grows only on a create, and a
/// distribution can be given no TTL and cannot be evicted: a `GetDistribution` has to
/// keep answering with what its create returned until a `DeleteDistribution` removes it,
/// so dropping a record would be a divergence rather than a cleanup. A maximum entry
/// count, refused on the create path before anything is inserted, is therefore the bound
/// available here — the same construct this workspace's other services use
/// (`MAX_TRUST_ANCHORS`, `MAX_PROFILES`). It sits far above any realistic client
/// workflow, so a caller never meets it by accident, and the refusal is the modelled
/// `TooManyDistributions`: "Processing your request would cause you to exceed the maximum
/// number of distributions allowed."
pub const MAX_DISTRIBUTIONS: usize = 10_000;

/// The most resource policies this emulator holds — one per resource that carries one.
///
/// Same construct and same reasoning as [`MAX_DISTRIBUTIONS`]: a policy record is what
/// `GetResourcePolicy` answers from, so it cannot be evicted, and the bound is a maximum
/// entry count refused on the write path before anything is inserted. Real CloudFront needs
/// no bound of its own here, because a policy exists only for a VPC origin and the quota on
/// those is enforced where one is created, which is why `PutResourcePolicy` models no limit
/// error at all; the refusal over this bound is therefore the modelled `InvalidArgument`,
/// the one general client refusal that operation declares. A write that REPLACES the policy
/// of a resource already in the store grows nothing, so it is never refused by it, and the
/// count sits far above any realistic client workflow.
pub const MAX_RESOURCE_POLICIES: usize = 1_000;

/// The most streaming (RTMP) distributions this emulator holds.
///
/// Same construct and same reasoning as [`MAX_DISTRIBUTIONS`]: a streaming distribution
/// record is what `GetStreamingDistribution` answers from, so it cannot be evicted and
/// the bound is a maximum entry count refused on the create path before anything is
/// minted. Over it, the refusal is the modelled `TooManyStreamingDistributions`:
/// "Processing your request would cause you to exceed the maximum number of streaming
/// distributions allowed."
pub const MAX_STREAMING_DISTRIBUTIONS: usize = 10_000;

/// The most invalidations this emulator holds for ONE distribution, or for ONE
/// distribution tenant.
///
/// Same shape as [`MAX_DISTRIBUTIONS`]: an invalidation record is what `GetInvalidation`
/// answers from, so it is bounded by a maximum entry count refused before the insert
/// rather than evicted. The refusal is the modelled `TooManyInvalidationsInProgress`:
/// "You have exceeded the maximum number of allowable InProgress invalidation batch
/// requests, or invalidation objects."
pub const MAX_INVALIDATIONS_PER_DISTRIBUTION: usize = 1_000;

/// The most ordered cache behaviours ONE distribution's configuration may state.
///
/// CloudFront's published quota is 25 cache behaviours per distribution (the Amazon
/// CloudFront Developer Guide's "Quotas", general quotas on distributions). It is also
/// what bounds the per-request work a stored configuration costs: every `Distribution`
/// payload walks the default cache behaviour and each ordered one, so a configuration
/// cannot be stored with an unbounded number of them. Over it, the refusal is the modelled
/// `TooManyCacheBehaviors`: "You cannot create more cache behaviors for the distribution."
pub const MAX_CACHE_BEHAVIORS: usize = 25;

/// The most alternate domain names (CNAMEs) ONE distribution's configuration may state.
///
/// CloudFront's published quota is 100 alternate domain names per distribution (the Amazon
/// CloudFront Developer Guide's "Quotas", general quotas on distributions). Over it, a
/// streaming distribution create is refused with the modelled
/// `TooManyStreamingDistributionCNAMEs`: "Your request contains more CNAMEs than are
/// allowed per distribution."
pub const MAX_DISTRIBUTION_CNAMES: usize = 100;

/// The most trusted signers ONE distribution's configuration may state, counted across
/// its cache behaviours.
///
/// CloudFront's published quota is 5 trusted signers (Amazon Web Services accounts) for a
/// distribution, and the modelled refusal is per distribution too: "Your request contains
/// more trusted signers than are allowed per distribution."
pub const MAX_TRUSTED_SIGNERS: usize = 5;

/// The most trusted key groups ONE distribution's configuration may state, counted across
/// its cache behaviours.
///
/// CloudFront's published quota is 4 key groups associated with a single distribution, and
/// the modelled refusal names the same scope: "The number of key groups referenced by this
/// distribution is more than the maximum allowed."
pub const MAX_TRUSTED_KEY_GROUPS: usize = 4;

/// The most paths ONE invalidation batch may name.
///
/// CloudFront's published quota for an invalidation request is 3,000 files, and the
/// batch is stored whole, so this bounds a single request's contribution to the store as
/// well. Over it, the refusal is the modelled `BatchTooLarge` (HTTP 413): "Invalidation
/// batch specified is too large."
pub const MAX_INVALIDATION_PATHS: usize = 3_000;

/// The most tags ONE CloudFront resource may carry.
///
/// CloudFront's published quota for tagging is 50 tags per distribution, and a tag set
/// is stored whole on the resource, so this bounds one resource's contribution to the
/// store as well. Over it, the refusal is the modelled `InvalidTagging`: "The tagging
/// specified is not valid."
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most domains ONE distribution tenant may hold.
///
/// The model states the quota on `UpdateDistributionTenantRequest$Domains`: "You must
/// specify at least one domain. Each distribution tenant can have up to 5 domains"
/// (`smithy_query(query="doc",shape="UpdateDistributionTenantRequest",member="Domains")`).
/// `DomainList` carries no `@length`, so it is enforced here rather than by the modelled
/// constraints — which also bounds what one record stores and what the conflict scan over
/// the store walks. Over it, the refusal is the modelled `InvalidArgument`: "An argument
/// is invalid."
pub const MAX_DOMAINS_PER_TENANT: usize = 5;

/// The most parameter VALUES one distribution tenant may carry.
///
/// Every value a tenant carries must name a parameter the multi-tenant distribution
/// defines, and no name may repeat ([`parameter_refusal`]), so a longer list names
/// definitions no distribution carries. CloudFront publishes no quota for it, so the
/// number is the EMULATOR's bound — the same order as [`MAX_TAGS_PER_RESOURCE`], this
/// service's other per-resource key-value list — on what one request may submit and one
/// record may retain. Over it, the refusal is the modelled `InvalidArgument`.
pub const MAX_PARAMETERS_PER_TENANT: usize = 50;

/// The most locations one geographic restriction may name.
///
/// `GeoRestriction$Items`: "The `Location` element is a two-letter, uppercase country code
/// for a country that you want to include in your `blacklist` or `whitelist`. Include one
/// `Location` element for each country." There are 26 x 26 two-letter codes, so a list
/// naming more than that names no set of countries at all. The model carries no
/// `@length`, so the bound is enforced where a customization is accepted — which also
/// caps what one tenant record retains.
pub const MAX_GEO_RESTRICTION_LOCATIONS: usize = 26 * 26;

/// The most distribution tenants this emulator holds.
///
/// A tenant record is held for the whole life of the emulator (a later read has to answer
/// with what the create returned), so the store's growth is bounded here rather than left
/// unbounded. CloudFront's own published quota for distribution tenants is per account and
/// adjustable, so this is the EMULATOR's bound, not a claim about the service; over it the
/// refusal is the modelled `EntityLimitExceeded`: "The entity limit has been exceeded."
pub const MAX_DISTRIBUTION_TENANTS: usize = 10_000;

/// `DomainResult$Status` — "Whether the domain is active or inactive" — for a domain a
/// distribution tenant is SERVED at: viewers asking for it are routed to the tenant.
pub const DOMAIN_STATUS_ACTIVE: &str = "active";

/// `DomainResult$Status` for a domain the tenant holds but is NOT served at yet: the
/// CloudFront managed certificate requested for it is still waiting to be validated, so
/// CloudFront cannot serve the domain and a viewer asking for it is routed nowhere
/// ([`StoredDistributionTenant::domain_pending_validation`]). The validation is what moves
/// it to [`DOMAIN_STATUS_ACTIVE`].
pub const DOMAIN_STATUS_INACTIVE: &str = "inactive";

/// The `CustomizationActionType` a distribution tenant's web ACL customization carries when
/// a web ACL of its own is associated with it: "you can specify `override` to specify a
/// separate WAF web ACL for the distribution tenant". The other value the enum admits
/// (`disable`) is the one that takes the tenant out of its distribution's protections
/// without naming a web ACL, which is a customization a caller states rather than one an
/// association produces.
pub const CUSTOMIZATION_ACTION_OVERRIDE: &str = "override";

/// `DnsConfiguration$Status` for a domain that "is correctly configured and points to the
/// correct routing endpoint of the connection group".
pub const DNS_CONFIGURATION_VALID: &str = "valid-configuration";
/// `DnsConfiguration$Status` for a domain where "there is either a missing DNS record or
/// the DNS record exists but it's using an incorrect routing endpoint": the status for a
/// verification that was MADE and disagreed — a DNS record that resolves the domain to
/// some other endpoint.
pub const DNS_CONFIGURATION_INVALID: &str = "invalid-configuration";

/// `DnsConfiguration$Status` for a domain CloudFront "can't validate": the status for a
/// verification that could not be MADE — "CloudFront can't verify the DNS record, or the
/// DNS lookup request failed or timed out" — as opposed to one that was made and
/// disagreed. A name no DNS record this service can read resolves is in exactly that
/// position: recorded behaviour, which answers this rather than
/// [`DNS_CONFIGURATION_INVALID`] for a domain with no DNS record.
pub const DNS_CONFIGURATION_UNKNOWN: &str = "unknown-configuration";

/// The prefix every distribution tenant id this service mints carries.
///
/// `GetDistributionTenant`'s `Identifier` is "the ARN, ID, or name of the distribution
/// tenant", so the three namespaces have to be told apart by shape: `DistributionTenant`'s
/// `Name` is `^[a-zA-Z0-9][a-zA-Z0-9-.]{1,126}[a-zA-Z0-9]$`, which admits no underscore,
/// and an ARN starts with `arn:` — so an id carrying an underscore can never be read as
/// either. The model states no id pattern, so the prefix is this emulator's choice.
pub const DISTRIBUTION_TENANT_ID_PREFIX: &str = "dt_";

/// The prefix every connection group id this service mints carries, told apart from a
/// tenant id and from a distribution id for the same reason
/// [`DISTRIBUTION_TENANT_ID_PREFIX`] is.
pub const CONNECTION_GROUP_ID_PREFIX: &str = "cg_";

/// The name this emulator gives the account's AUTOMATICALLY created connection group.
///
/// `ConnectionGroupSummary$Name` is required, so the group CloudFront creates for you
/// ("CloudFront will automatically create a default connection group for you") carries
/// one too. AWS documents no name for it, so this is the emulator's choice; it is stable
/// across reads, and a create that has already taken this name leaves the automatic one
/// to [`ComAmazonawsCloudfrontCloudfront20200531State::default_connection_group`]'s
/// fallback, which is derived from the group's own minted id.
pub const DEFAULT_CONNECTION_GROUP_NAME: &str = "default";

/// `CreateConnectionGroup$Ipv6Enabled` for a create that named none: "Enable IPv6 for
/// the connection group. The default is `true`."
pub const DEFAULT_CONNECTION_GROUP_IPV6_ENABLED: bool = true;

/// `CreateConnectionGroup$Enabled` for a create that named none.
///
/// "Enable the connection group" — the member is optional, and a connection group is
/// created to route the traffic of the distribution tenants placed in it, so leaving it
/// out asks for the routing group, exactly as [`DEFAULT_TENANT_ENABLED`] does for a
/// tenant.
pub const DEFAULT_CONNECTION_GROUP_ENABLED: bool = true;

/// How many connection groups this emulator holds.
///
/// `CreateConnectionGroup` models `EntityLimitExceeded` ("The entity limit has been
/// exceeded"), so the store is bounded; the bound is this emulator's, chosen like
/// [`MAX_KEY_VALUE_STORES`] rather than read off a published quota, and it is an entry
/// count refused before the insert.
pub const MAX_CONNECTION_GROUPS: usize = 1_000;

/// `DistributionTenant$Enabled` for a create that named none.
///
/// "Indicates whether the distribution tenant should be enabled when created. If the
/// distribution tenant is disabled, the distribution tenant won't serve traffic" — the
/// member is optional, and a tenant is created to serve the domains the same call
/// associates with it, so leaving it out asks for the serving tenant.
pub const DEFAULT_TENANT_ENABLED: bool = true;

/// The longest a domain name may be: RFC 1035's 255-octet wire limit less the length and
/// root bytes, the bound every DNS name obeys.
pub const MAX_DOMAIN_NAME_LEN: usize = 253;

/// The longest one label of a domain name may be (RFC 1035).
pub const MAX_DOMAIN_LABEL_LEN: usize = 63;

/// `KeyValueStore$Status` while CloudFront is still provisioning a key value store.
///
/// A store CloudFront has just accepted is not yet usable by a CloudFront function, so
/// the create answers this non-terminal status and the clock — not a later call — owes
/// the store [`KVS_STATUS_READY`].
pub const KVS_STATUS_PROVISIONING: &str = "PROVISIONING";

/// `KeyValueStore$Status` once provisioning has finished: the status a store has to be
/// in before its data plane (`cloudfront-keyvaluestore`, which serves a store in exactly
/// this terminal status) can serve a key.
pub const KVS_STATUS_READY: &str = "READY";

/// How long after the call that creates it a key value store is owed
/// [`KVS_STATUS_READY`].
///
/// CloudFront publishes no provisioning deadline, so — like [`PROPAGATION_DELAY`] — this
/// span is the EMULATOR's choice rather than a claim about the service: long enough that
/// a caller describing the store straight after the create still observes
/// [`KVS_STATUS_PROVISIONING`], short enough that a caller that waits observes the
/// terminal one. The instant is stored on the record; nothing ever sleeps for it.
pub const KVS_PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// `AnycastIpList$Status` while the list's addresses are still being deployed.
///
/// The member documents its own values — "The status of the Anycast static IP list. Valid
/// values: `Deployed`, `Deploying`, or `Failed`." — so a list CloudFront has just accepted
/// is in this non-terminal one and the clock, not a later call, owes it
/// [`STATUS_DEPLOYED`]. `Failed` is reachable only by an allocation this emulator never
/// performs, so nothing here ever reports it.
pub const ANYCAST_IP_LIST_STATUS_DEPLOYING: &str = "Deploying";

/// What an `AnycastIpList$Id` this service MINTS starts with, as the ids in this crate's
/// `ListConnectionGroups` cases are written (`ipl-08d1a1b2c3d4e5f67`).
///
/// `AnycastIpList$Id` carries no pattern in the model, so this is the form this service
/// writes an id in rather than a constraint it may read an id against: an identifier a
/// caller submits is read with [`is_namespaced_resource_id`], which is why a list id under
/// another namespace label is answered as a list that is ABSENT rather than as an identifier
/// that cannot be read.
pub const ANYCAST_IP_LIST_ID_PREFIX: &str = "ipl-";

/// How many lowercase hex digits follow [`ANYCAST_IP_LIST_ID_PREFIX`] in an
/// `AnycastIpList$Id`, from the same ids: one mint draw (64 bits, 16 digits) zero-padded to
/// this width, so every id this service mints is the width a client reads.
pub const ANYCAST_IP_LIST_ID_HEX_LEN: usize = 17;

/// The `IpCount` values a create may allocate an Anycast static IP list for:
/// `CreateAnycastIpListRequest$IpCount` is "The number of static IP addresses that are
/// allocated to the Anycast static IP list. Valid values: 21 or 3." The member carries no
/// `@range` in the model, so the documented value set is what admits an `IpCount` and
/// anything else is an argument CloudFront cannot allocate for
/// ([`invalid_anycast_ip_count`]).
pub const ANYCAST_IP_COUNTS: [i32; 2] = [3, 21];

/// `IpAddressType` "ipv4" — "`ipv4` only"
/// (`smithy_query(query="shape",shape="IpAddressType")`). Also what a create that names no
/// address type allocates from: the list has to report the family its addresses came from,
/// and IPv4 is the one every Anycast static IP list carries.
pub const IP_ADDRESS_TYPE_IPV4: &str = "ipv4";

/// `IpAddressType` "ipv6" — "`ipv6` only".
pub const IP_ADDRESS_TYPE_IPV6: &str = "ipv6";

/// `IpAddressType` "dualstack" — "`dualstack` - Allocate a list of both IPv4 and IPv6
/// addresses", so a list of this type carries addresses of BOTH families.
pub const IP_ADDRESS_TYPE_DUALSTACK: &str = "dualstack";

/// `IpamCidrStatus` while an IPAM CIDR configuration's range is still being provisioned
/// (`smithy_query(query="shape",shape="IpamCidrStatus")`), which is where a configuration
/// a create just submitted starts: the status is the service's, never the caller's.
pub const IPAM_CIDR_STATUS_PROVISIONING: &str = "provisioning";

/// `IpamCidrStatus` once the range has been provisioned — owed by the same clock that owes
/// the list its deployment, since the provisioning is part of that deployment.
pub const IPAM_CIDR_STATUS_PROVISIONED: &str = "provisioned";

/// The IPv4 block an Anycast static IP is allocated from: TEST-NET-2, the
/// `198.51.100.0/24` block reserved for documentation (RFC 5737). The addresses CloudFront
/// allocates are globally routable ones this emulator cannot hand out, so it allocates from
/// a block reserved for exactly this purpose rather than from address space that belongs to
/// somebody.
pub const ANYCAST_IPV4_PREFIX: &str = "198.51.100.";

/// How many addresses [`ANYCAST_IPV4_PREFIX`]'s block can serve: a `/24` holds 256
/// addresses, of which the network and broadcast ones are no host address, so host numbers
/// run 1..=254. It is also the bound on the whole Anycast static IP surface of this
/// emulator — a create that cannot be served an address from the block is refused with the
/// modelled `EntityLimitExceeded` ([`anycast_ip_pool_exhausted`]).
pub const ANYCAST_IP_POOL: u32 = 254;

/// The IPv6 block an Anycast static IP is allocated from: `2001:db8::/32`, reserved for
/// documentation (RFC 3849), for the reason [`ANYCAST_IPV4_PREFIX`]'s block is.
pub const ANYCAST_IPV6_PREFIX: &str = "2001:db8::";

/// `ManagedCertificateStatus` while the domains of a CloudFront managed ACM certificate
/// are still being validated
/// (`smithy_query(query="shape",shape="ManagedCertificateStatus")`).
///
/// A certificate CloudFront has just requested for a distribution tenant has not been
/// validated yet, so the request answers this non-terminal status and the clock — not a
/// later call — owes the certificate [`CERTIFICATE_STATUS_ISSUED`].
pub const CERTIFICATE_STATUS_PENDING_VALIDATION: &str = "pending-validation";

/// `ManagedCertificateStatus` once the certificate has been validated and issued: the
/// terminal status a tenant's viewers are served over it in.
pub const CERTIFICATE_STATUS_ISSUED: &str = "issued";

/// How long after the call that requests it a CloudFront managed ACM certificate is owed
/// [`CERTIFICATE_STATUS_ISSUED`].
///
/// No validation deadline is published, so — like [`PROPAGATION_DELAY`] — this span is the
/// EMULATOR's choice rather than a claim about the service: long enough that a caller
/// reading the certificate straight after the request still observes
/// [`CERTIFICATE_STATUS_PENDING_VALIDATION`], short enough that a caller that waits
/// observes the terminal one. The instant is stored on the record; nothing ever sleeps for
/// it.
pub const CERTIFICATE_VALIDATION_DELAY: Duration = Duration::from_millis(500);

/// The `ValidationTokenHost` that asks CLOUDFRONT to serve the HTTP validation token of a
/// CloudFront managed ACM certificate: "For `cloudfront`, CloudFront will automatically
/// serve the validation token. Choose this mode if you can point the domain's DNS to
/// CloudFront immediately"
/// (`smithy_query(query="shape",shape="ManagedCertificateRequest")`).
///
/// Serving the token AT the domain is only possible once the domain resolves to a
/// CloudFront resource, which is why this mode — and not the other one, which leaves the
/// token to the caller's own infrastructure — makes the create check the domain's DNS
/// before it admits the request
/// ([`ComAmazonawsCloudfrontCloudfront20200531State::unverifiable_certificate_domain`]).
pub const VALIDATION_TOKEN_HOST_CLOUDFRONT: &str = "cloudfront";

/// The well-known path an HTTP domain validation token is served at, between the host and
/// the token's own file name.
pub const VALIDATION_TOKEN_PATH: &str = "/.well-known/pki-validation/";

/// The suffix of the host that serves the content of an HTTP domain validation token, the
/// one a self-hosted validation redirects to.
pub const VALIDATION_HOST_SUFFIX: &str = "acm-validations.aws";

/// The region a CloudFront managed ACM certificate lives in: CloudFront serves viewers
/// over certificates in `us-east-1`, so that is the region segment of one's ARN, and the
/// only region segment any ARN of this global service's resources carries.
pub const CERTIFICATE_REGION: &str = "us-east-1";

/// The most key value stores this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_DISTRIBUTIONS`]: a store record
/// is what `DescribeKeyValueStore` answers from, so it cannot be evicted, and the create
/// path is refused at this entry count with the modelled `EntityLimitExceeded` ("The
/// entity limit has been exceeded") instead of growing further. It matches the data
/// plane's own ceiling on distinct stores, so a store this service accepts is one that
/// service can also hold.
pub const MAX_KEY_VALUE_STORES: usize = 1_000;

/// The most continuous deployment policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_KEY_VALUE_STORES`]: a policy
/// record is what `GetContinuousDeploymentPolicy` answers from, so it cannot be evicted,
/// and the create path is refused at this entry count with the modelled
/// `TooManyContinuousDeploymentPolicies` ("You have reached the maximum number of
/// continuous deployment policies for this Amazon Web Services account") instead of
/// growing further. The VALUE is this emulator's bound rather than a published quota.
pub const MAX_CONTINUOUS_DEPLOYMENT_POLICIES: usize = 20;

/// The largest share of production traffic a continuous deployment policy may send to a
/// staging distribution.
///
/// `ContinuousDeploymentSingleWeightConfig$Weight` is documented as "The percentage of
/// traffic to send to a staging distribution, expressed as a decimal number between 0 and
/// 0.15", so a weight outside that closed interval is no weight this parameter takes.
pub const MAX_TRAFFIC_WEIGHT: f32 = 0.15;

/// The smallest share of production traffic the same member takes, from the same
/// sentence ("between 0 and 0.15").
pub const MIN_TRAFFIC_WEIGHT: f32 = 0.0;

/// The shortest session stickiness TTL a continuous deployment policy may name.
///
/// `SessionStickinessConfig$IdleTTL` and `$MaximumTTL` are each documented as "Allowed
/// values are 300–3600 seconds (5–60 minutes)", so this bound covers both members.
pub const MIN_SESSION_STICKINESS_TTL: i32 = 300;

/// The longest session stickiness TTL either member takes, from the same sentence.
pub const MAX_SESSION_STICKINESS_TTL: i32 = 3_600;

/// The most staging distribution domain names one continuous deployment policy names.
///
/// `StagingDistributionDnsNames` carries no `@length` in the model, so this is the
/// EMULATOR's bound rather than a published quota — the one caller-chosen LENGTH a
/// `ContinuousDeploymentPolicyConfig` carries, bounded on the way in like every other
/// count this service stores ([`MAX_CACHE_BEHAVIORS`], [`MAX_INVALIDATION_PATHS`]): the
/// list is persisted whole and later compared name by name against every stored policy,
/// so the work a create does is bounded by this times
/// [`MAX_CONTINUOUS_DEPLOYMENT_POLICIES`] rather than by a length the request picks. It
/// is small because a continuous deployment policy routes traffic to ONE staging
/// distribution — the member's own documentation is singular ("The CloudFront domain name
/// of the staging distribution. For example: `d111111abcdef8.cloudfront.net`") — while
/// still admitting a handful, which is what keeps this a resource bound rather than a
/// fidelity claim about a count AWS refuses.
pub const MAX_STAGING_DISTRIBUTION_DNS_NAMES: usize = 10;

/// The most origin access identities this emulator holds.
///
/// CloudFront's published quota is 100 origin access identities per account, and the
/// operation models `TooManyCloudFrontOriginAccessIdentities` — "Processing your request
/// would cause you to exceed the maximum number of origin access identities allowed" —
/// for a create that would exceed it. An identity record is what a later read answers
/// from, so it cannot be evicted: the create path is refused at this entry count rather
/// than growing further.
pub const MAX_ORIGIN_ACCESS_IDENTITIES: usize = 100;

/// How many lowercase-hex characters an Amazon S3 canonical user id is written in.
///
/// `CloudFrontOriginAccessIdentity$S3CanonicalUserId` is "The Amazon S3 canonical user ID
/// for the origin access identity, used when giving the origin access identity read
/// permission to an object in Amazon S3", and S3 writes a canonical user id as 64
/// lowercase hex characters — the form a bucket ACL grantee takes, which is what makes
/// the shape part of the contract rather than cosmetic.
pub const S3_CANONICAL_USER_ID_LEN: usize = 64;

/// The most origin access controls this emulator holds.
///
/// CloudFront's published quota is 100 origin access controls per account, and
/// `CreateOriginAccessControl` models `TooManyOriginAccessControls` — "The number of
/// origin access controls in your Amazon Web Services account exceeds the maximum
/// allowed" — for a create that would exceed it. A control record is what a later read
/// answers from, so it cannot be evicted: the create path is refused at this entry count
/// rather than growing further.
pub const MAX_ORIGIN_ACCESS_CONTROLS: usize = 100;

/// The longest `OriginAccessControlConfig$Name` this service accepts.
///
/// The member carries no `@length` in the model, but its documentation states the bound:
/// "A name to identify the origin access control. You can specify up to 64 characters."
pub const MAX_ORIGIN_ACCESS_CONTROL_NAME: usize = 64;

/// The most response headers policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_KEY_VALUE_STORES`]: a policy
/// record is what a later read answers from, so it cannot be evicted, and the create path
/// is refused at this entry count with the modelled `TooManyResponseHeadersPolicies`
/// ("You have reached the maximum number of response headers policies for this Amazon Web
/// Services account") instead of growing further. CloudFront's own ceiling is an account
/// quota that can be raised and the model states no number, so this bound is the
/// emulator's rather than a claim about the service.
pub const MAX_RESPONSE_HEADERS_POLICIES: usize = 1_000;

/// The most origin request policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_RESPONSE_HEADERS_POLICIES`]: a
/// policy record is what a later read answers from, so it cannot be evicted, and the
/// create path is refused at this entry count with the modelled
/// `TooManyOriginRequestPolicies` ("You have reached the maximum number of origin request
/// policies for this Amazon Web Services account") instead of growing further.
/// CloudFront's own ceiling is an account quota that can be raised and the model states no
/// number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_ORIGIN_REQUEST_POLICIES: usize = 1_000;

/// The most cache policies this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_ORIGIN_REQUEST_POLICIES`]: a
/// policy record is what a later read answers from, so it cannot be evicted, and the
/// create path is refused at this entry count with the modelled `TooManyCachePolicies`
/// ("You have reached the maximum number of cache policies for this Amazon Web Services
/// account") instead of growing further. CloudFront's own ceiling is an account quota that
/// can be raised and the model states no number, so this bound is the emulator's rather
/// than a claim about the service.
pub const MAX_CACHE_POLICIES: usize = 1_000;

/// The most headers ONE cache policy or origin request policy may name.
///
/// CloudFront's published quota is 10 headers per cache policy and 10 headers per origin
/// request policy (the Amazon CloudFront Developer Guide's "Quotas", the cache key and
/// origin request settings), and both policy kinds model a refusal naming exactly that
/// scope — `TooManyHeadersInCachePolicy` and `TooManyHeadersInOriginRequestPolicy`. The
/// list is stored WHOLE inside the policy record and is walked for every request the policy
/// is attached to, so the bound is refused on every payload that states a policy rather
/// than only on the create.
pub const MAX_HEADERS_IN_POLICY: usize = 10;

/// The most cookies ONE cache policy or origin request policy may name: the same published
/// quota [`MAX_HEADERS_IN_POLICY`] states for headers, under
/// `TooManyCookiesInCachePolicy` / `TooManyCookiesInOriginRequestPolicy`.
pub const MAX_COOKIES_IN_POLICY: usize = 10;

/// The most query strings ONE cache policy or origin request policy may name: the same
/// published quota [`MAX_HEADERS_IN_POLICY`] states for headers, under
/// `TooManyQueryStringsInCachePolicy` / `TooManyQueryStringsInOriginRequestPolicy`.
pub const MAX_QUERY_STRINGS_IN_POLICY: usize = 10;

/// The prefix a public key id carries.
///
/// CloudFront answers a `CreatePublicKey` with an id like `K2K8NC4HVFE3MO` — a `K`
/// followed by [`ID_BODY_LEN`] characters of the id alphabet — so the prefix is what
/// separates the public key namespace from the `E`-prefixed one every other CloudFront
/// entity id is drawn from.
pub const PUBLIC_KEY_ID_PREFIX: char = 'K';

/// The most public keys this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_CACHE_POLICIES`]: a public key
/// record is what a later read answers from, so it cannot be evicted, and the create path
/// is refused at this entry count with the modelled `TooManyPublicKeys` ("The maximum
/// number of public keys for field-level encryption have been created. To create a new
/// public key, delete one of the existing keys.") instead of growing further. CloudFront's
/// own ceiling is an account quota that can be raised and the model states no number, so
/// this bound is the emulator's rather than a claim about the service.
pub const MAX_PUBLIC_KEYS: usize = 1_000;

/// The most key groups this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_PUBLIC_KEYS`]: a key group record
/// is what a later read answers from, so it cannot be evicted, and the create path is
/// refused at this entry count with the modelled `TooManyKeyGroups` ("You have reached the
/// maximum number of key groups for this Amazon Web Services account.") instead of growing
/// further. CloudFront's own ceiling is an account quota that can be raised and the model
/// states no number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_KEY_GROUPS: usize = 1_000;

/// The most public keys ONE key group may name.
///
/// CloudFront's published quota is 5 public keys in a single key group (the Amazon
/// CloudFront Developer Guide's "Quotas", quotas on signed URLs and signed cookies), and
/// the modelled refusal names the same scope: "The number of public keys in this key group
/// is more than the maximum allowed." The list is stored whole, so this also bounds one
/// group's contribution to the store.
pub const MAX_PUBLIC_KEYS_IN_KEY_GROUP: usize = 5;

/// The prefix a field-level encryption profile id carries.
///
/// CloudFront answers a `CreateFieldLevelEncryptionProfile` with an id like
/// `P280MFCLSYOCVU` — a `P` followed by [`ID_BODY_LEN`] characters of the id alphabet — so
/// the prefix is what separates the profile namespace from the `E`-prefixed one every other
/// CloudFront entity id is drawn from and from the `K`-prefixed public key one.
pub const FIELD_LEVEL_ENCRYPTION_PROFILE_ID_PREFIX: char = 'P';

/// The most field-level encryption profiles this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as [`MAX_PUBLIC_KEYS`]: a profile record is
/// what a later read answers from, so it cannot be evicted, and the create path is refused
/// at this entry count with the modelled `TooManyFieldLevelEncryptionProfiles` ("The maximum
/// number of profiles for field-level encryption have been created.") instead of growing
/// further. CloudFront's own ceiling is an account quota that can be raised and the model
/// states no number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_FIELD_LEVEL_ENCRYPTION_PROFILES: usize = 1_000;

/// The prefix a field-level encryption CONFIGURATION id carries.
///
/// CloudFront answers a `CreateFieldLevelEncryptionConfig` with an id in the shape its own
/// answers show — a `C` followed by [`ID_BODY_LEN`] characters of the id alphabet — so the
/// prefix is what separates the configuration namespace from the `P`-prefixed profile one,
/// the `K`-prefixed public key one and the `E`-prefixed distribution one.
pub const FIELD_LEVEL_ENCRYPTION_CONFIG_ID_PREFIX: char = 'C';

/// The most field-level encryption configurations this emulator holds.
///
/// CHOSEN by this emulator, for the same reason as
/// [`MAX_FIELD_LEVEL_ENCRYPTION_PROFILES`]: a configuration record is what a later read
/// answers from, so it cannot be evicted, and the create path is refused at this entry
/// count with the modelled `TooManyFieldLevelEncryptionConfigs` ("The maximum number of
/// configurations for field-level encryption have been created.") instead of growing
/// further. CloudFront's own ceiling is an account quota that can be raised and the model
/// states no number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_FIELD_LEVEL_ENCRYPTION_CONFIGS: usize = 1_000;

/// The most query argument-profile mappings ONE field-level encryption configuration may
/// carry.
///
/// CHOSEN by this emulator: the mappings are stored whole inside the configuration record,
/// so this is what bounds one configuration's contribution to the store, and a request past
/// it is refused with the modelled `TooManyFieldLevelEncryptionQueryArgProfiles` ("The
/// maximum number of query arg profiles for field-level encryption have been created.").
/// CloudFront's own ceiling is an account quota that can be raised and the model states no
/// number, so this bound is the emulator's rather than a claim about the service.
pub const MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES: usize = 100;

/// The most content type-profile mappings ONE field-level encryption configuration may
/// carry.
///
/// CHOSEN by this emulator, for the same reason as
/// [`MAX_FIELD_LEVEL_ENCRYPTION_QUERY_ARG_PROFILES`], and refused with the modelled
/// `TooManyFieldLevelEncryptionContentTypeProfiles` ("The maximum number of content type
/// profiles for field-level encryption have been created.").
pub const MAX_FIELD_LEVEL_ENCRYPTION_CONTENT_TYPE_PROFILES: usize = 100;

/// The `PriceClass` reported for a distribution whose stored configuration names none:
/// the class that covers every CloudFront edge location, which is what
/// "`PriceClass_All`, CloudFront responds to requests for your objects from all
/// CloudFront edge locations" describes.
pub const DEFAULT_PRICE_CLASS: &str = "PriceClass_All";

/// The `HttpVersion` reported for a distribution whose stored configuration names none.
/// `DistributionConfig$HttpVersion`: "The default value for new web distributions is
/// `http2`."
pub const DEFAULT_HTTP_VERSION: &str = "http2";

/// `GeoRestriction$RestrictionType`'s value for a distribution that restricts nothing,
/// reported for a stored configuration that carries no `Restrictions`.
pub const GEO_RESTRICTION_NONE: &str = "none";

/// `GeoRestriction$RestrictionType`'s value whose `Location` items "specify the countries
/// in which you don't want CloudFront to distribute your content" — so every country but
/// those is served.
pub const GEO_RESTRICTION_BLACKLIST: &str = "blacklist";

/// `GeoRestriction$RestrictionType`'s value whose `Location` items "specify the countries
/// in which you want CloudFront to distribute your content" — so only those are served.
pub const GEO_RESTRICTION_WHITELIST: &str = "whitelist";

/// The `ConnectionMode` value that makes a distribution MULTI-TENANT.
///
/// `DistributionConfig$ConnectionMode`: "This field specifies whether the connection mode
/// is through a standard distribution (direct) or a multi-tenant distribution with
/// distribution tenants (tenant-only)" — the model's `ConnectionMode` enum admits exactly
/// `direct` and `tenant-only`.
pub const CONNECTION_MODE_TENANT_ONLY: &str = "tenant-only";

/// The `ConnectionMode` value that makes a distribution a STANDARD one.
///
/// `DistributionConfig$ConnectionMode` is optional and the service applies nothing to it
/// (see [`resolve_distribution_config`]), so a configuration that names none describes a
/// standard distribution: one with an edge domain of its own that serves its own traffic.
/// The model's `ConnectionMode` enum admits exactly `direct` and `tenant-only`.
pub const CONNECTION_MODE_DIRECT: &str = "direct";

/// The `DomainName` a MULTI-TENANT distribution reports.
///
/// A tenant-only distribution serves no traffic of its own: its viewers reach it through
/// the routing endpoint of a CONNECTION GROUP, and the distribution-level members that
/// would describe one (`Aliases`, `IsIPV6Enabled`, `AnycastIpListId`) are documented as
/// unsupported for it. So there is no `d<label>.cloudfront.net` domain to report, and
/// CloudFront answers the required `Distribution$DomainName` with this fixed placeholder
/// instead. Recorded behaviour — the model states no placeholder — but a fixed literal
/// rather than a generated value.
pub const MULTI_TENANT_DOMAIN_NAME: &str = "-";

/// The order the service emits a cache behaviour's HTTP method list in.
///
/// `MethodsList` is an ORDERED XML list of `<Method>` elements, so its element order is
/// part of the wire value — and CloudFront emits the methods of the chosen set in its own
/// fixed order rather than in the order the request listed them. This is the order
/// CloudFront's own responses carry for the full seven-method set; the smaller sets
/// `AllowedMethods$Quantity` admits ("2 (for `GET` and `HEAD` requests), 3 (for `GET`,
/// `HEAD`, and `OPTIONS` requests) and 7") come back as its subsequences — `HEAD, GET`
/// and `HEAD, GET, OPTIONS`, which is also the order [`DEFAULT_ALLOWED_METHODS`] is
/// written in. Recorded behaviour: the model fixes the member names (`Method`'s enum
/// values) but not the order they are emitted in.
pub const EMITTED_METHOD_ORDER: [&str; 7] = [
    "HEAD", "DELETE", "POST", "GET", "OPTIONS", "PUT", "PATCH",
];

/// `IsIPV6Enabled` for a configuration that names none: a new distribution answers IPv6
/// DNS requests with an IPv6 address ("Enable IPv6" is on for a new distribution — the
/// Amazon CloudFront Developer Guide's distribution settings), which is what `true`
/// means for `DistributionConfig$IsIPV6Enabled`.
pub const DEFAULT_IS_IPV6_ENABLED: bool = true;

/// `Staging` for a configuration that names none: a distribution a caller creates
/// directly is the primary distribution, not the staging distribution of a continuous
/// deployment policy.
pub const DEFAULT_STAGING: bool = false;

/// `Origin$ConnectionAttempts` for an origin that names none. `Origin$ConnectionAttempts`:
/// "The minimum number is 1, the maximum is 3, and the default (if you don't specify
/// otherwise) is 3."
pub const DEFAULT_CONNECTION_ATTEMPTS: i32 = 3;

/// `Origin$ConnectionTimeout` for an origin that names none, in seconds.
/// `Origin$ConnectionTimeout`: "The minimum timeout is 1 second, the maximum is 10
/// seconds, and the default (if you don't specify otherwise) is 10 seconds."
pub const DEFAULT_CONNECTION_TIMEOUT: i32 = 10;

/// `CustomOriginConfig$OriginReadTimeout` for a custom origin that names none, in
/// seconds: "The minimum timeout is 1 second, the maximum is 120 seconds, and the default
/// (if you don't specify otherwise) is 30 seconds."
pub const DEFAULT_ORIGIN_READ_TIMEOUT: i32 = 30;

/// `CustomOriginConfig$OriginKeepaliveTimeout` for a custom origin that names none, in
/// seconds: "The minimum timeout is 1 second, the maximum is 120 seconds, and the default
/// (if you don't specify otherwise) is 5 seconds."
pub const DEFAULT_ORIGIN_KEEPALIVE_TIMEOUT: i32 = 5;

/// `CustomOriginConfig$OriginSslProtocols` for a custom origin that names none: the
/// protocols CloudFront uses to connect to an origin over HTTPS when the configuration
/// chooses none. `OriginSslProtocols$Items` is an `SslProtocol` list, whose values the
/// member documents as "`SSLv3`, `TLSv1`, `TLSv1.1`, and `TLSv1.2`"; the three TLS ones
/// are what a configuration that names none gets, in protocol order.
pub const DEFAULT_ORIGIN_SSL_PROTOCOLS: [&str; 3] = ["TLSv1", "TLSv1.1", "TLSv1.2"];

/// `AllowedMethods$Items` (and `CachedMethods$Items`) for a cache behaviour that names
/// none: the read-only methods, which are the ones CloudFront forwards to the origin and
/// caches the response of unless a configuration asks for more.
pub const DEFAULT_ALLOWED_METHODS: [&str; 2] = ["HEAD", "GET"];

/// `SmoothStreaming` for a cache behaviour that names none: Microsoft Smooth Streaming is
/// something a configuration opts into.
pub const DEFAULT_SMOOTH_STREAMING: bool = false;

/// `Compress` for a cache behaviour that names none: automatic compression is something a
/// configuration opts into.
pub const DEFAULT_COMPRESS: bool = false;

/// `MinTTL` for a cache behaviour that caches on `ForwardedValues` and names no minimum,
/// in seconds: CloudFront forwards the next request to the origin as soon as the object is
/// stale, which is what a zero minimum means.
pub const DEFAULT_MIN_TTL: i64 = 0;

/// `DefaultTTL` for a cache behaviour that caches on `ForwardedValues` and names none, in
/// seconds: the one day the Amazon CloudFront Developer Guide documents as the default
/// TTL applied when the origin adds no `Cache-Control` or `Expires` header.
pub const DEFAULT_DEFAULT_TTL: i64 = 86_400;

/// `MaxTTL` for a cache behaviour that caches on `ForwardedValues` and names none, in
/// seconds: the one year the Amazon CloudFront Developer Guide documents as the default
/// maximum TTL.
pub const DEFAULT_MAX_TTL: i64 = 31_536_000;

/// `CustomErrorResponse$ErrorCachingMinTTL` for a custom error response that names none,
/// in seconds: the five minutes the Amazon CloudFront Developer Guide documents as how
/// long CloudFront caches an error response from the origin by default
/// ("Customizing error responses").
pub const DEFAULT_ERROR_CACHING_MIN_TTL: i64 = 300;

/// `ViewerCertificate$SSLSupportMethod` reported for a distribution served on CloudFront's
/// own certificate: such a distribution "accepts HTTPS connections from all viewers
/// including those that don't support SNI", which is what `vip` names.
pub const SSL_SUPPORT_METHOD_VIP: &str = "vip";

/// `ViewerCertificate$MinimumProtocolVersion` reported for a distribution served on
/// CloudFront's own certificate: "If the distribution uses the CloudFront domain name such
/// as `d111111abcdef8.cloudfront.net` (you set `CloudFrontDefaultCertificate` to `true`),
/// CloudFront automatically sets the security policy to `TLSv1` regardless of the value
/// that you set here."
pub const MINIMUM_PROTOCOL_VERSION_DEFAULT_CERTIFICATE: &str = "TLSv1";

/// `ViewerCertificate$CertificateSource` for a distribution served on CloudFront's own
/// certificate, from the `CertificateSource` enum.
pub const CERTIFICATE_SOURCE_CLOUDFRONT: &str = "cloudfront";

/// `ViewerCertificate$CertificateSource` for a distribution served on an ACM certificate.
pub const CERTIFICATE_SOURCE_ACM: &str = "acm";

/// `ViewerCertificate$CertificateSource` for a distribution served on an IAM server
/// certificate.
pub const CERTIFICATE_SOURCE_IAM: &str = "iam";

/// The single endpoint CloudFront's control API is served on.
///
/// CloudFront is a GLOBAL service with one API endpoint, so the fully qualified URI of a
/// resource it just created is that resource's path under this host — it is not derived
/// from the `Host` header of the request that created it.
pub const SERVICE_ENDPOINT: &str = "cloudfront.amazonaws.com";

/// The 501 message for the one case [`service_state`] answers `None`: this service is
/// mounted on an engine that holds no per-service state, so there is no distribution
/// store to serve the request from. Unreachable through the router, which builds one
/// shared `engine::InMemoryEngine`; it is answered honestly rather than by fabricating a
/// response or by constructing a private store whose writes would be dropped.
pub const STORE_UNAVAILABLE: &str =
    "the CloudFront distribution store is unavailable: this service is mounted on an engine \
     that holds no per-service state";

/// The error a handler answers when [`service_state`] has no store for it: HTTP 501
/// carrying [`STORE_UNAVAILABLE`], in the operation's own error channel.
///
/// Generic over the operation's modelled error enum, so every handler reports this one
/// condition the same way without any of them having to name a modelled error that would
/// misdescribe it (CloudFront models no server fault for these operations).
pub fn store_unavailable<E>() -> crate::scaffolded::Scaffolded<E> {
    crate::scaffolded::Scaffolded::NotImplemented(crate::scaffolded::NotImplemented {
        message: STORE_UNAVAILABLE.to_string(),
    })
}

/// How many characters follow the leading letter of a generated CloudFront token: a
/// distribution id, an invalidation id and a version token (`ETag`) are all a letter
/// followed by 13 uppercase alphanumerics (`E`+13 for a distribution and for the version
/// token, `I`+13 for an invalidation), the shape CloudFront's own values take.
///
/// 13 is also what makes the rendering injective on `u64` (36^13 > 2^64), so every
/// character of a minted token carries mint state rather than padding.
const ID_BODY_LEN: usize = 13;

/// The alphabet CloudFront draws a distribution id, an invalidation id and a version
/// token from: decimal digits and uppercase ASCII letters.
const TOKEN_ALPHABET: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";

/// The tag set one resource carries: tag key -> the tag's value, where a tag whose
/// `Value` the request left out carries none.
///
/// A map rather than the submitted list, because a resource carries at most ONE tag per
/// key; a `BTreeMap` is what makes the order a tag read reports deterministic, which
/// CloudFront leaves unspecified.
pub type TagSet = BTreeMap<String, Option<String>>;

/// The tag set `tags` describes, or the REASON a tag set no resource can carry was
/// refused.
///
/// Two rules the model cannot express: a resource carries at most one tag per key, so a
/// request naming one key twice describes no tag set at all; and a resource carries at
/// most [`MAX_TAGS_PER_RESOURCE`] tags. Each tag's key and value length and character
/// constraints ARE modelled (`TagKey`, `TagValue`), so those are already refused before
/// any handler runs.
///
/// ONE rule for every resource kind this service tags. The reason carries no error
/// wording of its own: each caller prefixes it with the wording of whichever refusal its
/// OWN model declares, so the message a client reads names the error it was given.
pub fn tag_set(tags: &Tags) -> Result<TagSet, String> {
    let items = tags.items.as_deref().unwrap_or_default();
    if items.len() > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "a CloudFront resource carries at most {MAX_TAGS_PER_RESOURCE} tags but the \
             request names {}",
            items.len()
        ));
    }
    let mut set = TagSet::new();
    for tag in items {
        // One lookup: a key the set already held is a request that does not describe a
        // tag set at all.
        if set.insert(tag.key.clone(), tag.value.clone()).is_some() {
            return Err(format!("the tag key {} is named more than once", tag.key));
        }
    }
    Ok(set)
}

/// The `Tags` projection a tag read returns for a stored tag set.
///
/// "A complex type that contains zero or more `Tag` elements": a resource with no tags
/// reports the empty list rather than nothing at all, so a caller reads the same shape
/// whether or not the resource is tagged. ONE projection for every resource kind this
/// service tags.
pub fn tags_projection(tags: &TagSet) -> Tags {
    Tags {
        items: Some(
            tags.iter()
                .map(|(key, value)| Tag {
                    key: key.clone(),
                    value: value.clone(),
                })
                .collect(),
        ),
    }
}

/// Add `added` to the tag set `into`, or report the REASON the resulting tag set is one
/// no resource can carry.
///
/// The ADD rule for every resource kind this service tags: a tagging call associates the
/// tags it names with the resource, so a key the resource already carries takes the value
/// the call names and every other key the resource carries is left exactly as it was —
/// adding tags is not replacing the tag set. The resulting set is still bounded by
/// [`MAX_TAGS_PER_RESOURCE`], counted over the UNION of the keys (a call that only
/// changes the values of keys the resource already carries adds nothing to the count).
///
/// All-or-nothing: the bound is decided before anything is written, so a refused add
/// leaves `into` exactly as it was. The reason carries no error wording of its own, for
/// the same reason [`tag_set`]'s does not: each caller prefixes it with the wording of
/// whichever refusal its own model declares.
pub fn merge_tags(into: &mut TagSet, added: TagSet) -> Result<(), String> {
    // The keys `added` contributes are the ones the resource does not carry yet, so the
    // union is counted without building a second set of every key.
    let resulting = into.len() + added.keys().filter(|key| !into.contains_key(*key)).count();
    if resulting > MAX_TAGS_PER_RESOURCE {
        return Err(format!(
            "a CloudFront resource carries at most {MAX_TAGS_PER_RESOURCE} tags but the \
             request would leave it carrying {resulting}"
        ));
    }
    into.extend(added);
    Ok(())
}

/// The modelled `InvalidTagging` for a tag set no resource can carry, wording `reason`
/// as that error documents itself — "The tagging specified is not valid."
pub fn invalid_tagging(reason: String) -> InvalidTagging {
    InvalidTagging {
        message: Some(format!("The tagging specified is not valid: {reason}")),
    }
}

/// `NoSuchResource`'s message, in the form CloudFront answers that code with: the error
/// covers whichever resource the request named, so the sentence names none of them, and
/// nothing in it is built out of the call.
pub const NO_SUCH_RESOURCE_MESSAGE: &str =
    "The specified resource doesn't exist. Update your request and try again.";

/// The modelled `NoSuchResource` for an ARN no resource this emulator holds carries.
///
/// The wording is the one CloudFront answers for this error class, and it names no part
/// of the request: the ARN that was asked about is not part of the message, so nothing
/// here is built out of the call.
pub fn no_such_resource() -> NoSuchResource {
    NoSuchResource {
        message: Some(NO_SUCH_RESOURCE_MESSAGE.to_string()),
    }
}

/// One CloudFront streaming (RTMP) distribution, as this emulator persists it.
///
/// The record is the single source of truth for the distribution's id, ARN, domain name,
/// status, version token (`ETag`) and configuration, so the create, a later get and a
/// later listing all answer from it rather than from the request that is being served —
/// the same shape [`StoredDistribution`] takes for a web distribution.
#[derive(Debug, Clone)]
pub struct StoredStreamingDistribution {
    /// `StreamingDistribution$Id`, allocated at creation and never reused.
    pub id: String,
    /// `StreamingDistribution$ARN`. CloudFront is a global service, so the ARN carries no
    /// region.
    pub arn: String,
    /// `StreamingDistribution$DomainName`, allocated at creation and stable for the
    /// distribution's whole life.
    pub domain_name: String,
    /// [`STATUS_IN_PROGRESS`] until [`StoredStreamingDistribution::deployed_due`], then
    /// [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this distribution's current configuration is owed to be `Deployed`.
    pub deployed_due: SystemTime,
    /// `StreamingDistribution$LastModifiedTime`: when the configuration last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this distribution.
    pub etag: String,
    /// The configuration as submitted, with every member the service itself materialises
    /// resolved into it ([`resolve_streaming_distribution_config`]), stored whole so a
    /// later read returns the configuration the distribution actually runs under.
    pub config: StreamingDistributionConfig,
    /// The tags associated with this distribution. A streaming distribution is one of the
    /// resource types the tagging surface accepts ([`TAGGABLE_RESOURCE_TYPES`]), so the
    /// set lives on the record the rest of the distribution is read from — the same place
    /// [`StoredDistribution::tags`] lives.
    pub tags: TagSet,
}

impl StoredStreamingDistribution {
    /// Apply every transition the clock owes this streaming distribution at `now`.
    ///
    /// Idempotent and monotonic, for the same reason [`StoredDistribution::settle`] is: a
    /// status that is already terminal stays put, so applying this on a read cannot change
    /// what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status != STATUS_DEPLOYED && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
    }

    /// "A complex type that lists the Amazon Web Services accounts, if any, that you
    /// included in the `TrustedSigners` complex type for this distribution."
    ///
    /// Read from the stored configuration, which is what says who the trusted signers are;
    /// each account is named once, at the position it first appears. A signer's
    /// `KeyPairIds` are the ACTIVE CloudFront key pairs of that account, and no call in
    /// this emulator creates a CloudFront key pair, so the reported set is empty — the
    /// same rule [`StoredDistribution::active_trusted_signers`] follows.
    pub fn active_trusted_signers(&self) -> ActiveTrustedSigners {
        let accounts = first_seen(
            self.config
                .trusted_signers
                .items
                .as_deref()
                .unwrap_or_default()
                .iter(),
        );
        ActiveTrustedSigners {
            enabled: self.config.trusted_signers.enabled,
            quantity: i32::try_from(accounts.len()).unwrap_or(i32::MAX),
            items: (!accounts.is_empty()).then(|| {
                accounts
                    .into_iter()
                    .map(|account| Signer {
                        aws_account_number: Some(account),
                        key_pair_ids: Some(KeyPairIds {
                            quantity: 0,
                            items: None,
                        }),
                    })
                    .collect()
            }),
        }
    }

    /// The `StreamingDistribution` projection `CreateStreamingDistribution` and
    /// `GetStreamingDistribution` return.
    pub fn to_streaming_distribution(&self) -> StreamingDistribution {
        StreamingDistribution {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(self.last_modified_time)),
            domain_name: self.domain_name.clone(),
            active_trusted_signers: self.active_trusted_signers(),
            streaming_distribution_config: self.config.clone(),
        }
    }

    /// The `StreamingDistributionSummary` projection `ListStreamingDistributions` returns.
    ///
    /// Every member it shares with
    /// [`StoredStreamingDistribution::to_streaming_distribution`] takes the same value:
    /// both read the one stored record. The configuration is stored resolved
    /// ([`resolve_streaming_distribution_config`]), so the members the summary requires
    /// but the configuration makes optional are present on it.
    pub fn to_summary(&self) -> StreamingDistributionSummary {
        let config = &self.config;
        StreamingDistributionSummary {
            id: self.id.clone(),
            arn: self.arn.clone(),
            status: self.status.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            domain_name: self.domain_name.clone(),
            s3_origin: config.s3_origin.clone(),
            aliases: config.aliases.clone().unwrap_or_default(),
            trusted_signers: config.trusted_signers.clone(),
            comment: config.comment.clone(),
            price_class: config
                .price_class
                .clone()
                .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
            enabled: config.enabled,
        }
    }
}

/// Resolve every member of a submitted `StreamingDistributionConfig` that the SERVICE
/// supplies, so the stored record is the configuration the distribution actually runs
/// under rather than the request document that asked for it.
///
/// Three members are optional on the wire and mean something definite when left out, so
/// each is materialised here once, when the configuration is STORED:
///
/// * `Aliases` — "a complex type that contains information about CNAMEs (alternate domain
///   names), if any": naming none is the empty wrapper;
/// * `Logging` — "a complex type that controls whether access logs are written": naming no
///   logging configuration means logging is off, with no bucket and no prefix;
/// * `PriceClass` — the price class the distribution serves from, which is
///   [`DEFAULT_PRICE_CLASS`] where a configuration states none, exactly as it is for a web
///   distribution ([`resolve_distribution_config`]).
///
/// Resolving on the way IN is what makes two creates that describe the same distribution —
/// one stating a default explicitly, one leaving it out — the same request to the
/// caller-reference comparison.
pub fn resolve_streaming_distribution_config(
    mut config: StreamingDistributionConfig,
) -> StreamingDistributionConfig {
    config.aliases = Some(config.aliases.unwrap_or(Aliases {
        quantity: 0,
        items: None,
    }));
    config.logging = Some(config.logging.unwrap_or(StreamingLoggingConfig {
        enabled: false,
        bucket: String::new(),
        prefix: String::new(),
    }));
    config.price_class = Some(
        config
            .price_class
            .unwrap_or_else(|| DEFAULT_PRICE_CLASS.to_string()),
    );
    config
}

/// Whether two streaming distribution configurations ask for the same thing, which is what
/// decides whether a repeated `CallerReference` is the same create again or a second
/// streaming distribution under a reference that is already taken.
///
/// The same comparison [`same_origin_access_identity_config`] makes, for the same reasons:
/// every member a caller can submit takes part — down to the members a request left out,
/// which are resolved before the comparison — and comparing the DESERIALIZED
/// configurations is what makes it insensitive to the request document's whitespace and
/// member order.
pub fn same_streaming_distribution_config(
    left: &StreamingDistributionConfig,
    right: &StreamingDistributionConfig,
) -> bool {
    format!("{left:?}") == format!("{right:?}")
}

/// One CloudFront key value store, as this emulator persists it.
///
/// The record is the single source of truth for the store's id, ARN, comment, status and
/// `LastModifiedTime`, so the create, a later describe and a later mutation all answer
/// from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredKeyValueStore {
    /// `KeyValueStore$Name` — "The name of the key value store." Unique per account, so
    /// it is also the key this store is held under.
    pub name: String,
    /// `KeyValueStore$Id` — "The unique Id for the key value store": the UUID minted at
    /// creation, which is also the resource part of [`StoredKeyValueStore::arn`].
    pub id: String,
    /// `KeyValueStore$ARN`. CloudFront is a global service, so the ARN carries no region.
    pub arn: String,
    /// `KeyValueStore$Comment`, as submitted. The member is required on the wire, so a
    /// create that named none reports the empty comment.
    pub comment: String,
    /// [`KVS_STATUS_PROVISIONING`] until [`StoredKeyValueStore::ready_due`], then
    /// [`KVS_STATUS_READY`].
    pub status: String,
    /// The instant this store is owed [`KVS_STATUS_READY`]. Stored rather than slept for.
    pub ready_due: SystemTime,
    /// `KeyValueStore$LastModifiedTime` — "The last-modified time of the key value
    /// store": when the resource last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this store.
    pub etag: String,
    /// The tags submitted with the create, held so a tagging operation reads what was
    /// stored rather than re-deriving it. The SAME representation a distribution's tags
    /// take, so one tagging rule covers both resource kinds.
    pub tags: TagSet,
}

impl StoredKeyValueStore {
    /// Apply the provisioning transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic: it only ever moves the non-terminal status to the
    /// terminal one, and only once the deadline has passed, so a read that calls it
    /// cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == KVS_STATUS_PROVISIONING && now >= self.ready_due {
            self.status = KVS_STATUS_READY.to_string();
        }
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// The `KeyValueStore` projection every key value store operation returns, read from
    /// the one stored record.
    pub fn to_key_value_store(&self) -> KeyValueStore {
        KeyValueStore {
            name: self.name.clone(),
            id: self.id.clone(),
            comment: self.comment.clone(),
            arn: self.arn.clone(),
            status: Some(self.status.clone()),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
        }
    }
}

/// One CloudFront Anycast static IP list, as this emulator persists it.
///
/// The record is the single source of truth for the list's id, ARN, name, status, address
/// type, the addresses ALLOCATED to it, its IPAM CIDR configurations, its
/// `LastModifiedTime` and its version token, so the create, a later read and a later
/// mutation all answer from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredAnycastIpList {
    /// `AnycastIpList$Id`, minted at creation. Also the key this list is held under and
    /// the path segment `GetAnycastIpList` addresses it by.
    pub id: String,
    /// `AnycastIpList$Name` — "The name of the Anycast static IP list", as submitted.
    pub name: String,
    /// `AnycastIpList$Arn`. CloudFront is a global service, so the ARN carries no region.
    pub arn: String,
    /// [`ANYCAST_IP_LIST_STATUS_DEPLOYING`] until
    /// [`StoredAnycastIpList::deployed_due`], then [`STATUS_DEPLOYED`].
    pub status: String,
    /// The instant this list is owed [`STATUS_DEPLOYED`]. Stored rather than slept for.
    pub deployed_due: SystemTime,
    /// `AnycastIpList$IpAddressType` — the family the addresses below were allocated
    /// from, [`IP_ADDRESS_TYPE_IPV4`] when the create named none.
    pub ip_address_type: String,
    /// `AnycastIpList$IpCount` — "The number of IP addresses in the Anycast static IP
    /// list", so it is the length of [`StoredAnycastIpList::anycast_ip_hosts`].
    pub ip_count: i32,
    /// The address SLOTS allocated to this list out of the service's pool: allocated ONCE,
    /// by the create, and held for the list's whole life, so no address is ever allocated
    /// to two lists.
    ///
    /// `AnycastIpList$AnycastIps` — "The static IP addresses that are allocated to the
    /// Anycast static IP list" — is these slots rendered in the families the list's
    /// `IpAddressType` admits ([`anycast_ips_of`]), so a list that changes its address type
    /// reports the SAME allocation in the new families rather than taking a second one.
    pub anycast_ip_hosts: Vec<u32>,
    /// The IPAM CIDR configurations this list was created or updated with, each carrying
    /// the status the SERVICE put on it. Empty when none were named, which is what makes
    /// `AnycastIpList$IpamConfig` absent rather than an element describing nothing.
    pub ipam_cidr_configs: Vec<IpamCidrConfig>,
    /// `AnycastIpList$LastModifiedTime` — "The last time the Anycast static IP list was
    /// modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this list.
    pub etag: String,
    /// The tags submitted with the create, in the SAME representation every other tagged
    /// resource of this service uses, so one tagging rule covers them all.
    pub tags: TagSet,
}

impl StoredAnycastIpList {
    /// Apply the deployment transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, for the reason [`StoredKeyValueStore::settle`] is: it only
    /// ever moves a non-terminal status to the terminal one, and only once the deadline has
    /// passed, so a read that calls it cannot change what a later caller sees. An IPAM CIDR
    /// configuration is provisioned as part of that deployment, so its own status settles
    /// on the same deadline.
    pub fn settle(&mut self, now: SystemTime) {
        if now < self.deployed_due {
            return;
        }
        if self.status == ANYCAST_IP_LIST_STATUS_DEPLOYING {
            self.status = STATUS_DEPLOYED.to_string();
        }
        for config in &mut self.ipam_cidr_configs {
            if config.status.as_deref() == Some(IPAM_CIDR_STATUS_PROVISIONING) {
                config.status = Some(IPAM_CIDR_STATUS_PROVISIONED.to_string());
            }
        }
    }

    /// `AnycastIpList$IpamConfig` — "The IPAM configuration for the Anycast static IP
    /// list, that contains the quantity and list of IPAM CIDR configurations."
    ///
    /// `None` for a list created without any: the member is optional, so a list that has no
    /// IPAM configuration reports none rather than a wrapper stating a quantity of zero.
    /// `IpamConfig$Quantity` is "The number of IPAM CIDR configurations in the
    /// `IpamCidrConfigs` list", so it is read off that list rather than stored beside it.
    pub fn to_ipam_config(&self) -> Option<IpamConfig> {
        if self.ipam_cidr_configs.is_empty() {
            return None;
        }
        Some(IpamConfig {
            quantity: i32::try_from(self.ipam_cidr_configs.len()).unwrap_or(i32::MAX),
            ipam_cidr_configs: self.ipam_cidr_configs.clone(),
        })
    }

    /// The `AnycastIpList` projection every Anycast static IP list operation returns, read
    /// from the one stored record.
    pub fn to_anycast_ip_list(&self) -> AnycastIpList {
        AnycastIpList {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            arn: self.arn.clone(),
            ip_address_type: Some(self.ip_address_type.clone()),
            ipam_config: self.to_ipam_config(),
            anycast_ips: anycast_ips_of(&self.anycast_ip_hosts, &self.ip_address_type),
            ip_count: self.ip_count,
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
        }
    }

    /// The `AnycastIpListSummary` projection `ListAnycastIpLists` serves, read from the one
    /// stored record: "an abbreviated version of the `AnycastIpList` structure. Omits the
    /// allocated static IP addresses (`AnycastIpList$AnycastIps`)." It is the only
    /// projection that carries the version token, as `AnycastIpListSummary$ETag` — "the
    /// current version (ETag value) of the Anycast static IP list" — which the full
    /// structure reports in the `ETag` header instead.
    pub fn to_summary(&self) -> AnycastIpListSummary {
        AnycastIpListSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            status: self.status.clone(),
            arn: self.arn.clone(),
            ip_count: self.ip_count,
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            ip_address_type: Some(self.ip_address_type.clone()),
            e_tag: Some(self.etag.clone()),
            ipam_config: self.to_ipam_config(),
        }
    }
}

/// One CloudFront continuous deployment policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, its configuration as
/// submitted, its `LastModifiedTime` and its version token, so the create, a later read
/// and a later listing all answer from it rather than from the request being served.
///
/// Nothing here is owed by the clock: `ContinuousDeploymentPolicy` carries no status
/// member, so a policy CloudFront has accepted is complete the moment it is stored.
#[derive(Debug, Clone)]
pub struct StoredContinuousDeploymentPolicy {
    /// `ContinuousDeploymentPolicy$Id`, minted at creation. Also the key this policy is
    /// held under, and the path segment `GetContinuousDeploymentPolicy` addresses it by.
    pub id: String,
    /// `ContinuousDeploymentPolicy$LastModifiedTime`: when the policy last changed.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this policy.
    pub etag: String,
    /// `ContinuousDeploymentPolicyConfig` as submitted, stored whole: a later read
    /// answers from this record, never from the request that is being served.
    pub config: ContinuousDeploymentPolicyConfig,
}

impl StoredContinuousDeploymentPolicy {
    /// The `ContinuousDeploymentPolicy` projection every continuous deployment policy
    /// operation returns, read from the one stored record.
    pub fn to_policy(&self) -> ContinuousDeploymentPolicy {
        ContinuousDeploymentPolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            continuous_deployment_policy_config: self.config.clone(),
        }
    }

    /// The staging distribution domain names this policy routes traffic to — the names
    /// that make a staging distribution one a policy already covers.
    pub fn staging_distribution_dns_names(&self) -> &[String] {
        staging_distribution_dns_names(&self.config)
    }
}

/// The staging distribution domain names a configuration names, in the order it states
/// them. An absent `Items` names none, whatever `Quantity` says.
pub fn staging_distribution_dns_names(config: &ContinuousDeploymentPolicyConfig) -> &[String] {
    config
        .staging_distribution_dns_names
        .items
        .as_deref()
        .unwrap_or_default()
}

/// One CloudFront connection group, as this emulator persists it.
///
/// A connection group is what a distribution tenant's viewers are routed through: it
/// holds the routing endpoint (the DNS name) CloudFront assigned to it, whether it is
/// enabled, whether IPv6 is on, and whether it is the account's automatically created
/// DEFAULT group. The record is the single source of truth for every one of them, so the
/// create, a later read and a later listing all answer from it rather than from the
/// request being served.
#[derive(Debug, Clone)]
pub struct StoredConnectionGroup {
    /// `ConnectionGroup$Id`, minted at creation and never reused. Also the key this
    /// record is held under, and the resume point a listing's `Marker` names.
    pub id: String,
    /// `ConnectionGroup$Name` — "Enter a friendly identifier that is unique within your
    /// Amazon Web Services account. This name can't be updated after you create the
    /// connection group."
    pub name: String,
    /// `ConnectionGroup$Arn`. CloudFront is a global service, so the ARN carries no
    /// region.
    pub arn: String,
    /// `ConnectionGroup$RoutingEndpoint` — "The routing endpoint (also known as the DNS
    /// name) that is assigned to the connection group, such as
    /// `d111111abcdef8.cloudfront.net`": assigned by the service at creation, so it is
    /// minted here and never derived from the request.
    pub routing_endpoint: String,
    /// `ConnectionGroup$AnycastIpListId`, exactly as submitted — and only ever an id the
    /// create resolved to an Anycast static IP list this service holds. It is also what
    /// `ListConnectionGroups`'s `AssociationFilter` filters on.
    pub anycast_ip_list_id: Option<String>,
    /// `ConnectionGroup$Ipv6Enabled` — whether IPv6 is enabled for the group.
    pub ipv6_enabled: bool,
    /// `ConnectionGroup$Enabled` — whether the connection group is enabled.
    pub enabled: bool,
    /// `ConnectionGroup$IsDefault` — "Whether the connection group is the default
    /// connection group for the distribution tenants." True of the ONE group this
    /// service creates by itself, when a tenant that named none needed one.
    pub is_default: bool,
    /// [`STATUS_IN_PROGRESS`] until [`StoredConnectionGroup::deployed_due`], then
    /// [`STATUS_DEPLOYED`]: a connection group's routing endpoint reaches the edge
    /// locations the same way a distribution's configuration does.
    pub status: String,
    /// The instant this group is owed [`STATUS_DEPLOYED`]. Stored rather than slept for.
    pub deployed_due: SystemTime,
    /// `ConnectionGroup$CreatedTime`.
    pub created_time: SystemTime,
    /// `ConnectionGroup$LastModifiedTime`.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this group.
    pub etag: String,
    /// The tags submitted with the create, in the one representation every tagged
    /// resource in this store uses.
    pub tags: TagSet,
}

impl StoredConnectionGroup {
    /// Apply the propagation transition the clock owes at `now`.
    ///
    /// Idempotent and monotonic, for the same reason [`StoredDistribution::settle`] is:
    /// it only ever moves the non-terminal status to the terminal one, and only once the
    /// deadline has passed, so a read that calls it cannot change what a later caller
    /// sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_IN_PROGRESS && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// The `ConnectionGroup` projection every single-group operation returns, read from
    /// the one stored record.
    pub fn to_connection_group(&self) -> ConnectionGroup {
        ConnectionGroup {
            id: Some(self.id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(
                self.last_modified_time,
            )),
            tags: Some(self.to_tags()),
            ipv6_enabled: Some(self.ipv6_enabled),
            routing_endpoint: Some(self.routing_endpoint.clone()),
            anycast_ip_list_id: self.anycast_ip_list_id.clone(),
            status: Some(self.status.clone()),
            enabled: Some(self.enabled),
            is_default: Some(self.is_default),
        }
    }

    /// The `ConnectionGroupSummary` a listing reports, read from the same stored record
    /// the single-group projection is read from — so a listing and a read can never
    /// disagree about one group.
    pub fn to_summary(&self) -> ConnectionGroupSummary {
        ConnectionGroupSummary {
            id: self.id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            routing_endpoint: self.routing_endpoint.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            e_tag: self.etag.clone(),
            anycast_ip_list_id: self.anycast_ip_list_id.clone(),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
            is_default: Some(self.is_default),
        }
    }
}

/// One CloudFront distribution tenant, as this emulator persists it.
///
/// A tenant is "a distribution tenant" of ONE multi-tenant distribution: it holds the
/// domains its viewers reach, the parameter values that fill in the distribution's
/// `TenantConfig`, and the customizations that override the distribution's own settings.
/// The record is the single source of truth for every one of them, so the create, a later
/// read and a later mutation all answer from it rather than from the request being served.
#[derive(Debug, Clone)]
pub struct StoredDistributionTenant {
    /// `DistributionTenant$Id`, allocated at creation and never reused.
    pub id: String,
    /// `DistributionTenant$Arn`. CloudFront is a global service, so the ARN carries no
    /// region.
    pub arn: String,
    /// `DistributionTenant$Name` — "unique within your Amazon Web Services account. This
    /// name can't be updated after you create the distribution tenant."
    pub name: String,
    /// `DistributionTenant$DistributionId` — the multi-tenant distribution this tenant
    /// was created under.
    pub distribution_id: String,
    /// The domains associated with this tenant, in the order the create listed them. Each
    /// is reported as a `DomainResult` under the status it is served at
    /// ([`Self::to_domain_results`]).
    pub domains: Vec<String>,
    /// `DistributionTenant$ConnectionGroupId` — the connection group the tenant's viewers
    /// are routed through: the one the create named, or the account's default one.
    pub connection_group_id: String,
    /// `DistributionTenant$Customizations`, exactly as submitted: these are "specific
    /// values that you can override or disable from the multi-tenant distribution", so the
    /// service supplies nothing of its own here.
    pub customizations: Option<Customizations>,
    /// `DistributionTenant$Parameters`, exactly as submitted — the values that fill in the
    /// distribution's `TenantConfig$ParameterDefinitions`.
    pub parameters: Vec<Parameter>,
    /// The CloudFront managed ACM certificate requested for this tenant, or `None` when
    /// the create asked for none. It is MINTED by the call that carried the
    /// `ManagedCertificateRequest` and is the single source of truth every later read of
    /// the certificate surface answers from.
    pub managed_certificate: Option<StoredManagedCertificate>,
    /// The tags submitted with the create, in the one representation every tagged
    /// resource in this store uses.
    pub tags: TagSet,
    /// `DistributionTenant$Enabled` — whether the tenant serves traffic.
    pub enabled: bool,
    /// [`STATUS_IN_PROGRESS`] until [`StoredDistributionTenant::deployed_due`], then
    /// [`STATUS_DEPLOYED`]: a tenant's configuration reaches the edge locations the same
    /// way its distribution's does.
    pub status: String,
    /// The instant this tenant's current configuration is owed to be deployed. Stored
    /// rather than slept for.
    pub deployed_due: SystemTime,
    /// `DistributionTenant$CreatedTime`.
    pub created_time: SystemTime,
    /// `DistributionTenant$LastModifiedTime`.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this tenant.
    pub etag: String,
    /// Every invalidation created under this distribution tenant, keyed by invalidation id.
    ///
    /// A tenant's invalidations are addressed under the TENANT
    /// (`/distribution-tenant/{Id}/invalidation/...`), so they are held on its record
    /// rather than on the multi-tenant distribution's: a create, a read and a list of one
    /// tenant's invalidations all answer from here, and no other tenant's listing reaches
    /// them.
    pub invalidations: BTreeMap<String, StoredInvalidation>,
}

impl StoredDistributionTenant {
    /// Apply every transition the clock owes this tenant at `now`: its own propagation, and
    /// the completion of each invalidation batch created under it.
    ///
    /// Idempotent and monotonic, for the same reason
    /// [`StoredDistribution::settle`] is: it only ever moves the non-terminal status to the
    /// terminal one, and only once the deadline has passed, so a read that calls it cannot
    /// change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == STATUS_IN_PROGRESS && now >= self.deployed_due {
            self.status = STATUS_DEPLOYED.to_string();
        }
        if let Some(certificate) = &mut self.managed_certificate {
            certificate.settle(now);
        }
        settle_invalidations(&mut self.invalidations, now);
    }

    /// Whether this tenant holds `domain`, compared as DNS names are.
    pub fn holds_domain(&self, domain: &str) -> bool {
        self.domains.iter().any(|held| same_domain(held, domain))
    }

    /// Whether `domain` is still waiting for this tenant's CloudFront managed certificate to
    /// be validated.
    ///
    /// A managed certificate is minted [`CERTIFICATE_STATUS_PENDING_VALIDATION`] over the
    /// domains it covers (one validation token each), and until it is issued CloudFront
    /// cannot serve those domains — so a domain it covers is not yet reachable at the edge.
    /// A tenant that asked for no managed certificate waits for nothing, and neither does a
    /// domain the certificate does not cover.
    pub fn domain_pending_validation(&self, domain: &str) -> bool {
        self.managed_certificate.as_ref().is_some_and(|certificate| {
            certificate.status == CERTIFICATE_STATUS_PENDING_VALIDATION
                && certificate
                    .validation_tokens
                    .iter()
                    .any(|token| same_domain(&token.domain, domain))
        })
    }

    /// Whether a viewer asking for `domain` is routed to this tenant: the tenant holds the
    /// domain AND the domain is not still waiting for a managed certificate
    /// ([`Self::domain_pending_validation`]).
    ///
    /// This is also what makes the tenant's claim on the domain an ENFORCED one
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]): a domain nothing
    /// routes yet is a domain no route collides with.
    pub fn serves_domain(&self, domain: &str) -> bool {
        self.holds_domain(domain) && !self.domain_pending_validation(domain)
    }

    /// The `Tags` projection a tag read returns, built from the one stored tag set.
    pub fn to_tags(&self) -> Tags {
        tags_projection(&self.tags)
    }

    /// The `DomainResult` list every projection of this tenant reports: the domains it
    /// holds, in the order they were associated, each with the status it is SERVED under.
    ///
    /// `DomainResult$Status` is "whether the domain is active or inactive", which is a
    /// property of the domain's own reachability rather than of the tenant: a domain a
    /// viewer is routed to ([`Self::serves_domain`]) reads [`DOMAIN_STATUS_ACTIVE`], and one
    /// that routes nowhere yet — a domain whose CloudFront managed certificate is still
    /// waiting to be validated, so CloudFront cannot serve it — reads
    /// [`DOMAIN_STATUS_INACTIVE`] until the validation makes it serve. The same predicate
    /// decides the tenant's claim on the domain
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::domain_in_use`]), so what a tenant
    /// reports about a domain and what the domain routes to cannot disagree.
    ///
    /// ONE mapping, so the full document and a listing's summary cannot drift apart about a
    /// tenant's domains.
    fn to_domain_results(&self) -> Vec<DomainResult> {
        self.domains
            .iter()
            .map(|domain| DomainResult {
                domain: domain.clone(),
                status: Some(
                    if self.serves_domain(domain) {
                        DOMAIN_STATUS_ACTIVE
                    } else {
                        DOMAIN_STATUS_INACTIVE
                    }
                    .to_string(),
                ),
            })
            .collect()
    }

    /// The `DistributionTenant` projection every distribution tenant operation returns,
    /// read from the one stored record.
    pub fn to_distribution_tenant(&self) -> DistributionTenant {
        DistributionTenant {
            id: Some(self.id.clone()),
            distribution_id: Some(self.distribution_id.clone()),
            name: Some(self.name.clone()),
            arn: Some(self.arn.clone()),
            domains: Some(self.to_domain_results()),
            tags: Some(self.to_tags()),
            customizations: self.customizations.clone(),
            // A tenant that was created with no parameter values carries none, rather
            // than an empty list the request never described.
            parameters: (!self.parameters.is_empty()).then(|| self.parameters.clone()),
            connection_group_id: Some(self.connection_group_id.clone()),
            created_time: Some(::aws_smithy_types::DateTime::from(self.created_time)),
            last_modified_time: Some(::aws_smithy_types::DateTime::from(
                self.last_modified_time,
            )),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
        }
    }

    /// The `DistributionTenantSummary` projection every LISTING of distribution tenants
    /// reports, read from the same stored record [`Self::to_distribution_tenant`] reads, so
    /// a listing and a single-tenant read can never disagree about one tenant.
    ///
    /// The summary carries the tenant's current version (`DistributionTenantSummary$ETag`),
    /// which the full projection reports beside the document rather than inside it; every
    /// other member is the same value under the same name. A summary carries no
    /// `Parameters`, so the parameter values a tenant holds are reached through a read of
    /// the tenant itself.
    pub fn to_summary(&self) -> DistributionTenantSummary {
        DistributionTenantSummary {
            id: self.id.clone(),
            distribution_id: self.distribution_id.clone(),
            name: self.name.clone(),
            arn: self.arn.clone(),
            domains: self.to_domain_results(),
            connection_group_id: Some(self.connection_group_id.clone()),
            customizations: self.customizations.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            e_tag: self.etag.clone(),
            enabled: Some(self.enabled),
            status: Some(self.status.clone()),
        }
    }

    /// The ARN of the ACM certificate this tenant's customizations name, or `None` when its
    /// customizations name none.
    ///
    /// `Customizations$Certificate` is "the Certificate Manager (ACM) certificate" the
    /// tenant overrides its distribution's with, so the ARN a tenant is associated with is
    /// the one in that member of its stored record — never one derived from anything else.
    pub fn customization_certificate_arn(&self) -> Option<&str> {
        self.customizations
            .as_ref()?
            .certificate
            .as_ref()
            .map(|certificate| certificate.arn.as_str())
    }

    /// Whether a valid TLS certificate attached to this tenant covers `domain`.
    ///
    /// Two certificates can be attached to a distribution tenant: the ACM certificate its
    /// customizations name — "the Certificate Manager (ACM) certificate" it overrides its
    /// distribution's with — and the CloudFront managed certificate it requested. A managed
    /// certificate is this service's OWN record, so both its status and the names it covers
    /// are read from it ([`StoredManagedCertificate::covers_domain`]): one that is still
    /// waiting for its domains to be validated secures nothing. A certificate named by ARN
    /// is a resource of another service, whose covered names this service holds no record
    /// of, so an attached one is taken at its word for the domain asked about — the same
    /// reading [`names_custom_certificate`] gives a distribution's viewer certificate.
    pub fn certificate_covers_domain(&self, domain: &str) -> bool {
        self.customization_certificate_arn().is_some()
            || self
                .managed_certificate
                .as_ref()
                .is_some_and(|certificate| certificate.covers_domain(domain))
    }

    /// The ARN of the WAF web ACL this tenant's customizations name, or `None` when its
    /// customizations name none — which includes a customization that DISABLES the web ACL
    /// it inherits, since disabling names no ARN.
    pub fn customization_web_acl_arn(&self) -> Option<&str> {
        self.customizations
            .as_ref()?
            .web_acl
            .as_ref()?
            .arn
            .as_deref()
    }
}

/// What an `UpdateDistributionTenant` changes about one stored distribution tenant.
///
/// Every member mirrors the one the update submits, and each is OPTIONAL in the same sense:
/// `None` is "the request left this member out", which leaves the stored value exactly as
/// it was, and `Some` is the value the tenant is updated to. The struct is what carries the
/// resolved change into
/// [`ComAmazonawsCloudfrontCloudfront20200531State::apply_distribution_tenant_update`], so
/// the rule for which members a partial update touches is stated in ONE place rather than
/// once per member at the call site.
#[derive(Debug, Default)]
pub struct DistributionTenantUpdate {
    /// `DistributionId` — "The ID for the multi-tenant distribution."
    pub distribution_id: Option<String>,
    /// The domain names from `Domains` — "The domains to update for the distribution
    /// tenant."
    pub domains: Option<Vec<String>>,
    /// `Customizations` — the tenant's overrides of its distribution's settings.
    pub customizations: Option<Customizations>,
    /// `Parameters` — the values that fill in the distribution's
    /// `TenantConfig$ParameterDefinitions`.
    pub parameters: Option<Vec<Parameter>>,
    /// `ConnectionGroupId` — "The ID of the target connection group."
    pub connection_group_id: Option<String>,
    /// `Enabled` — "whether the distribution tenant should be updated to an enabled state".
    pub enabled: Option<bool>,
    /// `ManagedCertificateRequest` — the CloudFront managed ACM certificate the update asks
    /// for, which MINTS one over the domains the tenant holds after the update.
    pub managed_certificate_request: Option<ManagedCertificateRequest>,
}

/// The customization set a tenant's record retains, or `None` when the set names nothing at
/// all.
///
/// A tenant's customizations are "specific values that you can override or disable from the
/// multi-tenant distribution", so a set in which every one of them is absent describes no
/// customization: it is retained as none rather than as an element describing nothing. ONE
/// rule for every write that stores a tenant's customizations.
fn retained_customizations(customizations: Customizations) -> Option<Customizations> {
    (customizations.web_acl.is_some()
        || customizations.certificate.is_some()
        || customizations.geo_restrictions.is_some())
    .then_some(customizations)
}

/// One CloudFront managed ACM certificate, as this emulator persists it.
///
/// The certificate is MINTED by the call that carries a `ManagedCertificateRequest` for a
/// distribution tenant — nothing a read does brings one into being — and this record is
/// the single source of truth for its ARN, its status, the validation-token host that was
/// asked for and the validation token of each domain it covers, so every later read of
/// the certificate answers from it rather than from the request being served.
///
/// Its status is the one thing here the CLOCK owes: a certificate that has just been
/// requested is [`CERTIFICATE_STATUS_PENDING_VALIDATION`] until its domains have been
/// validated ([`CERTIFICATE_VALIDATION_DELAY`]), and [`CERTIFICATE_STATUS_ISSUED`] after.
#[derive(Debug, Clone)]
pub struct StoredManagedCertificate {
    /// `ManagedCertificateDetails$CertificateArn` — the ARN minted for the certificate at
    /// the moment it was requested, fixed for its whole life.
    pub arn: String,
    /// `ManagedCertificateDetails$CertificateStatus`:
    /// [`CERTIFICATE_STATUS_PENDING_VALIDATION`] until
    /// [`StoredManagedCertificate::issued_due`], then [`CERTIFICATE_STATUS_ISSUED`].
    pub status: String,
    /// The instant this certificate is owed its issuance. Stored rather than slept for.
    pub issued_due: SystemTime,
    /// The request the certificate was minted from, held because what it asked for is
    /// what the certificate reports: its `ValidationTokenHost` is
    /// `ManagedCertificateDetails$ValidationTokenHost`.
    pub request: ManagedCertificateRequest,
    /// One validation token per domain the certificate covers, in the order the request
    /// that minted it listed them — "Contains details about the validation token of the
    /// specified CloudFront managed ACM certificate."
    pub validation_tokens: Vec<StoredValidationToken>,
}

impl StoredManagedCertificate {
    /// Apply the transition the clock owes this certificate at `now`.
    ///
    /// Idempotent and monotonic for the same reason every other `settle` here is: it only
    /// ever moves the non-terminal status to the terminal one, and only once the deadline
    /// has passed, so a read that calls it cannot change what a later caller sees.
    pub fn settle(&mut self, now: SystemTime) {
        if self.status == CERTIFICATE_STATUS_PENDING_VALIDATION && now >= self.issued_due {
            self.status = CERTIFICATE_STATUS_ISSUED.to_string();
        }
    }

    /// Whether this certificate is one a viewer connection can be served over NOW, and
    /// covers `domain`.
    ///
    /// A certificate secures a domain only once it has been ISSUED: while it is
    /// [`CERTIFICATE_STATUS_PENDING_VALIDATION`] the domains it was requested for have not
    /// been validated, so it covers none of them yet. Issued, it covers the names it was
    /// minted over — one validation token each — and every name a wildcard among them
    /// covers ([`certificate_covers`]).
    pub fn covers_domain(&self, domain: &str) -> bool {
        self.status == CERTIFICATE_STATUS_ISSUED
            && certificate_covers(
                self.validation_tokens
                    .iter()
                    .map(|token| token.domain.as_str()),
                domain,
            )
    }

    /// The `ManagedCertificateDetails` projection the certificate surface returns, read
    /// from the one stored record.
    pub fn to_managed_certificate_details(&self) -> ManagedCertificateDetails {
        ManagedCertificateDetails {
            certificate_arn: Some(self.arn.clone()),
            certificate_status: Some(self.status.clone()),
            // "Specify how the HTTP validation token will be served when requesting the
            // CloudFront managed ACM certificate": the mode the request chose is the mode
            // the certificate is validated in, so it is reported back unchanged.
            validation_token_host: Some(self.request.validation_token_host.clone()),
            validation_token_details: Some(
                self.validation_tokens
                    .iter()
                    .map(StoredValidationToken::to_detail)
                    .collect(),
            ),
        }
    }
}

/// The HTTP validation token of ONE domain of a managed certificate.
///
/// A token is minted per domain with the certificate, so the pair of locations a caller
/// serving the token itself has to redirect between is a property of the stored record
/// rather than of the call that reads it.
#[derive(Debug, Clone)]
pub struct StoredValidationToken {
    /// `ValidationTokenDetail$Domain` — the domain this token validates.
    pub domain: String,
    /// The token's own opaque name, the last segment of both locations below.
    pub token: String,
}

impl StoredValidationToken {
    /// The `ValidationTokenDetail` a certificate read reports for this token.
    pub fn to_detail(&self) -> ValidationTokenDetail {
        ValidationTokenDetail {
            domain: self.domain.clone(),
            // "The domain to redirect from" is where a viewer asks for the token — the
            // well-known path under the domain being validated; "The domain to redirect
            // to" is where the token's content is served from.
            redirect_from: Some(validation_token_location(&self.domain, &self.token)),
            redirect_to: Some(validation_token_location(
                &format!("{}.{VALIDATION_HOST_SUFFIX}", self.token),
                &self.token,
            )),
        }
    }
}

/// Where the HTTP validation token `token` is served under `host`: the well-known
/// validation path under the host, ending in the token's own file name.
pub fn validation_token_location(host: &str, token: &str) -> String {
    format!("{host}{VALIDATION_TOKEN_PATH}{token}.txt")
}

/// The ARN of a CloudFront managed ACM certificate. The certificate is an ACM resource in
/// the one region CloudFront serves viewers over certificates from
/// ([`CERTIFICATE_REGION`]): `arn:aws:acm:us-east-1:<account>:certificate/<id>`.
pub fn managed_certificate_arn(id: &str) -> String {
    ::engine::emulator_arn("acm", CERTIFICATE_REGION, &format!("certificate/{id}"))
}

/// One CloudFront origin access identity, as this emulator persists it.
///
/// The record is the single source of truth for the identity's id, its S3 canonical user
/// id, its version token and its configuration as submitted, so the create, a later
/// `GetCloudFrontOriginAccessIdentity` and a later `GetCloudFrontOriginAccessIdentityConfig`
/// all answer from it rather than from the request being served.
///
/// An identity carries NO status: CloudFront answers a create with the identity ready to
/// be named in a bucket policy, so there is no transition the clock owes one and nothing
/// here for [`ComAmazonawsCloudfrontCloudfront20200531State::settle`] to materialise.
#[derive(Debug, Clone)]
pub struct StoredOriginAccessIdentity {
    /// `CloudFrontOriginAccessIdentity$Id` — "The ID for the origin access identity":
    /// the token minted at creation, which is also the key this record is held under and
    /// the path segment its `Location` names.
    pub id: String,
    /// `CloudFrontOriginAccessIdentity$S3CanonicalUserId` — the canonical user id a
    /// bucket policy or ACL names to grant this identity read access, minted with the
    /// identity and fixed for its whole life.
    pub s3_canonical_user_id: String,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this identity.
    pub etag: String,
    /// The configuration as submitted. Both of its members are required on the wire, so
    /// there is nothing for the service to resolve into it: a later read answers with
    /// what was stored.
    pub config: CloudFrontOriginAccessIdentityConfig,
}

impl StoredOriginAccessIdentity {
    /// The `CloudFrontOriginAccessIdentity` projection every identity operation returns,
    /// read from the one stored record.
    pub fn to_identity(&self) -> CloudFrontOriginAccessIdentity {
        CloudFrontOriginAccessIdentity {
            id: self.id.clone(),
            s3_canonical_user_id: self.s3_canonical_user_id.clone(),
            cloud_front_origin_access_identity_config: Some(self.config.clone()),
        }
    }

    /// The `CloudFrontOriginAccessIdentitySummary` one `ListCloudFrontOriginAccessIdentities`
    /// page element carries, read from the one stored record.
    ///
    /// Every member of the summary is required on the wire, and each is read from this
    /// record: the minted id, the canonical user id minted with it, and "the comment for
    /// this origin access identity, as originally specified when created" — the comment
    /// of the stored configuration rather than anything re-derived per listing.
    pub fn to_summary(&self) -> CloudFrontOriginAccessIdentitySummary {
        CloudFrontOriginAccessIdentitySummary {
            id: self.id.clone(),
            s3_canonical_user_id: self.s3_canonical_user_id.clone(),
            comment: self.config.comment.clone(),
        }
    }
}

/// One CloudFront response headers policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, the configuration it
/// was created with, its `LastModifiedTime` and its version token, so the create and
/// every later read answer from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `ResponseHeadersPolicy` is `Id`,
/// `LastModifiedTime` and `ResponseHeadersPolicyConfig` only — a policy carries no status
/// and no propagation deadline, so a policy this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredResponseHeadersPolicy {
    /// `ResponseHeadersPolicy$Id` — the identifier minted when the policy was created,
    /// and the key the policy is held under.
    pub id: String,
    /// The configuration as submitted — "Contains metadata about the response headers
    /// policy, and a set of configurations that specify the HTTP headers." Stored whole,
    /// so a read reports what was stored rather than re-deriving anything from a request.
    pub config: ResponseHeadersPolicyConfig,
    /// `ResponseHeadersPolicy$LastModifiedTime` — "The date and time when the response
    /// headers policy was last modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this policy.
    pub etag: String,
}

impl StoredResponseHeadersPolicy {
    /// The `ResponseHeadersPolicy` projection every response headers policy operation
    /// returns, read from the one stored record.
    pub fn to_response_headers_policy(&self) -> ResponseHeadersPolicy {
        ResponseHeadersPolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            response_headers_policy_config: self.config.clone(),
        }
    }
}

/// One CloudFront origin access control, as this emulator persists it.
///
/// The record is the single source of truth for the control's id, the configuration it
/// was created with and its version token, so the create, a later
/// `GetOriginAccessControl` and every `ListOriginAccessControls` page answer from it
/// rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `OriginAccessControl` is `Id` and
/// `OriginAccessControlConfig` only — a control carries no status and no propagation
/// deadline, so a control this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredOriginAccessControl {
    /// `OriginAccessControl$Id` — "The unique identifier of the origin access control":
    /// the token minted at creation, which is also the key this record is held under and
    /// the path segment its `Location` names.
    pub id: String,
    /// The configuration as submitted — stored whole, so a read reports what was stored
    /// rather than re-deriving anything from a request.
    pub config: OriginAccessControlConfig,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this control.
    pub etag: String,
}

impl StoredOriginAccessControl {
    /// The `OriginAccessControl` projection every origin access control operation
    /// returns, read from the one stored record.
    pub fn to_origin_access_control(&self) -> OriginAccessControl {
        OriginAccessControl {
            id: self.id.clone(),
            origin_access_control_config: Some(self.config.clone()),
        }
    }

    /// The `OriginAccessControlSummary` a listing reports for this control.
    ///
    /// `OriginAccessControlSummary$Description` is REQUIRED while
    /// `OriginAccessControlConfig$Description` is optional, so a control created without
    /// a description is summarised with an empty one rather than with no element.
    pub fn to_summary(&self) -> OriginAccessControlSummary {
        OriginAccessControlSummary {
            id: self.id.clone(),
            description: self.config.description.clone().unwrap_or_default(),
            name: self.config.name.clone(),
            signing_protocol: self.config.signing_protocol.clone(),
            signing_behavior: self.config.signing_behavior.clone(),
            origin_access_control_origin_type: self
                .config
                .origin_access_control_origin_type
                .clone(),
        }
    }
}

/// One CloudFront origin request policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, the configuration it was
/// created with, its `LastModifiedTime` and its version token, so the create and every
/// later read answer from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `OriginRequestPolicy` is `Id`,
/// `LastModifiedTime` and `OriginRequestPolicyConfig` only — a policy carries no status and
/// no propagation deadline, so a policy this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredOriginRequestPolicy {
    /// `OriginRequestPolicy$Id` — the identifier minted when the policy was created, and
    /// the key the policy is held under.
    pub id: String,
    /// The configuration as submitted — "An origin request policy configuration." Stored
    /// whole, so a read reports what was stored rather than re-deriving anything from a
    /// request, down to the members the create left out.
    pub config: OriginRequestPolicyConfig,
    /// `OriginRequestPolicy$LastModifiedTime` — "The date and time when the origin request
    /// policy was last modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this policy.
    pub etag: String,
}

impl StoredOriginRequestPolicy {
    /// The `OriginRequestPolicy` projection every origin request policy operation returns,
    /// read from the one stored record.
    pub fn to_origin_request_policy(&self) -> OriginRequestPolicy {
        OriginRequestPolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            origin_request_policy_config: self.config.clone(),
        }
    }
}

/// One CloudFront cache policy, as this emulator persists it.
///
/// The record is the single source of truth for the policy's id, the configuration it was
/// created with, its `LastModifiedTime` and its version token, so the create and every
/// later read answer from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `CachePolicy` is `Id`,
/// `LastModifiedTime` and `CachePolicyConfig` only — a policy carries no status and no
/// propagation deadline, so a policy this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredCachePolicy {
    /// `CachePolicy$Id` — the identifier minted when the policy was created, and the key
    /// the policy is held under.
    pub id: String,
    /// The configuration the policy runs under — "A cache policy configuration." Stored
    /// RESOLVED, so a read reports what was stored rather than re-deriving anything from a
    /// request: a member the create left out carries the value CloudFront supplies for it
    /// ([`resolve_cache_policy_config`]).
    pub config: CachePolicyConfig,
    /// `CachePolicy$LastModifiedTime` — "The date and time when the cache policy was last
    /// modified."
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this policy.
    pub etag: String,
}

impl StoredCachePolicy {
    /// The `CachePolicy` projection every cache policy operation returns, read from the
    /// one stored record.
    pub fn to_cache_policy(&self) -> CachePolicy {
        CachePolicy {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            cache_policy_config: self.config.clone(),
        }
    }
}

/// The `CachePolicyType` a policy this ACCOUNT created carries — "custom – Returns only
/// the custom policies created in your Amazon Web Services account". One of the two values
/// the model's `CachePolicyType` enum admits (`CACHE_POLICY_TYPE_VALUES`).
pub const CACHE_POLICY_TYPE_CUSTOM: &str = "custom";

/// The `CachePolicyType` a policy AWS created carries — "managed – Returns only the managed
/// policies created by Amazon Web Services". The other value the model's `CachePolicyType`
/// enum admits.
pub const CACHE_POLICY_TYPE_MANAGED: &str = "managed";

/// One entry of the catalog of AWS-MANAGED cache policies.
///
/// A managed cache policy is created by AWS rather than by this account: every account has
/// the same ones, under the identifier and name AWS publishes for each, which is what
/// `ListCachePolicies`'s filter separates from the account's own policies ("managed –
/// Returns only the managed policies created by Amazon Web Services"). Nothing a caller
/// does creates, changes or removes one, so the catalog is STATIC data read beside the
/// mutable store rather than a slice of it — it consumes none of the account's cache policy
/// allowance and no call can take an entry out of it.
///
/// The entries in [`MANAGED_CACHE_POLICIES`] are the ones Amazon publishes in the
/// CloudFront Developer Guide ("Using the managed cache policies"), which lists each
/// policy's name, its identifier and its cache key and TTL settings; the model itself
/// carries no list of them (`smithy_query(query="shape",shape="CachePolicyType")` admits
/// only the two type values), so the catalog is written out here.
#[derive(Debug, Clone)]
pub struct ManagedCachePolicy {
    /// `CachePolicy$Id` — the identifier AWS publishes for this policy, the same in every
    /// account.
    pub id: &'static str,
    /// `CachePolicyConfig$Name` — the name AWS publishes for this policy.
    pub name: &'static str,
    /// `CachePolicyConfig$Comment` — what the policy is for.
    pub comment: &'static str,
    /// `CachePolicyConfig$MinTTL`, in seconds.
    pub min_ttl: i64,
    /// `CachePolicyConfig$DefaultTTL`, in seconds.
    pub default_ttl: i64,
    /// `CachePolicyConfig$MaxTTL`, in seconds.
    pub max_ttl: i64,
    /// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingGzip`.
    pub gzip: bool,
    /// `ParametersInCacheKeyAndForwardedToOrigin$EnableAcceptEncodingBrotli`.
    pub brotli: bool,
    /// A value of `CachePolicyHeaderBehavior`.
    pub header_behavior: &'static str,
    /// The headers the cache key includes, when the behaviour names a list.
    pub headers: &'static [&'static str],
    /// A value of `CachePolicyCookieBehavior`.
    pub cookie_behavior: &'static str,
    /// The cookies the cache key includes, when the behaviour names a list.
    pub cookies: &'static [&'static str],
    /// A value of `CachePolicyQueryStringBehavior`.
    pub query_string_behavior: &'static str,
    /// The query strings the cache key includes, when the behaviour names a list.
    pub query_strings: &'static [&'static str],
}

impl ManagedCachePolicy {
    /// The `CachePolicy` projection a read of this managed policy reports: its published
    /// identity beside the configuration AWS gave it.
    pub fn to_cache_policy(&self) -> CachePolicy {
        self.to_stored().to_cache_policy()
    }

    /// This catalog entry as a cache policy RECORD, so every read of a cache policy answers
    /// a published policy exactly as it answers one this account created — the two are the
    /// same resource kind, differing only in who created them.
    pub fn to_stored(&self) -> StoredCachePolicy {
        StoredCachePolicy {
            id: self.id.to_string(),
            config: self.to_config(),
            last_modified_time: managed_policy_last_modified(),
            etag: self.etag(),
        }
    }

    /// The version token a read of this managed policy reports, derived from its published
    /// id by the one rule every published policy's version follows
    /// ([`managed_policy_etag`]).
    pub fn etag(&self) -> String {
        managed_policy_etag(self.id)
    }

    /// The configuration AWS published for this policy.
    fn to_config(&self) -> CachePolicyConfig {
        CachePolicyConfig {
            comment: Some(self.comment.to_string()),
            name: self.name.to_string(),
            default_ttl: Some(self.default_ttl),
            max_ttl: Some(self.max_ttl),
            min_ttl: self.min_ttl,
            parameters_in_cache_key_and_forwarded_to_origin: Some(
                crate::ops::types::ParametersInCacheKeyAndForwardedToOrigin {
                    enable_accept_encoding_gzip: self.gzip,
                    enable_accept_encoding_brotli: Some(self.brotli),
                    headers_config: crate::ops::types::CachePolicyHeadersConfig {
                        header_behavior: self.header_behavior.to_string(),
                        headers: name_list(self.headers)
                            .map(|(quantity, items)| crate::ops::types::Headers { quantity, items }),
                    },
                    cookies_config: crate::ops::types::CachePolicyCookiesConfig {
                        cookie_behavior: self.cookie_behavior.to_string(),
                        cookies: name_list(self.cookies).map(|(quantity, items)| {
                            crate::ops::types::CookieNames { quantity, items }
                        }),
                    },
                    query_strings_config: crate::ops::types::CachePolicyQueryStringsConfig {
                        query_string_behavior: self.query_string_behavior.to_string(),
                        query_strings: name_list(self.query_strings).map(|(quantity, items)| {
                            crate::ops::types::QueryStringNames { quantity, items }
                        }),
                    },
                },
            ),
        }
    }
}

/// The `(Quantity, Items)` wrapper a name list travels in, or `None` when the behaviour
/// names no list at all — a behaviour of `none`, `all` or `allExcept` nothing is listed
/// under carries no wrapper rather than an empty one.
fn name_list(names: &'static [&'static str]) -> Option<(i32, Option<Vec<String>>)> {
    if names.is_empty() {
        return None;
    }
    Some((
        i32::try_from(names.len()).unwrap_or(i32::MAX),
        Some(names.iter().map(|name| (*name).to_string()).collect()),
    ))
}

/// The instant every policy AWS published reports as its `LastModifiedTime`.
///
/// A published policy belongs to AWS and no call in this account changes one, so the
/// instant is FIXED rather than drawn from the clock: every read of a published catalog, in
/// any process, reports the same one. It is this service's own API version date
/// (`2020-05-31T00:00:00Z`), the earliest instant any policy of this API could have been
/// published at.
pub fn managed_policy_last_modified() -> SystemTime {
    SystemTime::UNIX_EPOCH + Duration::from_secs(MANAGED_POLICY_LAST_MODIFIED_UNIX_SECONDS)
}

/// The version token a read of a policy AWS published reports.
///
/// DERIVED from the published id rather than minted: nothing changes a published policy, so
/// its version is the same on every read and in every process, which a drawn token could
/// not be. It takes the shape every version token of this service takes
/// ([`is_version_token`]): `E` followed by [`ID_BODY_LEN`] characters of the id alphabet,
/// here the leading hex digits of the id.
pub fn managed_policy_etag(id: &str) -> String {
    let body: String = id
        .chars()
        .filter(char::is_ascii_hexdigit)
        .map(|digit| digit.to_ascii_uppercase())
        .take(ID_BODY_LEN)
        .collect();
    format!("E{body}")
}

/// `2020-05-31T00:00:00Z` in seconds since the Unix epoch.
const MANAGED_POLICY_LAST_MODIFIED_UNIX_SECONDS: u64 = 1_590_883_200;

/// The AWS-managed cache policies, in the order Amazon PUBLISHES them rather than the
/// order their ids sort in — the order every listing of this service serves them in, ahead
/// of the policies this account created ([`policy_listing_key`]), so the first policy of a
/// managed page is the first policy Amazon lists.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes for it, and the
/// entries stand in the order that same list presents them (see [`ManagedCachePolicy`] for
/// the source).
pub const MANAGED_CACHE_POLICIES: &[ManagedCachePolicy] = &[
    ManagedCachePolicy {
        id: "658327ea-f89d-4fab-a63d-7e88639e58f6",
        name: "Managed-CachingOptimized",
        comment: "Policy with caching enabled. Supports Gzip and Brotli compression.",
        min_ttl: 1,
        default_ttl: 86_400,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: true,
        header_behavior: "none",
        headers: &[],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "4135ea2d-6df8-44a3-9df3-4b5a84be39ad",
        name: "Managed-CachingDisabled",
        comment: "Policy with caching disabled",
        min_ttl: 0,
        default_ttl: 0,
        max_ttl: 0,
        gzip: false,
        brotli: false,
        header_behavior: "none",
        headers: &[],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "b2884449-e4de-46a7-ac36-70bc7f1ddd6d",
        name: "Managed-CachingOptimizedForUncompressedObjects",
        comment: "Policy with caching enabled that does not support compression",
        min_ttl: 1,
        default_ttl: 86_400,
        max_ttl: 31_536_000,
        gzip: false,
        brotli: false,
        header_behavior: "none",
        headers: &[],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "08627262-05a9-4f76-9ded-b50ca2e3a84f",
        name: "Managed-Elemental-MediaPackage",
        comment: "Policy for an AWS Elemental MediaPackage origin",
        min_ttl: 0,
        default_ttl: 86_400,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: false,
        header_behavior: "whitelist",
        headers: &["Origin"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "whitelist",
        query_strings: &["aws.manifestfilter", "start", "end", "m"],
    },
    ManagedCachePolicy {
        id: "2e54312d-136d-493c-8eb9-b001f22f67d2",
        name: "Managed-Amplify",
        comment: "Policy for an AWS Amplify web application origin",
        min_ttl: 2,
        default_ttl: 2,
        max_ttl: 600,
        gzip: true,
        brotli: true,
        header_behavior: "whitelist",
        headers: &["Authorization", "Host"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "83da9c7e-98b4-4e11-a168-04f0df8e2c65",
        name: "Managed-UseOriginCacheControlHeaders",
        comment: "Policy for an origin that returns Cache-Control headers",
        min_ttl: 0,
        default_ttl: 0,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: true,
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedCachePolicy {
        id: "4cc15a8a-d715-48a4-82b8-cc0b614638fe",
        name: "Managed-UseOriginCacheControlHeaders-QueryStrings",
        comment: "Policy for an origin that returns Cache-Control headers and serves \
                  different objects for different query strings",
        min_ttl: 0,
        default_ttl: 0,
        max_ttl: 31_536_000,
        gzip: true,
        brotli: true,
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
];

/// The managed cache policy `id` names, or `None` when the catalog holds none — what tells
/// an id AWS published from one this account minted.
pub fn managed_cache_policy(id: &str) -> Option<&'static ManagedCachePolicy> {
    MANAGED_CACHE_POLICIES
        .iter()
        .find(|managed| managed.id == id)
}

/// Where [`MANAGED_CACHE_POLICIES`] lists the policy `id`, or `None` for an id the
/// catalog does not hold — the rank a listing orders a published cache policy by
/// ([`policy_listing_key`]).
pub fn managed_cache_policy_rank(id: &str) -> Option<usize> {
    MANAGED_CACHE_POLICIES
        .iter()
        .position(|managed| managed.id == id)
}

/// The key a listing of a policy family orders ONE policy by, given where the published
/// catalog lists it (`rank`, `None` for a policy this account created) and its id.
///
/// A policy AWS published sorts where the published catalog lists it, and ahead of every
/// policy this account created; the account's own policies sort among themselves by id.
/// That is ONE total order over both sources rather than two concatenated lists, so
/// `Marker` resumes at an id whichever source it came from, and every page of a walk serves
/// each policy exactly once.
pub fn policy_listing_key(rank: Option<usize>, id: &str) -> (usize, String) {
    match rank {
        Some(rank) => (rank, String::new()),
        None => (usize::MAX, id.to_string()),
    }
}

/// The `OriginRequestPolicyType` a policy this ACCOUNT created carries — "custom – Returns
/// only the custom policies created in your Amazon Web Services account". One of the two
/// values the model's `OriginRequestPolicyType` enum admits.
pub const ORIGIN_REQUEST_POLICY_TYPE_CUSTOM: &str = "custom";

/// The `OriginRequestPolicyType` a policy AWS created carries — "managed – Returns only the
/// managed policies created by Amazon Web Services". The other value the same enum admits.
pub const ORIGIN_REQUEST_POLICY_TYPE_MANAGED: &str = "managed";

/// One entry of the catalog of AWS-MANAGED origin request policies.
///
/// The same reference data [`ManagedCachePolicy`] holds for the cache policy family, for
/// the origin request policy family: AWS creates these policies, every account has the same
/// ones under the identifier and name AWS publishes for each, and nothing a caller does
/// here creates, changes or removes one — so the catalog is STATIC data read beside the
/// mutable store rather than a slice of it.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes in the
/// CloudFront Developer Guide ("Using the managed origin request policies"); the model
/// itself carries no list of them (`smithy_query(query="shape",
/// shape="OriginRequestPolicyType")` admits only the two type values), so the catalog is
/// written out here.
#[derive(Debug, Clone)]
pub struct ManagedOriginRequestPolicy {
    /// `OriginRequestPolicy$Id` — the identifier AWS publishes for this policy, the same in
    /// every account.
    pub id: &'static str,
    /// `OriginRequestPolicyConfig$Name` — the name AWS publishes for this policy.
    pub name: &'static str,
    /// `OriginRequestPolicyConfig$Comment` — what the policy is for.
    pub comment: &'static str,
    /// A value of `OriginRequestPolicyHeaderBehavior`.
    pub header_behavior: &'static str,
    /// The headers the behaviour names, when it names a list.
    pub headers: &'static [&'static str],
    /// A value of `OriginRequestPolicyCookieBehavior`.
    pub cookie_behavior: &'static str,
    /// The cookies the behaviour names, when it names a list.
    pub cookies: &'static [&'static str],
    /// A value of `OriginRequestPolicyQueryStringBehavior`.
    pub query_string_behavior: &'static str,
    /// The query strings the behaviour names, when it names a list.
    pub query_strings: &'static [&'static str],
}

impl ManagedOriginRequestPolicy {
    /// The `OriginRequestPolicy` projection a read of this published policy reports: its
    /// published identity beside the configuration AWS gave it.
    pub fn to_origin_request_policy(&self) -> OriginRequestPolicy {
        self.to_stored().to_origin_request_policy()
    }

    /// This catalog entry as an origin request policy RECORD, so every read of an origin
    /// request policy answers a published policy exactly as it answers one this account
    /// created — the two are the same resource kind, differing only in who created them.
    pub fn to_stored(&self) -> StoredOriginRequestPolicy {
        StoredOriginRequestPolicy {
            id: self.id.to_string(),
            config: self.to_config(),
            last_modified_time: managed_policy_last_modified(),
            etag: self.etag(),
        }
    }

    /// The version token a read of this published policy reports, by the one rule every
    /// published policy's version follows ([`managed_policy_etag`]).
    pub fn etag(&self) -> String {
        managed_policy_etag(self.id)
    }

    /// The configuration AWS published for this policy.
    fn to_config(&self) -> OriginRequestPolicyConfig {
        OriginRequestPolicyConfig {
            comment: Some(self.comment.to_string()),
            name: self.name.to_string(),
            headers_config: crate::ops::types::OriginRequestPolicyHeadersConfig {
                header_behavior: self.header_behavior.to_string(),
                headers: name_list(self.headers)
                    .map(|(quantity, items)| crate::ops::types::Headers { quantity, items }),
            },
            cookies_config: crate::ops::types::OriginRequestPolicyCookiesConfig {
                cookie_behavior: self.cookie_behavior.to_string(),
                cookies: name_list(self.cookies)
                    .map(|(quantity, items)| crate::ops::types::CookieNames { quantity, items }),
            },
            query_strings_config: crate::ops::types::OriginRequestPolicyQueryStringsConfig {
                query_string_behavior: self.query_string_behavior.to_string(),
                query_strings: name_list(self.query_strings).map(|(quantity, items)| {
                    crate::ops::types::QueryStringNames { quantity, items }
                }),
            },
        }
    }
}

/// The AWS-managed origin request policies, in the order Amazon PUBLISHES them rather than
/// the order their ids sort in — the order every listing of this service serves them in,
/// ahead of the policies this account created ([`policy_listing_key`]).
///
/// Each entry's identifier, name and settings are the ones Amazon publishes for it, and the
/// entries stand in the order that same list presents them (see
/// [`ManagedOriginRequestPolicy`] for the source).
pub const MANAGED_ORIGIN_REQUEST_POLICIES: &[ManagedOriginRequestPolicy] = &[
    ManagedOriginRequestPolicy {
        id: "acba4595-bd28-49b8-b9fe-13317c0390fa",
        name: "Managed-UserAgentRefererHeaders",
        comment: "Policy to forward user-agent and referer headers to origin",
        header_behavior: "whitelist",
        headers: &["referer", "user-agent"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "59781a5b-3903-41f3-afcb-af62929ccde1",
        name: "Managed-CORS-CustomOrigin",
        comment: "Policy to forward the Origin header, for CORS requests to a custom origin",
        header_behavior: "whitelist",
        headers: &["Origin"],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "88a5eaf4-2fd4-4709-b370-b4c650ea3fcf",
        name: "Managed-CORS-S3Origin",
        comment: "Policy to forward the CORS request headers, for CORS requests to an \
                  Amazon S3 origin",
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "none",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "216adef6-5c7f-47e4-b989-5492eafa07d3",
        name: "Managed-AllViewer",
        comment: "Policy to forward all parameters in viewer requests",
        header_behavior: "allViewer",
        headers: &[],
        cookie_behavior: "all",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "33f36d7e-f396-46d9-90e0-52428a34d9dc",
        name: "Managed-AllViewerAndCloudFrontHeaders-2022-06",
        comment: "Policy to forward all parameters in viewer requests and all CloudFront \
                  headers added through 2022-06",
        header_behavior: "allViewerAndWhitelistCloudFront",
        headers: &[
            "CloudFront-Forwarded-Proto",
            "CloudFront-Is-Android-Viewer",
            "CloudFront-Is-Desktop-Viewer",
            "CloudFront-Is-IOS-Viewer",
            "CloudFront-Is-Mobile-Viewer",
            "CloudFront-Is-SmartTV-Viewer",
            "CloudFront-Is-Tablet-Viewer",
            "CloudFront-Viewer-ASN",
            "CloudFront-Viewer-Address",
            "CloudFront-Viewer-City",
            "CloudFront-Viewer-Country",
            "CloudFront-Viewer-Country-Name",
            "CloudFront-Viewer-Country-Region",
            "CloudFront-Viewer-Country-Region-Name",
            "CloudFront-Viewer-Http-Version",
            "CloudFront-Viewer-Latitude",
            "CloudFront-Viewer-Longitude",
            "CloudFront-Viewer-Metro-Code",
            "CloudFront-Viewer-Postal-Code",
            "CloudFront-Viewer-TLS",
            "CloudFront-Viewer-Time-Zone",
        ],
        cookie_behavior: "all",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "775133bc-15f2-49f9-abea-afb2e0bf67d2",
        name: "Managed-Elemental-MediaTailor-PersonalizedManifests",
        comment: "Policy for an AWS Elemental MediaTailor origin serving personalized \
                  manifests",
        header_behavior: "whitelist",
        headers: &[
            "Origin",
            "Access-Control-Request-Headers",
            "Access-Control-Request-Method",
            "User-Agent",
            "X-Forwarded-For",
        ],
        cookie_behavior: "none",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
    ManagedOriginRequestPolicy {
        id: "b689b0a8-53d0-40ab-baf2-68738e2966ac",
        name: "Managed-AllViewerExceptHostHeader",
        comment: "Policy to forward all parameters in viewer requests except for the Host \
                  header",
        header_behavior: "allExcept",
        headers: &["host"],
        cookie_behavior: "all",
        cookies: &[],
        query_string_behavior: "all",
        query_strings: &[],
    },
];

/// The managed origin request policy `id` names, or `None` when the catalog holds none —
/// what tells an id AWS published from one this account minted.
pub fn managed_origin_request_policy(id: &str) -> Option<&'static ManagedOriginRequestPolicy> {
    MANAGED_ORIGIN_REQUEST_POLICIES
        .iter()
        .find(|managed| managed.id == id)
}

/// Where [`MANAGED_ORIGIN_REQUEST_POLICIES`] lists the policy `id`, or `None` for an id the
/// catalog does not hold — the rank a listing orders a published origin request policy by
/// ([`policy_listing_key`]).
pub fn managed_origin_request_policy_rank(id: &str) -> Option<usize> {
    MANAGED_ORIGIN_REQUEST_POLICIES
        .iter()
        .position(|managed| managed.id == id)
}

/// The `ResponseHeadersPolicyType` a policy this ACCOUNT created carries — the custom
/// policies "created in your Amazon Web Services account". One of the two values the
/// model's `ResponseHeadersPolicyType` enum admits.
pub const RESPONSE_HEADERS_POLICY_TYPE_CUSTOM: &str = "custom";

/// The `ResponseHeadersPolicyType` a policy AWS created carries — the managed policies
/// "created by Amazon Web Services". The other value the same enum admits.
pub const RESPONSE_HEADERS_POLICY_TYPE_MANAGED: &str = "managed";

/// The CORS settings a published response headers policy carries, when it carries any.
///
/// Every member here is one of `ResponseHeadersPolicyCorsConfig`'s; a policy that states no
/// CORS behaviour at all carries no `CorsConfig` element rather than an empty one.
#[derive(Debug, Clone)]
pub struct ManagedCorsSettings {
    /// `AccessControlAllowOrigins$Items` — the origins the policy allows.
    pub allow_origins: &'static [&'static str],
    /// `AccessControlAllowHeaders$Items` — the request headers the policy allows.
    pub allow_headers: &'static [&'static str],
    /// `AccessControlAllowMethods$Items` — the methods the policy allows, each a value of
    /// `ResponseHeadersPolicyAccessControlAllowMethodsValues`.
    pub allow_methods: &'static [&'static str],
    /// `AccessControlExposeHeaders$Items` — the response headers the policy exposes to
    /// script. A published CORS policy carries the element whether or not it names a header,
    /// so an empty list here is an element with `Quantity` 0 and an empty `Items` rather
    /// than no element at all.
    pub expose_headers: &'static [&'static str],
    /// `AccessControlAllowCredentials`.
    pub allow_credentials: bool,
    /// `AccessControlMaxAgeSec`, in seconds, for a policy that answers preflight requests.
    pub max_age_seconds: Option<i32>,
    /// `OriginOverride` — whether CloudFront overrides a header the origin already set.
    pub origin_override: bool,
}

/// One entry of the catalog of AWS-MANAGED response headers policies.
///
/// The same reference data [`ManagedCachePolicy`] holds for the cache policy family, for
/// the response headers policy family: AWS creates these policies, every account has the
/// same ones under the identifier and name AWS publishes for each, and nothing a caller
/// does here creates, changes or removes one.
///
/// Each entry's identifier, name and settings are the ones Amazon publishes in the
/// CloudFront Developer Guide ("Using the managed response headers policies").
#[derive(Debug, Clone)]
pub struct ManagedResponseHeadersPolicy {
    /// `ResponseHeadersPolicy$Id` — the identifier AWS publishes for this policy, the same
    /// in every account.
    pub id: &'static str,
    /// `ResponseHeadersPolicyConfig$Name` — the name AWS publishes for this policy.
    pub name: &'static str,
    /// `ResponseHeadersPolicyConfig$Comment` — what the policy is for.
    pub comment: &'static str,
    /// The policy's `CorsConfig`, or `None` for a policy that states no CORS behaviour.
    pub cors: Option<ManagedCorsSettings>,
    /// Whether the policy carries the published security headers
    /// ([`managed_security_headers_config`]). The published policies that add security
    /// headers all add the SAME set, so the set is one constant here rather than repeated
    /// per entry.
    pub security_headers: bool,
}

impl ManagedResponseHeadersPolicy {
    /// The `ResponseHeadersPolicy` projection a read of this published policy reports.
    pub fn to_response_headers_policy(&self) -> ResponseHeadersPolicy {
        self.to_stored().to_response_headers_policy()
    }

    /// This catalog entry as a response headers policy RECORD, so every read of a response
    /// headers policy answers a published policy exactly as it answers one this account
    /// created.
    pub fn to_stored(&self) -> StoredResponseHeadersPolicy {
        StoredResponseHeadersPolicy {
            id: self.id.to_string(),
            config: self.to_config(),
            last_modified_time: managed_policy_last_modified(),
            etag: self.etag(),
        }
    }

    /// The version token a read of this published policy reports, by the one rule every
    /// published policy's version follows ([`managed_policy_etag`]).
    pub fn etag(&self) -> String {
        managed_policy_etag(self.id)
    }

    /// The configuration AWS published for this policy.
    fn to_config(&self) -> ResponseHeadersPolicyConfig {
        ResponseHeadersPolicyConfig {
            comment: Some(self.comment.to_string()),
            name: self.name.to_string(),
            cors_config: self.cors.as_ref().map(|cors| {
                crate::ops::types::ResponseHeadersPolicyCorsConfig {
                    access_control_allow_origins:
                        crate::ops::types::ResponseHeadersPolicyAccessControlAllowOrigins {
                            quantity: i32::try_from(cors.allow_origins.len()).unwrap_or(i32::MAX),
                            items: cors
                                .allow_origins
                                .iter()
                                .map(|origin| (*origin).to_string())
                                .collect(),
                        },
                    access_control_allow_headers:
                        crate::ops::types::ResponseHeadersPolicyAccessControlAllowHeaders {
                            quantity: i32::try_from(cors.allow_headers.len()).unwrap_or(i32::MAX),
                            items: cors
                                .allow_headers
                                .iter()
                                .map(|header| (*header).to_string())
                                .collect(),
                        },
                    access_control_allow_methods:
                        crate::ops::types::ResponseHeadersPolicyAccessControlAllowMethods {
                            quantity: i32::try_from(cors.allow_methods.len()).unwrap_or(i32::MAX),
                            items: cors
                                .allow_methods
                                .iter()
                                .map(|method| (*method).to_string())
                                .collect(),
                        },
                    access_control_allow_credentials: cors.allow_credentials,
                    // A published CORS policy states its exposed-header list even when the
                    // list is empty, so the element is always serialized and carries the
                    // list's own length beside the list itself.
                    access_control_expose_headers: Some(
                        crate::ops::types::ResponseHeadersPolicyAccessControlExposeHeaders {
                            quantity: i32::try_from(cors.expose_headers.len())
                                .unwrap_or(i32::MAX),
                            items: Some(
                                cors.expose_headers
                                    .iter()
                                    .map(|header| (*header).to_string())
                                    .collect(),
                            ),
                        },
                    ),
                    access_control_max_age_sec: cors.max_age_seconds,
                    origin_override: cors.origin_override,
                }
            }),
            security_headers_config: if self.security_headers {
                Some(managed_security_headers_config().into())
            } else {
                None
            },
            // The published policies state no server-timing, custom or removed headers, so
            // those elements are absent rather than empty.
            server_timing_headers_config: None,
            custom_headers_config: None,
            remove_headers_config: None,
        }
    }
}

/// The security headers every published policy that adds them adds: the set Amazon
/// publishes for `Managed-SecurityHeadersPolicy`, which the combined CORS-and-security
/// policies carry unchanged.
pub fn managed_security_headers_config() -> crate::ops::types::ResponseHeadersPolicySecurityHeadersConfig
{
    crate::ops::types::ResponseHeadersPolicySecurityHeadersConfig {
        xss_protection: Some(crate::ops::types::ResponseHeadersPolicyXSSProtection {
            r#override: false,
            protection: true,
            mode_block: Some(true),
            report_uri: None,
        }),
        frame_options: Some(crate::ops::types::ResponseHeadersPolicyFrameOptions {
            r#override: false,
            frame_option: "SAMEORIGIN".to_string(),
        }),
        referrer_policy: Some(crate::ops::types::ResponseHeadersPolicyReferrerPolicy {
            r#override: false,
            referrer_policy: "strict-origin-when-cross-origin".to_string(),
        }),
        // The published policy states no Content-Security-Policy of its own: the directives
        // belong to the application, so AWS leaves that header to a policy the account
        // writes.
        content_security_policy: None,
        content_type_options: Some(crate::ops::types::ResponseHeadersPolicyContentTypeOptions {
            r#override: false,
        }),
        strict_transport_security: Some(
            crate::ops::types::ResponseHeadersPolicyStrictTransportSecurity {
                r#override: false,
                include_subdomains: Some(true),
                preload: Some(false),
                access_control_max_age_sec: MANAGED_STRICT_TRANSPORT_SECURITY_MAX_AGE_SECONDS,
            },
        ),
    }
}

/// `Strict-Transport-Security`'s `max-age` in the published security headers: one year, the
/// value Amazon publishes for `Managed-SecurityHeadersPolicy`.
const MANAGED_STRICT_TRANSPORT_SECURITY_MAX_AGE_SECONDS: i32 = 31_536_000;

/// `Access-Control-Max-Age` in the published policies that answer preflight requests, the
/// value Amazon publishes for them.
const MANAGED_CORS_PREFLIGHT_MAX_AGE_SECONDS: i32 = 600;

/// The CORS settings the published policies that allow a SIMPLE cross-origin request carry:
/// every origin, and no list of its own for the request headers, the methods or the exposed
/// headers — the behaviour those lists would narrow is the published policy's own, so each
/// list is served empty, and a published policy leaves a header the origin already set
/// alone.
const MANAGED_SIMPLE_CORS: ManagedCorsSettings = ManagedCorsSettings {
    allow_origins: &["*"],
    allow_headers: &[],
    allow_methods: &[],
    expose_headers: &[],
    allow_credentials: false,
    max_age_seconds: None,
    origin_override: false,
};

/// The CORS settings the published policies that also answer PREFLIGHT requests carry: the
/// simple settings plus what a preflight answer adds — the methods it names, every exposed
/// response header, and the published `Access-Control-Max-Age`.
///
/// The methods are every value `AccessControlAllowMethods$Items` admits beside the `ALL`
/// shorthand (`smithy_query(query="shape",
/// shape="ResponseHeadersPolicyAccessControlAllowMethodsValues")`), in the order the
/// published policy states them — a published policy states the methods themselves rather
/// than the shorthand.
const MANAGED_PREFLIGHT_CORS: ManagedCorsSettings = ManagedCorsSettings {
    allow_origins: &["*"],
    allow_headers: &[],
    allow_methods: &["GET", "HEAD", "PUT", "POST", "PATCH", "DELETE", "OPTIONS"],
    expose_headers: &["*"],
    allow_credentials: false,
    max_age_seconds: Some(MANAGED_CORS_PREFLIGHT_MAX_AGE_SECONDS),
    origin_override: false,
};

/// The AWS-managed response headers policies, in the order Amazon PUBLISHES them rather
/// than the order their ids sort in — the order every listing of this service serves them
/// in, ahead of the policies this account created ([`policy_listing_key`]).
///
/// Each entry's identifier, name and settings are the ones Amazon publishes for it, and the
/// entries stand in the order that same list presents them (see
/// [`ManagedResponseHeadersPolicy`] for the source).
pub const MANAGED_RESPONSE_HEADERS_POLICIES: &[ManagedResponseHeadersPolicy] = &[
    ManagedResponseHeadersPolicy {
        id: "60669652-455b-4ae9-85a4-c4c02393f86c",
        name: "Managed-SimpleCORS",
        comment: "Allows all origins for simple CORS requests",
        cors: Some(MANAGED_SIMPLE_CORS),
        security_headers: false,
    },
    ManagedResponseHeadersPolicy {
        id: "5cc3b908-e619-4b99-88e5-2cf7f45965bd",
        name: "Managed-CORS-With-Preflight",
        comment: "Allows all origins for CORS requests, including preflight requests",
        cors: Some(MANAGED_PREFLIGHT_CORS),
        security_headers: false,
    },
    ManagedResponseHeadersPolicy {
        id: "eaab4381-ed33-4a86-88ca-d9558dc6cd63",
        name: "Managed-CORS-with-preflight-and-SecurityHeadersPolicy",
        comment: "Allows all origins for CORS requests, including preflight requests, and \
                  adds a set of security headers to every response",
        cors: Some(MANAGED_PREFLIGHT_CORS),
        security_headers: true,
    },
    ManagedResponseHeadersPolicy {
        id: "e61eb345-f6c1-4b97-f0d2-0bdcb2b0b6d3",
        name: "Managed-CORS-and-SecurityHeadersPolicy",
        comment: "Allows all origins for simple CORS requests, and adds a set of security \
                  headers to every response",
        cors: Some(MANAGED_SIMPLE_CORS),
        security_headers: true,
    },
    ManagedResponseHeadersPolicy {
        id: "67f7725c-6f97-4210-82d7-5512b31e9d03",
        name: "Managed-SecurityHeadersPolicy",
        comment: "Adds a set of security headers to every response",
        cors: None,
        security_headers: true,
    },
];

/// The managed response headers policy `id` names, or `None` when the catalog holds none —
/// what tells an id AWS published from one this account minted.
pub fn managed_response_headers_policy(
    id: &str,
) -> Option<&'static ManagedResponseHeadersPolicy> {
    MANAGED_RESPONSE_HEADERS_POLICIES
        .iter()
        .find(|managed| managed.id == id)
}

/// Where [`MANAGED_RESPONSE_HEADERS_POLICIES`] lists the policy `id`, or `None` for an id
/// the catalog does not hold — the rank a listing orders a published response headers
/// policy by ([`policy_listing_key`]).
pub fn managed_response_headers_policy_rank(id: &str) -> Option<usize> {
    MANAGED_RESPONSE_HEADERS_POLICIES
        .iter()
        .position(|managed| managed.id == id)
}

/// One CloudFront public key, as this emulator persists it.
///
/// The record is the single source of truth for the key's id, the configuration it was
/// uploaded with, the instant it was uploaded and its version token, so the create and
/// every later read — `GetPublicKey` and every `ListPublicKeys` page — answer from it
/// rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `PublicKey` is `Id`, `CreatedTime`
/// and `PublicKeyConfig` only — a public key carries no status and no propagation
/// deadline, so a key this service accepts is immediately complete.
#[derive(Debug, Clone)]
pub struct StoredPublicKey {
    /// `PublicKey$Id` — "The identifier of the public key": the token minted at creation,
    /// which is also the key this record is held under and the path segment its
    /// `Location` names.
    pub id: String,
    /// `PublicKey$CreatedTime` — "The date and time when the public key was uploaded."
    /// Written once, by the create, so every later read reports the same instant.
    pub created_time: SystemTime,
    /// The configuration the key is held under — stored RESOLVED, so a read reports what
    /// was stored rather than re-deriving anything from a request: a member the create left
    /// out carries the value CloudFront supplies for it ([`resolve_public_key_config`]).
    pub config: PublicKeyConfig,
    /// The version token the last mutation handed back, and the single source of truth
    /// for every `If-Match` check against this public key.
    pub etag: String,
}

impl StoredPublicKey {
    /// The `PublicKey` projection every public key operation returns, read from the one
    /// stored record.
    pub fn to_public_key(&self) -> PublicKey {
        PublicKey {
            id: self.id.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            public_key_config: self.config.clone(),
        }
    }

    /// The `PublicKeySummary` a listing reports for this key.
    ///
    /// Every member is the stored record's: `PublicKeySummary` repeats the configuration's
    /// `Name`, `EncodedKey` and `Comment` beside the identity the create minted, so the
    /// summary reports the `Comment` the stored configuration carries and the read invents
    /// nothing of its own.
    pub fn to_summary(&self) -> PublicKeySummary {
        PublicKeySummary {
            id: self.id.clone(),
            name: self.config.name.clone(),
            created_time: ::aws_smithy_types::DateTime::from(self.created_time),
            encoded_key: self.config.encoded_key.clone(),
            comment: self.config.comment.clone(),
        }
    }
}

/// One CloudFront key group, as this emulator persists it.
///
/// "A key group contains a list of public keys that you can use with CloudFront signed URLs
/// and signed cookies." The record is the single source of truth for the group's id, the
/// configuration it was created with, the instant it was last written and its version
/// token, so the create and every later read — `GetKeyGroup` above all — answer from it
/// rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `KeyGroup` is `Id`,
/// `LastModifiedTime` and `KeyGroupConfig` only — a key group carries no status and no
/// propagation deadline, so one this service accepts is usable at once.
#[derive(Debug, Clone)]
pub struct StoredKeyGroup {
    /// `KeyGroup$Id` — "The identifier for the key group": the token minted at creation,
    /// which is also the key this record is held under and the path segment its `Location`
    /// names.
    pub id: String,
    /// The configuration as submitted — "A key group configuration." Stored whole, so a
    /// read reports what was stored rather than re-deriving anything from a request, down
    /// to the members the create left out.
    pub config: KeyGroupConfig,
    /// `KeyGroup$LastModifiedTime` — "The date and time when the key group was last
    /// modified." Written by the call that last wrote the record, so every later read
    /// reports the same instant.
    pub last_modified_time: SystemTime,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this key group.
    pub etag: String,
}

impl StoredKeyGroup {
    /// The `KeyGroup` projection every key group operation returns, read from the one
    /// stored record.
    pub fn to_key_group(&self) -> KeyGroup {
        KeyGroup {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            key_group_config: self.config.clone(),
        }
    }
}

/// One CloudFront field-level encryption profile, as this emulator persists it.
///
/// The record is the single source of truth for the profile's id, the configuration it was
/// created with, the instant it was last written and its version token, so the create and
/// every later read — `GetFieldLevelEncryptionProfile` and
/// `GetFieldLevelEncryptionProfileConfig` — answer from it rather than from the request
/// being served.
///
/// Nothing in here is owed by the CLOCK: the model's `FieldLevelEncryptionProfile` is `Id`,
/// `LastModifiedTime` and `FieldLevelEncryptionProfileConfig` only — a profile carries no
/// status and no propagation deadline, so one this service accepts is usable at once.
#[derive(Debug, Clone)]
pub struct StoredFieldLevelEncryptionProfile {
    /// `FieldLevelEncryptionProfile$Id` — "The ID for a field-level encryption profile
    /// configuration": the token minted at creation, which is also the key this record is
    /// held under and the path segment its `Location` names.
    pub id: String,
    /// `FieldLevelEncryptionProfile$LastModifiedTime` — "The last time the field-level
    /// encryption profile was updated", which for a profile nothing has updated yet is the
    /// instant the create stored it.
    pub last_modified_time: SystemTime,
    /// The configuration the profile runs under — stored RESOLVED, so a read reports what
    /// was stored rather than re-deriving anything from a request: a member the create left
    /// out carries the value CloudFront supplies for it
    /// ([`resolve_field_level_encryption_profile_config`]).
    pub config: FieldLevelEncryptionProfileConfig,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this profile.
    pub etag: String,
}

impl StoredFieldLevelEncryptionProfile {
    /// The `FieldLevelEncryptionProfile` projection every profile operation returns, read
    /// from the one stored record.
    pub fn to_profile(&self) -> FieldLevelEncryptionProfile {
        FieldLevelEncryptionProfile {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            field_level_encryption_profile_config: self.config.clone(),
        }
    }

    /// The `FieldLevelEncryptionProfileSummary` a listing reports for this profile.
    ///
    /// Every member is the stored record's: the summary repeats the configuration's `Name`,
    /// `EncryptionEntities` and `Comment` beside the identity the create minted and the
    /// instant that record was last written, so the summary reports the `Comment` the
    /// stored configuration carries and the read invents nothing of its own.
    pub fn to_summary(&self) -> FieldLevelEncryptionProfileSummary {
        FieldLevelEncryptionProfileSummary {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            name: self.config.name.clone(),
            encryption_entities: self.config.encryption_entities.clone(),
            comment: self.config.comment.clone(),
        }
    }
}

/// One CloudFront field-level encryption CONFIGURATION, as this emulator persists it.
///
/// The record is the single source of truth for the configuration's id, the configuration it
/// was created with, the instant it was last written and its version token, so the create and
/// every later read — `GetFieldLevelEncryption` and `GetFieldLevelEncryptionConfig` — answer
/// from it rather than from the request being served.
///
/// Nothing in here is owed by the CLOCK: the model's `FieldLevelEncryption` is `Id`,
/// `LastModifiedTime` and `FieldLevelEncryptionConfig` only — a configuration carries no
/// status and no propagation deadline, so one this service accepts is usable at once.
#[derive(Debug, Clone)]
pub struct StoredFieldLevelEncryptionConfig {
    /// `FieldLevelEncryption$Id` — "The configuration ID for a field-level encryption
    /// configuration": the token minted at creation, which is also the key this record is
    /// held under and the path segment its `Location` names.
    pub id: String,
    /// `FieldLevelEncryption$LastModifiedTime` — "The last time the field-level encryption
    /// configuration was changed", which for a configuration nothing has changed yet is the
    /// instant the create stored it.
    pub last_modified_time: SystemTime,
    /// The configuration the resource runs under — stored RESOLVED, so a read reports what
    /// was stored rather than re-deriving anything from a request: a member the create left
    /// out carries the value CloudFront supplies for it
    /// ([`resolve_field_level_encryption_config`]).
    pub config: FieldLevelEncryptionConfig,
    /// The version token the last mutation handed back, and the single source of truth for
    /// every `If-Match` check against this configuration.
    pub etag: String,
}

impl StoredFieldLevelEncryptionConfig {
    /// The `FieldLevelEncryption` projection every configuration operation returns, read
    /// from the one stored record.
    pub fn to_field_level_encryption(&self) -> FieldLevelEncryption {
        FieldLevelEncryption {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            field_level_encryption_config: self.config.clone(),
        }
    }

    /// The `FieldLevelEncryptionSummary` a listing reports for this configuration.
    ///
    /// Every member is the stored record's: the summary repeats the stored configuration's
    /// `Comment`, `QueryArgProfileConfig` and `ContentTypeProfileConfig` beside the identity
    /// the create minted, so the summary reports what the stored configuration carries —
    /// the members the service resolved into it included — and the read invents nothing of
    /// its own. `CallerReference` is not a summary member, so it is not reported here.
    pub fn to_summary(&self) -> FieldLevelEncryptionSummary {
        FieldLevelEncryptionSummary {
            id: self.id.clone(),
            last_modified_time: ::aws_smithy_types::DateTime::from(self.last_modified_time),
            comment: self.config.comment.clone(),
            query_arg_profile_config: self.config.query_arg_profile_config.clone(),
            content_type_profile_config: self.config.content_type_profile_config.clone(),
        }
    }
}

/// Per-service state for com-amazonaws-cloudfront-cloudfront2020-05-31. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsCloudfrontCloudfront20200531State {
    /// Every distribution this emulator holds, keyed by distribution id. A `BTreeMap` is
    /// what gives `ListDistributions` the stable total order its `Marker` indexes into.
    pub distributions: BTreeMap<String, StoredDistribution>,
    /// `DistributionConfig$CallerReference` -> distribution id, so a caller reference
    /// already associated with a distribution is detected without scanning the store.
    pub caller_references: HashMap<String, String>,
    /// The listener each distribution's EDGE is served on, keyed by distribution id: the
    /// address it published as its `DomainName` and the handle that keeps the port open
    /// ([`EdgeEndpoint`]). A distribution that serves no traffic of its own — a multi-tenant
    /// one — has no entry, and dropping an entry (the delete) closes that port.
    pub edge_endpoints: BTreeMap<String, EdgeEndpoint>,
    /// The counter every generated id, domain name and version token is minted from, so
    /// no two mints collide and each mutation yields a FRESH `ETag`.
    pub next_token_seq: u64,
    /// Every key value store this emulator holds, keyed by its name — "In your account,
    /// the key value store names must be unique", which is what makes the name the key
    /// and a repeated name an `EntityAlreadyExists`. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through.
    pub key_value_stores: BTreeMap<String, StoredKeyValueStore>,
    /// Every Anycast static IP list this emulator holds, keyed by its minted id — the
    /// identifier every Anycast static IP list operation addresses one by, and the id a
    /// connection group's `AnycastIpListId` resolves against
    /// ([`ComAmazonawsCloudfrontCloudfront20200531State::holds_anycast_ip_list`]). A
    /// `BTreeMap` for the same reason as `distributions`: one stable total order for
    /// `ListAnycastIpLists`'s `Marker` to index into.
    pub anycast_ip_lists: BTreeMap<String, StoredAnycastIpList>,
    /// The host number of the next Anycast static IP this store allocates, within the
    /// blocks [`ANYCAST_IPV4_PREFIX`] and [`ANYCAST_IPV6_PREFIX`] name. One counter for
    /// both families, so no two addresses this store allocates — of either family — are
    /// ever rendered from the same number, and the addresses of a list are stable for its
    /// whole life because nothing ever re-allocates them.
    pub next_anycast_ip: u32,
    /// Every continuous deployment policy this emulator holds, keyed by its id — the
    /// identifier `GetContinuousDeploymentPolicy` addresses one by. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for
    /// `ListContinuousDeploymentPolicies`'s `Marker` to index into.
    pub continuous_deployment_policies: BTreeMap<String, StoredContinuousDeploymentPolicy>,
    /// Every distribution tenant this emulator holds, keyed by tenant id. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order a listing pages
    /// through.
    pub distribution_tenants: BTreeMap<String, StoredDistributionTenant>,
    /// Every connection group this emulator holds, keyed by connection group id — the
    /// identifier every connection group operation addresses one by. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for
    /// `ListConnectionGroups`'s `Marker` to index into.
    ///
    /// At most ONE entry carries [`StoredConnectionGroup::is_default`]: "When you first
    /// create a distribution tenant and you don't specify a connection group, CloudFront
    /// will automatically create a default connection group for you" — so that one is
    /// created ONCE, by the first call that needs it, and every later tenant that names no
    /// connection group is routed through it.
    pub connection_groups: BTreeMap<String, StoredConnectionGroup>,
    /// Every origin access identity this emulator holds, keyed by its id. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_access_identities: BTreeMap<String, StoredOriginAccessIdentity>,
    /// Every response headers policy this emulator holds, keyed by its minted id — the
    /// identifier every response headers policy operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub response_headers_policies: BTreeMap<String, StoredResponseHeadersPolicy>,
    /// Every CloudFront function this emulator holds, keyed by its name — "A function
    /// with the same name already exists in this Amazon Web Services account" is what
    /// makes the name the key and a repeated name a `FunctionAlreadyExists`. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through. Everything that reads or writes it lives in [`functions`].
    pub functions: BTreeMap<String, StoredFunction>,
    /// Every origin access control this emulator holds, keyed by its minted id — the
    /// identifier every origin access control operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_access_controls: BTreeMap<String, StoredOriginAccessControl>,
    /// Every origin request policy this emulator holds, keyed by its minted id — the
    /// identifier every origin request policy operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for a listing to
    /// page through.
    pub origin_request_policies: BTreeMap<String, StoredOriginRequestPolicy>,
    /// Every cache policy this emulator holds, keyed by its minted id — the identifier
    /// every cache policy operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through.
    pub cache_policies: BTreeMap<String, StoredCachePolicy>,
    /// Every connection function this emulator holds, keyed by its minted id — the
    /// identifier every mutating connection function operation addresses one by (its name
    /// and its ARN resolve to the same record). A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for a listing to page through. Everything
    /// that reads or writes it lives in [`connection_functions`].
    pub connection_functions: BTreeMap<String, StoredConnectionFunction>,
    /// Every public key this emulator holds, keyed by its minted id — the identifier every
    /// public key operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for `ListPublicKeys`'s `Marker` to index
    /// into.
    pub public_keys: BTreeMap<String, StoredPublicKey>,
    /// Every key group this emulator holds, keyed by its minted id — the identifier every
    /// key group operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for `ListKeyGroups`'s `Marker` to index
    /// into.
    pub key_groups: BTreeMap<String, StoredKeyGroup>,
    /// Every field-level encryption profile this emulator holds, keyed by its minted id —
    /// the identifier every field-level encryption profile operation addresses one by. A
    /// `BTreeMap` for the same reason as `distributions`: one stable total order for
    /// `ListFieldLevelEncryptionProfiles`'s `Marker` to index into.
    pub field_level_encryption_profiles: BTreeMap<String, StoredFieldLevelEncryptionProfile>,
    /// Every field-level encryption configuration this emulator holds, keyed by its minted
    /// id — the identifier every field-level encryption configuration operation addresses
    /// one by. A `BTreeMap` for the same reason as `distributions`: one stable total order
    /// for `ListFieldLevelEncryptionConfigs`'s `Marker` to index into.
    pub field_level_encryption_configs: BTreeMap<String, StoredFieldLevelEncryptionConfig>,
    /// Every streaming (RTMP) distribution this emulator holds, keyed by its id — the
    /// identifier every streaming distribution operation addresses one by. A `BTreeMap`
    /// for the same reason as `distributions`: one stable total order for
    /// `ListStreamingDistributions`'s `Marker` to index into.
    pub streaming_distributions: BTreeMap<String, StoredStreamingDistribution>,
    /// Every real-time log configuration this emulator holds, keyed by its name — "The
    /// unique name of this real-time log configuration", which is what makes the name the
    /// key and a repeated name a `RealtimeLogConfigAlreadyExists`. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order for a listing to page
    /// through. Everything that reads or writes it lives in [`realtime_log_configs`].
    pub realtime_log_configs: BTreeMap<String, StoredRealtimeLogConfig>,
    /// Every trust store this emulator holds, keyed by its minted id — the identifier every
    /// trust store operation addresses one by (its name and its ARN resolve to the same
    /// record). A `BTreeMap` for the same reason as `distributions`: one stable total order
    /// for `ListTrustStores`'s `Marker` to index into. Everything that reads or writes it
    /// lives in [`trust_stores`].
    pub trust_stores: BTreeMap<String, StoredTrustStore>,
    /// The resource policy of every resource that carries one, keyed by the ARN of the
    /// resource it is attached to — the one identifier the resource-policy surface
    /// addresses a policy by, in all three of its operations, so a policy written under an
    /// ARN is the policy every later read of that ARN answers from. A `BTreeMap` for the
    /// same reason as `distributions`: one stable total order.
    ///
    /// The value is the policy document exactly as it was submitted: the surface stores a
    /// document, never a re-derivation of one, so what a read reports is byte for byte what
    /// the last write persisted.
    pub resource_policies: BTreeMap<String, String>,
    /// Every VPC origin this emulator holds, keyed by its minted id — the identifier every
    /// VPC origin operation addresses one by. A `BTreeMap` for the same reason as
    /// `distributions`: one stable total order for `ListVpcOrigins`'s `Marker` to index
    /// into. Everything that reads or writes it lives in [`vpc_origins`].
    pub vpc_origins: BTreeMap<String, StoredVpcOrigin>,
}

// ---------------------------------------------------------------------------
// Resolving a submitted configuration into the one the distribution runs under
// ---------------------------------------------------------------------------

/// Resolve the members the service supplies on a cache behaviour.
///
/// `DefaultCacheBehavior` and `CacheBehavior` are two Smithy structures declaring the SAME
/// members, so ONE macro resolves both — two copies of this walk would be two things to
/// keep in step.
///
/// `$multi_tenant` says which KIND of distribution the behaviour belongs to, for the same
/// reason [`resolve_distribution_config`] reads it: a member documented as supported only
/// on a standard distribution is no member of a multi-tenant one's configuration, so the
/// service supplies no default for it there either.
macro_rules! resolve_behavior_members {
    ($behavior:expr, $multi_tenant:expr) => {{
        let mut behavior = $behavior;
        let multi_tenant: bool = $multi_tenant;
        // An absent trusted-key-group wrapper is a behaviour that requires no signed URL
        // or cookie: not enabled, and nobody in the list. `TrustedSigners` says the same
        // thing for the deprecated account-based form, and it is one of the members only a
        // standard distribution has, so only a standard behaviour gets it.
        if !multi_tenant {
            behavior.trusted_signers = Some(behavior.trusted_signers.unwrap_or_default());
        }
        behavior.trusted_key_groups = Some(behavior.trusted_key_groups.unwrap_or_default());
        behavior.allowed_methods = Some(resolve_allowed_methods(behavior.allowed_methods));
        if !multi_tenant {
            behavior.smooth_streaming =
                Some(behavior.smooth_streaming.unwrap_or(DEFAULT_SMOOTH_STREAMING));
        }
        behavior.compress = Some(behavior.compress.unwrap_or(DEFAULT_COMPRESS));
        behavior.lambda_function_associations =
            Some(behavior.lambda_function_associations.unwrap_or_default());
        behavior.function_associations = Some(behavior.function_associations.unwrap_or_default());
        behavior.field_level_encryption_id =
            Some(behavior.field_level_encryption_id.unwrap_or_default());
        // "The gRPC configuration for your cache behavior" is off unless it is asked for.
        behavior.grpc_config = Some(behavior.grpc_config.unwrap_or_default());
        // The legacy cache-key members travel as ONE group: a behaviour that states its
        // cache key with `ForwardedValues` also gets the three TTLs resolved, while one
        // that attaches a cache policy states none of them and gets none. The three TTLs
        // are standard-only members as well, so only a standard behaviour has them
        // supplied.
        if let Some(forwarded) = behavior.forwarded_values {
            behavior.forwarded_values = Some(resolve_forwarded_values(forwarded));
            if !multi_tenant {
                behavior.min_ttl = Some(behavior.min_ttl.unwrap_or(DEFAULT_MIN_TTL));
                behavior.default_ttl = Some(behavior.default_ttl.unwrap_or(DEFAULT_DEFAULT_TTL));
                behavior.max_ttl = Some(behavior.max_ttl.unwrap_or(DEFAULT_MAX_TTL));
            }
        }
        behavior
    }};
}

/// The unit tests of the records and helpers this module holds.
#[cfg(test)]
mod tests;

/// The unit tests of the page-size clamp above ([`page_size_at`]), in their own file
/// because that clamp is what bounds the size of every response this service serves.
#[cfg(test)]
mod paging;

/// Resolving a SUBMITTED configuration into the one the service stores — every member
/// CloudFront materialises when a request leaves it out — and the admission rules a
/// submitted configuration is checked against: the quantity agreements, the per-list
/// quotas and the sentences each refusal states. A SUBMODULE for the same reason
/// [`functions`] is one; `pub use` keeps every name reachable as `crate::state::<name>`.
mod configuration;
pub use configuration::*;

/// How a resource of this service is NAMED and compared: the ARNs, paths and location URIs
/// one is addressed by, the predicates that say whether a value is an id or a version token
/// of a given kind, the `If-Match` check, and the equality a submitted value is compared
/// with a stored one by. A SUBMODULE for the same reason [`functions`] is one; `pub use`
/// keeps every name reachable as `crate::state::<name>`.
mod identity;
pub use identity::*;

/// The reads over the DISTRIBUTION COLLECTION: what a create is refused with, how a listing
/// walks the distributions in the one order they are served in, and which stored
/// configurations reference a given policy, key group or web ACL. A SUBMODULE for the same
/// reason [`functions`] is one; `pub use` keeps every name reachable as
/// `crate::state::<name>`.
mod distribution_queries;
pub use distribution_queries::*;

/// The store's own API: every lookup, allocation, mutation and clock settlement the
/// handlers reach [`ComAmazonawsCloudfrontCloudfront20200531State`] through. The struct and
/// its maps stay in this file; the inherent methods over them live in the submodule, so
/// that surface grows without touching the records themselves. Nothing is re-exported: the
/// methods are reached through the struct.
mod store;

/// The DISTRIBUTION record itself: the invalidations created under one, the responses its
/// edge holds, the path matcher both are read with, and the transitions the clock owes
/// them. A SUBMODULE for the same reason [`functions`] is one; `pub use` keeps every name
/// reachable as `crate::state::<name>`.
mod distributions;
pub use distributions::*;

/// The tenant PARAMETER references a distribution configuration carries: what names one, in
/// which members, and the rule a reference is admitted by. A SUBMODULE for the same reason
/// [`functions`] is one; `pub use` keeps every name reachable as `crate::state::<name>`.
mod tenant_parameters;
pub use tenant_parameters::*;

/// The EXECUTION ROLE behind a Lambda@Edge association: which role Lambda reports for the
/// function version a cache behaviour names, and whether the trust policy this account's IAM
/// holds for that role lets the edge invoke the function under it. A SUBMODULE for the same
/// reason [`functions`] is one; `pub use` keeps every name reachable as
/// `crate::state::<name>`.
mod lambda_edge_execution_role;
pub use lambda_edge_execution_role::*;

/// The ADDRESS a distribution's edge is reached at: the loopback listener the core opens for
/// each distribution that serves traffic of its own, and the leg that hands a viewer request
/// it accepts to this service's own data plane. A SUBMODULE for the same reason [`functions`]
/// is one; `pub use` keeps every name reachable as `crate::state::<name>`.
mod edge_endpoint;
pub use edge_endpoint::*;
