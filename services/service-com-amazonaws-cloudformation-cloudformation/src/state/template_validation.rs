// SPDX-License-Identifier: CC0-1.0

//! What a SUBMITTED template is held to BEFORE the operation it was submitted to acts on
//! it: the template-validation rules CloudFormation applies synchronously, and the
//! `ValidationError` each one answers.
//!
//! CloudFormation "validates the template" as part of accepting a stack operation, and a
//! template whose own declarations cannot be resolved is one it never deploys: the call is
//! refused with HTTP 400 and NOTHING is written, rather than accepted with a stack id and
//! then deployed into a stack whose resources carry nulls. [`refusal`] is that whole pass,
//! in ONE place, so every operation that TAKES a body holds it to the same rules and words
//! the refusal the same way: the three that submit one for deployment — `CreateStack`,
//! `UpdateStack`, `CreateChangeSet` — and `ValidateTemplate`, whose whole answer IS a
//! verdict on the body, over the values the template's own `Default`s put in force.
//! `ValidateTemplate` reads that pass through [`validation_refusal`]: its verdict is on what
//! the DOCUMENT decides, so the one rule it is not held to is the one that evaluates what an
//! intrinsic resolves to.
//!
//! Every rule here is decided from the template DOCUMENT alone, which is what makes it a
//! rule about the request rather than about the deployment:
//!
//! * a body the reader will not read at all is a template-FORM error
//!   ([`super::template::not_well_formed`]), and so is one it DOES read that states no
//!   mapping of template sections — a sequence or a scalar at the top level, or a section
//!   stated as something other than the mapping that section is
//!   ([`super::template::section_map`]);
//! * the `AWS::LanguageExtensions` directives are functions only that transform defines, so
//!   a template using one without declaring it is refused — and `Fn::ForEach` is refused in
//!   a section the transform does not define it for;
//! * an intrinsic function whose arguments are not the ones it takes is refused for its own
//!   arity, as is a condition's boolean operator;
//! * a `Ref` naming something the template does not declare, and a `Condition` naming a
//!   condition it does not declare, are unresolved DEPENDENCIES: the section they appear in
//!   is named, because that is where the author has to look — while an `Fn::GetAtt` reading
//!   an attribute off a resource the template does not declare is reported for the FUNCTION
//!   instance that read it, as is an `Fn::Sub` whose dotted `${Name.Attribute}` reference
//!   names no resource attribute at all ([`substitution_attribute_refusal`], which the
//!   operations that only SUMMARISE a template are held to as well);
//! * a `DependsOn` naming a logical id the `Resources` section does not declare is an
//!   unresolved dependency too — the creation order it asks for cannot be resolved, and that
//!   is decided from the document before anything is provisioned;
//! * an `Fn::FindInMap` whose keys the `Mappings` section does not carry resolves to
//!   nothing — unless the lookup names a `DefaultValue` to fall back to — and one naming a
//!   map that section does not declare at all is reported for the missing map; this is the
//!   one rule decided by EVALUATING the lookup, so only a body submitted for deployment is
//!   held to it;
//! * an output's `Export.Name` is fixed from the template and the values in force, so one
//!   computed from a resource, from a value another stack exports or from the Region's
//!   Availability Zones is refused, naming the output that declared it;
//! * and a dependency CYCLE among the declared resources is refused, since no creation
//!   order satisfies it.
//!
//! The semantic rules are applied to the template's PROCESSED stage — the document
//! CloudFormation really deploys — so a resource a transform's expansion contributes is
//! held to them, and a loop's own identifier is not read as an undeclared name. A template
//! whose processed stage this service cannot compute is left to the stack's own failure
//! ([`super::template::process`]), which is the one thing this pass has nothing to say
//! about.
//!
//! The `Rules` section is NOT part of this pass: a rule's assertion is checked against the
//! parameter values a deployment runs with, and a failing one fails that deployment rather
//! than the request that submitted it — [`rules_failure`].

use ::std::collections::{BTreeMap, BTreeSet};

use ::serde_json::Value;

use super::template::{self, Template};

/// The pseudo parameters a template may read without declaring them: "CloudFormation
/// provides pseudo parameters ... you don't declare them in your template".
const PSEUDO_PARAMETERS: [&str; 8] = [
    "AWS::AccountId",
    "AWS::NoValue",
    "AWS::NotificationARNs",
    "AWS::Partition",
    "AWS::Region",
    "AWS::StackId",
    "AWS::StackName",
    "AWS::URLSuffix",
];

/// The template sections the `Fn::ForEach` directive may appear in, as the
/// language-extensions transform defines it ("you can use the Fn::ForEach intrinsic
/// function in the Conditions, Resources and Outputs sections").
const FOR_EACH_SECTIONS: [&str; 3] = ["Conditions", "Outputs", "Resources"];

/// The `Resources` section, named in the refusal of an unresolved dependency inside it.
const RESOURCES: &str = "Resources";

/// The `Outputs` section, likewise.
const OUTPUTS: &str = "Outputs";

/// The `Conditions` section, likewise.
const CONDITIONS: &str = "Conditions";

/// `Some(message)` for a submitted template body CloudFormation refuses outright, worded
/// as the service words it; `None` for one it accepts.
///
/// `processed` is the stage the submitting operation computed for `body`
/// ([`super::template::process_with_parameters`]), so the semantic rules are applied to the
/// document that really deploys without expanding the template twice. `supplied` carries the
/// parameter values the request passed and `region` the Region it was signed for: a lookup
/// or a key an expression decides is resolved against them, exactly as the deployment would
/// resolve it.
pub fn refusal(
    body: &str,
    processed: &template::Processed,
    supplied: &BTreeMap<String, String>,
    region: &str,
) -> Option<String> {
    pass(body, processed, supplied, region, Pass::Submitted)
}

/// Which pass over a body this is: what the operation reading it DOES with the template is
/// what decides which rules the body is held to.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Pass {
    /// A body submitted for DEPLOYMENT (`CreateStack`, `UpdateStack`, `CreateChangeSet`):
    /// every rule here, since the values the deployment would run with are the ones in force.
    Submitted,
    /// A body submitted only to be VALIDATED (`ValidateTemplate`): the rules the DOCUMENT
    /// decides on its own. What a `Mappings` lookup resolves to is not among them — "validates
    /// a specified template" is a verdict on the template's form and its own declarations, and
    /// CloudFormation does not evaluate the template's intrinsics to give it, so a lookup this
    /// body makes is left to the operation that deploys it.
    Validated,
}

/// [`refusal`] for a body submitted to be VALIDATED rather than deployed: every rule the
/// DOCUMENT decides on its own, and none that evaluates what an intrinsic resolves to.
pub fn validation_refusal(
    body: &str,
    processed: &template::Processed,
    supplied: &BTreeMap<String, String>,
    region: &str,
) -> Option<String> {
    pass(body, processed, supplied, region, Pass::Validated)
}

