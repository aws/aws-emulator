// SPDX-License-Identifier: CC0-1.0

//! The TASKS registered with a maintenance window: "information about a task defined for a
//! maintenance window" (`smithy_query(query="shape",shape="MaintenanceWindowTask")`).
//!
//! `RegisterTaskWithMaintenanceWindow` mints one, `UpdateMaintenanceWindowTask` edits it and
//! `GetMaintenanceWindowTask` reports it, all through the one store below
//! ([`super::ComAmazonawsSsmAmazonssmState::maintenance_window_tasks`]), so whatever the
//! register and the updates persisted is what a later read reports and what deleting the
//! window removes.
//!
//! A task belongs to ONE maintenance window of one Region — a window is reached through the
//! endpoint of the Region it was created in — so the store is keyed by
//! `(region, window task id)`, with the window it belongs to on the record: the id is unique
//! across the Region, which is what lets a read address one by `(WindowId, WindowTaskId)` and
//! what makes a task of another window no task of this one. The id is the SERVICE's to
//! choose: the register takes no identifier for the task and answers the `WindowTaskId` it
//! minted, which the model fixes as the 8-4-4-4-12 hexadecimal UUID
//! (`MaintenanceWindowTaskId`'s pattern).
//!
//! Several members are not held as the register stated them, because the model states what
//! the service puts there instead:
//!
//! * `MaxConcurrency`/`MaxErrors` — "For maintenance window tasks without a target specified,
//!   you can't supply a value for this option. Instead, the system inserts a placeholder
//!   value of 1" (`RegisterTaskWithMaintenanceWindowRequest$MaxConcurrency`), so a TARGETLESS
//!   task holds [`TARGETLESS_PLACEHOLDER`] for both whatever the register stated
//!   ([`held_bound`]).
//! * `CutoffBehavior` — "CONTINUE_TASK: When the cutoff time is reached, any tasks that are
//!   running continue. The default value"
//!   (`RegisterTaskWithMaintenanceWindowRequest$CutoffBehavior`) states how a task whose
//!   registration chose nothing BEHAVES at the cutoff, not a value the registration stores:
//!   `MaintenanceWindowTask$CutoffBehavior` carries no modelled default, so a register that
//!   named no cutoff behaviour leaves the member unset and a read reports nothing for it,
//!   rather than reporting a choice the caller never made back to it.
//! * `ServiceRoleArn` — "if you do not specify a service role ARN, Systems Manager uses a
//!   service-linked role in your account" (`MaintenanceWindowTask$ServiceRoleArn`), so a task
//!   whose register (or replacing update) named no role holds the account's service-linked role
//!   for this service ([`service_linked_role_arn`]) rather than nothing at all: every
//!   registered task names the identity it runs as, which is what a read-modify-write of the
//!   task feeds back without rewriting who it runs as.
//! * `TaskParameters` — the map "that should be passed to the task when it is run" is one the
//!   service materialises for every registered task, so a task registered with no parameters
//!   holds the EMPTY map and a read reports it present and empty
//!   ([`MaintenanceWindowTaskRecord::reported_task_parameters`]).
//!
//! What a task TARGETS is stated as criteria rather than resolved here, the way a window's
//! targets are ([`super::maintenance_window_targets`]) — except that one criterion names a
//! registered target of the same window by id ("specify maintenance window targets using the
//! following format: Key=WindowTargetIds,Values=",
//! `RegisterTaskWithMaintenanceWindowRequest$Targets`), which is the reference
//! `DeregisterTargetFromMaintenanceWindow` checks for before removing a target
//! ([`MaintenanceWindowTaskRecord::references_target`]).

use std::collections::BTreeMap;
use std::time::SystemTime;

use crate::ops::types::{
    AlarmConfiguration, LoggingInfo, MaintenanceWindowTask,
    MaintenanceWindowTaskInvocationParameters, MaintenanceWindowTaskParameterValueExpression,
    Target,
};

