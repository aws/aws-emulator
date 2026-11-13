// SPDX-License-Identifier: CC0-1.0

//! The `Message` a MediaLive rejection carries, end to end over the restJson1
//! wire.
//!
//! Rule: where MediaLive's message for a rejection CLASS is fixed, every
//! operation that reaches that class answers with exactly that message — a
//! constant sentence, or a constant template whose only variable is an
//! identifier the REQUEST supplied — and never with prose that interpolates
//! state the caller did not name (another resource's id, the resource's current
//! status, the region). The error code is the same `ConflictException` for every
//! busy/wrong-state conflict, so the message is the only discriminator a client
//! has for the reason.
//!
//! Every test drives the service the way a client does — through
//! `MultiServiceRouter::dispatch` with real HTTP requests — so what is asserted
//! is the emulator's wire behaviour, not an internal function's return value.
//!
//! The suites are tables over the CLASS: the recorded case of each rejection is
//! ONE row, and the other rows are the inputs the class generalises over (how
//! many resources hold the subject, which state it is in, which resource kind
//! the collision happened on, which way a value falls outside the rule).

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::channel_lifecycle::wait_for_the_clock;
use crate::input_security_group::{create_group, describe_group, update_group};
use crate::support::{entry_point, percent_encoded};
use crate::wire::{call, error_code, error_message, medialive, ACCOUNT, DEFAULT_REGION as REGION};

/// The message `DeleteInput` answers for an input a channel still attaches,
/// parameterized by the request's own `InputId` and by nothing else.
fn busy_input(input_id: &str) -> String {
    format!("Input {input_id} is busy, it cannot be deleted")
}

/// The fixed message a channel delete carries while the channel has not
/// stopped.
const CHANNEL_NOT_STOPPED: &str =
    "Cannot delete a channel that has not been stopped yet. Please stop the channel and try again.";

/// The fixed message a monitor resource's create carries when the region
/// already holds one of that type under the requested `Name` — the one
/// uniqueness rule all of them document ("Names must be unique within the scope
/// of a resource type in a specific region").
const NAME_ALREADY_IN_USE: &str = "Name already in use";

/// The fixed message a monitor template GROUP's delete carries while the group
/// still holds templates ("ensure its existing templates are moved to another
/// group or deleted", `smithy_query(query="doc",
/// shape="DeleteEventBridgeRuleTemplateGroup")`).
const TEMPLATE_GROUP_NOT_EMPTY: &str = "Template group must be empty before deletion.";

/// The fixed message an input security group's create or update carries when
/// its whitelist rules are not all valid IPv4 CIDRs.
const INVALID_WHITELIST_CIDR: &str = "Every whitelist needs to have a valid IPv4 CIDR.";

/// One `CreateInput`, answering the created input — with the resources its
/// configuration names put in the account first (`crate::support`).
async fn create_input(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = crate::support::resolved_input_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/inputs", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateInput: expected 201, got {status}: {response}"
    );
    response["input"].clone()
}

/// One `CreateChannel`, answering the created channel.
async fn create_channel(router: &MultiServiceRouter, case: &str, body: Value) -> Value {
    let body = crate::support::resolved_channel_body(router, body).await;
    let (status, response) = call(router, "POST", "/prod/channels", Some(body)).await;
    assert_eq!(
        status, 201,
        "case {case:?}: CreateChannel: expected 201, got {status}: {response}"
    );
    response["channel"].clone()
}

/// Assert the refusal's status, code and exact message, and that the message
/// carries nothing the request did not name.
fn assert_refusal(
    case: &str,
    status: u16,
    body: &Value,
    expected_status: u16,
    expected_code: &str,
    expected_message: &str,
    must_not_name: &[&str],
) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected status {expected_status}, got {status}: {body}"
    );
    assert_eq!(
        error_code(status, body),
        expected_code,
        "case {case:?}: expected {expected_code}, got {body}"
    );
    assert_eq!(
        error_message(body),
        expected_message,
        "case {case:?}: expected the message {expected_message:?}, got {:?}",
        error_message(body)
    );
    for leaked in must_not_name {
        assert!(
            !error_message(body).contains(leaked),
            "case {case:?}: expected the message not to carry {leaked:?}, got {:?}",
            error_message(body)
        );
    }
}

/// Rule row for the busy-input class: an input of some shape, held by some
/// number of channels through some number of attachments each.
struct BusyInput {
    case: &'static str,
    input: fn() -> Value,
    channels: usize,
    attachments_per_channel: usize,
}

