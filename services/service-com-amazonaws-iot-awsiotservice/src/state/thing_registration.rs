// SPDX-License-Identifier: CC0-1.0

//! BULK THING PROVISIONING: what a thing-registration task DOES, and what a reader of one
//! reports about it.
//!
//! `StartThingRegistrationTask` "creates a bulk thing provisioning task" out of three
//! caller-supplied values: an object of the emulator's own Amazon S3 ("The name of input
//! file within the S3 bucket. This file contains a newline delimited JSON file. Each line
//! contains the parameter values to provision one device (thing)"), the provisioning
//! template those parameter values are substituted into, and the IAM role AWS IoT reads the
//! object under. So one line of that object provisions ONE thing, and the thing it
//! provisions is the one the template's `AWS::IoT::Thing` resource describes once the line's
//! parameters are substituted into its properties.
//!
//! The thing a line provisions is registered in the SAME thing registry `CreateThing`
//! writes ([`super::ComAmazonawsIotAwsiotserviceState::things`]), under the same rules: a
//! `ThingTypeName` and a `BillingGroup` resolve against the records the account holds, a
//! name already registered with the same configuration is the thing that is already there,
//! and one registered with a different configuration is a line that cannot be provisioned.
//! A reader of a thing therefore cannot tell how the thing got into the registry, which is
//! the point of the bulk path.
//!
//! What the TASK reports is the run of those lines: how many things it provisioned, how many
//! lines it could not provision, and how far it has got ([`ThingRegistrationTask`]). The run
//! is performed by the call that starts it, and the task is reported in the non-terminal
//! status AWS answers a fresh one in until the clock makes its completion due
//! ([`REGISTRATION_SPAN`]) — the due instant is recorded, never slept for, so a reader
//! derives the status from the clock and writes nothing.
//!
//! The object is read through the emulator's own in-process transport (the operation
//! callback), so the read never leaves this process, and HOW MUCH is read is bounded twice:
//! against the length Amazon S3's own answer declares, before a byte reaches this process,
//! and against the bytes that arrive ([`MAX_INPUT_FILE_BYTES`]).

use std::collections::BTreeMap;
use std::time::{Duration, SystemTime};

use ::serde_json::{Map, Value};
use ::service_common::iot_job_executions::is_modeled_thing_name;

use super::{ComAmazonawsIotAwsiotserviceState, ThingConfiguration};
use crate::ops::types::DescribeThingRegistrationTaskOutput;

/// How long after `StartThingRegistrationTask` the task it started is reported complete.
///
/// A fresh task is answered in a NON-TERMINAL status ([`TASK_STATUS_IN_PROGRESS`]), so the
/// clock owes its move to a terminal one; the span is the emulator's own, chosen so a caller
/// describing the task straight after the start still observes the run in progress, and the
/// due instant is recorded rather than slept for.
pub const REGISTRATION_SPAN: Duration = Duration::from_secs(3);

/// The status a task is in while its run is still owed: `Status`'s non-terminal value
/// ([`crate::ops::types::STATUS_VALUES`]).
pub const TASK_STATUS_IN_PROGRESS: &str = "InProgress";

/// The status a task that performed its run holds.
pub const TASK_STATUS_COMPLETED: &str = "Completed";

/// The status a task whose run could not be performed at all holds — the input file was not
/// readable, so no line of it was ever provisioned.
pub const TASK_STATUS_FAILED: &str = "Failed";

/// `percentageProgress` for a run that is complete: "The progress of the bulk provisioning
/// task expressed as a percentage", and every line of the file has been attempted.
const PERCENTAGE_COMPLETE: i32 = 100;

/// `percentageProgress` for a run that has got nowhere: one still owed, and one that could
/// not be performed at all.
const PERCENTAGE_NONE: i32 = 0;

/// Most bytes of the input object this emulator reads into the process for one task.
///
/// A caller names the object, so the read has to be bounded by something other than the
/// caller's own file: this is the emulator's ceiling, far above any file a client under test
/// stages, and a task naming a larger object is reported `Failed` rather than read.
pub const MAX_INPUT_FILE_BYTES: usize = 1 << 20;

/// Most lines — things — one task provisions. The emulator's own ceiling on the work ONE
/// request does, beside the byte ceiling above; the registry those things go into carries its
/// own ([`super::MAX_THINGS`]), so one task cannot fill it either.
pub const MAX_INPUT_FILE_LINES: usize = 1_000;

/// Most thing-registration tasks the account's store holds.
///
/// A caller grows this store one `StartThingRegistrationTask` at a time, so it is bounded
/// like every other store a caller grows ([`super::MAX_TOPIC_RULES`],
/// [`super::MAX_THING_TYPES`]).
pub const MAX_THING_REGISTRATION_TASKS: usize = 1_000;

/// The resource type of the template resource that describes the THING a line provisions.
/// A provisioning template's `Resources` may also describe the certificate and the policy a
/// device is provisioned with; those are resources this emulator's registry does not hold,
/// so they are left alone rather than approximated.
const THING_RESOURCE_TYPE: &str = "AWS::IoT::Thing";

