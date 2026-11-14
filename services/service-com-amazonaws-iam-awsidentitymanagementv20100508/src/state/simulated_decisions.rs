// SPDX-License-Identifier: CC0-1.0

//! What a policy SIMULATION decides, and the bounds one simulation request is answered
//! within.
//!
//! The two operations that run a simulation — `SimulateCustomPolicy` and
//! `SimulatePrincipalPolicy` — answer the SAME question: for each `(action, resource)`
//! pair a request names, what do the policies in force decide, and which of the
//! documents handed to the simulation determined it. They differ only in where those
//! documents come from — the request alone for the first, the principal the request
//! names TOGETHER with the request's own for the second — so the decision, the report
//! it is answered as and the bounds one request is admitted within live here once, and
//! each operation supplies the documents and names them in its own vocabulary.
//!
//! Everything here is a READ: a simulation reports what the documents decide and writes
//! nothing — simulating a call neither performs it nor records it.
//!
//! The decision itself is [`::service_common::iam_authorization`], the tree's ONE
//! evaluation of IAM policy documents against `(action, resource)`, so a simulation and
//! this service's own request-time gate cannot disagree about what a document grants.

use ::service_common::iam_authorization::{self as iam, PreparedDocument};

use crate::ops::types::{
    ContextEntry, EvaluationResult, OrganizationsDecisionDetail,
    PermissionsBoundaryDecisionDetail, ResourceSpecificResult, Statement,
};

use super::authorization::{DECISION_ALLOWED, DECISION_EXPLICIT_DENY, DECISION_IMPLICIT_DENY};

/// The resource a simulation is about when the request names none: "if this parameter
/// is not provided, then the value defaults to * (all resources)" (`ResourceArns`).
pub const ANY_RESOURCE: &str = "*";

/// How many resources one request may ask to have decided.
///
/// The model bounds the number of RESULTS a page carries (`maxItemsType`, range
/// 1..1000) but puts no bound on `ResourceArns`, and the work a simulation does is the
/// PRODUCT of the actions, the resources and the documents named — so the same ceiling
/// is applied to the resource axis, and a request past it is refused rather than
/// served. Nothing is silently dropped: the refusal says what the bound is.
const MAX_SIMULATED_RESOURCES: usize = 1_000;

/// How many policy documents one request may supply in one list, for the same reason
/// [`MAX_SIMULATED_RESOURCES`] exists: the documents are the third factor of that
/// product, each may be 131,072 characters, and every one of them is walked for every
/// pair the page decides.
const MAX_SIMULATED_DOCUMENTS: usize = 10;

/// How many LEVELS of an Organizations hierarchy one request may supply, which is the
/// bound the member's own target states: `OrganizationPolicyListType` is length 0..7
/// (`smithy_query(query="shape",shape="OrganizationPolicyListType")`).
///
/// It is counted beside [`MAX_SIMULATED_DOCUMENTS`], not covered by it: that cap measures
/// the DOCUMENTS the levels carry, and a level carrying none contributes nothing to it,
/// while every level is walked for every pair the page decides
/// ([`organization_outcome`]) — so without this the work would be the page's pairs times a
/// factor the request alone chooses.
const MAX_SIMULATED_ORGANIZATION_LEVELS: usize = 7;

/// How many (action, resource) pairs one PAGE may decide. The page's action count is
/// narrowed so this holds, which bounds both the work the request does and the size of
/// the response it gets back; the actions that do not fit are reported on the next
/// page.
const MAX_EVALUATIONS_PER_PAGE: usize = 1_000;

/// How many pairs are decided between two yields back to the async executor, so a
/// simulation cannot hold a worker thread for its whole run.
const EVALUATIONS_PER_YIELD: usize = 64;

/// How many context keys one simulation may report as missing a value.
///
/// The list is reported on every result the page carries, so its length multiplies the
/// response's size; the keys come from the conditions the deciding documents carry, which
/// the request itself can supply. Past this bound the request is refused rather than
/// answered.
const MAX_SIMULATED_CONTEXT_KEYS: usize = 100;

/// How long a context key NAME a report of one may carry, which is the bound the member
/// naming a context key states: `ContextKeyNameType` is length 5..256
/// (`smithy_query(query="shape",shape="ContextKeyNameType")`), so a longer name is no
/// context key name and no request could have supplied a value for it.
///
/// It is what bounds the SIZE of a report beside the COUNT
/// [`MAX_SIMULATED_CONTEXT_KEYS`] bounds: the names come out of the documents the request
/// supplies, each of which may be 131,072 characters, and the keys one pair needed a value
/// for are kept for the whole page ([`decide`]) — so without a bound on the name the page's
/// retention would be its pairs times what those documents carry rather than times a
/// constant of this module.
const MAX_CONTEXT_KEY_NAME_CHARACTERS: usize = 256;

/// How many condition values one request may supply in `ContextEntries` — counted over
/// every value of every entry, since each is one value the decision may test against.
/// Bounded for the same reason the other axes are: the keys the documents test are
/// looked up among them.
const MAX_SIMULATED_CONTEXT_VALUES: usize = 1_000;

