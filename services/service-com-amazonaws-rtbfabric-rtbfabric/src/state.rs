// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-rtbfabric-rtbfabric service.
//!
//! The gateway-pair + link lifecycle is the service's core loop: a requester
//! gateway and a responder gateway are created independently, and a `Link`
//! between two stored gateways is the primary resource. Everything an operation
//! answers is read back out of the records below — `activeLinksCount` /
//! `totalLinksCount` are DERIVED by scanning [`ComAmazonawsRtbfabricRtbfabricState::links`],
//! never stored.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

/// The `@idempotencyToken`s a stored resource's accepted changes were presented
/// under, which is what a retried update is answered from.
mod accepted_updates;
/// The `/responder-gateway/{gatewayId}/certificate` surface's record.
mod certificates;
/// The two external-link surfaces' records: the outbound links a requester gateway
/// opens and the inbound links a responder gateway takes.
mod external_links;
/// The one encoding this service's paginated reads state a page boundary in.
mod page_tokens;
/// The one page walk this service's paginated reads take over their records.
mod paging;
/// The routing rules a link carries.
mod routing_rules;
/// The `/tags/{resourceArn}` surface's one reading of a caller-supplied ARN and of
/// the record it addresses.
mod tagging;
// Named rather than glob, so this module's public surface stays explicit, and every
// path a handler uses (`crate::state::…`) keeps resolving.
pub use accepted_updates::{AcceptedUpdates, Retry, ACCEPTED_UPDATES_RETAINED};
pub use certificates::{
    CertificateAssociation, CERTIFICATE_ASSOCIATIONS_PER_GATEWAY_QUOTA,
    CERTIFICATE_EXTERNAL_ONLY_MESSAGE, CERTIFICATE_STATUS_ASSOCIATED,
    CERTIFICATE_STATUS_DISASSOCIATED, CERTIFICATE_STATUS_PENDING_ASSOCIATION,
    CERTIFICATE_STATUS_PENDING_DISASSOCIATION,
};
pub use external_links::{
    inbound_external_link_domain_name, InboundExternalLink, OutboundExternalLink,
    CONNECTIVITY_TYPE_EXTERNAL_INBOUND, CONNECTIVITY_TYPE_PUBLIC_EGRESS,
    INBOUND_EXTERNAL_LINK_EXTERNAL_ONLY_MESSAGE,
};
pub use page_tokens::{
    decode_next_token, encode_next_token, token_refusal, PageTokenKey, TokenRefusal, Walk,
    EXPIRED_TOKEN_CODE, EXPIRED_TOKEN_STATUS, MAX_TOKEN_LENGTH, TOKEN_LIFETIME,
};
pub use paging::{page_of, page_size, DEFAULT_PAGE_SIZE, MAX_PAGE_SIZE};
pub use routing_rules::{
    conditions_stated, priority_conflict_message, LinkRoutingRule, ROUTING_RULE_QUOTA,
    RULE_CONDITIONS_EMPTY_MESSAGE, RULE_STATUS_ACTIVE, RULE_STATUS_CREATION_IN_PROGRESS,
    RULE_STATUS_DELETED, RULE_STATUS_DELETION_IN_PROGRESS, RULE_STATUS_UPDATE_IN_PROGRESS,
};
pub use tagging::{
    created_tags_within_quota, resource_tags, resource_tags_mut, tag_count_within_quota,
    tagged_resource, tagging_unresolved, TaggedResource, TaggedResourceId, ARN_VENDOR,
    TAGGING_UNRESOLVED_CODE, TAGGING_UNRESOLVED_MESSAGE, TAGGING_UNRESOLVED_STATUS,
    TAGS_PER_RESOURCE_QUOTA,
};

use crate::ops::types::{
    LinkAttributes, LinkLogSettings, ListenerConfig, ManagedEndpointConfiguration,
    ModuleConfiguration, TrustStoreConfiguration,
};

/// How long after the creating call a resource this service reports in a
/// non-terminal status becomes settled.
///
/// AWS publishes no duration for RTB Fabric gateway or link provisioning, so
/// this span is the emulator's own choice: non-zero, so a caller acting straight
/// after the create still observes the transient status, and short enough that
/// nothing has to wait long for the settled one.
pub const PROVISIONING_DELAY: Duration = Duration::from_millis(500);

/// The region a request that carries no SigV4 credential scope is treated as
/// addressing: the Region a resource such a request creates belongs to, and the
/// one every DNS name this service assigns falls back to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How many requester gateways, and how many responder gateways, the store holds
/// before the matching create refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// AWS publishes adjustable per-account quotas on RTB Fabric gateways, and this
/// emulator models neither Service Quotas nor an account's adjustments, so this
/// is the emulator's OWN bound on the store rather than a claim about any
/// account's limit. It sits far above any workflow an emulator serves, and it is
/// what stops the state growing without end: each gateway delete releases that
/// gateway's record, its links, its routing rules, its certificate associations
/// and its client tokens
/// ([`ComAmazonawsRtbfabricRtbfabricState::release_requester_gateway`] and
/// [`ComAmazonawsRtbfabricRtbfabricState::release_responder_gateway`]).
pub const GATEWAY_QUOTA: usize = 1_000;

/// How many links the store holds before `CreateLink` refuses with the modelled
/// `ServiceQuotaExceededException` (HTTP 402).
///
/// The emulator's own bound, for the same reason as [`GATEWAY_QUOTA`].
/// `DeleteLink` releases a link, so this bounds what is live rather than what was
/// ever created.
pub const LINK_QUOTA: usize = 5_000;

/// How many modules one link's module flow holds before `UpdateLinkModuleFlow`
/// refuses with the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// AWS publishes no per-flow module count, so this bound is the emulator's own,
/// for the same reason as [`GATEWAY_QUOTA`]: it is what stops one link's stored
/// flow growing without end, while sitting far above any flow an emulator
/// serves. It is checked against the flow the call states, BEFORE that flow is
/// stored, so the store never holds more than this many modules for a link.
pub const FLOW_MODULES_PER_LINK_QUOTA: usize = 100;