/// The members of a provisioning template this reading takes: the declared `Parameters`
/// (each with the `Default` it may carry) and the `Resources`, each of which states its
/// `Type` and its `Properties`.
const PARAMETERS_MEMBER: &str = "Parameters";
const DEFAULT_MEMBER: &str = "Default";
const RESOURCES_MEMBER: &str = "Resources";
const TYPE_MEMBER: &str = "Type";
const PROPERTIES_MEMBER: &str = "Properties";

/// The intrinsic function a template property substitutes a parameter value with:
/// `{"Ref": "<parameter name>"}`.
const REF_FUNCTION: &str = "Ref";

/// The properties of the `AWS::IoT::Thing` resource this registry can provision: the thing's
/// name, its attributes, its thing type and its billing group — which is exactly the
/// `CreateThing` request a line amounts to.
const THING_NAME_PROPERTY: &str = "ThingName";
const ATTRIBUTE_PAYLOAD_PROPERTY: &str = "AttributePayload";
const THING_TYPE_NAME_PROPERTY: &str = "ThingTypeName";
const BILLING_GROUP_PROPERTY: &str = "BillingGroup";

/// The members of the modelled `AttributePayload` shape `CreateThing` takes: the attribute
/// map, and the `merge` flag ("only valid when calling `UpdateThing` or `UpdateThingGroup`").
/// A payload carrying either states that SHAPE; a provisioning template states the attribute
/// map itself, which is a payload carrying neither.
const ATTRIBUTES_MEMBER: &str = "attributes";
const MERGE_MEMBER: &str = "merge";

/// What a task reports as its `message` when the input file could not be read.
///
/// It names what the service could not do and what the caller should check, and echoes no
/// value the request carried: the caller already knows the bucket and key it sent.
pub const UNREADABLE_INPUT_FILE_MESSAGE: &str =
    "Unable to read the input file from S3. Check the bucket, the key and the permissions \
     granted by the role.";

/// What a task reports as its `message` when the input file is not text at all, so no line
/// of it can carry the JSON parameter values of a device.
pub const NON_TEXT_INPUT_FILE_MESSAGE: &str =
    "Unable to read the input file: it is not a newline delimited JSON text file.";

/// What a task's `ERRORS` report states for a line that provisioned no thing because the LINE
/// describes none: it is no JSON object of parameter values, or the thing resource of the
/// template cannot be rendered from the values it carries (a parameter the template gives no
/// `Default` for, or a `ThingName` the registry cannot be addressed by).
///
/// Fixed text about the line, with nothing of the line itself echoed into it. A line the
/// registry REFUSED carries the registry's own reason instead
/// ([`super::ThingRegistrationRefusal::message`]).
pub const UNPROVISIONABLE_LINE_MESSAGE: &str =
    "The line provisions no thing: it is not a JSON object of parameter values the thing \
     resource of the provisioning template can be rendered from.";

/// What `StartThingRegistrationTask` refuses a `roleArn` with when IAM answered that this
/// service cannot assume it: the role is what AWS IoT reads the input file under, so a role
/// it cannot assume is a request it cannot perform, and nothing is recorded for it.
///
/// Fixed text, as AWS IoT words this failure: it is about the role the request named, which
/// the caller already sent, so nothing of the request is echoed into it.
pub const UNASSUMABLE_ROLE_MESSAGE: &str = "Cannot assume provided role";

/// What `StartThingRegistrationTask` refuses with once the account holds
/// [`MAX_THING_REGISTRATION_TASKS`] tasks.
///
/// The operation models no limit-exceeded error — its errors are `InternalFailureException`,
/// `InvalidRequestException`, `ThrottlingException` and `UnauthorizedException` — so the
/// refusal past the ceiling is the modelled `InvalidRequestException` naming the limit, in
/// the wording this crate already refuses its other caller-grown stores past (see
/// `CreateThingType` and [`super::MAX_THING_TYPES`]).
pub fn too_many_tasks_message() -> String {
    format!(
        "the maximum of {MAX_THING_REGISTRATION_TASKS} thing registration tasks is already \
         reached"
    )
}

/// What a task reports as its `message` when the input file holds more lines than one task
/// provisions.
pub fn too_many_lines_message() -> String {
    format!(
        "Unable to provision the input file: a bulk thing provisioning task provisions at \
         most {MAX_INPUT_FILE_LINES} things."
    )
}

/// What a task reports as its `message` when the input object is larger than this emulator
/// reads for one task.
pub fn oversized_input_file_message() -> String {
    format!(
        "Unable to read the input file: the maximum size of a bulk thing provisioning input \
         file is {MAX_INPUT_FILE_BYTES} bytes."
    )
}

/// Why a `templateBody` is no provisioning template at all — a fact about the REQUEST, so it
/// is refused rather than recorded as a task that fails.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TemplateRejection {
    /// The body does not read as a JSON object.
    NotAnObject,
    /// It declares no `Resources`, which is where a template says what to provision.
    NoResources,
    /// Its `Resources` describe no [`THING_RESOURCE_TYPE`] with `Properties`, so it
    /// provisions no thing.
    NoThingResource,
}

