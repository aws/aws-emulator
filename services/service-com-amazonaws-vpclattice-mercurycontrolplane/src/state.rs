// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-vpclattice-mercurycontrolplane service.
//!
//! One store holds the service-network / service / service-association / listener /
//! listener-rule / resource-policy slice, and the
//! helpers every handler in `src/ops/` shares live here too: identifier resolution
//! (bare id or ARN), id and ARN minting, the request's region, and the clock-owed
//! `CREATE_IN_PROGRESS -> ACTIVE` and `DELETE_IN_PROGRESS -> gone` spans. Handlers reach
//! the store through
//! [`service_state`]; nothing in `src/ops/` keeps a second copy of a resource.

use std::collections::HashMap;
use std::time::{Duration, SystemTime};

use ::aws_smithy_schema_server::constraints::{envelope, Violation, ViolationKind};

use crate::ops::types::{
    ConflictException, ResourceNotFoundException, ValidationException, ValidationExceptionField,
};

/// This service's ARN namespace and sigv4 signing name (OPERATIONS.md, "Wire protocol").
pub const ARN_SERVICE: &str = "vpc-lattice";

/// The region for a request that carries no SigV4 credential scope (an unsigned call).
pub const DEFAULT_REGION: &str = "us-east-1";

/// The ARN resource kind of a service network: `.../servicenetwork/sn-…`.
pub const KIND_SERVICE_NETWORK: &str = "servicenetwork";
/// The ARN resource kind of a service: `.../service/svc-…`.
pub const KIND_SERVICE: &str = "service";
/// The ARN resource kind of a service-network/service association.
pub const KIND_SERVICE_ASSOCIATION: &str = "servicenetworkserviceassociation";
/// The ARN resource kind of a listener. A listener ARN nests under its service's:
/// `…:service/svc-…/listener/listener-…` (`ListenerArn`'s modelled pattern).
pub const KIND_LISTENER: &str = "listener";
/// The ARN resource kind of a listener rule. A rule ARN nests under its listener's:
/// `…:service/svc-…/listener/listener-…/rule/rule-…` (`RuleArn`'s modelled pattern).
pub const KIND_RULE: &str = "rule";
/// The ARN resource kind of a target group: `.../targetgroup/tg-…`
/// (`TargetGroupArn`'s modelled pattern).
pub const KIND_TARGET_GROUP: &str = "targetgroup";
/// The ARN resource kind of a resource configuration:
/// `.../resourceconfiguration/rcfg-…` (`ResourceConfigurationArn`'s modelled pattern).
pub const KIND_RESOURCE_CONFIGURATION: &str = "resourceconfiguration";

/// The id prefix of a service network (`^sn-[0-9a-z]{17}$`).
pub const PREFIX_SERVICE_NETWORK: &str = "sn-";
/// The id prefix of a service (`^svc-[0-9a-z]{17}$`).
pub const PREFIX_SERVICE: &str = "svc-";
/// The id prefix of a service-network/service association (`^snsa-[0-9a-z]{17}$`).
pub const PREFIX_SERVICE_ASSOCIATION: &str = "snsa-";
/// The id prefix of a listener (`^listener-[0-9a-z]{17}$`).
pub const PREFIX_LISTENER: &str = "listener-";
/// The id prefix of a listener rule (`^rule-[0-9a-z]{17}$`).
pub const PREFIX_RULE: &str = "rule-";
/// The id prefix of a target group (`^tg-[0-9a-z]{17}$`).
pub const PREFIX_TARGET_GROUP: &str = "tg-";
/// The id prefix of a resource configuration (`^rcfg-[0-9a-z]{17}$`).
pub const PREFIX_RESOURCE_CONFIGURATION: &str = "rcfg-";

/// Every modelled id of this service is its prefix plus 17 `[0-9a-z]` characters.
const ID_SUFFIX_LEN: usize = 17;

/// The first character of a minted id's suffix, which is fixed: see [`mint_id`].
const ID_LEADING_CHARACTER: char = '0';

/// How many drawn characters follow [`ID_LEADING_CHARACTER`] in a minted id's suffix: the
/// 16 hex digits of a 64-bit value.
const ID_ENTROPY_LEN: usize = ID_SUFFIX_LEN - 1;

/// A resource kind of this service, and the spellings the wire needs of it.
///
/// One kind is spelled THREE different ways on the wire and they are not
/// interchangeable: the ARN path segment and `name_index` key ([`Self::arn_kind`]), the
/// resource-type TOKEN an error's `resourceType` member carries ([`Self::wire_type`]),
/// and the name an error MESSAGE calls the resource by ([`Self::message_name`]). Every
/// error this service builds takes its kind from here, so no handler can reuse the
/// internal key where the token belongs.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResourceKind {
    /// A service network (`sn-…`).
    ServiceNetwork,
    /// A service (`svc-…`).
    Service,
    /// A service-network/service association (`snsa-…`).
    ServiceAssociation,
    /// A listener of a service (`listener-…`).
    Listener,
    /// A rule of a listener (`rule-…`).
    Rule,
    /// A target group (`tg-…`).
    TargetGroup,
    /// A resource configuration (`rcfg-…`).
    ResourceConfiguration,
}

impl ResourceKind {
    /// How many kinds there are, so a per-kind cache can hold one slot each.
    pub const COUNT: usize = 7;

    /// This kind's slot in a per-kind array ([`Self::COUNT`] wide).
    pub const fn slot(self) -> usize {
        match self {
            Self::ServiceNetwork => 0,
            Self::Service => 1,
            Self::ServiceAssociation => 2,
            Self::Listener => 3,
            Self::Rule => 4,
            Self::TargetGroup => 5,
            Self::ResourceConfiguration => 6,
        }
    }

    /// The ARN path segment of this kind (`…:servicenetwork/sn-…`), which is also the
    /// prefix of its [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] keys.
    pub const fn arn_kind(self) -> &'static str {
        match self {
            Self::ServiceNetwork => KIND_SERVICE_NETWORK,
            Self::Service => KIND_SERVICE,
            Self::ServiceAssociation => KIND_SERVICE_ASSOCIATION,
            Self::Listener => KIND_LISTENER,
            Self::Rule => KIND_RULE,
            Self::TargetGroup => KIND_TARGET_GROUP,
            Self::ResourceConfiguration => KIND_RESOURCE_CONFIGURATION,
        }
    }

    /// The id prefix of this kind.
    pub const fn id_prefix(self) -> &'static str {
        match self {
            Self::ServiceNetwork => PREFIX_SERVICE_NETWORK,
            Self::Service => PREFIX_SERVICE,
            Self::ServiceAssociation => PREFIX_SERVICE_ASSOCIATION,
            Self::Listener => PREFIX_LISTENER,
            Self::Rule => PREFIX_RULE,
            Self::TargetGroup => PREFIX_TARGET_GROUP,
            Self::ResourceConfiguration => PREFIX_RESOURCE_CONFIGURATION,
        }
    }

    /// The resource-type token the `resourceType` member of this service's error shapes
    /// carries for this kind: the resource type in UPPER_SNAKE_CASE.
    ///
    /// SOURCE: recorded AWS behaviour. `ResourceNotFoundException`, `ConflictException`
    /// and `ServiceQuotaExceededException` all declare `resourceType` as a required
    /// `String` and the model enumerates no values for it
    /// (`smithy_query(query="shape",shape="ResourceNotFoundException")`), while every
    /// recorded vpc-lattice error spells it as the resource type in upper snake case —
    /// `SERVICE_NETWORK`, `SERVICE`, `SERVICE_NETWORK_SERVICE_ASSOCIATION`. It is a
    /// structured field clients branch on, NOT the ARN path segment.
    pub const fn wire_type(self) -> &'static str {
        match self {
            Self::ServiceNetwork => "SERVICE_NETWORK",
            Self::Service => "SERVICE",
            Self::ServiceAssociation => "SERVICE_NETWORK_SERVICE_ASSOCIATION",
            Self::Listener => "LISTENER",
            Self::Rule => "RULE",
            Self::TargetGroup => "TARGET_GROUP",
            Self::ResourceConfiguration => "RESOURCE_CONFIGURATION",
        }
    }

    /// The name an error MESSAGE calls this kind by: the resource type in PascalCase,
    /// with an association named `Association` as the recorded messages spell it
    /// ("Association snsa-… cannot be found.").
    ///
    /// Source: recorded vpc-lattice not-found bodies, which spell the kind as its
    /// CamelCase resource name — `ServiceNetwork` for a service network, `Service` for a
    /// service, and `Association` (not the full association shape name) for a
    /// service-network/service association. No listener not-found body is recorded, so a
    /// listener takes the same rule the other three follow: the Smithy resource name,
    /// `com.amazonaws.vpclattice#Listener`. A listener RULE follows it too, over
    /// `com.amazonaws.vpclattice#Rule`.
    pub const fn message_name(self) -> &'static str {
        match self {
            Self::ServiceNetwork => "ServiceNetwork",
            Self::Service => "Service",
            Self::ServiceAssociation => "Association",
            Self::Listener => "Listener",
            Self::Rule => "Rule",
            Self::TargetGroup => "TargetGroup",
            Self::ResourceConfiguration => "ResourceConfiguration",
        }
    }

    /// How a message names this kind as the SCOPE another resource's name is unique in
    /// ("Listener with name … already exists in svc-… service."): the resource type in
    /// lower-case words, as the recorded listener-name conflict spells the owning
    /// service.
    pub const fn scope_name(self) -> &'static str {
        match self {
            Self::ServiceNetwork => "service network",
            Self::Service => "service",
            Self::ServiceAssociation => "association",
            Self::Listener => "listener",
            Self::Rule => "rule",
            Self::TargetGroup => "target group",
            Self::ResourceConfiguration => "resource configuration",
        }
    }

    /// AWS's `ResourceNotFoundException.message` construction:
    /// `"<ResourceType> <identifier> cannot be found."`.
    ///
    /// `identifier` is the identifier the CALLER submitted — whichever modelled form it
    /// sent (bare id or ARN) — so the message names back the thing the request named, and
    /// is reportable even when nothing resolved. The model types `message` as a required
    /// String and states no text, so the construction ([`Self::message_name`] plus the
    /// submitted identifier plus "cannot be found.") is what the recordings fix, not one
    /// string.
    pub fn not_found_message(self, identifier: &str) -> String {
        format!("{} {identifier} cannot be found.", self.message_name())
    }

    /// The `@pattern` the model puts on this kind's `name` member, or `None` for a kind
    /// that has no name (an association is named by the pair it links).
    ///
    /// SOURCE: `smithy_query(query="shape",shape="ServiceName,ServiceNetworkName,ListenerName,RuleName")`.
    pub const fn name_pattern(self) -> Option<&'static str> {
        match self {
            Self::ServiceNetwork => Some(SERVICE_NETWORK_NAME_PATTERN),
            Self::Service => Some(SERVICE_NAME_PATTERN),
            Self::Listener => Some(LISTENER_NAME_PATTERN),
            Self::Rule => Some(RULE_NAME_PATTERN),
            Self::TargetGroup => Some(TARGET_GROUP_NAME_PATTERN),
            Self::ResourceConfiguration => Some(RESOURCE_CONFIGURATION_NAME_PATTERN),
            Self::ServiceAssociation => None,
        }
    }
}

/// The most items one page of a list operation carries.
///
/// This is the maximum the model allows `maxResults` (`range 1..100`), so it is also
/// the right ceiling for a request that sends no `maxResults`: a single list call can
/// never be made to serialise the whole store, and `nextToken` pages the rest.
pub const MAX_PAGE_SIZE: usize = 100;

