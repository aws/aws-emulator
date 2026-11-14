// SPDX-License-Identifier: CC0-1.0

//! The stored blueprint records and the runs of them that lay a workflow out, with the
//! statuses each moves through.

use super::*;

/// The `BlueprintStatus` a blueprint is in the instant its registration is recorded:
/// "Creating — The blueprint registration is in progress" (`Blueprint$Status`). One of the
/// modelled `BlueprintStatus` values, whose published list the generated
/// [`crate::ops::types::BLUEPRINT_STATUS_VALUES`] carries (`ACTIVE`, `CREATING`, `FAILED`,
/// `UPDATING`).
pub const BLUEPRINT_STATUS_CREATING: &str = "CREATING";

/// The `BlueprintStatus` of a blueprint whose registration has finished: "Active — The
/// blueprint has been successfully registered." See [`BLUEPRINT_STATUS_CREATING`].
pub const BLUEPRINT_STATUS_ACTIVE: &str = "ACTIVE";

/// The `BlueprintStatus` of a blueprint whose registration could not finish: "Failed — The
/// blueprint registration failed." See [`BLUEPRINT_STATUS_CREATING`].
pub const BLUEPRINT_STATUS_FAILED: &str = "FAILED";

/// The `Version` of the FIRST registration of a blueprint, which is the version every
/// blueprint a create registers holds: a blueprint's definition is versioned from one, and
/// an update of it registers the next version.
pub const FIRST_BLUEPRINT_VERSION: i32 = 1;

/// How long a blueprint stays in the status its registration put it in before the clock
/// owes it [`BLUEPRINT_STATUS_ACTIVE`].
///
/// CHOSEN, not documented: Glue publishes no duration for a blueprint registration, so this
/// is the emulator's own span — deliberately not zero, so a caller that reads a blueprint
/// straight after registering it still observes the registration in progress. The due
/// instant is stored on the record and materialised by whichever later call reads it;
/// nothing sleeps.
pub const BLUEPRINT_SETTLES_AFTER: Duration = Duration::from_secs(3);

/// What a registration established about the archive published at a blueprint's
/// `BlueprintLocation` — "specifies the path in Amazon S3 where the blueprint is published"
/// — which is what decides the terminal status of that registration.
///
/// `CreateBlueprint` is asynchronous: it records the registration and Glue then FETCHES the
/// published archive, so a registration pointed at an object the object store does not serve
/// comes to rest having failed rather than being refused at the call.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlueprintArchive {
    /// The object store served the archive, so there is a blueprint definition to register.
    Published,
    /// The object store ANSWERED that the archive cannot be read — it holds nothing at that
    /// address, the read is not permitted, or it answered something no archive can be read
    /// through — or the location names no Amazon S3 object at all.
    Unreadable,
}

/// Read the archive published at `location`, through this emulator's own Amazon S3, and
/// answer what that read established.
///
/// The read is a HEAD of the object: what a registration needs to know is whether the
/// archive at that address can be FETCHED, not what is in it, and heading the object answers
/// that without buffering an archive of any size. What the store ANSWERED is read through the
/// tree's one reading of an object store's answer
/// ([`::service_common::s3_object_read::answered_refusal`]), so this registration and every
/// other read this service makes of an object agree about what an answer MEANT.
///
/// Only what the store ANSWERED fails a registration: a failure carrying no error code is no
/// answer about the object — there is no object store mounted to ask, or the call could not
/// be dispatched — so nothing is established and the registration is not failed for it, the
/// same way a job run is admitted when nothing can be shown about its role.
pub async fn read_blueprint_archive(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    location: &str,
) -> BlueprintArchive {
    // The location is checked for naming an object at all BEFORE anything is asked of the
    // store: a value naming no bucket, or naming a prefix rather than one object, names no
    // archive anywhere.
    let Some((bucket, key)) = split_s3_location(location) else {
        return BlueprintArchive::Unreadable;
    };
    if key.is_empty() || key.ends_with('/') {
        return BlueprintArchive::Unreadable;
    }
    let client = ::service_common::s3_client::s3_client(callback, region);
    let Err(error) = client.head_object().bucket(bucket).key(key).send().await else {
        return BlueprintArchive::Published;
    };
    match ::service_common::s3_object_read::answered_refusal(&error) {
        ::service_common::s3_object_read::ObjectRead::Unanswered => BlueprintArchive::Published,
        _ => BlueprintArchive::Unreadable,
    }
}

