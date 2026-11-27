// SPDX-License-Identifier: CC0-1.0

//! The STUDIO DOMAIN slice of this service's state: the domain `CreateDomain` writes,
//! `DescribeDomain` replays, `ListDomains` pages over and `DeleteDomain` tears down, with
//! its ARN kind, its id mint, its Studio address, its statuses, the instant its clock-owed
//! transitions are due from, its store bound and the sentences its refusals are worded
//! with.
//!
//! Its own file, like the other resource slices here, so the parent module stays inside
//! the project's file-length limit. Every name is re-exported from [`super`], so a
//! handler keeps naming `crate::state::…`.

use super::*;
use crate::ops::types::{
    DefaultSpaceSettings, DomainSettings, StudioWebPortalSettings, UserSettings,
};

/// The ARN resource kind a domain carries, i.e. the `<kind>` in [`resource_arn`]. Fixed
/// by `DomainArn`'s modelled pattern
/// (`^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:domain/`), and named once here so
/// the create and every reader mint and match the same ARN.
pub const DOMAIN_ARN_KIND: &str = "domain";

/// See [`MODEL_QUOTA`]. Bounds the domain store the same way, and is what `CreateDomain`
/// answers `ResourceLimitExceeded` past, so a caller creating domain after domain cannot
/// grow the store without end.
pub const DOMAIN_QUOTA: usize = 1_000;

/// `DomainStatus`'s members a domain of this emulator moves through. The modelled enum
/// also carries the delete and update failure states, which no implemented operation
/// reaches yet.
pub const DOMAIN_STATUS_PENDING: &str = "Pending";
/// See [`DOMAIN_STATUS_PENDING`].
pub const DOMAIN_STATUS_IN_SERVICE: &str = "InService";
/// See [`DOMAIN_STATUS_PENDING`]. The terminal status a domain whose provisioning could not
/// complete settles in — a placement the account's network does not admit
/// ([`crate::state::domain_network`]) — beside the `FailureReason` that states why.
pub const DOMAIN_STATUS_FAILED: &str = "Failed";
/// See [`DOMAIN_STATUS_PENDING`]. What a domain reports from the `DeleteDomain` that asked
/// for its deletion until the clock completes the teardown ([`Domain::begin_deletion`]).
pub const DOMAIN_STATUS_DELETING: &str = "Deleting";
/// See [`DOMAIN_STATUS_PENDING`]. What a domain reports from the `UpdateDomain` that asked
/// for a change until the clock completes it ([`Domain::begin_update`]): SageMaker applies
/// the change to the domain's own apps and network after it has answered, so the domain is
/// not in service while that is in flight.
pub const DOMAIN_STATUS_UPDATING: &str = "Updating";
/// See [`DOMAIN_STATUS_PENDING`]. The status a domain whose UPDATE could not be applied
/// reports: no operation here fails an update, so no record carries it — it is named because
/// such a domain still holds the apps and the file system a presigned domain URL signs its
/// holder in to, which is the rule that refusal states
/// ([`crate::state::DOMAIN_STATUSES_SIGNING_IN`]), and a handler must not compare a bare
/// string literal.
pub const DOMAIN_STATUS_UPDATE_FAILED: &str = "Update_Failed";

/// How long after `CreateDomain` the clock owes the `Pending` -> `InService` (or
/// [`DOMAIN_STATUS_FAILED`]) transition of a Studio domain.
///
/// The same compressed, never-zero span this domain's own MEMBERS take
/// ([`USER_PROFILE_IN_SERVICE_AFTER`]), because a domain's provisioning is work SageMaker
/// does after the create has answered: it builds the domain's EFS volume, its network
/// interfaces and its apps, which takes minutes on the real service. So the window is
/// observable here too — a caller that reads the domain straight after creating it sees
/// `Pending`, and every create that hangs a member off the domain in that window is refused
/// ([`domain_not_in_service_refusal`]), which is the state those refusals exist in at all —
/// while a caller that polls for the condition reaches `InService` without waiting a real
/// provisioning out. The due instant is STORED and never slept on
/// ([`Domain::settle_status`]), so a poller spends none of its interval on the settle: the
/// first request at or after the instant materialises it.
pub const DOMAIN_IN_SERVICE_AFTER: Duration = USER_PROFILE_IN_SERVICE_AFTER;

// The transitions an UPDATE and a DELETE of a domain owe stay owed from the instant the
// call that decided them answers (`transition_owed_at`): `UpdateDomain` admits the change
// and `DeleteDomain` admits the teardown, so nothing is left to compute once each of those
// has answered — see `transition_owed_at` for that rule and what it costs. The PROVISIONING
// a create starts is different work, and takes the span above.

