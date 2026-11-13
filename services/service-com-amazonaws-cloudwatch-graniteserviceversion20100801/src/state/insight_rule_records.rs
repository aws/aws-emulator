// SPDX-License-Identifier: CC0-1.0

//! The Contributor Insights rule RECORDS this store holds, and the managed-rule catalog a
//! managed rule is enabled from.
//!
//! One concern in one file: the stored rule ([`StoredInsightRule`]), the schema and state
//! vocabulary every rule is written and reported under, the sentences the rule operations
//! refuse with, the resource ARN a managed rule names, and the catalog that says WHICH managed
//! rule a resource can enable — "The template name for the managed Contributor Insights rule,
//! as returned by `ListManagedInsightRules`." (`ManagedRule$TemplateName`).
//!
//! The catalog has two halves, and a listing reports them in this order:
//!
//! * the templates the SERVICE publishes (`SERVICE_MANAGED_RULE_TEMPLATES`) — fixed text of
//!   the API, the same for every account, which is what makes `ListManagedInsightRules` report
//!   the managed rules of a supported resource without anything having enabled one;
//! * the templates this account's store was given beside them
//!   ([`State::publish_managed_rule_template`]), for a resource kind another part of this
//!   emulator publishes managed rules for.
//!
//! The EVALUATION of a rule — the syntax its definition carries, read over the log events its
//! groups receive — is [`super::insight_rules`]; this file holds no evaluation.

use super::{
    insight_rules, ComAmazonawsCloudwatchGraniteserviceversion20100801State as State,
    ARN_NAMESPACE,
};
use crate::ops::types::{InsightRule, PartialFailure, Tag};

/// A stored Contributor Insights rule: the modelled shape `DescribeInsightRules` reports,
/// plus the tags `PutInsightRule` accepted (no [`InsightRule`] member carries them).
#[derive(Debug, Clone, Default)]
pub struct StoredInsightRule {
    /// The rule as `DescribeInsightRules` reports it.
    pub rule: InsightRule,
    /// The rule's `Definition` as the evaluation READS it ([`insight_rules::LogRuleDefinition`]),
    /// or `None` for a definition that is not a log rule this emulator evaluates (a managed
    /// rule's, which names no log group and no contribution).
    ///
    /// Derived from `rule.definition` by the write that stored the rule, so an ingest decides
    /// which rules collect from a log group without re-reading the definition TEXT of every
    /// rule the store holds — the first factor of an ingest's cost
    /// ([`insight_rules::State::rules_collecting_from`]).
    pub collection: Option<insight_rules::LogRuleDefinition>,
    /// What the log events this rule has evaluated contributed — what `GetInsightRuleReport`
    /// reports from ([`insight_rules`]). A rule collects them while it is ENABLED, as the
    /// events arrive, so this is empty for a rule nothing has been ingested for yet.
    pub observations: Vec<insight_rules::ContributorObservation>,
    /// The tags the rule was created with.
    pub tags: Vec<Tag>,
    /// What this rule was enabled from, for a rule `PutManagedInsightRules` created;
    /// `None` for one a caller wrote with `PutInsightRule`.
    pub managed: Option<ManagedRuleOrigin>,
    /// What this rule collects for a RESOURCE, for a rule another Amazon Web Services service
    /// minted when Contributor Insights was turned on for one of its own resources
    /// ([`insight_rules`] is the log-rule evaluation; this is the other kind of collection a
    /// rule can hold). `None` for every rule written against the log-rule syntax.
    pub resource: Option<super::resource_insight_rules::ResourceCollection>,
}

/// What a managed Contributor Insights rule was enabled from: the template
/// `PutManagedInsightRules` named, and the Amazon Web Services resource whose data the rule
/// collects.
///
/// `ListManagedInsightRules` reports the pair back as the `TemplateName` and `ResourceARN` of
/// its description ("The template name for the managed rule. Used to enable managed rules
/// using `PutManagedInsightRules`.", `ManagedRuleDescription$TemplateName`; "If a managed rule
/// is enabled, this is the ARN for the related Amazon Web Services resource.",
/// `ManagedRuleDescription$ResourceARN`), which is why it is kept beside the rule rather than
/// derived back out of it.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ManagedRuleOrigin {
    /// The managed-rule template the rule was enabled from.
    pub template_name: String,
    /// The ARN of the resource the rule collects data for.
    pub resource_arn: String,
}

/// One managed Contributor Insights rule template an account can enable a rule from.
///
/// "The template name for the managed Contributor Insights rule, as returned by
/// `ListManagedInsightRules`." (`ManagedRule$TemplateName`) — the template a caller names is one
/// the SERVICE publishes for a kind of resource ("When you enable a managed rule, you create a
/// Contributor Insights rule that collects data from Amazon Web Services services.",
/// `PutManagedInsightRules`), so a template is held as the name it is published under and the
/// resources it collects data for, and nothing a caller sends adds one.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManagedRuleTemplate {
    /// The name the template is published under, which a `ManagedRule` entry names it by.
    pub template_name: String,
    /// The ARN vendor whose resources this template collects data for — the third field of
    /// their ARNs (`::service_common::arn::Arn::vendor`).
    pub resource_vendor: String,
    /// The resource type inside that vendor the template applies to
    /// (`::service_common::arn::Arn::resource_type`), or `None` when it applies to every
    /// resource the vendor holds.
    pub resource_type: Option<String>,
}

impl ManagedRuleTemplate {
    /// The template published under `template_name` for every resource of the same vendor and
    /// type as `resource_arn`, or `None` when `resource_arn` is not an ARN.
    #[must_use]
    pub fn for_resource(template_name: &str, resource_arn: &str) -> Option<Self> {
        let arn = parse_resource_arn(resource_arn)?;
        Some(Self {
            template_name: template_name.to_string(),
            resource_vendor: arn.vendor.to_string(),
            resource_type: Some(arn.resource_type().to_string()),
        })
    }

    /// Whether this template is one of the managed rules available for `resource_arn`: the
    /// ARN's vendor is the one the template collects data for, and its resource type is the one
    /// the template applies to when the template names a type at all.
    #[must_use]
    pub fn available_for(&self, resource_arn: &str) -> bool {
        parse_resource_arn(resource_arn).is_some_and(|arn| {
            self.resource_vendor == arn.vendor
                && self
                    .resource_type
                    .as_deref()
                    .is_none_or(|resource_type| resource_type == arn.resource_type())
        })
    }
}