/// The whole pass, which [`refusal`] and [`validation_refusal`] are the two readings of: one
/// list of rules in one order, so the two cannot drift apart about anything they share.
fn pass(
    body: &str,
    processed: &template::Processed,
    supplied: &BTreeMap<String, String>,
    region: &str,
    reading: Pass,
) -> Option<String> {
    // A body that is no template DOCUMENT at all is refused for its FORM, in the one place
    // that verdict lives: "CloudFormation first checks if the template is valid JSON ... if
    // both these checks fail, CloudFormation returns a template validation error", and a body
    // it DOES read but that states no mapping of template sections has a structure the format
    // does not have ([`template::section_map`]).
    let document = match template::section_map(body) {
        Ok(document) => document,
        Err(message) => return Some(message),
    };
    // A parameter is named by its own section key, and the names a parameter may be given
    // are the alphanumeric ones — which is also what a template using a DIRECTIVE as a
    // parameter name runs into.
    if let Some(message) = parameter_name_refusal(&document) {
        return Some(message);
    }
    // Every value IN FORCE is held to its own declaration: the `Parameters` section is the
    // template's input contract, so a value outside what the declaration admits — one the
    // request supplied, or the declaration's own `Default` where it supplied none, since both
    // are values of that parameter — is refused before anything is provisioned
    // ([`super::parameter_constraints`], the ONE place that rule lives).
    if let Some(message) = super::parameter_constraints::refusal(body, supplied) {
        return Some(message);
    }
    // The directives are read from the SUBMITTED document: whether a template uses one is a
    // question about what the author wrote, not about what the expansion left behind.
    let declares_extensions = template::body_declared_transforms(body)
        .iter()
        .any(|name| name == template::LANGUAGE_EXTENSIONS);
    if let Some(message) = directive_refusal(&document, declares_extensions) {
        return Some(message);
    }
    // The rules below are about the document CloudFormation DEPLOYS. A template with no
    // processed stage has nothing for them to read, and the operation that submitted it
    // fails the stack with the transform's own reason instead.
    let deployed = processed.body(body)?;
    // A body past a documented per-template quota is refused by that rule, which the
    // submitting operation applies on its own; nothing here reports part of such a template.
    let template = template::parse(deployed)?;
    // A section whose own STRUCTURE the template format fixes is held to it: a `Description`
    // past its documented maximum, and a `Mappings` member that is no map. Both are
    // template-FORMAT errors decided from the document alone, which is why this pass and the
    // operations that only report on a body give the same verdict
    // ([`super::template::format_refusal`]).
    if let Some(message) = template::format_refusal(deployed, &template) {
        return Some(message);
    }
    // An export name is the handle OTHER stacks import by, so what it may be computed from is
    // fixed: an output whose `Export.Name` reads a resource, a value another stack exports or
    // the Region's Availability Zones is refused for that name ([`export_name_refusal`]).
    if let Some(message) = export_name_refusal(deployed, &template) {
        return Some(message);
    }
    if let Some(message) = arity_refusal(&template) {
        return Some(message);
    }
    if let Some(message) = unresolved_refusal(&template) {
        return Some(message);
    }
    // What a `Mappings` lookup resolves to is EVALUATED work: the values the deployment would
    // run with decide it, so only the pass over a body submitted FOR DEPLOYMENT makes it.
    if reading == Pass::Submitted {
        // What an expression inside a lookup resolves to: the parameters in force — the values
        // the request passed, over the template's own declared defaults — and the pseudo
        // parameters whose value this request fixes.
        let mut values = template.parameter_defaults();
        for (key, value) in supplied {
            values.insert(key.clone(), value.clone());
        }
        let resolved = Resolution {
            parameters: &values,
            region,
        };
        if let Some(message) = mapping_refusal(&template, &resolved) {
            return Some(message);
        }
    }
    cycle_refusal(&template)
}

/// `Some(message)` for a `Parameters` section whose key is not a name a parameter may be
/// given: "the parameter's logical name" is alphanumeric, so anything else — a
/// language-extensions directive among them — is no parameter declaration at all.
fn parameter_name_refusal(document: &Value) -> Option<String> {
    let parameters = document.get("Parameters")?.as_object()?;
    let name = parameters
        .keys()
        .find(|name| !name.chars().all(|character| character.is_ascii_alphanumeric()))?;
    Some(format!(
        "Template format error: Parameter name {name} is non alphanumeric."
    ))
}

/// What an expression inside a template resolves against where the REQUEST decides it.
struct Resolution<'a> {
    /// The parameter values in force: what the request passed, over the declared defaults.
    parameters: &'a BTreeMap<String, String>,
    /// The Region the request was signed for, which is what `AWS::Region` stands for.
    region: &'a str,
}

impl Resolution<'_> {
    /// What `{"Ref": "<name>"}` stands for, or `None` for a name whose value this request
    /// does not fix — which leaves the expression one this pass has nothing to say about.
    fn value_of(&self, name: &str) -> Option<String> {
        match name {
            "AWS::Region" => Some(self.region.to_string()),
            "AWS::Partition" => Some(template::PARTITION.to_string()),
            "AWS::AccountId" => Some(::emulator_server::emulator_account_id().to_string()),
            name => self.parameters.get(name).cloned(),
        }
    }
}

// ---------------------------------------------------------------------------
// The language-extensions directives: which template may use them, and where
// ---------------------------------------------------------------------------

/// `Some(message)` when the submitted document uses a function the
/// `AWS::LanguageExtensions` transform defines without declaring that transform, or uses
/// `Fn::ForEach` in a section the transform does not define it for.
fn directive_refusal(document: &Value, declares_extensions: bool) -> Option<String> {
    let sections = document.as_object()?;
    // The DYNAMIC `Fn::GetAtt` first, wherever it is: a template that names a resource with
    // an expression is refused for the arguments that function takes, which is the verdict
    // whatever else the template also uses.
    if !declares_extensions
        && sections
            .values()
            .any(|node| holds_dynamic_get_att(node, 0))
    {
        return Some(GET_ATT_PARAMETERS.to_string());
    }
    sections
        .values()
        .find_map(|node| directives_in(node, declares_extensions, 0))
}

/// Whether a subtree names a resource's attribute with an expression rather than with the
/// resource's own logical name.
fn holds_dynamic_get_att(node: &Value, depth: usize) -> bool {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return false;
    }
    match node {
        Value::Array(items) => items
            .iter()
            .any(|item| holds_dynamic_get_att(item, depth + 1)),
        Value::Object(members) => members.iter().any(|(key, member)| {
            (key == template::FN_GET_ATT && is_dynamic_get_att(member))
                || holds_dynamic_get_att(member, depth + 1)
        }),
        _ => false,
    }
}

/// [`directive_refusal`] over one section's subtree.
fn directives_in(node: &Value, declares_extensions: bool, depth: usize) -> Option<String> {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return None;
    }
    match node {
        Value::Array(items) => items
            .iter()
            .find_map(|item| directives_in(item, declares_extensions, depth + 1)),
        Value::Object(members) => {
            for (key, member) in members {
                if !declares_extensions
                    && (key.starts_with(template::FOR_EACH_PREFIX)
                        || key == template::FN_LENGTH
                        || key == template::FN_TO_JSON_STRING)
                {
                    return Some(unsupported_function(key));
                }
                if let Some(message) = directives_in(member, declares_extensions, depth + 1) {
                    return Some(message);
                }
            }
            None
        }
        _ => None,
    }
}

/// Whether an `Fn::GetAtt` argument names its resource with an expression rather than with
/// the logical name itself.
fn is_dynamic_get_att(argument: &Value) -> bool {
    match argument {
        Value::Array(parts) => parts.first().is_some_and(|first| !first.is_string()),
        Value::String(_) => false,
        // Neither documented form, which the arity rule reports.
        _ => false,
    }
}

/// How `Fn::ForEach` in a section the declared transform does not define it for FAILS the
/// deployment: the request is accepted — the template is well-formed and the loop IS one of
/// that transform's functions — and the transform then cannot apply it where it stands,
/// which is the one thing a transform's failure says.
fn for_each_not_supported() -> String {
    format!(
        "Transform {} failed with: Error parsing the template.",
        template::LANGUAGE_EXTENSIONS
    )
}

/// How an `Fn::GetAtt` that does not name a resource and an attribute is refused.
const GET_ATT_PARAMETERS: &str = "Template error: every Fn::GetAtt object requires two \
                                  non-empty parameters, the resource name and the resource \
                                  attribute";

// ---------------------------------------------------------------------------
// Arity: the arguments each intrinsic function takes
// ---------------------------------------------------------------------------

/// `Some(message)` for an intrinsic function the deployed document spells with arguments
/// that are not the ones it takes.
fn arity_refusal(template: &Template) -> Option<String> {
    sections(template).find_map(|(_, node)| arity_in(&node, 0))
}

