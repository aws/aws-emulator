// SPDX-License-Identifier: CC0-1.0

//! Case rules: the resource `CreateCaseRule` mints, a template references and a
//! batch read reports.
//!
//! A case rule is a resource of its own, scoped to a domain exactly as that
//! domain's fields, templates and layouts are — the model gives it an ARN
//! template of `domain/{domainId}/case-rule/{caseRuleId}`
//! (`smithy_query(query="shape", shape="CaseRule")`) — so the record lives under
//! [`Domain`] and everything a rule needs from the store lives here rather than
//! in one operation's file: the create, the batch read and a template that
//! references a rule all resolve the same records through these helpers.
//!
//! What a rule holds is a `CaseRuleDetails` union the caller composed, and its
//! `required` and `hidden` variants nest boolean conditions without end, so the
//! walk a write performs over them is bounded here
//! ([`case_rule_conditions_too_complex`]) before any field reference it carries
//! is resolved ([`case_rule_field_ids`]).

use super::*;

use crate::ops::types::{
    BooleanCondition, CaseRuleDetails, CaseRuleSummary, GetCaseRuleResponse, OperandOne,
    TemplateRule,
};

/// Most case rules one domain holds. EMULATOR-OWNED ceiling on request-driven
/// growth, as [`MAX_DOMAINS`]: the real service's limit is an account quota a
/// support request can raise, while this one exists so a caller cannot grow the
/// in-memory store without end.
pub const MAX_CASE_RULES_PER_DOMAIN: usize = 500;

/// Most case rule identifiers one `BatchGetCaseRule` request may carry: the
/// model's `BatchGetCaseRuleRequest$caseRules` cap of `1..50`
/// (`smithy_query(query="operation", operation="BatchGetCaseRule")`), re-checked
/// in the handler for the same reason as [`MAX_CASE_FIELD_IDS`]. It also bounds
/// the response, whose `caseRules` and `errors` lists carry the same `..50` cap.
pub const MAX_BATCH_CASE_RULE_IDS: usize = 50;

/// Most case rules one template may reference: the model's
/// `CreateTemplateRequest$rules` / `UpdateTemplateRequest$rules` cap of `..50`
/// (`smithy_query(query="operation", operation="CreateTemplate,UpdateTemplate")`),
/// re-checked in the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_TEMPLATE_CASE_RULES: usize = 50;

/// Entries one `BooleanConditionList` may carry: the model's cap of `..100`
/// (`smithy_query(query="shape", shape="BooleanConditionList")`), re-checked in
/// the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_CASE_RULE_CONDITIONS_PER_LIST: usize = 100;

/// Most parent-to-child option mappings one field-options rule may carry: the
/// model's `FieldOptionsCaseRule$parentChildFieldOptionsMappings` cap of
/// `1..200` (`smithy_query(query="shape", shape="FieldOptionsCaseRule")`),
/// re-checked in the handler for the same reason as [`MAX_CASE_FIELD_IDS`].
pub const MAX_PARENT_CHILD_FIELD_OPTIONS_MAPPINGS: usize = 200;

/// Most clauses ONE case rule's condition tree may hold in all. A
/// `BooleanCondition` nests through `andAll` and `orAll` without a modelled
/// bound on its depth, so a tree of `MAX_CASE_RULE_CONDITIONS_PER_LIST`-wide
/// lists grows its clause count multiplicatively with its depth; the write walks
/// every clause to resolve the field references it carries, so the total is
/// bounded here as well, exactly as [`MAX_CASE_FILTER_CLAUSES`] bounds a search
/// filter's tree.
pub const MAX_CASE_RULE_CONDITION_CLAUSES: usize = 100;

/// Deepest `BooleanCondition` nesting a case rule may carry, bounding the walk a
/// single request can drive for the same reason as
/// [`MAX_CASE_RULE_CONDITION_CLAUSES`].
pub const MAX_CASE_RULE_CONDITION_DEPTH: usize = 10;

/// A case rule: "what rule type should take place, under what conditions"
/// (`smithy_query(query="doc", shape="CaseRuleDetails")`), as the create wrote
/// it.
#[derive(Debug, Clone)]
pub struct CaseRule {
    pub case_rule_id: String,
    pub case_rule_arn: String,
    pub name: String,
    pub description: Option<String>,
    /// The rule exactly as the write gave it, in the union variant it arrived
    /// in, so a read hands back the record rather than a re-encoding of it.
    pub rule: CaseRuleDetails,
    /// Whether the rule has been deleted. `GetCaseRuleResponse` carries a
    /// `deleted` member (`smithy_query(query="shape",
    /// shape="GetCaseRuleResponse")`), so a read reports the flag on the record
    /// rather than the record's absence.
    pub deleted: bool,
    /// The rule's tags, as [`Field::tags`]: `CreateCaseRule` takes no tags, so a
    /// rule carries none until it is tagged and a read reports an EMPTY tag map
    /// for it until then ([`tags_output`]).
    pub tags: HashMap<String, String>,
    pub created_time: SystemTime,
    pub last_modified_time: SystemTime,
}