/// One managed rule `ListManagedInsightRules` reports for a resource: the template it is
/// available from, and the Contributor Insights rule enabled from that template when one is
/// enabled for the resource.
#[derive(Debug, Clone, Copy)]
pub struct AvailableManagedRule<'a> {
    /// The template the rule is (or would be) enabled from.
    pub template_name: &'a str,
    /// The rule enabled from the template for this resource — its name and record — or `None`
    /// when the template is available but nothing has enabled it: "If present, it contains
    /// information about the Contributor Insights rule that contains information about the
    /// related Amazon Web Services resource." (`ManagedRuleDescription$RuleState`)
    pub rule: Option<(&'a String, &'a StoredInsightRule)>,
}

/// One managed-rule template the SERVICE itself publishes, as fixed text of the API: the name
/// a `ManagedRule` names it by, and the kind of resource it collects data for.
#[derive(Debug, Clone, Copy)]
struct ServiceManagedRuleTemplate {
    /// The published template name.
    template_name: &'static str,
    /// The ARN vendor of the resources it collects data for.
    resource_vendor: &'static str,
    /// The resource type inside that vendor.
    resource_type: &'static str,
}

impl ServiceManagedRuleTemplate {
    /// Whether this template is published for the KIND of resource `resource_arn` names: its
    /// ARN vendor and the resource type its resource field opens with, whether or not the ARN
    /// goes on to name an individual resource of that kind.
    fn published_for_kind(&self, resource_arn: &str) -> bool {
        parse_resource_arn(resource_arn).is_some_and(|arn| {
            self.resource_vendor == arn.vendor && self.resource_type == arn.resource_type()
        })
    }

    /// Whether `resource_arn` names an INDIVIDUAL resource of that kind — the resource type
    /// and a non-empty id of it — which is what a rule enabled from this template collects
    /// data for. A value naming the kind alone (`vpc-endpoint-service`, or
    /// `vpc-endpoint-service/`) names no resource to collect from.
    fn names_individual_resource(&self, resource_arn: &str) -> bool {
        self.published_for_kind(resource_arn)
            && ::service_common::arn::resource_id_of_type(resource_arn, self.resource_type)
                .is_some_and(|id| !id.is_empty())
    }

    /// Whether this template is one of the managed rules available for `resource_arn`: it is
    /// published for that resource's kind and the ARN names an individual resource of it.
    fn available_for(&self, resource_arn: &str) -> bool {
        self.names_individual_resource(resource_arn)
    }
}

/// The ARN vendor whose resources the published templates below collect data for.
const VPC_ENDPOINT_SERVICE_VENDOR: &str = "ec2";

/// The resource type those templates apply to: a VPC endpoint service.
const VPC_ENDPOINT_SERVICE_TYPE: &str = "vpc-endpoint-service";

/// The managed Contributor Insights rule templates this SERVICE publishes, in the order a
/// listing reports them.
///
/// Source: the managed rules AWS publishes for a VPC endpoint service — the per-endpoint-id
/// rules for new connections, bytes, reset packets and active connections that the AWS
/// PrivateLink documentation lists for a service provider, each at version 1. They are fixed
/// text of the API rather than anything an account configures, which is why
/// `ListManagedInsightRules` reports them for such a resource before anything has enabled one,
/// and keeps reporting them after the rule enabled from one is deleted.
const SERVICE_MANAGED_RULE_TEMPLATES: &[ServiceManagedRuleTemplate] = &[
    ServiceManagedRuleTemplate {
        template_name: "VpcEndpointService-NewConnectionsByEndpointId-v1",
        resource_vendor: VPC_ENDPOINT_SERVICE_VENDOR,
        resource_type: VPC_ENDPOINT_SERVICE_TYPE,
    },
    ServiceManagedRuleTemplate {
        template_name: "VpcEndpointService-BytesByEndpointId-v1",
        resource_vendor: VPC_ENDPOINT_SERVICE_VENDOR,
        resource_type: VPC_ENDPOINT_SERVICE_TYPE,
    },
    ServiceManagedRuleTemplate {
        template_name: "VpcEndpointService-RstPacketsByEndpointId-v1",
        resource_vendor: VPC_ENDPOINT_SERVICE_VENDOR,
        resource_type: VPC_ENDPOINT_SERVICE_TYPE,
    },
    ServiceManagedRuleTemplate {
        template_name: "VpcEndpointService-ActiveConnectionsByEndpointId-v1",
        resource_vendor: VPC_ENDPOINT_SERVICE_VENDOR,
        resource_type: VPC_ENDPOINT_SERVICE_TYPE,
    },
];

/// The schema NAME every Contributor Insights rule a caller creates is written against: "For
/// rules that you create, this is always `{"Name": "CloudWatchLogRule", "Version": 1}`. For
/// managed rules, this is `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`).
pub const INSIGHT_RULE_SCHEMA_NAME: &str = "CloudWatchLogRule";

/// The version both schema names are at — the one the member's documented value states. See
/// [`INSIGHT_RULE_SCHEMA_NAME`].
pub const INSIGHT_RULE_SCHEMA_VERSION: u64 = 1;

/// A rule's schema as `InsightRule$Schema` carries it: the schema's name and version, joined
/// by `/`.
///
/// `InsightRuleSchema` is a plain string, and the member's documented value
/// (`{"Name": …, "Version": …}`) is the schema that string NAMES rather than the text of it —
/// so what goes on the wire is the compact pair, which is what lets a client tell a rule it
/// wrote from a rule the service manages by reading the name before the `/`.
#[must_use]
pub fn insight_rule_schema(schema_name: &str) -> String {
    format!("{schema_name}/{INSIGHT_RULE_SCHEMA_VERSION}")
}

/// The same schema as the JSON object a rule DEFINITION states it with
/// (`InsightRule$Definition`, "the definition of the rule, as a JSON object").
///
/// Built from the same name and version the wire form is ([`insight_rule_schema`]), so the
/// schema a definition declares and the one the rule reports cannot drift apart.
#[must_use]
pub fn insight_rule_schema_document(schema_name: &str) -> ::serde_json::Value {
    ::serde_json::json!({"Name": schema_name, "Version": INSIGHT_RULE_SCHEMA_VERSION})
}