/// The `idleTimeoutSeconds` a service gets when its create omits the member.
///
/// From that member's own documentation: "The valid range is 60 to 600 seconds. If you
/// don't specify a value, the default is 60 seconds."
pub const DEFAULT_IDLE_TIMEOUT_SECONDS: i32 = 60;

/// `sharingConfig.enabled` a service network gets when its create sends no
/// `sharingConfig` (or sends one that omits `enabled`).
///
/// `sharingConfig` is a member the service fills in itself: `CreateServiceNetwork` and
/// `GetServiceNetwork` both report it for a create that never mentioned it, so the
/// stored value is a real setting and not "absent". The recorded AWS answers for such a
/// create report `{"enabled": true}` — sharing enabled is what a service network has
/// always had — so that is the default here.
pub const DEFAULT_SHARING_ENABLED: bool = true;

/// The `@length` ceiling the model puts on `TagMap` (`len ..200`).
///
/// Checked in the handlers: a map's cardinality is one of the modelled constraints the
/// boundary does not refuse for this service's `tags` member, and a caller must not be
/// able to store more tags than AWS accepts.
pub const MAX_TAGS: usize = 200;

/// How many service networks this emulator will hold for the account.
///
/// The real service caps service networks per account per Region with a service quota,
/// and `CreateServiceNetwork` models `ServiceQuotaExceededException` (HTTP 402) for
/// exceeding it. The number here is NOT a fidelity claim about the published quota
/// (which is smaller and adjustable): it is the emulator's own ceiling, set generously
/// so no realistic workflow meets it, so that a caller looping on create cannot grow
/// this process's memory without bound.
pub const MAX_SERVICE_NETWORKS: usize = 1_000;

/// How many services this emulator will hold for the account. Same reasoning, and
/// `CreateService` models the same `ServiceQuotaExceededException`.
pub const MAX_SERVICES: usize = 1_000;

/// How many service-network/service associations this emulator will hold. Same
/// reasoning, and `CreateServiceNetworkServiceAssociation` models the same
/// `ServiceQuotaExceededException`.
pub const MAX_SERVICE_ASSOCIATIONS: usize = 1_000;

/// How many listeners this emulator will hold for one service. Same reasoning as the
/// ceilings above, and `CreateListener` models the same `ServiceQuotaExceededException`.
pub const MAX_LISTENERS_PER_SERVICE: usize = 100;

/// How many rules this emulator will hold for one listener. Same reasoning as the
/// ceilings above, and `CreateRule` models the same `ServiceQuotaExceededException`.
/// Not a fidelity claim about the published per-listener rule quota: it is this
/// emulator's own ceiling, so a caller looping on create cannot grow this process's
/// memory without bound.
pub const MAX_RULES_PER_LISTENER: usize = 100;

/// How many target groups this emulator will hold for the account. Same reasoning as
/// the ceilings above, and `CreateTargetGroup` models the same
/// `ServiceQuotaExceededException`.
pub const MAX_TARGET_GROUPS: usize = 1_000;

/// The listener port a create gets when it omits `port`, by protocol.
///
/// From the `port` member's own documentation: "The listener port. You can specify a
/// value from 1 to 65535. For HTTP, the default is 80. For HTTPS, the default is 443."
/// No default is documented for any other protocol, so [`default_port_for`] answers
/// `None` there and the handler refuses a create that sends no port rather than
/// inventing one.
pub const DEFAULT_PORT_HTTP: i32 = 80;
/// The documented default port of an `HTTPS` listener; see [`DEFAULT_PORT_HTTP`].
pub const DEFAULT_PORT_HTTPS: i32 = 443;

/// The weight a forward action's only target group gets when the caller sends none.
///
/// From `WeightedTargetGroup`'s `weight` documentation: "Only required if you specify
/// multiple target groups for a forward action. … If there's only one target group
/// specified, then the default value is 100."
pub const DEFAULT_SINGLE_TARGET_GROUP_WEIGHT: i32 = 100;

/// The name of the implicit DEFAULT rule every listener is created with.
///
/// "Each listener has a default rule for checking connection requests, but you can
/// define additional rules" (`DeleteRule`'s own documentation; `CreateListener`'s
/// `defaultAction` restates it as "The action for the default rule. Each listener has a
/// default rule. The default rule is used if no other rules match."), so a listener's
/// rule set is never empty: the create materialises that rule beside the listener
/// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::insert_default_rule`]). The name it
/// carries is RECORDED AWS behaviour — the model states none.
pub const DEFAULT_RULE_NAME: &str = "default";

/// The priority the default rule carries.
///
/// It sits ABOVE every priority `RulePriority`'s `@range(1, 2000)` lets a caller ask
/// for, which is what makes the default rule the last one evaluated and keeps a rule a
/// caller creates from ever colliding with it. SOURCE: recorded AWS behaviour; the model
/// states no priority for the default rule (and its own range is the range of a priority
/// a CALLER may submit).
pub const DEFAULT_RULE_PRIORITY: i32 = 99_999;

/// How the service refuses a write against a listener's DEFAULT rule.
///
/// The RULE is `UpdateRule`'s own documentation: "Updates a specified rule for the
/// listener. You can't modify a default listener rule. To modify a default listener
/// rule, use UpdateListener." It is a service rule the model carries no trait for, so it
/// is reported through [`service_rule_exception`] over the identifier member that names
/// the rule, not as a modelled constraint report.
pub const DEFAULT_RULE_NOT_MODIFIABLE_MESSAGE: &str =
    "You can't modify a default listener rule. To modify a default listener rule, use \
     UpdateListener.";

/// How the service refuses a DELETE of a listener's DEFAULT rule.
///
/// The RULE is `DeleteRule`'s own documentation: "You can delete additional listener
/// rules, but you cannot delete the default rule." Like the modify refusal
/// ([`DEFAULT_RULE_NOT_MODIFIABLE_MESSAGE`]) it is a service rule the model carries no
/// trait for, so it is reported through [`service_rule_exception`] over the identifier
/// member that names the rule. No recording covers it, so the sentence is that
/// documented rule restated; the member it names is the requirement.
pub const DEFAULT_RULE_NOT_DELETABLE_MESSAGE: &str = "You can't delete a default listener rule.";

/// The HTTP status codes a fixed-response action may carry.
///
/// From `FixedResponseAction`'s `statusCode` documentation: "The HTTP response code.
/// Only `404` and `500` status codes are supported." The modelled `@range(100, 599)` is
/// wider than that, so this narrower catalog is enforced in the handlers.
pub const FIXED_RESPONSE_STATUS_CODES: &[i32] = &[404, 500];

/// How the service refuses a fixed-response status code outside
/// [`FIXED_RESPONSE_STATUS_CODES`].
///
/// The RULE is the member's own documentation ("Only `404` and `500` status codes are
/// supported."); this is the sentence the service answers it with — a service-rule
/// refusal, reported through [`service_rule_exception`] rather than as a modelled
/// constraint report. SOURCE: recorded AWS behaviour; the wording is an example of that
/// refusal, the member it names (`statusCode`) is the requirement.
pub const UNSUPPORTED_STATUS_CODE_MESSAGE: &str = "Provided status code is not supported.";

/// How long after its create call a resource still reports `CREATE_IN_PROGRESS`.
///
/// VPC Lattice creates are asynchronous: the model gives a service and an association a
/// non-terminal `CREATE_IN_PROGRESS` status and the resource reaches `ACTIVE` on its
/// own. AWS documents no duration for it, so this is the emulator's CHOSEN span, inside
/// the 250–750 ms window this project fixes for a transition the clock owes. It is
/// never zero, so a caller that reads straight back still observes the transient
/// status; nothing sleeps for it — the due instant is stored and compared on read.
pub const CREATE_TRANSITION: Duration = Duration::from_millis(500);

/// How long after its delete call a resource still reports `DELETE_IN_PROGRESS` before
/// it is gone.
///
/// A VPC Lattice delete is asynchronous: `DeleteTargetGroup` answers with the
/// non-terminal `DELETE_IN_PROGRESS` and the record disappears on its own, so a read of
/// it is first that status and then the modelled 404. AWS documents no duration for it,
/// so this is the emulator's CHOSEN span: never zero, so a caller that reads straight
/// after the delete still observes the transient status and the state errors that go with
/// it, and short enough that an await-deletion loop finishes promptly. Nothing sleeps for
/// it — the due instant is stored and compared on read.
pub const DELETE_TRANSITION: Duration = Duration::from_secs(3);

/// The uppercase base-36 alphabet a Route 53 hosted-zone id is drawn from (`[0-9A-Z]`).
const BASE36_UPPER: &[u8; 36] = b"0123456789ABCDEFGHIJKLMNOPQRSTUVWXYZ";
/// The lowercase hex alphabet every minted resource id's suffix is drawn from
/// (`[0-9a-f]`); see [`mint_id`].
const HEX_LOWER: &[u8; 16] = b"0123456789abcdef";

/// A service network: the logical boundary services and VPCs associate with.
#[derive(Debug, Clone)]
pub struct ServiceNetwork {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// `AuthType`: `NONE` unless the create call asked for `AWS_IAM` (the model
    /// documents `NONE` as the default).
    pub auth_type: String,
    /// `sharingConfig.enabled` as the create call set it, or
    /// [`DEFAULT_SHARING_ENABLED`] when it sent no value for it. Never "absent": the
    /// service reports this member on every response that models it.
    pub sharing_enabled: bool,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

/// A VPC Lattice service. Named `LatticeService` so it does not read as this crate's
/// emulated `Service`.
#[derive(Debug, Clone)]
pub struct LatticeService {
    pub id: String,
    pub arn: String,
    pub name: String,
    pub custom_domain_name: Option<String>,
    pub certificate_arn: Option<String>,
    pub auth_type: String,
    /// What the caller sent, or [`DEFAULT_IDLE_TIMEOUT_SECONDS`] — the value the
    /// member's own documentation gives a create that omits it.
    pub idle_timeout_seconds: i32,
    pub dns_domain_name: String,
    pub dns_hosted_zone_id: String,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// The instant the clock owes this service its `ACTIVE` status.
    pub active_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

impl LatticeService {
    /// `ServiceStatus` at `now`: the transient create status until the stored due
    /// instant passes, `ACTIVE` from then on. A pure function of stored state and the
    /// clock, so reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            "ACTIVE"
        } else {
            "CREATE_IN_PROGRESS"
        }
    }

    /// This service's DNS entry, as persisted.
    pub fn dns_entry(&self) -> (String, String) {
        (self.dns_domain_name.clone(), self.dns_hosted_zone_id.clone())
    }
}

/// An association between one service network and one service.
#[derive(Debug, Clone)]
pub struct ServiceNetworkServiceAssociation {
    pub id: String,
    pub arn: String,
    pub created_by: String,
    pub created_at: SystemTime,
    /// The instant the clock owes this association its `ACTIVE` status.
    pub active_at: SystemTime,
    pub service_id: String,
    pub service_arn: String,
    pub service_name: String,
    pub service_network_id: String,
    pub service_network_arn: String,
    pub service_network_name: String,
    pub dns_domain_name: String,
    pub dns_hosted_zone_id: String,
    pub custom_domain_name: Option<String>,
    pub tags: HashMap<String, String>,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

impl ServiceNetworkServiceAssociation {
    /// `ServiceNetworkServiceAssociationStatus` at `now`, on the same clock-owed rule
    /// as [`LatticeService::status`].
    pub fn status(&self, now: SystemTime) -> &'static str {
        if now >= self.active_at {
            "ACTIVE"
        } else {
            "CREATE_IN_PROGRESS"
        }
    }
}