/// The ARN of a case rule of a domain, as the model's `CaseRule` resource
/// template spells its path: `domain/{domainId}/case-rule/{caseRuleId}`
/// (`smithy_query(query="shape", shape="CaseRule")`).
pub fn case_rule_arn(region: &str, domain_id: &str, case_rule_id: &str) -> String {
    ::engine::emulator_arn(
        ARN_SERVICE,
        region,
        &format!("domain/{domain_id}/case-rule/{case_rule_id}"),
    )
}

impl Domain {
    /// The RECORD this domain holds for `case_rule_id`, live or deleted, or
    /// `None` when it holds none at all: what the batch read reports, with
    /// [`CaseRule::deleted`] saying which of the two it is.
    pub fn case_rule_record(&self, case_rule_id: &str) -> Option<&CaseRule> {
        self.case_rules.get(case_rule_id)
    }

    /// The LIVE case rule `case_rule_id` names, for writing, or the modelled
    /// 404 for it. A rule that has been deleted is not one a caller can change,
    /// exactly as [`Domain::template`] treats a deleted template — the record
    /// survives for the batch read only ([`Self::case_rule_record`]).
    pub fn case_rule_mut(
        &mut self,
        case_rule_id: &str,
    ) -> Result<&mut CaseRule, ResourceNotFoundException> {
        self.case_rules
            .get_mut(case_rule_id)
            .filter(|rule| !rule.deleted)
            .ok_or_else(|| not_found(ResourceKind::CaseRule, case_rule_id))
    }

    /// Every case rule this domain still holds, in creation order and each
    /// paired with the key [`order_key`] builds for it, with the deleted ones
    /// left out: what a listing of case rules reports. A deleted rule is left
    /// out of the LISTING only — [`Self::case_rule_record`] still reports the
    /// record, exactly as [`Self::live_templates`] treats a deleted template.
    pub fn live_case_rules(&self) -> Vec<(String, &CaseRule)> {
        let mut rules: Vec<(String, &CaseRule)> = self
            .case_rules
            .values()
            .filter(|rule| !rule.deleted)
            .map(|rule| (order_key(rule.created_time, &rule.case_rule_id), rule))
            .collect();
        rules.sort_unstable_by(|(left, _), (right, _)| left.cmp(right));
        rules
    }

    /// Refuse a new case rule once the domain holds
    /// [`MAX_CASE_RULES_PER_DOMAIN`] of them.
    pub fn case_rule_capacity(&self) -> Result<(), ServiceQuotaExceededException> {
        capacity(
            self.case_rules.len(),
            MAX_CASE_RULES_PER_DOMAIN,
            "case rules in this domain",
        )
    }

    /// Whether a LIVE case rule of this domain already holds `name`: what a
    /// create of a rule is refused on, as a create of a field or a template is.
    pub fn case_rule_name_taken(&self, name: &str) -> bool {
        self.case_rules
            .values()
            .any(|rule| !rule.deleted && rule.name == name)
    }

    /// The ids among `ids` that this domain holds no live case rule for, in the
    /// order they were given and without repeats. Empty means every reference
    /// resolves. Linear in the number of ids: one hash lookup each.
    ///
    /// What a request that takes out a NEW reference to a case rule — a
    /// template's `rules` — resolves against: a deleted rule is not one a caller
    /// can reference, exactly as [`Domain::unknown_field_ids`] treats a deleted
    /// field.
    pub fn unknown_case_rule_ids<'a, I: IntoIterator<Item = &'a str>>(&self, ids: I) -> Vec<String> {
        let mut reported: HashSet<&str> = HashSet::new();
        let mut unknown: Vec<String> = Vec::new();
        for id in ids {
            let resolves = self
                .case_rules
                .get(id)
                .is_some_and(|rule: &CaseRule| !rule.deleted);
            if !resolves && reported.insert(id) {
                unknown.push(id.to_string());
            }
        }
        unknown
    }
}

