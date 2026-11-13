// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-appfabric-fabricfrontendservice service.
//!
//! AppFabric's resources form one chain: an **app bundle** owns its
//! **ingestions**, and an ingestion owns its **ingestion destinations**. The
//! store mirrors that chain, so a child is reachable only through its parent and
//! deleting a parent takes its children with it.
//!
//! Every record carries the fields the modelled response shapes need, and the
//! `to_*` methods below are the ONLY way a handler builds a response: a response
//! is therefore always read back out of what a create call wrote.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AppAuthorization, AppAuthorizationSummary, AppBundle, AppBundleSummary, ConflictException,
    Credential, Destination, DestinationConfiguration, Ingestion, IngestionDestination,
    IngestionDestinationSummary, IngestionSummary, ProcessingConfiguration,
    ResourceNotFoundException, ServiceQuotaExceededException, Tag, TaskError, Tenant,
    UserAccessResultItem, UserAccessTaskItem, ValidationException, ValidationExceptionField,
};

/// The ARN namespace (and sigv4 signing name) this service's ARNs carry, from
/// the model's `aws.api#service` trait (`arnNamespace: appfabric`).
pub const ARN_SERVICE: &str = "appfabric";

/// The region an ARN is built in when the request carries no SigV4 credential
/// scope to read one from — the region every generated emulator defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The DNS suffix of the partition this emulator serves, which the model's
/// endpoint rule set leaves as the `aws` partition's
/// (`https://appfabric.{Region}.{PartitionResult#dnsSuffix}`).
pub const DNS_SUFFIX: &str = "amazonaws.com";

/// The two values the model's `IngestionState` enum carries
/// (`INGESTION_STATE_VALUES` in the generated types): an ingestion is created
/// `disabled`, `StartIngestion` enables it and `StopIngestion` disables it again.
/// Named here so the handlers that write the state and the ones that read it
/// cannot drift apart over a bare literal.
pub const INGESTION_STATE_ENABLED: &str = "enabled";

/// The other one — see [`INGESTION_STATE_ENABLED`].
pub const INGESTION_STATE_DISABLED: &str = "disabled";

/// The `IngestionDestinationStatus` an ingestion destination whose
/// configuration was ACCEPTED carries, from the model's enum of that name
/// (`INGESTION_DESTINATION_STATUS_VALUES` = `Active` | `Failed`), whose own
/// documentation states it: "`Active`: The ingestion destination is active and
/// is ready to be used." Named here, beside the other modelled states this
/// crate writes, so the handlers that write the status and the ones that read
/// it cannot drift apart over a bare literal. What writes it is
/// [`IngestionDestinationRecord::mark_accepted`].
pub const INGESTION_DESTINATION_STATUS_ACTIVE: &str = "Active";

/// How long after an enabled ingestion loses a destination association the
/// disable AppFabric documents for it falls due.
///
/// A CHOSEN span, not an AWS-published duration: the model documents only that
/// such an ingestion fails and is EVENTUALLY disabled, with no figure. It is
/// non-zero on purpose — a caller reading the ingestion straight after the
/// delete still sees it `enabled`, and still gets the state refusals that go
/// with that, the way it would against AppFabric.
pub const INGESTION_DISABLE_AFTER_DESTINATION_LOSS: Duration = Duration::from_millis(500);

/// The largest page any `List*` operation returns, which is also the upper bound
/// the model puts on `maxResults` (`@range(min: 1, max: 100)`), and the page size
/// used when a caller names none.
pub const MAX_PAGE_SIZE: usize = 100;

/// How long a `nextToken` stays usable after the call that issued it.
///
/// An AWS-PUBLISHED duration, not a chosen one: the model states it in the
/// documentation of the `nextToken` member of every paginated operation of this
/// service — "Each pagination token expires after 24 hours. Using an expired
/// pagination token will return an HTTP 400 InvalidToken error." A token older
/// than this is refused by [`decode_page_token`] instead of being read as a
/// position.
pub const PAGE_TOKEN_LIFETIME: Duration = Duration::from_secs(24 * 60 * 60);

/// The app bundles ONE account may hold in ONE Region.
///
/// AppFabric's published, non-adjustable service quota: an account holds at most
/// one app bundle per Region, so a second `CreateAppBundle` in a Region whose
/// bundle still exists is refused with the `ServiceQuotaExceededException` the
/// operation models, and succeeds again once `DeleteAppBundle` has removed it.
pub const MAX_APP_BUNDLES_PER_REGION: usize = 1;

/// The emulator's ceiling on stored app bundles ACROSS every Region.
///
/// A RESOURCE GUARD, not an AWS-published quota: the store lives in the
/// emulator's process and serves every Region a caller may sign for, so the
/// per-Region quota above alone does not bound it. Past the ceiling
/// `CreateAppBundle` refuses with the same `ServiceQuotaExceededException`.
pub const MAX_APP_BUNDLES: usize = 1_000;

/// The same guard for the ingestions one app bundle may hold.
pub const MAX_INGESTIONS_PER_APP_BUNDLE: usize = 1_000;

/// The same guard for the ingestion destinations one ingestion may hold.
pub const MAX_DESTINATIONS_PER_INGESTION: usize = 1_000;

/// The same guard for the app authorizations one app bundle may hold.
pub const MAX_APP_AUTHORIZATIONS_PER_APP_BUNDLE: usize = 1_000;

/// The state an app authorization is created in, from the model's
/// `AppAuthorizationStatus` enum (`APP_AUTHORIZATION_STATUS_VALUES`), whose own
/// documentation states it: "`PendingConnect`: The initial state of the app
/// authorization. The app authorization is created but not yet connected."
pub const APP_AUTHORIZATION_STATUS_PENDING_CONNECT: &str = "PendingConnect";

/// The state `ConnectAppAuthorization` moves an authorization to, from the same
/// enum: "`Connected`: The app authorization is connected to the application,
/// and is ready to be used."
pub const APP_AUTHORIZATION_STATUS_CONNECTED: &str = "Connected";

/// The state the connect handshake reaches when the application answered a
/// validation exception instead of the access token AppFabric asked it for, from
/// the same enum, whose own documentation states it: "`ConnectionValidationFailed`:
/// The app authorization received a validation exception when trying to connect
/// to the application. If the app authorization is in this state, you should
/// verify the configured credentials and try to connect the app authorization
/// again."
pub const APP_AUTHORIZATION_STATUS_CONNECTION_VALIDATION_FAILED: &str = "ConnectionValidationFailed";

/// How long after a connect call the outcome of an OAuth2 handshake falls due.
///
/// A CHOSEN span, not an AWS-published duration: the model states no figure for
/// how long the exchange with the application takes. It is non-zero on purpose —
/// a caller reading the authorization straight after the connect call still sees
/// it `PendingConnect`, the way it would against AppFabric, and polls until the
/// outcome lands.
pub const OAUTH2_HANDSHAKE_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// The `AuthType` whose credential is an API key, from the model's `AuthType`
/// enum (`shared_types::AUTH_TYPE_VALUES` = `apiKey` | `oauth2`). An
/// authorization of this type is authorized by the key the create call carried,
/// so it has no consent step to report a URL for.
pub const AUTH_TYPE_API_KEY: &str = "apiKey";

/// The other `AuthType` from the same enum: the one whose authorization is
/// granted through an OAuth consent step, which is what
/// [`AppAuthorizationRecord::auth_url`] reports a URL for.
pub const AUTH_TYPE_OAUTH2: &str = "oauth2";

/// The user persona every app authorization carries, from the model's doc on
/// `AppAuthorization$persona`: "This field should always be `admin`." One of the
/// model's `Persona` values (`PERSONA_VALUES`).
pub const APP_AUTHORIZATION_PERSONA_ADMIN: &str = "admin";

/// The applications this service supports, which is the value set its `app`
/// member is closed over.
///
/// The member targets `String255`, so the MODEL constrains only its length; the
/// set of names is stated in the member's own documentation
/// (`smithy_query(query="doc",shape="CreateAppAuthorizationRequest",member="app")`:
/// "The name of the application. Valid values are: SLACK ASANA JIRA M365
/// M365AUDITLOGS ZOOM ZENDESK OKTA GOOGLE DROPBOX SMARTSHEET CISCO") and is
/// enforced by the service rather than by the wire schema. The names are
/// compared exactly: each one is a fixed upper-case token, so a different
/// spelling of one is a different value and names no supported application.
pub const SUPPORTED_APPLICATIONS: &[&str] = &[
    "SLACK",
    "ASANA",
    "JIRA",
    "M365",
    "M365AUDITLOGS",
    "ZOOM",
    "ZENDESK",
    "OKTA",
    "GOOGLE",
    "DROPBOX",
    "SMARTSHEET",
    "CISCO",
];

/// Whether `app` names one of the applications this service supports (see
/// [`SUPPORTED_APPLICATIONS`]).
pub fn is_supported_app(app: &str) -> bool {
    SUPPORTED_APPLICATIONS.contains(&app)
}

/// A modelled `ValidationException` refusing an `app` outside
/// [`SUPPORTED_APPLICATIONS`].
///
/// The sentence is the service's own, with the application name the request
/// carried as its only variable part: "app `<app>` is not supported". The faulted
/// member is `app`, so `reason` is `fieldValidationFailed` and `fieldList` names
/// it — a value of the request is what cannot be served.
pub fn app_not_supported(app: &str) -> ValidationException {
    field_validation_failed("app", &format!("app {app} is not supported"))
}

/// The number of tags ONE resource may hold.
///
/// Not a resource guard but the model's own bound: a `TagList` is
/// `@length(min: 0, max: 50)`, and `ListTagsForResource` reports a resource's
/// whole tag set as one `TagList` — a resource holding more could not be
/// reported, so `TagResource` refuses a merge that would take it past this.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The same guard for remembered idempotency tokens. A token is caller-chosen, so
/// this map would otherwise grow for the life of the process with one entry per
/// distinct token; past the ceiling [`ComAmazonawsAppfabricFabricfrontendserviceState::remember_client_token`]
/// evicts the oldest entry, which is the one least likely to still be retried.
pub const MAX_CLIENT_TOKENS: usize = 10_000;

/// The non-terminal `ResultStatus` a user access task is started in, from the
/// model's `ResultStatus` enum (`RESULT_STATUS_VALUES`), whose own documentation
/// states it: "`IN_PROGRESS`: The user access task is in progress."
pub const RESULT_STATUS_IN_PROGRESS: &str = "IN_PROGRESS";

/// The terminal `ResultStatus` of a task whose search could not be carried out,
/// from the same enum: "`FAILED`: The user access task failed."
pub const RESULT_STATUS_FAILED: &str = "FAILED";

/// How long after `StartUserAccessTasks` a task's non-terminal status settles.
///
/// A CHOSEN span, not an AWS-published duration: AppFabric documents only that a
/// task runs until the user access data is found or the application's API calls
/// time out, and this emulator makes no outbound connection, so the span is the
/// emulator's own. It is non-zero on purpose — a caller polling straight after
/// the start call still observes the `IN_PROGRESS` status, the way it would
/// against AppFabric.
pub const USER_ACCESS_TASK_SETTLES_AFTER: Duration = Duration::from_millis(500);

/// The emulator's ceiling on the user access tasks ONE app bundle keeps.
///
/// A RESOURCE GUARD, not an AWS-published quota, and NOT answered with an error:
/// `StartUserAccessTasks` models no quota error, and a user access task is a
/// transient poll record rather than a resource a caller creates, so past the
/// ceiling the OLDEST tasks are dropped (see
/// [`AppBundleRecord::evict_oldest_user_access_tasks`]) — an evicted task simply
/// stops being pollable, which is how the same bound on spent idempotency tokens
/// behaves ([`MAX_CLIENT_TOKENS`]). It is ten times the number of tasks ONE
/// start call can mint (one per app authorization, bounded by
/// [`MAX_APP_AUTHORIZATIONS_PER_APP_BUNDLE`]), so repeated start calls over a
/// bundle at the authorization ceiling keep several calls' worth of tasks
/// pollable rather than only the newest call's.
pub const MAX_USER_ACCESS_TASKS_PER_APP_BUNDLE: usize = 10 * MAX_APP_AUTHORIZATIONS_PER_APP_BUNDLE;