/// How many `@idempotencyToken` records the store keeps before a create refuses
/// with the modelled `ServiceQuotaExceededException` (HTTP 402).
///
/// A record is written ONLY by a create that actually stored a resource whose id
/// this service minted — the two gateway creates, the two external-link creates
/// and the routing-rule create — so the map cannot hold more than those resources'
/// own quotas; the bound is checked anyway, so the retention rule survives a future
/// create that forgets it. Each record is fixed size — both the key and the
/// parameter fingerprint are 32-byte digests — so a `clientToken` or a request body
/// the model leaves unbounded in length cannot grow the bytes retained per record.
pub const IDEMPOTENCY_QUOTA: usize = 2 * GATEWAY_QUOTA + LINK_QUOTA + ROUTING_RULE_QUOTA;

/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: creation accepted,
/// the gateway is not usable yet.
pub const GATEWAY_STATUS_PENDING_CREATION: &str = "PENDING_CREATION";
/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: the gateway is usable.
pub const GATEWAY_STATUS_ACTIVE: &str = "ACTIVE";
/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: a configuration
/// change the gateway has taken is being applied to it while it is in service.
pub const GATEWAY_STATUS_PENDING_UPDATE: &str = "PENDING_UPDATE";
/// `RequesterGatewayStatus`/`ResponderGatewayStatus` member: terminal, the
/// gateway is gone.
pub const GATEWAY_STATUS_DELETED: &str = "DELETED";

/// The message this service's `ResourceNotFoundException` carries: fixed text
/// naming neither the resource asked for nor its id.
///
/// EVERY not-found path of this service answers with it, for every kind of
/// resource this store holds — a gateway of either kind, a link between two
/// gateways, a link with one end outside the fabric (the outbound external links
/// a requester gateway opens and the inbound external links a responder gateway
/// takes), a routing rule, a certificate association — and whichever call
/// addressed it: a read, a create resolving the gateway it is addressed to, an
/// update or a delete. So the refusal says the same thing whichever resource was
/// addressed, and no path echoes an id the caller supplied back at them. The rule
/// is the wording, not the list: a handler added for a resource this store does
/// not hold yet words its not-found the same way.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str = "The requested resource was not found.";

/// `LinkStatus` member: creation is accepted, the link is not carrying traffic
/// yet. A link to an endpoint OUTSIDE the fabric is not requested of a peer —
/// nothing accepts or rejects it — so this, not [`LINK_STATUS_PENDING_REQUEST`],
/// is where such a link starts.
pub const LINK_STATUS_PENDING_CREATION: &str = "PENDING_CREATION";
/// `LinkStatus` member: the link request is accepted and not yet put to the peer.
pub const LINK_STATUS_PENDING_REQUEST: &str = "PENDING_REQUEST";
/// `LinkStatus` member: the link request is outstanding against the peer.
pub const LINK_STATUS_REQUESTED: &str = "REQUESTED";
/// `LinkStatus` member: the peer has accepted the request and the link is
/// coming up.
pub const LINK_STATUS_ACCEPTED: &str = "ACCEPTED";
/// `LinkStatus` member: the link carries traffic.
pub const LINK_STATUS_ACTIVE: &str = "ACTIVE";
/// `LinkStatus` member: a configuration change the link has taken is being
/// applied to it while it carries traffic.
pub const LINK_STATUS_PENDING_UPDATE: &str = "PENDING_UPDATE";
/// `LinkStatus` member: terminal, the peer declined the link request.
pub const LINK_STATUS_REJECTED: &str = "REJECTED";
/// `LinkStatus` member: terminal, the link could not be established.
pub const LINK_STATUS_FAILED: &str = "FAILED";
/// `LinkStatus` member: terminal, the link is gone.
pub const LINK_STATUS_DELETED: &str = "DELETED";

/// Whether a link in `status` is up: carrying traffic, or carrying traffic while
/// a configuration change it has already taken is applied. One statement of what
/// "carrying traffic" means for every kind of link this store holds — a link
/// between two gateways ([`Link::is_up`]) and a link with one end outside the
/// fabric, whose record carries a status of the same `LinkStatus` enum — so
/// nothing has to repeat the status list to answer the same question.
pub fn link_status_is_up(status: &str) -> bool {
    status == LINK_STATUS_ACTIVE || status == LINK_STATUS_PENDING_UPDATE
}

/// `LinkDirection` member: the link was opened by a requester gateway.
pub const LINK_DIRECTION_REQUEST: &str = "REQUEST";
/// `LinkDirection` member: the link was opened by a responder gateway.
pub const LINK_DIRECTION_RESPONSE: &str = "RESPONSE";

/// `ConnectivityType` member: both ends are gateways in this account's VPCs.
pub const CONNECTIVITY_TYPE_DEFAULT: &str = "DEFAULT";

/// `GatewayType` member: the gateway accepts links from outside the fabric.
pub const GATEWAY_TYPE_EXTERNAL: &str = "EXTERNAL";

/// A requester gateway, as `CreateRequesterGateway` persisted it.
#[derive(Debug, Clone)]
pub struct RequesterGateway {
    pub gateway_id: String,
    /// The Region this gateway was created in, as the creating request's SigV4
    /// credential scope named it ([`request_region`]) — the one Region fact the
    /// record holds, so that whatever Region a later caller signs for, an ARN of
    /// this gateway resolves exactly when its Region field names THIS Region. See
    /// [`tagged_resource`].
    pub region: String,
    pub domain_name: String,
    pub status: String,
    /// The instant the clock makes this record's pending status — the one a
    /// create or an update left it in — `ACTIVE` due; `None` once the record has
    /// settled.
    pub settles_at: Option<SystemTime>,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Every `@idempotencyToken` an accepted change of this gateway's
    /// configuration was presented under, so a retry of any of them is answered
    /// from this record rather than applied to it again. Empty until one is
    /// accepted. See [`AcceptedUpdates`].
    pub accepted_updates: AcceptedUpdates,
}