use super::ComAmazonawsSsmAmazonssmState;

/// What `MaxConcurrency` and `MaxErrors` hold for a task registered without targets: "the
/// system inserts a placeholder value of 1"
/// (`RegisterTaskWithMaintenanceWindowRequest$MaxConcurrency`).
pub const TARGETLESS_PLACEHOLDER: &str = "1";

/// What `Priority` holds for a register that carried none, from the modelled default of
/// `MaintenanceWindowTask$Priority` (`default 0`) — "the lower the number the higher the
/// priority".
pub const DEFAULT_PRIORITY: i32 = 0;

/// The service principal the role a maintenance window task runs as is linked to, in the
/// global spelling IAM files and trusts a service by — the same principal this service's other
/// role readings name it with ([`super::activation_role::HYBRID_REGISTRATION_PRINCIPAL`]).
pub const SERVICE_LINKED_ROLE_PRINCIPAL: &str =
    super::activation_role::HYBRID_REGISTRATION_PRINCIPAL;

/// The name of the service-linked role Systems Manager keeps in the account for its
/// maintenance window tasks, which is the name IAM publishes for
/// [`SERVICE_LINKED_ROLE_PRINCIPAL`] and therefore the role IAM's own reads resolve
/// (`::service_com_amazonaws_iam_awsidentitymanagementv20100508::state::published_service_linked_role_name`).
pub const SERVICE_LINKED_ROLE_NAME: &str = "AWSServiceRoleForAmazonSSM";

/// The ARN of the role a task whose request named none runs as: "if you do not specify a
/// service role ARN, Systems Manager uses a service-linked role in your account"
/// (`MaintenanceWindowTask$ServiceRoleArn`).
///
/// A role is an IAM resource, and IAM is global, so the ARN names IAM and carries no Region
/// whichever Region the request was signed for; it is composed by IAM's own role-ARN and
/// service-linked-role-path builders, so the ARN this service reports is the one IAM reports
/// for that role.
#[must_use]
pub fn service_linked_role_arn() -> String {
    use ::service_com_amazonaws_iam_awsidentitymanagementv20100508::state as iam;

    iam::role_arn(
        &iam::service_linked_role_path(SERVICE_LINKED_ROLE_PRINCIPAL),
        SERVICE_LINKED_ROLE_NAME,
    )
}

/// The role a task is HELD as running as, given the role the request stated: the role it
/// stated, or the account's service-linked role ([`service_linked_role_arn`]) where it stated
/// none.
///
/// The register and the update read the member the same way, because the sentence is stated on
/// both (`RegisterTaskWithMaintenanceWindowRequest$ServiceRoleArn`,
/// `UpdateMaintenanceWindowTaskRequest$ServiceRoleArn`).
#[must_use]
pub fn held_service_role_arn(stated: Option<String>) -> String {
    stated.unwrap_or_else(service_linked_role_arn)
}

/// The criterion key by which a task's `Targets` names a REGISTERED target of its own window
/// rather than nodes: "specify maintenance window targets using the following format:
/// Key=WindowTargetIds,Values=" (`RegisterTaskWithMaintenanceWindowRequest$Targets`).
pub const WINDOW_TARGET_IDS_KEY: &str = "WindowTargetIds";

/// The filter keys a describe of a window's tasks may narrow by: "the supported filter keys
/// are `WindowTaskId`, `TaskArn`, `Priority`, and `TaskType`"
/// (`DescribeMaintenanceWindowTasksRequest$Filters`). What a refusal of another key lists, and
/// the one place that set is written.
pub const TASK_FILTER_KEYS: &[&str] = &["WindowTaskId", "TaskArn", "Priority", "TaskType"];

/// The page size a describe of a window's tasks serves when the request names none: the
/// modelled maximum of `MaintenanceWindowMaxResults` (`range 10..100`).
pub const MAX_TASK_PAGE_SIZE: i32 = 100;

