// SPDX-License-Identifier: CC0-1.0

//! The PIPELINE DEFINITION slice of this service's state: the reading of a pipeline
//! definition document — its steps, their dependencies and the parameters they
//! reference — and the sentences each violation of it is refused with.
//!
//! Moved out of [`super`] unchanged, so the parent module stays inside the project's
//! file-length limit and this one reading reads as one unit. Every name is
//! re-exported from [`super`], so a handler keeps naming `crate::state::…`.

use super::*;

/// Why SageMaker refuses this pipeline definition, or `None` when it accepts it.
///
/// A pipeline definition is a JSON document in the schema SageMaker Pipelines
/// publishes: a `Version` (the only published one is [`PIPELINE_DEFINITION_VERSION`])
/// and a `Steps` list whose entries each carry a `Name` and a `Type`, optionally
/// depending on other steps of the same definition through `DependsOn`. The service
/// resolves that dependency graph when the pipeline is written — not when it runs — so
/// a definition is refused unless
///
/// * it parses as a JSON object carrying those two keys,
/// * every step declares a name no other step in the definition declares,
/// * every `DependsOn` entry names another step the definition declares — never the
///   waiting step itself, and never a step of a kind nothing can wait on
///   ([`PIPELINE_UNDEPENDABLE_STEP_TYPES`]) — and the resulting graph is acyclic,
/// * every property reference to one of the pipeline's own parameters names a parameter
///   the definition declares, and
/// * it declares at most [`PIPELINE_MAX_STEPS`] steps and
///   [`PIPELINE_MAX_STEP_DEPENDENCIES`] dependency edges between them, the two counts
///   the walk's work is the product of — so validating a definition stays short whatever
///   the megabyte the member's modelled length admits is filled with.
///
/// The returned text is the refusal's message; the caller renders it as the 400
/// `ValidationException` that an unsatisfiable definition is refused with. Each DEFECT
/// CLASS carries its own diagnosis, so a caller matching on the reason can tell one
/// definition defect from another: see [`PIPELINE_DAG_CYCLE`],
/// [`step_cannot_depend_on_itself`], [`step_depends_on_unknown_step`],
/// [`step_cannot_depend_on_step_kind`] and [`unknown_parameter_property_reference`].
pub fn pipeline_definition_violation(definition: &str) -> Option<String> {
    let parsed = match ::serde_json::from_str::<::serde_json::Value>(definition) {
        Ok(parsed) => parsed,
        Err(error) => {
            return Some(format!("PipelineDefinition is not valid JSON: {error}"));
        }
    };
    let Some(document) = parsed.as_object() else {
        return Some(
            "PipelineDefinition must be a JSON object carrying the keys Version and Steps"
                .to_string(),
        );
    };
    match document.get("Version").and_then(::serde_json::Value::as_str) {
        None => {
            return Some(format!(
                "PipelineDefinition is missing the required key Version, whose only \
                 supported value is {PIPELINE_DEFINITION_VERSION}"
            ));
        }
        Some(version) if version != PIPELINE_DEFINITION_VERSION => {
            return Some(format!(
                "PipelineDefinition declares the unsupported Version {version}; the only \
                 supported value is {PIPELINE_DEFINITION_VERSION}"
            ));
        }
        Some(_) => {}
    }
    let Some(steps) = document.get("Steps").and_then(::serde_json::Value::as_array) else {
        return Some(
            "PipelineDefinition is missing the required key Steps, a list of the pipeline's \
             steps"
                .to_string(),
        );
    };
    if steps.is_empty() {
        return Some(PIPELINE_DEFINITION_NEEDS_A_STEP.to_string());
    }
    // The cheap pre-check, before any graph work: the step list alone is already past
    // the bound, so nothing below needs to look at it.
    if steps.len() > PIPELINE_MAX_STEPS {
        return Some(too_many_steps(steps.len()));
    }

    let mut declared = DeclaredSteps::default();
    if let Some(violation) = collect_pipeline_steps(steps, 1, None, &mut declared) {
        return Some(violation);
    }
    // Resolution and the cycle walk both reach a dependency through `by_name`, so each
    // is O((steps + edges) log steps) over counts the two bounds above cap. Every way one
    // EDGE can be unsatisfiable is diagnosed before the graph as a whole is walked, so the
    // refusal states the defect the caller can act on rather than the condition it shows
    // up as: a step naming itself, a target no step declares, or a target of a kind
    // nothing can wait on.
    for step in &declared.steps {
        for dependency in &step.depends_on {
            if *dependency == step.name {
                return Some(step_cannot_depend_on_itself(step.name));
            }
            let Some(&target) = declared.by_name.get(dependency) else {
                return Some(step_depends_on_unknown_step(step.name, dependency));
            };
            // `by_name` only ever holds the index of a step already collected, so the
            // lookup resolves; going on rather than indexing keeps the walk total.
            let Some(target_step) = declared.steps.get(target) else {
                continue;
            };
            if PIPELINE_UNDEPENDABLE_STEP_TYPES.contains(&target_step.step_type) {
                return Some(step_cannot_depend_on_step_kind(
                    step.name,
                    target_step.step_type,
                    dependency,
                ));
            }
        }
    }
    if pipeline_graph_has_cycle(&declared) {
        return Some(PIPELINE_DAG_CYCLE.to_string());
    }
    // Resolved last: a definition whose step graph does not resolve is refused for THAT,
    // and only a definition that does have a graph has arguments worth resolving the
    // references of.
    let parameters: BTreeSet<&str> = declared_parameter_names(&parsed).into_iter().collect();
    unresolved_parameter_reference(&parsed, &parameters)
}

