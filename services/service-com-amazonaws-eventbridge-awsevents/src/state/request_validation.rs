// SPDX-License-Identifier: CC0-1.0

//! The submitted VALUES this service refuses, and the wording of each refusal.
//!
//! Every rule below is one EventBridge enforces while the model expresses none of
//! it — a reserved namespace, the service a referenced resource has to belong to,
//! a member another member makes required — so the boundary validator passes the
//! request and the check has to be made where the write happens. Each is stated
//! ONCE here and reached from every operation that writes the member in question,
//! so two writers of one member cannot admit different values: the dead-letter
//! ARN `CreateEventBus`, `UpdateEventBus` and `PutTargets` each write goes through
//! one check, and so does the tag set every tag write of this service merges
//! ([`super::merge_tags`]).
//!
//! Each refusal is worded as the service words that refusal: a sentence stating
//! the requirement the submitted value failed, in the member's own terms. Where
//! the service states only that a parameter is not valid, the refusal is that
//! subject clause alone ([`super::parameter_not_valid`]).

use super::{
    invalid_arn_violation, parameter_not_valid, schedule, DeadLetterConfig, Tag, Target,
    ARN_NAMESPACE, DEFAULT_EVENT_BUS_NAME,
};

/// The namespace EventBridge reserves for the event buses IT names.
///
/// Only one bus name in it is a customer's to create: the partner event bus,
/// whose name must exactly match the partner event source it is matched to and so
/// lies in `aws.partner/...` (`EventSourceName`'s modelled pattern,
/// `^aws\.partner(/[\.\-_A-Za-z0-9]+){2,}$`). The same namespace is reserved for
/// the `source` of a published event (`crate::ops::put_events`) and for the
/// placeholders an input transformation fills itself
/// ([`super::input_transform`]) — one reserved namespace, read at each member a
/// caller can write it into.
const RESERVED_BUS_NAMESPACE: &str = "aws.";

/// Why `name` is not a name a CUSTOM event bus may carry, or `None` when it is.
///
/// A custom bus is one this account names itself, and the reserved namespace is
/// not the account's to name in: a bus created there would shadow the buses
/// EventBridge itself delivers a service's events to. The partner event bus is
/// the exception, and it is not created by naming it — it is created by naming
/// the partner event source it matches (`CreateEventBus$EventSourceName`), which
/// the caller must already have been offered — so a request that names no source
/// may not reach into the namespace at all.
pub fn reserved_bus_name_violation(name: &str) -> Option<String> {
    name.starts_with(RESERVED_BUS_NAMESPACE)
        .then(|| format!("EventBus name starting with '{RESERVED_BUS_NAMESPACE}' is not valid."))
}

/// The ARN namespace of Amazon SQS, named once: it is the one service a
/// dead-letter queue is a queue of, and the service whose FIFO queues a target
/// may only be delivered to with a message group ([`target_parameters_violation`]).
const SQS_ARN_NAMESPACE: &str = "sqs";

/// What the service states for a dead-letter destination that is not a queue: the
/// SERVICE the submitted ARN named, which is the one thing wrong with it, and
/// that EventBridge supports no dead-letter resource of that service. Stated
/// wherever a `DeadLetterConfig` is written.
fn unsupported_dead_letter_resource(vendor: &str) -> String {
    format!("{vendor} is not supported as a dead letter resource.")
}

/// Why a `DeadLetterConfig` names no dead-letter queue, or `None` when it does.
///
/// `DeadLetterConfig$Arn` is "the ARN of the SQS queue specified as the target
/// for the dead-letter queue", and the structure itself is "configuration details
/// of the Amazon SQS queue for EventBridge to use as a dead-letter queue (DLQ)":
/// the destination is a queue of ONE service, so an ARN naming a resource of any
/// other service is one EventBridge supports no dead-letter write to. The member
/// is modelled as a plain 1..1600 `ResourceArn`, so the boundary validator passes
/// any text — and text that is no ARN at all names no service to report, so it is
/// refused as the malformed ARN it is ([`super::invalid_arn_violation`]).
pub fn dead_letter_arn_violation(config: &DeadLetterConfig) -> Option<String> {
    let arn = config.arn.as_deref()?;
    match ::service_common::arn::vendor(arn) {
        Some(SQS_ARN_NAMESPACE) => None,
        Some(vendor) => Some(unsupported_dead_letter_resource(vendor)),
        None => Some(invalid_arn_violation(arn)),
    }
}