/// How a `MatchedStatements` entry names an identity-based policy the simulation decided
/// from (`Statement$SourcePolicyType`, "the type of the policy").
///
/// The two kinds of document a simulation decides from are the ones the operations take:
/// the identity-based policies (a simulated principal's own and the ones supplied in
/// `PolicyInputList`) and the single resource-based policy supplied in `ResourcePolicy` —
/// which is the discriminator the member exists to give, "which policy type is
/// responsible for an allow or deny decision".
const IDENTITY_POLICY_SOURCE: &str = "IAM Policy";

/// How a `MatchedStatements` entry names the resource-based policy the request supplied.
/// See [`IDENTITY_POLICY_SOURCE`].
const RESOURCE_POLICY_SOURCE: &str = "Resource Policy";

/// How a `MatchedStatements` entry identifies the resource-based policy the request
/// supplied (`Statement$SourcePolicyId`, "the identifier of the policy that was provided
/// as an input"): the input member it arrived in, of which there is exactly one.
const RESOURCE_POLICY_ID: &str = "ResourcePolicy";

/// How a `MatchedStatements` entry identifies a document the request supplied in
/// `PolicyInputList`: that member and the document's position in it, counted from one.
pub const SUPPLIED_POLICY_ID: &str = "PolicyInputList";

/// The simulation scenarios `ResourceHandlingOption` names, each with the resources the
/// request must define to run it.
///
/// "By specifying the type of simulation to run, you enable the policy simulator to
/// enforce the presence of the required resources to ensure reliable simulation results.
/// If your simulation does not match one of the following scenarios, then you can omit
/// this parameter" — so the admissible values are the scenarios the member documents,
/// and each is served only once the resources that scenario lists are named. The
/// scenarios and their resources are the member's own
/// (`smithy_query(query="doc",shape="SimulateCustomPolicyRequest",member="ResourceHandlingOption")`),
/// each resource written as the TYPE segment of its ARN, which is how a request names
/// one.
const SIMULATION_SCENARIOS: &[(&str, &[&str])] = &[
    (
        "EC2-VPC-InstanceStore",
        &["instance", "image", "security-group", "network-interface"],
    ),
    (
        "EC2-VPC-InstanceStore-Subnet",
        &[
            "instance",
            "image",
            "security-group",
            "network-interface",
            "subnet",
        ],
    ),
    (
        "EC2-VPC-EBS",
        &[
            "instance",
            "image",
            "security-group",
            "network-interface",
            "volume",
        ],
    ),
    (
        "EC2-VPC-EBS-Subnet",
        &[
            "instance",
            "image",
            "security-group",
            "network-interface",
            "subnet",
            "volume",
        ],
    ),
];

/// How many digits an Amazon Web Services account id is written with, which is the form
/// `ResourceOwner` names an account by when it names one bare rather than as the
/// account's root ARN.
const ACCOUNT_ID_DIGITS: usize = 12;

/// What one simulation request supplies, as the shared admission below reads it.
///
/// Every member here is one both simulation operations take; a member only one of them
/// models (a principal's ARN, the policies to exclude, the SCP hierarchy) is checked by
/// that operation.
pub struct SimulationInput<'a> {
    /// The identity-based documents the request supplied (`PolicyInputList`).
    pub documents: &'a [String],
    /// The permissions boundary it supplied (`PermissionsBoundaryPolicyInputList`).
    pub boundary: &'a [String],
    /// The service control policies it supplied, one entry per level of the
    /// Organizations hierarchy (`OrderedOrganizationPolicyInputList`) — empty for an
    /// operation that takes none.
    pub organization_levels: &'a [Vec<String>],
    /// The resource-based policy it supplied (`ResourcePolicy`).
    pub resource_policy: Option<&'a str>,
    /// The resources it asks about (`ResourceArns`).
    pub resource_arns: &'a [String],
    /// The condition values it supplied (`ContextEntries`).
    pub context_entries: Option<&'a [ContextEntry]>,
    /// The actions it asks about (`ActionNames`).
    pub action_names: &'a [String],
    /// The simulation scenario it declared (`ResourceHandlingOption`).
    pub resource_handling_option: Option<&'a str>,
}