/// How SageMaker diagnoses a step that names ITSELF among the steps it depends on — an
/// edge that can never be satisfied, and the one cycle a caller can read off the step
/// alone, so it is stated as the step's own defect rather than as a property of the graph.
fn step_cannot_depend_on_itself(step: &str) -> String {
    format!("Step: {step} can not depend on itself.")
}

/// How SageMaker diagnoses a `DependsOn` entry naming a step the definition does not
/// declare: both the step that waits and the name nothing declares, as the request sent
/// them, so the caller can tell a misspelled dependency from a missing declaration.
fn step_depends_on_unknown_step(step: &str, missing: &str) -> String {
    format!("Step: {step} depends on unknown Step: {missing}")
}

/// How SageMaker diagnoses a `DependsOn` entry whose target is one of the step kinds
/// nothing may wait on ([`PIPELINE_UNDEPENDABLE_STEP_TYPES`]): the waiting step, the
/// target's KIND (the step's own `Type` with `Step` after it, as the service names the
/// kind) and the target.
fn step_cannot_depend_on_step_kind(step: &str, target_type: &str, target: &str) -> String {
    format!("Step: {step} can not depends on {target_type}Step: {target}")
}

/// How SageMaker diagnoses a definition whose `Steps` list holds nothing.
///
/// FIXED text carrying nothing from the request: the sentence names the part of the
/// request being judged (`Pipeline definition`) and the requirement it breaks — the
/// minimum number of steps a pipeline must declare — with no closing period, because the
/// requirement is quoted rather than written out as prose.
const PIPELINE_DEFINITION_NEEDS_A_STEP: &str =
    "Pipeline definition: At least 1 step must be provided";

/// How SageMaker diagnoses a step graph that is not a DAG.
///
/// FIXED text carrying nothing from the request: the service reports that the resolution
/// found a cycle, and neither the steps on it nor the path through them — a caller acts on
/// the defect class, and the enumeration of a path is one of many the same graph admits.
const PIPELINE_DAG_CYCLE: &str = "Detected cycle in the DAG.";

/// How SageMaker diagnoses a property reference to a pipeline parameter the definition
/// does not declare: the reference AS WRITTEN, in the brackets the service quotes a
/// reference in.
fn unknown_parameter_property_reference(parameter: &str) -> String {
    format!("Unknown property reference [{PIPELINE_PARAMETER_REFERENCE_PREFIX}{parameter}].")
}

/// The first property reference in `document` that addresses a pipeline parameter not
/// among `declared`, as the refusal's message, or `None` when every one of them resolves.
///
/// A definition's steps reach the pipeline's own parameters through the schema's
/// `{"Get": "Parameters.<name>"}` references, which may sit anywhere inside a step's
/// arguments, so the whole document is walked — in document order, and iteratively, so a
/// deeply nested definition cannot drive the walk into unbounded recursion. The work is
/// linear in the document already parsed above, whose size the member's modelled length
/// bounds. A reference with any other prefix addresses something else (a step's
/// properties, the execution) and is not a parameter reference.
fn unresolved_parameter_reference(
    document: &::serde_json::Value,
    declared: &BTreeSet<&str>,
) -> Option<String> {
    let mut pending = vec![document];
    while let Some(value) = pending.pop() {
        match value {
            ::serde_json::Value::Object(members) => {
                if let Some(parameter) = members
                    .get(PIPELINE_PROPERTY_REFERENCE_KEY)
                    .and_then(::serde_json::Value::as_str)
                    .and_then(|reference| {
                        reference.strip_prefix(PIPELINE_PARAMETER_REFERENCE_PREFIX)
                    })
                {
                    if !declared.contains(parameter) {
                        return Some(unknown_parameter_property_reference(parameter));
                    }
                }
                // Reversed, so the walk pops the members in the order the document wrote
                // them and the refusal names the FIRST unresolved reference.
                pending.extend(members.values().rev());
            }
            ::serde_json::Value::Array(entries) => pending.extend(entries.iter().rev()),
            _ => {}
        }
    }
    None
}

