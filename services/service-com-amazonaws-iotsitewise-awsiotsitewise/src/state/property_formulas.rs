// SPDX-License-Identifier: CC0-1.0

//! What an asset-model write holds a property definition to BEFORE anything is
//! stored: the names a model's properties may carry, the scope a formula's
//! variables resolve in, the formula itself, and the window a metric aggregates
//! over.
//!
//! # The rule
//!
//! An asset-model write resolves and parses the definition it is handed and
//! refuses it as a bad request, so nothing unusable is ever stored:
//!
//! * two properties of one model may not share a NAME, because a name is a handle
//!   a same-model variable resolves by (`VariableValue.propertyId`: "You can use
//!   the property `name` if it's from the same asset model");
//! * a variable resolves in ONE scope — the model being written, or, when the
//!   variable names a `hierarchyId`, the child model of THAT hierarchy
//!   (`VariableValue.hierarchyId`: "The ID of the hierarchy to query for the
//!   property ID") — so a hierarchy the model does not declare and a property the
//!   scope does not hold both name nothing; and the NAME spelling of a
//!   `propertyId` is one the model being written accepts alone, because it is
//!   documented for a property "from the same asset model", so a reference
//!   through a hierarchy has to be an identifier and a name there is an id
//!   invalid for that variable;
//! * a formula is PARSED: every identifier in it is a declared variable or a
//!   function applied to arguments, and every declared variable appears in it
//!   (`Metric.expression` / `Transform.expression` are the expressions, and
//!   `.variables` "the list of variables used in the expression");
//! * a tumbling window's interval names a span inside the published limits
//!   (`TumblingWindow.interval`: "The interval time must be between 1 minute and 1
//!   week").
//!
//! Every refusal here is a message the calling operation lifts into its own
//! modelled `InvalidRequestException` (HTTP 400), and every one of them is decided
//! before a single field is written, so a refused write leaves the model exactly
//! as it was.
//!
//! A variable that DOES resolve is then rewritten to the id it resolved to —
//! [`DefinitionUnderWrite::resolve_variable_references`] — because a name and an
//! `externalId:` alias are accepted INPUT spellings of an id, and every property
//! and hierarchy of a written model carries a server-assigned one: the scope a
//! reference is checked in is the scope it is resolved in, so a stored model
//! reports the identifier of the member each of its references names.
//!
//! The expression grammar itself is IoT SiteWise's own formula language, so the
//! identifiers of an expression are read here rather than through a parser for
//! somebody else's language: what this reads off an expression is only its
//! identifier tokens and where each one starts, which is what the two formula
//! rules ask about.
//!
//! What that reading deliberately does NOT decide is whether a FUNCTION the
//! formula applies exists. The language's function catalogue lives only in the
//! IoT SiteWise User Guide — the Smithy model carries no list of it, and AWS adds
//! to it — so a hand-kept copy here would refuse formulas the real service
//! computes, which is the worse failure of the two: an identifier applied to
//! arguments is therefore read as a function reference, and only an identifier
//! that is NOT applied has to be a variable the formula declares. Checking a
//! function name needs that catalogue as a modelled input, not as a list written
//! from one sample.

use super::*;

use crate::ops::types::{MetricWindow, VariableValue};

/// The shortest span a tumbling window's `interval` may name.
///
/// DOCUMENTED, not chosen: `TumblingWindow.interval` states "The interval time
/// must be between 1 minute and 1 week".
pub const MIN_TUMBLING_INTERVAL: Duration = Duration::from_secs(60);

/// The longest span a tumbling window's `interval` may name; see
/// [`MIN_TUMBLING_INTERVAL`] for the sentence that fixes both bounds.
pub const MAX_TUMBLING_INTERVAL: Duration = Duration::from_secs(7 * 24 * 60 * 60);

/// The unit letters a window's interval or offset may be spelled with, and how
/// many seconds each one is worth.
///
/// Source: `TumblingWindow`'s own documentation — "You can use `m`, `h`, `d`, and
/// `w` when you specify an interval or offset. Note that `m` represents minutes,
/// `h` represents hours, `d` represents days, and `w` represents weeks. You can
/// also use `s` to represent seconds in `offset`." The seconds letter is read
/// here too, so a value spelled with it is BOUND-CHECKED as the span it names
/// rather than refused for its spelling.
/// The letters are matched EXACTLY as the documentation spells them — lowercase —
/// so a spelling the real service does not accept is not accepted here either.
const INTERVAL_UNITS: [(char, u64); 5] = [
    ('s', 1),
    ('m', 60),
    ('h', 60 * 60),
    ('d', 24 * 60 * 60),
    ('w', 7 * 24 * 60 * 60),
];

