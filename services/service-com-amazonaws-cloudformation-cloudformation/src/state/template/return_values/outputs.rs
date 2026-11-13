// SPDX-License-Identifier: CC0-1.0

//! What a template's OUTPUTS and its intrinsic functions are evaluated against, and the
//! evaluation itself: the [`Scope`] every expression resolves in, the outputs a deployment
//! publishes from it, the export names and other stacks' outputs a body really reads, and the
//! bounded [`Resolved`] value one expression yields.
//!
//! One file of [`super`]'s own concern — the return values and identities a resource
//! publishes are computed THERE, out of this scope — split out so each file holds one of the
//! two.

use super::*;

/// Everything an intrinsic function resolves against.
#[derive(Debug, Clone, Copy)]
pub struct Scope<'a> {
    pub stack: StackScope<'a>,
    /// The parameter values in force on the stack, keyed by `ParameterKey`.
    pub parameters: &'a BTreeMap<String, String>,
    /// The TYPE each parameter was declared with, which decides whether a `Ref` to it
    /// resolves to one value or to a list of them.
    pub parameter_types: &'a BTreeMap<String, String>,
    /// The stack's resources, keyed by logical id.
    pub resources: &'a BTreeMap<String, StoredResource>,
    /// The template's `Mappings` section.
    pub mappings: &'a Value,
    /// The template's `Conditions` section.
    pub conditions: &'a Value,
    /// The exported output values a `Fn::ImportValue` resolves against, keyed by export
    /// name: "the value of an output exported by another stack". An export name this map
    /// does not carry is not exported in the account and Region the deployment runs in, so
    /// the import resolves to nothing.
    pub imports: &'a BTreeMap<String, String>,
    /// The other stacks' output values an `Fn::GetStackOutput` resolves against, keyed by
    /// the reference that reads one: "the value of an output of another stack", read by the
    /// stack and the output key rather than by an export name. A reference this map does not
    /// carry reads an output no stack of the account and Region holds, so it resolves to
    /// nothing — see [`crate::state::stack_outputs`].
    pub stack_outputs: &'a crate::state::stack_outputs::Values,
}

/// Evaluate every output a template declares, in `OutputKey` order (the protocol fixes no
/// order for `Stack$Outputs`).
///
/// One entry per output the stack HAS — an output the template declares under a
/// `Condition` that evaluates false is not one of them ("output values that are
/// conditionally created"), and a condition this module cannot evaluate leaves the output
/// reported rather than dropping it. Whether or not its value could be evaluated does not
/// change that: the output exists because the template declares it, and
/// [`StoredOutput::value`] is `None` where this module cannot compute the value rather
/// than carrying an invented one. The `budget` is the deployment's — shared with
/// [`provision`] — so the work of a whole request is bounded, not just each output's.
pub fn evaluate_outputs(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<StoredOutput> {
    let mut outputs = Vec::with_capacity(template.outputs.len());
    for declared in &template.outputs {
        let reported = declared.condition.as_deref().is_none_or(|condition| {
            condition_holds(condition, scope, 0, budget).unwrap_or(true)
        });
        if !reported {
            continue;
        }
        outputs.push(StoredOutput {
            key: declared.key.clone(),
            value: text(&declared.value, scope, 0, budget),
            description: declared.description.clone(),
            export_name: declared
                .export_name
                .as_ref()
                .and_then(|name| text(name, scope, 0, budget)),
        });
    }
    outputs
}

/// Every export name a template IMPORTS: the resolved argument of each `Fn::ImportValue`
/// its resources and its outputs name, in the order the template declares them and without
/// repeats.
///
/// This is the stack's half of the cross-stack reference index `ListImports` reports —
/// "the stacks that are importing an exported output value" — so it is computed where the
/// template is deployed, from the document that really deploys, and charged to that
/// deployment's own budget like every other pass over a caller-sized body. An argument
/// that is itself an expression is evaluated, so an import whose name a `Fn::Sub` or a
/// parameter decides is the export name it really resolved to.
///
/// DECLARATION ORDER, because a deployment that cannot resolve an import reports the FIRST
/// one it could not ([`super::StoredStack::unresolved_cross_stack_failure`]), which is the one
/// the template names first — the index the names also feed is keyed by name, so the order
/// is nothing to it.
pub fn imported_export_names(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<String> {
    let mut found = Vec::new();
    for resource in &template.resources {
        collect_imports(&resource.declaration, scope, 0, budget, &mut found);
    }
    for output in &template.outputs {
        collect_imports(&output.value, scope, 0, budget, &mut found);
        if let Some(export_name) = &output.export_name {
            collect_imports(export_name, scope, 0, budget, &mut found);
        }
    }
    found
}

/// Collect every `Fn::ImportValue` argument inside one declaration, bounded by the
/// deployment's depth limit and its shared budget: ONE pass over the node, whatever the
/// document carries. A name already collected is not collected twice, so the walk answers
/// each imported name once, at the position it was first named.
pub(crate) fn collect_imports(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
    found: &mut Vec<String>,
) {
    if depth > MAX_INTRINSIC_DEPTH || budget.spend(1).is_none() {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_imports(item, scope, depth + 1, budget, found);
            }
        }
        Value::Object(members) => {
            for (name, argument) in members {
                if name == FN_IMPORT_VALUE {
                    if let Some(export_name) = text(argument, scope, depth + 1, budget) {
                        if !found.contains(&export_name) {
                            found.push(export_name);
                        }
                    }
                }
                collect_imports(argument, scope, depth + 1, budget, found);
            }
        }
        _ => {}
    }
}