/// [`arity_refusal`] over one node.
fn arity_in(node: &Value, depth: usize) -> Option<String> {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return None;
    }
    match node {
        Value::Array(items) => items.iter().find_map(|item| arity_in(item, depth + 1)),
        Value::Object(members) => {
            // An intrinsic function is a single-member object whose name is the function's,
            // and the functions a template may use are the documented ones: anything else
            // under an `Fn::` name is no function CloudFormation resolves.
            if members.len() == 1 {
                if let Some(name) = members.keys().next() {
                    if name.starts_with(FUNCTION_PREFIX) && !is_documented_function(name) {
                        return Some(unsupported_function(name));
                    }
                }
            }
            for (key, member) in members {
                let arity = match key.as_str() {
                    // "[MapName, TopLevelKey, SecondLevelKey]", plus the optional fourth
                    // argument that carries the lookup's `DefaultValue`.
                    template::FN_FIND_IN_MAP => {
                        let length = member.as_array().map_or(0, Vec::len);
                        (!(3..=4).contains(&length)).then(|| FIND_IN_MAP_PARAMETERS.to_string())
                    }
                    // "[logicalNameOfResource, attributeName]", or the short form's one
                    // dotted string.
                    template::FN_GET_ATT => {
                        let documented = match member {
                            Value::Array(parts) => parts.len() >= 2,
                            Value::String(dotted) => dotted.contains('.'),
                            _ => false,
                        };
                        (!documented).then(|| GET_ATT_PARAMETERS.to_string())
                    }
                    // "[condition_name, value_if_true, value_if_false]".
                    template::FN_IF => {
                        let length = member.as_array().map_or(0, Vec::len);
                        (length != 3).then(|| IF_PARAMETERS.to_string())
                    }
                    // "Fn::Length returns the number of elements within an array or an
                    // intrinsic function that returns an array": an argument that is
                    // neither can be no list at all.
                    template::FN_LENGTH => (!countable(member)).then(|| LENGTH_PARAMETERS.to_string()),
                    _ => boolean_arity(key, member),
                };
                if let Some(message) = arity {
                    return Some(message);
                }
                if let Some(message) = arity_in(member, depth + 1) {
                    return Some(message);
                }
            }
            None
        }
        _ => None,
    }
}

/// `Some(message)` when a boolean operator is spelled with a number of conditions it does
/// not take, and `None` for anything that is not one.
fn boolean_arity(name: &str, argument: &Value) -> Option<String> {
    let length = argument.as_array().map_or(0, Vec::len);
    match name {
        // "Fn::Equals: [value_1, value_2]".
        "Fn::Equals" => (length != 2).then(|| condition_parameters(name, "two values")),
        // "Fn::Not: [condition]".
        "Fn::Not" => (length != 1).then(|| condition_parameters(name, "one condition")),
        // "You can use ... a minimum of 2 conditions and a maximum of 10 conditions."
        "Fn::And" | "Fn::Or" => (!(2..=usize::from(MAX_BOOLEAN_CONDITIONS)).contains(&length))
            .then(|| {
                condition_parameters(name, &format!("2 to {MAX_BOOLEAN_CONDITIONS} conditions"))
            }),
        _ => None,
    }
}

/// The most conditions one `Fn::And` or `Fn::Or` takes: "a maximum of 10 conditions".
const MAX_BOOLEAN_CONDITIONS: u8 = 10;

/// How a condition function spelled with the wrong number of arguments is refused.
fn condition_parameters(name: &str, takes: &str) -> String {
    format!("Template error: every {name} object requires a list of {takes}")
}

/// The prefix every intrinsic function but `Ref` is named with.
const FUNCTION_PREFIX: &str = "Fn::";

/// The intrinsic functions a template may use, as the "Intrinsic function reference"
/// publishes them: the value functions, the condition functions, the ones the
/// `AWS::LanguageExtensions` transform defines (held to that declaration by
/// [`directive_refusal`]) and the rule functions the `Rules` section asserts with.
const DOCUMENTED_FUNCTIONS: [&str; 25] = [
    "Fn::Base64",
    "Fn::GetAtt",
    "Fn::GetAZs",
    "Fn::ImportValue",
    "Fn::Join",
    "Fn::Select",
    "Fn::Split",
    "Fn::Sub",
    "Ref",
    "Fn::Cidr",
    "Fn::FindInMap",
    "Fn::Transform",
    "Fn::If",
    "Fn::And",
    "Fn::Equals",
    "Fn::Not",
    "Fn::Or",
    "Fn::Contains",
    "Fn::EachMemberEquals",
    "Fn::EachMemberIn",
    "Fn::RefAll",
    "Fn::ValueOf",
    "Fn::ValueOfAll",
    "Fn::Length",
    "Fn::ToJsonString",
];

/// Whether `name` is one of the functions a template may use: a documented one, one the
/// service advertises as supported and resolves itself ([`SERVICE_RESOLVED_FUNCTIONS`]), or
/// the loop directive, which carries its own loop name after the function's
/// ([`super::template::FOR_EACH_PREFIX`]).
fn is_documented_function(name: &str) -> bool {
    DOCUMENTED_FUNCTIONS.contains(&name)
        || SERVICE_RESOLVED_FUNCTIONS.contains(&name)
        || name.starts_with(template::FOR_EACH_PREFIX)
}

/// The functions the service resolves ITSELF that are not in the published "Intrinsic
/// function reference", and that a template may therefore use even though that reference
/// does not list them: the service advertises them as supported ([`ADVERTISED_FUNCTIONS`]),
/// and a function advertised as supported is one a template may spell.
///
/// Only the ones this service really resolves are here: `Fn::GetStackOutput`, whose value is
/// one output of another stack of the account and Region
/// ([`super::stack_outputs`]). The other advertised names this service resolves no value
/// for stay refused by name, since a template may not deploy with a function nothing
/// resolved.
const SERVICE_RESOLVED_FUNCTIONS: [&str; 1] = [super::stack_outputs::FN_GET_STACK_OUTPUT];

/// The roster the "unsupported function" refusal advertises, in the order the service lists
/// it: the three functions the SERVICE resolves for itself — one of which a template may
/// really use ([`SERVICE_RESOLVED_FUNCTIONS`]), while the other two appear because the
/// refusal reports the whole set the service resolves — then the value functions, the
/// condition functions — `Condition` among them, which is how a condition names another —
/// then the rule functions, and `Fn::Sub` and `Fn::Cidr` last.
///
/// FIXED text, independent of the template the refusal answers: nothing of the request
/// reaches it but the offending function's name. That is why it is a roster of its own
/// rather than a rendering of [`DOCUMENTED_FUNCTIONS`] — the service advertises one list,
/// while what a template may actually USE additionally depends on the transforms it
/// declares, so the directives `AWS::LanguageExtensions` defines ([`template::FN_LENGTH`],
/// [`template::FN_TO_JSON_STRING`], the loop) are NOT advertised: a response refusing one
/// of them may not list it as supported.
const ADVERTISED_FUNCTIONS: [&str; 26] = [
    "Fn::GetStackOutput",
    "Fn::AccountIdFromAlias",
    "Fn::GetConstantAZs",
    "Fn::Base64",
    "Fn::GetAtt",
    "Fn::GetAZs",
    "Fn::ImportValue",
    "Fn::Join",
    "Fn::Split",
    "Fn::FindInMap",
    "Fn::Select",
    "Ref",
    "Fn::Equals",
    "Fn::If",
    "Fn::Not",
    "Condition",
    "Fn::And",
    "Fn::Or",
    "Fn::Contains",
    "Fn::EachMemberEquals",
    "Fn::EachMemberIn",
    "Fn::ValueOf",
    "Fn::ValueOfAll",
    "Fn::RefAll",
    "Fn::Sub",
    "Fn::Cidr",
];

/// How a function no template may use is refused, naming it and the functions the service
/// advertises as supported — which is what a caller who misspelled one needs.
fn unsupported_function(name: &str) -> String {
    format!(
        "Template Error: Encountered unsupported function: {name} Supported functions are: \
         [{}]",
        ADVERTISED_FUNCTIONS.join(", ")
    )
}