/// The literal keywords a formula may name without declaring them as variables:
/// the two boolean literals of the expression language, which the comparison
/// functions take and return.
///
/// Only those two: every other bare word of a formula is taken to be a variable
/// it has to declare. A further bare constant of the language — if it has one —
/// would belong here, and widening it needs the same kind of evidence the
/// function question does (see the module header).
const FORMULA_LITERALS: [&str; 2] = ["true", "false"];

/// How many characters of an expression this module reads.
///
/// The modelled ceiling: `Metric.expression` and `Transform.expression` both
/// target `Expression`, `@length 0..1024`
/// (`smithy_query(query="shape", shape="Metric")`), which the framework's own
/// constraint walk has already enforced by the time a handler runs — so no
/// expression that reaches here is longer, and the bound is stated again in the
/// reading itself so its cost is capped whoever calls it.
pub const MAX_EXPRESSION_CHARACTERS: usize = 1024;

/// How many identifiers of one expression this module collects.
///
/// The character bound above is the real ceiling — an identifier and the
/// operator or comma beside it take at least two characters, so 1024 characters
/// hold at most half that many identifiers — and this states the bound on the
/// collection itself, so the memory the reading uses is capped by the input bound
/// rather than by what a caller sends. No identifier of an expression that can
/// reach a handler is ever cut by it.
pub const MAX_FORMULA_IDENTIFIERS: usize = MAX_EXPRESSION_CHARACTERS;

/// How many variables one formula may declare.
///
/// DOCUMENTED, not chosen: `Metric.expression` and `Transform.expression` both
/// state it — "You can specify up to 10 variables per expression. You can specify
/// up to 10 functions per expression" — and the list shapes carry no length
/// constraint of their own, so the write is what enforces it. It is also what
/// bounds the per-variable resolution below: whatever a request declares, at most
/// this many variables are ever resolved.
pub const MAX_VARIABLES_PER_EXPRESSION: usize = 10;

/// The name of the formula member a refusal reports a bad variable of: the two
/// shapes that declare `variables`, named as the model names them.
const METRIC_FORMULA: &str = "Metric";

/// See [`METRIC_FORMULA`].
const TRANSFORM_FORMULA: &str = "Transform";

/// Whether `label` is spelled as an IDENTIFIER of a property rather than as its
/// name: the modelled `ID` form ([`super::names_modelled_id`]), or
/// `externalId:` followed by a non-empty external id.
///
/// Those are the two spellings every resolution scope accepts. The NAME spelling
/// is documented for one scope only — a property "from the same asset model" —
/// so this is what a scope that is not the model being written holds a reference
/// to (see [`Spellings`]).
///
/// `VariableValue.propertyId` targets `Macro` rather than `ID`, so a reference's
/// spelling is not constrained on the wire and the write is what reads it. The
/// `ID` form itself is read by the one function that states it
/// ([`super::names_modelled_id`]); the external-id half is LOOSER here on
/// purpose — this asks which SPELLING a reference used, not whether the value
/// satisfies a member's `@pattern`, and an external id that does not is refused
/// where that pattern is enforced.
fn spelled_as_property_id(label: &str) -> bool {
    match label.strip_prefix(EXTERNAL_ID_PREFIX) {
        Some(external_id) => !external_id.is_empty(),
        None => super::names_modelled_id(label),
    }
}

/// One identifier of a formula expression: its text, where it starts, and whether
/// the expression APPLIES it to arguments.
///
/// The position is counted in characters from the start of the expression, which
/// is what a refusal naming an unknown identifier reports.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FormulaIdentifier {
    pub text: String,
    pub at: usize,
    /// Whether the identifier is followed by `(` — an identifier applied to
    /// arguments is a function of the expression language, not a variable.
    pub applied: bool,
}

