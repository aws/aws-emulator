// SPDX-License-Identifier: CC0-1.0

//! The Run Command slice: a command a caller sent, and the one invocation it holds per
//! managed node the send resolved.
//!
//! `SendCommand` writes a [`CommandRecord`] here, `GetCommandInvocation` reports the
//! [`CommandInvocationRecord`] one of its nodes holds and `ListCommands` reports the
//! commands a Region holds, narrowed by the criteria a request names
//! ([`CommandRecord::matches_filter`]), so what a send persisted is what a later read
//! reports. `CancelCommand` is the other write of this slice: it stops the invocations of
//! one command that are still running ([`CommandRecord::cancel`]), which every read of the
//! command then reports. A command is keyed by `(region, command id)`: a command is sent to
//! the Systems Manager endpoint of one Region and identified there by the id this service
//! minted for it.
//!
//! "An invocation is a copy of a command sent to a specific managed node. … if a user
//! runs `SendCommand` against three managed nodes, then a command invocation is created
//! for each requested managed node ID" (`CommandInvocation`), so the invocations live ON
//! the command record, keyed by node id, rather than in a second store that could hold a
//! command's invocations after the command itself was dropped.
//!
//! Every invocation is minted `Pending` — "the command hasn't been sent to the managed
//! node" (`CommandInvocation$StatusDetails`) — and stays there: delivering a command is
//! the SSM Agent's own poll from the node, and this emulator runs no agent on any node,
//! so nothing picks an invocation up. Nothing therefore reports an exit code, output or
//! an execution window for a step that never ran; the command's counts are COUNTED from
//! the invocations the store holds ([`CommandRecord::completed_count`] and its siblings)
//! rather than asserted.
//!
//! The one write a read makes is the drop the clock already owes: Run Command retains a
//! command's history for [`COMMAND_HISTORY_RETENTION`], after which the command is gone
//! whether or not anybody called.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use crate::ops::types::{
    AlarmConfiguration, CloudWatchOutputConfig, Command, CommandInvocation, CommandPlugin,
    NotificationConfig, Target,
};
use super::ManagedNodeRecord;

/// The `CommandStatus` and `CommandInvocationStatus` value a command and its invocations
/// are minted with: "Pending: The command hasn't been sent to any managed nodes"
/// (`Command$StatusDetails`). From the modelled enums
/// (`crate::ops::types::COMMAND_STATUS_VALUES` and `COMMAND_INVOCATION_STATUS_VALUES`).
pub const COMMAND_STATUS_PENDING: &str = "Pending";

/// The `CommandInvocationStatus` values that are terminal, as `Command$StatusDetails`
/// lists them ("Success", "Delivery Timed Out", "Execution Timed Out", "Failed",
/// "Cancelled" — the last three spelled as the modelled enum spells them). What
/// [`CommandRecord::completed_count`] counts: "the number of targets for which the
/// command invocation reached a terminal state" (`Command$CompletedCount`).
pub const TERMINAL_INVOCATION_STATUSES: &[&str] = &["Success", "Cancelled", "Failed", "TimedOut"];

/// The `CommandInvocationStatus` values that count as errors: "the number of targets for
/// which the status is Failed or Execution Timed Out" (`Command$ErrorCount`).
pub const ERROR_INVOCATION_STATUSES: &[&str] = &["Failed", "TimedOut"];

/// The `StatusDetails` of an invocation the command never reached: "Delivery Timed Out:
/// The command wasn't delivered to the managed node before the delivery timeout expired"
/// (`CommandInvocation$StatusDetails`). What `Command$DeliveryTimedOutCount` counts.
pub const DELIVERY_TIMED_OUT_STATUS_DETAILS: &str = "Delivery Timed Out";

/// The `MaxConcurrency` a send that names none runs with: "The default value is 50"
/// (`SendCommandRequest$MaxConcurrency`).
pub const DEFAULT_MAX_CONCURRENCY: &str = "50";

/// The `MaxErrors` a send that names none runs with: "The default value is 0"
/// (`SendCommandRequest$MaxErrors`).
pub const DEFAULT_MAX_ERRORS: &str = "0";

/// The delivery timeout a send that names none runs with, in seconds: the Systems Manager
/// User Guide's "Understanding command timeout values" gives the `TimeoutSeconds` default
/// as 3,600 seconds, within the modelled 30..2592000 range.
pub const DEFAULT_TIMEOUT_SECONDS: i32 = 3600;

/// How long a command stays readable after it was requested: the Systems Manager User
/// Guide's "Run Command reports command history for 30 days" — after that the command and
/// its invocations are no longer reported.
pub const COMMAND_HISTORY_RETENTION: Duration = Duration::from_secs(30 * 24 * 60 * 60);

/// How many commands this store keeps at once, oldest evicted first.
///
/// The retention window above is measured in days, so it alone leaves the store growing
/// with one record per send for as long as the emulator runs. This is the emulator's own
/// guard — not an AWS quota — the same shape as the document store's total guard: past it
/// the oldest command's history is dropped early, so a caller sending commands in a loop
/// cannot grow this store without bound.
pub const MAX_COMMANDS_RETAINED: usize = 1_000;

/// The `ResponseCode` reported for an invocation that has run no step yet: a response code
/// is a step's own exit code, and an invocation nothing has started has none, which this
/// service reports as `-1` rather than as the zero that means "the step succeeded"
/// (`GetCommandInvocationResult$ResponseCode`).
pub const NO_RESPONSE_CODE: i32 = -1;

/// The `ResponseCode` of a step that SUCCEEDED: "the zero that means the step succeeded"
/// — the exit code a plugin that performed its work reports
/// (`GetCommandInvocationResult$ResponseCode`).
pub const SUCCESSFUL_RESPONSE_CODE: i32 = 0;

/// The target `Key` that names managed nodes by their node ids, and the value that names
/// every node of the Region: `Key=InstanceIds,Values=instance-id-1,…` and
/// `Key=InstanceIds,Values=*` (the `Target` doc's Run Command formats).
pub const TARGET_KEY_INSTANCE_IDS: &str = "InstanceIds";

/// The target `Key` that names every instance of the Region: `Key=AWS::EC2::Instance,
/// Values=*` (same doc).
pub const TARGET_KEY_EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// The target `Values` entry that selects every node the key can select (same doc).
pub const TARGET_VALUE_ALL: &str = "*";

/// The target `Key` that names a resource group by name: `Key=resource-groups:Name,
/// Values=resource-group-name` (same doc).
pub const TARGET_KEY_RESOURCE_GROUP_NAME: &str = "resource-groups:Name";

/// The same selection spelled the way the Automation and Change Manager formats spell it:
/// `Key=ResourceGroup,Values=resource-group-name` (same doc).
pub const TARGET_KEY_RESOURCE_GROUP: &str = "ResourceGroup";

/// The target keys a send may name, in the `Target` doc's own order: the node-id forms,
/// the tag forms ([`super::TAG_FILTER_KEY_PREFIX`] and [`super::TAG_KEY_FILTER_KEY`]) and
/// the resource-group forms.
pub fn supported_target_keys() -> Vec<&'static str> {
    vec![
        TARGET_KEY_INSTANCE_IDS,
        TARGET_KEY_EC2_INSTANCE,
        "tag:<tag-key>",
        super::TAG_KEY_FILTER_KEY,
        TARGET_KEY_RESOURCE_GROUP_NAME,
        TARGET_KEY_RESOURCE_GROUP,
    ]
}

/// How a send refuses a target whose `Key` is none of the forms it selects nodes by: the
/// sentence names the FORMS a caller may spell rather than the key it was handed, so it is
/// the same text whatever arrived (recorded behaviour — corpus `simparity-wire-aws-v1`,
/// fixtures `ssm-ecsm-run-command-on-an-unmanaged-instance-refusals` step `task#4` and
/// `ssm-vscan-traj-send-command-validation-order`).
pub const INVALID_TARGET_KEY: &str = "The target key is not valid. You can specify instance \
                                      IDs, or use tag:name=value, tag-key=value or \
                                      resource-groups:attribute=value.";

/// How a send refuses a request that names no node to run on at all: "`Targets` is required
/// if you don't provide one or more managed node IDs in the call" (`Command$Targets`), and
/// the sentence names the two ways a caller may say which nodes rather than anything the
/// request carried, so it is the same text whatever arrived (recorded behaviour — corpus
/// `simparity-wire-aws-v1`, fixture `ssm-vscan-traj-send-command-validation-order`).
pub const NO_NODES_NAMED_OR_DESCRIBED: &str =
    "You must specify instance IDs or a target value. Targets can include tag-key names, tag \
     key-pair values, or resource group names.";