/// Rule: `DeleteInput` on an input a channel still attaches is refused with
/// `ConflictException` (409) carrying "Input <InputId> is busy, it cannot be
/// deleted" — the request's own id and nothing about the channels that hold it.
///
/// The rows differ along what the message must NOT depend on: the input's type
/// and through how many attachments its channel holds it. No row has two
/// channels hold one input: an input is attached to at most one channel at a
/// time, so the second `CreateChannel` is refused with its own conflict.
#[tokio::test]
async fn a_busy_input_refusal_names_only_the_input_the_request_asked_about() {
    let table: [BusyInput; 3] = [
        BusyInput {
            case: "url pull held by one channel",
            input: || {
                json!({
                    "name": "pull-attached",
                    "type": "URL_PULL",
                    "sources": [{ "url": "https://example.com/a.m3u8" }],
                })
            },
            channels: 1,
            attachments_per_channel: 1,
        },
        BusyInput {
            case: "rtmp push held by one channel",
            input: || {
                json!({
                    "name": "push-attached",
                    "type": "RTMP_PUSH",
                    "destinations": [{ "streamName": "live/a" }],
                })
            },
            channels: 1,
            attachments_per_channel: 1,
        },
        BusyInput {
            case: "mediaconnect held twice by one channel",
            input: || {
                json!({
                    "name": "flow-attached",
                    "type": "MEDIACONNECT",
                    "mediaConnectFlows": [
                        { "flowArn": format!("arn:aws:mediaconnect:{REGION}:{ACCOUNT}:flow:1:a") },
                    ],
                    "roleArn": "arn:aws:iam::111122223333:role/MediaLiveAccessRole",
                })
            },
            channels: 1,
            attachments_per_channel: 2,
        },
    ];

    for BusyInput {
        case,
        input,
        channels,
        attachments_per_channel,
    } in table
    {
        let router = medialive();
        let created = create_input(&router, case, input()).await;
        let id = created["id"].as_str().unwrap_or_default().to_string();

        let mut channel_ids: Vec<String> = Vec::new();
        for index in 0..channels {
            let attachments: Vec<Value> = (0..attachments_per_channel)
                .map(|slot| json!({ "inputId": id, "inputAttachmentName": format!("slot-{slot}") }))
                .collect();
            let channel = create_channel(
                &router,
                case,
                json!({
                    "name": format!("attaching-{index}"),
                    "inputAttachments": attachments,
                }),
            )
            .await;
            assert_eq!(
                channel["inputAttachments"].as_array().map(Vec::len),
                Some(attachments_per_channel),
                "case {case:?}: expected channel {index} to attach the input {attachments_per_channel} time(s), got {:?}",
                channel["inputAttachments"]
            );
            channel_ids.push(channel["id"].as_str().unwrap_or_default().to_string());
        }

        let (status, refusal) = call(&router, "DELETE", &format!("/prod/inputs/{id}"), None).await;
        // The channels that hold the input are not in the message. Their ids
        // cannot be the probe — each resource family mints from its own counter,
        // so a channel and an input can share an id — so what must be absent is
        // the channel wording and the names only a channel here has, beside the
        // exact message asserted below.
        let mut leaked: Vec<&str> = vec!["channel", "arn:"];
        let names: Vec<String> = (0..channels).map(|index| format!("attaching-{index}")).collect();
        leaked.extend(names.iter().map(String::as_str));
        assert_eq!(
            channel_ids.len(),
            channels,
            "case {case:?}: expected {channels} attaching channel(s), got {channel_ids:?}"
        );
        assert_refusal(
            case,
            status,
            &refusal,
            409,
            "ConflictException",
            &busy_input(&id),
            &leaked,
        );
    }
}