/// A listener of one service: the process that checks for connection requests to it.
///
/// The model gives a listener no status member — AWS answers a created listener with
/// nothing non-terminal — so there is no clock-owed transition here, only the
/// timestamps `GetListener` and `ListListeners` report.
#[derive(Debug, Clone)]
pub struct Listener {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// `ListenerProtocol`, exactly as the create sent it (the model constrains it to
    /// `LISTENER_PROTOCOL_VALUES`).
    pub protocol: String,
    /// The port the create sent, or the protocol's documented default; see
    /// [`default_port_for`].
    pub port: i32,
    pub service_id: String,
    pub service_arn: String,
    /// The default rule's action, stored as the modelled union after
    /// [`normalized_rule_action`] filled in the documented weight default.
    pub default_action: crate::ops::types::RuleAction,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

/// A listener rule: one priority, one match and one action under a listener.
///
/// The model gives a rule no status member, so there is no clock-owed transition here —
/// only the timestamps `GetRule` and `ListRules` report.
#[derive(Debug, Clone)]
pub struct Rule {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// The listener this rule belongs to; a rule is only reachable through it.
    pub listener_id: String,
    /// The listener's service, so a read through the wrong service is a 404.
    pub service_id: String,
    /// `RulePriority`. "Each rule for a specific listener must have a unique priority"
    /// (the `priority` member's own documentation), so this is unique per `listener_id`.
    pub priority: i32,
    /// The modelled `RuleMatch` as it was PERSISTED: the match the caller sent after
    /// [`normalized_rule_match`] materialised its collections, so every read reports the
    /// same match.
    pub r#match: crate::ops::types::RuleMatch,
    /// The rule's action, stored as the modelled union after [`normalized_rule_action`]
    /// filled in the documented weight default.
    pub action: crate::ops::types::RuleAction,
    /// `isDefault`: "Indicates whether this is the default rule." A listener's default
    /// rule is the one `CreateListener` carries as `defaultAction`; every rule
    /// `CreateRule` mints is an additional rule, so this is false for them.
    pub is_default: bool,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

/// A target group: a collection of targets a service routes traffic to.
///
/// `TargetGroupStatus` is non-terminal right after a create (`CREATE_IN_PROGRESS`), so
/// the clock owes this record its `ACTIVE` status the way it owes a service one; the due
/// instant is stored and compared on read ([`Self::status`]).
#[derive(Debug, Clone)]
pub struct TargetGroup {
    pub id: String,
    pub arn: String,
    pub name: String,
    /// `TargetGroupType`, exactly as the create sent it (the model constrains it to the
    /// enum's value set, so the boundary refuses anything else before a handler runs).
    pub group_type: String,
    /// The configuration as PERSISTED: the caller's config with every value its
    /// documentation defaults filled in ([`normalized_target_group_config`]), so every
    /// later read reports the same configuration.
    pub config: crate::ops::types::TargetGroupConfig,
    pub tags: HashMap<String, String>,
    pub created_at: SystemTime,
    pub last_updated_at: SystemTime,
    /// The instant the clock owes this target group its `ACTIVE` status.
    pub active_at: SystemTime,
    /// The instant the clock owes this target group's REMOVAL, set by the delete that
    /// started it ([`ComAmazonawsVpclatticeMercurycontrolplaneState::begin_target_group_deletion`]);
    /// `None` for a target group no delete has touched. A record with a due instant
    /// reports `DELETE_IN_PROGRESS` until it passes and is then gone
    /// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::expire_deleted_target_groups`]).
    pub deleting_at: Option<SystemTime>,
    /// Creation order, so a listing pages deterministically. See [`encode_cursor`].
    pub sequence: u64,
}

impl TargetGroup {
    /// `TargetGroupStatus` at `now`: the transient create status until the stored due
    /// instant passes, `ACTIVE` from then on, and the transient delete status from the
    /// delete that started its removal. A pure function of stored state and the clock, so
    /// reading it changes nothing a later caller sees.
    pub fn status(&self, now: SystemTime) -> &'static str {
        if self.deleting_at.is_some() {
            "DELETE_IN_PROGRESS"
        } else if self.is_creating(now) {
            "CREATE_IN_PROGRESS"
        } else {
            "ACTIVE"
        }
    }

    /// Whether the clock still owes this target group its `ACTIVE` status at `now` — its
    /// creation is in progress, which is one of the two states a delete is refused in.
    pub fn is_creating(&self, now: SystemTime) -> bool {
        self.deleting_at.is_none() && now < self.active_at
    }

    /// Whether a delete has started and the clock has made this record's removal due, so
    /// the resource no longer exists for any caller.
    pub fn deletion_due(&self, now: SystemTime) -> bool {
        self.deleting_at.is_some_and(|due| now >= due)
    }
}

/// The `TargetGroupType` values the model enumerates
/// (`smithy_query(query="shape",shape="TargetGroupType")`).
pub const TYPE_IP: &str = "IP";
/// See [`TYPE_IP`].
pub const TYPE_LAMBDA: &str = "LAMBDA";
/// See [`TYPE_IP`].
pub const TYPE_INSTANCE: &str = "INSTANCE";
/// See [`TYPE_IP`].
pub const TYPE_ALB: &str = "ALB";

/// Every `TargetGroupConfig` member, with the target-group types its OWN documentation
/// says it is supported for.
///
/// SOURCE: `smithy_query(query="shape",shape="TargetGroupConfig")`, where each member
/// states either which type it is "Not supported" for (`port`, `protocol`,
/// `protocolVersion`, `vpcIdentifier`: not `LAMBDA`; `healthCheck`: not `LAMBDA` or
/// `ALB`) or the single type it is "Supported only if" (`ipAddressType`: `IP`;
/// `lambdaEventStructureVersion`: `LAMBDA`). A member sent for any other type is
/// refused ([`check_target_group_config`]) and a documented default is filled in only
/// for a type that supports the member ([`normalized_target_group_config`]).
pub const CONFIG_MEMBER_SUPPORT: &[(&str, &[&str])] = &[
    ("port", &[TYPE_IP, TYPE_INSTANCE, TYPE_ALB]),
    ("protocol", &[TYPE_IP, TYPE_INSTANCE, TYPE_ALB]),
    ("protocolVersion", &[TYPE_IP, TYPE_INSTANCE, TYPE_ALB]),
    ("vpcIdentifier", &[TYPE_IP, TYPE_INSTANCE, TYPE_ALB]),
    ("ipAddressType", &[TYPE_IP]),
    ("healthCheck", &[TYPE_IP, TYPE_INSTANCE]),
    ("lambdaEventStructureVersion", &[TYPE_LAMBDA]),
];

/// The `TargetGroupConfig` members a target group of each type cannot be created
/// without, in the order a refusal lists them.
///
/// SOURCE: `smithy_query(query="shape",shape="TargetGroupConfig")`, where `port`,
/// `protocol` and `vpcIdentifier` — the three members that say where the targets are
/// reached — are each documented as "Not supported if the target group type is
/// `LAMBDA`", together with recorded AWS behaviour, which refuses a create of a type
/// that DOES support them whose config leaves them null. So every type routing to
/// targets inside a VPC needs all three, and `LAMBDA`, which supports none of them,
/// needs none. No modelled trait carries this, which is why it is checked here
/// ([`check_target_group_config`]).
///
/// A member whose own documentation states a default is still satisfied by that default
/// ([`config_member_supplied`]): `port` is "For HTTP, the default is 80. For HTTPS, the
/// default is 443", derivable only once the protocol is known.
pub const REQUIRED_CONFIG_MEMBERS: &[(&str, &[&str])] = &[
    (TYPE_IP, &["port", "protocol", "vpcIdentifier"]),
    (TYPE_INSTANCE, &["port", "protocol", "vpcIdentifier"]),
    (TYPE_ALB, &["port", "protocol", "vpcIdentifier"]),
    (TYPE_LAMBDA, &[]),
];

/// The `protocolVersion` a target group gets when its create omits the member: "The
/// protocol version. The default is `HTTP1`." Filled in only for a type that supports
/// the member at all (see [`CONFIG_MEMBER_SUPPORT`]).
pub const DEFAULT_TARGET_GROUP_PROTOCOL_VERSION: &str = "HTTP1";

/// The `ipAddressType` an `IP` target group gets when its create omits the member:
/// "Supported only if the target group type is `IP`. The default is `IPV4`."
pub const DEFAULT_IP_ADDRESS_TYPE: &str = "IPV4";

/// The `lambdaEventStructureVersion` a `LAMBDA` target group gets when its create omits
/// the member: "Supported only if the target group type is `LAMBDA`. The default is
/// `V1`."
pub const DEFAULT_LAMBDA_EVENT_STRUCTURE_VERSION: &str = "V1";

/// The health-check `protocol` a config gets when it omits the member: "The possible
/// protocols are `HTTP` and `HTTPS`. The default is `HTTP`."
pub const DEFAULT_HEALTH_CHECK_PROTOCOL: &str = "HTTP";

/// The health-check `path` a config gets when it omits the member: "The default path is
/// `/`."
pub const DEFAULT_HEALTH_CHECK_PATH: &str = "/";

/// The ranges `HealthCheckConfig`'s own documentation states for its numeric members,
/// each NARROWER than the member's modelled `@range`, as (member, minimum, maximum,
/// default).
///
/// SOURCE: `smithy_query(query="shape",shape="HealthCheckConfig")` —
/// `healthCheckIntervalSeconds` "The range is 5–300 seconds. The default is 30
/// seconds.", `healthCheckTimeoutSeconds` "The range is 1–120 seconds. The default is 5
/// seconds.", `healthyThresholdCount` "The range is 2–10. The default is 5.",
/// `unhealthyThresholdCount` "The range is 2–10. The default is 2." The modelled traits
/// admit 0 for all four, so the documented bounds are enforced in the handlers
/// ([`check_health_check`]) and the documented defaults filled in
/// ([`normalized_health_check`]).
pub const HEALTH_CHECK_RANGES: &[(&str, i32, i32, i32)] = &[
    ("healthCheckIntervalSeconds", 5, 300, 30),
    ("healthCheckTimeoutSeconds", 1, 120, 5),
    ("healthyThresholdCount", 2, 10, 5),
    ("unhealthyThresholdCount", 2, 10, 2),
];

/// Each [`HEALTH_CHECK_RANGES`] member's value in `health_check`, in that table's
/// order, so a bound, a default and the value they constrain cannot drift apart.
fn ranged_health_check_members(
    health_check: &crate::ops::types::HealthCheckConfig,
) -> [Option<i32>; 4] {
    [
        health_check.health_check_interval_seconds,
        health_check.health_check_timeout_seconds,
        health_check.healthy_threshold_count,
        health_check.unhealthy_threshold_count,
    ]
}

/// Refuse a health-check member outside the range its own documentation states, as the
/// violation the boundary reports for the same trait at the same path; `Ok(())` when
/// every sent member is inside its documented range.
///
/// `path_prefix` is where the health check sits in the request being checked
/// (`config.healthCheck` under `CreateTargetGroup`), so the reported member path is the
/// one the caller sent the value at.
pub fn check_health_check(
    path_prefix: &str,
    health_check: &crate::ops::types::HealthCheckConfig,
) -> Result<(), FieldViolation> {
    for (sent, &(member, min, max, _)) in ranged_health_check_members(health_check)
        .iter()
        .zip(HEALTH_CHECK_RANGES)
    {
        let Some(value) = sent else {
            continue;
        };
        let kind = if *value < min {
            ViolationKind::MinValue(min.to_string())
        } else if *value > max {
            ViolationKind::MaxValue(max.to_string())
        } else {
            continue;
        };
        return Err(Violation {
            path: format!("{path_prefix}.{member}"),
            kind,
            value: Some(value.to_string()),
            sensitive: false,
        });
    }
    Ok(())
}

/// Whether `config` sends `member`, named as the model names the member.
fn config_member_sent(config: &crate::ops::types::TargetGroupConfig, member: &str) -> bool {
    match member {
        "port" => config.port.is_some(),
        "protocol" => config.protocol.is_some(),
        "protocolVersion" => config.protocol_version.is_some(),
        "vpcIdentifier" => config.vpc_identifier.is_some(),
        "ipAddressType" => config.ip_address_type.is_some(),
        "healthCheck" => config.health_check.is_some(),
        "lambdaEventStructureVersion" => config.lambda_event_structure_version.is_some(),
        _ => false,
    }
}

/// Whether a target group of `group_type` supports `member` at all; see
/// [`CONFIG_MEMBER_SUPPORT`].
fn config_member_supported(group_type: &str, member: &str) -> bool {
    CONFIG_MEMBER_SUPPORT
        .iter()
        .find(|(name, _)| *name == member)
        .is_none_or(|(_, types)| types.contains(&group_type))
}

/// The config members a target group of `group_type` cannot be created without, in the
/// order the refusal lists them; empty for a type that requires none
/// ([`REQUIRED_CONFIG_MEMBERS`]).
pub fn required_config_members(group_type: &str) -> &'static [&'static str] {
    REQUIRED_CONFIG_MEMBERS
        .iter()
        .find(|(name, _)| *name == group_type)
        .map(|(_, required)| *required)
        .unwrap_or(&[])
}