/// `Some(message)` when a case rule asks for more work than one request may,
/// else `None`. `member` is the request member the rule arrived in, so the
/// refusal names what a client has to fix.
///
/// The `required` and `hidden` variants carry a `BooleanConditionList` that
/// nests through `andAll` and `orAll` with no modelled bound on its depth, so
/// both the depth and the total clause count are held to
/// [`MAX_CASE_RULE_CONDITION_DEPTH`] and [`MAX_CASE_RULE_CONDITION_CLAUSES`],
/// and each list to the model's own [`MAX_CASE_RULE_CONDITIONS_PER_LIST`]. The
/// walk is iterative and stops at the first breach, so checking a rule is itself
/// bounded by those constants.
pub fn case_rule_conditions_too_complex(member: &str, rule: &CaseRuleDetails) -> Option<String> {
    let top = match rule {
        CaseRuleDetails::Required(required) => &required.conditions,
        CaseRuleDetails::Hidden(hidden) => &hidden.conditions,
        // A field-options rule carries no conditions at all: its work is the
        // mapping list the model already caps at `1..200`, which is re-checked
        // where the rule is written.
        CaseRuleDetails::fieldOptions(_) => return None,
    };
    let mut pending: Vec<(&Vec<BooleanCondition>, usize)> = vec![(top, 1)];
    let mut clauses: usize = 0;
    while let Some((conditions, depth)) = pending.pop() {
        if depth > MAX_CASE_RULE_CONDITION_DEPTH {
            return Some(format!(
                "{member} nests deeper than {MAX_CASE_RULE_CONDITION_DEPTH} levels; at most that \
                 many are accepted in one request"
            ));
        }
        if let Some(message) =
            list_too_long(member, conditions.len(), MAX_CASE_RULE_CONDITIONS_PER_LIST)
        {
            return Some(message);
        }
        for condition in conditions {
            clauses += 1;
            if clauses > MAX_CASE_RULE_CONDITION_CLAUSES {
                return Some(format!(
                    "{member} carries more than {MAX_CASE_RULE_CONDITION_CLAUSES} conditions; at \
                     most that many are accepted in one request"
                ));
            }
            match condition {
                BooleanCondition::equalTo(_) | BooleanCondition::notEqualTo(_) => {}
                BooleanCondition::andAll(compound) | BooleanCondition::orAll(compound) => {
                    pending.push((&compound.conditions, depth + 1));
                }
            }
        }
    }
    None
}

/// Every field id a case rule references, so they can be resolved against the
/// domain's own schema before the rule is stored: the fields its conditions
/// compare on, and the parent and child fields a field-options rule maps
/// between. Iterative, and run only after
/// [`case_rule_conditions_too_complex`] has bounded the tree.
pub fn case_rule_field_ids(rule: &CaseRuleDetails) -> Vec<String> {
    let conditions = match rule {
        CaseRuleDetails::Required(required) => &required.conditions,
        CaseRuleDetails::Hidden(hidden) => &hidden.conditions,
        CaseRuleDetails::fieldOptions(options) => {
            return options
                .parent_field_id
                .iter()
                .chain(options.child_field_id.iter())
                .cloned()
                .collect();
        }
    };
    let mut ids: Vec<String> = Vec::new();
    let mut pending: Vec<&Vec<BooleanCondition>> = vec![conditions];
    while let Some(conditions) = pending.pop() {
        for condition in conditions {
            match condition {
                BooleanCondition::equalTo(operands) | BooleanCondition::notEqualTo(operands) => {
                    // `OperandOne` is a union with one member, `fieldId`: the
                    // left-hand side of a case rule's comparison is always a
                    // field of the domain
                    // (`smithy_query(query="shape", shape="OperandOne")`).
                    let OperandOne::fieldId(field_id) = &operands.operand_one;
                    ids.push(field_id.clone());
                }
                BooleanCondition::andAll(compound) | BooleanCondition::orAll(compound) => {
                    pending.push(&compound.conditions);
                }
            }
        }
    }
    ids
}

/// Every field id a template's `rules` references: the field each association
/// names the rule as acting upon, where it names one (`TemplateRule$fieldId` is
/// optional, `smithy_query(query="shape", shape="TemplateRule")`).
pub fn template_rule_field_ids(rules: &[TemplateRule]) -> Vec<String> {
    rules
        .iter()
        .filter_map(|rule| rule.field_id.clone())
        .collect()
}

/// The sentence the service refuses a BATCH READ of case rules with when the
/// identifiers it was given are not case-rule identifiers at all. Fixed text,
/// interpolating nothing: the call is refused as a whole, and the offending
/// identifiers are deliberately not echoed back.
pub const INVALID_CASE_RULE_IDS_MESSAGE: &str = "The CaseRuleId(s) provided are not valid";

