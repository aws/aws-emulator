// SPDX-License-Identifier: CC0-1.0

//! The CloudFormation template a stack carries: reading it, and evaluating what it
//! declares.
//!
//! A stack's `Outputs`, and the identifiers a client reads out of them, are not request
//! input — they are COMPUTED from the template body the stack stores:
//!
//! 1. [`parse`] reads the body. CloudFormation accepts a template as JSON *or* YAML
//!    ("You can write CloudFormation templates in JSON or YAML"), and the YAML form may
//!    spell an intrinsic function with its short-form tag (`!Ref x` is documented as the
//!    short form of `{"Ref": "x"}`), so the YAML reader turns every short-form tag back
//!    into the long form before anything looks at the tree. One `serde_json::Value`
//!    therefore represents either spelling.
//! 2. [`provision`] turns each `Resources` entry into a [`StoredResource`]: its physical
//!    resource id and the attributes `Fn::GetAtt` reads off it, per the "Return values"
//!    section the resource-type reference publishes for that type.
//! 3. [`evaluate_outputs`] evaluates each `Outputs` entry's `Value` against the stack's
//!    parameters, its resources and the pseudo parameters, which is what
//!    `DescribeStacks` reports as `Stack$Outputs`.
//!
//! Beside those three, [`process`] computes a body's PROCESSED stage — the document
//! CloudFormation deploys and `GetTemplate(TemplateStage=Processed)` serves — by applying
//! the transforms the template declares, under its own expansion bounds
//! ([`MAX_EXPANDED_ENTRIES`], [`MAX_EXPANDED_BYTES`], [`MAX_EXPANSION_DEPTH`]). See the
//! transform section further down for which transforms this service can apply and what it
//! answers for the ones it cannot.
//!
//! A `Conditions` section decides what the stack HAS: a resource or an output the
//! template declares under a condition that evaluates false is not part of it ("you can
//! … create resources conditionally"), while a condition this module cannot evaluate
//! leaves both in place rather than dropping them.
//!
//! A stack that finished deploying reports a VALUE for every output its template
//! declares, because that is the only way a client learns what CloudFormation created:
//! `Ref` answers a declared resource's physical id, `Fn::GetAtt` the named attribute of
//! one — from that type's published "Return values" where [`return_values`] carries it,
//! from the resource's own declared property where the attribute IS that property, and
//! from CloudFormation's uniform `Arn`/`Id`/`Name` naming otherwise (see
//! [`conventional_attributes`]) — and the intrinsic functions an output composes them
//! with are evaluated rather than dropped.
//!
//! ## Creating the resource in the service that owns it
//!
//! CREATING the resource in the service that owns it is
//! [`super::provisioning`](crate::state::provisioning), which calls that service's own
//! create API for the resource types this emulator provisions. This module computes only
//! what CloudFormation's documented naming and return values give a declared resource;
//! nothing here reports a status, an error or a value for an upstream call that never ran.
//!
//! An expression this module cannot evaluate yields NO value rather than a guessed one:
//! the output is reported with its key and description and without an `OutputValue`,
//! because `Output$OutputValue` is optional and a made-up ARN is worse than an absent
//! one. What is left unevaluated is an expression whose INPUT this emulator does not
//! hold — a `Ref` to a name the template never declares, a `Fn::ImportValue` naming an
//! export no stack of this account and Region publishes, a
//! `Fn::Transform` naming a macro this emulator runs nothing for (the `AWS::Include`
//! snippets it DOES read are spliced in before this module sees the template —
//! [`super::include`]), or a
//! `Fn::GetAtt` for an attribute that is neither published for the type, declared as a
//! property of the resource, nor one of the conventional names — never a function whose
//! value follows from the template and the stack.
//!
//! ## Bounds
//!
//! The template body is caller-supplied, so a single `CreateStack`/`UpdateStack` request
//! must not be able to spend unbounded CPU, stack or memory here. Three independent
//! limits do that, and each refuses (yields no value) rather than recursing, allocating
//! or looping further:
//!
//! * **Before any tree is built** — [`document`] refuses a body past the documented
//!   `TemplateBody` maximum, whichever spelling it is in: `serde_json`'s own limit bounds
//!   nesting DEPTH, not how many bytes or nodes a FLAT body carries, so the length bound
//!   cannot live in the YAML guard alone. [`guard_yaml_body`] then bounds the NESTING the
//!   recursive YAML loader can descend into (flow collections open at once, compact
//!   indicators per line, leading indentation), none of which grows with how big a flat
//!   template is. The loader's own recursion is NOT abortable from an `EventReceiver`
//!   (`on_event` returns `()`), and exhausting the stack aborts the process rather than
//!   unwinding, so that nesting has to be refused BEFORE `load` is called.
//! * **While the tree is built** — [`MAX_TEMPLATE_NODES`] nodes and
//!   [`MAX_TEMPLATE_DEPTH`] levels for the YAML spelling, counted as its builder builds;
//!   the node bound is checked by [`within_node_bound`] for the JSON tree `serde_json`
//!   builds in one go, whose nesting is already bounded by that reader's own recursion
//!   limit (the YAML loader needs its own because an `EventReceiver` cannot abort `load`).
//!   Plus the documented per-template quotas on
//!   [resources](MAX_RESOURCES_PER_TEMPLATE), [outputs](MAX_OUTPUTS_PER_TEMPLATE) and
//!   [parameters](MAX_PARAMETERS_PER_TEMPLATE): a body past one of those is not a template
//!   CloudFormation would accept, and this reader answers `None` for it rather than
//!   reporting a truncated subset of it.
//! * **While expressions are evaluated** — ONE [`Budget`] per deployment bounds the TOTAL
//!   work of evaluating every resource name and every output of a template, counting both
//!   the nodes visited and the bytes they produce, so no composition of functions can
//!   multiply the cost of its parts. [`MAX_INTRINSIC_DEPTH`], [`MAX_RESOLVED_LIST_LEN`]
//!   and [`MAX_RESOLVED_TEXT_BYTES`] bound each single expression in addition, and
//!   `Fn::Sub` resolves each of its variables ONCE per substitution however many times its
//!   source names it.

use std::collections::{BTreeMap, BTreeSet};

use ::serde_json::{Map, Value};

use super::{StoredOutput, StoredResource};

/// The most nodes one parsed template body may hold.
///
/// Chosen: a 51,200-byte body (the documented `TemplateBody` maximum) cannot hold more
/// than this many YAML nodes in any realistic template, so the cap only ever stops a body
/// built to explode the parse.
pub const MAX_TEMPLATE_NODES: usize = 20_000;

/// The most levels one parsed template body may nest. Chosen: far past the nesting of a
/// template's `Resources` → properties → intrinsic trees.
pub const MAX_TEMPLATE_DEPTH: usize = 32;

/// The most resources one template declares.
///
/// The documented CloudFormation quota ("Resources — maximum number of resources that you
/// can declare in a template: 500"), used here as the bound on the per-stack resource
/// records this module materialises.
pub const MAX_RESOURCES_PER_TEMPLATE: usize = 500;

/// The most outputs one template declares.
///
/// The documented CloudFormation quota ("Outputs — maximum number of outputs that you can
/// declare in a template: 200").
pub const MAX_OUTPUTS_PER_TEMPLATE: usize = 200;

/// The most bytes a template's own `Description` carries.
///
/// "Description: … must be a literal string that's between 0 and 1024 bytes in length",
/// which is also the `@length(min: 1, max: 1024)` the model puts on the `Description`
/// members that report it (`ValidateTemplateOutput$Description`).
pub const MAX_DESCRIPTION_BYTES: usize = 1024;

/// How the service refuses a template whose `Description` is past that maximum: the SECTION
/// it is about, quoted, and the length it may not exceed.
///
/// Worded once, in the one place the bound lives, so the operations that read a body cannot
/// word the same template-format error two ways.
pub fn description_too_long() -> String {
    format!("Template format error: 'Description' length is greater than {MAX_DESCRIPTION_BYTES}.")
}

/// How the service refuses a `Mappings` member that is not a map: "Mappings: a mapping of keys
/// and associated values", so each member of the section is a mapping of its top-level keys to
/// the mappings that hold their values, and the refusal names the member that is not one.
fn mappings_member_not_a_map(name: &str) -> String {
    format!("Template format error: Every Mappings member {name} must be a map")
}

/// Whether one member of the `Mappings` section is spelled as a mapping is: an object whose
/// own members are the mappings a lookup's second key reads
/// (`Mappings` → map name → top-level key → second-level key → value).
///
/// A top-level key whose value is EMPTY (a YAML key with nothing under it) states no entry
/// rather than a wrong structure, exactly as an empty SECTION does ([`section_map`]), and a
/// top-level key whose name is a language-extensions DIRECTIVE ([`FOR_EACH_PREFIX`]) is a loop
/// over entries rather than one of them — answered by the rules about that directive.
fn is_a_mapping(stated: &Value) -> bool {
    stated.as_object().is_some_and(|declared| {
        declared.iter().all(|(key, value)| {
            value.is_object() || value.is_null() || key.starts_with(FOR_EACH_PREFIX)
        })
    })
}

/// `Some(message)` for a template document whose SECTIONS are spelled in a way the template
/// format does not admit, or `None` for one whose spelling it does.
///
/// These are template-FORMAT rules: each is decided from the document alone, with nothing
/// resolved and nothing deployed, which is why every operation that READS a body — the ones
/// that submit it, the one that validates it and the ones that only report on it — gives the
/// same verdict, worded once here.
///
/// * `Description` "must be a literal string that's between 0 and 1024 bytes in length"
///   ([`MAX_DESCRIPTION_BYTES`]);
/// * each member of `Mappings` is "a mapping of keys and associated values", so a member that
///   is not one — one that is no object, or whose own top-level keys do not hold the mappings
///   a lookup reads ([`is_a_mapping`]) — declares no mapping at all. The member reported is
///   the FIRST one the body declares ([`super::declaration_order`]), which is where its author
///   reads it; a member whose name is a language-extensions DIRECTIVE ([`FOR_EACH_PREFIX`])
///   declares no mapping either way and is answered by the rules about that directive.
///
/// The sections are checked in the order a template states them.
pub fn format_refusal(body: &str, declared: &Template) -> Option<String> {
    if declared
        .description
        .as_deref()
        .is_some_and(|description| description.len() > MAX_DESCRIPTION_BYTES)
    {
        return Some(description_too_long());
    }
    let members: Vec<(&String, &Value)> = declared.mappings.as_object()?.iter().collect();
    super::declaration_order::as_declared(
        body,
        super::declaration_order::MAPPINGS,
        &members,
        |(name, _)| name.as_str(),
    )
    .into_iter()
    .find(|(name, stated)| !is_a_mapping(stated) && !name.starts_with(FOR_EACH_PREFIX))
    .map(|(name, _)| mappings_member_not_a_map(name))
}

/// The most attributes one provisioned resource records for `Fn::GetAtt` to read.
///
/// A resource's attributes are its type's published return values, the conventional
/// names and the properties the template declared for it — all three bounded by the
/// template body, which is itself bounded; this is the explicit ceiling on the
/// per-resource map so one caller's template cannot grow it further. Chosen: far past
/// the properties any resource declaration carries.
pub const MAX_ATTRIBUTES_PER_RESOURCE: usize = 128;

/// How many Availability Zones a Region has, for `Fn::GetAZs`.
///
/// The account's network is modelled Region by Region rather than stored (see
/// `services/common/src/network.rs`): a Region has three Availability Zones, named
/// `<region>a`, `<region>b`, `<region>c`, which is exactly what EC2's own
/// `availability_zones_in_region` reports — so a template reading the zones here reads
/// the zones `DescribeAvailabilityZones` lists.
pub const AVAILABILITY_ZONES_PER_REGION: usize = 3;

/// The most parameters one template declares. The documented CloudFormation quota
/// ("Parameters — maximum number of parameters that you can declare in a template: 200").
pub const MAX_PARAMETERS_PER_TEMPLATE: usize = 200;

/// The most mappings one template declares. The documented CloudFormation quota ("Mappings —
/// maximum number of mappings that you can declare in your template: 200").
pub const MAX_MAPPINGS_PER_TEMPLATE: usize = 200;

/// How deep one intrinsic-function expression may nest before it is left unevaluated.
/// Chosen: deeper than any `Fn::Join`/`Fn::Sub`/`Fn::GetAtt` composition a template
/// writes, and shallow enough that the evaluator's recursion is bounded by a constant.
pub const MAX_INTRINSIC_DEPTH: usize = 16;

/// The most elements an evaluated list value may carry (`Fn::Split`'s result, a
/// `Fn::Join` list). Chosen.
pub const MAX_RESOLVED_LIST_LEN: usize = 256;

/// The most bytes one evaluated value may carry. Chosen: `Output$OutputValue` carries an
/// identifier, not a payload, and this keeps a template that joins a body into itself
/// from growing one.
pub const MAX_RESOLVED_TEXT_BYTES: usize = 8 * 1024;

/// The total work one template deployment may do: the nodes its expressions evaluate,
/// weighted by the bytes they produce (see [`Budget`]).
///
/// Chosen so that the biggest template CloudFormation accepts evaluates comfortably
/// within it (200 outputs and 500 resource names, each a composition of a handful of
/// functions) while the WORST case stays a few milliseconds of work: at most
/// `MAX_EVALUATION_WORK` nodes and at most `MAX_EVALUATION_WORK * BYTES_PER_WORK_UNIT`
/// bytes of evaluated text per request. Bounding the TOTAL — rather than only each
/// expression's depth and size — is what keeps one function's cost from being multiplied
/// by another's: a composition that would re-evaluate its parts exponentially spends the
/// budget instead, and its output is reported without a value.
pub const MAX_EVALUATION_WORK: usize = 50_000;

/// How many bytes of evaluated text count as one unit of [`Budget`].
const BYTES_PER_WORK_UNIT: usize = 256;

/// The most leading spaces a line of a YAML template body may carry, which is what block
/// nesting costs per level ([`guard_yaml_body`]). Chosen: far past the indentation of a
/// template written at any sane indent step, and small enough that the loader's recursion
/// stays shallow.
pub const MAX_TEMPLATE_INDENT: usize = 256;

/// The most flow collections (`[`, `{`) a YAML template body may have OPEN at once —
/// closers subtracted — which is the nesting a well-formed body reaches. Chosen: far past
/// any template's `!Join`/`!GetAtt`/property nesting.
pub const MAX_TEMPLATE_FLOW_DEPTH: usize = 64;

/// The most flow openers (`[`, `{`) a YAML template body may carry in total.
///
/// The backstop for [`MAX_TEMPLATE_FLOW_DEPTH`]: a byte scan cannot tell a `]` inside a
/// quoted scalar from a real closer, and such a `]` lowers the running depth while the
/// loader's own depth keeps climbing — but no body can nest deeper than its opener COUNT.
/// Chosen as the deepest the loader is MEASURED to survive with room to spare (the
/// `a_template_body_built_to_explode_the_parse_is_read_as_no_template` suite drives a body
/// of just under this many openers with every closer quoted, which is the worst case this
/// bound permits), while sitting far above the flow collections a YAML template uses in
/// total — a template written entirely in flow style is JSON, which `serde_json` reads
/// instead and which this guard never sees.
pub const MAX_TEMPLATE_FLOW_OPENERS: usize = 2_048;

/// The most compact block indicators (`- `, `? `) one line of a YAML template body may
/// carry, which is where block nesting (`- - - x`) descends without costing a column. A
/// hyphen inside a name, an ARN or a description is not one of these (no space follows
/// it). Chosen.
pub const MAX_TEMPLATE_COMPACT_PER_LINE: usize = 256;

/// The work left for one template deployment to do.
///
/// One budget is created per deployment and threaded through every evaluation it performs,
/// so what is bounded is the TOTAL work of a request rather than each expression on its
/// own: nesting depth and value size bound the factors, and this bounds their product.
#[derive(Debug)]
pub struct Budget {
    remaining: usize,
}

impl Budget {
    /// A full budget, [`MAX_EVALUATION_WORK`] units.
    pub fn new() -> Self {
        Self {
            remaining: MAX_EVALUATION_WORK,
        }
    }

    /// Charge `units` of work, or `None` once the budget is spent — which is what makes an
    /// expression answer with no value instead of with unbounded work.
    fn spend(&mut self, units: usize) -> Option<()> {
        self.remaining = self.remaining.checked_sub(units)?;
        Some(())
    }

    /// Charge for producing `bytes` of evaluated text: one unit for the node plus one per
    /// [`BYTES_PER_WORK_UNIT`] bytes, so copying bytes is paid for as well as visiting
    /// nodes.
    fn spend_text(&mut self, bytes: usize) -> Option<()> {
        self.spend(1 + bytes / BYTES_PER_WORK_UNIT)
    }
}

impl Default for Budget {
    fn default() -> Self {
        Self::new()
    }
}

/// Pre-flight bounds on an untrusted YAML body, applied BEFORE the loader builds a tree
/// from it. Only the YAML spelling needs these: the JSON spelling is read by `serde_json`,
/// which enforces its own nesting limit. (The body LENGTH is bounded for both spellings in
/// [`document_within`]; `max_bytes` is re-checked here so this guard holds on its own
/// terms.)
///
/// What has to be bounded is how deep the loader can RECURSE, because an `EventReceiver`
/// cannot abort `load` (`on_event` returns `()`) and because exhausting the stack aborts
/// the process rather than unwinding. Three measures do that, and each is a bound on
/// nesting rather than a count of how big the document is, so a large, flat template is
/// not affected by any of them:
///
/// * [`MAX_TEMPLATE_FLOW_DEPTH`] — the flow collections (`[`, `{`) open at once, with
///   closers subtracted: the nesting a well-formed body actually reaches.
/// * [`MAX_TEMPLATE_FLOW_OPENERS`] — the total flow openers, which is the backstop for
///   the one way the measure above can be fooled: this is a byte scan, so a `]` inside a
///   scalar lowers the running depth while the loader's own depth keeps climbing. However
///   many closers a body hides, its real flow nesting cannot exceed its opener COUNT.
/// * [`MAX_TEMPLATE_INDENT`] columns of leading indentation and
///   [`MAX_TEMPLATE_COMPACT_PER_LINE`] compact `- `/`? ` indicators on one line — the two
///   ways block nesting descends (a column per level across lines, or a compact entry
///   within one line).
///
/// Nothing here counts a byte that does not open nesting: a hyphen inside a name, an ARN
/// or a description is not a `- ` indicator, and a brace in a long flow expression costs
/// depth only while it is open. Over-counting (a `[` inside a quoted scalar) is the only
/// failure mode, and each bound sits far above the nesting a CloudFormation template
/// written in YAML reaches.
pub(super) fn guard_yaml_body(body: &str, max_bytes: usize) -> Option<()> {
    if body.len() > max_bytes {
        return None;
    }
    let mut flow_depth = 0usize;
    let mut flow_openers = 0usize;
    let mut compact_on_line = 0usize;
    let mut indent = 0usize;
    let mut counting_indent = true;
    let mut previous = b'\n';

    for byte in body.bytes() {
        match byte {
            b'\n' | b'\r' => {
                indent = 0;
                compact_on_line = 0;
                counting_indent = true;
            }
            b' ' | b'\t' if counting_indent => {
                indent += 1;
                if indent > MAX_TEMPLATE_INDENT {
                    return None;
                }
            }
            b'[' | b'{' => {
                counting_indent = false;
                flow_depth += 1;
                flow_openers += 1;
                if flow_depth > MAX_TEMPLATE_FLOW_DEPTH
                    || flow_openers > MAX_TEMPLATE_FLOW_OPENERS
                {
                    return None;
                }
            }
            b']' | b'}' => {
                counting_indent = false;
                flow_depth = flow_depth.saturating_sub(1);
            }
            _ => counting_indent = false,
        }
        // A `-` or `?` is a block indicator only where a token may start — at the
        // beginning of a line or after a space — and only when a space follows it, which
        // is what separates `- item` from the hyphen in `my-topic` or `a-b-c`.
        if matches!(previous, b'-' | b'?')
            && matches!(byte, b' ' | b'\t' | b'\n' | b'\r')
            && compact_on_line_opener(&mut compact_on_line)
        {
            return None;
        }
        previous = byte;
    }
    Some(())
}

/// Count one compact block indicator on the current line, answering `true` once the line
/// is past [`MAX_TEMPLATE_COMPACT_PER_LINE`].
fn compact_on_line_opener(compact_on_line: &mut usize) -> bool {
    *compact_on_line += 1;
    *compact_on_line > MAX_TEMPLATE_COMPACT_PER_LINE
}

/// The partition every ARN this emulator mints belongs to, which is what
/// `{"Ref": "AWS::Partition"}` resolves to (`::emulator_server::emulator_arn` builds
/// `arn:aws:…`).
pub const PARTITION: &str = "aws";

/// What `{"Ref": "AWS::URLSuffix"}` resolves to in the `aws` partition.
const URL_SUFFIX: &str = "amazonaws.com";

/// How many hexadecimal characters an AWS Transfer Family resource identifier carries
/// after its one-letter prefix.
///
/// Transfer's own Smithy model fixes it: `ServerId` is `^s-([0-9a-f]{17})$` and
/// `WorkflowId` is `^w-([a-z0-9]{17})$`, both `length 19..19`.
const TRANSFER_ID_HEX_DIGITS: usize = 17;

/// How many characters an Amazon Bedrock AgentCore identifier carries after its label.
///
/// The AgentCore control-plane model fixes it: every one of its identifiers ends in a
/// `-` plus exactly ten characters — `MemoryId` is
/// `^[a-zA-Z][a-zA-Z0-9-_]{0,99}-[a-zA-Z0-9]{10}$`, `AgentRuntimeId` is
/// `^[a-zA-Z][a-zA-Z0-9_]{0,99}-[a-zA-Z0-9]{10}$` and `GatewayId` is
/// `^([0-9a-z][-]?){1,100}-[0-9a-z]{10}$`. Those alphabets are wider than hexadecimal
/// (which is why this is not named after hex, as `TRANSFER_ID_HEX_DIGITS` is): the
/// suffix [`generated_service_id`] mixes is hexadecimal, and hexadecimal satisfies every
/// one of them.
const AGENTCORE_ID_SUFFIX_CHARS: usize = 10;

/// The longest LABEL an AgentCore identifier carries before that suffix.
///
/// 48, the tightest of the bounds the model publishes: `AgentRuntimeArn` admits
/// `[a-zA-Z][a-zA-Z0-9_]{0,47}` and `GatewayArn` `([0-9a-z][-]?){1,48}` before the
/// suffix, so a label cut to 48 characters satisfies both the identifier's pattern and
/// the ARN's.
const AGENTCORE_LABEL_CHARS: usize = 48;

/// The ARN namespace (and SigV4 signing name) AgentCore resources carry:
/// `arn:aws:bedrock-agentcore:<region>:<account>:<resource>`, as every `arnReference`
/// in the AgentCore control-plane model spells it (`MemoryArn`, `GatewayArn`,
/// `AgentRuntimeArn`).
const AGENTCORE_ARN_SERVICE: &str = "bedrock-agentcore";

/// The version an AgentCore Runtime carries when it is first created.
///
/// `CreateAgentRuntime` answers an `agentRuntimeVersion` (`^([1-9][0-9]{0,4})$`,
/// `length 1..5`) for the version it published, and a resource CloudFormation has just
/// declared has exactly one.
const FIRST_AGENTCORE_RUNTIME_VERSION: u32 = 1;

