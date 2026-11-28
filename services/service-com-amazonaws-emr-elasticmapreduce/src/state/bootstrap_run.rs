// SPDX-License-Identifier: CC0-1.0

//! RUNNING a cluster's BOOTSTRAP ACTIONS: fetching each action's script out of Amazon S3 and
//! performing it before the cluster reports itself ready.
//!
//! # The rule this module implements
//!
//! "A list of bootstrap actions to run before Hadoop starts on the cluster nodes"
//! (`RunJobFlow$BootstrapActions` doc), each one "the script run by the bootstrap action" with
//! "[the] location in Amazon S3 of the script to run during a bootstrap action"
//! (`BootstrapActionConfig$ScriptBootstrapAction`, `ScriptBootstrapActionConfig$Path` docs). So
//! a launch that carried bootstrap actions does not merely RECORD them: the cluster occupies
//! `BOOTSTRAPPING` — the state between its capacity being provisioned and its being ready —
//! and in it each action's script is fetched from the location its `Path` names and PERFORMED,
//! in launch order, so the script's own effects are in place by the time the cluster reports
//! `RUNNING`/`WAITING` and a step of it can run. A cluster whose launch carried none is owed
//! nothing here and comes up as it always did.
//!
//! The actions run on the cluster NODES, so each one is performed ONCE ON EVERY NODE the launch
//! provisioned (primary, core and task alike), as a program of that node: what it reads of the
//! node it runs on, and what the run leaves behind, are that node's ([`super::bootstrap_nodes`]).
//! A script that writes one object per node therefore leaves as many objects as the cluster has
//! nodes, and the cluster is ready only once every node has answered for every action.
//!
//! The outcome is the script's own: a script that exited zero is a bootstrap action that
//! succeeded, and one that exited anything else takes the CLUSTER down — it reaches
//! `TERMINATED_WITH_ERRORS` through `TERMINATING`, carrying the modelled
//! `BOOTSTRAP_FAILURE` reason ([`ClusterRecord::begin_bootstrap_failure`]) — rather than
//! leaving the cluster to come up as though the action had run. No later action of that
//! cluster runs: the nodes the actions were running on are going away. The two outcomes that
//! fail a cluster are exactly the two Amazon EMR words a reason for
//! ([`BootstrapActionRun::failed`]): a script that really exited non-zero, and one whose object
//! the node could not fetch.
//!
//! # What a script reaches
//!
//! The SAME bounded session a `command-runner.jar` step's command line reaches
//! ([`::service_common::posix_shell_session`], whose catalog of session-local utilities and
//! whose bounds are stated there), with two programs its environment provides: the NODE's own
//! instance metadata client ([`super::bootstrap_nodes`]) and the AWS CLI's Amazon S3 transfers
//! ([`super::step_command_transfers`]) — really made against this emulator's own object store,
//! signed as the role the cluster's Amazon EC2 instances obtain from their instance profile
//! ([`super::instance_profile_credentials`]), which is the credential a bootstrap action runs
//! with. A script is performed as a SCRIPT rather than as one command line, so the session is
//! given its own working tree ([`ShellSession::with_file_utilities`]) and with it the grammar's
//! control flow and the file utilities a script is written in, and the file Amazon EMR
//! publishes the node's role in ([`INSTANCE_INFO_PATH`]) is seeded into that tree. That tree
//! STARTS with what the node's own local disk holds and is kept beside the node afterwards
//! ([`super::node_file_systems`]), because a node has storage that outlives the one program
//! that wrote to it: what one action leaves there the next action, and the steps that run after
//! the bootstrap phase, read. The tree is
//! service-held memory, keyed by the path the script named and bounded by that module: no
//! host file, directory or path is opened, created or read. NO host command, process or shell
//! is started here or anywhere this calls, and a command the session provides no program for
//! answers the status a shell gives a command it could not find.
//!
//! A script the session performed NOTHING of is reported as exactly that
//! ([`BootstrapActionAnswer::NotPerformed`]) and fails no cluster: nothing ran, so there is no
//! status the script exited with and no failure of the script to blame. Such a cluster stays
//! short of ready — it reports neither a success for a script nothing performed nor a
//! `BOOTSTRAP_FAILURE` Amazon EMR has no wording for.
//!
//! "A list of command line arguments to pass to the bootstrap action script"
//! (`ScriptBootstrapActionConfig$Args` doc) is persisted and reported by
//! `ListBootstrapActions` ([`super::bootstrap_actions`]); the session resolves no POSITIONAL
//! parameter, so a script that reads one reads an empty value — the action still runs, and
//! widening that session is its own change rather than this one.
//!
//! # When a script runs, and what one call may spend
//!
//! An action is owed its run from the instant its cluster entered `BOOTSTRAPPING` — the phase
//! the clock owes a launch ([`ClusterRecord::advance_to`]) — and the run is performed by the
//! next operation that reports that cluster, with no store guard held, exactly as a step's
//! command is ([`super::command_step_run`]). A cluster whose actions have not all run on all of
//! its nodes does not become ready ([`ClusterRecord::bootstrap_actions_have_run`]): its
//! bootstrap actions run "before Hadoop starts on the cluster nodes", so it stays
//! `BOOTSTRAPPING` until each has answered rather than reporting itself ready for a script
//! nothing has performed.
//!
//! Where one runs — the thread, the claim that keeps one run of one action on one node
//! performed once however many callers report its cluster, and the permit — is
//! [`super::performed_programs`]'s. What ONE call spends is bounded here
//! ([`MAX_BOOTSTRAP_ACTION_RUNS_PER_CALL`], [`BOOTSTRAP_CALL_CEILING`]); a run whose turn did
//! not come stays owed and the next call performs it.

