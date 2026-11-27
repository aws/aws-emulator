// SPDX-License-Identifier: CC0-1.0

//! The payment connectors one payment manager holds: the record a connector is
//! stored as, the status it reports, the address an end user completes a
//! service-orchestrated authorization at, and the response envelopes every
//! operation that reports a connector renders.
//!
//! One concern, kept beside the payment manager record that holds the connectors
//! rather than inside any one `ops/` file: a connector is addressed only THROUGH
//! its manager
//! (`/payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`), so
//! the create and every later read answer from this one shape and cannot drift.
//!
//! ## The two ways a connector is provisioned
//!
//! `provisionMode` decides what the create leaves behind
//! (`smithy_query doc shape=CreatePaymentConnectorRequest member=provisionMode`):
//!
//! * `MANUAL` — "You provide the credential provider configurations directly", so
//!   the connector authenticates with the credentials the request named from the
//!   moment it is created, and the only thing left is the `CREATING` -> `READY`
//!   transition the clock owes it;
//! * `QUICK_CREATE` — "The service orchestrates OAuth consent and provisions the
//!   credential provider for you", so the connector is created waiting for an end
//!   user to complete that consent: it reports `PENDING_AUTHENTICATION` and the
//!   address to complete it at ([`payment_connector_authorization_url`]), and no
//!   clock makes that consent happen — it is given at the payment provider, by a
//!   person, or it is not given at all.

use super::*;

use super::consent_portals::escaped;
use crate::ops::types::{
    CreatePaymentConnectorOutput, CredentialsProviderConfiguration, GetPaymentConnectorOutput,
    UpdatePaymentConnectorOutput,
};

/// `PaymentConnectorStatus` members this service drives. The enum's full value
/// list is `CREATING | UPDATING | DELETING | READY | CREATE_FAILED |
/// UPDATE_FAILED | DELETE_FAILED | AWS_MARKETPLACE_SUBSCRIPTION_REQUIRED |
/// PENDING_AUTHENTICATION | PROVISIONING | AUTHENTICATION_EXPIRED |
/// AUTHENTICATION_FAILED` (`smithy_query shape=PaymentConnectorStatus`); the
/// failure statuses belong to a provisioning that failed and the remaining
/// authentication ones to a consent that was given and then lapsed, neither of
/// which anything here makes happen.
pub const PAYMENT_CONNECTOR_STATUS_CREATING: &str = "CREATING";
pub const PAYMENT_CONNECTOR_STATUS_UPDATING: &str = "UPDATING";
pub const PAYMENT_CONNECTOR_STATUS_DELETING: &str = "DELETING";
pub const PAYMENT_CONNECTOR_STATUS_READY: &str = "READY";
pub const PAYMENT_CONNECTOR_STATUS_PENDING_AUTHENTICATION: &str = "PENDING_AUTHENTICATION";

/// `PaymentConnectorProvisionMode`'s full value list, `MANUAL | QUICK_CREATE`
/// (`smithy_query shape=PaymentConnectorProvisionMode`).
pub const PAYMENT_CONNECTOR_PROVISION_MODE_MANUAL: &str = "MANUAL";
pub const PAYMENT_CONNECTOR_PROVISION_MODE_QUICK_CREATE: &str = "QUICK_CREATE";

/// The `provisionMode` a request that names none is served under: "If you don't
/// specify a value, the default is MANUAL"
/// (`smithy_query doc shape=CreatePaymentConnectorRequest member=provisionMode`).
pub const DEFAULT_PAYMENT_CONNECTOR_PROVISION_MODE: &str =
    PAYMENT_CONNECTOR_PROVISION_MODE_MANUAL;