/// Every other stack's output a template READS with an `Fn::GetStackOutput`, as the pair
/// (stack, output key) each reference resolves to, in the order the template declares them
/// and without repeats.
///
/// The reading stack's half of that relation: it is what the deployment resolves against
/// the Region's own table and then KEEPS ([`crate::state::stack_outputs`]), so it is
/// computed where the template is deployed, from the document that really deploys, and
/// charged to that deployment's own budget like every other pass over a caller-sized body.
/// An argument that is itself an expression is evaluated, so a reference whose stack or
/// output key a parameter decides is the pair it really names.
///
/// DECLARATION ORDER, for the reason [`imported_export_names`] is in it: a deployment that
/// cannot resolve one reports the FIRST it could not.
pub fn stack_output_reads(
    template: &Template,
    scope: &Scope<'_>,
    budget: &mut Budget,
) -> Vec<crate::state::stack_outputs::Reference> {
    let mut found = Vec::new();
    for resource in &template.resources {
        collect_stack_output_reads(&resource.declaration, scope, 0, budget, &mut found);
    }
    for output in &template.outputs {
        collect_stack_output_reads(&output.value, scope, 0, budget, &mut found);
        if let Some(export_name) = &output.export_name {
            collect_stack_output_reads(export_name, scope, 0, budget, &mut found);
        }
    }
    found
}

/// Collect every `Fn::GetStackOutput` reference inside one declaration, bounded by the
/// deployment's depth limit and its shared budget, exactly as [`collect_imports`] is: ONE
/// pass over the node, each reference answered once, at the position it was first named.
pub(super) fn collect_stack_output_reads(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
    found: &mut Vec<crate::state::stack_outputs::Reference>,
) {
    if depth > MAX_INTRINSIC_DEPTH || budget.spend(1).is_none() {
        return;
    }
    match value {
        Value::Array(items) => {
            for item in items {
                collect_stack_output_reads(item, scope, depth + 1, budget, found);
            }
        }
        Value::Object(members) => {
            for (name, argument) in members {
                if name == crate::state::stack_outputs::FN_GET_STACK_OUTPUT {
                    if let Some(reference) =
                        stack_output_reference(argument, scope, depth + 1, budget)
                    {
                        if !found.contains(&reference) {
                            found.push(reference);
                        }
                    }
                }
                collect_stack_output_reads(argument, scope, depth + 1, budget, found);
            }
        }
        _ => {}
    }
}

