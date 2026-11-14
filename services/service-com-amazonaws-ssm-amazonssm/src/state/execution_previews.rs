// SPDX-License-Identifier: CC0-1.0

//! The execution previews this service holds: what a preview of an Automation runbook
//! WORKS OUT, the record `StartExecutionPreview` persists and the report
//! `GetExecutionPreview` projects.
//!
//! A preview is keyed by `(region, execution preview id)`, the way every other resource of
//! this service is: it is created at the Systems Manager endpoint of one Region and is
//! identified there by the id the start minted.
//!
//! # What a preview is worked out from
//!
//! "Initiates the process of creating a preview showing the effects that running a
//! specified Automation runbook would have on the targeted resources"
//! (`StartExecutionPreview`), so everything a preview reports is COMPUTED from the runbook
//! the start resolved and the execution inputs it carried, against the state this store
//! already holds:
//!
//! * `StepPreviews` counts the runbook's own steps by the impact each one's action would
//!   have ([`step_impact`]) — the steps read out of the resolved version's content by the
//!   one runbook reader this service has ([`super::automation::runbook::steps_of`]), so a
//!   preview and an execution read the same runbook;
//! * `TargetPreviews` counts the resources the inputs' targets resolve to, grouped by the
//!   type each resolved resource is ([`target_type_of`]) — resolved through the one node
//!   matcher this service has ([`super::commands::nodes_matching_target`]), so a criterion
//!   that describes no registered node contributes no resource rather than an invented one;
//! * `Regions` are the Regions the inputs' `TargetLocations` name, and the Region the start
//!   was signed for when they name none;
//! * `TotalAccounts` is how many accounts those same target locations name, and this one
//!   account when they name none.
//!
//! Nothing here runs a step: a preview is the report of what running one WOULD do, which is
//! why the whole of it is this pure computation over the content and the store.
//!
//! # What the clock owes a preview
//!
//! A preview is minted `Pending` — the service has not worked it out yet — and the service
//! has it worked out [`COMPLETION_DELAY`] later, which is the one transition this emulator
//! owes a start: from then on the read reports it `Success` with the computation it
//! persisted and the instant it was due as its `EndedAt`, so two reads of one finished
//! preview report the same instant. The work itself is done once, when the start persisted
//! the record, so the clock moves a status rather than computing anything late.

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::serde_json::Value;

use crate::ops::types::{
    AutomationExecutionInputs, AutomationExecutionPreview, ExecutionPreview, Target, TargetPreview,
};

use super::automation::runbook::RunbookStep;
use super::ComAmazonawsSsmAmazonssmState;

/// The `ExecutionPreviewStatus` a preview is minted with: nothing has worked it out yet.
/// From the modelled enum (`crate::ops::types::EXECUTION_PREVIEW_STATUS_VALUES`).
pub const STATUS_PENDING: &str = "Pending";

/// The `ExecutionPreviewStatus` of a preview that has been worked out (same enum).
pub const STATUS_SUCCESS: &str = "Success";

/// How long after a start the service has the preview worked out: the one transition this
/// emulator owes a `StartExecutionPreview`, from `Pending` to `Success`. The same span this
/// service's other asynchronous start owes ([`super::automation::PICKUP_DELAY`]), so a
/// caller acting straight after the start still sees the preview it was answered with.
pub const COMPLETION_DELAY: Duration = super::automation::PICKUP_DELAY;

/// How many execution previews this store keeps at once, oldest evicted first.
///
/// The emulator's own guard — not an AWS quota — the same shape as the automation store's:
/// past it the oldest preview is forgotten, so a caller starting previews in a loop cannot
/// grow this store without bound.
pub const MAX_PREVIEWS_RETAINED: usize = 1_000;

/// The `ImpactType` of a step that "would make changes to the targets through actions that
/// create, modify, or delete resources" (`AutomationExecutionPreview$StepPreviews`). From
/// the modelled enum (`crate::ops::types::IMPACT_TYPE_VALUES`).
pub const IMPACT_MUTATING: &str = "Mutating";

/// The `ImpactType` of a step that "would retrieve data about resources but not make
/// changes to them" (same doc).
pub const IMPACT_NON_MUTATING: &str = "NonMutating";

/// The `ImpactType` of a step whose outcome Systems Manager does not know: one that
/// "invokes executions performed by another orchestration service like Lambda, Step
/// Functions, or Amazon Web Services Systems Manager Run Command", or that "might also call
/// a third-party API" (same doc).
pub const IMPACT_UNDETERMINED: &str = "Undetermined";