/// The page `ListDomains` serves when the request names no `MaxResults`: its
/// `MaxResults` doc says "The default value for MaxResults is 10". [`MAX_PAGE_SIZE`]
/// still caps the page above it, as it does for every listing here.
pub const DOMAIN_DEFAULT_PAGE_SIZE: i32 = 10;

/// `AppNetworkAccessType`'s documented default: "Specifies the VPC used for non-EFS
/// traffic. The default value is PublicInternetOnly."
pub const DEFAULT_APP_NETWORK_ACCESS_TYPE: &str = "PublicInternetOnly";

/// The other member of `AppNetworkAccessType`'s catalogue beside the default above
/// (`smithy_query(query="shape",shape="AppNetworkAccessType")`: `PublicInternetOnly` |
/// `VpcOnly`) — the mode whose traffic is entirely the customer's VPC's. Named once here so
/// every reader that asks which mode a domain is in weighs the same value.
pub const APP_NETWORK_ACCESS_TYPE_VPC_ONLY: &str = "VpcOnly";

/// The two values the app network ACCESS of a domain takes: the access a domain in the
/// VPC SageMaker manages has — "Non-EFS traffic is through a VPC managed by Amazon
/// SageMaker AI, which allows direct internet access" — and the access a domain whose
/// traffic is entirely the customer's VPC's has, which reaches nothing beyond it: "All
/// traffic is through the specified VPC and subnets".
pub const APP_NETWORK_ACCESS_DIRECT_INTERNET_ONLY: &str = "DirectInternetOnly";
/// See [`APP_NETWORK_ACCESS_DIRECT_INTERNET_ONLY`].
pub const APP_NETWORK_ACCESS_VPC_ONLY: &str = "VpcOnly";

/// How a domain's non-EFS traffic is routed, in the vocabulary a read reports the domain's
/// app network ACCESS in: a domain confined to the VPC its create named reaches only that
/// VPC and its subnets, and a domain in the VPC SageMaker manages reaches the internet
/// directly.
///
/// PROJECTED from the domain's own access type rather than stored beside it, so a domain
/// whose type an update changes reports the access that type puts it in from the next read
/// on, and no second copy of the one fact can drift from it.
#[must_use]
pub fn app_network_access(app_network_access_type: &str) -> &'static str {
    if app_network_access_type == APP_NETWORK_ACCESS_TYPE_VPC_ONLY {
        APP_NETWORK_ACCESS_VPC_ONLY
    } else {
        APP_NETWORK_ACCESS_DIRECT_INTERNET_ONLY
    }
}

/// `HomeEfsFileSystemCreation`'s documented default: "Indicates whether to create a home
/// EFS file system for the domain. Defaults to Enabled."
pub const DEFAULT_HOME_EFS_FILE_SYSTEM_CREATION: &str = "Enabled";

/// `TagPropagation`'s documented default: "Indicates whether custom tag propagation is
/// supported for the domain. Defaults to DISABLED."
pub const DEFAULT_TAG_PROPAGATION: &str = "DISABLED";

/// A fresh `DomainId`: `d-` and the twelve lowercase characters the modelled `DomainId`
/// pattern admits after it (`^d-(-*[a-z0-9]){1,61}$`).
///
/// The caller never supplies it — a domain is addressed by the id SageMaker mints, so the
/// id is the server's to generate and is derived from nothing in the request. Same scheme
/// as every other id in this crate ([`mint_short_id`]), whose digits are lowercase hex
/// and so inside this pattern's alphabet.
pub fn mint_domain_id() -> String {
    format!("d-{}", mint_short_id())
}

/// The last path segment of a Studio domain's LANDING address: what a caller handed the
/// domain's `Url` opens, as `crate::service::studio_landing` serves it.
pub const STUDIO_LANDING_SEGMENT: &str = "studio";

/// The path a Studio domain's landing address is served at:
/// `/_aws/sagemaker/domain/<DomainId>/studio`, with the id percent-encoded.
///
/// One function for the mint and for the route — the same arrangement the presigned
/// sessions use ([`presigned_session_path`]) — so the address a `Url` names and the
/// address this emulator serves cannot drift apart. The namespace segment begins with
/// `_`, which no modelled path segment carries, so it collides with no control-plane
/// route.
pub fn studio_domain_path(domain_id: &str) -> String {
    format!(
        "/{namespace}/{PRESIGNED_SESSION_PLANE}/{DOMAIN_ARN_KIND}/{domain}/\
         {STUDIO_LANDING_SEGMENT}",
        namespace = ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE,
        domain = encoded_url_value(domain_id),
    )
}