/// Whether `config` leaves null any member a target group of `group_type` cannot be
/// created without.
///
/// `None` is a config the create did not send at all, which supplies nothing — so a
/// type that requires members is refused whether its config is absent or merely
/// incomplete.
fn config_leaves_required_members_null(
    group_type: &str,
    config: Option<&crate::ops::types::TargetGroupConfig>,
) -> bool {
    required_config_members(group_type)
        .iter()
        .any(|member| !config_member_supplied(config, member))
}

/// Whether the config supplies a value for `member`: sent, or filled in from the default
/// the member's OWN documentation states for the rest of this config
/// ([`normalized_target_group_config`] stores the same value).
///
/// `port` is the only required member documenting a default ("For HTTP, the default is
/// 80. For HTTPS, the default is 443"), and that default is derivable only once the
/// protocol the targets are reached over is known ([`default_port_for`]) — so under a
/// protocol with no documented default an omitted port is missing.
fn config_member_supplied(
    config: Option<&crate::ops::types::TargetGroupConfig>,
    member: &str,
) -> bool {
    let Some(config) = config else {
        return false;
    };
    if config_member_sent(config, member) {
        return true;
    }
    member == "port" && config.protocol.as_deref().and_then(default_port_for).is_some()
}

/// Refuse a config that sends a member the target group's TYPE does not support, or that
/// leaves null a member that type requires, as the service-rule violation each is;
/// `Ok(())` when every sent member is supported and every required member is supplied.
///
/// `config` is `None` for a create that sends no config at all: both rules still apply,
/// since a type that requires config members is not satisfied by an absent config.
///
/// The rules are the members' own documentation ([`CONFIG_MEMBER_SUPPORT`],
/// [`REQUIRED_CONFIG_MEMBERS`]), which the model carries no constraint trait for — so
/// they are reported over the bare member each rule constrains
/// ([`service_rule_exception`]) rather than as a constraint report.
pub fn check_target_group_config(
    group_type: &str,
    config: Option<&crate::ops::types::TargetGroupConfig>,
) -> Result<(), ServiceRuleViolation> {
    if let Some(config) = config {
        for (member, _) in CONFIG_MEMBER_SUPPORT {
            if config_member_sent(config, member) && !config_member_supported(group_type, member) {
                return Err(ServiceRuleViolation {
                    member: (*member).to_string(),
                    message: unsupported_config_member_message(member, group_type),
                });
            }
        }
    }
    if config_leaves_required_members_null(group_type, config) {
        return Err(ServiceRuleViolation {
            // The refusal is about the `config` member as a whole, not about one of the
            // members inside it: that is where the caller has to supply the values.
            member: CONFIG_MEMBER.to_string(),
            message: required_config_members_message(group_type),
        });
    }
    Ok(())
}

/// The input member a target group's configuration is sent at, as the model names it.
pub const CONFIG_MEMBER: &str = "config";

/// How the service refuses a config that leaves null a member the target group's type
/// requires: the members that type demands, pipe-joined, and the type itself.
///
/// SOURCE: recorded AWS behaviour. The sentence is fixed text for this refusal class;
/// its variable parts are the type's own required-member list
/// ([`REQUIRED_CONFIG_MEMBERS`]) and the target-group type token taken from the
/// request — it names what the type demands, not which of them this one request left
/// out.
pub fn required_config_members_message(group_type: &str) -> String {
    format!(
        "Config fields {} must not be null for {group_type} Target Group",
        required_config_members(group_type).join("|")
    )
}

/// How the service refuses a `TargetGroupConfig` member the target group's type does
/// not support.
///
/// The construction — the member the rule constrains and the type that does not support
/// it, both taken from the request — is the requirement; the sentence is this service's
/// wording of it, since the model states no text for the refusal.
pub fn unsupported_config_member_message(member: &str, group_type: &str) -> String {
    format!("{member} is not supported for target group type {group_type}.")
}

/// The health check as it is PERSISTED: the caller's health check with every value its
/// documentation defaults filled in (the last column of [`HEALTH_CHECK_RANGES`],
/// [`DEFAULT_HEALTH_CHECK_PROTOCOL`] and [`DEFAULT_HEALTH_CHECK_PATH`]).
///
/// A member whose documentation states no default is left exactly as sent: `enabled`,
/// `protocolVersion`, `matcher`, and `port`, whose documented default is "the port that
/// a target receives traffic on" — a property of the target, not a value this service
/// can fix at create time.
pub fn normalized_health_check(
    health_check: &crate::ops::types::HealthCheckConfig,
) -> crate::ops::types::HealthCheckConfig {
    let mut normalized = health_check.clone();
    normalized.protocol = Some(
        normalized
            .protocol
            .unwrap_or_else(|| DEFAULT_HEALTH_CHECK_PROTOCOL.to_string()),
    );
    normalized.path = Some(
        normalized
            .path
            .unwrap_or_else(|| DEFAULT_HEALTH_CHECK_PATH.to_string()),
    );
    let defaults = HEALTH_CHECK_RANGES.iter().map(|(_, _, _, default)| *default);
    let mut filled = [0_i32; 4];
    for ((slot, sent), default) in filled
        .iter_mut()
        .zip(ranged_health_check_members(&normalized))
        .zip(defaults)
    {
        *slot = sent.unwrap_or(default);
    }
    normalized.health_check_interval_seconds = Some(filled[0]);
    normalized.health_check_timeout_seconds = Some(filled[1]);
    normalized.healthy_threshold_count = Some(filled[2]);
    normalized.unhealthy_threshold_count = Some(filled[3]);
    normalized
}

/// The target-group config as it is PERSISTED: the caller's config with every value its
/// documentation defaults filled in, for the members this `group_type` supports.
///
/// A default is NEVER filled in for a member the type does not support (see
/// [`CONFIG_MEMBER_SUPPORT`]), so a `LAMBDA` target group reports no protocol version
/// and an `INSTANCE` one no Lambda event-structure version. `port` takes the default its
/// member documents for the protocol the targets are reached over
/// ([`default_port_for`]), which is derivable only once a protocol is known.
pub fn normalized_target_group_config(
    group_type: &str,
    config: Option<&crate::ops::types::TargetGroupConfig>,
) -> crate::ops::types::TargetGroupConfig {
    let mut config = config.cloned().unwrap_or_default();
    if config_member_supported(group_type, "protocolVersion") {
        config.protocol_version = Some(
            config
                .protocol_version
                .unwrap_or_else(|| DEFAULT_TARGET_GROUP_PROTOCOL_VERSION.to_string()),
        );
    }
    if config_member_supported(group_type, "ipAddressType") {
        config.ip_address_type = Some(
            config
                .ip_address_type
                .unwrap_or_else(|| DEFAULT_IP_ADDRESS_TYPE.to_string()),
        );
    }
    if config_member_supported(group_type, "lambdaEventStructureVersion") {
        config.lambda_event_structure_version = Some(
            config
                .lambda_event_structure_version
                .unwrap_or_else(|| DEFAULT_LAMBDA_EVENT_STRUCTURE_VERSION.to_string()),
        );
    }
    if let Some(protocol) = config.protocol.clone() {
        config.port = config.port.or_else(|| default_port_for(&protocol));
    }
    config.health_check = config.health_check.as_ref().map(normalized_health_check);
    config
}

/// The default port of a listener speaking `protocol`, or `None` where the `port`
/// member documents no default for it.
pub fn default_port_for(protocol: &str) -> Option<i32> {
    match protocol {
        "HTTP" => Some(DEFAULT_PORT_HTTP),
        "HTTPS" => Some(DEFAULT_PORT_HTTPS),
        _ => None,
    }
}

/// Refuse a rule action outside a constraint its own documentation states, as the
/// SERVICE-RULE violation AWS reports it; `Ok(())` when it satisfies them.
///
/// The modelled traits the boundary already enforces (`targetGroups`' `@length(1, 10)`,
/// `weight`'s `@range(0, 999)`, `statusCode`'s `@range(100, 599)`) are not re-checked
/// here; what is checked is the narrower documented catalog of fixed-response status
/// codes ([`FIXED_RESPONSE_STATUS_CODES`]), which no modelled trait carries — so it is
/// reported as a service rule ([`ServiceRuleViolation`]) and not as a constraint report.
pub fn check_rule_action(
    action: &crate::ops::types::RuleAction,
) -> Result<(), ServiceRuleViolation> {
    match action {
        crate::ops::types::RuleAction::fixedResponse(fixed) => {
            if !FIXED_RESPONSE_STATUS_CODES.contains(&fixed.status_code) {
                return Err(ServiceRuleViolation {
                    // The member the RULE constrains, named as the model names it —
                    // never the path it happens to sit at inside this request.
                    member: "statusCode".to_string(),
                    message: UNSUPPORTED_STATUS_CODE_MESSAGE.to_string(),
                });
            }
            Ok(())
        }
        crate::ops::types::RuleAction::Forward(_) => Ok(()),
    }
}

/// Refuse a listener create that carries no `port` under a protocol the `port` member
/// documents no default for, as the service-rule violation it is.
///
/// The `port` member is modelled OPTIONAL and documents a default for `HTTP` (80) and
/// `HTTPS` (443) only ([`default_port_for`]), so under any other protocol the service's
/// own rule makes it mandatory: no modelled trait carries that, which is why it is
/// reported as a service rule over the bare member rather than as a missing required
/// member.
pub fn effective_port(protocol: &str, sent: Option<i32>) -> Result<i32, ServiceRuleViolation> {
    sent.or_else(|| default_port_for(protocol))
        .ok_or_else(|| ServiceRuleViolation {
            member: "port".to_string(),
            message: format!("Port must be specified for protocol {protocol}."),
        })
}