/// Every identifier `expression` names, in the order it names them.
///
/// Reads the expression as the formula language lays it out: a quoted literal is
/// text rather than identifiers, a number is a number (so the exponent of `1e3`
/// is not an identifier of its own), and an identifier is a letter or `_`
/// followed by letters, digits and `_`. TOTAL over every string a caller can
/// send, and it cannot panic.
///
/// # Bounds
///
/// The expression is caller-controlled, so this reading is bounded on its face:
/// it is a single pass with NO recursion — the depth of a caller's parenthesis
/// nest is never a depth of this reading, which does not look at parentheses
/// beyond the one that makes an identifier a function call — it reads at most
/// [`MAX_EXPRESSION_CHARACTERS`] characters, and it collects at most
/// [`MAX_FORMULA_IDENTIFIERS`] identifiers, so neither the time it takes nor the
/// memory it uses depends on how long or how nested the text is. Both ceilings
/// sit at the modelled limits of what can reach a handler at all, so nothing a
/// caller can send through the wire is cut short by them.
#[must_use]
pub fn formula_identifiers(expression: &str) -> Vec<FormulaIdentifier> {
    let characters: Vec<char> = expression
        .chars()
        .take(MAX_EXPRESSION_CHARACTERS)
        .collect();
    let mut identifiers = Vec::new();
    let mut at = 0;
    while at < characters.len() && identifiers.len() < MAX_FORMULA_IDENTIFIERS {
        let character = characters[at];
        if character == '\'' || character == '"' {
            // A quoted literal: its contents are text, not identifiers. An
            // unterminated quote runs to the end of the expression.
            at += 1;
            while at < characters.len() && characters[at] != character {
                at += 1;
            }
            at += 1;
            continue;
        }
        if character.is_ascii_digit() {
            // A number literal, including the `e` of an exponent and the `.` of a
            // fraction, so neither becomes an identifier.
            at += 1;
            while at < characters.len()
                && (characters[at].is_ascii_alphanumeric() || characters[at] == '.')
            {
                at += 1;
            }
            continue;
        }
        if character.is_alphabetic() || character == '_' {
            let start = at;
            while at < characters.len()
                && (characters[at].is_alphanumeric() || characters[at] == '_')
            {
                at += 1;
            }
            let text: String = characters[start..at].iter().collect();
            let mut after = at;
            while after < characters.len() && characters[after].is_whitespace() {
                after += 1;
            }
            identifiers.push(FormulaIdentifier {
                text,
                at: start,
                applied: characters.get(after) == Some(&'('),
            });
            continue;
        }
        at += 1;
    }
    identifiers
}

/// The span the window interval or offset `value` names, or `None` when it names
/// none.
///
/// Both spellings `TumblingWindow` documents: the `<count><unit>` shorthand over
/// [`INTERVAL_UNITS`], and the ISO 8601 duration form ("The `interval` and
/// `offset` parameters support the ISO 8601 format. For example, `PT5S`
/// represents 5 seconds"), read through the one ISO 8601 span reading this
/// workspace shares rather than a second grammar written here.
#[must_use]
pub fn window_span(value: &str) -> Option<Duration> {
    if value
        .chars()
        .next()
        .is_some_and(|first| first.eq_ignore_ascii_case(&'P'))
    {
        return ::service_common::iso8601_span::iso8601_span(value);
    }
    let unit = value.chars().next_back()?;
    let (_, seconds) = INTERVAL_UNITS
        .into_iter()
        .find(|(letter, _)| unit == *letter)?;
    let count: u64 = value[..value.len() - unit.len_utf8()].parse().ok()?;
    count.checked_mul(seconds).map(Duration::from_secs)
}

/// The message a property name a model already carries is refused with: the
/// duplicated name, as the request spelled it.
#[must_use]
pub fn duplicate_property_name_message(name: &str) -> String {
    format!("Property {name} name is not unique in the Asset Model.")
}

/// The message a HIERARCHY name a model already carries is refused with: the
/// duplicated name, as the request spelled it.
///
/// A hierarchy's name is a handle of the model just as a property's is — a
/// formula variable may name the hierarchy it reads through by name ("You can use
/// the hierarchy's name instead of the hierarchy's ID") — so two hierarchies of
/// one model may not share one, and the refusal names the hierarchy rather than
/// the property.
#[must_use]
pub fn duplicate_hierarchy_name_message(name: &str) -> String {
    format!("Hierarchy {name} name is not unique.")
}

/// The message a `propertyId` that names nothing in its resolution scope is
/// refused with: the id exactly as the request sent it — AWS names the id the
/// request supplied, because it is that id which the scope does not hold.
#[must_use]
pub fn property_does_not_exist_message(property_id: &str) -> String {
    format!("Property {property_id} does not exist.")
}

/// The message a `hierarchyId` that matches no hierarchy the model declares is
/// refused with: the label exactly as the request sent it.
#[must_use]
pub fn hierarchy_does_not_exist_message(hierarchy_id: &str) -> String {
    format!("AssetModelHierarchy {hierarchy_id} does not exist.")
}

/// The message a `propertyId` spelled in a form the scope it resolves in does not
/// accept is refused with: the label exactly as the request sent it, and the
/// formula member whose variables carry it.
///
/// A property NAME is a spelling only the model being written accepts, so a
/// reference scoped to a hierarchy's child model carries an id or an `externalId:`
/// alias; a name there is not a property the scope does not hold but an id that is
/// invalid for that variable, which is the distinction AWS words here.
#[must_use]
pub fn invalid_property_id_message(property_id: &str, formula: &str) -> String {
    format!("Property Id {property_id} invalid for variables in {formula} property.")
}

/// The message a declared variable the expression never names is refused with:
/// the expression, then the variable.
#[must_use]
pub fn unused_variable_message(expression: &str, variable: &str) -> String {
    format!(
        "`{expression}`: Variable [{variable}] is unused in formula. \
         Formulas must use all defined variables"
    )
}