/// One invocation of a command: the copy of it that one managed node holds.
#[derive(Debug, Clone)]
pub struct CommandInvocationRecord {
    /// The node this invocation was created for, and the key it is held under.
    pub instance_id: String,
    /// "The fully qualified host name of the managed node"
    /// (`CommandInvocation$InstanceName`), as the node itself reported it; `None` for a
    /// node that has reported none.
    pub instance_name: Option<String>,
    /// A `CommandInvocationStatus` value.
    pub status: String,
    /// The detailed status, in the spelling `CommandInvocation$StatusDetails` documents.
    pub status_details: String,
    /// What the agent in the node's guest ran for this invocation, and what the guest
    /// answered, for an invocation something has been delivered to; `None` for one that
    /// reached no guest to run in.
    pub outcome: Option<InvocationOutcome>,
    /// The script the node's agent has NOT PICKED UP yet, for an invocation whose node had no
    /// operating system up when the send reached it — a node this emulator holds no booted
    /// guest for ([`AwaitedDelivery`]). `None` for every other invocation: one already
    /// delivered, and one there is nothing to deliver to the node at all.
    pub awaiting: Option<AwaitedDelivery>,
    /// When the cancel a caller asked for this invocation settles, for one `CancelCommand`
    /// reached while it was still running; `None` for an invocation nobody cancelled.
    ///
    /// A cancelled invocation reports `Cancelling` until that instant and `Cancelled`
    /// after it, and nothing a step did moves it on — "the command was terminated before
    /// it was completed" ([`CommandInvocationRecord::materialize_due`]).
    pub cancel_settles_at: Option<SystemTime>,
}

/// The script one node's agent has not picked up yet, kept so that the agent runs THAT script
/// when its guest comes up.
///
/// A send performs its document's script in each node's guest as it goes
/// ([`crate::ops::send_command`]), which is the agent in that guest taking the command. An
/// agent whose guest is not up has taken nothing: "Pending: The command hasn't been sent to
/// the managed node" is the status that invocation stays in (`CommandInvocation$StatusDetails`),
/// and the delivery is owed to the clock — the guest comes up at the instant Amazon EC2 owes
/// its boot, and the agent in it then takes the command whether or not anybody calls in
/// between ([`super::command_deliveries::settle_due_deliveries`] is what materialises it).
///
/// What is kept is what that delivery needs and nothing more: the step whose plugin runs and
/// the lines it delivers, both resolved when the send was made, so a document or a parameter
/// that changed afterwards cannot change what the agent was sent.
#[derive(Debug, Clone)]
pub struct AwaitedDelivery {
    /// The step of the document whose plugin the agent runs — what `GetCommandInvocation`'s
    /// `PluginName` then names.
    pub plugin_name: String,
    /// The lines that step delivers to the guest, as ONE script.
    pub commands: Vec<String>,
    /// When the invocation's own move off `Pending` was owed when the send was made
    /// ([`InvocationOutcome::due_at`]): a delivery that happens later does not bring that
    /// instant forward, and one that happens after it has passed leaves the invocation
    /// finished as soon as the run is in.
    pub due_at: SystemTime,
}

/// What the agent on one node ran for one invocation, and what the guest answered.
///
/// A delivery is PERFORMED when the command is sent — the agent is in the guest and the
/// guest is this emulator's own ([`super::guest_execution`]) — and the invocation's move off
/// `Pending` is what the clock then owes: [`InvocationOutcome::due_at`] is when the run has
/// finished as far as a caller reading it is concerned, so a client that reads straight
/// after the send still sees `Pending`, the status Run Command answers a send with.
#[derive(Debug, Clone)]
pub struct InvocationOutcome {
    /// The step of the document whose plugin ran — what `GetCommandInvocation`'s
    /// `PluginName` names ([`CommandRecord::step_names`] holds every step the document
    /// declared).
    pub plugin_name: String,
    /// The lines the guest wrote to its standard output, in the order it wrote them.
    pub output: Vec<String>,
    /// The lines the guest wrote to its standard ERROR, in the order it wrote them — what the
    /// script reported it could not do, which is what `StandardErrorContent` carries beside
    /// the exit code below.
    pub errors: Vec<String>,
    /// "The exit code of the plugin script" (`GetCommandInvocationResult$ResponseCode`):
    /// the status the guest's own shell answered with.
    pub response_code: i32,
    /// When the invocation's move off `Pending` falls due.
    pub due_at: SystemTime,
    /// Whether a guest took the command at all. `false` for a node nothing could deliver
    /// to — one whose instance this emulator holds no running, booted guest for — whose
    /// invocation times out at the send's own delivery timeout instead of reporting an exit
    /// code no plugin produced.
    pub delivered: bool,
}

impl InvocationOutcome {
    /// The `CommandInvocationStatus` this outcome leaves the invocation in: "Success: The
    /// command … was received by the SSM Agent and … returned an exit code of zero",
    /// against "Failed: The command … was not successful on the managed node. For a plugin,
    /// this indicates that the result code was not zero" (`CommandInvocation$StatusDetails`)
    /// — and "Delivery Timed Out: The command wasn't delivered to the managed node before
    /// the delivery timeout expired" for one no guest took.
    pub fn status(&self) -> &'static str {
        if !self.delivered {
            INVOCATION_STATUS_TIMED_OUT
        } else if self.response_code == 0 {
            INVOCATION_STATUS_SUCCESS
        } else {
            INVOCATION_STATUS_FAILED
        }
    }

    /// The detailed status, in the spelling `CommandInvocation$StatusDetails` documents.
    pub fn status_details(&self) -> &'static str {
        if self.delivered {
            self.status()
        } else {
            DELIVERY_TIMED_OUT_STATUS_DETAILS
        }
    }

    /// The whole of what the guest wrote, as `StandardOutputContent` carries it: the lines
    /// in order, each ended by a newline the way a terminal writes them.
    ///
    /// "The first 24,000 characters written by the plugin to `stdout`"
    /// (`GetCommandInvocationResult$StandardOutputContent`, whose modelled length is
    /// `0..24000`), so a script that wrote more is reported by its first characters.
    pub fn standard_output(&self) -> String {
        cut_to(&self.lines(&self.output), MAX_STANDARD_OUTPUT_BYTES)
    }

    /// The whole of what the guest reported it could not do, as `StandardErrorContent`
    /// carries it: "the first 8,000 characters written by the plugin to `stderr`"
    /// (`GetCommandInvocationResult$StandardErrorContent`, whose modelled length is
    /// `0..8000`).
    pub fn standard_error(&self) -> String {
        cut_to(&self.lines(&self.errors), MAX_STANDARD_ERROR_BYTES)
    }

    /// One of the two streams as the member carrying it reads: the lines in order, each ended
    /// by a newline the way a terminal writes them.
    fn lines(&self, stream: &[String]) -> String {
        stream
            .iter()
            .map(|line| format!("{line}\n"))
            .collect()
    }
}

/// The modelled lengths of the two members a finished invocation reports its streams in:
/// `GetCommandInvocationResult$StandardOutputContent` (`len 0..24000`) and
/// `$StandardErrorContent` (`len 0..8000`).
const MAX_STANDARD_OUTPUT_BYTES: usize = 24_000;

/// See [`MAX_STANDARD_OUTPUT_BYTES`].
const MAX_STANDARD_ERROR_BYTES: usize = 8_000;

/// `written` cut to `ceiling` bytes, on a character boundary so what is reported is still
/// text — the "first N characters" each of those members carries.
fn cut_to(written: &str, ceiling: usize) -> String {
    if written.len() <= ceiling {
        return written.to_string();
    }
    let mut cut = ceiling;
    while cut > 0 && !written.is_char_boundary(cut) {
        cut -= 1;
    }
    written[..cut].to_string()
}

/// `at` as the ISO-8601 instant a command invocation's execution times are written in:
/// "date and time are written in ISO-8601 format"
/// (`GetCommandInvocationResult$ExecutionStartDateTime`), rendered by the one timestamp
/// writer this tree formats a modelled instant with.
pub fn iso8601(at: SystemTime) -> String {
    wire_instant(at)
        .fmt(::aws_smithy_types::date_time::Format::DateTime)
        .unwrap_or_default()
}

/// How long the invocation took, as an ISO-8601 duration: the span between the instant the
/// agent took the command and the instant its run was owed to have finished
/// (`GetCommandInvocationResult$ExecutionElapsedTime`).
pub fn iso8601_elapsed(from: SystemTime, to: SystemTime) -> String {
    let seconds = to
        .duration_since(from)
        .unwrap_or_default()
        .as_secs();
    format!("PT{seconds}S")
}