/// Why a `ScheduleExpression` is not one EventBridge accepts, or `None` when it
/// is.
///
/// A schedule expression is a rate or a cron expression — "The scheduling
/// expression. For example, `cron(0 20 * * ? *)` or `rate(5 minutes)`"
/// (`PutRule`'s `ScheduleExpression`) — so an expression EventBridge would fire
/// nothing on is refused rather than stored and silently never fired. Whether it
/// states a schedule at all is decided by the PARSER that fires one
/// ([`schedule::states_a_schedule`]): each form's grammar is read field by field
/// there, so an expression this service accepts is one the clock can make due,
/// and none is taken on the strength of its opening word alone.
///
/// The refusal names the PARAMETER and stops there: EventBridge discloses
/// nothing about what is wrong with a schedule expression it will not take — not
/// the text submitted, not which of the two forms was expected — so the message
/// is the subject clause alone, with no reason after it
/// ([`super::parameter_not_valid`]).
pub fn schedule_expression_violation(expression: &str) -> Option<String> {
    (!schedule::states_a_schedule(expression)).then(|| parameter_not_valid("ScheduleExpression"))
}

/// Why a rule carrying a `ScheduleExpression` cannot live on the event bus
/// `bus`, or `None` when it can.
///
/// A scheduled rule self-triggers — "rules with `ScheduleExpressions`
/// self-trigger based on the given schedule" (`PutRule`) — and EventBridge fires
/// such a rule on the account's default event bus only: a schedule on any other
/// bus would be a rule the service never triggers, so the request is refused
/// rather than answered with a rule that fires nowhere.
pub fn schedule_expression_bus_violation(bus: &str) -> Option<String> {
    (bus != DEFAULT_EVENT_BUS_NAME)
        .then(|| "ScheduleExpression is supported only on the default event bus.".to_string())
}

/// The reserved prefix a tag key a CUSTOMER writes may not carry.
///
/// The `aws:` namespace is the one Amazon Web Services itself tags resources in,
/// so a key in it is not a caller's to set or change on any resource of any
/// service.
const RESERVED_TAG_KEY_PREFIX: &str = "aws:";

/// Why a submitted tag list carries a key this service will not store, or `None`
/// when every key is the caller's own.
///
/// One reserved key refuses the whole write, so the tag set a resource carries
/// never holds half of a request. Matched without regard to case: the reserved
/// namespace is the prefix itself rather than one spelling of it, so a key may
/// not reach into it by changing case.
///
/// The refusal states the rule AWS states for the namespace — the prefix is
/// reserved for Amazon Web Services' own use, and a caller can neither create nor
/// modify a key carrying it — rather than echoing a key back: what is wrong with
/// the key is the namespace it is in, which the caller already has.
pub fn reserved_tag_key_violation(tags: &[Tag]) -> Option<String> {
    tags.iter()
        .find(|tag| {
            tag.key
                .get(..RESERVED_TAG_KEY_PREFIX.len())
                .is_some_and(|prefix| prefix.eq_ignore_ascii_case(RESERVED_TAG_KEY_PREFIX))
        })
        .map(|_| {
            format!("Tag keys starting with '{RESERVED_TAG_KEY_PREFIX}' are reserved for AWS use.")
        })
}

/// Why one target of a `PutTargets` request cannot be installed as given, or
/// `None` when it can.
///
/// The per-target requirements that follow from the target's own TYPE and from
/// the members it carries, in the order a caller has to resolve them: the role
/// EventBridge needs to invoke the target at all, the parameters the target's
/// own kind of resource can only be delivered to with, the service its
/// dead-letter destination has to belong to, and the JSON paths it reshapes its
/// input with. Each is a property of ONE target, so the request is refused whole
/// before any of its targets is stored — a rule carrying a target EventBridge
/// could never invoke would report back a healthy rule that delivers nothing.
///
/// This is the per-target counterpart of [`super::target_arn_violation`], which
/// checks the FORM of each ARN-valued member and runs first: a value that is no
/// ARN is reported as such rather than as a resource of the wrong service.
pub fn target_violation(target: &Target) -> Option<String> {
    target_role_violation(target)
        .or_else(|| target_parameters_violation(target))
        .or_else(|| {
            target
                .dead_letter_config
                .as_ref()
                .and_then(dead_letter_arn_violation)
        })
        .or_else(|| target_input_path_violation(target))
}

