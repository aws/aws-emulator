// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig service.
//!
//! Holds the cluster -> control panel -> routing control hierarchy, and the safety rules
//! a control panel scopes: every resource a
//! create call minted, keyed by the ARN that call handed the client. A handler reaches
//! it through [`service_state`] and answers from what is stored here — a describe or a
//! list never recomputes a body from its own request.
//!
//! Three values are DERIVED rather than stored, so no read can see them stale:
//!
//! * `Status` — computed from the instant the create call made the deployment due
//!   ([`deploy_due`]), so a resource reports `PENDING` until then and `DEPLOYED`
//!   afterwards without any operation writing it.
//! * `ControlPanel$RoutingControlCount` — counted from [`routing_controls`] on every
//!   projection ([`Store::routing_control_count`]).
//! * `Cluster$ClusterEndpoints` — the data-plane addresses a client DIALS to get or set a
//!   routing control state, so they are computed from the base URL this emulator is
//!   reachable at, as the asking request's own context reports it
//!   ([`cluster_endpoints`]). Stored once at create time they would name wherever the
//!   emulator was reachable then, which is not where a later caller has to send its
//!   request. Their ORDER is the cluster's own — [`ClusterRecord::home_region`] leads, see
//!   [`cluster_endpoint_regions`] — so every read publishes the same order.
//!
//! The refusals every handler shares are worded here as well, so two operations cannot
//! say the same thing differently: the sentences a lookup, a reference and a dependent
//! refusal state, and [`read_resource_arn`], the gate every caller-supplied ARN passes
//! before it is resolved — its FORM ([`read_arn_format`]), then whether the caller may act
//! on the resource it names ([`authorize_resource_arn`]), both decided from the request
//! alone.
//!
//! [`routing_controls`]: Store::routing_controls

/// The Amazon Route 53 health checks that FOLLOW a routing control stored here. They are
/// resources of Route 53, so they are read from that service's own store rather than held
/// beside the records above.
pub mod associated_health_checks;

use std::collections::{BTreeMap, VecDeque};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use crate::ops::types::{
    AccessDeniedException, AssertionRule, Cluster, ClusterEndpoint, ControlPanel, GatingRule,
    RoutingControl, Rule, RuleConfig, ValidationException,
};

/// The ARN service segment Route 53 Application Recovery Controller's recovery-control
/// resources carry: AWS documents them as
/// `arn:aws:route53-recovery-control::<account>:cluster/<id>` (the model's
/// `arnNamespace`/`endpointPrefix`, `route53-recovery-control-config`, names the API
/// endpoint, not the resource namespace).
pub const ARN_SERVICE: &str = "route53-recovery-control";

/// Resource ARNs in this service are account-wide, not Regional — AWS writes them with
/// an empty Region segment (`arn:aws:route53-recovery-control::<account>:...`).
pub const ARN_REGION: &str = "";

/// The `Status` value a resource reports before its deployment is due.
pub const STATUS_PENDING: &str = "PENDING";

/// The `Status` value a resource reports once its deployment is due.
pub const STATUS_DEPLOYED: &str = "DEPLOYED";

/// The name AWS gives the control panel it creates together with a cluster.
pub const DEFAULT_CONTROL_PANEL_NAME: &str = "DefaultControlPanel";

/// How long after the create call a resource's `PENDING` -> `DEPLOYED` transition
/// becomes due.
///
/// AWS returns these resources `PENDING` and deploys them asynchronously, so the span
/// is this emulator's own choice; it is deliberately non-zero so a caller that reads
/// straight back still observes the transient status.
const DEPLOY_DELAY: Duration = Duration::from_millis(500);

/// The page size a list operation serves when the request names no `MaxResults`.
///
/// `ListRoutingControls$MaxResults` is modelled `range 1..1000`, so "unspecified" is
/// served as the whole modelled maximum.
pub const DEFAULT_PAGE_SIZE: usize = 1000;

/// The most clusters this store holds at once.
///
/// The model declares `ServiceQuotaExceededException` ("You attempted to create more
/// resources than the service allows based on service quotas") but states no number, so
/// the figure is CHOSEN: it exists to bound caller-driven growth of the store, not to
/// reproduce an AWS quota.
pub const MAX_CLUSTERS: usize = 100;

/// The most control panels this store holds at once — CHOSEN, for the same reason as
/// [`MAX_CLUSTERS`]. One of every cluster's panels is the default panel it was created
/// with, so this bounds the caller-created ones too.
pub const MAX_CONTROL_PANELS: usize = 1_000;

/// The most routing controls this store holds at once — CHOSEN, for the same reason as
/// [`MAX_CLUSTERS`].
pub const MAX_ROUTING_CONTROLS: usize = 10_000;

/// The most safety rules this store holds at once — CHOSEN, for the same reason as
/// [`MAX_CLUSTERS`].
pub const MAX_SAFETY_RULES: usize = 10_000;

/// The most `ClientToken` records the idempotency ledger remembers at once — CHOSEN,
/// for the same reason: a caller must not be able to grow the ledger without limit.
///
/// A forgotten token simply has no memory left: a replayed create then performs the
/// create again instead of returning the first resource.
pub const MAX_IDEMPOTENCY_TOKENS: usize = 10_000;

/// The Regions Route 53 ARC publishes a cluster data-plane endpoint in — "an array of
/// the five cluster endpoints (one for each supported Amazon Web Services Region)"
/// (`CreateCluster`'s documentation, via
/// `smithy_query(query="doc",shape="CreateCluster")`).
pub const CLUSTER_ENDPOINT_REGIONS: &[&str] = &[
    "ap-northeast-1",
    "ap-southeast-2",
    "eu-west-1",
    "us-east-1",
    "us-west-2",
];

/// The Region a request carrying no SigV4 credential scope is attributed to: the one
/// Region AWS serves this control plane from, so a cluster created by an unsigned request
/// is the cluster an AWS client creates when it names no Region of its own.
const DEFAULT_REGION: &str = "us-west-2";

/// The Region this request was addressed to, read off its SigV4 credential scope;
/// [`DEFAULT_REGION`] for a request that carries no scope.
pub fn request_region(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| !region.is_empty())
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The Region labels a cluster whose own Region is `home_region` publishes its endpoints
/// under, in the order it publishes them: the cluster's OWN Region first, then the
/// remaining supported Regions in [`CLUSTER_ENDPOINT_REGIONS`] order.
///
/// The published SET is the supported Regions, one endpoint each; the ORDER is the
/// cluster's. A cluster leads with the entry for the Region it itself lives in and is
/// addressed on, so a client that dials the first published entry — the common shortcut,
/// since this data plane has no generic Regional endpoint to fall back on — reaches the
/// endpoint of its own Region rather than one picked independently of the cluster.
///
/// A `home_region` the supported set does not name publishes no endpoint of its own, so
/// there is nothing for it to lead with and the supported Regions keep their own order.
/// Matching is case-insensitive, as a Region label read off a credential scope is.
pub fn cluster_endpoint_regions(home_region: &str) -> Vec<&'static str> {
    let mut regions: Vec<&'static str> = CLUSTER_ENDPOINT_REGIONS.to_vec();
    if let Some(at) = regions
        .iter()
        .position(|region| region.eq_ignore_ascii_case(home_region))
    {
        let own = regions.remove(at);
        regions.insert(0, own);
    }
    regions
}

/// The instant a create call performed now makes the resource's deployment due.
pub fn deploy_due() -> Instant {
    Instant::now() + DEPLOY_DELAY
}

/// The `Status` a resource whose deployment is due at `due` reports right now.
///
/// Purely a function of the clock, so reporting it writes nothing.
pub fn status_at(due: Instant) -> &'static str {
    if Instant::now() >= due {
        STATUS_DEPLOYED
    } else {
        STATUS_PENDING
    }
}