/// Whether an `Fn::Length` argument is something that can be a list: a list itself, or an
/// intrinsic function — one member named `Ref` or `Fn::<Name>` — which may return one.
fn countable(argument: &Value) -> bool {
    match argument {
        Value::Array(_) => true,
        Value::Object(members) => {
            members.len() == 1
                && members
                    .keys()
                    .next()
                    .is_some_and(|name| name == "Ref" || name.starts_with("Fn::"))
        }
        _ => false,
    }
}

/// How an `Fn::Length` whose argument can be no list is refused.
const LENGTH_PARAMETERS: &str = "Template error: every Fn::Length object requires a list, \
                                 or a function that returns one";

/// How an `Fn::FindInMap` that does not name a map and two keys is refused. Fixed text, as
/// the service words it: the three parameters are named as that refusal names them.
const FIND_IN_MAP_PARAMETERS: &str = "Template error: every Fn::FindInMap object requires \
                                      three parameters, the map name, map key and the \
                                      attribute for return value";

/// How an `Fn::If` that does not name a condition and both values is refused.
const IF_PARAMETERS: &str = "Template error: every Fn::If object requires three \
                             parameters, the condition name and the values to return when \
                             it holds and when it does not";

// ---------------------------------------------------------------------------
// Unresolved dependencies: names the template does not declare
// ---------------------------------------------------------------------------

/// `Some(message)` when a section of the deployed document names something the template
/// does not declare: a `Ref` or an `Fn::GetAtt` naming no parameter and no resource, a
/// `Condition` naming no declared condition, or a `DependsOn` naming no declared resource.
///
/// The section is named in the refusal, because that is where the author has to look.
///
/// A walk of its own rather than [`super::template::references`]: that one collects the
/// names a value REFERS to for the dependency relation, where a false positive is harmless
/// (it is filtered to the declared resources), while this one has to decide a REFUSAL — so
/// it honours what `Fn::Sub`'s own variable map supplies, reads condition names, and carries
/// the section each reference sits in, none of which that relation needs.
fn unresolved_refusal(template: &Template) -> Option<String> {
    let parameters: BTreeSet<&str> = template
        .parameters
        .iter()
        .map(|parameter| parameter.key.as_str())
        .collect();
    let resources: BTreeSet<&str> = template
        .resources
        .iter()
        .map(|resource| resource.logical_id.as_str())
        .collect();
    let conditions: BTreeSet<&str> = declared_conditions(template);
    let declared = Declared {
        parameters: &parameters,
        resources: &resources,
        conditions: &conditions,
    };
    // A resource's or an output's own `Condition` names a condition the same way `Fn::If`
    // does, so it is held to the same rule.
    for resource in &template.resources {
        if let Some(condition) = &resource.condition {
            if !conditions.contains(condition.as_str()) {
                return Some(condition_not_defined(condition));
            }
        }
    }
    for output in &template.outputs {
        if let Some(condition) = &output.condition {
            if !conditions.contains(condition.as_str()) {
                return Some(condition_not_defined(condition));
            }
        }
    }
    sections(template)
        .find_map(|(section, node)| unresolved_in(&node, section, &declared, 0))
        // A `DependsOn` names RESOURCES of the same template — "with the DependsOn
        // attribute you can specify that the creation of a specific resource follows
        // another" — and it is read off the resource's own attributes rather than out of an
        // expression, so the walk above cannot see it. A name the template declares no
        // resource under is the same unresolved dependency of the `Resources` section a
        // `Ref` to an undeclared name is: the order it asks for cannot be resolved, so the
        // template is refused rather than deployed.
        .or_else(|| dangling_depends_on(template, &resources))
}

/// `Some(name)` for the first `DependsOn` entry anywhere in `template` naming no declared
/// resource.
///
/// Both spellings the attribute takes are read — "DependsOn: String | List of String" — and
/// an attribute that is neither (which CloudFormation refuses for its own form) names no
/// resource here. A declaration that depends on itself is as unresolvable as one that names
/// nothing, but it IS a declared resource, so it is the cycle rule that refuses it
/// ([`cycle_refusal`]) and not this one.
fn dangling_depends_on(template: &Template, resources: &BTreeSet<&str>) -> Option<String> {
    template.resources.iter().find_map(|resource| {
        depends_on_names(&resource.declaration)
            .into_iter()
            .find(|name| !resources.contains(name.as_str()))
            .map(|name| unresolved_dependency_in(&name, RESOURCES))
    })
}

/// The logical ids one resource declaration's `DependsOn` names, in the order it names them.
fn depends_on_names(declaration: &Value) -> Vec<String> {
    match declaration.get(DEPENDS_ON) {
        Some(Value::String(name)) => vec![name.clone()],
        Some(Value::Array(names)) => names
            .iter()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        _ => Vec::new(),
    }
}

/// The resource attribute that states a creation order of the author's own.
const DEPENDS_ON: &str = "DependsOn";

/// The names a template declares, which is what every reference in it has to name.
struct Declared<'a> {
    parameters: &'a BTreeSet<&'a str>,
    resources: &'a BTreeSet<&'a str>,
    conditions: &'a BTreeSet<&'a str>,
}

/// The conditions a template's `Conditions` section declares.
fn declared_conditions(template: &Template) -> BTreeSet<&str> {
    template
        .conditions
        .as_object()
        .map(|conditions| conditions.keys().map(String::as_str).collect())
        .unwrap_or_default()
}

/// [`unresolved_refusal`] over one node of `section`.
fn unresolved_in(
    node: &Value,
    section: &str,
    declared: &Declared<'_>,
    depth: usize,
) -> Option<String> {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return None;
    }
    match node {
        Value::Array(items) => items
            .iter()
            .find_map(|item| unresolved_in(item, section, declared, depth + 1)),
        Value::Object(members) => {
            for (key, member) in members {
                match (key.as_str(), member) {
                    // "Ref returns the value of the specified parameter or resource", so a
                    // name that is neither — nor a pseudo parameter — resolves to nothing.
                    ("Ref", Value::String(name)) => {
                        if !declared.parameters.contains(name.as_str())
                            && !declared.resources.contains(name.as_str())
                            && !PSEUDO_PARAMETERS.contains(&name.as_str())
                        {
                            return Some(unresolved_dependency_in(name, section));
                        }
                    }
                    // "Fn::GetAtt returns the value of an attribute from a resource in the
                    // template": the first argument names a RESOURCE, never a parameter, and
                    // a name the template declares no resource under is reported for the
                    // INTRINSIC that read it.
                    (template::FN_GET_ATT, argument) => {
                        if let Some(name) = get_att_resource(argument) {
                            if !declared.resources.contains(name) {
                                return Some(get_att_undefined_resource(name));
                            }
                        }
                    }
                    // "Fn::If: [condition_name, ...]" and `{"Condition": "<name>"}` both
                    // name a condition the `Conditions` section has to declare.
                    (template::FN_IF, Value::Array(branches)) => {
                        if let Some(name) = branches.first().and_then(Value::as_str) {
                            if !declared.conditions.contains(name) {
                                return Some(condition_not_defined(name));
                            }
                        }
                    }
                    ("Condition", Value::String(name)) => {
                        if !declared.conditions.contains(name.as_str()) {
                            return Some(condition_not_defined(name));
                        }
                    }
                    // "Fn::Sub substitutes variables in an input string": a `${Name}` the
                    // substitution's own variable map does not supply names what a `Ref`
                    // names, and a `${Name.Attribute}` what an `Fn::GetAtt` does.
                    ("Fn::Sub", argument) => {
                        if let Some(message) = substitution_refusal(argument, section, declared)
                        {
                            return Some(message);
                        }
                    }
                    _ => {}
                }
                if let Some(message) = unresolved_in(member, section, declared, depth + 1) {
                    return Some(message);
                }
            }
            None
        }
        _ => None,
    }
}