/// `PaymentConnectorType`'s full value list, `CoinbaseCDP | StripePrivy`
/// (`smithy_query shape=PaymentConnectorType`) — the same two vendors the token
/// vault's payment credential providers are configured for
/// ([`super::PAYMENT_VENDOR_COINBASE_CDP`]), because the connector's type is what
/// decides which credential provider it authenticates through.
pub const PAYMENT_CONNECTOR_TYPE_COINBASE_CDP: &str = super::PAYMENT_VENDOR_COINBASE_CDP;
pub const PAYMENT_CONNECTOR_TYPE_STRIPE_PRIVY: &str = super::PAYMENT_VENDOR_STRIPE_PRIVY;

/// How long a freshly created payment connector stays `CREATING` before the clock
/// owes it `READY`.
///
/// A connector is a child of the payment manager that holds it, so it settles on
/// the same span the payments family uses
/// ([`super::PAYMENT_MANAGER_SETTLE_DELAY`]): one chosen span for the whole
/// family rather than a second one only this resource would wait out. AWS
/// documents no duration for the transition.
pub const PAYMENT_CONNECTOR_SETTLE_DELAY: Duration = super::PAYMENT_MANAGER_SETTLE_DELAY;

/// How many payment connectors one payment manager will hold.
///
/// `CreatePaymentConnector` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding the real service's quota; this number is not a fidelity claim about
/// that quota but the emulator's own ceiling, set generously so no realistic
/// workflow meets it, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_CONNECTORS_PER_PAYMENT_MANAGER: usize = 1_000;

/// The modelled not-found error for a payment connector nothing resolves: the
/// resource, then the `paymentConnectorId` the request's path carried.
///
/// Every route that addresses ONE connector
/// (`/payments/managers/{paymentManagerId}/connectors/{paymentConnectorId}`)
/// reports THIS error, whether the gap is the connector or the manager that
/// would hold it: the connector is the resource the operation was asked to act
/// on, so it is the one reported missing and the id reported is the connector's
/// — the parent's own refusal
/// ([`connector_parent_payment_manager_not_found`]) belongs to the route that
/// addresses the manager's connector collection.
///
/// The payments plane words a missing resource this way — the text around the
/// identifier is fixed and the identifier itself is the caller's own value, taken
/// verbatim from the request — exactly as its sibling
/// [`super::payment_manager_not_found`] does, so one plane reports a missing
/// resource in one sentence.
pub fn payment_connector_not_found(payment_connector_id: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Payment connector not found: {payment_connector_id}"
        )),
    }
}

/// The modelled not-found error reported for the PARENT payment manager by the
/// route that addresses a manager's payment connector COLLECTION
/// (`/payments/managers/{paymentManagerId}/connectors`): the resource, then the
/// id that path carried under the `managerId=` label.
///
/// A route addressing the collection acts on the manager itself — there is no
/// connector id in it to report — and this surface words that identifier as the
/// labelled key/value pair its sibling payment-manager refusals carry
/// ([`super::payment_manager_update_conflict`]'s `managerId=`), not as the bare
/// id a route addressing the manager resource itself reports
/// ([`super::payment_manager_not_found`]). The text around the identifier is
/// fixed and the identifier is the caller's own value, taken verbatim from the
/// request path. The model states no message text, so the wording is the
/// recorded one.
pub fn connector_parent_payment_manager_not_found(
    payment_manager_id: &str,
) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: Some(format!(
            "Payment manager not found: managerId={payment_manager_id}"
        )),
    }
}

/// The modelled conflict for a write asked of a payment connector whose own
/// write is still under way: its create or a previous update has not taken
/// effect yet, so this one collides with it.
///
/// The sentence is fixed and tells the caller what to wait for; its variable
/// parts are the id the request's path carried, the status the connector is in
/// right now and the one the clock owes it, each as the enum spells it — the
/// same tokens every response reports a status in. Worded as its sibling
/// [`super::consent_portal_status_conflict`] words the same refusal, so one
/// service does not report a busy resource in two sentences.
pub fn payment_connector_status_conflict(
    payment_connector_id: &str,
    status: &str,
    settled_status: &str,
) -> ConflictException {
    ConflictException {
        message: Some(format!(
            "Payment connector {payment_connector_id} is in {status} status and cannot be \
             modified. Retry the request once its status is {settled_status}."
        )),
    }
}