/// The smallest page that read serves, from the same modelled range.
pub const MIN_TASK_PAGE_SIZE: i32 = 10;

/// The page size a task describe serves (see `super::page_size`).
pub fn task_page_size(max_results: Option<i32>) -> Result<usize, String> {
    super::page_size(max_results, MIN_TASK_PAGE_SIZE, MAX_TASK_PAGE_SIZE)
}

/// Whether `key` is one of the filter keys a task describe takes ([`TASK_FILTER_KEYS`]).
#[must_use]
pub fn is_task_filter_key(key: &str) -> bool {
    TASK_FILTER_KEYS.contains(&key)
}

/// How many tasks this store holds for ONE maintenance window.
///
/// This is the EMULATOR's own ceiling rather than a quota Systems Manager publishes: a
/// register is the only thing that admits a task, callers may run as many as they like, and
/// the store is held in memory for as long as the process runs. A register past it is refused
/// through the modelled `ResourceLimitExceededException` and writes nothing.
pub const MAX_TASKS_PER_WINDOW: usize = 50;

/// What a register refused by [`MAX_TASKS_PER_WINDOW`] reports: the emulator's own ceiling,
/// named as the emulator's, with the figure it stands at.
#[must_use]
pub fn task_ceiling_message(window_id: &str) -> String {
    format!(
        "This emulator holds at most {MAX_TASKS_PER_WINDOW} tasks for the maintenance window \
         {window_id}"
    )
}

/// How this service reports a `WindowTaskId` that names no task of the window the request
/// addressed, through `DoesNotExistException` — whose own account of its class is "the
/// specified resource, such as a maintenance window or patch baseline, doesn't exist".
#[must_use]
pub fn task_not_found_message(window_id: &str, window_task_id: &str) -> String {
    format!("Maintenance window {window_id} has no task {window_task_id}")
}

/// How `DeregisterTargetFromMaintenanceWindow` reports a target it will not remove because a
/// task of the same window still names it: the fixed sentence, then the id of the task that
/// names it, read from this store.
#[must_use]
pub fn target_referenced_message(window_task_id: &str) -> String {
    format!(
        "This Target cannot be deregistered because it is still referenced in Task: \
         {window_task_id}"
    )
}

/// What `MaxConcurrency` or `MaxErrors` is HELD as, given the targets the task now has and the
/// value the request stated for the bound: a targetless task holds the documented placeholder
/// ([`TARGETLESS_PLACEHOLDER`]) and a targeted one holds what was stated.
///
/// The register and the update read the bounds the same way, because the sentence is stated on
/// both (`RegisterTaskWithMaintenanceWindowRequest$MaxConcurrency`,
/// `UpdateMaintenanceWindowTaskRequest$MaxConcurrency`).
#[must_use]
pub fn held_bound(targets: &[Target], stated: Option<String>) -> Option<String> {
    if targets.is_empty() {
        Some(TARGETLESS_PLACEHOLDER.to_string())
    } else {
        stated
    }
}