/// Where the Studio domain `domain_id` is served: the endpoint the request arrived on
/// ([`::smithy_server_runtime::OperationContext::endpoint`]) plus that domain's landing
/// path, which `CreateDomain`, `DescribeDomain` and `ListDomains` all report as the
/// domain's `Url`.
///
/// The host is NEVER one of AWS's own names: a caller handed this address FOLLOWS it, and
/// a Studio DNS name of AWS's (`studio-d-<id>.studio.<region>.sagemaker.aws`) resolves to
/// nothing a client of the emulator can reach, so the Studio flow the member exists for could
/// never be driven here. The DOMAIN's own half of the address is therefore its PATH, which is
/// what the one endpoint this emulator serves can carry — derived from the domain's own id, so
/// it is stable across reads and distinct per domain, and minted ONCE by the create so every
/// later reader replays the same address.
pub fn studio_domain_url(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    domain_id: &str,
) -> String {
    format!(
        "{endpoint}{path}",
        endpoint = ctx.endpoint().as_str().trim_end_matches('/'),
        path = studio_domain_path(domain_id),
    )
}

/// Whether a fetch of the request target `uri` has ALREADY reached the published address
/// `url`, i.e. names that address's own path.
///
/// Both sides are read through the crate's one reading of a request target's PATH
/// ([`::service_common::request_path`]), so an address published as a whole URL and a target
/// that arrived as an origin-form path are compared the same way. What this decides is whether
/// a fetch is AT the address a resource publishes, so a request that is there is served rather
/// than handed off to the address it has already reached.
#[must_use]
pub fn addresses_published_path(url: &str, uri: &str) -> bool {
    ::service_common::request_path(url) == ::service_common::request_path(uri)
}

/// `StudioWebPortal`'s resolved value for a domain whose create named none: the member
/// decides "whether the user can access Studio", and a domain SageMaker AI creates admits
/// Studio — recorded behaviour, which is also the only value `studio::` may be the landing
/// experience with.
pub const DEFAULT_STUDIO_WEB_PORTAL: &str = "ENABLED";

/// `DefaultLandingUri`'s documented value for Studio: "studio::: Indicates that Studio is
/// the default experience. This value can only be passed if StudioWebPortal is set to
/// ENABLED."
pub const STUDIO_LANDING_URI: &str = "studio::";

/// `DefaultLandingUri`'s other documented value: "app:JupyterServer:: Indicates that
/// Studio Classic is the default experience." — where the domain does not admit Studio at
/// all, which is the landing experience left.
pub const STUDIO_CLASSIC_LANDING_URI: &str = "app:JupyterServer:";

/// `AutoMountHomeEFS`'s resolved value for a domain whose create named none: a domain's
/// home EFS volume is auto-mounted — recorded behaviour. (`DefaultAsDomain`, the third
/// member of the enum, is "only supported for user profiles"; `Disabled` is what a create
/// that withholds auto-mounting names itself.)
pub const DEFAULT_AUTO_MOUNT_HOME_EFS: &str = "Enabled";

/// `ExecutionRoleSessionNameMode`'s resolved value for a domain whose create named none:
/// the session name of the execution role corresponds to the user's identity — recorded
/// behaviour for a domain created now.
pub const DEFAULT_EXECUTION_ROLE_SESSION_NAME_MODE: &str = "USER_IDENTITY";

/// The domain's `DefaultUserSettings` as the service RESOLVES them, i.e. what every reader
/// of the domain reports: the settings the create carried, with each member the service
/// resolves a value for filled in.
///
/// A reader of a domain reports the configuration the domain CARRIES, not the subset of it
/// the request happened to mention, so the members SageMaker resolves for a domain are
/// resolved ONCE here, on the write path:
///
/// * `StudioWebPortal` — whether the domain admits Studio ([`DEFAULT_STUDIO_WEB_PORTAL`]);
/// * `DefaultLandingUri` — the experience a user is directed to, which is Studio's own
///   ([`STUDIO_LANDING_URI`]) for a domain that admits Studio and Studio Classic's
///   ([`STUDIO_CLASSIC_LANDING_URI`]) for one that does not, since `studio::` "can only be
///   passed if StudioWebPortal is set to ENABLED";
/// * `AutoMountHomeEFS` — whether the home EFS volume is auto-mounted
///   ([`DEFAULT_AUTO_MOUNT_HOME_EFS`]);
/// * `StudioWebPortalSettings.ExecutionRoleSessionNameMode` — how an execution-role
///   session is named ([`DEFAULT_EXECUTION_ROLE_SESSION_NAME_MODE`]).
///
/// Every value the request DID carry is kept exactly as it was sent: this fills gaps and
/// overrides nothing.
pub fn resolved_default_user_settings(mut settings: UserSettings) -> UserSettings {
    let studio_web_portal = settings
        .studio_web_portal
        .get_or_insert_with(|| DEFAULT_STUDIO_WEB_PORTAL.to_string())
        .clone();
    settings.default_landing_uri.get_or_insert_with(|| {
        if studio_web_portal == DEFAULT_STUDIO_WEB_PORTAL {
            STUDIO_LANDING_URI.to_string()
        } else {
            STUDIO_CLASSIC_LANDING_URI.to_string()
        }
    });
    settings
        .auto_mount_home_efs
        .get_or_insert_with(|| DEFAULT_AUTO_MOUNT_HOME_EFS.to_string());
    settings
        .studio_web_portal_settings
        .get_or_insert_with(StudioWebPortalSettings::default)
        .execution_role_session_name_mode
        .get_or_insert_with(|| DEFAULT_EXECUTION_ROLE_SESSION_NAME_MODE.to_string());
    settings
}