/// `at` as the modelled timestamp this service reports it in: the instant TRUNCATED to the
/// millisecond it falls in ([`super::NANOS_PER_MILLISECOND`]).
///
/// The crate's one timestamp writer ([`super::wire_instant`]), reachable under this path too
/// because this tree's projections reach it here.
pub use super::wire_instant;

/// The `NotificationConfig` a command reports, from the one its send configured.
///
/// Every member is reported, because the structure is part of the command's configuration
/// and a command reports its whole configuration ([`CommandRecord::to_command`]): a member
/// the send left unset names no topic, asks for no event and asks for no notification
/// type, which is the empty string, the empty list and the empty string respectively.
fn reported_notification_config(configured: Option<&NotificationConfig>) -> NotificationConfig {
    NotificationConfig {
        notification_arn: Some(
            configured
                .and_then(|held| held.notification_arn.clone())
                .unwrap_or_default(),
        ),
        notification_events: Some(
            configured
                .and_then(|held| held.notification_events.clone())
                .unwrap_or_default(),
        ),
        notification_type: Some(
            configured
                .and_then(|held| held.notification_type.clone())
                .unwrap_or_default(),
        ),
    }
}

/// The `CloudWatchOutputConfig` a command reports, from the one its send configured — read
/// the same way, so a send that named no log group reports none by name and output not
/// enabled (`CloudWatchOutputConfig$CloudWatchOutputEnabled` is `@default` false).
fn reported_cloud_watch_output_config(
    configured: Option<&CloudWatchOutputConfig>,
) -> CloudWatchOutputConfig {
    CloudWatchOutputConfig {
        cloud_watch_log_group_name: Some(
            configured
                .and_then(|held| held.cloud_watch_log_group_name.clone())
                .unwrap_or_default(),
        ),
        cloud_watch_output_enabled: configured
            .is_some_and(|held| held.cloud_watch_output_enabled),
    }
}

/// The `CommandInvocationStatus` of an invocation whose plugin exited zero (from the
/// modelled enum, `crate::ops::types::COMMAND_INVOCATION_STATUS_VALUES`).
pub const INVOCATION_STATUS_SUCCESS: &str = "Success";

/// The `CommandInvocationStatus` of one whose plugin did not (same enum).
pub const INVOCATION_STATUS_FAILED: &str = "Failed";

/// The `CommandInvocationStatus` of one the command never reached (same enum), whose
/// detailed status is [`DELIVERY_TIMED_OUT_STATUS_DETAILS`].
pub const INVOCATION_STATUS_TIMED_OUT: &str = "TimedOut";

/// The `CommandInvocationStatus` a cancel is being carried out under: "Cancelling" from
/// the modelled enum (`crate::ops::types::COMMAND_INVOCATION_STATUS_VALUES`), which a
/// command carries too (`COMMAND_STATUS_VALUES`) — the cancel has been accepted and the
/// work it stops has not stopped yet, so it is not a terminal status.
pub const STATUS_CANCELLING: &str = "Cancelling";

/// The status a settled cancel leaves behind: "Cancelled: The command was terminated
/// before it was completed. This is a terminal state" (`Command$StatusDetails` and
/// `CommandInvocation$StatusDetails`), one of [`TERMINAL_INVOCATION_STATUSES`].
pub const STATUS_CANCELLED: &str = "Cancelled";

/// How long after a `CancelCommand` the cancel it asked for has stopped the work that was
/// still running.
///
/// CHOSEN, the same span this emulator gives every transition the clock owes
/// ([`INVOCATION_COMPLETES_AFTER`]): a caller reading straight after the cancel still sees
/// the `Cancelling` the service accepted it as, and a client polling for a terminal status
/// converges well inside any budget. A cancel with nothing left to stop — a command no
/// invocation of is still running — settles at once instead, because there is no work in
/// flight for it to wait on.
pub const CANCEL_SETTLES_AFTER: Duration = INVOCATION_COMPLETES_AFTER;

/// The `CommandStatus` of a command whose invocations have not all finished: "InProgress:
/// The command has been sent to at least one managed node but hasn't reached a final state
/// on all managed nodes" (`Command$StatusDetails`).
pub const COMMAND_STATUS_IN_PROGRESS: &str = "InProgress";

/// How long after a send the invocation it created has finished running.
///
/// CHOSEN: the span this emulator gives a transition the clock owes. The span is never
/// zero, so a caller reading an invocation straight after `SendCommand` sees the `Pending`
/// the send itself reported, and a client polling for a terminal status converges well
/// inside any budget.
pub const INVOCATION_COMPLETES_AFTER: Duration = Duration::from_secs(3);

/// How many guests ONE send performs its document's script in.
///
/// The emulator's own guard, NOT an AWS quota: a send selects its nodes by criteria the
/// caller writes (`Key=InstanceIds,Values=*` selects every node of the Region), and each
/// delivery performs a caller-supplied script in a guest — so the work ONE request can make
/// this process do is the PRODUCT of the nodes it selects and the bound on one run
/// (`ComAmazonawsEc2Amazonec2State::run_in_instance_guest`). This caps the first factor,
/// which is what keeps that product small enough to be beside the point on a developer's
/// machine. A node past it is reported as one the command was not delivered to, the same
/// way a node with no guest to run in is, rather than as one whose script ran.
pub const MAX_GUEST_DELIVERIES_PER_SEND: usize = 64;

impl CommandInvocationRecord {
    /// The invocation a send creates for `node`: nothing has been delivered to the node
    /// yet, so it is `Pending`.
    pub fn pending(instance_id: impl Into<String>, instance_name: Option<String>) -> Self {
        Self {
            instance_id: instance_id.into(),
            instance_name,
            status: COMMAND_STATUS_PENDING.to_string(),
            status_details: COMMAND_STATUS_PENDING.to_string(),
            outcome: None,
            awaiting: None,
            cancel_settles_at: None,
        }
    }

    /// Keep `awaited` as the script this invocation's node has not picked up yet, and answer
    /// the invocation to its caller's hand.
    ///
    /// Recorded beside the placeholder outcome a send leaves for a node nothing reached, so an
    /// invocation whose guest never comes up still times out at the send's own delivery
    /// timeout; the awaited script is what a later delivery runs
    /// ([`super::command_deliveries::settle_due_deliveries`]).
    pub fn awaits_delivery(&mut self, awaited: AwaitedDelivery) {
        self.awaiting = Some(awaited);
    }

    /// Cancel this invocation at `now`, with the cancel settling at
    /// `now + CANCEL_SETTLES_AFTER`; answers whether there was anything to cancel.
    ///
    /// "Attempts to cancel the command specified by the Command ID" (`CancelCommand`): an
    /// invocation that has already reached a terminal status has finished, so a cancel of it
    /// stops nothing and leaves the status it reached — only one still running is cancelled.
    pub fn cancel(&mut self, now: SystemTime) -> bool {
        if is_terminal_status(&self.status) {
            return false;
        }
        self.cancel_settles_at = Some(now + CANCEL_SETTLES_AFTER);
        true
    }

    /// Record what the agent ran for this invocation, with its move off `Pending` owed at
    /// `due_at`.
    ///
    /// A delivery is what the agent in the guest TOOK, so recording one leaves nothing
    /// awaiting: a node whose agent has run the script is not one that has still to pick it
    /// up.
    pub fn delivered(&mut self, outcome: InvocationOutcome) {
        if outcome.delivered {
            self.awaiting = None;
        }
        self.outcome = Some(outcome);
    }

    /// Materialise the move the clock owes this invocation at `now`: an invocation whose
    /// run had finished by then carries the status the plugin's exit code gives it.
    pub fn materialize_due(&mut self, now: SystemTime) {
        // A cancelled invocation was terminated before it was completed, so what a step did
        // no longer decides its status: it is being cancelled until the cancel settles and
        // cancelled after that, whatever the agent had been asked to run.
        if let Some(settles_at) = self.cancel_settles_at {
            let status = if now < settles_at {
                STATUS_CANCELLING
            } else {
                STATUS_CANCELLED
            };
            self.status = status.to_string();
            self.status_details = status.to_string();
            return;
        }
        let Some(outcome) = self.outcome.as_ref().filter(|held| now >= held.due_at) else {
            return;
        };
        self.status = outcome.status().to_string();
        self.status_details = outcome.status_details().to_string();
    }