impl RequesterGateway {
    /// Materialise the transition the clock has already made due — the created
    /// gateway coming up, or a configuration change it has taken reaching
    /// service — dated at the instant it was owed rather than now, so the record
    /// does not depend on when it was next read. Both transitions end at
    /// [`GATEWAY_STATUS_ACTIVE`]: a gateway that is in service is what either one
    /// is owed.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.settles_at.filter(|due| *due <= now) {
            self.status = GATEWAY_STATUS_ACTIVE.to_string();
            self.settles_at = None;
            self.updated_at = due;
        }
    }

    /// Whether this gateway is in service, so a configuration change can be
    /// applied to it: it is carrying traffic, or it is already applying one.
    /// Every other status reports the gateway moving through a lifecycle of its
    /// own, and has no configuration in service to change. Stated exactly as
    /// [`ResponderGateway::in_service`] states it for the other kind.
    pub fn in_service(&self) -> bool {
        self.status == GATEWAY_STATUS_ACTIVE || self.status == GATEWAY_STATUS_PENDING_UPDATE
    }

    /// The words a refusal carries when this gateway's current status does not
    /// admit the transition a call asks for: the gateway the call addressed and
    /// the status that refuses it, both from the record.
    pub fn status_conflict_message(&self, transition: &str) -> String {
        format!(
            "requester gateway {} is {} and cannot be {}",
            self.gateway_id, self.status, transition
        )
    }
}

/// A responder gateway, as `CreateResponderGateway` persisted it.
#[derive(Debug, Clone)]
pub struct ResponderGateway {
    pub gateway_id: String,
    /// The Region this gateway was created in: see [`RequesterGateway::region`].
    pub region: String,
    pub status: String,
    /// See [`RequesterGateway::settles_at`].
    pub settles_at: Option<SystemTime>,
    pub vpc_id: String,
    pub subnet_ids: Vec<String>,
    pub security_group_ids: Vec<String>,
    /// The domain name the caller named, if it named one. A gateway reached
    /// through a managed endpoint instead has none, so this is never synthesised.
    pub domain_name: Option<String>,
    pub port: i32,
    pub protocol: String,
    pub listener_config: Option<ListenerConfig>,
    pub trust_store_configuration: Option<TrustStoreConfiguration>,
    pub managed_endpoint_configuration: Option<ManagedEndpointConfiguration>,
    pub gateway_type: Option<String>,
    pub client_routing_policy: Option<String>,
    pub external_inbound_endpoint: Option<String>,
    pub description: Option<String>,
    pub tags: HashMap<String, String>,
    /// Every `@idempotencyToken` a change this gateway took was presented under —
    /// a change of its configuration, or a certificate association, neither of
    /// which mints an id of its own — exactly as
    /// [`RequesterGateway::accepted_updates`] holds the other kind's.
    pub accepted_updates: AcceptedUpdates,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
}

impl ResponderGateway {
    /// Materialise the transition the clock has already made due — the created
    /// gateway coming up, or a configuration change it has taken reaching
    /// service — dated at the instant it was owed rather than now, so the record
    /// does not depend on when it was next read. Both transitions end at
    /// [`GATEWAY_STATUS_ACTIVE`]: a gateway that is in service is what either one
    /// is owed.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.settles_at.filter(|due| *due <= now) {
            self.status = GATEWAY_STATUS_ACTIVE.to_string();
            self.settles_at = None;
            self.updated_at = due;
        }
    }

    /// Whether this gateway is in service, so a configuration change can be
    /// applied to it: it is carrying traffic, or it is already applying one.
    /// Every other status reports the gateway moving through a lifecycle of its
    /// own, and has no configuration in service to change.
    pub fn in_service(&self) -> bool {
        self.status == GATEWAY_STATUS_ACTIVE || self.status == GATEWAY_STATUS_PENDING_UPDATE
    }

    /// The words a refusal carries when this gateway's current status does not
    /// admit the transition a call asks for: the gateway the call addressed and
    /// the status that refuses it, both from the record.
    pub fn status_conflict_message(&self, transition: &str) -> String {
        format!(
            "responder gateway {} is {} and cannot be {}",
            self.gateway_id, self.status, transition
        )
    }

    /// Whether this gateway is `EXTERNAL` — the type that takes links from outside
    /// the fabric, and the only one a certificate can be associated with. A gateway
    /// created without a `gatewayType` is not external: the member is optional, and
    /// nothing is assumed for a caller that left it out.
    pub fn is_external(&self) -> bool {
        self.gateway_type.as_deref() == Some(GATEWAY_TYPE_EXTERNAL)
    }
}

/// A link between two stored gateways, as `CreateLink` persisted it.
///
/// Holds the members `CreateLink` takes but its response shape does not carry
/// (`tags`, `httpResponderAllowed`, `timeoutInMillis`): `GetLink` reads them
/// back from here, which is the whole point of storing the request rather than
/// echoing it.
#[derive(Debug, Clone)]
pub struct Link {
    pub link_id: String,
    pub gateway_id: String,
    pub peer_gateway_id: String,
    pub status: String,
    /// The instant the clock makes `PENDING_REQUEST` -> `REQUESTED` due; `None`
    /// once the record has settled.
    pub settles_at: Option<SystemTime>,
    pub direction: String,
    pub connectivity_type: String,
    pub attributes: Option<LinkAttributes>,
    pub log_settings: LinkLogSettings,
    pub http_responder_allowed: Option<bool>,
    pub timeout_in_millis: Option<i64>,
    /// The module flow in service on this link: what the flow this link runs
    /// holds, in the order the call that configured it stated the modules.
    pub flow_modules: Vec<ModuleConfiguration>,
    /// The module flow this link has taken and not yet put in service, `None`
    /// when no flow is awaiting it. It becomes [`flow_modules`](Self::flow_modules)
    /// at the instant the clock makes the link's way to `ACTIVE` due, so a flow
    /// reaches service exactly when the link carrying it does — which is why the
    /// change is held here rather than written straight over the flow in
    /// service. `Some(Vec::new())` is a flow of no modules, a change of its own:
    /// it is what clears the flow in service.
    pub pending_module_flow: Option<Vec<ModuleConfiguration>>,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub updated_at: SystemTime,
    /// Every `@idempotencyToken` an accepted change of this link's configuration
    /// was presented under, which is what a retried update is answered from.
    /// See [`AcceptedUpdates`].
    pub accepted_updates: AcceptedUpdates,
}