/// The Regional data-plane endpoints a cluster publishes, one per label in
/// [`CLUSTER_ENDPOINT_REGIONS`], for an emulator reachable at `endpoint_base`, in the
/// order a cluster whose own Region is `home_region` publishes them
/// ([`cluster_endpoint_regions`]).
///
/// `ClusterEndpoint$Endpoint` is not a label a client reads: it is the address the client
/// then SENDS its `GetRoutingControlState` / `UpdateRoutingControlState(s)` /
/// `ListRoutingControls` call to ("Specify an endpoint and Amazon Web Services Region when
/// you want to set or retrieve a routing control state in the cluster"), and the cluster
/// API has no generic regional endpoint to fall back on. So what is published is the one
/// address this emulator answers on — `endpoint_base`, the base URL
/// [`::smithy_server_runtime::OperationContext::endpoint`] reports ("the base URL a later
/// request addresses… Callers send normally signed requests to it") — and never a host
/// beneath AWS's own domain, which nothing here serves and which would send the call to
/// the real service.
///
/// The model names no hostname of its own, and the cluster data plane this emulator serves
/// is served at that one base for every Region label, so the five entries differ in
/// `Region` alone: AWS documents a cluster as "a set of five redundant Regional endpoints"
/// against which the same routing controls are read and written, and here the one endpoint
/// is redundantly reachable under each label.
///
/// `NetworkType` takes no part in it either. It says which IP families the published
/// endpoints answer on (`IPV4` / `DUALSTACK`), which on AWS is a difference of hostname;
/// this emulator answers on exactly the address it is reachable at, whatever family that
/// address belongs to, so there is no second host to name.
///
/// The member's `@pattern ^[A-Za-z0-9:.\/_-]*$` describes the DNS names AWS itself vends
/// and admits no brackets, so an emulator bound to an IPv6 address cannot be named inside
/// it: RFC 3986 requires the brackets in an authority, and an address a client has to DIAL
/// is published as the client must dial it rather than trimmed into the pattern and left
/// unreachable. Every other base — a name, an IPv4 address, with or without a port — is
/// inside both the pattern and the `@length 1..128`.
pub fn cluster_endpoints(endpoint_base: &str, home_region: &str) -> Vec<(String, String)> {
    let endpoint = endpoint_base.trim_end_matches('/');
    cluster_endpoint_regions(home_region)
        .into_iter()
        .map(|region| (region.to_string(), endpoint.to_string()))
        .collect()
}

/// One stored cluster.
#[derive(Debug, Clone)]
pub struct ClusterRecord {
    /// Creation order, so `ListClusters`-style ordering is stable: the minted id is
    /// opaque and carries no order.
    pub seq: u64,
    pub arn: String,
    pub name: String,
    /// The account that owns the cluster — this emulator's account id.
    pub owner: String,
    pub network_type: String,
    /// The Region this cluster lives in: the one its create call was addressed to
    /// ([`request_region`]). The cluster publishes its own Region's data-plane endpoint
    /// first ([`cluster_endpoint_regions`]), and that order belongs to the cluster rather
    /// than to the read, so every later projection publishes the same one.
    pub home_region: String,
    pub tags: BTreeMap<String, String>,
    /// The control panel AWS creates together with the cluster, and the one
    /// `CreateRoutingControl` attaches to when the request names none.
    pub default_control_panel_arn: String,
    /// When this cluster's `PENDING` -> `DEPLOYED` transition became due.
    pub deploy_due: Instant,
}

impl ClusterRecord {
    /// Move the cluster onto `network_type`, making the deployment of its data-plane
    /// endpoints due again — so a caller cannot write one half of the transition without
    /// the other.
    ///
    /// A request naming the network the cluster is already on writes NOTHING: there is no
    /// deployment to redo. The endpoints themselves are not written either way: they are
    /// projected from the address this emulator is reachable at ([`cluster_endpoints`]),
    /// which the network type does not decide here.
    ///
    /// Answers whether the record changed, so a caller can tell a redeploy from a
    /// no-op.
    pub fn set_network_type(&mut self, network_type: String) -> bool {
        if self.network_type == network_type {
            return false;
        }
        self.network_type = network_type;
        self.deploy_due = deploy_due();
        true
    }

    /// This record as the modelled `Cluster` shape — the single projection create,
    /// describe, list and update all answer with.
    ///
    /// `endpoint_base` is the base URL this emulator is reachable at
    /// (`smithy_server_runtime::OperationContext::endpoint`), from which the cluster's
    /// published data-plane endpoints are derived ([`cluster_endpoints`]): the address a
    /// client has to send a routing-control state call to is where this emulator answers,
    /// not where it answered when the cluster was created.
    pub fn to_cluster(&self, endpoint_base: &str) -> Cluster {
        Cluster {
            cluster_arn: Some(self.arn.clone()),
            cluster_endpoints: Some(
                cluster_endpoints(endpoint_base, &self.home_region)
                    .into_iter()
                    .map(|(region, endpoint)| ClusterEndpoint {
                        endpoint: Some(endpoint),
                        region: Some(region),
                    })
                    .collect(),
            ),
            name: Some(self.name.clone()),
            status: Some(status_at(self.deploy_due).to_string()),
            owner: Some(self.owner.clone()),
            network_type: Some(self.network_type.clone()),
        }
    }
}

/// One stored control panel.
#[derive(Debug, Clone)]
pub struct ControlPanelRecord {
    /// Creation order within the service, for a stable list page order.
    pub seq: u64,
    pub arn: String,
    pub cluster_arn: String,
    pub name: String,
    pub owner: String,
    /// True for the panel created together with the cluster.
    pub default_control_panel: bool,
    pub tags: BTreeMap<String, String>,
    pub deploy_due: Instant,
}

impl ControlPanelRecord {
    /// Move the control panel onto `name` — the only thing an update changes — making the
    /// deployment of the renamed panel due again.
    ///
    /// A request naming the name the panel already carries writes NOTHING, so it does not
    /// restart a deployment either.
    pub fn set_name(&mut self, name: String) {
        if self.name == name {
            return;
        }
        self.name = name;
        self.deploy_due = deploy_due();
    }

    /// This record as the modelled `ControlPanel` shape.
    ///
    /// `routing_control_count` is passed in because it is counted from the routing
    /// controls stored under this panel, never kept on the record.
    pub fn to_control_panel(&self, routing_control_count: i32) -> ControlPanel {
        ControlPanel {
            cluster_arn: Some(self.cluster_arn.clone()),
            control_panel_arn: Some(self.arn.clone()),
            default_control_panel: Some(self.default_control_panel),
            name: Some(self.name.clone()),
            routing_control_count: Some(routing_control_count),
            status: Some(status_at(self.deploy_due).to_string()),
            owner: Some(self.owner.clone()),
            // The wire publishes a control panel's ARN under the generic member name as
            // well as the typed one, holding the same value — the one stored ARN fills
            // both, so the two can never disagree.
            arn: Some(self.arn.clone()),
        }
    }
}

/// One stored routing control.
#[derive(Debug, Clone)]
pub struct RoutingControlRecord {
    /// Creation order within the service, which is the order `ListRoutingControls`
    /// pages through.
    pub seq: u64,
    pub arn: String,
    pub control_panel_arn: String,
    pub cluster_arn: String,
    pub name: String,
    pub owner: String,
    /// The tags the tagging operations hold on this routing control. `CreateRoutingControl`
    /// models no `Tags` member, so a created one starts untagged.
    pub tags: BTreeMap<String, String>,
    pub deploy_due: Instant,
    /// The sequence number of the Route 53 health check state that backs this routing
    /// control, which the wire reports beside the control's modelled members.
    ///
    /// A control's own number, starting at [`FIRST_HEALTH_CHECK_SEQUENCE_NUMBER`] when it
    /// is created and advanced by [`Self::set_name`] whenever the control's configuration
    /// is deployed again — the only change a client of this API can make to it. The model
    /// declares no such member, so what a caller may rely on is that it is there, that it
    /// is a number, and that reading the control twice reports the same one; the span
    /// between two numbers is this emulator's own choice.
    pub health_check_sequence_number: i64,
}

/// The sequence number a routing control's backing health check starts at.
pub const FIRST_HEALTH_CHECK_SEQUENCE_NUMBER: i64 = 1;

impl RoutingControlRecord {
    /// Move the routing control onto `name` — the only thing an update changes — making
    /// the deployment of the changed configuration due again.
    ///
    /// A request naming the name the control already carries writes NOTHING, so it does
    /// not restart a deployment either.
    pub fn set_name(&mut self, name: String) {
        if self.name == name {
            return;
        }
        self.name = name;
        self.deploy_due = deploy_due();
        // Deploying the changed configuration advances the backing health check's
        // sequence number with it, so a client ordering health-check updates off that
        // number sees the new configuration as the later one.
        self.health_check_sequence_number = self.health_check_sequence_number.saturating_add(1);
    }