/// How many Regions a preview reports, from the modelled `RegionList` length (1..50): a
/// preview over more target Regions than that reports the first 50 the inputs name, in the
/// inputs' own order, rather than a list the model could not carry.
pub const MAX_PREVIEW_REGIONS: usize = 50;

/// How many resource types a preview reports a count for, from the modelled
/// `TargetPreviewList` length (0..50). Past it the types latest in the report's own
/// ascending type order are left out, for the same reason.
pub const MAX_TARGET_PREVIEWS: usize = 50;

/// The `TargetPreview$TargetType` of a resolved resource that is an EC2 instance, in the
/// resource-type spelling this service's model uses for it (the `SourceType` enum, and the
/// `Target$Key` form [`super::commands::TARGET_KEY_EC2_INSTANCE`]).
pub const TARGET_TYPE_EC2_INSTANCE: &str = "AWS::EC2::Instance";

/// The `TargetPreview$TargetType` of a resolved resource that is a hybrid-activation
/// managed node, from the same `SourceType` enum.
pub const TARGET_TYPE_MANAGED_INSTANCE: &str = "AWS::SSM::ManagedInstance";

/// The `aws:executeAwsApi` action, which calls ONE Amazon Web Services API — so what it
/// would do to a resource is the API it names rather than the action itself. From the
/// Systems Manager Automation action reference, the same reference
/// [`super::automation::ACTION_BRANCH`] and its siblings come from.
pub const ACTION_EXECUTE_AWS_API: &str = "aws:executeAwsApi";

/// The input of [`ACTION_EXECUTE_AWS_API`] that names the API it calls (same reference).
pub const EXECUTE_AWS_API_INPUT: &str = "Api";

/// The API-name prefixes of a read: "This category generally includes `Describe*`,
/// `List*`, `Get*`, and similar read-only API actions"
/// (`AutomationExecutionPreview$StepPreviews`) — the `similar` ones being the other verbs
/// an Amazon Web Services API reads with.
pub const READ_ONLY_API_PREFIXES: &[&str] =
    &["Describe", "List", "Get", "BatchGet", "Lookup", "Search", "Query", "Scan", "Head"];

/// The Automation actions that create, modify or delete a resource, and so are
/// [`IMPACT_MUTATING`].
///
/// The names are the Systems Manager Automation action reference's own; each is classified
/// by what that reference says it does, against the three `ImpactType` descriptions
/// (`AutomationExecutionPreview$StepPreviews`).
pub const MUTATING_ACTIONS: &[&str] = &[
    super::automation::instance_state::ACTION_CHANGE_INSTANCE_STATE,
    "aws:copyImage",
    "aws:createImage",
    "aws:createStack",
    "aws:createTags",
    "aws:deleteImage",
    "aws:deleteStack",
    "aws:runInstances",
];

/// The Automation actions that retrieve data, wait, or touch only the execution's own
/// values, and so are [`IMPACT_NON_MUTATING`] (same reference, same classification).
pub const NON_MUTATING_ACTIONS: &[&str] = &[
    "aws:approve",
    "aws:assertAwsResourceProperty",
    super::automation::ACTION_BRANCH,
    super::automation::ACTION_PAUSE,
    super::automation::ACTION_SLEEP,
    "aws:updateVariable",
    "aws:waitForAwsResourceProperty",
];

/// The Automation actions that hand the work to another orchestration service or to code
/// this service does not see, and so are [`IMPACT_UNDETERMINED`] (same reference, same
/// classification).
pub const UNDETERMINED_ACTIONS: &[&str] = &[
    "aws:executeAutomation",
    "aws:executeScript",
    "aws:executeStateMachine",
    "aws:invokeLambdaFunction",
    "aws:invokeWebhook",
    "aws:loop",
    "aws:runCommand",
];