use ::service_common::posix_shell_session::{
    layered, EnvironmentPrograms, SessionConfig, ShellSession,
};

use super::*;

/// How large a bootstrap script this emulator reads.
///
/// CHOSEN, not an AWS limit: the script is an object a caller uploaded, so what one run takes
/// into this process is bounded here rather than by the object's own size. It is enforced by
/// the RANGED read, so nothing larger is ever buffered, and a script that is not text is one
/// no session can read.
pub const MAX_BOOTSTRAP_SCRIPT_BYTES: usize = 256 * 1024;

/// How many bootstrap-action RUNS one call performs, leaving the rest to the next call that
/// reports the cluster.
///
/// CHOSEN as the ceiling Amazon EMR puts on one cluster's actions — "you can specify up to
/// 16 bootstrap actions per cluster" (Amazon EMR Management Guide, "Create bootstrap actions to
/// install additional software") — so a cluster whose launch carried the most actions it may
/// settles its first node's actions in one call; the runs owed on its other nodes stay owed to
/// the next call.
pub const MAX_BOOTSTRAP_ACTION_RUNS_PER_CALL: usize = 16;

/// How long ONE call spends performing owed bootstrap actions before it leaves the rest to the
/// next call.
///
/// CHOSEN, the sibling of the step-command queue's own ceiling
/// ([`super::command_step_run::COMMAND_CALL_CEILING`]): the ceiling on what a REQUEST costs,
/// beside the ceiling on what one script's transfers cost. A further action is performed only
/// while a whole one still fits inside this span.
pub const BOOTSTRAP_CALL_CEILING: Duration = Duration::from_secs(8);

/// The thread one bootstrap action's script is performed on
/// ([`performed_on_its_own_thread`]).
const BOOTSTRAP_THREAD_NAME: &str = "emr-bootstrap-action";

/// What one cluster's bootstrap PHASE is claimed under ([`claimed_program`]): the key
/// `<cluster-id>/<this>`, which the key of one RUN of that phase extends with the action's
/// position and the node it is performed on.
const BOOTSTRAP_PHASE_CLAIM: &str = "bootstrap";

/// How much of what a bootstrap action's script WROTE the session keeps.
///
/// CHOSEN, the sibling of the step command's own limits
/// ([`super::step_command_status`]): the two console streams of a script reach the log files
/// the run leaves on its node, gzipped under its cluster's `LogUri` ([`super::node_logs`]),
/// and nothing else — so what is kept is bounded here as well as where the file is written.
const BOOTSTRAP_OUTPUT_LIMITS: ::service_common::posix_shell_session::OutputLimits =
    ::service_common::posix_shell_session::OutputLimits {
        lines: 256,
        line_bytes: 4 * 1024,
    };

/// What ONE bootstrap action of a cluster came to, on ONE of its nodes.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BootstrapActionAnswer {
    /// The script RAN: the status it exited with, which is what the action came to.
    Exited(i32),
    /// Its script was not fetched from the location the action names, so there was nothing to
    /// run; the detail is what the object store answered about it.
    Unfetched(String),
    /// The session performed nothing of the script at all, so nothing is known about what it
    /// would have come to — neither a success nor a status the script exited with.
    NotPerformed,
}

