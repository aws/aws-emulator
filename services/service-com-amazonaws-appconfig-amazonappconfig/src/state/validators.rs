// SPDX-License-Identifier: CC0-1.0

//! A configuration profile's validators: the check the configuration data has to pass
//! before a call that puts it in service may proceed.
//!
//! "A validator provides a syntactic or semantic check to ensure the configuration that
//! you want to deploy functions as intended. To validate your application configuration
//! data, you provide a schema or an Amazon Web Services Lambda function that runs against
//! the configuration. The configuration deployment or update can only proceed when the
//! configuration data is valid" (`smithy_query(query="doc",shape="Validator")`). So a
//! profile's `Validators` are not metadata that merely round-trips: an operation AppConfig
//! documents as validating a configuration runs EVERY validator the profile carries against
//! the candidate version's content before it does anything, and a validator that reports
//! the content non-compliant refuses that call with `BadRequestException` (HTTP 400),
//! leaving nothing behind.
//!
//! A validator's `Content` is "either the JSON Schema content or the Amazon Resource Name
//! (ARN) of an Lambda function"
//! (`smithy_query(query="doc",shape="Validator",member="Content")`), one per
//! `ValidatorType`:
//!
//! * `LAMBDA` — the `Content` names a function, invoked synchronously with the candidate
//!   configuration; a function that answers an error has found the content non-compliant,
//!   which is the verdict that refuses the call. The invocation goes through the one Lambda
//!   seam this crate has ([`super::extension_actions::invoke_function`]), over the
//!   in-process callback, so the function invoked is the one the emulator's own Lambda
//!   service holds and no socket leaves the machine. It is bounded by
//!   [`LAMBDA_CHECK_BUDGET`], which is what a gated call waits on a function that never
//!   answers, since the client the invocation is made with disables its own timeouts.
//! * `JSON_SCHEMA` — the `Content` is a JSON Schema document, and the verdict is whether
//!   the candidate configuration satisfies it, decided in process by the `boon` evaluator
//!   ([`json_schema_rejection`]). The evaluation resolves no schema resource beyond the
//!   validator's own document ([`NoSchemaResources`]), and it is BOUNDED on every axis a
//!   caller controls, because both documents are caller-stored:
//!
//!   * size — the schema is `Validator$Content` (`length 0..32768`) and the candidate is a
//!     stored hosted version, capped where it is stored;
//!   * depth and breadth — a schema deeper than [`MAX_SCHEMA_DEPTH`] or holding more JSON
//!     nodes than [`MAX_SCHEMA_NODES`] is not evaluated, which is what rules out the
//!     nested-combinator schema whose cost grows with its own depth rather than its size;
//!   * references — a schema carrying more reference keywords than [`MAX_SCHEMA_REFS`] is
//!     not evaluated either: a reference makes the schema a graph rather than a tree, so one
//!     subschema can be walked once per path that reaches it. Every form counts
//!     ([`REFERENCE_KEYWORDS`]), because a schema that declares a dialect is read under it;
//!   * steps — a schema node is visited at most once per document node per such path, so
//!     [`MAX_EVALUATION_PAIRS`] caps the PRODUCT of the two node counts and the paths the
//!     schema's refs can unfold into, rather than each factor alone;
//!   * wall clock — the compile and the apply run on a blocking thread under
//!     [`SCHEMA_CHECK_BUDGET`], so they occupy no async worker and no request waits on them
//!     for longer than that. The budget bounds the WAIT; what bounds the WORK is the four
//!     bounds above, because a blocking task cannot be cancelled and so runs to completion
//!     after the request it was serving has been answered.
//!
//!   A check that cannot be completed inside those bounds has NOT shown the configuration
//!   valid, so it refuses the call it gates with this service's own validator-rejection
//!   message — the same way a `LAMBDA` invocation that does not answer inside its budget
//!   does — rather than letting an unchecked configuration through.
//!
//! A validator that yields NO verdict — a `LAMBDA` validator whose invocation does not
//! happen at all as opposed to happening and answering an error, or a `JSON_SCHEMA`
//! validator whose `Content` is no JSON Schema this evaluator can compile — does not refuse
//! the call it gates, for the reason
//! [`super::extension_actions::Failure::refuses_the_call`] gives: nothing is known about
//! what it would have reported, and a refusal reported for it would be a verdict no
//! validator gave.

