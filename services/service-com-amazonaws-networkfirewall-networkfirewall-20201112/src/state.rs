// SPDX-License-Identifier: CC0-1.0

//! Engine state for the com-amazonaws-networkfirewall-networkfirewall-20201112 service.
//!
//! This module owns the service's persisted records and every projection from a
//! record onto a modelled shape, so each `src/ops/` handler only resolves,
//! mutates and projects — it never re-derives an ARN, an id or a status token.

use std::collections::{HashMap, HashSet};
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    Address, AnalysisResult, Attachment, EncryptionConfiguration, Firewall, FirewallMetadata,
    FirewallPolicy, FirewallPolicyMetadata, FirewallPolicyResponse, FirewallStatus, FlowFilter,
    FlowOperation, FlowOperationMetadata, LogDestinationConfig, LoggingConfiguration,
    MatchAttributes, PerObjectStatus, PortRange, RuleGroup, RuleGroupMetadata, RuleGroupResponse,
    SourceMetadata, StatelessRule, SubnetMapping, SummaryConfiguration, SyncState, Tag,
};

pub mod firewall_endpoints;
pub mod firewall_policies_v2;
pub mod log_destinations;
pub mod stateful_rules;

/// The ARN namespace (and SigV4 signing name) this service's ARNs carry, from the
/// model's `aws.api#service` trait: `arnNamespace: network-firewall`
/// (`smithy_query(query="service_info")`).
pub const ARN_NAMESPACE: &str = "network-firewall";

/// The region an ARN is built in when the request carries no SigV4 credential
/// scope to read one from, matching the region the emulator's generated client
/// configuration defaults to.
pub const DEFAULT_REGION: &str = "us-east-1";

/// How long after `CreateFirewall` the firewall's endpoints finish provisioning,
/// i.e. when `FirewallStatus.Status` becomes `READY`.
///
/// CHOSEN, not documented: AWS publishes no provisioning duration for Network
/// Firewall, so this is the emulator's own span. It is deliberately non-zero so a
/// caller that describes the firewall immediately still observes `PROVISIONING`,
/// and short enough that a test can wait it out.
pub const FIREWALL_PROVISIONING_SPAN: Duration = Duration::from_millis(500);

/// `FirewallType` members (`smithy_query(query="shape",shape="FirewallType")`):
/// "a standard Network Firewall firewall", which is the kind this service
/// creates and the one a request that names no type asks for ("if you don't
/// specify a type, the default is `FIREWALL`").
pub const FIREWALL_TYPE_FIREWALL: &str = "FIREWALL";
/// See [`FIREWALL_TYPE_FIREWALL`]. "An internet gateway firewall, which Amazon
/// Web Services Network Security Manager creates and manages for a VPC" — a kind
/// nothing in this service creates, since `CreateFirewall` refuses an
/// `IgwFirewallConfiguration`.
pub const FIREWALL_TYPE_IGW: &str = "IGW_FIREWALL";

/// Whether `firewall_type` — a request's `FirewallType`, absent or not — names
/// the kind of firewall this service holds.
///
/// "If you don't specify a type, the default is `FIREWALL`", so an absent member
/// asks for the standard kind; every other value asks for a kind this service
/// holds none of.
pub fn names_the_stored_firewall_type(firewall_type: Option<&str>) -> bool {
    firewall_type.is_none_or(|kind| kind == FIREWALL_TYPE_FIREWALL)
}

/// What a request asking to create or delete the NAT gateways of a firewall that
/// is not a proxy mode firewall is refused with: the rule the model states on
/// both members that take them ("you can't specify this for a firewall that
/// isn't a proxy mode firewall", and "you can't specify `TRUE` for a firewall
/// that isn't a proxy mode firewall"). It names neither the firewall nor the
/// mappings the request carried.
pub const NAT_GATEWAYS_NOT_PROXY_MODE_MESSAGE: &str =
    "You can't specify this for a firewall that isn't a proxy mode firewall.";

/// `FirewallStatusValue` members (`smithy_query(query="shape",shape="FirewallStatusValue")`).
pub const FIREWALL_PROVISIONING: &str = "PROVISIONING";
/// See [`FIREWALL_PROVISIONING`].
pub const FIREWALL_READY: &str = "READY";
/// See [`FIREWALL_PROVISIONING`].
pub const FIREWALL_DELETING: &str = "DELETING";

/// `ConfigurationSyncState` / `PerObjectSyncStatus` member `IN_SYNC`.
pub const SYNC_IN_SYNC: &str = "IN_SYNC";

/// `AttachmentStatus` members (`smithy_query(query="shape",shape="AttachmentStatus")`).
pub const ATTACHMENT_CREATING: &str = "CREATING";
/// See [`ATTACHMENT_CREATING`].
pub const ATTACHMENT_READY: &str = "READY";
/// See [`ATTACHMENT_CREATING`].
pub const ATTACHMENT_DELETING: &str = "DELETING";

/// `FlowOperationType` members (`smithy_query(query="shape",shape="FlowOperationType")`).
pub const FLOW_OPERATION_FLUSH: &str = "FLOW_FLUSH";
/// See [`FLOW_OPERATION_FLUSH`].
pub const FLOW_OPERATION_CAPTURE: &str = "FLOW_CAPTURE";

/// `FlowOperationStatus` members
/// (`smithy_query(query="shape",shape="FlowOperationStatus")`).
pub const FLOW_OPERATION_IN_PROGRESS: &str = "IN_PROGRESS";
/// See [`FLOW_OPERATION_IN_PROGRESS`].
pub const FLOW_OPERATION_COMPLETED: &str = "COMPLETED";

/// How long after a flow start command (`StartFlowFlush`, `StartFlowCapture`) the
/// operation leaves `IN_PROGRESS` for `COMPLETED`.
///
/// CHOSEN, not documented: AWS publishes no duration for a flow operation — only
/// that a start command is asynchronous (`DescribeFlowOperation$StatusMessage` is
/// "if the asynchronous operation fails ... the reason for the error or failure",
/// and the status a start command reports is what the describe command is then
/// polled for). It is deliberately non-zero so a caller that describes the
/// operation immediately still observes `IN_PROGRESS`, and short enough that a
/// test can wait it out.
pub const FLOW_OPERATION_SPAN: Duration = Duration::from_millis(500);

/// `ResourceStatus` members (`smithy_query(query="shape",shape="ResourceStatus")`).
pub const RESOURCE_ACTIVE: &str = "ACTIVE";
/// See [`RESOURCE_ACTIVE`].
pub const RESOURCE_DELETING: &str = "DELETING";

/// `RuleGroupType` members (`smithy_query(query="shape",shape="RuleGroupType")`).
pub const RULE_GROUP_STATELESS: &str = "STATELESS";
/// See [`RULE_GROUP_STATELESS`].
pub const RULE_GROUP_STATEFUL: &str = "STATEFUL";
/// See [`RULE_GROUP_STATELESS`].
pub const RULE_GROUP_STATEFUL_DOMAIN: &str = "STATEFUL_DOMAIN";
/// See [`RULE_GROUP_STATELESS`]. A firewall ENGINE rule group: its rules are
/// the ones a Next Generation policy's firewall section evaluates.
pub const RULE_GROUP_FIREWALL: &str = "FIREWALL";
/// See [`RULE_GROUP_STATELESS`]. An IPS ENGINE rule group: its rules are the
/// ones a Next Generation policy's IPS section evaluates.
pub const RULE_GROUP_IPS: &str = "IPS";

/// The `RuleGroupType` members that name an ENGINE rule group — one written
/// through a `FirewallRuleGroup` or an `IpsRuleGroup` document rather than
/// through the `RuleGroup` one every other type takes.
pub const RULE_GROUP_ENGINE_TYPES: [&str; 2] = [RULE_GROUP_FIREWALL, RULE_GROUP_IPS];

/// `ResourceManagedStatus` members
/// (`smithy_query(query="shape",shape="ResourceManagedStatus")`), the scope a
/// `ListRuleGroups` request asks for.
pub const SCOPE_ACCOUNT: &str = "ACCOUNT";
/// See [`SCOPE_ACCOUNT`].
pub const SCOPE_MANAGED: &str = "MANAGED";

/// `EncryptionType` members (`smithy_query(query="shape",shape="EncryptionType")`).
pub const ENCRYPTION_AWS_OWNED_KMS_KEY: &str = "AWS_OWNED_KMS_KEY";

/// `IdentifiedType` members (`smithy_query(query="shape",shape="IdentifiedType")`).
pub const IDENTIFIED_FORWARDING_ASYMMETRICALLY: &str = "STATELESS_RULE_FORWARDING_ASYMMETRICALLY";
/// See [`IDENTIFIED_FORWARDING_ASYMMETRICALLY`].
pub const IDENTIFIED_CONTAINS_TCP_FLAGS: &str = "STATELESS_RULE_CONTAINS_TCP_FLAGS";

/// The stateless standard actions that forward a packet on rather than dropping
/// it, and so are the ones the stateless analyzer looks at
/// (`RuleDefinition$Actions`: "aws:pass ... permits it to go to its intended
/// destination", "aws:forward_to_sfe ... forwards it to the stateful rule engine").
pub const STATELESS_FORWARDING_ACTIONS: [&str; 2] = ["aws:pass", "aws:forward_to_sfe"];

/// The encryption settings a resource reports, given what the request asked for.
///
/// `EncryptionConfiguration`'s own documentation states the default: "Your data
/// is encrypted by default with an Amazon Web Services owned key that Amazon Web
/// Services owns and manages for you", so an absent member means the AWS-owned
/// key — never "no encryption settings at all".
///
/// Both members are then reported: `KeyId` names the key protecting the
/// resource, and the identifier of the AWS-owned key is the
/// [`ENCRYPTION_AWS_OWNED_KMS_KEY`] token itself (the model does not state this,
/// so it is RECORDED behaviour: every recorded response for a resource created
/// without an `EncryptionConfiguration` carries
/// `{Type: "AWS_OWNED_KMS_KEY", KeyId: "AWS_OWNED_KMS_KEY"}`). A customer
/// managed key keeps the identifier the request gave, which is the only key id
/// the service can report for it.
pub fn effective_encryption_configuration(
    requested: Option<EncryptionConfiguration>,
) -> EncryptionConfiguration {
    let mut effective = requested.unwrap_or_else(|| EncryptionConfiguration {
        key_id: None,
        r#type: ENCRYPTION_AWS_OWNED_KMS_KEY.to_string(),
    });
    if effective.r#type == ENCRYPTION_AWS_OWNED_KMS_KEY && effective.key_id.is_none() {
        effective.key_id = Some(ENCRYPTION_AWS_OWNED_KMS_KEY.to_string());
    }
    effective
}

/// The tag collection a response reports for a resource it READ out of state or
/// REMOVED from it: the stored list, empty when the resource carries no tags.
///
/// A read and a delete answer from a stored record rather than echoing a
/// request, so every collection on that record is projected — a tag list with no
/// entries is reported as an empty list, not as an absent member. RECORDED
/// behaviour: the describe and the delete responses for a resource created with
/// no `Tags` carry `Tags: []`, where that resource's create and update responses
/// (which report the member as the request gave it) carry none.
pub fn projected_resource_tags(stored: Option<Vec<Tag>>) -> Option<Vec<Tag>> {
    Some(stored.unwrap_or_default())
}

/// The upper bound of `PaginationMaxResults` (`range 1..100`), and therefore the
/// page size a request that omits `MaxResults` gets.
pub const MAX_PAGE_SIZE: i32 = 100;

/// The highest number of distinct Availability Zones one firewall can span here:
/// every firewall subnet must sit in a different Availability Zone, and
/// [`availability_zone_of`] names them `<region>a`..`<region>z`.
pub const MAX_AVAILABILITY_ZONES: usize = 26;

/// How many firewalls this emulator keeps.
///
/// CHOSEN, not an AWS quota: every `CreateFirewall` retains a record for as long
/// as the firewall is not deleted, so without a ceiling a caller that loops with
/// fresh names grows the emulator's memory without bound. The ceiling is far
/// above any workflow an emulator serves, and a request past it is refused with
/// the modelled `LimitExceededException` ("Unable to perform the operation
/// because doing so would violate a limit setting").
pub const MAX_STORED_FIREWALLS: usize = 1_000;

/// How many firewall policies this emulator keeps. See [`MAX_STORED_FIREWALLS`].
pub const MAX_STORED_FIREWALL_POLICIES: usize = 1_000;

/// How many rule groups this emulator keeps. See [`MAX_STORED_FIREWALLS`].
pub const MAX_STORED_RULE_GROUPS: usize = 1_000;

/// How many `FlowFilters` entries one flow operation may carry: "You can use up
/// to 20 filters to configure a single flow operation"
/// (`StartFlowFlush$FlowFilters`, and the same sentence on every other member
/// that takes them). The model puts no length constraint on `FlowFilters`, so the
/// handler is what enforces the documented ceiling.
pub const MAX_FLOW_FILTERS: usize = 20;

/// How many flow operations this emulator keeps per firewall.
///
/// CHOSEN, not an AWS quota: every start command retains a record for as long as
/// its firewall lives, so without a ceiling a caller that loops grows the
/// emulator's memory without bound. It is far above any workflow an emulator
/// serves, and a request past it is refused with the modelled
/// `ThrottlingException` ("Unable to process the request due to throttling
/// limitations") — the only volume refusal a flow start command models.
pub const MAX_STORED_FLOW_OPERATIONS: usize = 1_000;

/// The highest `Capacity` a rule group may be created with here.
///
/// The model puts NO range on `RuleCapacity`, so the ceiling is enforced by the
/// handler rather than by the constraint validator, and a request above it is
/// refused as the malformed input it is: the modelled `InvalidRequestException`
/// ("you specified an unsupported parameter name or value") carrying the built
/// capacity refusal ([`declared_capacity_exceeded_message`]), and NOT a
/// quota condition a client would retry against.
///
/// 30,000 units is the ceiling the recorded service refuses above; it also bounds
/// what one request can make this emulator retain, since a rule group's rules are
/// bounded only by the capacity they consume ([`required_capacity`] must not
/// exceed `Capacity`).
pub const MAX_RULE_GROUP_CAPACITY: i32 = 30_000;

/// How many entries any one list inside a retained `FirewallPolicy` document may
/// carry.
///
/// CHOSEN, not an AWS quota: the model puts NO length constraint on the policy's
/// rule-group reference, default-action or custom-action lists, so one request
/// could otherwise make a single retained record arbitrarily large. AWS bounds
/// the same thing by rule CAPACITY (the `Consumed*RuleCapacity` members), which
/// this emulator totals from the rule groups a policy references (see
/// [`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::consumed_policy_capacity`]);
/// this entry count bounds the lists that carry no capacity of their own and is
/// far above any real policy. A request past it is refused with the modelled
/// `LimitExceededException`.
pub const MAX_POLICY_LIST_ENTRIES: usize = 64;

/// The first list in `policy` that exceeds [`MAX_POLICY_LIST_ENTRIES`], named
/// with its length, or `None` when the document is within bounds.
pub fn policy_document_breach(policy: &FirewallPolicy) -> Option<String> {
    let lists = [
        (
            "StatelessRuleGroupReferences",
            policy
                .stateless_rule_group_references
                .as_ref()
                .map_or(0, Vec::len),
        ),
        (
            "StatefulRuleGroupReferences",
            policy
                .stateful_rule_group_references
                .as_ref()
                .map_or(0, Vec::len),
        ),
        (
            "StatelessCustomActions",
            policy.stateless_custom_actions.as_ref().map_or(0, Vec::len),
        ),
        (
            "StatelessDefaultActions",
            policy.stateless_default_actions.len(),
        ),
        (
            "StatelessFragmentDefaultActions",
            policy.stateless_fragment_default_actions.len(),
        ),
        (
            "StatefulDefaultActions",
            policy.stateful_default_actions.as_ref().map_or(0, Vec::len),
        ),
    ];
    lists
        .into_iter()
        .find(|(_, length)| *length > MAX_POLICY_LIST_ENTRIES)
        .map(|(member, length)| {
            format!(
                "FirewallPolicy.{member} has {length} entries; at most \
                 {MAX_POLICY_LIST_ENTRIES} are allowed."
            )
        })
}

/// The standard actions a firewall policy's stateless default-action lists may
/// name.
///
/// Documented on both members that take them
/// (`FirewallPolicy$StatelessDefaultActions` and
/// `FirewallPolicy$StatelessFragmentDefaultActions`, identical text): "You must
/// specify one of the standard actions: aws:pass, aws:drop, or
/// aws:forward_to_sfe. In addition, you can specify custom actions that are
/// compatible with your standard section choice." The target shape is a plain
/// list of string rather than an enum, so the vocabulary is enforced here rather
/// than by the model's constraint validator.
pub const STATELESS_STANDARD_ACTIONS: [&str; 3] = ["aws:pass", "aws:drop", "aws:forward_to_sfe"];

/// The input paths a firewall policy document's stateless default-action lists
/// arrive on, which are also the member names its refusals name.
pub const STATELESS_DEFAULT_ACTION_MEMBERS: [&str; 2] = [
    "StatelessDefaultActions",
    "StatelessFragmentDefaultActions",
];

/// Whether `action` names one of the [`STATELESS_STANDARD_ACTIONS`].
///
/// The `aws:` namespace prefix is PART of a standard action's name — every one
/// of them is documented with it — so an entry that carries no namespace names
/// no standard action, and an entry whose namespace is not that one names an
/// action of some other vocabulary. Neither half is case-sensitive, so a request
/// may spell the name in any case and the service keeps the spelling it was
/// given. A token the vocabulary does not carry names no standard action, however
/// it is namespaced.
pub fn is_standard_stateless_action(action: &str) -> bool {
    let Some((namespace, token)) = action.split_once(':') else {
        return false;
    };
    if !namespace.eq_ignore_ascii_case("aws") {
        return false;
    }
    STATELESS_STANDARD_ACTIONS.iter().any(|standard| {
        let standard = standard.split_once(':').map_or(*standard, |(_, token)| token);
        token.eq_ignore_ascii_case(standard)
    })
}

