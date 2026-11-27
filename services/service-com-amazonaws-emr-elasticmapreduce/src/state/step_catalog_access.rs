// SPDX-License-Identifier: CC0-1.0

//! WHETHER the work of a step may reach the cluster's Hive metastore: the AWS Glue Data
//! Catalog calls that work makes, decided against the permissions in force on the role the
//! step runs as.
//!
//! # The rule this module implements
//!
//! A step's work runs on the cluster's Amazon EC2 instances — "Also called instance profile
//! and Amazon EC2 role. An IAM role for an Amazon EMR cluster. The Amazon EC2 instances of
//! the cluster assume this role" (`RunJobFlow$JobFlowRole` doc) — or, where the step carries
//! one, as its own runtime role (`Step$ExecutionRoleArn`). So every AWS request that work
//! issues is authorized against the permissions in force on THAT role, an explicit `Deny`
//! final ([`super::instance_profile_credentials`], which states the same rule for the
//! credential the work presents).
//!
//! A cluster whose configuration makes its Hive metastore the Glue Data Catalog
//! ([`uses_glue_data_catalog`]) reaches that catalog through its metastore client, so
//! resolving a table a statement names IS such a request: the client reads the database and
//! then the table, and a statement that changes the metastore's partitions writes them. The
//! actions those calls are authorized under are the Glue ones AWS's own
//! `AmazonElasticMapReduceforEC2Role` policy grants a cluster's instance role
//! ([`GET_DATABASE_ACTION`], [`GET_TABLE_ACTION`], [`BATCH_CREATE_PARTITION_ACTION`],
//! [`BATCH_DELETE_PARTITION_ACTION`]), each named on the resources Glue publishes it on: the
//! catalog, the database in it and the table in that database.
//!
//! A call the role's policies DENY is not made: the metastore client raises Glue's own
//! [`ACCESS_DENIED_CODE`] carrying IAM's sentence for that refusal
//! ([`access_denied_message`]), the SQL client the statement was submitted through exits
//! non-zero, and the step settles `FAILED` with that error — never `COMPLETED` for a catalog
//! read the role may not make.
//!
//! An empty or unreadable policy set is NO such answer and withholds nothing, exactly as
//! every other reading of a role in this crate takes it
//! ([`::service_common::iam_role::role_denies_action`]): a role this emulator holds no
//! identity policy for is the routine state of one provisioned out of band, and an emulator
//! with no IAM mounted answers nothing at all.
//!
//! # The two forms the decision takes
//!
//! * [`StepCatalogAccess`] — one command LINE's: a step whose command line does more than
//!   invoke the SQL client runs that client as a program of its session
//!   ([`super::step_sql_cli`]), and the statements it runs are read while the line runs, so
//!   each table is decided when the client resolves it. The hop is made by blocking that
//!   command's own thread on the runtime handle, the way the Amazon S3 transfers of the same
//!   line are made ([`super::step_command_transfers`]), and bounded by a deadline of this
//!   command's own — one [`TRANSFER_BUDGET`] for every decision it asks for, beside the budget
//!   its transfers spend, so what one command may spend deciding is bounded whatever it
//!   transfers.
//! * [`resolve_due_step_catalog_access`] — one bare STATEMENT's: a command line that is
//!   nothing but the client's invocation has its statement read off the submission
//!   ([`read_step_work`]), so what it reaches is known before anything runs and is decided
//!   by the reporting operation, with this service's store released. Such a step does not END
//!   until it has been ([`StepRecord::awaits_catalog_access`]), so nothing reports a step
//!   `COMPLETED` for a catalog read nobody decided.
//!
//! Both end in ONE evaluation of the role's policies, the simulation this tree asks IAM for
//! ([`::service_common::iam_role`]), and both word the refusal with the shared sentence
//! ([`::service_common::iam_authorization`]), so the two forms cannot answer differently.

use ::std::collections::BTreeMap;
use ::std::sync::{Arc, Mutex, PoisonError};

use ::smithy_server_runtime::EmbeddedEmulator;

use super::*;

/// The ARN namespace of the AWS Glue Data Catalog, which is also its IAM action prefix.
const GLUE: &str = "glue";