impl TemplateRejection {
    /// What the refusal reports. Fixed text: the fault is in the template's structure, and
    /// nothing of the caller's body is echoed back into the message.
    pub fn message(self) -> &'static str {
        match self {
            Self::NotAnObject => "Invalid template: the provisioning template is not a JSON object",
            Self::NoResources => "Invalid template: the provisioning template declares no Resources",
            Self::NoThingResource => {
                "Invalid template: the provisioning template declares no AWS::IoT::Thing resource"
            }
        }
    }
}

/// A provisioning template, as much of it as this registry provisions: the `Default` each
/// declared parameter carries, and the properties of its `AWS::IoT::Thing` resource.
#[derive(Debug, Clone)]
pub struct ProvisioningTemplate {
    /// `Parameters`: the declared name of each parameter that carries a `Default`, and that
    /// default — what a line which does not supply the parameter provisions with.
    defaults: BTreeMap<String, String>,
    /// The `Properties` of the template's thing resource, with their `Ref`s unsubstituted:
    /// the same properties serve every line of the file.
    thing_properties: Map<String, Value>,
}

impl ProvisioningTemplate {
    /// Read `body` as a provisioning template, or say why it is none.
    pub fn read(body: &str) -> Result<Self, TemplateRejection> {
        let Ok(Value::Object(template)) = ::serde_json::from_str::<Value>(body) else {
            return Err(TemplateRejection::NotAnObject);
        };
        let defaults = template
            .get(PARAMETERS_MEMBER)
            .and_then(Value::as_object)
            .map(declared_defaults)
            .unwrap_or_default();
        let Some(resources) = template.get(RESOURCES_MEMBER).and_then(Value::as_object) else {
            return Err(TemplateRejection::NoResources);
        };
        let thing_properties = resources
            .values()
            .filter(|resource| {
                resource.get(TYPE_MEMBER).and_then(Value::as_str) == Some(THING_RESOURCE_TYPE)
            })
            .find_map(|resource| resource.get(PROPERTIES_MEMBER).and_then(Value::as_object))
            .ok_or(TemplateRejection::NoThingResource)?
            .clone();
        Ok(Self {
            defaults,
            thing_properties,
        })
    }

    /// The thing ONE line of the input file provisions: this template's thing properties with
    /// the line's `parameters` substituted into them.
    ///
    /// `None` when the line provisions no thing: a property whose `Ref` names a parameter the
    /// line does not supply and the template gives no default for, a property this reading
    /// cannot render as the value it means, a missing `ThingName`, or one that is no
    /// `ThingName` the registry can be addressed by.
    pub fn provisioned_thing(&self, parameters: &Map<String, Value>) -> Option<ProvisionedThing> {
        let substitutions = self.substitutions(parameters);
        let thing_name = self.rendered(THING_NAME_PROPERTY, &substitutions)?;
        if !is_modeled_thing_name(&thing_name) {
            return None;
        }
        let thing_type_name = match self.thing_properties.get(THING_TYPE_NAME_PROPERTY) {
            Some(property) => Some(render_text(property, &substitutions)?),
            None => None,
        };
        let billing_group_name = match self.thing_properties.get(BILLING_GROUP_PROPERTY) {
            Some(property) => Some(render_text(property, &substitutions)?),
            None => None,
        };
        Some(ProvisionedThing {
            thing_name,
            configuration: ThingConfiguration {
                thing_type_name,
                attributes: self.rendered_attributes(&substitutions)?,
                billing_group_name,
            },
        })
    }

    /// The value each `Ref` of the thing properties resolves to for one line: the parameter
    /// values the line supplies, over the defaults the template declares.
    fn substitutions(&self, parameters: &Map<String, Value>) -> BTreeMap<String, String> {
        let mut substitutions = self.defaults.clone();
        for (name, value) in parameters {
            if let Some(text) = scalar_text(value) {
                substitutions.insert(name.clone(), text);
            }
        }
        substitutions
    }

    /// The property `name` rendered for one line, or `None` when the template states none or
    /// states one this reading cannot render.
    fn rendered(
        &self,
        name: &str,
        substitutions: &BTreeMap<String, String>,
    ) -> Option<String> {
        render_text(self.thing_properties.get(name)?, substitutions)
    }

    /// The thing's attributes for one line.
    ///
    /// A template states them under `AttributePayload`, as the attribute map itself. The
    /// modelled `AttributePayload` shape `CreateThing` takes holds that map under its own
    /// `attributes` member, so a payload stating that shape ([`ATTRIBUTES_MEMBER`],
    /// [`MERGE_MEMBER`]) is read through it and the two forms provision the same thing —
    /// which also keeps a modelled member of the shape from being provisioned as an attribute
    /// of its own.
    ///
    /// `None` when an attribute's value is one this reading cannot render, which is a line
    /// that provisions nothing rather than a thing with an attribute left out.
    fn rendered_attributes(
        &self,
        substitutions: &BTreeMap<String, String>,
    ) -> Option<BTreeMap<String, String>> {
        let Some(payload) = self.thing_properties.get(ATTRIBUTE_PAYLOAD_PROPERTY) else {
            return Some(BTreeMap::new());
        };
        let payload = payload.as_object()?;
        let none = Map::new();
        let attributes = match (
            payload.get(ATTRIBUTES_MEMBER),
            payload.contains_key(MERGE_MEMBER),
        ) {
            // The modelled shape: the map is the member's value, and a member that is no map
            // is a payload this reading cannot take.
            (Some(stated), _) => stated.as_object()?,
            // The modelled shape stating no attributes at all.
            (None, true) => &none,
            // The template states the attribute map itself.
            (None, false) => payload,
        };
        attributes
            .iter()
            .map(|(name, value)| render_text(value, substitutions).map(|text| (name.clone(), text)))
            .collect()
    }
}

