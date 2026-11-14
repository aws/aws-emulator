// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7 service.
//!
//! The primary resource is the Classic load balancer (the model's `AccessPoint`):
//! [`LoadBalancer`], keyed by `LoadBalancerName` in a [`BTreeMap`] so
//! `DescribeLoadBalancers` has one stable order to paginate over.
//!
//! One thing a load balancer reports is NOT this service's to hold: a subnet is EC2's
//! resource, and which Availability Zone a subnet is in is EC2's to say, so the zones of a
//! load balancer placed in subnets are resolved against EC2 ([`resolve_subnet_placement`])
//! and kept beside the subnet set that produced them.

use std::collections::{BTreeMap, BTreeSet};
use std::time::SystemTime;

/// The ELBv2 crate, which this front door already serves on the endpoint the two models
/// share (`src/service/api_version.rs`): its `state::network` is the ONE place either
/// model asks EC2 about the account's network, so the subnet lookup lives there rather
/// than a second time here.
use service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10 as elbv2;

/// The EC2 refusal a request's subnets or security groups ran into, as the ELBv2 crate's
/// one network lookup reports it: re-exported here so a handler of THIS model names it
/// without reaching across to the other crate's module path.
pub use elbv2::state::network::NetworkRefusal;

/// The scheme a load balancer gets when the request names none.
///
/// From `CreateLoadBalancer`'s own documentation: "By default, Elastic Load Balancing
/// creates an Internet-facing load balancer".
pub const SCHEME_INTERNET_FACING: &str = "internet-facing";

/// The only other scheme `CreateLoadBalancer` documents: "Specify internal to create a
/// load balancer with a DNS name that resolves to private IP addresses."
pub const SCHEME_INTERNAL: &str = "internal";

/// The region an emulated request is attributed to when it carries no SigV4 scope.
pub const DEFAULT_REGION: &str = "us-east-1";

/// The maximum number of load balancers one account holds in one region.
///
/// From `CreateLoadBalancer`'s own documentation: "You can create up to 20 load
/// balancers per region per account."
pub const MAX_LOAD_BALANCERS_PER_REGION: usize = 20;

/// The largest number of DISTINCT load balancers one `DescribeLoadBalancers` request may
/// name.
///
/// AWS publishes no quota for the member and accepts a list that repeats a name, so the
/// handler deduplicates first and applies this to the distinct names only, after each one
/// is known to exist. A region holds at most [`MAX_LOAD_BALANCERS_PER_REGION`] load
/// balancers, so a request whose names all exist can never reach this bound: it is the
/// backstop that keeps the work one request can ask for bounded by the store rather than
/// by the caller.
pub const MAX_LOAD_BALANCER_NAMES_PER_REQUEST: usize = MAX_LOAD_BALANCERS_PER_REGION;

/// The listener protocols the model documents for `Listener$Protocol` /
/// `Listener$InstanceProtocol`: "The load balancer transport protocol to use for
/// routing: HTTP, HTTPS, TCP, or SSL." Listed in the order this service's own refusal
/// names them, which is what [`listener_protocol_message`] states.
pub const LISTENER_PROTOCOLS: &[&str] = &["HTTP", "TCP", "HTTPS", "SSL"];

/// The refusal a listener protocol member carrying something outside
/// [`LISTENER_PROTOCOLS`] earns, stated over the protocols that member may carry: the
/// caller fixes the value from the set, and the sentence names which member it has to fix.
pub fn listener_protocol_message(member: &str) -> String {
    format!("{member} must be one of {}", LISTENER_PROTOCOLS.join(", "))
}

/// Published Classic Load Balancer quotas, from the "Elastic Load Balancing endpoints
/// and quotas" page of the AWS General Reference. Each one bounds a collection a
/// CALLER drives; together with [`MAX_REGIONS`], which bounds the one store dimension
/// AWS publishes no quota for, the store a request can grow is bounded too.
///
/// "Listeners per load balancer: 100".
pub const MAX_LISTENERS_PER_LOAD_BALANCER: usize = 100;

/// "Registered instances per load balancer: 1,000".
pub const MAX_INSTANCES_PER_LOAD_BALANCER: usize = 1_000;

/// "Tags per load balancer: 10".
pub const MAX_TAGS_PER_LOAD_BALANCER: usize = 10;

/// "Security groups per load balancer: 5".
pub const MAX_SECURITY_GROUPS_PER_LOAD_BALANCER: usize = 5;

/// The refusal a `SecurityGroups` list past that quota earns, stated over the quota: the
/// caller fixes the list from the number, and it is the same quota wherever a load
/// balancer's groups are set, so both operations that take the member state it this way.
pub fn security_groups_quota_message(count: usize) -> String {
    format!(
        "SecurityGroups must carry at most {MAX_SECURITY_GROUPS_PER_LOAD_BALANCER} entries; \
         got {count}"
    )
}

/// The refusal a request whose tags would push a load balancer's tag set past
/// [`MAX_TAGS_PER_LOAD_BALANCER`] earns, stated over the quota: the caller fixes the
/// request from the number, and it is the same quota wherever a load balancer's tags are
/// set, so every operation that takes a tag list states it this way.
pub fn too_many_tags_message() -> String {
    format!("Quota of {MAX_TAGS_PER_LOAD_BALANCER} tags per load balancer reached")
}

/// The refusal a request naming one tag key more than once earns.
///
/// `DuplicateTagKeysException` is documented "A tag key was specified more than once", and
/// the sentence Elastic Load Balancing states it with is FIXED text in full: it states the
/// rule the caller has to satisfy and names neither the key nor the load balancer, so
/// nothing of the request is interpolated into it.
pub fn duplicate_tag_key_message() -> String {
    "Duplicate Tag Keys found in the request. Tag keys should be unique.".to_string()
}

/// Why a request's tag list cannot be collected: the two refusals every operation taking a
/// `TagList` answers, each under its own modelled error.
#[derive(Debug)]
pub enum TagRefusal {
    /// The list names more distinct keys than a load balancer may hold, so no load balancer
    /// can end up carrying them — `TooManyTags`.
    TooMany,
    /// One key, stated more than once in one request — `DuplicateTagKeys`.
    DuplicateKey,
}

impl TagRefusal {
    /// The sentence this refusal states: one error code answers it wherever a load
    /// balancer's tags are set, so the wording lives here rather than in each handler.
    pub fn reported_message(&self) -> String {
        match self {
            Self::TooMany => too_many_tags_message(),
            Self::DuplicateKey => duplicate_tag_key_message(),
        }
    }
}

/// The DISTINCT tags of one request's tag list, keyed by tag key.
///
/// A tag's value is optional ("Each tag consists of a key and an optional value"), and a
/// tag given no value is collected under the empty value, so one key holds one value
/// whichever operation set it.
///
/// `TagList` carries no modelled maximum, so this is also where the work one request can
/// ask for is bounded: collecting stops at the first key past
/// [`MAX_TAGS_PER_LOAD_BALANCER`] distinct ones, and at the first key stated twice, because
/// neither request can be served whatever the store holds.
pub fn collect_requested_tags<'a>(
    requested: impl IntoIterator<Item = (&'a str, Option<&'a str>)>,
) -> Result<BTreeMap<String, String>, TagRefusal> {
    let mut tags: BTreeMap<String, String> = BTreeMap::new();
    for (key, value) in requested {
        if tags.len() >= MAX_TAGS_PER_LOAD_BALANCER && !tags.contains_key(key) {
            return Err(TagRefusal::TooMany);
        }
        if tags
            .insert(key.to_string(), value.unwrap_or_default().to_string())
            .is_some()
        {
            return Err(TagRefusal::DuplicateKey);
        }
    }
    Ok(tags)
}