/// Why `request` cannot be simulated, as the sentence to refuse it with, or `None` when
/// it can.
///
/// The refusal is the modelled `InvalidInputException` of whichever operation asked —
/// "an invalid or out-of-range value was supplied for an input parameter" — which is why
/// this answers the sentence rather than an error: each operation lifts it into its own
/// error channel.
pub fn request_defect(request: &SimulationInput<'_>) -> Option<String> {
    // Every supplied document has to be a policy document before anything is decided
    // from it: the rule is the shared one every operation taking a policy document applies
    // (`crate::state::policy_document_defect`). An entry of `PolicyInputList` is refused by
    // its POSITION in that list, counted from one
    // (`crate::state::policy_input_list_item_invalid`), because the member carries several
    // documents and the caller has to be told which of them was rejected.
    for (position, document) in (1..).zip(request.documents) {
        if super::policy_document_defect(document).is_some() {
            return Some(super::policy_input_list_item_invalid(position));
        }
    }
    // Every other IDENTITY-side document the request supplies is refused with the fixed
    // sentence the shared validator words, which carries neither the member that supplied
    // the document nor anything read out of it.
    let documents = request
        .boundary
        .iter()
        .chain(request.organization_levels.iter().flatten())
        .map(String::as_str);
    for document in documents {
        if let Some(defect) = super::policy_document_defect(document) {
            return Some(defect.message().to_string());
        }
    }
    // "A resource-based policy to include in the simulation provided as a string"
    // (`ResourcePolicy`): what that member takes is a policy of one CLASS — one that can
    // be attached to a resource — so the document is read by the tree's one reading of that
    // class (`service_common::iam_policy::resource_policy_defect`, which every service
    // handed such a document reads it by, so two of them cannot disagree about whether a
    // document is one), and a document breaking any clause of it is refused as exactly
    // that, in the one sentence IAM words that refusal in
    // (`super::INVALID_RESOURCE_POLICY`), rather than through the identity-policy
    // validator's parse-failure sentence: the sentence is the only thing that tells the two
    // classes apart behind the one error code both are refused with.
    if request.resource_policy.is_some_and(|document| {
        ::service_common::iam_policy::resource_policy_defect(document).is_some()
    }) {
        return Some(super::INVALID_RESOURCE_POLICY.to_string());
    }
    // "An IAM entity can only have one permissions boundary in effect at a time. You can
    // input only one permissions boundary when you pass a policy to this operation."
    if request.boundary.len() > 1 {
        return Some(format!(
            "Invalid value for PermissionsBoundaryPolicyInputList: only one permissions \
             boundary may be simulated, got {}",
            request.boundary.len()
        ));
    }
    // How much one request may ask to be decided is bounded, because the work is the
    // PRODUCT of the actions, the resources and the documents it names: past either cap
    // the request is refused rather than served, so a single call cannot make a
    // simulation do unbounded work.
    for (member, named, cap) in [
        (
            "PolicyInputList",
            request.documents.len(),
            MAX_SIMULATED_DOCUMENTS,
        ),
        (
            "ResourceArns",
            request.resource_arns.len(),
            MAX_SIMULATED_RESOURCES,
        ),
        (
            "ContextEntries",
            supplied_context_values(request.context_entries),
            MAX_SIMULATED_CONTEXT_VALUES,
        ),
        (
            "OrderedOrganizationPolicyInputList",
            request.organization_levels.len(),
            MAX_SIMULATED_ORGANIZATION_LEVELS,
        ),
        (
            "OrderedOrganizationPolicyInputList",
            request.organization_levels.iter().map(Vec::len).sum(),
            MAX_SIMULATED_DOCUMENTS,
        ),
    ] {
        if named > cap {
            return Some(format!(
                "Invalid value for {member}: at most {cap} may be simulated in one \
                 request, got {named}"
            ));
        }
    }

    // "Each operation must include the service identifier, such as iam:CreateUser": an
    // action name with no service prefix names no action, so nothing can be decided
    // about it.
    if let Some(action) = request
        .action_names
        .iter()
        .find(|action| !names_a_service_action(action))
    {
        return Some(format!(
            "Invalid Action name: {action} must name a service and an action, as in \
             iam:CreateUser"
        ));
    }

    // "A list of ARNs of Amazon Web Services resources to include in the simulation …
    // Each API in the ActionNames parameter is evaluated for each resource in this list":
    // an entry that is neither an ARN nor the wildcard the member defaults to names no
    // resource the simulation could decide a statement's `Resource` element against, so
    // nothing can be reported about it. The refusal names every such entry, in the order
    // the request carries them, because the member is a list and the caller has to be told
    // which of its values are not resource names.
    let malformed: Vec<&str> = request
        .resource_arns
        .iter()
        .filter(|named| {
            named.as_str() != ANY_RESOURCE && ::service_common::arn::parse(named).is_none()
        })
        .map(String::as_str)
        .collect();
    if !malformed.is_empty() {
        return Some(format!(
            "ResourceNames are not in a valid ARN format: {}.",
            malformed.join(", ")
        ));
    }

    // "Each resource in the simulation is treated as if it had this policy attached"
    // (`ResourcePolicy`): a simulation that supplies a resource-based policy decides it
    // for the resources the request names, so each of them has to name ONE resource for
    // that policy to be attached to. A resource set standing for every resource names
    // none — whether the request spells the wildcard or leaves the member out, since
    // "if this parameter is not provided, then the value defaults to * (all resources)"
    // (`ResourceArns`) — and is refused by the reading every member that must name one
    // thing shares (`super::names_no_single_entity`).
    if request.resource_policy.is_some() {
        let unattachable = request
            .resource_arns
            .iter()
            .map(String::as_str)
            .find(|named| super::names_no_single_entity(named))
            .or_else(|| request.resource_arns.is_empty().then_some(ANY_RESOURCE));
        if let Some(named) = unattachable {
            return Some(super::entity_arn_not_one_entity_message(named));
        }
    }

    // The scenario a request declares is one of the ones the member publishes, and it is
    // run only once the resources that scenario needs are named — the enforcement the
    // member exists to ask for ([`SIMULATION_SCENARIOS`]).
    if let Some(scenario) = request.resource_handling_option {
        let Some((_, required)) = SIMULATION_SCENARIOS
            .iter()
            .find(|(published, _)| *published == scenario)
        else {
            return Some(format!(
                "Invalid value for ResourceHandlingOption: {scenario} is not a \
                 simulation scenario"
            ));
        };
        if let Some(missing) = required.iter().find(|kind| {
            !request
                .resource_arns
                .iter()
                .any(|arn| ::service_common::arn::resource_type(arn) == Some(*kind))
        }) {
            return Some(format!(
                "Invalid value for ResourceArns: the {scenario} simulation scenario \
                 requires a resource of type {missing}"
            ));
        }
    }
    None
}