/// One resource a template declares, as the template spells it.
#[derive(Debug, Clone)]
pub struct DeclaredResource {
    pub logical_id: String,
    /// `Type`, e.g. `AWS::SNS::Topic`.
    pub resource_type: String,
    /// `Properties`, with intrinsic functions still unevaluated.
    pub properties: Value,
    /// `Condition`: the name of the condition that governs whether this resource is
    /// created at all ("the optional `Conditions` section … controls whether certain
    /// resources are created"). `None` for a resource the template declares
    /// unconditionally.
    pub condition: Option<String>,
    /// The WHOLE resource object the template declares, `Type` and every other
    /// `ResourceAttribute` (`Metadata`, `DeletionPolicy`, `UpdatePolicy`, …) included.
    ///
    /// A deployment only needs the type and the properties; comparing two templates needs
    /// the rest, because `ResourceChange$Scope` names the resource ATTRIBUTES that differ.
    pub declaration: Value,
}

/// One output a template declares, as the template spells it.
#[derive(Debug, Clone)]
pub struct DeclaredOutput {
    pub key: String,
    /// `Value`, with intrinsic functions still unevaluated.
    pub value: Value,
    pub description: Option<String>,
    /// `Export.Name`, with intrinsic functions still unevaluated.
    pub export_name: Option<Value>,
    /// `Condition`: the name of the condition that governs whether this output is
    /// reported at all ("Condition: … output values that are conditionally created").
    pub condition: Option<String>,
}

/// One parameter a template declares, as the template spells it.
///
/// A declaration keeps every member a client READING a template is told about
/// (`ValidateTemplate` reports `TemplateParameter$ParameterKey`, `$DefaultValue`,
/// `$NoEcho` and `$Description`), not only the two a deployment evaluates expressions
/// with — this is the one reader of the `Parameters` section.
#[derive(Debug, Clone)]
pub struct DeclaredParameter {
    /// The name the section keys this declaration by, which is what a `Ref` names and
    /// what a stack operation passes a `Parameter$ParameterKey` for.
    pub key: String,
    /// `Type` (required): "The data type for the parameter". What decides whether a
    /// `Ref` to it resolves to one value or to a LIST of them: "CommaDelimitedList: An
    /// array of literal strings that are separated by commas", and likewise every
    /// `List<…>` type.
    pub declared_type: Option<String>,
    /// `Default`, where the declaration gives a scalar one: "A value of the appropriate
    /// type for the template to use if no value is specified when a stack is created".
    /// Only a scalar default is a value a `Ref` can resolve to without knowing the
    /// parameter's type; a `List<Number>` default is left to the request.
    pub default: Option<String>,
    /// `NoEcho`: "Whether to mask the parameter value to prevent it from being displayed
    /// in the console, command line tools, or API" — `false` where the declaration omits
    /// it, which is the documented default.
    pub no_echo: bool,
    /// `Description`: "A string of up to 4000 characters that describes the parameter".
    pub description: Option<String>,
    /// `AllowedValues`: "An array containing the list of values allowed for the parameter",
    /// each as the text the declaration spells it with. It is the one constraint a client
    /// reading a template is told about — "although other constraints might be defined in
    /// the stack template, CloudFormation returns only the AllowedValues property"
    /// (`ParameterConstraints`) — and is EMPTY for a declaration that permits any value of
    /// its type.
    pub allowed_values: Vec<String>,
    /// `AllowedPattern`: "A regular expression that represents the patterns to allow for
    /// String types", as the declaration spells it.
    pub allowed_pattern: Option<String>,
    /// `ConstraintDescription`: "A string that explains a constraint when the constraint is
    /// violated" — the clause CloudFormation reports IN PLACE of the constraint's own
    /// default clause, which is what the member is documented FOR ("by adding a constraint
    /// description … you can display the following customized error message").
    pub constraint_description: Option<String>,
    /// `MinValue`: "A numeric value that determines the smallest numeric value you want to
    /// allow for Number types."
    pub min_value: Option<f64>,
    /// `MaxValue`: "A numeric value that determines the largest numeric value you want to
    /// allow for Number types."
    pub max_value: Option<f64>,
    /// `MinLength`: "An integer value that determines the smallest number of characters you
    /// want to allow for String types."
    pub min_length: Option<u64>,
    /// `MaxLength`: "An integer value that determines the largest number of characters you
    /// want to allow for String types."
    pub max_length: Option<u64>,
}

/// The latest `AWSTemplateFormatVersion`, which is the version in FORCE for a template that
/// declares none: "if you don't specify a value, CloudFormation assumes the latest template
/// format version", and the latest format version is the only value the section accepts.
pub const LATEST_FORMAT_VERSION: &str = "2010-09-09";

/// The template format version in force for `declared`: the one the template states, or
/// [`LATEST_FORMAT_VERSION`] for one that states none — a template declaring no
/// `AWSTemplateFormatVersion` still conforms to a version, which is what a reader of the
/// version (`GetTemplateSummaryOutput$Version`) asks for.
pub fn format_version_in_force(declared: &Template) -> String {
    declared
        .format_version
        .clone()
        .unwrap_or_else(|| LATEST_FORMAT_VERSION.to_string())
}

/// A template body, as this service reads it.
#[derive(Debug, Clone, Default)]
pub struct Template {
    /// The template's own `Description`, which is where CloudFormation takes
    /// `Stack$Description` from.
    pub description: Option<String>,
    /// `AWSTemplateFormatVersion`: "the CloudFormation template version that the template
    /// conforms to", as the text the template spells it with. `None` for a template that
    /// declares none, which the reader is not free to invent a version for — "if you don't
    /// specify a value, CloudFormation assumes the latest template format version", and
    /// which version that is, is CloudFormation's to know rather than the template's to
    /// state.
    pub format_version: Option<String>,
    /// The `Metadata` section: "objects that provide additional information about the
    /// template", as the template spells them. `Value::Null` for a template that declares
    /// no such section.
    pub metadata: Value,
    /// The `Parameters` section: one entry per declared parameter, in the order the
    /// reader walked the section. The SINGLE record of what a template declares as a
    /// parameter — [`Self::parameter_defaults`] and [`Self::parameter_types`] are views
    /// of it, so nothing holds a second copy.
    pub parameters: Vec<DeclaredParameter>,
    /// The `Transform` section: the names of the macros the template declares, in
    /// declaration order (`ValidateTemplateOutput$DeclaredTransforms`).
    pub transforms: Vec<String>,
    /// The `Mappings` section, which `Fn::FindInMap` reads.
    pub mappings: Value,
    /// The `Conditions` section, which `Fn::If` and a resource's or output's own
    /// `Condition` are evaluated against.
    pub conditions: Value,
    pub resources: Vec<DeclaredResource>,
    /// The logical ids of the `Resources` entries that declare NO resource, because they
    /// carry no string `Type` ("Type: required") — the entries this reader skipped.
    ///
    /// Kept because a template carrying one is refused for THAT entry rather than for the
    /// section ([`resources_refusal`]): CloudFormation names the entry it could not read,
    /// and an entry this reader dropped silently is one no operation could name.
    pub untyped_resources: Vec<String>,
    pub outputs: Vec<DeclaredOutput>,
}

impl Template {
    /// Each declared parameter's `Default` where it has a scalar one: the value a `Ref`
    /// to that parameter resolves to when the request supplied none.
    ///
    /// Builds the map on each call (the declarations are the stored form), so a caller
    /// that needs it more than once binds it to a local rather than calling in a loop.
    pub fn parameter_defaults(&self) -> BTreeMap<String, String> {
        self.parameters
            .iter()
            .filter_map(|parameter| {
                parameter
                    .default
                    .clone()
                    .map(|default| (parameter.key.clone(), default))
            })
            .collect()
    }

    /// Each declared parameter's `Type`, which is what decides whether a `Ref` to it
    /// resolves to one value or to a LIST of them.
    ///
    /// Allocates per call, the same way [`Self::parameter_defaults`] does.
    pub fn parameter_types(&self) -> BTreeMap<String, String> {
        self.parameters
            .iter()
            .filter_map(|parameter| {
                parameter
                    .declared_type
                    .clone()
                    .map(|declared_type| (parameter.key.clone(), declared_type))
            })
            .collect()
    }
}

/// The `Capability` value that acknowledges a template's IAM resources when
/// CloudFormation generates their names (the model's `Capability` enum).
pub const CAPABILITY_IAM: &str = "CAPABILITY_IAM";

/// The `Capability` value that acknowledges a template's IAM resources when the template
/// gives one of them a CUSTOM name (the model's `Capability` enum).
pub const CAPABILITY_NAMED_IAM: &str = "CAPABILITY_NAMED_IAM";

/// The IAM resource types a template has to be acknowledged for, each with the property
/// a resource of that type takes a CUSTOM name from (`None` for a type that has no such
/// property).
///
/// Source: "Acknowledging IAM resources in CloudFormation templates" in the
/// CloudFormation User Guide, which lists these resource types as the ones requiring
/// `CAPABILITY_IAM`, and "you must specify `CAPABILITY_NAMED_IAM` if you have IAM
/// resources with custom names" for the named form.
const IAM_CAPABILITY_RESOURCES: &[(&str, Option<&str>)] = &[
    (IAM_ACCESS_KEY, None),
    ("AWS::IAM::Group", Some("GroupName")),
    ("AWS::IAM::InstanceProfile", Some("InstanceProfileName")),
    ("AWS::IAM::ManagedPolicy", Some("ManagedPolicyName")),
    ("AWS::IAM::OIDCProvider", None),
    ("AWS::IAM::Policy", None),
    ("AWS::IAM::Role", Some("RoleName")),
    ("AWS::IAM::SAMLProvider", None),
    ("AWS::IAM::ServerCertificate", None),
    ("AWS::IAM::ServiceLinkedRole", None),
    ("AWS::IAM::User", Some("UserName")),
    ("AWS::IAM::UserToGroupAddition", None),
];

/// The capabilities a template's own declarations require, and the resources that
/// required them.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RequiredCapabilities {
    /// The `Capability` values, without repetition
    /// (`ValidateTemplateOutput$Capabilities`). Empty for a template that declares
    /// nothing needing acknowledgement.
    pub capabilities: Vec<String>,
    /// `CapabilitiesReason`: "the list of resources that generated the values in the
    /// Capabilities response element" — `None` when there are no capabilities to
    /// explain.
    pub reason: Option<String>,
}

/// "A list of all the template resource types that are defined in the template": the type
/// of each declared resource, without repetition, in the order the template's resources are
/// held in (`super::declared_template` sorts them by logical id, so one body yields one
/// order however the workspace was built).
///
/// A view of the declarations, like [`Template::parameter_types`], rather than a second
/// record of them: `GetTemplateSummary` reports it, and anything else that needs the set of
/// types a template defines reads it here.
pub fn declared_resource_types(template: &Template) -> Vec<String> {
    let mut types: Vec<String> = Vec::new();
    for resource in &template.resources {
        if !types.contains(&resource.resource_type) {
            types.push(resource.resource_type.clone());
        }
    }
    types
}

/// What a template has to be acknowledged for: the IAM resources it declares.
///
/// A template declaring any of [`IAM_CAPABILITY_RESOURCES`] requires
/// [`CAPABILITY_IAM`], and one that gives such a resource a custom name requires
/// [`CAPABILITY_NAMED_IAM`] instead — "if your stack template contains IAM resources
/// with custom names, you must specify CAPABILITY_NAMED_IAM". A resource the template
/// declares under a condition still counts: the acknowledgement is about what the
/// template CONTAINS, which is what the request is held to before any condition is
/// evaluated.
pub fn required_capabilities(template: &Template) -> RequiredCapabilities {
    let mut types: Vec<&str> = Vec::new();
    let mut named = false;
    for resource in &template.resources {
        let Some((resource_type, name_property)) = IAM_CAPABILITY_RESOURCES
            .iter()
            .find(|(known, _)| *known == resource.resource_type)
        else {
            continue;
        };
        if !types.contains(resource_type) {
            types.push(resource_type);
        }
        // A custom name is one the TEMPLATE sets: the property's presence is what makes
        // it custom, whatever expression spells it.
        if let Some(property) = name_property {
            if resource
                .properties
                .get(property)
                .is_some_and(|name| !name.is_null())
            {
                named = true;
            }
        }
    }
    if types.is_empty() {
        return RequiredCapabilities::default();
    }
    let capability = if named {
        CAPABILITY_NAMED_IAM
    } else {
        CAPABILITY_IAM
    };
    RequiredCapabilities {
        capabilities: vec![capability.to_string()],
        reason: Some(format!(
            "The following resource(s) require capabilities: [{}]",
            types.join(", ")
        )),
    }
}

/// Whether `provided` — a request's own `Capabilities` — acknowledges the single
/// capability `required`.
///
/// [`CAPABILITY_NAMED_IAM`] is the stronger acknowledgement: "If you have IAM resources,
/// you can specify either capability", so it also acknowledges a template whose IAM
/// resources all take generated names. [`CAPABILITY_IAM`] alone does NOT acknowledge a
/// custom-named one — "if your stack template contains IAM resources with custom names,
/// you must specify CAPABILITY_NAMED_IAM".
fn acknowledges(provided: &[String], required: &str) -> bool {
    provided
        .iter()
        .any(|value| value == required || (required == CAPABILITY_IAM && value == CAPABILITY_NAMED_IAM))
}

/// The capabilities `template` requires that `provided` leaves unacknowledged, in the order
/// [`required_capabilities`] reports them.
///
/// Empty — the only case a request may go ahead on — when the request acknowledged
/// everything the template's own declarations require.
pub fn missing_capabilities(template: &Template, provided: &[String]) -> Vec<String> {
    required_capabilities(template)
        .capabilities
        .into_iter()
        .filter(|required| !acknowledges(provided, required))
        .collect()
}

/// The `InsufficientCapabilitiesException` message naming the capabilities a request did
/// not acknowledge, as the service words it: `Requires capabilities :
/// [CAPABILITY_NAMED_IAM]`.
///
/// Built from the request itself — the capabilities the submitted template requires and
/// this request withheld — rather than from any fixed text.
pub fn insufficient_capabilities_message(missing: &[String]) -> String {
    format!("Requires capabilities : [{}]", missing.join(", "))
}

/// Read a template body, whether it is spelled as JSON or as YAML.
///
/// A body whose `Transform` section names a macro this emulator PROCESSES is processed
/// first ([`super::sam`], for `AWS::Serverless-2016-10-31`): "CloudFormation processes the
/// template fragments" a macro is given and then works with what came back, so every
/// reader of a template — a change set's plan, a deployment's resources, a stack's outputs —
/// sees the resources the macro produced rather than the ones that were its input. The
/// documented per-template quotas below are therefore applied to the PROCESSED template,
/// which is the one CloudFormation validates.
///
/// `None` for a body that is neither, for a YAML body past the nesting bounds
/// [`guard_yaml_body`] puts on an untrusted document, and for one past a documented
/// per-template quota — a caller may put anything in `TemplateBody`, and a body this
/// reader will not read declares no resources and no outputs rather than a subset of them.
/// [`not_well_formed`] is how every operation refuses such a body.
pub fn parse(body: &str) -> Option<Template> {
    parse_within(body, super::TEMPLATE_BODY_MAX_BYTES)
}

/// [`parse`] under the caller's OWN ceiling on the body ([`document_within`]): the maximum
/// SIZE of a body is a quota of the API that takes it rather than a property of the format, so
/// an operation that reads a template larger than the inline-submission quota reads it here.
pub fn parse_within(body: &str, max_bytes: usize) -> Option<Template> {
    let mut document = document_within(body, max_bytes)?;
    super::sam::expand(&mut document);
    template_of(&document)
}

/// How a body [`parse`] will not read AT ALL is refused — the ONE wording for it, so the
/// same body is not refused two different sentences by two operations.
///
/// "CloudFormation first checks if the template is valid JSON. If it isn't, CloudFormation
/// checks if the template is valid YAML. If both these checks fail, CloudFormation returns
/// a template validation error", and the error it returns for a body it cannot read as a
/// template is a `Template format error` saying the body is not well-formed. The same
/// verdict covers a body past the bounds this reader puts on an untrusted document: a body
/// it will not read is not a template it read part of.
pub const NOT_WELL_FORMED: &str = "Template format error: not well-formed.";

/// How a body [`parse`] will not read is refused, naming where the reader stopped when it
/// can say so.
///
/// "CloudFormation first checks if the template is valid JSON. If it isn't, CloudFormation
/// checks if the template is valid YAML" — so the verdict on a body that is neither is the
/// YAML reader's, and the refusal reports the line and column that reader stopped at, which
/// is what a caller needs to find the spot. A body refused for a reason that has no
/// position — one past the bounds this reader puts on an untrusted document, or one holding
/// a second document — falls back to [`NOT_WELL_FORMED`].
pub fn not_well_formed(body: &str) -> String {
    match yaml_failure_position(body) {
        Some((line, column)) => format!(
            "Template format error: YAML not well-formed. (line {line}, column {column})"
        ),
        None => NOT_WELL_FORMED.to_string(),
    }
}

/// How a body this reader DOES read but that is no template document is refused — the ONE
/// wording for it, for the same reason [`NOT_WELL_FORMED`] is worded once.
///
/// A template is "a JSON- or YAML-formatted text file" whose top level is the template's
/// SECTIONS, each named by its own key and holding what that section declares. A document
/// that is anything else states no section at all — a sequence, a scalar — and so does one
/// whose section key holds something other than the mapping that section is; neither is a
/// structure the template format has, which is what the service says about it rather than
/// giving a verdict on sections it never read.
pub const UNSUPPORTED_STRUCTURE: &str = "Template format error: unsupported structure.";

/// The sections whose value IS a mapping of what they declare: "Parameters: the values to
/// pass to your template at runtime", "Mappings: a mapping of keys and associated values",
/// "Conditions", "Metadata", "Rules", "Resources: the stack resources and their properties"
/// and "Outputs" are each an object keyed by the name of what it declares.
///
/// The sections left out are the ones whose value is not a mapping — the format version and
/// the description are text, `Transform` is a name or a list of them — so nothing here says
/// anything about their spelling.
const MAPPING_SECTIONS: [&str; 7] = [
    "Conditions",
    "Mappings",
    "Metadata",
    "Outputs",
    "Parameters",
    "Resources",
    "Rules",
];

/// The template DOCUMENT a body states — the mapping of template sections — or how its FORM
/// is refused.
///
/// The ONE place that verdict lives, so every operation that reads a body refuses the same
/// body the same way: a body this reader will not read at all is not well-formed
/// ([`not_well_formed`]), and a body it DOES read into something that is not a mapping of
/// sections — or whose section holds something other than the mapping that section is — has a
/// structure the format does not have ([`UNSUPPORTED_STRUCTURE`]). A section whose value is
/// EMPTY (a YAML key with nothing under it) states no entry rather than a wrong structure, so
/// it is left to the rule about that section's own content.
pub fn section_map(body: &str) -> Result<Value, String> {
    let Some(document) = document(body) else {
        return Err(not_well_formed(body));
    };
    let Some(sections) = document.as_object() else {
        return Err(UNSUPPORTED_STRUCTURE.to_string());
    };
    let misspelled = MAPPING_SECTIONS.iter().any(|section| {
        sections
            .get(*section)
            .is_some_and(|stated| !stated.is_object() && !stated.is_null())
    });
    if misspelled {
        return Err(UNSUPPORTED_STRUCTURE.to_string());
    }
    Ok(document)
}

/// The line and column the YAML reader stopped at, both counted from 1 as a template
/// format error reports them, or `None` when the body carries no YAML syntax error (it is
/// refused for one of the bounds instead).
fn yaml_failure_position(body: &str) -> Option<(usize, usize)> {
    /// The event stream is not needed to find the position, so nothing is built from it:
    /// this receiver drops every event, which is what keeps a second pass over a refused
    /// body as cheap as a scan.
    struct Discard;

    impl ::yaml_rust2::parser::EventReceiver for Discard {
        fn on_event(&mut self, _event: ::yaml_rust2::parser::Event) {}
    }

    // The same guard the reader itself applies, so a body refused for its nesting is not
    // scanned a second time.
    guard_yaml_body(body, super::TEMPLATE_BODY_MAX_BYTES)?;
    let error = ::yaml_rust2::parser::Parser::new_from_str(body)
        .load(&mut Discard, true)
        .err()?;
    // Taken from the error's OFFSET into the body rather than from the line and column the
    // scanner carries beside it: a body that ends mid-document is stopped at its end, and
    // the position a caller has to look at is that end — not the start of a line the body
    // does not have.
    Some(position_of(body, error.marker().index()))
}

/// The 1-based line and column of the `index`th CHARACTER of `body`.
///
/// Counts characters, because the reader's marker is a character offset rather than a byte
/// one — which is also why this is not the byte-offset reading
/// [`service_common::json_parse_fault`] does of a JSON parser's own diagnostic. A position
/// past the end of the body is its end, which is where a body that stops mid-document
/// stops.
fn position_of(body: &str, index: usize) -> (usize, usize) {
    let mut line = 1usize;
    let mut column = 1usize;
    for character in body.chars().take(index) {
        if character == '\n' {
            line += 1;
            column = 1;
        } else {
            column += 1;
        }
    }
    (line, column)
}

/// How a body that declares no resource is refused, likewise worded once.
///
/// "Resources (required): Specifies the stack resources and their properties" is the one
/// section a template cannot leave out, so a body whose `Resources` section is missing or
/// empty is refused rather than read as a template declaring nothing. An entry that IS
/// there but carries no `Type` is refused for that entry instead — see
/// [`untyped_resource`].
pub const NO_RESOURCES_DECLARED: &str =
    "Template format error: At least one Resources member must be defined.";

/// How an entry of the `Resources` section that carries no `Type` is refused: named by its
/// own path in the template, since "Type: required" is a rule about THAT entry and the
/// caller has to find it from this text alone.
pub fn untyped_resource(logical_id: &str) -> String {
    format!(
        "Template format error: [/Resources/{logical_id}] Every Resources object must \
         contain a Type member."
    )
}

/// `Some(message)` for a template whose `Resources` section this service will not read as a
/// set of declared resources, worded once for every operation that reads a body.
///
/// The two cases are distinct refusals: an entry carrying no `Type` is refused for that
/// entry — and refuses the whole template, however many of its siblings are well-formed —
/// while a section that is missing or holds nothing at all is refused for declaring no
/// resource. A template with more than one untyped entry is refused for the first of them in
/// logical-id order, so the refusal does not depend on the order the body happened to list
/// them in.
///
/// `None` for a section declaring at least one resource and no untyped entry: there is a
/// template to report on.
pub fn resources_refusal(declared: &Template) -> Option<String> {
    if let Some(logical_id) = declared.untyped_resources.iter().min() {
        return Some(untyped_resource(logical_id));
    }
    declared
        .resources
        .is_empty()
        .then(|| NO_RESOURCES_DECLARED.to_string())
}