/// A stored blueprint: the reusable workflow template a blueprint create registers.
///
/// Everything a caller of this record can observe through the modelled `Blueprint` shape is
/// held here — the name it is addressed by, where its archive is published, its
/// description, when it was registered and the status of that registration — plus the
/// `Tags` a create carried, which is what `ListBlueprints` narrows its answer by.
#[derive(Debug, Clone)]
pub struct BlueprintEntry {
    /// The blueprint's name, which is also its key in
    /// [`ComAmazonawsGlueAwsglueState::blueprints`].
    pub name: String,
    /// `Blueprint$BlueprintLocation`: "Specifies the path in Amazon S3 where the blueprint
    /// is published", exactly as the registration sent it.
    pub blueprint_location: String,
    /// `Blueprint$Description`: "The description of the blueprint", and `None` when the
    /// registration carried none.
    pub description: Option<String>,
    /// `Blueprint$CreatedOn`: "The date and time the blueprint was registered."
    pub created_on: SystemTime,
    /// `Blueprint$LastModifiedOn`: "The date and time the blueprint was last modified",
    /// which for a blueprint nothing has updated is the instant it was registered.
    pub last_modified_on: SystemTime,
    /// One of the modelled `BlueprintStatus` values: the status of this blueprint's
    /// registration.
    pub status: String,
    /// The instant the clock owes this blueprint its next status; `None` once it has
    /// settled.
    pub settles_at: Option<SystemTime>,
    /// The `BlueprintStatus` the clock owes this blueprint at [`Self::settles_at`]: the
    /// terminal status its registration is headed for, decided by whether the archive
    /// published at [`Self::blueprint_location`] could be read ([`BlueprintArchive`]).
    pub settles_to: String,
    /// `Blueprint$ErrorMessage`: "An error message", which a registration carries only once
    /// it has come to rest having failed.
    pub error_message: Option<String>,
    /// The `ErrorMessage` the clock gives this blueprint when it moves it into
    /// [`Self::settles_to`], and `None` for a registration whose archive was read.
    pub settles_with_error: Option<String>,
    /// `Blueprint$Version`: the version of the definition this registration holds, which
    /// for the registration a create wrote is [`FIRST_BLUEPRINT_VERSION`].
    pub version: i32,
    /// The tags the blueprint was registered with, which the modelled `Blueprint` shape
    /// does not carry.
    ///
    /// Kept because `ListBlueprints` "filters the list by an Amazon Web Services resource
    /// tag", so they decide which blueprints a filtered listing reports. NOT yet reachable
    /// through the tagging operations, for the reason [`CustomEntityTypeEntry::tags`]
    /// gives: [`TaggedResourceKind`] carries no blueprint variant yet.
    pub tags: HashMap<String, String>,
    /// The runs `StartBlueprintRun` started of this blueprint, oldest first and bounded by
    /// [`MAX_RUNS_PER_BLUEPRINT`].
    ///
    /// A run lives on its blueprint because every operation that addresses one names the
    /// blueprint too: `GetBlueprintRun` takes a `BlueprintName` beside the `RunId`, exactly
    /// as `GetJobRun` takes a `JobName` beside its run id.
    pub runs: Vec<BlueprintRunEntry>,
}

