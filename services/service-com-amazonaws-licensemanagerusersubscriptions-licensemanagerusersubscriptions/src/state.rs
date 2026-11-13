// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions service.
//!
//! One store holds every resource the service's operations share: identity-provider
//! registrations, the product users subscribed through them, the EC2 instances that
//! provide those subscriptions, the users associated to those instances, the license
//! server endpoints created for a registration, and the tags written against any of
//! those resources' ARNs. The two fleets this service resolves a request against but
//! does not own — the account's EC2 instances and its AWS Directory Service directories —
//! live here too, each behind its own seed seam; the instances are ALSO resolved against
//! the service that owns them ([`ec2_fleet`]), which is what makes an instance a client
//! really launched a target this service accepts, and so are the subnets and the security
//! group a registration's `Settings` name, and so is the directory an identity-provider
//! registration is for ([`ad_directory_fleet`]), which is what makes a directory a client
//! really created one a registration can be built on. The credentials secret a request's
//! `CredentialsProvider` names is resolved the same way, against the service that owns
//! secrets ([`credentials_secret`]). The
//! list/lookup/pagination/filter helpers live here as well, so the operation handlers in
//! `ops/` all resolve a resource the same way instead of each growing its own copy.

/// Resolving the EC2 resources a request names — the instance it acts on, the network a
/// registration's settings name — against what the account has, through the service that
/// owns them.
pub mod ec2_fleet;

/// Resolving the Secrets Manager secret a request's `CredentialsProvider` names against the
/// secrets the account holds, through the service that owns them.
pub mod credentials_secret;

/// Resolving the AWS Directory Service directory an identity-provider registration names
/// against the directories the account has, through the plane that creates them.
pub mod ad_directory_fleet;

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    CredentialsProvider, Filter, IdentityProvider, IdentityProviderSummary, InstanceSummary,
    InstanceUserSummary, LicenseServer, LicenseServerEndpoint, LicenseServerSettings,
    ProductUserSummary, ServerEndpoint, ServerSettings, Settings, SERVER_TYPE_VALUES,
};

/// The status a registration carries while its VPC endpoints are being provisioned.
pub const STATUS_REGISTERING: &str = "REGISTERING";
/// The status a registration carries once provisioning completed.
pub const STATUS_REGISTERED: &str = "REGISTERED";
/// The status a registration carries from the moment `DeregisterIdentityProvider`
/// accepted its deregistration until the teardown that status announces completes.
pub const STATUS_DEREGISTERING: &str = "DEREGISTERING";
/// The status a product user carries while its subscription is active.
pub const STATUS_SUBSCRIBED: &str = "SUBSCRIBED";
/// The status a product user carries from the moment `StopProductSubscription` accepted
/// the end of its subscription until the teardown that status announces completes.
///
/// Non-terminal: the subscription is REPORTED in it — that is what the status, and the
/// `SubscriptionEndDate` reported beside it, are for — while it is being torn down, and it
/// stops being reported when the teardown falls due; see [`UNSUBSCRIPTION_SPAN`] and
/// [`begin_product_user_unsubscription`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::begin_product_user_unsubscription).
pub const STATUS_UNSUBSCRIBED: &str = "UNSUBSCRIBED";
/// The status a user association carries while the instance is being configured.
pub const STATUS_ASSOCIATING: &str = "ASSOCIATING";
/// The status a user association carries once configuration completed.
pub const STATUS_ASSOCIATED: &str = "ASSOCIATED";
/// The status a user association carries from the moment `DisassociateUser` accepted its
/// disassociation until the teardown that status announces has completed.
///
/// Non-terminal: an association is REPORTED in it (that is what the status is for) while
/// the instance is being reconfigured, and stops being reported when the teardown falls
/// due — see [`DISASSOCIATION_SPAN`] and
/// [`begin_instance_user_disassociation`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::begin_instance_user_disassociation).
pub const STATUS_DISASSOCIATING: &str = "DISASSOCIATING";
/// The status an instance carries in this service's view while at least one user is
/// associated with it.
///
/// `InstanceSummary.Status` is a free-form string in the model (no enum to honour), and
/// the only thing this service knows about an instance is that a user association brought
/// it into view; so the one writer records this one value rather than inventing a
/// per-instance status nothing computes.
pub const STATUS_ACTIVE: &str = "ACTIVE";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries while its network
/// endpoint is being provisioned.
pub const PROVISIONING_STATUS_PROVISIONING: &str = "PROVISIONING";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries once provisioning
/// completed.
pub const PROVISIONING_STATUS_PROVISIONED: &str = "PROVISIONED";
/// The `LicenseServerEndpointProvisioningStatus` an endpoint carries from the moment
/// `DeleteLicenseServerEndpoint` accepted its deletion until that deletion completes.
pub const PROVISIONING_STATUS_DELETING: &str = "DELETING";
/// The terminal `LicenseServerEndpointProvisioningStatus` an endpoint reaches once the
/// deletion that put it in [`PROVISIONING_STATUS_DELETING`] has completed.
pub const PROVISIONING_STATUS_DELETED: &str = "DELETED";
/// The `LicenseServerHealthStatus` a license server carries once it is provisioned and
/// serving.
pub const HEALTH_STATUS_HEALTHY: &str = "HEALTHY";
/// The `LicenseServerHealthStatus` a license server carries while there is no serving
/// host to report a verdict about — before its provisioning completed, and once its
/// teardown has begun.
///
/// The third value the model publishes, `UNHEALTHY`, is a verdict about a host that is
/// failing; nothing here runs a host that can fail, so it is never reported rather than
/// invented.
pub const HEALTH_STATUS_NOT_APPLICABLE: &str = "NOT_APPLICABLE";

/// Whether `status` is a status a user association carries because its disassociation was
/// accepted.
///
/// An association in it is still REPORTED — its instance goes on providing user-based
/// subscriptions in `ListInstances` while the teardown runs — but it is no longer a live
/// resource: it is not taggable ([`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::holds_resource`]),
/// and the [`instance_user_key`] identity it occupies cannot be
/// associated again until the teardown has completed.
pub fn disassociation_started(status: &str) -> bool {
    status == STATUS_DISASSOCIATING
}

/// Whether `status` is a status a product user carries because the end of its subscription
/// was accepted.
///
/// A subscription in it is still REPORTED — `ListProductSubscriptions` keeps reporting it,
/// carrying this status and its end date, so a client polling the list observes the
/// subscription ending — but it is no longer a LIVE subscription: it resolves for no
/// operation that acts on a subscription, it is not taggable
/// ([`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::holds_resource`]),
/// and the [`product_user_key`] identity it occupies cannot be
/// subscribed again until the teardown has completed.
pub fn unsubscribe_started(status: &str) -> bool {
    status == STATUS_UNSUBSCRIBED
}

/// Whether `status` is a registration status a record carries because its deregistration
/// was accepted.
///
/// A registration in it is still REPORTED — that is what answering the non-terminal
/// [`STATUS_DEREGISTERING`] means — but it is no longer a LIVE registration: it resolves
/// for no operation that acts on a registration, it provides no product to an instance
/// view, it is not taggable, and it keeps its `(identity provider, product)` pair taken so
/// a register for that pair conflicts instead of succeeding while the teardown runs.
pub fn deregistration_started(status: &str) -> bool {
    status == STATUS_DEREGISTERING
}

/// Whether `status` is a `LicenseServerEndpointProvisioningStatus` an endpoint carries
/// because its deletion was accepted — `DELETING` while the deletion runs, `DELETED` once
/// it finished.
///
/// An endpoint in either status is still REPORTED (that is the point of modelling the two
/// statuses) but is no longer a live resource: it cannot be addressed by
/// `(IdentityProviderArn, ServerType)`, it is not taggable, and it does not count against
/// the account's endpoint quota.
pub fn endpoint_deletion_started(status: &str) -> bool {
    status == PROVISIONING_STATUS_DELETING || status == PROVISIONING_STATUS_DELETED
}