/// A Contributor Insights rule that is evaluating log events: "The state of the rule. Valid
/// values are ENABLED and DISABLED." (`PutInsightRuleInput$RuleState`)
pub const INSIGHT_RULE_ENABLED: &str = "ENABLED";

/// A Contributor Insights rule that is not evaluating log events
/// (`PutInsightRuleInput$RuleState`).
pub const INSIGHT_RULE_DISABLED: &str = "DISABLED";

/// The `ExceptionType` a batch Contributor Insights rule operation reports a well-formed name
/// it holds no rule for under: "The type of error." (`PartialFailure$ExceptionType`) — the
/// name addresses a rule that is not there, which is the resource-not-found class and not a
/// fault in the value itself, reported per entry rather than as a refusal of the whole
/// request.
const INSIGHT_RULE_NOT_FOUND_EXCEPTION: &str = "ResourceNotFoundException";

/// The `FailureCode` beside it: "The code of the error." (`PartialFailure$FailureCode`) —
/// which member of the entry the failure is about, here the rule name the request carried.
const INSIGHT_RULE_NAME_FAILURE_CODE: &str = "RULE_NAME";

/// How CloudWatch states that one of the names it was given is not a rule this store holds:
/// fixed text around the name the request carried, ended as a sentence.
///
/// The wording of the BATCH operations' per-entry report: they describe such a name in a
/// `Failures` entry, and the entry's description is this sentence.
pub fn unknown_insight_rule(rule_name: &str) -> String {
    format!("Rule {rule_name} does not exist.")
}

/// How CloudWatch refuses a request that READS a Contributor Insights rule by a name no rule of
/// this account and Region answers to: the coded form its Contributor Insights refusals state a
/// refusal about ONE request member in ([`EMPTY_RULE_NAMES`], [`missing_parameter_message`]) —
/// the member's own code ([`INSIGHT_RULE_NAME_FAILURE_CODE`]), then the value the request
/// carried, and nothing else.
///
/// Such a read is refused as a bad VALUE for the member that carried the name rather than as a
/// resource that is missing, which is the class the reading operation's recorded refusal carries
/// (`GetInsightRuleReport`, measured against `monitoring`): the client has to repair the request
/// member, and the code it branches on says so.
pub fn invalid_insight_rule_name(rule_name: &str) -> String {
    format!("{INSIGHT_RULE_NAME_FAILURE_CODE}: {rule_name}")
}

/// The `ExceptionType` such an operation reports a `RuleNames` member that is not a rule NAME
/// at all under: "The type of error." (`PartialFailure$ExceptionType`) — the value itself is
/// the fault, which is the invalid-parameter class, and the two classes are what a batch caller
/// branches on: a name that is legal but addresses nothing says "create the rule", a value this
/// member may not carry says "repair the request".
const INVALID_INSIGHT_RULE_NAME_EXCEPTION: &str = "InvalidParameterException";

/// The `FailureCode` beside it: "The code of the error." (`PartialFailure$FailureCode`) — the
/// rule name the request carried, reported as the INVALID value it is rather than as the member
/// alone ([`INSIGHT_RULE_NAME_FAILURE_CODE`], which codes a name that is merely unknown).
const INVALID_INSIGHT_RULE_NAME_FAILURE_CODE: &str = "INVALID_RULE_NAME";

/// The characters a Contributor Insights rule name may be built from, as the expression a
/// client re-validates a name against: `InsightRuleName`'s modelled `@pattern`
/// (`smithy_query(query="shape",shape="InsightRuleName")`) without the anchors Smithy wraps a
/// whole-value match in — the rendering this service publishes an expression in
/// (`service_common::validation::unanchored`).
const INSIGHT_RULE_NAME_PATTERN: &str = r"^[\x20-\x7E]+$";

/// How CloudWatch states a rule name carrying no characters at all: wholly fixed text, nothing
/// of the member interpolated — there is no value to echo.
pub const EMPTY_INSIGHT_RULE_NAME: &str = "RuleName cannot be an empty string.";

/// Why `rule_name` is not a Contributor Insights rule NAME, or `None` when it is one.
///
/// The legality of a rule name is the one grammar `InsightRuleName` declares — 1..128
/// characters, each of them printable ASCII — so a value outside it is not a name this service
/// could hold a rule under, whichever member carried it and whichever batch operation was
/// called. Each of the three ways a value misses that grammar is stated as the class it is: no
/// characters at all ([`EMPTY_INSIGHT_RULE_NAME`]), more characters than a name may carry, and
/// characters the grammar does not admit — the last quoting the expression a client
/// re-validates against, which is what locates the repair.
#[must_use]
pub fn insight_rule_name_fault(rule_name: &str) -> Option<String> {
    if rule_name.is_empty() {
        return Some(EMPTY_INSIGHT_RULE_NAME.to_string());
    }
    if rule_name.chars().count() > MAX_INSIGHT_RULE_NAME_LEN {
        return Some(format!(
            "RuleName cannot be longer than {MAX_INSIGHT_RULE_NAME_LEN} characters."
        ));
    }
    if !rule_name.chars().all(is_insight_rule_name_character) {
        return Some(format!(
            "RuleName \"{rule_name}\" does not match the pattern: {}",
            ::service_common::validation::unanchored(INSIGHT_RULE_NAME_PATTERN)
        ));
    }
    None
}

/// Whether `character` is one the rule-name grammar admits: printable ASCII, which is the span
/// `^[\x20-\x7E]+$` names — the space through the tilde, and nothing else (no control byte, no
/// character outside ASCII).
fn is_insight_rule_name_character(character: char) -> bool {
    matches!(character, '\x20'..='\x7E')
}

/// The one `Failures` entry a batch Contributor Insights rule operation reports for a member
/// that is not a rule name at all, described by the fault the value has
/// ([`insight_rule_name_fault`]).
///
/// "If the operation could not process one of the rules, the following data is returned for
/// each of those rules." (`PartialFailure`) — the entry carries the member VERBATIM, whatever
/// bytes it was sent with, so a client can match the entry to the element it submitted.
fn illegal_insight_rule_name_failure(rule_name: &str, fault: String) -> PartialFailure {
    PartialFailure {
        failure_resource: Some(rule_name.to_string()),
        exception_type: Some(INVALID_INSIGHT_RULE_NAME_EXCEPTION.to_string()),
        failure_code: Some(INVALID_INSIGHT_RULE_NAME_FAILURE_CODE.to_string()),
        failure_description: Some(fault),
    }
}