/// One bootstrap action this cluster has ANSWERED for on one of its nodes, kept in the order
/// the runs were answered for.
#[derive(Debug, Clone)]
pub struct BootstrapActionRun {
    /// Which action of the launch's list this is, counted from one — the number Amazon EMR
    /// names a bootstrap action by when it reports one failing.
    pub position: usize,
    /// Where its script was fetched from, as the launch wrote it.
    pub path: String,
    /// WHICH NODE it ran on ([`BootstrapNode::key`]): an action runs once on every node the
    /// launch provisioned, so a run is of one action on one node.
    pub node: String,
    /// What it came to.
    pub answer: BootstrapActionAnswer,
    /// WHEN it was answered for, which is the instant its own log lines carry
    /// ([`super::node_logs`]).
    pub at: SystemTime,
    /// What the script WROTE to its output streams while it ran, and the status it exited
    /// with — the text of the log files this run is owed under its cluster's `LogUri`
    /// ([`super::node_logs`]). `None` for a run nothing performed: there was no process, so
    /// there is no stream and no status.
    pub log: Option<StepRunLog>,
    /// Whether those files have been written, which is what keeps one run's directory
    /// written once however many callers report its cluster afterwards.
    pub log_delivered: bool,
}

impl BootstrapActionRun {
    /// Whether this run succeeded: its script ran and exited zero, which is what lets the
    /// cluster go on coming up.
    #[must_use]
    pub fn succeeded(&self) -> bool {
        self.answer == BootstrapActionAnswer::Exited(0)
    }

    /// Whether this run FAILED THE CLUSTER: the script really ran and exited non-zero, or the
    /// node could not fetch the object the action names. Those are the two outcomes Amazon EMR
    /// words a `BOOTSTRAP_FAILURE` for.
    ///
    /// A run the session performed nothing of is neither: nothing ran, so there is no status to
    /// report and no failure of the script to blame — such a cluster is owed no shutdown here
    /// ([`BootstrapActionAnswer::NotPerformed`]).
    #[must_use]
    pub fn failed(&self) -> bool {
        match &self.answer {
            BootstrapActionAnswer::Exited(status) => *status != 0,
            BootstrapActionAnswer::Unfetched(_) => true,
            BootstrapActionAnswer::NotPerformed => false,
        }
    }

    /// `ClusterStateChangeReason$Message` for the shutdown this action brings, naming the node
    /// it ran on where the cluster holds one.
    ///
    /// Called only for a run that FAILED the cluster ([`Self::failed`]), which is why there is
    /// no wording here for a script nothing performed: Amazon EMR has no message for that
    /// outcome, because it has no such outcome.
    #[must_use]
    pub fn failure_message(&self, instance_id: Option<&str>) -> String {
        let position = self.position;
        match &self.answer {
            // RECORDED/published Amazon EMR wording for a bootstrap action that ran and
            // exited non-zero, which names the node it ran on and the action's position.
            BootstrapActionAnswer::Exited(_) => match instance_id {
                Some(id) => format!(
                    "On the master instance ({id}), bootstrap action {position} returned a \
                     non-zero return code"
                ),
                None => {
                    format!("Bootstrap action {position} returned a non-zero return code")
                }
            },
            // The same for an action whose script the node could not fetch.
            BootstrapActionAnswer::Unfetched(_) => match instance_id {
                Some(id) => format!(
                    "Master instance ({id}) failed attempting to download bootstrap action \
                     {position} file from S3"
                ),
                None => format!(
                    "Failed attempting to download bootstrap action {position} file from S3"
                ),
            },
            // Never published: such a run fails no cluster ([`Self::failed`]).
            BootstrapActionAnswer::NotPerformed => String::new(),
        }
    }
}

/// One bootstrap action this service owes a run, on one node.
#[derive(Debug, Clone)]
struct BootstrapActionPlan {
    /// Its position in the launch's list, counted from one.
    position: usize,
    /// The Amazon S3 location of its script, as the launch wrote it.
    path: String,
    /// The node it is owed its run on.
    node: BootstrapNode,
}