/// The `credentialProviderConfigurations` member of one request, as the union
/// member names spell the vendor the configuration belongs to
/// (`smithy_query shape=CredentialsProviderConfiguration`).
///
/// `CredentialsProviderConfiguration` is "The credential provider configuration
/// for a payment connector. Specifies the payment provider type and its
/// associated credential provider", so the member a configuration is carried in
/// is what names that provider type, and the one that does not match the
/// connector's own `type` configures a different vendor's provider.
pub fn configured_connector_type(
    configuration: &CredentialsProviderConfiguration,
) -> &'static str {
    match configuration {
        CredentialsProviderConfiguration::coinbaseCDP(_) => PAYMENT_CONNECTOR_TYPE_COINBASE_CDP,
        CredentialsProviderConfiguration::stripePrivy(_) => PAYMENT_CONNECTOR_TYPE_STRIPE_PRIVY,
    }
}

/// The `credentialProviderArn` one configuration names, whichever vendor's member
/// carries it.
pub fn configured_credential_provider_arn(
    configuration: &CredentialsProviderConfiguration,
) -> &str {
    match configuration {
        CredentialsProviderConfiguration::coinbaseCDP(vendor)
        | CredentialsProviderConfiguration::stripePrivy(vendor) => &vendor.credential_provider_arn,
    }
}

/// Whether `credential_provider_arn` is the ARN of a payment credential provider
/// this account's token vault really holds.
///
/// The whole ARN is compared against the one the provider itself reports
/// ([`super::payment_credential_provider_arn`], which the provider's create
/// minted and every read of it returns), rather than its tail: an ARN that names
/// another partition, service, Region, account or token vault addresses a
/// provider this account does not hold, whatever name it ends in.
pub fn holds_payment_credential_provider(
    providers: &BTreeMap<String, PaymentCredentialProviderRecord>,
    credential_provider_arn: &str,
) -> bool {
    providers
        .values()
        .any(|provider| provider.credential_provider_arn == credential_provider_arn)
}

/// One payment connector of a payment manager: the configuration
/// `CreatePaymentConnector` persisted for it, and the status it reports.
///
/// Every operation that reports a connector renders it from this record, so a
/// create and a later read cannot disagree about its configuration or status.
#[derive(Debug, Clone)]
pub struct PaymentConnectorRecord {
    pub payment_connector_id: String,
    pub payment_manager_id: String,
    pub name: String,
    pub description: Option<String>,
    /// `type` — the payment provider integration this connector is for. Named
    /// without the keyword the modelled member spells it with.
    pub connector_type: String,
    pub provision_mode: String,
    pub credential_provider_configurations: Vec<CredentialsProviderConfiguration>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Where an end user completes the consent a `QUICK_CREATE` connector waits
    /// for; `None` for a connector whose credentials the caller provided itself,
    /// which waits for no consent.
    pub authorization_url: Option<String>,
    /// The status the connector reports until [`Self::settles_at`].
    pub transient_status: String,
    /// The status it reports from then on.
    pub settled_status: String,
    /// When the clock owes the move from one to the other. Stored, never slept
    /// on: a read materialises the transition the clock already made due.
    pub settles_at: SystemTime,
    /// When an accepted deletion removes this record, once
    /// `DeletePaymentConnector` has accepted one; `None` for a connector nobody
    /// has deleted. Stored, never slept on: the removal is the clock's,
    /// materialised by [`PaymentConnectorStore::expire_deleted`] on the next call
    /// that reads the payments plane ([`super::payment_connector_deletion`]).
    pub removal_due_at: Option<SystemTime>,
}