/// The action as it is PERSISTED: the caller's action with every value its
/// documentation defaults filled in.
///
/// Today that is one default — a forward action whose single target group carries no
/// `weight` gets [`DEFAULT_SINGLE_TARGET_GROUP_WEIGHT`]. A weight omitted beside other
/// target groups is left absent: the member documents a default only for the
/// single-target-group case.
pub fn normalized_rule_action(
    action: &crate::ops::types::RuleAction,
) -> crate::ops::types::RuleAction {
    let mut action = action.clone();
    if let crate::ops::types::RuleAction::Forward(forward) = &mut action {
        if forward.target_groups.len() == 1 {
            if let Some(only) = forward.target_groups.first_mut() {
                only.weight = Some(only.weight.unwrap_or(DEFAULT_SINGLE_TARGET_GROUP_WEIGHT));
            }
        }
    }
    action
}

/// The match as it is PERSISTED: the caller's match with every collection its HTTP
/// criteria carry materialised, so a rule stores a LIST of header matches — an empty one
/// when the request configured none — rather than the absence of a list.
///
/// A rule's match is reported back as it is stored, and AWS reports an HTTP match's
/// `headerMatches` as a present list whether or not any header match is configured: a
/// client that reads the list to decide whether header matching is in force gets a list
/// either way. The member is optional on INPUT (and its `@length` bounds only constrain
/// the entries a caller submits), so materialising it belongs to what gets stored, which
/// keeps every projection of a rule's match — the create's own answer, a read and an
/// update's answer — agreeing with one another. SOURCE: recorded AWS behaviour; the model
/// states no projection rule for the member.
pub fn normalized_rule_match(
    r#match: &crate::ops::types::RuleMatch,
) -> crate::ops::types::RuleMatch {
    let mut r#match = r#match.clone();
    match &mut r#match {
        crate::ops::types::RuleMatch::httpMatch(http) => {
            if http.header_matches.is_none() {
                http.header_matches = Some(Vec::new());
            }
        }
    }
    r#match
}

/// The match a listener's DEFAULT rule carries: HTTP criteria with no condition
/// configured, since that rule is the one "used if no other rules match"
/// (`CreateListener`'s `defaultAction`) and so matches on nothing of its own.
///
/// Built through [`normalized_rule_match`], so the default rule's match is stored the
/// way a caller's is and every projection of a rule's match agrees with the others.
pub fn default_rule_match() -> crate::ops::types::RuleMatch {
    normalized_rule_match(&crate::ops::types::RuleMatch::httpMatch(
        crate::ops::types::HttpMatch::default(),
    ))
}

/// The most `@idempotencyToken` -> resource-id entries this service remembers.
///
/// Tokens are caller-supplied, so the map is caller-driven growth that holds no resource
/// of its own: an entry naming a deleted resource is dropped with it
/// ([`IdempotencyTokens::forget_resource`]), and beyond this many live entries the oldest
/// is evicted ([`IdempotencyTokens::record`]). An evicted token is indistinguishable from
/// one whose idempotency window has passed: the next create carrying it mints a new
/// resource.
const MAX_IDEMPOTENCY_TOKENS: usize = 1024;

/// A bounded `@idempotencyToken` -> created-resource-id store.
///
/// Insertion-ordered and capped at [`MAX_IDEMPOTENCY_TOKENS`]: recording the
/// (cap + 1)-th token forgets the oldest one rather than growing the map.
#[derive(Debug, Default)]
pub struct IdempotencyTokens {
    by_key: HashMap<String, String>,
    oldest_first: std::collections::VecDeque<String>,
}

impl IdempotencyTokens {
    /// The resource id a previous call recorded under `key`, if it is still remembered.
    pub fn get(&self, key: &str) -> Option<&String> {
        self.by_key.get(key)
    }

    /// Remember that `key` created `resource_id`, evicting the oldest entries to stay
    /// within [`MAX_IDEMPOTENCY_TOKENS`].
    pub fn record(&mut self, key: String, resource_id: String) {
        if self.by_key.insert(key.clone(), resource_id).is_none() {
            self.oldest_first.push_back(key);
        }
        while self.oldest_first.len() > MAX_IDEMPOTENCY_TOKENS {
            if let Some(evicted) = self.oldest_first.pop_front() {
                self.by_key.remove(&evicted);
            }
        }
    }

    /// How many tokens are remembered right now.
    pub fn len(&self) -> usize {
        self.by_key.len()
    }

    /// Whether no token is remembered.
    pub fn is_empty(&self) -> bool {
        self.by_key.is_empty()
    }

    /// Forget every token that names `resource_id` — called when that resource is
    /// deleted, so a token record never outlives the resource it created.
    pub fn forget_resource(&mut self, resource_id: &str) {
        self.by_key.retain(|_, created| created != resource_id);
        let live = &self.by_key;
        self.oldest_first.retain(|key| live.contains_key(key));
    }
}

/// Per-service state for com-amazonaws-vpclattice-mercurycontrolplane. A handler reaches it through [`service_state`].
///
/// Every map here is bounded. The three resource maps are capped at
/// [`MAX_SERVICE_NETWORKS`], [`MAX_SERVICES`] and [`MAX_SERVICE_ASSOCIATIONS`] — a create
/// that would exceed its ceiling is refused with the modelled
/// `ServiceQuotaExceededException` — and `name_index` holds exactly one key per stored
/// resource, released by the same delete that removes the record. The token store is
/// capped and evicted on its own terms; see [`IdempotencyTokens`].
#[derive(Debug, Default)]
pub struct ComAmazonawsVpclatticeMercurycontrolplaneState {
    /// Keyed by `sn-…` id. At most [`MAX_SERVICE_NETWORKS`] entries.
    pub service_networks: HashMap<String, ServiceNetwork>,
    /// Keyed by `svc-…` id. At most [`MAX_SERVICES`] entries.
    pub services: HashMap<String, LatticeService>,
    /// Keyed by `snsa-…` id. At most [`MAX_SERVICE_ASSOCIATIONS`] entries.
    pub service_associations: HashMap<String, ServiceNetworkServiceAssociation>,
    /// Keyed by `listener-…` id. At most [`MAX_LISTENERS_PER_SERVICE`] entries per
    /// service, and every listener of a deleted service goes with it
    /// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::remove_listeners_of`]).
    pub listeners: HashMap<String, Listener>,
    /// Keyed by `rule-…` id. At most [`MAX_RULES_PER_LISTENER`] entries per listener,
    /// and every rule of a deleted listener goes with it
    /// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::remove_rules_of`]).
    pub rules: HashMap<String, Rule>,
    /// Keyed by `tg-…` id. At most [`MAX_TARGET_GROUPS`] entries, and a target group
    /// whose asynchronous deletion has fallen due leaves this map on the next operation
    /// that reads it
    /// ([`ComAmazonawsVpclatticeMercurycontrolplaneState::expire_deleted_target_groups`]).
    pub target_groups: HashMap<String, TargetGroup>,
    /// `<resource id>` -> the resource-based permission policy document
    /// `PutResourcePolicy` attached to that resource, for the kinds whose ARN the
    /// operation accepts ([`policy_target`]). One entry per resource that has a policy,
    /// written only for a resource this store holds and removed by that resource's
    /// delete, so the resource ceilings above bound this map too; each document is at
    /// most the `@length` ceiling the model puts on `PolicyString`.
    pub resource_policies: HashMap<String, String>,
    /// `<ARN resource kind>/<name>` -> id. A name must be unique to the account, so a
    /// second create under a taken name is a `ConflictException`. One key per stored
    /// resource, so the resource ceilings bound this map too.
    pub name_index: HashMap<String, String>,
    /// `<operation>/<clientToken>` -> the id the first call created, so a retry with the
    /// same `@idempotencyToken` returns that resource instead of creating a second one.
    pub idempotency_tokens: IdempotencyTokens,
    /// Hands out [`ServiceNetwork::sequence`] and its siblings: the creation order a
    /// listing pages by. Monotonic, so a deleted resource's place is never reused.
    next_sequence: u64,
}

impl ComAmazonawsVpclatticeMercurycontrolplaneState {
    /// The creation-order number for the resource being created now.
    pub fn next_sequence(&mut self) -> u64 {
        self.next_sequence += 1;
        self.next_sequence
    }

    /// How many services are associated with `service_network_id`, counted from the
    /// association records at read time rather than from a stored counter.
    pub fn associated_service_count(&self, service_network_id: &str) -> i64 {
        i64::try_from(
            self.service_associations
                .values()
                .filter(|assoc| assoc.service_network_id == service_network_id)
                .count(),
        )
        .unwrap_or(i64::MAX)
    }

    /// How many service networks `service_id` is associated with, counted the same way.
    pub fn associated_network_count(&self, service_id: &str) -> i64 {
        i64::try_from(
            self.service_associations
                .values()
                .filter(|assoc| assoc.service_id == service_id)
                .count(),
        )
        .unwrap_or(i64::MAX)
    }

    /// How many listeners `service_id` holds, counted from the listener records.
    pub fn listener_count(&self, service_id: &str) -> usize {
        self.listeners
            .values()
            .filter(|listener| listener.service_id == service_id)
            .count()
    }

    /// How many rules `listener_id` holds, counted from the rule records.
    pub fn rule_count(&self, listener_id: &str) -> usize {
        self.rules
            .values()
            .filter(|rule| rule.listener_id == listener_id)
            .count()
    }

    /// The rule of `listener_id` that already holds `priority`, if there is one.
    ///
    /// "Each rule for a specific listener must have a unique priority" (the `priority`
    /// member's own documentation), so a create or update that would take a priority
    /// another rule of the same listener holds is a `ConflictException`.
    pub fn rule_with_priority(&self, listener_id: &str, priority: i32) -> Option<&Rule> {
        self.rules
            .values()
            .find(|rule| rule.listener_id == listener_id && rule.priority == priority)
    }

    /// Materialise `listener`'s implicit DEFAULT rule, answering the rule id it minted.
    ///
    /// "Each listener has a default rule for checking connection requests, but you can
    /// define additional rules" (`DeleteRule`'s documentation), so a listener is created
    /// WITH that rule rather than acquiring one later: it is stored as a rule record like
    /// any other — `isDefault` true, [`DEFAULT_RULE_NAME`], [`DEFAULT_RULE_PRIORITY`],
    /// the listener's own `defaultAction` as its action and an ARN nested under the
    /// listener's — so every path that reports a listener's rules reports it from the one
    /// rule store, it takes part in the per-listener name and priority uniqueness rules,
    /// and it goes with the listener when the listener or its service is deleted
    /// ([`Self::remove_rules_of`]).
    pub fn insert_default_rule(
        &mut self,
        listener: &Listener,
        region: &str,
        now: SystemTime,
    ) -> String {
        let id = mint_id(PREFIX_RULE);
        let sequence = self.next_sequence();
        let record = Rule {
            arn: rule_arn(region, &listener.service_id, &listener.id, &id),
            id: id.clone(),
            name: DEFAULT_RULE_NAME.to_string(),
            listener_id: listener.id.clone(),
            service_id: listener.service_id.clone(),
            priority: DEFAULT_RULE_PRIORITY,
            r#match: default_rule_match(),
            // The default rule's action IS the listener's `defaultAction`, which the
            // listener record already holds normalised, so the two cannot disagree.
            action: listener.default_action.clone(),
            is_default: true,
            tags: HashMap::new(),
            created_at: now,
            last_updated_at: now,
            sequence,
        };
        self.name_index
            .insert(rule_name_key(&listener.id, &record.name), id.clone());
        self.rules.insert(id.clone(), record);
        id
    }