/// `Some(message)` for an `Fn::Sub` whose source string names something neither its own
/// variable map nor the template declares.
fn substitution_refusal(
    argument: &Value,
    section: &str,
    declared: &Declared<'_>,
) -> Option<String> {
    // "Fn::Sub: [ String, { Var1Name: Var1Value, ... } ]", or the string on its own.
    let (source, variables) = match argument {
        Value::String(source) => (source.as_str(), None),
        Value::Array(parts) => (
            parts.first()?.as_str()?,
            parts.get(1).and_then(Value::as_object),
        ),
        _ => return None,
    };
    for (name, path) in template::substitution_names(source) {
        // "Fn::Sub: [ String, { Var1Name: Var1Value, … } ]": a variable the substitution
        // SUPPLIES is substituted whatever it is spelled like, so a key carrying the whole
        // dotted reference supplies that reference as much as a bare name supplies a bare
        // one.
        if supplied(variables, &name, path.as_deref()) {
            continue;
        }
        match path {
            // `${Name.Attribute}` reads an attribute off a RESOURCE — it is an `Fn::GetAtt`
            // by another spelling — so a reference whose own first segment names no declared
            // resource is no resource attribute at all, and is reported for the reference it
            // spelled ([`invalid_resource_attribute`]).
            Some(reference) => {
                if !declared.resources.contains(name.as_str()) {
                    return Some(invalid_resource_attribute(&reference));
                }
            }
            None => {
                if !declared.parameters.contains(name.as_str())
                    && !declared.resources.contains(name.as_str())
                    && !PSEUDO_PARAMETERS.contains(&name.as_str())
                {
                    return Some(unresolved_dependency_in(&name, section));
                }
            }
        }
    }
    None
}

/// Whether the substitution's own variable map supplies this variable: by the name as
/// written, and — for a dotted reference — by the whole reference, which is how such a key is
/// spelled in the map.
fn supplied(
    variables: Option<&::serde_json::Map<String, Value>>,
    name: &str,
    reference: Option<&str>,
) -> bool {
    variables.is_some_and(|supplied| {
        supplied.contains_key(name)
            || reference.is_some_and(|reference| supplied.contains_key(reference))
    })
}

/// `Some(message)` for an `Fn::Sub` anywhere in `template` whose dotted reference names no
/// resource attribute of it — the one rule of a substitution that is about the REFERENCE
/// rather than about an unresolved name, so an operation that validates no dependencies can
/// still be held to it.
///
/// Read over the same sections every other rule of this pass reads ([`sections`]), so a
/// reference in any of them is reported.
pub fn substitution_attribute_refusal(template: &Template) -> Option<String> {
    let resources: BTreeSet<&str> = template
        .resources
        .iter()
        .map(|resource| resource.logical_id.as_str())
        .collect();
    sections(template).find_map(|(_, node)| attribute_reference_in(&node, &resources, 0))
}

/// [`substitution_attribute_refusal`] over one node.
fn attribute_reference_in(
    node: &Value,
    resources: &BTreeSet<&str>,
    depth: usize,
) -> Option<String> {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return None;
    }
    match node {
        Value::Array(items) => items
            .iter()
            .find_map(|item| attribute_reference_in(item, resources, depth + 1)),
        Value::Object(members) => members.iter().find_map(|(key, member)| {
            if key == "Fn::Sub" {
                if let Some(message) = dotted_reference_refusal(member, resources) {
                    return Some(message);
                }
            }
            attribute_reference_in(member, resources, depth + 1)
        }),
        _ => None,
    }
}

/// `Some(message)` for one `Fn::Sub` whose source string carries a dotted reference naming
/// no declared resource, honouring the substitution's own variable map exactly as
/// [`substitution_refusal`] does.
fn dotted_reference_refusal(argument: &Value, resources: &BTreeSet<&str>) -> Option<String> {
    let (source, variables) = match argument {
        Value::String(source) => (source.as_str(), None),
        Value::Array(parts) => (
            parts.first()?.as_str()?,
            parts.get(1).and_then(Value::as_object),
        ),
        _ => return None,
    };
    template::substitution_names(source)
        .into_iter()
        .find_map(|(name, path)| {
            let reference = path?;
            if supplied(variables, &name, Some(&reference)) {
                return None;
            }
            (!resources.contains(name.as_str())).then(|| invalid_resource_attribute(&reference))
        })
}

/// How an `Fn::Sub` whose dotted reference is no resource attribute is refused.
///
/// A TEMPLATE error rather than a template-FORMAT one, and it names the FUNCTION instance
/// rather than the section, exactly as the `Fn::GetAtt` form of the same mistake does
/// ([`get_att_undefined_resource`]): the document is well formed and the variable is well
/// spelled — what is wrong is the reference it reads, which is the one thing the message
/// states.
fn invalid_resource_attribute(reference: &str) -> String {
    format!("Template error: instance of Fn::Sub references invalid resource attribute {reference}")
}

/// The logical name an `Fn::GetAtt` reads its attribute off, or `None` when the argument
/// names it with an expression rather than with the name itself (the language-extensions
/// form, resolved from the parameters in force rather than from the document).
fn get_att_resource(argument: &Value) -> Option<&str> {
    match argument {
        Value::Array(parts) => parts.first()?.as_str(),
        Value::String(dotted) => Some(template::resource_of(dotted)),
        _ => None,
    }
}

/// How a reference to a name the template does not declare is refused, naming the section
/// the reference is in.
fn unresolved_dependency_in(name: &str, section: &str) -> String {
    format!(
        "Template format error: Unresolved resource dependencies [{name}] in the {section} \
         block of the template"
    )
}

/// How an `Fn::GetAtt` reading an attribute off a resource the template does not declare is
/// refused.
///
/// This is a TEMPLATE error rather than a template-FORMAT one, and it names the function
/// rather than the section: the document is well formed and the reference is well spelled —
/// what is wrong is the resource the instance of the function names, which is the one thing
/// the message states.
fn get_att_undefined_resource(name: &str) -> String {
    format!("Template error: instance of Fn::GetAtt references undefined resource {name}")
}

/// How a reference to a condition the template does not declare is refused.
fn condition_not_defined(name: &str) -> String {
    format!("Template format error: Condition {name} is not defined.")
}

// ---------------------------------------------------------------------------
// Export names: what an output's `Export.Name` may be computed from
// ---------------------------------------------------------------------------

/// `Some(message)` for an output whose `Export.Name` is computed from something an export name
/// may not depend on, naming the output that declared it.
///
/// An export name is the handle OTHER stacks import a value by, so CloudFormation fixes it
/// from the template, the parameter values in force and the stack's own pseudo parameters
/// alone: "the Name field of Export must not depend on any resources, imported values, or
/// Fn::GetAZs". A `Ref` to a declared RESOURCE, any `Fn::GetAtt` (whose first argument names
/// nothing but a resource), an `Fn::ImportValue` and an `Fn::GetAZs` are each such a
/// dependency, however deeply the name's expression nests one — while an `Fn::Sub` or an
/// `Fn::Join` over parameters and pseudo parameters is a name CloudFormation can fix, so those
/// are left alone.
///
/// The output reported is the FIRST one the body declares ([`super::declaration_order`]),
/// which is where its author reads it.
fn export_name_refusal(body: &str, template: &Template) -> Option<String> {
    let resources: BTreeSet<&str> = template
        .resources
        .iter()
        .map(|resource| resource.logical_id.as_str())
        .collect();
    super::declaration_order::as_declared(
        body,
        super::declaration_order::OUTPUTS,
        &template.outputs,
        |output| output.key.as_str(),
    )
    .into_iter()
    .find(|output| {
        output
            .export_name
            .as_ref()
            .is_some_and(|name| reads_the_deployment(name, &resources, 0))
    })
    .map(|output| malformed_export_name(&output.key))
}

/// How an output whose `Export.Name` depends on one of them is refused: the OUTPUT is named —
/// that is the declaration whose author has to change it — followed by the clause stating what
/// such a name may not read, as the service words it.
fn malformed_export_name(key: &str) -> String {
    format!(
        "Template format error: Output {key} is malformed.  The Name field of Export must not \
         depend on any resources, imported values, or Fn::GetAZs."
    )
}