impl PaymentConnectorRecord {
    /// The record a create stores for one connector, under the id the create
    /// minted and the authorization address a service-orchestrated provisioning
    /// publishes for it.
    ///
    /// The statuses follow the provision mode: a connector carrying the caller's
    /// own credentials is `CREATING` with `READY` owed by the clock, while one
    /// whose credentials the service is to provision after OAuth consent is
    /// `PENDING_AUTHENTICATION` and stays there — the consent is an end user's
    /// act at the payment provider, so no clock brings it about.
    #[allow(clippy::too_many_arguments)]
    pub fn written(
        payment_connector_id: String,
        payment_manager_id: String,
        name: String,
        description: Option<String>,
        connector_type: String,
        provision_mode: String,
        credential_provider_configurations: Vec<CredentialsProviderConfiguration>,
        authorization_url: Option<String>,
        now: SystemTime,
    ) -> Self {
        let awaits_consent = provision_mode == PAYMENT_CONNECTOR_PROVISION_MODE_QUICK_CREATE;
        let (transient_status, settled_status, settles_at) = if awaits_consent {
            (
                PAYMENT_CONNECTOR_STATUS_PENDING_AUTHENTICATION.to_string(),
                PAYMENT_CONNECTOR_STATUS_PENDING_AUTHENTICATION.to_string(),
                now,
            )
        } else {
            (
                PAYMENT_CONNECTOR_STATUS_CREATING.to_string(),
                PAYMENT_CONNECTOR_STATUS_READY.to_string(),
                now + PAYMENT_CONNECTOR_SETTLE_DELAY,
            )
        };
        Self {
            payment_connector_id,
            payment_manager_id,
            name,
            description,
            connector_type,
            provision_mode,
            credential_provider_configurations,
            created_at: now,
            updated_at: now,
            authorization_url: authorization_url.filter(|_| awaits_consent),
            transient_status,
            settled_status,
            settles_at,
            removal_due_at: None,
        }
    }

    /// Whether every write already asked of this connector has taken effect by
    /// `now`, which is what decides whether a further write collides with one
    /// still in flight ([`payment_connector_status_conflict`]).
    ///
    /// A connector waiting for an end user's consent is settled: nothing is
    /// being written to it — the consent is an act at the payment provider — so
    /// it is reconfigurable, while one the clock still owes a `CREATING` or
    /// `UPDATING` transition is not.
    pub fn is_settled(&self, now: SystemTime) -> bool {
        now >= self.settles_at
    }

    /// Apply one update's PATCH to the connector: "This operation uses PATCH
    /// semantics, so you only need to specify the fields you want to change"
    /// (`smithy_query doc shape=UpdatePaymentConnector`) — a member the request
    /// carried replaces the stored one, and a member it omitted leaves the
    /// stored one alone.
    ///
    /// The write is not instantaneous: the connector moves to `UPDATING` and the
    /// clock owes it the status it was in before, exactly as a create's
    /// transition is owed. That status is the connector's own settled one rather
    /// than `READY`: a connector whose credentials the service provisions after
    /// OAuth consent goes back to waiting for that consent, which no update
    /// grants.
    pub fn apply_update(
        &mut self,
        description: Option<String>,
        connector_type: Option<String>,
        credential_provider_configurations: Option<Vec<CredentialsProviderConfiguration>>,
        now: SystemTime,
    ) {
        if description.is_some() {
            self.description = description;
        }
        if let Some(connector_type) = connector_type {
            self.connector_type = connector_type;
        }
        if let Some(configurations) = credential_provider_configurations {
            self.credential_provider_configurations = configurations;
        }
        self.updated_at = now;
        self.transient_status = PAYMENT_CONNECTOR_STATUS_UPDATING.to_string();
        self.settles_at = now + PAYMENT_CONNECTOR_SETTLE_DELAY;
    }