/// How `CreateDomain` states that the request named no network for the domain to
/// communicate through: the whole sentence is fixed text for this refusal — it interpolates
/// nothing from the request.
///
/// `VpcId`'s and `SubnetIds`' own docs make each optional only "for domains created from
/// Amazon SageMaker Unified Studio", so a SageMaker AI domain — the only kind created
/// here — has to name both, and a request that leaves either out is refused before any
/// domain exists.
pub const DOMAIN_NETWORK_REQUIRED: &str = "VpcId and SubnetIds are required for SageMaker \
     AI Domain not created by SageMaker Unified Studio.";

/// How the domain operations state a `DomainId` this account holds no domain under: the
/// entity, then the ARN the requested id resolves to in the Region the request signed for
/// IN SQUARE BRACKETS, then `does not exist.` — the sentence the Studio family of
/// resources words this fact with, closing period included, as an app's own not-found
/// refusal does (`App [<arn>] does not exist.`).
///
/// Stated once here so the reader and whichever update or delete resolves the same id
/// later cannot word the one fact differently.
pub fn domain_not_found(region: &str, domain_id: &str) -> String {
    format!(
        "Domain [{}] does not exist.",
        resource_arn(region, DOMAIN_ARN_KIND, domain_id)
    )
}

/// How an operation whose subject is a resource INSIDE the domain states a `DomainId` this
/// account holds no domain under: the entity, then the id THE REQUEST SUPPLIED in square
/// brackets, then `does not exist` — recorded AWS behaviour, closing period included, i.e.
/// without one.
///
/// Distinct from [`domain_not_found`] on purpose: AWS words the one fact for the reader it
/// answers. An operation addressing the DOMAIN itself (`DescribeDomain`, `DeleteDomain`)
/// names it by the ARN its id resolves to, while one whose subject is a member of the
/// domain (`CreateUserProfile`) names the id the caller passed, which is all that caller
/// said about the domain.
pub fn domain_id_not_found(domain_id: &str) -> String {
    format!("Domain [{domain_id}] does not exist")
}

/// How the domain operations state that a domain cannot take the change asked of it while
/// the service is already moving it: the entity, then the ARN the request resolves to IN
/// SQUARE BRACKETS — the subject form [`domain_not_found`] names the same resource with —
/// then the status that stands in the way.
///
/// What a delete of a domain the service is already tearing down is answered with, so a
/// repeat of that call reports the work in flight rather than claiming a second deletion.
pub fn domain_not_deletable(arn: &str, status: &str) -> String {
    studio_resource_not_deletable(DOMAIN_ENTITY, arn, status)
}

/// The entity label the Studio refusals name a domain by, i.e. the subject of
/// [`studio_resource_not_deletable`] when the resource being deleted is the domain itself.
pub const DOMAIN_ENTITY: &str = "Domain";

/// How a Studio operation states that the resource it addresses cannot take the removal
/// asked of it while the service is already removing it: the entity, then the ARN the
/// request resolves to IN SQUARE BRACKETS, then the status that stands in the way.
///
/// One sentence for the whole family — the domain and every member it holds — so a repeat
/// of any of their deletes reports the work in flight the same way, with only the entity
/// and the resource changing.
pub fn studio_resource_not_deletable(entity: &str, arn: &str, status: &str) -> String {
    format!("{entity} [{arn}] cannot be deleted while it is in {status} status.")
}