use std::time::Duration;

use ::serde_json::{Map, Value};

use crate::ops::types::Validator;

use super::extension_actions::{encoded, invoke_function, Failure, LAMBDA_ARN_NAMESPACE};

/// The `ValidatorType` values the model lists (`VALIDATOR_TYPE_VALUES` in
/// `crate::ops::types`), each spelled once here because the evaluation keys off it.
pub const JSON_SCHEMA: &str = "JSON_SCHEMA";
/// See [`JSON_SCHEMA`].
pub const LAMBDA: &str = "LAMBDA";

/// How long one `LAMBDA` check may wait for the function's answer.
///
/// The client the invocation is made with disables its own timeouts — there is no network
/// behind it to time out — so this is what bounds the gated call's wait on a function that
/// never answers. EMULATOR-chosen: no published AppConfig limit covers it.
const LAMBDA_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// How long one `JSON_SCHEMA` check may spend compiling the schema and applying it to the
/// candidate configuration.
///
/// The evaluator has no step budget of its own, so this is the wall clock that bounds the
/// gated call's WAIT on it; the work itself is bounded by the shape bounds above, since a
/// blocking task cannot be cancelled. The work runs on a BLOCKING thread, so it occupies no
/// async worker. EMULATOR-chosen, the same span the `LAMBDA` check waits: no published
/// AppConfig limit covers it.
const SCHEMA_CHECK_BUDGET: Duration = Duration::from_secs(5);

/// The deepest a validator's schema may nest for this emulator to evaluate it.
///
/// A schema's cost against one document node grows with the schema's own NESTING — a
/// combinator (`anyOf`, `allOf`, `oneOf`) nested inside another is applied inside it — so a
/// small document can describe a very deep evaluation. This bounds that nesting, far deeper
/// than a configuration schema an operator writes. EMULATOR-chosen: no published AppConfig
/// limit covers it.
const MAX_SCHEMA_DEPTH: usize = 24;

/// The most JSON nodes a validator's schema may hold for this emulator to evaluate it.
///
/// `Validator$Content` is `length 0..32768`, which bounds the schema's nodes only loosely;
/// this bounds them directly, so the per-document-node cost of the evaluation is known
/// before it starts. EMULATOR-chosen: no published AppConfig limit covers it.
const MAX_SCHEMA_NODES: usize = 4096;

/// The most reference keywords a validator's schema may carry for this emulator to evaluate
/// it.
///
/// A reference is what makes a schema a graph rather than a tree: a referenced subschema is
/// evaluated once per PATH that reaches it, so a chain of subschemas each naming the next
/// twice unfolds into `2^refs` walks of the schema while the document itself stays small and
/// shallow. Bounding the references is what keeps that unfolding finite before anything
/// runs, and its factor is carried into [`MAX_EVALUATION_PAIRS`]. A configuration schema an
/// operator writes reuses a handful of definitions, far inside this. EMULATOR-chosen: no
/// published AppConfig limit covers it.
const MAX_SCHEMA_REFS: usize = 8;

/// The most `(schema node, document node)` pairs one check may have to consider.
///
/// A schema that is a tree visits each of its nodes at most once per document node it
/// applies to, so the PRODUCT of the two node counts bounds the keyword evaluations the
/// check performs — which is the step budget the evaluator itself does not offer. Bounding
/// each count alone would not: a small schema against a large document, or the reverse,
/// multiplies out. A schema carrying REFERENCES is not a tree, so its node count enters that
/// product multiplied by the `2^refs` paths those references can unfold into
/// ([`MAX_SCHEMA_REFS`]), which is what makes this a bound rather than an estimate.
/// EMULATOR-chosen: no published AppConfig limit covers it.
const MAX_EVALUATION_PAIRS: usize = 50_000_000;