/// The largest `AvailabilityZones` or `Subnets` list a create accepts.
///
/// AWS states this quota structurally ("one subnet per Availability Zone") rather than
/// as a number, and this emulator models no region topology, so this is a bound the
/// emulator chooses: comfortably above the Availability Zone count of any AWS region,
/// and low enough that one request cannot grow the store without limit.
pub const MAX_PLACEMENTS_PER_LOAD_BALANCER: usize = 50;

/// The largest `Instances` list one register/deregister/describe-health request may
/// carry: the same bound as the set it can grow, so the per-request work is bounded by
/// a constant rather than by the caller.
pub const MAX_INSTANCES_PER_REQUEST: usize = MAX_INSTANCES_PER_LOAD_BALANCER;

/// The number of distinct regions this store will hold records for.
///
/// Every published quota above is per region or per load balancer, so the region
/// dimension is the one a caller could otherwise multiply freely: the region comes from
/// the request's SigV4 credential scope, which the caller writes. AWS publishes no
/// quota here (a region is not a resource an account creates), so this is a bound the
/// emulator chooses, comfortably above the number of regions AWS publishes across all
/// partitions. A create that would open one region past it is refused.
pub const MAX_REGIONS: usize = 64;

/// The periodic health check of one registered instance, which is what
/// `DescribeInstanceHealth` reports the outcome of.
mod instance_checks;
pub use instance_checks::*;

/// The registered-instance set of a load balancer, and the health checking each
/// registration starts.
mod instance_health;
pub use instance_health::*;

/// Whether the account's network puts a load balancer in a VPC of its own rather than in
/// EC2-Classic or the default VPC, which is what the EC2-Classic zone API turns on.
mod vpc_placement;
pub use vpc_placement::*;

/// The ADDRESS one load balancer publishes as its `DNSName`, and the socket of this
/// emulator's own that answers there.
mod load_balancer_endpoint;
pub use load_balancer_endpoint::*;

/// Which Availability Zone EC2 reports each registered instance in, which is what decides
/// who a load balancer node may send a request to while cross-zone load balancing is off.
mod instance_zones;
pub use instance_zones::*;

/// The health of one instance with respect to one load balancer, as the three members of
/// `InstanceState` report it together.
///
/// Every field is one of the values that member's own documentation enumerates, so a
/// projection cannot report a state, a reason code or a description Elastic Load
/// Balancing does not use.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InstanceHealth {
    pub state: &'static str,
    pub reason_code: &'static str,
    pub description: &'static str,
}

/// `InstanceState$State`, from that member's documented valid values
/// (`InService` | `OutOfService` | `Unknown`).
pub const STATE_OUT_OF_SERVICE: &str = "OutOfService";

/// `InstanceState$ReasonCode`, from that member's documented valid values
/// (`ELB` | `Instance` | `N/A`), documented as "Information about the cause of
/// <code>OutOfService</code> instances. Specifically, whether the cause is Elastic Load
/// Balancing or the instance."
pub const REASON_CODE_ELB: &str = "ELB";
pub const REASON_CODE_INSTANCE: &str = "Instance";

/// One of the descriptions `InstanceState$Description` enumerates: the instance is not
/// registered with the load balancer the health was asked about, so Elastic Load
/// Balancing — not the instance — is why it takes no traffic.
pub const HEALTH_NOT_REGISTERED: InstanceHealth = InstanceHealth {
    state: STATE_OUT_OF_SERVICE,
    reason_code: REASON_CODE_ELB,
    description: "Instance is not currently registered with the LoadBalancer.",
};

/// The health of an instance whose registration has not yet been health-checked: again
/// Elastic Load Balancing's own doing, with the description that member enumerates for it.
pub const HEALTH_REGISTRATION_IN_PROGRESS: InstanceHealth = InstanceHealth {
    state: STATE_OUT_OF_SERVICE,
    reason_code: REASON_CODE_ELB,
    description: "Instance registration is still in progress.",
};

/// The health of an instance that has failed the health checks it owes: the reason is the
/// instance, with the description `InstanceState$Description` enumerates for exactly that
/// — fixed text, naming the threshold rather than substituting its value.
pub const HEALTH_FAILED_HEALTH_CHECKS: InstanceHealth = InstanceHealth {
    state: STATE_OUT_OF_SERVICE,
    reason_code: REASON_CODE_INSTANCE,
    description: "Instance has failed at least the UnhealthyThreshold number of health \
                  checks consecutively.",
};

/// How often one health check of a registered instance falls due.
///
/// The CADENCE of a check, which the verdict a registration reaches is counted in: a load
/// balancer's configured `HealthCheck$Interval` is the cadence of a real probe, and this
/// emulator opens no connection, so the span is this emulator's own choice — long enough
/// that a caller reading straight after a registration still sees the checks running, and
/// short enough that the configured thresholds are reached without a client waiting out
/// the real interval. What each due check OBSERVES is
/// [`observe_instances`](instance_checks::observe_instances); the thresholds that turn a
/// run of them into a verdict are the load balancer's own.
pub const HEALTH_CHECK_SPAN: std::time::Duration = std::time::Duration::from_secs(3);

/// One listener of a load balancer, keyed logically by its `LoadBalancerPort`.
#[derive(Debug, Clone)]
pub struct ListenerEntry {
    pub protocol: String,
    pub load_balancer_port: i32,
    /// Defaults to `protocol` when the create request omitted it.
    pub instance_protocol: String,
    pub instance_port: i32,
    pub ssl_certificate_id: Option<String>,
    pub policy_names: Vec<String>,
}

impl ListenerEntry {
    /// Whether `other` asks for the SAME listener this one already configures.
    ///
    /// The comparison is the one `DuplicateListenerException` is stated over — "A
    /// listener already exists for the specified load balancer name and port, but with a
    /// different instance port, protocol, or SSL certificate" — so a request that repeats
    /// a configured listener exactly is a no-op rather than a conflict, and one that
    /// differs in any of those is the conflict that error names. `policy_names` is not
    /// part of it: a listener's policies are set by other operations, not by the create.
    pub fn configures_the_same(&self, other: &Self) -> bool {
        self.protocol == other.protocol
            && self.load_balancer_port == other.load_balancer_port
            && self.instance_protocol == other.instance_protocol
            && self.instance_port == other.instance_port
            && self.ssl_certificate_id == other.ssl_certificate_id
    }
}