/// The message an identifier that is neither a declared variable nor a function
/// of the expression language is refused with: the expression, the identifier and
/// where in the expression it starts.
#[must_use]
pub fn unknown_identifier_message(expression: &str, identifier: &str, at: usize) -> String {
    format!(
        "`{expression}`: Unknown function or variable '{identifier}' at pos {at} \
         in expression '{expression}'"
    )
}

/// The message a formula declaring more variables than the published quota is
/// refused with: the formula, the count it declares and the quota it exceeds.
///
/// The RULE is grounded — the quota is the one the expression members publish —
/// but the WORDING is this emulator's, unlike the refusals above it: no AWS text
/// for this class was available to reproduce, so the message states the quota and
/// the count the request declared rather than guessing at a sentence.
#[must_use]
pub fn variable_quota_message(expression: &str, declared: usize) -> String {
    format!(
        "`{expression}`: {declared} variables are defined, but a formula may define at most \
         {MAX_VARIABLES_PER_EXPRESSION}"
    )
}

/// The message an interval outside the published window limits is refused with:
/// the formula the window belongs to, then the interval as the request spelled
/// it.
#[must_use]
pub fn window_interval_message(expression: &str, interval: &str) -> String {
    format!("`{expression}`: Window time interval '{interval}' is not within limits")
}

/// The message an id-less property definition whose NAME an existing property
/// already holds is refused with: the name the request sent, and the id of the
/// property the creation would have replaced.
///
/// `UpdateAssetModel` replaces the whole model, and a property's id is what every
/// time series, variable reference and ingest path addressing it names, so a
/// definition that would re-create an existing property under a new id is refused
/// rather than silently destroying it: the member documents the alternative —
/// "If you are calling UpdateAssetModel to modify an existing property: This can
/// be either the actual ID in UUID format, or else `externalId:` followed by the
/// external ID".
#[must_use]
pub fn property_replacement_message(name: &str, existing_property_id: &str) -> String {
    format!(
        "Cannot create property {name}. Creation will result in replacement of property \
         {existing_property_id}"
    )
}

/// Refuse a property list two of whose entries carry one name.
///
/// Asked of every list of properties one model holds — the model's own and each
/// composite model's — because a name is the handle a same-model variable
/// resolves by, so two properties sharing one would make every reference to it
/// ambiguous.
///
/// LINEAR in the length of the list: the names seen so far are a set, so what a
/// caller's property list costs grows with the list and not with its square.
pub fn check_property_names_unique(properties: &[AssetModelProperty]) -> Result<(), String> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(properties.len());
    for property in properties {
        if !seen.insert(property.name.as_str()) {
            return Err(duplicate_property_name_message(&property.name));
        }
    }
    Ok(())
}

/// Refuse a hierarchy list two of whose entries carry one name.
///
/// Asked of the hierarchies one model holds, for the reason
/// [`check_property_names_unique`] is asked of its properties: a name is a
/// handle a variable may resolve a hierarchy by, so two hierarchies sharing one
/// would make every reference through it ambiguous.
///
/// LINEAR in the length of the list, like its sibling above.
pub fn check_hierarchy_names_unique(hierarchies: &[AssetModelHierarchy]) -> Result<(), String> {
    let mut seen: HashSet<&str> = HashSet::with_capacity(hierarchies.len());
    for hierarchy in hierarchies {
        if !seen.insert(hierarchy.name.as_str()) {
            return Err(duplicate_hierarchy_name_message(&hierarchy.name));
        }
    }
    Ok(())
}

/// Which spellings of a property a resolution scope accepts.
///
/// `VariableValue.propertyId` documents the NAME spelling for one scope only —
/// "You can use the property `name` if it's from the same asset model" — so a
/// scope that is NOT the model being written resolves a reference by the
/// property's id or its `externalId:` alias alone, and a name reaching it names
/// nothing.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spellings {
    /// The id, the `externalId:` alias and the name: the model being written.
    IncludingNames,
    /// The id and the `externalId:` alias: the child model of a declared
    /// hierarchy, which is not the model being written.
    IdsOnly,
}

/// Every label that names one of `properties` — each one's id, its `externalId:`
/// alias and, where `spellings` admits it, its name — mapped to the id of the
/// property it names, or to `None` for a property that carries no id at all.
///
/// A resolution scope is asked the same question once per variable, so it is
/// indexed ONCE here and each question is then a single map lookup, whatever the
/// scope holds. The id each label resolves to is what turns an accepted INPUT
/// spelling into the value a read of the model reports (see
/// [`DefinitionUnderWrite::resolve_variable_references`]).
fn property_labels<'a>(
    properties: impl Iterator<Item = &'a AssetModelProperty>,
    spellings: Spellings,
) -> HashMap<String, Option<String>> {
    let mut labels = HashMap::new();
    for property in properties {
        let id = property.id.clone().filter(|id| !id.is_empty());
        if let Some(id) = id.clone() {
            labels.insert(id.clone(), Some(id));
        }
        if let Some(external_id) = property.external_id.as_deref() {
            labels.insert(format!("{EXTERNAL_ID_PREFIX}{external_id}"), id.clone());
        }
        if spellings == Spellings::IncludingNames {
            labels.insert(property.name.clone(), id);
        }
    }
    labels
}