    /// What the node's agent RAN for this invocation, once the clock has made its move off
    /// `Pending` due and a guest really took it — and `None` otherwise, because an
    /// invocation nothing has finished has no exit code and no output a plugin produced.
    /// A CANCELLED invocation reports none either: its run was terminated before it was
    /// completed, so there is no finished step to report an exit code or output for.
    pub fn ran(&self) -> Option<&InvocationOutcome> {
        if self.cancel_settles_at.is_some() {
            return None;
        }
        let finished = TERMINAL_INVOCATION_STATUSES.contains(&self.status.as_str());
        self.outcome
            .as_ref()
            .filter(|outcome| finished && outcome.delivered)
    }

    /// Whether a `Status` filter value selects this invocation.
    ///
    /// An invocation carries the two readings of its status a command does — the modelled
    /// `CommandInvocationStatus` and the detailed status `CommandInvocation$StatusDetails`
    /// spells in words ("Delivery Timed Out") — and the documented filter values carry no
    /// spaces, so a value selects it when it names either.
    pub fn matches_status_filter(&self, value: &str) -> bool {
        self.status == value || self.status_details.replace(' ', "") == value
    }
}

/// A command as this store holds it: the request as Systems Manager received it, and the
/// invocations it created.
#[derive(Debug, Clone)]
pub struct CommandRecord {
    /// "A unique identifier for this command" (`Command$CommandId`), and part of the
    /// store key.
    pub command_id: String,
    /// The Region the command was sent to; the other part of its store key.
    pub region: String,
    /// The document the command runs, by the name the document is held under — never the
    /// ARN spelling a request may have named it by, since `Command$DocumentName` is a
    /// document NAME.
    pub document_name: String,
    /// The `DocumentVersion` the send resolved.
    pub document_version: String,
    /// The names of the document's steps, as the content declared them when the command
    /// was sent: what `GetCommandInvocation`'s `PluginName` names one of.
    pub step_names: Vec<String>,
    pub comment: Option<String>,
    /// The parameter values the send supplied, ordered so two reads agree.
    pub parameters: BTreeMap<String, Vec<String>>,
    /// The node ids the send named, exactly as it named them.
    pub instance_ids: Vec<String>,
    /// The search criteria the send named.
    pub targets: Vec<Target>,
    pub output_s3_bucket_name: Option<String>,
    pub output_s3_key_prefix: Option<String>,
    pub max_concurrency: String,
    pub max_errors: String,
    /// The delivery timeout the send ran with, in seconds.
    pub timeout_seconds: i32,
    pub service_role: Option<String>,
    pub notification_config: Option<NotificationConfig>,
    pub cloud_watch_output_config: Option<CloudWatchOutputConfig>,
    pub alarm_configuration: Option<AlarmConfiguration>,
    /// When the command was requested (`Command$RequestedDateTime`).
    pub requested_at: SystemTime,
    /// A `CommandStatus` value.
    pub status: String,
    /// The detailed status, in the spelling `Command$StatusDetails` documents.
    pub status_details: String,
    /// One invocation per node the send resolved, keyed by node id so a read of one
    /// node's invocation is a lookup and a listing walks a stable order.
    pub invocations: BTreeMap<String, CommandInvocationRecord>,
    /// When the cancel a caller asked for this COMMAND settles, for a `CancelCommand` that
    /// cancelled it on every node it was requested on; `None` for a command nobody
    /// cancelled and for one cancelled on some of its nodes only, whose status is still
    /// read off the invocations it holds ([`Self::materialize_due`]).
    pub cancel_settles_at: Option<SystemTime>,
}

impl CommandRecord {
    /// When the command expires: "`ExpiresAfter` is calculated based on the total timeout
    /// for the overall command" (`Command$ExpiresAfter`), which is the delivery timeout
    /// the send ran with, measured from the request.
    pub fn expires_after(&self) -> SystemTime {
        self.requested_at + Duration::from_secs(self.timeout_seconds.max(0) as u64)
    }

    /// When this command stops being reported ([`COMMAND_HISTORY_RETENTION`]).
    pub fn retained_until(&self) -> SystemTime {
        self.requested_at + COMMAND_HISTORY_RETENTION
    }

    /// "The number of targets for the command" (`Command$TargetCount`): the nodes the
    /// send resolved, which is what it created an invocation for.
    pub fn target_count(&self) -> i32 {
        i32::try_from(self.invocations.len()).unwrap_or(i32::MAX)
    }

    /// "The number of targets for which the command invocation reached a terminal state"
    /// (`Command$CompletedCount`).
    pub fn completed_count(&self) -> i32 {
        self.count_invocations(|invocation| {
            TERMINAL_INVOCATION_STATUSES.contains(&invocation.status.as_str())
        })
    }

    /// "The number of targets for which the status is Failed or Execution Timed Out"
    /// (`Command$ErrorCount`).
    pub fn error_count(&self) -> i32 {
        self.count_invocations(|invocation| {
            ERROR_INVOCATION_STATUSES.contains(&invocation.status.as_str())
        })
    }

    /// "The number of targets for which the status is Delivery Timed Out"
    /// (`Command$DeliveryTimedOutCount`).
    pub fn delivery_timed_out_count(&self) -> i32 {
        self.count_invocations(|invocation| {
            invocation.status_details == DELIVERY_TIMED_OUT_STATUS_DETAILS
        })
    }

    fn count_invocations(&self, mut matches: impl FnMut(&CommandInvocationRecord) -> bool) -> i32 {
        i32::try_from(
            self.invocations
                .values()
                .filter(|invocation| matches(invocation))
                .count(),
        )
        .unwrap_or(i32::MAX)
    }

    /// The invocation `instance_id` holds of this command, or `None` when the command
    /// created none for that node.
    pub fn invocation(&self, instance_id: &str) -> Option<&CommandInvocationRecord> {
        self.invocations.get(instance_id)
    }

    /// Materialise the moves the clock owes this command at `now`: every invocation whose
    /// run had finished by then, and the command's own status, which is what its
    /// invocations have reached.
    ///
    /// "Pending: The command hasn't been sent to any managed nodes"; "InProgress: The
    /// command has been sent to at least one managed node but hasn't reached a final state
    /// on all managed nodes"; "Success: The command successfully ran on all invocations";
    /// "Failed: The value of MaxErrors or more command invocations shows a status of Failed";
    /// "Cancelled: The command was terminated before it was completed"
    /// (`Command$StatusDetails`) — so a command is read off its invocations rather than
    /// recorded a second time.
    pub fn materialize_due(&mut self, now: SystemTime) {
        for invocation in self.invocations.values_mut() {
            invocation.materialize_due(now);
        }
        // A command a caller cancelled on every node it was requested on reports the cancel
        // itself: it is being cancelled until the cancel settles and cancelled after that,
        // whatever its invocations would otherwise have reached.
        if let Some(settles_at) = self.cancel_settles_at {
            let status = if now < settles_at {
                STATUS_CANCELLING
            } else {
                STATUS_CANCELLED
            };
            self.status = status.to_string();
            self.status_details = status.to_string();
            return;
        }
        // Only a command something DELIVERED or CANCELLED is read off its invocations: a
        // record whose invocations nothing has run and nobody has cancelled carries the
        // status whatever wrote it recorded, and this is not the place to overwrite it.
        if !self
            .invocations
            .values()
            .any(|invocation| {
                invocation.cancel_settles_at.is_some()
                    || invocation
                        .outcome
                        .as_ref()
                        .is_some_and(|outcome| outcome.delivered)
            })
        {
            return;
        }
        let terminal = self.completed_count();
        let failed = self.error_count();
        let cancelling = self.count_invocations(|invocation| invocation.status == STATUS_CANCELLING);
        let cancelled = self.count_invocations(|invocation| invocation.status == STATUS_CANCELLED);
        let status = if cancelling > 0 {
            // The cancel of some of this command's nodes is still being carried out, which
            // is what the command is doing.
            STATUS_CANCELLING
        } else if terminal < self.target_count() {
            if terminal == 0 {
                COMMAND_STATUS_PENDING
            } else {
                COMMAND_STATUS_IN_PROGRESS
            }
        } else if failed > 0 {
            INVOCATION_STATUS_FAILED
        } else if cancelled > 0 {
            // Every invocation has finished and one of them was terminated before it
            // completed, so the command did not run everywhere it was sent.
            STATUS_CANCELLED
        } else {
            INVOCATION_STATUS_SUCCESS
        };
        self.status = status.to_string();
        self.status_details = status.to_string();
    }