/// Where the page a request asks for begins: the POSITION its marker names, or the start
/// of the results when it carries none.
///
/// `None` is a token this service's encoding cannot have produced, which the operation
/// refuses rather than reads as the start of the results — exactly as this crate's list
/// operations do with theirs.
pub fn resume_position(marker: Option<&str>) -> Option<usize> {
    match marker {
        Some(marker) => super::decode_marker(super::MARKER_SCOPE_SIMULATION_RESULTS, marker)
            .and_then(|key| key.parse::<usize>().ok()),
        None => Some(0),
    }
}

/// How many actions one page decides: the request's `MaxItems`, narrowed so the page's
/// (action, resource) pairs stay within [`MAX_EVALUATIONS_PER_PAGE`].
///
/// Answering fewer results than asked for is what the model provides for — "IAM might
/// return fewer than the MaxItems number of results even when there are more results
/// available. In that case, the IsTruncated response element returns true, and Marker
/// contains a value to include in the subsequent call" — so the rest of the actions are
/// reported on the next page rather than dropped, and neither the work nor the response
/// grows with the product.
///
/// `maxItemsType` is range 1..1000 and the boundary validator has already refused
/// anything outside it, so the only choice left here is the default for an absent
/// `MaxItems`. `.max(1)` is what makes the cast lossless.
pub fn page_size(max_items: Option<i32>, resources: usize) -> usize {
    let asked = max_items.unwrap_or(super::DEFAULT_MAX_ITEMS).max(1) as usize;
    asked.min((MAX_EVALUATIONS_PER_PAGE / resources.max(1)).max(1))
}

/// Whether the resources a simulation is about are owned by an account OTHER than the
/// simulated caller's.
///
/// "An ARN representing the Amazon Web Services account ID that specifies the owner of
/// any simulated resource that does not identify its owner in the resource ARN … If the
/// ResourceOwner parameter is not specified, then the owner of the resources and the
/// resource policy defaults to the account of the identity provided in CallerArn"
/// (`ResourceOwner`) — so the owner is the account that member names, or the caller's own
/// when it names none, and a simulation whose two accounts differ is the cross-account
/// one the member exists for ([`Simulated::cross_account`]). A value naming no account
/// is answered the same way as an absent one, since it says nothing about who owns the
/// resources.
pub fn cross_account(resource_owner: Option<&str>, caller_arn: &str) -> bool {
    let Some(owner) = resource_owner.and_then(account_of) else {
        return false;
    };
    account_of(caller_arn).is_some_and(|caller| caller != owner)
}

/// The account `value` names: the account segment of an ARN — "the ARN for an account
/// uses the following syntax: arn:aws:iam::AWS-account-ID:root" — or the account id
/// itself where that is what the value is. `None` for a value that names neither.
fn account_of(value: &str) -> Option<String> {
    if let Some(parsed) = ::service_common::arn::parse(value) {
        return (!parsed.account.is_empty()).then(|| parsed.account.to_string());
    }
    (value.len() == ACCOUNT_ID_DIGITS && value.chars().all(|digit| digit.is_ascii_digit()))
        .then(|| value.to_string())
}

/// The condition values a request supplies, as the shared evaluation takes them: one
/// `(key, value)` pair per value, since a key tested against any of its values holds.
pub fn supplied_context(entries: Option<&[ContextEntry]>) -> Vec<(String, String)> {
    let mut context = Vec::new();
    for entry in entries.unwrap_or_default() {
        let Some(key) = entry.context_key_name.as_deref() else {
            continue;
        };
        for value in entry.context_key_values.iter().flatten() {
            context.push((key.to_string(), value.clone()));
        }
    }
    context
}

/// How many condition values `entries` carries in all — the measure
/// [`MAX_SIMULATED_CONTEXT_VALUES`] is taken against, counted from the request's own
/// lists before anything is built from them.
fn supplied_context_values(entries: Option<&[ContextEntry]>) -> usize {
    entries
        .unwrap_or_default()
        .iter()
        .map(|entry| entry.context_key_values.as_ref().map_or(0, Vec::len))
        .sum()
}

/// Whether `action` names a service and an action within it, the form `ActionNames`
/// requires ("each operation must include the service identifier, such as
/// iam:CreateUser").
fn names_a_service_action(action: &str) -> bool {
    match action.split_once(':') {
        Some((service, operation)) => !service.is_empty() && !operation.is_empty(),
        None => false,
    }
}