/// The longest validator report a refusal carries, in characters.
///
/// What a validator reports grows with the configuration it was run against rather than
/// with what a client needs to read; the `Message` member carries the head of it.
const MAX_DETAIL_CHARS: usize = 1024;

/// The configuration version a profile's validators are run against: what each validator
/// is handed, and what the refusal of a non-compliant one names.
#[derive(Debug, Clone, Copy)]
pub struct Candidate<'a> {
    /// The application the configuration profile belongs to.
    pub application_id: &'a str,
    /// The configuration profile whose validators are being run.
    pub configuration_profile_id: &'a str,
    /// The version under validation, as the caller named it — a version number or a
    /// version label, the two spellings `ConfigurationVersion` documents.
    pub configuration_version: &'a str,
    /// The profile's `LocationUri`: "information about the source location of the
    /// configuration", which the event a `LAMBDA` validator is handed carries.
    pub location_uri: &'a str,
    /// The configuration bytes a consumer of this profile would be served for that
    /// version, read through the same projection the data plane serves
    /// ([`super::ConfigurationProfile::served_content`]) — a validator checks what would
    /// go into service, not a different document.
    pub content: &'a [u8],
}

/// The first non-compliant verdict `validators` give for `candidate`, as the `Message` of
/// the `BadRequestException` the gated call is refused with; `None` when no validator
/// reported the content non-compliant.
///
/// The validators are run in the order the profile carries them and the first verdict is
/// answered, so a call is refused for the first check it failed rather than for the last.
pub async fn first_rejection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    validators: &[Validator],
    candidate: &Candidate<'_>,
    request_region: &str,
) -> Option<String> {
    for validator in validators {
        let rejection = match validator.r#type.as_str() {
            LAMBDA => lambda_rejection(ctx, &validator.content, candidate, request_region).await,
            JSON_SCHEMA => json_schema_rejection(&validator.content, candidate).await,
            // A `Type` outside the model's `ValidatorType` enum is refused by the boundary
            // validator before any handler stores it, so there is no check to run for one.
            _ => None,
        };
        if rejection.is_some() {
            return rejection;
        }
    }
    None
}

/// The verdict a `LAMBDA` validator gives: whether the function the validator's `Content`
/// names, invoked with the candidate configuration, reports an error.
async fn lambda_rejection(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    function: &str,
    candidate: &Candidate<'_>,
    request_region: &str,
) -> Option<String> {
    // The function is reached in the Region its own ARN names, falling back to the Region
    // this request was signed for — the same resolution an extension action's target gets.
    let region = ::service_common::arn::region_of(function, LAMBDA_ARN_NAMESPACE)
        .unwrap_or(request_region);
    let event = validator_event(candidate).to_string();
    match ::tokio::time::timeout(
        LAMBDA_CHECK_BUDGET,
        invoke_function(ctx, function, region, event),
    )
    .await
    {
        // The function ran and reported no error: it found the content compliant. What it
        // RETURNED is not read — a validator reports a verdict, it does not rewrite the
        // configuration.
        Ok(Ok(_)) => None,
        Ok(Err(failure)) if failure.refuses_the_call() => {
            Some(rejection_message(candidate, LAMBDA, &lambda_detail(function, &failure)))
        }
        // The invocation did not happen at all; see this module's own documentation.
        Ok(Err(_)) => None,
        // The invocation happened and did not answer in time, which is an invocation that
        // failed rather than one that was never made: the configuration was not shown
        // valid, so the call it gates is refused.
        Err(_) => Some(rejection_message(
            candidate,
            LAMBDA,
            &format!("the function {function} did not answer the invocation in time"),
        )),
    }
}