/// The documented per-template quota this body is past, worded as the refusal, or `None`
/// when it is within every one of them.
///
/// The counts are taken on the PROCESSED template — the one CloudFormation validates. A
/// body this reader will not read AT ALL is past no quota: that is a form error, which the
/// operations reading a body answer on their own terms.
///
/// This is the rule's other half, and the ONE place the ceilings are counted:
/// [`template_of`] consults [`quota_violation_in`] and reports no part of an over-quota
/// template, while an operation that SUBMITS one refuses the request outright rather than
/// deploying a stack holding none of it — the quotas are the ceilings
/// `DescribeAccountLimits` advertises, and an advertised ceiling a write is not held to is
/// no ceiling.
pub fn quota_violation(body: &str) -> Option<String> {
    quota_violation_within(body, super::TEMPLATE_BODY_MAX_BYTES)
}

/// [`quota_violation`] under the caller's OWN ceiling on the body, for an operation that reads
/// a template larger than the inline-submission quota ([`parse_within`]): the per-template
/// quotas are the same whatever carried the body.
pub fn quota_violation_within(body: &str, max_bytes: usize) -> Option<String> {
    let mut document = document_within(body, max_bytes)?;
    super::sam::expand(&mut document);
    quota_violation_in(&document)
}

/// [`quota_violation`] over an already-parsed, already-expanded document, so a submitting
/// operation and the reader it hands the body to count the same sections once each rather
/// than disagreeing about them.
fn quota_violation_in(document: &Value) -> Option<String> {
    // In section order, so a template past more than one is refused for the same quota
    // however its body is spelled.
    METERED_SECTIONS
        .iter()
        .find_map(|(section, worded, quota)| {
            let declared = document
                .get(section)
                .and_then(Value::as_object)
                .map_or(0, Map::len);
            (declared > *quota).then(|| worded(declared, *quota))
        })
}

/// How the service words the refusal of a section past the count quota on it: the noun the
/// limit is stated with, the count the template DECLARES and the maximum.
///
/// The sections the template reader meters do not share ONE sentence — recorded behaviour, so
/// each form is written out rather than derived from the section's key. The `Resources` quota
/// is worded as a NUMBER OF things against a maximum ([`numbered_section_violation`]); the
/// `Parameters`, `Mappings` and `Outputs` quotas are worded as a COUNT against a maximum
/// ([`counted_section_violation`]), each with the noun that limit is stated with.
fn numbered_section_violation(plural: &str, declared: usize, quota: usize) -> String {
    format!(
        "Template format error: Number of {plural}, {declared}, is greater than maximum \
         allowed, {quota}"
    )
}

/// See [`numbered_section_violation`].
fn counted_section_violation(counted: &str, declared: usize, quota: usize) -> String {
    format!(
        "Template format error: {counted} count {declared} is greater than max allowed {quota}"
    )
}