    /// The status at `now`: the transient one until the clock owes the
    /// transition, the settled one from then on. No read performs it — it is due
    /// at that instant whether or not anybody looks.
    ///
    /// A connector whose own deletion has been accepted reports `DELETING`
    /// whatever its create or update was doing: the resource is going away, which
    /// is the one thing a client polling it needs to see, exactly as the manager
    /// that holds it reports its own accepted deletion
    /// ([`PaymentManagerRecord::status_at`]).
    pub fn status_at(&self, now: SystemTime) -> &str {
        if self.is_deleting() {
            PAYMENT_CONNECTOR_STATUS_DELETING
        } else if now < self.settles_at {
            &self.transient_status
        } else {
            &self.settled_status
        }
    }

    /// The `authorizationUrl` a response reports: "This field is only present
    /// when the payment connector status is PENDING_AUTHENTICATION"
    /// (`smithy_query doc shape=CreatePaymentConnectorResponse
    /// member=authorizationUrl`), so the stored address is reported exactly while
    /// the connector is waiting at it.
    fn reported_authorization_url(&self, now: SystemTime) -> Option<String> {
        self.authorization_url
            .clone()
            .filter(|_| self.status_at(now) == PAYMENT_CONNECTOR_STATUS_PENDING_AUTHENTICATION)
    }

    /// This connector as `CreatePaymentConnector`'s response envelope: every
    /// member `CreatePaymentConnectorResponse` declares, read off the record just
    /// written. That response declares neither the description nor the provision
    /// mode, so both are reported by a read of the connector.
    pub fn create_output(&self, now: SystemTime) -> CreatePaymentConnectorOutput {
        CreatePaymentConnectorOutput {
            payment_connector_id: self.payment_connector_id.clone(),
            payment_manager_id: self.payment_manager_id.clone(),
            name: self.name.clone(),
            r#type: self.connector_type.clone(),
            credential_provider_configurations: self.credential_provider_configurations.clone(),
            created_at: timestamp(self.created_at),
            status: self.status_at(now).to_string(),
            authorization_url: self.reported_authorization_url(now),
        }
    }

    /// This connector as `UpdatePaymentConnector`'s response envelope: the
    /// members `UpdatePaymentConnectorResponse` declares, read off the record the
    /// update just wrote. That response declares the instant of that write where
    /// the create's declares the instant of the create, and neither the
    /// description nor the provision mode, so both are reported by a read of the
    /// connector.
    pub fn update_output(&self, now: SystemTime) -> UpdatePaymentConnectorOutput {
        let created = self.create_output(now);
        UpdatePaymentConnectorOutput {
            payment_connector_id: created.payment_connector_id,
            payment_manager_id: created.payment_manager_id,
            name: created.name,
            r#type: created.r#type,
            credential_provider_configurations: created.credential_provider_configurations,
            last_updated_at: timestamp(self.updated_at),
            status: created.status,
            authorization_url: created.authorization_url,
        }
    }

    /// This connector as `GetPaymentConnector`'s response envelope — the members
    /// [`Self::create_output`] renders, off the same record, plus the four
    /// `GetPaymentConnectorResponse` declares beyond it: the description the
    /// create persisted, how the connector was provisioned, the instant of the
    /// last write to it, and when its service-managed credentials last took
    /// effect.
    ///
    /// `credentialsUpdatedAt` is "first set when the credentials are provisioned
    /// and is updated by each rotation … present only for payment connectors with
    /// a provisionMode of QUICK_CREATE"
    /// (`smithy_query doc shape=GetPaymentConnectorResponse
    /// member=credentialsUpdatedAt`): a connector carrying the caller's own
    /// credentials has none, and one still waiting for consent has had none
    /// provisioned yet, so neither reports an instant.
    pub fn get_output(&self, now: SystemTime) -> GetPaymentConnectorOutput {
        let created = self.create_output(now);
        GetPaymentConnectorOutput {
            payment_connector_id: created.payment_connector_id,
            name: created.name,
            description: self.description.clone(),
            r#type: created.r#type,
            provision_mode: Some(self.provision_mode.clone()),
            credential_provider_configurations: created.credential_provider_configurations,
            created_at: created.created_at,
            last_updated_at: timestamp(self.updated_at),
            status: created.status,
            authorization_url: created.authorization_url,
            credentials_updated_at: None,
        }
    }
}