/// Why `policy`'s stateless default actions are not a set of actions this
/// service can act on, or `None` when both lists name one it can.
///
/// Three conditions, all stated by the documentation of the members that take
/// them (`FirewallPolicy$StatelessDefaultActions` and
/// `FirewallPolicy$StatelessFragmentDefaultActions`, identical text: "You must
/// specify one of the standard actions: aws:pass, aws:drop, or
/// aws:forward_to_sfe. In addition, you can specify custom actions that are
/// compatible with your standard section choice"):
///
/// * neither list may be empty: one of them has to carry the default this
///   policy applies, so there is nothing for an empty one to specify.
/// * every entry must NAME an action — one of the
///   [`STATELESS_STANDARD_ACTIONS`] ([`is_standard_stateless_action`]) or a
///   custom action the SAME document declares in `StatelessCustomActions` ("You
///   name each custom action that you define, and then you can use it by name in
///   your default actions specifications" — `FirewallPolicy$StatelessCustomActions`;
///   a custom action is named, not namespaced, so its name must match as
///   declared). Anything else names no action at all.
/// * one list may name only ONE standard action: it is the single default the
///   policy applies to a packet no stateless rule matched, so two DIFFERENT
///   ones in the same list cannot both be it (the same action named twice names
///   one action).
///
/// Either way the document is refused rather than retained: a stored policy whose
/// default action nothing implements, or whose default is ambiguous, would
/// silently handle every non-matching packet differently from the policy the
/// caller asked for.
///
/// Each refusal is built FROM the request in the service's request-derived
/// message shapes: the stem names the member that failed and what failed about
/// it, the context is the input path it looked at
/// ([`context_message`]), and where there IS an offending value the brackets
/// carry it verbatim ([`parameter_context_message`]) — the one entry that names
/// nothing, or every standard action the list names, in request order. The first
/// failure is reported, lists in the order [`STATELESS_DEFAULT_ACTION_MEMBERS`]
/// names them and, within one list, in the order the conditions are stated above.
///
/// The document's own declared custom action names are collected into a SET, so
/// the scan costs one lookup per default action however many custom actions the
/// document declares, and the whole check is linear in the document's size. Both
/// handlers run it after [`policy_document_breach`], which caps every one of
/// those lists at [`MAX_POLICY_LIST_ENTRIES`].
pub fn stateless_default_action_breach(policy: &FirewallPolicy) -> Option<String> {
    let declared: HashSet<&str> = policy
        .stateless_custom_actions
        .iter()
        .flatten()
        .map(|custom| custom.action_name.as_str())
        .collect();
    let members: [(&str, &Vec<String>); 2] = [
        (
            STATELESS_DEFAULT_ACTION_MEMBERS[0],
            &policy.stateless_default_actions,
        ),
        (
            STATELESS_DEFAULT_ACTION_MEMBERS[1],
            &policy.stateless_fragment_default_actions,
        ),
    ];

    members
        .iter()
        .find_map(|(member, actions)| stateless_action_list_breach(member, member, actions, &declared))
}

/// Why the list `member` carries is not a set of actions this service can act
/// on, or `None` when it is, applying the three conditions
/// [`stateless_default_action_breach`] states in the order it states them.
///
/// `member` names the list in the stem and `context` is the input path it sits
/// on: the two coincide for a top-level member of a firewall policy document,
/// and differ for a list nested inside a document (a stateless rule's `Actions`,
/// [`stateless_rule_action_breach`]). The CONDITIONS are the same wherever the
/// list sits, because they are conditions on the one documented action
/// vocabulary, so there is one implementation of them.
fn stateless_action_list_breach(
    member: &str,
    context: &str,
    actions: &[String],
    declared: &HashSet<&str>,
) -> Option<String> {
    if actions.is_empty() {
        return Some(context_message(
            &format!("{member} cannot be null or empty"),
            context,
        ));
    }

    let unnamed = actions.iter().find(|action| {
        !is_standard_stateless_action(action) && !declared.contains(&action.as_str())
    });
    if let Some(action) = unnamed {
        return Some(parameter_context_message(
            &format!("{member} is invalid"),
            action,
            context,
        ));
    }

    // One list may name only one standard action, and an action named twice is
    // still ONE action: what a spelling NAMES is the standard token it carries
    // ([`standard_action_token`]).
    let mut named: HashSet<String> = HashSet::new();
    let mut standard: Vec<&str> = Vec::new();
    for action in actions {
        if is_standard_stateless_action(action) && named.insert(standard_action_token(action)) {
            standard.push(action);
        }
    }
    (standard.len() > 1).then(|| {
        parameter_context_message(
            &format!("{member} cannot exist together"),
            &standard.join(", "),
            context,
        )
    })
}

/// The standard action `action` names, as one comparable token: its name with
/// the `aws:` namespace dropped and the case normalised.
///
/// Two entries name the same standard action exactly when this is equal for
/// both, since neither half of the name is case-sensitive
/// ([`is_standard_stateless_action`]).
fn standard_action_token(action: &str) -> String {
    action
        .split_once(':')
        .map_or(action, |(_, token)| token)
        .to_ascii_lowercase()
}

/// The member a firewall policy document's stateful default actions arrive on,
/// which is also the member name its refusals name and the input path they
/// report as their context.
pub const STATEFUL_DEFAULT_ACTIONS_MEMBER: &str = "StatefulDefaultActions";

/// The rule order a stateful default action is valid under: "The stateful default
/// action is optional, and is only valid when using the strict rule order"
/// (`FirewallPolicy$StatefulDefaultActions`).
pub const STRICT_RULE_ORDER: &str = "STRICT_ORDER";

/// The rule order in force when a policy document names none:
/// "`STRICT_ORDER` is the recommended option, but `DEFAULT_ACTION_ORDER` is the
/// default option" (`StatefulEngineOptions$RuleOrder`). Both values are the two
/// `RuleOrder` carries ([`crate::ops::types::RULE_ORDER_VALUES`]).
pub const DEFAULT_RULE_ORDER: &str = "DEFAULT_ACTION_ORDER";

/// The actions a firewall policy's `StatefulDefaultActions` may name.
///
/// The documented value list of `FirewallPolicy$StatefulDefaultActions`
/// (`smithy_query(query="doc",shape="FirewallPolicy",member="StatefulDefaultActions")`:
/// "Valid values of the stateful default action"), in the order it states them.
/// The target shape is a plain list of string rather than an enum, so the
/// vocabulary is enforced here rather than by the model's constraint validator.
pub const STATEFUL_DEFAULT_ACTIONS: [&str; 8] = [
    "aws:drop_strict",
    "aws:drop_established",
    "aws:alert_strict",
    "aws:alert_established",
    "aws:drop_established_app_layer",
    "aws:alert_established_app_layer",
    "aws:drop_established_app_layer_to_server",
    "aws:alert_established_app_layer_to_server",
];

/// The rule order `policy` is evaluated under: the one its
/// `StatefulEngineOptions` names, or [`DEFAULT_RULE_ORDER`] when the document
/// leaves it out — which is what "`DEFAULT_ACTION_ORDER` is the default option"
/// (`StatefulEngineOptions$RuleOrder`) means. Whatever arrives here is one of
/// [`crate::ops::types::RULE_ORDER_VALUES`]: the value set is a declared
/// enumeration, so a value
/// outside it is refused before any handler runs.
pub fn effective_rule_order(policy: &FirewallPolicy) -> &str {
    policy
        .stateful_engine_options
        .as_ref()
        .and_then(|options| options.rule_order.as_deref())
        .unwrap_or(DEFAULT_RULE_ORDER)
}

/// Why `policy`'s stateful default actions are not a set of actions this service
/// can act on, or `None` when they are — including when the document names none.
///
/// Two conditions, both stated by the documentation of the member that takes them
/// (`FirewallPolicy$StatefulDefaultActions`: "The default actions to take on a
/// packet that doesn't match any stateful rules. The stateful default action is
/// optional, and is only valid when using the strict rule order", followed by the
/// list of "Valid values of the stateful default action"):
///
/// * the member may only be USED under the strict rule order, so a document that
///   names a stateful default action while any other rule order is in effect
///   ([`effective_rule_order`]) is refused for the order it is asking for, not
///   for the actions it names — the order governs whether the member may carry
///   anything at all, so it is judged first;
/// * every entry must name one of the [`STATEFUL_DEFAULT_ACTIONS`]. The
///   documented names are lower-case and this vocabulary is matched as published,
///   so a differently-cased spelling names none of them.
///
/// A document naming no stateful default action — the member absent, or present
/// and empty — specifies nothing for either condition to judge, which is what
/// makes the action "optional" under both rule orders.
///
/// Each refusal is built FROM the request in the service's request-derived
/// message shape ([`parameter_context_message`]): the stem names the member and
/// what failed about it, the brackets carry the offending value — the rule order
/// in effect, or the rejected action verbatim as the request spelled it — and the
/// context is the input path the check looked at. The first failure is reported,
/// entries in request order. Both handlers run this after
/// [`policy_document_breach`], which caps the list at
/// [`MAX_POLICY_LIST_ENTRIES`].
///
/// Either way the document is refused rather than retained: a stored policy whose
/// stateful default action nothing implements, or that asks for a default action
/// the rule order it runs under does not apply, would silently inspect traffic
/// differently from the policy the caller asked for.
pub fn stateful_default_action_breach(policy: &FirewallPolicy) -> Option<String> {
    let actions = policy.stateful_default_actions.as_ref()?;
    if actions.is_empty() {
        return None;
    }

    let rule_order = effective_rule_order(policy);
    if rule_order != STRICT_RULE_ORDER {
        return Some(parameter_context_message(
            &format!("{STATEFUL_DEFAULT_ACTIONS_MEMBER} has invalid rule order"),
            rule_order,
            STATEFUL_DEFAULT_ACTIONS_MEMBER,
        ));
    }

    let unsupported = actions
        .iter()
        .find(|action| !STATEFUL_DEFAULT_ACTIONS.contains(&action.as_str()))?;
    Some(parameter_context_message(
        &format!("{STATEFUL_DEFAULT_ACTIONS_MEMBER} has invalid action"),
        unsupported,
        STATEFUL_DEFAULT_ACTIONS_MEMBER,
    ))
}

/// How many `LogDestinationConfigs` entries one logging configuration may carry.
///
/// CHOSEN, not an AWS quota: the model puts NO length constraint on the list, and
/// every entry is both retained on the firewall's record and resolved against the
/// service that owns its destination
/// ([`log_destinations::attachment_refused`]) — so without a ceiling one request
/// could make a single retained record arbitrarily large and ask the destination
/// owners arbitrarily many questions. It is far above any firewall's logging:
/// `LogType` has three members, and each can be sent to each of the three
/// `LogDestinationType` members. A request past it is refused as the bad request
/// it is, since the update models no limit error.
pub const MAX_LOG_DESTINATION_CONFIGS: usize = 16;

/// What a logging configuration past [`MAX_LOG_DESTINATION_CONFIGS`] is refused
/// with, naming the member, how many entries it carried and how many are allowed
/// — the same shape [`policy_document_breach`] states its own ceiling in.
pub fn too_many_log_destinations_message(found: usize) -> String {
    format!(
        "LoggingConfiguration.LogDestinationConfigs has {found} entries; at most \
         {MAX_LOG_DESTINATION_CONFIGS} are allowed."
    )
}

/// How many resource policies this emulator keeps.
///
/// A resource policy exists only for a resource that is still stored — the
/// attach path refuses an ARN that names none, and deleting a resource detaches
/// its policy — so this is exactly the number of resources that can carry one. A
/// request past it is refused rather than retained, so no sequence of calls can
/// grow the store beyond it even if that invariant is ever broken.
pub const MAX_STORED_RESOURCE_POLICIES: usize =
    MAX_STORED_FIREWALL_POLICIES + MAX_STORED_RULE_GROUPS;

/// The `RulesSource` members a rule group's rules can be given in, as the
/// model names them (`smithy_query(query="shape",shape="RulesSource")`).
pub const RULES_SOURCE_STATELESS_RULES: &str = "StatelessRulesAndCustomActions";
/// See [`RULES_SOURCE_STATELESS_RULES`].
pub const RULES_SOURCE_STATEFUL_RULES: &str = "StatefulRules";
/// See [`RULES_SOURCE_STATELESS_RULES`].
pub const RULES_SOURCE_LIST: &str = "RulesSourceList";
/// See [`RULES_SOURCE_STATELESS_RULES`].
pub const RULES_SOURCE_RULES_STRING: &str = "RulesString";

/// The `RulesSource` member `group`'s rules were given in, for an error message
/// that names the offending part of the request. `rule_group_document_breach`
/// has already refused a document that does not set exactly one of them.
pub fn rules_source_member(group: &RuleGroup) -> &'static str {
    let source = &group.rules_source;
    if source.stateless_rules_and_custom_actions.is_some() {
        RULES_SOURCE_STATELESS_RULES
    } else if source.stateful_rules.is_some() {
        RULES_SOURCE_STATEFUL_RULES
    } else if source.rules_source_list.is_some() {
        RULES_SOURCE_LIST
    } else {
        RULES_SOURCE_RULES_STRING
    }
}

/// The `RulesSource` member `group`'s rules are ACCOUNTED under — the member
/// holding the individual rules the service read out of the document, which is
/// what [`required_capacity`] totals and therefore what a capacity refusal
/// names.
///
/// It differs from [`rules_source_member`], the member the request GAVE the
/// rules in, for exactly one source: rules in Suricata flat format arrive as "a
/// string containing stateful rule group rules specifications in Suricata flat
/// format, with one rule per line", and "the call response returns a RuleGroup
/// object that Network Firewall has populated from your string"
/// (`CreateRuleGroupRequest$Rules`), so that string's lines ARE the rule group's
/// individual stateful rules — counted one apiece by [`required_capacity`] — and
/// the member they are accounted under is `StatefulRules`, whichever way the
/// string reached the service (the top-level `Rules` setting or
/// `RuleGroup.RulesSource.RulesString`).
pub fn accounted_rules_source_member(group: &RuleGroup) -> &'static str {
    if group.rules_source.rules_string.is_some() {
        RULES_SOURCE_STATEFUL_RULES
    } else {
        rules_source_member(group)
    }
}

/// `group` as this service STORES it, which is what every later read of the rule
/// group reports.
///
/// One normalisation, over a stateless rule group's rules: `StatelessRule$Priority`
/// "indicates the order in which to run this rule relative to all of the rules
/// that are defined for a stateless rule group. Network Firewall evaluates the
/// rules in a rule group starting with the lowest priority setting", and the
/// priorities of one rule group are unique ([`stateless_rule_breach`] refuses a
/// document where they are not), so the rules have ONE evaluation order and the
/// stored list is held in it — ascending by priority — however the request
/// happened to order them. A read then reports the rules in the order they run,
/// and two requests that differ only in the order they list the same rules store
/// the same rule group.
///
/// Nothing else about the document is rewritten: a stateful source has no
/// ordering of its own (its rules are evaluated by the engine's own rule order,
/// `StatefulRuleOptions$RuleOrder`) and is stored exactly as it was given.
pub fn stored_rule_group_document(mut group: RuleGroup) -> RuleGroup {
    if let Some(stateless) = group.rules_source.stateless_rules_and_custom_actions.as_mut() {
        stateless
            .stateless_rules
            .sort_by_key(|rule| rule.priority);
    }
    group
}

/// Why `group` is not a rule group document of type `group_type`, or `None` when
/// it is one.
///
/// Two documented rules: "Each rule group requires a single `RulesSource`. You
/// can use an instance of this for either stateless rules or stateful rules"
/// (`RulesSource`), so exactly one of the four `RulesSource` members is set and
/// the one that is set must belong to the half the `Type` names; and
/// "<RuleVariables> ... You can only use these for stateful rule groups"
/// (`RuleGroup$RuleVariables`).
///
/// A member set on the wrong half is refused in the service's request-derived
/// shape with a context and no bracketed value ([`context_message`]): the member
/// itself is what is in the wrong place, so the stem names it and the context is
/// the input path it arrived on, and no part of the submitted document is quoted
/// back.
pub fn rule_group_document_breach(group_type: &str, group: &RuleGroup) -> Option<String> {
    let source = &group.rules_source;
    let stateless = source.stateless_rules_and_custom_actions.is_some();
    let members = [
        (RULES_SOURCE_RULES_STRING, source.rules_string.is_some()),
        (RULES_SOURCE_LIST, source.rules_source_list.is_some()),
        (RULES_SOURCE_STATEFUL_RULES, source.stateful_rules.is_some()),
        (RULES_SOURCE_STATELESS_RULES, stateless),
    ];
    let given: Vec<&str> = members
        .into_iter()
        .filter_map(|(member, present)| present.then_some(member))
        .collect();
    if given.len() != 1 {
        return Some(format!(
            "RuleGroup.RulesSource must specify exactly one of RulesString, RulesSourceList, \
             StatefulRules or StatelessRulesAndCustomActions; this request specified {}.",
            if given.is_empty() {
                "none of them".to_string()
            } else {
                given.join(", ")
            }
        ));
    }
    if (group_type == RULE_GROUP_STATELESS) != stateless {
        let member = rules_source_member(group);
        return Some(context_message(
            &format!("{member} is invalid"),
            &format!("RulesSource.{member}"),
        ));
    }
    if group_type == RULE_GROUP_STATELESS && group.rule_variables.is_some() {
        return Some(
            "RuleGroup.RuleVariables can only be used in a stateful rule group.".to_string(),
        );
    }
    None
}

/// The input path one stateless rule of a submitted rule group document sits on,
/// as this service's refusals name it.
///
/// The rule is located by its `Priority` rather than by its position in the
/// list, which is what `StatelessRule$Priority` makes possible: "You must ensure
/// that the priority settings are unique for the rule group", so the priority
/// identifies one rule of the document ([`stateless_rule_breach`] refuses a
/// document where it does not).
pub fn stateless_rule_path(priority: i32) -> String {
    format!("StatelessRulesAndCustomActions.StatelessRules[Priority={priority}]")
}

/// Why `group`'s stateless rules are not rules this service can act on, or
/// `None` when they are — including for a document that carries no stateless
/// rules at all, which this check has nothing to say about.
///
/// Two conditions, each stated by the documentation of the member it is about:
///
/// * a priority identifies ONE rule of the group: "You must ensure that the
///   priority settings are unique for the rule group ... To ensure unique
///   priority settings for your rule groups, set unique priorities for the
///   stateless rules that you define inside any single `StatelessRules` object"
///   (`StatelessRule$Priority`). Two rules sharing one is therefore a bad
///   request, not a rule group with an undefined evaluation order;
/// * every entry of a rule's `Actions` must NAME an action — one of the
///   [`STATELESS_STANDARD_ACTIONS`] ([`is_standard_stateless_action`]) or a
///   custom action the SAME `StatelessRulesAndCustomActions` declares
///   ("Additionally, you can specify a custom action. To do this, you define a
///   custom action by name and type, then provide the name you've assigned to
///   the action in this `Actions` setting") — the list may not be empty, and
///   only ONE of its entries may be standard: "For every rule, you must specify
///   exactly one of the following standard actions" (`RuleDefinition$Actions`).
///   These are the conditions a firewall policy's stateless default actions are
///   read by, because it is the same documented vocabulary, so they have one
///   implementation ([`stateless_action_list_breach`]) and this rule states only
///   where the list sits.
///
/// Either way the document is refused rather than retained: a stored rule group
/// whose rules have no defined order, or whose action nothing implements, would
/// inspect traffic differently from the rule group the caller asked for.
///
/// Each refusal takes the service's request-derived message shape
/// ([`parameter_context_message`]): the stem names the member and what failed
/// about it, the brackets carry the offending value, and the context is the input
/// path that value sits on ([`stateless_rule_path`]). The conditions are reported
/// in the order they are stated above — the collection's own invariant before any
/// one rule's contents — and within one condition the first rule in request order.
pub fn stateless_rule_breach(group: &RuleGroup) -> Option<String> {
    let stateless = group
        .rules_source
        .stateless_rules_and_custom_actions
        .as_ref()?;

    let mut seen: HashSet<i32> = HashSet::new();
    for rule in &stateless.stateless_rules {
        if !seen.insert(rule.priority) {
            let priority = rule.priority.to_string();
            return Some(parameter_context_message(
                "Priority has duplicate values",
                &priority,
                &stateless_rule_path(rule.priority),
            ));
        }
    }

    // The document's own custom action names, collected once, so checking one
    // rule's actions costs one lookup per action however many the document
    // declares.
    let declared: HashSet<&str> = stateless
        .custom_actions
        .iter()
        .flatten()
        .map(|custom| custom.action_name.as_str())
        .collect();
    stateless
        .stateless_rules
        .iter()
        .find_map(|rule| stateless_rule_action_breach(rule, &declared))
}