/// Whether `label` names the hierarchy `hierarchy`: its id, its `externalId:`
/// alias or its name ("You can use the hierarchy's name instead of the
/// hierarchy's ID").
fn names_hierarchy(hierarchy: &AssetModelHierarchy, label: &str) -> bool {
    matches_id_label(
        hierarchy.id.as_deref(),
        hierarchy.external_id.as_deref(),
        label,
    ) || hierarchy.name == label
}

/// The definition one asset-model write is about to store, as the rules of this
/// module read it.
///
/// Built by the calling operation from what it has already resolved, so the
/// definition the rules are asked about is exactly the one that would be written.
/// Its resolution scopes are INDEXED when it is built — once per write, by
/// [`property_labels`] — so resolving a variable is a set lookup rather than a
/// scan of the scope, and the cost of a write grows with the definition's size
/// rather than with its square.
#[derive(Debug, Clone, Default)]
pub struct DefinitionUnderWrite {
    /// The model's own top-level property definitions.
    pub properties: Vec<AssetModelProperty>,
    /// The hierarchies the definition declares — the only hierarchies a variable
    /// of this model may name.
    pub hierarchies: Vec<AssetModelHierarchy>,
    /// The property lists the write states for the composite models themselves,
    /// each checked by the same rules as the model's own list.
    pub composite_property_lists: Vec<Vec<AssetModelProperty>>,
    /// Every label naming a property the model will carry — its own and the ones
    /// its composite models contribute — mapped to that property's id: the scope
    /// a variable with no `hierarchyId` resolves in.
    scope: HashMap<String, Option<String>>,
    /// `childAssetModelId` -> every label naming a property that child model
    /// exposes, mapped to that property's id: the scope a variable naming one of
    /// this definition's hierarchies resolves in.
    hierarchy_scopes: HashMap<String, HashMap<String, Option<String>>>,
}

impl DefinitionUnderWrite {
    /// Whether the model being written carries a property the label
    /// `property_label` names.
    fn holds_property(&self, property_label: &str) -> bool {
        self.scope.contains_key(property_label)
    }

    /// Whether the child model of `hierarchy` exposes a property the label
    /// `property_label` names.
    fn child_holds_property(&self, hierarchy: &AssetModelHierarchy, property_label: &str) -> bool {
        self.hierarchy_scopes
            .get(&hierarchy.child_asset_model_id)
            .is_some_and(|labels| labels.contains_key(property_label))
    }

    /// The id of the property the label `property_label` names in the model being
    /// written, when that label names one and it carries an id.
    fn resolved_property(&self, property_label: &str) -> Option<&str> {
        self.scope.get(property_label)?.as_deref()
    }

    /// The id of the property the label `property_label` names in the CHILD model
    /// of `hierarchy`, when that label names one and it carries an id.
    fn child_resolved_property(
        &self,
        hierarchy: &AssetModelHierarchy,
        property_label: &str,
    ) -> Option<&str> {
        self.hierarchy_scopes
            .get(&hierarchy.child_asset_model_id)?
            .get(property_label)?
            .as_deref()
    }

    /// Rewrite every variable reference `properties` carries from the spelling
    /// the request used to the id of the member it resolves to, so what is STORED
    /// — and so what every read of the model reports — is the identifier of the
    /// referenced property and hierarchy rather than the accepted input spelling.
    ///
    /// `VariableValue.propertyId` and `VariableValue.hierarchyId` each document a
    /// name and an `externalId:` alias as alternative spellings of an id ("You can
    /// use the property `name` if it's from the same asset model", "You can use
    /// the hierarchy's name instead of the hierarchy's ID"), and every property
    /// and hierarchy of a written model carries a server-assigned id, so a
    /// reference is resolved once, here, against the same scopes it was checked
    /// in. A label that resolves to nothing is left as it arrived: it was already
    /// refused before anything was stored.
    ///
    /// Asked by every operation that writes property definitions onto an asset
    /// model, so one rule resolves references for all of them.
    pub fn resolve_variable_references(&self, properties: &mut [AssetModelProperty]) {
        for property in properties {
            for variable in formula_variables_mut(property) {
                self.resolve_variable(&mut variable.value);
            }
        }
    }

    /// [`Self::resolve_variable_references`] over the property list each composite
    /// model of the written definition states; a composite model that states none
    /// contributes the properties of the component model it reuses, which were
    /// resolved by the write that stated them.
    pub fn resolve_composite_variable_references(
        &self,
        composite_models: &mut [AssetModelCompositeModel],
    ) {
        for composite in composite_models {
            if let Some(properties) = composite.properties.as_mut() {
                self.resolve_variable_references(properties);
            }
        }
    }