impl Link {
    /// The status the clock owes a link in `status`, if any: a request this
    /// service has taken but not yet put to the peer is owed `REQUESTED`, a
    /// request the peer has accepted is owed `ACTIVE` once the link is up, and
    /// a link applying a configuration change is owed `ACTIVE` again once that
    /// change is in service. Every other status is settled, so nothing is owed
    /// on it.
    fn owed_status(status: &str) -> Option<&'static str> {
        match status {
            LINK_STATUS_PENDING_REQUEST => Some(LINK_STATUS_REQUESTED),
            LINK_STATUS_ACCEPTED | LINK_STATUS_PENDING_UPDATE => Some(LINK_STATUS_ACTIVE),
            _ => None,
        }
    }

    /// Materialise the transition the clock has already made due — the request
    /// reaching the peer, or the accepted link coming up — dated at the instant
    /// it was owed rather than now, so the record does not depend on when it was
    /// next read. A link reaching service takes the module flow awaiting it into
    /// service with it, because a flow runs exactly when the link carrying it
    /// does.
    pub fn settle(&mut self, now: SystemTime) {
        if let Some(due) = self.settles_at.filter(|due| *due <= now) {
            if let Some(next) = Self::owed_status(&self.status) {
                self.status = next.to_string();
                self.updated_at = due;
                if next == LINK_STATUS_ACTIVE {
                    // The link is in service now, so the module flow awaiting it
                    // is the flow it runs, and nothing is awaiting it any more.
                    if let Some(taken) = self.pending_module_flow.take() {
                        self.flow_modules = taken;
                    }
                }
            }
            self.settles_at = None;
        }
    }

    /// The module flow this link has taken and not yet put in service, as a
    /// reader reports it: the modules awaiting the link, and no module at all
    /// when nothing is awaiting it.
    pub fn pending_flow_modules(&self) -> Vec<ModuleConfiguration> {
        self.pending_module_flow.clone().unwrap_or_default()
    }

    /// Whether `gateway_id` is one of this link's two ends — either end may read
    /// or delete it, so `GET /gateway/{gatewayId}/link/{linkId}` resolves from
    /// the requester's and the responder's id alike.
    pub fn has_end(&self, gateway_id: &str) -> bool {
        self.gateway_id == gateway_id || self.peer_gateway_id == gateway_id
    }

    /// This link's OTHER end, seen from `gateway_id`: a link is one record with
    /// two ends, so a response addressed to one of them reports the other as its
    /// `peerGatewayId`. An id that is neither end never reaches here, because the
    /// link is resolved by [`has_end`](Self::has_end) first.
    pub fn other_end(&self, gateway_id: &str) -> &str {
        if self.gateway_id == gateway_id {
            &self.peer_gateway_id
        } else {
            &self.gateway_id
        }
    }

    /// Whether the link request is still outstanding — accepted by this service
    /// but not yet answered by the peer. These are the only statuses from which
    /// the peer can still accept or decline the request.
    pub fn request_outstanding(&self) -> bool {
        self.status == LINK_STATUS_PENDING_REQUEST || self.status == LINK_STATUS_REQUESTED
    }

    /// Whether this link's request has already been accepted — the status an
    /// accept leaves, and the one the clock then moves it on to. A repeat of the
    /// accept asks for the state the link is already in, so it is answered with
    /// this record rather than refused.
    pub fn request_accepted(&self) -> bool {
        self.status == LINK_STATUS_ACCEPTED || self.status == LINK_STATUS_ACTIVE
    }

    /// Whether this link is up: carrying traffic, or carrying traffic while a
    /// configuration change it has already taken is applied. A further change
    /// to a link in one of these statuses is applied to a live link, which is
    /// what `PENDING_UPDATE` reports, so the clock owes it `ACTIVE` again.
    pub fn is_up(&self) -> bool {
        link_status_is_up(&self.status)
    }

    /// Whether an outstanding request on this link is waiting for `gateway_id`
    /// to answer it. A link is opened from one end and answered by the OTHER:
    /// the opener is [`Link::gateway_id`] — whichever kind of gateway it is — so
    /// the request is outstanding AGAINST the peer end, and never against the
    /// end that asked for it.
    pub fn awaits_answer_from(&self, gateway_id: &str) -> bool {
        self.request_outstanding() && self.peer_gateway_id == gateway_id
    }

    /// Whether `gateway_id` is the end this link was opened FROM — the end that
    /// asked for the link rather than the end the request was put to. The
    /// request is answered by the OTHER end, so neither answer to it — the
    /// acceptance that establishes the link nor the decline that ends it — is
    /// ever this end's to give.
    pub fn opened_from(&self, gateway_id: &str) -> bool {
        self.gateway_id == gateway_id
    }

    /// Whether this link has reached a status nothing moves it out of: the peer
    /// declined the request, the link could not be established, or it is gone.
    pub fn terminal(&self) -> bool {
        self.status == LINK_STATUS_REJECTED
            || self.status == LINK_STATUS_FAILED
            || self.status == LINK_STATUS_DELETED
    }

    /// The message this service's `ConflictException` carries when the link's
    /// current status does not admit the transition a call asks for: the link
    /// and the gateway the call addressed, from the request, and the status that
    /// refuses it, from the record.
    pub fn status_conflict_message(&self, gateway_id: &str, transition: &str) -> String {
        format!(
            "link {} on gateway {} is {} and cannot be {}",
            self.link_id, gateway_id, self.status, transition
        )
    }

    /// The message this service's `ConflictException` carries when the end the
    /// link was opened FROM asks to answer the request itself: an answer —
    /// `answer` is the past participle of the one the call asked for, the
    /// acceptance that establishes the link or the decline that ends it — is the
    /// answer of the end the request was put to, and the record names which end
    /// that is.
    pub fn opener_cannot_answer_message(&self, gateway_id: &str, answer: &str) -> String {
        format!(
            "link {} was requested by gateway {} and can be {} only by gateway {}",
            self.link_id, gateway_id, answer, self.peer_gateway_id
        )
    }
}

/// What the first call carrying an `@idempotencyToken` minted, so a retry with
/// the same token answers with the same resource instead of a second one.
///
/// Fixed size: see [`IDEMPOTENCY_QUOTA`] for the retention rule.
#[derive(Debug, Clone)]
pub struct IdempotentCreate {
    /// The id the first call minted.
    pub resource_id: String,
    /// A digest of that call's other parameters: the documented contract is that
    /// a retry of the same token with DIFFERENT parameters fails rather than
    /// silently answering the first call's resource.
    pub fingerprint: [u8; 32],
}