/// Whether an expression reads something an export name may not depend on: a resource the
/// template declares, a value another stack exports, or the Availability Zones of a Region.
fn reads_the_deployment(node: &Value, resources: &BTreeSet<&str>, depth: usize) -> bool {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return false;
    }
    match node {
        Value::Array(items) => items
            .iter()
            .any(|item| reads_the_deployment(item, resources, depth + 1)),
        Value::Object(members) => members.iter().any(|(key, member)| {
            let reads = match key.as_str() {
                // "Fn::GetAtt returns the value of an attribute from a resource in the
                // template", and the other two read what no template states at all.
                template::FN_GET_ATT | template::FN_IMPORT_VALUE | template::FN_GET_AZS => true,
                // "Ref returns the value of the specified parameter or resource": only a
                // name the template declares a RESOURCE under is a dependency on one.
                "Ref" => member
                    .as_str()
                    .is_some_and(|name| resources.contains(name)),
                // An `Fn::Sub` variable names what a `Ref` names (`${Name}`) or what an
                // `Fn::GetAtt` reads (`${Name.Attribute}`), so it is read the same way. What
                // the substitution's own variable map supplies is read below, as any other
                // node of the expression is.
                "Fn::Sub" => substitution_reads_a_resource(member, resources),
                _ => false,
            };
            reads || reads_the_deployment(member, resources, depth + 1)
        }),
        _ => false,
    }
}

/// Whether one `Fn::Sub`'s source string reads a resource: a `${Name.Attribute}` reference, or
/// a `${Name}` naming a declared resource, that the substitution's own variable map does not
/// supply.
fn substitution_reads_a_resource(argument: &Value, resources: &BTreeSet<&str>) -> bool {
    let (source, variables) = match argument {
        Value::String(source) => (source.as_str(), None),
        Value::Array(parts) => match parts.first().and_then(Value::as_str) {
            Some(source) => (source, parts.get(1).and_then(Value::as_object)),
            None => return false,
        },
        _ => return false,
    };
    template::substitution_names(source)
        .into_iter()
        .any(|(name, path)| {
            !supplied(variables, &name, path.as_deref())
                && (path.is_some() || resources.contains(name.as_str()))
        })
}

// ---------------------------------------------------------------------------
// Mappings: the lookups a template's `Fn::FindInMap`s really make
// ---------------------------------------------------------------------------

/// `Some(message)` for an `Fn::FindInMap` whose keys the `Mappings` section does not carry
/// and that names no `DefaultValue` to fall back to.
///
/// Only a lookup whose three keys the DOCUMENT decides is checked: a key an expression
/// supplies (a `Ref` to a parameter) is one the request decides, and a lookup nested inside
/// another is resolved here so the outer one can be checked too.
fn mapping_refusal(template: &Template, resolved: &Resolution<'_>) -> Option<String> {
    sections(template)
        .find_map(|(_, node)| mapping_in(&node, &template.mappings, resolved, 0))
}

/// [`mapping_refusal`] over one node.
fn mapping_in(
    node: &Value,
    mappings: &Value,
    resolved: &Resolution<'_>,
    depth: usize,
) -> Option<String> {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return None;
    }
    match node {
        Value::Array(items) => items
            .iter()
            .find_map(|item| mapping_in(item, mappings, resolved, depth + 1)),
        Value::Object(members) => {
            for (key, member) in members {
                if key == template::FN_FIND_IN_MAP {
                    if let Some(message) = lookup_refusal(member, mappings, resolved, depth) {
                        return Some(message);
                    }
                }
                if let Some(message) = mapping_in(member, mappings, resolved, depth + 1) {
                    return Some(message);
                }
            }
            None
        }
        _ => None,
    }
}

/// `Some(message)` when ONE `Fn::FindInMap`'s lookup misses and it carries no default.
///
/// The miss is diagnosed where it happens: a lookup naming a map the `Mappings` section does
/// not declare at all is reported for that MAP — the section is where its author has to add
/// it — while one whose map is declared but whose keys that map does not carry is reported
/// for the lookup it made.
fn lookup_refusal(
    argument: &Value,
    mappings: &Value,
    resolved: &Resolution<'_>,
    depth: usize,
) -> Option<String> {
    let keys = argument.as_array()?;
    if keys.len() < 3 {
        // Its arity is reported by the rule that owns arity.
        return None;
    }
    let map = static_text(&keys[0], mappings, resolved, depth)?;
    let top = static_text(&keys[1], mappings, resolved, depth)?;
    let second = static_text(&keys[2], mappings, resolved, depth)?;
    if found_in(mappings, &map, &top, &second).is_some() || carries_default(keys) {
        return None;
    }
    if mappings.get(map.as_str()).is_none() {
        return Some(mapping_not_present(&map));
    }
    Some(format!(
        "Template error: Unable to get mapping for {map}::{top}::{second}"
    ))
}

/// How a lookup into a map the template does not declare is refused: the map it named, taken
/// from the intrinsic, inside fixed text naming the section that would have to declare it.
fn mapping_not_present(map: &str) -> String {
    format!(
        "Template error: Mapping named '{map}' is not present in the 'Mappings' section of \
         template."
    )
}

/// Whether an `Fn::FindInMap`'s arguments name a `DefaultValue` for the lookup to fall back
/// to: "the value to be returned if the key is not found".
fn carries_default(keys: &[Value]) -> bool {
    keys.get(3)
        .and_then(|options| options.get(template::FIND_IN_MAP_DEFAULT_VALUE))
        .is_some()
}

/// The value a `Mappings` lookup finds, or `None` when the section carries no such entry.
fn found_in<'a>(mappings: &'a Value, map: &str, top: &str, second: &str) -> Option<&'a Value> {
    let found = mappings.get(map)?.get(top)?.get(second)?;
    (!found.is_null()).then_some(found)
}

/// The text a scalar template value spells, and nothing for a value that is not one.
fn scalar_text(value: &Value) -> Option<String> {
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Null | Value::Array(_) | Value::Object(_) => None,
    }
}

/// The text an `Fn::FindInMap` key spells, where the TEMPLATE and the REQUEST decide it: a
/// literal, a `Ref` whose value this request fixes, or another `Fn::FindInMap` whose own
/// lookup resolves. `None` for a key something else decides, which this check has nothing
/// to say about.
fn static_text(
    value: &Value,
    mappings: &Value,
    resolved: &Resolution<'_>,
    depth: usize,
) -> Option<String> {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return None;
    }
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Object(members) if members.len() == 1 => {
            let (name, argument) = members.iter().next()?;
            match name.as_str() {
                "Ref" => resolved.value_of(argument.as_str()?),
                template::FN_FIND_IN_MAP => {
                    let keys = argument.as_array()?;
                    if keys.len() < 3 {
                        return None;
                    }
                    let map = static_text(&keys[0], mappings, resolved, depth + 1)?;
                    let top = static_text(&keys[1], mappings, resolved, depth + 1)?;
                    let second = static_text(&keys[2], mappings, resolved, depth + 1)?;
                    match found_in(mappings, &map, &top, &second).and_then(scalar_text) {
                        Some(found) => Some(found),
                        None => keys
                            .get(3)
                            .and_then(|options| {
                                options.get(template::FIND_IN_MAP_DEFAULT_VALUE)
                            })
                            .and_then(|default| {
                                static_text(default, mappings, resolved, depth + 1)
                            }),
                    }
                }
                _ => None,
            }
        }
        _ => None,
    }
}

// ---------------------------------------------------------------------------
// Dependency cycles
// ---------------------------------------------------------------------------