/// An app bundle, as `CreateAppBundle` stored it.
#[derive(Debug, Clone)]
pub struct AppBundleRecord {
    /// The bundle's bare UUID — the key it is stored under, and one of the two
    /// forms `appBundleIdentifier` accepts.
    pub id: String,
    pub arn: String,
    /// The Region the create call was signed for: the Region this bundle lives
    /// in, whose quota it counts against (see [`MAX_APP_BUNDLES_PER_REGION`]),
    /// and the one its [`arn`](Self::arn) carries. It is also the Region every
    /// resource nested UNDER this bundle lives in, because a nested resource
    /// belongs to the parent it hangs off rather than to the endpoint a later
    /// create call was signed for.
    pub region: String,
    pub customer_managed_key_arn: Option<String>,
    pub tags: Vec<Tag>,
    pub created_at: SystemTime,
    /// Creation order, so a listing is stable and a page token can name a
    /// position that survives a deletion.
    pub sequence: u64,
    pub ingestions: HashMap<String, IngestionRecord>,
    /// This bundle's app authorizations, keyed by bare UUID:
    /// `CreateAppAuthorization` writes them and `ConnectAppAuthorization` moves
    /// their status. An ingestion depends on one of them (see
    /// [`AppBundleRecord::connected_authorization`]), so they are stored under
    /// the same bundle record the ingestions are.
    pub app_authorizations: HashMap<String, AppAuthorizationRecord>,
    /// The user access tasks `StartUserAccessTasks` started under this bundle,
    /// keyed by task id: `BatchGetUserAccessTasks` polls them back by that id,
    /// and both operations scope them to the bundle the way the two modelled
    /// `appBundleIdentifier` members do.
    pub user_access_tasks: HashMap<String, UserAccessTaskRecord>,
}

impl AppBundleRecord {
    /// The modelled `AppBundle` for this record.
    pub fn to_app_bundle(&self) -> AppBundle {
        AppBundle {
            arn: self.arn.clone(),
            customer_managed_key_arn: self.customer_managed_key_arn.clone(),
        }
    }

    /// The modelled `AppBundleSummary` for this record.
    pub fn to_summary(&self) -> AppBundleSummary {
        AppBundleSummary {
            arn: self.arn.clone(),
        }
    }

    /// The ingestion `identifier` names within this bundle, by ARN or bare UUID.
    pub fn resolve_ingestion(&self, identifier: &str) -> Option<&IngestionRecord> {
        match ingestion_key(self, identifier) {
            Some(key) => self.ingestions.get(&key),
            None => None,
        }
    }

    /// The ingestion `identifier` names within this bundle, for mutation.
    pub fn resolve_ingestion_mut(&mut self, identifier: &str) -> Option<&mut IngestionRecord> {
        match ingestion_key(self, identifier) {
            Some(key) => self.ingestions.get_mut(&key),
            None => None,
        }
    }

    /// This bundle's ingestions in creation order.
    pub fn ingestions_in_order(&self) -> Vec<&IngestionRecord> {
        let mut ingestions: Vec<&IngestionRecord> = self.ingestions.values().collect();
        ingestions.sort_by_key(|ingestion| ingestion.sequence);
        ingestions
    }

    /// The app authorization `identifier` names within this bundle, by ARN or
    /// bare UUID — the same two forms every other `Identifier` accepts.
    pub fn resolve_app_authorization(&self, identifier: &str) -> Option<&AppAuthorizationRecord> {
        match app_authorization_key(self, identifier) {
            Some(key) => self.app_authorizations.get(&key),
            None => None,
        }
    }

    /// The same app authorization, for mutation.
    pub fn resolve_app_authorization_mut(
        &mut self,
        identifier: &str,
    ) -> Option<&mut AppAuthorizationRecord> {
        match app_authorization_key(self, identifier) {
            Some(key) => self.app_authorizations.get_mut(&key),
            None => None,
        }
    }

    /// This bundle's app authorizations in creation order.
    pub fn app_authorizations_in_order(&self) -> Vec<&AppAuthorizationRecord> {
        let mut authorizations: Vec<&AppAuthorizationRecord> =
            self.app_authorizations.values().collect();
        authorizations.sort_by_key(|authorization| authorization.sequence);
        authorizations
    }

    /// This bundle's app authorization for one (application, tenant) pair, if it
    /// holds one.
    ///
    /// The pair is what identifies an authorization inside a bundle: AppFabric
    /// holds at most one authorization per (bundle, app, tenant) triple, so this
    /// is both the uniqueness check `CreateAppAuthorization` takes before it
    /// inserts and the dependency lookup an ingestion is validated against.
    pub fn authorization_for(&self, app: &str, tenant: &str) -> Option<&AppAuthorizationRecord> {
        self.app_authorizations
            .values()
            .find(|authorization| authorization.app == app && authorization.tenant_identifier == tenant)
    }

    /// The same authorization, but only once it has completed the connect
    /// handshake as of `now` — the state an ingestion of that (app, tenant) pair
    /// requires.
    ///
    /// The handshake rule itself lives in
    /// [`AppAuthorizationRecord::is_connected`], which this composes with the
    /// lookup above, so every caller that needs "a usable authorization for
    /// this pair" asks one question.
    pub fn connected_authorization(
        &self,
        app: &str,
        tenant: &str,
        now: SystemTime,
    ) -> Option<&AppAuthorizationRecord> {
        self.authorization_for(app, tenant)
            .filter(|authorization| authorization.is_connected(now))
    }

    /// This bundle's ingestion for one (application, tenant) pair, if it holds
    /// one — the mirror of [`AppBundleRecord::authorization_for`].
    ///
    /// The pair is what ties an ingestion to an app authorization: an ingestion
    /// is created for an (`app`, `tenantId`) pair and only for a pair whose
    /// authorization has completed the connect handshake (see
    /// [`AppBundleRecord::connected_authorization`]), so the ingestion this
    /// answers is the one ASSOCIATED with that authorization — what a delete of
    /// the authorization has to find gone first.
    /// The OLDEST matching ingestion is the one answered, so a bundle that holds
    /// more than one for the same pair names the same record on every call and a
    /// refusal built from it reads the same twice.
    pub fn ingestion_for(&self, app: &str, tenant: &str) -> Option<&IngestionRecord> {
        self.ingestions_in_order()
            .into_iter()
            .find(|ingestion| ingestion.app == app && ingestion.tenant_id == tenant)
    }

    /// This bundle's user access tasks in start order.
    pub fn user_access_tasks_in_order(&self) -> Vec<&UserAccessTaskRecord> {
        let mut tasks: Vec<&UserAccessTaskRecord> = self.user_access_tasks.values().collect();
        tasks.sort_by_key(|task| task.sequence);
        tasks
    }

    /// Drop this bundle's OLDEST user access tasks until it is back within
    /// [`MAX_USER_ACCESS_TASKS_PER_APP_BUNDLE`] — see that ceiling for why a task
    /// is evicted rather than refused.
    pub fn evict_oldest_user_access_tasks(&mut self) {
        let surplus = self
            .user_access_tasks
            .len()
            .saturating_sub(MAX_USER_ACCESS_TASKS_PER_APP_BUNDLE);
        if surplus == 0 {
            return;
        }
        let evicted: Vec<String> = self
            .user_access_tasks_in_order()
            .into_iter()
            .take(surplus)
            .map(|task| task.id.clone())
            .collect();
        for id in evicted {
            self.user_access_tasks.remove(&id);
        }
    }
}

/// An app authorization, as `CreateAppAuthorization` stored it and
/// `ConnectAppAuthorization` last moved it.
///
/// The CREDENTIAL the create call carried is deliberately NOT part of the
/// record: no modelled response reports it back (`AppAuthorization` has no
/// credential member, and both credential shapes are `@sensitive`), so the only
/// thing any later read needs is WHICH kind of credential was supplied — which
/// is what [`AppAuthorizationRecord::credential_kind`] holds.
#[derive(Debug, Clone)]
pub struct AppAuthorizationRecord {
    pub id: String,
    pub arn: String,
    pub app_bundle_arn: String,
    /// The bare UUID of the app bundle that owns this authorization — the key
    /// the bundle is stored under, and the form of `appBundleIdentifier` the
    /// authorization's own URLs are built with (see
    /// [`AppAuthorizationRecord::auth_url`]).
    pub app_bundle_id: String,
    /// The Region this authorization lives in: the one the app bundle that owns
    /// it lives in (see [`AppBundleRecord::region`]), which is the one its
    /// [`arn`](Self::arn) carries and the one whose service endpoint its URLs
    /// name — not whichever Region's endpoint the create call was signed for.
    pub region: String,
    pub app: String,
    pub tenant_identifier: String,
    pub tenant_display_name: String,
    pub auth_type: String,
    /// The union member the caller set on `credential` (`oauth2Credential` or
    /// `apiKeyCredential`) — never the secret inside it.
    pub credential_kind: String,
    /// WHICH OAuth2 client id this authorization is registered with, as the
    /// one-way digest [`render_client_id`] reduces it to — `None` for an
    /// authorization whose credential carries no client id at all.
    ///
    /// Kept because the refusal a duplicate call is answered with is selected
    /// over it (see [`duplicate_authorization_refusal`]): which sentence names
    /// what the request repeats cannot be decided without remembering WHICH
    /// client id each record is registered with. The digest is enough to decide
    /// it, and no credential material can be read back out of what is stored —
    /// the same reason a [`request_fingerprint`] keeps a [`render_credential`]
    /// digest rather than the credential.
    pub client_id_digest: Option<String>,
    /// The `AppAuthorizationStatus` the last call that moved this authorization
    /// wrote. What a read REPORTS is [`AppAuthorizationRecord::status_at`],
    /// which also accounts for a handshake outcome the clock owes but no call
    /// has written.
    pub status: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: Vec<Tag>,
    pub sequence: u64,
    /// The instant from which this authorization reads
    /// `ConnectionValidationFailed` because the clock owes it the outcome of a
    /// handshake a connect call started and the application never validated
    /// ([`OAUTH2_HANDSHAKE_SETTLES_AFTER`] past that call). `None` when nothing
    /// is owed. Stored rather than slept for, so a read reports the outcome the
    /// clock already owes.
    pub validation_fails_at: Option<SystemTime>,
}

impl AppAuthorizationRecord {
    /// This authorization's `AppAuthorizationStatus` at `now`: the value the
    /// last call that moved it wrote, until the failed outcome of a handshake in
    /// flight falls due, and `ConnectionValidationFailed` from then on.
    pub fn status_at(&self, now: SystemTime) -> &str {
        match self.validation_fails_at {
            Some(due) if now >= due => APP_AUTHORIZATION_STATUS_CONNECTION_VALIDATION_FAILED,
            _ => &self.status,
        }
    }

    /// When this authorization last changed, as `now` sees it: a handshake
    /// outcome the clock has already made due is a change to the record, so
    /// from that instant on it is what `updatedAt` reports.
    pub fn changed_at(&self, now: SystemTime) -> SystemTime {
        match self.validation_fails_at {
            Some(due) if now >= due && due > self.updated_at => due,
            _ => self.updated_at,
        }
    }

    /// Whether this authorization has completed the connect handshake at `now` —
    /// the ONE place that rule is spelled out.
    ///
    /// AppFabric reaches an application only through an authorization in the
    /// state the model's `AppAuthorizationStatus` documents as ready to be used
    /// ([`APP_AUTHORIZATION_STATUS_CONNECTED`]), so both the ingestion
    /// dependency check and the user-access fan-out decide with this.
    pub fn is_connected(&self, now: SystemTime) -> bool {
        self.status_at(now) == APP_AUTHORIZATION_STATUS_CONNECTED
    }

    /// Record the handshake outcome of an authorization the application
    /// ACCEPTED, as of `now`: it is connected and ready to be used, and nothing
    /// is owed on it any more.
    ///
    /// Answers whether the record changed, which is what decides whether the
    /// call that ran the handshake dates it.
    pub fn connected(&mut self, now: SystemTime) -> bool {
        if self.is_connected(now) {
            return false;
        }
        self.status = APP_AUTHORIZATION_STATUS_CONNECTED.to_string();
        self.updated_at = now;
        self.validation_fails_at = None;
        true
    }