/// The number of links a gateway is an end of, by status. Every member is
/// derived by scanning [`ComAmazonawsRtbfabricRtbfabricState::links`].
#[derive(Debug, Default, Clone, Copy)]
pub struct LinkCounts {
    /// Every link this gateway is an end of.
    pub total: i32,
    /// Those carrying traffic, as [`link_status_is_up`] defines it.
    pub active: i32,
    /// Those whose request is still outstanding and waiting for THIS gateway to
    /// answer it, as [`Link::awaits_answer_from`] defines it.
    pub requested: i32,
}

/// Per-service state for com-amazonaws-rtbfabric-rtbfabric. A handler reaches it through [`service_state`].
///
/// Every map below is bounded: gateways by [`GATEWAY_QUOTA`], links by
/// [`LINK_QUOTA`], routing rules by [`ROUTING_RULE_QUOTA`], one gateway's
/// certificate associations by [`CERTIFICATE_ASSOCIATIONS_PER_GATEWAY_QUOTA`] and
/// the idempotency records by [`IDEMPOTENCY_QUOTA`], each checked by the call that
/// would insert, BEFORE it inserts. That is also what bounds the per-request work
/// the linear scans here do under the state lock.
#[derive(Debug, Default)]
pub struct ComAmazonawsRtbfabricRtbfabricState {
    /// Keyed by `gatewayId`. Bounded by [`GATEWAY_QUOTA`]; released by
    /// `DeleteRequesterGateway` through
    /// [`release_requester_gateway`](Self::release_requester_gateway).
    pub requester_gateways: HashMap<String, RequesterGateway>,
    /// Keyed by `gatewayId`. Bounded by [`GATEWAY_QUOTA`]; released by
    /// `DeleteResponderGateway` through
    /// [`release_responder_gateway`](Self::release_responder_gateway).
    pub responder_gateways: HashMap<String, ResponderGateway>,
    /// Keyed by (the `gatewayId` the link was opened from, `linkId`). Bounded by
    /// [`LINK_QUOTA`]; released by `DeleteLink`.
    pub links: HashMap<(String, String), Link>,
    /// Keyed by (the `gatewayId` the link was opened from, `linkId`). An outbound
    /// external link belongs to the requester gateway it was opened from and is
    /// addressed only through it. Counted together with [`links`](Self::links)
    /// against [`LINK_QUOTA`]; released with its gateway
    /// ([`release_dependents`](Self::release_dependents)).
    pub outbound_external_links: HashMap<(String, String), OutboundExternalLink>,
    /// Keyed by (the `gatewayId` the link was created on, `linkId`). An inbound
    /// external link belongs to the responder gateway it was created on and is
    /// addressed only through it. Counted together with [`links`](Self::links)
    /// against [`LINK_QUOTA`]; released with its gateway
    /// ([`release_dependents`](Self::release_dependents)).
    pub inbound_external_links: HashMap<(String, String), InboundExternalLink>,
    /// Keyed by (`linkId`, `ruleId`). Bounded by [`ROUTING_RULE_QUOTA`]; released
    /// with the link it routes for ([`release_link`](Self::release_link)), and so
    /// is the client-token record that minted it.
    pub routing_rules: HashMap<(String, String), LinkRoutingRule>,
    /// Keyed by (`gatewayId`, `acmCertificateArn`) — the pair that identifies one
    /// certificate association. Bounded by
    /// [`CERTIFICATE_ASSOCIATIONS_PER_GATEWAY_QUOTA`] per gateway, and so by that
    /// bound times [`GATEWAY_QUOTA`] in all.
    pub certificate_associations: HashMap<(String, String), CertificateAssociation>,
    /// Keyed by [`idempotency_key`], which folds the operation and the caller's
    /// `clientToken` into one 32-byte digest: a token is scoped to the operation
    /// it was presented to. Bounded by [`IDEMPOTENCY_QUOTA`], and written only by
    /// a create that stored a resource.
    pub idempotency: HashMap<[u8; 32], IdempotentCreate>,
    /// Monotonic source of the id suffixes this emulator instance mints, so no
    /// two gateways or links ever share an id.
    pub next_id: u64,
    /// The secret this store's `nextToken`s are authenticated under, drawn from the
    /// system's randomness when the store is created and never reported
    /// ([`PageTokenKey`], whose own `Debug` renders nothing of it). It is what lets a
    /// paginated read tell a token one of its walks ISSUED from one a caller
    /// assembled or another store minted.
    pub page_tokens: PageTokenKey,
}

impl ComAmazonawsRtbfabricRtbfabricState {
    /// The next id suffix, unique within this emulator instance.
    fn next_suffix(&mut self) -> String {
        self.next_id = self.next_id.saturating_add(1);
        format!("{:016x}", self.next_id)
    }

    /// A fresh `gatewayId`, matching the modelled `^rtb-gw-[a-z0-9-]{1,25}$`.
    pub fn mint_gateway_id(&mut self) -> String {
        format!("rtb-gw-{}", self.next_suffix())
    }

    /// A fresh `linkId`, matching the modelled `^link-[a-z0-9-]{1,25}$`.
    pub fn mint_link_id(&mut self) -> String {
        format!("link-{}", self.next_suffix())
    }

    /// A fresh `ruleId`, matching the modelled `^rule-[a-z0-9-]{1,25}$`.
    pub fn mint_rule_id(&mut self) -> String {
        format!("rule-{}", self.next_suffix())
    }

    /// Whether `gateway_id` names a stored gateway of either kind.
    pub fn gateway_exists(&self, gateway_id: &str) -> bool {
        self.requester_gateways.contains_key(gateway_id)
            || self.responder_gateways.contains_key(gateway_id)
    }

