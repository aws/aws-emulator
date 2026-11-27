// SPDX-License-Identifier: CC0-1.0

//! How this service states that a request named a resource it does not hold.
//!
//! AWS IoT has no single not-found sentence: each resource FAMILY has its own fixed
//! text, and the variable parts of it come from the request and from this account's
//! state — never from any one call's recording.
//!
//! * The REGISTRY families — a thing, a thing group, a thing type, a billing group —
//!   qualify the name with the ACCOUNT the call was made for:
//!   `<Family> <accountId>:<name> not found.` ([`registry_not_found`]).
//! * The JOBS and COMMANDS planes word it `<Family> <id> cannot be found.`
//!   ([`plane_not_found`]) — including a command EXECUTION, whose sentence is shared with
//!   the device-facing half of the same plane
//!   ([`missing_command_execution_message`]).
//! * The AUTHORIZER family words it `Authorizer <name> not found`.
//! * The DEVICE DEFENDER audit-task family words it
//!   `Audit task with TaskId=<id> was not found` ([`missing_audit_task_message`]).
//! * The DEVICE DEFENDER CONFIGURATION families — a scheduled audit, and the write path of a
//!   mitigation action — word it by naming the INPUT MEMBER the request carried the name at:
//!   `Resource <member>=<name> was not found` ([`unresolved_resource_message`]), with no
//!   account qualifier and no terminating period.
//! * The BULK THING REGISTRATION task family words it with a sentence that carries no
//!   variable part at all ([`MISSING_THING_REGISTRATION_TASK_MESSAGE`]): the task id is
//!   not interpolated.
//! * The CA-CERTIFICATE REGISTRATION CODE family words it with a sentence that carries no
//!   variable part either ([`MISSING_REGISTRATION_CODE_MESSAGE`]), because the operation
//!   that addresses it names nothing at all.
//!
//! Every operation that addresses ONE resource of a family refuses a lookup miss with
//! that family's sentence, so a describe, a delete and an update of the same resource
//! cannot word the same miss three ways — which is the whole reason these sentences live
//! here rather than in each handler.
//!
//! THREE families are SPLIT, because the service itself words their halves differently, and a
//! split one is stated here too so both halves stay in this one place:
//!
//! * the FLEET METRIC family, whose read states the registry families' account-qualified
//!   sentence ([`missing_fleet_metric_message`]) while its write states the name alone
//!   ([`fleet_metric_write_not_found_message`]) — `FleetMetric <metricName> doesn't exist`,
//!   which is what `UpdateFleetMetric` answers;
//! * the THING family, split the same way: every READ of a thing states the
//!   account-qualified sentence ([`missing_thing_message`]), while the operation that
//!   WRITES the thing record addressed by name — `UpdateThing` — states the thing name
//!   alone ([`thing_write_not_found_message`]);
//! * the THING GROUP family, whose operations addressing the GROUP state the registry
//!   families' sentence ([`missing_thing_group_message`]), while the operation that WRITES
//!   the group record addressed by name — `UpdateThingGroup` — states the group name alone
//!   ([`thing_group_write_not_found_message`]), the operations addressing a
//!   resource by ARN alone — the tagging plane — state
//!   [`tagged_thing_group_not_found_message`] and the operations that write a THING's
//!   membership of a group state
//!   [`membership_missing_thing_group_message`].
//!
//! A family is split here only on that evidence, never to let a handler phrase a miss its
//! own way: a handler names the half it answers with and words nothing itself.

/// The sentence a REGISTRY family states a missing resource with: the resource noun, the
/// account the call was made for, `:`, the name the request asked about, and a
/// terminating period.
///
/// The account id is this emulator's own ([`engine::emulator_account_id`]), which is the
/// account every record this service holds belongs to.
fn registry_not_found(family: &str, name: &str) -> String {
    format!(
        "{family} {}:{name} not found.",
        ::engine::emulator_account_id()
    )
}

/// The sentence the JOBS and COMMANDS planes state a missing resource with: the resource
/// noun, the id the request asked about, and the fixed clause ` cannot be found.`
fn plane_not_found(family: &str, id: &str) -> String {
    format!("{family} {id} cannot be found.")
}