impl ClusterRecord {
    /// Every bootstrap action of this cluster that is owed a run, in launch order: at most one
    /// per node, since a node runs its actions in launch order and one at a time.
    ///
    /// A cluster owes them only while it is BOOTSTRAPPING — before that its capacity is not up
    /// and after it the phase is over — and only while every run answered so far succeeded: one
    /// that failed is taking the cluster down, so the actions behind it never run, and one the
    /// session performed nothing of is an answer no later run of the same script changes.
    fn owed_bootstrap_actions(&self, region: &str) -> Vec<BootstrapActionPlan> {
        if self.status.state != cluster_state::BOOTSTRAPPING {
            return Vec::new();
        }
        if self.bootstrap_runs.iter().any(|run| !run.succeeded()) {
            return Vec::new();
        }
        let mut owed: Vec<BootstrapActionPlan> = Vec::new();
        for node in self.bootstrap_nodes(region) {
            let key = node.key();
            // The first action this node has not answered for yet: the ones before it have
            // succeeded on this node, so this is the one whose turn it is here.
            let next = self
                .bootstrap_actions
                .iter()
                .enumerate()
                .find(|(index, _)| !self.answered_bootstrap_run(index + 1, &key));
            if let Some((index, config)) = next {
                owed.push(BootstrapActionPlan {
                    position: index + 1,
                    path: config.script_bootstrap_action.path.clone(),
                    node,
                });
            }
        }
        owed
    }

    /// Whether the action at `position` has already been answered for on the node `node`
    /// names, which is what keeps one action performed once per node.
    fn answered_bootstrap_run(&self, position: usize, node: &str) -> bool {
        self.bootstrap_runs
            .iter()
            .any(|run| run.position == position && run.node == node)
    }

    /// Whether every bootstrap action this cluster's launch carried has run and succeeded ON
    /// EVERY NODE of it, which is what lets the cluster report itself ready.
    ///
    /// The actions run "before Hadoop starts on the cluster nodes"
    /// (`RunJobFlow$BootstrapActions` doc) — on the nodes, so a node that has not answered for
    /// one of them holds the cluster in its bootstrap phase exactly as an unanswered action
    /// does.
    #[must_use]
    pub fn bootstrap_actions_have_run(&self) -> bool {
        if self.bootstrap_actions.is_empty() {
            return true;
        }
        if self.bootstrap_runs.iter().any(|run| !run.succeeded()) {
            return false;
        }
        self.bootstrap_node_keys().iter().all(|key| {
            (1..=self.bootstrap_actions.len())
                .all(|position| self.answered_bootstrap_run(position, key))
        })
    }

    /// The run that FAILED the cluster, once one has ([`BootstrapActionRun::failed`]).
    fn failed_bootstrap_action(&self) -> Option<&BootstrapActionRun> {
        self.bootstrap_runs.iter().find(|run| run.failed())
    }

    /// Record what one bootstrap action came to on one node.
    ///
    /// Takes the answer only for a run of that action on that node which is not already
    /// recorded, and only once every action BEFORE it has succeeded there — so a run another
    /// call recorded in the window this one's guard was released is not recorded twice, and no
    /// action of a node is answered for out of order.
    pub(crate) fn record_bootstrap_action_run(&mut self, run: BootstrapActionRun) {
        if self.answered_bootstrap_run(run.position, &run.node) {
            return;
        }
        let in_order = (1..run.position).all(|position| {
            self.bootstrap_runs
                .iter()
                .any(|held| held.position == position && held.node == run.node && held.succeeded())
        });
        if in_order {
            self.bootstrap_runs.push(run);
        }
    }