    /// Cancel this command at `now`, on the nodes `named` — or on every node it was
    /// requested on when `named` is empty — and materialise what that cancel makes due.
    ///
    /// "Attempts to cancel the command specified by the Command ID"; "(Optional) A list of
    /// managed node IDs on which you want to cancel the command. If not provided, the
    /// command is canceled on every node on which it was requested" (`CancelCommand`). What
    /// a cancel stops is work that is still running, so an invocation that has already
    /// reached a terminal status keeps the status it reached, and a command that has already
    /// reached one is not cancelled at all. A cancel that covers every node the command was
    /// requested on is the command's own outcome; one that covers some of them leaves the
    /// command's status to be read off its invocations.
    pub fn cancel(&mut self, named: &[String], now: SystemTime) {
        if is_terminal_status(&self.status) {
            return;
        }
        let selected: Vec<String> = if named.is_empty() {
            self.invocations.keys().cloned().collect()
        } else {
            named.to_vec()
        };
        let mut running = false;
        for id in &selected {
            if let Some(invocation) = self.invocations.get_mut(id) {
                running |= invocation.cancel(now);
            }
        }
        if self
            .invocations
            .keys()
            .all(|id| selected.iter().any(|named| named == id))
        {
            // A cancel waits only on work that is in flight: a command no invocation of is
            // still running — one with no invocation at all, or none left to stop — has
            // nothing for the cancel to reach, so it settles at once.
            self.cancel_settles_at = Some(if running {
                now + CANCEL_SETTLES_AFTER
            } else {
                now
            });
        }
        self.materialize_due(now);
    }

    /// Whether a `Status` filter value selects this command.
    ///
    /// A command carries two readings of its status: the modelled `CommandStatus`, and the
    /// detailed status `Command$StatusDetails` spells in words ("Delivery Timed Out"). The
    /// documented filter values cover both readings and carry no spaces
    /// ([`LIST_COMMANDS_STATUS_FILTER_VALUES`]), so a value selects this command when it
    /// names either of them.
    pub fn matches_status_filter(&self, value: &str) -> bool {
        self.status == value || self.status_details.replace(' ', "") == value
    }

    /// Whether the filter `key`=`value` selects this command.
    ///
    /// Each key narrows by one property of the command: when it was requested, the status
    /// it holds, whether that status is terminal, or the document it runs. A key this
    /// service does not know selects nothing — its caller refuses such a key before
    /// anything is asked about the history.
    pub fn matches_filter(&self, key: &str, value: &str) -> bool {
        match key {
            COMMAND_FILTER_KEY_INVOKED_AFTER => {
                parse_filter_instant(value).is_some_and(|at| self.requested_at >= at)
            }
            COMMAND_FILTER_KEY_INVOKED_BEFORE => {
                parse_filter_instant(value).is_some_and(|at| self.requested_at <= at)
            }
            COMMAND_FILTER_KEY_STATUS => self.matches_status_filter(value),
            // "Executing … currently still running" is every status that is not terminal;
            // "Complete … already completed" is every status that is.
            COMMAND_FILTER_KEY_EXECUTION_STAGE => match value {
                EXECUTION_STAGE_EXECUTING => !is_terminal_status(&self.status),
                EXECUTION_STAGE_COMPLETE => is_terminal_status(&self.status),
                _ => false,
            },
            COMMAND_FILTER_KEY_DOCUMENT_NAME => self.document_name == value,
            _ => false,
        }
    }

    /// The modelled `Command` for this command: every member either echoed from the
    /// request as it was received, resolved by the send, or counted from the invocations
    /// the store holds.
    ///
    /// The projection is COMPLETE: Run Command reports a command's whole configuration on
    /// every read of it, so a member the request left unset is reported as what that member
    /// holds for a command nobody configured — the empty string for a scalar, the empty
    /// list for a list, and the structure with its own members at their defaults for a
    /// configuration structure — rather than omitted. That is what lets a client read the
    /// echo to confirm what the command was accepted with instead of having to tell "the
    /// service did not report it" from "the service dropped it".
    ///
    /// The one member whose stored form decides whether it is reported is `Parameters`: a
    /// send that supplied no parameter value supplied no map, and nothing in this service
    /// holds an entry to report for it.
    pub fn to_command(&self) -> Command {
        Command {
            command_id: Some(self.command_id.clone()),
            document_name: Some(self.document_name.clone()),
            document_version: Some(self.document_version.clone()),
            comment: Some(self.reported_comment()),
            expires_after: Some(wire_instant(self.expires_after())),
            parameters: (!self.parameters.is_empty()).then(|| {
                self.parameters
                    .iter()
                    .map(|(name, values)| (name.clone(), values.clone()))
                    .collect()
            }),
            instance_ids: Some(self.instance_ids.clone()),
            targets: Some(self.targets.clone()),
            requested_date_time: Some(wire_instant(self.requested_at)),
            status: Some(self.status.clone()),
            status_details: Some(self.status_details.clone()),
            // "(Deprecated) You can no longer specify this parameter. The system ignores
            // it. Instead, Systems Manager automatically determines the Amazon Web Services
            // Region of the S3 bucket" (`Command$OutputS3Region`) — so what a request
            // carried is ignored, and what is REPORTED is the Region Systems Manager
            // determined, which is the Region the command was sent to.
            output_s3_region: Some(self.region.clone()),
            output_s3_bucket_name: Some(self.output_s3_bucket_name.clone().unwrap_or_default()),
            output_s3_key_prefix: Some(self.output_s3_key_prefix.clone().unwrap_or_default()),
            max_concurrency: Some(self.max_concurrency.clone()),
            max_errors: Some(self.max_errors.clone()),
            target_count: self.target_count(),
            completed_count: self.completed_count(),
            error_count: self.error_count(),
            delivery_timed_out_count: self.delivery_timed_out_count(),
            service_role: Some(self.service_role.clone().unwrap_or_default()),
            notification_config: Some(reported_notification_config(
                self.notification_config.as_ref(),
            )),
            cloud_watch_output_config: Some(self.reported_cloud_watch_output_config()),
            timeout_seconds: Some(self.timeout_seconds),
            // `AlarmConfiguration`'s own members carry the defaults the model declares for
            // them, so the structure a send configured no alarm with is the defaulted one:
            // no alarm named, and a poll failure not ignored.
            alarm_configuration: Some(self.alarm_configuration.clone().unwrap_or_default()),
            // "The CloudWatch alarm that was invoked by the command"
            // (`Command$TriggeredAlarms`): no alarm has been evaluated for a command
            // nothing has delivered, so none was invoked.
            triggered_alarms: Some(Vec::new()),
            // The members beyond the published model. None of them is settable through this
            // API — `SendCommandRequest` carries no client name, no client source id, no
            // command signature and no interactive flag — so a command sent through it is
            // reported as what it is: issued by no named client, carrying neither
            // signature, and not interactive.
            client_name: Some(String::new()),
            client_source_id: Some(String::new()),
            has_send_command_signature: Some(false),
            has_cancel_command_signature: Some(false),
            interactive: Some(false),
        }
    }

    /// The `Comment` every read of this command reports: "the comment text for the command",
    /// which is the empty string for a send that carried none — a read reports the command's
    /// WHOLE configuration ([`Self::to_command`] carries the rule and the reason).
    ///
    /// Read here by every projection of a command, so the command reads
    /// ([`Self::to_command`]), the invocation reads ([`Self::to_command_invocation`]) and
    /// [`crate::ops::get_command_invocation`] cannot disagree about what a command with no
    /// comment reports.
    pub fn reported_comment(&self) -> String {
        self.comment.clone().unwrap_or_default()
    }

    /// The `CloudWatchOutputConfig` every read of this command reports, read the same way: the
    /// structure with its own members at their defaults for a send that configured no CloudWatch
    /// output — no log group named, and output not enabled.
    pub fn reported_cloud_watch_output_config(&self) -> CloudWatchOutputConfig {
        reported_cloud_watch_output_config(self.cloud_watch_output_config.as_ref())
    }

    /// Whether the filter `key`=`value` selects ONE invocation of this command.
    ///
    /// Every key but the status narrows by a property of the COMMAND — when it was
    /// requested and which document it runs are facts of the command, shared by each of its
    /// invocations — so those are read through [`Self::matches_filter`]. The status is the
    /// INVOCATION's: "ListCommandInvocations provide status about command execution", and an
    /// invocation of a command reaches its own status, which is what a caller narrowing a
    /// listing of invocations by status is asking about.
    pub fn matches_invocation_filter(
        &self,
        invocation: &CommandInvocationRecord,
        key: &str,
        value: &str,
    ) -> bool {
        match key {
            COMMAND_FILTER_KEY_STATUS => invocation.matches_status_filter(value),
            other => self.matches_filter(other, value),
        }
    }