/// The sections the template reader meters, each with the quota on it and the sentence its
/// refusal is worded with.
///
/// The wording is the SERVICE's for that ONE limit rather than something derived from the
/// section's key, so each is listed rather than computed — wording them together would word
/// one limit's refusal with another's.
type SectionQuota = (&'static str, fn(usize, usize) -> String, usize);

const METERED_SECTIONS: &[SectionQuota] = &[
    (
        "Parameters",
        |declared, quota| counted_section_violation("Parameter", declared, quota),
        MAX_PARAMETERS_PER_TEMPLATE,
    ),
    (
        "Mappings",
        |declared, quota| counted_section_violation("Mappings", declared, quota),
        MAX_MAPPINGS_PER_TEMPLATE,
    ),
    (
        "Resources",
        |declared, quota| numbered_section_violation("resources", declared, quota),
        MAX_RESOURCES_PER_TEMPLATE,
    ),
    (
        "Outputs",
        |declared, quota| counted_section_violation("Outputs", declared, quota),
        MAX_OUTPUTS_PER_TEMPLATE,
    ),
];

/// The `Description` a template declares, if it declares one.
pub fn description(body: &str) -> Option<String> {
    parse(body).and_then(|template| template.description)
}

/// The scheme an Amazon S3 URI carries (`s3://bucket/key`).
pub(crate) const S3_URI_SCHEME: &str = "s3://";

/// The bucket, key and object version an Amazon S3 URI names: `s3://bucket/key`, with the
/// optional `?versionId=` a versioned object is addressed by.
///
/// `None` for text that is not one — a local path, or a URL of any other scheme. The ONE
/// reading of that spelling in this crate, so the places a template may carry such a URI (a
/// SAM resource's code or definition location, an `AWS::Include` snippet's `Location`)
/// cannot drift apart about what one is.
pub(crate) fn s3_uri_parts(uri: &str) -> Option<(&str, &str, Option<&str>)> {
    let (bucket, key) = uri.strip_prefix(S3_URI_SCHEME)?.split_once('/')?;
    let (key, version) = match key.split_once("?versionId=") {
        Some((key, version)) => (key, Some(version)),
        None => (key, None),
    };
    (!bucket.is_empty() && !key.is_empty()).then_some((bucket, key, version))
}

/// The template body as one JSON value: `serde_json` for a JSON body, the YAML reader
/// otherwise.
///
/// JSON is tried first because YAML 1.2 is a superset of JSON and `serde_json` reads the
/// JSON spelling exactly (and far more cheaply) than a YAML event walk would, under its
/// own nesting limit.
///
/// The body's LENGTH is refused here, before either reader runs and for both spellings: a
/// tree costs several times the bytes it was read from (the sections below are cloned out
/// of it), and `serde_json`'s own limit bounds nesting depth rather than size, so a large
/// FLAT body would otherwise be read and copied unbounded.
pub(crate) fn document(body: &str) -> Option<Value> {
    document_within(body, super::TEMPLATE_BODY_MAX_BYTES)
}

/// [`document`] under the caller's OWN ceiling on the body.
///
/// The reader is the one reading of this format in the emulator, and a template body is the
/// same document wherever it is submitted — but the maximum SIZE of one is a quota of the
/// API that takes it, not a property of the format: AWS SAR publishes a packaged template as
/// an S3 object, whose documented maximum is far above this service's inline `TemplateBody`
/// quota. So that service reads its own bodies through here with its own ceiling, rather
/// than through a second reader of the same format. Every other bound the reader applies —
/// nodes, depth, flow and block nesting, one document, no alias — belongs to the format and
/// is the same for both callers.
pub fn document_within(body: &str, max_bytes: usize) -> Option<Value> {
    if body.len() > max_bytes {
        return None;
    }
    if let Ok(value) = ::serde_json::from_str::<Value>(body) {
        // `serde_json` bounds how deep it recurses but counts no nodes, so the tree it
        // built is held to the same node bound the YAML builder applies as it builds.
        return within_node_bound(&value).then_some(value);
    }
    yaml_document(body, max_bytes)
}

/// A constraint a parameter declaration states as a NUMBER, in whatever scalar spelling the
/// body states it with: `MinValue`/`MaxValue` are "a numeric value", and a body that quotes
/// one states the same constraint as one that does not.
fn declared_number(stated: &Value) -> Option<f64> {
    scalar_text(stated)?.trim().parse::<f64>().ok()
}

/// The same reading for a constraint stated as a COUNT of characters
/// (`MinLength`/`MaxLength`, "an integer value"), which is never negative.
fn declared_length(stated: &Value) -> Option<u64> {
    scalar_text(stated)?.trim().parse::<u64>().ok()
}

/// Whether a built tree is within [`MAX_TEMPLATE_NODES`] nodes.
///
/// Iterative, with an explicit stack: a recursive walk of a caller-supplied tree would
/// spend the process stack on nesting the caller chose. The pending stack cannot outgrow
/// the node count, which is what the bound refuses.
fn within_node_bound(document: &Value) -> bool {
    let mut nodes = 0usize;
    let mut pending: Vec<&Value> = vec![document];
    while let Some(value) = pending.pop() {
        nodes += 1;
        if nodes > MAX_TEMPLATE_NODES {
            return false;
        }
        match value {
            Value::Array(items) => pending.extend(items.iter()),
            Value::Object(entries) => pending.extend(entries.values()),
            _ => {}
        }
    }
    true
}

/// The sections of a parsed template this service reads, or `None` when a section is past
/// the documented quota on it (such a template is not one CloudFormation accepts, so
/// nothing here reports part of it — [`quota_violation_in`] is where those ceilings are
/// counted, once, for this reader and for the operations that refuse such a body).
fn template_of(document: &Value) -> Option<Template> {
    if quota_violation_in(document).is_some() {
        return None;
    }
    let mut template = Template {
        mappings: document
            .get("Mappings")
            .filter(|mappings| mappings.is_object())
            .cloned()
            .unwrap_or(Value::Null),
        // "Conditions: Conditions that control whether certain resources are created or
        // whether certain resource properties are assigned a value" — a section, like
        // `Mappings`, that the evaluator reads rather than a declaration list.
        conditions: document
            .get("Conditions")
            .filter(|conditions| conditions.is_object())
            .cloned()
            .unwrap_or(Value::Null),
        // "The optional Metadata section … objects that provide additional information
        // about the template": a section read as the document it is, like `Mappings` and
        // `Conditions`, rather than a declaration list.
        metadata: document
            .get("Metadata")
            .filter(|metadata| metadata.is_object())
            .cloned()
            .unwrap_or(Value::Null),
        ..Template::default()
    };
    // The version the template STATES, in whatever scalar spelling it states it (a JSON
    // body quotes it; a YAML one may not).
    template.format_version = document
        .get("AWSTemplateFormatVersion")
        .and_then(scalar_text);
    template.description = document
        .get("Description")
        .and_then(Value::as_str)
        .filter(|description| !description.is_empty())
        .map(ToString::to_string);
    if let Some(parameters) = document.get("Parameters").and_then(Value::as_object) {
        for (name, declaration) in parameters {
            template.parameters.push(DeclaredParameter {
                key: name.clone(),
                // "Type (required): The data type for the parameter" — kept because a
                // list-typed parameter's value is an ARRAY to every function that reads
                // it.
                declared_type: declaration
                    .get("Type")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                // Only a scalar `Default` is a value a `Ref` can resolve to without
                // knowing the parameter's type; a `List<Number>` default is left to the
                // request.
                default: declaration.get("Default").and_then(scalar_text),
                no_echo: declaration.get("NoEcho").is_some_and(flag),
                description: declaration
                    .get("Description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                    .map(ToString::to_string),
                // Every value the declaration permits, each as the text it spells it with:
                // a numeric allowed value is reported as text for the same reason a
                // numeric `Default` is.
                allowed_values: declaration
                    .get("AllowedValues")
                    .and_then(Value::as_array)
                    .map(|values| values.iter().filter_map(scalar_text).collect())
                    .unwrap_or_default(),
                // The other constraints a declaration may state, each read in whatever
                // scalar spelling the body states it with (a JSON body quotes a number
                // where a YAML one may not), since every one of them is a value a request's
                // own parameter value is held to.
                allowed_pattern: declaration
                    .get("AllowedPattern")
                    .and_then(Value::as_str)
                    .filter(|pattern| !pattern.is_empty())
                    .map(ToString::to_string),
                constraint_description: declaration
                    .get("ConstraintDescription")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                    .map(ToString::to_string),
                min_value: declaration.get("MinValue").and_then(declared_number),
                max_value: declaration.get("MaxValue").and_then(declared_number),
                min_length: declaration.get("MinLength").and_then(declared_length),
                max_length: declaration.get("MaxLength").and_then(declared_length),
            });
        }
    }
    template.transforms = declared_transforms(document.get("Transform"));
    if let Some(resources) = document.get("Resources").and_then(Value::as_object) {
        for (logical_id, declaration) in resources {
            // "The Resources section declares the AWS resources that you want to include
            // in the stack … Type: required": an entry with no string `Type` declares no
            // resource.
            let Some(resource_type) = declaration.get("Type").and_then(Value::as_str) else {
                template.untyped_resources.push(logical_id.clone());
                continue;
            };
            template.resources.push(DeclaredResource {
                logical_id: logical_id.clone(),
                resource_type: resource_type.to_string(),
                properties: declaration
                    .get("Properties")
                    .cloned()
                    .unwrap_or(Value::Null),
                condition: declaration
                    .get("Condition")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
                declaration: declaration.clone(),
            });
        }
    }
    if let Some(outputs) = document.get("Outputs").and_then(Value::as_object) {
        for (key, declaration) in outputs {
            // "Value (required): The value of the property returned by the
            // DescribeStacks operation."
            let Some(value) = declaration.get("Value") else {
                continue;
            };
            template.outputs.push(DeclaredOutput {
                key: key.clone(),
                value: value.clone(),
                description: declaration
                    .get("Description")
                    .and_then(Value::as_str)
                    .filter(|description| !description.is_empty())
                    .map(ToString::to_string),
                export_name: declaration
                    .get("Export")
                    .and_then(|export| export.get("Name"))
                    .cloned(),
                condition: declaration
                    .get("Condition")
                    .and_then(Value::as_str)
                    .map(ToString::to_string),
            });
        }
    }
    Some(template)
}

/// The names the `Transform` section declares.
///
/// "The optional Transform section … specifies one or more macros that CloudFormation
/// uses to process your template", spelled as a single name or as a list of them, and a
/// macro entry may be the name alone or an object carrying `Name` beside the macro's
/// `Parameters` — so the NAME is what is reported either way, and an entry that names
/// nothing contributes nothing.
pub(crate) fn declared_transforms(section: Option<&Value>) -> Vec<String> {
    fn transform_name(entry: &Value) -> Option<String> {
        match entry {
            Value::String(name) => Some(name.clone()),
            Value::Object(_) => entry
                .get("Name")
                .and_then(Value::as_str)
                .map(ToString::to_string),
            _ => None,
        }
    }
    match section {
        Some(Value::Array(entries)) => entries.iter().filter_map(transform_name).collect(),
        Some(entry) => transform_name(entry).into_iter().collect(),
        None => Vec::new(),
    }
}

/// A template scalar as the boolean CloudFormation reads it: a true boolean, or the text
/// a JSON template spells one as (`"NoEcho": "true"`), case-insensitively. Anything else
/// is not a boolean and reads as `false`.
fn flag(value: &Value) -> bool {
    match value {
        Value::Bool(flag) => *flag,
        Value::String(text) => text.eq_ignore_ascii_case("true"),
        _ => false,
    }
}

/// A scalar as the text an intrinsic function substitutes for it. A number or a boolean
/// in a template is spelled back as the text CloudFormation substitutes; a list or an
/// object is not a scalar at all.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// The YAML spelling of a template
// ---------------------------------------------------------------------------

/// Read a YAML template body into the same tree a JSON body parses to, expanding every
/// CloudFormation short-form intrinsic tag into its long form.
///
/// Built on the parser's EVENT stream rather than on a loaded tree for two reasons: the
/// loader drops the `!Ref`/`!GetAtt` tags that carry a template's whole meaning, and an
/// event walk sees a YAML alias (`*anchor`) BEFORE the loader copies the anchored node,
/// so an alias chain cannot be expanded here at all.
fn yaml_document(body: &str, max_bytes: usize) -> Option<Value> {
    #[derive(Default)]
    struct Builder {
        /// The open collections, innermost last: each holds the items/entries read so
        /// far and the tag the collection itself carried.
        open: Vec<Frame>,
        /// The finished document, once the outermost node closes.
        root: Option<Value>,
        /// Set when the document is one this reader will not build: an alias, a second
        /// document, or a body past [`MAX_TEMPLATE_NODES`]/[`MAX_TEMPLATE_DEPTH`].
        refused: bool,
        nodes: usize,
    }

    enum Frame {
        Sequence {
            tag: Option<String>,
            items: Vec<Value>,
        },
        Mapping {
            tag: Option<String>,
            entries: Map<String, Value>,
            /// The key read but not yet paired with its value.
            pending_key: Option<String>,
        },
    }

    impl Builder {
        /// Place one finished node: as the root, as the next item of the open sequence,
        /// or as the key or the value of the open mapping.
        fn place(&mut self, node: Value) {
            self.nodes += 1;
            if self.nodes > MAX_TEMPLATE_NODES {
                self.refused = true;
                return;
            }
            if self.open.is_empty() {
                // A second document in the same stream: a template is one document.
                if self.root.is_some() {
                    self.refused = true;
                }
                self.root = Some(node);
                return;
            }
            let mut refuse = false;
            match self.open.last_mut().expect("a frame is open, just checked") {
                Frame::Sequence { items, .. } => items.push(node),
                Frame::Mapping {
                    entries,
                    pending_key,
                    ..
                } => match pending_key.take() {
                    // A mapping key is a scalar; a template never keys a section by a
                    // list or a map, and this reader does not invent a spelling for one.
                    None => match scalar_text(&node) {
                        Some(key) => *pending_key = Some(key),
                        None => refuse = true,
                    },
                    Some(key) => {
                        entries.insert(key, node);
                    }
                },
            }
            if refuse {
                self.refused = true;
            }
        }

        fn open_frame(&mut self, frame: Frame) {
            if self.open.len() >= MAX_TEMPLATE_DEPTH {
                self.refused = true;
                return;
            }
            self.open.push(frame);
        }

        fn close_frame(&mut self) {
            let Some(frame) = self.open.pop() else {
                self.refused = true;
                return;
            };
            let (tag, node) = match frame {
                Frame::Sequence { tag, items } => (tag, Value::Array(items)),
                Frame::Mapping { tag, entries, .. } => (tag, Value::Object(entries)),
            };
            let node = match tag {
                Some(tag) => long_form(&tag, node),
                None => node,
            };
            self.place(node);
        }
    }

    impl ::yaml_rust2::parser::EventReceiver for Builder {
        fn on_event(&mut self, event: ::yaml_rust2::parser::Event) {
            use ::yaml_rust2::parser::Event;
            if self.refused {
                return;
            }
            match event {
                Event::Scalar(value, style, _anchor, tag) => {
                    let quoted = !matches!(style, ::yaml_rust2::scanner::TScalarStyle::Plain);
                    let node = yaml_scalar(value, quoted);
                    let node = match short_form_tag(tag.as_ref()) {
                        Some(name) => long_form(&name, node),
                        None => node,
                    };
                    self.place(node);
                }
                Event::SequenceStart(_anchor, tag) => {
                    let tag = short_form_tag(tag.as_ref());
                    self.open_frame(Frame::Sequence {
                        tag,
                        items: Vec::new(),
                    });
                }
                Event::MappingStart(_anchor, tag) => {
                    let tag = short_form_tag(tag.as_ref());
                    self.open_frame(Frame::Mapping {
                        tag,
                        entries: Map::new(),
                        pending_key: None,
                    });
                }
                Event::SequenceEnd | Event::MappingEnd => self.close_frame(),
                // An alias copies an anchored node, which is how a few hundred bytes of
                // YAML expand into gigabytes. Nothing in a CloudFormation template needs
                // one, so a body carrying one is not read here.
                Event::Alias(_) => self.refused = true,
                _ => {}
            }
        }
    }

    let mut builder = Builder::default();
    guard_yaml_body(body, max_bytes)?;
    ::yaml_rust2::parser::Parser::new_from_str(body)
        .load(&mut builder, true)
        .ok()?;
    if builder.refused || !builder.open.is_empty() {
        return None;
    }
    builder.root
}

/// The intrinsic function a YAML tag names, if it names one.
///
/// A CloudFormation short-form tag is a LOCAL tag (handle `!`) whose suffix is the
/// function's name: `!Ref`, `!GetAtt`, `!Sub`, … A standard YAML tag (`!!str`, handle
/// `!!`) is not one, and neither is an unknown local tag.
fn short_form_tag(tag: Option<&::yaml_rust2::parser::Tag>) -> Option<String> {
    let tag = tag?;
    if tag.handle != "!" {
        return None;
    }
    SHORT_FORM_FUNCTIONS
        .iter()
        .find(|name| **name == tag.suffix)
        .map(|name| (*name).to_string())
}

/// Every intrinsic function (and condition function) CloudFormation documents a YAML
/// short form for, plus `Ref` and `Condition`.
///
/// Source: "Intrinsic function reference" and "Condition functions" in the
/// CloudFormation User Guide, which give each function's full name and note that "you can
/// use the short form `!Fn::…`" in YAML.
const SHORT_FORM_FUNCTIONS: [&str; 20] = [
    "And",
    "Base64",
    "Cidr",
    "Condition",
    "Equals",
    "FindInMap",
    "GetAZs",
    "GetAtt",
    "If",
    "ImportValue",
    "Join",
    "Length",
    "Not",
    "Or",
    "Ref",
    "Select",
    "Split",
    "Sub",
    "ToJsonString",
    "Transform",
];

/// One short-form tag as the long form a JSON template spells: `!Ref x` is
/// `{"Ref": "x"}`, `!Condition c` is `{"Condition": "c"}`, and every other function is
/// `{"Fn::<Name>": …}`.
fn long_form(name: &str, node: Value) -> Value {
    let key = match name {
        "Ref" | "Condition" => name.to_string(),
        other => format!("Fn::{other}"),
    };
    let mut wrapper = Map::new();
    wrapper.insert(key, node);
    Value::Object(wrapper)
}

/// One YAML scalar as a JSON value.
///
/// A QUOTED scalar is always text. A plain one resolves by YAML's core schema, so
/// `Outputs` that join a number or a boolean see the same value a JSON template would
/// give; anything else is text.
///
/// A number is only taken as a number when writing it back gives the SAME text the
/// template spelled: CloudFormation substitutes what the template says, so `007` and a
/// 20-digit account id stay the text they are rather than becoming `7` or an exponent.
fn yaml_scalar(value: String, quoted: bool) -> Value {
    if quoted {
        return Value::String(value);
    }
    match value.as_str() {
        "" | "~" | "null" | "Null" | "NULL" => return Value::Null,
        "true" | "True" | "TRUE" => return Value::Bool(true),
        "false" | "False" | "FALSE" => return Value::Bool(false),
        _ => {}
    }
    if let Some(number) = value
        .parse::<i64>()
        .ok()
        .map(|integer| Value::Number(integer.into()))
        .or_else(|| {
            value
                .parse::<f64>()
                .ok()
                .and_then(::serde_json::Number::from_f64)
                .map(Value::Number)
        })
        .filter(|number| scalar_text(number).as_deref() == Some(value.as_str()))
    {
        return number;
    }
    Value::String(value)
}

// ---------------------------------------------------------------------------
// Resources
// ---------------------------------------------------------------------------

/// The stack a template is being evaluated for: what the pseudo parameters resolve to,
/// and what `Ref`/`Fn::GetAtt` resolve against.
#[derive(Debug, Clone, Copy)]
pub struct StackScope<'a> {
    pub region: &'a str,
    pub stack_name: &'a str,
    pub stack_id: &'a str,
    /// The Amazon SNS topic ARNs the stack publishes its events to, in the order the stack
    /// records them — what `AWS::NotificationARNs` stands for. Empty for a stack that
    /// carries none, which is a resolvable value (the empty list) and not an unknown.
    pub notification_arns: &'a [String],
}

/// Materialise the resources a template declares.
///
/// Each resource gets the physical resource id CloudFormation's documented naming rules
/// give it — the name the template asked for where it names one, CloudFormation's
/// generated `<stack>-<logical id>-<suffix>` name where it does not — and the attributes
/// that type's "Return values" section publishes for `Fn::GetAtt`.
///
/// A name the template builds with an intrinsic function is evaluated first (the common
/// `TopicName: !Ref TopicName` case), against the stack's parameters and pseudo
/// parameters — and against the resources of the same template that are created BEFORE
/// this one: the resources are named in creation order ([`creation_order`]), so a name
/// built from a `Ref` to another resource of the template resolves to that resource's own
/// physical id, exactly as CloudFormation resolves it. A resource takes the generated name
/// only where its declaration names it nowhere, or names it from something this module
/// cannot resolve at all (a resource of a dependency cycle, which no stack could deploy).
/// (What a later round resolves is an IDENTIFIER built from another resource's — see
/// [`resolve_dependent_identities`] — not the name it was computed from.)
///
/// Every record starts out NOT materialised: this function computes the identity
/// CloudFormation gives the resource, and
/// [`super::provisioning`](crate::state::provisioning) is what then creates it in the
/// service that owns it, where this emulator provisions that type.
///
/// A resource the template declares under a `Condition` is materialised only where that
/// condition HOLDS ("you can … create resources conditionally"); a condition this module
/// cannot evaluate leaves the resource in place rather than silently dropping it.
///
/// `scope` is the deployment's scope with NO resources resolved yet — which is what this
/// function produces. The attributes each resource answers are filled in in a second
/// pass, once every resource's identity is known, so an attribute that is the resource's
/// own declared property resolves even where that property refers to another resource of
/// the same template (`VpcId: !Ref Vpc`). A type whose own IDENTIFIER is a function of
/// another resource's is resolved the same way, by
/// [`resolve_dependent_identities`].
pub fn provision(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<StoredResource> {
    let mut names: Vec<(&DeclaredResource, String)> = Vec::new();
    let mut identities: BTreeMap<String, StoredResource> = BTreeMap::new();
    // CREATION order, so the resources a declaration references are already named when its
    // own name is evaluated: that is the order CloudFormation creates them in, and a
    // resource it has created has the identity a `Ref` to it resolves to.
    for declared in creation_order(template) {
        if !is_created(declared, scope, budget) {
            continue;
        }
        let name = {
            let named_so_far = Scope {
                resources: &identities,
                ..*scope
            };
            physical_name(declared, &named_so_far, budget)
        };
        let (physical_resource_id, attributes) = return_values(declared, &name, scope, budget);
        identities.insert(
            declared.logical_id.clone(),
            StoredResource {
                logical_id: declared.logical_id.clone(),
                resource_type: declared.resource_type.clone(),
                physical_resource_id,
                attributes,
                metadata: declared_metadata(declared),
                // Filled by [`resolve_properties`], once every resource's identity is
                // known; `materialised` by
                // [`super::provisioning`](crate::state::provisioning), once the owning
                // service has created the resource.
                properties: Value::Null,
                materialised: false,
                deletion_policy: declared_deletion_policy(declared),
            },
        );
        names.push((declared, name));
    }
    // The few types whose own identifier is built from ANOTHER resource's had nothing to
    // resolve against above, so they are recomputed once the rest are known.
    resolve_dependent_identities(&names, &mut identities, scope, budget);

    let mut provisioned: Vec<StoredResource> = names
        .iter()
        .filter_map(|(declared, _)| identities.get(&declared.logical_id).cloned())
        .collect();
    let resolved = Scope {
        resources: &identities,
        ..*scope
    };
    for (resource, (declared, name)) in provisioned.iter_mut().zip(&names) {
        attach_attributes(resource, declared, name, &resolved, budget);
    }
    provisioned
}

/// The template's resources in CREATION order: each one after every resource of the same
/// template it DEPENDS ON.
///
/// That is the order CloudFormation creates a stack's resources in — it reads `DependsOn`
/// and the implicit dependencies a `Ref` or an `Fn::GetAtt` to another resource creates,
/// and creates each resource only once the resources it needs exist. It matters because
/// [`crate::state::provisioning`] walks this order when it calls the owning services: a
/// pipeline whose `RoleArn` is `Fn::GetAtt` of a role of the same template cannot be
/// created before that role is.
///
/// A stable topological order: among resources with no dependency between them the
/// DECLARATION order is kept, so a template whose resources are independent is created,
/// recorded and reported exactly as it is written. A dependency cycle (which no stack
/// could deploy) leaves the resources it holds in declaration order rather than dropping
/// them.
fn creation_order(template: &Template) -> Vec<&DeclaredResource> {
    creation_order_of(template, &dependency_map(template))
}

/// [`creation_order`] over a dependency relation that has ALREADY been built.
///
/// The relation is the expensive half — one scan of every resource's declaration — so a
/// caller that needs both the order and the relation itself ([`waits_on_each`]) builds it
/// once and passes it here rather than having it built a second time. `needs` is in
/// DECLARATION order, one entry per `template.resources`, which is what
/// [`dependency_map`] answers.
fn creation_order_of<'a>(
    template: &'a Template,
    needs: &[BTreeSet<String>],
) -> Vec<&'a DeclaredResource> {
    let declared: Vec<&DeclaredResource> = template.resources.iter().collect();
    let mut emitted: BTreeSet<String> = BTreeSet::new();
    let mut ordered: Vec<&DeclaredResource> = Vec::with_capacity(declared.len());
    let mut taken = vec![false; declared.len()];
    // One pass per resource at most: each pass emits every resource whose dependencies
    // are already out, in declaration order.
    for _ in 0..declared.len() {
        let mut progressed = false;
        for (index, resource) in declared.iter().enumerate() {
            if taken[index] {
                continue;
            }
            if needs
                .get(index)
                .is_some_and(|needed| needed.iter().any(|name| !emitted.contains(name.as_str())))
            {
                continue;
            }
            taken[index] = true;
            emitted.insert(resource.logical_id.clone());
            ordered.push(resource);
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    // Anything a cycle held back, in declaration order.
    for (index, resource) in declared.iter().enumerate() {
        if !taken[index] {
            ordered.push(resource);
        }
    }
    ordered
}

/// The DIRECT dependency relation of a template, keyed by logical id: for each resource
/// that depends on any resource of the same template, the logical ids it depends on.
///
/// The same relation [`creation_order`] walks — `DependsOn` plus the implicit dependency a
/// `Ref`, an `Fn::GetAtt` or an `Fn::Sub` variable creates — handed out so a caller can
/// answer "what did this resource wait for": which is what makes a stack operation's event
/// log a TIMELINE rather than a set ([`crate::state::timeline`]). A resource that depends
/// on nothing is absent from the answer.
///
/// ONE pass over an already bounded body (a template past the documented per-template
/// quotas is refused before anything reads it, [`quota_violation`]), so this costs what
/// [`creation_order`] itself costs.
pub fn direct_dependencies(template: &Template) -> BTreeMap<String, BTreeSet<String>> {
    template
        .resources
        .iter()
        .zip(dependency_map(template))
        .filter(|(_, needs)| !needs.is_empty())
        .map(|(resource, needs)| (resource.logical_id.clone(), needs))
        .collect()
}

/// The dependency relation of a whole template, built ONCE: one entry per resource, in
/// declaration order, carrying the logical ids that resource depends on.
fn dependency_map(template: &Template) -> Vec<BTreeSet<String>> {
    template
        .resources
        .iter()
        .map(|resource| dependencies_of(resource, template))
        .collect()
}

/// The logical ids of the resources of the same template that `resource` depends on:
/// every one its `DependsOn` names, plus every one a `Ref`, an `Fn::GetAtt` or an
/// `Fn::Sub` variable inside its declaration resolves against.
///
/// Only names the template declares as RESOURCES count: a `Ref` to a parameter or to a
/// pseudo parameter is not a dependency on anything that has to be created first.
fn dependencies_of(resource: &DeclaredResource, template: &Template) -> BTreeSet<String> {
    let mut referenced: BTreeSet<String> = references(&resource.declaration)
        .names()
        .into_iter()
        .map(ToString::to_string)
        .collect();
    if let Some(depends_on) = resource.declaration.get("DependsOn") {
        match depends_on {
            Value::String(name) => {
                referenced.insert(name.clone());
            }
            Value::Array(names) => {
                for name in names {
                    if let Some(name) = name.as_str() {
                        referenced.insert(name.to_string());
                    }
                }
            }
            _ => {}
        }
    }
    referenced.remove(&resource.logical_id);
    referenced
        .into_iter()
        .filter(|name| {
            template
                .resources
                .iter()
                .any(|declared| &declared.logical_id == name)
        })
        .collect()
}

/// Which of `roots` each resource of the template WAITS ON: for every resource that
/// depends on one of those ids, directly or through another resource, the ids it waits on.
/// A resource that waits on none is absent from the answer, and a root is never reported
/// as waiting on itself.
///
/// The relation walked is the one [`creation_order`] walks — `DependsOn` plus the implicit
/// dependency a `Ref`, an `Fn::GetAtt` or an `Fn::Sub` variable creates — so a resource
/// waits on a root exactly when CloudFormation would not create it before that root
/// exists. This is what a stack operation gated on a wait condition has NOT started
/// ([`crate::state::wait_conditions`]).
///
/// ONE pass over the template, whatever `roots` holds: the dependency relation is built
/// once ([`dependency_map`]), and each resource is visited once in CREATION order — which
/// puts it after everything it depends on — taking the union of what those wait on plus
/// any root among them. The work is charged to the deployment's [`Budget`], like every
/// other evaluation over a caller-supplied body, and a template that exhausts it is
/// answered with what the pass had reached: fewer resources reported as waiting, never
/// unbounded work.
pub fn waits_on_each(
    template: &Template,
    roots: &BTreeSet<String>,
    budget: &mut Budget,
) -> BTreeMap<String, BTreeSet<String>> {
    let mut waiting: BTreeMap<String, BTreeSet<String>> = BTreeMap::new();
    if roots.is_empty() {
        return waiting;
    }
    let needs = dependency_map(template);
    let by_logical_id: BTreeMap<&str, &BTreeSet<String>> = template
        .resources
        .iter()
        .zip(&needs)
        .map(|(resource, needed)| (resource.logical_id.as_str(), needed))
        .collect();
    for resource in creation_order_of(template, &needs) {
        let Some(dependencies) = by_logical_id.get(resource.logical_id.as_str()) else {
            continue;
        };
        // Charged per dependency examined, which is what this pass costs.
        if budget.spend(1 + dependencies.len()).is_none() {
            break;
        }
        let mut waits: BTreeSet<String> = BTreeSet::new();
        for dependency in dependencies.iter() {
            if roots.contains(dependency) {
                waits.insert(dependency.clone());
            }
            if let Some(inherited) = waiting.get(dependency) {
                waits.extend(inherited.iter().cloned());
            }
        }
        waits.remove(&resource.logical_id);
        if !waits.is_empty() {
            waiting.insert(resource.logical_id.clone(), waits);
        }
    }
    waiting
}

/// The names the intrinsic functions inside one template value refer to, split by HOW each
/// is referred to.
///
/// The split is what `ResourceChangeDetail$ChangeSource` names: a `Ref` names a VALUE —
/// "`ParameterReference` entities are `Ref` intrinsic functions that get template parameter
/// values", "`ResourceReference` entities are `Ref` intrinsic functions that refer to
/// resources in the template" — while "`ResourceAttribute` entities are `Fn::GetAtt`
/// intrinsic functions that get resource attribute values". `Fn::Sub`'s `${Name}` and
/// `${Name.Attribute}` variables are those same two forms spelled inside a string.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct References {
    /// The names a `Ref` (or a bare `${Name}`) names: a parameter, a pseudo parameter or a
    /// resource of the same template.
    pub by_value: BTreeSet<String>,
    /// The ATTRIBUTE PATHS an `Fn::GetAtt` (or a `${Name.Attribute}`) reads, each spelled
    /// as that intrinsic spells it: `<logical id>.<attribute>`, which is the whole entity a
    /// change caused by one of them names.
    pub by_attribute: BTreeSet<String>,
}

impl References {
    /// Every RESOURCE name referred to, however it is referred to: an attribute path names
    /// its resource in the segment before the first dot.
    pub fn names(&self) -> BTreeSet<&str> {
        self.by_value
            .iter()
            .map(String::as_str)
            .chain(self.by_attribute.iter().map(|path| resource_of(path)))
            .collect()
    }

    /// Each attribute path with the resource it reads the attribute off.
    pub fn attribute_targets(&self) -> impl Iterator<Item = (&str, &str)> {
        self.by_attribute
            .iter()
            .map(|path| (resource_of(path), path.as_str()))
    }
}

/// The resource an `Fn::GetAtt` path names: everything before the first dot.
pub fn resource_of(attribute_path: &str) -> &str {
    attribute_path
        .split('.')
        .next()
        .unwrap_or(attribute_path)
}

/// Every name the intrinsic functions inside one template value refer to.
pub fn references(value: &Value) -> References {
    let mut found = References::default();
    collect_references(value, 0, &mut found);
    found
}

fn collect_references(value: &Value, depth: usize, into: &mut References) {
    if depth > MAX_TEMPLATE_DEPTH {
        return;
    }
    match value {
        Value::Object(members) => {
            for (key, member) in members {
                match (key.as_str(), member) {
                    ("Ref", Value::String(name)) => {
                        into.by_value.insert(name.clone());
                    }
                    // `Fn::GetAtt` takes `[logicalId, attribute]` or the `logicalId.attribute`
                    // short form, and names the same entity either way.
                    ("Fn::GetAtt", Value::Array(arguments)) => {
                        let path: Vec<&str> =
                            arguments.iter().filter_map(Value::as_str).collect();
                        if !path.is_empty() {
                            into.by_attribute.insert(path.join("."));
                        }
                    }
                    ("Fn::GetAtt", Value::String(path)) => {
                        into.by_attribute.insert(path.clone());
                    }
                    // `Fn::Sub`'s `${Name}` and `${Name.Attribute}` variables name the
                    // same resources `Ref` and `Fn::GetAtt` do.
                    ("Fn::Sub", _) => {
                        let source = match member {
                            Value::String(text) => Some(text.as_str()),
                            Value::Array(arguments) => {
                                arguments.first().and_then(Value::as_str)
                            }
                            _ => None,
                        };
                        if let Some(source) = source {
                            for (name, path) in substitution_names(source) {
                                match path {
                                    Some(path) => {
                                        into.by_attribute.insert(path);
                                    }
                                    None => {
                                        into.by_value.insert(name);
                                    }
                                }
                            }
                        }
                        collect_references(member, depth + 1, into);
                    }
                    _ => collect_references(member, depth + 1, into),
                }
            }
        }
        Value::Array(items) => {
            for item in items {
                collect_references(item, depth + 1, into);
            }
        }
        _ => {}
    }
}

/// The variable names an `Fn::Sub` source string names, each `${…}` without its
/// attribute path, and the WHOLE `${…}` where it read an attribute path off the name (the
/// `${Name.Attribute}` form, which is an `Fn::GetAtt` by another spelling).
pub fn substitution_names(source: &str) -> Vec<(String, Option<String>)> {
    let mut names = Vec::new();
    let mut rest = source;
    while let Some(start) = rest.find("${") {
        let after = &rest[start + 2..];
        let Some(end) = after.find('}') else { break };
        let variable = after[..end].trim();
        if !variable.is_empty() && !variable.starts_with('!') {
            let name = variable.split('.').next().unwrap_or(variable);
            let path = (name.len() < variable.len()).then(|| variable.to_string());
            names.push((name.to_string(), path));
        }
        rest = &after[end + 1..];
    }
    names
}

/// How many rounds [`resolve_dependent_identities`] recomputes a dependent identity in.
///
/// Chosen as one past the longest chain of identifier dependencies the catalogued types
/// form — an ELBv2 listener rule's ARN is built from its listener's, which is built from
/// its load balancer's — so the last round is the one that finds nothing left to change.
const MAX_IDENTITY_ROUNDS: usize = 4;

/// Whether a resource type's published identifier is a function of ANOTHER resource of the
/// template, so it can only be computed once that resource's identity is known.
///
/// Seven of the catalogued types are: an Amazon ECS service's ARN carries the CLUSTER that
/// hosts it (`service/<cluster>/<name>`), an ELBv2 listener's and listener rule's ARNs nest
/// inside the ARN of the load balancer and the listener their declaration names, an Amazon
/// S3 Vectors index's nests inside the ARN of the vector bucket its declaration names, a
/// CodeArtifact repository's nests inside the DOMAIN its declaration names
/// (`repository/<domain>/<name>`), and an AWS Lambda published version's and alias's are the
/// ARN of the FUNCTION their declaration names with a qualifier. Every other type's return
/// values are functions of its own name and its own declared properties, so one pass
/// computes them.
fn identity_depends_on_another_resource(resource_type: &str) -> bool {
    matches!(
        resource_type,
        ECS_SERVICE
            | ELB_LISTENER
            | ELB_LISTENER_RULE
            | S3VECTORS_INDEX
            | CODEARTIFACT_REPOSITORY
            | LAMBDA_VERSION
            | LAMBDA_ALIAS
    )
}

/// Recompute the identity of each resource whose identifier is built from another
/// resource's, until a round changes nothing.
///
/// CloudFormation creates a stack's resources in the order their `Ref`s and `Fn::GetAtt`s
/// imply, so a resource whose identifier embeds another's is created after it and embeds
/// the real one. [`provision`]'s first pass has no resource to resolve against — the
/// identities are what it is producing — so a dependent resource is computed again here,
/// against the identities already known, and that is repeated until it settles: a listener
/// resolves once its load balancer's ARN is known, and the rule inside it the round after.
/// Only the [`identity_depends_on_another_resource`] types are recomputed, so the work is
/// bounded by how many of THOSE a template declares rather than by its resource count, and
/// a dependency the rounds do not settle (a cycle, which no deployable template has) keeps
/// the identity the last round computed.
fn resolve_dependent_identities(
    names: &[(&DeclaredResource, String)],
    identities: &mut BTreeMap<String, StoredResource>,
    scope: &Scope<'_>,
    budget: &mut Budget,
) {
    let dependent: Vec<&(&DeclaredResource, String)> = names
        .iter()
        .filter(|(declared, _)| identity_depends_on_another_resource(&declared.resource_type))
        .collect();
    if dependent.is_empty() {
        return;
    }
    for _round in 0..MAX_IDENTITY_ROUNDS {
        let mut recomputed: Vec<(String, String, BTreeMap<String, String>)> =
            Vec::with_capacity(dependent.len());
        {
            let known = Scope {
                resources: &*identities,
                ..*scope
            };
            for (declared, name) in &dependent {
                let (physical_resource_id, attributes) =
                    return_values(declared, name, &known, budget);
                recomputed.push((declared.logical_id.clone(), physical_resource_id, attributes));
            }
        }
        let mut changed = false;
        for (logical_id, physical_resource_id, attributes) in recomputed {
            let Some(resource) = identities.get_mut(&logical_id) else {
                continue;
            };
            if resource.physical_resource_id != physical_resource_id
                || resource.attributes != attributes
            {
                resource.physical_resource_id = physical_resource_id;
                resource.attributes = attributes;
                changed = true;
            }
        }
        if !changed {
            return;
        }
    }
}

/// The resource type of a wait condition — the one type that exists to wait for signals:
/// "CloudFormation doesn't proceed with a stack creation or update until resources receive
/// the required number of signals" (`SignalResource`).
pub const WAIT_CONDITION: &str = "AWS::CloudFormation::WaitCondition";

/// Whether the declaration of a resource makes a stack operation WAIT for signals on it.
///
/// A signal is sent "in conjunction with a creation policy or update policy"
/// (`SignalResource`), and those attributes are what declare the wait — each for the
/// operation it governs, which is what `updating` says: a `CreationPolicy`'s
/// `ResourceSignal` (the signals a CREATE must receive before the resource is complete), an
/// `UpdatePolicy` whose policy asks the UPDATE to wait on them (`WaitOnResourceSignals`),
/// and a [`WAIT_CONDITION`], which waits for them whichever operation deploys it. A
/// resource whose declaration carries none of those is signalled with nothing waiting on
/// the signal.
pub fn waits_for_resource_signals(declared: &DeclaredResource, updating: bool) -> bool {
    if declared.resource_type == WAIT_CONDITION {
        return true;
    }
    if !updating
        && declared
            .declaration
            .get("CreationPolicy")
            .and_then(|policy| policy.get("ResourceSignal"))
            .is_some()
    {
        return true;
    }
    // `UpdatePolicy` keys its policies by name (`AutoScalingRollingUpdate`,
    // `AutoScalingReplacingUpdate`); the wait is whichever of them asks for it.
    updating
        && declared
            .declaration
            .get("UpdatePolicy")
            .and_then(Value::as_object)
            .is_some_and(|policies| {
                policies
                    .values()
                    .any(|policy| policy.get("WaitOnResourceSignals").is_some_and(flag))
            })
}

/// The content of the `Metadata` resource attribute a declaration carries, as JSON.
///
/// `Metadata` is one of the resource attributes CloudFormation defines on a template's
/// `Resources` entry (the model's `ResourceAttribute` enum lists it beside
/// `CreationPolicy`, `DeletionPolicy`, `Properties`, `Tags`, `UpdatePolicy` and
/// `UpdateReplacePolicy`), and `StackResourceDetail$Metadata` reports its content
/// verbatim. Rendered as JSON whatever the body was spelled in, because the member is a
/// string on the wire and `Metadata` is a JSON object in a template.
///
/// `None` for a declaration with no `Metadata`, and for one whose `Metadata` is `null` —
/// never an invented value.
fn declared_metadata(declared: &DeclaredResource) -> Option<String> {
    let metadata = declared.declaration.get("Metadata")?;
    if metadata.is_null() {
        return None;
    }
    // `Value`'s `Display` is its compact JSON rendering and cannot fail, so no `Metadata`
    // a template declares can be lost to a serialisation error here.
    Some(metadata.to_string())
}

/// The `DeletionPolicy` resource attribute a declaration carries, as written.
///
/// `DeletionPolicy` is the attribute that says "what to do with the resource when its stack
/// is deleted" — one of the resource attributes CloudFormation defines on a template's
/// `Resources` entry, beside the `Metadata` [`declared_metadata`] reads and the
/// `UpdateReplacePolicy` a replacement's own action is read off
/// ([`super::change_plan`]). It is kept on the record so the walk that DELETES a stack's
/// resources ([`super::provisioning::delete`]) reads the attribute of the template that
/// deployed the resource.
///
/// `None` for a declaration that states none — the documented default
/// ([`super::deletion_policy::DELETE`]) — and for one whose value is not a string, which
/// names no policy.
fn declared_deletion_policy(declared: &DeclaredResource) -> Option<String> {
    declared
        .declaration
        .get("DeletionPolicy")
        .and_then(Value::as_str)
        .map(str::to_string)
}

/// Whether a declared resource is one this deployment creates.
///
/// Unconditional unless the declaration names a `Condition`, and then exactly when that
/// condition evaluates TRUE. A condition this module cannot evaluate is not an answer of
/// `false`: the resource is created, because dropping a resource the template declares
/// would be a worse answer than materialising one a false condition would have excluded.
fn is_created(
    declared: &DeclaredResource,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> bool {
    declared.condition.as_deref().is_none_or(|condition| {
        condition_holds(condition, scope, 0, budget).unwrap_or(true)
    })
}

/// Fill in the attributes a provisioned resource answers `Fn::GetAtt` with, beyond the
/// ones its type's published return values already gave it:
///
/// 1. the conventional `Arn`/`Id`/`Name` names every type answers
///    ([`conventional_attributes`]);
/// 2. each property the template DECLARED for the resource, under the property's own
///    name — CloudFormation publishes a great many attributes that are exactly the
///    property the template set (`AWS::EC2::Subnet`'s `AvailabilityZone`, `CidrBlock`
///    and `VpcId` among them), and the declared value is the one the resource has.
///
/// Neither ever overwrites an attribute already present: the type's published return
/// value wins over the convention, and the convention over the raw property.
fn attach_attributes(
    resource: &mut StoredResource,
    declared: &DeclaredResource,
    name: &str,
    scope: &Scope<'_>,
    budget: &mut Budget,
) {
    for (attribute, value) in
        conventional_attributes(&declared.resource_type, name, resource, scope.stack)
    {
        if resource.attributes.len() >= MAX_ATTRIBUTES_PER_RESOURCE {
            return;
        }
        resource.attributes.entry(attribute).or_insert(value);
    }
    let Some(properties) = declared.properties.as_object() else {
        return;
    };
    for (property, declaration) in properties {
        if resource.attributes.len() >= MAX_ATTRIBUTES_PER_RESOURCE {
            return;
        }
        if resource.attributes.contains_key(property) {
            continue;
        }
        // Only a property that resolves to ONE value is an attribute: a list- or
        // object-valued property is not something `Fn::GetAtt` reports as a string.
        if let Some(value) = text(declaration, scope, 0, budget) {
            resource.attributes.insert(property.clone(), value);
        }
    }
}

/// The attributes CloudFormation's UNIFORM return-value naming gives a resource of ANY
/// type.
///
/// Every resource type publishes its own "Return values" section, and [`return_values`]
/// carries the ones whose values do not follow from the resource's identity. Across those
/// sections the NAMES are uniform, though: a type's `Arn`/`<Type>Arn` attribute is the
/// resource's ARN, its `Id`/`<Type>Id` the physical id `Ref` returns, and its
/// `Name`/`<Type>Name` the resource's name. A resource of a type this service carries no
/// catalogue entry for therefore still answers those, so an output over it resolves
/// instead of being reported with a key and no value.
///
/// The ARN is the resource's OWN: the one its type's published return values already gave
/// it, where they gave it one (an `…Arn` attribute, or a physical id that is itself an
/// ARN), and otherwise the type's SERVICE segment as the ARN namespace
/// (`AWS::EC2::Subnet` -> `ec2`) with its TYPE segment, hyphenated, as the resource path
/// (`arn:aws:ec2:<region>:<account>:subnet/<name>`) — the form the majority of AWS ARNs
/// take.
///
/// That minted form is the same kind of approximation as the physical id it is built
/// from, and it diverges the same way: a type whose published ARN carries a
/// multi-segment resource path (`AWS::AutoScaling::AutoScalingGroup` is
/// `autoScalingGroup:<id>:autoScalingGroupName/<name>`) gets an ARN of this emulator's
/// uniform shape rather than of that type's. Narrowing it is the business of the open
/// fidelity issue on the identifiers a stack's outputs carry
/// (`20261022T154846Z-…-stack-outputs-return-synthesized-logical-names-instead`): the
/// answer is a catalogue entry in [`return_values`], which wins here, for each type whose
/// real identifiers this emulator can produce — not an absent value, which is what a
/// client cannot use at all.
///
/// The names are offered for every type, so `Fn::GetAtt` answers one real CloudFormation
/// would reject at validation time for a type that publishes no such attribute. This
/// service does not validate a template against the resource-type registry at all (it has
/// none), so refusing those names would not make it stricter — it would only make the
/// attributes a type DOES publish depend on whether the catalogue happens to carry it.
fn conventional_attributes(
    resource_type: &str,
    name: &str,
    resource: &StoredResource,
    stack: StackScope<'_>,
) -> Vec<(String, String)> {
    // A resource type is `AWS::<Service>::<Type>`: the service segment is the ARN
    // namespace and the last segment is the resource's own type. Anything else (a
    // malformed `Type`) carries no convention at all.
    let Some((service, segment)) = type_segments(resource_type) else {
        return Vec::new();
    };
    let arn = known_arn(resource).unwrap_or_else(|| {
        ::emulator_server::emulator_arn(
            &service.to_lowercase(),
            stack.region,
            &format!("{}/{name}", hyphenated(segment)),
        )
    });
    let mut conventional = arn_attributes(resource_type, &arn);
    conventional.extend(identity_attributes(
        resource_type,
        &resource.physical_resource_id,
    ));
    conventional.push(("Name".to_string(), name.to_string()));
    conventional.push((format!("{segment}Name"), name.to_string()));
    conventional
}

/// The attributes the uniform convention reads a resource's ARN off: its `Arn`/`<Type>Arn`
/// names, which are "the resource's ARN" ([`conventional_attributes`]).
///
/// Public for the same reason [`identity_attributes`] is: an ARN is not always
/// CloudFormation's own to compute. Where the OWNING service MINTS it — WAF answers
/// `CreateWebACL` with the `ARN` it built from the scope, the name and the id it assigned —
/// the ARN is the one that service reported, which only
/// [`super::provisioning`](crate::state::provisioning) learns. It re-points these
/// attributes through this one function, so the ARN a resource reports under either name is
/// the same string and neither can be left holding a value the owning service does not know.
pub fn arn_attributes(resource_type: &str, arn: &str) -> Vec<(String, String)> {
    let Some((_, segment)) = type_segments(resource_type) else {
        return Vec::new();
    };
    vec![
        ("Arn".to_string(), arn.to_string()),
        (format!("{segment}Arn"), arn.to_string()),
    ]
}

/// Give a resource the physical id `identity` and bring the attributes read off that id
/// into step with it ([`identity_attributes`]).
///
/// The one place a resource TAKES OVER an identifier that is not the one the naming rules
/// computed, so the `Id`/`<Type>Id` attributes a resource reports and the id its `Ref`
/// resolves to cannot drift apart. Both ways that happens go through it: an identifier the
/// OWNING service minted, which only
/// [`super::provisioning`](crate::state::provisioning) learns, and the composite primary
/// identifier a BINDING's own resolved properties make ([`composite_identity`]).
pub fn adopt_identity(resource: &mut StoredResource, identity: String) {
    for (attribute, value) in identity_attributes(&resource.resource_type, &identity) {
        // An attribute the convention already gave the resource is re-pointed; a new one
        // is added only while the resource is under its attribute bound, which is what
        // caps the map.
        if resource.attributes.contains_key(&attribute)
            || resource.attributes.len() < MAX_ATTRIBUTES_PER_RESOURCE
        {
            resource.attributes.insert(attribute, value);
        }
    }
    resource.physical_resource_id = identity;
}

/// The attributes the uniform convention reads off a resource's PHYSICAL ID: its
/// `Id`/`<Type>Id` names, which are "the physical id `Ref` returns"
/// ([`conventional_attributes`]).
///
/// Public because the physical id is not always CloudFormation's own to compute: where the
/// OWNING service mints a resource's identity — AWS WAF Classic answers `CreateIPSet` with
/// the `IPSetId` that type's `Ref` returns — the id is the one that service answered, which
/// only [`super::provisioning`](crate::state::provisioning) learns. [`adopt_identity`]
/// re-points these attributes through this one function.
pub fn identity_attributes(
    resource_type: &str,
    physical_resource_id: &str,
) -> Vec<(String, String)> {
    let Some((_, segment)) = type_segments(resource_type) else {
        return Vec::new();
    };
    vec![
        ("Id".to_string(), physical_resource_id.to_string()),
        (format!("{segment}Id"), physical_resource_id.to_string()),
    ]
}

/// The `(service, type)` segments of a resource type spelled `AWS::<Service>::<Type>`, or
/// `None` for anything else — a malformed `Type`, which carries no convention at all.
fn type_segments(resource_type: &str) -> Option<(&str, &str)> {
    let segments: Vec<&str> = resource_type.split("::").collect();
    let (Some(service), Some(segment)) = (segments.get(1), segments.last()) else {
        return None;
    };
    if segments.len() < 3 || service.is_empty() || segment.is_empty() {
        return None;
    }
    Some((service, segment))
}

/// The ARN a resource ALREADY carries: its physical resource id where that is an ARN
/// ("Ref returns the Amazon Resource Name (ARN)", for the types documented that way), or
/// the value of the one `…Arn` attribute its type's published return values gave it.
///
/// This is what keeps the conventional `Arn` of a type [`return_values`] carries from
/// disagreeing with the ARN that type really publishes (`AWS::SNS::Topic`'s `TopicArn`,
/// `AWS::SQS::Queue`'s `Arn`).
fn known_arn(resource: &StoredResource) -> Option<String> {
    if resource.physical_resource_id.starts_with("arn:") {
        return Some(resource.physical_resource_id.clone());
    }
    let mut published = resource
        .attributes
        .iter()
        .filter(|(attribute, value)| attribute.ends_with("Arn") && value.starts_with("arn:"));
    let (_, only) = published.next()?;
    // Two published ARNs and no rule for which is "the" ARN: the convention mints the
    // resource's own rather than choosing one of them.
    if published.next().is_some() {
        return None;
    }
    Some(only.clone())
}

/// A resource type's segment as an ARN resource path spells it: `AutoScalingGroup` ->
/// `auto-scaling-group`, `LaunchTemplate` -> `launch-template`, `VPC` -> `vpc`.
///
/// A hyphen goes before an upper-case character that starts a word — one following a
/// lower-case character or digit, or one followed by a lower-case character — so an
/// initialism stays one word.
fn hyphenated(segment: &str) -> String {
    let characters: Vec<char> = segment.chars().collect();
    let mut out = String::with_capacity(segment.len() + 4);
    for (index, character) in characters.iter().enumerate() {
        let starts_word = index > 0
            && character.is_ascii_uppercase()
            && (characters[index - 1].is_ascii_lowercase()
                || characters[index - 1].is_ascii_digit()
                || characters
                    .get(index + 1)
                    .is_some_and(char::is_ascii_lowercase));
        if starts_word {
            out.push('-');
        }
        out.push(character.to_ascii_lowercase());
    }
    out
}

/// The name a declared resource carries.
///
/// CloudFormation's own convention is that a resource type's name property is the type's
/// last segment plus `Name` (`TopicName` on `AWS::SNS::Topic`, `QueueName` on
/// `AWS::SQS::Queue`, `BucketName` on `AWS::S3::Bucket`), with `Name` itself used by the
/// newer types and `Identifier` by the Amazon RDS ones (`DBClusterIdentifier` on
/// `AWS::RDS::DBCluster`, `DBInstanceIdentifier` on `AWS::RDS::DBInstance`, each of which
/// is also what that type's `Ref` returns); where none is declared, "CloudFormation
/// generates a unique physical ID and uses that ID for the resource name", in the form
/// `<stack name>-<logical id>-<unique id>`.
///
/// The name is the property's EVALUATED value: `naming` is the scope the deployment has
/// reached, which carries the resources created before this one ([`provision`]), so a name
/// stated as a `Ref` or an `Fn::Sub` over one of them is the identifier that resource
/// really carries. A declaration whose name property resolves to nothing at all is one that
/// named the resource nowhere, and such a resource takes the generated name.
///
/// A type whose owning API spells its name member with the resource's FULL noun rather
/// than the type's last segment is read through [`declared_name_property`] first, so the
/// name such a template asked for is still the name the resource carries.
///
/// A type whose owning API takes that name inside an INPUT STRUCTURE is read through
/// [`declared_name_path`] before either convention, for the same reason: the name is a
/// MEMBER of one of the type's properties rather than a property, and it is still the name
/// the resource is created under.
fn physical_name(
    declared: &DeclaredResource,
    naming: &Scope<'_>,
    budget: &mut Budget,
) -> String {
    let nested_name = declared_name_path(&declared.resource_type)
        .and_then(|path| property_at(&declared.properties, path))
        .and_then(|value| text(value, naming, 0, budget));
    let declared_name = nested_name.or_else(|| {
        name_property_candidates(&declared.resource_type)
            .into_iter()
            .find_map(|property| {
                declared
                    .properties
                    .get(&property)
                    .and_then(|value| text(value, naming, 0, budget))
            })
    });
    match declared_name {
        // The name the TEMPLATE supplied, exactly as it supplied it: it is the caller's,
        // and a name the owning service refuses is reported as the failure that service
        // answers rather than silently rewritten into a different resource than the one
        // the template asked for.
        Some(name) => name,
        // A GENERATED name is held to the naming rules of the service that owns the type,
        // because a name that service would refuse could not be provisioned at all — the
        // suffix the declared VARIANT's name has to carry among them.
        None => generated_name(
            &declared.resource_type,
            &declared.logical_id,
            naming.stack,
            required_name_suffix(declared, naming, budget),
        ),
    }
}

/// The suffix Amazon SNS and Amazon SQS require on the name of a FIFO resource: "For a
/// FIFO (first-in-first-out) topic, the name must end with the .fifo suffix"
/// (`CreateTopicInput$Name`) and "A FIFO queue name must end with the .fifo suffix"
/// (`CreateQueueInput$QueueName`).
const FIFO_NAME_SUFFIX: &str = ".fifo";

/// The property a declaration asks for the FIFO variant of its type with, for the types
/// whose owning service requires [`FIFO_NAME_SUFFIX`] on such a resource's name:
/// `AWS::SNS::Topic`'s `FifoTopic` and `AWS::SQS::Queue`'s `FifoQueue`, each named
/// identically to the topic/queue attribute that asks for it.
fn fifo_variant_property(resource_type: &str) -> Option<&'static str> {
    match resource_type {
        SNS_TOPIC => Some("FifoTopic"),
        SQS_QUEUE => Some("FifoQueue"),
        _ => None,
    }
}

/// The suffix the owning service REQUIRES on the name of the variant this declaration asks
/// for, which a name CloudFormation generates therefore ends with — `""` where the type has
/// no such variant, or where the declaration does not ask for it.
///
/// A generated name without it is a name the owning service refuses, so the resource could
/// not be provisioned at all; a name the template DECLARED is the caller's and is passed
/// through as written ([`physical_name`]), refusal included. The flag is the EVALUATED
/// property, so a variant a parameter or a condition decides is still the variant the name
/// is built for.
fn required_name_suffix(
    declared: &DeclaredResource,
    naming: &Scope<'_>,
    budget: &mut Budget,
) -> &'static str {
    let asked_for_fifo = fifo_variant_property(&declared.resource_type)
        .and_then(|property| declared.properties.get(property))
        .and_then(|value| text(value, naming, 0, budget))
        .is_some_and(|value| value.eq_ignore_ascii_case("true"));
    if asked_for_fifo {
        FIFO_NAME_SUFFIX
    } else {
        ""
    }
}