/// `Some(message)` when the declared resources depend on each other in a cycle, naming the
/// resources that form it: no creation order satisfies such a template, so CloudFormation
/// refuses it instead of creating a resource whose references cannot be resolved.
///
/// The members are listed in the order the resource INDEX yields them
/// ([`as_the_index_yields_them`]) — a cycle has no first member of its own, every one of
/// them being reached from every other, so the list is not the chain the walk happened to
/// take. Everything that decides it comes from the request, namely its logical ids, so one
/// body is refused with one list on every run and whatever order the relation was walked
/// in. The order a body DECLARES its resources in decides nothing here, and is not
/// available to this rule anyway: a template is read into a document whose object members
/// are sorted ([`super::template::document`]), so the declarations' own order is gone
/// before any rule sees them.
fn cycle_refusal(template: &Template) -> Option<String> {
    let dependencies = template::direct_dependencies(template);
    let cycle = cycle_through(&dependencies)?;
    Some(format!(
        "Circular dependency between resources: [{}]",
        as_the_index_yields_them(&cycle).join(", ")
    ))
}

/// How many slots the index that orders the list holds.
///
/// A resource graph is keyed by logical id, and the list is walked off that index slot by
/// slot. Sixteen slots is the width recorded refusals are consistent with, and the width
/// such an index keeps for a cycle of the size one template may declare.
const INDEX_SLOTS: u32 = 16;

/// One cycle's logical ids as the refusal lists them: in the order the index they are keyed
/// by hands them out, which is by slot ([`index_slot`]) rather than by id.
///
/// Recorded behaviour, and NOT the ids' own sort order: one recorded refusal lists its pair
/// of ids ascending and another lists its pair descending, and both are the order an index
/// keyed by those ids' hashes walks them in. Two ids that share a slot are listed in id
/// order, the one tie-break the request does not decide — the order they were keyed in is
/// the body's declaration order, which no rule here can see.
fn as_the_index_yields_them(cycle: &[String]) -> Vec<String> {
    let mut listed = cycle.to_vec();
    listed.sort_by(|left, right| {
        index_slot(left)
            .cmp(&index_slot(right))
            .then_with(|| left.cmp(right))
    });
    listed
}

/// The slot the index puts one logical id in: its hash mixed with its own high half — an
/// index spreads the high bits down before it masks, or two ids differing only above the
/// mask would land in one slot — and then masked to the slot count.
fn index_slot(logical_id: &str) -> u32 {
    let hash = id_hash(logical_id);
    (hash ^ (hash >> 16)) & (INDEX_SLOTS - 1)
}

/// The hash a logical id is keyed by: every UTF-16 code unit of it folded into the running
/// value times 31, wrapping at 32 bits — the hash the service's own index keys a string by,
/// so the same id lands in the same slot here as there.
fn id_hash(logical_id: &str) -> u32 {
    logical_id.encode_utf16().fold(0u32, |hash, unit| {
        hash.wrapping_mul(31).wrapping_add(u32::from(unit))
    })
}

/// The resources of one cycle in the dependency relation, each followed by the resource it
/// depends on, or `None` for a relation that has none.
///
/// Walked from each resource in logical-id order, so one template yields one answer; which
/// resources this hands back is what the refusal names, and the order they are LISTED in is
/// [`as_the_index_yields_them`]'s.
fn cycle_through(dependencies: &BTreeMap<String, BTreeSet<String>>) -> Option<Vec<String>> {
    let mut settled: BTreeSet<&str> = BTreeSet::new();
    for start in dependencies.keys() {
        let mut path: Vec<&str> = Vec::new();
        if let Some(cycle) = walk(start, dependencies, &mut settled, &mut path) {
            return Some(cycle);
        }
    }
    None
}

/// Depth-first walk from `logical_id`, reporting the cycle it closes.
///
/// `settled` holds the resources already walked with no cycle under them, so the whole
/// relation is walked once however many resources point at the same one; `path` is the
/// chain being walked, which is what a cycle is read off. The recursion is bounded by the
/// resources one template may declare ([`super::template::MAX_RESOURCES_PER_TEMPLATE`]),
/// which the quota rule refuses a body past before anything reads it.
fn walk<'a>(
    logical_id: &'a str,
    dependencies: &'a BTreeMap<String, BTreeSet<String>>,
    settled: &mut BTreeSet<&'a str>,
    path: &mut Vec<&'a str>,
) -> Option<Vec<String>> {
    if let Some(position) = path.iter().position(|walked| *walked == logical_id) {
        return Some(path[position..].iter().map(|id| (*id).to_string()).collect());
    }
    if settled.contains(logical_id) {
        return None;
    }
    path.push(logical_id);
    if let Some(needs) = dependencies.get(logical_id) {
        for dependency in needs {
            if let Some(cycle) = walk(dependency, dependencies, settled, path) {
                return Some(cycle);
            }
        }
    }
    path.pop();
    settled.insert(logical_id);
    None
}

// ---------------------------------------------------------------------------
// The sections every rule above walks
// ---------------------------------------------------------------------------