/// The `ImpactType` running `step` would have on the targeted resources.
///
/// The step's action decides it: the catalogues above classify the actions the Automation
/// action reference publishes, [`ACTION_EXECUTE_AWS_API`] is decided by the API it names
/// (a read-only verb retrieves data, anything else changes a resource), and an action no
/// catalogue names is [`IMPACT_UNDETERMINED`] — "Systems Manager Automation doesn't know
/// the outcome" of work it does not run itself, which is exactly the case for an action it
/// does not publish.
pub fn step_impact(step: &RunbookStep) -> &'static str {
    let action = step.action.as_str();
    if MUTATING_ACTIONS.contains(&action) {
        return IMPACT_MUTATING;
    }
    if NON_MUTATING_ACTIONS.contains(&action) {
        return IMPACT_NON_MUTATING;
    }
    if action == ACTION_EXECUTE_AWS_API {
        return match step.inputs.get(EXECUTE_AWS_API_INPUT).and_then(Value::as_str) {
            Some(api) if READ_ONLY_API_PREFIXES.iter().any(|read| api.starts_with(read)) => {
                IMPACT_NON_MUTATING
            }
            // Any other verb creates, modifies or deletes what it is called on.
            Some(_) => IMPACT_MUTATING,
            // A step that names no API names nothing this could read, so what it would do
            // is not known either.
            None => IMPACT_UNDETERMINED,
        };
    }
    IMPACT_UNDETERMINED
}

/// The `TargetPreview$TargetType` one resolved managed node is counted under, or `None`
/// for a node this store holds no type for.
///
/// "A type of resource that was included in the execution preview"
/// (`TargetPreview$TargetType`): the node's own `SourceType` is that resource type when
/// the node reports one ("The type of the source resource",
/// `InstanceInformation$SourceType`), and a node that reports none is counted under the
/// resource type its `ResourceType` names, in the same `AWS::`-namespaced spelling.
pub fn target_type_of(node: &super::ManagedNodeRecord) -> Option<String> {
    if let Some(source_type) = node
        .properties
        .source_type
        .as_deref()
        .filter(|held| !held.is_empty())
    {
        return Some(source_type.to_string());
    }
    match node.properties.resource_type.as_deref() {
        Some(super::RESOURCE_TYPE_EC2_INSTANCE) => Some(TARGET_TYPE_EC2_INSTANCE.to_string()),
        Some(super::RESOURCE_TYPE_MANAGED_INSTANCE) => {
            Some(TARGET_TYPE_MANAGED_INSTANCE.to_string())
        }
        // A type this service does not publish is reported as the node reports it rather
        // than translated into one it does.
        Some(other) if !other.is_empty() => Some(other.to_string()),
        _ => None,
    }
}

/// What a preview worked out, before a status is attached to it: the four members
/// `AutomationExecutionPreview` reports.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ComputedPreview {
    /// How many of the runbook's steps carry each `ImpactType` ([`step_impact`]), keyed by
    /// that impact type. An impact type no step carries holds no entry, so the counts add
    /// up to the number of steps the runbook declares.
    pub step_impacts: BTreeMap<String, i32>,
    /// The Regions the execution would run in, in the order the inputs name them.
    pub regions: Vec<String>,
    /// How many resources of each type the targets resolve to, keyed by that type
    /// ([`target_type_of`]).
    pub target_counts: BTreeMap<String, i32>,
    /// How many accounts the execution would run in.
    pub total_accounts: i32,
}

impl ComputedPreview {
    /// The modelled `ExecutionPreview` for this computation — the `Automation` member of
    /// the union, because an Automation runbook is what was previewed.
    pub fn to_execution_preview(&self) -> ExecutionPreview {
        ExecutionPreview::Automation(AutomationExecutionPreview {
            step_previews: Some(
                self.step_impacts
                    .iter()
                    .map(|(impact, count)| (impact.clone(), *count))
                    .collect(),
            ),
            regions: Some(self.regions.clone()),
            target_previews: Some(
                self.target_counts
                    .iter()
                    .map(|(target_type, count)| TargetPreview {
                        count: *count,
                        target_type: Some(target_type.clone()),
                    })
                    .collect(),
            ),
            total_accounts: self.total_accounts,
        })
    }
}