/// The Glue action that reads ONE database's definition, which is what the metastore client
/// does to resolve the database a table name qualifies in.
///
/// SOURCE: the Glue actions AWS's own `AmazonElasticMapReduceforEC2Role` policy grants a
/// cluster's instance role, which are the calls that role's metastore client makes.
pub const GET_DATABASE_ACTION: &str = "glue:GetDatabase";

/// The Glue action that reads ONE table's definition, which is what the metastore client does
/// to resolve a table a statement names. SOURCE: as [`GET_DATABASE_ACTION`].
pub const GET_TABLE_ACTION: &str = "glue:GetTable";

/// The Glue action that REGISTERS partitions on a table, which is what the metastore client
/// does for a statement that adds them. SOURCE: as [`GET_DATABASE_ACTION`].
pub const BATCH_CREATE_PARTITION_ACTION: &str = "glue:BatchCreatePartition";

/// The Glue action that REMOVES a partition of a table, which is what the metastore client
/// does for a statement that drops one. SOURCE: as [`GET_DATABASE_ACTION`].
pub const BATCH_DELETE_PARTITION_ACTION: &str = "glue:BatchDeletePartition";

/// The resource portion of the ARN of the catalog itself.
const CATALOG_RESOURCE: &str = "catalog";

/// The resource type a Glue database is addressed by in an ARN.
const DATABASE_RESOURCE_TYPE: &str = "database";

/// The resource type a Glue table is addressed by in an ARN.
const TABLE_RESOURCE_TYPE: &str = "table";

/// The error code AWS Glue refuses a call the caller's policies do not allow with, which is
/// what the cluster's metastore client reports to the engine that asked.
pub const ACCESS_DENIED_CODE: &str = "AccessDeniedException";

/// `FailureDetails$Reason` for a step whose metastore access the permissions in force on the
/// role it runs as refused, beside the catalog's own message for it
/// ([`access_denied_message`]).
pub const CATALOG_ACCESS_DENIED_REASON: &str = "The metastore refused the step's catalog access.";

/// How many `(action, resource)` decisions ONE step's work asks for. CHOSEN: a bound on the
/// IAM hops one step can cost, above the pairs a realistic statement asks about by a wide
/// margin.
const MAX_DECISIONS_PER_STEP: usize = 64;

/// How many steps' catalog access ONE call decides, leaving the rest to the next call that
/// reports the cluster. CHOSEN: the ceiling on what one REQUEST costs, in the units the
/// command queue is bounded in ([`MAX_COMMANDS_RESOLVED_PER_CALL`]).
const MAX_STEPS_DECIDED_PER_CALL: usize = 32;

/// What the permissions in force on the role a step runs as say about the Glue Data Catalog
/// calls its work makes.
#[derive(Debug, Clone)]
pub enum CatalogAccess {
    /// Nothing in them denies those calls, so the work makes them.
    Granted,
    /// One of them is denied: the metastore client's own error for that refusal, which is what
    /// the step FAILS with.
    Refused(StepFailure),
}

/// The ARN of this account's Glue Data Catalog in `region`.
fn catalog_arn(region: &str) -> String {
    ::engine::emulator_arn(GLUE, region, CATALOG_RESOURCE)
}

/// The ARNs a call on the database `database` is authorized on: the catalog, and the database
/// itself — Glue publishes a database as `database/<name>` under the catalog's own ARN, and
/// names both as resources of the call that reads it.
fn database_arns(region: &str, database: &str) -> Vec<String> {
    vec![
        catalog_arn(region),
        ::engine::emulator_arn(GLUE, region, &format!("{DATABASE_RESOURCE_TYPE}/{database}")),
    ]
}

/// The ARNs a call on one table is authorized on: the catalog, the database that holds it, and
/// the table itself (`table/<database>/<name>`).
fn table_arns(region: &str, table: &TableReference) -> Vec<String> {
    let mut arns = database_arns(region, &table.database);
    arns.push(::engine::emulator_arn(
        GLUE,
        region,
        &format!("{TABLE_RESOURCE_TYPE}/{}/{}", table.database, table.name),
    ));
    arns
}