/// One task registered with a maintenance window, as the store holds it.
///
/// Every member a read reports is here, so a read projects this record rather than inventing
/// anything.
#[derive(Debug, Clone)]
pub struct MaintenanceWindowTaskRecord {
    /// The server-minted id this record is stored under.
    pub window_task_id: String,
    /// The Region whose endpoint the task was registered through.
    pub region: String,
    /// "The ID of the maintenance window the task should be added to" — required on register.
    pub window_id: String,
    /// "The ARN of the task to run" — required on register, and held as stated: "Maintenance
    /// Window does not validate the TaskArn when you register a task"
    /// (`MaintenanceWindowTask$TaskArn`).
    pub task_arn: String,
    /// "The type of task being registered" (`MaintenanceWindowTaskType`) — required on
    /// register.
    pub task_type: String,
    /// "The targets (either managed nodes or maintenance window targets)", exactly as the
    /// register stated them.
    pub targets: Vec<Target>,
    /// "The parameters that should be passed to the task when it is run", ordered so two
    /// reads of one task report them the same way. EMPTY for a register that carried none,
    /// which a read reports as the empty map (see the module docs).
    pub task_parameters: BTreeMap<String, MaintenanceWindowTaskParameterValueExpression>,
    /// "The parameters that the task should use during execution", held as stated.
    pub task_invocation_parameters: Option<MaintenanceWindowTaskInvocationParameters>,
    /// "The priority of the task in the maintenance window", defaulted to
    /// [`DEFAULT_PRIORITY`] by the register.
    pub priority: i32,
    /// "The maximum number of targets this task can be run for, in parallel", as
    /// [`held_bound`] reads it.
    pub max_concurrency: Option<String>,
    /// "The maximum number of errors allowed before this task stops being scheduled", as
    /// [`held_bound`] reads it.
    pub max_errors: Option<String>,
    /// "A structure containing information about an Amazon Simple Storage Service (Amazon S3)
    /// bucket to write managed node-level logs to", or `None`.
    pub logging_info: Option<LoggingInfo>,
    /// "The Amazon Resource Name (ARN) of the IAM service role for Amazon Web Services
    /// Systems Manager to assume when running a maintenance window task" — the role stated,
    /// or the account's service-linked role where none was ([`held_service_role_arn`]).
    pub service_role_arn: String,
    /// "An optional name for the task", or `None`.
    pub name: Option<String>,
    /// "An optional description for the task", or `None`.
    pub description: Option<String>,
    /// "Indicates whether tasks should continue to run after the cutoff time specified in the
    /// maintenance windows is reached", held exactly as the register (or a later update)
    /// stated it and `None` where neither stated it (see the module docs).
    pub cutoff_behavior: Option<String>,
    /// "The CloudWatch alarm you want to apply to your maintenance window task", or `None`.
    pub alarm_configuration: Option<AlarmConfiguration>,
    /// The "user-provided idempotency token" the register carried, or `None`: what makes a
    /// retry of that register report the task it already minted rather than a second one.
    pub client_token: Option<String>,
    /// When the register ran, so a listing of one window's tasks walks them in the order they
    /// were registered where their ids do not order them.
    pub registered_at: SystemTime,
}

impl MaintenanceWindowTaskRecord {
    /// The modelled `MaintenanceWindowTask` for this record, which is what a listing of one
    /// window's tasks reports: every member either held as the register stated it or the one
    /// the service chose for it.
    #[must_use]
    pub fn to_maintenance_window_task(&self) -> MaintenanceWindowTask {
        MaintenanceWindowTask {
            window_id: Some(self.window_id.clone()),
            window_task_id: Some(self.window_task_id.clone()),
            task_arn: Some(self.task_arn.clone()),
            // `MaintenanceWindowTask` names the register's `TaskType` `Type`.
            r#type: Some(self.task_type.clone()),
            targets: Some(self.targets.clone()),
            task_parameters: Some(self.reported_task_parameters()),
            priority: self.priority,
            logging_info: self.logging_info.clone(),
            service_role_arn: Some(self.service_role_arn.clone()),
            max_concurrency: self.max_concurrency.clone(),
            max_errors: self.max_errors.clone(),
            name: self.name.clone(),
            description: self.description.clone(),
            cutoff_behavior: self.cutoff_behavior.clone(),
            alarm_configuration: self.alarm_configuration.clone(),
        }
    }

    /// The task parameters a read reports: the map this task holds, which is the EMPTY map for
    /// a register that stated none — the map is one the service materialises for every
    /// registered task, so a read reports it present and empty rather than leaving the member
    /// out (see the module docs).
    #[must_use]
    pub fn reported_task_parameters(
        &self,
    ) -> std::collections::HashMap<String, MaintenanceWindowTaskParameterValueExpression> {
        self.task_parameters
            .iter()
            .map(|(name, expression)| (name.clone(), expression.clone()))
            .collect()
    }