/// Why `rule`'s `Actions` are not actions this service can act on: the one
/// implementation of the documented action vocabulary
/// ([`stateless_action_list_breach`]), applied to the list this rule carries,
/// with the member stated as `Actions` and the context the input path that list
/// sits on — the rule located by its priority ([`stateless_rule_path`]).
fn stateless_rule_action_breach(rule: &StatelessRule, declared: &HashSet<&str>) -> Option<String> {
    let context = format!(
        "{}.RuleDefinition.Actions",
        stateless_rule_path(rule.priority)
    );
    stateless_action_list_breach(
        "Actions",
        &context,
        &rule.rule_definition.actions,
        declared,
    )
}

/// The actions a resource policy on a shared RULE GROUP MAY grant, from the AWS
/// `PutResourcePolicy` documentation's list of operations that may appear in the
/// statement for a rule group resource ("For a rule group resource, you can
/// specify the following operations in the Actions section of the statement").
///
/// It is a PERMITTED set, not a required one: a statement may grant any non-empty
/// subset of it (see [`resource_policy_matches_permissions`]).
pub const RULE_GROUP_POLICY_ACTIONS: [&str; 3] = [
    "network-firewall:ListRuleGroups",
    "network-firewall:CreateFirewallPolicy",
    "network-firewall:UpdateFirewallPolicy",
];

/// The actions a resource policy on a shared FIREWALL POLICY may grant. See
/// [`RULE_GROUP_POLICY_ACTIONS`].
pub const FIREWALL_POLICY_POLICY_ACTIONS: [&str; 2] = [
    "network-firewall:AssociateFirewallPolicy",
    "network-firewall:ListFirewallPolicies",
];

/// The two resource kinds Network Firewall can share through a resource policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SharedResource {
    /// A rule group, named by a `*-rulegroup/<name>` ARN.
    RuleGroup,
    /// A firewall policy, named by a `firewall-policy/<name>` ARN.
    FirewallPolicy,
}

impl SharedResource {
    /// The actions a policy on this kind of resource may grant — the set its
    /// statements' actions must be drawn FROM, not a set they must all name.
    pub fn permitted_actions(self) -> &'static [&'static str] {
        match self {
            Self::RuleGroup => &RULE_GROUP_POLICY_ACTIONS,
            Self::FirewallPolicy => &FIREWALL_POLICY_POLICY_ACTIONS,
        }
    }
}

/// What `arn` names, or `None` when it names a resource kind that cannot carry a
/// resource policy (a firewall, say).
pub fn shared_resource_kind(arn: &str) -> Option<SharedResource> {
    let (resource_type, _) = own_resource(arn)?;
    match resource_type {
        ARN_TYPE_FIREWALL_POLICY => Some(SharedResource::FirewallPolicy),
        group_type if ARN_TYPES_RULE_GROUP.contains(&group_type) => Some(SharedResource::RuleGroup),
        _ => None,
    }
}

/// The `(resource type, resource name)` the ARN `arn` names, or `None` when it
/// names no resource of THIS service.
///
/// The ARN grammar is the workspace's one caller-supplied-ARN parse
/// ([`::service_common::arn`]), so this service splits no ARN itself. What it
/// adds is what a Network Firewall ARN is: its own vendor namespace — another
/// vendor's resource is addressable by no operation here, whatever its resource
/// field spells — and a resource field naming both a TYPE and a resource
/// (`<type>/<name>`, or the `<type>:<name>` form AWS also writes), which every
/// one of this service's resources takes.
pub fn own_resource(arn: &str) -> Option<(&str, &str)> {
    let parsed = ::service_common::arn::parse_of(arn, ARN_NAMESPACE)?;
    let resource_type = parsed.resource_type();
    let resource_name = ::service_common::arn::resource_id_of_type(arn, resource_type)?;
    (!resource_type.is_empty() && !resource_name.is_empty())
        .then_some((resource_type, resource_name))
}

/// The ARN resource type a firewall carries (`firewall/<name>`).
pub const ARN_TYPE_FIREWALL: &str = "firewall";

/// The ARN resource type a firewall policy carries (`firewall-policy/<name>`).
pub const ARN_TYPE_FIREWALL_POLICY: &str = "firewall-policy";

/// The ARN resource type a stateless rule group carries.
pub const ARN_TYPE_STATELESS_RULE_GROUP: &str = "stateless-rulegroup";

/// The ARN resource type a stateful rule group carries.
pub const ARN_TYPE_STATEFUL_RULE_GROUP: &str = "stateful-rulegroup";

/// The ARN resource type a stateful DOMAIN rule group carries.
pub const ARN_TYPE_STATEFUL_DOMAIN_RULE_GROUP: &str = "stateful-domain-rulegroup";

/// The ARN resource types a rule group carries, one per half of the rule-group
/// name space plus the stateful-domain form of it — the types
/// [`rule_group_resource_path`] builds and the ones a rule-group reference may
/// name.
pub const ARN_TYPES_RULE_GROUP: [&str; 3] = [
    ARN_TYPE_STATELESS_RULE_GROUP,
    ARN_TYPE_STATEFUL_RULE_GROUP,
    ARN_TYPE_STATEFUL_DOMAIN_RULE_GROUP,
];

/// The resource kinds Network Firewall tags: "You can tag the Amazon Web Services
/// resources that you manage through Network Firewall: firewalls, firewall
/// policies, and rule groups" (`TagResource`, `UntagResource` and
/// `ListTagsForResource` all state it), which is exactly the set of resources
/// this state holds.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TaggableResource {
    /// A firewall, named by a `firewall/<name>` ARN.
    Firewall,
    /// A firewall policy, named by a `firewall-policy/<name>` ARN.
    FirewallPolicy,
    /// A rule group, named by a `*-rulegroup/<name>` ARN.
    RuleGroup,
}

/// What `arn` names, or `None` when it names a resource kind Network Firewall
/// does not tag — an ARN "that is malformed, or corresponds to a resource that
/// isn't valid in the context of the request", which is what
/// `InvalidRequestException` documents.
pub fn taggable_resource_kind(arn: &str) -> Option<TaggableResource> {
    let (resource_type, _) = own_resource(arn)?;
    match resource_type {
        ARN_TYPE_FIREWALL => Some(TaggableResource::Firewall),
        ARN_TYPE_FIREWALL_POLICY => Some(TaggableResource::FirewallPolicy),
        group_type if ARN_TYPES_RULE_GROUP.contains(&group_type) => {
            Some(TaggableResource::RuleGroup)
        }
        _ => None,
    }
}

/// How many tags one resource may carry: "You can specify one or more tags to add
/// to each Amazon Web Services resource, up to 50 tags for a resource"
/// (`TagResource`, and the same sentence on `UntagResource` and
/// `ListTagsForResource`). DOCUMENTED, not chosen — and it is also what bounds the
/// tag state one resource can accumulate.
pub const MAX_TAGS_PER_RESOURCE: usize = 50;

/// What a tag write that would take a resource past [`MAX_TAGS_PER_RESOURCE`] is
/// refused with. The ceiling is a property of the service, not of the request, so
/// the message names the limit and the resulting count and quotes nothing from the
/// request.
pub fn tag_limit_message(requested: usize) -> String {
    format!(
        "A resource can have at most {MAX_TAGS_PER_RESOURCE} tags; this request would leave it \
         with {requested}."
    )
}

/// What a request naming an ARN of a kind Network Firewall does not tag is refused
/// with: the service's one request-derived shape (see [`parameter_message`]), a
/// fixed stem plus the offending ARN in parameter brackets.
pub fn untaggable_arn_message(arn: &str) -> String {
    parameter_message(
        "resource ARN does not name a firewall, firewall policy or rule group",
        arn,
    )
}

/// Why a tag write could not be served.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TagWriteBreach {
    /// No stored resource carries that ARN.
    NotFound,
    /// The write would leave the resource with more than
    /// [`MAX_TAGS_PER_RESOURCE`] tags, carrying the count it would reach.
    TooManyTags(usize),
}

/// Whether `document` is a resource policy this service accepts for a resource of
/// `kind`.
///
/// A Network Firewall resource policy is validated against the actions the shared
/// resource's TYPE admits ([`SharedResource::permitted_actions`]), and the rule is
/// CONTAINMENT, not set equality: the documentation lists the operations a
/// statement "can specify" for each resource type, so every action the policy
/// grants must come from that type's set, while the actions it omits are simply
/// not granted — a caller that shares a resource with the least privilege it needs
/// grants a SUBSET and is accepted. An action outside the set is one the
/// documentation does not allow the statement to name, so it fails validation
/// (`InvalidResourcePolicyException`, "The policy statement failed validation.").
/// The document itself must also be a policy: a JSON object with at least one
/// `Allow` statement that names a principal and an action.
///
/// The answer is a yes/no on purpose: every way of failing is answered with the
/// one [`INVALID_RESOURCE_POLICY_MESSAGE`], so no detail of the submitted
/// document — an offending action, a parse position — can reach the wire.
pub fn resource_policy_matches_permissions(document: &str, kind: SharedResource) -> bool {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(document) else {
        return false;
    };
    if !parsed.is_object() {
        return false;
    }
    let statements: Vec<&::serde_json::Value> = match parsed.get("Statement") {
        Some(::serde_json::Value::Array(statements)) => statements.iter().collect(),
        Some(statement) if statement.is_object() => vec![statement],
        _ => return false,
    };
    if statements.is_empty() {
        return false;
    }

    let mut granted: ::std::collections::BTreeSet<String> = ::std::collections::BTreeSet::new();
    for statement in statements {
        if statement.get("Effect").and_then(::serde_json::Value::as_str) != Some("Allow") {
            return false;
        }
        if statement.get("Principal").is_none() {
            return false;
        }
        match statement.get("Action") {
            Some(::serde_json::Value::String(action)) => {
                granted.insert(action.clone());
            }
            Some(::serde_json::Value::Array(actions)) if !actions.is_empty() => {
                for action in actions {
                    match action.as_str() {
                        Some(action) => {
                            granted.insert(action.to_string());
                        }
                        None => return false,
                    }
                }
            }
            _ => return false,
        }
    }

    let permitted = kind.permitted_actions();
    granted
        .iter()
        .all(|action| permitted.contains(&action.as_str()))
}

/// The principal type whose values name an AWS account or one of its identities:
/// the one whose account values [`stored_resource_policy`] writes as root ARNs.
const AWS_PRINCIPAL_TYPE: &str = "AWS";

/// A resource policy as this service HOLDS one, for the reads that report it.
///
/// The policy language lets a statement name a whole account by its bare account
/// id, and that id and the account's root ARN name the SAME principal — so the
/// document a policy store holds carries the root ARN whichever of the two was
/// submitted ([`::service_common::iam_policy_principals::account_root_principal`],
/// the tree's one spelling of an account principal), and a client comparing the
/// document it reads against the one it sent sees one spelling rather than
/// rewriting the policy forever.
///
/// Only a value of the `AWS` principal type that is a bare account id is written:
/// every other value of that element — the wildcard, an identity's ARN, a root ARN
/// already — is the caller's own, and so is every other byte of the document,
/// member order, list form and the whitespace between the tokens included
/// ([`::service_common::iam_policy_rendering::rewritten_values`], which walks the
/// text rather than re-serialising it). That keeps this a FIXED POINT: a document
/// it wrote is one it leaves alone, so re-putting what was read stores the same
/// bytes again.
pub fn stored_resource_policy(document: &str) -> String {
    ::service_common::iam_policy_rendering::rewritten_values(
        document,
        &::service_common::iam_policy_rendering::ElementValues {
            element: AWS_PRINCIPAL_TYPE,
            written_as: &::service_common::iam_policy_principals::account_root_principal,
        },
    )
}

/// Sixteen bytes nothing else in this process will produce again: the first half
/// of a SHA-256 (the `sha2` crate, not a hand-written hash) over a seed that is
/// unique per call.
///
/// The seed is a process-wide counter that strictly increases (so two mints in
/// one clock tick differ), the current instant (so a restarted process does not
/// repeat the sequence) and one operating-system seeded value per process
/// (`RandomState`'s keys are OS-seeded the first time one is built, which is the
/// entropy `std` exposes), so ids minted by different processes differ too.
/// NOTHING about the request enters the seed: an id is never derived from client
/// input.
///
/// This is the same scheme the sibling service crates mint their modelled ids
/// with (for example `service-com-amazonaws-synthetics-synthetics`'s
/// `state::new_uuid`).
fn fresh_bytes() -> [u8; 16] {
    use ::sha2::Digest as _;
    use ::std::hash::BuildHasher as _;
    use ::std::sync::atomic::{AtomicU64, Ordering};
    use ::std::sync::OnceLock;

    /// Distinguishes two values minted in the same instant by the same process.
    static MINTED: AtomicU64 = AtomicU64::new(0);
    /// One value per process, drawn from the operating system.
    static PROCESS_SEED: OnceLock<u64> = OnceLock::new();

    let process_seed = *PROCESS_SEED.get_or_init(|| {
        ::std::collections::hash_map::RandomState::new().hash_one(::std::process::id())
    });
    let minted = MINTED.fetch_add(1, Ordering::Relaxed);
    let nanos = SystemTime::now()
        .duration_since(SystemTime::UNIX_EPOCH)
        .map(|since_epoch| since_epoch.as_nanos())
        .unwrap_or_default();

    let mut digest = ::sha2::Sha256::new();
    digest.update(process_seed.to_le_bytes());
    digest.update(minted.to_le_bytes());
    digest.update(nanos.to_le_bytes());
    let mut bytes = [0u8; 16];
    bytes.copy_from_slice(&digest.finalize()[..16]);
    bytes
}

/// A fresh server-generated id in the `ResourceId` / `UpdateToken` form
/// (`^([0-9a-f]{8})-([0-9a-f]{4}-){3}([0-9a-f]{12})$`, length 36), which is
/// exactly a lowercase-hyphenated version-4 UUID: [`fresh_bytes`] laid out with
/// the version-4 and RFC 4122 variant nibbles and hex-encoded (the `hex` crate,
/// not a hand-written codec) into the five modelled groups.
pub fn new_uuid() -> String {
    let mut bytes = fresh_bytes();
    // Version 4 in the high nibble of byte 6 and the RFC 4122 variant in the top
    // bits of byte 8, the layout a random UUID carries on the wire.
    bytes[6] = (bytes[6] & 0x0f) | 0x40;
    bytes[8] = (bytes[8] & 0x3f) | 0x80;

    let digits = ::hex::encode(bytes);
    format!(
        "{}-{}-{}-{}-{}",
        &digits[0..8],
        &digits[8..12],
        &digits[12..16],
        &digits[16..20],
        &digits[20..32]
    )
}

/// The region this request was signed for, read from its SigV4 credential scope
/// so every ARN an operation mints matches the caller's own endpoint region.
pub fn request_region(ctx: &smithy_server_runtime::OperationContext<'_>) -> String {
    emulator_server::sigv4_credential_scope(&ctx.parts().headers)
        .map(|scope| scope.region)
        .unwrap_or_else(|| DEFAULT_REGION.to_string())
}

/// `arn:aws:network-firewall:<region>:<account>:firewall/<name>`.
pub fn firewall_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("firewall/{name}"))
}

/// `arn:aws:network-firewall:<region>:<account>:firewall-policy/<name>`.
pub fn firewall_policy_arn(region: &str, name: &str) -> String {
    emulator_server::emulator_arn(ARN_NAMESPACE, region, &format!("firewall-policy/{name}"))
}

/// The resource part of a rule group's ARN, and therefore the key its record is
/// stored under: `stateless-rulegroup/<name>` for a `STATELESS` rule group and
/// `stateful-rulegroup/<name>` for every stateful one.
///
/// The type is part of the resource path because `DescribeRuleGroup$Type` is
/// "required for requests that do not include the `RuleGroupARN`" — the ARN
/// alone identifies a rule group, so it must carry what the name alone does not.
/// Two rule groups therefore collide exactly when they share this path, which is
/// what makes a name reusable across the stateless and stateful halves of the
/// service.
pub fn rule_group_resource_path(group_type: &str, name: &str) -> String {
    format!("{}/{name}", rule_group_arn_type(group_type))
}

/// The ARN resource type a rule group of `group_type` carries: the stateless
/// half of the name space has its own, and every stateful type shares the other.
pub fn rule_group_arn_type(group_type: &str) -> &'static str {
    if group_type == RULE_GROUP_STATELESS {
        ARN_TYPE_STATELESS_RULE_GROUP
    } else {
        ARN_TYPE_STATEFUL_RULE_GROUP
    }
}

/// `arn:aws:network-firewall:<region>:<account>:<stateful|stateless>-rulegroup/<name>`.
pub fn rule_group_arn(region: &str, group_type: &str, name: &str) -> String {
    emulator_server::emulator_arn(
        ARN_NAMESPACE,
        region,
        &rule_group_resource_path(group_type, name),
    )
}

/// The capacity the rules in `group` consume, by the calculation
/// `CreateRuleGroupRequest$Capacity` documents.
///
/// For a stateless rule group it is "the sum of the capacity requirements of the
/// individual rules", where one rule's requirement is the product of its match
/// settings' values and a setting that is absent (which matches anything) counts
/// as 1. For a stateful rule group it is "the number of individual rules": one
/// per `StatefulRules` entry, one per rule line of a Suricata `RulesString`, or
/// one per domain in a `RulesSourceList`.
pub fn required_capacity(group_type: &str, group: &RuleGroup) -> i32 {
    let source = &group.rules_source;
    let total: i64 = if group_type == RULE_GROUP_STATELESS {
        source
            .stateless_rules_and_custom_actions
            .as_ref()
            .map(|stateless| {
                stateless
                    .stateless_rules
                    .iter()
                    .map(|rule| {
                        let attributes = &rule.rule_definition.match_attributes;
                        let settings = [
                            attributes.sources.as_ref().map_or(0, Vec::len),
                            attributes.destinations.as_ref().map_or(0, Vec::len),
                            attributes.source_ports.as_ref().map_or(0, Vec::len),
                            attributes.destination_ports.as_ref().map_or(0, Vec::len),
                            attributes.protocols.as_ref().map_or(0, Vec::len),
                            attributes.tcp_flags.as_ref().map_or(0, Vec::len),
                        ];
                        settings
                            .into_iter()
                            .map(|elements| i64::try_from(elements.max(1)).unwrap_or(i64::MAX))
                            .fold(1i64, i64::saturating_mul)
                    })
                    .fold(0i64, i64::saturating_add)
            })
            .unwrap_or_default()
    } else {
        let from_rules = source.stateful_rules.as_ref().map_or(0, Vec::len);
        let from_string = source
            .rules_string
            .as_deref()
            .map_or(0, |rules| suricata_rule_lines(rules).count());
        let from_domains = source
            .rules_source_list
            .as_ref()
            .map_or(0, |list| list.targets.len());
        i64::try_from(from_rules + from_string + from_domains).unwrap_or(i64::MAX)
    };
    i32::try_from(total).unwrap_or(i32::MAX)
}

/// The individual rules in a Suricata flat-format rules string: one per line,
/// skipping blank lines and `#` comments, which carry no rule.
pub fn suricata_rule_lines(rules: &str) -> impl Iterator<Item = &str> {
    rules
        .lines()
        .map(str::trim)
        .filter(|line| !line.is_empty() && !line.starts_with('#'))
}