/// The sections of a read template that carry expressions, each with the name the refusals
/// report it by: every resource's declaration WITHOUT its `Metadata` ("the Metadata
/// attribute ... CloudFormation does not evaluate intrinsic functions in it"), every
/// output's value and export name, and the `Conditions` section.
fn sections(template: &Template) -> impl Iterator<Item = (&'static str, Value)> + '_ {
    let resources = template
        .resources
        .iter()
        .map(|resource| (RESOURCES, evaluated_declaration(&resource.declaration)));
    let outputs = template.outputs.iter().flat_map(|output| {
        [
            Some((OUTPUTS, output.value.clone())),
            output.export_name.clone().map(|name| (OUTPUTS, name)),
        ]
    });
    resources
        .chain(outputs.flatten())
        .chain([(CONDITIONS, template.conditions.clone())])
}

/// One resource's declaration as the rules above read it: everything but `Metadata`, which
/// is the one resource attribute whose contents CloudFormation does not evaluate, so a key
/// or an object inside it that merely LOOKS like an intrinsic is no declaration about this
/// template.
fn evaluated_declaration(declaration: &Value) -> Value {
    let Some(members) = declaration.as_object() else {
        return declaration.clone();
    };
    Value::Object(
        members
            .iter()
            .filter(|(key, _)| key.as_str() != METADATA)
            .map(|(key, value)| (key.clone(), value.clone()))
            .collect(),
    )
}

/// The resource attribute CloudFormation evaluates nothing in.
const METADATA: &str = "Metadata";

// ---------------------------------------------------------------------------
// The `Rules` section
// ---------------------------------------------------------------------------

/// The template member that declares the rules a deployment's parameter values are
/// asserted against.
const RULES: &str = "Rules";

/// `Some(reason)` for a template a request may SUBMIT but whose deployment cannot be
/// carried out, worded as the failure that deployment records.
///
/// Three rules answer here rather than refusing the request, because none is a question
/// about the request's own shape:
///
/// * a `Rules` assertion the parameter values in force do not satisfy ([`rules_failure`]);
/// * an `Fn::ForEach` in a section the declared transform does not define it for — the
///   template is well-formed and the loop IS one of that transform's functions, so what
///   fails is the transform's own application;
/// * a document the SAM macro cannot process ([`super::sam::transform_failure`]) — the same
///   case: a transform either processes a template or fails on it, and its failure is the
///   deployment's, not the request's.
///
/// The operation that reads a failure here answers its caller normally and then ends the
/// operation it opened in the service's failure state, which for a create is followed by the
/// rollback that create's `OnFailure` asks for.
pub fn deployment_failure(body: &str, parameters: &BTreeMap<String, String>) -> Option<String> {
    for_each_placement_failure(body)
        .or_else(|| super::sam::transform_failure(body))
        .or_else(|| rules_failure(body, parameters))
}

/// `Some(reason)` for an `Fn::ForEach` the declared transform cannot apply where it stands.
fn for_each_placement_failure(body: &str) -> Option<String> {
    let document = template::document(body)?;
    let sections = document.as_object()?;
    sections.iter().find_map(|(section, node)| {
        (!FOR_EACH_SECTIONS.contains(&section.as_str()) && holds_for_each(node, 0))
            .then(for_each_not_supported)
    })
}

/// Whether a subtree carries an `Fn::ForEach` loop.
fn holds_for_each(node: &Value, depth: usize) -> bool {
    if depth > template::MAX_TEMPLATE_DEPTH {
        return false;
    }
    match node {
        Value::Array(items) => items.iter().any(|item| holds_for_each(item, depth + 1)),
        Value::Object(members) => members.iter().any(|(key, member)| {
            key.starts_with(template::FOR_EACH_PREFIX) || holds_for_each(member, depth + 1)
        }),
        _ => false,
    }
}

/// `Some(reason)` when a `Rules` assertion does not hold for the parameter values a
/// deployment runs with, worded as the failure that deployment records.
///
/// "Rules ... validates a parameter or a combination of parameters passed to a template
/// during a stack creation or stack update": the assertions are checked against the VALUES
/// in force, so this is a property of the deployment rather than of the request — the stack
/// operation that reads a failure here fails and rolls back, with the rule's own
/// `AssertDescription` as the reason the caller is told.
///
/// The sentence is the fixed text, the `AssertDescription` character for character, and this
/// service's own sentence end: the description is the SUBMITTER's text and is reproduced as it
/// was written, so one that ends in its own punctuation keeps it and the period terminating
/// CloudFormation's sentence lands after it. That is what leaves room for the rollback clause
/// to follow without re-punctuating anything ([`super::rollback_requested`]).
///
/// An assertion this service cannot evaluate from the parameters alone holds: a rule is a
/// guard on the values, and nothing here fails a deployment over an expression it did not
/// understand.
pub fn rules_failure(body: &str, parameters: &BTreeMap<String, String>) -> Option<String> {
    let document = template::document(body)?;
    let rules = document.get(RULES)?.as_object()?;
    for (_name, rule) in rules {
        // "RuleCondition (optional): Determines when a rule takes effect."
        let applies = match rule.get("RuleCondition") {
            Some(condition) => assertion_holds(condition, parameters, 0).unwrap_or(false),
            None => true,
        };
        if !applies {
            continue;
        }
        let Some(assertions) = rule.get("Assertions").and_then(Value::as_array) else {
            continue;
        };
        for assertion in assertions {
            let Some(assert) = assertion.get("Assert") else {
                continue;
            };
            if assertion_holds(assert, parameters, 0) == Some(false) {
                let description = assertion
                    .get("AssertDescription")
                    .and_then(Value::as_str)
                    .unwrap_or_default();
                return Some(format!(
                    "Parameter validation failed: assertion error: {description}."
                ));
            }
        }
    }
    None
}

/// Whether one rule expression holds for the parameter values in force, or `None` for one
/// this service cannot evaluate from those values alone.
///
/// The rule functions are the documented ones that read nothing but the parameters:
/// `Fn::Equals`, `Fn::Not`, `Fn::And`, `Fn::Or`, `Fn::Contains`, `Fn::EachMemberEquals` and
/// `Fn::EachMemberIn`. The ones that read the account's own resources (`Fn::RefAll`,
/// `Fn::ValueOf`, `Fn::ValueOfAll`) are not evaluated here, so an assertion using one is
/// not one this service judges.
fn assertion_holds(
    expression: &Value,
    parameters: &BTreeMap<String, String>,
    depth: usize,
) -> Option<bool> {
    if depth > template::MAX_INTRINSIC_DEPTH {
        return None;
    }
    if let Some(flag) = expression.as_bool() {
        return Some(flag);
    }
    let members = expression.as_object()?;
    if members.len() != 1 {
        return None;
    }
    let (name, argument) = members.iter().next()?;
    let arguments = argument.as_array();
    match name.as_str() {
        "Fn::Equals" => {
            let arguments = arguments?;
            if arguments.len() != 2 {
                return None;
            }
            let left = rule_value(&arguments[0], parameters, depth)?;
            let right = rule_value(&arguments[1], parameters, depth)?;
            Some(left == right)
        }
        "Fn::Not" => {
            let arguments = arguments?;
            if arguments.len() != 1 {
                return None;
            }
            Some(!assertion_holds(&arguments[0], parameters, depth + 1)?)
        }
        "Fn::And" | "Fn::Or" => {
            let arguments = arguments?;
            let all = name == "Fn::And";
            let mut holds = all;
            for argument in arguments {
                let evaluated = assertion_holds(argument, parameters, depth + 1)?;
                holds = if all {
                    holds && evaluated
                } else {
                    holds || evaluated
                };
            }
            Some(holds)
        }
        // "Fn::Contains: Returns true if a specified string matches at least one value in
        // a list": [list_of_strings, value]. The candidates are indexed once, so the
        // comparison is logarithmic in the list rather than a scan per member.
        "Fn::Contains" => {
            let arguments = arguments?;
            if arguments.len() != 2 {
                return None;
            }
            let candidates: BTreeSet<String> =
                rule_values(&arguments[0], parameters, depth)?.into_iter().collect();
            let value = rule_value(&arguments[1], parameters, depth)?;
            Some(candidates.contains(&value))
        }
        // "Fn::EachMemberEquals: Returns true if a specified string matches all values in
        // a list": [list_of_strings, value].
        "Fn::EachMemberEquals" => {
            let arguments = arguments?;
            if arguments.len() != 2 {
                return None;
            }
            let members = rule_values(&arguments[0], parameters, depth)?;
            let value = rule_value(&arguments[1], parameters, depth)?;
            Some(members.iter().all(|member| *member == value))
        }
        // "Fn::EachMemberIn: Returns true if each member in a list of strings matches at
        // least one value in a second list of strings":
        // [strings_to_check, strings_to_match]. The second list is indexed once, so the
        // work is the sum of the two lengths rather than their product.
        "Fn::EachMemberIn" => {
            let arguments = arguments?;
            if arguments.len() != 2 {
                return None;
            }
            let checked = rule_values(&arguments[0], parameters, depth)?;
            let against: BTreeSet<String> =
                rule_values(&arguments[1], parameters, depth)?.into_iter().collect();
            Some(checked.iter().all(|member| against.contains(member)))
        }
        _ => None,
    }
}

/// One value a rule expression compares: a literal, or a `Ref` to a parameter the
/// deployment carries a value for.
fn rule_value(
    value: &Value,
    parameters: &BTreeMap<String, String>,
    depth: usize,
) -> Option<String> {
    if depth > template::MAX_INTRINSIC_DEPTH {
        return None;
    }
    match value {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        Value::Object(members) if members.len() == 1 => {
            let (name, argument) = members.iter().next()?;
            (name == "Ref")
                .then(|| parameters.get(argument.as_str()?).cloned())
                .flatten()
        }
        _ => None,
    }
}

/// The list of values a rule expression reads: a literal list, or a `Ref` to a parameter
/// whose value is "an array of literal strings that are separated by commas".
///
/// `None` for a list past [`super::template::MAX_RESOLVED_LIST_LEN`], the bound every list
/// this service resolves is held to: both the list's LENGTH and the request-supplied text it
/// is split from are the caller's to choose, so the split is stopped at the bound rather
/// than materialising as many strings as a body holds commas — and an assertion over a list
/// past it is one this service does not judge.
fn rule_values(
    value: &Value,
    parameters: &BTreeMap<String, String>,
    depth: usize,
) -> Option<Vec<String>> {
    if depth > template::MAX_INTRINSIC_DEPTH {
        return None;
    }
    match value {
        Value::Array(items) => {
            if items.len() > template::MAX_RESOLVED_LIST_LEN {
                return None;
            }
            items
                .iter()
                .map(|item| rule_value(item, parameters, depth + 1))
                .collect()
        }
        other => {
            let text = rule_value(other, parameters, depth + 1)?;
            // One past the bound is enough to know it is past it, so nothing larger is
            // ever built.
            let items: Vec<String> = text
                .split(',')
                .take(template::MAX_RESOLVED_LIST_LEN + 1)
                .map(|item| item.trim().to_string())
                .collect();
            (items.len() <= template::MAX_RESOLVED_LIST_LEN).then_some(items)
        }
    }
}