impl BlueprintEntry {
    /// Record the blueprint a `CreateBlueprint` describes, in
    /// [`BLUEPRINT_STATUS_CREATING`] with [`BLUEPRINT_STATUS_ACTIVE`] due
    /// [`BLUEPRINT_SETTLES_AFTER`] out — "Creating — The blueprint registration is in
    /// progress" — and every other member taken from the request, which carries the whole
    /// registration.
    pub fn from_create_input(
        input: &CreateBlueprintInput,
        now: SystemTime,
        archive: BlueprintArchive,
    ) -> Self {
        // A registration whose archive the object store serves is owed the status of a
        // blueprint that was registered; one whose archive it cannot serve is owed the
        // terminal failure status carrying the location that could not be read.
        let (settles_to, settles_with_error) = match archive {
            BlueprintArchive::Published => (BLUEPRINT_STATUS_ACTIVE, None),
            BlueprintArchive::Unreadable => (
                BLUEPRINT_STATUS_FAILED,
                Some(messages::blueprint_location_not_accessible(
                    &input.blueprint_location,
                )),
            ),
        };
        Self {
            name: input.name.clone(),
            blueprint_location: input.blueprint_location.clone(),
            description: input.description.clone(),
            created_on: now,
            last_modified_on: now,
            status: BLUEPRINT_STATUS_CREATING.to_string(),
            settles_at: Some(now.checked_add(BLUEPRINT_SETTLES_AFTER).unwrap_or(now)),
            settles_to: settles_to.to_string(),
            // Nothing has gone wrong yet: a registration carries an error message only once
            // it has come to rest having failed.
            error_message: None,
            settles_with_error,
            // The registration a create wrote is the first version of the blueprint.
            version: FIRST_BLUEPRINT_VERSION,
            tags: input.tags.clone().unwrap_or_default(),
            // A blueprint a registration wrote has not been run.
            runs: Vec::new(),
        }
    }

    /// Move this blueprint into the status the clock owed it at `now`, if it owed one.
    ///
    /// The registration was due to finish at [`Self::settles_at`] whether or not anybody
    /// called, so materialising it on a read reports the status the clock had already
    /// reached, and two reads of a settled blueprint report the same record.
    /// [`Self::last_modified_on`] does not move with it: the registration finishing is the
    /// create completing, not a later modification of the blueprint.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.status = self.settles_to.clone();
        // "An error message": the registration that comes to rest without having read the
        // archive it was pointed at carries the reason it did not.
        self.error_message = self.settles_with_error.clone();
    }

    /// Whether this blueprint's registration has finished, i.e. is in
    /// [`BLUEPRINT_STATUS_ACTIVE`]: "Active — The blueprint has been successfully
    /// registered" (`Blueprint$Status`).
    ///
    /// Call [`Self::settle_if_due`] first, or a registration the clock has already finished
    /// still reports as being in progress.
    pub fn is_active(&self) -> bool {
        self.status == BLUEPRINT_STATUS_ACTIVE
    }

    /// Give every run of this blueprint the state the CLOCK already owed it at `now`.
    ///
    /// This is the one write a reporting operation performs, exactly as
    /// [`WorkflowEntry::settle_due_runs`] is for a workflow run: a run's next state was due
    /// at [`BlueprintRunEntry::settles_at`] whether or not anybody called, so materialising
    /// it on read reports the state the clock had already reached.
    pub fn settle_due_runs(&mut self, now: SystemTime) {
        for run in &mut self.runs {
            run.settle_if_due(now);
        }
    }

    /// The stored run with this id, or `None` when this blueprint has no such run.
    pub fn run(&self, run_id: &str) -> Option<&BlueprintRunEntry> {
        self.runs.iter().find(|run| run.run_id == run_id)
    }

    /// Record the run `run_id` of this blueprint, as the start that minted that id describes
    /// it, at `now`.
    ///
    /// Everything the clock already owed this blueprint's runs is materialised first, so a
    /// read of any of them reports the state the clock had already reached; a run already in
    /// flight does not stop another being recorded, because a blueprint publishes no ceiling
    /// on its concurrent runs. The id is minted by the start that lays the run's workflow out
    /// ([`ComAmazonawsGlueAwsglueState::start_blueprint_run`]), because the workflow a run
    /// creates carries the id of the run that created it.
    pub fn begin_run(
        &mut self,
        input: &StartBlueprintRunInput,
        run_id: String,
        now: SystemTime,
        layout: &BlueprintRunLayout,
    ) {
        self.settle_due_runs(now);
        self.record_run(BlueprintRunEntry::begin(input, run_id, now, layout));
    }

    /// Append a run, keeping at most [`MAX_RUNS_PER_BLUEPRINT`] of them.
    pub fn record_run(&mut self, run: BlueprintRunEntry) {
        self.runs.push(run);
        if self.runs.len() > MAX_RUNS_PER_BLUEPRINT {
            let overflow = self.runs.len() - MAX_RUNS_PER_BLUEPRINT;
            self.runs.drain(..overflow);
        }
    }

    /// Rebuild the modelled `Blueprint` shape from what is stored, so every reader of a
    /// blueprint reports the same registration.
    pub fn to_blueprint(&self) -> Blueprint {
        Blueprint {
            name: Some(self.name.clone()),
            description: self.description.clone(),
            created_on: Some(to_date_time(self.created_on)),
            last_modified_on: Some(to_date_time(self.last_modified_on)),
            // "A JSON string that indicates the list of parameter specifications for the
            // blueprint": Glue reads that list out of the blueprint ARCHIVE published at
            // `BlueprintLocation`. Nothing here reads that archive, so there is no
            // parameter list to report rather than one invented for it.
            parameter_spec: None,
            blueprint_location: Some(self.blueprint_location.clone()),
            // "Specifies a path in Amazon S3 where the blueprint is copied when you call
            // CreateBlueprint/UpdateBlueprint to register the blueprint in Glue": nothing
            // here copies the archive, so there is no such path a caller could read.
            blueprint_service_location: None,
            status: Some(self.status.clone()),
            // "An error message": a registration that came to rest having failed carries the
            // reason it did, and one whose archive was read carries none.
            error_message: self.error_message.clone(),
            // The version of the definition this registration holds.
            version: Some(self.version),
            // "When there are multiple versions of a blueprint and the latest version has
            // some errors, this attribute indicates the last successful blueprint
            // definition": the registration stored here is itself the latest and has no
            // error, so there is no earlier definition to fall back to.
            last_active_definition: None,
        }
    }
}