/// The event a `LAMBDA` validator's function is handed: the resources the configuration
/// belongs to, the version under validation, its content base64-encoded and the location
/// the configuration is held at, keyed as the AppConfig User Guide's validator page
/// documents them — the page the model's `Validator` shape links to.
fn validator_event(candidate: &Candidate<'_>) -> Value {
    Value::Object(Map::from_iter([
        (
            "applicationId".to_string(),
            Value::from(candidate.application_id),
        ),
        (
            "configurationProfileId".to_string(),
            Value::from(candidate.configuration_profile_id),
        ),
        (
            "configurationVersion".to_string(),
            Value::from(candidate.configuration_version),
        ),
        ("content".to_string(), Value::from(encoded(candidate.content))),
        ("uri".to_string(), Value::from(candidate.location_uri)),
    ]))
}

/// What a failed validator invocation contributes to the refusal: the function that was
/// invoked, the error it answered and, when its answer carried one, the Lambda request id
/// of the invocation — the same three facts this service reports for a failed extension
/// action (`super::extension_actions::ActionTarget::refusal_message`).
fn lambda_detail(function: &str, failure: &Failure) -> String {
    let mut detail = format!("error invoking {function}: {}", summarised(&failure.detail));
    if let Some(request_id) = &failure.request_id {
        detail.push_str(&super::extension_actions::lambda_request_id_clause(request_id));
    }
    detail
}

/// How AppConfig refuses a call whose candidate configuration a validator reported
/// non-compliant: derived from the request — the version and profile the validator ran
/// against, the type of validator that gave the verdict, and the detail it gave.
fn rejection_message(candidate: &Candidate<'_>, validator_type: &str, detail: &str) -> String {
    format!(
        "Configuration version '{}' of configuration profile '{}' did not pass the \
         {validator_type} validator: {detail}",
        candidate.configuration_version, candidate.configuration_profile_id
    )
}

/// The verdict a `JSON_SCHEMA` validator gives: whether the candidate configuration
/// satisfies the JSON Schema document the validator's `Content` holds.
///
/// `None` — no verdict — when the `Content` is not a JSON Schema this evaluator can compile
/// at all: nothing is then known about the candidate configuration, and a refusal reported
/// for it would be a verdict no validator gave. A candidate that is not a JSON document is
/// handed to the schema as the JSON string its text is, so the verdict on it — and the
/// diagnostic a refusal of it carries — is the schema's own.
///
/// Every bound this check runs under is applied here, in the order that puts the cheapest
/// first: the schema's own shape ([`MAX_SCHEMA_DEPTH`], [`MAX_SCHEMA_NODES`]), then the
/// steps the two documents together imply ([`MAX_EVALUATION_PAIRS`]), then the wall clock
/// ([`SCHEMA_CHECK_BUDGET`]) over the evaluation itself, which runs on a blocking thread so
/// no async worker is occupied by it. A check that cannot be completed inside them refuses
/// the call it gates: the configuration was not shown valid.
async fn json_schema_rejection(
    schema_document: &str,
    candidate: &Candidate<'_>,
) -> Option<String> {
    let schema: Value = ::serde_json::from_str(schema_document).ok()?;
    let shape = measured(&schema);
    if shape.depth > MAX_SCHEMA_DEPTH
        || shape.nodes > MAX_SCHEMA_NODES
        || shape.refs > MAX_SCHEMA_REFS
    {
        tracing::warn!(
            schema_depth = shape.depth,
            schema_nodes = shape.nodes,
            schema_refs = shape.refs,
            "a configuration profile's JSON_SCHEMA validator carries a schema beyond the \
             shape this emulator evaluates"
        );
        return Some(rejection_message(candidate, JSON_SCHEMA, UNEVALUABLE_SCHEMA));
    }

    let instance = match ::serde_json::from_slice::<Value>(candidate.content) {
        Ok(document) => document,
        // Content that is not a JSON document is still HANDED to the schema, as the text it
        // is: the diagnostic a refusal carries is the schema library's own, so what a
        // caller is told is which instance type was seen and which the schema allows,
        // rather than a sentence this emulator wrote about the document being unreadable.
        // A JSON string is the one value such content is, so that is the instance the
        // schema is applied to — and bytes that are not text at all have no such value, so
        // they keep the finding below.
        Err(_) => match ::std::str::from_utf8(candidate.content) {
            Ok(text) => Value::String(text.to_string()),
            Err(_) => {
                return Some(schema_violation_message(
                    schema_document,
                    NOT_A_JSON_DOCUMENT,
                    ROOT_NODE,
                ))
            }
        },
    };
    let instance_nodes = measured(&instance).nodes;
    // A reference lets one subschema be reached by several paths, so the schema's node count
    // enters the step bound multiplied by the paths its references can unfold into.
    let unfolded = shape.nodes.saturating_mul(1 << shape.refs);
    if unfolded.saturating_mul(instance_nodes) > MAX_EVALUATION_PAIRS {
        tracing::warn!(
            schema_nodes = shape.nodes,
            schema_refs = shape.refs,
            instance_nodes,
            "a JSON_SCHEMA check would consider more node pairs than this emulator evaluates"
        );
        return Some(rejection_message(candidate, JSON_SCHEMA, BEYOND_STEP_BUDGET));
    }

    let url = validator_schema_url(candidate);
    // The compile and the apply are synchronous and have no yield point, so they run on a
    // blocking thread: the request waits on them for at most the budget, and the async
    // worker that serves it is never occupied by them.
    let evaluated = ::tokio::time::timeout(
        SCHEMA_CHECK_BUDGET,
        ::tokio::task::spawn_blocking(move || evaluate(schema, instance, url)),
    )
    .await;
    match evaluated {
        Ok(Ok(SchemaVerdict::Violation { finding, node })) => {
            Some(schema_violation_message(schema_document, &finding, &node))
        }
        Ok(Ok(SchemaVerdict::Compliant | SchemaVerdict::NoVerdict)) => None,
        // The evaluation did not finish inside the budget, or the thread it ran on did not
        // come back with an answer. Either way the configuration was NOT shown valid, so
        // the call it gates is refused — the same reading the `LAMBDA` arm gives an
        // invocation that was made and did not answer in time.
        Ok(Err(_)) | Err(_) => {
            tracing::warn!(
                "a JSON_SCHEMA check did not answer inside the budget this emulator allows"
            );
            Some(rejection_message(candidate, JSON_SCHEMA, BEYOND_TIME_BUDGET))
        }
    }
}