/// Rule: `DeleteChannel` on a channel that has not stopped is refused with
/// `ConflictException` (409) carrying the fixed sentence of that class, which
/// names neither the channel nor the state it is in.
///
/// The rows are the states a channel can be in while it has not stopped yet —
/// the recorded `RUNNING` case is one of them — because the message is the
/// class's, not the state's.
#[tokio::test]
async fn a_channel_delete_before_it_stopped_carries_the_fixed_message() {
    // (case, the state the channel is left in before the delete)
    let table: [&str; 4] = ["CREATING", "STARTING", "RUNNING", "STOPPING"];

    for state in table {
        let case = format!("channel in {state}");
        let router = medialive();
        let channel = create_channel(&router, &case, json!({ "name": "doomed" })).await;
        let id = channel["id"].as_str().unwrap_or_default().to_string();

        // A new channel is CREATING; every other row drives it on from there.
        if state != "CREATING" {
            wait_for_the_clock();
            let (status, started) =
                call(&router, "POST", &format!("/prod/channels/{id}/start"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StartChannel: got {status}: {started}"
            );
        }
        if state == "RUNNING" || state == "STOPPING" {
            wait_for_the_clock();
        }
        if state == "STOPPING" {
            let (status, stopping) =
                call(&router, "POST", &format!("/prod/channels/{id}/stop"), None).await;
            assert_eq!(
                status, 200,
                "case {case:?}: StopChannel: got {status}: {stopping}"
            );
        }

        let (status, described) = call(&router, "GET", &format!("/prod/channels/{id}"), None).await;
        assert_eq!(
            described["state"],
            json!(state),
            "case {case:?}: expected the channel in {state} before the delete, got {status}: {described}"
        );

        let (status, refusal) = call(&router, "DELETE", &format!("/prod/channels/{id}"), None).await;
        assert_refusal(
            &case,
            status,
            &refusal,
            409,
            "ConflictException",
            CHANNEL_NOT_STOPPED,
            &[id.as_str(), state],
        );
    }
}

/// One monitor resource whose `Name` must be unique within its type and region.
#[derive(Debug, Clone, Copy)]
enum Monitor {
    /// POST /prod/cloudwatch-alarm-template-groups
    AlarmGroup,
    /// POST /prod/cloudwatch-alarm-templates
    AlarmTemplate,
    /// POST /prod/eventbridge-rule-template-groups
    RuleGroup,
    /// POST /prod/eventbridge-rule-templates
    RuleTemplate,
    /// POST /prod/signal-maps
    SignalMap,
}

impl Monitor {
    /// The collection this resource's create is bound to
    /// (`smithy_query(query="digest")`).
    fn collection(self) -> &'static str {
        match self {
            Monitor::AlarmGroup => "/prod/cloudwatch-alarm-template-groups",
            Monitor::AlarmTemplate => "/prod/cloudwatch-alarm-templates",
            Monitor::RuleGroup => "/prod/eventbridge-rule-template-groups",
            Monitor::RuleTemplate => "/prod/eventbridge-rule-templates",
            Monitor::SignalMap => "/prod/signal-maps",
        }
    }

    /// Whether this resource is created inside a template group, and if so which
    /// group kind it belongs to.
    fn parent(self) -> Option<Monitor> {
        match self {
            Monitor::AlarmTemplate => Some(Monitor::AlarmGroup),
            Monitor::RuleTemplate => Some(Monitor::RuleGroup),
            _ => None,
        }
    }
}

/// The body that creates one `kind` called `name`, inside `parent` (the group
/// identifier, where the kind has one) and discovered from `channel_arn` (the
/// signal map's `DiscoveryEntryPointArn`).
///
/// Every member a create requires comes from the model
/// (`smithy_query(query="operation",
/// operation="CreateCloudWatchAlarmTemplate,CreateEventBridgeRuleTemplate,CreateSignalMap")`),
/// with one modelled value each.
fn monitor_body(kind: Monitor, name: &str, parent: &str, channel_arn: &str) -> Value {
    match kind {
        Monitor::AlarmGroup | Monitor::RuleGroup => json!({ "name": name }),
        Monitor::AlarmTemplate => json!({
            "name": name,
            "groupIdentifier": parent,
            // `CloudWatchAlarmTemplateComparisonOperator`,
            // `CloudWatchAlarmTemplateStatistic`,
            // `CloudWatchAlarmTemplateTargetResourceType` and
            // `CloudWatchAlarmTemplateTreatMissingData` values, from those enums.
            "comparisonOperator": "GreaterThanThreshold",
            "evaluationPeriods": 1,
            "metricName": "InputVideoFrameRate",
            "period": 60,
            "statistic": "Average",
            "targetResourceType": "MEDIALIVE_CHANNEL",
            "threshold": 29.97,
            "treatMissingData": "notBreaching",
        }),
        Monitor::RuleTemplate => json!({
            "name": name,
            "groupIdentifier": parent,
            // An `EventBridgeRuleTemplateEventType` value, and an SNS topic ARN —
            // the kind `EventBridgeRuleTemplateTarget$Arn` documents.
            "eventType": "MEDIALIVE_CHANNEL_STATE_CHANGE",
            "eventTargets": [{ "arn": format!("arn:aws:sns:{REGION}:{ACCOUNT}:alerts") }],
        }),
        Monitor::SignalMap => json!({
            "name": name,
            "discoveryEntryPointArn": channel_arn,
        }),
    }
}