    /// Record the OTHER handshake outcome, as of `now`: the application answered
    /// a validation exception, so the authorization reads
    /// `ConnectionValidationFailed` once the handshake started at `now` falls
    /// due ([`OAUTH2_HANDSHAKE_SETTLES_AFTER`]) rather than the instant the call
    /// returns — the outcome is owed to the clock, not to this call, which is
    /// why nothing is dated here.
    ///
    /// A handshake whose failure is already owed, or already reported, has
    /// nothing left to apply: re-running it would only move the instant the same
    /// outcome lands on.
    pub fn validation_failing(&mut self, now: SystemTime) -> bool {
        if self.validation_fails_at.is_some()
            || self.status_at(now) == APP_AUTHORIZATION_STATUS_CONNECTION_VALIDATION_FAILED
        {
            return false;
        }
        self.validation_fails_at = Some(now + OAUTH2_HANDSHAKE_SETTLES_AFTER);
        true
    }

    /// Put this authorization back in the state a create leaves it in, as of
    /// `now`: it awaits the connect handshake again, and any outcome the clock
    /// still owed an earlier handshake is dropped with it.
    pub fn pending_connect(&mut self, now: SystemTime) {
        self.status = APP_AUTHORIZATION_STATUS_PENDING_CONNECT.to_string();
        self.updated_at = now;
        self.validation_fails_at = None;
    }

    /// Whether this authorization is the kind whose grant runs through an OAuth
    /// consent step — its stored `authType` is [`AUTH_TYPE_OAUTH2`] — the ONE
    /// place that rule is spelled out, as [`is_connected`](Self::is_connected)
    /// is for the status.
    ///
    /// Everything that hangs off the question (the URL the consent step runs at,
    /// and whether the connect handshake has to carry the `authRequest` that
    /// step hands back) decides with this, so a change to how the stored
    /// `authType` is spelled cannot leave one caller matching and another not.
    pub fn is_oauth2(&self) -> bool {
        self.auth_type == AUTH_TYPE_OAUTH2
    }

    /// Whether this authorization is REGISTERED with the OAuth2 client id
    /// `supplied_digest` names — a DIGEST as [`render_client_id`] reduces a
    /// credential to, never the client id itself.
    ///
    /// A credential that carries no client id (an API key) registers none, so it
    /// matches no record: `None` on either side is not a match, and only two
    /// equal digests are.
    pub fn registered_with_client_id(&self, supplied_digest: Option<&str>) -> bool {
        match (self.client_id_digest.as_deref(), supplied_digest) {
            (Some(stored), Some(supplied)) => stored == supplied,
            _ => false,
        }
    }

    /// The modelled `Tenant` this authorization was created for.
    pub fn to_tenant(&self) -> Tenant {
        Tenant {
            tenant_identifier: self.tenant_identifier.clone(),
            tenant_display_name: self.tenant_display_name.clone(),
        }
    }

    /// The URL this authorization reports as its `authUrl` ("The application URL
    /// for the OAuth flow"), or `None` when its `authType` has no OAuth flow to
    /// report one for.
    ///
    /// An `oauth2` authorization is granted through a consent step, and the code
    /// that step hands back is what `ConnectAppAuthorization` takes in its
    /// `authRequest` ("after permission is granted in the application OAuth
    /// page"), so the URL this reports is where that flow continues — see
    /// [`oauth_flow_url`]. An `apiKey` authorization carries its credential
    /// already and has no consent step, so it reports no URL.
    pub fn auth_url(&self) -> Option<String> {
        if self.is_oauth2() {
            Some(oauth_flow_url(&self.region, &self.app_bundle_id, &self.id))
        } else {
            None
        }
    }

    /// The modelled `AppAuthorization` for this record, as `now` reads it.
    pub fn to_app_authorization(&self, now: SystemTime) -> AppAuthorization {
        AppAuthorization {
            app_authorization_arn: self.arn.clone(),
            app_bundle_arn: self.app_bundle_arn.clone(),
            app: self.app.clone(),
            tenant: self.to_tenant(),
            auth_type: self.auth_type.clone(),
            status: self.status_at(now).to_string(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.changed_at(now)),
            persona: Some(APP_AUTHORIZATION_PERSONA_ADMIN.to_string()),
            auth_url: self.auth_url(),
        }
    }

    /// The modelled `AppAuthorizationSummary` for this record, as `now` reads it.
    pub fn to_summary(&self, now: SystemTime) -> AppAuthorizationSummary {
        AppAuthorizationSummary {
            app_authorization_arn: self.arn.clone(),
            app_bundle_arn: self.app_bundle_arn.clone(),
            app: self.app.clone(),
            tenant: self.to_tenant(),
            status: self.status_at(now).to_string(),
            updated_at: ::aws_smithy_types::DateTime::from(self.changed_at(now)),
        }
    }
}

/// One user access task, as `StartUserAccessTasks` started it.
///
/// A task searches ONE application tenant for ONE email address, which is why it
/// is minted per app authorization: the tenant it searches, and the display name
/// `BatchGetUserAccessTasks` reports for that tenant, are the authorization's.
#[derive(Debug, Clone)]
pub struct UserAccessTaskRecord {
    /// The task's `UUID` — the key it is stored under and the id a caller polls
    /// it by.
    pub id: String,
    pub app: String,
    pub tenant_identifier: String,
    pub tenant_display_name: String,
    /// The target user's email address, as the start call named it. Reported
    /// back by the modelled `UserAccessResultItem$email`.
    pub email: String,
    pub started_at: SystemTime,
    /// The instant the clock makes this task's `IN_PROGRESS` status settle into
    /// its terminal one ([`USER_ACCESS_TASK_SETTLES_AFTER`] past
    /// [`UserAccessTaskRecord::started_at`]). Stored rather than slept for, so a
    /// poll reports the status the clock already owes.
    pub settles_at: SystemTime,
    /// Start order, so a bundle's tasks are reported in a stable order and the
    /// oldest is the one evicted first.
    pub sequence: u64,
}

impl UserAccessTaskRecord {
    /// The modelled `UserAccessTaskItem` the start call reports for this task.
    pub fn to_task_item(&self) -> UserAccessTaskItem {
        UserAccessTaskItem {
            app: self.app.clone(),
            tenant_id: self.tenant_identifier.clone(),
            task_id: Some(self.id.clone()),
            // The task WAS started, so there is no start-time error to report;
            // whether its search succeeded is what a poll reports.
            error: None,
        }
    }

    /// This task's `ResultStatus` at `now`: `IN_PROGRESS` until the clock makes
    /// it due to settle, and terminal from then on.
    pub fn result_status(&self, now: SystemTime) -> &'static str {
        if now < self.settles_at {
            RESULT_STATUS_IN_PROGRESS
        } else {
            RESULT_STATUS_FAILED
        }
    }

    /// The modelled `UserAccessResultItem` a poll reports for this task at `now`.
    ///
    /// The user members (`userId`, `userFullName`, `userFirstName`,
    /// `userLastName`, `userStatus`) are reported by AppFabric out of what the
    /// application answered. This emulator serves no application and makes no
    /// outbound connection, so nothing searched for them and they stay ABSENT
    /// rather than carrying a value nothing computed; once the task has
    /// terminated, the modelled `taskError` says exactly that.
    pub fn to_result_item(&self, now: SystemTime) -> UserAccessResultItem {
        let status = self.result_status(now);
        UserAccessResultItem {
            app: Some(self.app.clone()),
            tenant_id: Some(self.tenant_identifier.clone()),
            tenant_display_name: Some(self.tenant_display_name.clone()),
            task_id: Some(self.id.clone()),
            result_status: Some(status.to_string()),
            email: Some(self.email.clone()),
            user_id: None,
            user_full_name: None,
            user_first_name: None,
            user_last_name: None,
            user_status: None,
            task_error: (status == RESULT_STATUS_FAILED)
                .then(|| user_access_search_not_performed(&self.app)),
        }
    }
}

/// The `TaskError` a user access task carries once it has terminated.
///
/// A user access search is an outbound call into the application itself, which
/// this emulator does not make: no search ran, so the task ends in the modelled
/// `FAILED` status with a reason that says so, rather than reporting a user
/// record nothing looked up.
pub fn user_access_search_not_performed(app: &str) -> TaskError {
    TaskError {
        error_code: Some("ApplicationNotReachable".to_string()),
        error_message: Some(format!(
            "The emulator makes no outbound connection to {app}, so the user access status \
             could not be searched for."
        )),
    }
}

/// The `TaskError` `StartUserAccessTasks` reports for an application tenant it
/// started no task for.
///
/// AppFabric searches an application tenant for a user only with an
/// authentication token for that user, which the tenant's app authorization
/// holds once it has completed the connect handshake (authorize → connect →
/// query). Until then there is no token to search with, so the fan-out reports
/// that tenant with this error and NO `taskId`, telling the caller the user
/// still has to authorize. Both the code and the sentence are FIXED: the service
/// words this error the same way whichever application, tenant or held status
/// the item is for, so nothing of the request or of the store is interpolated
/// into it.
pub fn user_access_no_authentication_token() -> TaskError {
    TaskError {
        error_code: Some(TASK_ERROR_CODE_NO_TOKEN.to_string()),
        error_message: Some(
            "No authentication token was found for this user. User needs to authorize."
                .to_string(),
        ),
    }
}

/// The `errorCode` AppFabric puts on the start-call `TaskError` above: the
/// service reports the missing authentication token as this fixed token, which
/// is what a client switches on.
const TASK_ERROR_CODE_NO_TOKEN: &str = "notfound";

/// The `UserAccessResultItem` a poll reports for a task id the named app bundle
/// holds no record for.
///
/// `BatchGetUserAccessTasks` is a BATCH poll: an id it cannot resolve is one
/// ENTRY's outcome, not a failure of the whole request, so the entry is answered
/// in place with the id that was asked for and the `taskError` saying the bundle
/// holds no record for it — which is what the all-optional member set of
/// `UserAccessResultItem` is for. Every other member stays absent: with no task
/// record, nothing knows the application, the tenant or the searched address.
pub fn user_access_task_record_not_found(task_id: &str) -> UserAccessResultItem {
    UserAccessResultItem {
        app: None,
        tenant_id: None,
        tenant_display_name: None,
        task_id: Some(task_id.to_string()),
        result_status: None,
        email: None,
        user_id: None,
        user_full_name: None,
        user_first_name: None,
        user_last_name: None,
        user_status: None,
        task_error: Some(TaskError {
            error_code: Some(TASK_ERROR_CODE_NO_RECORD.to_string()),
            error_message: Some(format!(
                "Couldn't find a record within current appBundle with the given taskId: {task_id}"
            )),
        }),
    }
}

/// The `errorCode` the per-entry `taskError` above carries: the service reports
/// the status of the missing-record condition as the code, the same for every
/// unresolvable id.
const TASK_ERROR_CODE_NO_RECORD: &str = "404";

/// An ingestion, as `CreateIngestion` stored it.
#[derive(Debug, Clone)]
pub struct IngestionRecord {
    pub id: String,
    pub arn: String,
    pub app_bundle_arn: String,
    pub app: String,
    pub tenant_id: String,
    pub ingestion_type: String,
    /// The `IngestionState` the last call that moved this ingestion wrote. What a
    /// read REPORTS is [`IngestionRecord::state_at`], which also accounts for a
    /// disable the clock owes but no call has written.
    pub state: String,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: Vec<Tag>,
    pub sequence: u64,
    /// This ingestion's destinations, keyed by bare UUID:
    /// `CreateIngestionDestination` writes them and
    /// `UpdateIngestionDestination` rewrites their configuration in place.
    pub destinations: HashMap<String, IngestionDestinationRecord>,
    /// The instant from which this ingestion reads `disabled` because the clock
    /// owes it the disable that follows losing a destination association while it
    /// was enabled ([`INGESTION_DISABLE_AFTER_DESTINATION_LOSS`] past the call
    /// that broke the association). `None` when nothing is owed. Stored rather
    /// than slept for, so a read reports the state the clock already owes.
    pub disables_at: Option<SystemTime>,
}

impl IngestionRecord {
    /// This ingestion's `IngestionState` at `now`: the value the last start or
    /// stop call wrote, until a disable the clock owes falls due, and `disabled`
    /// from then on.
    pub fn state_at(&self, now: SystemTime) -> &str {
        match self.disables_at {
            Some(due) if now >= due => INGESTION_STATE_DISABLED,
            _ => &self.state,
        }
    }

    /// When this ingestion last changed, as `now` sees it: a disable the clock has
    /// already made due is a change to the record, so from that instant on it is
    /// what `updatedAt` reports.
    pub fn changed_at(&self, now: SystemTime) -> SystemTime {
        match self.disables_at {
            Some(due) if now >= due && due > self.updated_at => due,
            _ => self.updated_at,
        }
    }