/// What applying one schema to one document reported.
#[derive(Debug)]
enum SchemaVerdict {
    /// The document satisfies the schema.
    Compliant,
    /// It does not: the finding about the violating node, and the node.
    Violation { finding: String, node: String },
    /// The `Content` is no JSON Schema this evaluator can compile, so nothing is known
    /// about the document.
    NoVerdict,
}

/// Compile `schema` and apply it to `instance`. Synchronous and owned: the caller runs this
/// on a blocking thread under a wall-clock budget, so it borrows nothing from the request.
fn evaluate(schema: Value, instance: Value, url: String) -> SchemaVerdict {
    let mut compiler = ::boon::Compiler::new();
    // The only schema resource is the validator's own document; see [`NoSchemaResources`].
    compiler.use_loader(Box::new(NoSchemaResources));
    compiler.set_default_draft(VALIDATOR_SCHEMA_DRAFT);
    if compiler.add_resource(&url, schema).is_err() {
        return SchemaVerdict::NoVerdict;
    }
    let mut schemas = ::boon::Schemas::new();
    let Ok(compiled) = compiler.compile(&url, &mut schemas) else {
        return SchemaVerdict::NoVerdict;
    };
    match schemas.validate(&instance, compiled) {
        Ok(()) => SchemaVerdict::Compliant,
        Err(failed) => {
            let (finding, node) = violation(&failed);
            SchemaVerdict::Violation { finding, node }
        }
    }
}

/// How deep `document` nests, how many JSON nodes it holds and how many `$ref` keywords it
/// carries — what the bounds on one check are decided from.
///
/// Iterative, and bounded by the document itself: every node is visited once, and the
/// document is one a `Create`/`Update` call had to store before this runs.
fn measured(document: &Value) -> DocumentShape {
    let mut pending: Vec<(&Value, usize)> = vec![(document, 1)];
    let mut shape = DocumentShape::default();
    while let Some((value, level)) = pending.pop() {
        shape.nodes += 1;
        shape.depth = shape.depth.max(level);
        match value {
            Value::Object(members) => {
                shape.refs += members
                    .keys()
                    .filter(|key| REFERENCE_KEYWORDS.contains(&key.as_str()))
                    .count();
                pending.extend(members.values().map(|member| (member, level + 1)));
            }
            Value::Array(items) => {
                pending.extend(items.iter().map(|item| (item, level + 1)));
            }
            _ => {}
        }
    }
    shape
}