/// The `BlueprintRunState` a run is in the instant `StartBlueprintRun` returns: "Running —
/// The blueprint run is in progress" (`BlueprintRun$State`). One of the modelled
/// `BlueprintRunState` values (`RUNNING`, `SUCCEEDED`, `FAILED`, `ROLLING_BACK`).
pub const BLUEPRINT_RUN_STATE_RUNNING: &str = "RUNNING";

/// The `BlueprintRunState` of a run that created the workflow it lays out: "Succeeded — The
/// blueprint run completed successfully" (`BlueprintRun$State`). See
/// [`BLUEPRINT_RUN_STATE_RUNNING`].
pub const BLUEPRINT_RUN_STATE_SUCCEEDED: &str = "SUCCEEDED";

/// The `BlueprintRunState` of a run that created no workflow: "Failed — The blueprint run
/// failed and rollback is complete" (`BlueprintRun$State`). See
/// [`BLUEPRINT_RUN_STATE_RUNNING`].
pub const BLUEPRINT_RUN_STATE_FAILED: &str = "FAILED";

/// The `BlueprintRunState` values a run never moves out of again: of the modelled enum
/// quoted on [`BLUEPRINT_RUN_STATE_RUNNING`], the ones that name a finished run. The other
/// two (`RUNNING`, `ROLLING_BACK`) each still owe a further state.
pub const TERMINAL_BLUEPRINT_RUN_STATES: [&str; 2] =
    [BLUEPRINT_RUN_STATE_SUCCEEDED, BLUEPRINT_RUN_STATE_FAILED];

/// How long a blueprint run stays in the state the call that started it put it in before
/// the clock owes it the next one.
///
/// CHOSEN, not documented: Glue publishes no duration for a blueprint run, so this is the
/// emulator's own span, the same one a blueprint registration takes
/// ([`BLUEPRINT_SETTLES_AFTER`]) — deliberately not zero, so a caller that reads the run
/// straight after starting it still observes it in progress. The due instant is stored on the
/// run and materialised by whichever later call reads it; nothing sleeps.
pub const BLUEPRINT_RUN_SETTLES_AFTER: Duration = BLUEPRINT_SETTLES_AFTER;