/// How long after `RegisterIdentityProvider` the registration becomes `REGISTERED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the registration is returned in the
/// non-terminal `REGISTERING` status, so a caller that reads it straight away still
/// observes that transient status, and any later read materialises the transition the
/// clock already owed.
const REGISTRATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `AssociateUser` the association becomes `ASSOCIATED`.
///
/// A chosen span, on the same footing as [`REGISTRATION_SPAN`]: AWS documents no
/// duration for it, the association is returned in the non-terminal `ASSOCIATING`
/// status, and the span is inside the emulator's 250-750 ms window so a caller that
/// reads straight back still observes the transient status.
const ASSOCIATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `DisassociateUser` the teardown the `DISASSOCIATING` status announces
/// completes — the instant the association stops being reported.
///
/// A chosen span, on the same footing as [`ASSOCIATION_SPAN`]: AWS documents no duration
/// for it, the association is returned in the non-terminal `DISASSOCIATING` status, and
/// the span is inside the emulator's 250-750 ms window so a caller that reads straight
/// back still observes that transient status — and the instance still in the service's
/// view — instead of the association vanishing between two reads.
const DISASSOCIATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `CreateLicenseServerEndpoint` the endpoint becomes `PROVISIONED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the endpoint is created in the
/// non-terminal `PROVISIONING` status, so a caller that lists it straight away still
/// observes that transient status, and a later read materialises the transition the
/// clock already owed.
const ENDPOINT_PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// How long after `DeleteLicenseServerEndpoint` the endpoint reaches the terminal
/// `DELETED`.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the delete call answers with the
/// non-terminal `DELETING` status, so a caller that lists the endpoint straight away
/// still observes that transient status, and a later read materialises the terminal
/// `DELETED` the clock already owed.
const ENDPOINT_DELETION_SPAN: Duration = Duration::from_millis(500);

/// How long after `DeregisterIdentityProvider` the registration stops being reported.
///
/// A chosen span (AWS documents no duration for it), kept inside the emulator's
/// 250-750 ms window for a clock-owed transition: the deregister call answers with the
/// non-terminal `DEREGISTERING` status — License Manager has to tear down the VPC
/// endpoints the registration provisioned — so a caller that lists the registration
/// straight away still observes that status, and a later read materialises the removal the
/// clock already owed.
const DEREGISTRATION_SPAN: Duration = Duration::from_millis(500);

/// How long after `StopProductSubscription` the teardown the `UNSUBSCRIBED` status
/// announces completes — the instant the subscription stops being reported.
///
/// A chosen span, on the same footing as [`DEREGISTRATION_SPAN`]: AWS documents no duration
/// for it, the call answers with the subscription already ended rather than removed, and
/// the span is inside the emulator's 250-750 ms window so a caller that lists straight
/// back still observes the ended subscription instead of it vanishing between two reads.
const UNSUBSCRIPTION_SPAN: Duration = Duration::from_millis(500);

/// How long the license a license server endpoint issues for a product subscription is
/// valid: the term counted from the instant the subscription started.
///
/// A CHOSEN term — AWS publishes no validity period for the license such a subscription
/// holds — and 30 days is a month of the monthly billing a user-based subscription is
/// charged on. It is not a status transition the clock owes, so it is deliberately not
/// one of the spans above: nothing falls due when it passes, it is only the date
/// `ProductUserSummary.LicenseExpirationDate` reports, derived from what the subscription
/// persisted so every read of one subscription answers the same instant.
const LICENSE_TERM: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// The most endpoints whose deletion has completed that the store keeps reporting.
///
/// A deleted endpoint stays observable in its terminal `DELETED` status rather than
/// vanishing between two reads, so the records cannot simply be dropped — but a caller
/// looping create/delete must not grow the store without bound either. Beyond this many,
/// the endpoints deleted EARLIEST are dropped, which keeps the retained set bounded by
/// the same number as the live one ([`MAX_RESOURCES_PER_KIND`]).
pub const MAX_DELETED_ENDPOINTS_RETAINED: usize = MAX_RESOURCES_PER_KIND;

/// The most subnets one `UpdateSettings` payload may name in `AddSubnets` or in
/// `RemoveSubnets`.
///
/// `Subnets` carries `@length(min: 1)` and NO maximum in the model, so the bound is the
/// account's rather than the shape's: EC2's default quota is 200 subnets per VPC, so a
/// payload naming more subnets than a VPC can hold could not be satisfied whatever the
/// store holds. Bounding the payload is also what keeps the reconciliation in
/// [`apply_update_settings`] — which runs with the service-state lock held — linear and
/// bounded in the request.
pub const MAX_SUBNETS_PER_UPDATE: usize = 200;

/// The most resources of one kind (registrations, user associations, license server
/// endpoints) this emulator holds for an account.
///
/// AWS bounds these resources per account and models the refusal:
/// `RegisterIdentityProvider`, `AssociateUser` and `CreateLicenseServerEndpoint` all
/// declare `ServiceQuotaExceededException` ("The request failed because a service quota
/// is exceeded"). The NUMBER here is the emulator's own — AWS publishes its quotas
/// separately and they change — chosen so that a caller minting distinct resources cannot
/// grow the in-memory store without bound, and far above anything a test or a recorded
/// fixture creates.
pub const MAX_RESOURCES_PER_KIND: usize = 1_000;

/// The most tags one of this service's resources carries.
///
/// The model bounds a tagging PAYLOAD (`Tags` has `@length(min: 0, max: 50)`) but says
/// nothing about the ACCUMULATION of repeated writes, and AWS's own limit is 50 tags per
/// resource — so 50 is the bound on the MERGED set too, enforced before a merge so a
/// repeated `TagResource` cannot grow one resource's tag map without limit.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The most characters a tag KEY holds.
///
/// `Tags` is `map<String, String>` and carries `@length(min: 0, max: 50)` on the MAP only
/// (`smithy_query(query="shape", shape="Tags")`), so nothing in the model constrains an
/// individual key and the boundary validator checks none. The rule is the
/// service-independent one from the AWS tagging reference ("Tag key length: Maximum 128
/// Unicode characters in UTF-8"), which every AWS tagging API enforces, counted in
/// CHARACTERS and not bytes for exactly that reason.
pub const MAX_TAG_KEY_CHARS: usize = 128;

/// The most characters a tag VALUE holds: the same tagging reference's "Tag value length:
/// Maximum 256 Unicode characters in UTF-8". A value may be EMPTY (unlike a key), which
/// is why only a maximum is stated here.
pub const MAX_TAG_VALUE_CHARS: usize = 256;

/// The fewest characters a tag KEY holds: a key is required, so the empty string is not
/// one ("Tag key length: Minimum 1" in the same reference).
pub const MIN_TAG_KEY_CHARS: usize = 1;

/// The tag-key namespace AWS keeps for itself: "Do not use the `aws:` prefix in your tag
/// names or values because it is reserved for AWS use", so a caller can neither set nor
/// remove a tag in it. Compared case-INSENSITIVELY, because AWS reserves the namespace
/// rather than one spelling of it.
pub const RESERVED_TAG_PREFIX: &str = "aws:";

/// Check the tag set a request asks this service to WRITE — `TagResource`'s `Tags`, and
/// the `Tags` map of every operation that creates a resource with tags.
///
/// Refuses, with [`StateError::Validation`] (every such operation models
/// `ValidationException`, HTTP 400), a key outside
/// [`MIN_TAG_KEY_CHARS`]..=[`MAX_TAG_KEY_CHARS`], a value longer than
/// [`MAX_TAG_VALUE_CHARS`], and any key in the [`RESERVED_TAG_PREFIX`] namespace. The
/// whole request is refused so a payload carrying one illegal tag writes NONE of its
/// tags. Keys are examined in sorted order, so a payload breaking several rules names the
/// same tag every time rather than whichever one the map happened to yield first.
pub fn validate_tags(tags: &HashMap<String, String>) -> Result<(), StateError> {
    let mut keys: Vec<&String> = tags.keys().collect();
    keys.sort_unstable();
    for key in keys {
        validate_tag_key("Tags", key)?;
        let value = &tags[key];
        let length = value.chars().count();
        if length > MAX_TAG_VALUE_CHARS {
            return Err(StateError::Validation(format!(
                "Tags: the value of tag key {key:?} must hold at most \
                 {MAX_TAG_VALUE_CHARS} characters, but holds {length}"
            )));
        }
    }
    Ok(())
}

/// Check the tag keys a request asks this service to REMOVE (`UntagResource`'s `TagKeys`).
///
/// The same key rule as [`validate_tags`], because it is a rule about what a tag key IS:
/// a key AWS reserves cannot be removed any more than it can be set, and a key outside
/// the length bounds names no tag any resource could carry. A legal key the resource does
/// not carry is NOT an error — removal stays idempotent.
pub fn validate_tag_keys(keys: &[String]) -> Result<(), StateError> {
    for key in keys {
        validate_tag_key("TagKeys", key)?;
    }
    Ok(())
}

/// The rules every tag key obeys, whichever member carries it. `member` is named in the
/// refusal so a caller sees which part of the request to fix.
fn validate_tag_key(member: &str, key: &str) -> Result<(), StateError> {
    let length = key.chars().count();
    if !(MIN_TAG_KEY_CHARS..=MAX_TAG_KEY_CHARS).contains(&length) {
        return Err(StateError::Validation(format!(
            "{member}: a tag key must hold between {MIN_TAG_KEY_CHARS} and \
             {MAX_TAG_KEY_CHARS} characters, but {key:?} holds {length}"
        )));
    }
    // `RESERVED_TAG_PREFIX` is spelled in lowercase, so lowercasing the key is what makes
    // the comparison case-insensitive.
    if key.to_lowercase().starts_with(RESERVED_TAG_PREFIX) {
        return Err(StateError::Validation(format!(
            "{member}: tag key {key:?} uses the {RESERVED_TAG_PREFIX} prefix, which is \
             reserved for AWS use"
        )));
    }
    Ok(())
}

/// What the operations that resolve an identity-provider reference by its `ARN` say about
/// one that resolves to no registration: the ARN named is not a valid parameter value.
///
/// One sentence in one place, because it is one resolution: `CreateLicenseServerEndpoint`
/// and `UpdateIdentityProviderSettings` both refuse an unresolvable ARN with it,
/// and neither models a not-found for this surface.
///
/// A request that names the identity provider ITSELF rather than an ARN is a different
/// refusal and carries no sentence of its own — see [`StateError::ProviderNotAuthorized`].
pub const IDENTITY_PROVIDER_NOT_VALID: &str = "Identity provider is not valid";

/// What the USER-ASSOCIATION plane says about an identity provider that no LIVE
/// registration of this account is for.
///
/// One sentence in one place, because it is one resolution: `AssociateUser` creates an
/// association through the registration of the directory it names and
/// `ListUserAssociations` reports the associations of that same directory, so both resolve
/// it with [`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::require_registered_provider_key`]
/// and both answer their modelled `ResourceNotFoundException` carrying this.
pub const IDENTITY_PROVIDER_NOT_REGISTERED: &str =
    "the identity provider is not registered for user-based subscriptions";

/// Why an operation could not be served from state.
///
/// Each handler maps this onto its OWN modelled error enum (the generated enums are
/// per-operation types), so the lookup rules themselves live here exactly once.
#[derive(Debug, Clone)]
pub enum StateError {
    /// The addressed resource is not in the store — HTTP 404 `ResourceNotFoundException`.
    NotFound(String),
    /// The resource already exists in a conflicting state — `ConflictException`.
    Conflict(String),
    /// The request is malformed in a way the model cannot express — HTTP 400
    /// `ValidationException`.
    Validation(String),
    /// No LIVE registration resolves for the identity-provider reference the request made:
    /// an `IdentityProviderArn` that is not one of this account's, or — on the
    /// USER-ASSOCIATION plane — a directory no live registration of this account is for.
    ///
    /// A statement about the reference the caller wrote, so it carries no message: the two
    /// planes word it differently and neither wording belongs to the lookup. The operations
    /// that manage or report the REGISTRATION report the ARN as an invalid parameter — the
    /// modelled `ValidationException` (HTTP 400) carrying [`IDENTITY_PROVIDER_NOT_VALID`] —
    /// and `AssociateUser` / `ListUserAssociations` answer their own modelled
    /// `ResourceNotFoundException` carrying [`IDENTITY_PROVIDER_NOT_REGISTERED`], because the
    /// fact their authorization is about — the caller's ownership of the EC2 instance — has
    /// already been established by the time the directory is resolved, so there is nothing
    /// left to withhold.
    ProviderNotRegistered,
    /// The request addressed a registration by the `(IdentityProvider, Product)` pair — the
    /// identity provider ITSELF — and this account holds no registration under that pair.
    ///
    /// A different refusal from [`StateError::ProviderNotRegistered`], because the request
    /// named a resource of ANOTHER service (the Active Directory the union carries) rather
    /// than a value this service minted: whether the caller may act through that directory
    /// is an authorization question, and this service answers it CLOSED — the non-modelled
    /// `UnauthorizedException` with its fixed sentence
    /// ([`crate::scaffolded::Scaffolded::unauthorized`]) — disclosing nothing about what the
    /// directory is, or is not, registered for.
    ProviderNotAuthorized,
}

/// The user-based subscription products this service admits.
///
/// `Product` is modelled as a plain `String`, so the boundary validator enforces nothing;
/// the set comes from `RegisterIdentityProvider.Product`'s own documentation — "Valid
/// values: VISUAL_STUDIO_ENTERPRISE | VISUAL_STUDIO_PROFESSIONAL |
/// OFFICE_PROFESSIONAL_PLUS | OFFICE_STANDARD | REMOTE_DESKTOP_SERVICES" — which the
/// `Product` member of `StartProductSubscription`, `StopProductSubscription`,
/// `DeregisterIdentityProvider` and `ListProductSubscriptions` repeats verbatim.
pub const PRODUCTS: [&str; 5] = [
    "VISUAL_STUDIO_ENTERPRISE",
    "VISUAL_STUDIO_PROFESSIONAL",
    "OFFICE_PROFESSIONAL_PLUS",
    "OFFICE_STANDARD",
    "REMOTE_DESKTOP_SERVICES",
];

/// Refuse a `Product` outside [`PRODUCTS`], naming the rejected value so a caller sees
/// which value was refused.
///
/// Every operation that takes a `Product` on the wire calls this before it resolves
/// anything in the store: whether a product name is one this service serves does not
/// depend on what is registered.
pub fn validate_product(product: &str) -> Result<(), StateError> {
    if PRODUCTS.contains(&product) {
        Ok(())
    } else {
        Err(StateError::Validation(format!(
            "Invalid product {product}"
        )))
    }
}

/// How this service states that a request named none of a set of members it had to name.
///
/// Several operations address a resource either by its ARN or by the members that identify
/// it, and refuse a request supplying neither by LISTING those members — the form
/// `StopProductSubscription` (`Username`/`IdentityProvider`/`Product`) and
/// `DisassociateUser` (`Username`/`InstanceId`/`IdentityProvider`) both use. The members
/// are listed in alphabetical order, as both of those sentences order them, so the
/// sentence does not depend on the order a handler happens to pass them in.
pub fn members_must_be_provided(members: &[&str]) -> String {
    let mut names: Vec<&str> = members.to_vec();
    names.sort_unstable();
    let listed = match names.split_last() {
        // A refusal that names no member states no requirement, so there is no sentence to
        // build: every call site passes a literal, non-empty list of the members the
        // operation's model declares.
        None => unreachable!("a refusal must name the members it requires"),
        Some((last, [])) => (*last).to_string(),
        Some((last, [only])) => format!("{only} and {last}"),
        Some((last, rest)) => format!("{}, and {last}", rest.join(", ")),
    };
    format!("{listed} must be provided")
}

/// The sentence a `StopProductSubscription` is refused with when the user it names has no
/// subscription in state: the one wording of that refusal, whether the identity the
/// request named matched nothing at all or named a domain the stored subscription's user
/// did not come from.
pub fn no_such_subscription(username: &str, product: &str, domain: Option<&str>) -> String {
    match domain_key(domain) {
        Some(domain) => format!(
            "user {username} has no subscription to product {product} in domain {domain}"
        ),
        None => format!("user {username} has no subscription to product {product}"),
    }
}

/// A registered identity provider, as `RegisterIdentityProvider` stored it.
#[derive(Debug, Clone)]
pub struct IdentityProviderRecord {
    /// The minted `IdentityProviderArn`; the key of `identity_providers`.
    pub arn: String,
    /// The provider union exactly as registered.
    pub identity_provider: IdentityProvider,
    /// The user-based subscription product this registration is for.
    pub product: String,
    /// The product-related configuration settings (subnets + security group) exactly as
    /// the registration supplied them.
    ///
    /// The input member is OPTIONAL, so a registration that provisions no activation-server
    /// VPC endpoints supplies none; the record then carries no subnet and no security group
    /// (an empty [`Settings`]) and every summary reports that empty configuration rather
    /// than values nobody asked for.
    pub settings: Settings,
    /// `REGISTERING` until [`REGISTRATION_SPAN`] has elapsed, then `REGISTERED`;
    /// `DEREGISTERING` from the moment a deregistration was accepted until
    /// [`DEREGISTRATION_SPAN`] has elapsed, at which point the record stops being reported
    /// altogether (the model gives the status no terminal counterpart).
    pub status: String,
    /// Set only when provisioning failed; never fabricated.
    pub failure_message: Option<String>,
    /// The account that owns the registration.
    pub owner_account_id: String,
    /// When the registration was created.
    pub created_at: SystemTime,
    /// When the `REGISTERING` -> `REGISTERED` transition falls due.
    pub registered_due_at: SystemTime,
    /// When the deregistration was accepted, or `None` while the registration is still
    /// live.
    pub deregistration_started_at: Option<SystemTime>,
    /// When the deregistration completes and the record stops being reported, or `None`
    /// while the registration is still live.
    pub deregistered_due_at: Option<SystemTime>,
}

/// What a settings write came to under the lock that writes
/// ([`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::update_live_settings`]).
#[derive(Debug, Clone)]
pub enum SettingsWrite {
    /// Stored: the settings the write landed on are the settings its resolution was made
    /// against, so the value now in the store is the value that resolved. The summary is
    /// the record as the write left it, boxed so this value is not the size of a whole
    /// summary whichever way the write went.
    Stored(Box<IdentityProviderSummary>),
    /// NOT stored: the registration's settings moved after they were read for the
    /// resolution, so folding the payload onto them would store a configuration nothing
    /// resolved. These are the settings the store holds now — the base a fresh resolution
    /// has to be made against.
    Moved(Settings),
}

/// Whether two `Settings` are the same configuration: the same subnets in the same order,
/// and the same security group.
///
/// `Settings` is a wire shape with no equality of its own, and its two members are all the
/// configuration a registration carries.
fn same_settings(one: &Settings, other: &Settings) -> bool {
    one.subnets == other.subnets && one.security_group_id == other.security_group_id
}

/// An EC2 instance that provides user-based subscriptions.
///
/// An instance is not created by an API of this service: it enters the service's view
/// through the user-association plane (`AssociateUser` names the `InstanceId` a user is
/// associated with), and `ListInstances` only REPORTS what is held here. So this record
/// carries no value `ListInstances` invents — every member is what the writer persisted.
#[derive(Debug, Clone)]
pub struct InstanceRecord {
    /// The EC2 instance id; the key of `instances`.
    pub instance_id: String,
    /// The status of the instance resource, as the writer recorded it. The model
    /// (`InstanceSummary.Status`) is a free-form string, so no vocabulary is imposed here.
    pub status: String,
    /// The user-based subscription products this instance provides, in the order they
    /// were added.
    pub products: Vec<String>,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The account that owns the instance.
    pub owner_account_id: String,
    /// The registration the instance's users were resolved through, when one is known.
    pub identity_provider: Option<IdentityProvider>,
    /// When this service last checked the instance's status — the instant the write that
    /// brought the instance into the view established the status reported above.
    ///
    /// The status is not taken from any request: it is what this service established about
    /// the instance, and establishing it is the check. So the writer that brings an
    /// instance into the view records when it ran
    /// ([`refresh_instance_view`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::refresh_instance_view)),
    /// a read reports that instant unchanged rather than checking (or inventing) one of its
    /// own, and a later write about what the instance PROVIDES leaves it alone. `None` only
    /// for a record written by something other than that writer.
    pub last_status_check_at: Option<SystemTime>,
}

/// A product user, as `StartProductSubscription` stored it.
#[derive(Debug, Clone)]
pub struct ProductUserRecord {
    /// The minted `ProductUserArn`; the key of `product_users`.
    pub arn: String,
    /// The user name from the identity provider.
    pub username: String,
    /// The user-based subscription product.
    pub product: String,
    /// The ARN of the identity provider this user was resolved through.
    pub identity_provider_arn: String,
    /// `SUBSCRIBED` while the subscription is active; [`STATUS_UNSUBSCRIBED`] from the
    /// moment `StopProductSubscription` accepted the end of the subscription until
    /// [`UNSUBSCRIPTION_SPAN`] has elapsed, at which point the record stops being reported
    /// altogether (the model gives the status no terminal counterpart).
    pub status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The Active Directory domain the user information came from.
    pub domain: Option<String>,
    /// When the subscription started — also the instant the license term of a
    /// license-based subscription is counted from (see
    /// [`license_expiration_date`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::license_expiration_date)).
    pub subscribed_at: SystemTime,
    /// When the subscription ended, once it has — what `SubscriptionEndDate` reports.
    pub unsubscribed_at: Option<SystemTime>,
    /// When the teardown the `UNSUBSCRIBED` status announces completes and the record stops
    /// being reported, or `None` while the subscription is still live.
    pub unsubscribed_due_at: Option<SystemTime>,
}

/// A user association to an EC2 instance, as `AssociateUser` stored it.
///
/// An association is to a DIRECTORY, not to a product registration:
/// `AssociateUser`/`DisassociateUser` carry an `IdentityProvider` and no `Product`
/// (`smithy_query(query="operation", operation="AssociateUser")`), so the record is keyed
/// by the same [`provider_key`] a registration is indexed under rather than by one
/// registration's ARN.
#[derive(Debug, Clone)]
pub struct InstanceUserRecord {
    /// The minted `InstanceUserArn`; the key of `instance_users`.
    pub arn: String,
    /// The user name from the identity provider.
    pub username: String,
    /// The EC2 instance the user was associated with.
    pub instance_id: String,
    /// The provider union exactly as the association named it.
    pub identity_provider: IdentityProvider,
    /// [`provider_key`] of `identity_provider`: what `instance_user_index` keys on.
    pub provider_key: String,
    /// `ASSOCIATING` until [`ASSOCIATION_SPAN`] has elapsed, then `ASSOCIATED`.
    pub status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// The Active Directory domain the user information came from.
    pub domain: Option<String>,
    /// ISO-8601 instant the association was created.
    pub association_date: Option<String>,
    /// ISO-8601 instant the association ended, once it has.
    pub disassociation_date: Option<String>,
    /// When the `ASSOCIATING` -> `ASSOCIATED` transition falls due.
    pub associated_due_at: SystemTime,
    /// When the teardown the `DISASSOCIATING` status announces falls due — the instant the
    /// association stops being reported — or `None` while the association is still live.
    pub disassociated_due_at: Option<SystemTime>,
}

/// One license-server host provisioned for a license server endpoint.
///
/// A host is provisioned by the create that mints its endpoint and is reported as a
/// `LicenseServer` of that endpoint ([`license_server`]), so the addresses it was given
/// are persisted here rather than recomputed per read: what one read reports about a host
/// is what the next one reports.
#[derive(Debug, Clone)]
pub struct LicenseServerHost {
    /// The subnet of the endpoint's registration the host was provisioned in — the
    /// subnet this host is the license server for.
    pub subnet: String,
    /// The private IPv4 address the host was given.
    pub ipv4_address: String,
    /// The IPv6 address the host was given, set only when the Active Directory the
    /// endpoint's registration recorded uses IPv6 ([`domain_uses_ipv6`]).
    pub ipv6_address: Option<String>,
}

/// A license server endpoint, as `CreateLicenseServerEndpoint` stored it.
#[derive(Debug, Clone)]
pub struct LicenseServerEndpointRecord {
    /// The minted `LicenseServerEndpointArn`; the key of `license_server_endpoints`.
    pub arn: String,
    /// The minted `LicenseServerEndpointId`.
    pub id: String,
    /// The registration this endpoint was created for.
    pub identity_provider_arn: String,
    /// The `ServerType` the endpoint serves (the model's only value is `RDS_SAL`).
    pub server_type: String,
    /// The license-server settings exactly as created, so nothing the caller supplied is
    /// lost between operations.
    pub settings: LicenseServerSettings,
    /// The network address of the endpoint, reported only once it is `PROVISIONED`:
    /// there is no address to reach while it is still being provisioned.
    pub endpoint: String,
    /// `PROVISIONING` until [`ENDPOINT_PROVISIONING_SPAN`] has elapsed, then
    /// `PROVISIONED`; `DELETING` from the moment a deletion was accepted until
    /// [`ENDPOINT_DELETION_SPAN`] has elapsed, then the terminal `DELETED`.
    pub provisioning_status: String,
    /// Set only when the service has something to report; never fabricated.
    pub status_message: Option<String>,
    /// When the endpoint was created.
    pub created_at: SystemTime,
    /// When the `PROVISIONING` -> `PROVISIONED` transition falls due.
    pub provisioned_due_at: SystemTime,
    /// The license-server hosts provisioned for this endpoint, in the order the subnets
    /// they serve are held in — what `LicenseServers` reports.
    pub license_servers: Vec<LicenseServerHost>,
    /// When the deletion was accepted, or `None` while the endpoint is still live. The
    /// order deletions were accepted in is what bounds the retained set (see
    /// [`MAX_DELETED_ENDPOINTS_RETAINED`]).
    pub deletion_started_at: Option<SystemTime>,
    /// When the `DELETING` -> `DELETED` transition falls due, or `None` while the
    /// endpoint is still live.
    pub deleted_due_at: Option<SystemTime>,
}

/// The status transition a stored record owes the clock, read off the record itself.
///
/// A record is in a non-terminal status exactly when its own status says so, and the
/// instant that status falls due is a field of the same record — so the record, not its
/// writer, is what knows whether anything is owed and when. Every writer that stores a
/// record of one of these kinds asks it through
/// [`schedule_record_transition`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::schedule_record_transition),
/// which is the ONE place "store a record in a non-terminal status" is paired with
/// "schedule the transition it owes": a new creating or teardown path gets the pending
/// guard from the writer it stores through and cannot be written without it. The sweep that
/// materialises those transitions recomputes the guard through this same trait
/// ([`fold_pending_transition`]), so a status or a due field added to an impl here is
/// honoured by the writers and by the sweep alike.
pub trait OwedTransition {
    /// When the clock next owes this record a status transition, or `None` when its status
    /// is terminal (nothing falls due for it).
    fn owed_transition_due_at(&self) -> Option<SystemTime>;
}

impl OwedTransition for IdentityProviderRecord {
    fn owed_transition_due_at(&self) -> Option<SystemTime> {
        if self.status == STATUS_REGISTERING {
            Some(self.registered_due_at)
        } else if deregistration_started(&self.status) {
            self.deregistered_due_at
        } else {
            None
        }
    }
}

impl OwedTransition for InstanceUserRecord {
    fn owed_transition_due_at(&self) -> Option<SystemTime> {
        if self.status == STATUS_ASSOCIATING {
            Some(self.associated_due_at)
        } else if disassociation_started(&self.status) {
            self.disassociated_due_at
        } else {
            None
        }
    }
}

impl OwedTransition for ProductUserRecord {
    fn owed_transition_due_at(&self) -> Option<SystemTime> {
        if unsubscribe_started(&self.status) {
            self.unsubscribed_due_at
        } else {
            None
        }
    }
}

impl OwedTransition for LicenseServerEndpointRecord {
    fn owed_transition_due_at(&self) -> Option<SystemTime> {
        if self.provisioning_status == PROVISIONING_STATUS_PROVISIONING {
            Some(self.provisioned_due_at)
        } else if self.provisioning_status == PROVISIONING_STATUS_DELETING {
            self.deleted_due_at
        } else {
            None
        }
    }
}

/// Whether the transition `record` owes the clock has already fallen due at `now`.
///
/// Asked of the RECORD through [`OwedTransition`], so "is anything owed, and when" is
/// answered in one place: the sweep that calls this decides only how to APPLY a transition
/// the clock has made due, never which statuses owe one.
fn transition_is_due(record: &impl OwedTransition, now: SystemTime) -> bool {
    record
        .owed_transition_due_at()
        .is_some_and(|due_at| now >= due_at)
}

/// Fold whatever transition `record` still owes the clock into `earliest`, keeping the
/// earliest instant and leaving it untouched for a record that owes nothing.
///
/// The read half of [`transition_is_due`], over the same trait: this is how
/// [`materialise_due`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::materialise_due)
/// recomputes the pending guard
/// ([`pending_transition_due_at`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::pending_transition_due_at))
/// from the records it just visited, so the sweep and the writers
/// ([`schedule_record_transition`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::schedule_record_transition))
/// agree on that fact by construction instead of by two copies of the same branches.
fn fold_pending_transition(record: &impl OwedTransition, earliest: &mut Option<SystemTime>) {
    if let Some(due_at) = record.owed_transition_due_at() {
        *earliest = Some(earliest.map_or(due_at, |held: SystemTime| held.min(due_at)));
    }
}

/// Per-service state for com-amazonaws-licensemanagerusersubscriptions-licensemanagerusersubscriptions. A handler reaches it through [`service_state`].
#[derive(Debug)]
pub struct ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    /// Registrations by `IdentityProviderArn`.
    pub identity_providers: HashMap<String, IdentityProviderRecord>,
    /// Registration ARNs in registration order, so a list is stable and pageable.
    pub identity_provider_order: Vec<String>,
    /// `(provider key, product) -> IdentityProviderArn`, so a caller that addresses a
    /// registration by `(IdentityProvider, Product)` resolves the same record an ARN does.
    pub identity_provider_index: HashMap<(String, String), String>,
    /// Product users by `ProductUserArn`.
    pub product_users: HashMap<String, ProductUserRecord>,
    /// Product-user ARNs in subscription order, so a list is stable and pageable.
    pub product_user_order: Vec<String>,
    /// [`ProductUserKey`] -> `ProductUserArn`, so a caller that addresses a subscription
    /// by the identity it was subscribed with resolves the same record an ARN does.
    pub product_user_index: HashMap<ProductUserKey, String>,
    /// License server endpoints by `LicenseServerEndpointArn`.
    pub license_server_endpoints: HashMap<String, LicenseServerEndpointRecord>,
    /// Endpoint ARNs in creation order, so a list is stable and pageable.
    pub license_server_endpoint_order: Vec<String>,
    /// `(IdentityProviderArn, ServerType) -> LicenseServerEndpointArn`, so a repeat
    /// create resolves the endpoint already in state instead of minting a second one.
    pub license_server_endpoint_index: HashMap<(String, String), String>,
    /// Instances providing user-based subscriptions, by EC2 instance id.
    pub instances: HashMap<String, InstanceRecord>,
    /// Instance ids in the order the instances entered the store, so a list is stable
    /// and pageable.
    pub instance_order: Vec<String>,
    /// Tags by resource ARN, written by whichever operation accepted them, so a later
    /// tagging operation reads the same store.
    pub tags: HashMap<String, HashMap<String, String>>,
    /// User associations by `InstanceUserArn`.
    pub instance_users: HashMap<String, InstanceUserRecord>,
    /// Association ARNs in association order, so a list is stable and pageable.
    pub instance_user_order: Vec<String>,
    /// [`InstanceUserKey`] -> `InstanceUserArn`, so a caller that addresses an association
    /// by the identity it was associated with resolves the same record its ARN does.
    pub instance_user_index: HashMap<InstanceUserKey, String>,
    /// `InstanceId -> the ARNs of the associations naming it, in association order`.
    ///
    /// The reverse of [`InstanceUserRecord::instance_id`], maintained by the one writer
    /// ([`insert_instance_user`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::insert_instance_user))
    /// and the one remover
    /// ([`remove_instance_user`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::remove_instance_user)).
    /// An instance's view is recomputed from exactly the associations that name it, so
    /// without this index every refresh would scan every association the account holds —
    /// and a request that refreshes every instance would pay instances x associations with
    /// the service-state lock held.
    pub instance_users_by_instance: HashMap<String, Vec<String>>,
    /// The EC2 instances this account has in this service's OWN fleet seam, keyed by
    /// instance id.
    ///
    /// Whether the caller may associate a user with an instance is a fact about the
    /// ACCOUNT's EC2 instances, not about this service, and this service models no
    /// operation that launches one — so the instances a request is resolved against come
    /// from the service that owns them, asked over the in-process callback
    /// ([`ec2_fleet`]). This map is the OTHER half of that resolution, not a closed list
    /// of the only ids that can be authorized: it holds the instances
    /// [`seed_account_instances`] starts the account with, so a process that mounts this
    /// service with no EC2 beside it still has instances to associate users with, plus any
    /// a test registers with
    /// [`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::register_account_instance`].
    pub account_instances: HashMap<String, String>,
    /// The AWS Directory Service directories this account has in this service's OWN fleet
    /// seam, keyed by `DirectoryId`, each carrying the `DirectoryStage` it is in.
    ///
    /// Whether an identity provider can be registered for a directory is a fact about the
    /// ACCOUNT's directories, not about this service, and this service models no operation
    /// that creates one — so the directories a request is resolved against come from the
    /// plane that creates them, read off the record that plane publishes
    /// ([`ad_directory_fleet`]). This map is the OTHER half of that resolution, not a closed
    /// list of the only ids that can be registered, exactly as
    /// [`account_instances`](Self::account_instances) above is for instances: it holds the
    /// directories [`seed_account_directories`] starts the account with, so a process that
    /// mounts this service with no Directory Service beside it still has a directory to
    /// register an identity provider for, plus any a test registers with
    /// [`ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::register_account_directory`].
    pub account_directories: HashMap<String, String>,
    /// The earliest instant at which a stored record owes the clock a status transition,
    /// or `None` when no record is in a non-terminal status.
    ///
    /// [`materialise_due`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::materialise_due)
    /// runs on EVERY request, so it must not sweep every stored record every time: this
    /// is the O(1) guard that makes the sweep happen only when the clock actually owes
    /// something. It is written by the one writer of each record kind, which schedules
    /// whatever the record it stores owes (the `schedule_record_transition` pairing over
    /// [`OwedTransition`]), so no creating or teardown path can store a record in a
    /// non-terminal status without this guard covering it; were one to, the transition
    /// would be delayed until another record fell due, never corrupt a record. The sweep
    /// recomputes it from the records it leaves pending, asked through that same trait.
    pub pending_transition_due_at: Option<SystemTime>,
    /// Source of the resource ids this service mints.
    pub next_id: u64,
    /// Source of the host addresses this service allocates to the license servers it
    /// provisions (see [`allocate_host_addresses`](ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState::allocate_host_addresses)).
    pub next_host_address: u32,
}

/// The EC2 instances the account starts with in this service's own fleet seam, as
/// `(instance id, Availability Zone)`.
///
/// Three instances in the Region the emulator writes into the SDK shared config
/// (`us-east-1`, see `vendor/server/src/lib.rs`), with ids of EC2's own
/// `i-[0-9a-f]{17}` form, so a process that mounts this service with no EC2 beside it has
/// instances to associate users with. It is NOT the set of instances this service
/// authorizes: an instance EC2 holds is resolved against EC2 itself
/// ([`ec2_fleet::authorizes_instance`]), whatever this seed says.
pub fn seed_account_instances() -> Vec<(String, String)> {
    [
        ("i-0f1e2d3c4b5a69788", "us-east-1a"),
        ("i-0e2d3c4b5a6978899", "us-east-1b"),
        ("i-0d3c4b5a697889900", "us-east-1c"),
    ]
    .into_iter()
    .map(|(id, zone)| (id.to_string(), zone.to_string()))
    .collect()
}

/// The stage a directory has to be in before an identity provider can be registered for
/// it: `Active`, one of the values AWS Directory Service's `DirectoryStage` publishes
/// (`Requested`, `Creating`, `Created`, `Active`, `Inoperable`, `Impaired`, `Restoring`,
/// `RestoreFailed`, `Deleting`, `Deleted`, `Failed`). A directory still being created, or
/// being deleted, is not one a registration can be built on.
pub const DIRECTORY_STAGE_ACTIVE: &str = "Active";

/// The directories the account starts with in this service's own fleet seam, as
/// `(DirectoryId, DirectoryStage)`.
///
/// Three `Active` directories with ids of Directory Service's own `d-[0-9a-f]{10}` form
/// (the `@pattern` `ActiveDirectoryIdentityProvider.DirectoryId` carries), so a process
/// that mounts this service with no Directory Service beside it has a directory to register
/// an identity provider for. It is NOT the set of directories this service accepts: a
/// directory the plane that creates directories holds is resolved against that plane's own
/// record ([`ad_directory_fleet::resolves_active_directory`]), whatever this seed says.
pub fn seed_account_directories() -> Vec<(String, String)> {
    ["d-9f8e7d6c5b", "d-1a2b3c4d5e", "d-4455667788"]
        .into_iter()
        .map(|id| (id.to_string(), DIRECTORY_STAGE_ACTIVE.to_string()))
        .collect()
}

impl Default for ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    fn default() -> Self {
        Self {
            identity_providers: HashMap::new(),
            identity_provider_order: Vec::new(),
            identity_provider_index: HashMap::new(),
            product_users: HashMap::new(),
            product_user_order: Vec::new(),
            product_user_index: HashMap::new(),
            instances: HashMap::new(),
            instance_order: Vec::new(),
            tags: HashMap::new(),
            instance_users: HashMap::new(),
            instance_user_order: Vec::new(),
            instance_user_index: HashMap::new(),
            instance_users_by_instance: HashMap::new(),
            license_server_endpoints: HashMap::new(),
            license_server_endpoint_order: Vec::new(),
            license_server_endpoint_index: HashMap::new(),
            account_instances: seed_account_instances().into_iter().collect(),
            account_directories: seed_account_directories().into_iter().collect(),
            pending_transition_due_at: None,
            next_id: 0,
            next_host_address: 0,
        }
    }
}

impl ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState {
    /// Mint the resource-id portion of an ARN: `<prefix>-<12 lowercase hex digits>`.
    ///
    /// Monotonic, so ids are unique within a run and the mint order is the list order.
    pub fn mint_id(&mut self, prefix: &str) -> String {
        self.next_id += 1;
        format!("{prefix}-{:012x}", self.next_id)
    }

    /// Materialise every transition the clock already made due at `now`.
    ///
    /// Only transitions that were owed at `now` whether or not anybody called: a
    /// registration whose [`REGISTRATION_SPAN`] has elapsed becomes `REGISTERED`, one whose
    /// deregistration was accepted has finished tearing down and stops being reported, a
    /// user association whose [`ASSOCIATION_SPAN`] has elapsed becomes `ASSOCIATED`, one
    /// whose disassociation was accepted [`DISASSOCIATION_SPAN`] ago has finished tearing
    /// down and stops being reported, a product user whose subscription was ended
    /// [`UNSUBSCRIPTION_SPAN`] ago has finished tearing down and stops being reported, and a
    /// license server endpoint whose [`ENDPOINT_PROVISIONING_SPAN`] has elapsed becomes
    /// `PROVISIONED`, and one whose deletion was accepted [`ENDPOINT_DELETION_SPAN`] ago
    /// reaches the terminal `DELETED`. Reporting operations call this so a read reflects
    /// the clock without the read itself changing anything a later caller would not have
    /// seen anyway.
    ///
    /// Every request runs this, so the common case must not scale with how much the store
    /// holds: `pending_transition_due_at` is the O(1) guard, and a record is visited only
    /// on the one request that finds a transition due.
    pub fn materialise_due(&mut self, now: SystemTime) {
        match self.pending_transition_due_at {
            Some(due) if now >= due => {}
            // Either nothing is in a non-terminal status, or the earliest transition is
            // still in the future: there is nothing to materialise, and nothing to scan.
            _ => return,
        }

        // The sweep also recomputes the guard, from the records that are still pending —
        // and it asks each record what it still owes ([`fold_pending_transition`]) rather
        // than restating, per kind, which statuses are non-terminal and which field carries
        // the due instant. The branches below therefore decide one thing only: how to APPLY
        // a transition the clock has already made due ([`transition_is_due`]). A record the
        // sweep just advanced owes nothing, and one not yet due owes the same instant it
        // owed before, so the guard is exactly the earliest transition the records left in
        // the store owe.
        let mut next: Option<SystemTime> = None;
        // A registration whose deregistration has completed is dropped from the store,
        // which cannot happen while `identity_providers` is borrowed: the sweep collects
        // them and the drops run below.
        let mut deregistered: Vec<String> = Vec::new();
        for record in self.identity_providers.values_mut() {
            if transition_is_due(record, now) {
                if record.status == STATUS_REGISTERING {
                    record.status = STATUS_REGISTERED.to_string();
                } else {
                    // The teardown the clock owes: a `DEREGISTERING` registration stops
                    // being reported once its span has elapsed, whether or not anybody
                    // called. It leaves the store below, so it owes the guard nothing.
                    deregistered.push(record.arn.clone());
                    continue;
                }
            }
            fold_pending_transition(record, &mut next);
        }
        // The associations whose teardown the clock has made due, collected while the
        // records are borrowed and dropped once the sweep is over.
        let mut torn_down: Vec<String> = Vec::new();
        for record in self.instance_users.values_mut() {
            if transition_is_due(record, now) {
                if record.status == STATUS_ASSOCIATING {
                    record.status = STATUS_ASSOCIATED.to_string();
                } else {
                    // The teardown the clock owes: a `DISASSOCIATING` association has
                    // finished being torn down once its span has elapsed, whether or not
                    // anybody called. It leaves the store below.
                    torn_down.push(record.arn.clone());
                    continue;
                }
            }
            fold_pending_transition(record, &mut next);
        }
        // The product subscriptions whose teardown the clock has made due, collected while
        // the records are borrowed and dropped once the sweep is over.
        let mut unsubscribed: Vec<String> = Vec::new();
        for record in self.product_users.values() {
            if transition_is_due(record, now) {
                // The only transition a subscription owes is the end of its teardown: a
                // subscription that was ended has finished being torn down once its span
                // has elapsed, whether or not anybody called. It leaves the store below.
                unsubscribed.push(record.arn.clone());
            } else {
                fold_pending_transition(record, &mut next);
            }
        }
        for record in self.license_server_endpoints.values_mut() {
            if transition_is_due(record, now) {
                record.provisioning_status =
                    if record.provisioning_status == PROVISIONING_STATUS_PROVISIONING {
                        PROVISIONING_STATUS_PROVISIONED.to_string()
                    } else {
                        // The deletion the clock owes: a `DELETING` endpoint reaches the
                        // terminal `DELETED` once its span has elapsed, whether or not
                        // anybody called.
                        PROVISIONING_STATUS_DELETED.to_string()
                    };
            }
            fold_pending_transition(record, &mut next);
        }
        let mut dropped = false;
        let mut dropped_keys: Vec<String> = Vec::new();
        for arn in deregistered {
            if let Some(record) = self.drop_identity_provider(&arn) {
                dropped = true;
                if let Ok(key) = provider_key(&record.identity_provider) {
                    if !dropped_keys.contains(&key) {
                        dropped_keys.push(key);
                    }
                }
            }
        }
        // No association outlives the last live registration of its directory: the drop
        // above is one of the two routes a registration leaves the store by, and both
        // enforce that the same way.
        for key in &dropped_keys {
            self.drop_instance_users_orphaned_by(key, now);
        }
        // The views those drops changed are recomputed ONCE for the whole sweep, not once
        // per dropped registration, and not at all when the sweep dropped nothing.
        if dropped {
            self.refresh_every_instance_view(now);
        }
        self.pending_transition_due_at = next;

        // A teardown that has completed is what finally removes the association, and the
        // instance it named is recomputed from the associations that are left: the last
        // one to finish takes its instance out of this service's view. Dropped in ONE
        // batch, so however many fell due together the work stays linear in what the
        // store holds.
        self.drop_instance_users(&torn_down, now);

        // The same for an ended product subscription: the teardown the clock owed is what
        // finally stops reporting it and frees the triple it occupied. Dropped in ONE batch
        // for the same reason.
        self.drop_product_users(&unsubscribed);
    }

    /// Record that a stored resource owes the clock a status transition at `due_at`.
    ///
    /// Reached through [`schedule_record_transition`](Self::schedule_record_transition),
    /// which is what every writer of a record uses; keeping the EARLIEST due instant is
    /// what lets [`materialise_due`](Self::materialise_due) skip the sweep on a request
    /// that owes nothing.
    fn schedule_transition(&mut self, due_at: SystemTime) {
        self.pending_transition_due_at = Some(
            self.pending_transition_due_at
                .map_or(due_at, |earliest| earliest.min(due_at)),
        );
    }

    /// Schedule whatever transition `record` owes the clock, and nothing when it owes none.
    ///
    /// The ONE pairing of "a record was stored" with "the transition that record owes is
    /// scheduled": every writer of a record — the creating writers
    /// ([`insert_identity_provider`](Self::insert_identity_provider),
    /// [`insert_instance_user`](Self::insert_instance_user),
    /// [`insert_product_user`](Self::insert_product_user),
    /// [`insert_license_server_endpoint`](Self::insert_license_server_endpoint)) and the
    /// teardown writers (the `begin_*` calls) alike — ends here, so the guard
    /// ([`pending_transition_due_at`](Self::pending_transition_due_at)) is a consequence of
    /// storing the record rather than a second step each caller has to remember. A record
    /// in a terminal status owes nothing and schedules nothing, so a writer does not have
    /// to know which of its records are pending either.
    fn schedule_record_transition(&mut self, record: &impl OwedTransition) {
        if let Some(due_at) = record.owed_transition_due_at() {
            self.schedule_transition(due_at);
        }
    }

    /// Whether `count` stored resources of one kind is already this emulator's limit.
    ///
    /// The caller answers the modelled `ServiceQuotaExceededException` when it is; see
    /// [`MAX_RESOURCES_PER_KIND`] for why there is a limit at all.
    pub fn at_resource_limit(count: usize) -> bool {
        count >= MAX_RESOURCES_PER_KIND
    }

    /// Register an EC2 instance the account has — the seam
    /// [`seed_account_instances`] fills and a test adds to.
    pub fn register_account_instance(
        &mut self,
        instance_id: impl Into<String>,
        availability_zone: impl Into<String>,
    ) -> Option<String> {
        self.account_instances
            .insert(instance_id.into(), availability_zone.into())
    }

    /// Register a directory the account has, in `stage` — the seam
    /// [`seed_account_directories`] fills and a test adds to.
    pub fn register_account_directory(
        &mut self,
        directory_id: impl Into<String>,
        stage: impl Into<String>,
    ) -> Option<String> {
        self.account_directories
            .insert(directory_id.into(), stage.into())
    }

    /// Whether this service's OWN fleet seam holds `directory_id` in the `Active` stage
    /// ([`DIRECTORY_STAGE_ACTIVE`]) — the stage an identity provider can be registered for.
    ///
    /// Half of the directory resolution: an id this answers `false` for is resolved against
    /// the plane that creates directories before the request is refused, which is what
    /// [`ad_directory_fleet::resolves_active_directory`] does — the one predicate every
    /// caller that resolves a caller-named directory asks. Nothing calls this on its own to
    /// decide a refusal.
    pub fn resolves_active_directory(&self, directory_id: &str) -> bool {
        self.account_directories
            .get(directory_id)
            .is_some_and(|stage| stage == DIRECTORY_STAGE_ACTIVE)
    }

    /// Whether this service's OWN fleet seam holds `instance_id`.
    ///
    /// Half of the instance resolution: an id this answers `false` for is resolved against
    /// the service that owns instances before the request is refused, which is what
    /// [`ec2_fleet::authorizes_instance`] does — the one predicate every operation that
    /// acts on a caller-named instance asks. Nothing calls this on its own to decide a
    /// refusal.
    pub fn authorizes_instance(&self, instance_id: &str) -> bool {
        self.account_instances.contains_key(instance_id)
    }

    /// Whether any LIVE registration in state is for the directory `key` names.
    ///
    /// `AssociateUser` carries no `Product`, so the registration it associates through is
    /// addressed by directory alone: a directory with a registration for ANY product is
    /// registered for the purposes of a user association. A registration whose
    /// deregistration was accepted is not one: it is still reported so a client can watch
    /// the teardown, not so new work can be built on it.
    pub fn provider_key_is_registered(&self, key: &str) -> bool {
        self.identity_provider_order
            .iter()
            .filter_map(|arn| self.identity_providers.get(arn))
            .filter(|record| !deregistration_started(&record.status))
            .any(|record| provider_key(&record.identity_provider).is_ok_and(|k| k == key))
    }

    /// Require that some LIVE registration of this account is for the directory `key`
    /// names — the resolution every operation of the USER-ASSOCIATION plane makes before it
    /// creates or reports an association through that registration.
    ///
    /// The rule and the refusal it carries live here together, once: the predicate is
    /// [`Self::provider_key_is_registered`] and the refusal is
    /// [`StateError::ProviderNotRegistered`], which each handler's own `into_error` words
    /// with its plane's sentence (the enums are per-operation types, so the mapping cannot
    /// move here). `AssociateUser` and `ListUserAssociations` both ask through this, so
    /// neither can decide this one fact its own way.
    pub fn require_registered_provider_key(&self, key: &str) -> Result<(), StateError> {
        if self.provider_key_is_registered(key) {
            Ok(())
        } else {
            Err(StateError::ProviderNotRegistered)
        }
    }

    /// The registration `arn` names, when it names one this account can still act on: a
    /// registration whose deregistration was accepted resolves for nothing, exactly as one
    /// that is gone.
    pub fn live_identity_provider(&self, arn: &str) -> Option<&IdentityProviderRecord> {
        self.identity_providers
            .get(arn)
            .filter(|record| !deregistration_started(&record.status))
    }

    /// [`live_identity_provider`](Self::live_identity_provider) for a caller that WRITES the
    /// registration it resolves: the same rule, by the same predicate, so a write cannot
    /// reach a registration a read would not.
    fn live_identity_provider_mut(&mut self, arn: &str) -> Option<&mut IdentityProviderRecord> {
        self.identity_providers
            .get_mut(arn)
            .filter(|record| !deregistration_started(&record.status))
    }

    /// The product user `arn` names, when it names a subscription this account can still
    /// act on: one whose end was accepted resolves for nothing, exactly as one that is
    /// gone, even though it is still reported while its teardown runs.
    pub fn live_product_user(&self, arn: &str) -> Option<&ProductUserRecord> {
        self.product_users
            .get(arn)
            .filter(|record| !unsubscribe_started(&record.status))
    }

    /// The user associations of one identity provider on one EC2 instance, in association
    /// order.
    ///
    /// An association is to a DIRECTORY (see [`InstanceUserRecord`]), so the provider is
    /// named by its [`provider_key`] rather than by a registration ARN — the same key
    /// `instance_user_index` is keyed on. Both halves of the scope are required because
    /// every caller addresses one: `ListUserAssociations` models `IdentityProvider` and
    /// `InstanceId` as required members, so there is no unscoped list to serve.
    pub fn instance_users_in_order(
        &self,
        provider_key: &str,
        instance_id: &str,
    ) -> Vec<InstanceUserRecord> {
        self.instance_user_order
            .iter()
            .filter_map(|arn| self.instance_users.get(arn))
            .filter(|record| record.provider_key == provider_key)
            .filter(|record| record.instance_id == instance_id)
            .cloned()
            .collect()
    }

    /// Resolve a user association from the two ways `DisassociateUser` can address one:
    /// its `InstanceUserArn`, or the `(IdentityProvider, InstanceId, Username, Domain)`
    /// identity it was associated with
    /// ([`instance_user_arn_for`](Self::instance_user_arn_for)).
    ///
    /// Whichever way a request addresses one, every member of an association's identity it
    /// names has to be the stored association's: the members are what say which association
    /// the caller asked for, so an ARN combined with a member naming another association
    /// resolves to nothing rather than to the record the ARN happens to name.
    ///
    /// `None` when state holds no such association — the caller decides how to refuse,
    /// because this surface fails CLOSED rather than reporting a not-found.
    pub fn resolve_instance_user(
        &self,
        instance_user_arn: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        instance_id: Option<&str>,
        username: Option<&str>,
        domain: Option<&str>,
    ) -> Option<&InstanceUserRecord> {
        if let Some(arn) = instance_user_arn {
            let record = self.instance_users.get(arn)?;
            // Every member of an association's identity that a request names is part of
            // WHICH association it names — that is the whole of the addressing below — so
            // an association addressed by ARN is also only the one the caller asked for
            // when each of those members it ALSO names is the stored association's. A
            // request naming a directory, an instance, a user name or a domain that is not
            // the stored one names no association state holds, and is refused by this
            // surface's fail-closed `None` rather than acted on against a record the
            // caller never addressed.
            let named_provider_key = match identity_provider {
                // A provider union that names no directory at all matches no stored
                // association, so a request carrying one addresses nothing.
                Some(provider) => Some(provider_key(provider).ok()?),
                None => None,
            };
            if names_other(named_provider_key.as_deref(), &record.provider_key)
                || names_other(instance_id, &record.instance_id)
                || names_other(username, &record.username)
                || names_other_domain(domain, record.domain.as_deref())
            {
                return None;
            }
            return Some(record);
        }
        // The member-by-member addressing matches on that same identity, through the
        // association index.
        let arn = self.instance_user_arn_for(
            &provider_key(identity_provider?).ok()?,
            instance_id?,
            username?,
            domain,
        )?;
        self.instance_users.get(arn)
    }

    /// Store one user association and index it everywhere it is reached from: by ARN, in
    /// association order, by its [`instance_user_key`] identity and under the instance it
    /// names — and schedule the transition the stored association owes the clock.
    ///
    /// The ONE writer, so no two of those views of the same association can disagree, and
    /// so an association stored in a non-terminal status is stored together with the guard
    /// its transition is swept on (see
    /// [`pending_transition_due_at`](Self::pending_transition_due_at)).
    pub fn insert_instance_user(&mut self, record: InstanceUserRecord) {
        let arn = record.arn.clone();
        let index_key = instance_user_key(
            &record.provider_key,
            &record.instance_id,
            &record.username,
            record.domain.as_deref(),
        );
        self.instance_users_by_instance
            .entry(record.instance_id.clone())
            .or_default()
            .push(arn.clone());
        self.schedule_record_transition(&record);
        self.instance_users.insert(arn.clone(), record);
        self.instance_user_order.push(arn.clone());
        self.instance_user_index.insert(index_key, arn);
    }

    /// Remove one user association from the store and from every index that reaches it,
    /// and recompute the view of the instance it named.
    ///
    /// This is the FINAL drop, which stops reporting the association altogether; a
    /// disassociation the caller asked for goes through
    /// [`begin_instance_user_disassociation`](Self::begin_instance_user_disassociation)
    /// instead, so the non-terminal status it answers with stays observable until the
    /// teardown the clock owes completes.
    pub fn remove_instance_user(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<InstanceUserRecord> {
        let record = self.instance_users.get(arn)?.clone();
        self.drop_instance_users(std::slice::from_ref(&record.arn), now);
        Some(record)
    }

    /// Drop the user associations `arns` names, and recompute the view of every instance
    /// they named.
    ///
    /// Batched on purpose: each association is unindexed in O(1) by the triple its record
    /// carries, the association ORDER is rebuilt in ONE pass however many associations are
    /// dropped together, and each instance's view is recomputed once against one
    /// precomputed registration map. So a request that finds several teardowns due pays
    /// the associations and the registrations once each rather than a product of the two
    /// with the service-state lock held.
    fn drop_instance_users(&mut self, arns: &[String], now: SystemTime) {
        let mut dropped: HashSet<String> = HashSet::new();
        let mut affected: Vec<String> = Vec::new();
        for arn in arns {
            let Some(record) = self.instance_users.remove(arn) else {
                continue;
            };
            dropped.insert(arn.clone());
            self.instance_user_index.remove(&instance_user_key(
                &record.provider_key,
                &record.instance_id,
                &record.username,
                record.domain.as_deref(),
            ));
            self.tags.remove(arn);
            let emptied = match self.instance_users_by_instance.get_mut(&record.instance_id) {
                Some(naming) => {
                    naming.retain(|held| held != arn);
                    naming.is_empty()
                }
                None => false,
            };
            if emptied {
                self.instance_users_by_instance.remove(&record.instance_id);
            }
            if !affected.contains(&record.instance_id) {
                affected.push(record.instance_id);
            }
        }
        if dropped.is_empty() {
            return;
        }
        self.instance_user_order.retain(|arn| !dropped.contains(arn));
        let products_by_key = self.products_by_provider_key();
        for instance_id in affected {
            let naming = self.associations_naming(&instance_id);
            self.apply_instance_view(&instance_id, &naming, &products_by_key, now);
        }
    }

    /// Drop every user association made through the directory `key` once that directory
    /// has no LIVE registration left.
    ///
    /// An association exists only through a registration of its directory — that is the
    /// resolution `AssociateUser` and `ListUserAssociations` both run before they report
    /// anything — so a directory that has just lost its last live registration holds no
    /// association any more: the integration every association through it depended on is
    /// being torn down with the registration. Dropping them here is what keeps the store
    /// from holding an association no operation can report and keeps an instance from
    /// being reported providing no product at all: the drop goes through
    /// [`drop_instance_users`](Self::drop_instance_users), so each instance's view is
    /// recomputed from the associations that are left and the instance leaves the view
    /// with the last one naming it.
    ///
    /// A directory that still has a live registration for ANY product keeps its
    /// associations: an association names a directory and no product, so it outlives the
    /// deregistration of any one product for that directory.
    fn drop_instance_users_orphaned_by(&mut self, key: &str, now: SystemTime) {
        if self.provider_key_is_registered(key) {
            return;
        }
        let orphaned: Vec<String> = self
            .instance_user_order
            .iter()
            .filter_map(|arn| self.instance_users.get(arn))
            .filter(|record| record.provider_key == key)
            .map(|record| record.arn.clone())
            .collect();
        // ONE batched drop however many associations the directory held, so a directory
        // with many users does not cost a rebuild of the association order per user.
        self.drop_instance_users(&orphaned, now);
    }

    /// Accept the disassociation of one user association: it enters `DISASSOCIATING` now
    /// and owes the clock the completion of its teardown at [`DISASSOCIATION_SPAN`], which
    /// is what finally stops reporting it.
    ///
    /// Disassociation is ASYNCHRONOUS — the instance has to be reconfigured, which is why
    /// the call answers the non-terminal `DISASSOCIATING` rather than a terminal status —
    /// so the record is NOT dropped here: a caller polling for the teardown to finish keeps
    /// seeing the association in `DISASSOCIATING`, and keeps seeing the instance it names
    /// in this service's view, instead of the state vanishing between two reads. What the
    /// disassociation does take away immediately is what made the association a LIVE
    /// resource: its tags, so the tagging surface no longer resolves it (see
    /// [`holds_resource`](Self::holds_resource)). The
    /// [`instance_user_key`] index entry is KEPT, because that identity is
    /// still occupied: the user is still being torn down from that instance, so it cannot
    /// be associated with it again until the teardown has completed.
    ///
    /// Returns the record as the disassociation left it (`DISASSOCIATING`), or `None` when
    /// the ARN names no stored association. An association whose disassociation was ALREADY
    /// accepted is returned unchanged, which is what this operation's `@idempotent` trait
    /// asks for: a repeat call neither restarts the teardown nor re-stamps the instant it
    /// ended.
    pub fn begin_instance_user_disassociation(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<InstanceUserRecord> {
        let record = self.instance_users.get_mut(arn)?;
        if disassociation_started(&record.status) {
            return Some(record.clone());
        }
        record.status = STATUS_DISASSOCIATING.to_string();
        record.disassociation_date = Some(iso8601(now));
        let due_at = now + DISASSOCIATION_SPAN;
        record.disassociated_due_at = Some(due_at);
        let disassociating = record.clone();
        self.tags.remove(arn);
        // The teardown the clock now owes, which is what stops reporting the association.
        self.schedule_record_transition(&disassociating);
        Some(disassociating)
    }

    /// Store one license server endpoint and index it everywhere it is reached from: by
    /// ARN, in creation order, and under the `(IdentityProviderArn, ServerType)` pair that
    /// a repeat create resolves it by — and schedule the transition the stored endpoint
    /// owes the clock.
    ///
    /// The ONE writer of an endpoint, exactly as the other kinds have
    /// ([`insert_identity_provider`](Self::insert_identity_provider),
    /// [`insert_instance_user`](Self::insert_instance_user)): the pair the endpoint is
    /// indexed under is read off the record, so no view of one endpoint can disagree with
    /// another, and a `PROVISIONING` endpoint cannot be stored without the guard its
    /// transition is swept on (see
    /// [`pending_transition_due_at`](Self::pending_transition_due_at)). The counterpart of
    /// [`begin_license_server_endpoint_deletion`](Self::begin_license_server_endpoint_deletion)
    /// and [`remove_license_server_endpoint`](Self::remove_license_server_endpoint).
    pub fn insert_license_server_endpoint(&mut self, record: LicenseServerEndpointRecord) {
        let arn = record.arn.clone();
        let index_key = (
            record.identity_provider_arn.clone(),
            record.server_type.clone(),
        );
        self.schedule_record_transition(&record);
        self.license_server_endpoints.insert(arn.clone(), record);
        self.license_server_endpoint_order.push(arn.clone());
        self.license_server_endpoint_index.insert(index_key, arn);
    }

    /// The license server endpoints in creation order.
    pub fn license_server_endpoints_in_order(&self) -> Vec<LicenseServerEndpointRecord> {
        self.license_server_endpoint_order
            .iter()
            .filter_map(|arn| self.license_server_endpoints.get(arn).cloned())
            .collect()
    }

    /// How many LIVE license server endpoints the account holds: an endpoint whose
    /// deletion was accepted ([`endpoint_deletion_started`]) is still reported, but it no
    /// longer occupies a slot in the account's quota.
    pub fn live_license_server_endpoint_count(&self) -> usize {
        self.license_server_endpoints
            .values()
            .filter(|record| !endpoint_deletion_started(&record.provisioning_status))
            .count()
    }

    /// Provision the license-server hosts of ONE license server endpoint: one host per
    /// subnet the registration the endpoint is created for provisions into
    /// (`Settings$Subnets`, "the subnets defined for the registered identity provider"),
    /// each with its own addresses.
    ///
    /// A subnet named twice is one subnet, so it gets one host. A registration that
    /// recorded no subnet provisions into none, and so gets no host: the endpoint then
    /// reports an empty `LicenseServers` list rather than a host in a subnet nobody named.
    /// No more than [`MAX_SUBNETS_PER_UPDATE`] hosts are provisioned for one endpoint —
    /// a VPC holds no more subnets than that, so there is nowhere for a further host to
    /// go, and this runs with the service-state lock held, so the work one request buys
    /// here stays bounded however long the stored subnet list is.
    ///
    /// Called once, by the create that mints the endpoint, so what an endpoint reports is
    /// what it was provisioned with: a later `UpdateIdentityProviderSettings` on its
    /// registration does not re-provision an endpoint that already exists.
    pub fn provision_license_servers(
        &mut self,
        registration: &IdentityProviderRecord,
    ) -> Vec<LicenseServerHost> {
        let dual_stack = domain_uses_ipv6(&registration.identity_provider);
        let mut provisioned: Vec<LicenseServerHost> = Vec::new();
        let mut served: HashSet<&str> = HashSet::new();
        for subnet in &registration.settings.subnets {
            if provisioned.len() >= MAX_SUBNETS_PER_UPDATE {
                break;
            }
            if !served.insert(subnet.as_str()) {
                continue;
            }
            let (ipv4_address, ipv6_address) = self.allocate_host_addresses(dual_stack);
            provisioned.push(LicenseServerHost {
                subnet: subnet.clone(),
                ipv4_address,
                ipv6_address,
            });
        }
        provisioned
    }

    /// Allocate the addresses of one license-server host: an IPv4 address always, and an
    /// IPv6 address when the host joins a `dual_stack` domain.
    ///
    /// Monotonic, so each host is given its own address in allocation order, and the
    /// address is stored on the endpoint record: a host keeps the address it was
    /// allocated for as long as it is reported.
    fn allocate_host_addresses(&mut self, dual_stack: bool) -> (String, Option<String>) {
        self.next_host_address = self.next_host_address.wrapping_add(1);
        let host = self.next_host_address;
        // The host portion of the block, laid out so an allocation never lands on the
        // network (`.0`) or broadcast (`.255`) address of its /24.
        let offset = host.wrapping_sub(1);
        let ipv4 = format!(
            "{HOST_IPV4_BLOCK}.{}.{}.{}",
            (offset / (254 * 256)) % 256,
            (offset / 254) % 256,
            offset % 254 + 1
        );
        let ipv6 = dual_stack.then(|| format!("{HOST_IPV6_BLOCK}::{host:x}"));
        (ipv4, ipv6)
    }

    /// Accept the deletion of one license server endpoint: it enters `DELETING` now and
    /// owes the clock the terminal `DELETED` at [`ENDPOINT_DELETION_SPAN`].
    ///
    /// Deletion is ASYNCHRONOUS — that is what `LicenseServerEndpointProvisioningStatus`
    /// models `DELETING` and `DELETED` for — so the record is NOT dropped here: a caller
    /// polling `ListLicenseServerEndpoints` keeps seeing the endpoint in `DELETING` and
    /// then in `DELETED`, instead of it vanishing between two reads. What the deletion
    /// does take away immediately is everything that made the endpoint a LIVE resource:
    /// the `(IdentityProviderArn, ServerType)` index entry, so a create for the same pair
    /// mints a new endpoint rather than resolving this one, and the tags, so the tagging
    /// surface no longer resolves it.
    ///
    /// Returns the record as the deletion left it (`DELETING`), or `None` when the ARN
    /// names no stored endpoint. An endpoint whose deletion was ALREADY accepted is
    /// returned unchanged, which is what this operation's `@idempotent` trait asks for:
    /// a repeat delete neither restarts the deletion nor moves the terminal status back.
    pub fn begin_license_server_endpoint_deletion(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<LicenseServerEndpointRecord> {
        let record = self.license_server_endpoints.get_mut(arn)?;
        if endpoint_deletion_started(&record.provisioning_status) {
            return Some(record.clone());
        }
        record.provisioning_status = PROVISIONING_STATUS_DELETING.to_string();
        record.deletion_started_at = Some(now);
        let due_at = now + ENDPOINT_DELETION_SPAN;
        record.deleted_due_at = Some(due_at);
        let deleting = record.clone();
        self.license_server_endpoint_index
            .retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        // The `DELETING` -> `DELETED` transition the clock now owes.
        self.schedule_record_transition(&deleting);
        self.prune_deleted_license_server_endpoints();
        Some(deleting)
    }

    /// Drop the endpoints whose deletion was accepted EARLIEST, so no more than
    /// [`MAX_DELETED_ENDPOINTS_RETAINED`] deleted endpoints are retained.
    fn prune_deleted_license_server_endpoints(&mut self) {
        let mut deleted: Vec<(SystemTime, String)> = self
            .license_server_endpoints
            .values()
            .filter(|record| endpoint_deletion_started(&record.provisioning_status))
            .map(|record| {
                (
                    record.deletion_started_at.unwrap_or(record.created_at),
                    record.arn.clone(),
                )
            })
            .collect();
        if deleted.len() <= MAX_DELETED_ENDPOINTS_RETAINED {
            return;
        }
        deleted.sort();
        let excess = deleted.len() - MAX_DELETED_ENDPOINTS_RETAINED;
        for (_, arn) in deleted.into_iter().take(excess) {
            self.remove_license_server_endpoint(&arn);
        }
    }

    /// Remove one license server endpoint from the store, the order it is listed in and
    /// the `(provider, server type)` index that reaches it.
    ///
    /// This is the FINAL drop, which stops reporting the endpoint altogether; a deletion
    /// the caller asked for goes through
    /// [`begin_license_server_endpoint_deletion`](Self::begin_license_server_endpoint_deletion)
    /// instead, so the statuses the model defines for it stay observable.
    pub fn remove_license_server_endpoint(
        &mut self,
        arn: &str,
    ) -> Option<LicenseServerEndpointRecord> {
        let record = self.license_server_endpoints.remove(arn)?;
        self.license_server_endpoint_order.retain(|a| a != arn);
        self.license_server_endpoint_index
            .retain(|_, indexed| indexed != arn);
        self.tags.remove(arn);
        Some(record)
    }

    /// Whether `arn` names a resource THIS service holds — the one resolution the
    /// tagging surface runs, whose `ResourceArn` may name any of this service's
    /// resource kinds.
    ///
    /// A license server endpoint whose deletion was accepted is NOT one: it is still
    /// reported in its `DELETING`/`DELETED` status, but it is on its way out and carries
    /// no tags any more, so the tagging surface resolves it exactly as it resolves an
    /// endpoint that is gone. A user association whose disassociation was accepted
    /// ([`disassociation_started`]), a product subscription whose end was accepted
    /// ([`unsubscribe_started`]), and a registration whose deregistration was accepted,
    /// are on their way out in exactly the same sense, so none is one either.
    pub fn holds_resource(&self, arn: &str) -> bool {
        self.live_identity_provider(arn).is_some()
            || self
                .product_users
                .get(arn)
                .is_some_and(|record| !unsubscribe_started(&record.status))
            || self
                .instance_users
                .get(arn)
                .is_some_and(|record| !disassociation_started(&record.status))
            || self
                .license_server_endpoints
                .get(arn)
                .is_some_and(|record| !endpoint_deletion_started(&record.provisioning_status))
    }

    /// Merge `tags` into the tags held against `arn`.
    ///
    /// Every tag the request carries has to be a legal tag ([`validate_tags`]) and the
    /// merged set is bounded by [`MAX_TAGS_PER_RESOURCE`]: a request that breaks either
    /// rule is refused WHOLE, leaving the stored map exactly as it was, so neither an
    /// illegal tag nor unbounded growth can reach the store. A key the resource already
    /// carries is overwritten and does not count as growth, which is also what the
    /// operation's `@idempotent` trait asks for.
    pub fn merge_tags(
        &mut self,
        arn: &str,
        tags: &HashMap<String, String>,
    ) -> Result<(), StateError> {
        // Whether a tag is legal is a fact about the request alone, so it is settled
        // before the stored set is even read.
        validate_tags(tags)?;
        let merged = match self.tags.get(arn) {
            Some(held) => {
                held.len() + tags.keys().filter(|key| !held.contains_key(*key)).count()
            }
            None => tags.len(),
        };
        if merged > MAX_TAGS_PER_RESOURCE {
            return Err(StateError::Validation(format!(
                "the resource would carry {merged} tags; a resource carries at most \
                 {MAX_TAGS_PER_RESOURCE}"
            )));
        }
        let held = self.tags.entry(arn.to_string()).or_default();
        for (key, value) in tags {
            held.insert(key.clone(), value.clone());
        }
        Ok(())
    }

    /// Store a registration: hold it by ARN, keep it in registration order, index it
    /// under the `(provider key, product)` pair that also addresses it, schedule the
    /// transition the stored registration owes the clock, and recompute every instance view
    /// the new registration changes.
    ///
    /// The counterpart of
    /// [`begin_identity_provider_deregistration`](Self::begin_identity_provider_deregistration)
    /// and [`remove_identity_provider`](Self::remove_identity_provider), and there for the
    /// same reason: which PRODUCTS an instance in this service's view provides is READ
    /// from the live registrations of the directories its associations name
    /// ([`refresh_instance_view`](Self::refresh_instance_view)), so a registration ADDED
    /// for a directory changes those views exactly as one leaving that set does. One writer
    /// owns the whole write, so no handler can store a registration and leave an instance
    /// reporting products the state no longer implies, or leave a `REGISTERING`
    /// registration with no sweep scheduled for it (see
    /// [`pending_transition_due_at`](Self::pending_transition_due_at)).
    pub fn insert_identity_provider(
        &mut self,
        key: String,
        record: IdentityProviderRecord,
        now: SystemTime,
    ) {
        let arn = record.arn.clone();
        let product = record.product.clone();
        self.schedule_record_transition(&record);
        self.identity_providers.insert(arn.clone(), record);
        self.identity_provider_order.push(arn.clone());
        self.identity_provider_index.insert((key.clone(), product), arn);
        self.refresh_instance_views_through(&key, now);
    }

    /// Recompute the view of every instance whose user associations resolve through the
    /// directory `key`.
    ///
    /// Only those instances can have changed: what an instance provides is the products
    /// the directories of the associations NAMING IT are registered for, so a change to
    /// one directory's registrations leaves every other instance's view as it was.
    ///
    /// Costed like [`refresh_every_instance_view`](Self::refresh_every_instance_view), and
    /// through the same two helpers so the two cannot drift: ONE pass over the
    /// associations to select the instances this directory reaches, then the registrations
    /// once and each selected instance's own associations through the reverse index
    /// ([`associations_naming`](Self::associations_naming)). This runs with the
    /// service-state lock held, so one request must not buy work that multiplies with the
    /// store.
    fn refresh_instance_views_through(&mut self, key: &str, now: SystemTime) {
        let affected: HashSet<String> = self
            .instance_user_order
            .iter()
            .filter_map(|arn| self.instance_users.get(arn))
            .filter(|association| association.provider_key == key)
            .map(|association| association.instance_id.clone())
            .collect();
        if affected.is_empty() {
            return;
        }
        let products_by_key = self.products_by_provider_key();
        for instance_id in affected {
            let naming = self.associations_naming(&instance_id);
            self.apply_instance_view(&instance_id, &naming, &products_by_key, now);
        }
    }

    /// The registrations in registration order.
    pub fn identity_providers_in_order(&self) -> Vec<IdentityProviderRecord> {
        self.identity_provider_order
            .iter()
            .filter_map(|arn| self.identity_providers.get(arn).cloned())
            .collect()
    }

    /// The instances in the order they entered the store.
    pub fn instances_in_order(&self) -> Vec<InstanceRecord> {
        self.instance_order
            .iter()
            .filter_map(|id| self.instances.get(id).cloned())
            .collect()
    }

    /// Record an instance, keeping the list order the order of first appearance: a
    /// re-recorded instance updates its record in place rather than moving in the list.
    ///
    /// Every writer that brings an instance into the service's view goes through here, so
    /// `instances` and `instance_order` cannot disagree.
    pub fn upsert_instance(&mut self, record: InstanceRecord) {
        let id = record.instance_id.clone();
        if self.instances.insert(id.clone(), record).is_none() {
            self.instance_order.push(id);
        }
    }

    /// Remove an instance from the store and from the list order.
    pub fn remove_instance(&mut self, instance_id: &str) -> Option<InstanceRecord> {
        let record = self.instances.remove(instance_id)?;
        self.instance_order.retain(|id| id != instance_id);
        Some(record)
    }

    /// Recompute what the service's view says about one instance, from the associations
    /// that still name it — or drop it from the view when none does.
    ///
    /// This is the writer [`InstanceRecord`] documents: an instance is not created by any
    /// API of this service, it enters the view because a user was associated with it and
    /// leaves with the last association that names it. Everything reported is a FUNCTION
    /// of the state at the call: the products are the ones the naming associations'
    /// DIRECTORIES are registered for (`AssociateUser` carries no `Product`, so they are
    /// read from the registrations rather than taken from the request), so a product stops
    /// being reported as soon as the last association or registration behind it is gone.
    /// An instance this brings INTO the view has its status established here, which is the
    /// status check `now` is recorded as — see [`InstanceRecord::last_status_check_at`].
    ///
    /// Every write that can change what an instance provides runs this, so no read has to
    /// (and no read does): an association created or removed refreshes the instance it
    /// names, and a registration stored
    /// ([`insert_identity_provider`](Self::insert_identity_provider)) or torn down
    /// refreshes the instances reached through its directory.
    ///
    /// An association whose teardown is still running still NAMES the instance, so the
    /// instance stays in view while that association is `DISASSOCIATING` and leaves it
    /// only when the last teardown has completed ([`materialise_due`](Self::materialise_due))
    /// — an instance a user is still being disassociated from is still configured for
    /// user-based subscriptions.
    ///
    /// ONE pass over the associations that name the instance and ONE over the
    /// registrations: this runs with the service-state lock held, so the work it costs must
    /// not multiply with how much the store holds beyond the lists it has to read.
    pub fn refresh_instance_view(&mut self, instance_id: &str, now: SystemTime) {
        let products_by_key = self.products_by_provider_key();
        let naming = self.associations_naming(instance_id);
        self.apply_instance_view(instance_id, &naming, &products_by_key, now);
    }

    /// The associations naming `instance_id`, in association order.
    ///
    /// Read from [`instance_users_by_instance`](Self::instance_users_by_instance), the
    /// reverse index the one association writer maintains, so recomputing one instance's
    /// view visits that instance's associations instead of scanning every association the
    /// account holds.
    fn associations_naming(&self, instance_id: &str) -> Vec<InstanceUserRecord> {
        self.instance_users_by_instance
            .get(instance_id)
            .map(|arns| {
                arns.iter()
                    .filter_map(|arn| self.instance_users.get(arn))
                    .cloned()
                    .collect()
            })
            .unwrap_or_default()
    }

    /// Recompute the view of every instance an association names: what a registration's
    /// removal changes is which PRODUCTS those instances provide.
    ///
    /// ONE pass over each of the three lists involved — the registrations, the associations
    /// of each instance (through the reverse index) and the instances — rather than
    /// re-reading two of them per instance: this runs with the service-state lock held, so
    /// one request must not be able to buy work that multiplies with the whole store.
    pub fn refresh_every_instance_view(&mut self, now: SystemTime) {
        let products_by_key = self.products_by_provider_key();
        for instance_id in self.instance_order.clone() {
            let naming = self.associations_naming(&instance_id);
            self.apply_instance_view(&instance_id, &naming, &products_by_key, now);
        }
    }

    /// The products each directory key has a LIVE registration for, in registration order.
    ///
    /// ONE pass over the registrations, so a refresh that needs the products of several
    /// directories does not rescan them once per directory. Only live registrations: a
    /// product stops being provided as soon as the registration behind it is being torn
    /// down.
    fn products_by_provider_key(&self) -> HashMap<String, Vec<String>> {
        let mut products: HashMap<String, Vec<String>> = HashMap::new();
        for record in self
            .identity_provider_order
            .iter()
            .filter_map(|arn| self.identity_providers.get(arn))
            .filter(|record| !deregistration_started(&record.status))
        {
            if let Ok(key) = provider_key(&record.identity_provider) {
                products.entry(key).or_default().push(record.product.clone());
            }
        }
        products
    }

    /// Write the view of ONE instance, computed from the associations that name it and the
    /// products their directories are registered for — or drop it from the view when no
    /// association names it.
    ///
    /// The one place an [`InstanceRecord`] is built, so the single-instance and the
    /// whole-store refresh above cannot disagree about what the view says.
    ///
    /// Bringing an instance INTO the view is what establishes the status reported for it,
    /// and establishing it is the status check: an instance that is not in the view yet is
    /// checked at `now`, and that instant is what every later read reports
    /// ([`InstanceRecord::last_status_check_at`]). Recomputing what an instance already in
    /// the view PROVIDES reads the registrations behind it, which says nothing new about
    /// the instance itself, so the recorded instant stands rather than being re-stamped by
    /// a write about something else.
    fn apply_instance_view(
        &mut self,
        instance_id: &str,
        naming: &[InstanceUserRecord],
        products_by_key: &HashMap<String, Vec<String>>,
        now: SystemTime,
    ) {
        let Some(first) = naming.first() else {
            self.remove_instance(instance_id);
            return;
        };

        let mut products: Vec<String> = Vec::new();
        for association in naming {
            let registered = products_by_key
                .get(&association.provider_key)
                .map(Vec::as_slice)
                .unwrap_or_default();
            for product in registered {
                if !products.contains(product) {
                    products.push(product.clone());
                }
            }
        }
        let identity_provider = Some(first.identity_provider.clone());
        // The instance's status was checked when it entered the view; a recompute of what
        // it provides is not another check, so the recorded instant is kept.
        let last_status_check_at = self
            .instances
            .get(instance_id)
            .and_then(|record| record.last_status_check_at)
            .or(Some(now));
        self.upsert_instance(InstanceRecord {
            instance_id: instance_id.to_string(),
            status: STATUS_ACTIVE.to_string(),
            products,
            status_message: None,
            owner_account_id: ::engine::emulator_account_id().to_string(),
            identity_provider,
            last_status_check_at,
        });
    }

    /// The product users of one registration (optionally one product), in subscription order.
    pub fn product_users_in_order(
        &self,
        identity_provider_arn: &str,
        product: Option<&str>,
    ) -> Vec<ProductUserRecord> {
        self.product_user_order
            .iter()
            .filter_map(|arn| self.product_users.get(arn))
            .filter(|record| record.identity_provider_arn == identity_provider_arn)
            .filter(|record| product.is_none_or(|p| record.product == p))
            .cloned()
            .collect()
    }

    /// Whether the registration `identity_provider_arn` names is backed by a license
    /// server endpoint: the license-based configuration whose licenses carry an expiry.
    ///
    /// The `(IdentityProviderArn, ServerType)` index is exactly the LIVE endpoints — an
    /// endpoint gives up its entry the moment a deletion is accepted — so a registration
    /// whose endpoint is being deleted is no longer license-backed. Which provisioning
    /// status the endpoint is in says whether its network address can be reached yet, not
    /// whether the registration it was created for is license-based, so it is not read
    /// here.
    ///
    /// One indexed lookup per server type the MODEL defines, so this costs the same
    /// whatever the store holds: it is asked once per product user reported, and a scan of
    /// the endpoints would make a list cost the PRODUCT of the two.
    pub fn license_backed(&self, identity_provider_arn: &str) -> bool {
        SERVER_TYPE_VALUES.iter().any(|server_type| {
            self.license_server_endpoint_index.contains_key(&(
                identity_provider_arn.to_string(),
                (*server_type).to_string(),
            ))
        })
    }

    /// Store one product user: the record, its place in the subscription order and the
    /// [`product_user_key`] identity that reaches it — and schedule whatever transition the
    /// stored subscription owes the clock (a live one owes none; see
    /// [`pending_transition_due_at`](Self::pending_transition_due_at)).
    ///
    /// The ONE writer of a product user, so the record, the list order and the index that
    /// reaches it cannot disagree about what is subscribed, nor about which user a stored
    /// subscription belongs to.
    pub fn insert_product_user(&mut self, record: ProductUserRecord) {
        let key = product_user_key(
            &record.identity_provider_arn,
            &record.product,
            &record.username,
            record.domain.as_deref(),
        );
        self.product_user_order.push(record.arn.clone());
        self.product_user_index.insert(key, record.arn.clone());
        self.schedule_record_transition(&record);
        self.product_users.insert(record.arn.clone(), record);
    }

    /// The expiration date of the license a product subscription holds, or `None` when the
    /// registration it was subscribed through uses no license server endpoint.
    ///
    /// A subscription served through a license server endpoint holds a license for
    /// [`LICENSE_TERM`] counted from the instant the subscription started, so the value is
    /// computed from what the store holds and every read of one subscription answers the
    /// same instant.
    pub fn license_expiration_date(&self, record: &ProductUserRecord) -> Option<String> {
        if !self.license_backed(&record.identity_provider_arn) {
            return None;
        }
        // `checked_add`, so an instant the term cannot be added to answers no expiry
        // instead of unwinding the request.
        record.subscribed_at.checked_add(LICENSE_TERM).map(iso8601)
    }

    /// The `ProductUserSummary` the model requires, built from the stored product user,
    /// the provider union of the registration it was subscribed through, and that
    /// registration's license backing.
    ///
    /// The ONE reader every operation that reports a product user goes through, so a
    /// subscription answers the same summary wherever it is reported.
    pub fn product_user_summary(
        &self,
        record: &ProductUserRecord,
        identity_provider: &IdentityProvider,
    ) -> ProductUserSummary {
        ProductUserSummary {
            username: record.username.clone(),
            product: record.product.clone(),
            identity_provider: identity_provider.clone(),
            status: record.status.clone(),
            product_user_arn: Some(record.arn.clone()),
            status_message: record.status_message.clone(),
            domain: record.domain.clone(),
            subscription_start_date: Some(iso8601(record.subscribed_at)),
            subscription_end_date: record.unsubscribed_at.map(iso8601),
            license_expiration_date: self.license_expiration_date(record),
        }
    }

    /// Resolve a registration from the two ways every operation can address one: its
    /// ARN, or the `(IdentityProvider, Product)` pair it was registered with.
    ///
    /// [`StateError::Validation`] when neither addressing is complete. An addressing that
    /// resolves to no live registration is refused per ADDRESSING, because the two name
    /// different things: an ARN is a value this service minted, so one it does not hold is
    /// an invalid parameter ([`StateError::ProviderNotRegistered`]), while the pair names
    /// the caller's own directory, so a pair this account has no registration under is an
    /// authorization question answered closed ([`StateError::ProviderNotAuthorized`]).
    ///
    /// Whichever way a request addresses one, every member of a registration's identity it
    /// names has to be the stored registration's, for the reason the user plane resolves a
    /// record the same way ([`resolve_instance_user`](Self::resolve_instance_user)): the
    /// members are what say which registration the caller asked for, so an ARN combined
    /// with a directory or a product that is another registration's is not registered.
    ///
    /// Only a LIVE registration resolves: one whose deregistration was accepted is still
    /// REPORTED in its `DEREGISTERING` status, but it is on its way out, so an operation
    /// that acts on a registration is told the identifier is not one of this account's
    /// identity providers exactly as it is once the record is gone.
    pub fn resolve_identity_provider(
        &self,
        arn: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
    ) -> Result<&IdentityProviderRecord, StateError> {
        if let Some(arn) = arn {
            let record = self
                .live_identity_provider(arn)
                .ok_or(StateError::ProviderNotRegistered)?;
            let named_key = match identity_provider {
                Some(provider) => Some(provider_key(provider)?),
                None => None,
            };
            let stored_key = provider_key(&record.identity_provider).ok();
            if names_other_option(named_key.as_deref(), stored_key.as_deref())
                || names_other(product, &record.product)
            {
                return Err(StateError::ProviderNotRegistered);
            }
            return Ok(record);
        }
        let (Some(identity_provider), Some(product)) = (identity_provider, product) else {
            return Err(StateError::Validation(members_must_be_provided(&[
                "IdentityProvider",
                "Product",
            ])));
        };
        let key = provider_key(identity_provider)?;
        let arn = self
            .identity_provider_index
            .get(&(key, product.to_string()))
            .ok_or(StateError::ProviderNotAuthorized)?;
        self.live_identity_provider(arn)
            .ok_or(StateError::ProviderNotAuthorized)
    }

    /// Fold an `UpdateSettings` payload onto the settings the registration `arn` holds AT
    /// THIS MOMENT, store the result and answer the summary of the record as the write left
    /// it — but ONLY while the settings held now are `resolved_from`, the ones the caller's
    /// network resolution was made against.
    ///
    /// The WRITE half of `UpdateIdentityProviderSettings`, which runs after that operation
    /// has resolved the network its payload would store against the account's own Amazon
    /// EC2 — a hop, so the lock the write takes is not the lock the read before it was
    /// taken under, and the store can have moved in between. All three decisions a write
    /// makes therefore happen here, under the one lock that writes:
    ///
    /// * the registration is resolved by this store's single liveness rule
    ///   ([`live_identity_provider_mut`](Self::live_identity_provider_mut), the writing form
    ///   of [`live_identity_provider`](Self::live_identity_provider)), so no update is
    ///   applied to a registration whose deregistration was accepted while the resolution
    ///   was in flight — a reference that stopped resolving is refused exactly as one that
    ///   never resolved, which is what every operation acting on a registration does;
    /// * the settings held now are compared with the ones the resolution was made against,
    ///   and a write whose base MOVED stores nothing and answers
    ///   [`SettingsWrite::Moved`] carrying the settings held now, because the value it would
    ///   store composes the payload with settings no resolution of this request ever saw —
    ///   the caller resolves that base and comes back;
    /// * the payload is folded onto the settings the record carries NOW rather than onto a
    ///   snapshot read earlier ([`apply_update_settings`]), so an update accepted in between
    ///   is something this one adds to instead of something it overwrites.
    ///
    /// Together, the value stored is exactly the value the resolution covered: the whole
    /// configuration — the payload's ids AND the ones already held — was resolved against
    /// the account's network, so no accepted write leaves a registration carrying a
    /// configuration that resolution would refuse (subnets of two VPCs, or a security group
    /// of neither).
    ///
    /// `Err(StateError::ProviderNotRegistered)` when `arn` names no live registration, and
    /// the payload's own refusals when the fold against the current settings has no result
    /// ([`apply_update_settings`]); nothing is stored in either case.
    pub fn update_live_settings(
        &mut self,
        arn: &str,
        resolved_from: &Settings,
        add_subnets: &[String],
        remove_subnets: &[String],
        security_group_id: Option<&str>,
    ) -> Result<SettingsWrite, StateError> {
        let record = self
            .live_identity_provider_mut(arn)
            .ok_or(StateError::ProviderNotRegistered)?;
        if !same_settings(&record.settings, resolved_from) {
            return Ok(SettingsWrite::Moved(record.settings.clone()));
        }
        // The fold reads the settings the record carries and yields the whole value to
        // store, so the registration resolved above is the one written: there is no second
        // lookup to disagree with it.
        let updated = apply_update_settings(
            &record.settings,
            add_subnets,
            remove_subnets,
            security_group_id,
        )?;
        record.settings = updated;
        Ok(SettingsWrite::Stored(Box::new(identity_provider_summary(
            record,
        ))))
    }

    /// The `ProductUserArn` of the subscription a request addresses by the members that
    /// identify a user — the registration, the product, the user name and `Domain` — or
    /// `None` when state holds none.
    ///
    /// `Domain` NARROWS which user that is, rather than being ignored: a request that names
    /// one addresses only the subscription stored for that domain (one indexed lookup on
    /// [`product_user_key`]), so the same user name in another Active Directory domain is
    /// another user. A request that names none does not narrow: it addresses the user's
    /// unqualified subscription when there is one, and otherwise the earliest subscription
    /// held for that user name in any domain, so a caller that never supplies `Domain` goes
    /// on addressing the subscription it started.
    ///
    /// The ONE place that match is made, so the occupancy `StartProductSubscription`
    /// refuses a duplicate on and the lookup `StopProductSubscription` resolves one by
    /// cannot disagree about which user is which.
    pub fn product_user_arn_for(
        &self,
        identity_provider_arn: &str,
        product: &str,
        username: &str,
        domain: Option<&str>,
    ) -> Option<&String> {
        if let Some(arn) = self.product_user_index.get(&product_user_key(
            identity_provider_arn,
            product,
            username,
            domain,
        )) {
            return Some(arn);
        }
        if domain_key(domain).is_some() {
            return None;
        }
        // The request named no domain and the user holds no unqualified subscription, so
        // there is no identity to look up: the subscriptions of that user name are read off
        // the subscription order, earliest first. One pass over what the store holds, which
        // is bounded ([`MAX_RESOURCES_PER_KIND`]), and only on this one addressing.
        self.product_user_order.iter().find(|arn| {
            self.product_users.get(*arn).is_some_and(|record| {
                record.identity_provider_arn == identity_provider_arn
                    && record.product == product
                    && record.username == username
            })
        })
    }

    /// The `InstanceUserArn` of the association a request addresses by the members that
    /// identify a user — the directory, the instance, the user name and `Domain` — or
    /// `None` when state holds none.
    ///
    /// The association plane's counterpart of [`product_user_arn_for`](Self::product_user_arn_for),
    /// matching `Domain` the same way for the same reason; the unqualified fallback reads
    /// only the associations of the instance the request names
    /// ([`instance_users_by_instance`](Self::instance_users_by_instance)), in association
    /// order.
    pub fn instance_user_arn_for(
        &self,
        provider_key: &str,
        instance_id: &str,
        username: &str,
        domain: Option<&str>,
    ) -> Option<&String> {
        if let Some(arn) = self.instance_user_index.get(&instance_user_key(
            provider_key,
            instance_id,
            username,
            domain,
        )) {
            return Some(arn);
        }
        if domain_key(domain).is_some() {
            return None;
        }
        self.instance_users_by_instance
            .get(instance_id)?
            .iter()
            .find(|arn| {
                self.instance_users.get(*arn).is_some_and(|record| {
                    record.provider_key == provider_key && record.username == username
                })
            })
    }

    /// Resolve a product user from the two ways `StopProductSubscription` can address
    /// one: its `ProductUserArn`, or the `(Username, IdentityProvider, Product, Domain)`
    /// identity it was subscribed with ([`product_user_arn_for`](Self::product_user_arn_for)).
    ///
    /// `ValidationException` when neither addressing is complete, `Ok(None)` for a
    /// `ProductUserArn` this account's store holds no live subscription under, and
    /// `ResourceNotFoundException` when the MEMBERS name no subscription in state.
    ///
    /// Whichever way a request addresses one, every member of a subscription's identity it
    /// names has to be the stored subscription's, for the reason the association plane
    /// resolves one the same way
    /// ([`resolve_instance_user`](Self::resolve_instance_user)): the members are what say
    /// which subscription the caller asked for.
    ///
    /// Only a LIVE subscription resolves: one whose end was already accepted is still
    /// REPORTED in its ended status ([`unsubscribe_started`]), but it is on its way out, so
    /// an operation that acts on a subscription is told the identifier names no
    /// subscription exactly as it is once the record is gone.
    ///
    /// One refusal per ADDRESSING, worded from the identifiers the REQUEST named and no
    /// others. A request that named the MEMBERS is refused with [`no_such_subscription`]
    /// for those members — whether the identity they name is held by nothing, by a
    /// subscription whose teardown is still running, or by a stored subscription some other
    /// member it named is not of. A request that supplied a `ProductUserArn` that resolves
    /// to nothing is a different fact: the ARN names a resource whose ownership by the
    /// caller cannot be established, which this service answers CLOSED — this is the
    /// `Ok(None)` the caller lifts into
    /// [`crate::scaffolded::Scaffolded::unauthorized`], exactly as the association plane
    /// and the license-server plane answer an ARN of their own that state does not hold, so
    /// no answer discloses whether an ARN resolves to anything or names an identifier this
    /// service minted.
    pub fn resolve_product_user(
        &self,
        product_user_arn: Option<&str>,
        username: Option<&str>,
        identity_provider: Option<&IdentityProvider>,
        product: Option<&str>,
        domain: Option<&str>,
    ) -> Result<Option<&ProductUserRecord>, StateError> {
        if let Some(arn) = product_user_arn {
            // THE FAIL-CLOSED ARM: an ARN this account's store holds no live subscription
            // under is not an identifier whose resource can be shown to be the caller's.
            let Some(record) = self.live_product_user(arn) else {
                return Ok(None);
            };
            // The provider union a request names is compared by the directory it names,
            // which is what the registration a subscription was stored through is indexed
            // under; a union naming no directory is a parameter that is not valid, exactly
            // as it is on the member-by-member addressing below.
            let named_provider_key = match identity_provider {
                Some(provider) => Some(provider_key(provider)?),
                None => None,
            };
            let stored_provider_key = self
                .identity_providers
                .get(&record.identity_provider_arn)
                .and_then(|registration| provider_key(&registration.identity_provider).ok());
            if names_other_option(named_provider_key.as_deref(), stored_provider_key.as_deref())
                || names_other(username, &record.username)
                || names_other(product, &record.product)
                || names_other_domain(domain, record.domain.as_deref())
            {
                // The members the request named name no subscription state holds, which is
                // the one wording of that refusal — each member the request left out is
                // the stored subscription's, so the sentence describes what was asked for.
                return Err(StateError::NotFound(no_such_subscription(
                    username.unwrap_or(&record.username),
                    product.unwrap_or(&record.product),
                    domain.or(record.domain.as_deref()),
                )));
            }
            return Ok(Some(record));
        }
        let (Some(username), Some(identity_provider), Some(product)) =
            (username, identity_provider, product)
        else {
            return Err(StateError::Validation(members_must_be_provided(&[
                "Username",
                "IdentityProvider",
                "Product",
            ])));
        };
        let provider = self.resolve_identity_provider(None, Some(identity_provider), Some(product))?;
        // ONE refusal for this whole addressing, because it is one fact: the identity the
        // request named has no LIVE subscription in state. That is just as true when the
        // identity index still holds an entry for it — it stays occupied while the
        // subscription it named is torn down
        // ([`begin_product_user_unsubscription`](Self::begin_product_user_unsubscription))
        // — as when it holds none, so the wording cannot depend on which of the two it is,
        // and the ARN that entry carries is not an identifier this request supplied.
        self.product_user_arn_for(&provider.arn, product, username, domain)
            .and_then(|arn| self.live_product_user(arn))
            .map(Some)
            .ok_or_else(|| StateError::NotFound(no_such_subscription(username, product, domain)))
    }

    /// Accept the deregistration of one registration: it enters `DEREGISTERING` now and
    /// stops being reported when the clock owes it, at [`DEREGISTRATION_SPAN`].
    ///
    /// Deregistration is ASYNCHRONOUS — License Manager has to tear down the VPC endpoints
    /// the registration provisioned, which is why the operation answers the non-terminal
    /// `DEREGISTERING` — so the record is NOT dropped here: a caller polling
    /// `ListIdentityProviders` keeps seeing the registration in `DEREGISTERING` until the
    /// teardown completes, instead of it vanishing between two reads. The
    /// `(identity provider, product)` index entry is kept for the same reason, so a
    /// register for that pair conflicts with the registration still being torn down rather
    /// than succeeding beside it.
    ///
    /// What the deregistration does take away at once is everything that depended on the
    /// registration being LIVE: the product users subscribed through it and the license
    /// server endpoints created for it (both are only reachable through their provider),
    /// its tags, and the products it contributed to the instance views. The endpoints are
    /// DELETED the same asynchronous way `DeleteLicenseServerEndpoint` deletes one — they
    /// enter `DELETING` and reach `DELETED` when the clock owes it — because how an
    /// endpoint's deletion becomes observable does not depend on what asked for it.
    ///
    /// An association names a directory and no product (`AssociateUser` carries no
    /// `Product`), so a user association outlives the deregistration of any ONE product
    /// for its directory — but not of the LAST one: the associations made through a
    /// directory that has no live registration left go with the integration they depended
    /// on, see
    /// [`drop_instance_users_orphaned_by`](Self::drop_instance_users_orphaned_by).
    ///
    /// Returns the record as the deregistration left it (`DEREGISTERING`), or `None` when
    /// the ARN names no stored registration. A registration whose deregistration was
    /// ALREADY accepted is returned unchanged, so a repeat neither restarts the teardown
    /// nor pushes its completion further out.
    pub fn begin_identity_provider_deregistration(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<IdentityProviderRecord> {
        let record = self.identity_providers.get_mut(arn)?;
        if deregistration_started(&record.status) {
            return Some(record.clone());
        }
        record.status = STATUS_DEREGISTERING.to_string();
        record.deregistration_started_at = Some(now);
        let due_at = now + DEREGISTRATION_SPAN;
        record.deregistered_due_at = Some(due_at);
        let deregistering = record.clone();

        self.remove_product_users_of(arn);
        let orphaned: Vec<String> = self
            .license_server_endpoints
            .values()
            .filter(|endpoint| endpoint.identity_provider_arn == arn)
            .map(|endpoint| endpoint.arn.clone())
            .collect();
        for endpoint_arn in orphaned {
            self.begin_license_server_endpoint_deletion(&endpoint_arn, now);
        }
        // The user associations made through this directory, when this was the last live
        // registration it had: nothing is associated through a directory this service no
        // longer integrates with.
        if let Ok(key) = provider_key(&deregistering.identity_provider) {
            self.drop_instance_users_orphaned_by(&key, now);
        }
        // Which products the instances in view provide is read from the LIVE
        // registrations, so a registration leaving that set changes those views.
        self.refresh_every_instance_view(now);
        self.tags.remove(arn);
        // The removal the clock now owes.
        self.schedule_record_transition(&deregistering);
        Some(deregistering)
    }

    /// Remove a registration from the store, the order it is listed in and the
    /// `(identity provider, product)` index that reaches it.
    ///
    /// This is the FINAL drop, which stops reporting the registration altogether and frees
    /// its `(identity provider, product)` pair; a deregistration the caller asked for goes
    /// through
    /// [`begin_identity_provider_deregistration`](Self::begin_identity_provider_deregistration)
    /// instead, so the `DEREGISTERING` status the operation answers stays observable.
    /// Anything still hanging off the registration goes with it, so no product user
    /// outlives the provider it was subscribed through however the drop was reached.
    pub fn remove_identity_provider(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<IdentityProviderRecord> {
        let record = self.drop_identity_provider(arn)?;
        // No association outlives the last live registration of its directory, however the
        // registration's removal was reached.
        if let Ok(key) = provider_key(&record.identity_provider) {
            self.drop_instance_users_orphaned_by(&key, now);
        }
        // Which products the instances in view provide is read from the registrations, so
        // removing one changes those views.
        self.refresh_every_instance_view(now);
        Some(record)
    }

    /// Drop one registration and everything held against it, WITHOUT recomputing the
    /// instance views.
    ///
    /// The one remover [`remove_identity_provider`](Self::remove_identity_provider) and the
    /// clock-owed sweep in [`materialise_due`](Self::materialise_due) share, so a sweep
    /// that drops several registrations recomputes those views ONCE instead of once per
    /// record: the recomputation reads the whole store, and this runs with the
    /// service-state lock held.
    fn drop_identity_provider(&mut self, arn: &str) -> Option<IdentityProviderRecord> {
        let record = self.identity_providers.remove(arn)?;
        self.identity_provider_order.retain(|a| a != arn);
        self.identity_provider_index
            .retain(|_, indexed| indexed != arn);
        self.remove_product_users_of(arn);
        self.tags.remove(arn);
        Some(record)
    }

    /// Remove every product user subscribed through the registration `provider_arn`
    /// names.
    ///
    /// Selects the subscriptions of the one registration and hands them to
    /// [`drop_product_users`](Self::drop_product_users), the one writer of a product
    /// user's removal, so this path cannot disagree with the other drop paths about what
    /// a removal takes away; the batched drop keeps the single pass over the subscription
    /// order and over the triple index that a registration with many subscribers needs.
    fn remove_product_users_of(&mut self, provider_arn: &str) {
        let orphaned: Vec<String> = self
            .product_users
            .values()
            .filter(|user| user.identity_provider_arn == provider_arn)
            .map(|user| user.arn.clone())
            .collect();
        self.drop_product_users(&orphaned);
    }

    /// Remove one product user from the store and from every index that reaches it.
    ///
    /// This is the FINAL drop, which stops reporting the subscription altogether and frees
    /// the [`product_user_key`] identity it occupied; an end of
    /// subscription the caller asked for goes through
    /// [`begin_product_user_unsubscription`](Self::begin_product_user_unsubscription)
    /// instead, so the ended subscription stays observable until the teardown the clock
    /// owes completes.
    pub fn remove_product_user(&mut self, arn: &str) -> Option<ProductUserRecord> {
        let record = self.product_users.get(arn)?.clone();
        self.drop_product_users(std::slice::from_ref(&record.arn));
        Some(record)
    }

    /// Drop the product users `arns` names from the store and from every index that reaches
    /// them.
    ///
    /// The ONE remover of a product user — every path that stops reporting a subscription
    /// (one user's final drop, the clock-owed sweep, the removal of the registration the
    /// user is subscribed through) comes through here, so no path can leave the three
    /// stores disagreeing about what is subscribed.
    ///
    /// Batched on purpose: the subscription ORDER and the triple index are each rebuilt in
    /// ONE pass however many subscriptions are dropped together, so a request that finds
    /// several teardowns due does not pay a scan of either list per record with the
    /// service-state lock held.
    fn drop_product_users(&mut self, arns: &[String]) {
        let mut dropped: HashSet<String> = HashSet::new();
        for arn in arns {
            if self.product_users.remove(arn).is_some() {
                self.tags.remove(arn);
                dropped.insert(arn.clone());
            }
        }
        if dropped.is_empty() {
            return;
        }
        self.product_user_order.retain(|arn| !dropped.contains(arn));
        self.product_user_index
            .retain(|_, indexed| !dropped.contains(indexed));
    }

    /// Accept the end of one product user's subscription: it enters [`STATUS_UNSUBSCRIBED`]
    /// now, reports the instant it ended, and owes the clock the completion of its teardown
    /// at [`UNSUBSCRIPTION_SPAN`], which is what finally stops reporting it.
    ///
    /// Ending a user-based subscription is a teardown the service REPORTS while it runs, so
    /// the record is NOT dropped here: a client polling `ListProductSubscriptions` keeps
    /// seeing the subscription in its ended status, with the end date the stop stamped,
    /// instead of it vanishing between two reads. The
    /// [`product_user_key`] index entry is KEPT, because that identity
    /// is still occupied: the user's subscription to that product is still being torn down,
    /// so it cannot be subscribed again until the teardown has completed. What the end of
    /// the subscription does take away immediately is what made it a LIVE subscription: its
    /// tags, so the tagging surface no longer resolves it (see
    /// [`holds_resource`](Self::holds_resource)).
    ///
    /// Returns the record as the end left it (`UNSUBSCRIBED`), or `None` when the ARN names
    /// no stored subscription. A subscription whose end was ALREADY accepted is returned
    /// unchanged, so a repeat neither restarts the teardown nor re-stamps the instant the
    /// subscription ended.
    pub fn begin_product_user_unsubscription(
        &mut self,
        arn: &str,
        now: SystemTime,
    ) -> Option<ProductUserRecord> {
        let record = self.product_users.get_mut(arn)?;
        if unsubscribe_started(&record.status) {
            return Some(record.clone());
        }
        record.status = STATUS_UNSUBSCRIBED.to_string();
        record.unsubscribed_at = Some(now);
        let due_at = now + UNSUBSCRIPTION_SPAN;
        record.unsubscribed_due_at = Some(due_at);
        let ended = record.clone();
        self.tags.remove(arn);
        // The teardown the clock now owes, which is what stops reporting the subscription.
        self.schedule_record_transition(&ended);
        Some(ended)
    }
}

/// The key a registration is indexed by, derived from the provider union.
///
/// `IdentityProvider` is a union whose only member is `ActiveDirectoryIdentityProvider`,
/// which identifies a directory either by `DirectoryId` (an AWS Managed directory) or by
/// the domain name in `ActiveDirectorySettings` (a self-managed one). One of the two must
/// be present, or the provider names no directory at all and nothing can be registered
/// for it — a combination the model cannot express, so it is refused here.
pub fn provider_key(identity_provider: &IdentityProvider) -> Result<String, StateError> {
    match identity_provider {
        IdentityProvider::ActiveDirectoryIdentityProvider(ad) => {
            if let Some(directory_id) = ad.directory_id.as_deref().filter(|id| !id.is_empty()) {
                return Ok(format!("directory/{directory_id}"));
            }
            let domain = ad
                .active_directory_settings
                .as_ref()
                .and_then(|settings| settings.domain_name.as_deref())
                .filter(|name| !name.is_empty());
            match domain {
                Some(domain) => Ok(format!("domain/{domain}")),
                None => Err(StateError::Validation(
                    "IdentityProvider.ActiveDirectoryIdentityProvider must specify either \
                     DirectoryId or ActiveDirectorySettings.DomainName"
                        .to_string(),
                )),
            }
        }
    }
}

/// The identity of ONE product subscription:
/// `(IdentityProviderArn, Product, Username, domain)`.
pub type ProductUserKey = (String, String, String, Option<String>);

/// The identity of ONE user association:
/// `(provider key, InstanceId, Username, domain)`.
pub type InstanceUserKey = (String, String, String, Option<String>);

/// The `Domain` half of a user's identity, normalised.
///
/// `Domain` is "the domain name of the Active Directory that contains the user" a
/// `StartProductSubscription`/`AssociateUser` subscribes or associates, and the
/// `StopProductSubscription`/`DisassociateUser` that ends one carries it BESIDE the user
/// name, the provider reference and the product or instance — so it is part of which user
/// a request names, not a comment on it. A request that supplies none, and one that
/// supplies the empty string, name the same unqualified user, so both key the same way.
pub fn domain_key(domain: Option<&str>) -> Option<String> {
    domain
        .filter(|domain| !domain.is_empty())
        .map(str::to_string)
}

/// Whether a member a request NAMES is not the stored resource's.
///
/// Every operation here addresses a resource either by its ARN or by the members that
/// identify it, and a request may supply BOTH. Each member it names is part of which
/// resource the caller asked for — that is all the member-by-member addressing is — so a
/// named value that is not the stored one means the request names no resource state holds,
/// however the rest of it addressed one. A member the request leaves out names nothing, so
/// there is nothing to compare: it does not narrow what the ARN resolved.
///
/// The ONE place that comparison is made, so no resolution can read a member the caller
/// named as a comment on the resource rather than as part of naming it. A member whose
/// stored counterpart is itself optional goes through [`names_other_option`], and `Domain`
/// through [`names_other_domain`], which both decide it the same way.
pub fn names_other(named: Option<&str>, stored: &str) -> bool {
    names_other_option(named, Some(stored))
}

/// [`names_other`] where the STORED value is optional too: a member the request names and
/// the resource does not have is not the resource's either, so it names another resource.
pub fn names_other_option(named: Option<&str>, stored: Option<&str>) -> bool {
    named.is_some_and(|named| stored != Some(named))
}

/// [`names_other`] for `Domain`, which is normalised before it is compared
/// ([`domain_key`]): the empty value and no value name the same unqualified user, so
/// neither narrows what was resolved.
pub fn names_other_domain(named: Option<&str>, stored: Option<&str>) -> bool {
    names_other_option(domain_key(named).as_deref(), domain_key(stored).as_deref())
}

/// The license-server settings a request NAMES, reduced to one comparable value.
///
/// `LicenseServerSettings` is a tree of structures and unions — the type of license server
/// and, under it, the credentials provider license-server user administration runs
/// through — and every part of it is a `@required` member of what the endpoint was BUILT
/// from, not a comment on an endpoint that already exists. So settings that are not a
/// stored endpoint's name no endpoint this service holds, which is the decision
/// [`names_other`] makes for a member addressed on its own.
///
/// The ONE place that tree is reduced to a value two of them can be compared by, so no
/// resolution can compare one part of it and ignore the rest.
pub fn license_server_settings_key(settings: &LicenseServerSettings) -> String {
    let ServerSettings::RdsSalSettings(rds_sal) = &settings.server_settings;
    format!(
        "{}/RdsSalSettings/{}",
        settings.server_type,
        credentials_provider_key(&rds_sal.rds_sal_credentials_provider)
    )
}

/// One `CredentialsProvider` reference, reduced for the same comparison: the union member
/// the request chose and the secret that member names.
///
/// `SecretsManagerCredentialsProvider$SecretId` is not `@required`, so a provider that
/// names no secret is its own value rather than equal to one that names any secret id.
pub fn credentials_provider_key(provider: &CredentialsProvider) -> String {
    let CredentialsProvider::SecretsManagerCredentialsProvider(secret) = provider;
    match secret.secret_id.as_deref() {
        Some(secret_id) => format!("SecretsManagerCredentialsProvider/secret/{secret_id}"),
        None => "SecretsManagerCredentialsProvider/no-secret".to_string(),
    }
}

/// The identity of the product subscription a request addresses, or a record holds.
///
/// The ONE place that key is built, so the occupancy a `StartProductSubscription` refuses
/// a duplicate on and the lookup a `StopProductSubscription` resolves one by cannot
/// disagree about which user is which.
pub fn product_user_key(
    identity_provider_arn: &str,
    product: &str,
    username: &str,
    domain: Option<&str>,
) -> ProductUserKey {
    (
        identity_provider_arn.to_string(),
        product.to_string(),
        username.to_string(),
        domain_key(domain),
    )
}

/// The identity of the user association a request addresses, or a record holds — the
/// association plane's counterpart of [`product_user_key`], built the one place for the
/// same reason.
pub fn instance_user_key(
    provider_key: &str,
    instance_id: &str,
    username: &str,
    domain: Option<&str>,
) -> InstanceUserKey {
    (
        provider_key.to_string(),
        instance_id.to_string(),
        username.to_string(),
        domain_key(domain),
    )
}

/// The `DirectoryId` of a provider union, when it names one.
///
/// `ListIdentityProviders` documents `DirectoryId` as a filterable attribute, so the
/// value a filter compares against comes from here.
pub fn directory_id(identity_provider: &IdentityProvider) -> Option<&str> {
    match identity_provider {
        IdentityProvider::ActiveDirectoryIdentityProvider(ad) => ad.directory_id.as_deref(),
    }
}

/// Format `at` the way this service's date members carry an instant: ISO-8601 UTC at
/// whole-second precision, e.g. `2025-03-15T00:00:00Z` — the format the model's
/// `LicenseExpirationDate` documentation shows.
pub fn iso8601(at: SystemTime) -> String {
    let seconds = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_secs() as i64)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs(seconds)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_else(|_| "1970-01-01T00:00:00Z".to_string())
}

/// The settings a registration carries after an `UpdateSettings` payload is applied to
/// them: the subnets it had, plus `AddSubnets`, minus `RemoveSubnets`, with
/// `SecurityGroupId` replaced when the payload names one.
///
/// Two payloads cannot be applied at all, and both are refused as invalid rather than
/// half-applied:
///
/// * a subnet in BOTH `AddSubnets` and `RemoveSubnets` asks for two contradictory things
///   at once, so there is no resulting set;
/// * a payload that TAKES AWAY the last subnet the registration carries contradicts
///   `Settings$Subnets`, which the model declares `@required` with `@length(min: 1)`.
///
/// That minimum is something an update can only violate by taking the registration below
/// it. A registration stored with no subnet at all is already below it — `Settings` is
/// optional on `RegisterIdentityProvider`, so a registration that provisions no
/// activation-server VPC endpoints carries the empty configuration — and a payload that
/// removes nothing from it leaves its subnets exactly as they were, which is not a
/// violation this request commits: such a payload is applied, so the `SecurityGroupId` of
/// a registration that holds no subnet can be set at all.
///
/// Order is preserved and duplicates collapse, so the result is the SET the caller asked
/// for however the payload spelled it; removing a subnet the registration does not carry
/// is a no-op, which is what the operation's `@idempotent` trait asks for.
///
/// Every step is LINEAR in the lists involved, and each list is bounded by
/// [`MAX_SUBNETS_PER_UPDATE`] before any of it runs: this function is called with the
/// service-state lock held, so the caller must not be able to buy quadratic (or
/// unbounded) work under it with one request.
pub fn apply_update_settings(
    settings: &Settings,
    add_subnets: &[String],
    remove_subnets: &[String],
    security_group_id: Option<&str>,
) -> Result<Settings, StateError> {
    for (member, list) in [
        ("AddSubnets", add_subnets),
        ("RemoveSubnets", remove_subnets),
    ] {
        if list.len() > MAX_SUBNETS_PER_UPDATE {
            return Err(StateError::Validation(format!(
                "UpdateSettings.{member} names {} subnets; at most {MAX_SUBNETS_PER_UPDATE} \
                 may be named in one update",
                list.len()
            )));
        }
    }

    let removed: HashSet<&str> = remove_subnets.iter().map(String::as_str).collect();
    let overlapping: Vec<&str> = add_subnets
        .iter()
        .map(String::as_str)
        .filter(|subnet| removed.contains(subnet))
        .collect();
    if !overlapping.is_empty() {
        return Err(StateError::Validation(format!(
            "UpdateSettings.AddSubnets and UpdateSettings.RemoveSubnets both name {}; \
             a subnet cannot be added and removed by one update",
            overlapping.join(", ")
        )));
    }

    let mut subnets: Vec<String> = Vec::new();
    let mut kept: HashSet<&str> = HashSet::new();
    for subnet in settings.subnets.iter().chain(add_subnets) {
        if !removed.contains(subnet.as_str()) && kept.insert(subnet.as_str()) {
            subnets.push(subnet.clone());
        }
    }
    // Only a fold that EMPTIED a non-empty set took the last subnet away; a result that is
    // empty because the registration held none is the state the store already carries.
    if subnets.is_empty() && !settings.subnets.is_empty() {
        return Err(StateError::Validation(
            "UpdateSettings would leave the registration with no subnet, and \
             Settings.Subnets requires at least one"
                .to_string(),
        ));
    }

    Ok(Settings {
        subnets,
        security_group_id: security_group_id
            .map(str::to_string)
            .unwrap_or_else(|| settings.security_group_id.clone()),
    })
}

/// The `IdentityProviderSummary` the model requires, built entirely from the stored
/// record — every member is what registration persisted.
pub fn identity_provider_summary(record: &IdentityProviderRecord) -> IdentityProviderSummary {
    IdentityProviderSummary {
        identity_provider: record.identity_provider.clone(),
        settings: record.settings.clone(),
        product: record.product.clone(),
        status: record.status.clone(),
        identity_provider_arn: Some(record.arn.clone()),
        failure_message: record.failure_message.clone(),
        owner_account_id: Some(record.owner_account_id.clone()),
    }
}

/// The `InstanceSummary` the model requires, built entirely from the stored record —
/// every member is what the writer persisted, and an absent one stays absent.
pub fn instance_summary(record: &InstanceRecord) -> InstanceSummary {
    InstanceSummary {
        instance_id: record.instance_id.clone(),
        status: record.status.clone(),
        products: record.products.clone(),
        last_status_check_date: record.last_status_check_at.map(iso8601),
        status_message: record.status_message.clone(),
        owner_account_id: Some(record.owner_account_id.clone()),
        identity_provider: record.identity_provider.clone(),
    }
}

/// The region this request was signed for, or the emulator's default when the request
/// carries no usable SigV4 credential scope.
///
/// Values outside the ARN grammar's region segment (`[a-z0-9-.]{1,63}`) are ignored, so
/// a minted ARN always satisfies the model's `Arn` pattern.
pub fn request_region(headers: &::http::HeaderMap) -> String {
    ::emulator_server::sigv4_credential_scope(headers)
        .map(|scope| scope.region)
        .filter(|region| {
            !region.is_empty()
                && region.len() <= 63
                && region
                    .bytes()
                    .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit() || b == b'-' || b == b'.')
        })
        .unwrap_or_else(|| "us-east-1".to_string())
}

/// An ARN for one of this service's resources: `arn:aws:<signing name>:<region>:<account>:<kind>/<id>`.
pub fn resource_arn(signing_name: &str, region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(signing_name, region, &format!("{kind}/{id}"))
}

/// The instant a registration created at `created_at` becomes `REGISTERED`.
pub fn registration_due_at(created_at: SystemTime) -> SystemTime {
    created_at + REGISTRATION_SPAN
}

/// The instant an association created at `created_at` becomes `ASSOCIATED`.
pub fn association_due_at(created_at: SystemTime) -> SystemTime {
    created_at + ASSOCIATION_SPAN
}

/// The `InstanceUserSummary` the model requires, built entirely from the stored
/// association — every member is what the association persisted.
pub fn instance_user_summary(record: &InstanceUserRecord) -> InstanceUserSummary {
    InstanceUserSummary {
        username: record.username.clone(),
        instance_id: record.instance_id.clone(),
        identity_provider: record.identity_provider.clone(),
        status: record.status.clone(),
        instance_user_arn: Some(record.arn.clone()),
        status_message: record.status_message.clone(),
        domain: record.domain.clone(),
        association_date: record.association_date.clone(),
        disassociation_date: record.disassociation_date.clone(),
    }
}

/// The instant a license server endpoint created at `created_at` becomes `PROVISIONED`.
pub fn endpoint_provisioned_due_at(created_at: SystemTime) -> SystemTime {
    created_at + ENDPOINT_PROVISIONING_SPAN
}

/// The network address of the license server endpoint `id`, in the regional form this
/// emulator mints for a service-owned endpoint: `<id>.<signing name>.<region>.amazonaws.com`.
pub fn endpoint_address(signing_name: &str, region: &str, id: &str) -> String {
    format!("{id}.{signing_name}.{region}.amazonaws.com")
}

/// The IPv4 block this emulator allocates license-server host addresses out of.
///
/// A license server runs on a host in the account's own VPC, so the address a client
/// reaches it on is a private one: `10.0.0.0/8`, the largest block RFC 1918 reserves for
/// private use, which is also the block the subnets of a VPC are carved out of.
const HOST_IPV4_BLOCK: &str = "10";

/// The IPv6 block this emulator allocates license-server host addresses out of.
///
/// `fd00::/8`, the unique-local block RFC 4193 reserves for addresses that are routable
/// only inside a site — the IPv6 counterpart of the private IPv4 block above.
const HOST_IPV6_BLOCK: &str = "fd00";

/// Whether the Active Directory `identity_provider` describes uses IPv6: it recorded at
/// least one IPv6 domain address (`ActiveDirectorySettings$DomainIpv6List`).
///
/// A host provisioned into such a domain is reachable over IPv6 as well as IPv4, so it is
/// given both addresses; one in an IPv4-only domain is given no IPv6 address rather than
/// one nothing reaches.
pub fn domain_uses_ipv6(identity_provider: &IdentityProvider) -> bool {
    let IdentityProvider::ActiveDirectoryIdentityProvider(active_directory) = identity_provider;
    active_directory
        .active_directory_settings
        .as_ref()
        .and_then(|settings| settings.domain_ipv6_list.as_ref())
        .is_some_and(|addresses| !addresses.is_empty())
}

/// The `LicenseServer` the model describes, built from the stored host and the
/// provisioning status of the endpoint the host was provisioned for.
///
/// A host is provisioned, and torn down, WITH its endpoint — nothing provisions or
/// deletes one on its own — so its `ProvisioningStatus` is the endpoint's.
/// `HealthStatus` is a verdict about a serving host: `HEALTHY` once the host is
/// provisioned, and [`HEALTH_STATUS_NOT_APPLICABLE`] in every other status, where there
/// is no serving host to report a verdict about. The addresses are reported from the
/// same instant, for the same reason the endpoint's own address is: there is nothing to
/// reach before provisioning completed.
pub fn license_server(host: &LicenseServerHost, provisioning_status: &str) -> LicenseServer {
    let provisioned = provisioning_status == PROVISIONING_STATUS_PROVISIONED;
    let health = if provisioned {
        HEALTH_STATUS_HEALTHY
    } else {
        HEALTH_STATUS_NOT_APPLICABLE
    };
    LicenseServer {
        provisioning_status: Some(provisioning_status.to_string()),
        health_status: Some(health.to_string()),
        ipv4_address: provisioned.then(|| host.ipv4_address.clone()),
        ipv6_address: provisioned.then(|| host.ipv6_address.clone()).flatten(),
    }
}

/// The `LicenseServerEndpoint` the model describes, built entirely from the stored
/// record.
///
/// `ServerEndpoint` is reported only once the endpoint is `PROVISIONED` — there is no
/// address to connect to while it is still being provisioned. `LicenseServers` reports
/// the hosts the endpoint was provisioned with, each through [`license_server`], so a
/// client polling the list can watch them come up and read their health; an endpoint
/// provisioned with no host reports an empty list, which is what it has.
/// `StatusMessage` is only what the record carries.
pub fn license_server_endpoint(record: &LicenseServerEndpointRecord) -> LicenseServerEndpoint {
    let provisioned = record.provisioning_status == PROVISIONING_STATUS_PROVISIONED;
    LicenseServerEndpoint {
        identity_provider_arn: Some(record.identity_provider_arn.clone()),
        server_type: Some(record.server_type.clone()),
        server_endpoint: provisioned.then(|| ServerEndpoint {
            endpoint: Some(record.endpoint.clone()),
        }),
        status_message: record.status_message.clone(),
        license_server_endpoint_id: Some(record.id.clone()),
        license_server_endpoint_arn: Some(record.arn.clone()),
        license_server_endpoint_provisioning_status: Some(record.provisioning_status.clone()),
        license_servers: Some(
            record
                .license_servers
                .iter()
                .map(|host| license_server(host, &record.provisioning_status))
                .collect(),
        ),
        creation_time: Some(::aws_smithy_types::DateTime::from(record.created_at)),
    }
}

/// One page of `items`, plus the token that reaches the next one.
///
/// `MaxResults` is the page size; a `NextToken` is this service's opaque cursor, minted
/// by the previous page of the SAME list (`kind` names that list: the operation and
/// whatever selected it) and rejected otherwise — a token is not a client-chosen value,
/// so an unparseable or foreign one is a `ValidationException` rather than a silent
/// restart from the beginning. A page size below 1 would make no progress, so it is
/// refused too.
pub fn paginate<T: Clone>(
    items: &[T],
    kind: &str,
    max_results: Option<i32>,
    next_token: Option<&str>,
) -> Result<(Vec<T>, Option<String>), StateError> {
    let offset = match next_token {
        Some(token) => decode_page_token(kind, token)?,
        None => 0,
    };
    if offset > items.len() {
        return Err(StateError::Validation(
            "NextToken does not refer to a position in this list".to_string(),
        ));
    }
    let page_size = match max_results {
        Some(max) if max < 1 => {
            return Err(StateError::Validation(format!(
                "MaxResults must be at least 1, got {max}"
            )))
        }
        Some(max) => max as usize,
        None => items.len(),
    };
    let end = offset.saturating_add(page_size).min(items.len());
    let page = items[offset..end].to_vec();
    let token = if end < items.len() {
        Some(encode_page_token(kind, end))
    } else {
        None
    };
    Ok((page, token))
}

/// This service's opaque pagination cursor: the list it belongs to and the offset it
/// resumes at.
///
/// Hex is an encoding, not a secret — a client can decode one. What makes a cursor
/// trustworthy here is that [`decode_page_token`] honours only a token this function
/// minted for the same list, so a forged or stale one is refused rather than applied to a
/// different list or to a position no page handed out.
fn encode_page_token(kind: &str, offset: usize) -> String {
    ::hex::encode(format!("{kind}:{offset}"))
}

/// Decode a cursor minted by [`encode_page_token`] for the same list.
///
/// The offset is split off the END, so a list identity may itself contain `:`.
///
/// A token is honoured only when it is, byte for byte, one [`encode_page_token`] mints for
/// this list — the decoder accepts exactly the spellings the minter writes and no others.
/// That matters because both layers it reads through are wider than the minter: `hex`
/// decodes upper- and mixed-case digits, and `usize`'s `FromStr` accepts a leading `+` and
/// any number of leading zeros, so a decode that stopped at "it parsed" would serve a page
/// for a cursor no page of any list ever handed out. Re-minting the cursor from what was
/// decoded and comparing it with what arrived is what rejects every such spelling at once,
/// without this function having to enumerate them.
fn decode_page_token(kind: &str, token: &str) -> Result<usize, StateError> {
    let invalid = || StateError::Validation("NextToken is not a valid pagination token".to_string());
    let decoded = ::hex::decode(token).map_err(|_| invalid())?;
    let decoded = String::from_utf8(decoded).map_err(|_| invalid())?;
    let (token_kind, offset) = decoded.rsplit_once(':').ok_or_else(invalid)?;
    if token_kind != kind {
        return Err(invalid());
    }
    let offset = offset.parse::<usize>().map_err(|_| invalid())?;
    if encode_page_token(kind, offset) != token {
        return Err(invalid());
    }
    Ok(offset)
}

/// A deterministic rendering of a filter set, for the list identity a pagination cursor
/// is minted against: a cursor is a position in ONE list, so it must not be accepted for
/// a list selected by different filters.
///
/// Every component is LENGTH-PREFIXED, so no two different filter sets can render to the
/// same signature — a separator appearing inside an attribute, operation or value cannot
/// make one set look like another.
pub fn filter_signature(filters: &[Filter]) -> String {
    let mut signature = String::new();
    for filter in filters {
        for part in [
            filter.attribute.as_deref().unwrap_or_default(),
            filter.operation.as_deref().unwrap_or_default(),
            filter.value.as_deref().unwrap_or_default(),
        ] {
            signature.push_str(&format!("{}.{part}", part.len()));
        }
    }
    signature
}

/// True when `attributes` satisfies every filter in `filters`.
///
/// `Filter.Attribute` names one of the attributes the calling operation's model
/// documents as filterable (`Product`/`DirectoryId` for `ListIdentityProviders`,
/// `Status`/`Username`/`Domain` for `ListProductSubscriptions`, `IdentityProviderArn`
/// for `ListLicenseServerEndpoints`): the caller passes those
/// as `(name, value)` pairs, matched EXACTLY as the documentation spells them, so a typo
/// is refused rather than silently honoured. `Filter.Operation` is the kind of search —
/// the model documents it as "the type of search (For example, eq, geq, leq)", so the
/// whole relational family those examples belong to (`eq`, `ne`, `gt`, `geq`, `lt`, `leq`)
/// is honoured, alongside the substring searches that are meaningful for these string
/// attributes, under either the short documented spelling or its PascalCase name; anything
/// else is refused, and an absent operation is equality. Every filterable attribute here
/// is a string, so the ordering comparisons order values lexicographically.
///
/// Whether a filter is WELL FORMED does not depend on what the store holds, so
/// [`validate_filters`] is what a list operation calls first, before it looks at a single
/// record: otherwise an empty (or fully filtered-out) store would answer 200 for a
/// request this service refuses.
pub fn filters_match(
    filters: &[Filter],
    attributes: &[(&str, Option<&str>)],
) -> Result<bool, StateError> {
    for filter in filters {
        let (index, operation) = parse_filter(filter, &attribute_names(attributes))?;
        let actual = attributes[index].1.unwrap_or_default();
        let wanted = filter.value.as_deref().unwrap_or_default();
        let matched = match operation {
            FilterOperation::Equals => actual == wanted,
            FilterOperation::NotEquals => actual != wanted,
            FilterOperation::GreaterThan => actual > wanted,
            FilterOperation::GreaterThanOrEquals => actual >= wanted,
            FilterOperation::LessThan => actual < wanted,
            FilterOperation::LessThanOrEquals => actual <= wanted,
            FilterOperation::Contains => actual.contains(wanted),
            FilterOperation::BeginsWith => actual.starts_with(wanted),
        };
        if !matched {
            return Ok(false);
        }
    }
    Ok(true)
}

/// Refuse a filter set that names an attribute this operation does not expose, or a
/// comparison this service does not implement — independently of the records in state.
pub fn validate_filters(filters: &[Filter], attribute_names: &[&str]) -> Result<(), StateError> {
    for filter in filters {
        parse_filter(filter, attribute_names)?;
    }
    Ok(())
}

/// The comparison a `Filter.Operation` names.
///
/// The ordering members are the family the model's own examples (`eq`, `geq`, `leq`) are
/// drawn from; on the string attributes these lists expose they compare lexicographically.
#[derive(Debug, Clone, Copy)]
enum FilterOperation {
    Equals,
    NotEquals,
    GreaterThan,
    GreaterThanOrEquals,
    LessThan,
    LessThanOrEquals,
    Contains,
    BeginsWith,
}

/// Every `Filter.Operation` spelling this service honours, paired with the comparison it
/// names: the relational family the model documents by example (`eq`, `geq`, `leq`) under
/// its short spelling and its PascalCase name, plus the substring searches. Matched
/// case-insensitively, so `geq`, `GEQ` and `GreaterThanOrEquals` all name one comparison.
const FILTER_OPERATIONS: &[(&str, FilterOperation)] = &[
    ("eq", FilterOperation::Equals),
    ("equals", FilterOperation::Equals),
    ("ne", FilterOperation::NotEquals),
    ("notequals", FilterOperation::NotEquals),
    ("gt", FilterOperation::GreaterThan),
    ("greaterthan", FilterOperation::GreaterThan),
    ("geq", FilterOperation::GreaterThanOrEquals),
    ("greaterthanorequals", FilterOperation::GreaterThanOrEquals),
    ("lt", FilterOperation::LessThan),
    ("lessthan", FilterOperation::LessThan),
    ("leq", FilterOperation::LessThanOrEquals),
    ("lessthanorequals", FilterOperation::LessThanOrEquals),
    ("contains", FilterOperation::Contains),
    ("beginswith", FilterOperation::BeginsWith),
];

/// The names of the attributes a list operation exposes, in the order it passed them.
fn attribute_names<'a>(attributes: &[(&'a str, Option<&str>)]) -> Vec<&'a str> {
    attributes.iter().map(|(name, _)| *name).collect()
}

/// How this service refuses a `Filter.Attribute` naming an attribute the list operation
/// it was sent to does not filter on.
///
/// The template is the service's: the fixed text `Invalid Filter attribute ` followed by
/// the ONE variable part — the rejected attribute name, copied from the request's
/// `Filter.Attribute` exactly as it arrived. The set of attributes the operation DOES
/// accept is not part of it: the refusal states what the request asked for, and the
/// filterable set is what the operation's documentation advertises.
///
/// ONE wording for every list operation of this service, because every one of them reaches
/// it through [`parse_filter`]: which attributes are filterable differs per operation, how
/// an unfilterable one is refused does not.
pub fn invalid_filter_attribute(attribute: &str) -> String {
    format!("Invalid Filter attribute {attribute}")
}

/// Resolve one filter against the attributes a list operation exposes: which attribute it
/// names (by index) and which comparison it asks for.
fn parse_filter(
    filter: &Filter,
    attribute_names: &[&str],
) -> Result<(usize, FilterOperation), StateError> {
    let Some(attribute) = filter.attribute.as_deref() else {
        return Err(StateError::Validation(
            "Filter.Attribute is required".to_string(),
        ));
    };
    let index = attribute_names
        .iter()
        .position(|name| *name == attribute)
        .ok_or_else(|| StateError::Validation(invalid_filter_attribute(attribute)))?;
    // An absent operation is equality: the filter is then just an attribute/value pair.
    let spelling = filter.operation.as_deref().unwrap_or("eq");
    let folded = spelling.to_ascii_lowercase();
    let operation = FILTER_OPERATIONS
        .iter()
        .find(|(name, _)| *name == folded)
        .map(|(_, operation)| *operation)
        .ok_or_else(|| {
            StateError::Validation(format!(
                "Filter.Operation {spelling} is not supported; supported operations: {}",
                FILTER_OPERATIONS
                    .iter()
                    .map(|(name, _)| *name)
                    .collect::<Vec<_>>()
                    .join(", ")
            ))
        })?;
    Ok((index, operation))
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsLicensemanagerusersubscriptionsLicensemanagerusersubscriptionsState>)
}

/// Whether `arn` addresses a resource of the account and Region THIS request is for.
///
/// An ARN carries both (`arn:<partition>:<vendor>:<region>:<account>:<resource>`), and
/// every ARN this service mints carries the Region the request was signed for together with
/// this emulator's account id ([`resource_arn`]). So the scope a request-supplied ARN is
/// read in is taken from the REQUEST — its SigV4 credential scope, through
/// [`request_region`], and the account this process serves — rather than believed from the
/// ARN: an ARN naming another account or another Region addresses nothing this account's
/// store holds in this Region, and is not an identifier whose resource can be shown to be
/// the caller's.
///
/// The ARN is read through `service_common::arn::parse`, the one parse in this tree, so this
/// service cannot disagree with another about what an ARN is: `false` for a value that is
/// not a well-formed resource ARN, which is the same answer for the same reason — such a
/// value identifies nothing. The VENDOR field is not matched here: an ARN of another service
/// names no user association of this one either, and the store lookup is what decides that.
fn addresses_request_scope(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    arn: &str,
) -> bool {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return false;
    };
    parsed.region == request_region(&ctx.parts().headers)
        && parsed.account == ::engine::emulator_account_id()
}

/// THE GATE for a request that names a user association by its `InstanceUserArn`: `None`
/// when the request goes on — either because it names no association, or because the
/// account's store holds the one it names — and the fail-closed refusal otherwise.
///
/// The ARN half of [`ec2_fleet::instance_refusal`], and the same decision about the same
/// thing: an `InstanceUserArn` identifies a user association ON an EC2 instance, so a
/// request that addresses one by ARN names that instance's association and nothing else,
/// and an ARN this account's store holds no association for cannot be shown to be the
/// caller's. The refusal is the value the handler returns on its own error channel for the
/// same fact ([`crate::scaffolded::ServiceRefusal::unauthorized`]), so the code, the status
/// and the sentence are not restated here and the two layers cannot answer one refusal two
/// ways.
///
/// The SCOPE the ARN is read in comes from `ctx`, exactly as the instance half takes the
/// Region it asks EC2 in from there: an ARN outside the account and Region the request is
/// for is refused before the store is consulted at all
/// ([`addresses_request_scope`]). `ctx` is required rather than optional so that scoping is
/// a fact about the signature instead of a promise in this comment.
///
/// A READ: nothing is materialised and nothing is written. An association whose teardown the
/// clock already owes is still held here, so the gate lets such a request through to the
/// handler, which materialises what is due and answers this very refusal for it. `None` when
/// this process holds no store for the service at all — there is then nothing to decide
/// here, and the handler reports that as the server failure it is.
pub fn instance_user_refusal<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    instance_user_arn: Option<&str>,
) -> Option<crate::scaffolded::ServiceRefusal> {
    let arn = instance_user_arn?;
    if !addresses_request_scope(ctx, arn) {
        return Some(crate::scaffolded::ServiceRefusal::unauthorized());
    }
    let state = service_state(engine)?;
    let held = state
        .lock()
        .resolve_instance_user(Some(arn), None, None, None, None)
        .is_some();
    if held {
        return None;
    }
    Some(crate::scaffolded::ServiceRefusal::unauthorized())
}