/// Everything one simulation decides every `(action, resource)` pair from: the documents
/// in force, the identity the simulation is of, and the condition values the request
/// carries. Read once per request rather than per pair.
pub struct Simulated<'a> {
    /// The identity-based documents in force, in the order the response names them.
    pub documents: &'a [PreparedDocument],
    /// How the response names each of those documents, one per entry of
    /// [`Self::documents`].
    pub sources: &'a [String],
    /// The simulated permissions boundary's documents; empty when no boundary takes part
    /// in the simulation at all.
    pub boundary: &'a [PreparedDocument],
    /// The service control policies in force at each level of the Organizations
    /// hierarchy the request named, in the order it named them; empty when the request
    /// named no hierarchy.
    pub organization_levels: &'a [Vec<PreparedDocument>],
    /// The resource-based policy the request supplied, when it supplied one.
    pub resource_policy: Option<&'a PreparedDocument>,
    /// The spelling of the simulated caller a resource-based statement GRANTS to.
    pub granting_principals: &'a [String],
    /// The spellings a resource-based statement DENIES: the caller's own and its
    /// account's.
    pub denying_principals: &'a [String],
    /// The condition values the request supplied.
    pub context: &'a [(String, String)],
    /// Whether the simulated resources are owned by another account than the simulated
    /// caller's, which is when an allow needs BOTH sides ([`cross_account`]).
    pub cross_account: bool,
}

/// What one `(action, resource)` pair was decided, as the report carries it.
struct Decided {
    /// The pair's own result.
    result: ResourceSpecificResult,
    /// Whether the service control policies in force allowed this pair, when the request
    /// named any level of a hierarchy at all.
    allowed_by_organizations: Option<bool>,
}

/// What the documents in force decide about each action of ONE page, over every resource
/// the request asks about.
///
/// One `EvaluationResult` per action, in the order the page carries them; `Err` is the
/// sentence to refuse the request with, for a report the response cannot carry
/// ([`MAX_SIMULATED_CONTEXT_KEYS`]). The page is the caller's: it decides exactly the
/// actions handed here, so a request asking about many actions does the work of the page
/// it asked for rather than of the whole list.
pub async fn evaluate_page(
    simulated: &Simulated<'_>,
    actions: &[String],
    resources: &[String],
    reports_each_resource: bool,
) -> Result<Vec<EvaluationResult>, String> {
    let mut results: Vec<EvaluationResult> = Vec::new();
    let mut decided_pairs = 0usize;
    for action in actions {
        let mut per_resource: Vec<ResourceSpecificResult> = Vec::new();
        // The hierarchy allowed the ACTION only where it allowed it on every resource
        // the request asked about, the way the permissions boundary's detail beside it is
        // aggregated ([`boundary_detail`]).
        let mut allowed_by_organizations: Option<bool> = None;
        // "A list of context keys that are required by the included input policies but
        // that were not provided by one of the input parameters": the keys the DECISION
        // needed a value for and the request carried none, which each resource's own
        // result already reports for its own pair ([`decide`]) — a pair reaches the
        // statements its action and resource match, so two pairs can need different keys.
        // The action's own result reports the union of its resources', the way it reports
        // the union of their matched statements; the per-resource lists are reported only
        // where the response carries the per-resource results at all. "This list is used
        // when the resource in a simulation is '*' … if you include a list of resources,
        // then any missing context values are instead included under the
        // ResourceSpecificResults section."
        //
        // Folded as each pair is decided rather than over the finished page, so the bound
        // below is reached — and the request refused — at the pair that reaches it,
        // before the keys of the pairs after it are retained.
        let mut missing: ::std::collections::BTreeMap<String, String> =
            ::std::collections::BTreeMap::new();
        for resource in resources {
            let decided = decide(simulated, action, resource);
            if let Some(allowed) = decided.allowed_by_organizations {
                allowed_by_organizations =
                    Some(allowed && allowed_by_organizations.unwrap_or(true));
            }
            fold_keys_into(
                &mut missing,
                decided
                    .result
                    .missing_context_values
                    .iter()
                    .flatten()
                    .cloned(),
            );
            // The list is reported on every result the page carries, so its length is a
            // factor of the response's size: past the bound the request is refused rather
            // than answered with a report nothing bounds. Truncating the list instead
            // would be worse — a short list would read as "these are the only keys
            // missing".
            if missing.len() > MAX_SIMULATED_CONTEXT_KEYS {
                return Err(format!(
                    "Invalid value for PolicyInputList: the policies in force test more \
                     than {MAX_SIMULATED_CONTEXT_KEYS} context keys the request supplied \
                     no value for, and at most that many may be reported in one simulation"
                ));
            }
            per_resource.push(decided.result);
            decided_pairs += 1;
            // The evaluation is synchronous work inside an async handler, so it hands the
            // executor back periodically: one simulation never holds a worker for its
            // whole run, whatever this page was asked to decide.
            if decided_pairs.is_multiple_of(EVALUATIONS_PER_YIELD) {
                ::tokio::task::yield_now().await;
            }
        }
        let missing: Vec<String> = missing.into_values().collect();
        if !reports_each_resource {
            for result in &mut per_resource {
                result.missing_context_values = None;
            }
        }
        let first = per_resource.first().cloned().unwrap_or_default();
        // "In the top-level result, this field contains the union of matched statements
        // across all requested resources" — so the statements the action's own result
        // reports are every resource's, each named once.
        let mut matched: Vec<Statement> = Vec::new();
        for result in &per_resource {
            for entry in result.matched_statements.iter().flatten() {
                report_once(&mut matched, entry.clone());
            }
        }
        results.push(EvaluationResult {
            eval_action_name: action.clone(),
            eval_resource_name: Some(first.eval_resource_name.clone()),
            eval_decision: most_restrictive(&per_resource),
            missing_context_values: (!missing.is_empty()).then_some(missing),
            permissions_boundary_decision_detail: boundary_detail(&per_resource),
            resource_specific_results: reports_each_resource.then_some(per_resource),
            matched_statements: Some(matched),
            // "Specifies whether the simulated operation is allowed by the Organizations
            // service control policies that impact the simulated user's account"
            // (`OrganizationsDecisionDetail$AllowedByOrganizations`): reported for a
            // simulation that named a hierarchy, and absent for one that named none,
            // where no SCP took part in the decision.
            organizations_decision_detail: allowed_by_organizations
                .map(|allowed_by_organizations| OrganizationsDecisionDetail {
                    allowed_by_organizations,
                }),
            // `EvalDecisionDetails` is for a decision reported per policy TYPE, which
            // nothing here reports: the decision's sources are already named by the
            // details beside it and by the statements it reports.
            eval_decision_details: None,
        });
    }
    Ok(results)
}