    /// Start the shutdown a FAILED bootstrap action brings, answering whether it was started.
    ///
    /// A bootstrap failure is a failure of the CLUSTER: its nodes could not be brought up the
    /// way the launch asked, so the cluster shuts down carrying the modelled reason for it
    /// (`ClusterStateChangeReasonCode` `BOOTSTRAP_FAILURE`) and the sentence Amazon EMR words
    /// that reason with, and the terminal state it reaches is `TERMINATED_WITH_ERRORS`
    /// ([`terminal_cluster_state`]) rather than `TERMINATED`.
    ///
    /// Answers `false` without touching the cluster when no action of it failed, when it is
    /// already shutting down or gone, and when the caller holds it under termination
    /// protection — which "prevents the Amazon EC2 instances from being terminated"
    /// (`Cluster$TerminationProtected` doc), so a cluster launched under it keeps the nodes a
    /// caller asked to keep rather than losing them to this shutdown.
    pub(crate) fn begin_bootstrap_failure(&mut self, now: SystemTime) -> bool {
        if self.status.state != cluster_state::BOOTSTRAPPING || self.termination_protection_holds()
        {
            return false;
        }
        let instance_id = self.master_ec2_instance_id().map(str::to_string);
        let Some(message) = self
            .failed_bootstrap_action()
            .map(|run| run.failure_message(instance_id.as_deref()))
        else {
            return false;
        };
        self.status.state = cluster_state::TERMINATING.to_string();
        self.status.state_change_reason = Some(StateChangeReasonRecord {
            code: cluster_state_change_reason_code::BOOTSTRAP_FAILURE.to_string(),
            message: Some(message),
        });
        self.status.bootstrapping_due_at = None;
        self.status.ready_due_at = None;
        let terminated_at = now + TRANSITION_SPAN;
        self.status.terminated_due_at = Some(terminated_at);
        // The cluster takes its step queue with it, exactly as a caller's own shutdown does
        // ([`ClusterRecord::begin_termination`]): no step stays `PENDING` for a cluster whose
        // nodes are going away.
        for step in &mut self.steps {
            step.stop_with_cluster(terminated_at);
        }
        true
    }

    /// The Amazon EC2 instance id of this cluster's primary node, where Amazon EC2 launched
    /// one for it.
    ///
    /// "Valid values are MASTER, CORE or TASK" (`InstanceGroupConfig$InstanceRole` doc) and the
    /// fleet vocabulary spells the primary role the same way (`InstanceFleetType`), so the
    /// primary node is an instance of whichever collection fills that role. It is named by the
    /// id Amazon EC2 minted for it (`Instance$Ec2InstanceId`), which is the node a caller sees
    /// everywhere else. A cluster whose instances this emulator did not launch holds none, and
    /// then nothing is named.
    #[must_use]
    pub fn master_ec2_instance_id(&self) -> Option<&str> {
        let primary_groups: Vec<&str> = self
            .groups
            .iter()
            .filter(|group| group.config.instance_role == instance_group_type::MASTER)
            .map(|group| group.id.as_str())
            .collect();
        let primary_fleets: Vec<&str> = self
            .fleets
            .iter()
            .filter(|fleet| fleet.config.instance_fleet_type == instance_group_type::MASTER)
            .map(|fleet| fleet.id.as_str())
            .collect();
        self.ec2_instances
            .iter()
            .find(|instance| {
                instance
                    .instance_group_id
                    .as_deref()
                    .is_some_and(|id| primary_groups.contains(&id))
                    || instance
                        .instance_fleet_id
                        .as_deref()
                        .is_some_and(|id| primary_fleets.contains(&id))
            })
            .map(|instance| instance.ec2_instance_id.as_str())
    }
}