/// The sentence the DEVICE DEFENDER configuration families state a missing resource with:
/// the fixed word `Resource`, the name of the INPUT MEMBER the request carried the
/// resource's name at, `=`, that name, and the fixed clause ` was not found`.
///
/// Neither the account nor a terminating period is part of it, unlike the registry
/// families' sentence ([`registry_not_found`]): this one states the member the service
/// could not resolve, so a caller is told which part of the request named nothing.
pub fn unresolved_resource_message(member: &str, name: &str) -> String {
    format!("Resource {member}={name} was not found")
}

/// How this service states that a `thingName` names no thing in the registry to a READ of
/// one (`DescribeThing`, and every other operation that reports on a thing record).
///
/// A thing is a REGISTRY resource of this account, so a read states the miss the way the
/// registry families state one: the family noun, the account the record would belong to,
/// and the name the request asked about. The WRITE of a thing record states it in the
/// family's other wording ([`thing_write_not_found_message`]).
pub fn missing_thing_message(thing_name: &str) -> String {
    registry_not_found("Thing", thing_name)
}

/// How this service states that a `thingName` names no thing in the registry to the
/// operation that WRITES that record addressed by name (`UpdateThing`): the family noun,
/// the name the request asked about, and ` not found.`, with the account NOT named — the
/// other half of [`missing_thing_message`], the way the fleet-metric family's write half
/// ([`fleet_metric_write_not_found_message`]) is the other half of its read's sentence.
///
/// The thing name the request sent is the sentence's ONLY variable part, so nothing of
/// the serving account appears in it.
pub fn thing_write_not_found_message(thing_name: &str) -> String {
    format!("Thing {thing_name} not found.")
}

/// How the JOBS plane states that a `targets` entry names no thing this account holds.
///
/// A target is resolved by the plane that runs work on devices, so the miss is stated the
/// way that plane states every other one ([`plane_not_found`]): the family noun, the name
/// the target ARN carried, and the clause ` cannot be found.` — not the registry
/// families' account-qualified sentence ([`missing_thing_message`]), which is what a READ
/// of the thing record answers with.
pub fn missing_job_target_thing_message(thing_name: &str) -> String {
    plane_not_found("Thing", thing_name)
}

/// How this service states that a `thingGroupName` names no thing group this account
/// holds — for a reader of one, a writer of one, and for a job whose target named it.
pub fn missing_thing_group_message(thing_group_name: &str) -> String {
    registry_not_found("ThingGroup", thing_group_name)
}

/// How this service states that a `thingGroupName` names no thing group this account holds
/// to the operation that WRITES that group record addressed by name (`UpdateThingGroup`):
/// the fixed words `Group ` and ` does not exist`, with the account NOT named and no
/// terminating period — the other half of [`missing_thing_group_message`], the way
/// [`thing_write_not_found_message`] is the other half of the thing family's read sentence.
///
/// The group name the `{thingGroupName}` URI label carried is the sentence's ONLY variable
/// part, so nothing of the serving account appears in it.
pub fn thing_group_write_not_found_message(thing_group_name: &str) -> String {
    format!("Group {thing_group_name} does not exist")
}

/// How the MEMBERSHIP plane states that a request named no thing group this account holds.
///
/// The operations that write which groups a THING belongs to — `AddThingToThingGroup`,
/// `RemoveThingFromThingGroup` and `UpdateThingGroupsForThing` — state this family's miss
/// as the lookup they make rather than as a read of the group: the fixed text says the
/// requested group could not be found, and the variable parts are the two things that
/// identify a group, the account the call was made for and the name the request asked
/// about. The operations that address the group RECORD state the registry families'
/// sentence instead ([`missing_thing_group_message`]).
///
/// The account id is this emulator's own ([`engine::emulator_account_id`]), the account
/// every record this service holds belongs to.
pub fn membership_missing_thing_group_message(thing_group_name: &str) -> String {
    format!(
        "Cannot find requested thing group for accountId: {} and thing group name: \
         {thing_group_name}",
        ::engine::emulator_account_id()
    )
}