/// What the documents in force decide about one `(action, resource)` pair, and which
/// statements decided it.
///
/// The decision is the shared evaluation's ([`service_common::iam_authorization`]): the
/// identity-based documents and, when the request supplies one, the resource-based policy
/// — evaluated against the simulated caller, so a statement whose `Principal` element
/// names that identity is one the simulation can match. A simulated permissions boundary
/// bounds the result the way IAM's does — a request the boundary does not allow is
/// denied, and the boundary can only take permission away, never grant it — and the SCPs
/// of a named hierarchy bound it the way Organizations' do.
///
/// The request is declared to carry its WHOLE condition context, because a simulation's
/// context is exactly the entries its caller supplied: nothing else can put a value in it.
/// So IAM's absent-key rule decides a clause over a key the request left out — a positive
/// comparison does not hold, while a negated one, an `IfExists` spelling and
/// `Null: {key: true}` do — rather than leaving the statement undecided, and
/// `MissingContextValues` is what the evaluation says it needed a value for and did not
/// get.
fn decide(simulated: &Simulated<'_>, action: &str, resource: &str) -> Decided {
    let granting = iam::Request::new(action, resource, simulated.granting_principals)
        .with_context(simulated.context)
        .with_complete_context();
    let identity = iam::identity_outcome_prepared(simulated.documents, &granting);
    let from_resource = match simulated.resource_policy {
        Some(document) => {
            let denying = iam::Request::new(action, resource, simulated.denying_principals)
                .with_context(simulated.context)
                .with_complete_context();
            let denied = iam::resource_outcome_prepared(document, &denying);
            if denied.decision == iam::Decision::Denied {
                denied
            } else {
                iam::resource_outcome_prepared(document, &granting)
            }
        }
        None => iam::Outcome::default(),
    };
    // Within one account an allow from EITHER side grants; across accounts both the
    // identity-based policies and the resource-based policy have to allow
    // ([`Simulated::cross_account`]).
    let decision = if simulated.cross_account {
        cross_account_decision(identity.decision, from_resource.decision)
    } else {
        super::authorization::decision_name(identity.decision, from_resource.decision)
    };

    // "The permissions boundary sets the maximum permissions that the entity can have":
    // a request outside it is denied even where the identity policies allow it.
    let no_boundary = simulated.boundary.is_empty();
    let from_boundary = iam::identity_outcome_prepared(simulated.boundary, &granting);
    let allowed_by_boundary =
        no_boundary || iam::is_allowed(from_boundary.decision, iam::Decision::NotStated);
    let decision = if allowed_by_boundary || decision == DECISION_EXPLICIT_DENY {
        decision
    } else {
        DECISION_IMPLICIT_DENY
    };

    // The SCPs of the hierarchy the request named, which cap the account's permissions
    // the way a boundary caps an entity's: an action no level allows is not available to
    // the account at all, and one a level explicitly denies is denied outright.
    let no_organization = simulated.organization_levels.is_empty();
    let from_organizations = organization_outcome(simulated.organization_levels, &granting);
    let allowed_by_organizations = from_organizations.decision == iam::Decision::Allowed;
    let decision = if no_organization {
        decision
    } else if from_organizations.decision == iam::Decision::Denied {
        DECISION_EXPLICIT_DENY
    } else if allowed_by_organizations || decision == DECISION_EXPLICIT_DENY {
        decision
    } else {
        DECISION_IMPLICIT_DENY
    };

    // "A list of the statements in the input policies that determine the result for this
    // part of the simulation ... Only statements that contributed to the reported
    // decision are included": the statements that DENIED when the reported decision is
    // the explicit deny, the ones that GRANTED when it is the allow, and none at all for
    // an implicit deny — nothing stated it, including one a permissions boundary or an
    // SCP caused, where the grant they took away no longer determines the result.
    let contributing = if decision == DECISION_EXPLICIT_DENY {
        Some(iam::Decision::Denied)
    } else if decision == DECISION_ALLOWED {
        Some(iam::Decision::Allowed)
    } else {
        None
    };
    let mut matched: Vec<Statement> = Vec::new();
    if let Some(contributing) = contributing {
        if identity.decision == contributing {
            for entry in &identity.matched {
                report_once(
                    &mut matched,
                    statement_entry(
                        simulated.sources.get(entry.document).cloned(),
                        IDENTITY_POLICY_SOURCE,
                    ),
                );
            }
        }
        if from_resource.decision == contributing && !from_resource.matched.is_empty() {
            report_once(
                &mut matched,
                statement_entry(Some(RESOURCE_POLICY_ID.to_string()), RESOURCE_POLICY_SOURCE),
            );
        }
    }

    Decided {
        result: ResourceSpecificResult {
            eval_resource_name: resource.to_string(),
            eval_resource_decision: decision.to_string(),
            // The keys THIS pair's decision needed a value for and the request supplied
            // none: every document that took part in it is asked, since a condition in any
            // of them is one the simulation needed the value for. Each answer is already
            // bounded by the evaluation
            // (`service_common::iam_authorization::MAX_REQUIRED_CONTEXT_KEYS`), and the
            // list kept per pair stops one key past the bound this report is answered
            // under and holds no name longer than one
            // ([`MAX_CONTEXT_KEY_NAME_CHARACTERS`]) — so the page's retention is its pairs
            // times two constants of this module rather than times what the documents
            // carry; [`evaluate_page`] refuses past the first of them.
            missing_context_values: {
                let missing = folded_keys(
                    identity
                        .required_context_keys
                        .iter()
                        .chain(&from_resource.required_context_keys)
                        .chain(&from_boundary.required_context_keys)
                        .chain(&from_organizations.required_context_keys)
                        .take(MAX_SIMULATED_CONTEXT_KEYS + 1)
                        .cloned(),
                );
                (!missing.is_empty()).then_some(missing)
            },
            matched_statements: Some(matched),
            eval_decision_details: None,
            permissions_boundary_decision_detail: (!no_boundary).then_some(
                PermissionsBoundaryDecisionDetail {
                    allowed_by_permissions_boundary: allowed_by_boundary,
                },
            ),
        },
        allowed_by_organizations: (!no_organization).then_some(allowed_by_organizations),
    }
}