/// Why a requested listener cannot be stored. Each variant is one rule the model states;
/// the OPERATION maps it onto the modelled error it declares for that rule, because the
/// two operations that create listeners declare different error sets.
#[derive(Debug, Clone)]
pub enum ListenerRejection {
    /// A `Protocol`/`InstanceProtocol` outside [`LISTENER_PROTOCOLS`], naming the member
    /// that carried it — which is what the caller has to fix.
    UnsupportedProtocol { member: &'static str },
    /// A back-end protocol from the other transport family than the front-end one.
    IncompatibleProtocols {
        protocol: String,
        instance_protocol: String,
    },
}

/// Project one requested listener onto the record this service stores, applying the two
/// rules the model states over a listener's protocols.
///
/// `instance_protocol` of `None` takes the documented default ("The protocol to use for
/// routing traffic to instances" defaults to the front-end protocol), and both protocols
/// are stored upper-cased, the spelling the model's own documentation and
/// `DescribeLoadBalancers` use.
pub fn listener_entry(
    protocol: &str,
    load_balancer_port: i32,
    instance_protocol: Option<&str>,
    instance_port: i32,
    ssl_certificate_id: Option<&str>,
) -> Result<ListenerEntry, ListenerRejection> {
    let protocol = protocol.to_ascii_uppercase();
    let instance_protocol =
        instance_protocol.map_or_else(|| protocol.clone(), str::to_ascii_uppercase);

    for (member, candidate) in [("Protocol", &protocol), ("InstanceProtocol", &instance_protocol)]
    {
        if !LISTENER_PROTOCOLS.contains(&candidate.as_str()) {
            return Err(ListenerRejection::UnsupportedProtocol { member });
        }
    }
    // From `Listener$InstanceProtocol`: "If the front-end protocol is TCP or SSL, the
    // back-end protocol must be TCP or SSL. If the front-end protocol is HTTP or HTTPS,
    // the back-end protocol must be HTTP or HTTPS."
    if transport_family(&protocol) != transport_family(&instance_protocol) {
        return Err(ListenerRejection::IncompatibleProtocols {
            protocol,
            instance_protocol,
        });
    }

    Ok(ListenerEntry {
        protocol,
        load_balancer_port,
        instance_protocol,
        instance_port,
        ssl_certificate_id: ssl_certificate_id.map(str::to_string),
        policy_names: Vec::new(),
    })
}

/// `HTTP`/`HTTPS` and `TCP`/`SSL` are the two families the model's compatibility rule is
/// stated over; a secure protocol belongs to the same family as its plain peer.
fn transport_family(protocol: &str) -> &'static str {
    match protocol {
        "HTTP" | "HTTPS" => "http",
        _ => "tcp",
    }
}

/// The maximum number of policies one load balancer holds.
///
/// AWS publishes no quota for policies per load balancer, and every policy is minted by
/// a CALLER, so this is a bound the emulator chooses to keep the store one caller can
/// grow finite. `TooManyPoliciesException` — "The quota for the number of policies for
/// this load balancer has been reached" — is the error every policy-creating operation
/// models for reaching it.
pub const MAX_POLICIES_PER_LOAD_BALANCER: usize = 100;

/// The `CookieExpirationPeriod` a duration-based stickiness policy gets when the create
/// request names none, from `CreateLBCookieStickinessPolicy`'s own documentation: "If you
/// do not specify this parameter, the default value is 0, which indicates that the sticky
/// session should last for the duration of the browser session."
pub const DEFAULT_COOKIE_EXPIRATION_PERIOD_SECONDS: i64 = 0;

/// What one stored policy configures: one variant per policy-creating operation, so the
/// list a describe projects a policy into follows from the stored record rather than from
/// a type string a handler wrote.
#[derive(Debug, Clone)]
pub enum PolicyKind {
    /// Created by `CreateLBCookieStickinessPolicy`: duration-based session stickiness,
    /// whose cookie goes stale after `cookie_expiration_period` seconds — 0 meaning "the
    /// duration of the browser session", the documented default.
    LbCookieStickiness { cookie_expiration_period: i64 },
    /// Created by `CreateAppCookieStickinessPolicy`: application-controlled session
    /// stickiness, whose sticky session follows the application cookie named by
    /// `cookie_name` ("The name of the application cookie used for stickiness").
    AppCookieStickiness { cookie_name: String },
    /// Created by `CreateLoadBalancerPolicy`: a policy of one of the policy types this
    /// service predefines ([`POLICY_TYPE_NAMES`]), carrying the attributes the create
    /// named. `Policies$OtherPolicies` is "the policies other than the stickiness
    /// policies", and the two stickiness members are the policies created by the two
    /// stickiness operations, so a policy stored under this variant is one a describe
    /// reports there.
    Other {
        policy_type_name: String,
        attributes: Vec<PolicyAttributeEntry>,
    },
}

impl PolicyKind {
    /// The `PolicyTypeName` a policy of this kind is of — the base policy type a describe
    /// reports it under.
    ///
    /// A stickiness policy's base type is the service's own (the two stickiness operations
    /// each create a policy of one predefined type), while a policy created by
    /// `CreateLoadBalancerPolicy` carries "the name of the base policy type" that create
    /// named. The `match` is exhaustive, so a new policy kind has to say which type it is
    /// of rather than being reported under none.
    pub fn policy_type_name(&self) -> &str {
        match self {
            PolicyKind::LbCookieStickiness { .. } => LB_COOKIE_STICKINESS_POLICY_TYPE,
            PolicyKind::AppCookieStickiness { .. } => APP_COOKIE_STICKINESS_POLICY_TYPE,
            PolicyKind::Other {
                policy_type_name, ..
            } => policy_type_name,
        }
    }

    /// The attributes this policy is configured by, as a describe reports them.
    ///
    /// A policy is configured by the attributes of its base policy type, so each
    /// stickiness kind reports the one attribute its create operation takes — named by
    /// that operation's own input member ([`COOKIE_EXPIRATION_PERIOD_ATTRIBUTE`],
    /// [`COOKIE_NAME_ATTRIBUTE`]) and carrying the value it was created with — and a
    /// policy created by `CreateLoadBalancerPolicy` reports the attributes it was created
    /// with, whole.
    pub fn attribute_entries(&self) -> Vec<PolicyAttributeEntry> {
        match self {
            PolicyKind::LbCookieStickiness {
                cookie_expiration_period,
            } => vec![PolicyAttributeEntry {
                name: COOKIE_EXPIRATION_PERIOD_ATTRIBUTE.to_string(),
                value: Some(cookie_expiration_period.to_string()),
            }],
            PolicyKind::AppCookieStickiness { cookie_name } => vec![PolicyAttributeEntry {
                name: COOKIE_NAME_ATTRIBUTE.to_string(),
                value: Some(cookie_name.clone()),
            }],
            PolicyKind::Other { attributes, .. } => attributes.clone(),
        }
    }
}

/// The attribute a duration-based stickiness policy is configured by: the period after
/// which its cookie goes stale, in seconds, named as `CreateLBCookieStickinessPolicy`'s
/// own input member names it.
pub const COOKIE_EXPIRATION_PERIOD_ATTRIBUTE: &str = "CookieExpirationPeriod";

/// The attribute an application-controlled stickiness policy is configured by: the
/// application cookie the sticky session follows, named as
/// `CreateAppCookieStickinessPolicy`'s own input member names it.
pub const COOKIE_NAME_ATTRIBUTE: &str = "CookieName";

/// One attribute of a stored policy, as `PolicyAttribute` carries it: "The name of the
/// attribute" and "The value of the attribute", the value left unreported where the
/// request named none.
///
/// Persisted by `CreateLoadBalancerPolicy` so the policy a caller created is stored whole
/// — which is what `DescribeLoadBalancerPolicies` reports as a policy's
/// `PolicyAttributeDescriptions`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PolicyAttributeEntry {
    pub name: String,
    pub value: Option<String>,
}

/// The policy types Elastic Load Balancing predefines for a Classic load balancer — the
/// `PolicyTypeName`s `DescribeLoadBalancerPolicyTypes` reports and
/// `CreateLoadBalancerPolicy` accepts as "the name of the base policy type".
///
/// Source: the policy types the Classic Load Balancer documentation publishes for that
/// operation (application- and duration-based stickiness, back-end server
/// authentication, public key, SSL negotiation and Proxy Protocol). The model declares
/// `PolicyTypeName` a plain string with no enum, so nothing has refused a name no policy
/// type carries by the time a handler runs; [`is_known_policy_type`] is that rule.
pub const POLICY_TYPE_NAMES: &[&str] = &[
    APP_COOKIE_STICKINESS_POLICY_TYPE,
    "BackendServerAuthenticationPolicyType",
    LB_COOKIE_STICKINESS_POLICY_TYPE,
    "ProxyProtocolPolicyType",
    "PublicKeyPolicyType",
    SSL_NEGOTIATION_POLICY_TYPE,
];