/// Create one monitor resource of `kind` called `name`, answering the status and
/// the body. The group a template needs is created first, under a name derived
/// from `name`, and a signal map's entry point is a channel of its own.
async fn create_monitor(
    router: &MultiServiceRouter,
    case: &str,
    kind: Monitor,
    name: &str,
) -> (u16, Value) {
    let parent = match kind.parent() {
        Some(group) => {
            let group_name = format!("{name}-group");
            let (status, created) = call(
                router,
                "POST",
                group.collection(),
                Some(monitor_body(group, &group_name, "", "")),
            )
            .await;
            // The group is created once per store; a row that creates the same
            // template twice reuses it, which is what makes the second create a
            // NAME collision rather than a new group.
            assert!(
                status == 201 || status == 409,
                "case {case:?}: expected the parent group to be created, got {status}: {created}"
            );
            group_name
        }
        None => String::new(),
    };
    let channel_arn = match kind {
        Monitor::SignalMap => entry_point(router, &format!("{name}-entry")).await,
        _ => String::new(),
    };
    call(
        router,
        "POST",
        kind.collection(),
        Some(monitor_body(kind, name, &parent, &channel_arn)),
    )
    .await
}

/// Rule: a monitor resource created under a `Name` its type already holds in the
/// region is refused with `ConflictException` (409) carrying the fixed message
/// "Name already in use" — not the name, not the colliding resource's id and not
/// the region.
///
/// The rows span every monitor resource whose create is implemented and whose
/// `Name` the model documents as unique "within the scope of a resource type in
/// a specific region" — the recorded eventbridge rule template is one of them —
/// and the names differ in shape (plain and outside ASCII), because the message
/// is the class's and depends on neither.
#[tokio::test]
async fn a_monitor_name_collision_carries_the_fixed_message() {
    let kinds: [Monitor; 5] = [
        Monitor::AlarmGroup,
        Monitor::AlarmTemplate,
        Monitor::RuleGroup,
        Monitor::RuleTemplate,
        Monitor::SignalMap,
    ];
    // Name shapes: a plain one, and one outside ASCII, which `Name`'s target
    // shape `__stringMin1Max255PatternS` admits (its pattern excludes only
    // whitespace).
    let names: [&str; 2] = ["taken-name", "既に使用中"];

    for kind in kinds {
        for name in names {
            let case = format!("{kind:?} named {name:?} twice");
            let router = medialive();

            let (status, first) = create_monitor(&router, &case, kind, name).await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected the first create to answer 201, got {status}: {first}"
            );
            let first_id = first["id"].as_str().unwrap_or_default().to_string();

            let (status, refusal) = create_monitor(&router, &case, kind, name).await;
            assert_refusal(
                &case,
                status,
                &refusal,
                409,
                "ConflictException",
                NAME_ALREADY_IN_USE,
                &[name, first_id.as_str(), REGION],
            );
        }
    }
}

/// Rule: a monitor template group whose delete is refused because it still holds
/// templates answers `ConflictException` (409) with the fixed sentence of that
/// class, naming neither the group nor the templates it holds.
///
/// The rows differ along the group KIND — both group resources document the same
/// precondition — and along how many templates the group still holds, which is
/// what the message must not depend on.
#[tokio::test]
async fn a_group_delete_before_it_is_empty_carries_the_fixed_message() {
    // (the group kind, the template kind it holds, how many it holds)
    let table: [(Monitor, Monitor, usize); 5] = [
        (Monitor::AlarmGroup, Monitor::AlarmTemplate, 1),
        (Monitor::AlarmGroup, Monitor::AlarmTemplate, 3),
        (Monitor::RuleGroup, Monitor::RuleTemplate, 1),
        (Monitor::RuleGroup, Monitor::RuleTemplate, 2),
        (Monitor::RuleGroup, Monitor::RuleTemplate, 5),
    ];

    for (group_kind, template_kind, held) in table {
        let case = format!("{group_kind:?} holding {held} template(s)");
        let router = medialive();

        let group = "crowded";
        let (status, created) = call(
            &router,
            "POST",
            group_kind.collection(),
            Some(monitor_body(group_kind, group, "", "")),
        )
        .await;
        assert_eq!(
            status, 201,
            "case {case:?}: expected the group to be created, got {status}: {created}"
        );

        let mut template_ids: Vec<String> = Vec::new();
        for index in 0..held {
            let (status, template) = call(
                &router,
                "POST",
                template_kind.collection(),
                Some(monitor_body(
                    template_kind,
                    &format!("member-{index}"),
                    group,
                    "",
                )),
            )
            .await;
            assert_eq!(
                status, 201,
                "case {case:?}: expected member {index} to be created, got {status}: {template}"
            );
            template_ids.push(template["id"].as_str().unwrap_or_default().to_string());
        }

        let (status, refusal) = call(
            &router,
            "DELETE",
            &format!("{}/{}", group_kind.collection(), percent_encoded(group)),
            None,
        )
        .await;
        let mut leaked: Vec<&str> = template_ids.iter().map(String::as_str).collect();
        leaked.push(group);
        assert_refusal(
            &case,
            status,
            &refusal,
            409,
            "ConflictException",
            TEMPLATE_GROUP_NOT_EMPTY,
            &leaked,
        );
    }
}