/// Perform every bootstrap action the cluster `cluster_id` already owes, and record what each
/// came to.
///
/// Called by the operations that report a cluster, BEFORE they take this service's store: the
/// run was owed the instant the cluster entered `BOOTSTRAPPING`, whoever asks about it next,
/// and neither the Amazon S3 read nor the session that performs the script may happen under
/// this service's lock. The store is taken only to read the owed actions and to write what each
/// came to, and this performs nothing at all for a cluster id this account holds none under.
///
/// Every transition this leads to was owed whether or not anybody called — the cluster's
/// bootstrap phase began at an instant the clock decided — so a reporting operation may apply
/// it: what it adds is the action's own answer, which is what the cluster then comes up or goes
/// down on.
pub async fn run_due_bootstrap_actions<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    cluster_id: &str,
    now: SystemTime,
) {
    let Some(state) = service_state(engine) else {
        return;
    };
    // ONE driver performs a cluster's bootstrap PHASE at a time ([`claimed_program`], the same
    // claim an individual run is taken under), so what the phase comes to does not depend on how
    // many of them arrived at it. The phase is a SEQUENCE and not a set of independent runs: an
    // action that did not succeed takes the cluster down, and the nodes the actions behind it
    // would have run on are going away with it — so a second driver performing another node's
    // turn beside this one would run a script on a node this phase had already stopped for. A
    // driver that finds the phase claimed leaves it to the one holding the claim, exactly as it
    // leaves an individual run it could not claim.
    let Some(_phase) = claimed_program(&format!("{cluster_id}/{BOOTSTRAP_PHASE_CLAIM}")) else {
        return;
    };
    let job_flow_role: Option<String> = {
        let mut store = state.lock();
        // The phases the clock owes are applied first, so a cluster whose capacity is up is
        // BOOTSTRAPPING and its actions are among the owed ones.
        store
            .cluster_mut(cluster_id, now)
            .filter(|cluster| !cluster.owed_bootstrap_actions(region).is_empty())
            .map(|cluster| cluster.job_flow_role.clone().unwrap_or_default())
    };
    let Some(job_flow_role) = job_flow_role else {
        return;
    };
    // What the cluster's Amazon EC2 instances obtain from their instance profile, resolved once
    // for this call: the credential a bootstrap action's own calls present, which is nothing at
    // all where that profile carries no role those instances may assume.
    let credentials = cluster_step_credentials(ctx, region, &job_flow_role).await;
    // What this CALL may spend performing them: a further run is started only while a whole one
    // still fits, so the rest stay owed to the next call.
    let started = ::std::time::Instant::now();
    let mut spent = 0_usize;
    // The actions of one node run in launch order, so each pass takes the run whose turn it is
    // on every node and the next pass takes the action behind it.
    loop {
        let owed: Vec<BootstrapActionPlan> = {
            let mut store = state.lock();
            match store.cluster_mut(cluster_id, now) {
                Some(cluster) => cluster.owed_bootstrap_actions(region),
                None => Vec::new(),
            }
        };
        if owed.is_empty() {
            return;
        }
        // Whether this pass PERFORMED any of them: a pass that performed none — every owed run
        // is being performed by another call that holds its claim — leaves them to that call
        // rather than asking for the same set again.
        let mut performed_any = false;
        for plan in owed {
            if spent >= MAX_BOOTSTRAP_ACTION_RUNS_PER_CALL
                || started.elapsed().saturating_add(TRANSFER_BUDGET) > BOOTSTRAP_CALL_CEILING
            {
                return;
            }
            // One run is performed at a time, so a run another call is still performing —
            // including one that call has already given up waiting for — is not performed a
            // second time: a script's transfers are real effects on the object store.
            let node = plan.node.key();
            let Some(claim) = claimed_program(&format!(
                "{cluster_id}/{BOOTSTRAP_PHASE_CLAIM}/{}/{node}",
                plan.position
            )) else {
                continue;
            };
            spent = spent.saturating_add(1);
            performed_any = true;
            // What this node's local disk already holds, which is what the script finds on it:
            // the files the actions BEFORE this one left there.
            let disk = {
                let store = state.lock();
                store
                    .clusters
                    .get(cluster_id)
                    .map(|cluster| cluster.node_disk(&node))
                    .unwrap_or_default()
            };
            let performed =
                performed_bootstrap_action(engine, ctx, region, &plan, &credentials, claim, disk)
                    .await;
            let Some(performed) = performed else {
                // Nothing answered: the run stays owed, so a later call performs it rather than
                // this one recording an answer nothing produced.
                continue;
            };
            let answer = performed.answer;
            let kept = performed.kept;
            let settled = answer != BootstrapActionAnswer::Exited(0);
            {
                let mut store = state.lock();
                if let Some(cluster) = store.clusters.get_mut(cluster_id) {
                    // What the script left on the node's local disk is the node's from now on,
                    // which is what the step that runs after the bootstrap phase reads.
                    cluster.keep_node_disk(&node, kept);
                    cluster.record_bootstrap_action_run(BootstrapActionRun {
                        position: plan.position,
                        path: plan.path.clone(),
                        node,
                        answer,
                        at: now,
                        log: performed.log,
                        log_delivered: false,
                    });
                    // The cluster can now reach what that answer leads to — ready once every
                    // action has succeeded on every node, or the shutdown a failure brings.
                    cluster.advance_to(now);
                }
            }
            if settled {
                // Either the cluster is going down — the nodes the actions behind this one
                // would have run on are going away with it — or nothing was performed, and no
                // later run of the same script changes that answer.
                return;
            }
        }
        if !performed_any {
            return;
        }
    }
}