    /// Rewrite ONE variable reference, in the scope `VariableValue` documents for
    /// it: with no `hierarchyId` the model being written, with one the hierarchy
    /// it names and the CHILD model of that hierarchy.
    fn resolve_variable(&self, value: &mut VariableValue) {
        let Some(hierarchy_label) = value.hierarchy_id.clone() else {
            let resolved = value
                .property_id
                .as_deref()
                .and_then(|label| self.resolved_property(label))
                .map(str::to_string);
            if let Some(id) = resolved {
                value.property_id = Some(id);
            }
            return;
        };
        let Some(hierarchy) = self
            .hierarchies
            .iter()
            .find(|hierarchy| names_hierarchy(hierarchy, &hierarchy_label))
        else {
            return;
        };
        let resolved_property = value
            .property_id
            .as_deref()
            .and_then(|label| self.child_resolved_property(hierarchy, label))
            .map(str::to_string);
        let resolved_hierarchy = hierarchy
            .id
            .as_deref()
            .filter(|id| !id.is_empty())
            .map(str::to_string);
        if let Some(id) = resolved_hierarchy {
            value.hierarchy_id = Some(id);
        }
        if let Some(id) = resolved_property {
            value.property_id = Some(id);
        }
    }
}

impl ComAmazonawsIotsitewiseAwsiotsitewiseState {
    /// The definition an asset-model write is about to store, read off what the
    /// operation has already resolved.
    ///
    /// `composite_model_links` is what tells a component-model-based composite
    /// model from an inline one: the first contributes the properties of the
    /// `COMPONENT_MODEL` it reuses — read from that model's stored record, so a
    /// variable may name one of them — while the second contributes the
    /// definitions the write itself states.
    ///
    /// Shared by every operation that writes property definitions onto an asset
    /// model, so all of them ask the rules about the same definition.
    pub fn definition_under_write(
        &self,
        properties: &[AssetModelProperty],
        hierarchies: &[AssetModelHierarchy],
        composite_models: &[AssetModelCompositeModel],
        composite_model_links: &HashMap<String, CompositeModelLink>,
    ) -> DefinitionUnderWrite {
        let mut definition = DefinitionUnderWrite {
            properties: properties.to_vec(),
            hierarchies: hierarchies.to_vec(),
            ..DefinitionUnderWrite::default()
        };
        let mut contributed: Vec<AssetModelProperty> = Vec::new();
        for composite in composite_models {
            let lent = composite
                .id
                .as_deref()
                .and_then(|composite_id| composite_model_links.get(composite_id))
                .and_then(|link| link.composed_asset_model_id.as_deref())
                .and_then(|component_id| self.asset_models.get(component_id));
            if let Some(component) = lent {
                contributed.extend(self.exposed_asset_model_properties(component, &HashSet::new()));
                continue;
            }
            if let Some(stated) = composite.properties.clone() {
                contributed.extend(stated.iter().cloned());
                definition.composite_property_lists.push(stated);
            }
        }
        self.index_definition(&mut definition, &contributed);
        definition
    }

    /// The definition a write to ONE composite model of `model` is about to
    /// store: the model as it stands, with `stated` as the property list of the
    /// composite model `composite_id`.
    ///
    /// `stated` is `None` for a component-model-based composite model, which
    /// states no properties of its own: `composed_asset_model_id` then names the
    /// `COMPONENT_MODEL` whose properties it contributes, so a variable of this
    /// write may name one of them. Every OTHER composite model of the model goes
    /// on contributing what it already does, and only the list this write states
    /// is held to the name and formula rules — the rest were held to them by the
    /// writes that stated them.
    pub fn definition_with_composite_model(
        &self,
        model: &AssetModelRecord,
        composite_id: &str,
        stated: Option<&[AssetModelProperty]>,
        composed_asset_model_id: Option<&str>,
    ) -> DefinitionUnderWrite {
        let mut definition = DefinitionUnderWrite {
            properties: model.properties.clone(),
            hierarchies: model.hierarchies.clone(),
            ..DefinitionUnderWrite::default()
        };
        let mut contributed: Vec<AssetModelProperty> = Vec::new();
        for held in model
            .composite_models
            .iter()
            .filter_map(|composite| composite.id.as_deref())
            .filter(|held| *held != composite_id)
        {
            contributed.extend(self.composite_model_properties(model, held));
        }
        if let Some(component) = composed_asset_model_id.and_then(|id| self.asset_models.get(id)) {
            contributed.extend(self.exposed_asset_model_properties(component, &HashSet::new()));
        }
        if let Some(stated) = stated {
            contributed.extend(stated.iter().cloned());
            definition.composite_property_lists.push(stated.to_vec());
        }
        self.index_definition(&mut definition, &contributed);
        definition
    }