/// The refusal for a definition declaring more steps than [`PIPELINE_MAX_STEPS`].
fn too_many_steps(declared: usize) -> String {
    format!(
        "PipelineDefinition declares {declared} steps, more than the {PIPELINE_MAX_STEPS} \
         steps one pipeline definition may hold"
    )
}

/// The key a pipeline definition's step names the Amazon SQS queue a callback step hands
/// its work over through under, as the SageMaker Python SDK's `CallbackStep` serialises its
/// `sqs_queue_url`.
const PIPELINE_STEP_QUEUE_KEY: &str = "SqsQueueUrl";

/// The key a condition step lists the conditions it evaluates under, as the SageMaker
/// Python SDK's `ConditionStep` serialises its `conditions`.
const PIPELINE_STEP_CONDITIONS_KEY: &str = "Conditions";

/// The key a fail step carries the message it ends the run with under, as the SageMaker
/// Python SDK's `FailStep` serialises its `error_message`.
const PIPELINE_STEP_ERROR_MESSAGE_KEY: &str = "ErrorMessage";

/// The two branch keys a condition step hands control to one of, in the schema's own
/// order: the steps run when every condition holds, then the steps run when one does not.
const PIPELINE_CONDITION_BRANCH_KEYS: [&str; 2] = ["IfSteps", "ElseSteps"];

/// `<pipeline arn>/execution/<id>` — the one `PipelineExecutionArn` form this service
/// mints, and the form the modelled pattern
/// `^arn:aws[a-z\-]*:sagemaker:[a-z0-9\-]*:[0-9]{12}:pipeline\/.*\/execution\/.*$`
/// describes: an execution's ARN extends the ARN of the pipeline it runs.
pub fn pipeline_execution_arn(pipeline_arn: &str, execution_id: &str) -> String {
    format!("{pipeline_arn}/execution/{execution_id}")
}

/// Every step name a pipeline definition declares, in declaration order, counting the
/// steps nested in a condition step's branches — the one namespace a step name lives in.
///
/// Reads the SAME walk `pipeline_definition_violation` validates with, so what a stored
/// definition declares is never computed twice from two readings of the schema. A
/// definition the walk cannot resolve declares nothing addressable, so the answer is
/// then whatever it resolved before the violation; a definition that reached the store
/// was accepted by that validator, so this walk reports none.
pub fn pipeline_definition_step_names(definition: &str) -> Vec<String> {
    pipeline_definition_step_declarations(definition)
        .into_iter()
        .map(|step| step.name)
        .collect()
}

/// One step a pipeline definition declares, in the members a step EXECUTION of it
/// reports and the members the run needs to PERFORM it: the name it is addressed by, the
/// display name and description the definition optionally gives it, the kind of work it
/// declares, and — for a callback step — the queue it hands that work over through and the
/// arguments it hands over with it.
///
/// `PipelineExecutionStep` carries the first three as `StepName`,
/// `StepDisplayName` and `StepDescription`
/// (`smithy_query(query="shape",shape="PipelineExecutionStep")`), and a definition's step
/// object carries them under `Name`, `DisplayName` and `Description` — the keys the
/// SageMaker Python SDK's `Step` serialises its own `name`, `display_name` and
/// `description` to. A step that declares neither optional key reports neither member.
///
/// `sqs_queue_url`, `conditions`, `error_message` and `arguments` are what
/// [`crate::state::dispatch_pipeline_steps`] performs a step from; that SDK's `CallbackStep`
/// serialises its queue beside the step's `Arguments`, as `SqsQueueUrl`, and a definition
/// written with the queue INSIDE the arguments names the same queue, so both placements are
/// read — and the same for a condition step's `Conditions` and a fail step's `ErrorMessage`.
///
/// `branch` is the condition step's branch this step is declared in, for a step nested in
/// one: a branch's steps run only when the condition step hands control to that branch, so
/// the run needs to know which branch a step belongs to before it performs anything.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStepDeclaration {
    pub name: String,
    pub display_name: Option<String>,
    pub description: Option<String>,
    pub step_type: String,
    pub sqs_queue_url: Option<String>,
    pub conditions: Option<::serde_json::Value>,
    pub error_message: Option<String>,
    pub arguments: Option<::serde_json::Value>,
    pub branch: Option<PipelineStepBranch>,
}