/// How a Studio create states that the domain already holds a member under the name the
/// request carried: the entity, then the name IN SINGLE QUOTES, then the `DomainId` it is
/// taken in — recorded AWS behaviour for the user profile's own collision, whose whole
/// sentence is fixed text but for those values.
///
/// The domain is named by the bare `d-…` id the request supplied rather than by an ARN, the
/// way every refusal a member-create words names the domain it was handed
/// ([`domain_id_not_found`]). One sentence for every member of the domain, so a second
/// create of a taken name is refused the same way whichever member it names.
pub fn studio_member_already_exists(entity: &str, name: &str, domain_id: &str) -> String {
    format!("A {entity} with name '{name}' already exists in Domain {domain_id}.")
}

/// How `DeleteDomain` states that the domain still holds Studio apps, which have to be
/// deleted first: the same bracketed-ARN subject as [`domain_not_found`], then the apps
/// that have not been deleted and so stand in the way.
///
/// Named for the sentence rather than for the question, so it cannot be mistaken for
/// [`ComAmazonawsSagemakerSagemakerState::domain_holds_apps`], which is what ASKS it.
pub fn domain_holds_apps_refusal(arn: &str) -> String {
    format!("Domain [{arn}] has non-deleted Apps.")
}

/// How `DeleteDomain` states that the domain still holds Studio user profiles, which have
/// to be deleted first ("All of the members of the domain must be deleted first"): the
/// same bracketed-ARN subject as [`domain_not_found`], then the members that stand in the
/// way.
///
/// Named for the sentence rather than for the question, like
/// [`domain_holds_apps_refusal`] beside it.
pub fn domain_holds_user_profiles_refusal(arn: &str) -> String {
    format!("Unable to delete Domain [{arn}] because UserProfile(s) are associated with it.")
}

/// How the Studio operations state that a resource cannot be created in a domain the
/// service has not finished provisioning: the would-be resource's ARN IN SQUARE BRACKETS,
/// then the domain's, then the status the create needs it in.
///
/// A domain only admits members once it reports [`DOMAIN_STATUS_IN_SERVICE`], so this is
/// what a create addressing a `Pending` (or failed, or tearing-down) domain is refused
/// with. Stated once here so every create that hangs a resource off a domain words the one
/// fact the same way.
pub fn domain_not_in_service_refusal(entity: &str, resource_arn: &str, domain_arn: &str) -> String {
    format!(
        "Unable to create {entity} [{resource_arn}] because Domain [{domain_arn}] is not in \
         {DOMAIN_STATUS_IN_SERVICE}."
    )
}

/// How `CreateDomain` states that this account already holds a domain under the `DomainName`
/// the request carried, in the Region it was signed for: the name IN SINGLE QUOTES, and the
/// fixed sentence that states where it is taken.
///
/// Recorded AWS behaviour, whose only variable part is the name the REQUEST sent — the
/// account and the Region are the ones the caller already addressed, so the sentence names
/// neither. One domain per name per Region is the invariant this states: the name is free
/// again once `DeleteDomain` has taken the domain that held it away.
#[must_use]
pub fn domain_name_already_exists(domain_name: &str) -> String {
    format!("The Domain Name '{domain_name}' already exists in this account in this region.")
}

/// The Region a domain's ARN names, or `None` for a value that is not an ARN this emulator
/// mints at all.
///
/// A stored domain carries the ARN its create minted from the Region that request was signed
/// for ([`resource_arn`]), so the ARN is where the domain's Region is read back from rather
/// than from a second copy of it beside the record. Read by `service_common::arn`, the one
/// ARN parse this workspace has.
fn domain_arn_region(arn: &str) -> Option<&str> {
    Some(::service_common::arn::parse_of(arn, ARN_SERVICE)?.region)
}

/// How the domain operations state that a domain cannot take the change asked of it while
/// the service is already removing it: the entity, then the ARN the request resolves to IN
/// SQUARE BRACKETS — the subject form [`domain_not_found`] names the same resource with —
/// then the status that stands in the way.
///
/// What an UPDATE of a domain the service is already tearing down is answered with: the
/// teardown is in flight, so there is nothing left to configure. Worded like
/// [`domain_not_deletable`] beside it, with the change it could not make in place of the
/// removal.
pub fn domain_not_updatable(arn: &str, status: &str) -> String {
    format!("Domain [{arn}] cannot be updated while it is in {status} status.")
}

/// The `HomeEfsFileSystemCreation` member a domain that HAS a home EFS file system carries,
/// i.e. the value an update cannot move away from.
pub const HOME_EFS_FILE_SYSTEM_CREATION_ENABLED: &str = DEFAULT_HOME_EFS_FILE_SYSTEM_CREATION;