/// The one `Failures` entry a batch Contributor Insights rule operation reports for a name
/// this store holds no rule for: "An array listing the rules that could not be deleted."
/// (`DeleteInsightRulesOutput$Failures`), worded and coded identically whichever of those
/// operations was called, so the entry is built here once.
fn unknown_insight_rule_failure(rule_name: &str) -> PartialFailure {
    PartialFailure {
        failure_resource: Some(rule_name.to_string()),
        exception_type: Some(INSIGHT_RULE_NOT_FOUND_EXCEPTION.to_string()),
        failure_code: Some(INSIGHT_RULE_NAME_FAILURE_CODE.to_string()),
        failure_description: Some(unknown_insight_rule(rule_name)),
    }
}

/// How CloudWatch states that a request named no Contributor Insights rule at all.
///
/// "An array of the rule names to delete." (`DeleteInsightRulesInput$RuleNames`, required) — an
/// array carrying no entry names nothing to act on, which this service refuses as a bad value
/// for `RuleNames` rather than as a change it applied to nothing.
pub const EMPTY_RULE_NAMES: &str = "EMPTY_RULE_NAMES: RuleNames cannot be an empty array.";

/// How CloudWatch refuses a `NextToken` a Contributor Insights listing was given that it cannot
/// page from, when the value is no pagination token at all.
///
/// Those listings state a refusal in the coded form their other refusals use
/// ([`EMPTY_RULE_NAMES`]): the condition's code, then fixed text for the class, echoing nothing
/// of the token — neither the value nor where it was unreadable. A value whose characters or
/// whose length no opaque token has ([`super::reads_as_a_page_token`]) carries no token for the
/// service to read anything out of, which is what this sentence says.
pub const UNREADABLE_PAGE_TOKEN: &str = "INVALID_TOKEN: Invalid token.";

/// How it refuses one that IS a pagination token but not one this service issued.
///
/// The value reads as an opaque token, so the service reads it — and what it carries is not a
/// cursor of the form this service mints its tokens in ([`super::decode_page_token`]), which is
/// a token of some other form: the version the token was written under is not one this service
/// supports, and that is what this sentence says.
pub const UNMINTED_PAGE_TOKEN: &str = "INVALID_TOKEN: Unsupported token version.";

/// Which of the two a Contributor Insights listing answers for `token`, read off the token
/// itself.
///
/// Shared by the listings that page these rules, so neither can word the same unusable token
/// differently from the other, and both tell the caller which of the two faults its value has.
#[must_use]
pub fn invalid_page_token_message(token: &str) -> &'static str {
    if super::reads_as_a_page_token(token) {
        UNMINTED_PAGE_TOKEN
    } else {
        UNREADABLE_PAGE_TOKEN
    }
}

/// How CloudWatch refuses a page size a Contributor Insights listing cannot answer: one that
/// asks for no result at all.
///
/// `MaxResults` is "the maximum number of results to return in one operation"
/// (`DescribeInsightRulesInput$MaxResults`, `InsightRuleMaxResults` range 1..500), so its
/// published minimum of one is the smallest page there is and a value below it asks for a page
/// that could report nothing. The listing judges that as a bad parameter VALUE — the coded form
/// above, naming the member and the bound its minimum states — rather than as the shape of the
/// request, so a client branching on the parameter-value class reaches its handler.
pub const NON_POSITIVE_MAX_RESULTS: &str = "INVALID_MAX_RESULTS: MaxResults must be greater than 0.";

/// How CloudWatch states a parameter a Contributor Insights rule operation needs that the
/// request did not supply at all, naming the request member it was looked for at.
///
/// Those operations state a refusal about ONE request member in a coded form — the fault, then
/// the member in the spelling a code uses, then the sentence ([`EMPTY_RULE_NAMES`],
/// [`INVALID_RESOURCE_ARN`]) — so the class is derived from the member here rather than written
/// out per member: a client branching on the code reads the same form whichever member and
/// whichever of those operations it submitted.
pub fn missing_parameter_message(parameter: &str) -> String {
    format!(
        "MISSING_{}: The {parameter} parameter must be present.",
        member_code(parameter)
    )
}

/// `parameter` as the CODE half of such a sentence spells it: the member name in upper case,
/// with the word boundaries its camel case marks written as `_` (`ResourceARN` ->
/// `RESOURCE_ARN`, `RuleNames` -> `RULE_NAMES`).
fn member_code(parameter: &str) -> String {
    let characters: Vec<char> = parameter.chars().collect();
    let mut code = String::with_capacity(characters.len() + 2);
    for (at, character) in characters.iter().enumerate() {
        let opens_a_word = character.is_ascii_uppercase()
            && at > 0
            && (characters[at - 1].is_ascii_lowercase()
                || characters
                    .get(at + 1)
                    .is_some_and(char::is_ascii_lowercase));
        if opens_a_word {
            code.push('_');
        }
        code.extend(character.to_uppercase());
    }
    code
}

/// The granularity a Contributor Insights rule's contributor data is aggregated at, and so the
/// step every `Period` a report may be asked at is a multiple of: one minute.
///
/// A rule folds the log events its groups receive into per-minute contributor data, so the
/// `Period` a report's datapoints are aggregated over ("The period, in seconds, to use for the
/// statistics in the InsightRuleMetricDatapoint results.",
/// `GetInsightRuleReportInput$Period`) is a whole number of minutes: there is no finer fold to
/// report from.
pub const INSIGHT_RULE_PERIOD_STEP_SECS: i32 = 60;

/// The widest `Period` a Contributor Insights report may be asked at: one day, the span
/// Contributor Insights keeps a rule's contributor data over. See
/// [`INSIGHT_RULE_PERIOD_STEP_SECS`].
pub const MAX_INSIGHT_RULE_PERIOD_SECS: i32 = 86_400;