    /// Whether this task names the registered target `window_target_id` among its targets,
    /// which is the reference that makes that target one still in use.
    ///
    /// The reference is the documented criterion form ([`WINDOW_TARGET_IDS_KEY`]) rather than
    /// any criterion carrying the id: a tag value that happens to read like an id names nodes,
    /// not a target. The key is read without regard to case, because the model's own text
    /// spells such a key both ways (`Key=InstanceIds` beside `Key=instanceids` in
    /// `MaintenanceWindowTask$Targets`).
    #[must_use]
    pub fn references_target(&self, window_target_id: &str) -> bool {
        self.targets.iter().any(|criterion| {
            criterion
                .key
                .as_deref()
                .is_some_and(|key| key.eq_ignore_ascii_case(WINDOW_TARGET_IDS_KEY))
                && criterion
                    .values
                    .as_deref()
                    .unwrap_or_default()
                    .iter()
                    .any(|value| value == window_target_id)
        })
    }

    /// Whether the filter `key`=`values` selects this task.
    ///
    /// Each key narrows by one property of the task, and a filter's values UNION: a task is
    /// selected when one of that filter's values is its value for the key. `Priority` is an
    /// integer on the record and a string in the filter, so it is compared as the decimal the
    /// caller writes it as. A key outside [`TASK_FILTER_KEYS`] selects nothing — its caller
    /// refuses such a key before anything is asked about the tasks.
    #[must_use]
    pub fn matches_filter(&self, key: &str, values: &[String]) -> bool {
        let held = match key {
            "WindowTaskId" => self.window_task_id.clone(),
            "TaskArn" => self.task_arn.clone(),
            "Priority" => self.priority.to_string(),
            // `MaintenanceWindowTask` names the register's `TaskType` `Type`, and this filter
            // key is the register's spelling of it.
            "TaskType" => self.task_type.clone(),
            _ => return false,
        };
        values.contains(&held)
    }

    /// The members a register SUPPLIED, rendered as one canonical string.
    ///
    /// This is what "the parameters don't match the original call to the API with the same
    /// idempotency token" (`IdempotentParameterMismatch`) is decided on: two registers
    /// carrying one idempotency token are the same call when their supplied members render the
    /// same way. The minted id, the Region and the token itself are not supplied members, so
    /// they are not in it.
    #[must_use]
    pub fn registration_fingerprint(&self) -> String {
        let targets: Vec<String> = self
            .targets
            .iter()
            .map(|target| {
                format!(
                    "{}={}",
                    target.key.as_deref().unwrap_or_default(),
                    target.values.clone().unwrap_or_default().join(",")
                )
            })
            .collect();
        let parameters: Vec<String> = self
            .task_parameters
            .iter()
            .map(|(name, expression)| {
                format!(
                    "{name}={}",
                    expression.values.clone().unwrap_or_default().join(",")
                )
            })
            .collect();
        format!(
            "window={}\narn={}\ntype={}\ntargets=[{}]\nparameters=[{}]\ninvocation={:?}\n\
             priority={}\nconcurrency={}\nerrors={}\nlogging={:?}\nrole={}\nname={}\n\
             description={}\ncutoff={}\nalarms={:?}\n",
            self.window_id,
            self.task_arn,
            self.task_type,
            targets.join("|"),
            parameters.join("|"),
            self.task_invocation_parameters,
            self.priority,
            self.max_concurrency.as_deref().unwrap_or_default(),
            self.max_errors.as_deref().unwrap_or_default(),
            self.logging_info,
            self.service_role_arn,
            self.name.as_deref().unwrap_or_default(),
            self.description.as_deref().unwrap_or_default(),
            self.cutoff_behavior.as_deref().unwrap_or_default(),
            self.alarm_configuration,
        )
    }
}