/// What performing ONE bootstrap action came to: what the action answered, what its script
/// wrote while it ran, and what its node's local disk holds afterwards.
#[derive(Debug, Clone)]
struct PerformedBootstrapAction {
    /// What the action came to ([`BootstrapActionAnswer`]).
    answer: BootstrapActionAnswer,
    /// The two console streams of the run and the status it exited with, for the log files the
    /// run leaves on its node ([`super::node_logs`]) — `None` for a run nothing performed.
    log: Option<StepRunLog>,
    /// What the node's local disk holds once the run is over, which is the node's from then on.
    kept: Vec<(String, String)>,
}

/// Perform ONE owed bootstrap action and answer what it came to, what its script wrote and what
/// the node's local disk holds afterwards — or `None` when nothing answered for it at all.
async fn performed_bootstrap_action<Eng: Send + Sync + 'static>(
    engine: &Eng,
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    plan: &BootstrapActionPlan,
    credentials: &StepCredentials,
    claim: InFlight,
    disk: Vec<(String, String)>,
) -> Option<PerformedBootstrapAction> {
    let script = match read_bootstrap_script(ctx, region, &plan.path).await {
        Ok(script) => script,
        // There is nothing to run: the node could not fetch the script the action names, which
        // is a failure of the action itself — and its disk is as it was, because nothing ran.
        Err(detail) => {
            return Some(PerformedBootstrapAction {
                answer: BootstrapActionAnswer::Unfetched(detail),
                log: None,
                kept: disk,
            })
        }
    };
    // The CLI this script's environment provides, built per run so each gets its own transfer
    // budget, and presenting on every call it makes what the cluster's instances obtained from
    // their instance profile.
    let node = plan.node.clone();
    let cli = step_object_cli(
        engine,
        ctx,
        region,
        credentials,
        &format!("bootstrap-{}-{}", plan.position, node.key()),
    );
    performed_on_its_own_thread(BOOTSTRAP_THREAD_NAME, TRANSFER_BUDGET, claim, move || {
        performed_script(&script, &node, cli, disk)
    })
    .await
}

/// The script of one bootstrap action, read from the object its `Path` names.
///
/// The read is RANGED, so at most [`MAX_BOOTSTRAP_SCRIPT_BYTES`] plus the one byte that shows
/// the object is past the bound is ever buffered, and a file that is not text is one no session
/// can read. It is made over the framework's in-process callback, the same transport a client's
/// own Amazon S3 request takes, so the bytes that run are the bytes that were uploaded.
async fn read_bootstrap_script(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    path: &str,
) -> Result<String, String> {
    let Some(object) = s3_object(path) else {
        return Err(format!(
            "The path {path} names no object of Amazon S3 to run."
        ));
    };
    let read = ::service_common::s3_client::s3_client(ctx.callback(), region)
        .get_object()
        .bucket(&object.bucket)
        .key(&object.key)
        .range(format!("bytes=0-{MAX_BOOTSTRAP_SCRIPT_BYTES}"))
        .send()
        .await
        .map_err(|error| {
            format!(
                "the object store answered {:?}: {}",
                ::aws_smithy_types::error::metadata::ProvideErrorMetadata::code(&error),
                ::aws_smithy_types::error::display::DisplayErrorContext(&error)
            )
        })?;
    let body = read
        .body
        .collect()
        .await
        .map_err(|error| format!("the script did not arrive in full: {error}"))?
        .into_bytes();
    if body.len() > MAX_BOOTSTRAP_SCRIPT_BYTES {
        return Err(format!(
            "the script is past the {MAX_BOOTSTRAP_SCRIPT_BYTES} bytes one may hold"
        ));
    }
    String::from_utf8(body.to_vec()).map_err(|_| "the script is not text".to_string())
}