/// The stateless rule-group analyzer's findings for `group`.
///
/// `AnalysisResult` documents the analyzer as running over STATELESS rule groups
/// and `IdentifiedType` documents exactly the two configurations it reports, each
/// for rules "with the action `pass` or `forward`":
///
/// * `STATELESS_RULE_FORWARDING_ASYMMETRICALLY` — "the rule's set of source IP
///   addresses or their associated port numbers, don't match the set of
///   destination IP addresses or their associated port numbers", mitigated by
///   there being "an existing return path". A rule is therefore reported unless
///   some forwarding rule in the group matches the mirrored traffic (its sources
///   and ports are this rule's destinations and ports, and vice versa); a rule
///   that names no addresses or ports matches anything in both directions and so
///   mirrors itself.
/// * `STATELESS_RULE_CONTAINS_TCP_FLAGS` — a forwarding rule "contains TCP
///   flags", whose mitigation is to "remove unnecessary TCP flag inspections
///   from the rules".
///
/// `IdentifiedRuleIds` is "the priority number of the stateless rules identified
/// in the analysis", so each finding lists the priorities it was raised for, in
/// the order the rules were given.
///
/// The pass is linear in the number of rules (the return paths are indexed once)
/// and the caller runs it OUTSIDE the state lock, so one request's analysis
/// neither scales faster than its own input nor blocks another caller's.
pub fn analyze_stateless_rule_group(group: &RuleGroup) -> Vec<AnalysisResult> {
    let Some(stateless) = group.rules_source.stateless_rules_and_custom_actions.as_ref() else {
        return Vec::new();
    };
    let forwarding: Vec<_> = stateless
        .stateless_rules
        .iter()
        .filter(|rule| {
            rule.rule_definition
                .actions
                .iter()
                .any(|action| STATELESS_FORWARDING_ACTIONS.contains(&action.as_str()))
        })
        .collect();

    // Indexed once, so looking a rule's return path up is a hash lookup rather
    // than a scan of the other rules: the whole pass costs one unit of work per
    // rule, whatever a caller sends.
    let directions: Vec<RuleDirection> = forwarding
        .iter()
        .map(|rule| RuleDirection::of(&rule.rule_definition.match_attributes))
        .collect();
    let indexed: HashSet<&RuleDirection> = directions.iter().collect();

    let mut asymmetric = Vec::new();
    let mut with_tcp_flags = Vec::new();
    for (rule, direction) in forwarding.iter().zip(&directions) {
        if !indexed.contains(&direction.reversed()) {
            asymmetric.push(rule.priority.to_string());
        }
        if rule
            .rule_definition
            .match_attributes
            .tcp_flags
            .as_ref()
            .is_some_and(|flags| !flags.is_empty())
        {
            with_tcp_flags.push(rule.priority.to_string());
        }
    }

    let mut results = Vec::new();
    if !asymmetric.is_empty() {
        results.push(AnalysisResult {
            identified_rule_ids: Some(asymmetric),
            identified_type: Some(IDENTIFIED_FORWARDING_ASYMMETRICALLY.to_string()),
            analysis_detail: Some(
                "The rule forwards traffic asymmetrically: no rule in this rule group \
                 matches the return traffic for the addresses and ports it matches."
                    .to_string(),
            ),
        });
    }
    if !with_tcp_flags.is_empty() {
        results.push(AnalysisResult {
            identified_rule_ids: Some(with_tcp_flags),
            identified_type: Some(IDENTIFIED_CONTAINS_TCP_FLAGS.to_string()),
            analysis_detail: Some(
                "The rule inspects TCP flags, which are not consistent in the forward and \
                 return directions of a connection."
                    .to_string(),
            ),
        });
    }
    results
}

/// The traffic one stateless rule matches, as comparable sets.
///
/// Two rules describe the same traffic when their sets are equal whatever order
/// the match settings were written in, and a rule's return path is this with the
/// source and destination halves swapped.
#[derive(Debug, Clone, PartialEq, Eq, Hash)]
struct RuleDirection {
    /// `MatchAttributes.Sources`, as address definitions.
    sources: Vec<String>,
    /// `MatchAttributes.Destinations`, as address definitions.
    destinations: Vec<String>,
    /// `MatchAttributes.SourcePorts`, as `from:to` ranges.
    source_ports: Vec<String>,
    /// `MatchAttributes.DestinationPorts`, as `from:to` ranges.
    destination_ports: Vec<String>,
}

impl RuleDirection {
    /// The traffic `attributes` matches.
    fn of(attributes: &MatchAttributes) -> Self {
        Self {
            sources: address_set(attributes.sources.as_ref()),
            destinations: address_set(attributes.destinations.as_ref()),
            source_ports: port_set(attributes.source_ports.as_ref()),
            destination_ports: port_set(attributes.destination_ports.as_ref()),
        }
    }

    /// The same traffic flowing the other way: the return path a symmetric rule
    /// group also matches.
    fn reversed(&self) -> Self {
        Self {
            sources: self.destinations.clone(),
            destinations: self.sources.clone(),
            source_ports: self.destination_ports.clone(),
            destination_ports: self.source_ports.clone(),
        }
    }
}

/// The address definitions of a match setting as a set, so two settings compare
/// equal when they match the same addresses whatever order they were given in.
/// An absent setting is the empty set, which is how "matches with any address" is
/// written.
fn address_set(addresses: Option<&Vec<Address>>) -> Vec<String> {
    let mut definitions: Vec<String> = addresses
        .map(|addresses| {
            addresses
                .iter()
                .map(|address| address.address_definition.clone())
                .collect()
        })
        .unwrap_or_default();
    definitions.sort();
    definitions.dedup();
    definitions
}

/// The port ranges of a match setting as a set. See [`address_set`].
fn port_set(ports: Option<&Vec<PortRange>>) -> Vec<String> {
    let mut ranges: Vec<String> = ports
        .map(|ports| {
            ports
                .iter()
                .map(|range| format!("{}:{}", range.from_port, range.to_port))
                .collect()
        })
        .unwrap_or_default();
    ranges.sort();
    ranges.dedup();
    ranges
}

/// The Availability Zone the firewall endpoint for the `index`-th subnet mapping
/// lives in.
///
/// The emulator holds no VPC/subnet inventory, so it cannot look a subnet's zone
/// up. Instead each firewall subnet is given its own zone in the request's
/// region, in request order, which is exactly the invariant the model documents
/// for `Firewall.SubnetMappings`: "each must belong to a different Availability
/// Zone".
pub fn availability_zone_of(region: &str, index: usize) -> String {
    // Unreachable today: `CreateFirewall` refuses more than
    // `MAX_AVAILABILITY_ZONES` subnet mappings before any zone is named, so the
    // modulo never wraps two subnets onto one zone key.
    debug_assert!(
        index < MAX_AVAILABILITY_ZONES,
        "subnet index {index} should have been refused before reaching here"
    );
    let letter = char::from(b'a' + u8::try_from(index % MAX_AVAILABILITY_ZONES).unwrap_or(0));
    format!("{region}{letter}")
}

/// A firewall endpoint id (`vpce-` plus 17 hex digits, the shape AWS VPC endpoint
/// ids take).
///
/// The FALLBACK id of an attachment: a firewall endpoint is a VPC endpoint Amazon EC2 holds
/// and reports the id of ([`firewall_endpoints::instantiate`]), so this is only reached
/// where there is no EC2 in this process to instantiate one in — the id then names no
/// endpoint at all (see the [`firewall_endpoints`] module docs).
pub fn new_endpoint_id() -> String {
    // `hex::encode` renders the 16 bytes as exactly 32 digits, so the 17-digit
    // prefix is always there.
    let digits = ::hex::encode(fresh_bytes());
    format!("vpce-{}", &digits[..17])
}

/// The attachment record for a firewall endpoint in `subnet_id`, which starts out
/// [`ATTACHMENT_CREATING`] whichever endpoint it carries.
///
/// `instantiated` is the endpoint Amazon EC2 created for that subnet, when it created one:
/// the attachment reports the id EC2 minted and is deleted there with the attachment. An
/// attachment EC2 could not be asked for falls back to [`new_endpoint_id`].
pub fn new_sync_state(subnet_id: &str, instantiated: Option<String>) -> SyncStateRecord {
    let instantiated_in_ec2 = instantiated.is_some();
    SyncStateRecord {
        subnet_id: subnet_id.to_string(),
        endpoint_id: instantiated.unwrap_or_else(new_endpoint_id),
        instantiated_in_ec2,
        attachment_status: ATTACHMENT_CREATING.to_string(),
    }
}

/// Render a `SystemTime` as the modelled timestamp type.
///
/// Built from whole seconds plus the sub-second nanoseconds rather than a float,
/// so the stored instant round-trips exactly.
pub fn as_date_time(at: SystemTime) -> ::aws_smithy_types::DateTime {
    let since_epoch = at
        .duration_since(SystemTime::UNIX_EPOCH)
        .unwrap_or_default();
    ::aws_smithy_types::DateTime::from_secs_and_nanos(
        i64::try_from(since_epoch.as_secs()).unwrap_or(i64::MAX),
        since_epoch.subsec_nanos(),
    )
}

/// The listing kind [`ListFirewalls`](crate::ops::list_firewalls) stamps on the
/// cursors it mints, so a cursor is only ever resumed by the listing that issued
/// it.
pub const PAGE_KIND_FIREWALLS: &str = "firewalls";

/// The listing kind [`ListFirewallPolicies`](crate::ops::list_firewall_policies)
/// stamps on its cursors. See [`PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_FIREWALL_POLICIES: &str = "firewall-policies";

/// The listing kind [`ListFlowOperations`](crate::ops::list_flow_operations)
/// stamps on its cursors. See [`PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_FLOW_OPERATIONS: &str = "flow-operations";

/// The listing kind [`ListRuleGroups`](crate::ops::list_rule_groups) stamps on
/// its cursors. See [`PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_RULE_GROUPS: &str = "rule-groups";

/// The listing kind
/// [`ListTagsForResource`](crate::ops::list_tags_for_resource) stamps on its
/// cursors. See [`PAGE_KIND_FIREWALLS`].
pub const PAGE_KIND_TAGS: &str = "tags";

/// Base64 of `<kind>:<key>` standing for "resume after this key" in that
/// listing's order.
///
/// Base64 (`aws_smithy_types::base64`, the codec AWS's own opaque tokens use)
/// keeps the token inside the modelled `PaginationToken` character class
/// (`^[0-9A-Za-z:\/+=]+$`), which a raw resource name does not: `ResourceName`
/// admits `-`, which the token class forbids. The `kind` prefix is what lets
/// [`decode_page_token`] refuse a cursor another listing minted; no
/// `ResourceName` can contain `:`, so the prefix is unambiguous.
pub fn encode_page_token(kind: &str, after_key: &str) -> String {
    ::aws_smithy_types::base64::encode(format!("{kind}:{after_key}"))
}

/// The key a cursor of `kind` names, or `None` when the token is not one THAT
/// listing issued.
pub fn decode_page_token(kind: &str, token: &str) -> Option<String> {
    let decoded = String::from_utf8(::aws_smithy_types::base64::decode(token).ok()?).ok()?;
    let (decoded_kind, after_key) = decoded.split_once(':')?;
    (decoded_kind == kind).then(|| after_key.to_string())
}

/// One page of a listing: the items, plus the token that resumes after them when
/// more remain.
#[derive(Debug)]
pub struct Page<T> {
    /// The items on this page.
    pub items: Vec<T>,
    /// Present only when at least one further item remains.
    pub next_token: Option<String>,
}

/// A `NextToken` the listing it was sent to did not mint.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct InvalidPageToken;

/// Cut `entries` — key-ordered `(key, item)` pairs — into the page `next_token`
/// and `max_results` ask for.
///
/// Resuming by KEY rather than by offset keeps a resumed page correct across a
/// delete between the two calls: the second page starts at the first key after
/// the one the token names, whether or not that key still exists.
///
/// A `max_results` of 0 is a page of no items — `TagsPaginationMaxResults` admits
/// it — and is answered like any other full page: with the cursor that resumes
/// where this call left off, so nothing is lost and nothing is repeated.
pub fn paginate<T>(
    mut entries: Vec<(String, T)>,
    kind: &str,
    next_token: Option<&str>,
    max_results: Option<i32>,
) -> Result<Page<T>, InvalidPageToken> {
    // Where this page starts: the key the incoming cursor names, and the empty
    // key — which sorts before every real one — for a first page.
    let mut resume_after = String::new();
    if let Some(token) = next_token {
        let after = decode_page_token(kind, token).ok_or(InvalidPageToken)?;
        entries.retain(|(key, _)| key.as_str() > after.as_str());
        resume_after = after;
    }
    // `MaxResults` carries its listing's modelled range (`PaginationMaxResults`
    // is 1..100, `TagsPaginationMaxResults` 0..100), which the boundary validator
    // already refused outside, so this clamp is unreachable defence and never the
    // rule: an out-of-range value is answered with a 400 before this runs, not
    // silently narrowed.
    debug_assert!(
        max_results.is_none_or(|requested| (0..=MAX_PAGE_SIZE).contains(&requested)),
        "MaxResults should have been refused at the boundary: {max_results:?}"
    );
    let limit = max_results.unwrap_or(MAX_PAGE_SIZE).clamp(0, MAX_PAGE_SIZE);
    let limit = usize::try_from(limit).unwrap_or(0);
    let next_token = if entries.len() > limit {
        // The cursor names the LAST key this page carries, so a page of zero
        // items hands back the cursor this call resumed from: a caller that asks
        // for no objects is told where to carry on from, not sent back to the
        // start of the listing.
        let after = match limit.checked_sub(1) {
            Some(last) => entries
                .get(last)
                .map_or_else(|| resume_after.clone(), |(key, _)| key.clone()),
            None => resume_after.clone(),
        };
        Some(encode_page_token(kind, &after))
    } else {
        None
    };
    entries.truncate(limit);
    Ok(Page {
        items: entries.into_iter().map(|(_, item)| item).collect(),
        next_token,
    })
}

/// Why a name/ARN pair did not name a stored resource.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResolveError {
    /// Neither the name nor the ARN was supplied.
    NoIdentifier,
    /// Both were supplied and they name different resources.
    Mismatch,
    /// The ARN does not have the form of a resource ARN that names one of this
    /// service's resources.
    MalformedArn,
    /// The ARN is well formed but names another service's resource, or a
    /// resource type the operation does not address.
    WrongResourceType,
    /// No stored resource carries that name or ARN.
    NotFound,
}

/// Why a `RuleGroupName`/`Type`/`RuleGroupArn` triple did not name a stored rule
/// group.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RuleGroupLookupError {
    /// A name was given without a `Type` and without an ARN, so the request
    /// names at most half of a rule group's identity.
    TypeRequired,
    /// The triple was well formed but did not resolve.
    Resolve(ResolveError),
}

/// What a request that gives a rule group's name but neither its `Type` nor its
/// ARN is refused with: `DescribeRuleGroup$Type` (and the same member on every
/// other rule-group operation) is "required for requests that do not include the
/// RuleGroupARN", so a name alone names at most half of a rule group's identity.
/// The text names the member and the resource kind only, and carries nothing
/// from the request.
pub const TYPE_REQUIRED_MESSAGE: &str = "Type must be set when referring to RuleGroup by name";

/// What a request supplying an ARN that is not a well-formed resource ARN is
/// refused with, whichever member the reference arrived in.
///
/// The defect is a property of the reference, not of the request: the text names
/// neither the member, nor the resource kind, nor the offending value. The model
/// fixes the classification and the status only
/// (`smithy_query(query="shape",shape="InvalidRequestException")`: client error,
/// HTTP 400, member `Message`, "Your request references an ARN that is
/// malformed, or corresponds to a resource that isn't valid in the context of
/// the request").
pub const ARN_INVALID_FORMAT_MESSAGE: &str = "Resource ARN has an invalid format.";

/// What a request supplying a resource NAME that is not a well-formed resource
/// name is refused with, whichever member the reference arrived in — the
/// name-shaped half of [`ARN_INVALID_FORMAT_MESSAGE`]'s pair.
///
/// The defect is a property of the reference, not of the request: the text names
/// neither the member, nor the resource kind, nor the offending value, and it is
/// recorded with no closing period where the ARN half carries one. The model
/// fixes the classification and the status only
/// (`smithy_query(query="shape",shape="InvalidRequestException")`: client error,
/// HTTP 400, member `Message`), and `ResourceName`
/// (`smithy_query(query="shape",shape="ResourceName")`: `@length` 1..128,
/// `@pattern "^[a-zA-Z0-9-]+$"`) fixes the form a name has to take.
///
/// Stated at the boundary seam that answers a refused reference
/// (`crate::service::validation`'s rule 0) and nowhere else: every binding of a
/// resource name is held to that one shape before any handler runs, so a name
/// outside its form never reaches the resolution path the way a malformed ARN
/// does, and no lookup here has such a value to refuse.
pub const NAME_INVALID_FORMAT_MESSAGE: &str = "Resource name has an invalid format";

/// What a request supplying a well-formed ARN whose resource-type segment is not
/// the type the operation addresses is refused with — the second half of
/// [`ARN_INVALID_FORMAT_MESSAGE`]'s documented pair, "a resource that isn't
/// valid in the context of the request". It names the mismatch itself and
/// carries neither the type asked for nor the one the ARN holds.
pub const ARN_TYPE_MISMATCH_MESSAGE: &str = "Resource Type and Arn(Type in Arn) do not match";

/// What a request supplying BOTH a name and an ARN that identify different
/// resources is refused with — the same family as [`ARN_TYPE_MISMATCH_MESSAGE`],
/// one segment of the ARN disagreeing with what the request said it should be. It
/// names the mismatch itself and carries neither member's value.
pub const ARN_NAME_MISMATCH_MESSAGE: &str = "Name and Arn(Name in Arn) do not match";

/// What EVERY `ResourceNotFoundException` this service answers carries.
///
/// Network Firewall's exception messages are a property of the exception, not of
/// the request: the same text answers a missing firewall, firewall policy, rule
/// group or resource policy, and it names neither the resource, nor its type, nor
/// its ARN. The model fixes the classification and the status only
/// (`smithy_query(query="shape",shape="ResourceNotFoundException")`: client error,
/// HTTP 400, member `Message`), so the text is the recorded one, which is
/// byte-identical across the operations the corpus exercises.
pub const RESOURCE_NOT_FOUND_MESSAGE: &str =
    "No object with the provided name or resource identifier was found";

/// What a create whose name is already taken is refused with — resource-agnostic
/// in the same way [`RESOURCE_NOT_FOUND_MESSAGE`] is: it names neither the kind
/// of resource, nor its type, nor the name the request asked for.
pub const RESOURCE_ALREADY_EXISTS_MESSAGE: &str = "A resource with the specified name already exists";

/// What EVERY `InvalidTokenException` this service answers carries: the token
/// check is the same optimistic-locking rule on every resource, so the message
/// names neither the resource nor the token
/// (`smithy_query(query="shape",shape="InvalidTokenException")`: client error,
/// HTTP 400, "The token you provided is stale or isn't valid for the operation").
pub const INVALID_TOKEN_MESSAGE: &str = "Update token isn't valid.";

/// What a flow operation naming BOTH `AvailabilityZone` and
/// `AvailabilityZoneId` is refused with.
///
/// The rule is the documented one, stated on `AvailabilityZoneId` of every
/// operation that takes the pair: "Specify either AvailabilityZone or
/// AvailabilityZoneId, not both. If you specify both, the request fails." The two
/// members name the same scope two ways, so the message names the members and
/// carries nothing from the request.
pub const ZONE_AMBIGUOUS_MESSAGE: &str =
    "Specify either AvailabilityZone or AvailabilityZoneId, not both.";