/// The `Default` of each declared parameter that carries one.
fn declared_defaults(parameters: &Map<String, Value>) -> BTreeMap<String, String> {
    parameters
        .iter()
        .filter_map(|(name, declaration)| {
            let default = declaration.get(DEFAULT_MEMBER).and_then(scalar_text)?;
            Some((name.clone(), default))
        })
        .collect()
}

/// One template property rendered for one line: the parameter a `{"Ref": "<name>"}` names,
/// or the literal value the template states.
///
/// `None` when the value is one this reading cannot render as the text the property means: a
/// `Ref` to a parameter with no value, any other intrinsic function, and a structure or list
/// where a property's value is a scalar.
fn render_text(value: &Value, substitutions: &BTreeMap<String, String>) -> Option<String> {
    match value {
        Value::Object(function) if function.len() == 1 => {
            let parameter = function.get(REF_FUNCTION)?.as_str()?;
            substitutions.get(parameter).cloned()
        }
        scalar => scalar_text(scalar),
    }
}

/// A JSON scalar as the text a template property or a parameter value means. A structure, a
/// list and `null` are not scalars, so they render nothing.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

/// The thing one line of the input file provisions: the `CreateThing` that line amounts to.
#[derive(Debug, Clone)]
pub struct ProvisionedThing {
    /// The name the thing is registered under — the registry's key.
    pub thing_name: String,
    /// Everything about the thing that is not its name, in the form the registry keeps and
    /// compares it ([`ThingConfiguration`]).
    pub configuration: ThingConfiguration,
}

/// What the run made of ONE line of the input file: which line it was, and either the thing
/// that line provisioned or the reason it provisioned none.
///
/// The counts a reader of the task reports are these records counted
/// ([`RegistrationOutcome`]), and the task's own REPORT objects are these records written out
/// ([`super::thing_registration_reports`]) — so what the task says it provisioned and what
/// its report lists cannot drift.
#[derive(Debug, Clone)]
pub struct LineReport {
    /// Which line of the input file this is about, counted from 1 as a reader of the file
    /// counts its lines — blank ones included, so the number addresses the line itself.
    pub line_number: usize,
    /// The name of the thing the line provisioned, or the reason it provisioned none.
    pub provisioned: Result<String, String>,
}

/// What a task's run amounted to: the counts a reader reports, what the run made of each
/// line, and the reason the run could not be performed at all when it could not.
#[derive(Debug, Clone, Default)]
pub struct RegistrationOutcome {
    /// "The number of things successfully provisioned."
    pub success_count: i32,
    /// "The number of things that failed to be provisioned."
    pub failure_count: i32,
    /// What the run made of each line of the input file, in the file's own line order: the
    /// records the task's reports are written from, and what the two counts above count. A
    /// run that could not be performed at all made nothing of any line, so it has none.
    pub lines: Vec<LineReport>,
    /// Why no line was provisioned, for a run that could not be performed — the task's
    /// `message`, and what makes its status [`TASK_STATUS_FAILED`].
    pub failure_message: Option<String>,
}

impl RegistrationOutcome {
    /// A run that was not performed at all, for the stated reason.
    pub fn unperformed(reason: String) -> Self {
        Self {
            success_count: 0,
            failure_count: 0,
            lines: Vec::new(),
            failure_message: Some(reason),
        }
    }
}

/// Read every line of `input_file` into the thing it provisions under `template`: `None` for
/// a line that provisions nothing, which the task counts as a failure rather than refusing.
///
/// One line is one thing ("Each line contains the parameter values to provision one device
/// (thing)"). This is the whole READ of the file, and it touches no shared state, so it runs
/// with NO store lock held: what the registry then has to do under the lock is the inserts
/// alone ([`register_planned`]).
///
/// `Err` is a run that was not performed at all — a file this reading cannot take line by
/// line — carrying what the task reports for it.
pub fn planned_things(
    template: &ProvisioningTemplate,
    input_file: &[u8],
) -> Result<Vec<PlannedLine>, RegistrationOutcome> {
    let Ok(text) = ::std::str::from_utf8(input_file) else {
        return Err(RegistrationOutcome::unperformed(
            NON_TEXT_INPUT_FILE_MESSAGE.to_string(),
        ));
    };
    // A newline delimited JSON file: every line is one device, and a blank line is no
    // device at all (a trailing newline ends the last line rather than starting an empty
    // one).
    let lines: Vec<(usize, &str)> = text
        .lines()
        // ENUMERATED before the blank ones are dropped, so a row of the task's report names
        // the line of the FILE it is about ([`PlannedLine::line_number`]).
        .enumerate()
        .map(|(index, line)| (index + 1, line))
        .filter(|(_, line)| !line.trim().is_empty())
        .collect();
    if lines.len() > MAX_INPUT_FILE_LINES {
        return Err(RegistrationOutcome::unperformed(too_many_lines_message()));
    }
    Ok(lines
        .into_iter()
        .map(|(line_number, line)| PlannedLine {
            line_number,
            thing: planned_thing(template, line),
        })
        .collect())
}