/// Why `period` is not a granularity a Contributor Insights report can be aggregated at, or
/// `None` when it is.
///
/// The `Period` member is modelled only as `range 1..`, so the granularities Contributor
/// Insights actually folds its contributor data at are held here: a multiple of
/// [`INSIGHT_RULE_PERIOD_STEP_SECS`], from that step up to
/// [`MAX_INSIGHT_RULE_PERIOD_SECS`]. The refusal names the condition by its CODE and states
/// the whole accepted set, the way this service's other Contributor Insights refusals do
/// ([`EMPTY_RULE_NAMES`]), and carries nothing of the value that was asked for.
#[must_use]
pub fn insight_rule_period_error(period: i32) -> Option<String> {
    let aggregatable = (INSIGHT_RULE_PERIOD_STEP_SECS..=MAX_INSIGHT_RULE_PERIOD_SECS)
        .contains(&period)
        && period % INSIGHT_RULE_PERIOD_STEP_SECS == 0;
    (!aggregatable).then(|| {
        format!(
            "INVALID_PERIOD: A Period must be a multiple of {INSIGHT_RULE_PERIOD_STEP_SECS} \
             seconds, from {INSIGHT_RULE_PERIOD_STEP_SECS} to {MAX_INSIGHT_RULE_PERIOD_SECS}."
        )
    })
}

/// How CloudWatch states the same thing for the operations that MOVE a rule's state
/// (`EnableInsightRules`, `DisableInsightRules`).
///
/// Those operations read their `RuleNames` as the set of rule states to write — "The state of
/// the rule. Valid values are ENABLED and DISABLED." (`PutInsightRuleInput$RuleState`) — so an
/// empty array is refused as the empty set of states it asks for, under the same
/// `InvalidParameterValueException`. Shared so neither of them can word it differently from
/// the other.
pub const EMPTY_RULE_STATES: &str = "EMPTY_RULE_STATES: RuleStates cannot be an empty array.";

/// The schema NAME a managed Contributor Insights rule is written against: "For managed rules,
/// this is `{"Name": "ServiceLogRule", "Version": 1}`" (`InsightRule$Schema`) — the schema of a
/// rule `PutManagedInsightRules` enabled, which is why `PutInsightRule` refuses a definition
/// carrying it ("You cannot edit these rules with PutInsightRule",
/// `PutManagedInsightRules`).
pub const MANAGED_INSIGHT_RULE_SCHEMA_NAME: &str = "ServiceLogRule";

/// The name the managed Contributor Insights rule enabled from `template_name` for
/// `resource_arn` takes.
///
/// Derived from the pair rather than minted per call, so enabling the same template for the
/// same resource twice names the SAME rule — which is what lets a second call re-enable the
/// rule the first one created: "If a previously created managed rule is currently disabled, a
/// subsequent call to this API will re-enable it." (`PutManagedInsightRules`).
///
/// The form is the template name, the resource's own id and the template's VERSION, in that
/// order: a published template carries its version as a trailing `-v<N>`
/// (`SERVICE_MANAGED_RULE_TEMPLATES`), and the rule enabled from it names the version of the
/// template it was built from LAST, after the resource whose data it collects — so one glance
/// at the name says which resource the rule watches and which version of the template built
/// it. A template name carrying no version names the resource alone. The result is held inside
/// `InsightRuleName`'s modelled 1..128 length and `^[\x20-\x7E]+$` pattern: a character the
/// pattern does not allow stands in as `-`, and an over-long name is cut to the maximum.
pub fn managed_insight_rule_name(template_name: &str, resource_arn: &str) -> String {
    let resource = resource_id(resource_arn);
    let (template, version) = template_version(template_name);
    let named = match version {
        Some(version) => format!("{template}-{resource}-{version}"),
        None => format!("{template}-{resource}"),
    };
    named
        .chars()
        .map(|c| if c.is_ascii_graphic() || c == ' ' { c } else { '-' })
        .take(MAX_INSIGHT_RULE_NAME_LEN)
        .collect()
}

/// The id of the resource `resource_arn` names: the resource field with the resource TYPE it
/// opens with removed (`vpce-svc-0a1b` of `vpc-endpoint-service/vpce-svc-0a1b`), or the whole
/// field when it carries no type. The value itself when it is not an ARN at all.
fn resource_id(resource_arn: &str) -> &str {
    let Some(arn) = parse_resource_arn(resource_arn) else {
        return resource_arn;
    };
    arn.resource
        .split_once(['/', ':'])
        .map_or(arn.resource, |(_, id)| id)
}

/// `template_name` split into the template and the VERSION it carries, or `None` for a version
/// when it carries none.
///
/// A published template's version is the trailing `-v<N>` of its name
/// (`SERVICE_MANAGED_RULE_TEMPLATES`); a trailing segment of any other form is part of the
/// template's own name.
fn template_version(template_name: &str) -> (&str, Option<&str>) {
    let Some((template, version)) = template_name.rsplit_once('-') else {
        return (template_name, None);
    };
    let numbered = version
        .strip_prefix('v')
        .is_some_and(|digits| !digits.is_empty() && digits.bytes().all(|b| b.is_ascii_digit()));
    if numbered && !template.is_empty() {
        (template, Some(version))
    } else {
        (template_name, None)
    }
}

/// `InsightRuleName`'s modelled maximum length, which a derived rule name is held inside.
const MAX_INSIGHT_RULE_NAME_LEN: usize = 128;

/// The managed-rule template `stored` was enabled from, or the empty string for a rule a
/// caller created with `PutInsightRule` — which orders the unmanaged rules before every
/// managed one, and is never reported.
pub fn template_name_of(stored: &StoredInsightRule) -> &str {
    stored
        .managed
        .as_ref()
        .map_or("", |origin| origin.template_name.as_str())
}

/// `value`'s ARN fields, or `None` when `value` is not a well-formed Amazon Resource Name.
///
/// A member documented as "The ARN of an Amazon Web Services resource"
/// (`ManagedRule$ResourceARN`, `ListManagedInsightRulesInput$ResourceARN`) is modelled only as
/// a 1..1024 string, so what makes it an ARN is read here — through the one shared parse, the
/// same one the alarm actions are read with.
pub fn parse_resource_arn(value: &str) -> Option<::service_common::arn::Arn<'_>> {
    ::service_common::arn::parse(value)
}