    /// Index the resolution scopes of `definition`: the labels naming a property
    /// the model will carry (its own, plus everything its composite models
    /// `contributed`), and the labels naming a property each declared hierarchy's
    /// CHILD model exposes.
    ///
    /// The two scopes accept different SPELLINGS: a property of the model being
    /// written is named by its id, its `externalId:` alias or its name, while one
    /// of a child model is named by its id or alias only — the name spelling is
    /// documented for a property "from the same asset model", and a child model is
    /// another model (see [`Spellings`]).
    ///
    /// Done ONCE per write, so every variable of every formula in it resolves
    /// through a set lookup. The child scopes are indexed per hierarchy, of which
    /// a model carries at most [`MAX_HIERARCHIES_PER_ASSET_MODEL`], so this costs
    /// one pass over what the write can reach at all.
    fn index_definition(
        &self,
        definition: &mut DefinitionUnderWrite,
        contributed: &[AssetModelProperty],
    ) {
        definition.scope = property_labels(
            definition.properties.iter().chain(contributed.iter()),
            Spellings::IncludingNames,
        );
        for hierarchy in &definition.hierarchies {
            if definition
                .hierarchy_scopes
                .contains_key(&hierarchy.child_asset_model_id)
            {
                continue;
            }
            let Some(child) = self.asset_models.get(&hierarchy.child_asset_model_id) else {
                continue;
            };
            let labels = property_labels(
                self.exposed_asset_model_properties(child, &HashSet::new())
                    .iter(),
                Spellings::IdsOnly,
            );
            definition
                .hierarchy_scopes
                .insert(hierarchy.child_asset_model_id.clone(), labels);
        }
    }

    /// Every property the stored asset model `model` exposes once the composite
    /// models in `removed` are gone: its OWN definitions, plus the ones each
    /// remaining composite model CONTRIBUTES — through
    /// [`Self::composite_model_properties`], so a component-model-based composite
    /// model contributes the component model's.
    ///
    /// The ONE definition of "the properties an asset model exposes", so the
    /// scope a variable resolves in and the dependency rule a composite-model
    /// removal is refused by see the same properties
    /// `DescribeAssetModelCompositeModel` reports. `removed` is empty for every
    /// caller but that removal, which asks what the model would expose without
    /// the composite models it is about to drop.
    pub fn exposed_asset_model_properties(
        &self,
        model: &AssetModelRecord,
        removed: &HashSet<String>,
    ) -> Vec<AssetModelProperty> {
        let mut exposed = model.properties.clone();
        for composite_id in model
            .composite_models
            .iter()
            .filter_map(|composite| composite.id.as_deref())
            .filter(|composite_id| !removed.contains(*composite_id))
        {
            exposed.extend(self.composite_model_properties(model, composite_id));
        }
        exposed
    }

    /// Refuse an asset-model definition the service would not accept: a
    /// duplicated property name, a variable that resolves to nothing, a formula
    /// that does not parse against its declared variables, or a window outside
    /// the published limits.
    ///
    /// Shared by every operation that writes property definitions onto an asset
    /// model, so one rule is enforced in one place and a model cannot be created
    /// outside what an update of it would accept. The `Err` carries the refusal
    /// message, which the calling operation lifts into its own modelled
    /// `InvalidRequestException`.
    pub fn check_asset_model_definition(
        &self,
        definition: &DefinitionUnderWrite,
    ) -> Result<(), String> {
        check_property_names_unique(&definition.properties)?;
        check_hierarchy_names_unique(&definition.hierarchies)?;
        for list in &definition.composite_property_lists {
            check_property_names_unique(list)?;
        }
        for property in definition
            .properties
            .iter()
            .chain(definition.composite_property_lists.iter().flatten())
        {
            self.check_property_formula(property, definition)?;
        }
        Ok(())
    }