/// The suffix the name of an Amazon SQS FIFO queue carries — a queue whose name
/// does not end in it is a standard queue. Source: Amazon SQS requires the name
/// of a FIFO queue to end with `.fifo` (`CreateQueue$QueueName`), and a queue
/// name is case-sensitive, so the suffix is matched as written.
const FIFO_QUEUE_SUFFIX: &str = ".fifo";

/// The `Target` member carrying the parameters an Amazon SQS queue target is
/// delivered with.
const SQS_PARAMETERS_MEMBER: &str = "SqsParameters";

/// What the service states for a target whose own resource can only be delivered
/// to with parameters the request did not specify: the `Target` member that
/// carries them, and the `Id` of the target they were required for. Stated once,
/// so every such requirement is refused in the same words.
fn missing_target_parameters(member: &str, target_id: &str) -> String {
    format!("Parameter(s) {member} must be specified for target: {target_id}.")
}

/// Why a target names none of the parameters its own kind of resource can only be
/// delivered to with, or `None` when it names them or needs none.
///
/// `Target$SqsParameters` "contains the message group ID to use when the target is
/// an Amazon SQS fair or FIFO queue", and a message sent to a FIFO queue without
/// one is a send that queue refuses (Amazon SQS `SendMessage$MessageGroupId`): a
/// FIFO queue target carrying no message group is a target EventBridge could never
/// deliver to, so the request is refused when the target is CONFIGURED rather than
/// stored and found undeliverable at every firing.
///
/// A FIFO queue is one whose NAME carries [`FIFO_QUEUE_SUFFIX`], which is the
/// resource field of its ARN — read through the one ARN parse
/// (`service_common::arn`), so a queue of this service's own reading. The member
/// is required as the parameter BLOCK: `SqsParameters$MessageGroupId` is modelled
/// `@length(min: 0)`, so a group id of no characters is a value the caller
/// specified, while a `SqsParameters` carrying no `MessageGroupId` at all
/// specifies none.
fn target_parameters_violation(target: &Target) -> Option<String> {
    let fifo_queue = ::service_common::arn::parse_of(&target.arn, SQS_ARN_NAMESPACE)
        .is_some_and(|queue| queue.resource.ends_with(FIFO_QUEUE_SUFFIX));
    let names_a_group = target
        .sqs_parameters
        .as_ref()
        .is_some_and(|parameters| parameters.message_group_id.is_some());
    (fifo_queue && !names_a_group)
        .then(|| missing_target_parameters(SQS_PARAMETERS_MEMBER, &target.id))
}

/// Why a target names no role for an invocation EventBridge makes AS one, or
/// `None` when the target needs none or names it.
///
/// "To be able to make API calls against the resources that you own, Amazon
/// EventBridge needs the appropriate permissions: for Lambda and Amazon SNS
/// resources, EventBridge relies on resource-based policies; for EC2 instances,
/// Kinesis Data Streams, Step Functions state machines and API Gateway APIs,
/// EventBridge relies on IAM roles that you specify in the `RoleARN` argument in
/// `PutTargets`" (this operation's own documentation) — and an event bus of
/// another account is in that second class too ("you must specify a `RoleArn`
/// with proper permissions in the `Target` structure"). A target of that class
/// carrying no `RoleArn` names nothing for EventBridge to assume, so the
/// delivery it asks for could never be made.
///
/// The classes are the ones that documentation ENUMERATES, read off the service
/// and the resource type of the target's ARN ([`ROLE_INVOKED_TARGETS`]) — a
/// target of any other service is one this operation accepts without a role,
/// whether it is invoked under a resource-based policy or under a role the
/// delivery assumes when one is given
/// ([`crate::ops::put_events`]'s own reading of the same sentence).
fn target_role_violation(target: &Target) -> Option<String> {
    if target.role_arn.is_some() || !invoked_under_a_named_role(&target.arn) {
        return None;
    }
    Some(format!("RoleArn is required for target {}.", target.arn))
}