/// How CloudWatch states that a `ResourceARN` is not the ARN of an Amazon Web Services
/// resource, or `None` when it is one.
///
/// The sentence is the service's own for this error class and names no value: the caller
/// renders it through whichever parameter fault its own model declares.
pub fn resource_arn_error(value: &str) -> Option<String> {
    parse_resource_arn(value)
        .is_none()
        .then(|| INVALID_RESOURCE_ARN.to_string())
}

/// CloudWatch's wording for a `ResourceARN` that is not an ARN.
pub const INVALID_RESOURCE_ARN: &str = "INVALID_RESOURCE_ARN: Invalid ARN.";

/// The SENTENCE half of it, which is what a batch entry reporting the same fault carries: a
/// `Failures` entry names the entry it is about through its own members, so the member code the
/// whole-request refusal opens with has nothing to key on there.
pub const NOT_AN_ARN: &str = "Invalid ARN.";

/// How CloudWatch states that a `ResourceARN` is a well-formed ARN that addresses no resource
/// a managed rule could collect data for — the fault one step past [`NOT_AN_ARN`], and named
/// for the member rather than for the ARN form.
pub const INVALID_RESOURCE_ARN_SENTENCE: &str = "Invalid ResourceArn.";

/// The resource types this service's OWN ARN namespace addresses, each as the token an ARN of
/// such a resource carries before the resource's own name
/// (`::service_common::arn::Arn::resource_type`).
///
/// "The ARN format of an alarm is arn:aws:cloudwatch:Region:account-id:alarm:alarm-name The
/// ARN format of a Contributor Insights rule is
/// arn:aws:cloudwatch:Region:account-id:insight-rule/insight-rule-name The ARN format of a
/// dashboard is arn:aws:cloudwatch::account-id:dashboard/dashboard-name The ARN format of a
/// metric stream is arn:aws:cloudwatch:Region:account-id:metric-stream/metric-stream-name"
/// (`TagResourceInput$ResourceARN`); a dataset is addressed "in the format
/// arn:aws:cloudwatch:Region:account-id:dataset/dataset-id" (`GetDatasetInput$DatasetIdentifier`)
/// and an alarm mute rule as `arn:aws:cloudwatch:[REGION]:123456789012:alarm-mute-rule:*`
/// (`PutAlarmMuteRule`).
const ARN_RESOURCE_TYPES: [&str; 6] = [
    "alarm",
    "alarm-mute-rule",
    "dashboard",
    "dataset",
    "insight-rule",
    "metric-stream",
];

/// Whether `value` addresses an Amazon Web Services resource, as against being well formed as
/// an ARN while addressing nothing.
///
/// Two things are read, because an ARN is text and only some of it is anyone's to interpret:
///
/// * every FIELD is one of an ARN Amazon Web Services issues ([`names_an_issued_arn`]) — a
///   partition of the AWS family, a Region label or none, an account id or none, and a resource
///   field that names a resource rather than a resource type alone. This holds for every
///   vendor: a value broken in one of those fields is the ARN of no resource of any service.
/// * which resource TYPES a vendor has is defined only by the service that owns the vendor
///   field, so a value in THIS service's namespace is held to the resource types this service
///   addresses ([`ARN_RESOURCE_TYPES`]), each carrying a non-empty id of a resource of that
///   type (the same thing `parse_taggable_arn` requires of the ARNs it reads), while a value in
///   another vendor's namespace is that vendor's to judge: this service cannot enumerate
///   another's resources, and treating them as unaddressed would reject the resources of the
///   very services whose data a managed rule collects.
pub fn addresses_resource(value: &str) -> bool {
    parse_resource_arn(value).is_some_and(|arn| {
        names_an_issued_arn(&arn)
            && (arn.vendor != ARN_NAMESPACE
                || ARN_RESOURCE_TYPES.iter().any(|resource_type| {
                    ::service_common::arn::resource_id_of_type(value, resource_type)
                        .is_some_and(|id| !id.is_empty())
                }))
    })
}