/// Add one action on each of `resources`, skipping a pair already asked about: a statement
/// naming one table twice asks about it once.
fn push(asked: &mut Vec<(String, String)>, action: &str, resources: Vec<String>) {
    for resource in resources {
        take(asked, (action.to_string(), resource));
    }
}

/// Keep one `(action, resource)` pair, unless it is already there or this step has asked for
/// as many decisions as one step may ([`MAX_DECISIONS_PER_STEP`]).
fn take(asked: &mut Vec<(String, String)>, pair: (String, String)) {
    if !asked.contains(&pair) && asked.len() < MAX_DECISIONS_PER_STEP {
        asked.push(pair);
    }
}

/// The `(action, resource)` pairs the metastore client performs to RESOLVE one table, in the
/// order it performs them: the database the name qualifies in, then the table in it.
fn resolving(region: &str, table: &TableReference) -> Vec<(String, String)> {
    let mut asked: Vec<(String, String)> = Vec::new();
    push(
        &mut asked,
        GET_DATABASE_ACTION,
        database_arns(region, &table.database),
    );
    push(&mut asked, GET_TABLE_ACTION, table_arns(region, table));
    asked
}

/// The pairs applying one metastore CHANGE performs: resolving the table it names, and then
/// the write that change is — registering the partitions it adds, or removing the one it
/// drops.
fn changing(region: &str, change: &MetastoreChange) -> Vec<(String, String)> {
    let (table, written) = match change {
        MetastoreChange::AddPartitions { table, .. } => (table, BATCH_CREATE_PARTITION_ACTION),
        MetastoreChange::DropPartition { table, .. } => (table, BATCH_DELETE_PARTITION_ACTION),
    };
    let mut asked = resolving(region, table);
    push(&mut asked, written, table_arns(region, table));
    asked
}

/// Every pair the work of one step performs against the catalog: resolving every table it
/// reads, and applying every change it makes.
fn asked_by(region: &str, work: &StepWork) -> Vec<(String, String)> {
    let mut asked: Vec<(String, String)> = Vec::new();
    let performed = work
        .tables
        .iter()
        .map(|table| resolving(region, table))
        .chain(work.changes.iter().map(|change| changing(region, change)));
    for pairs in performed {
        for pair in pairs {
            take(&mut asked, pair);
        }
    }
    asked
}

/// The error the cluster's metastore client reports when the catalog refused a call the role
/// the work runs as may not make.
///
/// Glue answers such a call with [`ACCESS_DENIED_CODE`] carrying IAM's own sentence about the
/// refusal — who asked, which action, which resource, and which of IAM's ways of reaching a
/// deny refused it — and that sentence is the shared one
/// ([`::service_common::iam_authorization`]), so this reads exactly as every other refusal of
/// this tree words the same decision. The principal is the SESSION the work's calls are made
/// as: a role is never acted as directly, so the identity a refusal is about is a session of
/// it ([`::service_common::iam_role::assumed_role_session_arn`]), opened under the step's own
/// id, which is the session name the step's calls present
/// ([`super::step_command_transfers`]).
#[must_use]
pub fn access_denied_message(
    role_arn: &str,
    session_name: &str,
    action: &str,
    resource: &str,
) -> String {
    let principal = ::service_common::iam_role::assumed_role_session_arn(role_arn, session_name);
    let refusal = ::service_common::iam_authorization::not_authorized_by_identity_policy(
        &principal,
        action,
        resource,
        ::service_common::iam_authorization::IDENTITY_POLICY_EXPLICIT_DENY,
    );
    format!("{ACCESS_DENIED_CODE}: {refusal}")
}

/// The refusal the pair `(action, resource)` is, as the metastore client raises it.
fn refusal(role_arn: &str, session_name: &str, action: &str, resource: &str) -> StepFailure {
    StepFailure {
        reason: CATALOG_ACCESS_DENIED_REASON.to_string(),
        message: access_denied_message(role_arn, session_name, action, resource),
    }
}