/// What the service control policies of a whole hierarchy say about one request.
///
/// "The simulator evaluates SCPs in the order that you provide, consistent with how
/// Organizations enforces SCPs": an SCP hierarchy makes an action available to the
/// account only where EVERY level of it allows the action, since each level is a filter on
/// what the levels above it left, and a statement of any level that denies the action
/// denies it outright. A level that states nothing about the request allows nothing, which
/// is why "allowed" here means every level allowed.
fn organization_outcome(levels: &[Vec<PreparedDocument>], request: &iam::Request<'_>) -> iam::Outcome {
    let mut required_context_keys: Vec<String> = Vec::new();
    let mut denied = false;
    let mut allowed_everywhere = true;
    for level in levels {
        let outcome = iam::identity_outcome_prepared(level, request);
        required_context_keys.extend(outcome.required_context_keys);
        match outcome.decision {
            iam::Decision::Denied => denied = true,
            iam::Decision::Allowed => {}
            iam::Decision::NotStated => allowed_everywhere = false,
        }
    }
    let decision = if denied {
        iam::Decision::Denied
    } else if allowed_everywhere {
        iam::Decision::Allowed
    } else {
        iam::Decision::NotStated
    };
    iam::Outcome {
        decision,
        // Which statement of which SCP decided it is not reported: `MatchedStatements`
        // names "the statements in the INPUT POLICIES", and what an SCP hierarchy decided
        // is reported by `OrganizationsDecisionDetail` instead.
        matched: Vec::new(),
        required_context_keys,
    }
}

/// The decision a CROSS-account request comes to, from what the two sides said.
///
/// "When the principal and the resource are in different accounts, both an identity-based
/// policy in the principal's account and a resource-based policy in the resource's account
/// must allow the request": an allow from one side alone grants nothing, while an explicit
/// deny from either still denies. Within one account the either-side rule applies instead,
/// which is the shared [`super::authorization::decision_name`].
fn cross_account_decision(identity: iam::Decision, resource: iam::Decision) -> &'static str {
    if identity == iam::Decision::Denied || resource == iam::Decision::Denied {
        DECISION_EXPLICIT_DENY
    } else if identity == iam::Decision::Allowed && resource == iam::Decision::Allowed {
        DECISION_ALLOWED
    } else {
        DECISION_IMPLICIT_DENY
    }
}