    /// Apply the members an update supplies onto `rule`, answering whether any of them
    /// changed it — and stamping `lastUpdatedAt` with `now` when one did.
    ///
    /// The ONE write path both update operations take: `UpdateRule` applies one rule's
    /// members and `BatchUpdateRule` applies an entry's, and AWS documents the batch as
    /// "Updates the listener rules in a batch", so the two cannot differ in WHAT a
    /// supplied member does to the stored record. A member the request leaves out leaves
    /// that part of the rule as it was ("You can also update a rule in a batch" is still
    /// an update, not a replace), and an update that supplies nothing changes nothing, so
    /// it leaves the timestamp where it is.
    pub fn apply_rule_update(
        rule: &mut Rule,
        priority: Option<i32>,
        r#match: Option<&crate::ops::types::RuleMatch>,
        action: Option<&crate::ops::types::RuleAction>,
        now: SystemTime,
    ) -> bool {
        let mut changed = false;
        if let Some(priority) = priority {
            rule.priority = priority;
            changed = true;
        }
        if let Some(r#match) = r#match {
            rule.r#match = normalized_rule_match(r#match);
            changed = true;
        }
        if let Some(action) = action {
            rule.action = normalized_rule_action(action);
            changed = true;
        }
        if changed {
            rule.last_updated_at = now;
        }
        changed
    }

    /// The listener of `service_id` that already binds `port`, if there is one.
    ///
    /// A listener's port is unique within one service: the service has one endpoint and
    /// a listener "is a process that checks for connection requests" on its port, so two
    /// listeners of one service cannot hold the same port and a create that would take an
    /// occupied one is a `ConflictException`. SOURCE: recorded AWS behaviour (the model
    /// states the uniqueness nowhere). Counted from the listener records the way
    /// [`Self::rule_with_priority`] counts priorities, so no second index can drift out
    /// of step with what is stored.
    pub fn listener_with_port(&self, service_id: &str, port: i32) -> Option<&Listener> {
        self.listeners
            .values()
            .find(|listener| listener.service_id == service_id && listener.port == port)
    }

    /// Drop one listener and everything that belongs to it, answering the record that was
    /// removed (`None` when no listener is stored under `listener_id`).
    ///
    /// Its rules go with it (a rule is a resource OF its listener), its name leaves the
    /// name index so the name and its port are free again, and no `clientToken` record
    /// outlives the resource it created.
    pub fn remove_listener(&mut self, listener_id: &str) -> Option<Listener> {
        let removed = self.listeners.remove(listener_id)?;
        self.remove_rules_of(&removed.id);
        self.name_index
            .remove(&listener_name_key(&removed.service_id, &removed.name));
        self.idempotency_tokens.forget_resource(&removed.id);
        Some(removed)
    }

    /// Drop one rule, answering the record that was removed (`None` when no rule is
    /// stored under `rule_id`).
    ///
    /// The ONE rule-removal path: the rule leaves the store, its name leaves the name
    /// index so that name and the priority it held are free for another rule of the same
    /// listener, and no `clientToken` record outlives the rule it created. `DeleteRule`
    /// takes it for one rule and [`Self::remove_rules_of`] takes it for every rule of a
    /// listener, so a rule dropped with its listener releases exactly what one dropped on
    /// its own does.
    pub fn remove_rule(&mut self, rule_id: &str) -> Option<Rule> {
        let removed = self.rules.remove(rule_id)?;
        self.name_index
            .remove(&rule_name_key(&removed.listener_id, &removed.name));
        self.idempotency_tokens.forget_resource(&removed.id);
        Some(removed)
    }

    /// Drop every rule of `listener_id`, releasing its name and token records.
    ///
    /// A rule is a resource OF its listener, so deleting the listener (or the service
    /// that holds it) deletes its rules: `DeleteService` documents that "listener rules"
    /// go with the service.
    pub fn remove_rules_of(&mut self, listener_id: &str) {
        let doomed: Vec<String> = self
            .rules
            .values()
            .filter(|rule| rule.listener_id == listener_id)
            .map(|rule| rule.id.clone())
            .collect();
        for id in doomed {
            // One removal path for a rule, so a rule dropped with its listener releases
            // exactly what one dropped on its own does.
            let _ = self.remove_rule(&id);
        }
    }

    /// A rule whose action forwards traffic to `target_group_id`, if this store holds one.
    ///
    /// "You can't delete a target group if it is used in a listener rule"
    /// (`DeleteTargetGroup`'s own documentation), and a listener's `defaultAction` is
    /// stored as that listener's default RULE ([`Self::insert_default_rule`]), so every
    /// use of a target group by a listener or by one of its rules is a rule record here.
    pub fn rule_forwarding_to(&self, target_group_id: &str) -> Option<&Rule> {
        self.rules
            .values()
            .find(|rule| action_forwards_to(&rule.action, target_group_id))
    }

    /// Start `target_group_id`'s asynchronous deletion, answering the record as it now
    /// reads (`None` when no target group is stored under that id).
    ///
    /// The delete does not remove the record: it stores the instant the clock owes its
    /// removal ([`DELETE_TRANSITION`]), so the target group reports `DELETE_IN_PROGRESS`
    /// until then and is gone afterwards ([`Self::expire_deleted_target_groups`]). A
    /// target group whose deletion is already under way keeps the instant its first
    /// delete set — "if you retry it while the status is `DELETE_IN_PROGRESS`, the status
    /// doesn't change" (`DeleteTargetGroup`'s `status` member).
    pub fn begin_target_group_deletion(
        &mut self,
        target_group_id: &str,
        now: SystemTime,
    ) -> Option<&TargetGroup> {
        let group = self.target_groups.get_mut(target_group_id)?;
        if group.deleting_at.is_none() {
            group.deleting_at = Some(now + DELETE_TRANSITION);
        }
        Some(group)
    }

    /// Materialise every target-group removal the clock has already made due: the record
    /// leaves the store and the account-unique name it held leaves the name index, so the
    /// name is free again and a later read of the id is the modelled not-found.
    ///
    /// Called by the operations that touch the target-group store before they read it, so
    /// a deletion that fell due completes whether or not anything else happened in the
    /// meantime; it writes nothing that was not already owed at the same instant.
    pub fn expire_deleted_target_groups(&mut self, now: SystemTime) {
        let doomed: Vec<String> = self
            .target_groups
            .values()
            .filter(|group| group.deletion_due(now))
            .map(|group| group.id.clone())
            .collect();
        for id in doomed {
            let _ = self.remove_target_group(&id);
        }
    }

    /// Drop one target group, answering the record that was removed (`None` when none is
    /// stored under `target_group_id`).
    ///
    /// The ONE target-group-removal path: its name leaves the name index so the
    /// account-unique name is free again, and no `clientToken` record outlives the
    /// resource it created.
    pub fn remove_target_group(&mut self, target_group_id: &str) -> Option<TargetGroup> {
        let removed = self.target_groups.remove(target_group_id)?;
        self.name_index
            .remove(&name_key(KIND_TARGET_GROUP, &removed.name));
        self.idempotency_tokens.forget_resource(&removed.id);
        Some(removed)
    }

    /// Drop every listener of `service_id`, releasing its name and token records.
    ///
    /// `DeleteService`'s own documentation: "If you delete a service, all resources
    /// related to the service, such as the resource policy, auth policy, listeners,
    /// listener rules, and access log subscriptions, are also deleted."
    pub fn remove_listeners_of(&mut self, service_id: &str) {
        let doomed: Vec<String> = self
            .listeners
            .values()
            .filter(|listener| listener.service_id == service_id)
            .map(|listener| listener.id.clone())
            .collect();
        for id in doomed {
            // One removal path for a listener, so a listener dropped with its service
            // releases exactly what one dropped on its own does.
            let _ = self.remove_listener(&id);
        }
    }

    /// The ARNs of the services that route traffic to `target_group_id`, derived from
    /// the listener and rule records at read time rather than from a stored list.
    ///
    /// A forward action naming this target group — a listener's default action or a
    /// rule's action — makes the service that holds it one of them. Sorted and
    /// deduplicated, so two reads of an unchanged store report the same list.
    pub fn service_arns_using(&self, target_group_id: &str) -> Vec<String> {
        let mut arns: Vec<String> = self
            .listeners
            .values()
            .filter(|listener| action_forwards_to(&listener.default_action, target_group_id))
            .map(|listener| listener.service_arn.clone())
            .chain(
                self.rules
                    .values()
                    .filter(|rule| action_forwards_to(&rule.action, target_group_id))
                    .filter_map(|rule| self.services.get(&rule.service_id))
                    .map(|service| service.arn.clone()),
            )
            .collect();
        arns.sort();
        arns.dedup();
        arns
    }

    /// Whether this store holds a resource of `kind` under `id`.
    ///
    /// The one place a kind maps to the registry that holds it, so an operation that
    /// takes an identifier of several kinds (a resource policy's `resourceArn`) resolves
    /// existence without a second copy of that mapping. A kind no implemented create
    /// mints yet holds nothing, so an identifier naming one is a not-found.
    pub fn holds(&self, kind: ResourceKind, id: &str) -> bool {
        match kind {
            ResourceKind::ServiceNetwork => self.service_networks.contains_key(id),
            ResourceKind::Service => self.services.contains_key(id),
            ResourceKind::ServiceAssociation => self.service_associations.contains_key(id),
            ResourceKind::Listener => self.listeners.contains_key(id),
            ResourceKind::Rule => self.rules.contains_key(id),
            ResourceKind::TargetGroup => self.target_groups.contains_key(id),
            // `CreateResourceConfiguration` is still a 501, so no resource
            // configuration can be stored for one to be found.
            ResourceKind::ResourceConfiguration => false,
        }
    }

    /// The association of this exact (service network, service) pair, if there is one.
    pub fn association_of(
        &self,
        service_network_id: &str,
        service_id: &str,
    ) -> Option<&ServiceNetworkServiceAssociation> {
        self.service_associations.values().find(|assoc| {
            assoc.service_network_id == service_network_id && assoc.service_id == service_id
        })
    }
}

/// Whether `action` forwards to the target group `target_group_id`, under either
/// modelled form of a `targetGroupIdentifier` (the bare id or the target group's ARN).
fn action_forwards_to(action: &crate::ops::types::RuleAction, target_group_id: &str) -> bool {
    match action {
        crate::ops::types::RuleAction::Forward(forward) => {
            forward.target_groups.iter().any(|weighted| {
                id_from_identifier(&weighted.target_group_identifier, PREFIX_TARGET_GROUP)
                    .is_some_and(|id| id == target_group_id)
            })
        }
        crate::ops::types::RuleAction::fixedResponse(_) => false,
    }
}

/// The key [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] is read with.
pub fn name_key(kind: &str, name: &str) -> String {
    format!("{kind}/{name}")
}

/// The [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] key of a listener
/// name. A listener name is unique within its SERVICE, not within the account, so the
/// owning service's id is part of the key.
pub fn listener_name_key(service_id: &str, name: &str) -> String {
    name_key(&format!("{KIND_SERVICE}/{service_id}/{KIND_LISTENER}"), name)
}

/// The [`ComAmazonawsVpclatticeMercurycontrolplaneState::name_index`] key of a rule
/// name. "The name must be unique within the listener" (`CreateRule`'s `name` member),
/// so the owning listener's id is part of the key.
pub fn rule_name_key(listener_id: &str, name: &str) -> String {
    name_key(&format!("{KIND_LISTENER}/{listener_id}/{KIND_RULE}"), name)
}

/// A `nextToken` no listing of this service ever minted.
#[derive(Debug, Clone, Copy)]
pub struct MalformedCursor;

/// Encode the resume cursor that follows the item with creation-order number `sequence`.
///
/// Opaque to the caller (hex of the internal cursor text) and well within `NextToken`'s
/// `@length(1, 2048)`. Naming the ORDER rather than an id keeps the cursor usable when
/// the item the page ended on is deleted between pages: the next page resumes at the
/// first surviving item after that point, instead of refusing a token this service
/// itself handed out.
pub fn encode_cursor(sequence: u64) -> String {
    ::hex::encode(format!("after-sequence:{sequence}"))
}