/// How the TAG plane states that a `resourceArn` names no thing group this account holds.
///
/// The operations that address a resource by ARN ALONE — `TagResource`, `UntagResource`
/// and `ListTagsForResource` — state this family's miss in their own sentence: the family
/// noun as TWO words, the name the ARN carried, and ` does not exist.` The account is not
/// named, unlike the sentence the operations addressing the GROUP state
/// ([`missing_thing_group_message`]), and the sentence ends in a period.
pub fn tagged_thing_group_not_found_message(thing_group_name: &str) -> String {
    format!("Thing Group {thing_group_name} does not exist.")
}

/// How this service states that a `metricName` names no fleet metric this account
/// holds to a READ of one (`DescribeFleetMetric`, and the tag plane's lookup of one).
///
/// A fleet metric is a REGISTRY resource of this account like a thing or a thing
/// group, so a read states the miss the same way: the family noun, the account the
/// record would belong to and the name the request asked about. A WRITE of one states
/// it in its own wording ([`fleet_metric_write_not_found_message`]).
pub fn missing_fleet_metric_message(metric_name: &str) -> String {
    registry_not_found("FleetMetric", metric_name)
}

/// How this service states that a `metricName` names no fleet metric this account holds
/// to a WRITE of one (`UpdateFleetMetric`): the name the request asked about and this
/// family's clause for a record that is not there, with the account NOT named — the
/// other half of [`missing_fleet_metric_message`].
pub fn fleet_metric_write_not_found_message(metric_name: &str) -> String {
    format!("FleetMetric {metric_name} doesn't exist")
}

/// How this service states that a `billingGroupName` names no billing group this account
/// holds — for a reader of one, a writer of one, and for a thing whose `billingGroupName`
/// named it.
///
/// A billing group is a REGISTRY resource of this account like a thing or a thing group, so
/// the miss is stated the way every registry family states one ([`registry_not_found`]): the
/// family noun as ONE word, the account the record would belong to, the name the request
/// asked about, and a terminating period.
pub fn missing_billing_group_message(billing_group_name: &str) -> String {
    registry_not_found("BillingGroup", billing_group_name)
}

/// How this service states that a `thingTypeName` names no thing type this account holds
/// — for a reader of one, for the deprecation of one, and for a thing whose
/// `thingTypeName` named it.
///
/// The family's noun is TWO capitalised words, unlike the thing-group family's single
/// one.
pub fn missing_thing_type_message(thing_type_name: &str) -> String {
    registry_not_found("Thing Type", thing_type_name)
}

/// How this service states that a `jobId` names no job: the jobs plane's own clause,
/// which every operation addressing a single job refuses a miss with.
pub fn missing_job_message(job_id: &str) -> String {
    plane_not_found("Job", job_id)
}

/// How this service states that a `jobTemplateId` names no job template this account
/// holds — for a reader of one and for a job created from one.
///
/// A job template is a resource of the JOBS plane, so the miss is stated the way that
/// plane states every other one ([`plane_not_found`]): the family noun, the id the request
/// asked about, and the clause ` cannot be found.` The noun is this family's own, in TWO
/// capitalised words — the same one its already-exists sentence carries
/// ([`super::existing_job_template_message`]), because a family has ONE noun whichever
/// sentence states it.
pub fn missing_job_template_message(job_template_id: &str) -> String {
    plane_not_found("Job Template", job_template_id)
}

/// How this service states that a `commandId` names no command: the commands plane's
/// clause, which is the jobs plane's — and NOT the registry families' account-qualified
/// sentence.
pub fn missing_command_message(command_id: &str) -> String {
    plane_not_found("Command", command_id)
}

/// How this service states that an `executionId` names no command execution of the device
/// the request addressed.
///
/// The same clause as the rest of the COMMANDS plane ([`plane_not_found`]), stated through
/// the SHARED sentence (`service_common::iot_commands::command_execution_not_found_message`)
/// because the device-facing half of this plane answers a missing execution too — a
/// device's report on an execution's reserved MQTT response topic — and one missing
/// execution must not be worded two ways.
pub fn missing_command_execution_message(execution_id: &str) -> String {
    ::service_common::iot_commands::command_execution_not_found_message(execution_id)
}