/// The policy type of the policies `CreateAppCookieStickinessPolicy` creates:
/// application-controlled session stickiness.
pub const APP_COOKIE_STICKINESS_POLICY_TYPE: &str = "AppCookieStickinessPolicyType";

/// The policy type of the policies `CreateLBCookieStickinessPolicy` creates:
/// duration-based session stickiness.
pub const LB_COOKIE_STICKINESS_POLICY_TYPE: &str = "LBCookieStickinessPolicyType";

/// The policy type of an SSL negotiation configuration — the type the sample policies
/// ([`SAMPLE_POLICIES`]) are of.
pub const SSL_NEGOTIATION_POLICY_TYPE: &str = "SSLNegotiationPolicyType";

/// The prefix the name of every sample policy carries, from
/// `DescribeLoadBalancerPolicies`' own documentation: "The names of the sample policies
/// have the ELBSample- prefix."
pub const SAMPLE_POLICY_NAME_PREFIX: &str = "ELBSample-";

/// The SSL negotiation configurations this service predefines, and the attribute set every
/// one of them is described by: the catalog `DescribeLoadBalancerPolicies` answers a
/// request that names no load balancer from.
mod sample_policies;
pub use sample_policies::*;

/// Whether `policy_type_name` names one of the policy types this service predefines.
///
/// A policy's base type is the service's own, not the caller's: a create naming a type
/// outside [`POLICY_TYPE_NAMES`] earns `PolicyTypeNotFoundException`.
pub fn is_known_policy_type(policy_type_name: &str) -> bool {
    POLICY_TYPE_NAMES.contains(&policy_type_name)
}

/// The largest `PolicyAttributes` list one `CreateLoadBalancerPolicy` request may carry.
///
/// AWS publishes no quota for the member, so this is a bound the emulator chooses: it
/// keeps the state one request can grow finite while staying comfortably above the
/// attribute count of the largest predefined policy type, whose protocol and cipher
/// attributes a caller sets one per attribute.
pub const MAX_POLICY_ATTRIBUTES_PER_POLICY: usize = 200;

/// One policy of a load balancer, as a policy-creating operation persists it and
/// `DescribeLoadBalancers` reports it.
///
/// A policy name "must be unique within the set of policies for this load balancer"
/// (`PolicyName`'s own documentation), so EVERY policy of one load balancer lives in the
/// one [`LoadBalancer::policies`] map whatever created it: a second map beside it would
/// let two creates mint two policies under one name.
#[derive(Debug, Clone)]
pub struct PolicyRecord {
    pub name: String,
    pub kind: PolicyKind,
}

/// Why `name` is not a policy name, or `Ok(())` when it is one.
///
/// From `PolicyName`'s documentation on every operation that creates a policy: "Policy
/// names must consist of alphanumeric characters and dashes (-)." The model declares the
/// member a plain string with no `@pattern` or `@length`, so nothing has refused a
/// malformed name by the time a handler runs. The message names the member and the rule
/// it broke, which is what a caller fixes the request from.
pub fn policy_name_rejection(name: &str) -> Result<(), String> {
    if name.is_empty() {
        return Err("PolicyName must not be empty".to_string());
    }
    match name
        .chars()
        .find(|character| !(character.is_ascii_alphanumeric() || *character == '-'))
    {
        Some(rejected) => Err(format!(
            "PolicyName must consist of alphanumeric characters and dashes (-); \
             '{name}' contains '{rejected}'"
        )),
        None => Ok(()),
    }
}

/// How this service states `DuplicatePolicyNameException` — "A policy with the specified
/// name already exists for this load balancer" — naming the policy name the caller has to
/// change.
///
/// Here, once, because a policy name is unique across the WHOLE policy set of one load
/// balancer: every policy-creating operation refuses a taken name, and all of them state
/// that refusal the same way.
pub fn duplicate_policy_name_message(policy_name: &str) -> String {
    format!("Policy Name [{policy_name}] already exists.")
}

/// How this service states `PolicyNotFoundException` — "One or more of the specified
/// policies do not exist" — naming the policy the request asked for and, where the
/// request named a load balancer, the load balancer whose policy set does not hold it.
///
/// A policy is a policy OF a load balancer, so which policies exist depends on which load
/// balancer was asked about: a request naming none asks about the sample policies the
/// service predefines, and its refusal names the policy alone.
pub fn policy_not_found_message(policy_name: &str, load_balancer_name: Option<&str>) -> String {
    match load_balancer_name {
        Some(load_balancer_name) => format!(
            "There is no policy with name {policy_name} for load balancer \
             {load_balancer_name}."
        ),
        None => format!("There is no policy with name {policy_name}."),
    }
}

/// A load balancer's health-check configuration.
///
/// Seeded at create with the Classic Load Balancer defaults so
/// `DescribeLoadBalancers` reports a real `HealthCheck` before anyone calls
/// `ConfigureHealthCheck`.
#[derive(Debug, Clone)]
pub struct HealthCheckConfig {
    pub target: String,
    pub interval: i32,
    pub timeout: i32,
    pub unhealthy_threshold: i32,
    pub healthy_threshold: i32,
}

impl HealthCheckConfig {
    /// The default health check of a newly created load balancer: a TCP check against
    /// the instance port of its first listener.
    pub fn default_for_instance_port(instance_port: i32) -> Self {
        Self {
            target: format!("TCP:{instance_port}"),
            interval: 30,
            timeout: 5,
            unhealthy_threshold: 2,
            healthy_threshold: 10,
        }
    }
}

/// The idle connection timeout, in seconds, of a load balancer nobody has modified.
///
/// From `LoadBalancerAttributes$ConnectionSettings`' own documentation: "By default,
/// Elastic Load Balancing maintains a 60-second idle connection timeout for both
/// front-end and back-end connections of your load balancer."
pub const DEFAULT_IDLE_TIMEOUT_SECONDS: i32 = 60;

/// The `ConnectionSettings$IdleTimeout` bounds, which the model declares as
/// `@range(min: 1, max: 3600)`.
pub const IDLE_TIMEOUT_SECONDS: (i32, i32) = (1, 3600);

/// Whether connection draining is enabled on a load balancer nobody has modified, from
/// `ConnectionDraining$Enabled`'s modelled `@default(false)`.
pub const DEFAULT_CONNECTION_DRAINING_ENABLED: bool = false;

/// The connection-draining timeout, in seconds, of a load balancer nobody has modified.
///
/// "The default timeout is 300 seconds" — the "Configure Connection Draining for Your
/// Classic Load Balancer" page of the Classic Load Balancers Guide, which
/// `ConnectionDraining`'s documentation links to. The value is AWS's, not one this
/// emulator chose. Whether draining is ON by default is a separate question, answered by
/// [`DEFAULT_CONNECTION_DRAINING_ENABLED`] from the model's own `@default(false)`.
pub const DEFAULT_CONNECTION_DRAINING_TIMEOUT_SECONDS: i32 = 300;

/// The `ConnectionDraining$Timeout` bounds. The model declares no range for the member,
/// so these are the bounds the same guide publishes ("a value between 1 and 3,600
/// seconds").
pub const CONNECTION_DRAINING_TIMEOUT_SECONDS: (i32, i32) = (1, 3600);