    /// Apply every status transition the clock has already made due.
    ///
    /// A reporting operation calls this before it answers: the change was owed
    /// at this same instant whether or not anybody asked, so materialising it
    /// here is not the read mutating the resource.
    pub fn settle_all(&mut self, now: SystemTime) {
        for gateway in self.requester_gateways.values_mut() {
            gateway.settle(now);
        }
        for gateway in self.responder_gateways.values_mut() {
            gateway.settle(now);
        }
        for link in self.links.values_mut() {
            link.settle(now);
        }
        for link in self.outbound_external_links.values_mut() {
            link.settle(now);
        }
        for link in self.inbound_external_links.values_mut() {
            link.settle(now);
        }
        for rule in self.routing_rules.values_mut() {
            rule.settle(now);
        }
        // A rule whose deletion the clock has now completed is gone, exactly as
        // the record a synchronous delete removes is: nothing reports a rule that
        // reached the terminal status, and the store does not grow with them. Its
        // client-token record goes with it, as on every other release path.
        let held_before = self.routing_rules.len();
        self.routing_rules
            .retain(|_, rule| rule.status != RULE_STATUS_DELETED);
        if self.routing_rules.len() != held_before {
            self.forget_released_client_tokens();
        }
        for association in self.certificate_associations.values_mut() {
            association.settle(now);
        }
    }

    /// How many links this store holds in all: those between two gateways and those
    /// to an endpoint outside the fabric. One bound covers both, because a link of
    /// either kind is a link the store holds ([`LINK_QUOTA`]).
    pub fn links_held(&self) -> usize {
        self.links
            .len()
            .saturating_add(self.outbound_external_links.len())
            .saturating_add(self.inbound_external_links.len())
    }

    /// The outbound external link `link_id` on the requester gateway `gateway_id`.
    ///
    /// Addressed only through the gateway it was opened from, so a link id that
    /// exists on ANOTHER gateway resolves to nothing here, exactly as one nothing
    /// minted does. The pair IS the key this map is keyed by, so the lookup is a
    /// hash of it rather than a scan.
    pub fn find_outbound_external_link(
        &self,
        gateway_id: &str,
        link_id: &str,
    ) -> Option<&OutboundExternalLink> {
        self.outbound_external_links
            .get(&(gateway_id.to_string(), link_id.to_string()))
    }

    /// The inbound external link `link_id` on the responder gateway `gateway_id`,
    /// resolved exactly as [`find_outbound_external_link`](Self::find_outbound_external_link)
    /// resolves the other direction's: a link with one end outside the fabric is
    /// addressed only through the gateway it was created on.
    pub fn find_inbound_external_link(
        &self,
        gateway_id: &str,
        link_id: &str,
    ) -> Option<&InboundExternalLink> {
        self.inbound_external_links
            .get(&(gateway_id.to_string(), link_id.to_string()))
    }

    /// [`find_outbound_external_link`](Self::find_outbound_external_link)'s link, for
    /// an operation that changes it.
    pub fn find_outbound_external_link_mut(
        &mut self,
        gateway_id: &str,
        link_id: &str,
    ) -> Option<&mut OutboundExternalLink> {
        self.outbound_external_links
            .get_mut(&(gateway_id.to_string(), link_id.to_string()))
    }

    /// [`find_inbound_external_link`](Self::find_inbound_external_link)'s link, for an
    /// operation that changes it.
    pub fn find_inbound_external_link_mut(
        &mut self,
        gateway_id: &str,
        link_id: &str,
    ) -> Option<&mut InboundExternalLink> {
        self.inbound_external_links
            .get_mut(&(gateway_id.to_string(), link_id.to_string()))
    }

    /// The link `link_id` between `gateway_id` and its peer, addressed from
    /// either end.
    pub fn find_link(&self, gateway_id: &str, link_id: &str) -> Option<&Link> {
        self.links
            .values()
            .find(|link| link.link_id == link_id && link.has_end(gateway_id))
    }

    /// [`find_link`](Self::find_link)'s link, for an operation that moves it to
    /// another status.
    pub fn find_link_mut(&mut self, gateway_id: &str, link_id: &str) -> Option<&mut Link> {
        self.links
            .values_mut()
            .find(|link| link.link_id == link_id && link.has_end(gateway_id))
    }

    /// The map key of [`find_link`](Self::find_link)'s link, for removing it.
    pub fn find_link_key(&self, gateway_id: &str, link_id: &str) -> Option<(String, String)> {
        self.links
            .iter()
            .find(|(_, link)| link.link_id == link_id && link.has_end(gateway_id))
            .map(|(key, _)| key.clone())
    }

    /// The routing rule `rule_id` on the link `link_id`, addressed from either of
    /// that link's two ends.
    ///
    /// A rule is only addressable through its own link, so a rule id that exists on
    /// ANOTHER link resolves to nothing here, exactly as one nothing minted does.
    pub fn find_routing_rule(
        &self,
        gateway_id: &str,
        link_id: &str,
        rule_id: &str,
    ) -> Option<&LinkRoutingRule> {
        self.find_link(gateway_id, link_id)?;
        self.routing_rules
            .get(&(link_id.to_string(), rule_id.to_string()))
    }

    /// [`find_routing_rule`](Self::find_routing_rule)'s rule, for an operation that
    /// changes it.
    pub fn find_routing_rule_mut(
        &mut self,
        gateway_id: &str,
        link_id: &str,
        rule_id: &str,
    ) -> Option<&mut LinkRoutingRule> {
        self.find_link(gateway_id, link_id)?;
        self.routing_rules
            .get_mut(&(link_id.to_string(), rule_id.to_string()))
    }

    /// Release the link `link_id` addressed from `gateway_id`: the record, every
    /// routing rule that routed for it, since a rule cannot outlive its link, and
    /// the client-token records those resources' creates wrote
    /// ([`forget_released_client_tokens`](Self::forget_released_client_tokens)).
    ///
    /// `None` when that gateway is no end of any link of that id, which is the
    /// not-found the caller is answered with.
    pub fn release_link(&mut self, gateway_id: &str, link_id: &str) -> Option<Link> {
        let key = self.find_link_key(gateway_id, link_id)?;
        let released = self.links.remove(&key)?;
        self.routing_rules
            .retain(|(rule_link_id, _), _| *rule_link_id != released.link_id);
        self.forget_released_client_tokens();
        Some(released)
    }

    /// Release the outbound external link `link_id` on the requester gateway
    /// `gateway_id`: the record, and with it the tags it carried and the
    /// client-token record its create wrote
    /// ([`forget_released_client_tokens`](Self::forget_released_client_tokens)).
    ///
    /// The pair IS this map's key, so the release addresses exactly the link
    /// [`find_outbound_external_link`](Self::find_outbound_external_link) reports: a
    /// link with one end outside the fabric is addressed only through the gateway it
    /// was opened from. No routing rule is held to it — a rule is created only on a
    /// link between two gateways — so the record is all there is to let go of.
    ///
    /// `None` when that pair names no stored link, which is the not-found the caller
    /// is answered with.
    pub fn release_outbound_external_link(
        &mut self,
        gateway_id: &str,
        link_id: &str,
    ) -> Option<OutboundExternalLink> {
        let released = self
            .outbound_external_links
            .remove(&(gateway_id.to_string(), link_id.to_string()))?;
        self.forget_released_client_tokens();
        Some(released)
    }

