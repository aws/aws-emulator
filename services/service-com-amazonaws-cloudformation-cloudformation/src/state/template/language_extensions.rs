// SPDX-License-Identifier: CC0-1.0

//! The `AWS::LanguageExtensions` transform's own expansion: the stage a submitted template
//! body is PROCESSED into, and the directives that transform defines (`Fn::ForEach`,
//! `Fn::Length`, `Fn::ToJsonString`) rewritten out of the document.
//!
//! One file of [`super`]'s concern — the reading and evaluation of a template live THERE —
//! split out so each file holds one of the two.

use super::*;

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
    // The largest template this service reads at all, enforced here too: this is the
    // expander's own guard, so no caller reaches the expansion with a body past the bound
    // every reading of a template is held to.
    if body.len() > crate::state::TEMPLATE_MAX_BYTES {
        return Processed::Unavailable(format!(
            "the template body exceeds {} bytes",
            crate::state::TEMPLATE_MAX_BYTES
        ));
    }
    let declared = body_declared_transforms(body);
    if declared.is_empty() {
        return Processed::Same;
    }
    let unsupported: Vec<&str> = declared
        .iter()
        .map(String::as_str)
        .filter(|name| !applied_by_this_service(name))
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
    // [`expand_with_parameters`](crate::state::sam::expand_with_parameters)).
    crate::state::sam::expand_with_parameters(&mut document, &parameters.values);
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
pub(super) fn in_key_order(value: Value) -> Value {
    in_key_order_within(value, 0)
}

/// [`in_key_order`] below `depth`, refusing to descend past [`MAX_EXPANSION_DEPTH`] as every
/// other walk over a caller-supplied document in this module does.
///
/// The document is a request body, which is bounded in SIZE and not in depth, so a native
/// recursion over it has to stop somewhere: a subtree nested deeper than any reading of a
/// template is held to is answered AS IT STANDS rather than re-ordered, which keeps the
/// ordering guarantee for every document this service will deploy and keeps one request from
/// driving the stack out from under the process.
pub(super) fn in_key_order_within(value: Value, depth: usize) -> Value {
    if depth > MAX_EXPANSION_DEPTH {
        return value;
    }
    match value {
        Value::Array(items) => Value::Array(
            items
                .into_iter()
                .map(|item| in_key_order_within(item, depth + 1))
                .collect(),
        ),
        Value::Object(map) => {
            let mut members: Vec<(String, Value)> = map.into_iter().collect();
            members.sort_by(|(left, _), (right, _)| left.cmp(right));
            Value::Object(
                members
                    .into_iter()
                    .map(|(key, member)| (key, in_key_order_within(member, depth + 1)))
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
pub(super) fn expand_directives(
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
pub(super) struct DirectiveParameters {
    /// Each parameter's value in force, keyed by the name a `Ref` names it by.
    pub(super) values: BTreeMap<String, String>,
    /// The parameters declared with a LIST type, whose value is "an array of literal
    /// strings that are separated by commas" rather than one string.
    pub(super) lists: BTreeSet<String>,
}

impl DirectiveParameters {
    /// What `{"Ref": "<name>"}` stands for inside a directive's argument, or `None` for a
    /// name with no value in force — which leaves the reference where it was, so the
    /// directive that needed it is reported unresolved rather than resolved to nothing.
    pub(super) fn value_of(&self, name: &str) -> Option<Value> {
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
pub(super) fn directive_parameters(
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
///
/// Every replacement is CHARGED to the expansion's budget before it is made, from an upper
/// bound on the value's own size ([`json_size_bound`]): both how many references a directive's
/// argument holds and how long one parameter's value is come from the submitting request, so
/// the product of the two is what one body could otherwise amplify into memory. A template
/// whose references would cost more than the budget is refused instead of resolved, exactly as
/// one whose loops would.
pub(super) fn resolve_parameter_refs(
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
                resolve_parameter_refs(item, parameters, budget, depth + 1)?;
            }
        }
        Value::Object(map) => {
            if map.len() == 1 {
                if let Some(name) = map.get(REF_FUNCTION).and_then(Value::as_str) {
                    if let Some(value) = parameters.value_of(name) {
                        budget.charge(REF_FUNCTION, 0, json_size_bound(&value, 0))?;
                        *node = value;
                    }
                    return Ok(());
                }
            }
            for value in map.values_mut() {
                resolve_parameter_refs(value, parameters, budget, depth + 1)?;
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
pub(super) fn insert_expanded(
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
pub(super) fn expand_for_each(
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
    resolve_parameter_refs(&mut collection, parameters, budget, depth + 1)?;
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
pub(super) fn identifier_reads(
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
pub(super) fn substitute_identifier(
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
pub(super) fn evaluate_directive(
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
            resolve_parameter_refs(&mut argument, parameters, budget, 0)?;
            match &argument {
                Value::Array(items) => Ok(Some(Value::from(items.len()))),
                // Not a list the DOCUMENT decides: the directive is left where it is, for
                // the deployment to evaluate against the whole scope — `Fn::GetAZs` and
                // `Fn::Split` are "an intrinsic function that returns an array" too, and
                // what they return is not in the template. An argument that can be no list
                // at all is refused where the submitted template is validated
                // ([`crate::state::template_validation`]).
                _ => Ok(None),
            }
        }
        FN_TO_JSON_STRING => {
            let mut argument = argument.clone();
            resolve_parameter_refs(&mut argument, parameters, budget, 0)?;
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
pub(super) fn escaped_size_bound(node: &Value) -> usize {
    json_size_bound(node, 0)
        .saturating_mul(2)
        .saturating_add(2)
}

/// An upper bound on `serde_json::to_string(node)`'s length, walked without allocating.
///
/// Every byte of a string can need escaping, so a string counts double; a subtree deeper
/// than [`MAX_EXPANSION_DEPTH`] counts as unbounded, which refuses the directive that
/// asked for it rather than walking further.
pub(super) fn json_size_bound(node: &Value, depth: usize) -> usize {
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
pub(super) fn holds_intrinsic(node: &Value, depth: usize) -> bool {
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
pub(super) fn rendered_item(item: &Value) -> Option<String> {
    match item {
        Value::String(text) => Some(text.clone()),
        Value::Number(number) => Some(number.to_string()),
        Value::Bool(flag) => Some(flag.to_string()),
        _ => None,
    }
}