impl ComAmazonawsBedrockagentcorecontrolAmazonbedrockagentcorecontrolState {
    /// Mint a `paymentConnectorId` for `name` that the manager `payment_manager_id`
    /// holds no connector under: the name lower-cased + `-` + 10 lower-case
    /// alphanumerics, per `PaymentConnectorId`'s pattern
    /// `^([0-9a-z_][-]?){1,100}-[0-9a-z]{10}$`.
    ///
    /// `PaymentConnectorName` is `^[a-zA-Z][a-zA-Z0-9_]{0,47}$`, which admits
    /// upper case where the id's pattern does not, so the stem is the name
    /// lower-cased — the widest stem that pattern accepts for a given name, and
    /// the same derivation the parent manager's own id carries
    /// ([`Self::mint_payment_manager_id`]).
    pub fn mint_payment_connector_id(
        &mut self,
        payment_manager_id: &str,
        name: &str,
        at: SystemTime,
    ) -> String {
        let stem = name.to_lowercase();
        loop {
            self.mint_counter = self.mint_counter.wrapping_add(1);
            let candidate = format!("{stem}-{}", suffix_for(name, self.mint_counter, at));
            let taken = self
                .payment_managers
                .get(payment_manager_id)
                .is_some_and(|manager| manager.payment_connectors.contains(&candidate));
            if !taken {
                return candidate;
            }
        }
    }
}

/// The payment connectors of ONE payment manager: the record of each, and what
/// each `clientToken` presented to that manager produced.
///
/// One store rather than a map beside a map, so a connector and the token that
/// created it cannot drift apart, and so the manager's record carries one field
/// for the whole concern.
#[derive(Debug, Clone, Default)]
pub struct PaymentConnectorStore {
    /// Every connector of the manager, keyed by `paymentConnectorId` — the
    /// identifier every connector route addresses one by. The key order is the
    /// deterministic list order a connector listing pages over.
    connectors: BTreeMap<String, PaymentConnectorRecord>,
    /// `<operation>/<clientToken>` -> the connector that token first created on
    /// this manager, so a replayed create answers with its first result instead
    /// of minting a second connector.
    idempotency: BTreeMap<String, String>,
    /// The keys of [`Self::idempotency`] in the order they were first recorded,
    /// so the oldest record is the one evicted at the ceiling.
    idempotency_order: VecDeque<String>,
}

impl PaymentConnectorStore {
    /// The connector this manager holds under `payment_connector_id`, if it holds
    /// one.
    pub fn get(&self, payment_connector_id: &str) -> Option<&PaymentConnectorRecord> {
        self.connectors.get(payment_connector_id)
    }

    /// The connector this manager holds under `payment_connector_id`, for a
    /// write to it.
    pub fn get_mut(
        &mut self,
        payment_connector_id: &str,
    ) -> Option<&mut PaymentConnectorRecord> {
        self.connectors.get_mut(payment_connector_id)
    }

    /// Whether this manager already holds a connector under
    /// `payment_connector_id`.
    pub fn contains(&self, payment_connector_id: &str) -> bool {
        self.connectors.contains_key(payment_connector_id)
    }

    /// How many connectors this manager holds, which is what the per-manager
    /// ceiling is read against.
    pub fn len(&self) -> usize {
        self.connectors.len()
    }

    /// Whether this manager holds no connector at all.
    pub fn is_empty(&self) -> bool {
        self.connectors.is_empty()
    }

    /// Store one connector under the id its create minted for it.
    pub fn insert(&mut self, record: PaymentConnectorRecord) {
        self.connectors
            .insert(record.payment_connector_id.clone(), record);
    }