/// The properties a resource of `resource_type` can take its physical NAME from, most
/// specific first: the type's OWN name property where it is neither convention, then the
/// two conventions ([`physical_name`]), then the Amazon RDS `Identifier` spelling.
///
/// The ONE list of those candidates: [`physical_name`] reads the first of them the
/// declaration carries, and [`name_property`] answers WHICH property that was — which is
/// what a change set needs, since a resource's physical name is fixed when it is created
/// (every name property's "Update requires" in the resource and property types reference is
/// `Replacement`).
///
/// A CUSTOM RESOURCE has NONE: its physical id is "a required custom resource
/// provider-defined physical ID that is unique for that provider", so no property of such a
/// declaration names the resource — a property spelled `Name` is one more value passed
/// through to the provider, exactly like every other ("This field contains the contents of
/// the custom resource's `Properties` section of the template"). Reading one as the
/// resource's name would make a template that changes it ask for a DIFFERENT resource, when
/// what AWS does with such a change is send the provider an `Update` and let the provider's
/// own answer decide whether that update replaced anything.
pub fn name_property_candidates(resource_type: &str) -> Vec<String> {
    if crate::state::resource_types::is_custom_resource(resource_type) {
        return Vec::new();
    }
    let segment = resource_type.rsplit("::").next().unwrap_or_default();
    declared_name_property(resource_type)
        .map(ToString::to_string)
        .into_iter()
        .chain([
            format!("{segment}Name"),
            "Name".to_string(),
            format!("{segment}Identifier"),
        ])
        .collect()
}

/// The property a declaration of `resource_type` takes its physical name from: the first
/// of [`name_property_candidates`] the `Properties` object carries, or `None` for a
/// declaration that names the resource nowhere (CloudFormation generates its name).
pub fn name_property(resource_type: &str, properties: &Value) -> Option<String> {
    name_property_candidates(resource_type)
        .into_iter()
        .find(|property| properties.get(property).is_some())
}

/// The properties of this type a resource cannot take a NEW VALUE for while it goes on
/// existing — the ones the resource and property types reference publishes with "Update
/// requires: `Replacement`" — BESIDE the name, which is in that position for every type
/// whose owning service cannot rename the resource ([`name_is_create_only`]) and is read
/// from the declaration rather than catalogued.
///
/// A resource is created with such a property and nothing changes it afterwards: an IAM
/// access key belongs to the user `CreateAccessKey` minted it for and carries the `Serial`
/// the template rotates it with ("incrementing this value notifies CloudFormation that you
/// want to rotate your access key… CloudFormation will replace the AccessKey resource with
/// a new access key"), so a declaration that states either differently asks for a DIFFERENT
/// resource — which CloudFormation replaces rather than updates.
///
/// A type with no entry has none beyond its name, so the name alone decides whether a
/// re-deploy means the same resource ([`super::declared_name`]).
///
/// The parts of a COMPOSITE primary identifier are in that position by construction
/// ([`composite_identity_properties`]): the resource IS the binding those values name, so
/// stating one of them differently asks for a different resource.
pub fn replacement_properties(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        IAM_ACCESS_KEY => &["Serial", "UserName"],
        // A spending limit is a cost control ABOUT one device: `UpdateSpendingLimit` takes
        // the limit's amount and its period and no device at all, so a declaration naming
        // another device asks for a different limit.
        BRAKET_SPENDING_LIMIT => &["DeviceArn"],
        // An environment's encryption key and who manages its VPC endpoints are fixed when
        // Amazon MWAA makes it: both are members of `CreateEnvironment` and of no member of
        // `UpdateEnvironment`, so a declaration restating either asks for a different
        // environment.
        MWAA_ENVIRONMENT => &["EndpointManagement", "KmsKey"],
        // An AppConfig environment and configuration profile belong to the application their
        // `ApplicationId` names, and a profile reads its configuration from the source its
        // `LocationUri` and `Type` fix: each is a member of that type's create and of no
        // member of its update (`UpdateConfigurationProfile` takes neither `LocationUri` nor
        // `Type`), so a declaration restating one asks for a different resource. A deployment
        // strategy's `ReplicateTo` is the same case against `UpdateDeploymentStrategy`.
        APPCONFIG_ENVIRONMENT => &["ApplicationId"],
        APPCONFIG_CONFIGURATION_PROFILE => &["ApplicationId", "LocationUri", "Type"],
        APPCONFIG_DEPLOYMENT_STRATEGY => &["ReplicateTo"],
        // A State Manager association may be paired with ONE node by the deprecated
        // `InstanceId`, which `CreateAssociation` takes and `UpdateAssociation` does not: the
        // node an association was created for cannot be changed on it, so a declaration
        // naming another node asks for a different association.
        SSM_ASSOCIATION => &["InstanceId"],
        // An Amazon Cognito user pool's sign-in aliases and its username attributes and their
        // case sensitivity are fixed when the pool is made: each is a member of
        // `CreateUserPool` and of no member of `UpdateUserPool`, so a declaration restating
        // one asks for a different pool. (Its `Schema` is NOT one of them: the schema is
        // append-only on the pool that exists, and `AddCustomAttributes` is the call that adds
        // to it — `crate::state::provisioning::cognito_user_pool`.) An app client's secret
        // generation is the same case against `UpdateUserPoolClient`, and an identity
        // provider's type against `UpdateIdentityProvider`.
        //
        // The POOL each of those three belongs to is not among them, even though no update
        // call moves a resource between pools: a declaration naming another pool is reported
        // as a change that was NOT APPLIED, by the arm that would have made the call
        // (`cognito_user_pool`'s own `create_only_properties`), which is the same reading an
        // identity-pool role attachment moved to another pool gets. A resource server's
        // identifier and an identity provider's name are those resources' identities, which
        // `name_is_create_only` already covers.
        COGNITO_USER_POOL => &["AliasAttributes", "UsernameAttributes", "UsernameConfiguration"],
        COGNITO_USER_POOL_CLIENT => &["GenerateSecret"],
        COGNITO_USER_POOL_IDENTITY_PROVIDER => &["ProviderType"],
        // A rotation schedule IS the rotation configured on the secret its `SecretId` names:
        // `RotateSecret` takes that secret's id and there is no call that moves a rotation to
        // another secret, so a declaration naming another secret asks for a different
        // resource — which is what that property's "Update requires: Replacement" says.
        SECRETSMANAGER_ROTATION_SCHEDULE => &["SecretId"],
        // A resource POLICY is the same case: `PutResourcePolicy` takes the secret's id and
        // there is no call that moves a policy to another secret, so a declaration naming
        // another secret asks for a different resource.
        SECRETSMANAGER_RESOURCE_POLICY => &["SecretId"],
        // A log stream, a metric filter and a subscription filter each exist INSIDE the log
        // group their `LogGroupName` names, and Amazon CloudWatch Logs publishes no call that
        // moves one to another group — every call on one takes the group beside the child's
        // own name — so a declaration naming another group asks for a different resource,
        // which is what that property's "Update requires: Replacement" says.
        crate::state::provisioning::logs_resources::LOGS_LOG_STREAM
        | crate::state::provisioning::logs_resources::LOGS_METRIC_FILTER
        | crate::state::provisioning::logs_resources::LOGS_SUBSCRIPTION_FILTER => {
            &["LogGroupName"]
        }
        // An Amazon ECS task-definition REVISION is immutable: that service publishes no call
        // that edits one — `RegisterTaskDefinition` is the only way to state a configuration,
        // and it makes a NEW revision — so every property of the type carries "Update
        // requires: Replacement", and a declaration that states any of them differently asks
        // for a new revision rather than a change to the one the stack has.
        ECS_TASK_DEFINITION => &[
            "ContainerDefinitions",
            "Cpu",
            "EnableFaultInjection",
            "EphemeralStorage",
            "ExecutionRoleArn",
            "Family",
            "InferenceAccelerators",
            "IpcMode",
            "Memory",
            "NetworkMode",
            "PidMode",
            "PlacementConstraints",
            "ProxyConfiguration",
            "RequiresCompatibilities",
            "RuntimePlatform",
            "TaskRoleArn",
            "Volumes",
        ],
        _ => composite_identity_properties(resource_type),
    }
}

/// The properties of this type that carry a value the resource cannot take anew AMONG
/// values it CAN: a change to such a property recreates the resource only when it is the
/// create-only value inside it that the two revisions state differently, which is the
/// `Conditionally` of `ResourceTargetDefinition$RequiresRecreation` ("to determine the
/// conditions for a `Conditionally` recreation, see the update behavior for that
/// property").
///
/// An Amazon Bedrock knowledge base is the case: `UpdateKnowledgeBase` takes a knowledge
/// base's whole `knowledgeBaseConfiguration` by id, so most of what that property states IS
/// updatable, while the embedding model inside it is not — the knowledge base's vectors were
/// produced by that model, so a knowledge base reading another one is a new knowledge base.
/// A change to the property is therefore reported as a recreation whose condition is which
/// value inside it changed, which is what the recorded preview of such a change answers
/// (`Replacement` `Conditional` rather than `True`).
///
/// A type with no entry has no such property: each of its properties is either create-only
/// ([`replacement_properties`]) or updatable.
pub fn conditionally_replacing_properties(resource_type: &str) -> &'static [&'static str] {
    match resource_type {
        BEDROCK_KNOWLEDGE_BASE => &["KnowledgeBaseConfiguration"],
        _ => &[],
    }
}