/// The key of the `Parameters` member that names the workflow a blueprint run lays out.
///
/// "The blueprint parameters as a string. You will have to provide a value for each key that
/// is required from the parameter spec that is defined in the `Blueprint$ParameterSpec`"
/// (`BlueprintRun$Parameters`): the spec itself lives in the archive published at the
/// blueprint's location, and the one parameter Glue's blueprint programming model has every
/// blueprint take from its caller is the name to give the workflow it creates, since that
/// workflow is what the run produces (`BlueprintRun$WorkflowName`). A start whose parameters
/// do not carry it names no workflow of its own ([`blueprint_run_workflow_name`]).
pub const BLUEPRINT_WORKFLOW_NAME_PARAMETER: &str = "WorkflowName";

/// The name of the workflow a run of `blueprint_name` lays out, given the `Parameters` the
/// start carried.
///
/// The caller's own [`BLUEPRINT_WORKFLOW_NAME_PARAMETER`] when its parameters are a JSON
/// object carrying that key with a string value a workflow can be named by — `NameString`,
/// "len 1..255" — because that is the name the caller asked the run to create the workflow
/// under.
///
/// Otherwise the BLUEPRINT's own name, which is CHOSEN: the layout a run performs is defined
/// by the published archive, which nothing here reads, so when the caller names no workflow
/// the only name this store knows for what the run produces is the blueprint's. A blueprint
/// name is inside `NameString`'s bounds, so the workflow a run creates always has a name
/// that can be addressed.
pub fn blueprint_run_workflow_name(blueprint_name: &str, parameters: Option<&str>) -> String {
    parameters
        .and_then(|parameters| serde_json::from_str::<serde_json::Value>(parameters).ok())
        .and_then(|parameters| {
            parameters
                .get(BLUEPRINT_WORKFLOW_NAME_PARAMETER)
                .and_then(|named| named.as_str())
                .map(str::to_string)
        })
        .filter(|named| (1..=MAX_WORKFLOW_NAME_LENGTH).contains(&named.chars().count()))
        .unwrap_or_else(|| blueprint_name.to_string())
}

/// The longest workflow name the model admits: `NameString`, "len 1..255", which is what
/// `CreateWorkflowRequest$Name` and every workflow read address a workflow by.
pub(super) const MAX_WORKFLOW_NAME_LENGTH: usize = 255;

/// What a blueprint run's layout of the workflow it produces came to.
///
/// A run exists to create a workflow — "The name of a workflow that is created as a result of
/// a successful blueprint run" (`BlueprintRun$WorkflowName`) — so what the run becomes is
/// decided by whether it could create one: it succeeds carrying the workflow it created, and
/// otherwise fails carrying the reason, because "if a blueprint run has an error, there will
/// not be a workflow created".
#[derive(Debug, Clone)]
pub enum BlueprintRunLayout {
    /// The run created the workflow of this name, which is the workflow it produced.
    Created(String),
    /// This account already held a workflow of this name, so the run created none and
    /// nothing already stored was overwritten.
    WorkflowAlreadyExists(String),
}

/// Why no run of a blueprint could be started
/// ([`ComAmazonawsGlueAwsglueState::start_blueprint_run`]).
///
/// Each variant is one of `StartBlueprintRun`'s own modelled failures, carrying what the
/// refusal has to name; the handler words them, so the store answers WHAT happened rather
/// than how it is reported.
#[derive(Debug, Clone)]
pub enum BlueprintRunNotStarted {
    /// This account holds no blueprint under the name the start addressed.
    NoSuchBlueprint,
    /// The blueprint has no ACTIVE definition to run: its registration has not finished, or
    /// finished without one.
    NoActiveDefinition,
    /// The store already holds as many workflows as it will ([`MAX_WORKFLOWS`]), so the
    /// workflow the run would create cannot be written.
    WorkflowCeilingReached(usize),
}