impl ComAmazonawsSsmAmazonssmState {
    /// The task `window_task_id` names among `window_id`'s tasks in `region`, or `None` when
    /// that window holds none under that id — which is what a not-found refusal reports.
    ///
    /// The window is part of the question: a task id names one record of the Region, and a
    /// request addressing it through ANOTHER window names no task of that window.
    pub fn maintenance_window_task(
        &self,
        region: &str,
        window_id: &str,
        window_task_id: &str,
    ) -> Option<&MaintenanceWindowTaskRecord> {
        self.maintenance_window_tasks
            .get(&(region.to_string(), window_task_id.to_string()))
            .filter(|held| held.window_id == window_id)
    }

    /// Every task `window_id` holds in `region`, in the order they were registered — the order
    /// a listing reports them in, with the minted id breaking a tie so the order is total and
    /// a page can resume at an id.
    pub fn maintenance_window_tasks_of(
        &self,
        region: &str,
        window_id: &str,
    ) -> Vec<&MaintenanceWindowTaskRecord> {
        let mut held: Vec<&MaintenanceWindowTaskRecord> = self
            .maintenance_window_tasks
            .iter()
            .filter(|((region_of, _), task)| region_of == region && task.window_id == window_id)
            .map(|(_, task)| task)
            .collect();
        held.sort_by(|left, right| {
            left.registered_at
                .cmp(&right.registered_at)
                .then_with(|| left.window_task_id.cmp(&right.window_task_id))
        });
        held
    }

    /// Store `record` as a task of its own Region, keyed by its minted id.
    pub fn insert_maintenance_window_task(&mut self, record: MaintenanceWindowTaskRecord) {
        let key = (record.region.clone(), record.window_task_id.clone());
        self.maintenance_window_tasks.insert(key, record);
    }

    /// Remove the task `window_task_id` names among `window_id`'s, answering whether there was
    /// one to remove.
    pub fn remove_maintenance_window_task(
        &mut self,
        region: &str,
        window_id: &str,
        window_task_id: &str,
    ) -> bool {
        if self
            .maintenance_window_task(region, window_id, window_task_id)
            .is_none()
        {
            return false;
        }
        self.maintenance_window_tasks
            .remove(&(region.to_string(), window_task_id.to_string()))
            .is_some()
    }

    /// Remove every task registered with `window_id` in `region`: what deleting the window
    /// does, since a task is only ever addressed through the window it belongs to.
    pub fn remove_maintenance_window_tasks_of(&mut self, region: &str, window_id: &str) {
        self.maintenance_window_tasks
            .retain(|(held_region, _), task| {
                held_region != region || task.window_id != window_id
            });
    }

    /// Whether `window_id` can take another task ([`MAX_TASKS_PER_WINDOW`]).
    pub fn can_hold_another_maintenance_window_task(&self, region: &str, window_id: &str) -> bool {
        self.maintenance_window_tasks_of(region, window_id).len() < MAX_TASKS_PER_WINDOW
    }

    /// The task of `window_id` that names the registered target `window_target_id`, or `None`
    /// when no task of that window does — which is what makes a safe deregistration of that
    /// target one the service refuses.
    pub fn maintenance_window_task_referencing_target(
        &self,
        region: &str,
        window_id: &str,
        window_target_id: &str,
    ) -> Option<&MaintenanceWindowTaskRecord> {
        self.maintenance_window_tasks_of(region, window_id)
            .into_iter()
            .find(|task| task.references_target(window_target_id))
    }

    /// The task `region` holds under the idempotency token `client_token`, or `None` when no
    /// register carrying that token has run there.
    pub fn maintenance_window_task_with_client_token(
        &self,
        region: &str,
        client_token: &str,
    ) -> Option<&MaintenanceWindowTaskRecord> {
        self.maintenance_window_tasks
            .iter()
            .filter(|((held, _), _)| held == region)
            .map(|(_, record)| record)
            .find(|record| record.client_token.as_deref() == Some(client_token))
    }
}