    /// This record as the `RoutingControl` shape the wire carries: the modelled members
    /// plus the health-check sequence number the endpoint reports beside them.
    pub fn to_routing_control(&self) -> RoutingControl {
        RoutingControl {
            control_panel_arn: Some(self.control_panel_arn.clone()),
            name: Some(self.name.clone()),
            routing_control_arn: Some(self.arn.clone()),
            status: Some(status_at(self.deploy_due).to_string()),
            owner: Some(self.owner.clone()),
            route53_health_check_sequence_number: Some(self.health_check_sequence_number),
        }
    }
}

/// The two kinds of safety rule, with the routing controls each kind evaluates.
///
/// A safety rule is one of them and never both: an assertion rule asserts its
/// `RuleConfig` over the controls a transaction changes, a gating rule evaluates its
/// `RuleConfig` over gating controls to decide whether its target controls may change at
/// all. Storing the controls inside the kind is what keeps a rule from holding the
/// members of the other kind.
#[derive(Debug, Clone)]
pub enum SafetyRuleControls {
    /// An assertion rule's `AssertedControls`.
    Assertion { asserted_controls: Vec<String> },
    /// A gating rule's `GatingControls` and the `TargetControls` they gate.
    Gating {
        gating_controls: Vec<String>,
        target_controls: Vec<String>,
    },
}

impl SafetyRuleControls {
    /// Every routing control the rule references, in the order its members declare them.
    pub fn referenced(&self) -> impl Iterator<Item = &str> {
        let (first, second) = match self {
            Self::Assertion { asserted_controls } => (asserted_controls, None),
            Self::Gating {
                gating_controls,
                target_controls,
            } => (gating_controls, Some(target_controls)),
        };
        first
            .iter()
            .chain(second.into_iter().flatten())
            .map(String::as_str)
    }

    /// Whether the rule references `routing_control_arn` — as a control it evaluates or as
    /// one it gates.
    pub fn references(&self, routing_control_arn: &str) -> bool {
        self.referenced()
            .any(|referenced| referenced == routing_control_arn)
    }

    /// This rule's kind and controls as a deterministic string, for an idempotency
    /// fingerprint.
    ///
    /// Each member is labelled and joined on its own, so two requests that differ only in
    /// how the same controls are split between a gating rule's two lists fingerprint
    /// differently.
    pub fn fingerprint(&self) -> String {
        match self {
            Self::Assertion { asserted_controls } => {
                format!("{ASSERTION_RULE_KIND}|asserted={}", asserted_controls.join(","))
            }
            Self::Gating {
                gating_controls,
                target_controls,
            } => format!(
                "{GATING_RULE_KIND}|gating={}|target={}",
                gating_controls.join(","),
                target_controls.join(",")
            ),
        }
    }

    /// The kind of rule this is, as the `Rule` projection and an update's refusal name
    /// it.
    pub fn kind(&self) -> &'static str {
        match self {
            Self::Assertion { .. } => ASSERTION_RULE_KIND,
            Self::Gating { .. } => GATING_RULE_KIND,
        }
    }
}

/// The name the assertion kind of safety rule is known by, which is also the member
/// `Rule` reports one under (`smithy_query(query="shape",shape="Rule")`).
pub const ASSERTION_RULE_KIND: &str = "ASSERTION";

/// The name the gating kind of safety rule is known by, and the member `Rule` reports one
/// under.
pub const GATING_RULE_KIND: &str = "GATING";

/// One stored safety rule, of either kind.
#[derive(Debug, Clone)]
pub struct SafetyRuleRecord {
    /// Creation order within the service, which is the order `ListSafetyRules` pages
    /// through.
    pub seq: u64,
    pub arn: String,
    /// The control panel the rule is scoped to: a safety rule guards the routing controls
    /// of one panel.
    pub control_panel_arn: String,
    pub name: String,
    pub owner: String,
    /// The evaluation period, in milliseconds, the rule holds changes off for.
    pub wait_period_ms: i32,
    pub rule_config: RuleConfig,
    /// Which kind of rule this is, and the controls that kind evaluates.
    pub controls: SafetyRuleControls,
    pub tags: BTreeMap<String, String>,
    pub deploy_due: Instant,
}

impl SafetyRuleRecord {
    /// Move the rule onto `name` and `wait_period_ms` — the only two things an update
    /// changes — making the deployment of the changed configuration due again.
    ///
    /// A request naming the name and wait period the rule already carries writes NOTHING,
    /// so it does not restart a deployment either.
    pub fn set_name_and_wait_period(&mut self, name: String, wait_period_ms: i32) {
        if self.name == name && self.wait_period_ms == wait_period_ms {
            return;
        }
        self.name = name;
        self.wait_period_ms = wait_period_ms;
        self.deploy_due = deploy_due();
    }

    /// This record as the modelled `AssertionRule` shape, or `None` when it is a gating
    /// rule.
    pub fn to_assertion_rule(&self) -> Option<AssertionRule> {
        let SafetyRuleControls::Assertion { asserted_controls } = &self.controls else {
            return None;
        };
        Some(AssertionRule {
            asserted_controls: asserted_controls.clone(),
            control_panel_arn: self.control_panel_arn.clone(),
            name: self.name.clone(),
            rule_config: self.rule_config.clone(),
            safety_rule_arn: self.arn.clone(),
            status: status_at(self.deploy_due).to_string(),
            wait_period_ms: self.wait_period_ms,
            owner: Some(self.owner.clone()),
            // The wire publishes a safety rule's ARN under the generic member name as
            // well as the typed one, holding the same value — the one stored ARN fills
            // both, so the two can never disagree.
            arn: Some(self.arn.clone()),
        })
    }

    /// This record as the modelled `GatingRule` shape, or `None` when it is an assertion
    /// rule.
    pub fn to_gating_rule(&self) -> Option<GatingRule> {
        let SafetyRuleControls::Gating {
            gating_controls,
            target_controls,
        } = &self.controls
        else {
            return None;
        };
        Some(GatingRule {
            control_panel_arn: self.control_panel_arn.clone(),
            gating_controls: gating_controls.clone(),
            name: self.name.clone(),
            rule_config: self.rule_config.clone(),
            safety_rule_arn: self.arn.clone(),
            status: status_at(self.deploy_due).to_string(),
            target_controls: target_controls.clone(),
            wait_period_ms: self.wait_period_ms,
            owner: Some(self.owner.clone()),
            // The generic ARN member, as on an assertion rule above.
            arn: Some(self.arn.clone()),
        })
    }

    /// This record as the modelled `Rule` shape a list page carries: the one member its
    /// kind fills, and nothing under the other.
    pub fn to_rule(&self) -> Rule {
        Rule {
            assertion: self.to_assertion_rule(),
            gating: self.to_gating_rule(),
        }
    }
}

/// What a `ClientToken` the store has already seen means for the call replaying it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TokenOutcome {
    /// The token is new: perform the create.
    Fresh,
    /// The token already created this resource with the same request: answer with it.
    Replay(String),
    /// The token already created a resource from a DIFFERENT request: refuse.
    Mismatch,
}

/// One `ClientToken` the store has honoured.
#[derive(Debug, Clone)]
struct IdempotencyRecord {
    /// The ARN the first call minted.
    arn: String,
    /// The request the first call carried, so a token replayed with different
    /// parameters is distinguishable from a true retry.
    fingerprint: String,
}

/// Per-service state for com-amazonaws-route53recoverycontrolconfig-route53recoverycontrolconfig. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState {
    /// Every created cluster, keyed by its ARN — the only identity a client holds.
    pub clusters: BTreeMap<String, ClusterRecord>,
    /// Every created control panel, keyed by its ARN.
    pub control_panels: BTreeMap<String, ControlPanelRecord>,
    /// Every created routing control, keyed by its ARN.
    pub routing_controls: BTreeMap<String, RoutingControlRecord>,
    /// Every created safety rule, of either kind, keyed by its ARN.
    pub safety_rules: BTreeMap<String, SafetyRuleRecord>,
    /// The resource policy attached to a cluster, keyed by that cluster's ARN — what
    /// `GetResourcePolicy` reports.
    ///
    /// A recovery-control resource policy is attached OUTSIDE this API (sharing a
    /// cluster with another account is what creates one), so no operation of this model
    /// writes this map: it is written through [`Store::attach_resource_policy`] and read
    /// through [`Store::resource_policy_of`], and a cluster nothing has attached a policy
    /// to simply holds no entry here.
    resource_policies: BTreeMap<String, String>,
    /// `"<Operation>#<ClientToken>"` -> the resource that token created. Held to
    /// [`MAX_IDEMPOTENCY_TOKENS`] by [`Self::remember_token`].
    idempotency: BTreeMap<String, IdempotencyRecord>,
    /// The order tokens entered `idempotency`, so the ledger can be held to
    /// [`MAX_IDEMPOTENCY_TOKENS`] by forgetting the oldest.
    idempotency_order: VecDeque<String>,
    /// Advanced once per minted id, so an id is never a function of the request.
    id_seed: u64,
    /// Advanced once per created resource, giving each record its creation order.
    seq_counter: u64,
}