/// What IAM answers about the catalog calls the work of ONE step makes, asked of the role that
/// work runs as.
///
/// The pairs are asked in the order the metastore client performs them and the FIRST denied
/// one is the answer, because that is where the client stops. A policy set that allows them or
/// merely says nothing about them grants the access, for the reason the module docs give.
pub async fn work_catalog_access(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    session_name: &str,
    work: &StepWork,
) -> CatalogAccess {
    for (action, resource) in asked_by(region, work) {
        if ::service_common::iam_role::role_denies_action(ctx, region, role_arn, &action, &resource)
            .await
        {
            return CatalogAccess::Refused(refusal(role_arn, session_name, &action, &resource));
        }
    }
    CatalogAccess::Granted
}

/// Decide the catalog access of every step of the cluster `cluster_id` whose BARE statement is
/// owed one, and record what IAM answered.
///
/// Called by the operations that report a step or its cluster, BEFORE this service's store is
/// taken and never with it held: the decision is an IAM call, which may not happen under this
/// service's lock. The store is taken only to read the steps owed a decision and to write what
/// was answered, and nothing is answered at all for a cluster id this account holds no cluster
/// under.
///
/// Every answer this records was owed whether or not anybody called: the work's catalog calls
/// belong to the step's own execution. What ONE call decides is bounded
/// ([`MAX_STEPS_DECIDED_PER_CALL`]); a step whose turn did not come stays owed its decision and
/// the next call gives it.
pub async fn resolve_due_step_catalog_access<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_id: &str,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // Which steps are owed a decision, what each one's work reaches, and the value that names
    // the role it runs as: its own runtime role where it carries one, and otherwise the
    // instance profile its cluster was launched with.
    let owed: Vec<(String, String, StepWork)> = {
        let store = state.lock();
        let Some(cluster) = store.clusters.get(cluster_id) else {
            return;
        };
        let launched_as = cluster.job_flow_role.clone().unwrap_or_default();
        cluster
            .steps
            .iter()
            .filter(|step| step.awaits_catalog_access())
            .take(MAX_STEPS_DECIDED_PER_CALL)
            .filter_map(|step| {
                Some((
                    step.id.clone(),
                    step.execution_role_arn
                        .clone()
                        .unwrap_or_else(|| launched_as.clone()),
                    step.work.clone()?,
                ))
            })
            .collect()
    };
    for (step_id, named, work) in owed {
        // The ROLE whose permissions the calls are decided against, resolved out of the value
        // that named it. Nothing resolved is nothing known about the permissions in force,
        // which withholds no call (see the module docs).
        let answered = match work_role_arn(ctx, region, &named).await {
            Some(role_arn) => work_catalog_access(ctx, region, &role_arn, &step_id, &work).await,
            None => CatalogAccess::Granted,
        };
        let mut store = state.lock();
        if let Some(step) = store
            .clusters
            .get_mut(cluster_id)
            .and_then(|cluster| cluster.step_mut(&step_id))
        {
            // Only a step that STILL awaits a decision takes this one: in the window the guard
            // was released another call may have answered it, or the caller may have cancelled
            // the step or its cluster.
            if step.awaits_catalog_access() {
                step.catalog_access = Some(answered);
            }
        }
    }
}

/// The catalog access of one command LINE, decided table by table as its SQL client resolves
/// them.
///
/// Built per command ([`step_catalog_access`]), which is what gives each command its own
/// deciding budget; the decisions it has already asked for are kept, so a command line naming
/// one table in several statements costs one hop.
#[derive(Clone)]
pub struct StepCatalogAccess {
    /// The in-process callback the decision is asked through.
    callback: EmbeddedEmulator,
    /// The async runtime that hop is driven on. A step's command is performed on a thread of
    /// its own, which is not one of this runtime's workers, so the call is made by blocking
    /// that thread on this handle rather than by making the session asynchronous.
    runtime: ::tokio::runtime::Handle,
    /// The Region the decision is asked in, which is the one the request that performs the
    /// step was addressed to.
    region: String,
    /// The role the work runs as, whose permissions decide the calls.
    role_arn: String,
    /// The session name the work's calls present, which is the step's own id.
    session_name: String,
    /// What IAM has already answered about a pair, so one command asks once.
    decided: Arc<Mutex<BTreeMap<(String, String), bool>>>,
    /// When this command's budget for the hops runs out: a span of its own, of the size one
    /// command's transfers are given ([`TRANSFER_BUDGET`]), so a command line cannot hold the
    /// operation performing it for longer however many tables it names.
    deadline: ::std::time::Instant,
}