    /// Release the inbound external link `link_id` on the responder gateway
    /// `gateway_id`, exactly as
    /// [`release_outbound_external_link`](Self::release_outbound_external_link)
    /// releases the other direction's: the two directions are separate records of the
    /// same kind, and what a link with one end outside the fabric holds does not
    /// depend on which way its traffic flows.
    pub fn release_inbound_external_link(
        &mut self,
        gateway_id: &str,
        link_id: &str,
    ) -> Option<InboundExternalLink> {
        let released = self
            .inbound_external_links
            .remove(&(gateway_id.to_string(), link_id.to_string()))?;
        self.forget_released_client_tokens();
        Some(released)
    }

    /// Whether a LIVE link already joins these two gateways, in either
    /// orientation: a link is one record with two ends, readable and deletable
    /// from both, so the reverse create would be a second record for the same
    /// pair. A link that has reached a terminal status joins them no longer —
    /// nothing moves it back out of that status — so the pair is free for a
    /// fresh request, which is a link of its own beside the terminal record.
    pub fn link_joining(&self, gateway_id: &str, peer_gateway_id: &str) -> Option<&Link> {
        self.links.values().find(|link| {
            !link.terminal() && link.has_end(gateway_id) && link.has_end(peer_gateway_id)
        })
    }

    /// Count the stored links `gateway_id` is an end of, as that gateway sees
    /// them: every one of them, those carrying traffic, and those whose request
    /// is still waiting for THIS gateway to answer.
    ///
    /// A link with one end OUTSIDE the fabric is one of that gateway's links
    /// too — `RequesterGatewayStatus`'s reader documents `totalLinksCount` as
    /// "The total count of links for the requester gateway" — so it counts here
    /// exactly as a link between two gateways does, in either direction. It is
    /// never counted as REQUESTED: nothing is asked of a peer, so no request of
    /// it is outstanding.
    pub fn link_counts(&self, gateway_id: &str) -> LinkCounts {
        let mine = self.links.values().filter(|link| link.has_end(gateway_id));
        let mut counts = LinkCounts::default();
        for link in mine {
            counts.total = counts.total.saturating_add(1);
            if link.is_up() {
                counts.active = counts.active.saturating_add(1);
            }
            if link.awaits_answer_from(gateway_id) {
                counts.requested = counts.requested.saturating_add(1);
            }
        }
        let external = self
            .outbound_external_links
            .values()
            .map(|link| (link.gateway_id.as_str(), link.status.as_str()))
            .chain(
                self.inbound_external_links
                    .values()
                    .map(|link| (link.gateway_id.as_str(), link.status.as_str())),
            )
            .filter(|(link_gateway_id, _)| *link_gateway_id == gateway_id);
        for (_, status) in external {
            counts.total = counts.total.saturating_add(1);
            if link_status_is_up(status) {
                counts.active = counts.active.saturating_add(1);
            }
        }
        counts
    }

    /// Release the requester gateway `gateway_id`: everything the store holds
    /// BECAUSE of that gateway goes with it, so no later read can report a
    /// resource whose gateway is gone.
    ///
    /// `None` when no requester gateway of that id is stored, which is the
    /// not-found the caller is answered with. Otherwise the record removed, and
    /// with it everything [`release_dependents`](Self::release_dependents) holds to
    /// that gateway.
    pub fn release_requester_gateway(&mut self, gateway_id: &str) -> Option<RequesterGateway> {
        let released = self.requester_gateways.remove(gateway_id)?;
        self.release_dependents(gateway_id);
        Some(released)
    }

    /// Release the responder gateway `gateway_id`, exactly as
    /// [`release_requester_gateway`](Self::release_requester_gateway) releases the
    /// other kind: the two gateway stores are separate, and what hangs off a gateway
    /// does not depend on which kind it is.
    pub fn release_responder_gateway(&mut self, gateway_id: &str) -> Option<ResponderGateway> {
        let released = self.responder_gateways.remove(gateway_id)?;
        self.release_dependents(gateway_id);
        Some(released)
    }

    /// Release everything the store holds BECAUSE the gateway `gateway_id` was
    /// stored:
    ///
    /// * every link the gateway was an end of — a link is one record joining two
    ///   stored gateways and cannot outlive either of them — and every routing rule
    ///   that routed for one of those links;
    /// * every external link, of either direction, created on that gateway, which
    ///   likewise has one end in the gateway that is gone;
    /// * every certificate association on that gateway, which is a fact about a
    ///   gateway that no longer exists;
    /// * every client-token record that minted one of the resources released here —
    ///   the gateway itself, its external links and the routing rules of
    ///   its links — so the id a replayed token would answer with is never one the
    ///   store no longer holds.
    fn release_dependents(&mut self, gateway_id: &str) {
        let orphaned: HashSet<String> = self
            .links
            .values()
            .filter(|link| link.has_end(gateway_id))
            .map(|link| link.link_id.clone())
            .collect();
        self.links.retain(|_, link| !link.has_end(gateway_id));
        self.outbound_external_links
            .retain(|(link_gateway_id, _), _| link_gateway_id != gateway_id);
        self.inbound_external_links
            .retain(|(link_gateway_id, _), _| link_gateway_id != gateway_id);
        self.routing_rules
            .retain(|(rule_link_id, _), _| !orphaned.contains(rule_link_id.as_str()));
        self.certificate_associations
            .retain(|(association_gateway_id, _), _| association_gateway_id != gateway_id);
        self.forget_released_client_tokens();
    }