    /// Write `state` as this ingestion's state as of `now` — the move a start or
    /// stop call makes. It also drops any disable the clock still owed: the
    /// caller has just decided this ingestion's state itself.
    pub fn set_state(&mut self, state: &str, now: SystemTime) {
        self.state = state.to_string();
        self.updated_at = now;
        self.disables_at = None;
    }

    /// Record that losing a destination association at `now` leaves this ingestion
    /// with a disable owed, which is what an ENABLED ingestion gets: it has lost a
    /// destination to deliver to, so it fails and the service disables it shortly
    /// after. An ingestion that is not enabled at `now` has no such failure and
    /// nothing is owed.
    pub fn lost_destination(&mut self, now: SystemTime) {
        if self.state_at(now) == INGESTION_STATE_ENABLED {
            self.disables_at = Some(now + INGESTION_DISABLE_AFTER_DESTINATION_LOSS);
        }
    }

    /// The modelled `Ingestion` for this record, as `now` reads it.
    pub fn to_ingestion(&self, now: SystemTime) -> Ingestion {
        Ingestion {
            arn: self.arn.clone(),
            app_bundle_arn: self.app_bundle_arn.clone(),
            app: self.app.clone(),
            tenant_id: self.tenant_id.clone(),
            created_at: ::aws_smithy_types::DateTime::from(self.created_at),
            updated_at: ::aws_smithy_types::DateTime::from(self.changed_at(now)),
            state: self.state_at(now).to_string(),
            ingestion_type: self.ingestion_type.clone(),
        }
    }

    /// The modelled `IngestionSummary` for this record, as `now` reads it.
    pub fn to_summary(&self, now: SystemTime) -> IngestionSummary {
        IngestionSummary {
            arn: self.arn.clone(),
            app: self.app.clone(),
            tenant_id: self.tenant_id.clone(),
            state: self.state_at(now).to_string(),
        }
    }

    /// The ingestion destination `identifier` names within this ingestion, by ARN
    /// or bare UUID.
    pub fn resolve_destination(&self, identifier: &str) -> Option<&IngestionDestinationRecord> {
        match destination_key(self, identifier) {
            Some(key) => self.destinations.get(&key),
            None => None,
        }
    }

    /// The ingestion destination `identifier` names within this ingestion, for
    /// mutation.
    pub fn resolve_destination_mut(
        &mut self,
        identifier: &str,
    ) -> Option<&mut IngestionDestinationRecord> {
        match destination_key(self, identifier) {
            Some(key) => self.destinations.get_mut(&key),
            None => None,
        }
    }

    /// This ingestion's destinations in creation order.
    pub fn destinations_in_order(&self) -> Vec<&IngestionDestinationRecord> {
        let mut destinations: Vec<&IngestionDestinationRecord> =
            self.destinations.values().collect();
        destinations.sort_by_key(|destination| destination.sequence);
        destinations
    }
}

/// An ingestion destination, as `CreateIngestionDestination` stored it and
/// `UpdateIngestionDestination` last wrote it.
#[derive(Debug, Clone)]
pub struct IngestionDestinationRecord {
    pub id: String,
    pub arn: String,
    pub ingestion_arn: String,
    pub processing_configuration: ProcessingConfiguration,
    pub destination_configuration: DestinationConfiguration,
    pub status: String,
    pub status_reason: Option<String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    pub tags: Vec<Tag>,
    pub sequence: u64,
}

impl IngestionDestinationRecord {
    /// Leave this destination in the state an ACCEPTED configuration puts it
    /// in: the terminal [`INGESTION_DESTINATION_STATUS_ACTIVE`] status and no
    /// status reason, which the model reserves for the other value of the same
    /// enum ("Only present when the `status` of ingestion destination is
    /// `Failed`"). The ONE place this crate writes that pair, so a destination a
    /// create call accepted and one an update call accepted cannot drift apart
    /// over it.
    pub fn mark_accepted(&mut self) {
        self.status = INGESTION_DESTINATION_STATUS_ACTIVE.to_string();
        self.status_reason = None;
    }

    /// The modelled `IngestionDestination` for this record.
    pub fn to_ingestion_destination(&self) -> IngestionDestination {
        IngestionDestination {
            arn: self.arn.clone(),
            ingestion_arn: self.ingestion_arn.clone(),
            processing_configuration: self.processing_configuration.clone(),
            destination_configuration: self.destination_configuration.clone(),
            status: Some(self.status.clone()),
            status_reason: self.status_reason.clone(),
            created_at: Some(::aws_smithy_types::DateTime::from(self.created_at)),
            updated_at: Some(::aws_smithy_types::DateTime::from(self.updated_at)),
        }
    }

    /// The modelled `IngestionDestinationSummary` for this record, which the
    /// model gives one member: the destination's ARN.
    pub fn to_summary(&self) -> IngestionDestinationSummary {
        IngestionDestinationSummary {
            arn: self.arn.clone(),
        }
    }
}

/// What one spent idempotency token created, so replaying the token replays the
/// resource instead of creating a second one.
#[derive(Debug, Clone)]
pub struct ClientTokenRecord {
    /// The ARN of the resource the first call created.
    pub arn: String,
    /// A canonical rendering of that call's parameters — see
    /// [`request_fingerprint`].
    pub fingerprint: String,
    /// When the token was spent, so the oldest can be evicted first once
    /// [`MAX_CLIENT_TOKENS`] is reached.
    pub sequence: u64,
}

/// Per-service state for com-amazonaws-appfabric-fabricfrontendservice. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsAppfabricFabricfrontendserviceState {
    /// App bundles, keyed by bare UUID.
    pub app_bundles: HashMap<String, AppBundleRecord>,
    /// Spent idempotency tokens, keyed by [`client_token_key`].
    pub client_tokens: HashMap<String, ClientTokenRecord>,
    /// The source of every record's `sequence`: monotonic across the whole
    /// store, so creation order is recoverable without an insertion-ordered map.
    next_sequence: u64,
}

impl ComAmazonawsAppfabricFabricfrontendserviceState {
    /// The next creation sequence. Never reused, so a page token that names a
    /// sequence keeps naming the same position for the life of the store.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// The app bundle `identifier` names, by ARN or bare UUID.
    pub fn resolve_bundle(&self, identifier: &str) -> Option<&AppBundleRecord> {
        self.bundle_key(identifier)
            .and_then(|key| self.app_bundles.get(&key))
    }

    /// The app bundle `identifier` names, for mutation.
    pub fn resolve_bundle_mut(&mut self, identifier: &str) -> Option<&mut AppBundleRecord> {
        match self.bundle_key(identifier) {
            Some(key) => self.app_bundles.get_mut(&key),
            None => None,
        }
    }

    /// The store key of the app bundle `identifier` names, by either modelled
    /// `Identifier` form — the one rule [`identifier_key`] holds, over the app
    /// bundles this store keeps.
    pub fn bundle_key(&self, identifier: &str) -> Option<String> {
        identifier_key(&self.app_bundles, identifier)
    }

    /// Remember what one spent idempotency token created.
    ///
    /// Bounded: at [`MAX_CLIENT_TOKENS`] the oldest entry is evicted to make room,
    /// so the map cannot grow with the number of distinct tokens a caller sends.
    /// An evicted token simply stops replaying — it behaves like a token never
    /// seen, which is the safe direction for a bound to fail in.
    pub fn remember_client_token(&mut self, key: String, arn: String, fingerprint: String) {
        if self.client_tokens.len() >= MAX_CLIENT_TOKENS && !self.client_tokens.contains_key(&key) {
            let oldest = self
                .client_tokens
                .iter()
                .min_by_key(|(_, spent)| spent.sequence)
                .map(|(token, _)| token.clone());
            if let Some(oldest) = oldest {
                self.client_tokens.remove(&oldest);
            }
        }
        let sequence = self.next_sequence();
        self.client_tokens.insert(
            key,
            ClientTokenRecord {
                arn,
                fingerprint,
                sequence,
            },
        );
    }

    /// Decide what a spent `@idempotencyToken` says about the call being served,
    /// for EVERY create this service models.
    ///
    /// The decision is one rule, so it lives in one place: a token spent for this
    /// (Region, operation) pair by a call whose parameters render differently
    /// (see [`request_fingerprint`]) is the conflict AWS documents as
    /// `IdempotentParameterMismatch` (returned as the modelled `ConflictException`
    /// [`idempotent_parameter_mismatch`] builds, which each handler lifts into its
    /// own error enum); a token spent by the SAME request replays the resource it
    /// created; and a token whose resource no longer exists is spent on nothing, so
    /// it is forgotten here and the caller's call goes on to create afresh.
    ///
    /// All each handler keeps is its own ARN-to-record resolver, `resolve`: it is
    /// handed the store and the ARN the token holds and answers with the response
    /// projection of that record (which level of the resource chain the ARN is
    /// looked up in, and whether the projection takes a clock, is the part that
    /// genuinely differs between the creates).
    ///
    /// `Ok(Some(_))` is a replay to return as-is, `Ok(None)` means "create", and
    /// `Err(_)` is the mismatch conflict.
    pub fn replay_spent_client_token<T>(
        &mut self,
        region: &str,
        operation: &str,
        token: &str,
        fingerprint: &str,
        resource_type: &str,
        resolve: impl FnOnce(&Self, &str) -> Option<T>,
    ) -> Result<Option<T>, ConflictException> {
        let key = client_token_key(region, operation, token);
        let Some(spent) = self.client_tokens.get(&key) else {
            return Ok(None);
        };
        if spent.fingerprint != fingerprint {
            return Err(idempotent_parameter_mismatch(token, resource_type));
        }
        let arn = spent.arn.clone();
        if let Some(replayed) = resolve(self, &arn) {
            return Ok(Some(replayed));
        }
        // The resource the token created was deleted since, so the token is spent
        // on nothing: forget it and let this call create a new resource.
        self.client_tokens.remove(&key);
        Ok(None)
    }

    /// How many app bundles this account holds in `region`.
    ///
    /// An app bundle belongs to the Region its create call was signed for, and
    /// AppFabric's quota is per account per Region, so this — not the size of
    /// the whole store — is what `CreateAppBundle` weighs against
    /// [`MAX_APP_BUNDLES_PER_REGION`].
    pub fn bundles_in_region(&self, region: &str) -> usize {
        self.app_bundles
            .values()
            .filter(|bundle| bundle.region == region)
            .count()
    }

    /// App bundles in creation order.
    pub fn bundles_in_order(&self) -> Vec<&AppBundleRecord> {
        let mut bundles: Vec<&AppBundleRecord> = self.app_bundles.values().collect();
        bundles.sort_by_key(|bundle| bundle.sequence);
        bundles
    }

    /// Where the resource an `Arn` names lives in the store, or `None` when no
    /// record carries that ARN.
    ///
    /// The resource-tagging operations take an ARN rather than a typed
    /// identifier, so this is the one place that decides which record a
    /// `resourceArn` denotes: every record this service persists carries its own
    /// ARN, and the walk is over the ownership chain (bundle → ingestion →
    /// ingestion destination), so a resource becomes taggable exactly when the
    /// operation that mints it starts persisting it.
    pub fn locate_tagged(&self, arn: &str) -> Option<TagTarget> {
        for bundle in self.app_bundles.values() {
            if bundle.arn == arn {
                return Some(TagTarget::AppBundle {
                    bundle: bundle.id.clone(),
                });
            }
            for authorization in bundle.app_authorizations.values() {
                if authorization.arn == arn {
                    return Some(TagTarget::AppAuthorization {
                        bundle: bundle.id.clone(),
                        authorization: authorization.id.clone(),
                    });
                }
            }
            for ingestion in bundle.ingestions.values() {
                if ingestion.arn == arn {
                    return Some(TagTarget::Ingestion {
                        bundle: bundle.id.clone(),
                        ingestion: ingestion.id.clone(),
                    });
                }
                for destination in ingestion.destinations.values() {
                    if destination.arn == arn {
                        return Some(TagTarget::IngestionDestination {
                            bundle: bundle.id.clone(),
                            ingestion: ingestion.id.clone(),
                            destination: destination.id.clone(),
                        });
                    }
                }
            }
        }
        None
    }