    /// Remove every connector of this manager whose accepted deletion the clock
    /// has already made due, with the idempotency records naming it.
    ///
    /// Lives with the store because the records, and the tokens naming them, are
    /// the store's own; what an accepted deletion MEANS, and when the removal is
    /// owed, is in [`super::payment_connector_deletion`]. A token record whose
    /// connector is gone can never replay anything again, so it goes with it,
    /// exactly as the manager plane drops its own.
    pub fn expire_deleted(&mut self, now: SystemTime) {
        self.connectors.retain(|_, record| !record.removal_due(now));
        let live = &self.connectors;
        self.idempotency
            .retain(|_, payment_connector_id| live.contains_key(payment_connector_id));
        let live = &self.idempotency;
        self.idempotency_order
            .retain(|token| live.contains_key(token));
    }

    /// The connector a previous request to `operation` carrying this
    /// `clientToken` created ON THIS MANAGER, if the record is still held and the
    /// connector still exists.
    ///
    /// Resolved against the manager the request addresses rather than against the
    /// whole account, because a connector is addressed only through its manager: a
    /// token presented to another manager has produced nothing there, so that
    /// request is served as a fresh one.
    pub fn replayed(
        &self,
        operation: &str,
        client_token: Option<&str>,
    ) -> Option<&PaymentConnectorRecord> {
        let payment_connector_id = self.idempotency.get(&token_key(operation, client_token?))?;
        self.connectors.get(payment_connector_id)
    }

    /// Record what a `clientToken` produced for `operation` on this manager,
    /// evicting the oldest record once [`MAX_REMEMBERED_CLIENT_TOKENS`] are held,
    /// for the same reason every other store of this service does.
    pub fn remember_client_token(
        &mut self,
        operation: &str,
        client_token: Option<String>,
        payment_connector_id: &str,
    ) {
        let Some(token) = client_token else {
            return;
        };
        remember_token(
            &mut self.idempotency,
            &mut self.idempotency_order,
            token_key(operation, &token),
            payment_connector_id.to_string(),
        );
    }
}

/// The path segment that names this service's payment-connector authorization
/// plane on the one endpoint this emulator serves its whole surface at, under the
/// reserved namespace every emulator-served plane is addressed below.
///
/// AWS orchestrates a `QUICK_CREATE` consent on hosts this emulator neither owns
/// nor answers on, so the address this service publishes is the address it
/// serves.
pub const PAYMENT_CONNECTOR_AUTHORIZATION_PLANE_SEGMENT: &str =
    "bedrock-agentcore-payment-connector-authorization";

/// The request path the authorization of one payment connector is served at: the
/// connector addressed as its own route addresses it, through the manager that
/// holds it.
///
/// A `PaymentManagerId` and a `PaymentConnectorId` are admitted only by
/// `^([0-9a-z_][-]?){1,100}-[0-9a-z]{10}$` and its sibling, so every character
/// either can carry is already a path character and neither needs escaping.
pub fn payment_connector_authorization_path(
    payment_manager_id: &str,
    payment_connector_id: &str,
) -> String {
    format!(
        "/{}/{PAYMENT_CONNECTOR_AUTHORIZATION_PLANE_SEGMENT}/{payment_manager_id}/\
         {payment_connector_id}",
        ::service_common::data_plane_addressing::EMULATOR_PATH_NAMESPACE
    )
}

/// Where an end user completes the OAuth consent a `QUICK_CREATE` payment
/// connector waits for: the endpoint THIS emulator is served at
/// (`endpoint_base`) followed by that connector's own path.
///
/// `authorizationUrl` is "The URL that the user must open to complete OAuth
/// consent" (`smithy_query doc shape=CreatePaymentConnectorResponse
/// member=authorizationUrl`) — an address a caller FOLLOWS, so the published one
/// is this emulator's own and `crate::service` serves exactly that path from the
/// stored connector, exactly as a consent portal's published page is
/// ([`super::consent_portal_url`]).
pub fn payment_connector_authorization_url(
    endpoint_base: &str,
    payment_manager_id: &str,
    payment_connector_id: &str,
) -> String {
    format!(
        "{}{}",
        endpoint_base.trim_end_matches('/'),
        payment_connector_authorization_path(payment_manager_id, payment_connector_id)
    )
}