/// How `UpdateDomain` states the one change its `HomeEfsFileSystemCreation` member does not
/// admit: that member's own doc states the rule — "you can change from Disabled to Enabled
/// to provision EFS on demand, but you cannot change from Enabled to Disabled" — so a
/// request asking for the other direction is refused and the domain keeps its file system.
///
/// FIXED text naming the member and the two values: the rule is about the pair, not about
/// anything the request chose.
pub fn home_efs_file_system_cannot_be_disabled() -> String {
    format!(
        "HomeEfsFileSystemCreation cannot be changed from \
         {HOME_EFS_FILE_SYSTEM_CREATION_ENABLED} to Disabled."
    )
}

/// How `UpdateDomain` states that the domain's apps stand in the way of the change asked of
/// its network access type: the bracketed-ARN subject [`domain_not_found`] names the domain
/// with, then the condition that has to hold.
///
/// `AppNetworkAccessType`'s own `UpdateDomain` doc states the rule: "this configuration can
/// only be modified if there are no apps in the InService, Pending, or Deleting state" — so
/// a domain still holding apps keeps the access type it has until they are deleted.
pub fn app_network_access_type_locked_by_apps(arn: &str) -> String {
    format!(
        "Unable to update AppNetworkAccessType for Domain [{arn}] because it has non-deleted \
         Apps."
    )
}

/// A Studio domain created by `CreateDomain`, as `DescribeDomain` replays it and
/// `ListDomains` summarises it.
///
/// `in_service_due_at` is the instant the clock owes this domain its move out of
/// `Pending` — into `InService`, or into [`DOMAIN_STATUS_FAILED`] where the create
/// resolved a placement the account's network does not admit; it is stored, never slept
/// on, and materialised by whichever reporting call first runs at or after it
/// ([`Domain::settle_status`]). The create owes it [`DOMAIN_IN_SERVICE_AFTER`] ahead, so
/// the requests that immediately follow the create still report the `Pending` domain and
/// the member-creates they make are refused; a test that needs the domain settled sets the
/// stored instant itself.
///
/// The members the request did not carry stay `None`: the model documents a default for
/// `AppNetworkAccessType`, `HomeEfsFileSystemCreation` and `TagPropagation` only, so
/// those three are resolved ONCE here on the write path and every other optional member
/// is reported exactly as the create sent it rather than as a value nobody asked for.
/// `tags` is stored so a later `ListTags` reports what the create carried.
#[derive(Debug, Clone)]
pub struct Domain {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub url: String,
    pub auth_mode: String,
    pub default_user_settings: UserSettings,
    pub domain_settings: Option<DomainSettings>,
    pub default_space_settings: Option<DefaultSpaceSettings>,
    pub subnet_ids: Option<Vec<String>>,
    pub vpc_id: Option<String>,
    pub kms_key_id: Option<String>,
    pub home_efs_file_system_kms_key_id: Option<String>,
    pub app_network_access_type: String,
    pub app_security_group_management: Option<String>,
    pub home_efs_file_system_creation: String,
    pub tag_propagation: String,
    pub status: String,
    /// Why this domain is in a failure status, i.e. the `FailureReason` a reader reports
    /// beside the status. `None` until the clock settles a domain whose provisioning could
    /// not complete, because only a domain in a failure status has a reason to report.
    pub failure_reason: Option<String>,
    /// Why this domain's provisioning will NOT complete, as the create resolved it against
    /// the account's own network ([`crate::state::domain_network_failure_reason`]), or `None`
    /// for a placement that holds together. Kept apart from [`Domain::failure_reason`]
    /// because it is what the clock still owes the domain rather than what the domain
    /// already reports: the create is served and the domain is `Pending` either way, and
    /// this decides WHICH status [`Domain::settle_status`] materialises.
    pub creation_failure_reason: Option<String>,
    pub tags: Vec<Tag>,
    pub creation_time: SystemTime,
    pub last_modified_time: SystemTime,
    pub in_service_due_at: SystemTime,
    /// The instant the clock owes this domain's removal, set by
    /// [`Domain::begin_deletion`]; `None` while no `DeleteDomain` has asked for one. Past
    /// it the domain is gone for every caller ([`Domain::is_released`]), whether or not a
    /// sweep has dropped the record yet — the way [`FeatureGroup::removal_due_at`] carries
    /// the same fact.
    pub removal_due_at: Option<SystemTime>,
}