/// Rule: a whitelist rule that is not a valid IPv4 CIDR is refused with
/// `BadRequestException` (400) carrying the fixed sentence of that class, which
/// names neither the offending member's index nor the value it carried — the
/// request is refused as a whole, and the operation models no per-member error.
///
/// The rows span the ways a value falls outside the rule (no prefix, a prefix
/// one past the published bound of 32, a non-numeric prefix, an octet out of
/// range, an IPv6 prefix, an empty string, a rule naming no CIDR at all, and a
/// valid rule followed by an invalid one at a non-zero index) across BOTH
/// operations that write a whitelist, since they document the member identically.
#[tokio::test]
async fn a_whitelist_rule_that_is_not_an_ipv4_cidr_carries_the_fixed_message() {
    // (case, the whitelistRules the request carries, the text the message must
    //  not carry)
    let table: [(&str, Value, &str); 8] = [
        ("bare address, no prefix", json!([{ "cidr": "10.0.0.0" }]), "10.0.0.0"),
        ("prefix one past the bound", json!([{ "cidr": "10.0.0.0/33" }]), "/33"),
        ("non-numeric prefix", json!([{ "cidr": "10.0.0.0/x" }]), "/x"),
        ("octet out of range", json!([{ "cidr": "256.0.0.0/8" }]), "256.0.0.0"),
        ("an IPv6 prefix", json!([{ "cidr": "2001:db8::/32" }]), "2001:db8::"),
        ("empty string", json!([{ "cidr": "" }]), "WhitelistRules"),
        ("a rule that names no CIDR", json!([{}]), "WhitelistRules"),
        (
            "a valid rule followed by an invalid one",
            json!([{ "cidr": "10.9.0.0/16" }, { "cidr": "nonsense" }]),
            "nonsense",
        ),
    ];

    for (case, rules, must_not_name) in table {
        let router = medialive();

        // The create path.
        let (status, refusal) = call(
            &router,
            "POST",
            "/prod/inputSecurityGroups",
            Some(json!({ "whitelistRules": rules })),
        )
        .await;
        assert_refusal(
            &format!("create with {case}"),
            status,
            &refusal,
            400,
            "BadRequestException",
            INVALID_WHITELIST_CIDR,
            &[must_not_name],
        );

        // The update path, on a group that already has a valid rule.
        let existing = create_group(
            &router,
            case,
            json!({ "whitelistRules": [{ "cidr": "10.0.0.0/16" }] }),
        )
        .await;
        let id = existing["id"].as_str().unwrap_or_default().to_string();
        let (status, refusal) = update_group(&router, &id, json!({ "whitelistRules": rules })).await;
        assert_refusal(
            &format!("update with {case}"),
            status,
            &refusal,
            400,
            "BadRequestException",
            INVALID_WHITELIST_CIDR,
            &[must_not_name],
        );

        // The refusal wrote nothing: the group still carries the rule it had,
        // and a rule that IS a valid IPv4 CIDR still goes through.
        let (status, described) = describe_group(&router, &id).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected the group to still be readable, got {status}: {described}"
        );
        assert_eq!(
            described["whitelistRules"],
            json!([{ "cidr": "10.0.0.0/16" }]),
            "case {case:?}: expected the refused update to leave the whitelist alone, got {:?}",
            described["whitelistRules"]
        );
        // Each published bound of an IPv4 prefix length is accepted, so the
        // refusal above is the rule's and not a blanket one.
        for cidr in ["0.0.0.0/0", "10.0.0.1/32"] {
            let (status, updated) =
                update_group(&router, &id, json!({ "whitelistRules": [{ "cidr": cidr }] })).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected {cidr} to be accepted, got {status}: {updated}"
            );
        }
    }
}