/// Whether a resource of this type cannot take a new physical NAME while it goes on
/// existing, so that the property it takes its name from ([`name_property`]) is one of its
/// create-only ones.
///
/// It is for a resource its owning service identifies BY that name: there is no call that
/// renames it, so a declaration stating another name asks for a different resource. A
/// service that identifies the resource by an identifier IT minted renames it in place
/// instead, and the service's own model is where that shows — Amazon Bedrock Agent's
/// `UpdateKnowledgeBase` takes a `knowledgeBaseId` together with a new required `name`, as
/// `UpdateAgent` takes an `agentId` and an `agentName`, `UpdateAgentAlias` an
/// `agentAliasId` and an `agentAliasName`, and `UpdateDataSource` a data source's id and a
/// new `name`. Renaming such a resource is a modification of it, not a recreation. An Amazon
/// Bedrock GUARDRAIL is in that position too: `UpdateGuardrail` takes a
/// `guardrailIdentifier` together with a new required `name`, the guardrail being identified
/// by the id Amazon Bedrock minted for it.
fn name_is_create_only(resource_type: &str) -> bool {
    !matches!(
        resource_type,
        BEDROCK_KNOWLEDGE_BASE
            | BEDROCK_AGENT
            | BEDROCK_AGENT_ALIAS
            | BEDROCK_DATA_SOURCE
            | BEDROCK_GUARDRAIL
    )
}

/// Every property of one declaration whose change RECREATES the resource: the name it takes
/// its identity from, where the type's own name is create-only ([`name_is_create_only`]),
/// and the type's other replacement properties ([`replacement_properties`]).
///
/// The one reading of "Update requires: `Replacement`" a change set's `RequiresRecreation`
/// details and a deployment's own replacement decision share, so a preview cannot say a
/// change recreates a resource that the deployment then updates in place.
pub fn replacing_properties(resource_type: &str, properties: &Value) -> Vec<String> {
    name_is_create_only(resource_type)
        .then(|| name_property(resource_type, properties))
        .flatten()
        .into_iter()
        .chain(
            replacement_properties(resource_type)
                .iter()
                .map(ToString::to_string),
        )
        .collect()
}

/// The naming rules of the service that OWNS a resource type: the longest name it accepts
/// for one, and the characters such a name may carry.
///
/// A generated `<stack name>-<logical id>-<unique id>` is built from a stack name and a
/// logical id, and those are LONGER — and admit more characters — than several owning
/// services' own names are: an IAM role's name is at most 64 characters while a stack name
/// may be 128 on its own, and an Amazon S3 bucket name "can consist only of lowercase
/// letters, numbers, dots (.), and hyphens (-)" and "must begin and end with a letter or
/// number" (Amazon S3 bucket naming rules). A name outside the owning service's rules is
/// not one that service would accept, so the resource could not be created under it at all
/// and the stack that declared it would roll back — which is why CloudFormation FITS the
/// name it generates to those rules, and why its generated bucket names come out lowercase
/// and within 63 characters (`mystack-mybucket-kdwwxmddtr2g`).
///
/// `None` for a type whose owning service publishes no bound a generated name could break;
/// such a name is used as it is built.
#[derive(Debug, Clone, Copy)]
struct NameRules {
    /// The longest name the owning service accepts.
    max_characters: usize,
    /// The characters such a name may carry.
    characters: Characters,
}

/// Which characters a name of a resource type may carry — the one thing the naming rules
/// below differ in beyond their length.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Characters {
    /// Every character a generated name is built from: a CloudFormation stack name is
    /// `[a-zA-Z][-a-zA-Z0-9]*` and a logical id is alphanumeric, and these owning services'
    /// name patterns admit all of those (IAM's `roleNameType` is `[\w+=,.@-]+`, Amazon Data
    /// Firehose's `DeliveryStreamName` is `[a-zA-Z0-9_.-]+`, CodePipeline's `PipelineName`
    /// is `[A-Za-z0-9.@\-_]+`), so only the LENGTH has to be fitted — and the shortening
    /// itself leaves no hyphen beside the one that joins the segments (see
    /// [`shortened_to`]), which is what an Elastic Load Balancing name requires.
    AsGenerated,
    /// Lowercase alphanumerics and the hyphen, beginning and ending with an alphanumeric:
    /// an Amazon S3 bucket name (the naming rules above), an Amazon ECR repository name
    /// (`RepositoryName`'s pattern, `^[a-z0-9]+((\.|_|__|-+)[a-z0-9]+)*…$` in Amazon ECR's
    /// and Amazon ECR Public's own models, which admits no upper case and requires an
    /// alphanumeric at each end and between separators) and an Amazon RDS identifier
    /// (lowercase, no trailing hyphen, no two consecutive hyphens).
    LowercaseAlphanumeric,
}

/// The resource types this emulator IMPLEMENTS: the ones [`return_values`] below carries
/// the documented `Ref` and `Fn::GetAtt` return values of, so a stack that declares one
/// gets the physical id and the attributes the type's "Return values" section publishes
/// rather than a name and nothing.
///
/// This is the list of extensions the registry of every account and Region holds as
/// Amazon-Web-Services-published public resource types (see
/// [`crate::state::registry::aws_published`]): the registry reports an extension for a
/// resource type this emulator carries the return values of, and nothing for a name it
/// knows nothing about. Keep it in step with the `match` in [`return_values`] — one entry
/// per arm.
///
/// It is NOT the set [`crate::state::provisioning`] creates in the owning service: the two
/// questions are independent — a type can have published return values this module knows
/// and no owning service mounted here (`AWS::Transfer::Workflow`), or an owning service that
/// creates it and no published return value beyond its name
/// (`AWS::CodePipeline::Pipeline`, whose `Ref` is the pipeline name, which is the default
/// this module already gives every type).
pub const IMPLEMENTED_RESOURCE_TYPES: &[&str] = &[
    ACMPCA_CERTIFICATE,
    ACMPCA_CERTIFICATE_AUTHORITY,
    ACMPCA_CERTIFICATE_AUTHORITY_ACTIVATION,
    APIGATEWAY_API_KEY,
    APIGATEWAY_AUTHORIZER,
    APIGATEWAY_DEPLOYMENT,
    APIGATEWAY_RESOURCE,
    APIGATEWAY_REST_API,
    APIGATEWAY_USAGE_PLAN,
    APIGATEWAY_USAGE_PLAN_KEY,
    APIGATEWAYV2_API,
    APIGATEWAYV2_API_MAPPING,
    APIGATEWAYV2_DOMAIN_NAME,
    BACKUP_VAULT,
    BATCH_COMPUTE_ENVIRONMENT,
    BATCH_JOB_QUEUE,
    BATCH_SERVICE_ENVIRONMENT,
    "AWS::BedrockAgentCore::Browser",
    "AWS::BedrockAgentCore::CodeInterpreter",
    "AWS::BedrockAgentCore::Gateway",
    "AWS::BedrockAgentCore::Memory",
    "AWS::BedrockAgentCore::Runtime",
    "AWS::BedrockAgentCore::WorkloadIdentity",
    CLOUDFRONT_CACHE_POLICY,
    CLOUDFRONT_DISTRIBUTION,
    CLOUDFRONT_ORIGIN_ACCESS_CONTROL,
    CLOUDFRONT_ORIGIN_ACCESS_IDENTITY,
    CLOUDFRONT_ORIGIN_REQUEST_POLICY,
    CLOUDFRONT_RESPONSE_HEADERS_POLICY,
    "AWS::CloudWatch::Alarm",
    CLOUDWATCH_METRIC_STREAM,
    CODEARTIFACT_DOMAIN,
    CODEARTIFACT_REPOSITORY,
    CODEBUILD_PROJECT,
    CODEBUILD_REPORT_GROUP,
    COGNITO_USER_POOL,
    "AWS::DynamoDB::Table",
    EC2_DHCP_OPTIONS,
    EC2_INTERNET_GATEWAY,
    EC2_NAT_GATEWAY,
    EC2_NETWORK_ACL,
    EC2_NETWORK_INTERFACE,
    EC2_ROUTE_TABLE,
    EC2_SECURITY_GROUP,
    EC2_SUBNET,
    EC2_VPC,
    EC2_VPC_ENDPOINT,
    EC2_VPN_GATEWAY,
    ECR_PUBLIC_REPOSITORY,
    ECR_REPOSITORY,
    ECS_CLUSTER,
    ECS_SERVICE,
    ECS_TASK_DEFINITION,
    EKS_CLUSTER,
    EKS_POD_IDENTITY_ASSOCIATION,
    ELB_LISTENER,
    ELB_LISTENER_RULE,
    ELB_LOAD_BALANCER,
    ELB_TARGET_GROUP,
    EVENTS_RULE,
    FIS_EXPERIMENT_TEMPLATE,
    GUARDDUTY_DETECTOR,
    GUARDDUTY_IP_SET,
    IAM_ACCESS_KEY,
    "AWS::IAM::Role",
    KMS_KEY,
    FIREHOSE_DELIVERY_STREAM,
    KINESIS_STREAM,
    "AWS::Lambda::Function",
    LAMBDA_ALIAS,
    LAMBDA_VERSION,
    "AWS::Logs::LogGroup",
    MQ_BROKER,
    MSK_CLUSTER,
    MSK_SERVERLESS_CLUSTER,
    MWAA_ENVIRONMENT,
    "AWS::RDS::DBCluster",
    "AWS::RDS::DBInstance",
    "AWS::RUM::AppMonitor",
    RESOURCE_GROUPS_GROUP,
    ROUTE53_HEALTH_CHECK,
    ROUTE53_HOSTED_ZONE,
    ROUTE53_CIDR_COLLECTION,
    ROUTE53_DELEGATION_SET,
    ROUTE53_TRAFFIC_POLICY,
    S3_BUCKET,
    S3TABLES_TABLE_BUCKET,
    S3VECTORS_INDEX,
    S3VECTORS_VECTOR_BUCKET,
    // The Amazon SageMaker AI types `provisioner::sagemaker_resources` creates: each
    // publishes the one attribute carrying the resource's own name.
    crate::state::provisioner::sagemaker_resources::SAGEMAKER_MODEL,
    crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE,
    crate::state::provisioner::sagemaker_resources::SAGEMAKER_NOTEBOOK_INSTANCE_LIFECYCLE_CONFIG,
    SCHEDULER_SCHEDULE,
    SCHEDULER_SCHEDULE_GROUP,
    SNS_TOPIC,
    SQS_QUEUE,
    "AWS::SecretsManager::Secret",
    SSM_ASSOCIATION,
    SSM_PARAMETER,
    STEP_FUNCTIONS_ACTIVITY,
    TRANSFER_SERVER,
    "AWS::Transfer::Workflow",
];

/// Evaluate one intrinsic function.
///
/// The functions this module does not implement yield no value, so an output that uses
/// one is reported without an `OutputValue`:
/// `Fn::Transform` (a macro this emulator runs nothing for). The CONDITION functions
/// (`Fn::Equals`, `Fn::And`, `Fn::Or`, `Fn::Not`, `Condition`) are not values either:
/// they are evaluated where CloudFormation allows them — the `Conditions` section and
/// `Fn::If` — by [`boolean`].
///
/// `Fn::ToJsonString` and `Fn::Length` are the language-extensions DIRECTIVES, which
/// [`process`] resolves in the processed stage: a deployment evaluates its outputs over
/// that stage, so a template declaring `AWS::LanguageExtensions` has them resolved before
/// this evaluator ever sees the document. In the stage that declares no transform they are
/// not functions CloudFormation would resolve either, so they yield no value here.
fn intrinsic(
    name: &str,
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    match name {
        // "The intrinsic function Ref returns the value of the specified parameter or
        // resource." The argument NAMES that parameter or resource, and the name may be
        // spelled by an expression rather than written out — the same reading
        // [`attribute`] gives `Fn::GetAtt`'s first argument — so it is evaluated to the
        // name first and then dereferenced. A plain string evaluates to itself, so the
        // written-out form is unchanged.
        "Ref" => reference(&text(argument, scope, depth + 1, budget)?, scope),
        // "The Fn::GetAtt intrinsic function returns the value of an attribute from a
        // resource in the template": [logicalNameOfResource, attributeName], or the
        // short form's dotted string.
        "Fn::GetAtt" => attribute(argument, scope, depth, budget),
        // "Fn::Sub substitutes variables in an input string with values that you
        // specify": a string, or [string, {variables}].
        "Fn::Sub" => substitute(argument, scope, depth, budget).map(Resolved::Text),
        // "The intrinsic function Fn::ImportValue returns the value of an output exported
        // by another stack": the argument is the EXPORT NAME, and the value is the one that
        // export carries in the account and Region this deployment runs in. An export name
        // nothing publishes there resolves to no value, the same as any other unresolvable
        // expression.
        FN_IMPORT_VALUE => {
            let name = text(argument, scope, depth + 1, budget)?;
            scope.imports.get(&name).cloned().map(Resolved::Text)
        }
        // `Fn::GetStackOutput` reads one OUTPUT of another stack of the account and Region
        // the deployment runs in, by the stack and the output key rather than by an export
        // name — so the output it reads needs no `Export` declaration. The value is the one
        // that output carries where the template is deployed, which the reading stack then
        // keeps ([`super::stack_outputs`]); a reference naming no output of any stack of
        // that Region resolves to no value, the same as any other unresolvable expression.
        super::stack_outputs::FN_GET_STACK_OUTPUT => {
            let reference = stack_output_reference(argument, scope, depth, budget)?;
            scope.stack_outputs.get(&reference).cloned().map(Resolved::Text)
        }
        // "Fn::Join appends a set of values into a single value, separated by the
        // specified delimiter": [delimiter, [values]].
        "Fn::Join" => {
            let [delimiter, values] = pair(argument)?;
            let delimiter = text(delimiter, scope, depth + 1, budget)?;
            let values = list(values, scope, depth + 1, budget)?;
            // The joined length is known before it is built, so a join whose result is
            // past the bound is never materialised even transiently. It is charged for
            // BEFORE it is allocated, rather than after.
            let joined = values
                .iter()
                .map(String::len)
                .sum::<usize>()
                .saturating_add(delimiter.len() * values.len().saturating_sub(1));
            if joined > MAX_RESOLVED_TEXT_BYTES {
                return None;
            }
            budget.spend_text(joined)?;
            Some(Resolved::Text(values.join(&delimiter)))
        }
        // "Fn::Select returns a single object from a list of objects by index":
        // [index, [objects]].
        "Fn::Select" => {
            let [index, values] = pair(argument)?;
            let index: usize = text(index, scope, depth + 1, budget)?.parse().ok()?;
            let values = list(values, scope, depth + 1, budget)?;
            values.get(index).cloned().map(Resolved::Text)
        }
        // "Fn::Split splits a string into a list of string values": [delimiter, string].
        "Fn::Split" => {
            let [delimiter, source] = pair(argument)?;
            let delimiter = text(delimiter, scope, depth + 1, budget)?;
            let source = text(source, scope, depth + 1, budget)?;
            if delimiter.is_empty() {
                return None;
            }
            let parts: Vec<String> = source
                .split(delimiter.as_str())
                .map(ToString::to_string)
                .collect();
            (parts.len() <= MAX_RESOLVED_LIST_LEN).then_some(Resolved::List(parts))
        }
        // "The intrinsic function Fn::FindInMap returns the value corresponding to keys
        // in a two-level map declared in the Mappings section":
        // [MapName, TopLevelKey, SecondLevelKey], plus the optional fourth argument that
        // carries "the value to be returned if the key is not found": `{"DefaultValue":
        // <value>}`. A lookup that misses resolves to that default where one is given, and
        // to NO value otherwise — which is what the submitted template is refused for.
        FN_FIND_IN_MAP => {
            let keys = argument.as_array()?;
            if keys.len() < 3 {
                return None;
            }
            let map = text(&keys[0], scope, depth + 1, budget)?;
            let top = text(&keys[1], scope, depth + 1, budget)?;
            let second = text(&keys[2], scope, depth + 1, budget)?;
            let found = scope
                .mappings
                .get(map)
                .and_then(|entries| entries.get(top))
                .and_then(|entries| entries.get(second))
                .and_then(scalar_text);
            match found {
                Some(found) => Some(Resolved::Text(found)),
                None => {
                    let default = keys.get(3)?.get(FIND_IN_MAP_DEFAULT_VALUE)?;
                    evaluate(default, scope, depth + 1, budget)
                }
            }
        }
        // "The intrinsic function Fn::GetAZs returns an array that lists Availability
        // Zones for a specified Region … you can specify "" (empty string) … to get all
        // Availability Zones for the Region the stack is created in."
        "Fn::GetAZs" => {
            let requested = if argument.is_null() {
                String::new()
            } else {
                text(argument, scope, depth + 1, budget)?
            };
            let region = if requested.is_empty() {
                scope.stack.region
            } else {
                requested.as_str()
            };
            Some(Resolved::List(availability_zones(region)))
        }
        // "The intrinsic function Fn::Base64 returns the Base64 representation of the
        // input string." The codec is `aws_smithy_types::base64`, the standard-alphabet
        // encoder the wire serializer already uses for every Smithy blob.
        "Fn::Base64" => {
            let source = text(argument, scope, depth + 1, budget)?;
            // Base64 expands its input by 4/3, so the encoded SIZE is refused before the
            // string is built and charged to the budget once it is — the same bound every
            // other text-producing arm is held to, so nesting one inside another cannot
            // multiply what one request allocates.
            if 4 * source.len().div_ceil(3) > MAX_RESOLVED_TEXT_BYTES {
                return None;
            }
            bounded(
                Resolved::Text(::aws_smithy_types::base64::encode(source)),
                budget,
            )
        }
        // "The intrinsic function Fn::Cidr returns an array of CIDR address blocks."
        "Fn::Cidr" => cidr(argument, scope, depth, budget),
        // "Fn::If returns one value if the specified condition evaluates to true and
        // another value if the specified condition evaluates to false":
        // [condition_name, value_if_true, value_if_false].
        "Fn::If" => {
            let chosen = chosen_branch(argument, scope, depth, budget)?;
            evaluate(chosen, scope, depth + 1, budget)
        }
        // "Fn::Length returns the number of elements within an array or an intrinsic
        // function that returns an array." (A `AWS::LanguageExtensions` function; this
        // service runs no transform, and the count is what it is either way.)
        "Fn::Length" => {
            let items = list(argument, scope, depth + 1, budget)?;
            Some(Resolved::Text(items.len().to_string()))
        }
        // "Fn::ToJsonString … converts an object or array to its corresponding JSON
        // string." (An `AWS::LanguageExtensions` function, as above.)
        "Fn::ToJsonString" => json_text(argument, scope, depth, budget).map(Resolved::Text),
        _ => None,
    }
}

/// The Availability Zones Region `region` has, in zone-name order, as `Fn::GetAZs` lists
/// them.
///
/// Derived from the Region name rather than stored: the zones of a Region are a fact of
/// the account's network, named `<region>` + a letter running from `a`
/// ([`AVAILABILITY_ZONES_PER_REGION`] of them) — the model of the account's network every
/// service of this emulator shares (stated in `services/common/src/network.rs`, and
/// implemented for EC2 by that crate's own `availability_zones_in_region`), so the zones a
/// template reads here are the zones `DescribeAvailabilityZones` reports.
fn availability_zones(region: &str) -> Vec<String> {
    (0..AVAILABILITY_ZONES_PER_REGION)
        .map(|index| {
            let letter = char::from(b'a' + u8::try_from(index).unwrap_or(0));
            format!("{region}{letter}")
        })
        .collect()
}

/// `{"Fn::Cidr": [ipBlock, count, cidrBits]}`: "the specified number of CIDRs"
/// (`count`) carved out of `ipBlock` in order, each with `cidrBits` subnet bits —
/// "specifying a value of 8 for this parameter will create a CIDR with a mask of /24"
/// for an IPv4 block, which is the block's address size (32 bits, or 128 for IPv6) less
/// those subnet bits.
///
/// No value for a request the arithmetic cannot satisfy: a block that is not a CIDR, a
/// subnet size larger than the block, or more subnets than the block holds.
fn cidr(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    use ::std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

    let arguments = argument.as_array()?;
    if arguments.len() < 3 {
        return None;
    }
    let block = text(&arguments[0], scope, depth + 1, budget)?;
    let count: usize = text(&arguments[1], scope, depth + 1, budget)?.parse().ok()?;
    let subnet_bits: u32 = text(&arguments[2], scope, depth + 1, budget)?.parse().ok()?;
    if count == 0 || count > MAX_RESOLVED_LIST_LEN {
        return None;
    }
    let (address, prefix) = block.split_once('/')?;
    let prefix: u32 = prefix.parse().ok()?;
    let (base, total_bits, is_ipv6) = match address.parse::<IpAddr>().ok()? {
        IpAddr::V4(address) => (u128::from(u32::from(address)), 32u32, false),
        IpAddr::V6(address) => (u128::from(address), 128u32, true),
    };
    if prefix > total_bits || subnet_bits > total_bits {
        return None;
    }
    let subnet_prefix = total_bits - subnet_bits;
    if subnet_prefix < prefix {
        return None;
    }
    // The block holds 2^(subnet prefix - block prefix) subnets of that size.
    let available = 1u128.checked_shl(subnet_prefix - prefix)?;
    if u128::try_from(count).ok()? > available {
        return None;
    }
    // The distance from one subnet's network to the next. A subnet that IS the whole
    // address space (`::/0` with 128 subnet bits) has no next one — the bound above has
    // already held `count` to the single subnet the block holds — so the step is never
    // added to anything there.
    let step = 1u128.checked_shl(subnet_bits).unwrap_or(0);
    let network = base & (ones(total_bits) ^ ones(total_bits - prefix));
    let mut blocks = Vec::with_capacity(count);
    for index in 0..count {
        let address = network.checked_add(u128::try_from(index).ok()?.checked_mul(step)?)?;
        let rendered = if is_ipv6 {
            Ipv6Addr::from(address).to_string()
        } else {
            Ipv4Addr::from(u32::try_from(address).ok()?).to_string()
        };
        blocks.push(format!("{rendered}/{subnet_prefix}"));
    }
    bounded(Resolved::List(blocks), budget)
}

/// The low `bits` bits set.
fn ones(bits: u32) -> u128 {
    if bits >= 128 {
        u128::MAX
    } else {
        (1u128 << bits) - 1
    }
}

/// Whether the condition `name` the template declares evaluates TRUE, or `None` when
/// this module cannot evaluate it — including when the template declares no condition by
/// that name.
fn condition_holds(
    name: &str,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<bool> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    let declaration = scope.conditions.get(name)?;
    boolean(declaration, scope, depth + 1, budget)
}