/// A convenient alias for this service's store, which the handlers name a lot.
pub type Store = ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState;

impl ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState {
    /// Mint a fresh id in the 8-4-4-4-12 hexadecimal form AWS gives these resources.
    ///
    /// Drawn from a counter the state advances (seeded from the clock on first use),
    /// never from the request, so creating the same resource twice yields two ids.
    pub fn mint_id(&mut self) -> String {
        let high = self.next_id_word();
        let low = splitmix64(high);
        let hex = format!("{high:016x}{low:016x}");
        format!(
            "{}-{}-{}-{}-{}",
            &hex[0..8],
            &hex[8..12],
            &hex[12..16],
            &hex[16..20],
            &hex[20..32]
        )
    }

    /// Mint a fresh SAFETY-RULE id: sixteen lowercase hexadecimal characters.
    ///
    /// A safety rule's id is not written in the 8-4-4-4-12 form the other
    /// recovery-control resources carry — AWS lays a safety-rule ARN out as
    /// `…:controlpanel/<control panel id>/safetyrule/<sixteen hexadecimal characters>`,
    /// so the FORM a client reads off the ARN it is handed is minted to match. The value
    /// itself is drawn from the same counter as every other id and is never a function of
    /// the request.
    pub fn mint_safety_rule_id(&mut self) -> String {
        format!("{:016x}", self.next_id_word())
    }

    /// The next 64 bits of the id stream, advancing the state's own seed (seeded from the
    /// clock on first use) so no id is a function of the request.
    fn next_id_word(&mut self) -> u64 {
        if self.id_seed == 0 {
            self.id_seed = now_nanos();
        }
        self.id_seed = splitmix64(self.id_seed);
        self.id_seed
    }

    /// The next creation-order number.
    pub fn next_seq(&mut self) -> u64 {
        self.seq_counter = self.seq_counter.wrapping_add(1);
        self.seq_counter
    }

    /// Whether the store already holds [`MAX_CLUSTERS`] clusters, so another create
    /// must be refused rather than grow it.
    pub fn clusters_full(&self) -> bool {
        self.clusters.len() >= MAX_CLUSTERS
    }

    /// Whether the store already holds [`MAX_CONTROL_PANELS`] control panels.
    pub fn control_panels_full(&self) -> bool {
        self.control_panels.len() >= MAX_CONTROL_PANELS
    }

    /// Whether the store already holds [`MAX_ROUTING_CONTROLS`] routing controls.
    pub fn routing_controls_full(&self) -> bool {
        self.routing_controls.len() >= MAX_ROUTING_CONTROLS
    }

    /// Whether the store already holds [`MAX_SAFETY_RULES`] safety rules.
    pub fn safety_rules_full(&self) -> bool {
        self.safety_rules.len() >= MAX_SAFETY_RULES
    }

    /// How many safety rules are stored under `control_panel_arn`.
    pub fn safety_rule_count(&self, control_panel_arn: &str) -> usize {
        self.safety_rules
            .values()
            .filter(|rule| rule.control_panel_arn == control_panel_arn)
            .count()
    }

    /// Whether any stored safety rule references `routing_control_arn` — as a control it
    /// evaluates or as one it gates.
    ///
    /// A routing control a rule names is a dependent of that rule, so this is what the
    /// delete of a routing control asks before removing it.
    pub fn safety_rules_reference(&self, routing_control_arn: &str) -> bool {
        self.safety_rules
            .values()
            .any(|rule| rule.controls.references(routing_control_arn))
    }

    /// The safety rule `control_panel_arn` already holds under `name`, other than the
    /// rule `except_arn` names, or `None` when that name is free there.
    ///
    /// A rule's name identifies it within its control panel, so this is what the create
    /// and the update both ask before taking a name; the record is answered rather than a
    /// yes/no because the refusal names the colliding rule's own id. Scanned in place: no
    /// ordering is needed to answer it.
    pub fn safety_rule_named(
        &self,
        control_panel_arn: &str,
        name: &str,
        except_arn: Option<&str>,
    ) -> Option<&SafetyRuleRecord> {
        self.safety_rules.values().find(|rule| {
            rule.control_panel_arn == control_panel_arn
                && rule.name == name
                && except_arn.is_none_or(|arn| rule.arn != arn)
        })
    }

    /// The control panel `cluster_arn` already holds under `name`, other than the panel
    /// `except_arn` names, or `None` when that name is free there.
    ///
    /// A control panel's name identifies it within its cluster, so this is what the create
    /// and the rename both ask before taking a name; the record is answered rather than a
    /// yes/no because the refusal names the colliding panel's own id. Scanned in place: no
    /// ordering is needed to answer it.
    pub fn control_panel_named(
        &self,
        cluster_arn: &str,
        name: &str,
        except_arn: Option<&str>,
    ) -> Option<&ControlPanelRecord> {
        self.control_panels.values().find(|panel| {
            panel.cluster_arn == cluster_arn
                && panel.name == name
                && except_arn.is_none_or(|arn| panel.arn != arn)
        })
    }

    /// `control_panel_arn`'s safety rules in creation order — the order a list page walks,
    /// with both kinds interleaved as they were created.
    pub fn ordered_safety_rules(&self, control_panel_arn: &str) -> Vec<&SafetyRuleRecord> {
        let mut found: Vec<&SafetyRuleRecord> = self
            .safety_rules
            .values()
            .filter(|rule| rule.control_panel_arn == control_panel_arn)
            .collect();
        found.sort_by_key(|rule| rule.seq);
        found
    }

    /// How many routing controls are stored under `control_panel_arn`.
    ///
    /// This is what `ControlPanel$RoutingControlCount` reports, so the count can never
    /// drift from the routing controls a list call returns.
    pub fn routing_control_count(&self, control_panel_arn: &str) -> i32 {
        self.routing_controls
            .values()
            .filter(|rc| rc.control_panel_arn == control_panel_arn)
            .count()
            .try_into()
            .unwrap_or(i32::MAX)
    }

    /// The routing control `control_panel_arn` already holds under `name`, other than the
    /// control `except_arn` names, or `None` when that name is free there.
    ///
    /// A routing control's name identifies it within its control panel, so this is what
    /// the create and the update both ask before taking a name; the record is answered
    /// rather than a yes/no because the refusal names the colliding control's own id.
    /// Scanned in place: no ordering is needed to answer it.
    pub fn routing_control_named(
        &self,
        control_panel_arn: &str,
        name: &str,
        except_arn: Option<&str>,
    ) -> Option<&RoutingControlRecord> {
        self.routing_controls.values().find(|rc| {
            rc.control_panel_arn == control_panel_arn
                && rc.name == name
                && except_arn.is_none_or(|arn| rc.arn != arn)
        })
    }

    /// `control_panel_arn`'s routing controls in creation order.
    pub fn ordered_routing_controls(&self, control_panel_arn: &str) -> Vec<&RoutingControlRecord> {
        let mut found: Vec<&RoutingControlRecord> = self
            .routing_controls
            .values()
            .filter(|rc| rc.control_panel_arn == control_panel_arn)
            .collect();
        found.sort_by_key(|rc| rc.seq);
        found
    }

    /// Every stored cluster, in creation order — the order an account-wide list pages
    /// through.
    pub fn ordered_clusters(&self) -> Vec<&ClusterRecord> {
        let mut found: Vec<&ClusterRecord> = self.clusters.values().collect();
        found.sort_by_key(|cluster| cluster.seq);
        found
    }