/// `keys` folded to one entry per context key and ordered, the way every report of a set
/// of context keys this service answers with is ordered
/// ([`crate::state::context_keys_in_policies`]): names are not distinguished by case, so
/// the folded name is the identity and the first spelling seen is the one reported.
fn folded_keys<I: IntoIterator<Item = String>>(keys: I) -> Vec<String> {
    let mut folded: ::std::collections::BTreeMap<String, String> =
        ::std::collections::BTreeMap::new();
    fold_keys_into(&mut folded, keys);
    folded.into_values().collect()
}

/// Fold `keys` INTO `folded`, which is the same fold [`folded_keys`] answers, kept open so
/// a caller folding one report out of many pieces reads the set so far between them.
///
/// A name longer than the longest context key name a report of one may carry
/// ([`MAX_CONTEXT_KEY_NAME_CHARACTERS`]) is left out: it names no context key, so the
/// simulation could have needed no value for it, and the fold's size stays a constant of
/// this module. Folding stops one entry past the bound a report is answered under
/// ([`MAX_SIMULATED_CONTEXT_KEYS`]), which is the length its caller refuses on: nothing is
/// retained for a report that cannot be answered.
fn fold_keys_into<I: IntoIterator<Item = String>>(
    folded: &mut ::std::collections::BTreeMap<String, String>,
    keys: I,
) {
    for key in keys {
        if key.len() > MAX_CONTEXT_KEY_NAME_CHARACTERS {
            continue;
        }
        folded.entry(key.to_ascii_lowercase()).or_insert(key);
        if folded.len() > MAX_SIMULATED_CONTEXT_KEYS {
            break;
        }
    }
}

/// The decision the ACTION's own result reports, from the decisions its resources were
/// each decided.
///
/// "The top-level `EvalDecision` reflects the most restrictive decision across all
/// resources (for example, if any resource produces `explicitDeny`, the top-level
/// decision is `explicitDeny`)" (`EvaluationResult`), so the member agrees with the
/// statements the same result reports as having determined it — the union across those
/// resources — rather than describing whichever resource the request happened to name
/// first. A request that named no resource is decided about one resource, `*`, so the
/// aggregate is that one decision.
///
/// The order is IAM's own: an explicit deny is the most restrictive outcome, then the
/// implicit deny, then the allow ([`restrictiveness`]).
fn most_restrictive(per_resource: &[ResourceSpecificResult]) -> String {
    per_resource
        .iter()
        .map(|result| result.eval_resource_decision.as_str())
        .max_by_key(|decision| restrictiveness(decision))
        // Nothing to aggregate is nothing stated about the request, which is the implicit
        // deny; a page always decides at least one resource, so this is the value of a
        // decision no resource was asked for rather than a fallback the response is built
        // from.
        .unwrap_or(DECISION_IMPLICIT_DENY)
        .to_string()
}

/// How restrictive one `PolicyEvaluationDecisionType` value is, for [`most_restrictive`]
/// to take the maximum of: the explicit deny withholds the most, the allow the least.
fn restrictiveness(decision: &str) -> u8 {
    if decision == DECISION_EXPLICIT_DENY {
        2
    } else if decision == DECISION_ALLOWED {
        0
    } else {
        1
    }
}

/// The permissions-boundary detail the ACTION's own result reports, from the details its
/// resources were each decided with.
///
/// Aggregated the way the decision beside it is: the boundary allowed the action only
/// where it allowed it on EVERY resource the request asked about, since one resource the
/// boundary withheld is a resource the entity may not act on. Absent when no permissions
/// boundary took part in the simulation at all, which is when no resource carries the
/// detail either.
fn boundary_detail(
    per_resource: &[ResourceSpecificResult],
) -> Option<PermissionsBoundaryDecisionDetail> {
    per_resource
        .iter()
        .filter_map(|result| result.permissions_boundary_decision_detail.as_ref())
        .map(|detail| detail.allowed_by_permissions_boundary)
        .reduce(|allowed_everywhere, allowed| allowed_everywhere && allowed)
        .map(
            |allowed_by_permissions_boundary| PermissionsBoundaryDecisionDetail {
                allowed_by_permissions_boundary,
            },
        )
}

/// One `MatchedStatements` entry: which policy a contributing statement is in, and of
/// which kind.
///
/// `StartPosition`/`EndPosition` are "the row and column of the beginning/end of the
/// Statement in an IAM policy", which nothing here measures — reporting a position that
/// was not measured would be worse than reporting none, so they stay absent, and the
/// source identification they are reported beside does not depend on them.
fn statement_entry(source_policy_id: Option<String>, source_policy_type: &str) -> Statement {
    Statement {
        source_policy_id,
        source_policy_type: Some(source_policy_type.to_string()),
        start_position: None,
        end_position: None,
    }
}

/// Append `entry` to `into` unless the same policy is already named there.
///
/// Without the statement positions, two contributing statements of ONE policy are the
/// same entry — the policy they are in and its kind — and the member is a list of what
/// determined the result, so naming that policy twice would report a distinction the
/// response does not carry.
fn report_once(into: &mut Vec<Statement>, entry: Statement) {
    if !into.iter().any(|held| {
        held.source_policy_id == entry.source_policy_id
            && held.source_policy_type == entry.source_policy_type
    }) {
        into.push(entry);
    }
}