    /// The tags a located resource holds, as the holding record stores them.
    pub fn tags_of(&self, target: &TagTarget) -> Option<&Vec<Tag>> {
        match target {
            TagTarget::AppBundle { bundle } => {
                self.app_bundles.get(bundle).map(|bundle| &bundle.tags)
            }
            TagTarget::Ingestion { bundle, ingestion } => self
                .app_bundles
                .get(bundle)?
                .ingestions
                .get(ingestion)
                .map(|ingestion| &ingestion.tags),
            TagTarget::IngestionDestination {
                bundle,
                ingestion,
                destination,
            } => self
                .app_bundles
                .get(bundle)?
                .ingestions
                .get(ingestion)?
                .destinations
                .get(destination)
                .map(|destination| &destination.tags),
            TagTarget::AppAuthorization {
                bundle,
                authorization,
            } => self
                .app_bundles
                .get(bundle)?
                .app_authorizations
                .get(authorization)
                .map(|authorization| &authorization.tags),
        }
    }

    /// The same tag set, for mutation by `TagResource` / `UntagResource`.
    pub fn tags_of_mut(&mut self, target: &TagTarget) -> Option<&mut Vec<Tag>> {
        match target {
            TagTarget::AppBundle { bundle } => self
                .app_bundles
                .get_mut(bundle)
                .map(|bundle| &mut bundle.tags),
            TagTarget::Ingestion { bundle, ingestion } => self
                .app_bundles
                .get_mut(bundle)?
                .ingestions
                .get_mut(ingestion)
                .map(|ingestion| &mut ingestion.tags),
            TagTarget::IngestionDestination {
                bundle,
                ingestion,
                destination,
            } => self
                .app_bundles
                .get_mut(bundle)?
                .ingestions
                .get_mut(ingestion)?
                .destinations
                .get_mut(destination)
                .map(|destination| &mut destination.tags),
            TagTarget::AppAuthorization {
                bundle,
                authorization,
            } => self
                .app_bundles
                .get_mut(bundle)?
                .app_authorizations
                .get_mut(authorization)
                .map(|authorization| &mut authorization.tags),
        }
    }
}

/// The resource a tagging call's `resourceArn` names: the chain of store keys
/// leading to the record that holds its tags.
///
/// Keys rather than a reference, so one immutable walk
/// ([`ComAmazonawsAppfabricFabricfrontendserviceState::locate_tagged`]) serves
/// both the reporting and the mutating tagging operations.
#[derive(Debug, Clone)]
pub enum TagTarget {
    AppBundle {
        bundle: String,
    },
    Ingestion {
        bundle: String,
        ingestion: String,
    },
    IngestionDestination {
        bundle: String,
        ingestion: String,
        destination: String,
    },
    AppAuthorization {
        bundle: String,
        authorization: String,
    },
}

impl TagTarget {
    /// The resource's kind, as a modelled error and an audit record name it.
    pub fn resource_type(&self) -> &'static str {
        match self {
            Self::AppBundle { .. } => "AppBundle",
            Self::Ingestion { .. } => "Ingestion",
            Self::IngestionDestination { .. } => "IngestionDestination",
            Self::AppAuthorization { .. } => "AppAuthorization",
        }
    }

    /// The resource's own bare id — the last key in the chain.
    pub fn id(&self) -> &str {
        match self {
            Self::AppBundle { bundle } => bundle,
            Self::Ingestion { ingestion, .. } => ingestion,
            Self::IngestionDestination { destination, .. } => destination,
            Self::AppAuthorization { authorization, .. } => authorization,
        }
    }
}

/// Merge `incoming` into `tags` the way `TagResource` does: a tag list is a map
/// of key-value pairs, so a key already present takes the new value in place
/// (keeping the position it was first assigned at) and a new key is appended.
///
/// Returns the merged set rather than mutating, so a caller can refuse a merge
/// that would take the resource past [`MAX_TAGS_PER_RESOURCE`] without having
/// changed anything.
pub fn merge_tags(tags: &[Tag], incoming: &[Tag]) -> Vec<Tag> {
    let mut merged = tags.to_vec();
    for tag in incoming {
        match merged.iter_mut().find(|held| held.key == tag.key) {
            Some(held) => held.value = tag.value.clone(),
            None => merged.push(tag.clone()),
        }
    }
    merged
}

/// The tag set a request's `tags` list assigns, as the resource holds it: the
/// list is collapsed through [`merge_tags`], so a key the SAME list carries
/// twice is held once with the last value that list gave it (the model puts no
/// `@uniqueItems` on `TagList`, so a request may legitimately repeat a key).
///
/// Every path that takes a tag list off the wire goes through this, so one tag
/// list always yields one stored tag set whichever operation received it.
pub fn tag_set(tags: Option<&[Tag]>) -> Vec<Tag> {
    merge_tags(&[], tags.unwrap_or_default())
}

/// A stored record an `Identifier` member can name: it carries the bare UUID it
/// is keyed by and the ARN minted for it, which are exactly the two forms the
/// model's `Identifier` shape accepts.
pub trait IdentifiedRecord {
    /// The record's bare UUID — the key it is stored under.
    fn id(&self) -> &str;
    /// The ARN the create call minted for it.
    fn arn(&self) -> &str;
}

impl IdentifiedRecord for AppBundleRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn arn(&self) -> &str {
        &self.arn
    }
}

impl IdentifiedRecord for IngestionRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn arn(&self) -> &str {
        &self.arn
    }
}

impl IdentifiedRecord for IngestionDestinationRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn arn(&self) -> &str {
        &self.arn
    }
}

impl IdentifiedRecord for AppAuthorizationRecord {
    fn id(&self) -> &str {
        &self.id
    }

    fn arn(&self) -> &str {
        &self.arn
    }
}

/// The key `records` holds the record `identifier` names, for ANY level of this
/// service's resource chain.
///
/// Resolving one of the two forms the model's `Identifier` shape accepts
/// (`^arn:.+$|^[a-f0-9]{8}-…$`) is ONE rule, so it is written once here and the
/// per-level functions below call it: an identifier in its ARN form answers the
/// key of the record whose own ARN it is, and any other identifier IS the key,
/// answered only when `records` holds it. Either way the answer is a key
/// `records` contains, or `None` when the collection holds no such record.
pub fn identifier_key<R: IdentifiedRecord>(
    records: &HashMap<String, R>,
    identifier: &str,
) -> Option<String> {
    if is_arn(identifier) {
        records
            .values()
            .find(|record| record.arn() == identifier)
            .map(|record| record.id().to_string())
    } else {
        records
            .contains_key(identifier)
            .then(|| identifier.to_string())
    }
}

/// The store key of the ingestion `identifier` names inside `bundle` — the one
/// rule [`identifier_key`] holds, over that bundle's ingestions.
pub fn ingestion_key(bundle: &AppBundleRecord, identifier: &str) -> Option<String> {
    identifier_key(&bundle.ingestions, identifier)
}

/// The store key of the ingestion destination `identifier` names inside
/// `ingestion` — the one rule [`identifier_key`] holds, over that ingestion's
/// destinations.
pub fn destination_key(ingestion: &IngestionRecord, identifier: &str) -> Option<String> {
    identifier_key(&ingestion.destinations, identifier)
}

/// The store key of the app authorization `identifier` names inside `bundle` —
/// the one rule [`identifier_key`] holds, over that bundle's app
/// authorizations.
pub fn app_authorization_key(bundle: &AppBundleRecord, identifier: &str) -> Option<String> {
    identifier_key(&bundle.app_authorizations, identifier)
}

/// Whether an `Identifier` is in its ARN form rather than its bare-UUID form.
/// The model's pattern allows exactly these two (`^arn:.+$` or a UUID).
fn is_arn(identifier: &str) -> bool {
    identifier.starts_with("arn:")
}

/// A fresh resource id: a version-4 UUID in the lowercase hyphenated form the
/// model's `UUID`/`Identifier` patterns require (RFC 9562 §5.4 — version `4` in
/// the high nibble of octet 6, variant `0b10` in the top bits of octet 8).
///
/// Its bits come from SHA-256 over three independent sources, so an id is both
/// unguessable and unique within the process:
///
/// * the OS entropy std seeds a fresh `RandomState` with on every call, whose
///   hash of the counter below is therefore unpredictable;
/// * a monotonic counter, which makes two ids minted in the same nanosecond on
///   different threads differ by construction;
/// * the wall clock in nanoseconds, so ids do not repeat across processes.
///
/// This takes NO new dependency: `sha2` is already in this crate's dependency
/// closure, and `Cargo.lock` is the one file every concurrent fix in this
/// repository touches, so leaving it untouched is what lets this slice land.
pub fn new_uuid() -> String {
    use std::fmt::Write as _;
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);
    let counter = MINTED.fetch_add(1, Ordering::Relaxed);

    let mut hasher = std::collections::hash_map::RandomState::new().build_hasher();
    hasher.write_u64(counter);
    let entropy = hasher.finish();

    let nanos = SystemTime::now()
        .duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, entropy.to_le_bytes());
    ::sha2::Digest::update(&mut digest, counter.to_le_bytes());
    ::sha2::Digest::update(&mut digest, nanos.to_le_bytes());
    let hashed = ::sha2::Digest::finalize(digest);

    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&hashed[..16]);
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let mut rendered = String::with_capacity(36);
    for (index, byte) in bytes.iter().enumerate() {
        if matches!(index, 4 | 6 | 8 | 10) {
            rendered.push('-');
        }
        // Infallible: writing to a String never fails.
        let _ = write!(rendered, "{byte:02x}");
    }
    rendered
}

/// The region to build this request's ARNs in: the one the caller signed for,
/// falling back to [`DEFAULT_REGION`] for an unsigned request.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    ::emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// This service's ARN for `resource`, e.g. `appbundle/<uuid>`.
pub fn appfabric_arn(region: &str, resource: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, resource)
}

/// The service endpoint host for `region`, as the model's own endpoint rule set
/// renders it (`https://appfabric.{Region}.{PartitionResult#dnsSuffix}`) in the
/// `aws` partition — the host a caller addresses this service at.
pub fn endpoint_host(region: &str) -> String {
    format!("{ARN_SERVICE}.{region}.{DNS_SUFFIX}")
}

/// The URL the OAuth flow of ONE app authorization continues at.
///
/// Built from the two halves the model states: the service endpoint for the
/// authorization's Region ([`endpoint_host`]) and the `ConnectAppAuthorization`
/// HTTP URI the model binds
/// (`/appbundles/{appBundleIdentifier}/appauthorizations/{appAuthorizationIdentifier}/connect`),
/// over the bare UUIDs the bundle and the authorization are stored under. It is
/// therefore derived from stored state rather than from the request, stable
/// across reads for as long as the authorization exists, and a URL this emulator
/// itself answers rather than one no endpoint serves. It carries no credential
/// material: the consent step's own secrets are the caller's.
pub fn oauth_flow_url(region: &str, app_bundle_id: &str, authorization_id: &str) -> String {
    format!(
        "https://{}/appbundles/{app_bundle_id}/appauthorizations/{authorization_id}/connect",
        endpoint_host(region)
    )
}

/// The KMS key ARN a `customerManagedKeyIdentifier` denotes.
///
/// The member takes a key ARN *or* a bare key id, while the response member is
/// an ARN (`customerManagedKeyArn`), so a bare id is expanded into the key ARN
/// it names and an ARN is kept as given.
pub fn customer_managed_key_arn(identifier: &str, region: &str) -> String {
    if is_arn(identifier) {
        identifier.to_string()
    } else {
        ::engine::emulator_arn("kms", region, &format!("key/{identifier}"))
    }
}

/// The [`ComAmazonawsAppfabricFabricfrontendserviceState::client_tokens`] key for
/// one operation's token in one Region.
///
/// Keyed by operation as well as token, because an idempotency token is scoped
/// to the operation it was passed to: the same token on `CreateAppBundle` and on
/// `CreateIngestion` is two independent requests.
///
/// Keyed by Region too, because each Region is its own endpoint holding its own
/// resources and its own quota: a token spent against one Region says nothing
/// about a call signed for another, so the same token in a second Region is a
/// fresh request there rather than a retry that replays the first Region's
/// resource.
///
/// Every component is length-prefixed (as [`request_fingerprint`]'s are), so no
/// caller-chosen token value can make two different (Region, operation, token)
/// triples render alike.
pub fn client_token_key(region: &str, operation: &str, token: &str) -> String {
    let mut key = String::new();
    push_component(&mut key, region);
    push_component(&mut key, operation);
    push_component(&mut key, token);
    key
}