    /// The stored control panels in creation order: those of one cluster when
    /// `cluster_arn` names one, otherwise every panel in the account.
    ///
    /// `ListControlPanels` scopes its page with the optional filter; the operations that
    /// work on one cluster pass `Some(..)`.
    pub fn ordered_control_panels(&self, cluster_arn: Option<&str>) -> Vec<&ControlPanelRecord> {
        let mut found: Vec<&ControlPanelRecord> = self
            .control_panels
            .values()
            .filter(|cp| cluster_arn.is_none_or(|arn| cp.cluster_arn == arn))
            .collect();
        found.sort_by_key(|cp| cp.seq);
        found
    }

    /// What a create call carrying `token` for `operation` should do, given the request
    /// `fingerprint`.
    ///
    /// A request with no token is always [`TokenOutcome::Fresh`]: `@idempotencyToken`
    /// makes the member optional, and without one there is nothing to replay.
    pub fn token_outcome(
        &self,
        operation: &str,
        token: Option<&str>,
        fingerprint: &str,
    ) -> TokenOutcome {
        let Some(token) = token else {
            return TokenOutcome::Fresh;
        };
        match self.idempotency.get(&token_key(operation, token)) {
            None => TokenOutcome::Fresh,
            Some(record) if record.fingerprint == fingerprint => {
                TokenOutcome::Replay(record.arn.clone())
            }
            Some(_) => TokenOutcome::Mismatch,
        }
    }

    /// Remember that `token` created `arn` from the request `fingerprint`, forgetting
    /// the oldest records once the ledger is at [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn remember_token(
        &mut self,
        operation: &str,
        token: Option<&str>,
        fingerprint: &str,
        arn: &str,
    ) {
        let Some(token) = token else {
            return;
        };
        let key = token_key(operation, token);
        let record = IdempotencyRecord {
            arn: arn.to_string(),
            fingerprint: fingerprint.to_string(),
        };
        if self.idempotency.insert(key.clone(), record).is_none() {
            self.idempotency_order.push_back(key);
        }
        while self.idempotency_order.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(oldest) = self.idempotency_order.pop_front() {
                self.idempotency.remove(&oldest);
            }
        }
    }

    /// Forget every token that created a resource whose ARN starts with `arn_prefix`.
    ///
    /// Called by the deletes, so a create/delete loop cannot grow the ledger and no
    /// token can report a resource that is gone as if it were still there. The prefix
    /// form is what releases a control panel's routing-control tokens with it: AWS
    /// nests a routing control's ARN inside its control panel's.
    pub fn forget_tokens_for(&mut self, arn_prefix: &str) {
        let ledger = &mut self.idempotency;
        let order = &mut self.idempotency_order;
        ledger.retain(|_, record| !record.arn.starts_with(arn_prefix));
        order.retain(|key| ledger.contains_key(key));
    }

    /// How many `ClientToken` records the ledger currently holds.
    pub fn idempotency_len(&self) -> usize {
        self.idempotency.len()
    }

    /// Whether this store holds a resource of ANY kind under `arn`.
    ///
    /// The one ARN -> record resolution across every kind is [`Self::tags_of`], which the
    /// tagging operations resolve their `ResourceArn` through; an operation that only
    /// needs to know whether the resource exists asks here rather than repeating that
    /// walk.
    pub fn holds_resource(&self, arn: &str) -> bool {
        self.tags_of(arn).is_some()
    }

    /// The resource policy attached to the cluster `arn` names, or `None` when that
    /// cluster holds none — and `None` for an ARN this store holds no cluster under, so no
    /// read can report a policy for a resource that is gone.
    ///
    /// `GetResourcePolicy` is documented as the policy "for a cluster", so only a cluster
    /// ARN can resolve to one.
    pub fn resource_policy_of(&self, arn: &str) -> Option<&str> {
        if !self.clusters.contains_key(arn) {
            return None;
        }
        self.resource_policies.get(arn).map(String::as_str)
    }

    /// Attach `policy` to the cluster `cluster_arn` names, replacing whatever it held,
    /// and answer whether that cluster exists — the write [`Self::resource_policy_of`]
    /// is the read of.
    ///
    /// NOT dead code and not reachable from any operation of this model: a recovery-control
    /// resource policy is created when a cluster is SHARED with another account, which
    /// happens outside this API, so this is where such an attachment enters the store (and
    /// what this crate's tests attach through). The read is the only thing this model
    /// serves.
    pub fn attach_resource_policy(&mut self, cluster_arn: &str, policy: &str) -> bool {
        if !self.clusters.contains_key(cluster_arn) {
            return false;
        }
        self.resource_policies
            .insert(cluster_arn.to_string(), policy.to_string());
        true
    }

    /// Forget the policy attached to the cluster `cluster_arn` names, so a policy does not
    /// outlive the cluster it was attached to. Called by the delete of a cluster.
    pub fn detach_resource_policy(&mut self, cluster_arn: &str) {
        self.resource_policies.remove(cluster_arn);
    }

    /// The tags stored on the resource `arn` names, whatever kind of resource it is, or
    /// `None` when this store holds no resource under that ARN.
    ///
    /// The tagging operations address a resource by ARN alone, so the ARN is resolved
    /// against every kind of resource stored here: each record keeps its own tag set,
    /// which is what a create's `Tags` filled and what the tag operations read and write.
    pub fn tags_of(&self, arn: &str) -> Option<&BTreeMap<String, String>> {
        if let Some(record) = self.clusters.get(arn) {
            return Some(&record.tags);
        }
        if let Some(record) = self.control_panels.get(arn) {
            return Some(&record.tags);
        }
        if let Some(record) = self.routing_controls.get(arn) {
            return Some(&record.tags);
        }
        self.safety_rules.get(arn).map(|record| &record.tags)
    }

    /// The same tag set, for a caller that writes it ([`tags_of`] is the read).
    ///
    /// [`tags_of`]: Store::tags_of
    pub fn tags_mut(&mut self, arn: &str) -> Option<&mut BTreeMap<String, String>> {
        if let Some(record) = self.clusters.get_mut(arn) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.control_panels.get_mut(arn) {
            return Some(&mut record.tags);
        }
        if let Some(record) = self.routing_controls.get_mut(arn) {
            return Some(&mut record.tags);
        }
        self.safety_rules.get_mut(arn).map(|record| &mut record.tags)
    }
}

/// A `ClientToken` is scoped to the operation that honoured it, so the same token on
/// two different creates is two independent tokens.
fn token_key(operation: &str, token: &str) -> String {
    format!("{operation}#{token}")
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsRoute53recoverycontrolconfigRoute53recoverycontrolconfigState>)
}

/// One page of a list operation: the items to return and the `NextToken` that resumes
/// after them (`None` when this page is the last one).
#[derive(Debug)]
pub struct Page<'a, T> {
    pub items: Vec<&'a T>,
    pub next_token: Option<String>,
}