    /// The modelled `CommandInvocation` for one node's invocation of this command — the
    /// command's own members plus that invocation's status.
    ///
    /// The projection is COMPLETE, for the reason [`Self::to_command`] states: the members an
    /// invocation carries OF THE COMMAND are reported whether or not the send configured them,
    /// so a send that carried no comment, named no service role and asked for neither
    /// notifications nor CloudWatch output is reported with the empty string and with each
    /// configuration structure at its own defaults rather than with those members omitted.
    ///
    /// `details` is what "If set this returns the response of the command executions and any
    /// command output" (`ListCommandInvocationsRequest$Details`) asks for: the per-step
    /// results of the invocation, which are reported only for a step that has RUN
    /// ([`CommandInvocationRecord::ran`]) — an invocation nothing has finished has no exit
    /// code or output a plugin produced.
    pub fn to_command_invocation(
        &self,
        invocation: &CommandInvocationRecord,
        details: bool,
    ) -> CommandInvocation {
        let ran = invocation.ran();
        CommandInvocation {
            command_id: Some(self.command_id.clone()),
            instance_id: Some(invocation.instance_id.clone()),
            instance_name: invocation.instance_name.clone(),
            comment: Some(self.reported_comment()),
            document_name: Some(self.document_name.clone()),
            document_version: Some(self.document_version.clone()),
            requested_date_time: Some(wire_instant(self.requested_at)),
            status: Some(invocation.status.clone()),
            status_details: Some(invocation.status_details.clone()),
            // "Gets the trace output sent by the agent" (`CommandInvocation$TraceOutput`):
            // the agent in the guest reports the one stream the session has — what the
            // program wrote — so there is no second, diagnostic stream to report, and the
            // URLs name files a plugin wrote to Amazon S3, of which there are none.
            trace_output: None,
            standard_output_url: Some(String::new()),
            standard_error_url: Some(String::new()),
            // The per-step results, which are only asked for.
            command_plugins: details
                .then(|| {
                    ran.map(|ran| {
                        vec![self.to_command_plugin(ran)]
                    })
                })
                .flatten(),
            service_role: Some(self.service_role.clone().unwrap_or_default()),
            notification_config: Some(reported_notification_config(
                self.notification_config.as_ref(),
            )),
            cloud_watch_output_config: Some(self.reported_cloud_watch_output_config()),
        }
    }

    /// The modelled `CommandPlugin` for the step that RAN on one node: "describes plugin
    /// details" — the step's name, the status it left the invocation in, the exit code it
    /// answered, when it ran and what it wrote.
    ///
    /// The one plugin an invocation of this emulator reports is the step the node's agent
    /// performed ([`InvocationOutcome::plugin_name`]): a document declaring several steps
    /// still has exactly one its platform runs ([`super::guest_execution::shell_step`]), so
    /// there is no second step with a status of its own.
    fn to_command_plugin(&self, ran: &InvocationOutcome) -> CommandPlugin {
        CommandPlugin {
            name: Some(ran.plugin_name.clone()),
            status: Some(ran.status().to_string()),
            status_details: Some(ran.status_details().to_string()),
            response_code: ran.response_code,
            // The agent took the command when it was sent and its run was owed to have
            // finished at the invocation's own due instant.
            response_start_date_time: Some(wire_instant(self.requested_at)),
            response_finish_date_time: Some(wire_instant(ran.due_at)),
            output: Some(truncated_plugin_output(&ran.standard_output())),
            // The URLs name the files a plugin wrote to Amazon S3: "if the S3 bucket for the
            // command wasn't specified, then this string is empty"
            // (`CommandPlugin$StandardOutputUrl`), and nothing here writes one.
            standard_output_url: Some(String::new()),
            standard_error_url: Some(String::new()),
            // "(Deprecated) You can no longer specify this parameter. The system ignores
            // it. Instead, Amazon Web Services Systems Manager automatically determines the
            // S3 bucket region" (`CommandPlugin$OutputS3Region`) — the same reading
            // `Command$OutputS3Region` gets in [`Self::to_command`]: what is reported is
            // the Region the command was sent to.
            output_s3_region: Some(self.region.clone()),
            output_s3_bucket_name: self.output_s3_bucket_name.clone(),
            output_s3_key_prefix: self.output_s3_key_prefix.clone(),
        }
    }
}

/// How many bytes of a plugin's output one `CommandPlugin` carries: the modelled length of
/// `CommandPlugin$Output` (`len 0..2500`), past which the member could not hold what the
/// guest wrote. The whole of it is what `GetCommandInvocation`'s `StandardOutputContent`
/// reports.
const MAX_PLUGIN_OUTPUT_BYTES: usize = 2_500;

/// `output` cut to [`MAX_PLUGIN_OUTPUT_BYTES`], on a character boundary so the member still
/// carries text.
fn truncated_plugin_output(output: &str) -> String {
    cut_to(output, MAX_PLUGIN_OUTPUT_BYTES)
}

/// The node ids one target NAMES, as opposed to the nodes it describes.
///
/// Two of the keys a command targets nodes by are IDENTITIES rather than criteria:
/// "`Key=InstanceIds,Values=i-02573cafcfEXAMPLE,i-0471e04240EXAMPLE`" and the
/// `AWS::EC2::Instance` spelling of the same selection both name the nodes themselves — the
/// other form `InstanceIds` takes — while a tag, a resource group or the wildcard describes
/// whatever matches. So the values of such a target are node ids the request named and are
/// resolved against the inventory the way a named node id is, and `*` is left out of them:
/// it describes every node rather than naming one.
pub fn node_ids_a_target_names(target: &Target) -> Vec<&str> {
    let key = target.key.as_deref().unwrap_or_default();
    if key != TARGET_KEY_INSTANCE_IDS && key != TARGET_KEY_EC2_INSTANCE {
        return Vec::new();
    }
    target
        .values
        .as_deref()
        .unwrap_or_default()
        .iter()
        .map(String::as_str)
        .filter(|value| *value != TARGET_VALUE_ALL)
        .collect()
}

///
/// A target is CRITERIA, not an identity: "an array of search criteria that targets
/// managed nodes using a Key,Value combination that you specify" (`SendCommand`'s
/// `Targets`), so criteria that describe no registered node select none rather than
/// refusing the send. The values of one target UNION (the `Target$Values` doc: "if you
/// specified `tag:ServerRole`, you could specify `value:WebServer` to run a command on
/// instances that include EC2 tags of `ServerRole,WebServer`"), and `*` selects every
/// node the key can select.
///
/// Matching a node against a key and its values is the one node matcher this service
/// already has ([`ManagedNodeRecord::matches_filter`]), so the node-id and tag forms are
/// not matched a second way here.
pub fn nodes_matching_target<'a>(
    store: &'a super::ComAmazonawsSsmAmazonssmState,
    region: &'a str,
    target: &Target,
) -> Result<Vec<&'a str>, String> {
    let key = target.key.as_deref().unwrap_or_default();
    let values: &[String] = target.values.as_deref().unwrap_or_default();
    let every_node = values.iter().any(|value| value == TARGET_VALUE_ALL);
    let matched = |filter_key: &'static str, ec2_only: bool| -> Vec<&'a str> {
        store
            .managed_nodes_in_region(region)
            .filter(move |(_, node)| {
                !ec2_only
                    || node.properties.resource_type.as_deref()
                        == Some(super::RESOURCE_TYPE_EC2_INSTANCE)
            })
            .filter(move |(_, node)| {
                every_node || node.matches_filter(filter_key, values, super::OPERATOR_EQUAL)
            })
            .map(|(id, _)| id)
            .collect()
    };

    match key {
        // The node-id form selects by identity; `AWS::EC2::Instance` is that same
        // selection narrowed to the nodes that ARE EC2 instances.
        TARGET_KEY_INSTANCE_IDS => Ok(matched(TARGET_KEY_INSTANCE_IDS, false)),
        TARGET_KEY_EC2_INSTANCE => Ok(matched(TARGET_KEY_INSTANCE_IDS, true)),
        // A resource group is a resource of another service, and nothing registers one in
        // this emulator, so a group names no node reachable here and the target selects
        // none — what the store holds, rather than a refusal.
        TARGET_KEY_RESOURCE_GROUP_NAME | TARGET_KEY_RESOURCE_GROUP => Ok(Vec::new()),
        super::TAG_KEY_FILTER_KEY => Ok(matched(super::TAG_KEY_FILTER_KEY, false)),
        _ => match key.strip_prefix(super::TAG_FILTER_KEY_PREFIX) {
            Some(name) if !name.is_empty() => Ok(store
                .managed_nodes_in_region(region)
                .filter(|(_, node)| {
                    node.tags
                        .get(name)
                        .is_some_and(|held| values.iter().any(|wanted| wanted == held))
                })
                .map(|(id, _)| id)
                .collect()),
            _ => Err(key.to_string()),
        },
    }
}