/// A canonical rendering of a create request's parameters, so a replayed
/// idempotency token carrying DIFFERENT parameters can be told from a true
/// retry (which replays the first call's resource).
///
/// Every component is LENGTH-PREFIXED rather than joined by a separator, so the
/// rendering is injective: no caller-chosen tag or tenant value can contain the
/// delimiter and make two different requests render alike (which would replay
/// the wrong resource, or refuse a true retry).
///
/// Tags are sorted, because a tag list is a set of key/value pairs: the same
/// tags in another order are the same request.
pub fn request_fingerprint(fields: &[(&str, &str)], tags: Option<&[Tag]>) -> String {
    let mut rendered = String::new();
    for (name, value) in fields {
        push_component(&mut rendered, name);
        push_component(&mut rendered, value);
    }
    let mut pairs: Vec<(&str, &str)> = tags
        .unwrap_or_default()
        .iter()
        .map(|tag| (tag.key.as_str(), tag.value.as_str()))
        .collect();
    pairs.sort_unstable();
    // The tag count is part of the rendering too, so a field value cannot be
    // confused with the start of the tag section.
    push_component(&mut rendered, &pairs.len().to_string());
    for (key, value) in pairs {
        push_component(&mut rendered, key);
        push_component(&mut rendered, value);
    }
    rendered
}

/// Append one length-prefixed component of a [`request_fingerprint`].
fn push_component(rendered: &mut String, value: &str) {
    rendered.push_str(&value.len().to_string());
    rendered.push(':');
    rendered.push_str(value);
}

/// A canonical rendering of a `ProcessingConfiguration`, so it can go into a
/// [`request_fingerprint`] as one component: the union member that was set, then
/// that member's own values.
///
/// Every component is length-prefixed for the same reason the fingerprint's are:
/// no two different configurations may render alike.
pub fn render_processing_configuration(configuration: &ProcessingConfiguration) -> String {
    let mut rendered = String::new();
    match configuration {
        ProcessingConfiguration::auditLog(audit_log) => {
            push_component(&mut rendered, "auditLog");
            push_component(&mut rendered, &audit_log.schema);
            push_component(&mut rendered, &audit_log.format);
        }
    }
    rendered
}

/// The same canonical rendering for a `DestinationConfiguration`.
pub fn render_destination_configuration(configuration: &DestinationConfiguration) -> String {
    let mut rendered = String::new();
    match configuration {
        DestinationConfiguration::auditLog(audit_log) => {
            push_component(&mut rendered, "auditLog");
            match &audit_log.destination {
                Destination::s3Bucket(bucket) => {
                    push_component(&mut rendered, "s3Bucket");
                    push_component(&mut rendered, &bucket.bucket_name);
                    push_component(&mut rendered, bucket.prefix.as_deref().unwrap_or_default());
                }
                Destination::firehoseStream(stream) => {
                    push_component(&mut rendered, "firehoseStream");
                    push_component(&mut rendered, &stream.stream_name);
                }
            }
        }
    }
    rendered
}

/// A canonical rendering of a `Credential`, so it can go into a
/// [`request_fingerprint`] as one component: the union member that was set AND
/// that member's own values, reduced to a one-way SHA-256 digest.
///
/// Every parameter of a request takes part in the idempotency judgement a spent
/// `clientToken` is decided by, and a credential is a parameter like any other:
/// a replay carrying another API key, client id or client secret is a different
/// request, not a retry. The DIGEST is what lets it take part without the
/// fingerprint the store keeps holding credential material — two different
/// credentials render differently, while neither can be read back out of what
/// was stored. The member name goes inside the digest with the values, so a
/// credential of another kind renders differently too.
///
/// Each value is length-prefixed before hashing, for the same reason a
/// fingerprint's components are: no two different credentials may render alike.
pub fn render_credential(credential: &Credential) -> String {
    let mut material = String::new();
    match credential {
        Credential::oauth2Credential(oauth2) => {
            push_component(&mut material, "oauth2Credential");
            push_component(&mut material, &oauth2.client_id);
            push_component(&mut material, &oauth2.client_secret);
        }
        Credential::apiKeyCredential(api_key) => {
            push_component(&mut material, "apiKeyCredential");
            push_component(&mut material, &api_key.api_key);
        }
    }

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, material.as_bytes());
    ::hex::encode(::sha2::Digest::finalize(digest))
}

/// The one-way digest of the OAuth2 CLIENT ID a credential carries, and `None`
/// for a credential that carries none (an API key).
///
/// It is what [`duplicate_authorization_refusal`] decides which sentence a
/// refused duplicate carries by: two credentials naming one client id digest
/// alike however their secrets differ — the secret is deliberately NOT part of
/// it, because rotating a secret does not make a new registration — while the
/// client id itself cannot be read back out of what is stored, for the reason
/// [`render_credential`] gives. The member name goes into the digest with the
/// value, so no other rendering this module hashes can collide with it.
pub fn render_client_id(credential: &Credential) -> Option<String> {
    let Credential::oauth2Credential(oauth2) = credential else {
        return None;
    };
    let mut material = String::new();
    push_component(&mut material, "clientId");
    push_component(&mut material, &oauth2.client_id);

    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, material.as_bytes());
    Some(::hex::encode(::sha2::Digest::finalize(digest)))
}

/// One page of a listing, and the token that asks for the page after it.
#[derive(Debug)]
pub struct Page<T> {
    pub items: Vec<T>,
    /// `None` on the last page, so a caller that follows tokens stops.
    pub next_token: Option<String>,
}

/// Take one page out of `records`, which must be in creation order.
///
/// `maxResults` is an upper limit on the page (the model bounds it at
/// 1..=[`MAX_PAGE_SIZE`], and a caller that names none gets a full page), and
/// the token names the sequence of the last record already returned, so the next
/// page starts after it even when records in between were deleted.
/// `listing` names the listing the token belongs to (see [`encode_page_token`]),
/// so a token one listing issued is refused by another instead of silently
/// skipping records, and a token no listing issued at all is refused by every
/// one of them.
///
/// `now` is the instant the call is answered as of: the token this page hands
/// out carries it as its issue time, and a token the caller brought is refused
/// once `now` is more than [`PAGE_TOKEN_LIFETIME`] past the instant IT was
/// issued.
pub fn page<R, T>(
    records: &[&R],
    listing: &str,
    sequence_of: impl Fn(&R) -> u64,
    item_of: impl Fn(&R) -> T,
    max_results: Option<i32>,
    next_token: Option<&str>,
    now: SystemTime,
) -> Result<Page<T>, ValidationException> {
    let after = match next_token {
        Some(token) => Some(decode_page_token(listing, token, now)?),
        None => None,
    };
    let limit = max_results.map_or(MAX_PAGE_SIZE, |max| {
        usize::try_from(max)
            .unwrap_or(MAX_PAGE_SIZE)
            .clamp(1, MAX_PAGE_SIZE)
    });

    let remaining: Vec<&R> = records
        .iter()
        .copied()
        .filter(|record| after.is_none_or(|after| sequence_of(record) > after))
        .collect();
    let items: Vec<T> = remaining.iter().take(limit).map(|r| item_of(r)).collect();
    // A token is issued only when records are left over, and it names the last
    // record THIS page returned — so the next page starts right after it.
    let next_token = (remaining.len() > items.len())
        .then(|| items.len().saturating_sub(1))
        .and_then(|last| remaining.get(last))
        .map(|last| encode_page_token(listing, sequence_of(last), now));

    Ok(Page { items, next_token })
}

/// The opaque `nextToken` naming the position after `sequence` WITHIN one
/// listing ([`APP_BUNDLE_LISTING`], or [`ingestion_listing`] for one bundle's
/// ingestions), as issued at `issued_at`.
///
/// The listing is part of the token because a position is only meaningful inside
/// the listing that issued it: a token carried across to another listing — or to
/// another bundle's ingestions — would otherwise be read as a position there and
/// silently skip records. Decoding checks it, so such a token is refused.
///
/// The ISSUE INSTANT is part of it because the token has a lifetime
/// ([`PAGE_TOKEN_LIFETIME`]): it is carried so that a later call can tell how old
/// the token it is handed is, which a token holding only a position cannot.
///
/// The INTEGRITY TAG ([`page_token_tag`]) is part of it because a `nextToken` is
/// opaque: a position is a position only because this listing rendered a token
/// naming it, so the token carries this service's own derivation over its fields.
/// Decoding recomputes it, which is what lets a value this service never
/// rendered — any other rendering of these same fields included — be refused
/// instead of read as some position in the listing.
///
/// The rendering is a version prefix, the listing and three fixed-width hex
/// fields: every character is URL-unreserved, so the token survives a round trip
/// through a query string whatever the client's encoder does with it (base64's
/// `+` and `/` do not), and it is well inside the 1..=2048 the model allows.
pub fn encode_page_token(listing: &str, sequence: u64, issued_at: SystemTime) -> String {
    let issued_at = epoch_millis(issued_at);
    let fields = format!("v1-{listing}-{sequence:016x}-{issued_at:016x}");
    let tag = page_token_tag(&fields);
    format!("{fields}-{tag}")
}

/// Bytes of the integrity tag a [`encode_page_token`] token carries after the
/// fields it is taken over.
const PAGE_TOKEN_TAG_LEN: usize = 8;

/// The domain separator the tag is taken over, so a tag minted for one of this
/// service's page tokens is a tag for nothing else.
const PAGE_TOKEN_DOMAIN: &[u8] = b"appfabric/page-token/v1\0";

/// The integrity tag of `fields`, the rendered fields of a page token: the
/// leading [`PAGE_TOKEN_TAG_LEN`] bytes of SHA-256 over the domain separator and
/// those fields, in hex.
///
/// It is a DERIVATION, not a secret-keyed authenticator: no key takes part, so
/// what it establishes is that the token is one this service's own encoder
/// rendered for exactly these fields — which is what a listing needs to tell a
/// token it issued from a position a caller wrote itself, since a token is
/// opaque and nothing publishes the derivation.
///
/// It is taken over the fields AS RENDERED rather than over their values, so a
/// token whose fields are re-rendered — a position written with fewer digits,
/// hex in another case — carries a tag that is not the one for it.
fn page_token_tag(fields: &str) -> String {
    let mut digest = <::sha2::Sha256 as ::sha2::Digest>::new();
    ::sha2::Digest::update(&mut digest, PAGE_TOKEN_DOMAIN);
    ::sha2::Digest::update(&mut digest, fields.as_bytes());
    ::hex::encode(&::sha2::Digest::finalize(digest)[..PAGE_TOKEN_TAG_LEN])
}

/// The position a `nextToken` names in `listing`, or a `ValidationException`
/// naming the member when the token is not one this listing issued, or is one it
/// issued longer than [`PAGE_TOKEN_LIFETIME`] before `now`.
///
/// A token is one this listing rendered only when it carries the integrity tag
/// [`encode_page_token`] derives for the very fields it holds: the digits a token
/// spells are read as a position, an issue instant and nothing else AFTER that
/// tag has been checked, so a well-formed position no page of this listing ever
/// handed out is refused here rather than answered with a page.
pub fn decode_page_token(
    listing: &str,
    token: &str,
    now: SystemTime,
) -> Result<u64, ValidationException> {
    let not_a_token =
        || field_validation_failed("nextToken", "nextToken is not a valid pagination token");

    // The tag is the last field, and the ones before it are what it is taken
    // over — the listing included, so a token of another listing fails the tag
    // as well as the prefix.
    let (fields, tag) = token.rsplit_once('-').ok_or_else(not_a_token)?;
    let (sequence, issued_at) = fields
        .strip_prefix(&format!("v1-{listing}-"))
        .and_then(|fields| fields.split_once('-'))
        .ok_or_else(not_a_token)?;
    if page_token_tag(fields) != tag {
        return Err(not_a_token());
    }
    let sequence = u64::from_str_radix(sequence, 16).map_err(|_| not_a_token())?;
    let issued_at = u64::from_str_radix(issued_at, 16).map_err(|_| not_a_token())?;

    // A token is expired once the lifetime has fully elapsed since it was
    // issued. An issue instant no clock could have produced (one that does not
    // land in the representable range) is not one this service rendered, so such
    // a token is refused as malformed rather than as expired.
    let expires_at = ::std::time::UNIX_EPOCH
        .checked_add(Duration::from_millis(issued_at))
        .and_then(|issued_at| issued_at.checked_add(PAGE_TOKEN_LIFETIME))
        .ok_or_else(not_a_token)?;
    if now > expires_at {
        return Err(expired_page_token());
    }

    Ok(sequence)
}