    /// Forget every client-token record whose resource the store no longer holds.
    ///
    /// A record exists to answer a replayed token with the resource the first call
    /// minted, so it is a fact ABOUT that resource: it is released together with
    /// it, exactly as a link's routing rules are. Keeping it would make a replay
    /// answer with — or refuse a fresh create over — an id the store no longer
    /// holds.
    ///
    /// Stated over what a record points at rather than over one release path, so
    /// every release honours it: the record survives exactly as long as a stored
    /// resource carries its id.
    fn forget_released_client_tokens(&mut self) {
        // Every id a stored resource of this service carries. A record is written
        // today by the two gateway creates, the two external-link creates and
        // the routing-rule create (see [`IDEMPOTENCY_QUOTA`]); the ids of links
        // between gateways are here for the rule rather than for a record that
        // exists, so a create that minted one under a token would be retained and
        // released by the same reading.
        let live: HashSet<&str> = self
            .requester_gateways
            .keys()
            .map(String::as_str)
            .chain(self.responder_gateways.keys().map(String::as_str))
            .chain(self.links.values().map(|link| link.link_id.as_str()))
            .chain(
                self.outbound_external_links
                    .values()
                    .map(|link| link.link_id.as_str()),
            )
            .chain(
                self.inbound_external_links
                    .values()
                    .map(|link| link.link_id.as_str()),
            )
            .chain(
                self.routing_rules
                    .values()
                    .map(|rule| rule.rule_id.as_str()),
            )
            .collect();
        self.idempotency
            .retain(|_, record| live.contains(record.resource_id.as_str()));
    }
}

/// The modelled timestamp for an instant held in state.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    ::aws_smithy_types::DateTime::from(at)
}

/// The digest an idempotency record is keyed by: the operation the token was
/// presented to, and the token itself. Hashing both keeps the retained key a
/// fixed 32 bytes however long a `clientToken` the caller chose — the model
/// bounds neither its length nor its content.
pub fn idempotency_key(operation: &str, client_token: &str) -> [u8; 32] {
    digest(&[operation.as_bytes(), client_token.as_bytes()])
}

/// The digest of a create's parameters, which an `@idempotencyToken` retry is
/// compared against. Fixed size, so a retained record never grows with the
/// request it came from.
pub fn parameter_fingerprint(rendering: &str) -> [u8; 32] {
    digest(&[rendering.as_bytes()])
}

/// SHA-256 over `parts`, each length-prefixed so two different splits of the
/// same bytes cannot collide.
fn digest(parts: &[&[u8]]) -> [u8; 32] {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    for part in parts {
        hasher.update((part.len() as u64).to_be_bytes());
        hasher.update(part);
    }
    hasher.finalize().into()
}

/// A tag map rendered canonically (sorted by key), so the same tags in a
/// different iteration order fingerprint identically while two DIFFERENT maps
/// never render alike.
///
/// Each key and each value is written with its own byte length in front of it,
/// which is what [`digest`] does to the parts it hashes and for the same reason:
/// a modelled tag value carries no pattern at all and a tag key admits `=`, so
/// any separator this rendering could choose is a character the tags themselves
/// may contain. With the lengths in the rendering, where one pair ends and the
/// next begins is read off it rather than guessed, so `{"a": "b,c=d"}` and
/// `{"a": "b", "c": "d"}` are the two distinct maps they are.
///
/// A map stating no pairs renders as the empty string, the same as no tags at
/// all: a create holds the tags it was given and neither form gives it any.
///
/// A fingerprint is only ever compared with one computed by the same build in
/// the same process, so the rest of a request can be rendered with `Debug`,
/// which quotes and escapes every string it writes; a `HashMap` cannot, because
/// its iteration order varies between two calls that carry identical tags.
pub fn canonical_tags(tags: Option<&HashMap<String, String>>) -> String {
    use std::fmt::Write as _;

    let mut pairs: Vec<(&str, &str)> = tags
        .map(|tags| {
            tags.iter()
                .map(|(key, value)| (key.as_str(), value.as_str()))
                .collect()
        })
        .unwrap_or_default();
    pairs.sort_unstable();
    let mut rendered = String::new();
    for (key, value) in &pairs {
        let _ = write!(rendered, "{}:{key}{}:{value}", key.len(), value.len());
    }
    rendered
}

/// One DNS label, from arbitrary text: lowercase, only `[a-z0-9-]`, and never
/// leading or trailing `-`, which is what the modelled `DomainName` pattern
/// accepts for each dot-separated label.
fn dns_label(text: &str) -> Option<String> {
    let kept: String = text
        .to_ascii_lowercase()
        .chars()
        .filter(|c| c.is_ascii_alphanumeric() || *c == '-')
        .collect();
    let trimmed = kept.trim_matches('-');
    (!trimmed.is_empty()).then(|| trimmed.to_string())
}

/// The Region the caller addressed, VERBATIM from the request's SigV4 credential
/// scope; [`DEFAULT_REGION`] when the request carries no scope (an anonymous call)
/// or names an empty one.
///
/// Verbatim because this is the Region a created resource BELONGS to, and a Region
/// is written into an ARN's Region field, whose grammar (`[a-zA-Z0-9_-]+`, the
/// modelled `RtbTaggableResourceArn` pattern) is wider than a DNS label's: a Region
/// reduced to a DNS label here would be one no ARN of that resource could spell.
/// The DNS names built from a Region reduce it themselves ([`region_dns_label`]),
/// which is the only place the narrower grammar applies.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `region` as one label of a DNS name: the modelled `DomainName` pattern admits
/// only `[a-z0-9-]` per label, while a Region may carry more, so a name built from
/// a Region reduces it ([`dns_label`]) and falls back to [`DEFAULT_REGION`] — itself
/// a label — when nothing of it survives.
fn region_dns_label(region: &str) -> String {
    dns_label(region).unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The DNS name this service assigns a gateway: the gateway's own id as the
/// leftmost label under the service's regional zone.
pub fn gateway_domain_name(gateway_id: &str, region: &str) -> String {
    format!(
        "{gateway_id}.rtbfabric.{}.amazonaws.com",
        region_dns_label(region)
    )
}

/// The public name an `EXTERNAL` responder gateway accepts inbound external
/// links on. An `INTERNAL` gateway has none.
pub fn external_inbound_endpoint(gateway_id: &str, region: &str) -> String {
    format!(
        "{gateway_id}.inbound.rtbfabric.{}.amazonaws.com",
        region_dns_label(region)
    )
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRtbfabricRtbfabricState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRtbfabricRtbfabricState>)
}