/// Work out what running `steps` with `inputs` would do, against the nodes `region` holds,
/// or `Err(key)` when a target's key is not one an automation targets by.
///
/// Pure over the store: it reads the runbook's steps and the Region's nodes and writes
/// nothing, so the same preview is worked out whenever it is asked for.
pub fn compute(
    store: &ComAmazonawsSsmAmazonssmState,
    region: &str,
    steps: &[RunbookStep],
    inputs: Option<&AutomationExecutionInputs>,
) -> Result<ComputedPreview, String> {
    let mut step_impacts: BTreeMap<String, i32> = BTreeMap::new();
    for step in steps {
        *step_impacts.entry(step_impact(step).to_string()).or_insert(0) += 1;
    }

    let locations = inputs
        .and_then(|inputs| inputs.target_locations.as_deref())
        .unwrap_or_default();

    // "Information about the Amazon Web Services Regions targeted by the execution
    // preview": the Regions the target locations name, and the Region the start itself was
    // signed for when they name none.
    let mut regions: Vec<String> = Vec::new();
    for location in locations {
        for named in location.regions.clone().unwrap_or_default() {
            if !named.is_empty() && !regions.iter().any(|held| held == &named) {
                regions.push(named);
            }
        }
    }
    if regions.is_empty() {
        regions.push(region.to_string());
    }
    regions.truncate(MAX_PREVIEW_REGIONS);

    // "Information about the Amazon Web Services accounts that were included in the
    // execution preview": the accounts the target locations name, less the ones they
    // exclude. A location set that names no account at all runs in the account the start
    // was made for, which is one; a set that names accounts and excludes every one of them
    // includes none, and is reported as none rather than as this account.
    let mut accounts: Vec<String> = Vec::new();
    let mut any_account_named = false;
    for location in locations {
        let excluded = location.exclude_accounts.clone().unwrap_or_default();
        for named in location.accounts.clone().unwrap_or_default() {
            if named.is_empty() {
                continue;
            }
            any_account_named = true;
            if excluded.iter().any(|exclude| exclude == &named)
                || accounts.iter().any(|held| held == &named)
            {
                continue;
            }
            accounts.push(named);
        }
    }
    let total_accounts = if any_account_named {
        i32::try_from(accounts.len()).unwrap_or(i32::MAX)
    } else {
        1
    };

    // "The number of resources of a certain type included in an execution preview": the
    // resources the inputs' own targets and each target location's targets resolve to,
    // counted once each and grouped by the type they are.
    let mut target_counts: BTreeMap<String, i32> = BTreeMap::new();
    let mut counted: Vec<String> = Vec::new();
    let targets = inputs
        .and_then(|inputs| inputs.targets.as_deref())
        .unwrap_or_default()
        .iter()
        .chain(
            locations
                .iter()
                .flat_map(|location| location.targets.as_deref().unwrap_or_default()),
        );
    for target in targets {
        for id in resolved_resources(store, region, target)? {
            if counted.iter().any(|held| held == &id) {
                continue;
            }
            let Some(node) = store.managed_node(region, &id) else {
                continue;
            };
            let Some(target_type) = target_type_of(node) else {
                continue;
            };
            counted.push(id);
            *target_counts.entry(target_type).or_insert(0) += 1;
        }
    }
    while target_counts.len() > MAX_TARGET_PREVIEWS {
        let Some(last) = target_counts.keys().next_back().cloned() else {
            break;
        };
        target_counts.remove(&last);
    }

    Ok(ComputedPreview {
        step_impacts,
        regions,
        target_counts,
        total_accounts,
    })
}

/// The ids of the resources one target resolves to in `region`, or `Err(key)` when the
/// target's key is not one an automation targets by.
///
/// The resolution itself is the ONE an automation resolves its targets by
/// ([`ComAmazonawsSsmAmazonssmState::automation_target_values`]), so a preview and a start
/// resolve one target the same way. What a PREVIEW adds is that it reports resources: a
/// resolved value counts only where it names a node this Region holds — a
/// `Key=ParameterValues` value naming none is a parameter value, not a resource, and is
/// counted as neither.
fn resolved_resources(
    store: &ComAmazonawsSsmAmazonssmState,
    region: &str,
    target: &Target,
) -> Result<Vec<String>, String> {
    Ok(store
        .automation_target_values(region, target)?
        .into_iter()
        .filter(|value| store.managed_node(region, value).is_some())
        .collect())
}

/// ONE execution preview: what the start worked out, and the status the clock has moved it
/// to.
#[derive(Debug, Clone)]
pub struct ExecutionPreviewRecord {
    /// "The generated ID for the existing execution preview"
    /// (`GetExecutionPreviewResponse$ExecutionPreviewId`), minted by this store's own
    /// minter ([`ComAmazonawsSsmAmazonssmState::mint_execution_preview_id`]).
    pub preview_id: String,
    /// The Region the start was signed for, which is the endpoint the preview is reached
    /// through.
    pub region: String,
    /// The name of the Automation runbook this previews, as the document is held under.
    pub document_name: String,
    /// The version of that runbook the start resolved, so a later update of the document
    /// does not change what was previewed.
    pub document_version: String,
    /// An `ExecutionPreviewStatus` value ([`STATUS_PENDING`], [`STATUS_SUCCESS`]).
    pub status: String,
    /// "Supplemental information about the current status of the execution preview"
    /// (`GetExecutionPreviewResponse$StatusMessage`); `None` while there is nothing
    /// supplemental to report.
    pub status_message: Option<String>,
    /// When the start persisted this preview.
    pub started_at: SystemTime,
    /// When the service has the preview worked out ([`COMPLETION_DELAY`]), which is the
    /// instant it is reported as having ended at.
    pub ready_at: SystemTime,
    /// What the start worked out ([`compute`]), reported once the preview has ended.
    pub computed: ComputedPreview,
}