/// How this service refuses a read of a job execution whose JOB does not exist.
///
/// The two conditions are distinguishable: a `jobId` no creator recorded has no execution
/// anywhere, and the refusal names that missing JOB rather than the thing the request
/// asked about, so a caller is told which resource to create. A job that DOES exist but
/// has no execution on the thing is the other condition
/// ([`missing_job_execution_message`]).
pub fn missing_job_execution_job_message(job_id: &str) -> String {
    format!("Job execution cannot be found as the Job {job_id} does not exist.")
}

/// How this service refuses a read of a job execution of an EXISTING job that the thing
/// has none of — the thing is not one of the job's targets, or it is no registered thing
/// at all.
///
/// This one is about the PAIR, so it names both halves the request asked for, in the
/// order the sentence states them: the thing first, then the job it was looked for
/// within.
pub fn missing_job_execution_message(thing_name: &str, job_id: &str) -> String {
    format!("Job execution for thing {thing_name} cannot be found within job {job_id}.")
}

/// How this service states that a `taskId` names no thing-registration task.
///
/// FIXED text with no variable part: this family keeps the requested id out of the
/// refusal, so nothing of the request is echoed back.
pub const MISSING_THING_REGISTRATION_TASK_MESSAGE: &str = "Task doesn't exist";

/// How this service states that the account has no CA-certificate registration code for a
/// `DeleteRegistrationCode` to delete (see [`super::registration_code`]).
///
/// FIXED text with no variable part, like the bulk-registration family's sentence above:
/// this resource is the account's single registration code and the request names nothing,
/// so there is no id, name or member to interpolate.
pub const MISSING_REGISTRATION_CODE_MESSAGE: &str = "The registration code does not exist.";

/// How this service states that a `taskId` names no Device Defender audit task this
/// account holds — for a reader of one and for a cancel of one.
///
/// The AUDIT TASK sentence names the `TaskId` the request asked about: the fixed text is
/// `Audit task with TaskId=` and ` was not found`, and the only variable part is the id
/// the request sent, so the audit is either an audit this account started or no audit at
/// all.
pub fn missing_audit_task_message(task_id: &str) -> String {
    format!("Audit task with TaskId={task_id} was not found")
}

/// How this service refuses an `indexName` that is no fleet index of this service at all.
///
/// Not a not-found: a name outside the set of indexes the service recognises
/// ([`super::FLEET_INDEX_NAMES`]) is a request this service cannot act on, so it is
/// refused as `InvalidRequestException` (400) ahead of any lookup of the account's
/// configuration — a 404 is for a name that IS one of them and is not enabled. The fixed
/// text names the member it is about, and the variable part is the name the request sent.
pub fn unrecognized_index_name_message(index_name: &str) -> String {
    format!("Unrecognized indexName {index_name}")
}

/// How this service states that an `authorizerName` names no authorizer this account
/// holds — for a reader of one, a writer of one, and a delete of one.
pub fn missing_authorizer_message(authorizer_name: &str) -> String {
    format!("Authorizer {authorizer_name} not found")
}

/// How this service refuses an operation that addresses ONE topic rule by name.
///
/// Not a not-found either: `GetTopicRule`, `ReplaceTopicRule`, `EnableTopicRule` and
/// `DisableTopicRule` model no not-found error, because AWS authorizes each against the
/// rule's OWN ARN — a name that names no rule cannot be authorized — so a miss and a
/// denial are one indistinguishable `UnauthorizedException` (401). The sentence is that
/// family's: the fixed text and the single-quoted rule name the URI label carried, with
/// neither the action nor an ARN in it, and no statement that the rule is absent, so a
/// caller cannot use the reply to confirm that a rule exists.
pub fn unauthorized_topic_rule_message(rule_name: &str) -> String {
    format!("Access to topic rule '{rule_name}' was denied")
}