/// The refusal a connection timeout outside its bounds earns, stated over the BOUND rather
/// than over the value the request carried: the caller fixes the value from the bound, and
/// the two timeout attributes of a load balancer ([`IDLE_TIMEOUT_SECONDS`] and
/// [`CONNECTION_DRAINING_TIMEOUT_SECONDS`]) carry the same one, so one sentence states it
/// for either.
pub fn timeout_bound_message((low, high): (i32, i32)) -> String {
    format!("Time out must be at least {low} second and no more than {high} seconds")
}

/// The access-log publishing interval, in minutes, of a load balancer nobody has
/// modified. From `AccessLog$EmitInterval`: "Default: 60 minutes".
pub const DEFAULT_ACCESS_LOG_EMIT_INTERVAL_MINUTES: i32 = 60;

/// The only two intervals `AccessLog$EmitInterval` accepts: "You can specify an interval
/// of either 5 minutes or 60 minutes."
pub const ACCESS_LOG_EMIT_INTERVALS_MINUTES: &[i32] = &[5, 60];

/// The one additional attribute this service documents, from
/// `AdditionalAttribute$Key`: "`elb.http.desyncmitigationmode` - Determines how the load
/// balancer handles requests that might pose a security risk to your application."
pub const DESYNC_MITIGATION_MODE_KEY: &str = "elb.http.desyncmitigationmode";

/// Its accepted values, from the same sentence: "The possible values are `monitor`,
/// `defensive`, and `strictest`."
pub const DESYNC_MITIGATION_MODES: &[&str] = &["monitor", "defensive", "strictest"];

/// Its value on a load balancer nobody has modified: "The default is `defensive`."
pub const DEFAULT_DESYNC_MITIGATION_MODE: &str = "defensive";

/// A load balancer's `AccessLog` attribute.
#[derive(Debug, Clone)]
pub struct AccessLogConfig {
    pub enabled: bool,
    pub emit_interval_minutes: i32,
    pub s3_bucket_name: Option<String>,
    pub s3_bucket_prefix: Option<String>,
}

/// A load balancer's `ConnectionDraining` attribute.
#[derive(Debug, Clone)]
pub struct ConnectionDrainingConfig {
    pub enabled: bool,
    pub timeout_seconds: i32,
}

/// The attributes of one load balancer: every member
/// `DescribeLoadBalancerAttributes` reports and `ModifyLoadBalancerAttributes` writes.
///
/// Seeded at create with the service defaults, so a freshly created load balancer
/// reports the same attribute set AWS reports for one — rather than an empty document —
/// and a modification of one attribute leaves the others at whatever they held.
#[derive(Debug, Clone)]
pub struct AttributeConfig {
    pub cross_zone_load_balancing_enabled: bool,
    pub access_log: AccessLogConfig,
    pub connection_draining: ConnectionDrainingConfig,
    pub idle_timeout_seconds: i32,
    /// The `AdditionalAttributes` entries, keyed so one key holds one value and the list
    /// is reported in a stable order. Bounded by the member's modelled `@length(max: 10)`.
    pub additional: BTreeMap<String, String>,
}

impl Default for AttributeConfig {
    /// The attributes of a load balancer nobody has modified.
    fn default() -> Self {
        Self {
            // `CrossZoneLoadBalancing$Enabled` is modelled `@default(false)`.
            cross_zone_load_balancing_enabled: false,
            access_log: AccessLogConfig {
                // `AccessLog$Enabled` is modelled `@default(false)`.
                enabled: false,
                emit_interval_minutes: DEFAULT_ACCESS_LOG_EMIT_INTERVAL_MINUTES,
                s3_bucket_name: None,
                s3_bucket_prefix: None,
            },
            connection_draining: ConnectionDrainingConfig {
                enabled: DEFAULT_CONNECTION_DRAINING_ENABLED,
                timeout_seconds: DEFAULT_CONNECTION_DRAINING_TIMEOUT_SECONDS,
            },
            idle_timeout_seconds: DEFAULT_IDLE_TIMEOUT_SECONDS,
            additional: BTreeMap::from([(
                DESYNC_MITIGATION_MODE_KEY.to_string(),
                DEFAULT_DESYNC_MITIGATION_MODE.to_string(),
            )]),
        }
    }
}

/// One Classic load balancer, as persisted by `CreateLoadBalancer` and read back by
/// every other operation.
#[derive(Debug, Clone)]
pub struct LoadBalancer {
    pub name: String,
    /// Generated once at create and kept for the record's lifetime.
    pub dns_name: String,
    pub canonical_hosted_zone_name: String,
    pub canonical_hosted_zone_name_id: String,
    pub scheme: String,
    pub created_time: SystemTime,
    pub listeners: Vec<ListenerEntry>,
    /// The zones a request named DIRECTLY, through `CreateLoadBalancer`'s
    /// `AvailabilityZones`. The zones the load balancer's SUBNETS put it in are
    /// [`LoadBalancer::subnet_zones`]; [`LoadBalancer::zones`] is the whole set it is in.
    pub availability_zones: Vec<String>,
    /// The subnets the load balancer is attached to, in attachment order, deduplicated.
    pub subnets: Vec<String>,
    /// The Availability Zone of each attached subnet, as EC2 reported it, keyed by subnet
    /// id.
    ///
    /// Which zone a subnet is in is EC2's to say, so it is resolved against EC2 when the
    /// subnet is attached and persisted here beside the subnet that produced it: a later
    /// read reports the same zone without asking again, and detaching the subnet takes its
    /// zone with it. A subnet EC2 could not be asked about holds no entry — the zone is
    /// left unreported rather than invented.
    pub subnet_zones: BTreeMap<String, String>,
    pub security_groups: Vec<String>,
    /// The security group `DescribeLoadBalancers` reports as this load balancer's SOURCE
    /// security group, as EC2 described the first group the load balancer was given.
    ///
    /// A security group is EC2's resource, so the name and the owner a load balancer
    /// reports it under are EC2's to say: they are resolved when the groups are associated
    /// and persisted here beside the ids that produced them, so a later read reports the
    /// same facts without asking again. `None` where there was no EC2 to ask, or where the
    /// load balancer carries no security group — the member is left unreported rather than
    /// invented.
    pub source_security_group: Option<SourceSecurityGroup>,
    pub vpc_id: Option<String>,
    /// Registered `InstanceId`s, deduplicated, in registration order, bounded by
    /// [`MAX_INSTANCES_PER_LOAD_BALANCER`].
    pub instances: InstanceSet,
    pub health_check: HealthCheckConfig,
    /// Every policy created for this load balancer, keyed by `PolicyName` so one name
    /// holds one policy and a describe reports them in one stable order. Bounded by
    /// [`MAX_POLICIES_PER_LOAD_BALANCER`].
    pub policies: BTreeMap<String, PolicyRecord>,
    /// The modifiable attributes `DescribeLoadBalancerAttributes` reports and
    /// `ModifyLoadBalancerAttributes` writes, seeded with the service defaults.
    pub attributes: AttributeConfig,
    pub tags: BTreeMap<String, String>,
}

impl LoadBalancer {
    /// Every Availability Zone this load balancer is in, deduplicated: the zones a request
    /// named directly, then the zone of each attached subnet in attachment order.
    ///
    /// Zone membership FOLLOWS the placement rather than being a second list beside it, so
    /// `DescribeLoadBalancers` reports the zones of the subnets the load balancer is
    /// attached to now — whichever operation attached or detached them.
    pub fn zones(&self) -> Vec<String> {
        let from_subnets = self
            .subnets
            .iter()
            .filter_map(|subnet_id| self.subnet_zones.get(subnet_id));
        let mut seen: BTreeSet<&str> = BTreeSet::new();
        let mut reported: Vec<String> = Vec::new();
        for zone in self.availability_zones.iter().chain(from_subnets) {
            if seen.insert(zone.as_str()) {
                reported.push(zone.clone());
            }
        }
        reported
    }