/// The (payment manager, payment connector) a request `uri` addresses, for a path
/// of the form [`payment_connector_authorization_path`] mints, and `None` for
/// every other path.
///
/// The inverse of the minting above, so a URL this service published resolves
/// back to the connector behind it. A trailing `/` is the same address (that is
/// how a browser normalises a bare path), and nothing below the connector's own
/// segment is this route: the authorization is one page, not a path space.
pub fn payment_connector_authorization_route(uri: &str) -> Option<(&str, &str)> {
    use ::service_common::data_plane_addressing::{first_segment, EMULATOR_PATH_NAMESPACE};
    let path = super::consent_portals::request_target_path(uri);
    let (namespace, rest) = first_segment(path)?;
    if namespace != EMULATOR_PATH_NAMESPACE {
        return None;
    }
    let (plane, rest) = first_segment(rest)?;
    if plane != PAYMENT_CONNECTOR_AUTHORIZATION_PLANE_SEGMENT {
        return None;
    }
    let (payment_manager_id, rest) = first_segment(rest)?;
    let (payment_connector_id, below) = first_segment(rest)?;
    (!payment_manager_id.is_empty() && !payment_connector_id.is_empty() && below == "/")
        .then_some((payment_manager_id, payment_connector_id))
}

/// The media type a payment connector's authorization address is served as: a web
/// page, served from this emulator's own origin.
pub const PAYMENT_CONNECTOR_AUTHORIZATION_MEDIA_TYPE: &str = "text/html; charset=utf-8";

/// The page a fetch of a connector's authorization address renders: the connector
/// this address belongs to, as the store holds it right now — its name, its id,
/// the payment provider the consent is given at and the status it currently
/// reports.
///
/// Every value comes from the stored record, so the page an end user opens cannot
/// disagree with what `GetPaymentConnector` reports about the same connector. The
/// consent itself is given to the payment provider by a person, which this
/// emulator does not stand in for: the page states what the connector is waiting
/// for and grants nothing, so no credential is reported for a provisioning that
/// never ran. The name is a caller-supplied string, so it is written into the
/// document through the escaping of the `html-escape` crate rather than inlined.
pub fn payment_connector_authorization_page(
    record: &PaymentConnectorRecord,
    now: SystemTime,
) -> String {
    format!(
        "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
         <title>{title}</title>\n</head>\n<body>\n<h1>{name}</h1>\n\
         <dl>\n<dt>Payment connector</dt><dd>{id}</dd>\n\
         <dt>Payment manager</dt><dd>{manager}</dd>\n\
         <dt>Payment provider</dt><dd>{provider}</dd>\n\
         <dt>Status</dt><dd>{status}</dd>\n</dl>\n</body>\n</html>\n",
        title = escaped(&format!(
            "AgentCore payment connector authorization — {}",
            record.name
        )),
        name = escaped(&record.name),
        id = escaped(&record.payment_connector_id),
        manager = escaped(&record.payment_manager_id),
        provider = escaped(&record.connector_type),
        status = escaped(record.status_at(now)),
    )
}

/// The page a fetch of an authorization address this emulator holds no payment
/// connector for renders: a connector that was never created, one created on
/// another manager, or one that has been deleted.
pub fn payment_connector_authorization_not_found_page() -> String {
    "<!DOCTYPE html>\n<html lang=\"en\">\n<head>\n<meta charset=\"utf-8\">\n\
     <title>AgentCore payment connector authorization</title>\n</head>\n<body>\n\
     <h1>No such payment connector</h1>\n</body>\n</html>\n"
        .to_string()
}