/// The branch of a condition step one nested step is declared in: the step that decides,
/// and whether this is the branch taken when every one of its conditions holds.
#[derive(Debug, Clone, PartialEq)]
pub struct PipelineStepBranch {
    pub condition_step: String,
    pub when_conditions_hold: bool,
}

/// Every step a pipeline definition declares, in declaration order, as the control plane
/// knows it — the same walk (and so the same order and the same one step-name namespace)
/// [`pipeline_definition_step_names`] reports the names of.
pub fn pipeline_definition_step_declarations(definition: &str) -> Vec<PipelineStepDeclaration> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return Vec::new();
    };
    let Some(steps) = parsed.get("Steps").and_then(::serde_json::Value::as_array) else {
        return Vec::new();
    };
    let mut declared = DeclaredSteps::default();
    let _ = collect_pipeline_steps(steps, 1, None, &mut declared);
    declared
        .steps
        .iter()
        .map(|step| PipelineStepDeclaration {
            name: step.name.to_string(),
            display_name: step.display_name.map(str::to_string),
            description: step.description.map(str::to_string),
            step_type: step.step_type.to_string(),
            sqs_queue_url: step.sqs_queue_url.map(str::to_string),
            conditions: step.conditions.cloned(),
            error_message: step.error_message.map(str::to_string),
            arguments: step.arguments.cloned(),
            branch: step.branch.map(|(condition_step, when_conditions_hold)| {
                PipelineStepBranch {
                    condition_step: condition_step.to_string(),
                    when_conditions_hold,
                }
            }),
        })
        .collect()
}

/// The default value each parameter a pipeline definition declares carries, keyed by the
/// parameter's name — the value a run that assigns none uses.
///
/// A definition's `Parameters` entries carry `Name`, `Type` and an optional `DefaultValue`
/// (the keys the SageMaker Python SDK's `ParameterString`/`ParameterInteger` serialise to),
/// so a parameter declared without one has no default and a run that assigns it nothing
/// leaves it unresolved. The values are the parsed JSON the definition wrote, so a numeric
/// default stays a number.
pub fn pipeline_definition_parameter_defaults(
    definition: &str,
) -> BTreeMap<String, ::serde_json::Value> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return BTreeMap::new();
    };
    parsed
        .get("Parameters")
        .and_then(::serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| {
                    let name = entry.get("Name").and_then(::serde_json::Value::as_str)?;
                    let default = entry.get("DefaultValue")?;
                    Some((name.to_string(), default.clone()))
                })
                .collect()
        })
        .unwrap_or_default()
}

/// Every parameter name a pipeline definition declares, in declaration order.
///
/// A definition's optional top-level `Parameters` list is what names the parameters an
/// execution may assign values to — `Parameter$Name`'s model doc: "This parameter name
/// must match a named parameter in the pipeline definition." A definition declaring no
/// `Parameters` names none, so every value a start call supplies for it is unmatched.
pub fn pipeline_definition_parameter_names(definition: &str) -> Vec<String> {
    let Ok(parsed) = ::serde_json::from_str::<::serde_json::Value>(definition) else {
        return Vec::new();
    };
    declared_parameter_names(&parsed)
        .into_iter()
        .map(str::to_string)
        .collect()
}

/// The parameter names a parsed definition's top-level `Parameters` list declares, in
/// declaration order — borrowed from the document, so the validator resolves a reference
/// against them without copying the names it already parsed.
///
/// The one reading of that list: [`pipeline_definition_parameter_names`] reports it to the
/// start call that assigns values, and [`pipeline_definition_violation`] resolves the
/// definition's own property references against it, so the two cannot disagree about what
/// a definition declares.
fn declared_parameter_names(document: &::serde_json::Value) -> Vec<&str> {
    document
        .get("Parameters")
        .and_then(::serde_json::Value::as_array)
        .map(|entries| {
            entries
                .iter()
                .filter_map(|entry| entry.get("Name").and_then(::serde_json::Value::as_str))
                .collect()
        })
        .unwrap_or_default()
}