/// Cut the page `next_token` and `max_results` select out of `ordered`.
///
/// The cursor is the KEY of the first item of the next page, not an index, so it needs
/// no server-side bookkeeping — a list call stays a pure read — and an item created or
/// deleted between two pages never shifts an already-returned item into view twice.
/// `None` when `next_token` names no item in `ordered`, which the caller answers with
/// the list operation's modelled client error.
pub fn paginate<'a, T, K>(
    ordered: &[&'a T],
    key: K,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Option<Page<'a, T>>
where
    K: Fn(&T) -> &str,
{
    let start = match next_token {
        None => 0,
        Some(token) => ordered.iter().position(|item| key(item) == token)?,
    };
    let page_size = match max_results {
        Some(n) if n > 0 => n as usize,
        _ => DEFAULT_PAGE_SIZE,
    };
    let end = start.saturating_add(page_size).min(ordered.len());
    Some(Page {
        items: ordered[start..end].to_vec(),
        next_token: ordered.get(end).map(|item| key(item).to_string()),
    })
}

/// The noun a cluster is named by in a not-found diagnosis ([`not_found_message`]).
pub const CLUSTER_NOUN: &str = "cluster";

/// The noun a control panel is named by in a not-found diagnosis.
pub const CONTROL_PANEL_NOUN: &str = "control panel";

/// The noun a routing control is named by in a not-found diagnosis.
pub const ROUTING_CONTROL_NOUN: &str = "routing control";

/// The noun a safety rule is named by in a not-found diagnosis.
pub const SAFETY_RULE_NOUN: &str = "safety rule";

/// The policy document a resource with NO policy attached to it reports.
///
/// `GetResourcePolicy` answers such a resource with the empty JSON policy document rather
/// than with no document at all: the model bounds `__policy` at `length 2..10240`
/// (`smithy_query(query="operation",operation="GetResourcePolicy")`), and two characters is
/// exactly this document, the shortest one the member can carry.
pub const EMPTY_RESOURCE_POLICY: &str = "{}";

/// The noun a resource whose kind the request does not say is named by in a not-found
/// diagnosis: what the tagging operations' `ResourceArn` is documented as, "the resource
/// that's tagged", when the ARN names no kind this service stores.
pub const RESOURCE_NOUN: &str = "resource";

/// The `ResourceNotFoundException$Message` for a request whose ARN resolves to no
/// resource of this service.
///
/// Route 53 ARC words every such refusal as the one fixed sentence
/// `No record corresponding to <noun> Arn(<arn>)`: the kind of resource that was looked
/// up, and the ARN the request asked for, echoed unchanged. Nothing else in it varies,
/// so every lookup in this crate builds its 404 message here rather than phrasing its
/// own.
pub fn not_found_message(noun: &str, arn: &str) -> String {
    format!("No record corresponding to {noun} Arn({arn})")
}

/// The `ResourceNotFoundException$Message` `DescribeRoutingControl` answers an ARN
/// resolving to no stored routing control with.
///
/// This one read does not state the [`not_found_message`] sentence the cluster, the control
/// panel and the safety rule lookups answer. Its recorded refusal is
/// `Routing control <id> does not exist in control panel <control panel id>`, where the
/// control is named by the id path its ARN nests — its control panel's id immediately
/// followed by its own, with no separator between them — and the panel by its id. Both ids
/// come from the ARN the REQUEST carried, so the sentence is a template: two replays of the
/// same recorded call, against freshly minted ids, are answered with each run's own ids.
///
/// `DeleteRoutingControl` is NOT worded this way: it states the shared sentence, which is
/// what its own recorded refusal carries. Each operation of this resource kind words its
/// own, so neither is derived from the other, and the two differ in which ids they name as
/// well as in their fixed text — confirmed by replaying the recorded refusals one handler
/// at a time.
///
/// The run-together ids are the service's own wording, NOT a typo to tidy up: separating
/// them makes the recorded refusal diverge.
///
/// An ARN that nests no routing control inside a control panel leaves the sentence no
/// control panel to name, so it falls back to [`not_found_message`] — the sentence every
/// other unresolvable ARN of this service is refused with.
pub fn routing_control_describe_not_found_message(arn: &str) -> String {
    let Some((control_panel_id, routing_control_id)) = nested_routing_control_ids(arn) else {
        return not_found_message(ROUTING_CONTROL_NOUN, arn);
    };
    format!(
        "Routing control {control_panel_id}{routing_control_id} does not exist in control panel \
         {control_panel_id}"
    )
}

/// The `ResourceNotFoundException$Message` `UpdateRoutingControl` answers an ARN resolving
/// to no stored routing control with.
///
/// The update's recorded refusal states its own sentence rather than the read's: the fixed
/// text `not found in`, with the routing control's own id before it — and only its own, not
/// the id path the read names — and its control panel's id after it. Both ids come from the
/// ARN the REQUEST carried, never from any one answer.
///
/// The missing space between the control's id and `not found in` is the service's own
/// wording, NOT a typo to tidy up: adding one, or naming the control by the read's id path,
/// makes the recorded refusal diverge. Each was checked against the recording on its own.
///
/// An ARN that nests no routing control falls back to [`not_found_message`], for the same
/// reason as the read's sentence: there is then no control panel to name.
pub fn routing_control_update_not_found_message(arn: &str) -> String {
    let Some((control_panel_id, routing_control_id)) = nested_routing_control_ids(arn) else {
        return not_found_message(ROUTING_CONTROL_NOUN, arn);
    };
    format!("{routing_control_id}not found in {control_panel_id}")
}

/// The `(control panel id, routing control id)` an ARN nesting a routing control inside a
/// control panel carries, or `None` for an ARN that nests no routing control.
fn nested_routing_control_ids(arn: &str) -> Option<(&str, &str)> {
    let parsed = ::service_common::arn::parse(arn)?;
    // A nested resource is appended to its parent's resource field, so the segments
    // alternate `<type>/<id>`: a routing control's ARN carries its control panel's type
    // and id, then its own.
    let segments: Vec<&str> = parsed.resource.split('/').collect();
    let ["controlpanel", control_panel_id, "routingcontrol", routing_control_id] =
        segments.as_slice()
    else {
        return None;
    };
    Some((control_panel_id, routing_control_id))
}

/// The name a cluster is named by where a request REFERENCES one it must attach to
/// ([`does_not_exist_message`]) or owns dependents of ([`cannot_delete_message`]).
///
/// Route 53 ARC names the KIND of resource in those sentences, in the title case it
/// writes them with (`Cannot delete a Cluster with non-default Control Panels.`), rather
/// than the lower-case prose noun a 404 uses.
pub const CLUSTER_RESOURCE: &str = "Cluster";

/// The name a control panel is named by in those same refusals.
pub const CONTROL_PANEL_RESOURCE: &str = "Control Panel";

/// The name a routing control is named by in those same refusals.
pub const ROUTING_CONTROL_RESOURCE: &str = "Routing Control";

/// The name a safety rule is named by in those same refusals.
pub const SAFETY_RULE_RESOURCE: &str = "Safety Rule";

/// The namespace this service's IAM actions are published under: the model's
/// `arnNamespace`, which is also its endpoint prefix
/// (`smithy_query(query="service_info")`) and so is NOT [`ARN_SERVICE`], the namespace its
/// RESOURCE ARNs carry.
pub const IAM_ACTION_NAMESPACE: &str = "route53-recovery-control-config";

/// The `ValidationException$Message` for a caller-supplied ARN whose SYNTAX is not an
/// ARN's at all.
///
/// Route 53 ARC reads such a value as a fact about the request rather than about the
/// account's state: the fixed clause `ARN format is invalid: ` followed by the value the
/// request sent, echoed exactly as it arrived, and closed by a full stop. That echoed
/// value is the sentence's only variable part.
pub fn invalid_arn_format_message(arn: &str) -> String {
    format!("ARN format is invalid: {arn}.")
}

/// Refuse a caller-supplied `arn` that is not a well-formed ARN, before anything resolves
/// it.
///
/// A value whose syntax is not an ARN's addresses no resource in any account, so it can
/// only be a request the caller built wrongly: it is refused as malformed INPUT
/// ([`invalid_arn_format_message`], `ValidationException`/400) and never as an absent
/// resource, which is a statement about what this account holds and is reserved for a
/// well-formed ARN that holds nothing ([`not_found_message`], 404).
///
/// The syntax is the tree's one ARN parse ([`::service_common::arn::parse`]), so this
/// service cannot disagree with another about what an ARN is: six colon-separated fields
/// led by `arn`, with a non-empty partition, vendor and resource. An empty Region and an
/// empty account are well formed — this service's own resource ARNs carry an empty Region
/// ([`ARN_REGION`]) — so neither is refused here.
pub fn read_arn_format(arn: &str) -> Result<(), String> {
    if ::service_common::arn::parse(arn).is_none() {
        return Err(invalid_arn_format_message(arn));
    }
    Ok(())
}

/// Why a caller-supplied ARN is refused before the handler resolves it, in the order the
/// two stages of [`read_resource_arn`] take them.
#[derive(Debug)]
pub enum ArnRefused {
    /// The value is not a well-formed ARN: `ValidationException` (400) carrying this
    /// sentence ([`invalid_arn_format_message`]).
    Malformed(String),
    /// The ARN is well formed and names ANOTHER account: `AccessDeniedException` (403).
    Denied(AccessDeniedException),
}

impl ArnRefused {
    /// This refusal as ONE operation's modelled error, built by that operation's own
    /// variant constructors.
    ///
    /// Which shape states each class is the same for every operation — a malformed value
    /// is wrong INPUT (400) and another account's ARN is a denial (403) — so the mapping
    /// lives here rather than being written out at each call site.
    pub fn lifted<E>(
        self,
        validation: impl FnOnce(ValidationException) -> E,
        denied: impl FnOnce(AccessDeniedException) -> E,
    ) -> E {
        match self {
            Self::Malformed(message) => validation(ValidationException { message }),
            Self::Denied(denial) => denied(denial),
        }
    }
}

/// The whole gate an operation passes a caller-supplied ARN through before resolving it:
/// its FORM first ([`read_arn_format`]), then whose it is ([`authorize_resource_arn`]).
///
/// The order is forced by what each stage reads: no account can be read out of a value
/// that is not an ARN, so the syntax is decided first, and only a well-formed ARN has an
/// owner to compare the caller against. An operation lifts each arm into its own modelled
/// shape ([`ArnRefused::lifted`]), so every operation of this service that takes such an
/// ARN refuses the two classes alike.
pub fn read_resource_arn(action: &str, arn: &str) -> Result<(), ArnRefused> {
    read_arn_format(arn).map_err(ArnRefused::Malformed)?;
    authorize_resource_arn(action, arn).map_err(ArnRefused::Denied)
}

/// Refuse a request that names `arn` when that ARN belongs to another account.
///
/// Whether the caller may act on a resource is decided from the REQUEST alone — the
/// principal that signed it, the action it asked for and the ARN it named — so it is
/// decided before the ARN is resolved: a caller who may not act on another account's
/// resource never learns from the answer whether one is stored under it. Every operation
/// that takes a caller-supplied ARN of this service's resources asks here first, through
/// [`read_resource_arn`], with its own modelled name as `action`, and lifts the payload
/// into its own `AccessDeniedException` (403).
///
/// `Ok` means only that the ARN names no OTHER account: an ARN of the caller's own
/// account, and a well-formed ARN whose account field is empty, are both left to the
/// lookup the handler goes on to do, which is what refuses them. A value that is not a
/// well-formed ARN never reaches this stage — [`read_resource_arn`] refuses it as
/// malformed input first, because an ARN no account can be read out of addresses nothing
/// to authorize.
pub fn authorize_resource_arn(action: &str, arn: &str) -> Result<(), AccessDeniedException> {
    // The tree's one ARN parse, so this service cannot disagree with another about which
    // account an ARN names.
    let account = ::service_common::arn::parse(arn).map(|parsed| parsed.account);
    let Some(account) = account.filter(|account| !account.is_empty()) else {
        return Ok(());
    };
    if account == ::engine::emulator_account_id() {
        return Ok(());
    }
    Err(AccessDeniedException {
        message: ::service_common::iam_authorization::not_authorized_by_resource_policy(
            &::service_common::caller_principal::ambient_user_arn(),
            &format!("{IAM_ACTION_NAMESPACE}:{action}"),
            arn,
        ),
    })
}

/// The `ValidationException$Message` for a create request that names a resource it would
/// attach to and that this service does not hold.
///
/// Such a request never resolves to a resource of its own, so it is refused by its
/// INPUT, naming the kind of resource it referenced and the ARN it sent:
/// `<Resource> <arn> does not exist.`
pub fn does_not_exist_message(resource: &str, arn: &str) -> String {
    format!("{resource} {arn} does not exist.")
}

/// The `ValidationException$Message` for a delete request against a resource that still
/// owns the dependents named by `dependents`.
///
/// `Cannot delete a <Resource> with <dependents>.` — the kind of resource the request
/// addressed, and the kind of children that keep it alive.
pub fn cannot_delete_message(resource: &str, dependents: &str) -> String {
    format!("Cannot delete a {resource} with {dependents}.")
}

/// The dependents a cluster cannot be deleted while it still owns: the control panels a
/// caller created in it, as opposed to the default panel that lives and dies with it.
pub const NON_DEFAULT_CONTROL_PANEL_DEPENDENTS: &str = "non-default Control Panels";

/// The dependents a cluster or a control panel cannot be deleted while it still owns.
pub const ROUTING_CONTROL_DEPENDENTS: &str = "Routing Controls";

/// The dependents a control panel cannot be deleted while it still owns.
pub const SAFETY_RULE_DEPENDENTS: &str = "Safety Rules";

/// The `ValidationException$Message` for a create or rename whose name is already taken
/// inside the parent that scopes it.
///
/// `A <resource> with the specified name already exists: <existing id>, <name>` — the
/// kind of resource the name belongs to, written as the lower-case prose noun of the
/// title-case `*_RESOURCE` constant, then the SERVER-CHOSEN id of the resource that
/// already holds the name ([`resource_id`] of its stored ARN), then the name the request
/// asked for, verbatim. The colliding resource is named by its own id, not by the parent
/// the name is scoped to, so a caller can read out of the refusal which stored resource
/// took the name as well as which name it asked for. No full stop: the sentence ends with
/// the requested name.
pub fn already_exists_message(resource: &str, existing_id: &str, name: &str) -> String {
    format!(
        "A {} with the specified name already exists: {existing_id}, {name}",
        resource.to_lowercase()
    )
}

/// The server-chosen id of the resource `arn` names: the last segment of its resource
/// path, which every create of this service mints with [`Store::mint_id`] and appends to
/// its parent's ARN.
///
/// A value no id can be read out of — one carrying no `/` at all — is its own id, so a
/// refusal that names a resource always names something.
pub fn resource_id(arn: &str) -> &str {
    arn.rsplit_once('/').map_or(arn, |(_, id)| id)
}

/// The `ValidationException$Message` for a delete request against the resource a parent
/// is created with and deleted with.
///
/// `Cannot delete a default <Resource>.` — the [`cannot_delete_message`] family, for the
/// refusal whose reason is what the resource IS rather than what it still owns.
pub fn cannot_delete_default_message(resource: &str) -> String {
    format!("Cannot delete a default {resource}.")
}

/// The `ValidationException$Message` for a request that must carry exactly ONE rule and
/// carries both kinds of rule.
///
/// Route 53 ARC words this refusal as one fixed sentence, with no part taken from the
/// request: not the member names, not the rule's name, not the control panel it would
/// have been created in.
///
/// A request carrying NEITHER kind names no resource for the front door ahead of these
/// operations to authorize the call against, so that layer refuses it first — its own HTTP
/// 401 `UnauthorizedException`, not this sentence
/// ([`crate::service`]'s authorization-target stage).
pub const INVALID_RULE_MESSAGE: &str = "Invalid Rule provided.";

/// The `ValidationException$Message` for an update that addresses a safety rule of the
/// OTHER kind than the update it carries.
///
/// `Safety Rule <arn> is a <stored> rule, not a <requested> rule.` — the rule the request
/// addressed, the kind it really is and the kind the update would have changed.
pub fn wrong_rule_kind_message(arn: &str, stored: &str, requested: &str) -> String {
    format!("{SAFETY_RULE_RESOURCE} {arn} is a {stored} rule, not a {requested} rule.")
}

/// A request's `Tags` map as a deterministic string, for an idempotency fingerprint.
pub fn tags_fingerprint(tags: Option<&std::collections::HashMap<String, String>>) -> String {
    let mut pairs: Vec<(&str, &str)> = tags
        .map(|t| t.iter().map(|(k, v)| (k.as_str(), v.as_str())).collect())
        .unwrap_or_default();
    pairs.sort_unstable();
    pairs
        .iter()
        .map(|(k, v)| format!("{k}={v}"))
        .collect::<Vec<_>>()
        .join(",")
}

/// A request's `Tags` map as the ordered map a record stores.
pub fn tags_map(tags: Option<std::collections::HashMap<String, String>>) -> BTreeMap<String, String> {
    tags.unwrap_or_default().into_iter().collect()
}

/// The most tags one resource carries at once.
///
/// AWS's tag restrictions state a maximum of 50 user-created tags per resource, so a
/// request that would leave a resource holding more is out of range
/// ([`too_many_tags_message`]). It also bounds caller-driven growth of the store, the
/// same reason the resource ceilings above exist.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// The longest tag KEY a resource holds.
///
/// AWS's tag restrictions state a maximum key length of 128 characters. The model puts no
/// `@length` on the map's key, which targets the service's unconstrained `__string`
/// (`smithy_query(query="shape",shape="__mapOf__stringMin0Max256PatternS")`), so this is
/// where that bound is applied; a tag VALUE is already bounded to 256 characters by the
/// type the same map targets.
pub const MAX_TAG_KEY_LENGTH: usize = 128;

/// The `ValidationException$Message` for a tag key longer than [`MAX_TAG_KEY_LENGTH`].
///
/// States the limit and how long the key the request sent is; the key itself is not echoed,
/// so a refusal of an enormous key is not answered with it.
pub fn tag_key_too_long_message(length: usize) -> String {
    format!(
        "A tag key may be at most {MAX_TAG_KEY_LENGTH} characters; this request carries one of \
         {length}."
    )
}

/// The `ValidationException$Message` for a tag request that would push a resource past
/// [`MAX_TAGS_PER_RESOURCE`].
///
/// Names the limit and the number the resource would be left holding, both as the
/// request and the stored resource decide them.
pub fn too_many_tags_message(resulting: usize) -> String {
    format!(
        "A resource may carry at most {MAX_TAGS_PER_RESOURCE} tags; this request would leave \
         it with {resulting}."
    )
}

/// The noun a request that resolved `arn` to no resource names the kind it looked for
/// by, read from the ARN itself.
///
/// The operations that address ONE kind of resource know the noun from the member they
/// took; the tagging operations take any of this service's resource ARNs, so the kind
/// comes from the ARN's own resource field — `cluster/<id>`, `controlpanel/<id>`,
/// `controlpanel/<id>/routingcontrol/<id>`, `controlpanel/<id>/safetyrule/<id>`, as the
/// create handlers mint them. A value that is not an ARN of one of those kinds is named
/// by the kind-agnostic [`RESOURCE_NOUN`].
pub fn noun_for_arn(arn: &str) -> &'static str {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return RESOURCE_NOUN;
    };
    // A nested resource is appended to its parent's resource field, so the segments
    // alternate `<type>/<id>` and the kind is the LAST type segment.
    let kind = parsed.resource.split('/').step_by(2).last().unwrap_or_default();
    match kind {
        "cluster" => CLUSTER_NOUN,
        "controlpanel" => CONTROL_PANEL_NOUN,
        "routingcontrol" => ROUTING_CONTROL_NOUN,
        "safetyrule" => SAFETY_RULE_NOUN,
        _ => RESOURCE_NOUN,
    }
}