/// Written by hand rather than derived: the environment the client is a program of requires it
/// and the callback carries no `Debug`. It names no field, because neither the callback nor the
/// role the work runs as belongs in a log.
impl ::std::fmt::Debug for StepCatalogAccess {
    fn fmt(&self, formatter: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        formatter.debug_struct("StepCatalogAccess").finish()
    }
}

/// The decision one command of the step `step_id` gets, for the work it performs as the role
/// `role_arn` — `None` where there is no async runtime to ask IAM on, in which case nothing is
/// known about the permissions in force and no call is withheld.
#[must_use]
pub fn step_catalog_access(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    role_arn: &str,
    step_id: &str,
) -> Option<StepCatalogAccess> {
    Some(StepCatalogAccess {
        callback: ctx.callback().clone(),
        runtime: ::tokio::runtime::Handle::try_current().ok()?,
        region: region.to_string(),
        role_arn: role_arn.to_string(),
        session_name: step_id.to_string(),
        decided: Arc::new(Mutex::new(BTreeMap::new())),
        deadline: ::std::time::Instant::now() + TRANSFER_BUDGET,
    })
}

impl StepCatalogAccess {
    /// The same decision with its budget BEGUN AGAIN, for the reason the object program's own
    /// is ([`StepObjectCli::renewed`]): each piece of a step's work is decided inside the
    /// budget one piece gets. What has already been DECIDED is carried over, so a pair this
    /// role has been answered about is not asked about twice.
    pub(super) fn renewed(&self) -> Self {
        Self {
            deadline: ::std::time::Instant::now() + TRANSFER_BUDGET,
            ..self.clone()
        }
    }

    /// The refusal RESOLVING `table` answers, or `None` when nothing in the role's permissions
    /// denies it.
    #[must_use]
    pub fn table_refusal(&self, table: &TableReference) -> Option<StepFailure> {
        self.first_refusal(resolving(&self.region, table))
    }

    /// The refusal APPLYING `change` answers, or `None` when nothing denies it.
    #[must_use]
    pub fn change_refusal(&self, change: &MetastoreChange) -> Option<StepFailure> {
        self.first_refusal(changing(&self.region, change))
    }

    /// The first pair of `asked` the role's permissions deny, as the metastore client's own
    /// error.
    fn first_refusal(&self, asked: Vec<(String, String)>) -> Option<StepFailure> {
        asked.into_iter().find_map(|(action, resource)| {
            self.denies(&action, &resource)
                .then(|| refusal(&self.role_arn, &self.session_name, &action, &resource))
        })
    }

    /// Whether IAM ANSWERED that the role's permissions deny `action` on `resource`, asked once
    /// per pair for one command.
    ///
    /// The hop is driven by blocking this command's thread on the runtime handle, inside what
    /// this command has left of its deciding budget; a budget already spent is no answer and
    /// withholds nothing, as every other non-answer here does.
    fn denies(&self, action: &str, resource: &str) -> bool {
        let pair = (action.to_string(), resource.to_string());
        if let Some(answered) = self
            .decided
            .lock()
            .unwrap_or_else(PoisonError::into_inner)
            .get(&pair)
        {
            return *answered;
        }
        let Some(left) = self
            .deadline
            .checked_duration_since(::std::time::Instant::now())
            .filter(|left| !left.is_zero())
        else {
            return false;
        };
        let answered = self.runtime.block_on(async {
            ::tokio::time::timeout(
                left,
                ::service_common::iam_role::role_denies_action_through(
                    &self.callback,
                    &self.region,
                    &self.role_arn,
                    action,
                    resource,
                ),
            )
            .await
            .unwrap_or(false)
        });
        let mut decided = self.decided.lock().unwrap_or_else(PoisonError::into_inner);
        if decided.len() < MAX_DECISIONS_PER_STEP {
            decided.insert(pair, answered);
        }
        answered
    }
}