/// What the session came to for `script`, performed as a program OF `node`, and what the
/// script wrote while it ran.
///
/// The session holds the parse, the allow-list of session-local utilities, the two environment
/// programs this module's header states, and nothing else: no process is started here. It is
/// given no variables — a bootstrap action is handed no environment of this service's or of the
/// host's — and the credential the CLI's own calls are signed with is held by that program
/// rather than by the session. What makes the run the NODE's is what is seeded into it: the
/// node's metadata client, and the file Amazon EMR publishes the node's role in
/// ([`super::bootstrap_nodes`]).
///
/// The one status that is not an answer about the script is the one a shell gives a utility it
/// could not find, which is what the session answers for a script it performed nothing of —
/// unless the script ENDED the session itself, which is a status it really asked for. Such a
/// run is reported as performed by nothing ([`BootstrapActionAnswer::NotPerformed`]), never as
/// a status the script exited with: nothing ran, so there is no status — and no stream and no
/// status are kept for it either.
fn performed_script(
    script: &str,
    node: &BootstrapNode,
    cli: Option<StepObjectCli>,
    disk: Vec<(String, String)>,
) -> PerformedBootstrapAction {
    // A script is written against storage, so the session is given a working tree of its own:
    // its redirections leave files the CLI then carries, and the grammar's control flow and the
    // file utilities a script uses are commands it performs.
    //
    // What the script WRITES is kept, because the log files it leaves on its node are what a
    // client reads it out of; the two streams are reported apart, so each file carries the
    // stream it is of.
    let config = SessionConfig {
        output: Some(BOOTSTRAP_OUTPUT_LIMITS),
        ..SessionConfig::default()
    };
    // The programs of the NODE the script runs on — its metadata client — beside the CLI its
    // environment provides, asked in that order, so the node's own program answers for a name
    // they would both carry.
    let programs: Box<dyn EnvironmentPrograms> = match cli {
        Some(cli) => layered(
            node.node_programs(),
            Box::new(cli) as Box<dyn EnvironmentPrograms>,
        ),
        None => node.node_programs(),
    };
    let mut session =
        ShellSession::with_programs(BTreeMap::new(), config, programs).with_file_utilities();
    // What this node's local disk already holds, which the script finds on it: a node has
    // storage, and it outlives the one program that wrote to it
    // ([`super::node_file_systems`]).
    seed_node_disk(&mut session, &disk);
    // The file every Amazon EMR bootstrap action reads its node's role from, published to the
    // node before its actions run.
    session.put_file(INSTANCE_INFO_PATH, &node.instance_info());
    let status = session.run(script);
    // What the script left on that disk is the node's from now on, whatever the run came to:
    // the writes happened.
    let kept = kept_node_disk(&session);
    if status == config.statuses.could_not_run && session.exit_status().is_none() {
        return PerformedBootstrapAction {
            answer: BootstrapActionAnswer::NotPerformed,
            log: None,
            kept,
        };
    }
    PerformedBootstrapAction {
        answer: BootstrapActionAnswer::Exited(status),
        log: Some(
            StepRunLog {
                stdout: ::service_common::posix_shell_session::stream_text(session.output()),
                stderr: ::service_common::posix_shell_session::stream_text(session.errors()),
                exit_status: Some(status),
            }
            .bounded(),
        ),
        kept,
    }
}

/// The state a cluster whose shutdown is due reaches, which follows from WHY it shut down.
///
/// `ClusterStateChangeReasonCode` separates the shutdowns a cluster was asked for or finished
/// its work with — `USER_REQUEST` and `ALL_STEPS_COMPLETED` — from the ones that are a FAILURE
/// of the cluster itself: `BOOTSTRAP_FAILURE`, `INSTANCE_FAILURE`, `INSTANCE_FLEET_TIMEOUT`,
/// `STEP_FAILURE`, `VALIDATION_ERROR` and `INTERNAL_ERROR`. A cluster that went down on one of
/// those ends `TERMINATED_WITH_ERRORS` — "the cluster was terminated due to errors" — and every
/// other one ends `TERMINATED`.
#[must_use]
pub fn terminal_cluster_state(reason: Option<&StateChangeReasonRecord>) -> &'static str {
    let failed = reason.is_some_and(|reason| {
        CLUSTER_FAILURE_REASON_CODES.contains(&reason.code.as_str())
    });
    if failed {
        cluster_state::TERMINATED_WITH_ERRORS
    } else {
        cluster_state::TERMINATED
    }
}

/// The `ClusterStateChangeReasonCode` values that are a FAILURE of the cluster rather than a
/// shutdown it was asked for or finished its work with
/// (`smithy_query(query="shape", shape="ClusterStateChangeReasonCode")`). See
/// [`terminal_cluster_state`].
const CLUSTER_FAILURE_REASON_CODES: &[&str] = &[
    cluster_state_change_reason_code::BOOTSTRAP_FAILURE,
    "INSTANCE_FAILURE",
    "INSTANCE_FLEET_TIMEOUT",
    "STEP_FAILURE",
    cluster_state_change_reason_code::VALIDATION_ERROR,
    "INTERNAL_ERROR",
];