/// How many runs one blueprint keeps.
///
/// CHOSEN: Glue publishes no retention for blueprint-run records, so this is the emulator's
/// own window, for the reason [`MAX_RUNS_PER_JOB`] gives. The OLDEST record is dropped
/// first, so the runs a caller is most likely to read are kept.
pub const MAX_RUNS_PER_BLUEPRINT: usize = 100;

/// How many blueprint run ids this process has minted. A nonce source for
/// [`new_blueprint_run_id`], exactly as [`MINTED_JOB_RUN_IDS`] is for a job run id.
pub(super) static MINTED_BLUEPRINT_RUN_IDS: AtomicU64 = AtomicU64::new(0);

/// How many 64-bit words a blueprint run id carries: two, i.e. 128 bits rendered as 32
/// lowercase hexadecimal characters.
pub(super) const BLUEPRINT_RUN_ID_WORDS: usize = 2;

/// A fresh `RunId`: the id the service assigns a blueprint run when it starts.
///
/// 32 lowercase hexadecimal characters from [`mint_hex_id`], which is inside `IdString`'s
/// `length 1..255` and satisfies its pattern. Glue documents no prefix for a blueprint run
/// id — unlike a job run's `jr_` or a workflow run's `wr_` — so this one carries none. It is
/// stored on the run, so every later read of that run reports the same id, and two runs never
/// share one.
///
/// This names a RESOURCE, not a credential: a credential would come from the credential
/// store, never from here.
pub fn new_blueprint_run_id() -> String {
    mint_hex_id(&MINTED_BLUEPRINT_RUN_IDS, BLUEPRINT_RUN_ID_WORDS)
}

/// One run of a blueprint, as the `StartBlueprintRun` that began it recorded it: "The
/// details of a blueprint run" (`BlueprintRun`).
///
/// Everything the modelled `BlueprintRun` shape reports is held here: the blueprint the run
/// is of and the id that addresses it within that blueprint, the `Parameters` and `RoleArn`
/// the start carried, and the state the run is in with the instants it moved between.
#[derive(Debug, Clone)]
pub struct BlueprintRunEntry {
    /// `BlueprintRun$RunId`, minted by [`new_blueprint_run_id`].
    pub run_id: String,
    /// `BlueprintRun$BlueprintName`: the blueprint this is a run of, which is also where the
    /// record lives ([`BlueprintEntry::runs`]).
    pub blueprint_name: String,
    /// `BlueprintRun$Parameters`: "The blueprint parameters as a string", exactly as the
    /// start sent them, and `None` for a start that carried none.
    pub parameters: Option<String>,
    /// `BlueprintRun$RoleArn`: "The role ARN. This role will be assumed by the Glue service
    /// and will be used to create the workflow and other entities of a workflow", which the
    /// start is `@required` to carry.
    pub role_arn: String,
    /// `BlueprintRun$StartedOn`: "The date and time that the blueprint run started."
    pub started_on: SystemTime,
    /// `BlueprintRun$CompletedOn`: "The date and time that the blueprint run completed",
    /// server-set once the run reaches a terminal state and absent until then.
    pub completed_on: Option<SystemTime>,
    /// One of the modelled `BlueprintRunState` values: the state this run is in.
    pub state: String,
    /// The instant the clock owes this run its next state; `None` once it has settled.
    pub settles_at: Option<SystemTime>,
    /// The `BlueprintRunState` the clock owes this run at [`Self::settles_at`].
    pub settles_to: String,
    /// `BlueprintRun$ErrorMessage`: "Indicates any errors that are seen while running the
    /// blueprint", which a run carries only once something has gone wrong with it.
    pub error_message: Option<String>,
    /// The `ErrorMessage` the clock gives this run when it moves it into
    /// [`Self::settles_to`], and `None` for a run whose layout went through.
    pub settles_with_error: Option<String>,
    /// `BlueprintRun$WorkflowName`: "The name of a workflow that is created as a result of a
    /// successful blueprint run", and `None` for a run that created none — "if a blueprint
    /// run has an error, there will not be a workflow created".
    pub workflow_name: Option<String>,
}

