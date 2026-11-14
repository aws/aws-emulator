// SPDX-License-Identifier: CC0-1.0

//! How this service states that a request named a resource it does not hold.
//!
//! AWS IoT has no single not-found sentence: each resource FAMILY has its own fixed
//! text, and the variable parts of it come from the request and from this account's
//! state — never from any one call's recording.
//!
//! * The REGISTRY families — a thing, a thing group, a thing type — qualify the name
//!   with the ACCOUNT the call was made for: `<Family> <accountId>:<name> not found.`
//!   ([`registry_not_found`]).
//! * The JOBS and COMMANDS planes word it `<Family> <id> cannot be found.`
//!   ([`plane_not_found`]).
//! * The AUTHORIZER family words it `Authorizer <name> not found`.
//! * The DEVICE DEFENDER family words it `Audit task <id> does not exist.`
//!   ([`missing_audit_task_message`]).
//! * The BULK THING REGISTRATION task family words it with a sentence that carries no
//!   variable part at all ([`MISSING_THING_REGISTRATION_TASK_MESSAGE`]): the task id is
//!   not interpolated.
//!
//! Every operation that addresses ONE resource of a family refuses a lookup miss with
//! that family's sentence, so a describe, a delete and an update of the same resource
//! cannot word the same miss three ways — which is the whole reason these sentences live
//! here rather than in each handler.

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

/// How this service states that a `thingName` names no thing in the registry.
pub fn missing_thing_message(thing_name: &str) -> String {
    registry_not_found("Thing", thing_name)
}

/// How this service states that a `thingGroupName` names no thing group this account
/// holds — for a reader of one, a writer of one, and for a job whose target named it.
pub fn missing_thing_group_message(thing_group_name: &str) -> String {
    registry_not_found("ThingGroup", thing_group_name)
}

/// How this service states that a `metricName` names no fleet metric this account
/// holds — for a reader of one and for a writer of one.
///
/// A fleet metric is a REGISTRY resource of this account like a thing or a thing
/// group, so the miss is stated the same way: the family noun, the account the record
/// would belong to and the name the request asked about.
pub fn missing_fleet_metric_message(metric_name: &str) -> String {
    registry_not_found("FleetMetric", metric_name)
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

/// How this service states that a `commandId` names no command: the commands plane's
/// clause, which is the jobs plane's — and NOT the registry families' account-qualified
/// sentence.
pub fn missing_command_message(command_id: &str) -> String {
    plane_not_found("Command", command_id)
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

/// How this service states that a `taskId` names no Device Defender audit task this
/// account holds — for a reader of one and for a cancel of one.
///
/// The DEVICE DEFENDER family states the miss of the resource the request named, so the
/// variable part is the id the request asked about and the rest is fixed: the audit is
/// either an audit this account started or no audit at all.
pub fn missing_audit_task_message(task_id: &str) -> String {
    format!("Audit task {task_id} does not exist.")
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