    /// Attach `subnet_id`, remembering the zone EC2 reported for it.
    ///
    /// Attaching one the load balancer already holds re-states the same placement: the set
    /// keeps its order and its length, and only the zone is refreshed.
    pub fn attach_subnet(&mut self, subnet_id: &str, zone: Option<&str>) {
        if let Some(zone) = zone {
            self.subnet_zones
                .insert(subnet_id.to_string(), zone.to_string());
        }
        if !self.subnets.iter().any(|held| held == subnet_id) {
            self.subnets.push(subnet_id.to_string());
        }
    }

    /// Add `zone` to the zones a request named DIRECTLY.
    ///
    /// The counterpart of [`LoadBalancer::attach_subnet`] for the placement that names a
    /// zone rather than a subnet: a zone the load balancer is already in — named directly
    /// before, or carried by one of its subnets — re-states the same placement, so the set
    /// keeps its order and its length and [`LoadBalancer::zones`] reports the same zones
    /// as before.
    pub fn enable_zone(&mut self, zone: &str) {
        if self.zones().iter().any(|held| held == zone) {
            return;
        }
        self.availability_zones.push(zone.to_string());
    }

    /// Detach `subnet_id`, and with it the zone it put the load balancer in. A subnet the
    /// load balancer is not attached to removes nothing.
    pub fn detach_subnet(&mut self, subnet_id: &str) {
        self.subnet_zones.remove(subnet_id);
        self.subnets.retain(|candidate| candidate != subnet_id);
    }

    /// Register `instance_id` as of `at`, keeping the set deduplicated and in
    /// registration order. Answers whether the id was newly added.
    pub fn register_instance(&mut self, instance_id: &str, at: SystemTime) -> bool {
        self.instances.insert(instance_id, at)
    }

    /// The health of `instance_id` with respect to this load balancer: the OUTCOME of the
    /// checks this load balancer has run against it.
    ///
    /// An instance takes traffic only while it passes this load balancer's health check,
    /// so what is reported is the verdict its checks have reached
    /// ([`HealthProbe::reported_health`]) — in service once `HealthyThreshold` consecutive
    /// checks were answered, out of service with the instance as the cause once
    /// `UnhealthyThreshold` consecutive checks were not, and the registration's own checks
    /// still running until one of those thresholds is reached. An instance that is not
    /// registered takes no traffic for a reason of Elastic Load Balancing's own, and no
    /// check of it is run at all.
    ///
    /// A read reports what the checks that have already fallen due decided, which is what
    /// [`LoadBalancer::settle_instance_health`] runs.
    pub fn instance_health(&self, instance_id: &str) -> InstanceHealth {
        match self.instances.probe(instance_id) {
            Some(probe) => probe.reported_health(),
            None => HEALTH_NOT_REGISTERED,
        }
    }

    /// Run every health check of this load balancer's registrations that the clock has
    /// made due at `now`, folding what each one observed ([`observe_instances`]) into the
    /// verdict it moves.
    ///
    /// Materialises only what the clock already owed: the checks were due at `now`
    /// whether or not anybody called, and `observed` says what each of them saw, so the
    /// verdict a caller reads does not depend on who read it. An id `observed` carries no
    /// entry for is one no check was run for.
    pub fn settle_instance_health(
        &mut self,
        observed: &BTreeMap<String, Option<InstanceObservation>>,
        now: SystemTime,
    ) {
        let thresholds = HealthThresholds::of(&self.health_check);
        for (instance_id, observation) in observed {
            if let Some(probe) = self.instances.probe_mut(instance_id) {
                probe.settle(*observation, thresholds, now);
            }
        }
    }

    /// Deregister every id in `ids`. Answers how many had been registered.
    pub fn deregister_instances(&mut self, ids: &BTreeSet<String>) -> usize {
        self.instances.remove_all(ids)
    }

    /// Whether `instance_id` is currently registered with this load balancer.
    pub fn has_instance(&self, instance_id: &str) -> bool {
        self.instances.contains(instance_id)
    }

    /// How many ids of `requested` are not registered yet — what registering them all
    /// would add to the stored set.
    pub fn new_instance_count<'a>(
        &self,
        requested: impl IntoIterator<Item = &'a String>,
    ) -> usize {
        let mut unseen: BTreeSet<&str> = BTreeSet::new();
        for id in requested {
            if !self.instances.contains(id) {
                unseen.insert(id.as_str());
            }
        }
        unseen.len()
    }
}

/// Per-service state for com-amazonaws-elasticloadbalancing-elasticloadbalancing-v7. A handler reaches it through [`service_state`].
///
/// A load balancer is a REGIONAL resource: one `LoadBalancerName` is unique within one
/// region (the same name may name a different load balancer in another), a describe
/// signed for one region does not see another's, and the documented "20 load balancers
/// per region per account" quota is counted per region. The store is therefore keyed by
/// region first, and each region's own map is keyed by name.
#[derive(Debug, Default)]
pub struct ComAmazonawsElasticloadbalancingElasticloadbalancingV7State {
    /// Region -> (`LoadBalancerName` -> record). `BTreeMap`s throughout, so
    /// `DescribeLoadBalancers` paginates one stable order.
    pub regions: BTreeMap<String, BTreeMap<String, LoadBalancer>>,
    /// The socket each load balancer's own address is served on, keyed by the region and
    /// the name of the load balancer it belongs to ([`LoadBalancerEndpoint`]).
    ///
    /// Held here rather than on [`LoadBalancer`] because it is a LIVE socket: the record is
    /// cloned by every read, and a socket is not a value that may be copied — dropping a
    /// handle closes the port. The record keeps the ADDRESS the socket answers on, as its
    /// `dns_name`, which is what every reader reports.
    pub endpoints: BTreeMap<(String, String), LoadBalancerEndpoint>,
}

impl ComAmazonawsElasticloadbalancingElasticloadbalancingV7State {
    /// The load balancers of `region`, or `None` when it holds none.
    pub fn load_balancers_in(&self, region: &str) -> Option<&BTreeMap<String, LoadBalancer>> {
        self.regions.get(region)
    }

    /// The load balancers of `region` for mutation, or `None` when it holds none. Does
    /// NOT create the region's map, so a read or a no-op delete leaves the store as it
    /// found it.
    pub fn load_balancers_in_mut(
        &mut self,
        region: &str,
    ) -> Option<&mut BTreeMap<String, LoadBalancer>> {
        self.regions.get_mut(region)
    }

    /// The load balancers of `region` for insertion, creating the region's (empty) map
    /// if this is the first record stored there. For a create path only, AFTER the
    /// request is known to be acceptable.
    ///
    /// `None` when the store already holds [`MAX_REGIONS`] regions and `region` is not
    /// one of them: the region comes from the caller's own credential scope, so the
    /// number of per-region budgets it can open is capped.
    pub fn region_entry(
        &mut self,
        region: &str,
    ) -> Option<&mut BTreeMap<String, LoadBalancer>> {
        if !self.regions.contains_key(region) && self.regions.len() >= MAX_REGIONS {
            return None;
        }
        Some(self.regions.entry(region.to_string()).or_default())
    }