/// The target classes `PutTargets` documents as invoked under the role the target
/// names: the SERVICE of the ARN, and the resource TYPE within it that names a
/// target of that class (`None` where the service names one kind of target only).
///
/// An ARN of the same service naming another kind of resource is NOT one of them:
/// an `arn:aws:events:…:api-destination/…` is an HTTP endpoint this service
/// invokes through a connection's own credentials, not the event bus of another
/// account the documentation is about.
const ROLE_INVOKED_TARGETS: [(&str, Option<&str>); 5] = [
    // "Step Functions state machines"
    ("states", Some("stateMachine")),
    // "Kinesis Data Streams"
    ("kinesis", Some("stream")),
    // "EC2 instances"
    ("ec2", Some("instance")),
    // "API Gateway APIs", whose invocation ARNs are of the `execute-api` service
    ("execute-api", None),
    // The event bus of another account or Region
    (ARN_NAMESPACE, Some("event-bus")),
];

/// Whether the target `arn` names a resource of one of [`ROLE_INVOKED_TARGETS`].
fn invoked_under_a_named_role(arn: &str) -> bool {
    let Some(parsed) = ::service_common::arn::parse(arn) else {
        return false;
    };
    ROLE_INVOKED_TARGETS
        .iter()
        .any(|(vendor, resource_type)| {
            parsed.vendor == *vendor
                && resource_type.is_none_or(|kind| parsed.resource_type() == kind)
        })
}

/// Why a target reshapes its input through a path EventBridge does not evaluate,
/// or `None` when every path it carries is one.
///
/// "If `InputPath` is specified in the form of JSONPath (for example,
/// `$.detail`), then only the part of the event specified in the path is passed
/// to the target", `InputPathsMap` is a map "where each value is a valid JSON
/// path", and "when you specify `InputPath` or `InputTransformer`, you must use
/// JSON dot notation, not bracket notation" (`PutTargets`). So a path that is not
/// rooted at the event, that names its members in bracket notation, or that
/// selects through a construct the supported subset does not carry, is refused
/// when the target is CONFIGURED rather than stored and found undeliverable at
/// delivery time.
///
/// The supported subset is the one this service evaluates
/// (`service_common::json_reference_path`, which admits `$`, `.name`, `[index]`
/// and one many-place selector), so a target this operation accepts is one every
/// delivery can reshape.
///
/// The paths are judged in a stable order — `InputPath` first, then the map's
/// keys sorted — because `InputPathsMap` arrives as an unordered map and a
/// refusal must name the same target for the same request every time. Either
/// member is one way of stating the same thing — the JSON path a target's input
/// is selected by — so the refusal is worded for the TARGET rather than for the
/// member or the path that carried it.
fn target_input_path_violation(target: &Target) -> Option<String> {
    let unsupported = |path: &String| !supported_path(path);
    let carried = target.input_path.as_ref().filter(|path| unsupported(path));
    let mapped = || {
        let map = target
            .input_transformer
            .as_ref()
            .and_then(|transformer| transformer.input_paths_map.as_ref())?;
        let mut paths: Vec<&String> = map.values().collect();
        paths.sort();
        paths.into_iter().find(|path| unsupported(path))
    };
    carried
        .or_else(mapped)
        .map(|_| format!("InputPath for target {} is invalid.", target.id))
}

/// The bracket forms a MEMBER may be named with in a JSON path, which this
/// member's documentation rules out ("you must use JSON dot notation, not bracket
/// notation"): a quoted name inside a `[…]` selector.
///
/// An index selector is bracketed too and is not one of these: it names an
/// element of an array rather than a member of an object, which is the only way
/// an element can be named at all.
const BRACKETED_MEMBER_FORMS: [&str; 2] = ["['", "[\""];

/// Whether `path` is one this service reads: a reference path in the supported
/// subset, naming each member of the event in dot notation.
fn supported_path(path: &str) -> bool {
    !BRACKETED_MEMBER_FORMS
        .iter()
        .any(|bracketed| path.contains(bracketed))
        && ::service_common::json_reference_path::selection_segments(path).is_some()
}