    /// Refuse ONE property definition whose `transform` or `metric` the service
    /// would not accept; a property carrying neither formula has nothing to
    /// check.
    fn check_property_formula(
        &self,
        property: &AssetModelProperty,
        definition: &DefinitionUnderWrite,
    ) -> Result<(), String> {
        if let Some(transform) = property.r#type.transform.as_ref() {
            self.check_formula(
                &transform.expression,
                &transform.variables,
                definition,
                TRANSFORM_FORMULA,
            )?;
        }
        if let Some(metric) = property.r#type.metric.as_ref() {
            // `Metric.expression` and `Metric.variables` both carry a modelled
            // default (`""` and `[]`), so a request that states neither states the
            // empty formula over no variables, which is what the rules are asked
            // about here.
            let expression = metric.expression.clone().unwrap_or_default();
            let variables = metric.variables.clone().unwrap_or_default();
            self.check_formula(&expression, &variables, definition, METRIC_FORMULA)?;
            check_metric_window(&expression, &metric.window)?;
        }
        Ok(())
    }

    /// Refuse one formula: a variable that resolves to nothing, an identifier the
    /// expression does not declare, or a declared variable the expression never
    /// names.
    ///
    /// The variables are resolved first — a formula over a property that does not
    /// exist is refused whatever its text says — then the expression is read
    /// against the names the variables declare.
    fn check_formula(
        &self,
        expression: &str,
        variables: &[ExpressionVariable],
        definition: &DefinitionUnderWrite,
        formula: &str,
    ) -> Result<(), String> {
        // The published per-formula quota, checked before anything is resolved:
        // every rule below is then asked of at most this many variables, whatever
        // the request declares.
        if variables.len() > MAX_VARIABLES_PER_EXPRESSION {
            return Err(variable_quota_message(expression, variables.len()));
        }
        for variable in variables {
            self.check_variable(&variable.value, definition, formula)?;
        }
        let named = formula_identifiers(expression);
        // An identifier the expression applies to arguments is a function of the
        // formula language; every other one has to be a variable this formula
        // declares, or one of the language's literals.
        for identifier in &named {
            if identifier.applied
                || FORMULA_LITERALS.contains(&identifier.text.as_str())
                || variables
                    .iter()
                    .any(|variable| variable.name == identifier.text)
            {
                continue;
            }
            return Err(unknown_identifier_message(
                expression,
                &identifier.text,
                identifier.at,
            ));
        }
        // A declared variable the expression never names is unused: the formula
        // does not depend on it, so the definition is refused rather than stored
        // with a variable wired to nothing.
        for variable in variables {
            if !named
                .iter()
                .any(|identifier| identifier.text == variable.name)
            {
                return Err(unused_variable_message(expression, &variable.name));
            }
        }
        Ok(())
    }

    /// Refuse a variable that resolves to nothing: a `hierarchyId` the definition
    /// declares no hierarchy for, a `propertyId` spelled in a form its scope does
    /// not accept, or one its scope does not hold.
    ///
    /// The scope is the one `VariableValue` documents: with no `hierarchyId` the
    /// model being written (its own properties and the ones its composite models
    /// contribute), with one the CHILD model of that hierarchy. Both were indexed
    /// when the definition was built, so this is a set lookup — and the child
    /// scope was indexed under the spellings it accepts, which do not include a
    /// property NAME ([`Spellings`]): a reference reaching it has to be an
    /// identifier, so one that is not is refused as an id invalid for the variable
    /// rather than as a property the scope lacks. A variable carrying no
    /// `propertyId` names no property to resolve — it addresses the property
    /// through its `propertyPath` instead — so there is nothing to refuse.
    ///
    /// `formula` is the formula member the variable belongs to, which the
    /// invalid-spelling refusal names.
    fn check_variable(
        &self,
        value: &VariableValue,
        definition: &DefinitionUnderWrite,
        formula: &str,
    ) -> Result<(), String> {
        match value.hierarchy_id.as_deref() {
            Some(hierarchy_label) => {
                // A model declares at most `MAX_HIERARCHIES_PER_ASSET_MODEL` of
                // them, so the label is resolved over a list of that size.
                let hierarchy = definition
                    .hierarchies
                    .iter()
                    .find(|hierarchy| names_hierarchy(hierarchy, hierarchy_label))
                    .ok_or_else(|| hierarchy_does_not_exist_message(hierarchy_label))?;
                let Some(property_label) = value.property_id.as_deref() else {
                    return Ok(());
                };
                if !spelled_as_property_id(property_label) {
                    return Err(invalid_property_id_message(property_label, formula));
                }
                if definition.child_holds_property(hierarchy, property_label) {
                    return Ok(());
                }
                Err(property_does_not_exist_message(property_label))
            }
            None => {
                let Some(property_label) = value.property_id.as_deref() else {
                    return Ok(());
                };
                if definition.holds_property(property_label) {
                    return Ok(());
                }
                Err(property_does_not_exist_message(property_label))
            }
        }
    }
}

/// Refuse a metric window whose tumbling interval names a span outside the
/// published limits — or no span at all, which is a span the limits cannot hold
/// either.
///
/// The refusal names the metric's own expression, because the window belongs to
/// that formula.
pub fn check_metric_window(expression: &str, window: &MetricWindow) -> Result<(), String> {
    let Some(tumbling) = window.tumbling.as_ref() else {
        return Ok(());
    };
    let within = window_span(&tumbling.interval)
        .is_some_and(|span| (MIN_TUMBLING_INTERVAL..=MAX_TUMBLING_INTERVAL).contains(&span));
    if within {
        Ok(())
    } else {
        Err(window_interval_message(expression, &tumbling.interval))
    }
}