/// One line of the input file, and the thing it provisions: `None` for a line that provisions
/// nothing, which the task counts as a failure and reports as one rather than refusing.
#[derive(Debug, Clone)]
pub struct PlannedLine {
    /// Which line of the input file this is, counted from 1 — see [`LineReport::line_number`].
    pub line_number: usize,
    /// The thing the line provisions, or `None` when it describes none.
    pub thing: Option<ProvisionedThing>,
}

/// The thing the one line `line` provisions, or `None` when it provisions none.
fn planned_thing(template: &ProvisioningTemplate, line: &str) -> Option<ProvisionedThing> {
    // "Each line contains the parameter values to provision one device": a line that is not
    // a JSON object of parameter values provisions nothing.
    let Ok(Value::Object(parameters)) = ::serde_json::from_str::<Value>(line) else {
        return None;
    };
    template.provisioned_thing(&parameters)
}

/// Register the things the file's lines planned, and answer what the run amounted to.
///
/// A line is provisioned exactly where the `CreateThing` it amounts to would succeed, because
/// it IS that registry write: the one
/// [`ComAmazonawsIotAwsiotserviceState::register_thing`] every writer of a thing makes, which
/// resolves the thing type and the billing group against the records the account holds, holds
/// the registry to its own ceiling ([`super::MAX_THINGS`]) and takes a name already registered
/// with the same configuration as the thing that is already there — so re-running a task over
/// the same file provisions the same things rather than failing every line.
///
/// The counts are per LINE: a line that planned no thing, and one whose thing that write
/// refuses, are each one failure, so one bad line does not cost the others.
pub fn register_planned(
    store: &mut ComAmazonawsIotAwsiotserviceState,
    region: &str,
    planned: Vec<PlannedLine>,
) -> RegistrationOutcome {
    let mut outcome = RegistrationOutcome::default();
    for line in planned {
        // The reason a line was not provisioned is the registry's own
        // ([`super::ThingRegistrationRefusal`]) where the write refused it, and the one fixed
        // sentence below where the line described no thing to write at all — which is what
        // the task's `ERRORS` report states for that line.
        let provisioned = match line.thing {
            None => Err(UNPROVISIONABLE_LINE_MESSAGE.to_string()),
            Some(thing) => store
                .register_thing(region, thing.thing_name, thing.configuration)
                .map(|registered| registered.thing_name)
                .map_err(|refusal| refusal.message()),
        };
        if provisioned.is_ok() {
            outcome.success_count += 1;
        } else {
            outcome.failure_count += 1;
        }
        outcome.lines.push(LineReport {
            line_number: line.line_number,
            provisioned,
        });
    }
    outcome
}

/// One bulk thing provisioning task, as this service persists it.
///
/// It holds what `DescribeThingRegistrationTask` reports member for member — the id is its
/// identity, the bucket, key, role and template body are what the start supplied, and the
/// counts are what its run amounted to — so a reader clones this record and the writer and
/// the reader cannot drift.
#[derive(Debug, Clone)]
pub struct ThingRegistrationTask {
    /// The id the task was started under; also its key in the store.
    pub task_id: String,
    /// "The provisioning template", verbatim as the start supplied it.
    pub template_body: String,
    /// "The S3 bucket that contains the input file."
    pub input_file_bucket: String,
    /// "The name of input file within the S3 bucket."
    pub input_file_key: String,
    /// "The role ARN that grants access to the input file bucket."
    pub role_arn: String,
    /// "The task creation date" — the instant of the start.
    pub creation_date: SystemTime,
    /// When the run the start kicked off is complete: the transition the clock already owes,
    /// whether or not anybody describes the task.
    pub completed_at: SystemTime,
    /// What the run amounted to ([`RegistrationOutcome`]), reported from `completed_at` on.
    pub outcome: RegistrationOutcome,
    /// Where this task's REPORT objects were written — the bucket of the emulator's own
    /// Amazon S3 they are in and the key of each one
    /// ([`super::thing_registration_reports::ReportObjects`]), which is what
    /// `ListThingRegistrationTaskReports` hands back a link per.
    pub reports: super::thing_registration_reports::ReportObjects,
}