/// The stack and the output one `Fn::GetStackOutput` argument names:
/// `{"StackName": <stack>, "OutputName": <output>}`, the two members the function takes.
/// Each is evaluated the way every other function's arguments are, so a member spelled by an
/// expression names what it resolves to.
///
/// `None` for an argument that names neither member, which is no reference at all — the
/// same answer every other expression this evaluator cannot read gives.
pub(crate) fn stack_output_reference(
    argument: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<crate::state::stack_outputs::Reference> {
    let members = argument.as_object()?;
    let stack = text(
        members.get(crate::state::stack_outputs::STACK_NAME)?,
        scope,
        depth + 1,
        budget,
    )?;
    let output = text(
        members.get(crate::state::stack_outputs::OUTPUT_NAME)?,
        scope,
        depth + 1,
        budget,
    )?;
    Some(crate::state::stack_outputs::Reference::new(stack, output))
}

/// An evaluated expression: CloudFormation's intrinsic functions produce either a string
/// or a list of strings (`Fn::Split`, `Fn::GetAZs`), and the list form is only ever
/// consumed by another function.
#[derive(Debug, Clone, PartialEq, Eq)]
pub(crate) enum Resolved {
    Text(String),
    List(Vec<String>),
}

/// Evaluate an expression to the text CloudFormation substitutes for it, or `None` when
/// this module cannot evaluate it — or when the deployment's work budget is spent.
pub(crate) fn text(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<String> {
    match evaluate(value, scope, depth, budget)? {
        Resolved::Text(text) => Some(text),
        // A list is not a value an output or a name can carry.
        Resolved::List(_) => None,
    }
}

/// Evaluate one expression.
pub(crate) fn evaluate(
    value: &Value,
    scope: &Scope<'_>,
    depth: usize,
    budget: &mut Budget,
) -> Option<Resolved> {
    if depth > MAX_INTRINSIC_DEPTH {
        return None;
    }
    // One unit for visiting this node, before anything it might cost: this is what makes
    // the TOTAL work of a deployment finite however the template composes its functions.
    budget.spend(1)?;
    match value {
        Value::Null => None,
        Value::String(_) | Value::Number(_) | Value::Bool(_) => scalar_text(value)
            .map(Resolved::Text)
            .and_then(|resolved| bounded(resolved, budget)),
        Value::Array(items) => {
            if items.len() > MAX_RESOLVED_LIST_LEN {
                return None;
            }
            let mut resolved = Vec::with_capacity(items.len());
            for item in items {
                resolved.push(text(item, scope, depth + 1, budget)?);
            }
            bounded(Resolved::List(resolved), budget)
        }
        Value::Object(members) => {
            // An intrinsic function is an object with exactly one member, whose name is
            // the function's.
            let mut entries = members.iter();
            let (name, argument) = entries.next()?;
            if entries.next().is_some() {
                return None;
            }
            intrinsic(name, argument, scope, depth, budget)
                .and_then(|resolved| bounded(resolved, budget))
        }
    }
}

/// Refuse an evaluated value past [`MAX_RESOLVED_TEXT_BYTES`], and charge the deployment's
/// budget for the bytes it carries, so no composition of functions grows one — or copies
/// one — without limit.
pub(crate) fn bounded(resolved: Resolved, budget: &mut Budget) -> Option<Resolved> {
    let size = match &resolved {
        Resolved::Text(text) => text.len(),
        Resolved::List(items) => items.iter().map(String::len).sum(),
    };
    budget.spend_text(size)?;
    (size <= MAX_RESOLVED_TEXT_BYTES).then_some(resolved)
}