    /// Remember the socket one load balancer's own address is served on, answering the
    /// `host:port` the load balancer then publishes as its `DNSName`.
    ///
    /// A load balancer that already holds an address keeps it: the socket already open is
    /// the one every published name points at, and the one handed here is dropped with its
    /// port.
    pub fn remember_endpoint(
        &mut self,
        region: &str,
        load_balancer_name: &str,
        endpoint: LoadBalancerEndpoint,
    ) -> String {
        self.endpoints
            .entry((region.to_string(), load_balancer_name.to_string()))
            .or_insert(endpoint)
            .authority()
            .to_string()
    }

    /// Close the socket one load balancer's address was served on, if it had one: dropping
    /// the handle closes the port and severs every connection in flight on it, which is
    /// what a deleted load balancer's address does.
    pub fn forget_endpoint(&mut self, region: &str, load_balancer_name: &str) {
        self.endpoints
            .remove(&(region.to_string(), load_balancer_name.to_string()));
    }
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsElasticloadbalancingElasticloadbalancingV7State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsElasticloadbalancingElasticloadbalancingV7State>)
}

/// The message a detach that would leave a load balancer attached to no subnet at all is
/// refused with.
///
/// `InvalidConfigurationRequest` is the refusal of every configuration rule a load balancer
/// has, so the message is what tells the rules apart: it names the load balancer the
/// request asked about and states the rule the change would have broken — a load balancer
/// in a VPC serves traffic from its subnets, so it cannot be left attached to none.
pub fn detached_every_subnet_message(load_balancer_name: &str) -> String {
    format!(
        "Requested configuration change for LoadBalancer \"{load_balancer_name}\" is invalid \
         because you attempted to detach all the subnets for this LoadBalancer and a \
         LoadBalancer cannot be attached to zero subnets in VPC."
    )
}

/// The message `CreateLoadBalancer` answers a `DuplicateLoadBalancerName` with.
///
/// A load balancer name is unique "within your set of load balancers for the region"
/// (`AccessPointName`'s own documentation), and the modelled
/// `DuplicateAccessPointNameException` is documented "The specified load balancer name
/// already exists for this account" — so the refusal names the load balancer the request
/// asked for and states the rule that produced it: the name is held by a load balancer
/// configured with parameters other than the ones this request carried.
pub fn duplicate_load_balancer_name_message(name: &str) -> String {
    format!("Load Balancer named {name} already exists and it is configured with different parameters.")
}

/// The message a listener-creating operation answers a `DuplicateListener` with.
///
/// `DuplicateListenerException` is documented "A listener already exists for the specified
/// load balancer name and port, but with a different instance port, protocol, or SSL
/// certificate", so the refusal names the load balancer and the `LoadBalancerPort` the
/// conflict is on and states WHICH of the listener's members the request may differ in —
/// the three that rule names. The stored listener's own settings are not it: a caller
/// reads this message to learn why the request was refused, and the request is what it
/// can fix.
pub fn duplicate_listener_message(load_balancer_name: &str, load_balancer_port: i32) -> String {
    format!(
        "A listener already exists for {load_balancer_name} with LoadBalancerPort \
         {load_balancer_port}, but with a different InstancePort, Protocol, or \
         SSLCertificateId"
    )
}

/// The security group a load balancer reports as its source security group: the group's
/// own name, and the account that owns it.
///
/// `SourceSecurityGroup` is documented as "Information about a source security group" with
/// `OwnerAlias` ("The owner of the security group") and `GroupName` ("The name of the
/// security group"), neither of which is the group ID a request names — so both are read
/// off what EC2 reports for that group, and a member EC2 reported nothing for stays
/// unreported.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct SourceSecurityGroup {
    pub owner_alias: Option<String>,
    pub group_name: Option<String>,
}

/// How Amazon EC2 names itself in the context clause of a fault reported through another
/// service's API.
const EC2_SERVICE_NAME: &str = "AmazonEC2";

/// The status a relayed context clause states when the answer that carried the refusal
/// reached this service without one: a read of the account's network is refused for the ids
/// the caller sent, which is a client fault.
const RELAYED_REFUSAL_STATUS: u16 = 400;

/// The MESSAGE this service states when a request names a subnet or a security group the
/// account holds no such resource for: the EC2 fault it ran into, RELAYED WHOLE.
///
/// Both refusals have one root — a read of an EC2-owned resource EC2 would not resolve — so
/// both state it the same way, which is why the wording lives here rather than in each
/// handler. What a caller reads is the fault as the SDK of the service that raised it
/// renders it: the sentence EC2 worded about the ids this request named, and the context of
/// the call that produced it — which service answered, the status and the error code it
/// answered with, and the id of that call. Neither half alone is the message: the sentence
/// without the context is EC2's refusal passed off as this service's own, and the context
/// without the sentence names nothing the caller sent.
///
/// Every part comes from the answer EC2 gave for THIS request, except the call's own request
/// id, which is minted per call like every other request id — never derived from the request
/// and never stored.
pub fn relayed_network_refusal_message(refusal: &NetworkRefusal) -> String {
    format!(
        "{} (Service: {EC2_SERVICE_NAME}; Status Code: {}; Error Code: {}; Request ID: {}; \
         Proxy: null)",
        refusal.reported_message(),
        refusal.status.unwrap_or(RELAYED_REFUSAL_STATUS),
        refusal.code,
        ::service_common::query_error_envelope::new_request_id()
    )
}

/// What EC2 says about the subnets a request named: the Availability Zone of each subnet
/// it reported one for, and the VPC they all belong to.
#[derive(Debug, Clone, Default)]
pub struct SubnetPlacement {
    /// Subnet id -> the Availability Zone EC2 reports it is in. A subnet EC2 reported no
    /// zone for, or could not be asked about, is absent.
    pub zones: BTreeMap<String, String>,
    /// The VPC the subnets belong to, reported only when every resolved subnet names the
    /// same one — the only placement AWS creates at all.
    pub vpc_id: Option<String>,
}

/// Resolve `subnet_ids` against EC2, which owns them, and answer the placement facts a
/// load balancer attached to them reports — or the EC2 refusal they ran into.
///
/// A subnet is not this service's resource: whether it exists at all, which zone it is in
/// and which VPC it belongs to are EC2's to say, so they are read from EC2 through the
/// in-process callback. `Err` carries the REFUSAL EC2 answered for an id the account
/// holds no subnet for — its code and the sentence EC2 worded it with, which names that id
/// — and the caller states it in the refusal ITS model declares. An
/// emulator with no EC2 mounted says NOTHING: the placement comes back empty and the
/// members it would have filled are left unreported rather than invented — and nothing is
/// refused on that silence, because this service never learned anything to refuse it on.
pub async fn resolve_subnet_placement(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    subnet_ids: &[String],
) -> Result<SubnetPlacement, NetworkRefusal> {
    let resolved = match elbv2::state::network::resolve_subnets(ctx, region, subnet_ids).await {
        elbv2::state::network::SubnetLookup::Resolved(resolved) => resolved,
        elbv2::state::network::SubnetLookup::Absent(refusal) => return Err(refusal),
        elbv2::state::network::SubnetLookup::Unknown => return Ok(SubnetPlacement::default()),
    };
    let zones = resolved
        .iter()
        .filter_map(|subnet| {
            let zone = subnet.availability_zone.clone()?;
            Some((subnet.subnet_id.clone(), zone))
        })
        .collect();
    let first = resolved.first().and_then(|subnet| subnet.vpc_id.clone());
    let vpc_id = first.filter(|vpc_id| {
        resolved
            .iter()
            .all(|subnet| subnet.vpc_id.as_deref() == Some(vpc_id.as_str()))
    });
    Ok(SubnetPlacement { zones, vpc_id })
}