/// What a delete refused because the object is protected against deletion
/// carries.
///
/// Resource-agnostic in the same way [`RESOURCE_NOT_FOUND_MESSAGE`] is: the
/// refusal is a property of the protection setting, not of the request, so it
/// names "the object" and neither the resource, nor its type, nor its name. The
/// model fixes the classification and the status only
/// (`smithy_query(query="shape",shape="InvalidOperationException")`: client
/// error, HTTP 400, member `Message`), so the text is the recorded one.
pub const DELETE_PROTECTED_MESSAGE: &str =
    "Unable to delete the object because delete protection is enabled";

/// What a delete refused because another resource still references the object
/// carries — the same resource-agnostic family as [`DELETE_PROTECTED_MESSAGE`],
/// naming neither the referencing resources nor how many there are.
pub const DELETE_IN_USE_MESSAGE: &str = "Unable to delete the object because it is still in use";

/// What EVERY `InvalidResourcePolicyException` this service answers carries.
///
/// Network Firewall shares a rule group or a firewall policy through Resource
/// Access Manager, so a submitted document is accepted only when it matches the
/// RAM managed permission for that resource type
/// ([`SharedResource::permitted_actions`]); one that does not — whether it is not
/// a policy document at all or grants an action the permission does not carry —
/// is refused with this one constant. It carries nothing from the request: not
/// the offending actions, not the ones the permission allows, not the resource.
/// The model fixes the classification and the status only
/// (`smithy_query(query="shape",shape="InvalidResourcePolicyException")`: client
/// error, HTTP 400, "The policy statement failed validation."), so the text is
/// the recorded one.
pub const INVALID_RESOURCE_POLICY_MESSAGE: &str =
    "The supplied policy does not match RAM managed permissions.";

/// What a request naming a rule-group TYPE the public API does not accept is
/// refused with.
///
/// `RuleGroupType` is one enumeration service-side and a subset of it in the
/// public model (`smithy_query(query="shape",shape="RuleGroupType")` declares
/// three values, and a refused value enumerates a wider set — see
/// [`crate::service`]'s validation wording): a value the service's own
/// enumeration carries but the public API does not accept breaks no declared
/// constraint, so it is refused as the invalid type it is, with the modelled
/// client error and this one constant. It carries nothing from the request —
/// neither the submitted type nor the values that are accepted — so the text is
/// the recorded one.
pub const INVALID_RULE_GROUP_TYPE_MESSAGE: &str =
    "The rule group type isn't valid. Enter a valid type, and try again.";

/// A refusal whose message IS built from the request: a fixed stem, then the
/// offending value in parameter brackets.
///
/// This is the one shape Network Firewall's request-derived error text takes, and
/// it is parseable by construction — a client matches the stem to tell which
/// check failed and reads the bracketed value to learn which input tripped it.
pub fn parameter_message(stem: &str, parameter: &str) -> String {
    format!("{stem}, parameter: [{parameter}]")
}

/// The same built shape with the request path the offending value arrived on
/// appended as context: `<stem>, parameter: [<value>], context: <path>`.
///
/// A check that reads one member of a nested input document reports WHERE it
/// looked as well as what it found, so a client can tell which part of a
/// submitted document tripped the check without parsing prose. Everything before
/// the brackets is fixed per check; only the bracketed value and the path vary.
pub fn parameter_context_message(stem: &str, parameter: &str, context: &str) -> String {
    context_message(&parameter_message(stem, parameter), context)
}

/// The same shape without a bracketed value: `<stem>, context: <path>`.
///
/// A check that failed on the ABSENCE of a value has no offending value to
/// bracket, so its message names only what failed and the input path it looked
/// at.
pub fn context_message(stem: &str, context: &str) -> String {
    format!("{stem}, context: {context}")
}

/// What a rule group whose rules need more capacity than is available to them is
/// refused with, on the create path and on the update path alike.
///
/// The refusal is one of the few this service builds FROM the request, and it
/// takes the built shape with a context ([`parameter_context_message`]): the stem
/// names the `RulesSource` member the rules are ACCOUNTED under
/// ([`accounted_rules_source_member`]) and the check that failed, the brackets
/// carry the capacity those rules REQUIRE (what a client needs in order to retry
/// with a sufficient `Capacity`, or to decide to split the rules when the
/// capacity is fixed), and the context is the input path that member sits on. The
/// capacity the rule group declares is not in the message: the caller sent it, and
/// `RuleGroupResponse$Capacity` reports it back.
pub fn capacity_exceeded_message(group: &RuleGroup, required: i32) -> String {
    let member = accounted_rules_source_member(group);
    capacity_exceeded(member, required, &format!("RulesSource.{member}"))
}

/// The input member a rule group's declared capacity arrives on, which is both
/// what its own refusal names and the context that refusal reports
/// (`CreateRuleGroupRequest$Capacity`).
pub const CAPACITY_MEMBER: &str = "Capacity";

/// What a request declaring more capacity than a rule group may hold
/// ([`MAX_RULE_GROUP_CAPACITY`]) is refused with: the SAME built capacity shape,
/// about the request's own `Capacity` member rather than about a rules source —
/// so the stem and the context name that member and the brackets carry the
/// capacity the request asked for, which is the value that has to come down.
pub fn declared_capacity_exceeded_message(capacity: i32) -> String {
    capacity_exceeded(CAPACITY_MEMBER, capacity, CAPACITY_MEMBER)
}

/// The one shape every capacity refusal this service answers takes: `<member>
/// capacity exceeded`, the capacity in parameter brackets, and the input path the
/// exceeded capacity belongs to as context.
fn capacity_exceeded(member: &str, capacity: i32, context: &str) -> String {
    parameter_context_message(
        &format!("{member} capacity exceeded"),
        &capacity.to_string(),
        context,
    )
}

/// The refusal for a reference naming an ARN no stored resource carries:
/// `referenced` — how this service names what the reference pointed at — then
/// `does not exist`, never the input member the reference arrived in, and the
/// offending ARN rides in the parameter brackets (see [`parameter_message`]).
///
/// What the stem's subject is, is a fact about the REFERENCED resource rather
/// than one spelling for every kind: a firewall policy reference is refused
/// naming the resource ([`REFERENCED_FIREWALL_POLICY`]) and a rule group
/// reference naming its ARN ([`REFERENCED_RULE_GROUP`]), each as this service
/// words that refusal.
pub fn missing_arn_message(referenced: &str, arn: &str) -> String {
    parameter_message(&format!("{referenced} does not exist"), arn)
}

/// How [`missing_arn_message`] names a rule group a reference did not resolve
/// to: the rule group's ARN.
pub const REFERENCED_RULE_GROUP: &str = "rule group ARN";

/// How [`missing_arn_message`] names a firewall policy a reference did not
/// resolve to: the firewall policy itself, the ARN it was named by riding in the
/// parameter brackets instead.
pub const REFERENCED_FIREWALL_POLICY: &str = "firewall policy";

/// The status of one firewall endpoint, as persisted.
#[derive(Debug, Clone)]
pub struct SyncStateRecord {
    /// The subnet the endpoint was created in.
    pub subnet_id: String,
    /// The endpoint id reported in route tables.
    pub endpoint_id: String,
    /// Whether [`SyncStateRecord::endpoint_id`] names an endpoint Amazon EC2 holds — one
    /// this service asked it to instantiate and it answered
    /// ([`firewall_endpoints::instantiate`]) — so the attachment's endpoint is deleted
    /// there when the attachment goes away.
    ///
    /// `false` where EC2 created no endpoint for the attachment at all — there was none in
    /// this process to ask, or it answered the ask with no endpoint: the id is then one this
    /// service minted and no route can resolve (see the [`firewall_endpoints`] module docs).
    pub instantiated_in_ec2: bool,
    /// An `AttachmentStatus` member.
    pub attachment_status: String,
}

/// One flow operation on a firewall, as persisted by a start command
/// (`StartFlowFlush`, `StartFlowCapture`) and read back by
/// `DescribeFlowOperation`.
///
/// A flow operation is scoped by the Availability Zone and/or the endpoint the
/// start command named, so those members belong to the record rather than to the
/// request that reads it: the describe command reports the scope the operation
/// was started with.
#[derive(Debug, Clone)]
pub struct FlowOperationRecord {
    /// `FlowOperationId`, and the key the record is stored under.
    pub flow_operation_id: String,
    /// A `FlowOperationType` member: which start command created this record.
    pub flow_operation_type: String,
    /// `AvailabilityZone` — the zone the operation is scoped to, if any.
    pub availability_zone: Option<String>,
    /// `AvailabilityZoneId` — the same scope named by zone id instead.
    pub availability_zone_id: Option<String>,
    /// `VpcEndpointAssociationArn` the operation is scoped to, if any.
    pub vpc_endpoint_association_arn: Option<String>,
    /// `VpcEndpointId` the operation is scoped to, if any.
    pub vpc_endpoint_id: Option<String>,
    /// `FlowOperation.MinimumFlowAgeInSeconds`.
    pub minimum_flow_age_in_seconds: Option<i32>,
    /// `FlowOperation.FlowFilters`, in the order the start command gave them.
    pub flow_filters: Vec<FlowFilter>,
    /// A `FlowOperationStatus` member.
    pub status: String,
    /// `FlowRequestTimestamp`: when the start command was accepted, and so the
    /// instant the clock measures this operation's run from (see
    /// [`FlowOperationRecord::completed_at`]).
    pub requested_at: SystemTime,
}

impl FlowOperationRecord {
    /// When the clock owes this operation its `IN_PROGRESS` -> `COMPLETED` move:
    /// [`FLOW_OPERATION_SPAN`] after the start command that began it.
    pub fn completed_at(&self) -> SystemTime {
        self.requested_at + FLOW_OPERATION_SPAN
    }

    /// Apply the status move the clock already made due, and say whether
    /// anything changed.
    ///
    /// A reporting operation calls this so a caller that waits out
    /// [`FLOW_OPERATION_SPAN`] sees `COMPLETED` — the transition was owed at
    /// [`FlowOperationRecord::completed_at`] whether or not anybody asked.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.status != FLOW_OPERATION_IN_PROGRESS || now < self.completed_at() {
            return false;
        }
        self.status = FLOW_OPERATION_COMPLETED.to_string();
        true
    }

    /// The `FlowOperation` shape for this record: the configuration the start
    /// command defined the operation with.
    pub fn to_flow_operation(&self) -> FlowOperation {
        FlowOperation {
            minimum_flow_age_in_seconds: self.minimum_flow_age_in_seconds,
            flow_filters: Some(self.flow_filters.clone()),
        }
    }

    /// The `FlowOperationMetadata` entry `ListFlowOperations` reports for this
    /// record.
    pub fn to_metadata(&self) -> FlowOperationMetadata {
        FlowOperationMetadata {
            flow_operation_id: Some(self.flow_operation_id.clone()),
            flow_operation_type: Some(self.flow_operation_type.clone()),
            flow_request_timestamp: Some(as_date_time(self.requested_at)),
            flow_operation_status: Some(self.status.clone()),
        }
    }

    /// The key this record is ordered and paginated by: the instant the start
    /// command was accepted, then the operation's own id.
    ///
    /// `ListFlowOperations` "returns a list of all flow operations ran in a
    /// specific firewall", so the listing order is the order they ran in. The id
    /// breaks a tie between two operations started in the same nanosecond, which
    /// makes the key unique and therefore usable as a page cursor.
    pub fn sort_key(&self) -> String {
        let nanos = self
            .requested_at
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|since_epoch| since_epoch.as_nanos())
            .unwrap_or_default();
        format!("{nanos:020}:{}", self.flow_operation_id)
    }
}

/// What a flow start command (`StartFlowFlush`, `StartFlowCapture`) defines its
/// operation with: the firewall whose flow table it acts on, the scope it is
/// narrowed to, and the filters that scope the flows it touches.
///
/// The two start commands take the SAME members, so they share one request
/// shape here and differ only in the `FlowOperationType` they pass to
/// [`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::start_flow_operation`].
#[derive(Debug, Clone)]
pub struct FlowOperationRequest {
    /// `FirewallArn` — required, and the firewall must be stored.
    pub firewall_arn: String,
    /// `AvailabilityZone`.
    pub availability_zone: Option<String>,
    /// `AvailabilityZoneId`.
    pub availability_zone_id: Option<String>,
    /// `VpcEndpointAssociationArn`.
    pub vpc_endpoint_association_arn: Option<String>,
    /// `VpcEndpointId`.
    pub vpc_endpoint_id: Option<String>,
    /// `MinimumFlowAgeInSeconds`.
    pub minimum_flow_age_in_seconds: Option<i32>,
    /// `FlowFilters`.
    pub flow_filters: Vec<FlowFilter>,
}

/// Why a flow start command was refused.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FlowStartBreach {
    /// The request named both `AvailabilityZone` and `AvailabilityZoneId`,
    /// which name the same scope two ways (see [`ZONE_AMBIGUOUS_MESSAGE`]).
    ZoneAmbiguous,
    /// More than [`MAX_FLOW_FILTERS`] filters were given; the count is carried
    /// so the refusal can name it.
    TooManyFilters(usize),
    /// `FirewallArn` names no stored firewall.
    FirewallNotFound,
    /// The firewall already holds [`MAX_STORED_FLOW_OPERATIONS`] of them.
    TooManyOperations,
}

/// How a `ListFlowOperations` request narrows the firewall's flow operations.
///
/// The listing "returns a list of all flow operations ran in a specific
/// firewall. You can optionally narrow the request scope by specifying the
/// operation type or Availability Zone associated with a firewall's flow
/// operations", so each member that is given must agree with what the start
/// command recorded.
///
/// Two rules make that precise, because the service holds no mapping between a
/// zone NAME, a zone ID and an endpoint — the three ways a scope can be spelled:
///
/// * an operation that did not state the member being narrowed on is not
///   narrowed out. One started without a scope was never narrowed to a zone or
///   an endpoint at all (it covers the whole firewall), and one started with a
///   DIFFERENT spelling of a scope cannot be shown to disagree with the request;
/// * an operation that stated the same member with a different value did not run
///   in the scope asked for, so it is left out.
#[derive(Debug, Clone, Default)]
pub struct FlowOperationFilter {
    /// Keep only operations of this `FlowOperationType`.
    pub flow_operation_type: Option<String>,
    /// Keep only operations scoped to this `AvailabilityZone`, or to none.
    pub availability_zone: Option<String>,
    /// Keep only operations scoped to this `AvailabilityZoneId`, or to none.
    pub availability_zone_id: Option<String>,
    /// Keep only operations scoped to this `VpcEndpointAssociationArn`, or to none.
    pub vpc_endpoint_association_arn: Option<String>,
    /// Keep only operations scoped to this `VpcEndpointId`, or to none.
    pub vpc_endpoint_id: Option<String>,
}

impl FlowOperationFilter {
    /// Whether `record` is in the narrowed listing.
    pub fn matches(&self, record: &FlowOperationRecord) -> bool {
        let scopes = [
            (&self.availability_zone, &record.availability_zone),
            (&self.availability_zone_id, &record.availability_zone_id),
            (
                &self.vpc_endpoint_association_arn,
                &record.vpc_endpoint_association_arn,
            ),
            (&self.vpc_endpoint_id, &record.vpc_endpoint_id),
        ];
        self.flow_operation_type
            .as_ref()
            .is_none_or(|wanted| *wanted == record.flow_operation_type)
            && scopes.iter().all(|(wanted, stored)| {
                match (wanted.as_ref(), stored.as_ref()) {
                    (Some(wanted), Some(stored)) => wanted == stored,
                    // An operation that did not state this member is not
                    // narrowed out by it: either it named no scope at all and
                    // covers the whole firewall, or it named the scope another
                    // way, which no mapping here can compare.
                    _ => true,
                }
            })
    }
}

/// Why a change to a firewall's subnet associations was refused.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SubnetAssociationBreach {
    /// The firewall is protected against changes to its subnet associations.
    Protected,
    /// The request named no subnet at all, so it asks for no association.
    NoSubnets,
    /// One subnet was named twice in the same request, which cannot mean two
    /// endpoints: a subnet is associated at most once.
    RepeatedSubnet(String),
    /// The change would need more Availability Zones than one firewall spans.
    ZoneCapacity,
    /// The change would remove every mapping the firewall has left, leaving it
    /// with no firewall endpoint at all; the subnets it names are carried so the
    /// refusal can report them.
    LastMapping(Vec<String>),
}

/// What a change one of a firewall's change-protection settings blocks is
/// refused with: `Cannot <Operation> because <Member> is on`.
///
/// Each setting is documented the same way on `Firewall`
/// (`Firewall$SubnetChangeProtection`: "A setting indicating whether the
/// firewall is protected against changes to the subnet associations. Use this
/// setting to protect against accidentally modifying the subnet associations for
/// a firewall that is in use"; `Firewall$FirewallPolicyChangeProtection` says
/// the same of the firewall policy association), so while the setting is on the
/// requested change is not a valid operation on this firewall
/// (`InvalidOperationException`, client, HTTP 400).
///
/// The model does not fix the text, so the wording is the recorded one, and it
/// is a FRAME rather than a constant: the refusal is the same whatever the
/// request asked to change, and the two things the error code alone cannot tell
/// a client are which call was refused and which setting refused it. Those are
/// its only variable parts — `operation` is the operation being served (so
/// `AssociateSubnets` and `DisassociateSubnets` each name themselves on their
/// own call) and `protection_member` is the `Firewall` member carrying the flag.
/// Nothing from the request body appears in it: no subnet id, ARN, account id or
/// firewall name (the same discipline as [`RESOURCE_NOT_FOUND_MESSAGE`]).
pub fn change_protected_message(operation: &str, protection_member: &str) -> String {
    format!("Cannot {operation} because {protection_member} is on")
}

/// How [`change_protected_message`] names the setting that protects a firewall's
/// subnet associations: the `Firewall` member carrying it.
pub const SUBNET_CHANGE_PROTECTION_MEMBER: &str = "SubnetChangeProtection";

/// How [`change_protected_message`] names the setting that protects a firewall's
/// policy association.
pub const FIREWALL_POLICY_CHANGE_PROTECTION_MEMBER: &str = "FirewallPolicyChangeProtection";

/// What a subnet-association change naming no subnet is refused with: the
/// request asks for no association at all, which no mapping can satisfy.
pub const NO_SUBNETS_MESSAGE: &str = "Specify at least one subnet.";

/// What a request naming a VPC the calling account cannot reach is refused with.
///
/// A firewall is created IN a VPC ("the unique identifier of the VPC where
/// Network Firewall should create the firewall", `CreateFirewall$VpcId`), and a
/// VPC that is not one of the account's own is a request that "references ... a
/// resource that isn't valid in the context of the request" —
/// `InvalidRequestException` (client, HTTP 400). The text is fixed for this error
/// class: it names neither the VPC nor its id, in the same resource-agnostic
/// family as [`RESOURCE_NOT_FOUND_MESSAGE`], so nothing from the request appears
/// in it.
pub const VPC_INACCESSIBLE_MESSAGE: &str = "Given VPC is inaccessible from account";

/// What a request naming subnets the calling account cannot reach is refused
/// with — the subnet side of [`VPC_INACCESSIBLE_MESSAGE`], and fixed text in the
/// same way: one sentence covering one subnet or several, carrying no id.
///
/// "Network Firewall creates a firewall endpoint in each subnet"
/// (`CreateFirewall$SubnetMappings`), so a subnet the account cannot reach is a
/// subnet no endpoint can be instantiated in.
pub const SUBNETS_INACCESSIBLE_MESSAGE: &str =
    "Given subnet(s) is/are inaccessible from account";