/// Decode a cursor minted by [`encode_cursor`]; no cursor starts at the beginning.
pub fn decode_cursor(next_token: Option<&str>) -> Result<u64, MalformedCursor> {
    let Some(token) = next_token else {
        return Ok(0);
    };
    let bytes = ::hex::decode(token).map_err(|_| MalformedCursor)?;
    let text = String::from_utf8(bytes).map_err(|_| MalformedCursor)?;
    text.strip_prefix("after-sequence:")
        .and_then(|sequence| sequence.parse::<u64>().ok())
        .ok_or(MalformedCursor)
}

/// The key [`ComAmazonawsVpclatticeMercurycontrolplaneState::idempotency_tokens`] is
/// read with: a token is scoped to the operation that minted it.
pub fn token_key(operation: &str, client_token: &str) -> String {
    format!("{operation}/{client_token}")
}

/// The region this request was signed for, from its SigV4 credential scope;
/// [`DEFAULT_REGION`] when it carries none.
pub fn region_of(parts: &::http::request::Parts) -> String {
    ::emulator_server::sigv4_credential_scope(&parts.headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// The emulator's account id, the `createdBy` and ARN account of every resource here.
pub fn account_id() -> &'static str {
    ::engine::emulator_account_id()
}

/// `arn:aws:vpc-lattice:<region>:<account>:<kind>/<id>`.
pub fn arn_for(region: &str, kind: &str, id: &str) -> String {
    ::engine::emulator_arn(ARN_SERVICE, region, &format!("{kind}/{id}"))
}

/// A listener's ARN, which nests under its service's:
/// `arn:aws:vpc-lattice:<region>:<account>:service/<service id>/listener/<listener id>`
/// (`ListenerArn`'s modelled pattern).
pub fn listener_arn(region: &str, service_id: &str, listener_id: &str) -> String {
    arn_for(
        region,
        &format!("{KIND_SERVICE}/{service_id}/{KIND_LISTENER}"),
        listener_id,
    )
}

/// A rule's ARN, which nests under its listener's:
/// `arn:aws:vpc-lattice:<region>:<account>:service/<service id>/listener/<listener
/// id>/rule/<rule id>` (`RuleArn`'s modelled pattern).
pub fn rule_arn(region: &str, service_id: &str, listener_id: &str, rule_id: &str) -> String {
    arn_for(
        region,
        &format!("{KIND_SERVICE}/{service_id}/{KIND_LISTENER}/{listener_id}/{KIND_RULE}"),
        rule_id,
    )
}

/// Mint a fresh resource id: `prefix` plus the 17-character suffix every id of this
/// service wears — the fixed leading [`ID_LEADING_CHARACTER`] and then
/// [`ID_ENTROPY_LEN`] characters of the LOWERCASE HEX alphabet ([`HEX_LOWER`]) — whatever
/// the kind.
///
/// SOURCE: recorded AWS behaviour. Every modelled id pattern of this service admits the
/// wider `[0-9a-z]` (`^svc-[0-9a-z]{17}$`, `^listener-[0-9a-z]{17}$`, …), so neither the
/// restriction to `[0-9a-f]` nor the leading character is modelled; every recorded id of
/// every kind — service network, service, association, listener, rule, target group —
/// renders a 64-bit value as 16 hex digits behind that one fixed character, the same
/// shape the EC2-family ids wear, and that form satisfies each modelled pattern and
/// length. The suffix also travels into every value that embeds an id: the resource ARNs
/// ([`arn_for`], [`listener_arn`], [`rule_arn`]), an error's `resourceId`, and the
/// generated DNS name ([`service_domain_name`]), so minting it here is the one place that
/// form is decided.
pub fn mint_id(prefix: &str) -> String {
    format!(
        "{prefix}{ID_LEADING_CHARACTER}{}",
        encode_in(fresh_entropy(), ID_ENTROPY_LEN, HEX_LOWER)
    )
}

/// Mint the Route 53 hosted-zone id of a service's DNS entry: `Z` plus 19 `[0-9A-Z]`
/// characters, the shape Route 53 gives a hosted zone.
pub fn mint_hosted_zone_id() -> String {
    format!("Z{}", encode_in(fresh_entropy(), 19, BASE36_UPPER))
}

/// The generated domain name of a service's DNS entry:
/// `<name>-<id suffix>.<account segment>.vpc-lattice-svcs.<region>.on.aws`, the form
/// VPC Lattice hands out. The account segment is derived from the account id, so every
/// service of one account shares it and it is stable across reads.
pub fn service_domain_name(name: &str, service_id: &str, region: &str) -> String {
    let suffix = service_id
        .strip_prefix(PREFIX_SERVICE)
        .unwrap_or(service_id);
    format!(
        "{name}-{suffix}.{}.vpc-lattice-svcs.{region}.on.aws",
        account_segment()
    )
}

/// The 7-hex account segment of a lattice domain name, derived from the account id.
fn account_segment() -> String {
    use sha2::{Digest, Sha256};
    let digest = Sha256::digest(account_id().as_bytes());
    digest[..4]
        .iter()
        .map(|byte| format!("{byte:02x}"))
        .collect::<String>()[..7]
        .to_string()
}

/// Entropy for one minted id.
///
/// Mixes this process's OS-seeded [`std::collections::hash_map::RandomState`] keys with
/// the clock and a process-wide counter, then hashes them, so ids are unique within a
/// run and unguessable across runs without any id ever being derived from the request.
fn fresh_entropy() -> u128 {
    use sha2::{Digest, Sha256};
    use std::hash::{BuildHasher, Hasher};
    use std::sync::atomic::{AtomicU64, Ordering};

    static MINTED: AtomicU64 = AtomicU64::new(0);

    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default()
        .as_nanos();
    let os_seeded = std::collections::hash_map::RandomState::new()
        .build_hasher()
        .finish();
    let digest = Sha256::digest(format!("{nanos}:{minted}:{os_seeded}").as_bytes());
    let mut bytes = [0_u8; 16];
    bytes.copy_from_slice(&digest[..16]);
    u128::from_be_bytes(bytes)
}

/// `value` rendered in `alphabet`, exactly `len` characters, most significant first.
fn encode_in(mut value: u128, len: usize, alphabet: &[u8]) -> String {
    let radix = alphabet.len() as u128;
    let mut out = vec![alphabet[0]; len];
    for slot in out.iter_mut().rev() {
        *slot = alphabet[(value % radix) as usize];
        value /= radix;
    }
    String::from_utf8(out).expect("the id alphabets are ASCII")
}

/// The resource id named by an identifier member, accepting both forms the model allows:
/// the bare id, or the resource's ARN (`arn:…:<kind>/<id>`).
///
/// `None` when the identifier names no id of `prefix`'s kind — which every handler
/// answers with `ResourceNotFoundException`, never with a lookup against another kind.
pub fn id_from_identifier(identifier: &str, prefix: &str) -> Option<String> {
    let tail = identifier
        .rsplit_once('/')
        .map_or(identifier, |(_, tail)| tail);
    let suffix = tail.strip_prefix(prefix)?;
    (suffix.len() == ID_SUFFIX_LEN
        && suffix
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit()))
    .then(|| tail.to_string())
}

/// The kinds a resource-based permission policy can be attached to, as the `ResourceArn`
/// the policy operations take enumerates them: a service network, a service, or a
/// resource configuration (`smithy_query(query="shape",shape="ResourceArn")`).
pub const POLICY_TARGET_KINDS: [ResourceKind; 3] = [
    ResourceKind::ServiceNetwork,
    ResourceKind::Service,
    ResourceKind::ResourceConfiguration,
];

/// The kind a resource-policy refusal names when the submitted `resourceArn` names no kind
/// a policy can be attached to at all: the first kind `ResourceArn` enumerates.
///
/// Unreachable for a request that satisfied that member's modelled pattern, which pairs
/// every ARN it accepts with one of [`POLICY_TARGET_KINDS`]; it is here so a handler can
/// still answer a modelled not-found instead of inventing a refusal the model does not
/// declare for it.
pub const UNRESOLVED_POLICY_TARGET_KIND: ResourceKind = POLICY_TARGET_KINDS[0];

/// The kind and id a resource-policy `resourceArn` names, or `None` when it names no
/// kind a policy can be attached to — which the policy operations answer with
/// `ResourceNotFoundException`.
///
/// `ResourceArn` admits only the ARN form, and its pattern pairs each kind's ARN path
/// segment with that kind's id prefix (`servicenetwork/sn-`, `service/svc-`,
/// `resourceconfiguration/rcfg-`), so both halves have to agree here; everything before
/// the kind segment is read the way [`id_from_identifier`] reads it, so an ARN of any
/// partition, region or account resolves to the id it names.
pub fn policy_target(resource_arn: &str) -> Option<(ResourceKind, String)> {
    let (head, tail) = resource_arn.rsplit_once('/')?;
    let arn_kind = head.rsplit_once(':').map_or(head, |(_, kind)| kind);
    POLICY_TARGET_KINDS.into_iter().find_map(|kind| {
        (arn_kind == kind.arn_kind())
            .then(|| id_from_identifier(tail, kind.id_prefix()))
            .flatten()
            .map(|id| (kind, id))
    })
}

/// The `@pattern` the model declares on `ServiceName`, which `CreateService.name`
/// targets. SOURCE: `smithy_query(query="shape",shape="ServiceName")`.
pub const SERVICE_NAME_PATTERN: &str = r"^(?!svc-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$";

/// The `@pattern` the model declares on `ServiceNetworkName`, which
/// `CreateServiceNetwork.name` targets.
/// SOURCE: `smithy_query(query="shape",shape="ServiceNetworkName")`.
pub const SERVICE_NETWORK_NAME_PATTERN: &str = r"^(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$";

/// The `@pattern` the model declares on `ListenerName`, which `CreateListener.name`
/// targets. SOURCE: `smithy_query(query="shape",shape="ListenerName")`.
pub const LISTENER_NAME_PATTERN: &str =
    r"^(?!listener-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$";

/// The `@pattern` the model declares on `RuleName`, which `CreateRule.name` targets.
/// SOURCE: `smithy_query(query="shape",shape="RuleName")`.
pub const RULE_NAME_PATTERN: &str = r"^(?!rule-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$";

/// The `@pattern` the model declares on `TargetGroupName`, which
/// `CreateTargetGroup.name` targets.
/// SOURCE: `smithy_query(query="shape",shape="TargetGroupName")`.
pub const TARGET_GROUP_NAME_PATTERN: &str = r"^(?!tg-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$";

/// The `@pattern` the model declares on `ResourceConfigurationName`, which
/// `CreateResourceConfiguration.name` targets.
/// SOURCE: `smithy_query(query="shape",shape="ResourceConfigurationName")`.
pub const RESOURCE_CONFIGURATION_NAME_PATTERN: &str =
    r"^(?!rcfg-)(?![-])(?!.*[-]$)(?!.*[-]{2})[a-z0-9-]+$";

/// The `@pattern`, with the whole-value anchors stripped, as a validation error reports
/// it.
///
/// The recorded vpc-lattice validation messages quote the pattern's BODY —
/// "Member must satisfy regular expression pattern: (?![-])…[a-z0-9-]+" — so the
/// anchors are dropped before the clause is built. The whole value is matched either
/// way, so they carry nothing for the caller.
///
/// The rule itself is the shared one every service renders a `@pattern` requirement
/// with (`service_common::validation::unanchored`), reached rather than restated here so
/// a constraint a handler checks by hand quotes its pattern exactly as one the request
/// boundary refuses does ([`crate::service`] renders the boundary's clauses through the
/// same function).
fn reported_pattern(pattern: &str) -> String {
    ::service_common::validation::unanchored(pattern)
}