/// One step of a pipeline definition, reduced to what the dependency graph is built
/// from and what a step execution of it reports: the name and kind it declares, the step
/// names it depends on, and the display name and description it optionally carries — all
/// borrowed from the parsed document so a large definition is not copied to walk it.
struct PipelineStep<'a> {
    name: &'a str,
    /// The step's declared `Type`, which decides whether another step may depend on it
    /// ([`PIPELINE_UNDEPENDABLE_STEP_TYPES`]), whether it holds branches of its own, and
    /// whether the run can perform it ([`crate::state::dispatch_pipeline_steps`]).
    step_type: &'a str,
    depends_on: Vec<&'a str>,
    display_name: Option<&'a str>,
    description: Option<&'a str>,
    /// The Amazon SQS queue a callback step hands its work over through, from the step's
    /// own `SqsQueueUrl` or from the same key inside its `Arguments`.
    sqs_queue_url: Option<&'a str>,
    /// The conditions a condition step evaluates, read from both placements the same way
    /// the queue above is.
    conditions: Option<&'a ::serde_json::Value>,
    /// The message a fail step ends the run with, read from both placements the same way.
    error_message: Option<&'a str>,
    /// The step's own `Arguments`, which a performed step hands to the worker.
    arguments: Option<&'a ::serde_json::Value>,
    /// The condition step's branch this step is declared in, and whether that is the
    /// branch taken when the conditions hold; `None` for a step declared beside the
    /// pipeline's own steps rather than inside a branch.
    branch: Option<(&'a str, bool)>,
}

/// Every step one definition declares, in declaration order, plus the index that makes
/// a name lookup logarithmic instead of a scan of everything declared so far — the
/// uniqueness check, dependency resolution and the cycle walk all go through it, and
/// `edges` is the running total of `DependsOn` entries, which is bounded as it grows.
#[derive(Default)]
struct DeclaredSteps<'a> {
    steps: Vec<PipelineStep<'a>>,
    by_name: BTreeMap<&'a str, usize>,
    edges: usize,
}