/// Whether every field of `arn` is the field of an ARN Amazon Web Services issues.
///
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>` is six fields of text, and the
/// three AWS fills in itself carry fixed forms: the partition is `aws` or one of the
/// `aws-`-prefixed partitions, the Region is a Region label where the resource is regional and
/// empty where it is not (`::service_common::arn::region_label`), and the account is a
/// twelve-digit account id, or empty for a resource that belongs to no account. A value broken
/// in one of them is the ARN of no resource AWS issued, whichever vendor it names.
fn names_an_issued_arn(arn: &::service_common::arn::Arn<'_>) -> bool {
    names_a_partition(arn.partition)
        && (arn.region.is_empty() || ::service_common::arn::region_label(arn.region).is_some())
        && (arn.account.is_empty() || names_an_account(arn.account))
        && names_a_resource_of_its_type(arn.resource)
}

/// Whether `partition` is one of the AWS partitions: `aws`, or `aws-` and the partition's own
/// label (`aws-cn`, `aws-us-gov`, `aws-iso-b`), in the label grammar every ARN field uses.
fn names_a_partition(partition: &str) -> bool {
    let labelled = partition == "aws"
        || partition
            .strip_prefix("aws-")
            .is_some_and(|label| !label.is_empty());
    labelled
        && partition
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

/// Whether `account` is an Amazon Web Services account id: twelve digits.
fn names_an_account(account: &str) -> bool {
    account.len() == ACCOUNT_ID_DIGITS
        && account.bytes().all(|byte| byte.is_ascii_digit())
}

/// How many digits an Amazon Web Services account id carries — the same twelve every ARN of
/// an account's resource carries in its fifth field.
const ACCOUNT_ID_DIGITS: usize = 12;

/// Whether an ARN's `resource` field names a resource rather than a resource TYPE alone: a
/// field that carries a separator carries a non-empty id after it (`table/Orders`), and a
/// field that carries none IS the resource's own name (`arn:aws:s3:::my-bucket`).
fn names_a_resource_of_its_type(resource: &str) -> bool {
    resource
        .split_once(['/', ':'])
        .is_none_or(|(_, id)| !id.is_empty())
}

/// The largest number of Contributor Insights rules this in-memory store holds.
///
/// A capacity bound, not an AWS quota: the rule records live in this process's memory, so
/// their number has to be finite. `PutInsightRule` answers its modelled `LimitExceededException`
/// ("The operation exceeded one or more limits.") rather than growing without limit once it is
/// reached; updating a rule that already exists adds none, so it is unaffected.
pub const MAX_INSIGHT_RULES: usize = 10_000;

impl State {
    /// Whether the catalog publishes a template named `template_name` at all — for ANY resource.
    ///
    /// "The template name for the managed Contributor Insights rule, as returned by
    /// `ListManagedInsightRules`." (`ManagedRule$TemplateName`) — a name no published template
    /// answers to names no template, whatever resource it was submitted beside. Exact match: a
    /// template is named as it is published, so a name differing in case or punctuation — or one
    /// outside the form a published template name takes at all — names a different template, and
    /// none is published under it.
    pub fn publishes_template_named(&self, template_name: &str) -> bool {
        SERVICE_MANAGED_RULE_TEMPLATES
            .iter()
            .any(|template| template.template_name == template_name)
            || self
                .managed_rule_templates
                .iter()
                .any(|template| template.template_name == template_name)
    }

    /// The managed Contributor Insights rules this store holds for `resource_arn`, as
    /// (rule name, rule) pairs ordered by template name and then rule name.
    ///
    /// "The managed rules that are available for the specified Amazon Web Services resource."
    /// (`ListManagedInsightRulesOutput$ManagedRules`) — a managed rule belongs to the resource
    /// its `PutManagedInsightRules` call named, so the resource is what selects them, and the
    /// order is total so one page of a listing follows the previous one.
    pub fn managed_insight_rules_for(
        &self,
        resource_arn: &str,
    ) -> Vec<(&String, &StoredInsightRule)> {
        let mut rules: Vec<(&String, &StoredInsightRule)> = self
            .insight_rules
            .iter()
            .filter(|(_, stored)| {
                stored
                    .managed
                    .as_ref()
                    .is_some_and(|origin| origin.resource_arn == resource_arn)
            })
            .collect();
        rules.sort_unstable_by(|(left_name, left), (right_name, right)| {
            template_name_of(left)
                .cmp(template_name_of(right))
                .then_with(|| left_name.cmp(right_name))
        });
        rules
    }

    /// Publish `template` into this ACCOUNT's half of the managed-rule catalog, replacing a
    /// template already published under the same name for the same resources.
    ///
    /// The seam the account side of the catalog is written through, beside the templates the
    /// service itself publishes (`SERVICE_MANAGED_RULE_TEMPLATES`): "The template name for
    /// the managed Contributor Insights rule, as returned by `ListManagedInsightRules`."
    /// (`ManagedRule$TemplateName`) — a caller names a published template, so nothing a
    /// `PutManagedInsightRules` request carries reaches this.
    ///
    /// Held ordered by template name, which is the order a listing reports this half of the
    /// catalog in: the order templates happened to be published in is not something a client
    /// can see.
    pub fn publish_managed_rule_template(&mut self, template: ManagedRuleTemplate) {
        match self.managed_rule_templates.iter_mut().find(|held| {
            held.template_name == template.template_name
                && held.resource_vendor == template.resource_vendor
                && held.resource_type == template.resource_type
        }) {
            Some(held) => *held = template,
            None => {
                let at = self
                    .managed_rule_templates
                    .partition_point(|held| held.template_name <= template.template_name);
                self.managed_rule_templates.insert(at, template);
            }
        }
    }

    /// Whether the catalog publishes any managed rule for the VENDOR `resource_arn` belongs to
    /// — the service that owns the resource — whatever type of that vendor's resources the ARN
    /// names.
    ///
    /// A vendor the catalog publishes nothing for is a service this emulator knows no managed
    /// rule for at all; inside a vendor it does publish for, WHICH of that vendor's resources a
    /// managed rule can be enabled for is a question about the ARN
    /// ([`Self::publishes_managed_rules_for`]).
    pub fn publishes_managed_rules_for_vendor(&self, resource_arn: &str) -> bool {
        let Some(arn) = parse_resource_arn(resource_arn) else {
            return false;
        };
        SERVICE_MANAGED_RULE_TEMPLATES
            .iter()
            .any(|template| template.resource_vendor == arn.vendor)
            || self
                .managed_rule_templates
                .iter()
                .any(|template| template.resource_vendor == arn.vendor)
    }

    /// Whether the catalog publishes any managed rule for the KIND of resource `resource_arn`
    /// names — its vendor and resource type — whether or not the ARN names an individual
    /// resource of that kind.
    ///
    /// A resource kind the catalog publishes nothing for is one this service knows no managed
    /// rule for at all, which is a different thing from a template name it cannot find for a
    /// kind it does publish rules for.
    pub fn publishes_managed_rules_for(&self, resource_arn: &str) -> bool {
        SERVICE_MANAGED_RULE_TEMPLATES
            .iter()
            .any(|template| template.published_for_kind(resource_arn))
            || self
                .managed_rule_templates
                .iter()
                .any(|template| template.available_for(resource_arn))
    }

    /// Whether `resource_arn` names an INDIVIDUAL resource of every kind the catalog publishes
    /// a managed rule for it under, as against naming the kind alone.
    ///
    /// A managed rule "collects data from Amazon Web Services services"
    /// (`PutManagedInsightRules`) for ONE resource, so the ARN has to name one: a value that
    /// stops at the resource type of a kind the service publishes rules for addresses no
    /// resource to collect from. A kind this account's half of the catalog published is
    /// addressed as the ARN it was published from, so it asks for nothing more.
    pub fn names_individual_resource(&self, resource_arn: &str) -> bool {
        SERVICE_MANAGED_RULE_TEMPLATES
            .iter()
            .filter(|template| template.published_for_kind(resource_arn))
            .all(|template| template.names_individual_resource(resource_arn))
    }

    /// Whether the catalog publishes a template named `template_name` for `resource_arn` — the
    /// resolution a `PutManagedInsightRules` entry is enabled by.
    ///
    /// Exact match on the name: a template is named as it is published, so a name that differs
    /// in case or punctuation names a different template — and, when none is published under
    /// it, no template at all.
    pub fn publishes_managed_rule(&self, template_name: &str, resource_arn: &str) -> bool {
        self.published_templates_for(resource_arn)
            .contains(&template_name)
    }

    /// Every managed-rule template the catalog publishes for `resource_arn`, in the order a
    /// listing reports them: the service's own templates first, then this account's.
    pub fn published_templates_for(&self, resource_arn: &str) -> Vec<&str> {
        SERVICE_MANAGED_RULE_TEMPLATES
            .iter()
            .filter(|template| template.available_for(resource_arn))
            .map(|template| template.template_name)
            .chain(
                self.managed_rule_templates
                    .iter()
                    .filter(|template| template.available_for(resource_arn))
                    .map(|template| template.template_name.as_str()),
            )
            .collect()
    }

    /// Every managed rule `ListManagedInsightRules` reports for `resource_arn`: ONE entry per
    /// template the catalog publishes for it, carrying the rule enabled from that template when
    /// one is enabled — the rules enabled for the resource first, then the templates nothing has
    /// enabled, each ordered by template name.
    ///
    /// "The managed rules that are available for the specified Amazon Web Services resource."
    /// (`ListManagedInsightRulesOutput$ManagedRules`) — available is what the catalog publishes
    /// for the resource, whatever has been enabled, so enabling a rule from a template and
    /// deleting it again leave the listing's ENTRIES alone and change only what their
    /// `RuleState` says ("If present, it contains information about the Contributor Insights
    /// rule…", `ManagedRuleDescription$RuleState`) and which half of the listing each falls in.
    /// One entry per template is what makes the template name identify an entry, which is what
    /// a listing's page token is.
    ///
    /// A rule whose template the catalog does NOT publish is still reported — the rule exists —
    /// among the enabled ones, and a resource holds at most one rule per template by
    /// construction (the rule's name is derived from the (template, resource) pair, so enabling
    /// the same template again names the same rule).
    pub fn managed_rules_available_for(&self, resource_arn: &str) -> Vec<AvailableManagedRule<'_>> {
        let enabled = self.managed_insight_rules_for(resource_arn);
        let mut available: Vec<AvailableManagedRule<'_>> = Vec::new();
        for (name, stored) in &enabled {
            let template_name = template_name_of(stored);
            if available
                .iter()
                .any(|reported| reported.template_name == template_name)
            {
                continue;
            }
            available.push(AvailableManagedRule {
                template_name,
                rule: Some((*name, *stored)),
            });
        }
        let mut unenabled: Vec<&str> = self
            .published_templates_for(resource_arn)
            .into_iter()
            .filter(|template_name| {
                !available
                    .iter()
                    .any(|reported| reported.template_name == *template_name)
            })
            .collect();
        unenabled.sort_unstable();
        available.extend(unenabled.into_iter().map(|template_name| {
            AvailableManagedRule {
                template_name,
                rule: None,
            }
        }));
        available
    }

    /// Move every Contributor Insights rule `rule_names` names to `rule_state`, reporting back
    /// one entry per name this store could not write that state on.
    ///
    /// "An array listing the rules that could not be disabled."
    /// (`DisableInsightRulesOutput$Failures`) — a name no rule answers to is reported as an
    /// entry of that array and the rest of the request still takes effect, rather than the
    /// whole call failing. "This array is empty if the API operation was successful for all
    /// the rules specified in the request." (`PartialFailure`)
    ///
    /// Each name is judged ON ITS OWN, in two steps, so one unusable entry costs the batch
    /// nothing else: a value that is not a rule NAME at all is reported as the invalid value it
    /// is ([`insight_rule_name_fault`]), and a name that is legal but addresses no rule of this
    /// store is reported as the resource that is not there
    /// ([`unknown_insight_rule_failure`]) — which is what lets a client tell "repair this
    /// value" from "create this rule" and keeps every other name in the request applied.
    ///
    /// The state a rule moves to is the only thing that differs between the operations that
    /// enable and disable rules, so the walk lives here and neither can drift from the other.
    /// Naming a rule that already holds the state asks for no change and is not a failure: the
    /// operations say what a rule's state BECOMES.
    pub fn set_insight_rule_state(
        &mut self,
        rule_names: &[String],
        rule_state: &str,
    ) -> Vec<PartialFailure> {
        let mut failures: Vec<PartialFailure> = Vec::new();
        for rule_name in rule_names {
            if let Some(fault) = insight_rule_name_fault(rule_name) {
                failures.push(illegal_insight_rule_name_failure(rule_name, fault));
                continue;
            }
            match self.insight_rules.get_mut(rule_name) {
                Some(stored) => stored.rule.state = rule_state.to_string(),
                None => failures.push(unknown_insight_rule_failure(rule_name)),
            }
        }
        failures
    }

    /// Remove every Contributor Insights rule `rule_names` names, reporting back one entry per
    /// rule it could not delete.
    ///
    /// "Permanently deletes the specified Contributor Insights rules."
    /// (`DeleteInsightRules`) — the rule and everything stored with it (its definition, its
    /// state and its tags) go away, so a later read reports no such rule. "An array listing the
    /// rules that could not be deleted." (`DeleteInsightRulesOutput$Failures`) carries the names
    /// the request did not leave in the state it asked for, with the rest of the request still
    /// taking effect.
    ///
    /// The names a request carries are the SET of rules it deletes, so a name it repeats asks
    /// for one deletion. A rule `PutManagedInsightRules` enabled is removed by the same walk:
    /// "The rules can be enabled, disabled, and deleted using EnableInsightRules,
    /// DisableInsightRules, and DeleteInsightRules." (`PutManagedInsightRules`).
    pub fn delete_insight_rules(&mut self, rule_names: &[String]) -> Vec<PartialFailure> {
        for rule_name in rule_names {
            self.insight_rules.remove(rule_name);
        }
        // "An array listing the rules that could not be deleted. You cannot delete built-in
        // rules." (`DeleteInsightRulesOutput$Failures`) — a DELETE is reported on by what it
        // left behind, not by what it found: a name this store holds no rule for already names
        // nothing, which is the end state the request asks for, so the walk above reached it
        // and there is no failure to report. Every rule this store does hold was put by a
        // caller or enabled from a managed-rule template, and this operation deletes all of
        // those, so nothing a request names is a rule it could not delete. That is why the
        // operations that MOVE a rule's state report the same name as a failure
        // ([`State::set_insight_rule_state`]) and this one does not: a state cannot be written
        // on a rule that is not there, while a deletion of one is already done.
        Vec::new()
    }
}