impl ThingRegistrationTask {
    /// Record a task started at `started_at` whose run amounted to `outcome`, with its
    /// completion due [`REGISTRATION_SPAN`] later.
    pub fn new(
        task_id: String,
        template_body: String,
        input_file_bucket: String,
        input_file_key: String,
        role_arn: String,
        started_at: SystemTime,
        outcome: RegistrationOutcome,
    ) -> Self {
        Self {
            task_id,
            template_body,
            input_file_bucket,
            input_file_key,
            role_arn,
            creation_date: started_at,
            completed_at: started_at + REGISTRATION_SPAN,
            outcome,
            // Written by the call that performs the run, and recorded on the task once the
            // store has taken them ([`super::thing_registration_reports::write_report_objects`]).
            reports: super::thing_registration_reports::ReportObjects::default(),
        }
    }

    /// Whether the run this task was answered in the middle of is still owed at `now`.
    fn running_at(&self, now: SystemTime) -> bool {
        now < self.completed_at
    }

    /// "The status of the bulk thing provisioning task" at `now`: the non-terminal status it
    /// was answered in until the clock makes its completion due, and from that instant the
    /// terminal one its run amounted to.
    pub fn status_at(&self, now: SystemTime) -> &'static str {
        if self.running_at(now) {
            TASK_STATUS_IN_PROGRESS
        } else if self.outcome.failure_message.is_some() {
            TASK_STATUS_FAILED
        } else {
            TASK_STATUS_COMPLETED
        }
    }

    /// This task as the response a reader hands back at `now`.
    ///
    /// The counts and the progress are the run's, reported from the instant the run is
    /// complete: until then the task is still running, so it reports no outcome. A run that
    /// could not be performed at all has got nowhere, so its progress stays
    /// [`PERCENTAGE_NONE`] and it reports the reason as its `message`.
    pub fn description_at(&self, now: SystemTime) -> DescribeThingRegistrationTaskOutput {
        let running = self.running_at(now);
        let failed = self.outcome.failure_message.is_some();
        DescribeThingRegistrationTaskOutput {
            task_id: Some(self.task_id.clone()),
            creation_date: Some(::aws_smithy_types::DateTime::from(self.creation_date)),
            // The last change the task made: its own start while the run is still owed, and
            // the completion's own instant from then on — never the moment of the read.
            last_modified_date: Some(::aws_smithy_types::DateTime::from(if running {
                self.creation_date
            } else {
                self.completed_at
            })),
            template_body: Some(self.template_body.clone()),
            input_file_bucket: Some(self.input_file_bucket.clone()),
            input_file_key: Some(self.input_file_key.clone()),
            role_arn: Some(self.role_arn.clone()),
            status: Some(self.status_at(now).to_string()),
            message: if running {
                None
            } else {
                self.outcome.failure_message.clone()
            },
            success_count: if running { 0 } else { self.outcome.success_count },
            failure_count: if running { 0 } else { self.outcome.failure_count },
            percentage_progress: if running || failed {
                PERCENTAGE_NONE
            } else {
                PERCENTAGE_COMPLETE
            },
        }
    }
}

/// What this service established about the bytes of a task's input file.
#[derive(Debug)]
pub enum InputFile {
    /// The file's bytes.
    Read(Vec<u8>),
    /// The bytes cannot be obtained, and this is what the task reports as its `message`.
    Unreadable(String),
}

/// The bytes of the object `bucket` and `key` name, read from the emulator's own Amazon S3.
///
/// Nothing here opens a socket or leaves the host: the client below rides the operation
/// callback, the emulator's in-process channel into its own services.
pub async fn input_file(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
    bucket: &str,
    key: &str,
) -> InputFile {
    let read = s3_client(ctx, region)
        .get_object()
        .bucket(bucket)
        .key(key)
        .send()
        .await;
    let read = match read {
        Ok(read) => read,
        Err(error) => {
            // The caller's own values are rendered in their escaped `Debug` form, so a
            // bucket or key carrying a newline cannot forge a log line.
            ::tracing::warn!(
                bucket = ?bucket,
                key = ?key,
                error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                "a bulk thing provisioning input file could not be read from the emulator's \
                 Amazon S3"
            );
            return InputFile::Unreadable(UNREADABLE_INPUT_FILE_MESSAGE.to_string());
        }
    };
    // The ceiling, against what the store SAYS the object holds, BEFORE any of it is read
    // into this process.
    if read
        .content_length()
        .and_then(|length| usize::try_from(length).ok())
        .is_some_and(|declared| declared > MAX_INPUT_FILE_BYTES)
    {
        return InputFile::Unreadable(oversized_input_file_message());
    }
    // Then the bytes, taken a chunk at a time and abandoned the moment they pass the
    // ceiling, so what is allocated here is bounded whatever the answer declared — an
    // answer that declared no length at all included.
    let mut body = read.body;
    let mut bytes: Vec<u8> = Vec::new();
    loop {
        match body.try_next().await {
            Ok(Some(chunk)) => {
                if bytes.len() + chunk.len() > MAX_INPUT_FILE_BYTES {
                    return InputFile::Unreadable(oversized_input_file_message());
                }
                bytes.extend_from_slice(&chunk);
            }
            Ok(None) => return InputFile::Read(bytes),
            Err(error) => {
                ::tracing::warn!(
                    bucket = ?bucket,
                    key = ?key,
                    error = %error,
                    "the bytes of a bulk thing provisioning input file did not arrive from \
                     the emulator's Amazon S3"
                );
                return InputFile::Unreadable(UNREADABLE_INPUT_FILE_MESSAGE.to_string());
            }
        }
    }
}