impl Domain {
    /// Materialise the transition out of a non-terminal status the clock already owes, the
    /// way [`Endpoint::settle_status`] does for an endpoint: it was due at
    /// `in_service_due_at` whether or not anybody read the domain, so the observer makes no
    /// difference to the result, and `LastModifiedTime` moves to the instant the transition
    /// was due rather than to "now".
    ///
    /// Both the create's `Pending` and the update's [`DOMAIN_STATUS_UPDATING`] are moved,
    /// because both are statuses SageMaker leaves the domain in while it is still working on
    /// it. WHICH status it lands in is what the create already resolved: `InService` for a
    /// domain whose provisioning completes, and [`DOMAIN_STATUS_FAILED`] with the
    /// `FailureReason` beside it for one whose placement the account's network does not admit
    /// ([`Domain::creation_failure_reason`]).
    ///
    /// Answers whether the transition happened on this call, so the caller can record it.
    pub fn settle_status(&mut self, now: SystemTime) -> bool {
        let settling =
            self.status == DOMAIN_STATUS_PENDING || self.status == DOMAIN_STATUS_UPDATING;
        if settling && now >= self.in_service_due_at {
            self.last_modified_time = self.in_service_due_at;
            // What the create resolved about the domain's placement decides WHICH status the
            // clock owed: a domain whose network the account does not admit fails with the
            // reason that create recorded, and never comes up.
            if let Some(reason) = self.creation_failure_reason.clone() {
                self.status = DOMAIN_STATUS_FAILED.to_string();
                self.failure_reason = Some(reason);
                return true;
            }
            self.status = DOMAIN_STATUS_IN_SERVICE.to_string();
            return true;
        }
        false
    }

    /// Start the change `UpdateDomain` asked for: the domain reports
    /// [`DOMAIN_STATUS_UPDATING`], and the clock owes it its way back into service from the
    /// instant that call answers ([`transition_owed_at`]).
    ///
    /// SageMaker applies a domain's change to the apps and the network of that domain AFTER
    /// it has answered the call, so the domain is not in service while that is in flight:
    /// [`DOMAIN_STATUS_UPDATING`] is what this call stores and the move out of it is the
    /// clock's — stored as an instant and never slept on ([`Domain::settle_status`]). The
    /// update ITSELF is what decides the domain's new configuration, so nothing is left to
    /// compute afterwards and the first request after it reports the settled domain. The
    /// change is a change to the domain, so it moves `last_modified_time` too.
    pub fn begin_update(&mut self, now: SystemTime) {
        self.status = DOMAIN_STATUS_UPDATING.to_string();
        self.last_modified_time = now;
        self.in_service_due_at = transition_owed_at(now);
    }

    /// Start the teardown `DeleteDomain` asked for: the domain reports `Deleting`, and the
    /// clock owes its removal from the instant that call answers ([`transition_owed_at`]).
    ///
    /// The record is NOT dropped here: `DomainStatus` carries `Deleting`, which is what
    /// this call stores and answers with, and the removal is the clock's — stored as an
    /// instant and never slept on, so a sweep frees the quota slot without any caller's
    /// read deciding anything ([`ComAmazonawsSagemakerSagemakerState::release_deleted_domains`]).
    /// The delete ITSELF admits the teardown, so nothing is left to compute afterwards and
    /// the first request after it finds the domain gone. The status change is a change to
    /// the domain, so it moves `last_modified_time` too.
    pub fn begin_deletion(&mut self, now: SystemTime) {
        self.status = DOMAIN_STATUS_DELETING.to_string();
        self.last_modified_time = now;
        self.removal_due_at = Some(transition_owed_at(now));
    }

    /// Whether a `DeleteDomain` has asked for this domain's deletion, i.e. it is
    /// `Deleting` and on its way out.
    pub fn is_deleting(&self) -> bool {
        self.removal_due_at.is_some()
    }

    /// Whether the removal the clock owed has completed, i.e. the domain is gone.
    ///
    /// A lookup answers `None` for such a domain and a listing leaves it out, so no caller
    /// sees a domain whose removal was already due — the record itself is dropped by
    /// [`ComAmazonawsSagemakerSagemakerState::release_deleted_domains`], which frees the
    /// quota slot it still held.
    pub fn is_released(&self, now: SystemTime) -> bool {
        self.removal_due_at.is_some_and(|due| now >= due)
    }
}

impl ComAmazonawsSagemakerSagemakerState {
    /// Whether this account already holds a domain named `name` in `region`, i.e. the name
    /// is taken and a create naming it has nothing to allocate.
    ///
    /// A domain's name is unique per account per REGION ([`domain_name_already_exists`]), so
    /// the comparison is against the Region each stored domain was created in, which its own
    /// ARN carries — the value the create minted it from. A domain whose removal the clock
    /// already completed holds nothing ([`Domain::is_released`]), the one rule every reader
    /// of the store follows; a domain the service is still tearing down DOES still hold its
    /// name, because it is still a domain of the account until that teardown finishes.
    ///
    /// `now` is the caller's instant, so one request weighs the store against ONE clock
    /// reading.
    pub fn domain_name_taken(&self, region: &str, name: &str, now: SystemTime) -> bool {
        self.domains.values().any(|domain| {
            domain.name == name
                && !domain.is_released(now)
                && domain_arn_region(&domain.arn).is_some_and(|held| held == region)
        })
    }