/// How this service words the `InvalidParameters` refusal.
///
/// That error states the rule in prose — "You must specify values for all required
/// parameters in the Amazon Web Services Systems Manager document (SSM document). You can
/// only supply values to parameters defined in the SSM document" — and the service words the
/// refusal itself in one FIXED sentence about the parameters as a set: it names neither the
/// document nor the parameter that was wrong, so nothing of the request is repeated into it.
/// Every operation that answers `InvalidParameters` answers this sentence, because the
/// refusal is about the values and not about what was being asked of the document. The
/// operations that read the same declared parameters but answer a DIFFERENT error class —
/// an Automation start's `InvalidAutomationExecutionParametersException`, a preview's
/// validation refusal — word their own refusal, through [`parameter_rejection`].
pub const INVALID_PARAMETERS: &str = "Parameters provided in document are invalid or not supported.";

/// Whether the parameter values a call supplied are not the ones the document declares —
/// the test the `InvalidParameters` refusal is answered on ([`INVALID_PARAMETERS`]).
pub fn parameters_are_rejected(
    document: &super::ParsedDocumentContent,
    supplied: &BTreeMap<String, Vec<String>>,
) -> bool {
    parameter_mismatch(document, supplied).is_some()
}

/// How the parameter values a call supplied are not the ones the document declares, or
/// `None` when they are.
///
/// Three readings of one rule, in the order a call meets them:
///
/// * a value supplied for a parameter the document does not declare is no value of that
///   document at all ("You can only supply values to parameters defined in the SSM
///   document"), and a parameter name is as the content spells it;
/// * a declared parameter is REQUIRED when the document declares no default for it: the
///   default is the value the document runs with when the call names none, so a parameter
///   that has one needs no value from the caller and a parameter that has none cannot be
///   left out ("You must specify values for all required parameters");
/// * a value supplied for a parameter the document DOES declare has to be one that
///   parameter admits — the enumeration, the pattern, the lengths and the counts its own
///   declaration states ([`super::DeclaredParameterConstraints`]) — since a value outside
///   them is not a value the document defined that parameter to take.
fn parameter_mismatch<'a>(
    document: &'a super::ParsedDocumentContent,
    supplied: &'a BTreeMap<String, Vec<String>>,
) -> Option<ParameterMismatch<'a>> {
    let declared = &document.parameters;
    for name in supplied.keys() {
        if !declared
            .iter()
            .any(|parameter| parameter.name.as_deref() == Some(name.as_str()))
        {
            return Some(ParameterMismatch::Undeclared(name.as_str()));
        }
    }
    let missing = declared
        .iter()
        .filter(|parameter| parameter.default_value.is_none())
        .filter_map(|parameter| parameter.name.as_deref())
        .find(|name| !supplied.contains_key(*name));
    if let Some(missing) = missing {
        return Some(ParameterMismatch::Missing(missing));
    }
    for constraints in &document.parameter_constraints {
        let Some(values) = supplied.get(&constraints.name) else {
            continue;
        };
        if !values_a_parameter_admits(constraints, values) {
            return Some(ParameterMismatch::Value(constraints.name.as_str()));
        }
    }
    None
}

/// Whether `values` are values the parameter `constraints` describes admits.
///
/// Each reading is the declaration's own: a parameter of a type that is ONE value takes one
/// ([`super::DeclaredParameterConstraints::takes_several_values`]), `minItems`/`maxItems`
/// bound how many there may be, `allowedValues` enumerate the values themselves,
/// `minChars`/`maxChars` bound each one's length, and `allowedPattern` is the expression
/// each has to match. A parameter that declares a pattern this service cannot read as a
/// regular expression restricts nothing by it, so a document is never unusable because of
/// one.
fn values_a_parameter_admits(
    constraints: &super::DeclaredParameterConstraints,
    values: &[String],
) -> bool {
    // A parameter NAMED with no values at all states no value for this reading to judge:
    // a rate-controlled Automation start names its target parameter that way, since the
    // values bound to it are the targets' rather than the call's
    // ([`super::automation::target_rejection`]).
    if values.is_empty() {
        return true;
    }
    if !constraints.takes_several_values() && values.len() > 1 {
        return false;
    }
    if constraints.min_items.is_some_and(|min| values.len() < min)
        || constraints.max_items.is_some_and(|max| values.len() > max)
    {
        return false;
    }
    let pattern = constraints
        .allowed_pattern
        .as_deref()
        .and_then(super::parameter_pattern);
    values.iter().all(|value| {
        if !constraints.allowed_values.is_empty()
            && !constraints
                .allowed_values
                .iter()
                .any(|allowed| allowed == value)
        {
            return false;
        }
        let length = value.chars().count();
        if constraints.min_chars.is_some_and(|min| length < min)
            || constraints.max_chars.is_some_and(|max| length > max)
        {
            return false;
        }
        pattern
            .as_ref()
            .is_none_or(|pattern| pattern.is_match(value))
    })
}