/// What a subnet disassociation that would empty a firewall's mapping set is
/// refused with, naming the subnets it would have removed.
///
/// `AssociateSubnets` is documented "This request creates an Network Firewall
/// firewall endpoint in each of the subnets", and `Firewall$SubnetMappings` is
/// "the public subnets that Network Firewall is using for the firewall. Each
/// subnet must belong to a different Availability Zone" — a firewall's endpoints
/// ARE its subnet mappings, so a firewall left with none would filter nothing,
/// and the removal is refused instead (`InvalidRequestException`, client,
/// HTTP 400).
///
/// The model does not fix the text, so the wording is the recorded one, and it
/// takes this service's one request-derived shape ([`parameter_message`]): a
/// fixed sentence naming the invariant, then the subnets the request asked to
/// remove in parameter brackets — which is what a caller has to drop from its
/// request in order to keep the firewall reachable. Several ids are listed in
/// the order the request gave them.
pub fn last_mapping_message(subnet_ids: &[String]) -> String {
    parameter_message(
        "availability zone subnet(s) cannot be removed as firewall must have at least one subnet \
         mapping",
        &subnet_ids.join(", "),
    )
}

/// What a subnet-association change past [`MAX_AVAILABILITY_ZONES`] is refused
/// with: `AssociateSubnets` is documented "You can specify one subnet for each
/// of the Availability Zones that the VPC spans", and
/// [`availability_zone_of`] names that many zones.
pub fn too_many_zones_message() -> String {
    format!(
        "A firewall can use at most {MAX_AVAILABILITY_ZONES} subnets, one per Availability Zone."
    )
}

/// The analysis types a firewall enables, as they are RETAINED: the types
/// `requested` names, each kept once and in the order it was first named, and
/// `None` where the request named no setting at all.
///
/// `EnabledAnalysisTypes` is "an optional setting indicating the specific traffic
/// analysis types to enable on the firewall"
/// (`smithy_query(query="doc",shape="Firewall",member="EnabledAnalysisTypes")`)
/// over an enumerated element type
/// (`smithy_query(query="shape",shape="EnabledAnalysisType")`), so the setting is
/// the SET of types that are on: naming one of them twice enables nothing twice,
/// and what is retained is that set rather than a transcript of the request —
/// which is also what keeps one request from making a single record arbitrarily
/// large, the discipline [`MAX_POLICY_LIST_ENTRIES`] and
/// [`MAX_LOG_DESTINATION_CONFIGS`] state for the other lists this state retains.
pub fn retained_analysis_types(requested: Option<Vec<String>>) -> Option<Vec<String>> {
    let requested = requested?;
    let mut named: HashSet<String> = HashSet::new();
    Some(
        requested
            .into_iter()
            .filter(|analysis_type| named.insert(analysis_type.clone()))
            .collect(),
    )
}

/// A firewall, as persisted. Every member `Firewall` / `FirewallStatus` reports
/// is read back from here.
#[derive(Debug, Clone)]
pub struct FirewallRecord {
    /// `Firewall.FirewallName`.
    pub firewall_name: String,
    /// `Firewall.FirewallArn`.
    pub firewall_arn: String,
    /// `Firewall.FirewallId`.
    pub firewall_id: String,
    /// `Firewall.FirewallPolicyArn`.
    pub firewall_policy_arn: String,
    /// `Firewall.VpcId`.
    pub vpc_id: String,
    /// The id of the Gateway Load Balancer endpoint service this firewall's endpoints are
    /// endpoints TO ([`firewall_endpoints::new_endpoint_service_id`]).
    ///
    /// One per firewall and minted with it, because "in each configured subnet, Network
    /// Firewall instantiates a firewall endpoint to handle network traffic"
    /// (`Attachment`) — every one of those endpoints reaches the same firewall, so every
    /// one of them is an endpoint to the same service. Not a modelled member: it is what
    /// the endpoints Amazon EC2 holds for this firewall are named by
    /// ([`firewall_endpoints::endpoint_service_name`]).
    pub endpoint_service_id: String,
    /// `Firewall.SubnetMappings`, in the order the create request gave them.
    pub subnet_mappings: Vec<SubnetMapping>,
    /// `Firewall.DeleteProtection`.
    pub delete_protection: bool,
    /// `Firewall.SubnetChangeProtection`.
    pub subnet_change_protection: bool,
    /// `Firewall.FirewallPolicyChangeProtection`.
    pub firewall_policy_change_protection: bool,
    /// `Firewall.Description`.
    pub description: Option<String>,
    /// `Firewall.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `Firewall.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `Firewall.EnabledAnalysisTypes`.
    pub enabled_analysis_types: Option<Vec<String>>,
    /// A `FirewallStatusValue` member.
    pub status: String,
    /// A `ConfigurationSyncState` member.
    pub config_sync_state: String,
    /// `FirewallStatus.SyncStates`, keyed by Availability Zone.
    pub sync_states: HashMap<String, SyncStateRecord>,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// When this firewall's endpoints last STARTED provisioning, and so the
    /// instant the clock measures that provisioning from (see
    /// [`FirewallRecord::ready_at`]): the `CreateFirewall` that stored the
    /// record, or a later call that attached a new firewall endpoint (see
    /// [`FirewallRecord::associate_subnets`]).
    pub provisioning_since: SystemTime,
    /// Every flow operation started on this firewall and not yet dropped with
    /// it, keyed by `FlowOperationId`. A flow operation acts on one firewall's
    /// flow table and is identified by `FirewallArn` + `FlowOperationId`, so it
    /// is held inside that firewall's record and deleting the firewall drops its
    /// flow operations with it.
    pub flow_operations: HashMap<String, FlowOperationRecord>,
    /// `LoggingConfiguration.LogDestinationConfigs`, in the order the last
    /// accepted `UpdateLoggingConfiguration` gave them.
    ///
    /// A firewall nobody configured logging on carries none, which is logging
    /// disabled: "if you omit this setting, Network Firewall disables logging for
    /// the firewall" (`UpdateLoggingConfigurationRequest$LoggingConfiguration`).
    pub log_destination_configs: Vec<LogDestinationConfig>,
    /// `EnableMonitoringDashboard`: "a boolean that reflects whether or not the
    /// firewall monitoring dashboard is enabled on a firewall", which is off
    /// until a call enables it.
    pub monitoring_dashboard_enabled: bool,
}

impl FirewallRecord {
    /// When the clock owes this firewall its `PROVISIONING` -> `READY` move:
    /// [`FIREWALL_PROVISIONING_SPAN`] after the call that started its endpoints
    /// provisioning.
    pub fn ready_at(&self) -> SystemTime {
        self.provisioning_since + FIREWALL_PROVISIONING_SPAN
    }

    /// Apply the status move the clock already made due, and say whether
    /// anything changed.
    ///
    /// A reporting operation calls this so a caller that waits out
    /// [`FIREWALL_PROVISIONING_SPAN`] sees `READY` — the transition was owed at
    /// [`FirewallRecord::ready_at`] whether or not anybody asked.
    pub fn settle(&mut self, now: SystemTime) -> bool {
        if self.status != FIREWALL_PROVISIONING || now < self.ready_at() {
            return false;
        }
        self.status = FIREWALL_READY.to_string();
        for sync_state in self.sync_states.values_mut() {
            sync_state.attachment_status = ATTACHMENT_READY.to_string();
        }
        true
    }

    /// Associate `requested` with this firewall, and answer the subnets that
    /// gained a firewall endpoint here.
    ///
    /// `AssociateSubnets` is documented "Associates the specified subnets in the
    /// Amazon VPC to the firewall. You can specify one subnet for each of the
    /// Availability Zones that the VPC spans. This request creates an Network
    /// Firewall firewall endpoint in each of the subnets", so a firewall's
    /// mappings are a SET of subnets with one endpoint per Availability Zone: a
    /// subnet the firewall already carries is already associated and gains no
    /// second endpoint, while every newly named subnet takes a free zone
    /// ([`availability_zone_of`]) and an endpoint that starts out
    /// [`ATTACHMENT_CREATING`] — which is what puts the firewall back in
    /// [`FIREWALL_PROVISIONING`] until the clock owes it `READY` again at
    /// [`FirewallRecord::ready_at`].
    ///
    /// `instantiated` carries the firewall endpoints Amazon EC2 created for this call,
    /// keyed by the subnet each one serves ([`firewall_endpoints::instantiate_all`]); each
    /// newly attached subnet TAKES its own out of that map, so what is left over on return
    /// is the endpoints this association did not use and the caller deletes again.
    ///
    /// Nothing is written on a refusal: every check runs before the first
    /// mutation.
    pub fn associate_subnets(
        &mut self,
        region: &str,
        requested: &[SubnetMapping],
        instantiated: &mut HashMap<String, String>,
        now: SystemTime,
    ) -> Result<Vec<String>, SubnetAssociationBreach> {
        if requested.is_empty() {
            return Err(SubnetAssociationBreach::NoSubnets);
        }
        if self.subnet_change_protection {
            return Err(SubnetAssociationBreach::Protected);
        }
        let mut named = HashSet::new();
        for mapping in requested {
            if !named.insert(mapping.subnet_id.as_str()) {
                return Err(SubnetAssociationBreach::RepeatedSubnet(
                    mapping.subnet_id.clone(),
                ));
            }
        }

        // A subnet this firewall already carries is already associated, so it is
        // not attached a second time and keeps the `IPAddressType` it was
        // associated with (`SubnetMapping$IPAddressType`: "You can't change the
        // IP address type after you create the subnet").
        let added: Vec<SubnetMapping> = {
            let associated: HashSet<&str> = self
                .subnet_mappings
                .iter()
                .map(|mapping| mapping.subnet_id.as_str())
                .collect();
            requested
                .iter()
                .filter(|mapping| !associated.contains(mapping.subnet_id.as_str()))
                .cloned()
                .collect()
        };
        if self.subnet_mappings.len() + added.len() > MAX_AVAILABILITY_ZONES {
            return Err(SubnetAssociationBreach::ZoneCapacity);
        }
        // One endpoint per Availability Zone, so each added subnet takes a zone
        // no endpoint of this firewall occupies yet.
        let zones: Vec<String> = (0..MAX_AVAILABILITY_ZONES)
            .map(|index| availability_zone_of(region, index))
            .filter(|zone| !self.sync_states.contains_key(zone))
            .take(added.len())
            .collect();
        if zones.len() < added.len() {
            return Err(SubnetAssociationBreach::ZoneCapacity);
        }

        let attached: Vec<String> = added
            .iter()
            .map(|mapping| mapping.subnet_id.clone())
            .collect();
        for (mapping, zone) in added.into_iter().zip(zones) {
            self.sync_states.insert(
                zone,
                new_sync_state(&mapping.subnet_id, instantiated.remove(&mapping.subnet_id)),
            );
            self.subnet_mappings.push(mapping);
        }
        if !attached.is_empty() {
            self.status = FIREWALL_PROVISIONING.to_string();
            self.provisioning_since = now;
        }
        Ok(attached)
    }

    /// Disassociate the subnets `requested` names from this firewall, and answer
    /// the subnets that lost a firewall endpoint here.
    ///
    /// `DisassociateSubnets` is documented "Removes the specified subnet
    /// associations from the firewall. This removes the firewall endpoints from
    /// the subnets and removes any network filtering protections that the
    /// endpoints were providing", so it is the inverse of
    /// [`FirewallRecord::associate_subnets`] over the same SET of subnets: every
    /// named subnet the firewall carries loses its mapping and the endpoint in
    /// its Availability Zone — which frees that zone for a later association —
    /// while a subnet the firewall does not carry is already not associated and
    /// loses nothing.
    ///
    /// No endpoint is created here, so this starts no provisioning and leaves
    /// the firewall's own status to [`FirewallRecord::settle`]: the endpoints
    /// this call removes are gone from every projection of the record, so no
    /// reader is shown an endpoint for a subnet the firewall no longer carries.
    ///
    /// What it cannot do is leave the firewall with NO mapping: a firewall
    /// filters through the endpoints its mappings are, so a request that would
    /// remove every one of them is refused
    /// ([`SubnetAssociationBreach::LastMapping`]).
    ///
    /// Nothing is written on a refusal: every check runs before the first
    /// mutation.
    pub fn disassociate_subnets(
        &mut self,
        requested: &[String],
    ) -> Result<Vec<String>, SubnetAssociationBreach> {
        if requested.is_empty() {
            return Err(SubnetAssociationBreach::NoSubnets);
        }
        if self.subnet_change_protection {
            return Err(SubnetAssociationBreach::Protected);
        }
        let mut named = HashSet::new();
        for subnet_id in requested {
            if !named.insert(subnet_id.as_str()) {
                return Err(SubnetAssociationBreach::RepeatedSubnet(subnet_id.clone()));
            }
        }

        // The subnets this call takes out of the mapping set, in the order the
        // REQUEST named them: a refusal reports them back as the caller sent
        // them, and the success path only asks whether there were any.
        let carried: HashSet<&str> = self
            .subnet_mappings
            .iter()
            .map(|mapping| mapping.subnet_id.as_str())
            .collect();
        let detached: Vec<String> = requested
            .iter()
            .filter(|subnet_id| carried.contains(subnet_id.as_str()))
            .cloned()
            .collect();
        // A firewall's endpoints are its subnet mappings, so a request that would
        // leave it with none is refused and nothing is removed (see
        // [`last_mapping_message`]). A request naming subnets this firewall does
        // not carry removes none of its mappings, so it cannot empty the set.
        if !self.subnet_mappings.is_empty() && self.subnet_mappings.len() == detached.len() {
            return Err(SubnetAssociationBreach::LastMapping(detached));
        }
        self.subnet_mappings
            .retain(|mapping| !named.contains(mapping.subnet_id.as_str()));
        self.sync_states
            .retain(|_, endpoint| !named.contains(endpoint.subnet_id.as_str()));
        Ok(detached)
    }

    /// The subnets `requested` names that this firewall does not carry yet, and so the ones
    /// an association would instantiate a firewall endpoint for.
    ///
    /// A READ of the record taken before the hop that instantiates those endpoints, so the
    /// hop asks for the endpoints the association needs and no others; the association
    /// itself re-reads the mapping set under the store guard
    /// ([`FirewallRecord::associate_subnets`]), which is what decides what is attached.
    pub fn unassociated_subnets(&self, requested: &[SubnetMapping]) -> Vec<String> {
        let associated: HashSet<&str> = self
            .subnet_mappings
            .iter()
            .map(|mapping| mapping.subnet_id.as_str())
            .collect();
        let mut named = HashSet::new();
        requested
            .iter()
            .map(|mapping| mapping.subnet_id.clone())
            .filter(|subnet_id| {
                !associated.contains(subnet_id.as_str()) && named.insert(subnet_id.clone())
            })
            .collect()
    }

    /// The firewall endpoints Amazon EC2 holds for this firewall — the ones to delete there
    /// when the firewall itself goes away (see [`SyncStateRecord::instantiated_in_ec2`]).
    pub fn instantiated_endpoints(&self) -> Vec<String> {
        self.sync_states
            .values()
            .filter(|sync_state| sync_state.instantiated_in_ec2)
            .map(|sync_state| sync_state.endpoint_id.clone())
            .collect()
    }

    /// The same endpoints, for the attachments of the subnets `subnet_ids` names alone — the
    /// ones a disassociation of those subnets removes.
    pub fn instantiated_endpoints_in(&self, subnet_ids: &[String]) -> Vec<String> {
        let named: HashSet<&str> = subnet_ids.iter().map(String::as_str).collect();
        self.sync_states
            .values()
            .filter(|sync_state| {
                sync_state.instantiated_in_ec2 && named.contains(sync_state.subnet_id.as_str())
            })
            .map(|sync_state| sync_state.endpoint_id.clone())
            .collect()
    }

    /// The `Firewall` shape for this record.
    pub fn to_firewall(&self) -> Firewall {
        Firewall {
            firewall_name: Some(self.firewall_name.clone()),
            firewall_arn: Some(self.firewall_arn.clone()),
            firewall_policy_arn: self.firewall_policy_arn.clone(),
            vpc_id: self.vpc_id.clone(),
            subnet_mappings: self.subnet_mappings.clone(),
            delete_protection: self.delete_protection,
            subnet_change_protection: self.subnet_change_protection,
            firewall_policy_change_protection: self.firewall_policy_change_protection,
            description: self.description.clone(),
            firewall_id: self.firewall_id.clone(),
            tags: self.tags.clone(),
            encryption_configuration: self.encryption_configuration.clone(),
            enabled_analysis_types: self.enabled_analysis_types.clone(),
            // `NumberOfAssociations` counts the `VpcEndpointAssociation`
            // resources that use this firewall. That resource is not implemented
            // (its operations still answer 501), so there is no count to report
            // and the member is left absent rather than filled with a guess.
            ..Firewall::default()
        }
    }

    /// The `FirewallStatus` shape for this record.
    pub fn to_firewall_status(&self) -> FirewallStatus {
        let sync_states = self
            .sync_states
            .iter()
            .map(|(zone, endpoint)| {
                let config = HashMap::from([(
                    self.firewall_policy_arn.clone(),
                    PerObjectStatus {
                        sync_status: Some(self.config_sync_state.clone()),
                        update_token: Some(self.update_token.clone()),
                    },
                )]);
                let sync_state = SyncState {
                    attachment: Some(Attachment {
                        subnet_id: Some(endpoint.subnet_id.clone()),
                        endpoint_id: Some(endpoint.endpoint_id.clone()),
                        status: Some(endpoint.attachment_status.clone()),
                        ..Attachment::default()
                    }),
                    config: Some(config),
                    ..SyncState::default()
                };
                (zone.clone(), sync_state)
            })
            .collect();
        FirewallStatus {
            status: self.status.clone(),
            configuration_sync_state_summary: self.config_sync_state.clone(),
            sync_states: Some(sync_states),
            ..FirewallStatus::default()
        }
    }

    /// The `FirewallMetadata` entry `ListFirewalls` reports for this record.
    pub fn to_metadata(&self) -> FirewallMetadata {
        FirewallMetadata {
            firewall_name: Some(self.firewall_name.clone()),
            firewall_arn: Some(self.firewall_arn.clone()),
            ..FirewallMetadata::default()
        }
    }

    /// The `LoggingConfiguration` this firewall's logging is, as both
    /// `UpdateLoggingConfiguration` and `DescribeLoggingConfiguration` report it.
    ///
    /// `LoggingConfiguration$LogDestinationConfigs` is a required member, so a
    /// firewall nobody configured logging on reports the configuration with an
    /// EMPTY list of destinations rather than no configuration: that is what
    /// "Network Firewall disables logging for the firewall" looks like to a
    /// reader, and it is the same document a caller edits and sends back.
    pub fn to_logging_configuration(&self) -> LoggingConfiguration {
        LoggingConfiguration {
            log_destination_configs: self.log_destination_configs.clone(),
        }
    }
}