    /// Whether another domain fits, i.e. the store is below [`DOMAIN_QUOTA`].
    pub fn domains_below_quota(&self) -> bool {
        self.domains.len() < DOMAIN_QUOTA
    }

    /// The domain a `DomainId` names, or `None` when this account holds none under it.
    ///
    /// The one lookup every domain reader resolves that member with, so no handler
    /// re-implements it: a domain whose removal the clock already completed resolves to
    /// nothing — past that instant the domain is gone whether or not the record has been
    /// dropped yet ([`Domain::is_released`]).
    ///
    /// `now` is the caller's instant, so one request weighs the store against ONE clock
    /// reading: the same instant its sweep and its transitions used.
    pub fn domain_mut(&mut self, domain_id: &str, now: SystemTime) -> Option<&mut Domain> {
        self.domains
            .get_mut(domain_id)
            .filter(|domain| !domain.is_released(now))
    }

    /// Whether the `DomainId` `domain_id` still resolves to a domain of this account, i.e.
    /// a reader would answer for it.
    ///
    /// Asked rather than resolved where only the existence matters — what a resource that
    /// hangs off the domain ([`UserProfile`]) is weighed against, so the one rule about
    /// which ids resolve ([`Self::domain_mut`]) holds for those resources too.
    pub fn domain_resolves(&self, domain_id: &str, now: SystemTime) -> bool {
        self.domains
            .get(domain_id)
            .is_some_and(|domain| !domain.is_released(now))
    }

    /// Drop every domain whose removal the clock already completed, and with it everything
    /// that hung off that domain.
    ///
    /// Materialises only what was already due: the removal was owed at the stored instant
    /// whether or not anybody called, and a lookup already answers `None` for such a
    /// domain ([`Domain::is_released`]) and for its user profiles
    /// ([`Self::user_profile_mut`]), so this changes nothing a later caller sees — it
    /// frees the quota slots the gone domain and its profiles still held. The call that
    /// asked for each removal already recorded it, so nothing is answered back.
    ///
    /// The profiles go WITH the domain rather than standing in the way of its teardown:
    /// `DeleteDomain` already refuses while the domain holds a profile no
    /// `DeleteUserProfile` has released ([`Self::domain_holds_user_profiles`]), so what is
    /// left here is a profile whose own removal the caller asked for first — it goes when
    /// the domain does, and so do the spaces and the apps that hung off it, so no record
    /// addresses a domain no reader resolves.
    pub fn release_deleted_domains(&mut self, now: SystemTime) {
        let released: Vec<String> = self
            .domains
            .values()
            .filter(|domain| domain.is_released(now))
            .map(|domain| domain.id.clone())
            .collect();
        for id in &released {
            self.domains.remove(id);
        }
        self.release_domain_user_profiles(&released);
        self.release_domain_spaces(&released);
        self.release_domain_apps(&released);
    }

    /// Whether the domain `domain_id` still holds Studio apps, i.e. apps created in it
    /// that no `DeleteApp` has torn down.
    ///
    /// An app hangs off the domain it was created in ([`AppKey::domain_id`]), so this is
    /// what `DeleteDomain` weighs before tearing the domain down: releasing the domain
    /// while its apps stayed behind would leave records addressing a domain no reader
    /// resolves. The retained record of a deleted app ([`App::is_released`]) is not a
    /// running app, so it stands in nothing's way.
    pub fn domain_holds_apps(&self, domain_id: &str) -> bool {
        self.apps
            .iter()
            .any(|(key, app)| key.domain_id == domain_id && !app.is_released())
    }

    /// Whether the domain `domain_id` still holds Studio user profiles, i.e. profiles
    /// created in it that no `DeleteUserProfile` has released.
    ///
    /// A user profile is "a single user within a domain", so it is one of the members AWS
    /// requires to be deleted before the domain itself: this is what `DeleteDomain` weighs
    /// beside [`Self::domain_holds_apps`]. A profile already on its way out
    /// ([`UserProfile::is_deleting`]) is not counted — the caller has asked for its
    /// removal, which is the order the teardown has to follow.
    pub fn domain_holds_user_profiles(&self, domain_id: &str) -> bool {
        self.user_profiles
            .values()
            .any(|profile| profile.domain_id == domain_id && !profile.is_deleting())
    }
}