impl BlueprintRunEntry {
    /// Record the run a `StartBlueprintRun` describes, in [`BLUEPRINT_RUN_STATE_RUNNING`]
    /// with its next state due [`BLUEPRINT_RUN_SETTLES_AFTER`] out — "Running — The
    /// blueprint run is in progress" — and the members the start carried taken from the
    /// request.
    ///
    /// `layout` is what the run's layout of the workflow it produces came to, which is what
    /// decides the state the clock owes the run: the run that created a workflow succeeds
    /// carrying it, and one that created none fails carrying why.
    pub fn begin(
        input: &StartBlueprintRunInput,
        run_id: String,
        now: SystemTime,
        layout: &BlueprintRunLayout,
    ) -> Self {
        let (settles_to, workflow_name, settles_with_error) = match layout {
            BlueprintRunLayout::Created(workflow_name) => (
                BLUEPRINT_RUN_STATE_SUCCEEDED,
                Some(workflow_name.clone()),
                None,
            ),
            BlueprintRunLayout::WorkflowAlreadyExists(workflow_name) => (
                BLUEPRINT_RUN_STATE_FAILED,
                None,
                Some(messages::blueprint_run_workflow_exists(workflow_name)),
            ),
        };
        Self {
            run_id,
            blueprint_name: input.blueprint_name.clone(),
            parameters: input.parameters.clone(),
            role_arn: input.role_arn.clone(),
            started_on: now,
            completed_on: None,
            state: BLUEPRINT_RUN_STATE_RUNNING.to_string(),
            settles_at: Some(now.checked_add(BLUEPRINT_RUN_SETTLES_AFTER).unwrap_or(now)),
            settles_to: settles_to.to_string(),
            // Nothing has gone wrong yet: a run carries an error message only once it has.
            error_message: None,
            settles_with_error,
            workflow_name,
        }
    }

    /// Whether this run has finished, by [`TERMINAL_BLUEPRINT_RUN_STATES`].
    pub fn is_terminal(&self) -> bool {
        TERMINAL_BLUEPRINT_RUN_STATES.contains(&self.state.as_str())
    }

    /// Move this run into the state the clock owed it at `now`, if it owed one.
    ///
    /// The run was due to leave the state the start put it in at [`Self::settles_at`]
    /// whether or not anybody called, and the instants recorded are that DUE instant, not
    /// the instant somebody read it — so two reads of a settled run report the same run.
    pub fn settle_if_due(&mut self, now: SystemTime) {
        let Some(due) = self.settles_at else {
            return;
        };
        if due > now {
            return;
        }
        self.settles_at = None;
        self.state = self.settles_to.clone();
        // "Indicates any errors that are seen while running the blueprint": the run that
        // comes to rest without having created a workflow carries the reason it did not.
        self.error_message = self.settles_with_error.clone();
        if self.is_terminal() {
            self.completed_on = Some(due);
        }
    }

    /// Rebuild the modelled `BlueprintRun` shape from what is stored, so every reader of a
    /// run reports the same run.
    pub fn to_blueprint_run(&self) -> BlueprintRun {
        BlueprintRun {
            blueprint_name: Some(self.blueprint_name.clone()),
            run_id: Some(self.run_id.clone()),
            // "The name of a workflow that is created as a result of a successful blueprint
            // run": the workflow the run laid out, which exists from the instant the run
            // created it, and nothing at all for a run that created none — "if a blueprint
            // run has an error, there will not be a workflow created".
            workflow_name: self.workflow_name.clone(),
            state: Some(self.state.clone()),
            started_on: Some(to_date_time(self.started_on)),
            completed_on: self.completed_on.map(to_date_time),
            error_message: self.error_message.clone(),
            // "If there are any errors while creating the entities of a workflow, we try to
            // roll back the created entities until that point and delete them": a run that
            // created no workflow created no entity either, so nothing was rolled back and no
            // rollback error arose.
            rollback_error_message: None,
            parameters: self.parameters.clone(),
            role_arn: Some(self.role_arn.clone()),
        }
    }
}