/// A firewall policy, as persisted.
#[derive(Debug, Clone)]
pub struct FirewallPolicyRecord {
    /// `FirewallPolicyResponse.FirewallPolicyName`.
    pub policy_name: String,
    /// `FirewallPolicyResponse.FirewallPolicyArn`.
    pub policy_arn: String,
    /// `FirewallPolicyResponse.FirewallPolicyId`.
    pub policy_id: String,
    /// The policy document itself, as `DescribeFirewallPolicy` returns it.
    pub policy: FirewallPolicy,
    /// `FirewallPolicyResponse.Description`.
    pub description: Option<String>,
    /// `FirewallPolicyResponse.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `FirewallPolicyResponse.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// A `ResourceStatus` member.
    pub status: String,
    /// How many firewalls are associated with this policy.
    pub number_of_associations: i32,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// `FirewallPolicyResponse.LastModifiedTime`.
    pub last_modified_time: SystemTime,
}

impl FirewallPolicyRecord {
    /// The `FirewallPolicyResponse` shape for this record.
    ///
    /// `consumed` is the capacity the rule groups this policy references reserve;
    /// it is read from the rule-group store, so it is passed in rather than held
    /// on the record (see
    /// [`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::consumed_policy_capacity`]).
    pub fn to_response(&self, consumed: ConsumedPolicyCapacity) -> FirewallPolicyResponse {
        FirewallPolicyResponse {
            firewall_policy_name: self.policy_name.clone(),
            firewall_policy_arn: self.policy_arn.clone(),
            firewall_policy_id: self.policy_id.clone(),
            description: self.description.clone(),
            firewall_policy_status: Some(self.status.clone()),
            tags: self.tags.clone(),
            number_of_associations: Some(self.number_of_associations),
            encryption_configuration: self.encryption_configuration.clone(),
            last_modified_time: Some(as_date_time(self.last_modified_time)),
            consumed_stateless_rule_capacity: Some(consumed.stateless),
            consumed_stateful_rule_capacity: Some(consumed.stateful),
            consumed_stateful_domain_capacity: Some(consumed.stateful_domain),
            ..FirewallPolicyResponse::default()
        }
    }

    /// The `FirewallPolicyMetadata` entry `ListFirewallPolicies` reports.
    pub fn to_metadata(&self) -> FirewallPolicyMetadata {
        FirewallPolicyMetadata {
            name: Some(self.policy_name.clone()),
            arn: Some(self.policy_arn.clone()),
        }
    }
}

/// The capacity the rule groups a firewall policy references reserve, as the
/// `FirewallPolicyResponse` `Consumed*` members report it.
///
/// `CreateRuleGroupRequest$Capacity` documents the reservation: "When you
/// reference a rule group from a firewall policy, Network Firewall reserves this
/// capacity for the rule group."
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct ConsumedPolicyCapacity {
    /// `ConsumedStatelessRuleCapacity`.
    pub stateless: i32,
    /// `ConsumedStatefulRuleCapacity`.
    pub stateful: i32,
    /// `ConsumedStatefulDomainCapacity` — "the total number of domain name
    /// specifications across all domain list rule groups in the firewall policy
    /// that use the `stateful-domain-rulegroup` resource type", which is the
    /// consumed (not the declared) capacity of those rule groups.
    pub stateful_domain: i32,
}

/// A rule group, as persisted. Every member `RuleGroupResponse` reports is read
/// back from here.
#[derive(Debug, Clone)]
pub struct RuleGroupRecord {
    /// `RuleGroupResponse.RuleGroupName`.
    pub rule_group_name: String,
    /// `RuleGroupResponse.RuleGroupArn`.
    pub rule_group_arn: String,
    /// `RuleGroupResponse.RuleGroupId`.
    pub rule_group_id: String,
    /// `RuleGroupResponse.Type`, a `RuleGroupType` member.
    pub rule_group_type: String,
    /// `RuleGroupResponse.Capacity`: fixed at creation.
    pub capacity: i32,
    /// The rule group document itself, as `DescribeRuleGroup` returns it.
    pub rule_group: RuleGroup,
    /// `RuleGroupResponse.Description`.
    pub description: Option<String>,
    /// `RuleGroupResponse.Tags`.
    pub tags: Option<Vec<Tag>>,
    /// `RuleGroupResponse.EncryptionConfiguration`.
    pub encryption_configuration: Option<EncryptionConfiguration>,
    /// `RuleGroupResponse.SourceMetadata`.
    pub source_metadata: Option<SourceMetadata>,
    /// `RuleGroupResponse.SummaryConfiguration`.
    pub summary_configuration: Option<SummaryConfiguration>,
    /// A `ResourceStatus` member.
    pub status: String,
    /// The optimistic-locking token; re-minted on every mutation.
    pub update_token: String,
    /// `RuleGroupResponse.LastModifiedTime`.
    pub last_modified_time: SystemTime,
}

impl RuleGroupRecord {
    /// The `RuleGroupResponse` shape for this record.
    ///
    /// `associations` is how many firewall policies reference this rule group;
    /// it is read from the firewall-policy store, so it is passed in rather than
    /// held on the record (see
    /// [`ComAmazonawsNetworkfirewallNetworkfirewall20201112State::rule_group_associations`]) —
    /// the same way a policy's consumed capacity is read from the rule-group
    /// store rather than kept beside it.
    ///
    /// `AnalysisResults` is left to the caller: the analyzer runs only when the
    /// request sets `AnalyzeRuleGroup`, and its findings are computed from the
    /// rules rather than stored.
    pub fn to_response(&self, associations: i32) -> RuleGroupResponse {
        RuleGroupResponse {
            rule_group_name: self.rule_group_name.clone(),
            rule_group_arn: self.rule_group_arn.clone(),
            rule_group_id: self.rule_group_id.clone(),
            r#type: Some(self.rule_group_type.clone()),
            capacity: Some(self.capacity),
            consumed_capacity: Some(required_capacity(&self.rule_group_type, &self.rule_group)),
            description: self.description.clone(),
            rule_group_status: Some(self.status.clone()),
            tags: self.tags.clone(),
            number_of_associations: Some(associations),
            encryption_configuration: self.encryption_configuration.clone(),
            source_metadata: self.source_metadata.clone(),
            summary_configuration: self.summary_configuration.clone(),
            last_modified_time: Some(as_date_time(self.last_modified_time)),
            // `SnsTopic` records changes to a MANAGED rule group, which only
            // Amazon Web Services and its partners publish; a rule group created
            // through this API has no such topic, so the member stays absent.
            ..RuleGroupResponse::default()
        }
    }

    /// The `RuleGroupMetadata` entry `ListRuleGroups` reports for this record.
    ///
    /// `VendorName` is "the name of the Amazon Web Services Marketplace seller
    /// that provides this rule group", so it stays absent for a rule group
    /// created through this API: that rule group has no seller.
    pub fn to_metadata(&self) -> RuleGroupMetadata {
        RuleGroupMetadata {
            name: Some(self.rule_group_name.clone()),
            arn: Some(self.rule_group_arn.clone()),
            ..RuleGroupMetadata::default()
        }
    }
}

/// How a `ListRuleGroups` request narrows the stored rule groups.
///
/// The listing "retrieves the metadata for the rule groups that you have
/// defined", and `Scope` says which collection that is: "the default setting of
/// `ACCOUNT` or a setting of `NULL` returns all of the rule groups in your
/// account. A setting of `MANAGED` returns all available managed rule groups."
///
/// `Type` narrows the account's own rule groups by the `RuleGroupType` each was
/// created with. The other two members describe a MANAGED rule group only —
/// `ManagedType` is "the general category of the Amazon Web Services managed
/// rule group" and `SubscriptionStatus` filters "the results to show only rule
/// groups with the specified subscription status" — so a request that sets
/// either, like one asking for the `MANAGED` scope, selects published rule
/// groups rather than the account's own. This service holds no published
/// catalog, since nothing creates a managed rule group (`CreateRuleGroup` makes
/// an account one), so such a request is answered with an empty listing rather
/// than an invented catalog.
///
/// INFERRED, not stated by the model: how an explicit `Scope: ACCOUNT` combines
/// with a managed-only member. `ManagedType` and `SubscriptionStatus` each name
/// a property only a published rule group has, so no rule group this account
/// created can satisfy one and the listing stays empty whatever the scope says.
/// Revisit this case if a managed catalog is ever modelled here.
#[derive(Debug, Clone, Default)]
pub struct RuleGroupFilter {
    /// `ListRuleGroups$Scope`, a `ResourceManagedStatus` member.
    pub scope: Option<String>,
    /// `ListRuleGroups$ManagedType`, a `ResourceManagedType` member.
    pub managed_type: Option<String>,
    /// `ListRuleGroups$SubscriptionStatus`, a `SubscriptionStatus` member.
    pub subscription_status: Option<String>,
    /// `ListRuleGroups$Type`, a `RuleGroupType` member.
    pub rule_group_type: Option<String>,
}

impl RuleGroupFilter {
    /// Whether the request asks only for rule groups Amazon Web Services or one
    /// of its partners publishes, of which this service holds none.
    pub fn selects_managed_only(&self) -> bool {
        self.scope.as_deref() == Some(SCOPE_MANAGED)
            || self.managed_type.is_some()
            || self.subscription_status.is_some()
    }

    /// Whether `record` — a rule group this account created — is in the narrowed
    /// listing.
    pub fn matches(&self, record: &RuleGroupRecord) -> bool {
        !self.selects_managed_only()
            && self
                .rule_group_type
                .as_ref()
                .is_none_or(|wanted| *wanted == record.rule_group_type)
    }
}

/// Per-service state for com-amazonaws-networkfirewall-networkfirewall-20201112. A handler reaches it through [`service_state`].
#[derive(Debug, Default)]
pub struct ComAmazonawsNetworkfirewallNetworkfirewall20201112State {
    /// Every created-and-not-deleted firewall, keyed by `FirewallName`.
    pub firewalls: HashMap<String, FirewallRecord>,
    /// `FirewallArn` -> `FirewallName`, so an operation resolves by either.
    pub firewall_arn_index: HashMap<String, String>,
    /// Every created-and-not-deleted policy, keyed by `FirewallPolicyName`.
    pub firewall_policies: HashMap<String, FirewallPolicyRecord>,
    /// `FirewallPolicyArn` -> `FirewallPolicyName`.
    pub firewall_policy_arn_index: HashMap<String, String>,
    /// Every created-and-not-deleted Next Generation policy, keyed by
    /// `FirewallPolicyV2Name`. Its own store, because its document is its own
    /// shape; the NAME space it draws from is the one the first-generation
    /// policies draw from (see
    /// [`firewall_policies_v2`](crate::state::firewall_policies_v2)).
    pub firewall_policies_v2: HashMap<String, firewall_policies_v2::FirewallPolicyV2Record>,
    /// `FirewallPolicyV2Arn` -> `FirewallPolicyV2Name`.
    pub firewall_policy_v2_arn_index: HashMap<String, String>,
    /// The resource policy attached to a shared rule group or firewall policy,
    /// keyed by that resource's ARN. An absent entry is what makes
    /// `DescribeResourcePolicy` answer `ResourceNotFoundException`.
    ///
    /// One entry can exist only for a resource this state still holds:
    /// `PutResourcePolicy` refuses an ARN that names no stored resource, and
    /// [`Self::detach_resource_policy`] drops the entry when that resource is
    /// deleted. The map therefore never holds more entries than
    /// [`MAX_STORED_FIREWALL_POLICIES`] + [`MAX_STORED_RULE_GROUPS`], which
    /// [`MAX_STORED_RESOURCE_POLICIES`] states as the ceiling the attach path
    /// checks.
    pub resource_policies: HashMap<String, String>,
    /// Every created-and-not-deleted rule group, keyed by the ARN resource path
    /// [`rule_group_resource_path`] builds, so the stateless and the stateful
    /// half of the service each have their own name space.
    pub rule_groups: HashMap<String, RuleGroupRecord>,
    /// `RuleGroupArn` -> the [`rule_groups`](Self::rule_groups) key.
    pub rule_group_arn_index: HashMap<String, String>,
}