/// The Amazon S3 client a task's input file is read with, addressed to `region`.
///
/// The transport is the emulator's own in-process callback, so the `endpoint_url` below is an
/// ADDRESS the in-process router reads rather than a host any resolver looks up: this client
/// reaches the emulator's own Amazon S3 and nothing else, whatever bucket or key a caller
/// names. The one caller-supplied value that reaches its authority is the signing region, and
/// it is narrowed before it gets here — [`super::request_region`] admits only a value shaped
/// like a Region identifier and otherwise answers the default Region.
///
/// There is nothing to retry, time out or watch for a stalled stream, each of which would
/// need a sleep implementation this client is not given. The same goes for caching the
/// identity: the credentials the callback hands over are used directly, and they only sign
/// this operation's in-process calls.
pub(super) fn s3_client(
    ctx: &::smithy_server_runtime::OperationContext<'_>,
    region: &str,
) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The in-process transport routes on the request's own path, and a virtual-host
        // bucket name is not a host this emulator resolves, so the request addresses its
        // bucket in the path.
        .force_path_style(true)
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .http_client(ctx.callback().http_client())
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled())
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

#[cfg(test)]
mod tests {
    use super::{
        planned_things, register_planned, too_many_lines_message, PlannedLine, ProvisionedThing,
        ProvisioningTemplate, TemplateRejection, MAX_INPUT_FILE_LINES,
    };
    use crate::state::{
        ComAmazonawsIotAwsiotserviceState, RegisteredThing, ThingConfiguration,
        ThingRegistrationRefusal, MAX_THINGS,
    };
    use ::serde_json::{json, Map, Value};

    /// The parameter values of one line of an input file.
    fn line(values: Value) -> Map<String, Value> {
        values.as_object().expect("an object of values").clone()
    }

    /// A template's thing properties are rendered per line: a `Ref` takes the line's value,
    /// a parameter the line omits takes the template's `Default`, and a literal property is
    /// the same for every line.
    #[test]
    fn a_line_provisions_the_thing_the_template_describes() {
        let template = ProvisioningTemplate::read(
            &json!({
                "Parameters": {
                    "SerialNumber": { "Type": "String" },
                    "Location": { "Type": "String", "Default": "WA" },
                },
                "Resources": {
                    "thing": {
                        "Type": "AWS::IoT::Thing",
                        "Properties": {
                            "ThingName": { "Ref": "SerialNumber" },
                            "AttributePayload": {
                                "serialNumber": { "Ref": "SerialNumber" },
                                "location": { "Ref": "Location" },
                            },
                            "ThingTypeName": "lightBulb",
                        },
                    },
                },
            })
            .to_string(),
        )
        .expect("the template reads");

        /// One row: the case, the line's parameter values, the thing name it provisions (or
        /// none) and the attributes the thing carries.
        type Row = (&'static str, Value, Option<&'static str>, Vec<(&'static str, &'static str)>);

        let rows: Vec<Row> = vec![
            (
                "a line supplying every parameter",
                json!({ "SerialNumber": "sn-1", "Location": "OR" }),
                Some("sn-1"),
                vec![("location", "OR"), ("serialNumber", "sn-1")],
            ),
            (
                "a line leaving a defaulted parameter out",
                json!({ "SerialNumber": "sn-2" }),
                Some("sn-2"),
                vec![("location", "WA"), ("serialNumber", "sn-2")],
            ),
            (
                "a numeric parameter value, which is the text of the number",
                json!({ "SerialNumber": 7, "Location": "OR" }),
                Some("7"),
                vec![("location", "OR"), ("serialNumber", "7")],
            ),
            (
                "a line leaving the name's parameter out",
                json!({ "Location": "OR" }),
                None,
                vec![],
            ),
            (
                "a name that is no ThingName the registry can be addressed by",
                json!({ "SerialNumber": "no spaces allowed" }),
                None,
                vec![],
            ),
        ];

        for (case, values, expected_name, expected_attributes) in rows {
            let provisioned = template.provisioned_thing(&line(values));
            let name = provisioned.as_ref().map(|thing| thing.thing_name.clone());
            assert_eq!(
                name.as_deref(),
                expected_name,
                "case {case:?}: expected {expected_name:?}, got {name:?}"
            );
            if let Some(thing) = provisioned {
                let attributes: Vec<(&str, &str)> = thing
                    .configuration
                    .attributes
                    .iter()
                    .map(|(name, value)| (name.as_str(), value.as_str()))
                    .collect();
                assert_eq!(
                    attributes, expected_attributes,
                    "case {case:?}: expected {expected_attributes:?}, got {attributes:?}"
                );
                assert_eq!(
                    thing.configuration.thing_type_name.as_deref(),
                    Some("lightBulb"),
                    "case {case:?}: the literal thing type is the same for every line"
                );
            }
        }
    }