/// One way the parameter values a call supplied are not the ones the document declares.
enum ParameterMismatch<'a> {
    /// A value was supplied for a parameter the document declares none of.
    Undeclared(&'a str),
    /// A parameter the document declares no default for was left without a value — the
    /// FIRST such parameter in the order the document declares them, which is the one a
    /// refusal names.
    Missing(&'a str),
    /// A declared parameter was supplied values its own declaration does not admit.
    Value(&'a str),
}

/// Why the parameter values a call supplied are not the ones the document declares, or
/// `None` when they are — the DIAGNOSTIC form, naming the document and the parameter it was
/// read against.
///
/// Read by the operations whose own error class words such a refusal that way; the ones that
/// answer `InvalidParameters` answer [`INVALID_PARAMETERS`] instead, on the same reading
/// ([`parameters_are_rejected`]).
///
/// A required parameter left without a value is stated as the Automation plane states it —
/// "Missing required parameter: <name> in user inputs.", fixed text around the name of the
/// parameter that was omitted — and names ONE parameter, because the sentence is about one:
/// the first the document declares that the call left out.
pub fn parameter_rejection(
    document_name: &str,
    document: &super::ParsedDocumentContent,
    supplied: &BTreeMap<String, Vec<String>>,
) -> Option<String> {
    match parameter_mismatch(document, supplied)? {
        ParameterMismatch::Undeclared(name) => Some(format!(
            "Document {document_name} doesn't support a parameter named {name}"
        )),
        ParameterMismatch::Missing(name) => Some(format!(
            "Missing required parameter: {name} in user inputs."
        )),
        ParameterMismatch::Value(name) => Some(format!(
            "Document {document_name} doesn't support the values supplied for parameter {name}"
        )),
    }
}

/// Why the document cannot run on a node the send NAMED, or `None` when it can.
///
/// "The document doesn't support the platform type of the given managed node IDs. For
/// example, you sent an document for a Windows managed node to a Linux node"
/// (`UnsupportedPlatformType`), so the platforms a document's own content restricts it to
/// ([`super::ParsedDocumentContent::platform_types`]) have to include the platform the
/// node reported. A node that has reported no platform restricts nothing.
pub fn unsupported_platform(
    document_name: &str,
    platform_types: &[String],
    node: &ManagedNodeRecord,
) -> Option<String> {
    let platform = node.properties.platform_type.as_deref()?;
    if platform_types.is_empty() || platform_types.iter().any(|held| held == platform) {
        return None;
    }
    Some(format!(
        "Document {document_name} doesn't support platform type {platform}"
    ))
}

/// The `CommandFilterKey` that narrows the history by when a command was requested:
/// "InvokedAfter: Specify a timestamp to limit your results … occurring July 7, 2021, and
/// later" (`CommandFilter$value`). From the modelled enum
/// (`crate::ops::types::COMMAND_FILTER_KEY_VALUES`).
pub const COMMAND_FILTER_KEY_INVOKED_AFTER: &str = "InvokedAfter";

/// The same narrowing from the other side: "InvokedBefore: Specify a timestamp to limit
/// your results … from before July 7, 2021" (same doc).
pub const COMMAND_FILTER_KEY_INVOKED_BEFORE: &str = "InvokedBefore";

/// The `CommandFilterKey` that narrows the history by a command's status: "Specify a valid
/// command status to see a list of all command executions with that status" (same doc).
pub const COMMAND_FILTER_KEY_STATUS: &str = "Status";

/// The `CommandFilterKey` that narrows the history by whether a command is still running
/// (same doc), whose values are [`EXECUTION_STAGE_VALUES`].
pub const COMMAND_FILTER_KEY_EXECUTION_STAGE: &str = "ExecutionStage";

/// The `CommandFilterKey` that narrows the history by the document a command runs:
/// "Specify name of the … document … for which you want to see command execution results"
/// (same doc).
pub const COMMAND_FILTER_KEY_DOCUMENT_NAME: &str = "DocumentName";

/// Whether `key` is a filter key a command history read accepts: the modelled
/// `CommandFilterKey` enum is that whole set, so nothing else names a property of a
/// command to narrow by.
pub fn is_command_filter_key(key: &str) -> bool {
    crate::ops::types::COMMAND_FILTER_KEY_VALUES.contains(&key)
}

/// The values a `Status` filter of a COMMAND history read may name, in the order
/// `CommandFilter$value` lists them ("The status values you can specify for
/// `ListCommands` are …") — a wider set than the modelled `CommandStatus` enum, because
/// the detailed status a command reports in words is also named here, spelled without its
/// spaces ([`CommandRecord::matches_status_filter`]).
pub const LIST_COMMANDS_STATUS_FILTER_VALUES: &[&str] = &[
    "Pending",
    "InProgress",
    "Success",
    "Cancelled",
    "Failed",
    "TimedOut",
    "AccessDenied",
    "DeliveryTimedOut",
    "ExecutionTimedOut",
    "Incomplete",
    "NoInstancesInTag",
    "LimitExceeded",
];

/// The `ExecutionStage` value that selects the commands still running: "Executing:
/// Returns a list of command executions that are currently still running"
/// (`CommandFilter$value`).
pub const EXECUTION_STAGE_EXECUTING: &str = "Executing";

/// The `ExecutionStage` value that selects the commands that are done: "Complete: Returns
/// a list of command executions that have already completed" (same doc).
pub const EXECUTION_STAGE_COMPLETE: &str = "Complete";

/// The values an `ExecutionStage` filter may name (same doc). What a refusal of another
/// value lists.
pub const EXECUTION_STAGE_VALUES: &[&str] = &[EXECUTION_STAGE_EXECUTING, EXECUTION_STAGE_COMPLETE];

/// Whether `status` is one of the terminal states, which a command and one of its
/// invocations reach by the same names: `Command$StatusDetails` and
/// `CommandInvocation$StatusDetails` both end at "Success", "Cancelled", "Failed" and the
/// timed-out states, so this reads the one set ([`TERMINAL_INVOCATION_STATUSES`]).
pub fn is_terminal_status(status: &str) -> bool {
    TERMINAL_INVOCATION_STATUSES.contains(&status)
}

/// Whether `id` is shaped like a `CommandId`: the modelled length is exactly 36, and this
/// service mints the hyphenated UUID form ([`super::ComAmazonawsSsmAmazonssmState::mint_command_id`]),
/// read by the one reader of that form this crate has.
pub fn is_command_id(id: &str) -> bool {
    super::is_activation_id(id)
}

/// The bounds the modelled `CommandMaxResults` carries (range 1..50).
pub const MIN_COMMAND_PAGE_SIZE: i32 = 1;

/// See [`MIN_COMMAND_PAGE_SIZE`]. It is also the page size a request that names no
/// `MaxResults` gets, the way every other read of this service serves one.
pub const MAX_COMMAND_PAGE_SIZE: i32 = 50;

/// The page size a command history read serves, or the refusal message for a `MaxResults`
/// outside the modelled range — which the protocol's boundary validator already refuses,
/// so this is a backstop rather than the first line of defence.
pub fn command_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(max_results, MIN_COMMAND_PAGE_SIZE, MAX_COMMAND_PAGE_SIZE)
}

/// The instant a timestamp filter value names, or `None` when the value names none.
///
/// "Specify a timestamp to limit your results. For example, specify
/// `2024-07-07T00:00:00Z`" (`CommandFilter$value`) — the ISO 8601 spelling
/// `::aws_smithy_types::date_time::Format::DateTime` reads, which is the one the service
/// renders `RequestedDateTime` in.
pub fn parse_filter_instant(value: &str) -> Option<SystemTime> {
    let parsed = ::aws_smithy_types::DateTime::from_str(
        value,
        ::aws_smithy_types::date_time::Format::DateTime,
    )
    .ok()?;
    SystemTime::try_from(parsed).ok()
}

/// Why `value` is not a value the filter key `key` may carry, or `None` when it is one.
/// `position` is the filter's place in the request's `Filters` list, counted from one.
///
/// A filter value has to be a legal value of what it filters: a timestamp for the two
/// instant keys, one of the status values the operation documents for a `Status` filter,
/// and one of the two stages for an `ExecutionStage` filter. `DocumentName` filters a
/// document name, which constrains nothing here — a name no document carries selects no
/// command rather than refusing the read.
///
/// The sentence is worded the way a request-validation refusal of this API is: the value,
/// the member it was carried in — spelled as the emulator's own boundary validator spells a
/// list member's path — and the constraint it failed, as the page-size refusal of every read
/// of this service already states one (`super::page_size`). The enum clause lists the values
/// the operation documents; the timestamp clause is this emulator's own wording of what
/// `CommandFilter$value` documents the two instant keys as taking ("Specify a timestamp …
/// For example, specify 2024-07-07T00:00:00Z"), which no model constraint states.
pub fn command_filter_value_rejection(position: usize, key: &str, value: &str) -> Option<String> {
    let failed = |constraint: String| {
        Some(format!(
            "Value '{value}' at 'filters.{position}.member.value' failed to satisfy \
             constraint: {constraint}"
        ))
    };
    let enumerated =
        |values: &[&str]| format!("Member must satisfy enum value set: [{}]", values.join(", "));
    match key {
        COMMAND_FILTER_KEY_INVOKED_AFTER | COMMAND_FILTER_KEY_INVOKED_BEFORE
            if parse_filter_instant(value).is_none() =>
        {
            failed(
                "Member must be a timestamp, for example 2024-07-07T00:00:00Z".to_string(),
            )
        }
        COMMAND_FILTER_KEY_STATUS if !LIST_COMMANDS_STATUS_FILTER_VALUES.contains(&value) => {
            failed(enumerated(LIST_COMMANDS_STATUS_FILTER_VALUES))
        }
        COMMAND_FILTER_KEY_EXECUTION_STAGE if !EXECUTION_STAGE_VALUES.contains(&value) => {
            failed(enumerated(EXECUTION_STAGE_VALUES))
        }
        _ => None,
    }
}

/// How this service states that a node id cannot be named beside a `Pending` status
/// filter: the sentence `ListCommandsRequest$InstanceId` states the rule in, verbatim —
/// "You can't specify a managed node ID in the same command that you specify Status =
/// Pending. This is because the command hasn't reached the managed node yet." It carries
/// nothing of the request, because the rule is about the two criteria and not about which
/// node or which filter was named.
pub const NODE_ID_WITH_PENDING_STATUS: &str =
    "You can't specify a managed node ID in the same command that you specify Status = \
     Pending. This is because the command hasn't reached the managed node yet.";

/// How this service states that the managed nodes a request NAMED cannot take the call.
///
/// `InvalidInstanceId` is what every operation that RESOLVES a node id answers with when
/// the ids name no node it can act on: "SSM Agent isn't registered with the SSM endpoint
/// … The managed node isn't in a valid state". The sentence carries no interpolated value
/// — it states the refusal for the set of ids the request named, whatever that set was,
/// so neither the ids nor the account are repeated into it.
pub const NODES_NOT_IN_A_VALID_STATE: &str = "Instances not in a valid state for account";

/// How Run Command words a call naming a command id its endpoint holds no command for.
///
/// `InvalidCommandId` states the rule in prose — "the specified command ID isn't valid.
/// Verify the ID and try again" — and the service words the refusal with the ID that
/// resolved nothing, taken from the request. The modelled shape declares no message member,
/// so this travels through [`crate::scaffolded::Scaffolded::invalid_command_id`]; every
/// operation that RESOLVES a command id words it the same way, because the refusal is about
/// the id and not about what was being asked of the command.
pub fn command_not_found(command_id: &str) -> String {
    format!("Command id {command_id} does not exist.")
}

/// How Run Command words a read whose command and node hold no invocation between them.
///
/// `InvocationDoesNotExist` states the rule in prose — "the command ID and managed node ID
/// you specified didn't match any invocations" — and the service words the refusal with the
/// PAIR that matched none: the command the read named and the node it named, in that order,
/// taken from the request. The modelled shape declares no message member, so this travels
/// through [`crate::scaffolded::Scaffolded::invocation_does_not_exist`].
pub fn invocation_not_found(command_id: &str, instance_id: &str) -> String {
    format!("Invocation not found for {command_id}, {instance_id}")
}