/// Walk `steps` (and the branches of any condition step among them) into `out`,
/// answering the first violation of the per-step rules, if any.
///
/// A condition step's `IfSteps`/`ElseSteps` are steps of the same pipeline: they share
/// the one step-name namespace and may depend on steps outside their branch, so the
/// graph is built over all of them. SageMaker nests them under the step's `Arguments`;
/// both placements are walked, so a definition written either way resolves the same.
///
/// The walk stops as soon as the definition is past [`PIPELINE_MAX_STEPS`] or
/// [`PIPELINE_MAX_STEP_DEPENDENCIES`], so the work it does is bounded by those two
/// counts rather than by the megabyte the member's modelled length admits.
fn collect_pipeline_steps<'a>(
    steps: &'a [::serde_json::Value],
    depth: usize,
    branch: Option<(&'a str, bool)>,
    out: &mut DeclaredSteps<'a>,
) -> Option<String> {
    if depth > PIPELINE_MAX_STEP_DEPTH {
        return Some(format!(
            "PipelineDefinition nests steps more than {PIPELINE_MAX_STEP_DEPTH} levels deep"
        ));
    }
    for step in steps {
        let Some(member) = step.as_object() else {
            return Some("every entry of Steps must be a JSON object".to_string());
        };
        let Some(name) = member
            .get("Name")
            .and_then(::serde_json::Value::as_str)
            .filter(|name| !name.is_empty())
        else {
            return Some("a step of PipelineDefinition is missing the required key Name".to_string());
        };
        let Some(step_type) = member.get("Type").and_then(::serde_json::Value::as_str) else {
            return Some(format!(
                "step {name} of PipelineDefinition is missing the required key Type"
            ));
        };
        if out.steps.len() == PIPELINE_MAX_STEPS {
            return Some(too_many_steps(out.steps.len() + 1));
        }
        if out.by_name.contains_key(name) {
            return Some(format!(
                "step name {name} is declared more than once; a pipeline definition's step \
                 names must be unique"
            ));
        }
        let mut depends_on = Vec::new();
        match member.get("DependsOn") {
            None | Some(::serde_json::Value::Null) => {}
            Some(::serde_json::Value::Array(entries)) => {
                if out.edges + entries.len() > PIPELINE_MAX_STEP_DEPENDENCIES {
                    return Some(format!(
                        "PipelineDefinition declares more than the \
                         {PIPELINE_MAX_STEP_DEPENDENCIES} DependsOn entries one pipeline \
                         definition may hold"
                    ));
                }
                out.edges += entries.len();
                for entry in entries {
                    match entry.as_str() {
                        Some(dependency) => depends_on.push(dependency),
                        None => {
                            return Some(format!(
                                "step {name} declares a DependsOn entry that is not a step name"
                            ));
                        }
                    }
                }
            }
            Some(_) => {
                return Some(format!(
                    "step {name} declares DependsOn as something other than a list of step names"
                ));
            }
        }
        out.by_name.insert(name, out.steps.len());
        let arguments = member.get("Arguments");
        out.steps.push(PipelineStep {
            name,
            step_type,
            depends_on,
            display_name: member
                .get("DisplayName")
                .and_then(::serde_json::Value::as_str),
            description: member
                .get("Description")
                .and_then(::serde_json::Value::as_str),
            sqs_queue_url: member
                .get(PIPELINE_STEP_QUEUE_KEY)
                .or_else(|| arguments.and_then(|args| args.get(PIPELINE_STEP_QUEUE_KEY)))
                .and_then(::serde_json::Value::as_str),
            conditions: member
                .get(PIPELINE_STEP_CONDITIONS_KEY)
                .or_else(|| arguments.and_then(|args| args.get(PIPELINE_STEP_CONDITIONS_KEY))),
            error_message: member
                .get(PIPELINE_STEP_ERROR_MESSAGE_KEY)
                .or_else(|| arguments.and_then(|args| args.get(PIPELINE_STEP_ERROR_MESSAGE_KEY)))
                .and_then(::serde_json::Value::as_str),
            arguments,
            branch,
        });

        // Only a condition step holds steps of its own, and only under the two branch
        // keys the schema gives it: any other step's `Arguments` are its own payload,
        // so a list that merely happens to be named `Steps` there is NOT walked as
        // pipeline steps.
        if step_type == PIPELINE_CONDITION_STEP_TYPE {
            for branch_key in PIPELINE_CONDITION_BRANCH_KEYS {
                let placements = [
                    member.get(branch_key),
                    member.get("Arguments").and_then(|args| args.get(branch_key)),
                ];
                // The first branch key is the one taken when every condition holds.
                let nested_branch =
                    Some((name, branch_key == PIPELINE_CONDITION_BRANCH_KEYS[0]));
                for nested in placements
                    .into_iter()
                    .flatten()
                    .filter_map(::serde_json::Value::as_array)
                {
                    if let Some(violation) =
                        collect_pipeline_steps(nested, depth + 1, nested_branch, out)
                    {
                        return Some(violation);
                    }
                }
            }
        }
    }
    None
}

/// Whether the steps' `DependsOn` edges form a cycle, i.e. whether the definition's
/// dependency graph is not a DAG.
///
/// A depth-first walk that carries its own stack rather than recursing, so a definition
/// with a long dependency chain cannot exhaust the call stack, and that resolves each
/// edge through the name index rather than scanning the steps. Answers only WHETHER the
/// graph is cyclic, because that is what the refusal states ([`PIPELINE_DAG_CYCLE`]
/// carries no path). A dependency no step declares is skipped here: it is refused before
/// this runs.
fn pipeline_graph_has_cycle(declared: &DeclaredSteps<'_>) -> bool {
    /// Not yet visited.
    const UNSEEN: u8 = 0;
    /// On the current walk's path.
    const ON_PATH: u8 = 1;
    /// Visited, and every path out of it was walked.
    const SETTLED: u8 = 2;

    let steps = &declared.steps;
    let mut colour = vec![UNSEEN; steps.len()];
    for start in 0..steps.len() {
        if colour[start] != UNSEEN {
            continue;
        }
        colour[start] = ON_PATH;
        let mut path: Vec<(usize, usize)> = vec![(start, 0)];
        while let Some(&(node, cursor)) = path.last() {
            let Some(dependency) = steps[node].depends_on.get(cursor) else {
                colour[node] = SETTLED;
                path.pop();
                continue;
            };
            if let Some(frame) = path.last_mut() {
                frame.1 += 1;
            }
            let Some(&next) = declared.by_name.get(dependency) else {
                continue;
            };
            match colour[next] {
                ON_PATH => return true,
                UNSEEN => {
                    colour[next] = ON_PATH;
                    path.push((next, 0));
                }
                _ => {}
            }
        }
    }
    false
}