/// Evaluate a CONDITION: "the condition functions … Fn::And, Fn::Equals, Fn::If,
/// Fn::Not, Fn::Or", plus a `Condition` naming another declared condition.
///
/// Separate from [`evaluate`] because a condition is not a value: CloudFormation allows
/// these functions in the `Conditions` section and in `Fn::If`, and nowhere a value is
/// expected.
fn boolean(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<bool> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    if let Some(flag) = value.as_bool() {
        return Some(flag);
    }
    let members = value.as_object()?;
    let mut entries = members.iter();
    let (name, argument) = entries.next()?;
    if entries.next().is_some() {
        return None;
    }
    match name.as_str() {
        // "Fn::Equals compares if two values are equal. Returns true if the two values
        // are equal or false if they aren't."
        "Fn::Equals" => {
            let [left, right] = pair(argument)?;
            let left = evaluate(left, scope, depth + 1, budget)?;
            let right = evaluate(right, scope, depth + 1, budget)?;
            Some(left == right)
        }
        // "Fn::Not returns true for a condition that evaluates to false or returns false
        // for a condition that evaluates to true": a one-element list.
        "Fn::Not" => {
            let conditions = argument.as_array()?;
            if conditions.len() != 1 {
                return None;
            }
            Some(!boolean(&conditions[0], scope, depth + 1, budget)?)
        }
        // "Fn::And returns true if all the specified conditions evaluate to true"; and
        // "Fn::Or returns true if any one of the specified conditions evaluate to true".
        "Fn::And" | "Fn::Or" => {
            let conditions = argument.as_array()?;
            if conditions.is_empty() || conditions.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let all = name == "Fn::And";
            let mut holds = all;
            for condition in conditions {
                let evaluated = boolean(condition, scope, depth + 1, budget)?;
                holds = if all {
                    holds && evaluated
                } else {
                    holds || evaluated
                };
            }
            Some(holds)
        }
        // "Condition: … the name of a condition that evaluates to true or false."
        "Condition" => condition_holds(argument.as_str()?, scope, depth + 1, budget),
        _ => None,
    }
}

/// `{"Fn::ToJsonString": <value>}`: the value with every intrinsic function inside it
/// resolved, serialized as JSON.
fn json_text(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    let resolved = json_value(argument, scope, depth + 1, budget)?;
    let rendered = ::serde_json::to_string(&resolved).ok()?;
    budget.spend_text(rendered.len())?;
    (rendered.len() <= MAX_RESOLVED_TEXT_BYTES).then_some(rendered)
}

/// One node of a `Fn::ToJsonString` argument with the intrinsic functions inside it
/// resolved: an intrinsic becomes the string (or the array of strings) it evaluates to,
/// and every other node keeps its shape.
fn json_value(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Value> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    budget.spend(1)?;
    match value {
        Value::Object(members) if is_intrinsic(members) => {
            match evaluate(value, scope, depth + 1, budget)? {
                Resolved::Text(text) => Some(Value::String(text)),
                Resolved::List(items) => {
                    Some(Value::Array(items.into_iter().map(Value::String).collect()))
                }
            }
        }
        Value::Object(members) => {
            if members.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let mut resolved = Map::new();
            for (key, member) in members {
                resolved.insert(key.clone(), json_value(member, scope, depth + 1, budget)?);
            }
            Some(Value::Object(resolved))
        }
        Value::Array(items) => {
            if items.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(json_value(item, scope, depth + 1, budget)?);
            }
            Some(Value::Array(resolved))
        }
        scalar => Some(scalar.clone()),
    }
}

/// Whether an object is an intrinsic function: one member, named `Ref` or `Fn::<Name>`.
fn is_intrinsic(members: &Map<String, Value>) -> bool {
    members.len() == 1
        && members
            .keys()
            .next()
            .is_some_and(|name| name == "Ref" || name.starts_with("Fn::"))
}

/// The two arguments a binary intrinsic function takes.
fn pair(argument: &Value) -> Option<[&Value; 2]> {
    let arguments = argument.as_array()?;
    match arguments.len() {
        2 => Some([&arguments[0], &arguments[1]]),
        _ => None,
    }
}

/// Evaluate an expression to a list of strings.
fn list(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Vec<String>> {
    // A `Fn::GetAtt` naming an attribute the owning service publishes as one value carrying
    // the whole list is a list of values, which is what the functions reading one here are
    // documented to be used on ([`attribute_items`]).
    if let Some(items) = attribute_items(value, scope, depth, budget) {
        return Some(items);
    }
    match evaluate(value, scope, depth, budget)? {
        Resolved::List(items) => Some(items),
        // A function that produced one string where a list was expected is not a list:
        // the list-valued expressions are the list-typed parameters (see
        // [`is_list_type`]) and the functions documented to return an array, and this
        // yields no value rather than splitting a string nobody asked to split.
        Resolved::Text(_) => None,
    }
}

/// `{"Ref": "<name>"}`: a pseudo parameter, a template parameter, or a resource's
/// physical id.
///
/// A parameter declared with a LIST type resolves to a list, not to one string:
/// "CommaDelimitedList: An array of literal strings that are separated by commas", and
/// likewise every `List<…>` type — which is what lets `Fn::Select`, `Fn::Join` and
/// `Fn::Length` read a `Ref` to one.
fn reference(name: &str, scope: &Scope<'_>) -> Option<Resolved> {
    // "CloudFormation provides pseudo parameters … use them the same way as you would a
    // parameter, as the argument for the Ref function."
    match name {
        "AWS::Region" => return Some(Resolved::Text(scope.stack.region.to_string())),
        "AWS::AccountId" => {
            return Some(Resolved::Text(
                ::emulator_server::emulator_account_id().to_string(),
            ))
        }
        "AWS::StackName" => return Some(Resolved::Text(scope.stack.stack_name.to_string())),
        "AWS::StackId" => return Some(Resolved::Text(scope.stack.stack_id.to_string())),
        "AWS::Partition" => return Some(Resolved::Text(PARTITION.to_string())),
        // "AWS::NotificationARNs — Returns the list of notification Amazon Resource Names
        // (ARNs) for the current stack": a LIST, so `Fn::Join`, `Fn::Select` and
        // `Fn::Length` read it the same way they read a list-typed parameter's `Ref`. A
        // stack with no notification topics resolves to the EMPTY list — which it HAS —
        // rather than to nothing.
        "AWS::NotificationARNs" => {
            let arns = scope.stack.notification_arns;
            return (arns.len() <= MAX_RESOLVED_LIST_LEN)
                .then(|| Resolved::List(arns.to_vec()));
        }
        "AWS::URLSuffix" => return Some(Resolved::Text(URL_SUFFIX.to_string())),
        // "AWS::NoValue … removes the corresponding resource property" — there is no
        // value to substitute.
        "AWS::NoValue" => return None,
        _ => {}
    }
    if let Some(value) = scope.parameters.get(name) {
        if is_list_type(scope.parameter_types.get(name).map(String::as_str)) {
            let items: Vec<String> = value
                .split(',')
                .map(|item| item.trim().to_string())
                .collect();
            return (items.len() <= MAX_RESOLVED_LIST_LEN).then_some(Resolved::List(items));
        }
        return Some(Resolved::Text(value.clone()));
    }
    // "Ref returns the physical ID of the resource" for a logical id the template
    // declares.
    scope
        .resources
        .get(name)
        .map(|resource| Resolved::Text(resource.physical_resource_id.clone()))
}

/// Whether a declared parameter type carries a LIST of values: `CommaDelimitedList`, or
/// any of the `List<…>` types ("List<Number>", "List<AWS::EC2::Subnet::Id>", …).
fn is_list_type(declared_type: Option<&str>) -> bool {
    declared_type.is_some_and(|declared| {
        declared == "CommaDelimitedList" || declared.starts_with("List<")
    })
}

/// `{"Fn::GetAtt": ["<logical id>", "<attribute>"]}`, or the short form's
/// `<logical id>.<attribute>`.
///
/// An attribute whose documented return value is a LIST resolves to that list, so the
/// functions that read one (`Fn::Join`, `Fn::Select`, `Fn::Length`) read it the same way
/// they read a list-typed parameter's `Ref` — the elements are carried one per indexed key
/// ([`return_values::list_attribute`]), and a path naming one index reads that element on its
/// own through the dotted notation above. An attribute the owning service publishes as ONE
/// value carrying the whole list is the other carrier, and [`attribute_items`] is where the
/// functions read its entries.
fn attribute(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    let (logical_id, attribute) = attribute_path(argument, scope, depth, budget)?;
    let attributes = &scope.resources.get(&logical_id)?.attributes;
    if let Some(value) = attributes.get(&attribute) {
        return Some(Resolved::Text(value.clone()));
    }
    // Held to the same length every other resolved list is ([`MAX_RESOLVED_LIST_LEN`]); the
    // bytes it carries are charged to the deployment's budget by [`evaluate`], which bounds
    // every intrinsic's result.
    list_attribute(attributes, &attribute)
        .filter(|elements| elements.len() <= MAX_RESOLVED_LIST_LEN)
        .map(Resolved::List)
}

/// The resource and the attribute path one `Fn::GetAtt` argument names, each half evaluated.
fn attribute_path(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<(String, String)> {
    match argument {
        Value::Array(parts) if parts.len() >= 2 => {
            let logical_id = text(&parts[0], scope, depth + 1, budget)?;
            // "Fn::GetAtt … attributeName … For attributes that return a list, use
            // dotted notation": every part past the first names one attribute path.
            let mut path = Vec::with_capacity(parts.len() - 1);
            for part in &parts[1..] {
                path.push(text(part, scope, depth + 1, budget)?);
            }
            Some((logical_id, path.join(".")))
        }
        // The YAML short form spells both halves as one dotted string.
        Value::String(dotted) => {
            let (logical_id, attribute) = dotted.split_once('.')?;
            Some((logical_id.to_string(), attribute.to_string()))
        }
        _ => None,
    }
}

/// The ENTRIES of a list-valued attribute an expression reads, where the owning service
/// publishes that attribute as ONE value carrying the whole list
/// ([`published_list_attribute_names`]).
///
/// A type publishes some of its return values as a list — a load balancer's `SecurityGroups`,
/// a target group's `LoadBalancerArns` — and CloudFormation's own `Fn::GetAtt` documentation
/// says what a template does with one: "for attributes that return a list, use `Fn::Select`".
/// Where the elements are carried one per indexed key, [`attribute`] already answers the list
/// itself; where the attribute is the whole list as one value, this is where the functions
/// that read a list read its entries, while an expression that wants ONE value reads that
/// value as it stands. The entries are the value split on the separator a list of values is
/// written with, and the EMPTY value is the empty list — which a target group no load balancer
/// routes to really has.
fn attribute_items(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Vec<String>> {
    let members = value.as_object()?;
    if members.len() != 1 {
        return None;
    }
    let argument = members.get(FN_GET_ATT)?;
    let (logical_id, attribute) = attribute_path(argument, scope, depth, budget)?;
    let resource = scope.resources.get(&logical_id)?;
    if !published_list_attribute_names(&resource.resource_type).contains(&attribute.as_str()) {
        return None;
    }
    let carried = resource.attributes.get(&attribute)?;
    if carried.is_empty() {
        return Some(Vec::new());
    }
    let items: Vec<String> = carried
        .split(LIST_ATTRIBUTE_SEPARATOR)
        .map(ToString::to_string)
        .collect();
    (items.len() <= MAX_RESOLVED_LIST_LEN).then_some(items)
}

/// The separator a list-valued attribute published as ONE value writes its entries with,
/// which is also the one a list of values takes wherever one string has to carry it
/// ([`attribute_items`]).
const LIST_ATTRIBUTE_SEPARATOR: char = ',';

/// `{"Fn::Sub": "<string>"}` or `{"Fn::Sub": ["<string>", {"<var>": <value>}]}`.
///
/// "To write a dollar sign and curly braces (${}) literally, add an exclamation point (!)
/// after the open curly brace", and a `${Name}` that names neither a local variable nor
/// anything resolvable leaves the whole substitution unevaluated rather than substituting
/// an empty string.
///
/// Each variable the source names is evaluated ONCE however many times it appears
/// (`resolved` below memoises it for this substitution), so repeating `${a}` costs one
/// evaluation of `a` rather than one per occurrence; the deployment's budget bounds the
/// rest.
fn substitute(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    let (source, variables) = match argument {
        Value::String(source) => (source.clone(), Map::new()),
        Value::Array(parts) if parts.len() == 2 => (
            parts[0].as_str()?.to_string(),
            parts[1].as_object()?.clone(),
        ),
        _ => return None,
    };
    let mut resolved: BTreeMap<&str, String> = BTreeMap::new();
    let mut out = String::new();
    let mut rest = source.as_str();
    while let Some(start) = rest.find("${") {
        budget.spend_text(start)?;
        out.push_str(&rest[..start]);
        let after = &rest[start + 2..];
        let end = after.find('}')?;
        let name = &after[..end];
        rest = &after[end + 1..];
        // `${!Literal}` is the escape for a literal `${Literal}`.
        if let Some(literal) = name.strip_prefix('!') {
            out.push_str("${");
            out.push_str(literal);
            out.push('}');
        } else if let Some((key, local)) = variables.get_key_value(name) {
            let substitution = match resolved.get(key.as_str()) {
                Some(already) => already.clone(),
                None => {
                    let evaluated = text(local, scope, depth + 1, budget)?;
                    resolved.insert(key.as_str(), evaluated.clone());
                    evaluated
                }
            };
            out.push_str(&substitution);
        } else if name.contains('.') {
            match attribute(&Value::String(name.to_string()), scope, depth + 1, budget)? {
                Resolved::Text(value) => out.push_str(&value),
                // A list is not a value a substitution can put into a string, exactly as a
                // list-typed `Ref` is not.
                Resolved::List(_) => return None,
            }
        } else {
            match reference(name, scope)? {
                Resolved::Text(value) => out.push_str(&value),
                // A list is not a value a substitution can put into a string.
                Resolved::List(_) => return None,
            }
        }
        budget.spend(1)?;
        if out.len() > MAX_RESOLVED_TEXT_BYTES {
            return None;
        }
    }
    budget.spend_text(rest.len())?;
    out.push_str(rest);
    (out.len() <= MAX_RESOLVED_TEXT_BYTES).then_some(out)
}

// ---------------------------------------------------------------------------
// The TRANSFORM stage: the document CloudFormation deploys, and the one
// `GetTemplate(TemplateStage=Processed)` serves.
//
// `GetTemplate` serves a template at a chosen stage — "To get the user-submitted template,
// specify Original. To get the template after CloudFormation has processed all transforms,
// specify Processed" — so a template that declares a transform has TWO stages, and the
// processed one is what is deployed.
//
// Only ONE transform can be applied here: `AWS::LanguageExtensions`, whose directives are
// rewrites of the template document itself and need nothing outside it. Every other
// transform is a macro backed by a Lambda function (`CreateStackInput$Capabilities`: "Each
// macro relies on an underlying Lambda service function for processing stack templates"),
// which this emulator cannot run — so a template that declares one has NO processed stage
// here, and the reads that would serve it say so rather than answering the unprocessed
// document as if it had been processed.
//
// The stage is computed from the template as a DOCUMENT, read by the same JSON-or-YAML
// reader every other path here uses, so whether a template can be processed (and therefore
// deployed) does not depend on the spelling it was submitted in. It is SERVED as JSON:
// `Original` is the submitted bytes, `Processed` is the expanded document.
// ---------------------------------------------------------------------------

/// The template member that declares the transforms to apply.
const TRANSFORM: &str = "Transform";

/// The one transform this service applies itself: AWS's language-extensions transform,
/// whose directives (`Fn::ForEach`, `Fn::Length`, `Fn::ToJsonString`) are rewrites of the
/// template document.
pub const LANGUAGE_EXTENSIONS: &str = "AWS::LanguageExtensions";

/// The loop directive's key prefix: `Fn::ForEach::<UniqueLoopName>`.
pub const FOR_EACH_PREFIX: &str = "Fn::ForEach::";
/// The directive that replaces itself with the number of items in a list.
pub const FN_LENGTH: &str = "Fn::Length";
/// The directive that replaces itself with the JSON text of its argument.
pub const FN_TO_JSON_STRING: &str = "Fn::ToJsonString";
/// `Fn::GetAtt`, whose first argument names the resource an attribute is read off.
pub const FN_GET_ATT: &str = "Fn::GetAtt";
/// `Fn::FindInMap`, which reads a value out of the `Mappings` section.
pub const FN_FIND_IN_MAP: &str = "Fn::FindInMap";
/// `Fn::If`, whose first argument names the condition that chooses between its branches.
pub const FN_IF: &str = "Fn::If";
/// The member of `Fn::FindInMap`'s optional fourth argument that carries "the value to be
/// returned if the key is not found".
pub const FIND_IN_MAP_DEFAULT_VALUE: &str = "DefaultValue";
/// `Fn::Sub`, whose template string is where a loop identifier is read inside a value.
const FN_SUB: &str = "Fn::Sub";
/// `Fn::ImportValue`, whose argument names the EXPORT a template imports.
const FN_IMPORT_VALUE: &str = "Fn::ImportValue";
/// `Ref`, the other way a loop's output map reads the loop identifier.
const REF_FUNCTION: &str = "Ref";

/// How a transform declaration this service cannot read is named in the refusal that
/// reports it.
const UNREADABLE_TRANSFORM: &str = "a transform this service cannot read";

/// The most entries the `Fn::ForEach` loops of ONE template may emit in total.
///
/// A loop's expansion is MULTIPLICATIVE — a loop nested in a loop runs once per outer item
/// — so the submitted body's own byte bound does not bound the expansion: a tiny template
/// can ask for a cross-product. This is the ceiling on that product, far above the
/// documented [`MAX_RESOURCES_PER_TEMPLATE`] quota a real template expands towards, and a
/// template that would pass it is reported as a stage this service cannot compute instead
/// of being expanded.
pub const MAX_EXPANDED_ENTRIES: usize = 10_000;

/// The most bytes one template's expansion may materialise, counted before any copy is
/// made: entries alone do not bound the work, because one entry can be a large subtree.
/// Eight times the documented submitted-body maximum.
pub const MAX_EXPANDED_BYTES: usize = 8 * super::TEMPLATE_BODY_MAX_BYTES;

/// The deepest the expander descends into a template document.
///
/// Bounds the recursion in [`expand_directives`] and its helpers explicitly, rather than
/// relying on the parser's own nesting limit: a document deeper than this is reported as a
/// stage this service cannot compute. Set to the reader's own [`MAX_TEMPLATE_DEPTH`], so
/// the two bounds state the same depth rather than one sitting unreachably above the
/// other — a document the reader refuses never reaches the expander at all.
pub const MAX_EXPANSION_DEPTH: usize = MAX_TEMPLATE_DEPTH;

/// What one template's expansion may still spend.
///
/// Charged BEFORE each loop copies anything, so a template whose loops would cost more
/// than the budget is refused instead of expanded: no allocation is made on account of
/// work that is already over the ceiling.
struct ExpansionBudget {
    entries_left: usize,
    bytes_left: usize,
}

impl ExpansionBudget {
    const fn new() -> Self {
        Self {
            entries_left: MAX_EXPANDED_ENTRIES,
            bytes_left: MAX_EXPANDED_BYTES,
        }
    }

    /// Spend `entries` entries and `bytes` bytes of the budget, or refuse the whole
    /// expansion. `what` names the directive being charged, for the refusal.
    fn charge(&mut self, what: &str, entries: usize, bytes: usize) -> Result<(), String> {
        if entries > self.entries_left || bytes > self.bytes_left {
            return Err(format!(
                "{what}: expanding this template would emit more than \
                 {MAX_EXPANDED_ENTRIES} entries or {MAX_EXPANDED_BYTES} bytes"
            ));
        }
        self.entries_left -= entries;
        self.bytes_left -= bytes;
        Ok(())
    }
}

/// The refusal for a document deeper than [`MAX_EXPANSION_DEPTH`].
fn too_deep() -> String {
    format!("the template nests more than {MAX_EXPANSION_DEPTH} levels deep")
}

/// A template's `Processed` stage, as computed from the submitted body.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Processed {
    /// The template declares no transform this service has to apply, so "If the template
    /// doesn't include transforms, Original and Processed return the same template" — the
    /// submitted body, byte for byte.
    Same,
    /// Every declared transform was applied: this is the resulting document.
    Applied(String),
    /// A declared transform this service cannot apply, and why. Nothing is guessed: the
    /// reads that serve the processed stage refuse instead.
    Unavailable(String),
}

impl Processed {
    /// The processed body of a stack whose submitted body is `original`, or `None` when
    /// this service could not compute it.
    pub fn body<'a>(&'a self, original: &'a str) -> Option<&'a str> {
        match self {
            Self::Same => Some(original),
            Self::Applied(body) => Some(body),
            Self::Unavailable(_) => None,
        }
    }

    /// Why the processed stage is unavailable, for the refusal that reports it.
    pub fn unavailable_reason(&self) -> Option<&str> {
        match self {
            Self::Unavailable(reason) => Some(reason),
            Self::Same | Self::Applied(_) => None,
        }
    }
}

/// The names of the transforms a template BODY declares, read from the parsed document.
///
/// `Transform` is one name, a list of names, or a macro object (`{"Name": ...,
/// "Parameters": ...}`) — or a list of those.
///
/// Read from the PARSED body, through the same JSON-or-YAML reader the rest of this module
/// uses ([`document`]), so a transform declared in either spelling and in any form the
/// parser accepts is seen — which is what both the `CAPABILITY_AUTO_EXPAND` refusal and
/// the processed stage depend on. [`declared_transforms`] is the same reading of an
/// ALREADY PARSED `Transform` section, for a caller that has the document in hand; this
/// one additionally NAMES a declaration it cannot read, because a refusal has to report
/// it.
pub fn body_declared_transforms(body: &str) -> Vec<String> {
    let Some(Value::Object(root)) = document(body) else {
        return Vec::new();
    };
    root.get(TRANSFORM).map(transform_names).unwrap_or_default()
}

/// The transform names one `Transform` node declares.
fn transform_names(node: &Value) -> Vec<String> {
    match node {
        Value::String(name) => vec![name.clone()],
        Value::Array(items) => items.iter().flat_map(transform_names).collect(),
        // A macro invoked with parameters: `{"Name": "MyMacro", "Parameters": {..}}`.
        Value::Object(map) => match map.get("Name").and_then(Value::as_str) {
            Some(name) => vec![name.to_string()],
            // A declaration this service cannot read is still a declaration: it is named
            // by its own text so the refusal can report it.
            None => vec![UNREADABLE_TRANSFORM.to_string()],
        },
        _ => vec![UNREADABLE_TRANSFORM.to_string()],
    }
}

/// The `Processed` stage of a submitted template body.
///
/// The transforms this service applies itself are AWS's own two: the SAM macro, whose
/// expansion [`super::sam`] owns, and the language-extensions transform, whose directives
/// are expanded here. Any OTHER declared transform is a macro backed by a Lambda function
/// that runs outside this emulator, so the stage is reported UNAVAILABLE rather than
/// computed from a macro nothing ran.
pub fn process(body: &str) -> Processed {
    process_with_parameters(body, &BTreeMap::new())
}

/// [`process`] with the parameter values the submitting request carried.
///
/// The language-extensions directives read the parameters in force: "Fn::Length returns the
/// number of elements within an array or an intrinsic function that returns an array", and a
/// `Ref` to a list-typed parameter IS such an intrinsic — so the stage this computes depends
/// on the values the stack is deployed with, exactly as the resources and outputs evaluated
/// from it do. A parameter the request passed no value for takes the template's own
/// `Default`; one with no value in force at all leaves the directive unresolved, which is a
/// stage this service reports as unavailable rather than guessing a count for.
pub fn process_with_parameters(body: &str, supplied: &BTreeMap<String, String>) -> Processed {
    // The documented submitted-body maximum, enforced here too: every handler checks it
    // before storing a body, and this is the expander's own guard so no caller reaches the
    // expansion with a body past that bound.
    if body.len() > super::TEMPLATE_BODY_MAX_BYTES {
        return Processed::Unavailable(format!(
            "the template body exceeds {} bytes",
            super::TEMPLATE_BODY_MAX_BYTES
        ));
    }
    let declared = body_declared_transforms(body);
    if declared.is_empty() {
        return Processed::Same;
    }
    let unsupported: Vec<&str> = declared
        .iter()
        .map(String::as_str)
        .filter(|name| *name != LANGUAGE_EXTENSIONS && *name != super::sam::TRANSFORM)
        .collect();
    if !unsupported.is_empty() {
        return Processed::Unavailable(format!(
            "the template declares the macro transform(s) [{}], which run as Lambda \
             functions outside this emulator",
            unsupported.join(", ")
        ));
    }
    // The document itself, read through the same JSON-or-YAML reader every other path in
    // this module uses, so the deployability of a template does not depend on the spelling
    // it was submitted in. The processed stage is a DOCUMENT, not a re-spelling of the
    // submitted text: it is serialised as JSON below whatever the body was spelled as,
    // which is the form this service serves it in and deploys.
    let Some(Value::Object(root)) = document(body) else {
        return Processed::Unavailable(
            "the template declares a transform and is not a document this service can read"
                .to_string(),
        );
    };
    let mut document = Value::Object(root);
    // What a `Ref` inside a directive's argument resolves to, read from the document's own
    // declarations and the values this request carried, BEFORE the `Parameters` section is
    // walked over by the expansion.
    let parameters = directive_parameters(&document, supplied);
    // The SAM macro's expansion reads the `Transform` section itself, so it runs while that
    // section is still there — with the parameter values in force, since one property of
    // that expansion reads a parameter (see
    // [`expand_with_parameters`](super::sam::expand_with_parameters)).
    super::sam::expand_with_parameters(&mut document, &parameters.values);
    if let Some(sections) = document.as_object_mut() {
        // The processed template is what CloudFormation deploys, and the transforms it
        // declared have been applied by then: the member does not survive into it.
        sections.remove(TRANSFORM);
    }
    let mut budget = ExpansionBudget::new();
    if let Err(reason) = expand_directives(&mut document, &parameters, &mut budget, 0) {
        return Processed::Unavailable(reason);
    }
    // Serialised in KEY order, whatever map type `serde_json` was compiled with: see
    // [`in_key_order`].
    match ::serde_json::to_string(&in_key_order(document)) {
        // The expansion is charged before it happens, so this is the belt to that brace:
        // the document this service stores and serves is bounded whatever the directives
        // were.
        Ok(processed) if processed.len() > MAX_EXPANDED_BYTES => Processed::Unavailable(format!(
            "the processed template would exceed {MAX_EXPANDED_BYTES} bytes"
        )),
        Ok(processed) => Processed::Applied(processed),
        Err(error) => Processed::Unavailable(error.to_string()),
    }
}

/// `value` with the members of every object in KEY order.
///
/// `serde_json`'s map type is chosen by a FEATURE any crate in the build may enable: with
/// `preserve_order` it keeps insertion order, without it key order. The document this
/// service serves may not depend on how the workspace happened to be compiled, so it is
/// put in key order here — the order CloudFormation's own processed template comes back
/// in.
fn in_key_order(value: Value) -> Value {
    match value {
        Value::Array(items) => Value::Array(items.into_iter().map(in_key_order).collect()),
        Value::Object(map) => {
            let mut members: Vec<(String, Value)> = map.into_iter().collect();
            members.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                members
                    .into_iter()
                    .map(|(key, member)| (key, in_key_order(member)))
                    .collect(),
            )
        }
        other => other,
    }
}

/// One resource's `Properties` as the JSON document `StackEvent$ResourceProperties`
/// carries ("BLOB of the properties used to create the resource"), in key order so the
/// text does not depend on the build.
pub fn properties_blob(properties: &Value) -> Option<String> {
    if properties.is_null() {
        return None;
    }
    ::serde_json::to_string(&in_key_order(properties.clone())).ok()
}

/// Apply the language-extensions directives in `node`, spending `budget` on what the loops
/// copy and refusing below `depth`.
///
/// `Err(reason)` for a directive this service cannot resolve from the document alone, and
/// for one that would cost more than the budget allows: nothing is guessed and nothing is
/// expanded past the ceiling — the processed stage is reported unavailable rather than
/// served half-expanded.
fn expand_directives(
    node: &mut Value,
    parameters: &DirectiveParameters,
    budget: &mut ExpansionBudget,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_EXPANSION_DEPTH {
        return Err(too_deep());
    }
    match node {
        Value::Array(items) => {
            for item in items.iter_mut() {
                expand_directives(item, parameters, budget, depth + 1)?;
            }
            Ok(())
        }
        Value::Object(map) => {
            let mut expanded = Map::new();
            for (key, mut value) in ::std::mem::take(map) {
                match key.strip_prefix(FOR_EACH_PREFIX) {
                    // A loop expands INTO the map that holds it, so its entries replace
                    // the directive's own key.
                    Some(loop_name) => {
                        for (entry_key, entry_value) in
                            expand_for_each(loop_name, &value, parameters, budget, depth + 1)?
                        {
                            insert_expanded(&mut expanded, entry_key, entry_value)?;
                        }
                    }
                    None => {
                        expand_directives(&mut value, parameters, budget, depth + 1)?;
                        insert_expanded(&mut expanded, key, value)?;
                    }
                }
            }
            *map = expanded;
            // The directives that REPLACE their own node, now their argument is expanded.
            if let Some(replacement) = evaluate_directive(map, parameters, budget)? {
                *node = replacement;
            }
            Ok(())
        }
        _ => Ok(()),
    }
}

/// The parameter values a language-extensions directive resolves a `Ref` against: the
/// values in force on the deployment whose stage is being computed.
#[derive(Debug, Default)]
struct DirectiveParameters {
    /// Each parameter's value in force, keyed by the name a `Ref` names it by.
    values: BTreeMap<String, String>,
    /// The parameters declared with a LIST type, whose value is "an array of literal
    /// strings that are separated by commas" rather than one string.
    lists: BTreeSet<String>,
}

impl DirectiveParameters {
    /// What `{"Ref": "<name>"}` stands for inside a directive's argument, or `None` for a
    /// name with no value in force — which leaves the reference where it was, so the
    /// directive that needed it is reported unresolved rather than resolved to nothing.
    fn value_of(&self, name: &str) -> Option<Value> {
        let value = self.values.get(name)?;
        if self.lists.contains(name) {
            // The split is stopped one past the bound every list this service resolves is
            // held to ([`MAX_RESOLVED_LIST_LEN`]): the text is request-supplied, so as many
            // items as a body holds commas must never be materialised — a value past the
            // bound leaves the reference unresolved instead.
            let items: Vec<Value> = value
                .split(',')
                .take(MAX_RESOLVED_LIST_LEN + 1)
                .map(|item| Value::String(item.trim().to_string()))
                .collect();
            return (items.len() <= MAX_RESOLVED_LIST_LEN).then_some(Value::Array(items));
        }
        Some(Value::String(value.clone()))
    }
}

/// The parameters in force for `document`: its own declared defaults, overridden by the
/// values the submitting request carried.
fn directive_parameters(
    document: &Value,
    supplied: &BTreeMap<String, String>,
) -> DirectiveParameters {
    let declared = template_of(document).unwrap_or_default();
    let mut values = declared.parameter_defaults();
    for (key, value) in supplied {
        values.insert(key.clone(), value.clone());
    }
    let lists = declared
        .parameter_types()
        .into_iter()
        .filter(|(_, declared_type)| is_list_type(Some(declared_type.as_str())))
        .map(|(key, _)| key)
        .collect();
    DirectiveParameters { values, lists }
}

/// Replace every `{"Ref": "<parameter>"}` inside a directive's argument with the value in
/// force for that parameter, so a directive reading a parameter resolves from the template
/// and the request alone. Refuses below [`MAX_EXPANSION_DEPTH`], like the expander itself.
fn resolve_parameter_refs(
    node: &mut Value,
    parameters: &DirectiveParameters,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_EXPANSION_DEPTH {
        return Err(too_deep());
    }
    match node {
        Value::Array(items) => {
            for item in items.iter_mut() {
                resolve_parameter_refs(item, parameters, depth + 1)?;
            }
        }
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(name) = map.get(REF_FUNCTION).and_then(Value::as_str) {
                    if let Some(value) = parameters.value_of(name) {
                        *node = value;
                    }
                    return Ok(());
                }
            }
            for value in map.values_mut() {
                resolve_parameter_refs(value, parameters, depth + 1)?;
            }
        }
        _ => {}
    }
    Ok(())
}