/// Milliseconds since the Unix epoch, which is how an instant is rendered into
/// a [`encode_page_token`] token: a whole number of milliseconds is plenty to
/// date a token whose lifetime is counted in hours.
fn epoch_millis(at: SystemTime) -> u64 {
    at.duration_since(::std::time::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_millis())
        .map(|millis| u64::try_from(millis).unwrap_or(u64::MAX))
        .unwrap_or_default()
}

/// A modelled `ValidationException` refusing a `nextToken` the service DID issue
/// but whose documented lifetime has run out.
///
/// The service models no separate token error (`ValidationException` is its only
/// HTTP 400), and the member is at fault by name, so the refusal is a
/// field-level one naming `nextToken` — distinct in wording from a token this
/// service never issued, because the caller's remedy differs: a listing restarted
/// from its first page issues a usable token, while a malformed token stays
/// malformed however often it is replayed.
fn expired_page_token() -> ValidationException {
    field_validation_failed(
        "nextToken",
        &format!(
            "nextToken is expired: each pagination token expires after {} hours",
            PAGE_TOKEN_LIFETIME.as_secs() / 3600
        ),
    )
}

/// The [`encode_page_token`] listing name of `ListAppBundles`.
pub const APP_BUNDLE_LISTING: &str = "appbundles";

/// The [`encode_page_token`] listing name of `ListIngestions` for ONE bundle:
/// the ingestions of two bundles are two listings, so a token from one is not a
/// position in the other.
pub fn ingestion_listing(bundle_id: &str) -> String {
    format!("ingestions-{bundle_id}")
}

/// The [`encode_page_token`] listing name of `ListIngestionDestinations` for ONE
/// ingestion, for the same reason: a position in one ingestion's destinations is
/// not a position in another's.
pub fn ingestion_destination_listing(ingestion_id: &str) -> String {
    format!("ingestiondestinations-{ingestion_id}")
}

/// The [`encode_page_token`] listing name of `ListAppAuthorizations` for ONE
/// bundle.
pub fn app_authorization_listing(bundle_id: &str) -> String {
    format!("appauthorizations-{bundle_id}")
}

/// The `reason` a `ValidationException` carries when a NAMED member of the
/// request is at fault, from the model's own `ValidationExceptionReason` value
/// set (`VALIDATION_EXCEPTION_REASON_VALUES` in the generated types:
/// `cannotParse` | `fieldValidationFailed` | `other` | `unknownOperation`).
///
/// Named here, beside the other modelled values this crate writes, so every
/// refusal of this class — a handler's own and one the request validator in
/// front of the handlers raises — spells it the same way.
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// The `reason` for a request this service could not READ at all, from the same
/// value set: nothing about it became a member value, so no member is named.
pub const VALIDATION_REASON_CANNOT_PARSE: &str = "cannotParse";

/// The `reason` for a refusal that faults no member of the request, from the
/// same value set — the state of the STORED resource forbids the call.
pub const VALIDATION_REASON_OTHER: &str = "other";

/// A modelled `ValidationException` refusing the call because the STORED
/// resource is not in the state the operation documents as its precondition.
///
/// `reason` is `other` rather than `fieldValidationFailed`, and there is no
/// `fieldList`: every member of the request is valid (the boundary validator
/// accepted them all) — it is the resource the request names that forbids the
/// call — so naming a member would not be truthful.
pub fn precondition_not_met(message: String) -> ValidationException {
    ValidationException {
        message,
        reason: VALIDATION_REASON_OTHER.to_string(),
        field_list: None,
    }
}

/// A modelled `ValidationException` refusing one member by name.
pub fn field_validation_failed(member: &str, message: &str) -> ValidationException {
    fields_not_valid(
        message.to_string(),
        vec![ValidationExceptionField {
            name: member.to_string(),
            message: message.to_string(),
        }],
    )
}

/// A modelled `ValidationException` refusing the members `fields` names, with
/// `message` stating the refusal as a whole.
///
/// The general form of [`field_validation_failed`], which is this with one
/// field: one request can fault SEVERAL members at once (the request validator
/// in front of the handlers walks every declared constraint before it answers),
/// and `fieldList` is the member a client reads to learn which.
pub fn fields_not_valid(
    message: String,
    fields: Vec<ValidationExceptionField>,
) -> ValidationException {
    ValidationException {
        message,
        reason: VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(fields),
    }
}

/// A modelled `ValidationException` refusing a request this service could not
/// READ: a member carrying a value of the wrong type, or a body that is not the
/// operation's input at all.
///
/// No `fieldList`: a request the codec could not read yields no member value to
/// fault, and the model leaves that member optional where it requires `reason`.
pub fn cannot_parse(message: String) -> ValidationException {
    ValidationException {
        message,
        reason: VALIDATION_REASON_CANNOT_PARSE.to_string(),
        field_list: None,
    }
}

/// A modelled `ResourceNotFoundException` for an app bundle the store does not
/// hold, as a REPORTING operation (`GetAppBundle`, `ListIngestions`,
/// `GetAppAuthorization`, …) states it.
///
/// The message is a CONSTANT: AppFabric's not-found text names the KIND of
/// resource and the path that refused, and never interpolates the identifier the
/// caller supplied — two reads of two different bundles are refused with the
/// same sentence. The identifier still travels in the modelled `resourceId`
/// member, which is where a client reads it from.
pub fn app_bundle_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The app bundle ID was not found.".to_string(),
        resource_id: identifier.to_string(),
        resource_type: "AppBundle".to_string(),
    }
}

/// The same refusal on a MUTATION path (`CreateAppAuthorization`,
/// `CreateIngestion`, `StopIngestion`, …), which AppFabric words differently:
/// a write names the bundle it writes INTO, so the refusal reports that
/// identifier as not valid rather than reporting a read that found nothing.
/// Same status (404) and same code as [`app_bundle_not_found`]; the two
/// sentences are what distinguishes the two paths, and neither carries the
/// caller's identifier.
pub fn app_bundle_id_not_valid(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The specified app bundle ID is not valid".to_string(),
        resource_id: identifier.to_string(),
        resource_type: "AppBundle".to_string(),
    }
}

/// A modelled `ResourceNotFoundException` for an ingestion the store does not
/// hold — a constant sentence, for the reason [`app_bundle_not_found`] states.
pub fn ingestion_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The specified ingestion does not exist".to_string(),
        resource_id: identifier.to_string(),
        resource_type: "Ingestion".to_string(),
    }
}

/// A modelled `ResourceNotFoundException` for an app authorization the store
/// does not hold — a constant sentence, for the same reason.
pub fn app_authorization_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The app authorization was not found.".to_string(),
        resource_id: identifier.to_string(),
        resource_type: "AppAuthorization".to_string(),
    }
}

/// A modelled `ResourceNotFoundException` for an ingestion destination the store
/// does not hold — the REPORTING path's refusal (`GetIngestionDestination`,
/// `ListIngestionDestinations`), which AppFabric answers with HTTP 404.
pub fn ingestion_destination_not_found(identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The specified ingestion destination does not exist".to_string(),
        resource_id: identifier.to_string(),
        resource_type: "IngestionDestination".to_string(),
    }
}

/// A modelled `ValidationException` refusing a MUTATION of an ingestion
/// destination because the ingestion it is nested under does not exist.
///
/// AppFabric checks the ingestion-destination chain on its mutation path as
/// INPUT validation rather than as a lookup: a `*IngestionDestination` write
/// naming an ingestion that does not exist is answered HTTP 400
/// `ValidationException`, while the reporting path answers HTTP 404
/// (see [`ingestion_not_found`]).
pub fn ingestion_does_not_exist() -> ValidationException {
    field_validation_failed(
        "ingestionIdentifier",
        "The specified ingestion does not exist",
    )
}

/// The same refusal for the ingestion destination the mutation names.
pub fn ingestion_destination_does_not_exist() -> ValidationException {
    field_validation_failed(
        "ingestionDestinationIdentifier",
        "The specified ingestion destination does not exist",
    )
}

/// The union member a `Credential` carries — never the secret inside it.
///
/// A credential is `@sensitive` on both arms and no modelled response reports it
/// back, so this is the whole of what an authorization record keeps of it: which
/// KIND of credential the caller supplied.
pub fn credential_kind(credential: &Credential) -> &'static str {
    match credential {
        Credential::oauth2Credential(_) => "oauth2Credential",
        Credential::apiKeyCredential(_) => "apiKeyCredential",
    }
}

/// The credential kind one `authType` requires.
///
/// The model documents the pairing on `credential`: "Specify credentials that
/// match the authorization type for your request. For example, if the
/// authorization type for your request is OAuth2 (oauth2), then you should
/// provide only the OAuth2 credentials." `AuthType` is closed by the model
/// (`AUTH_TYPE_VALUES` = `apiKey` | `oauth2`) and the boundary validator has
/// already refused anything else, so the `oauth2` arm is the only remaining
/// alternative to `apiKey`.
pub fn credential_kind_for_auth_type(auth_type: &str) -> &'static str {
    if auth_type == AUTH_TYPE_API_KEY {
        "apiKeyCredential"
    } else {
        "oauth2Credential"
    }
}

/// A modelled `ValidationException` refusing a `credential` that is not the kind
/// the request's `authType` uses — see [`credential_kind_for_auth_type`].
pub fn credential_does_not_match_auth_type(auth_type: &str) -> ValidationException {
    field_validation_failed(
        "credential",
        &format!(
            "credential must be a {} for authType {auth_type}",
            credential_kind_for_auth_type(auth_type)
        ),
    )
}

/// A modelled `ConflictException` for an ingestion whose stored `state` forbids
/// the transition the call asks for.
///
/// `StartIngestion` and `StopIngestion` each move an ingestion one way between
/// the model's two `IngestionState` values, so a call that finds it already in
/// the state it would move it to has no transition to make: that is the conflict
/// both operations model ("the request has created a conflict, check the request
/// parameters and try again"). The message is built from the request's target
/// and the stored state, which is what tells a caller why.
pub fn ingestion_state_conflict(arn: &str, held: &str, required: &str) -> ConflictException {
    ConflictException {
        message: format!("The ingestion is {held}; this request requires it to be {required}"),
        resource_id: arn.to_string(),
        resource_type: "Ingestion".to_string(),
    }
}

/// A modelled `ValidationException` refusing a connect call that carries no
/// `authRequest` for an authorization whose `authType` requires one.
///
/// The model states the condition on the member: `authRequest` "is required if
/// the app authorization for the request is configured with an OAuth2 (oauth2)
/// authorization type".
pub fn auth_request_required(auth_type: &str) -> ValidationException {
    field_validation_failed(
        "authRequest",
        &format!("authRequest is required for an app authorization with authType {auth_type}"),
    )
}

/// The sentence AppFabric refuses an ingestion with when the app bundle holds NO
/// app authorization at all for the request's (application, tenant) pair.
///
/// FIXED TEXT, with no variable part: AppFabric says "the specified application
/// and tenant" rather than echoing either value, so two refusals naming two
/// different pairs read identically — the pair a caller asked for is in its own
/// request, not in the service's sentence.
pub const APP_AUTHORIZATION_NOT_FOUND_FOR_INGESTION: &str =
    "The app authorization for the specified application and tenant was not found. \
     Create an app authorization for the application and tenant before creating an ingestion.";

/// The sibling sentence for the pair the bundle DOES hold an app authorization
/// for, one that has not completed the connect handshake.
///
/// Same error shape, and the same value-free form, with the remediation naming
/// the connect step instead of the create step: AppFabric states the handshake
/// the pair never completed in the past tense ("was not connected"), and names
/// neither the application nor the tenant it refused.
pub const APP_AUTHORIZATION_NOT_CONNECTED_FOR_INGESTION: &str =
    "The app authorization for the specified application and tenant was not connected. \
     Connect the app authorization for the application and tenant before creating an ingestion.";