/// Wall-clock nanoseconds since the epoch, used only to seed the id counter.
fn now_nanos() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(1, |d| d.as_nanos() as u64)
        | 1
}

/// SplitMix64: a bijection on `u64`, so advancing the id counter through it never
/// revisits a value before the counter wraps.
fn splitmix64(state: u64) -> u64 {
    let mut z = state.wrapping_add(0x9E37_79B9_7F4A_7C15);
    z = (z ^ (z >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    z = (z ^ (z >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    z ^ (z >> 31)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// Changing a cluster's network type makes its deployment due again; requesting the
    /// network it is already on writes nothing. The published endpoints follow the
    /// address this emulator is reachable at either way, so the transition leaves them
    /// alone.
    #[test]
    fn setting_the_network_type_redeploys_only_when_it_changes() {
        let arn = "arn:aws:route53-recovery-control::000000000000:cluster/abc";
        // The base this emulator is reachable at, as `OperationContext::endpoint`
        // reports it to a handler.
        let base = "https://127.0.0.1:9297";
        // Rows: every ordered pair of the values `NetworkType` declares
        // (`smithy_query(query="shape",shape="NetworkType")` -> DUALSTACK | IPV4).
        for from in ["DUALSTACK", "IPV4"] {
            for to in ["DUALSTACK", "IPV4"] {
                let case = format!("{from} -> {to}");
                let mut record = ClusterRecord {
                    seq: 1,
                    arn: arn.to_string(),
                    name: "c".to_string(),
                    owner: "000000000000".to_string(),
                    network_type: from.to_string(),
                    home_region: "us-east-1".to_string(),
                    tags: BTreeMap::new(),
                    default_control_panel_arn: String::new(),
                    deploy_due: Instant::now(),
                };
                let was_due = record.deploy_due;

                let changed = record.set_network_type(to.to_string());

                assert_eq!(
                    changed,
                    from != to,
                    "case {case}: the record changes exactly when the network type does, \
                     got {changed}"
                );
                assert_eq!(
                    record.network_type, to,
                    "case {case}: expected the requested network type, got {}",
                    record.network_type
                );
                let published = record.to_cluster(base);
                assert_eq!(
                    endpoint_values(&published),
                    vec![base.to_string(); CLUSTER_ENDPOINT_REGIONS.len()],
                    "case {case}: the published endpoints must stay the address the \
                     emulator is reachable at"
                );
                assert_eq!(
                    record.deploy_due > was_due,
                    from != to,
                    "case {case}: only a change re-deploys the cluster"
                );
            }
        }
    }

    /// The `Endpoint` of every entry of a projected cluster, in the order it reports
    /// them.
    fn endpoint_values(cluster: &Cluster) -> Vec<String> {
        cluster
            .cluster_endpoints
            .as_deref()
            .unwrap_or_default()
            .iter()
            .map(|entry| entry.endpoint.clone().unwrap_or_default())
            .collect()
    }

    /// A record the capacity tests can store without going through a handler.
    fn routing_control(arn: &str) -> RoutingControlRecord {
        RoutingControlRecord {
            seq: 0,
            arn: arn.to_string(),
            control_panel_arn: "panel".to_string(),
            cluster_arn: "cluster".to_string(),
            name: arn.to_string(),
            owner: "000000000000".to_string(),
            tags: BTreeMap::new(),
            deploy_due: Instant::now(),
            health_check_sequence_number: FIRST_HEALTH_CHECK_SEQUENCE_NUMBER,
        }
    }

    /// The ledger never grows past its ceiling, however many tokens a caller spends,
    /// and what it keeps is the newest.
    #[test]
    fn the_idempotency_ledger_is_bounded_and_keeps_the_newest_tokens() {
        let mut state = Store::default();
        let total = MAX_IDEMPOTENCY_TOKENS + 25;
        for n in 0..total {
            state.remember_token("CreateCluster", Some(&format!("token-{n}")), "fp", "arn");
        }
        assert!(
            state.idempotency_len() <= MAX_IDEMPOTENCY_TOKENS,
            "case {total} tokens: the ledger must stay within {MAX_IDEMPOTENCY_TOKENS}, got {}",
            state.idempotency_len()
        );
        let newest = format!("token-{}", total - 1);
        assert_eq!(
            state.token_outcome("CreateCluster", Some(&newest), "fp"),
            TokenOutcome::Replay("arn".to_string()),
            "case newest token: it must still be remembered"
        );
        assert_eq!(
            state.token_outcome("CreateCluster", Some("token-0"), "fp"),
            TokenOutcome::Fresh,
            "case oldest token: it must have been evicted"
        );
    }

    /// Deleting a resource releases the tokens that created it, so a create/delete loop
    /// cannot accumulate ledger entries.
    #[test]
    fn deleting_a_resource_releases_its_tokens() {
        let mut state = Store::default();
        let panel = "arn:aws:route53-recovery-control::000000000000:controlpanel/p";
        let control = format!("{panel}/routingcontrol/r");
        state.remember_token("CreateControlPanel", Some("t-panel"), "fp", panel);
        state.remember_token("CreateRoutingControl", Some("t-control"), "fp", &control);
        assert_eq!(
            state.idempotency_len(),
            2,
            "case two tokens spent: both are remembered"
        );

        // The prefix form releases the panel's routing-control tokens with it.
        state.forget_tokens_for(panel);
        assert_eq!(
            state.idempotency_len(),
            0,
            "case panel deleted: every token naming it or a resource under it is released, \
             got {}",
            state.idempotency_len()
        );
        for (case, token) in [("panel", "t-panel"), ("routing control", "t-control")] {
            assert_eq!(
                state.token_outcome("CreateControlPanel", Some(token), "fp"),
                TokenOutcome::Fresh,
                "case released {case} token: it must no longer name a deleted resource"
            );
        }
    }

    /// Each resource store reports itself full exactly at its ceiling, which is what the
    /// create handlers refuse past.
    #[test]
    fn a_resource_store_reports_itself_full_at_its_ceiling() {
        let mut state = Store::default();
        assert!(
            !state.routing_controls_full(),
            "case empty store: it is not full"
        );
        for n in 0..MAX_ROUTING_CONTROLS {
            let arn = format!("arn/{n}");
            state.routing_controls.insert(arn.clone(), routing_control(&arn));
            assert_eq!(
                state.routing_controls_full(),
                n + 1 >= MAX_ROUTING_CONTROLS,
                "case {} stored: full must flip only at {MAX_ROUTING_CONTROLS}",
                n + 1
            );
        }
        assert!(
            state.routing_controls_full(),
            "case {MAX_ROUTING_CONTROLS} stored: the store is full"
        );
    }
}