/// Insert one expanded entry, refusing a key that is already taken.
///
/// An expansion emits the KEYS its output map spells, and CloudFormation refuses a template
/// that declares one logical id twice ("A duplicate logical ID was found"), so two entries
/// claiming one key is a template this service will not expand — never a declaration
/// silently dropped because another overwrote it.
fn insert_expanded(
    expanded: &mut Map<String, Value>,
    key: String,
    value: Value,
) -> Result<(), String> {
    if expanded.contains_key(&key) {
        return Err(format!(
            "the expansion would declare {key:?} more than once, which is a duplicate name \
             in the processed template"
        ));
    }
    expanded.insert(key, value);
    Ok(())
}

/// One `Fn::ForEach::<loop_name>` loop's entries: the output map once per collection item,
/// with the loop identifier resolved to that item.
fn expand_for_each(
    loop_name: &str,
    node: &Value,
    parameters: &DirectiveParameters,
    budget: &mut ExpansionBudget,
    depth: usize,
) -> Result<Vec<(String, Value)>, String> {
    if depth > MAX_EXPANSION_DEPTH {
        return Err(too_deep());
    }
    let malformed =
        || format!("{FOR_EACH_PREFIX}{loop_name}: expected [Identifier, Collection, OutputMap]");
    let Value::Array(parts) = node else {
        return Err(malformed());
    };
    let [identifier, collection, output_map] = &parts[..] else {
        return Err(malformed());
    };
    let (Value::String(identifier), Value::Object(output_map)) = (identifier, output_map) else {
        return Err(malformed());
    };
    let mut collection = collection.clone();
    expand_directives(&mut collection, parameters, budget, depth + 1)?;
    // "The collection ... can be a list or a reference to a list": a `Ref` to a list-typed
    // parameter is resolved to the items in force for it, like every other directive's
    // argument.
    resolve_parameter_refs(&mut collection, parameters, depth + 1)?;
    let Value::Array(collection) = collection else {
        return Err(format!(
            "{FOR_EACH_PREFIX}{loop_name}: the collection is not a list this service can \
             resolve from the template alone"
        ));
    };

    // The text one instance of the output map starts from, and how many times this loop's
    // identifier is READ in it — both ways: `${identifier}` inside a key or an `Fn::Sub`
    // string, and a `{"Ref": "<identifier>"}` node that is replaced by the item whole.
    // Substituting an item for either grows that text by at most the item's own JSON
    // length, which is what makes the charge below cover what the copy will actually
    // materialise.
    //
    // The reads are counted over the output map's own DATA ([`identifier_reads`]), not over
    // the serialized text: substitution operates on the data, while serialization escapes
    // `"`, `\` and the control bytes — so an identifier carrying one of those has a
    // reference spelling that never occurs literally in the serialized text, and a count
    // taken there would be zero while every one of those reads still allocates.
    let output_text = ::serde_json::to_string(output_map).unwrap_or_default();
    let placeholder = format!("${{{identifier}}}");
    let reads_of = |needle: &str, ref_name: Option<&str>| -> usize {
        output_map.iter().fold(0usize, |reads, (key, value)| {
            reads
                .saturating_add(key.matches(needle).count())
                .saturating_add(identifier_reads(value, needle, ref_name, depth + 1))
        })
    };
    // AWS's ForEach documents a second reference form, `&{Identifier}`, used where a
    // logical id has to drop the collection value's non-alphanumeric characters. This
    // service does not apply that form, and leaving it literal would emit ONE output key
    // for every item of the collection, so the stage is reported unavailable instead of
    // served with resources silently collapsed into one.
    if reads_of(format!("&{{{identifier}}}").as_str(), None) > 0 {
        return Err(format!(
            "{FOR_EACH_PREFIX}{loop_name}: the &{{{identifier}}} form of the loop reference is \
             not one this service can apply"
        ));
    }
    let reads = reads_of(placeholder.as_str(), Some(identifier.as_str()));

    let mut entries = Vec::new();
    for item in &collection {
        let Some(rendered) = rendered_item(item) else {
            return Err(format!(
                "{FOR_EACH_PREFIX}{loop_name}: a collection item is not a value a logical id \
                 can be built from"
            ));
        };
        // Charged BEFORE this item's copy is made, from the SUBSTITUTED size: nothing is
        // allocated on account of work that is already over the ceiling. A loop nested
        // inside this one charges the same budget again, so the PRODUCT of the nesting is
        // what the ceiling bounds. The item's JSON spelling is its text plus its quotes.
        budget.charge(
            &format!("{FOR_EACH_PREFIX}{loop_name}"),
            output_map.len(),
            output_text
                .len()
                .saturating_add(reads.saturating_mul(rendered.len().saturating_add(2))),
        )?;
        let mut instance = Map::new();
        for (key, value) in output_map {
            let mut value = value.clone();
            substitute_identifier(&mut value, &placeholder, item, &rendered, depth + 1)?;
            // Through the duplicate-refusing helper, like every other entry this expansion
            // emits: two keys of ONE output map can collide after substitution too, and a
            // declaration is never dropped because another overwrote it.
            insert_expanded(&mut instance, key.replace(&placeholder, &rendered), value)?;
        }
        // Expanded only now: a nested loop or an `Fn::Length` inside the output map is
        // resolved against THIS item's substitutions.
        let mut instance = Value::Object(instance);
        expand_directives(&mut instance, parameters, budget, depth + 1)?;
        let Value::Object(instance) = instance else {
            return Err(malformed());
        };
        entries.extend(instance);
    }
    Ok(entries)
}

/// How many times one spelling of a loop reference is read inside an output map's VALUE —
/// the sites [`substitute_identifier`] rewrites, counted in the same shape it walks: a key
/// of a nested map, an `Fn::Sub` template string (bare or two-argument), and a single-member
/// `{"Ref": "<identifier>"}` node, which is replaced by the item whole and so counts once.
///
/// Counted over the DATA, which is what substitution allocates against. `ref_name` is the
/// identifier whose `Ref` node that whole-node replacement belongs to, and `None` for a
/// spelling it does not apply to. Stops at [`MAX_EXPANSION_DEPTH`], which is exactly where
/// [`substitute_identifier`] stops rewriting.
fn identifier_reads(
    node: &Value,
    needle: &str,
    ref_name: Option<&str>,
    depth: usize,
) -> usize {
    if depth > MAX_EXPANSION_DEPTH {
        return 0;
    }
    match node {
        Value::Array(items) => items.iter().fold(0usize, |reads, value| {
            reads.saturating_add(identifier_reads(value, needle, ref_name, depth + 1))
        }),
        Value::Object(map) => {
            if map.len() == 1
                && ref_name.is_some_and(|name| {
                    map.get(REF_FUNCTION).and_then(Value::as_str) == Some(name)
                })
            {
                return 1;
            }
            let mut reads = 0usize;
            for (key, value) in map {
                reads = reads.saturating_add(key.matches(needle).count());
                let in_value = if key == FN_SUB {
                    match value {
                        Value::String(text) => text.matches(needle).count(),
                        Value::Array(items) => {
                            let template = match items.first() {
                                Some(Value::String(text)) => text.matches(needle).count(),
                                _ => 0,
                            };
                            items.iter().skip(1).fold(template, |reads, variables| {
                                reads.saturating_add(identifier_reads(
                                    variables,
                                    needle,
                                    ref_name,
                                    depth + 1,
                                ))
                            })
                        }
                        other => identifier_reads(other, needle, ref_name, depth + 1),
                    }
                } else {
                    identifier_reads(value, needle, ref_name, depth + 1)
                };
                reads = reads.saturating_add(in_value);
            }
            reads
        }
        _ => 0,
    }
}

/// Resolve one loop identifier inside an output map's value: `{"Ref": "<identifier>"}`
/// becomes the item, and `${<identifier>}` inside an `Fn::Sub` template string becomes the
/// item's text. Refuses below [`MAX_EXPANSION_DEPTH`], like the expander itself.
fn substitute_identifier(
    node: &mut Value,
    placeholder: &str,
    item: &Value,
    rendered: &str,
    depth: usize,
) -> Result<(), String> {
    if depth > MAX_EXPANSION_DEPTH {
        return Err(too_deep());
    }
    match node {
        Value::Array(items) => {
            for value in items.iter_mut() {
                substitute_identifier(value, placeholder, item, rendered, depth + 1)?;
            }
        }
        Value::Object(map) => {
            if map.len() == 1
                && map
                    .get(REF_FUNCTION)
                    .and_then(Value::as_str)
                    .is_some_and(|name| format!("${{{name}}}") == placeholder)
            {
                *node = item.clone();
                return Ok(());
            }
            let mut substituted = Map::new();
            for (key, mut value) in ::std::mem::take(map) {
                if key == FN_SUB {
                    // `Fn::Sub` is where a value reads the loop identifier, in either
                    // documented form: the bare template string, or the two-argument
                    // `["<template>", {<variables>}]` whose first element is that string
                    // and whose variable map may read the identifier too.
                    match &mut value {
                        Value::String(text) => *text = text.replace(placeholder, rendered),
                        Value::Array(items) => {
                            if let Some(Value::String(text)) = items.first_mut() {
                                *text = text.replace(placeholder, rendered);
                            }
                            for variables in items.iter_mut().skip(1) {
                                substitute_identifier(
                                    variables,
                                    placeholder,
                                    item,
                                    rendered,
                                    depth + 1,
                                )?;
                            }
                        }
                        other => {
                            substitute_identifier(other, placeholder, item, rendered, depth + 1)?
                        }
                    }
                } else {
                    substitute_identifier(&mut value, placeholder, item, rendered, depth + 1)?;
                }
                // The same refusal the expansion's own inserts follow: substituting the
                // identifier into a nested key must not collapse two members into one.
                insert_expanded(&mut substituted, key.replace(placeholder, rendered), value)?;
            }
            *map = substituted;
        }
        _ => {}
    }
    Ok(())
}

/// The value a language-extensions directive replaces its own node with, or `None` when
/// this map is not one.
fn evaluate_directive(
    map: &Map<String, Value>,
    parameters: &DirectiveParameters,
    budget: &mut ExpansionBudget,
) -> Result<Option<Value>, String> {
    // An intrinsic function is a single-member object, so a map with any other member
    // beside it is a plain template fragment.
    if map.len() != 1 {
        return Ok(None);
    }
    let Some((name, argument)) = map.iter().next() else {
        return Ok(None);
    };
    match name.as_str() {
        FN_LENGTH => {
            // The argument's own parameter references first: "an intrinsic function that
            // returns an array" includes a `Ref` to a list-typed parameter.
            let mut argument = argument.clone();
            resolve_parameter_refs(&mut argument, parameters, 0)?;
            match &argument {
                Value::Array(items) => Ok(Some(Value::from(items.len()))),
                // Not a list the DOCUMENT decides: the directive is left where it is, for
                // the deployment to evaluate against the whole scope — `Fn::GetAZs` and
                // `Fn::Split` are "an intrinsic function that returns an array" too, and
                // what they return is not in the template. An argument that can be no list
                // at all is refused where the submitted template is validated
                // ([`super::template_validation`]).
                _ => Ok(None),
            }
        }
        FN_TO_JSON_STRING => {
            let mut argument = argument.clone();
            resolve_parameter_refs(&mut argument, parameters, 0)?;
            if holds_intrinsic(&argument, 0) {
                // Likewise: a document still holding an intrinsic function is rendered by
                // the deployment, which can evaluate it.
                return Ok(None);
            }
            // Charged BEFORE the text is produced, from an upper bound computed without
            // building it: this directive renders a document INTO a string, which escapes
            // every quote and backslash, so one nested inside another would otherwise
            // double its own input per level with nothing to stop it.
            budget.charge(FN_TO_JSON_STRING, 1, escaped_size_bound(&argument))?;
            // Rendered in KEY order, like the processed document itself: see
            // [`in_key_order`].
            ::serde_json::to_string(&in_key_order(argument))
                .map(|text| Some(Value::String(text)))
                .map_err(|error| error.to_string())
        }
        _ => Ok(None),
    }
}

/// An upper bound on the bytes rendering `node` into a JSON STRING costs, computed without
/// building either: the document's own worst-case length, then doubled for the escaping a
/// string spelling applies to it, plus its quotes.
fn escaped_size_bound(node: &Value) -> usize {
    json_size_bound(node, 0)
        .saturating_mul(2)
        .saturating_add(2)
}

/// An upper bound on `serde_json::to_string(node)`'s length, walked without allocating.
///
/// Every byte of a string can need escaping, so a string counts double; a subtree deeper
/// than [`MAX_EXPANSION_DEPTH`] counts as unbounded, which refuses the directive that
/// asked for it rather than walking further.
fn json_size_bound(node: &Value, depth: usize) -> usize {
    if depth > MAX_EXPANSION_DEPTH {
        return usize::MAX;
    }
    match node {
        Value::Null => 4,
        Value::Bool(_) => 5,
        Value::Number(number) => number.to_string().len(),
        Value::String(text) => text.len().saturating_mul(2).saturating_add(2),
        Value::Array(items) => items.iter().fold(2, |sum, item| {
            sum.saturating_add(json_size_bound(item, depth + 1))
                .saturating_add(1)
        }),
        Value::Object(map) => map.iter().fold(2, |sum, (key, value)| {
            sum.saturating_add(key.len().saturating_mul(2).saturating_add(4))
                .saturating_add(json_size_bound(value, depth + 1))
                .saturating_add(1)
        }),
    }
}

/// Whether `node` still holds an intrinsic function — a `Ref` or an `Fn::*` member — so a
/// directive that needs a fully resolved argument knows it has not got one.
///
/// A subtree deeper than [`MAX_EXPANSION_DEPTH`] is reported as unresolved rather than
/// walked further, which refuses the directive that asked.
fn holds_intrinsic(node: &Value, depth: usize) -> bool {
    if depth > MAX_EXPANSION_DEPTH {
        return true;
    }
    match node {
        Value::Array(items) => items.iter().any(|item| holds_intrinsic(item, depth + 1)),
        Value::Object(map) => map.iter().any(|(key, value)| {
            key == REF_FUNCTION || key.starts_with("Fn::") || holds_intrinsic(value, depth + 1)
        }),
        _ => false,
    }
}

/// One collection item as the text a logical id or an `Fn::Sub` string is built from. A
/// list or an object is not such a value, which is what `None` reports.
fn rendered_item(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}

#[cfg(test)]
mod tests;
mod return_values;
mod conditions;
mod naming_rules;
pub(crate) use naming_rules::*;
use conditions::chosen_branch;
pub use return_values::*;