/// What [`measured`] reports about one document.
#[derive(Debug, Default)]
struct DocumentShape {
    /// The deepest nesting level in it, the root counting as one.
    depth: usize,
    /// How many JSON nodes it holds.
    nodes: usize,
    /// How many reference keywords it carries — the ones that make a schema a graph.
    refs: usize,
}

/// Every keyword a schema names another subschema with, counted by [`measured`] because
/// each of them is what can make the evaluation walk one subschema more than once.
///
/// All of the forms, not just `$ref`: a schema that DECLARES a dialect is read under that
/// dialect, and `$dynamicRef` (2020-12) and `$recursiveRef` (2019-09) re-apply a subschema
/// to the same instance node exactly as `$ref` does — so a bound that counted only `$ref`
/// would be escaped by a schema that declares a newer dialect.
const REFERENCE_KEYWORDS: [&str; 3] = ["$ref", "$dynamicRef", "$recursiveRef"];

/// The detail a refusal carries when the validator's schema is outside the shape this
/// emulator evaluates. It describes the CONDITION only: the schema itself is a sensitive
/// member, and it is not reflected into this refusal or into the log.
const UNEVALUABLE_SCHEMA: &str = "the schema nests deeper, or holds more elements, than \
                                  this emulator evaluates, so the configuration was not \
                                  shown valid";

/// See [`UNEVALUABLE_SCHEMA`]: the detail for a check whose schema and configuration would
/// together take more evaluation steps than this emulator performs.
const BEYOND_STEP_BUDGET: &str = "applying the schema to this configuration would take \
                                  more steps than this emulator performs, so the \
                                  configuration was not shown valid";

/// See [`UNEVALUABLE_SCHEMA`]: the detail for a check that did not answer inside the budget.
const BEYOND_TIME_BUDGET: &str = "the schema could not be applied to this configuration \
                                  inside the time this emulator allows, so the \
                                  configuration was not shown valid";

/// The schema resources a compiled schema resolves a `$ref` against: NOTHING but the
/// document the compilation was handed.
///
/// A validator's `Content` is a document any caller stores and `$ref` is a dynamic
/// resolution mechanism, so a schema could name a resource by any URL — including a `file:`
/// one. Registering this as the compiler's loader REPLACES the one it comes with, so a
/// reference the compiled document does not satisfy resolves to an error instead of
/// reaching the host's filesystem or network. `pub(crate)` because the feature-flags
/// attribute-constraint check compiles a schema through the same evaluator and resolves
/// just as little ([`super::feature_flags`]).
#[derive(Debug)]
pub(crate) struct NoSchemaResources;

impl ::boon::UrlLoader for NoSchemaResources {
    fn load(&self, _url: &str) -> Result<Value, Box<dyn ::std::error::Error>> {
        // The reference itself is part of the validator's `Content`, which the model marks
        // sensitive, so it is not written to the log: that a reference was not resolved is
        // what a developer needs here, and the document that carried it travels only in the
        // refusal the gated call answers with.
        tracing::warn!(
            "a configuration profile's JSON_SCHEMA validator references a schema resource \
             outside its own document, which this emulator resolves nothing for"
        );
        Err("this emulator resolves no schema resource outside the validator's own \
             document"
            .into())
    }
}

/// The dialect a validator's schema that declares no `$schema` of its own is read under:
/// AppConfig's configuration-validator documentation names JSON Schema version 4.x as what
/// a `JSON_SCHEMA` validator carries. A document that DOES declare a dialect keeps it, so a
/// draft-07 schema is evaluated as draft 07.
const VALIDATOR_SCHEMA_DRAFT: ::boon::Draft = ::boon::Draft::V4;