impl ComAmazonawsNetworkfirewallNetworkfirewall20201112State {
    /// The `FirewallName` the given name and/or ARN identifies.
    pub fn resolve_firewall(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, ResolveError> {
        resolve(
            &self.firewall_arn_index,
            name,
            name,
            arn,
            &[ARN_TYPE_FIREWALL],
            |key| self.firewalls.contains_key(key),
        )
    }

    /// The `FirewallName` the given name, ARN and `FirewallType` identify.
    ///
    /// The TYPE is part of the identification: "if you specify the firewall ARN,
    /// the type must match the resource type in the ARN. If you specify only the
    /// firewall name, Network Firewall uses this value to identify the firewall.
    /// If you don't specify a type, the default is `FIREWALL`"
    /// (`DescribeFirewallRequest$FirewallType`, and the same text on every
    /// operation that takes the member). The other value names an internet
    /// gateway firewall, which this service holds none of
    /// ([`names_the_stored_firewall_type`]), so a request naming it identifies no
    /// firewall — never the standard firewall that happens to carry the same
    /// name, exactly as a rule-group request naming an engine type identifies no
    /// rule group of another kind.
    ///
    /// Every operation that takes the member resolves through here, so one
    /// identification rule serves the reads, the updates, the delete and the
    /// associations alike.
    pub fn resolve_firewall_request(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
        firewall_type: Option<&str>,
    ) -> Result<String, ResolveError> {
        if !names_the_stored_firewall_type(firewall_type) {
            return Err(ResolveError::NotFound);
        }
        self.resolve_firewall(name, arn)
    }

    /// Accept a flow operation of `operation_type` on the firewall `request`
    /// names, and answer the record it stored.
    ///
    /// The checks are the documented ones, in the order a client can observe
    /// them: the two Availability Zone members name one scope two ways, so
    /// naming both is a bad request ("Specify either AvailabilityZone or
    /// AvailabilityZoneId, not both. If you specify both, the request fails");
    /// `FlowFilters` is capped at [`MAX_FLOW_FILTERS`] ("You can use up to 20
    /// filters to configure a single flow operation"); and `FirewallArn` is a
    /// reference to a firewall this service holds, so an ARN no stored firewall
    /// carries locates no resource.
    ///
    /// The operation starts asynchronously, so the record lands
    /// [`FLOW_OPERATION_IN_PROGRESS`] and the clock owes it its move to
    /// [`FLOW_OPERATION_COMPLETED`] at [`FlowOperationRecord::completed_at`].
    pub fn start_flow_operation(
        &mut self,
        operation_type: &str,
        request: FlowOperationRequest,
        now: SystemTime,
    ) -> Result<(String, FlowOperationRecord), FlowStartBreach> {
        if request.availability_zone.is_some() && request.availability_zone_id.is_some() {
            return Err(FlowStartBreach::ZoneAmbiguous);
        }
        if request.flow_filters.len() > MAX_FLOW_FILTERS {
            return Err(FlowStartBreach::TooManyFilters(request.flow_filters.len()));
        }
        let key = self
            .resolve_firewall(None, Some(&request.firewall_arn))
            .map_err(|_| FlowStartBreach::FirewallNotFound)?;
        let firewall = self
            .firewalls
            .get_mut(&key)
            .ok_or(FlowStartBreach::FirewallNotFound)?;
        if firewall.flow_operations.len() >= MAX_STORED_FLOW_OPERATIONS {
            return Err(FlowStartBreach::TooManyOperations);
        }

        let record = FlowOperationRecord {
            flow_operation_id: new_uuid(),
            flow_operation_type: operation_type.to_string(),
            availability_zone: request.availability_zone,
            availability_zone_id: request.availability_zone_id,
            vpc_endpoint_association_arn: request.vpc_endpoint_association_arn,
            vpc_endpoint_id: request.vpc_endpoint_id,
            minimum_flow_age_in_seconds: request.minimum_flow_age_in_seconds,
            flow_filters: request.flow_filters,
            status: FLOW_OPERATION_IN_PROGRESS.to_string(),
            requested_at: now,
        };
        firewall
            .flow_operations
            .insert(record.flow_operation_id.clone(), record.clone());
        Ok((firewall.firewall_arn.clone(), record))
    }

    /// The flow operation `flow_operation_id` names on the firewall
    /// `firewall_arn` names, with the status move the clock already made due
    /// applied, or `None` when either names nothing stored.
    ///
    /// A flow operation is identified by the PAIR, so an id that exists on
    /// another firewall is not found here: `DescribeFlowOperation` takes
    /// `FirewallArn` as a required member beside the id.
    pub fn settled_flow_operation(
        &mut self,
        firewall_arn: &str,
        flow_operation_id: &str,
        now: SystemTime,
    ) -> Option<(String, FlowOperationRecord)> {
        let key = self.resolve_firewall(None, Some(firewall_arn)).ok()?;
        let firewall = self.firewalls.get_mut(&key)?;
        let stored_arn = firewall.firewall_arn.clone();
        let operation = firewall.flow_operations.get_mut(flow_operation_id)?;
        operation.settle(now);
        Some((stored_arn, operation.clone()))
    }

    /// The flow operations of the firewall `firewall_arn` names that `filter`
    /// keeps, ordered by [`FlowOperationRecord::sort_key`] and each with the
    /// status move the clock already made due applied, or `None` when the ARN
    /// names no stored firewall.
    ///
    /// The pairs are `(cursor key, metadata)`, which is what [`paginate`] cuts
    /// into a page.
    pub fn settled_flow_operation_page(
        &mut self,
        firewall_arn: &str,
        filter: &FlowOperationFilter,
        now: SystemTime,
    ) -> Option<Vec<(String, FlowOperationMetadata)>> {
        let key = self.resolve_firewall(None, Some(firewall_arn)).ok()?;
        let firewall = self.firewalls.get_mut(&key)?;
        for operation in firewall.flow_operations.values_mut() {
            operation.settle(now);
        }
        let mut entries: Vec<(String, FlowOperationMetadata)> = firewall
            .flow_operations
            .values()
            .filter(|operation| filter.matches(operation))
            .map(|operation| (operation.sort_key(), operation.to_metadata()))
            .collect();
        entries.sort_by(|left, right| left.0.cmp(&right.0));
        Some(entries)
    }

    /// The `FirewallPolicyName` the given name and/or ARN identifies.
    pub fn resolve_firewall_policy(
        &self,
        name: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, ResolveError> {
        resolve(
            &self.firewall_policy_arn_index,
            name,
            name,
            arn,
            &[ARN_TYPE_FIREWALL_POLICY],
            |key| self.firewall_policies.contains_key(key),
        )
    }

    /// The [`rule_groups`](Self::rule_groups) key a request's `RuleGroupName`,
    /// `Type` and `RuleGroupArn` identify.
    ///
    /// A rule group's name is unique only within its type, so the store is keyed
    /// by the resource path [`rule_group_resource_path`] builds from both and
    /// `Type` "is required for requests that do not include the RuleGroupARN"
    /// (`DescribeRuleGroup$Type`). With an ARN in hand the name adds nothing the
    /// ARN does not already carry, so a typeless name is resolved through the ARN
    /// — and still checked against it, like every other name beside an ARN.
    ///
    /// `Type` narrows which ARN resource type the reference may carry, because
    /// the type is part of a rule group's ARN: a request that asks for one half
    /// of the name space with the other half's ARN names the wrong kind of
    /// resource.
    ///
    /// Every rule-group operation that takes this triple resolves it here, so one
    /// identification rule serves the read, the update and the delete alike.
    pub fn resolve_rule_group_request(
        &self,
        name: Option<&str>,
        group_type: Option<&str>,
        arn: Option<&str>,
    ) -> Result<String, RuleGroupLookupError> {
        // A firewall-engine or an IPS-engine rule group is a rule group of its
        // own kind, held in neither half of this name space: the rule-group
        // writes store none ([`RULE_GROUP_ENGINE_TYPES`]), so a request that
        // asks for that kind resolves to nothing — never to the rule group of
        // another kind that happens to carry the same name.
        if group_type.is_some_and(|group_type| RULE_GROUP_ENGINE_TYPES.contains(&group_type)) {
            return Err(RuleGroupLookupError::Resolve(ResolveError::NotFound));
        }
        let resource_path = match (name, group_type) {
            (Some(name), Some(group_type)) => Some(rule_group_resource_path(group_type, name)),
            (Some(_), None) if arn.is_none() => {
                return Err(RuleGroupLookupError::TypeRequired);
            }
            (_, _) => None,
        };
        let requested_type = group_type.map(rule_group_arn_type);
        let resource_types: &[&str] = match &requested_type {
            Some(resource_type) => ::std::slice::from_ref(resource_type),
            None => &ARN_TYPES_RULE_GROUP,
        };
        resolve(
            &self.rule_group_arn_index,
            name,
            resource_path.as_deref(),
            arn,
            resource_types,
            |key| self.rule_groups.contains_key(key),
        )
        .map_err(RuleGroupLookupError::Resolve)
    }

    /// Why a `FirewallPolicy` document's rule-group references cannot be served,
    /// as a message naming the first offending reference, or `None` when every
    /// one of them resolves.
    ///
    /// Two documented rules, both of them `InvalidRequestException`'s ("Your
    /// request references an ARN that is malformed, or corresponds to a resource
    /// that isn't valid in the context of the request"):
    ///
    /// * a reference names a rule group that EXISTS — the reference is that rule
    ///   group's ARN (`StatelessRuleGroupReference$ResourceArn` is "the Amazon
    ///   Resource Name (ARN) of the stateless rule group"), so an ARN no stored
    ///   rule group carries names no resource at all; and
    /// * it names one of the KIND its list is for: `StatelessRuleGroupReferences`
    ///   are "references to the stateless rule groups that are used in the
    ///   policy" and a `StatefulRuleGroupReference$ResourceArn` is "the ARN of
    ///   the stateful rule group", so a stateless reference naming a stateful
    ///   rule group (or the reverse) is a resource that is not valid in the
    ///   context of the request.
    ///
    /// This reads the store, so a caller runs it in the same critical section it
    /// inserts the policy in: a reference cannot be resolved and then go stale
    /// before the record lands.
    pub fn policy_reference_breach(&self, policy: &FirewallPolicy) -> Option<String> {
        let stateless = policy
            .stateless_rule_group_references
            .iter()
            .flatten()
            .map(|reference| (&reference.resource_arn, false));
        let stateful = policy
            .stateful_rule_group_references
            .iter()
            .flatten()
            .map(|reference| (&reference.resource_arn, true));

        for (arn, wants_stateful) in stateless.chain(stateful) {
            let Some(record) = self
                .rule_group_arn_index
                .get(arn)
                .and_then(|key| self.rule_groups.get(key))
            else {
                // The message is built from the request, so it takes the
                // service's one request-derived shape: a fixed stem naming the
                // referenced KIND and the defect, then the offending ARN in
                // parameter brackets. The input member the reference arrived in
                // is not part of it.
                return Some(missing_arn_message(REFERENCED_RULE_GROUP, arn));
            };
            // Every type other than STATELESS is a stateful rule group, which is
            // the same split the store's keys are built on.
            let is_stateful = record.rule_group_type != RULE_GROUP_STATELESS;
            if is_stateful != wants_stateful {
                // Same shape as the miss above: a fixed stem plus the ARN in
                // parameter brackets, naming the kind the reference list is for
                // rather than the list's member name.
                return Some(parameter_message(
                    &format!(
                        "{} rule group ARN is not valid",
                        if wants_stateful {
                            "stateful"
                        } else {
                            "stateless"
                        }
                    ),
                    arn,
                ));
            }
        }
        None
    }

    /// Add or overwrite `tags` on the resource `arn` names.
    ///
    /// A tag is a key:value pair, so a key the resource already carries takes
    /// the value this request gives and keeps the position it was first applied
    /// at, while a key it does not carry is appended in request order; a key
    /// repeated within one request ends up with the last value given. The
    /// documented ceiling ([`MAX_TAGS_PER_RESOURCE`]) is checked against the
    /// MERGED list before anything is stored, so a refused write leaves the
    /// resource's tags exactly as they were.
    pub fn apply_tags(&mut self, arn: &str, tags: Vec<Tag>) -> Result<(), TagWriteBreach> {
        let slot = self.resource_tags_mut(arn).ok_or(TagWriteBreach::NotFound)?;
        let mut merged = slot.clone().unwrap_or_default();
        for tag in tags {
            match merged.iter_mut().find(|stored| stored.key == tag.key) {
                Some(stored) => stored.value = tag.value,
                None => merged.push(tag),
            }
        }
        if merged.len() > MAX_TAGS_PER_RESOURCE {
            return Err(TagWriteBreach::TooManyTags(merged.len()));
        }
        *slot = Some(merged);
        Ok(())
    }

    /// Remove the tags `keys` names from the resource `arn` names, and answer
    /// `None` when `arn` names no stored resource of a taggable kind.
    ///
    /// Removal is by key, so a key the resource does not carry removes nothing
    /// and is not an error: `UntagResource` "removes the tags with the specified
    /// keys", which is a statement about the resulting tag set rather than about
    /// each key having been present. A resource left with no tags carries the
    /// member ABSENT, exactly as one created with no `Tags` does.
    pub fn remove_tags(&mut self, arn: &str, keys: &[String]) -> Option<()> {
        let slot = self.resource_tags_mut(arn)?;
        let mut remaining = slot.take().unwrap_or_default();
        remaining.retain(|tag| !keys.iter().any(|key| key == &tag.key));
        *slot = (!remaining.is_empty()).then_some(remaining);
        Some(())
    }

    /// The tags of the resource `arn` names — empty when it carries none — or
    /// `None` when `arn` names no stored resource of a kind this service tags.
    ///
    /// READ ONLY: a tag listing reports what the tag writes left behind and
    /// changes nothing a later caller sees.
    pub fn resource_tags(&self, arn: &str) -> Option<&[Tag]> {
        let stored = match taggable_resource_kind(arn)? {
            TaggableResource::Firewall => {
                &self.firewalls.get(self.firewall_arn_index.get(arn)?)?.tags
            }
            TaggableResource::FirewallPolicy => {
                &self
                    .firewall_policies
                    .get(self.firewall_policy_arn_index.get(arn)?)?
                    .tags
            }
            TaggableResource::RuleGroup => {
                &self
                    .rule_groups
                    .get(self.rule_group_arn_index.get(arn)?)?
                    .tags
            }
        };
        Some(stored.as_deref().unwrap_or_default())
    }

    /// The tag list of the resource `arn` names, for writing, or `None` when
    /// `arn` names no stored resource of a kind this service tags.
    ///
    /// Every resource that can be tagged keeps its tags on its OWN record — the
    /// same list its create, describe and delete responses project — so a tag
    /// write and a resource read can never disagree about one resource's tags.
    pub fn resource_tags_mut(&mut self, arn: &str) -> Option<&mut Option<Vec<Tag>>> {
        match taggable_resource_kind(arn)? {
            TaggableResource::Firewall => {
                let key = self.firewall_arn_index.get(arn)?.clone();
                self.firewalls.get_mut(&key).map(|record| &mut record.tags)
            }
            TaggableResource::FirewallPolicy => {
                let key = self.firewall_policy_arn_index.get(arn)?.clone();
                self.firewall_policies
                    .get_mut(&key)
                    .map(|record| &mut record.tags)
            }
            TaggableResource::RuleGroup => {
                let key = self.rule_group_arn_index.get(arn)?.clone();
                self.rule_groups
                    .get_mut(&key)
                    .map(|record| &mut record.tags)
            }
        }
    }

    /// Drop the resource policy attached to `resource_arn`, if any.
    ///
    /// Called wherever a shareable resource is removed: the policy is reachable
    /// only through that resource's own ARN, so an entry left behind would be
    /// unreadable state that nothing can ever delete.
    pub fn detach_resource_policy(&mut self, resource_arn: &str) {
        self.resource_policies.remove(resource_arn);
    }

    /// The capacity the rule groups `policy` references reserve. A reference to a
    /// rule group this emulator does not hold reserves nothing.
    ///
    /// The three members are deliberately NOT in one unit, because the model
    /// does not define them in one unit: `ConsumedStatelessRuleCapacity` and
    /// `ConsumedStatefulRuleCapacity` count the capacity a reference RESERVES, so
    /// a referenced rule group contributes its declared `Capacity`
    /// (`CreateRuleGroupRequest$Capacity`: "when you reference a rule group from
    /// a firewall policy, Network Firewall reserves this capacity for the rule
    /// group"), while `ConsumedStatefulDomainCapacity` is defined as a count of
    /// domain name specifications ("the total number of domain name
    /// specifications across all domain list rule groups in the firewall policy
    /// that use the `stateful-domain-rulegroup` resource type"), which is a
    /// domain-list group's CONSUMED capacity, not its declared one.
    ///
    /// A domain-list reference is therefore counted in the domain member ONLY,
    /// not in `stateful` as well: the model singles those rule groups out as a
    /// class of their own, measured in domain name specifications, so adding
    /// their declared capacity to the stateful total would report one reference
    /// under two members in two different units.
    pub fn consumed_policy_capacity(&self, policy: &FirewallPolicy) -> ConsumedPolicyCapacity {
        let mut consumed = ConsumedPolicyCapacity::default();
        for arn in policy_rule_group_references(policy) {
            let Some(record) = self
                .rule_group_arn_index
                .get(arn)
                .and_then(|key| self.rule_groups.get(key))
            else {
                continue;
            };
            match record.rule_group_type.as_str() {
                // Reserved: the declared capacity.
                RULE_GROUP_STATELESS => {
                    consumed.stateless = consumed.stateless.saturating_add(record.capacity);
                }
                // Counted in domain name specifications: the capacity the domain
                // list consumes.
                RULE_GROUP_STATEFUL_DOMAIN => {
                    consumed.stateful_domain = consumed.stateful_domain.saturating_add(
                        required_capacity(&record.rule_group_type, &record.rule_group),
                    );
                }
                // Reserved: the declared capacity.
                _ => {
                    consumed.stateful = consumed.stateful.saturating_add(record.capacity);
                }
            }
        }
        consumed
    }

    /// How many stored firewall policies reference the rule group `rule_group_arn`
    /// names.
    ///
    /// `RuleGroupResponse$NumberOfAssociations` is "the number of firewall
    /// policies that use this rule group", and a policy uses a rule group by
    /// naming its ARN in `FirewallPolicy.StatelessRuleGroupReferences` or
    /// `StatefulRuleGroupReferences` — so the count is READ from the policy
    /// store rather than kept as a counter beside it: a policy created, updated
    /// or deleted changes which rule groups it names, and a derived count cannot
    /// drift from the documents it is derived from.
    ///
    /// One policy that somehow named the same rule group twice would still be
    /// one policy using it, so the referencing policies are counted, not their
    /// references.
    ///
    /// READ ONLY: it reports what the policy writes left behind and changes
    /// nothing a later caller sees.
    pub fn rule_group_associations(&self, rule_group_arn: &str) -> i32 {
        let using = self
            .firewall_policies
            .values()
            .filter(|policy| {
                policy_rule_group_references(&policy.policy)
                    .any(|reference| reference == rule_group_arn)
            })
            .count();
        i32::try_from(using).unwrap_or(i32::MAX)
    }
}

/// Every rule-group ARN a firewall policy document references, stateless first.
fn policy_rule_group_references(policy: &FirewallPolicy) -> impl Iterator<Item = &str> {
    policy
        .stateless_rule_group_references
        .iter()
        .flatten()
        .map(|reference| reference.resource_arn.as_str())
        .chain(
            policy
                .stateful_rule_group_references
                .iter()
                .flatten()
                .map(|reference| reference.resource_arn.as_str()),
        )
}

/// The resource type and the resource name a reference ARN names, or why it names
/// no resource of this service.
///
/// [`own_resource`] is the one grammar; this reads its two refusals apart,
/// because they are different to a caller: a value that is not an ARN of this
/// service's form is MALFORMED, while one that is an ARN of another vendor names
/// a resource that is simply not addressable here.
fn reference_resource(arn: &str) -> Result<(&str, &str), ResolveError> {
    let vendor = ::service_common::arn::vendor(arn).ok_or(ResolveError::MalformedArn)?;
    if vendor != ARN_NAMESPACE {
        return Err(ResolveError::WrongResourceType);
    }
    own_resource(arn).ok_or(ResolveError::MalformedArn)
}

/// The shared name-or-ARN resolution rule, in the order a caller observes it: a
/// reference must be supplied; a supplied ARN must be a resource ARN of this
/// service ([`reference_resource`]); a name supplied beside it must be the name
/// that ARN carries; the ARN's resource type must be one of `resource_types`; and
/// only then must the resource the reference identifies — `name_key`, or the ARN
/// through the index — still exist.
///
/// `name` is the resource name the request gave and `name_key` the store key that
/// name identifies (the two differ where a resource's key is not its bare name).
/// The reference is CHECKED before the store is consulted, so a request that
/// names no resource — no identifier at all, an ARN that is not one, an ARN
/// carrying another name or another kind of resource — is a bad request rather
/// than a lookup miss.
fn resolve(
    arn_index: &HashMap<String, String>,
    name: Option<&str>,
    name_key: Option<&str>,
    arn: Option<&str>,
    resource_types: &[&str],
    exists: impl Fn(&str) -> bool,
) -> Result<String, ResolveError> {
    if let Some(arn) = arn {
        let (resource_type, resource_name) = reference_resource(arn)?;
        if name.is_some_and(|name| name != resource_name) {
            return Err(ResolveError::Mismatch);
        }
        if !resource_types.contains(&resource_type) {
            return Err(ResolveError::WrongResourceType);
        }
    }
    let key = match (name_key, arn) {
        (Some(name_key), None) => name_key.to_string(),
        // With both in hand the ARN must still be one this service minted: it
        // names a region and an account beyond the name the check above
        // compared, so an ARN nothing is stored under locates no resource even
        // when its name segment reads right.
        (Some(name_key), Some(arn)) => {
            let from_arn = arn_index.get(arn).cloned().ok_or(ResolveError::NotFound)?;
            if from_arn != name_key {
                return Err(ResolveError::Mismatch);
            }
            from_arn
        }
        (None, Some(arn)) => arn_index.get(arn).cloned().ok_or(ResolveError::NotFound)?,
        (None, None) => return Err(ResolveError::NoIdentifier),
    };
    if exists(&key) {
        Ok(key)
    } else {
        Err(ResolveError::NotFound)
    }
}

/// Generate one operation's [`ResolveError`] -> modelled-error mapping.
///
/// The handlers that resolve a resource by name or ARN each own a DIFFERENT error
/// enum (`#[operation_error]` admits only the variants that operation's model
/// declares), so the mapping cannot be a single function. This macro keeps it a
/// single piece of wording: a request whose REFERENCE is bad — no identifier at
/// all, an ARN that is not a well-formed one, an ARN of another kind of resource,
/// or a name and an ARN naming two different ones — is a bad request, each with
/// the one constant its check owns; only a well-formed reference that resolves to
/// nothing stored is a miss, answered with the service's one constant
/// [`RESOURCE_NOT_FOUND_MESSAGE`]. None of them carries anything from the
/// request.
///
/// Internal to this crate: `pub(crate) use` below keeps it off the crate's
/// public surface while still letting `src/ops/*` reach it by path.
macro_rules! impl_resolve_error {
    (
        $mapper:ident -> $error:ty, $modeled:ident,
        name_member = $name_member:literal, arn_member = $arn_member:literal
    ) => {
        /// Map a name/ARN resolution failure onto this operation's modelled errors.
        fn $mapper(error: $crate::state::ResolveError) -> $error {
            match error {
                $crate::state::ResolveError::NotFound => $modeled::ResourceNotFoundException(
                    $crate::ops::types::ResourceNotFoundException {
                        message: ::std::option::Option::Some(
                            $crate::state::RESOURCE_NOT_FOUND_MESSAGE.to_string(),
                        ),
                    },
                ),
                $crate::state::ResolveError::NoIdentifier => $modeled::InvalidRequestException(
                    $crate::ops::types::InvalidRequestException {
                        message: ::std::option::Option::Some(
                            concat!(
                                "At least one of ",
                                $name_member,
                                " or ",
                                $arn_member,
                                " must be set"
                            )
                            .to_string(),
                        ),
                    },
                ),
                $crate::state::ResolveError::MalformedArn => $modeled::InvalidRequestException(
                    $crate::ops::types::InvalidRequestException {
                        message: ::std::option::Option::Some(
                            $crate::state::ARN_INVALID_FORMAT_MESSAGE.to_string(),
                        ),
                    },
                ),
                $crate::state::ResolveError::WrongResourceType => {
                    $modeled::InvalidRequestException($crate::ops::types::InvalidRequestException {
                        message: ::std::option::Option::Some(
                            $crate::state::ARN_TYPE_MISMATCH_MESSAGE.to_string(),
                        ),
                    })
                }
                $crate::state::ResolveError::Mismatch => {
                    $modeled::InvalidRequestException($crate::ops::types::InvalidRequestException {
                        message: ::std::option::Option::Some(
                            $crate::state::ARN_NAME_MISMATCH_MESSAGE.to_string(),
                        ),
                    })
                }
            }
            .into()
        }
    };
}

pub(crate) use impl_resolve_error;

/// Generate one rule-group operation's [`RuleGroupLookupError`] ->
/// modelled-error mapping, on top of the [`ResolveError`] mapper
/// [`impl_resolve_error`] generated for the same operation.
///
/// Every operation that identifies a rule group by `RuleGroupName`/`Type`/
/// `RuleGroupArn` shares both refusals — a name without a type or an ARN is a bad
/// request, everything else is the ordinary name/ARN resolution — and each still
/// owns a different error enum, so this is the same per-operation expansion.
///
/// Internal to this crate: `pub(crate) use` below keeps it off the crate's
/// public surface while still letting `src/ops/*` reach it by path.
macro_rules! impl_rule_group_lookup_error {
    ($mapper:ident -> $error:ty, $modeled:ident, resolve = $resolve_mapper:ident) => {
        /// Map a rule-group lookup failure onto this operation's modelled errors.
        fn $mapper(error: $crate::state::RuleGroupLookupError) -> $error {
            match error {
                $crate::state::RuleGroupLookupError::TypeRequired => {
                    <$error>::from($modeled::InvalidRequestException(
                        $crate::ops::types::InvalidRequestException {
                            message: ::std::option::Option::Some(
                                $crate::state::TYPE_REQUIRED_MESSAGE.to_string(),
                            ),
                        },
                    ))
                }
                $crate::state::RuleGroupLookupError::Resolve(error) => $resolve_mapper(error),
            }
        }
    };
}

pub(crate) use impl_rule_group_lookup_error;

/// This service's store in the shared engine: call it with a handler's `&*self.engine`.
///
/// Answer `None` with a modelled server error, never with a fresh engine.
pub fn service_state<Eng: Send + Sync + 'static>(
    engine: &Eng,
) -> Option<::engine::ServiceState<ComAmazonawsNetworkfirewallNetworkfirewall20201112State>> {
    (engine as &dyn ::std::any::Any)
        .downcast_ref::<::engine::InMemoryEngine>()
        .map(::engine::InMemoryEngine::state::<ComAmazonawsNetworkfirewallNetworkfirewall20201112State>)
}