/// Whether `id` has the FORM of a case-rule identifier: the 8-4-4-4-12
/// hexadecimal form every identifier this service mints takes ([`new_id`]).
///
/// The model constrains `CaseRuleId` by length alone (`1..500`,
/// `smithy_query(query="shape", shape="CaseRuleId")`), so an identifier of any
/// other form passes the boundary validator and reaches the handler. Such an
/// identifier names no rule the service could ever have minted, so it is a bad
/// REQUEST rather than an identifier the domain happens to hold no rule for —
/// which is why it is refused ([`invalid_case_rule_ids`]) instead of being
/// reported per item.
///
/// Only the form is checked here, in either hexadecimal case; the lookup that
/// follows a well-formed identifier is exact, so a well-formed identifier the
/// domain holds no record for is still reported per item.
pub fn case_rule_id_is_well_formed(id: &str) -> bool {
    let groups: Vec<&str> = id.split('-').collect();
    groups.len() == 5
        && groups.iter().map(|group| group.len()).eq([8, 4, 4, 4, 12])
        && groups
            .iter()
            .all(|group| group.bytes().all(|byte| byte.is_ascii_hexdigit()))
}

/// The modelled 400 a batch read of case rules refuses a request whose
/// identifiers are not well-formed ([`case_rule_id_is_well_formed`]) with.
pub fn invalid_case_rule_ids() -> ValidationException {
    ValidationException {
        message: INVALID_CASE_RULE_IDS_MESSAGE.to_string(),
    }
}

/// The modelled 400 for a request whose `member` CONFIGURES a resource with
/// references to the domain's case rules — a template's `rules` — and names ids
/// the domain holds no live rule for.
///
/// The same frame as [`unknown_fields_in`], which refuses an unresolvable FIELD
/// reference the same way: the member the references rode in, then the ids the
/// service could not resolve, taken from the REQUEST verbatim and rendered by
/// [`name_ids`] so one request cannot make the refusal grow without bound.
pub fn unknown_case_rules_in(member: &str, ids: &[String]) -> ValidationException {
    ValidationException {
        message: format!(
            "The following case rule IDs provided in {member} do not exist: {}",
            name_ids(ids)
        ),
    }
}

/// The `RuleType` of a stored rule: the member of the `CaseRuleDetails` union it
/// holds, which is what names its type.
///
/// The model names the three members of `CaseRuleDetails` after the three values
/// of `RuleType`
/// (`smithy_query(query="shape", shape="CaseRuleDetails,RuleType")`), so the
/// rule body itself says which type it is and a summary reports it from the
/// record rather than from anything the request said. One of
/// `RULE_TYPE_VALUES`.
pub fn case_rule_type(rule: &CaseRuleDetails) -> &'static str {
    match rule {
        CaseRuleDetails::Required(_) => "Required",
        CaseRuleDetails::Hidden(_) => "Hidden",
        CaseRuleDetails::fieldOptions(_) => "FieldOptions",
    }
}

/// One stored case rule as a LISTING reports it: the identity of the record, its
/// type and its description — the projection `CaseRuleSummary` declares
/// (`smithy_query(query="shape", shape="CaseRuleSummary")`), which carries the
/// rule's type rather than the rule body itself.
pub fn case_rule_summary(rule: &CaseRule) -> CaseRuleSummary {
    CaseRuleSummary {
        case_rule_id: rule.case_rule_id.clone(),
        name: rule.name.clone(),
        case_rule_arn: rule.case_rule_arn.clone(),
        rule_type: case_rule_type(&rule.rule).to_string(),
        description: rule.description.clone(),
    }
}

/// One stored case rule as a read reports it: the record, with its instants
/// rendered in the model's `date-time` form and its tag set present whatever it
/// holds.
///
/// The ONE projection of a case rule, so the batch read and any later reader
/// cannot drift apart.
pub fn case_rule_output(rule: &CaseRule) -> GetCaseRuleResponse {
    GetCaseRuleResponse {
        case_rule_id: rule.case_rule_id.clone(),
        name: rule.name.clone(),
        case_rule_arn: rule.case_rule_arn.clone(),
        rule: rule.rule.clone(),
        description: rule.description.clone(),
        deleted: rule.deleted,
        created_time: Some(timestamp(rule.created_time)),
        last_modified_time: Some(timestamp(rule.last_modified_time)),
        tags: tags_output(&rule.tags),
    }
}