/// The node a violation of the candidate document as a WHOLE is reported at, written as the
/// root JSON pointer.
const ROOT_NODE: &str = "/";

/// The finding a candidate that is no TEXT at all earns: there is no schema keyword to
/// report, because the bytes the schema would be applied to are not a JSON document and not
/// a JSON string either. Content that IS text is handed to the schema as that string, so
/// the finding reported for it is the schema library's own.
const NOT_A_JSON_DOCUMENT: &str = "the configuration is not a JSON document";

/// The URL the validator's schema document is compiled as a resource under: the profile
/// whose validator it is, which is the only resource the evaluation resolves.
fn validator_schema_url(candidate: &Candidate<'_>) -> String {
    format!(
        "https://appconfig.amazonaws.com/applications/{}/configurationprofiles/{}/validators",
        candidate.application_id, candidate.configuration_profile_id
    )
}

/// The one violation a refusal reports: the finding about the node, and the node itself.
///
/// A schema applied to a document can fail in several places at once and the refusal
/// carries one text, so the report is of the DEEPEST first finding — the keyword that was
/// applied to the most specific node — which is the one that names the member a caller has
/// to change.
fn violation(failed: &::boon::ValidationError) -> (String, String) {
    let leaf = deepest(failed);
    let node = leaf.instance_location.to_string();
    let node = if node.is_empty() {
        ROOT_NODE.to_string()
    } else {
        node
    };
    (finding(&leaf.kind), node)
}

/// The deepest cause of `failed`: a schema that fails through a reference or a subschema
/// reports the keyword that actually failed under the keyword that led to it.
///
/// Iterative rather than recursive: the depth of the report follows the nesting of the
/// stored schema, so walking it must not grow the stack with it.
fn deepest<'a>(failed: &'a ::boon::ValidationError) -> &'a ::boon::ValidationError<'a, 'a> {
    let mut leaf = failed;
    while let Some(cause) = leaf.causes.first() {
        leaf = cause;
    }
    leaf
}

/// The finding about the violating node, as the error class words it.
///
/// A type mismatch is the one finding whose wording is recorded: the instance's own
/// primitive type beside the ones the schema allows, each one quoted with the quotes
/// ESCAPED, as the report carries them — the report is a document that was quoted once
/// already by the time it reaches the message. Every other keyword is reported with the
/// evaluator's own finding, which names the keyword that failed and what it wanted — a
/// finding invented per keyword here would be wording no validator produced.
fn finding(kind: &::boon::ErrorKind) -> String {
    match kind {
        ::boon::ErrorKind::Type { got, want } => format!(
            "instance type ({got}) does not match any allowed primitive type (allowed: [{}])",
            want.iter()
                .map(|allowed| format!("\\\"{allowed}\\\""))
                .collect::<Vec<String>>()
                .join(",")
        ),
        other => other.to_string(),
    }
}

/// How AppConfig refuses a call whose candidate configuration a `JSON_SCHEMA` validator
/// reported non-compliant.
///
/// The schema document is echoed as the profile carries it, and the bracketed report is
/// derived from the candidate and that schema: the finding about the violating node and the
/// node itself. Nothing of any one run is written here — both variable parts come from the
/// stored documents the evaluation was run over.
fn schema_violation_message(schema_document: &str, finding: &str, node: &str) -> String {
    format!(
        "Could not build JSON schema from invalid input:\n {schema_document}.\n Original \
         error message:\n[message: \"{}\" - parent node: {node}]",
        summarised(finding)
    )
}

/// `detail` as a refusal carries it: on one line, because the `Message` member carries one
/// text, and no longer than [`MAX_DETAIL_CHARS`].
fn summarised(detail: &str) -> String {
    let flattened = detail.split_whitespace().collect::<Vec<&str>>().join(" ");
    match flattened.char_indices().nth(MAX_DETAIL_CHARS) {
        Some((end, _)) => format!("{}\u{2026}", &flattened[..end]),
        None => flattened,
    }
}