/// One member whose submitted value broke a constraint, as this service reports it.
///
/// This is the framework's own violation type
/// (`aws_smithy_schema_server::constraints::Violation`), NOT a second one: a constraint
/// a handler has to check by hand is then reported by exactly the code that reports the
/// ones the boundary refuses before a handler runs.
pub type FieldViolation = Violation;

/// The modelled `ValidationExceptionReason` for a member whose value failed validation
/// (`smithy_query(query="shape",shape="ValidationExceptionReason")`).
pub const VALIDATION_REASON_FIELD: &str = "fieldValidationFailed";

/// AWS's `ValidationException` for members whose submitted values were refused.
///
/// The construction — which the recorded responses show, which the AWS SDKs and CLI
/// parse, and which the framework's [`envelope`] owns — is
/// "<N> validation error(s) detected: " over the clause
/// "Value '<submitted value>' at '<member path>' failed to satisfy constraint:
/// <requirement>" per violation, with `fieldList` carrying one entry per violation whose
/// own `message` is that clause without the count. The text is therefore DERIVED from
/// (member path, submitted value, violated constraint) — never a bespoke sentence per
/// check.
///
/// Every caller reports at least one violation; an empty slice describes no refusal and
/// is not a refusal this service can answer.
pub fn validation_exception(violations: &[FieldViolation]) -> ValidationException {
    debug_assert!(
        !violations.is_empty(),
        "a ValidationException has to report at least one violated member"
    );
    ValidationException {
        message: envelope(violations).unwrap_or_default(),
        reason: VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(
            violations
                .iter()
                .map(|violation| ValidationExceptionField {
                    name: violation.path.clone(),
                    message: violation.message(),
                })
                .collect(),
        ),
    }
}

/// One value a documented SERVICE RULE refuses — a rule the model carries no constraint
/// trait for (the fixed-response status-code catalog, a `port` that is mandatory under a
/// protocol with no documented default) — as this service reports one.
///
/// It is NOT a [`FieldViolation`]: a modelled constraint is reported by the construction
/// the framework owns (`"<N> validation error(s) detected: Value '…' at '<path>' failed
/// to satisfy constraint: …"`), whereas a service rule is reported with the rule's own
/// sentence over the bare member it constrains ([`service_rule_exception`]).
#[derive(Debug, Clone)]
pub struct ServiceRuleViolation {
    /// The input member the rule constrains, named as the MODEL names the member
    /// (`statusCode`), not the dotted path it sits at inside the request.
    pub member: String,
    /// The rule's refusal, built from the request.
    pub message: String,
}

/// AWS's `ValidationException` for a value a documented service RULE refuses.
///
/// `fieldList` carries one entry whose `name` is the bare member the rule constrains and
/// whose `message` is the rule's sentence, and the top-level `message` IS that sentence:
/// a service rule is not a modelled constraint, so it gets neither the
/// `"<N> validation error(s) detected: "` envelope nor the dotted member path
/// [`validation_exception`] builds for one.
///
/// SOURCE: recorded AWS behaviour for the fixed-response status-code rule (HTTP 400,
/// `fieldList[0].name` the bare member, `fieldList[0].message` equal to the top-level
/// message). `reason` stays the modelled [`VALIDATION_REASON_FIELD`]: the refusal is
/// still about one input member's value.
pub fn service_rule_exception(violation: &ServiceRuleViolation) -> ValidationException {
    ValidationException {
        message: violation.message.clone(),
        reason: VALIDATION_REASON_FIELD.to_string(),
        field_list: Some(vec![ValidationExceptionField {
            name: violation.member.clone(),
            message: violation.message.clone(),
        }]),
    }
}

/// Refuse a resource name that `kind`'s modelled `name` pattern forbids, as the
/// violation AWS reports: the member, the submitted name and the modelled pattern.
///
/// The pattern itself is evaluated (with `fancy-regex`, which supports the look-around
/// the patterns are written with), so the refusal and the constraint it reports can
/// never drift apart. It has to happen here: the boundary validator compiles a
/// `@pattern` with the `regex` crate, which has no look-around, and treats a pattern it
/// cannot compile as matching — so the name reaches the handler unchecked. The `@length`
/// bounds are a separate trait the boundary does enforce.
pub fn check_resource_name(
    kind: ResourceKind,
    member: &str,
    name: &str,
) -> Result<(), FieldViolation> {
    let Some(pattern) = kind.name_pattern() else {
        return Ok(());
    };
    // A match ERROR (the engine's backtrack limit) is deliberately read as "does not
    // match": a name is at most 63 characters by its `@length`, so no name this service
    // accepts can reach that limit, and refusing a name the service cannot prove legal
    // is the safe side — it never persists an unvalidated name.
    if compiled_pattern(kind).is_some_and(|compiled| compiled.is_match(name).unwrap_or(false)) {
        return Ok(());
    }
    Err(Violation {
        path: member.to_string(),
        kind: ViolationKind::Pattern(reported_pattern(pattern)),
        value: Some(name.to_string()),
        sensitive: false,
    })
}

/// This kind's modelled name `@pattern`, compiled once per kind; `None` for a kind
/// that has no name.
///
/// The pattern comes from [`ResourceKind::name_pattern`] — the ONE place a kind's
/// pattern is named, and read here rather than passed in, so the compiled regex and the
/// constraint a violation reports cannot drift apart; this only caches the compilation,
/// one slot per kind.
fn compiled_pattern(kind: ResourceKind) -> Option<&'static ::fancy_regex::Regex> {
    static COMPILED: [::std::sync::OnceLock<::fancy_regex::Regex>; ResourceKind::COUNT] = [
        ::std::sync::OnceLock::new(),
        ::std::sync::OnceLock::new(),
        ::std::sync::OnceLock::new(),
        ::std::sync::OnceLock::new(),
        ::std::sync::OnceLock::new(),
        ::std::sync::OnceLock::new(),
        ::std::sync::OnceLock::new(),
    ];
    let pattern = kind.name_pattern()?;
    Some(COMPILED[kind.slot()].get_or_init(|| {
        ::fancy_regex::Regex::new(pattern).expect("a modelled name @pattern compiles")
    }))
}

/// Refuse a `tags` map larger than the modelled `TagMap` ceiling, as the violation the
/// boundary would report for the same trait; `Ok(())` when it fits.
///
/// A map's value is rendered as its entry count, the way the framework's own collection
/// `@length` check renders one.
pub fn check_tag_count(tags: Option<&HashMap<String, String>>) -> Result<(), FieldViolation> {
    let count = tags.map_or(0, HashMap::len);
    if count > MAX_TAGS {
        return Err(Violation {
            path: "tags".to_string(),
            kind: ViolationKind::MaxLength(MAX_TAGS as i64),
            value: Some(format!("{count} entries")),
            sensitive: false,
        });
    }
    Ok(())
}

/// AWS's `ResourceNotFoundException` for an identifier that names no persisted resource
/// of `kind`.
///
/// The message is built from the request ([`ResourceKind::not_found_message`]): "<Kind>
/// <identifier> cannot be found.", over the identifier the caller actually sent (SOURCE:
/// recorded AWS behaviour — the model types `message` as a required `String` and states
/// no text, and every recorded not-found of this service reads this way). `resourceType`
/// is `kind`'s wire token.
pub fn resource_not_found(kind: ResourceKind, identifier: &str) -> ResourceNotFoundException {
    ResourceNotFoundException {
        message: kind.not_found_message(identifier),
        resource_id: identifier.to_string(),
        resource_type: kind.wire_type().to_string(),
    }
}

/// A `ConflictException` about a resource of `kind`, over that guard's own message.
///
/// Every conflict of this service goes through here, so `resourceType` is the kind's
/// wire token and never the internal registry key.
pub fn conflict(kind: ResourceKind, resource_id: &str, message: String) -> ConflictException {
    ConflictException {
        message,
        resource_id: resource_id.to_string(),
        resource_type: kind.wire_type().to_string(),
    }
}

/// AWS's conflict message for a create whose `name` is already taken: it names the
/// submitted name and the scope the name has to be unique in — `account` plus the
/// account id for an account-scoped name ([`duplicate_name_in_account_message`]). A name
/// scoped to an owning RESOURCE reads the other way round and has its own construction,
/// [`duplicate_name_in_resource_message`].
///
/// SOURCE: recorded AWS behaviour (409 `ConflictException`, account-scoped names); the
/// exact wording is an example of that construction, the identifiers in it are the
/// requirement.
pub fn duplicate_name_message(name: &str, scope_kind: &str, scope_id: &str) -> String {
    format!("Resource with name {name} already exists in {scope_kind} {scope_id}.")
}

/// [`duplicate_name_message`] for a name that is unique per account, the scope of a
/// service network's and a service's `name`.
pub fn duplicate_name_in_account_message(name: &str) -> String {
    duplicate_name_message(name, "account", account_id())
}

/// AWS's conflict message for a create whose `name` is already taken inside an owning
/// RESOURCE rather than inside the account: it names the kind whose name is taken, the
/// submitted name, and the resource the name has to be unique in (a listener's `name`
/// documents "A listener name must be unique within a service"; a rule's, "The name must
/// be unique within the listener").
///
/// SOURCE: recorded AWS behaviour — "Listener with name <name> already exists in
/// <service id> service." The construction (the kind, the submitted name, the owning
/// resource's id and kind) is the requirement; the exact sentence is an example of it.
pub fn duplicate_name_in_resource_message(
    kind: ResourceKind,
    name: &str,
    scope: ResourceKind,
    scope_id: &str,
) -> String {
    format!(
        "{} with name {name} already exists in {scope_id} {}.",
        kind.message_name(),
        scope.scope_name()
    )
}

/// AWS's conflict message for a write refused because a rule of the listener already has
/// the priority at issue — "Each rule for a specific listener must have a unique
/// priority" (the `priority` member's own documentation), restated by `RuleUpdate`'s as
/// "A listener can't have multiple rules with the same priority".
///
/// SOURCE: recorded AWS behaviour (409 `ConflictException`, from a rule create at a
/// priority another rule holds and from a batch update moving a rule onto one). The
/// construction — the wording over the priority that already exists — is the
/// requirement; which priority that is belongs to the refusal, and each caller passes
/// it: a create names the priority it asked for, which is the one the blocking rule
/// holds, while a batch update names the priority of the rule its refused entry named.
/// Every path that refuses the collision reports it through here so their wording cannot
/// drift apart.
pub fn duplicate_priority_message(priority: i32) -> String {
    format!("Rule with priority {priority} already exist!")
}

/// AWS's conflict message for a listener create that would bind a port another listener
/// of the same service already holds.
///
/// SOURCE: recorded AWS behaviour (409 `ConflictException` over the occupied port). The
/// port the request asked for is the requirement; the wording is an example.
pub fn port_occupied_message(port: i32) -> String {
    format!("Port {port} is occupied")
}

/// AWS's conflict message for a second association of a (service network, service) pair:
/// it names both ARNs the association links. Same source as
/// [`duplicate_name_message`].
pub fn duplicate_association_message(service_network_arn: &str, service_arn: &str) -> String {
    format!("Association for {service_network_arn} and {service_arn} already exists.")
}

/// A stored instant as the modelled timestamp.
pub fn timestamp(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_f64(since_epoch.as_secs_f64())
}

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsVpclatticeMercurycontrolplaneState>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsVpclatticeMercurycontrolplaneState>)
}