/// Which call a caller still owes before an ingestion of one (application,
/// tenant) pair can be created — what the refusal below states, and the ONLY
/// thing the handler tells it about the store: a stored status cannot be
/// rendered into a message it never reaches.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum OwedAuthorizationStep {
    /// The app bundle holds no app authorization for the pair at all.
    CreateAuthorization,
    /// It holds one that has not completed the connect handshake.
    ConnectAuthorization,
}

/// A modelled `ValidationException` refusing an ingestion of an (application,
/// tenant) pair the app bundle holds no CONNECTED app authorization for.
///
/// `owed` selects which of the two sentences above the refusal carries — the one
/// whose remediation the caller still owes. Neither sentence has a variable
/// part, so nothing of the request or of the stored record reaches the message.
pub fn app_authorization_not_connected(owed: OwedAuthorizationStep) -> ValidationException {
    precondition_not_met(
        match owed {
            OwedAuthorizationStep::CreateAuthorization => APP_AUTHORIZATION_NOT_FOUND_FOR_INGESTION,
            OwedAuthorizationStep::ConnectAuthorization => {
                APP_AUTHORIZATION_NOT_CONNECTED_FOR_INGESTION
            }
        }
        .to_string(),
    )
}

/// A modelled `ValidationException` for a create call that would be the SECOND
/// app authorization of one (app bundle, app, tenant) triple.
///
/// AppFabric holds at most one authorization per triple, so the second create —
/// whether it follows the first or races it — cannot be carried out. It is the
/// VALIDATION error of the two client errors `CreateAppAuthorization` models:
/// the request can never succeed as written while the first authorization
/// stands, so it is not a conflict that settles on its own and a retry of it is
/// pointless. The sentence is AppFabric's own and has no variable part — it
/// states the tenant and the authorization type generically rather than echoing
/// either — so `reason` is `other` and no member is faulted (see
/// [`precondition_not_met`]).
pub fn app_authorization_already_exists() -> ValidationException {
    precondition_not_met(
        "An AppAuthorization already exists for this tenant and this authType.".to_string(),
    )
}

/// A modelled `ValidationException` for a call refused because the OAuth2 CLIENT
/// ID it supplied is the one the app authorization standing in its way is
/// already registered with.
///
/// The client id names the caller's registration of the application with
/// AppFabric, so a call that repeats a held registration's client id is refused
/// for THAT — the thing the request itself carries — rather than for the record
/// it collides with: the sentence states the scope the client id is held in (the
/// app bundle and the application) and asks the caller to supply another. It is
/// the VALIDATION error of the two client errors `CreateAppAuthorization`
/// models, for the reason [`app_authorization_already_exists`] gives: the
/// request cannot succeed as written while that registration stands. The
/// sentence is wholly fixed text, with no part of the request or of the stored
/// record in it (the client id a caller asked about is in its own request), so
/// `reason` is `other` and no member is faulted — see [`precondition_not_met`].
///
/// The sentence states WHERE the registration it names is held — the app bundle
/// and the application — not a uniqueness rule over that scope: a client id
/// backs as many of an application's tenants as a caller registers
/// authorizations for, and this refusal is reached only where the authorization
/// standing in the call's way is registered with the very client id the request
/// supplied.
pub fn client_id_already_in_use() -> ValidationException {
    precondition_not_met("ClientId is already being used for this AppBundle and App.".to_string())
}

/// WHICH sentence a call is refused with when it would leave an app bundle
/// holding a SECOND app authorization of one application for one tenant: `held`
/// is the authorization already standing there, `supplied_client_id` the OAuth2
/// client id digest the call's own credential registers (see
/// [`render_client_id`], `None` for a credential that registers none), and
/// `otherwise` the refusal for a call that repeats only the tenant — the create
/// path's [`app_authorization_already_exists`], the update path's
/// [`tenant_already_authorized`].
///
/// AppFabric names what the REQUEST repeats. A call that supplies the very
/// client id `held` is registered with repeats that registration, so it is
/// refused for the client id ([`client_id_already_in_use`]) — which is what the
/// losers of a race to register one client id are told, every one of them having
/// sent the credential the winner registered. A call that supplies another
/// credential, or none at all, repeats only the tenant and is refused for that.
pub fn duplicate_authorization_refusal(
    held: &AppAuthorizationRecord,
    supplied_client_id: Option<&str>,
    otherwise: ValidationException,
) -> ValidationException {
    if held.registered_with_client_id(supplied_client_id) {
        client_id_already_in_use()
    } else {
        otherwise
    }
}

/// A modelled `ValidationException` refusing an update that would move an app
/// authorization onto an (application, tenant) pair its app bundle already holds
/// ANOTHER authorization for.
///
/// The same at-most-one-per-triple invariant [`app_authorization_already_exists`]
/// guards on the create path. AppFabric documents no refusal of its own for an
/// update that would take a pair the bundle already holds, and
/// `UpdateAppAuthorization` models no conflict error, so the refusal is the
/// `ValidationException` it does model, naming the `tenant` member the request
/// supplied.
pub fn tenant_already_authorized(app: &str) -> ValidationException {
    field_validation_failed(
        "tenant",
        &format!(
            "another app authorization for app {app} and the specified tenant already exists"
        ),
    )
}

/// A modelled `ValidationException` refusing a call that would leave the
/// ingestion ASSOCIATED with an app authorization — the one the bundle holds for
/// that authorization's (application, tenant) pair, see
/// [`AppBundleRecord::ingestion_for`] — without an app authorization at all.
///
/// The precondition is the one `DeleteAppAuthorization` documents ("You must
/// delete the associated ingestion before you can delete an app authorization"),
/// and it binds every call that would break the association rather than only the
/// delete: an ingestion exists only for a pair whose authorization has completed
/// the connect handshake, so a call that removes the authorization from the pair
/// — by deleting it or by re-pointing it at another tenant — has to find the
/// ingestion gone first. `action` is what the refused call would have done to the
/// authorization, so the one precondition is worded once for every caller.
pub fn associated_ingestion_still_held(
    authorization_arn: &str,
    ingestion_arn: &str,
    action: &str,
) -> ValidationException {
    precondition_not_met(format!(
        "App authorization {authorization_arn} is associated with ingestion {ingestion_arn}; \
         you must delete the associated ingestion before you can {action} an app authorization."
    ))
}

/// A modelled `ValidationException` refusing a `DeleteAppBundle` of a bundle that
/// still holds app authorizations.
///
/// The precondition is the one the operation documents ("You must delete all
/// associated app authorizations before you can delete an app bundle"), and the
/// message is AppFabric's own sentence for a bundle that is not deletable yet:
/// it has no variable part — it states that the bundle the request named cannot
/// be deleted, not which authorization is in the way — so two refusals of two
/// different bundles read identically, and the bundle a caller asked about is in
/// its own request. `DeleteAppBundle` models both `ConflictException` (409) and
/// `ValidationException` (400), and AppFabric answers this one as the
/// validation error: a request that cannot succeed as written until the caller
/// deletes the contents itself, rather than a conflict that settles on its own.
/// `reason` is therefore `other` and no member is faulted — see
/// [`precondition_not_met`].
pub fn app_bundle_cannot_be_deleted() -> ValidationException {
    precondition_not_met("The specified app bundle cannot be deleted.".to_string())
}

/// A modelled `ResourceNotFoundException` for a tagging call whose `resourceArn`
/// names no resource this store holds.
///
/// The message is the constant the model documents the shape with ("The
/// specified resource does not exist."), for the reason
/// [`app_bundle_not_found`] states: AppFabric's not-found text does not echo the
/// identifier the caller supplied. The ARN is the only identifier the caller
/// gave, so it is the `resourceId`; the `resourceType` is read out of the ARN
/// itself (see [`arn_resource_type`]) because an ARN naming nothing has no
/// record to read a kind from.
pub fn tagged_resource_not_found(arn: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: "The specified resource does not exist.".to_string(),
        resource_id: arn.to_string(),
        resource_type: arn_resource_type(arn).to_string(),
    }
}

/// The `resourceType` an error reports for an ARN that names no resource of a
/// kind this service mints — another service's ARN, a value that is not an ARN
/// at all, or one of this service's own whose deepest type segment is not a type
/// this service has.
pub const GENERIC_RESOURCE_TYPE: &str = "Resource";

/// The kind of resource an ARN names, from the LAST type segment of its
/// resource field: `…:appbundle/<id>/ingestion/<id>` names an ingestion.
///
/// Only an ARN in THIS service's namespace can name one of its resources: the
/// third field of an ARN is the service namespace, and it is what says whose
/// resource the value addresses, so an ARN whose namespace is another service's
/// — and a value that is not a well-formed ARN at all — names nothing here
/// however its text reads, and is reported as the generic
/// [`GENERIC_RESOURCE_TYPE`].
///
/// Inside this service's own ARNs the resource field is a chain of `<type>/<id>`
/// pairs (AWS also writes the pair as `<type>:<id>`), so the type segments are
/// the EVEN ones: an identifier that happens to read like a type name sits in an
/// odd slot and names no type. That alternation is what this service's ARNs are
/// built from — the type segments are `appbundle`, `ingestion`,
/// `ingestiondestination` and `appauthorization` (see `CreateAppBundle`,
/// `CreateIngestion`, `CreateIngestionDestination` and
/// `CreateAppAuthorization`), each followed by a UUID, which carries neither
/// separator. The chain's last type segment is the resource the ARN names; a
/// segment naming none of those kinds is a kind this service does not have — the
/// generic type again, not the kind of the enclosing resource — so an ARN whose
/// resource field does not spell one of those chains is reported generically
/// too. An empty segment is no segment at all (a resource field written with a
/// doubled or trailing separator still spells the same chain), so it takes no
/// slot in the alternation.
pub fn arn_resource_type(arn: &str) -> &'static str {
    let Some(parsed) = ::service_common::arn::parse_of(arn, ARN_SERVICE) else {
        return GENERIC_RESOURCE_TYPE;
    };
    let mut kind = GENERIC_RESOURCE_TYPE;
    let segments = parsed
        .resource
        .split(['/', ':'])
        .filter(|segment| !segment.is_empty());
    for segment in segments.step_by(2) {
        kind = match segment {
            "appbundle" => "AppBundle",
            "ingestion" => "Ingestion",
            "ingestiondestination" => "IngestionDestination",
            "appauthorization" => "AppAuthorization",
            _ => GENERIC_RESOURCE_TYPE,
        };
    }
    kind
}

/// A modelled `ServiceQuotaExceededException` for a create call that would take
/// the store past one of the ceilings above.
pub fn quota_exceeded(
    resource_id: &str,
    resource_type: &str,
    quota_code: &str,
    limit: usize,
) -> ServiceQuotaExceededException {
    ServiceQuotaExceededException {
        message: format!(
            "The limit of {limit} {resource_type} resources for {resource_id} has been reached."
        ),
        resource_id: resource_id.to_string(),
        resource_type: resource_type.to_string(),
        service_code: ARN_SERVICE.to_string(),
        quota_code: quota_code.to_string(),
    }
}

/// A modelled `ConflictException` for an idempotency token replayed with
/// different parameters.
///
/// AWS documents this case as an `IdempotentParameterMismatch` failure, a shape
/// this service's model does not declare; `ConflictException` ("the request has
/// created a conflict, check the request parameters and try again") is the
/// modelled error that states it.
pub fn idempotent_parameter_mismatch(
    client_token: &str,
    resource_type: &str,
) -> ConflictException {
    ConflictException {
        message: format!(
            "The clientToken {client_token} was already used with different request parameters."
        ),
        resource_id: client_token.to_string(),
        resource_type: resource_type.to_string(),
    }
}

/// Record one state-changing AppFabric operation: what the caller asked for, who
/// asked (the access key id the request presented), which server-minted resource
/// changed and how the call ended.
///
/// The one place this service writes an audit record, so every create and delete
/// records the same fields. Only wire metadata and server-minted identifiers go
/// in: never a bundle's tags, its customer-managed key or any credential
/// material.
pub fn audit_resource_change(
    parts: &::http::request::Parts,
    operation: &'static str,
    resource_type: &'static str,
    resource_id: &str,
    outcome: &str,
) {
    ::tracing::info!(
        audit = true,
        operation,
        outcome,
        caller_akid = %::engine::extract_akid(&parts.headers),
        resource_type,
        resource_id,
        "appfabric resource changed"
    );
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsAppfabricFabricfrontendserviceState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsAppfabricFabricfrontendserviceState>)
}