impl ExecutionPreviewRecord {
    /// Move this preview to the status the clock already owes it at `now`.
    ///
    /// The ONE transition a start owes: a preview the service has had time to work out is
    /// `Success` from the instant it was DUE, so its `EndedAt` is that instant rather than
    /// whenever a read happened to arrive.
    pub fn materialize_due(&mut self, now: SystemTime) {
        if self.status == STATUS_PENDING && now >= self.ready_at {
            self.status = STATUS_SUCCESS.to_string();
        }
    }

    /// Whether this preview has been worked out, and so reports what it computed.
    pub fn ended(&self) -> bool {
        self.status == STATUS_SUCCESS
    }

    /// "A UTC timestamp indicating when the execution preview operation ended"
    /// (`GetExecutionPreviewResponse$EndedAt`), or `None` for one that has not ended.
    pub fn ended_at(&self) -> Option<SystemTime> {
        self.ended().then_some(self.ready_at)
    }

    /// "Information about the changes that would be made if an execution were run"
    /// (`GetExecutionPreviewResponse$ExecutionPreview`), or `None` for a preview that has
    /// not been worked out yet: there is nothing to report until it has.
    pub fn execution_preview(&self) -> Option<ExecutionPreview> {
        self.ended().then(|| self.computed.to_execution_preview())
    }
}

/// How a read words the miss of an execution preview id this Region holds none of: the id
/// that was asked for, in the sentence the Automation slice this previews words a missing
/// execution with ("Automation execution {id} does not exist.",
/// `crate::ops::get_automation_execution`) — so the two reads of one slice word the same
/// refusal the same way, and a caller reads back the id whose preview it asked for.
///
/// The modelled `ExecutionPreviewId` pattern is enforced before a handler runs, so the
/// value this refuses is always a well-formed id the Region's endpoint minted no preview
/// under.
pub fn execution_preview_not_found_message(preview_id: &str) -> String {
    format!("Execution preview {preview_id} does not exist.")
}

impl ComAmazonawsSsmAmazonssmState {
    /// The execution preview `preview_id` names in `region`, or `None` when that Region
    /// holds none.
    pub fn execution_preview(
        &self,
        region: &str,
        preview_id: &str,
    ) -> Option<&ExecutionPreviewRecord> {
        self.execution_previews
            .get(&(region.to_string(), preview_id.to_string()))
    }

    /// Store `record` as `region`'s execution preview of that id.
    ///
    /// Past [`MAX_PREVIEWS_RETAINED`] the oldest preview is forgotten, so this store holds
    /// a bounded number of records however many starts arrive.
    pub fn insert_execution_preview(&mut self, record: ExecutionPreviewRecord) {
        let key = (record.region.clone(), record.preview_id.clone());
        self.execution_previews.insert(key, record);
        while self.execution_previews.len() > MAX_PREVIEWS_RETAINED {
            let Some(oldest) = self
                .execution_previews
                .iter()
                .min_by_key(|(_, preview)| preview.started_at)
                .map(|(key, _)| key.clone())
            else {
                break;
            };
            self.execution_previews.remove(&oldest);
        }
    }

    /// Mint the id of an execution preview.
    ///
    /// Modelled exactly 36 characters long with the hyphenated UUID pattern
    /// (`ExecutionPreviewId`), minted through [`super::minted_uuid`] from this store's own
    /// creation instant and its own mint counter — so no two previews of one run collide
    /// and the value is not a count a caller can read off the wire.
    pub fn mint_execution_preview_id(&mut self) -> String {
        self.minted_execution_preview_ids += 1;
        let since_epoch = self
            .created_at
            .duration_since(::std::time::UNIX_EPOCH)
            .unwrap_or_default();
        super::minted_uuid(since_epoch.as_nanos(), self.minted_execution_preview_ids)
    }

    /// Materialise everything the clock owes every execution preview at `now`.
    pub fn advance_execution_previews(&mut self, now: SystemTime) {
        for preview in self.execution_previews.values_mut() {
            preview.materialize_due(now);
        }
    }
}