/// Resolve the security groups a request associates with a load balancer against EC2,
/// which owns them, and answer the source security group the load balancer then reports —
/// or the EC2 refusal they ran into.
///
/// A security group is EC2's resource, so whether one exists is EC2's to say: `Err` carries
/// the REFUSAL EC2 answered for an id the account holds no group for — its code and the
/// sentence EC2 worded it with, which names that id — and the caller
/// states it in the refusal its model declares. `Ok(None)` is silence — a request that
/// associates no group, or an emulator with no EC2 to ask — on which nothing is refused and
/// nothing is reported.
///
/// The source security group is the FIRST group the request names:
/// `LoadBalancerDescription$SourceSecurityGroup` is a single structure, while a load
/// balancer may carry several groups, so it is the one the association names first.
pub async fn resolve_source_security_group(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    security_groups: &[String],
) -> Result<Option<SourceSecurityGroup>, NetworkRefusal> {
    let resolved =
        match elbv2::state::network::resolve_security_groups(ctx, region, security_groups).await {
            elbv2::state::network::SecurityGroupLookup::Resolved(resolved) => resolved,
            elbv2::state::network::SecurityGroupLookup::Absent(refusal) => return Err(refusal),
            elbv2::state::network::SecurityGroupLookup::Unknown => return Ok(None),
        };
    let Some(first) = security_groups.first() else {
        return Ok(None);
    };
    Ok(resolved
        .iter()
        .find(|group| &group.group_id == first)
        .map(|group| SourceSecurityGroup {
            owner_alias: group.owner_id.clone(),
            group_name: group.group_name.clone(),
        }))
}

/// The region this request is attributed to: the region field of its SigV4 credential
/// scope when it names a region, else [`DEFAULT_REGION`].
///
/// The scope is caller-written and the vendored parser accepts any non-empty field
/// there, so a value that is not shaped like an AWS region name is not treated as one:
/// it is attributed to the default region rather than opening a store partition of its
/// own.
pub fn region_from_request(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .filter(|region| looks_like_region(region))
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// Whether `region` is shaped like a published AWS region name: three or four
/// hyphen-separated segments, the last a one- or two-digit number and the rest
/// lower-case letters — `us-east-1`, `ap-southeast-2`, `us-gov-west-1`, `cn-north-1`.
fn looks_like_region(region: &str) -> bool {
    let segments: Vec<&str> = region.split('-').collect();
    if !(3..=4).contains(&segments.len()) {
        return false;
    }
    let Some((number, words)) = segments.split_last() else {
        return false;
    };
    let words_look_right = words.iter().all(|word| {
        (1..=10).contains(&word.len()) && word.bytes().all(|byte| byte.is_ascii_lowercase())
    });
    let number_looks_right =
        (1..=2).contains(&number.len()) && number.bytes().all(|byte| byte.is_ascii_digit());
    words_look_right && number_looks_right
}

/// The first 16 bytes of SHA-256 over `label` and `region`, as one integer: a stable
/// per-region value to derive a generated id from.
fn region_digest(region: &str, label: &str) -> u128 {
    use sha2::{Digest, Sha256};

    let mut hasher = Sha256::new();
    hasher.update(label.as_bytes());
    hasher.update(b"/");
    hasher.update(region.as_bytes());
    let digest = hasher.finalize();
    let mut head = [0u8; 16];
    head.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(head)
}

/// A stable 10-digit id for a load balancer's DNS name, derived from the region and
/// name so the same record in the same region always resolves to the same host.
fn dns_suffix(region: &str, name: &str) -> u64 {
    // A 10-digit value, the shape Elastic Load Balancing's own DNS names carry.
    let value = (region_digest(region, name) % 9_000_000_000) as u64;
    1_000_000_000 + value
}

/// The name Elastic Load Balancing's OWN zone would give a load balancer:
/// `<name>-<id>.<region>.elb.amazonaws.com`, prefixed `internal-` for the `internal`
/// scheme (whose name resolves to private addresses).
///
/// NOT what a load balancer publishes as its `DNSName`: that name is the address a client
/// DIALS, and a host in AWS's own zone resolves to nothing on the machine this emulator
/// runs on, so what is published is the `host:port` of the socket this emulator answers
/// the load balancer's traffic on ([`LoadBalancerEndpoint`]). This is the fallback behind
/// it, for an engine that vends no listener at all — and it is what
/// `CanonicalHostedZoneName` reports, the name under which the load balancer's record
/// lives in the Route 53 hosted zone a client points an alias at.
pub fn dns_name(region: &str, name: &str, scheme: &str) -> String {
    let suffix = dns_suffix(region, name);
    if scheme == SCHEME_INTERNAL {
        format!("internal-{name}-{suffix}.{region}.elb.amazonaws.com")
    } else {
        format!("{name}-{suffix}.{region}.elb.amazonaws.com")
    }
}

/// The published Route 53 hosted zone id of the Elastic Load Balancing (Classic)
/// endpoint in each region.
///
/// Source: the "Elastic Load Balancing endpoints and quotas" page of the AWS General
/// Reference, which publishes one hosted zone id per region for Classic Load Balancers.
const CLASSIC_ELB_HOSTED_ZONE_IDS: &[(&str, &str)] = &[
    ("af-south-1", "Z268VQBMOI5EKX"),
    ("ap-east-1", "Z3DQVH9N71FHZ0"),
    ("ap-northeast-1", "Z14GRHDCWA56QT"),
    ("ap-northeast-2", "ZWKZPGTI48KDX"),
    ("ap-northeast-3", "Z5LXEXXYQ4HTP"),
    ("ap-south-1", "ZP97RAFLXTNZK"),
    ("ap-southeast-1", "Z1LMS91P8CMLE5"),
    ("ap-southeast-2", "Z1GM3OXH4ZPM65"),
    ("ca-central-1", "ZQSVJUPU6J1EY"),
    ("eu-central-1", "Z215JYRZR1TBD5"),
    ("eu-north-1", "Z23TAZ6LKFMNIO"),
    ("eu-south-1", "Z3ULH7SSC9OV64"),
    ("eu-west-1", "Z32O12XQLNTSW2"),
    ("eu-west-2", "ZHURV8PSTC4K8"),
    ("eu-west-3", "Z3Q77PNBQS71R4"),
    ("me-south-1", "ZS929ML54UICD"),
    ("sa-east-1", "Z2P70J7HTTTPLU"),
    ("us-east-1", "Z35SXDOTRQ7X7K"),
    ("us-east-2", "Z3AADJGX6KTTL2"),
    ("us-west-1", "Z368ELLRRE2KJ0"),
    ("us-west-2", "Z1H1FL5HABSF5"),
];

/// The hosted zone id to report for a load balancer in `region`.
///
/// A region the published table above does not cover still gets a well-formed,
/// region-stable id (`Z` + 13 upper-case alphanumerics), because every load balancer
/// has exactly one hosted zone id and the field is not nullable in practice.
pub fn hosted_zone_id(region: &str) -> String {
    if let Some((_, id)) = CLASSIC_ELB_HOSTED_ZONE_IDS
        .iter()
        .find(|(candidate, _)| *candidate == region)
    {
        return (*id).to_string();
    }

    // 13 base-36 digits taken from one 128-bit digest of the region (36^13 < 2^128), so
    // an uncovered region always reports the same well-formed id.
    const ALPHABET: &[u8] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
    let mut value = region_digest(region, "hosted-zone");
    let mut id = String::from("Z");
    for _ in 0..13 {
        id.push(char::from(ALPHABET[(value % 36) as usize]));
        value /= 36;
    }
    id
}