    /// A body that is no provisioning template says WHICH way it is none.
    #[test]
    fn a_body_that_describes_no_thing_is_rejected() {
        // (case, the body, the rejection)
        let rows: Vec<(&str, String, TemplateRejection)> = vec![
            (
                "not JSON at all",
                "{".to_string(),
                TemplateRejection::NotAnObject,
            ),
            (
                "a JSON array",
                json!([]).to_string(),
                TemplateRejection::NotAnObject,
            ),
            (
                "an object with no Resources",
                json!({ "Parameters": {} }).to_string(),
                TemplateRejection::NoResources,
            ),
            (
                "Resources describing another resource type only",
                json!({
                    "Resources": {
                        "certificate": { "Type": "AWS::IoT::Certificate", "Properties": {} },
                    },
                })
                .to_string(),
                TemplateRejection::NoThingResource,
            ),
            (
                "a thing resource with no Properties",
                json!({ "Resources": { "thing": { "Type": "AWS::IoT::Thing" } } }).to_string(),
                TemplateRejection::NoThingResource,
            ),
        ];

        for (case, body, expected) in rows {
            let read = ProvisioningTemplate::read(&body);
            let rejection = read.as_ref().err().copied();
            assert_eq!(
                rejection,
                Some(expected),
                "case {case:?}: expected {expected:?}, got {rejection:?}"
            );
        }
    }

    /// The emulator's own ceiling on the lines ONE task provisions: a file at the ceiling is
    /// read line by line, and one past it is a run that is not performed at all.
    #[test]
    fn a_file_past_the_line_ceiling_is_not_provisioned() {
        let template = ProvisioningTemplate::read(
            &json!({
                "Parameters": { "SerialNumber": { "Type": "String" } },
                "Resources": {
                    "thing": {
                        "Type": "AWS::IoT::Thing",
                        "Properties": { "ThingName": { "Ref": "SerialNumber" } },
                    },
                },
            })
            .to_string(),
        )
        .expect("the template reads");
        let file = |lines: usize| {
            (0..lines)
                .map(|index| json!({ "SerialNumber": format!("sn-{index}") }).to_string())
                .collect::<Vec<String>>()
                .join("\n")
        };

        // (case, how many lines the file holds, how many things it plans)
        let rows: [(&str, usize, Option<usize>); 3] = [
            ("one line", 1, Some(1)),
            ("the file at the ceiling", MAX_INPUT_FILE_LINES, Some(MAX_INPUT_FILE_LINES)),
            ("one line past the ceiling", MAX_INPUT_FILE_LINES + 1, None),
        ];

        for (case, lines, expected) in rows {
            let planned = planned_things(&template, file(lines).as_bytes());
            match expected {
                Some(count) => {
                    let read = planned.as_ref().map(Vec::len).ok();
                    assert_eq!(
                        read,
                        Some(count),
                        "case {case:?}: expected {count} planned things, got {read:?}"
                    );
                }
                None => {
                    let message = planned
                        .as_ref()
                        .err()
                        .and_then(|outcome| outcome.failure_message.clone());
                    assert_eq!(
                        message,
                        Some(too_many_lines_message()),
                        "case {case:?}: expected the line-ceiling message, got {message:?}"
                    );
                }
            }
        }
    }

    /// The registry's own ceiling holds for the bulk path too: a line whose thing would grow
    /// the registry past [`MAX_THINGS`] is a failure, with nothing registered for it.
    #[test]
    fn a_line_past_the_registry_ceiling_is_a_failure() {
        let mut store = ComAmazonawsIotAwsiotserviceState::default();
        // The registry filled to its ceiling, written straight into the map so the fill costs
        // nothing: what is under test is the ceiling, not the write that reached it.
        for index in 0..MAX_THINGS {
            let thing_name = format!("held-{index}");
            store.things.insert(
                thing_name.clone(),
                RegisteredThing::new(
                    thing_name.clone(),
                    format!("id-{index}"),
                    crate::state::thing_arn("us-east-1", &thing_name),
                    ThingConfiguration::default(),
                ),
            );
        }

        let planned = vec![PlannedLine {
            line_number: 1,
            thing: Some(ProvisionedThing {
                thing_name: "past-the-ceiling".to_string(),
                configuration: ThingConfiguration::default(),
            }),
        }];
        let outcome = register_planned(&mut store, "us-east-1", planned);
        assert_eq!(
            (outcome.success_count, outcome.failure_count),
            (0, 1),
            "expected the line to fail against the full registry, got {outcome:?}"
        );
        // What the line's failure REPORTS is the registry's own reason, which is what the
        // task's `ERRORS` report then states for that line.
        let reported = outcome
            .lines
            .first()
            .map(|line| (line.line_number, line.provisioned.clone()));
        assert_eq!(
            reported,
            Some((
                1,
                Err(ThingRegistrationRefusal::RegistryFull.message())
            )),
            "expected the line's own record of the refusal, got {reported:?}"
        );
        assert_eq!(
            store.things.len(),
            MAX_THINGS,
            "expected the registry to stay at its ceiling, got {} things",
            store.things.len()
        );
        assert!(
            !store.things.contains_key("past-the-ceiling"),
            "expected nothing registered for the refused line"
        );
    }
}
