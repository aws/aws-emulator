// SPDX-License-Identifier: CC0-1.0

//! The other question a resource type's schema makes decidable about a property's VALUE: the
//! spelling it admits, stated as a pattern.
//!
//! A resource type's schema states, beside the set of values a property publishes, the
//! `pattern` a value of that property has to match — the form a value has to be IN for the
//! owning service to be able to carry the declaration out at all (an ARN of a particular
//! kind, a name over a particular alphabet). So a property document carrying a value outside
//! its property's pattern is one no handler of that type could ever be given: the stage that
//! runs before provisioning reports it, quoting the value and the pattern it does not match,
//! and the operation deploys nothing. That is the question this module answers, beside the
//! four [`super`] already answers (a required property the declaration omits, a member the
//! type does not have, a group it may carry at most one of, and a value outside the set its
//! property publishes).
//!
//! The judgement is made of the DEPLOYED property document, as every judgement of that stage
//! is, and it is the same kind of catalogue: one row per (resource type, property) whose
//! pattern the type's schema states, so a property no row covers is left to the service that
//! owns the resource rather than guessed at. A value that is not TEXT is admitted — it is no
//! spelling either way (an expression this service left unevaluated, a structure the author
//! wrote where the schema states a string), and what it fails is a constraint this stage does
//! not judge.
//!
//! A schema's `pattern` belongs to the property it is declared on WHEREVER that property sits
//! in the declaration, so a row names the CHAIN of members leading from the resource's
//! property document down to the judged value ([`PatternedProperty::property`]) rather than
//! one property name — the same reading the value-set question of this stage is in
//! ([`super::enum_properties`]). A member of a LIST-valued property is judged element by
//! element, the first element outside the pattern is what the stage reports, and the path it
//! reports names THAT element: the element's index under the member it sits in, which is the
//! form a recorded refusal of a list-valued property carries.
//!
//! A pattern is applied the way the schema language applies it: as a search over the value, so
//! an expression the schema anchors itself constrains the whole value and one it does not
//! constrains the part of the value it matches. The expressions are this catalogue's own text
//! rather than a caller's, so the one no engine of this tree reads constrains nothing instead
//! of failing a declaration the real service accepts — which is also why the evaluation is the
//! ONE evaluation of a published expression this tree holds
//! ([`::service_common::unapplied_patterns::modelled_pattern_admits`]): a schema states the
//! expressions its owning API publishes, look-around and all, and an engine without
//! look-around reads none of those.

use ::serde_json::Value;

use super::{PropertyFailure, StoredResource};

/// One property whose admitted spelling its type's schema states as a pattern.
#[derive(Debug, Clone, Copy)]
pub struct PatternedProperty {
    /// The resource type the property belongs to, as a template declares it.
    pub resource_type: &'static str,
    /// The chain of members leading from the resource's property document down to the property
    /// the schema states the pattern on, each spelled as a template spells it: one name for a
    /// property of the resource itself, and the members stepped through for one nested inside a
    /// structured or list-valued property.
    pub property: &'static [&'static str],
    /// The regular expression its type's schema states for the property, in the schema's own
    /// spelling — which is also the text a refusal quotes, since what the author has to act on
    /// is the pattern the property is held to.
    pub pattern: &'static str,
}

/// The properties this stage knows a published pattern of.
///
/// Source for each row: the `Pattern` the resource type's entry in the "AWS resource and
/// property types reference" of the CloudFormation User Guide publishes for that property,
/// which is the `pattern` the type's registry schema states — the form the OWNING API
/// publishes for the member the property is passed to, so a value outside it is one that
/// service would refuse and the validation refuses it first.
///
/// Each row carries the SCHEMA's own spelling, which is the text a refusal quotes, and a row's
/// comment names the owning API member's `@pattern` beside it: the two state the same form but
/// not always with the same anchoring (a schema states the CodePipeline role-ARN form
/// unanchored and trailing `.*` where the API anchors it at the start), and what a declaration
/// is held to — and told about — is the schema's.
pub const PATTERNED_PROPERTIES: &[PatternedProperty] = &[
    // `AWS::CodePipeline::Pipeline`'s `RoleArn` is `PipelineDeclaration$roleArn`, "the Amazon
    // Resource Name (ARN) for CodePipeline to use to either perform actions with no
    // actionRoleArn, or to use to assume roles for actions with an actionRoleArn" — an IAM
    // role ARN of this partition and account, the form CodePipeline publishes for that member
    // as `^arn:aws(-[\w]+)*:iam::[0-9]{12}:role/` and the schema states as the row below.
    PatternedProperty {
        resource_type: "AWS::CodePipeline::Pipeline",
        property: &["RoleArn"],
        pattern: r"arn:aws(-[\w]+)*:iam::[0-9]{12}:role/.*",
    },
    // ...and its `Name` is `PipelineDeclaration$name`, "the name of the pipeline", over the
    // alphabet CodePipeline publishes for that member as `^[A-Za-z0-9.@\-_]+$` and the schema
    // states as the row below.
    PatternedProperty {
        resource_type: "AWS::CodePipeline::Pipeline",
        property: &["Name"],
        pattern: r"[A-Za-z0-9.@\-_]+",
    },
    // `AWS::Lambda::Function`'s `Role` is `CreateFunction$Role`, "the Amazon Resource Name
    // (ARN) of the function's execution role" — the role ARN form AWS Lambda publishes for
    // that member, which this type's schema states verbatim, anchors and all.
    PatternedProperty {
        resource_type: "AWS::Lambda::Function",
        property: &["Role"],
        pattern: r"^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z_0-9+=,.@\-_/]+$",
    },
    // `AWS::Pipes::Pipe`'s `RoleArn` is `CreatePipe$RoleArn`, "the ARN of the role that allows
    // the pipe to send data to the target" — the role ARN form Amazon EventBridge Pipes
    // publishes for that member, which this type's schema states verbatim, anchors and all.
    PatternedProperty {
        resource_type: "AWS::Pipes::Pipe",
        property: &["RoleArn"],
        pattern: r"^arn:(aws[a-zA-Z-]*)?:iam::\d{12}:role/?[a-zA-Z0-9+=,.@\-_/]+$",
    },
    // `AWS::CloudHSM::Cluster`'s `HsmType` is `CreateCluster$HsmType`, "the type of HSM to use
    // in the cluster" — the form CloudHSM publishes for that member, which this type's schema
    // states verbatim, anchors and all.
    PatternedProperty {
        resource_type: crate::state::template::CLOUDHSM_CLUSTER,
        property: &["HsmType"],
        pattern: r"^((p|)hsm[0-9][a-z.]*\.[a-zA-Z]+)$",
    },
    // `AWS::SSMQuickSetup::ConfigurationManager`'s `Name` is "a name for the configuration
    // manager", which the type's own schema states as the row below: a narrower form than the
    // one Quick Setup publishes for `CreateConfigurationManager$Name`
    // (`^[ A-Za-z0-9._-]{0,120}$`) — the alphabet without the dot, at least one character and
    // at most fifty — which is what the resource reference publishes for the property, and so
    // what a declaration is held to and told about.
    PatternedProperty {
        resource_type: crate::state::template::SSM_QUICKSETUP_CONFIGURATION_MANAGER,
        property: &["Name"],
        pattern: r"^[ A-Za-z0-9_-]{1,50}$",
    },
    // `AWS::BCM::Dashboard`'s `Name` is `CreateDashboard$name`, "the name of the dashboard",
    // and its `Description` that call's `description`, "a description of the dashboard" — the
    // forms Billing and Cost Management Dashboards publishes for those members
    // (`smithy_query(query="operation", operation="CreateDashboard")` of the
    // `com-amazonaws-bcmdashboards-awsbcmdashboardsservice` model), which this type's schema
    // states verbatim, anchors, look-around and all.
    PatternedProperty {
        resource_type: crate::state::template::BCM_DASHBOARD,
        property: &["Name"],
        pattern: r"^(?!.* {2})[a-zA-Z][a-zA-Z0-9 _-]{0,48}[a-zA-Z0-9_-]$",
    },
    PatternedProperty {
        resource_type: crate::state::template::BCM_DASHBOARD,
        property: &["Description"],
        pattern: r"^(?!.* {2})[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*$",
    },
    // ...and the `Title` and `Description` of each of its `Widgets` are `Widget$title`, "the
    // title of the widget", and `Widget$description`, "a description of the widget's purpose or
    // the data it displays" (`smithy_query(query="shape", shape="Widget")` of the same model):
    // members NESTED inside a list-valued property, each held to the form that API publishes
    // for it and this type's schema states verbatim.
    PatternedProperty {
        resource_type: crate::state::template::BCM_DASHBOARD,
        property: &["Widgets", "Title"],
        pattern: r"^(?!.* {2})[a-zA-Z0-9_-][ a-zA-Z0-9_-]*[a-zA-Z0-9_-]$",
    },
    PatternedProperty {
        resource_type: crate::state::template::BCM_DASHBOARD,
        property: &["Widgets", "Description"],
        pattern: r"^(?!.* {2})[ a-zA-Z0-9.,!?;:@#$%&\-_/\\]*$",
    },
    // `AWS::AIOps::InvestigationGroup`'s `RoleArn` is `CreateInvestigationGroup$roleArn`, "the
    // ARN of the IAM role that CloudWatch investigations will use when it gathers investigation
    // data", and its `Name` that call's `name`, "a name for the investigation group" — the forms
    // that service publishes for those members
    // (`smithy_query(query="operation",operation="CreateInvestigationGroup")` of the
    // `com-amazonaws-aiops-aiops` model: `^arn:.*$` and
    // `^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$`), which this type's schema states verbatim, anchors
    // and all. A value outside either is one that service would refuse, so the stage that runs
    // before provisioning refuses it first.
    PatternedProperty {
        resource_type: crate::state::template::AIOPS_INVESTIGATION_GROUP,
        property: &["Name"],
        pattern: r"^[\-_A-Za-z0-9\[\]\(\)\{\}\.: ]+$",
    },
    PatternedProperty {
        resource_type: crate::state::template::AIOPS_INVESTIGATION_GROUP,
        property: &["RoleArn"],
        pattern: r"^arn:.*$",
    },
    // `AWS::BackupGateway::Hypervisor`'s properties, in the order its own property reference
    // publishes them, each held to the form Backup Gateway publishes for the member of
    // `ImportHypervisorConfiguration`/`UpdateHypervisor` the property is passed to, which this
    // type's schema states verbatim, anchors and all.
    //
    // `Host` is "the server host of the hypervisor", whose published form is any non-empty line
    // (`@pattern("^.+$")`).
    PatternedProperty {
        resource_type: crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
        property: &["Host"],
        pattern: r"^.+$",
    },
    // `KmsKeyArn` is the key the hypervisor's credentials are encrypted with, published as a
    // KMS key or alias ARN, or a bare alias.
    PatternedProperty {
        resource_type: crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
        property: &["KmsKeyArn"],
        pattern: r"^(^arn:(aws|aws-cn|aws-us-gov):kms:([a-zA-Z0-9-]+):([0-9]+):(key|alias)/(\S+)$)|(^alias/(\S+)$)$",
    },
    // `LogGroupArn` is the group the hypervisor reports into — the member only
    // `UpdateHypervisor` carries — published as a CloudWatch Logs log-group ARN or the empty
    // value that clears it.
    PatternedProperty {
        resource_type: crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
        property: &["LogGroupArn"],
        pattern: r"^$|^arn:(aws|aws-cn|aws-us-gov):logs:([a-zA-Z0-9-]+):([0-9]+):log-group:[a-zA-Z0-9_\-\/\.]+:\*$",
    },
    // `Name` is the name of the hypervisor, over the alphabet that service publishes for it
    // (`@pattern("^[a-zA-Z0-9-]*$")` with `@length(1..100)`, the bound being a constraint class
    // this stage does not judge).
    PatternedProperty {
        resource_type: crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
        property: &["Name"],
        pattern: r"^[a-zA-Z0-9-]*$",
    },
    // The type's `Username` and `Password` publish a pattern too, and are deliberately NOT
    // rows: both are `@sensitive` members of the owning API, and what this stage reports for a
    // value outside its pattern names the property it judged — so a credential outside its own
    // form is left to the service that owns the resource, which refuses it without this stage
    // having to carry it.
];

/// `Some(failure)` for a resource carrying a property value outside the pattern its type's
/// schema states for that property ([`PATTERNED_PROPERTIES`]) — the first such property in the
/// order the catalogue states them, and within a list-valued one the first element outside the
/// pattern; `None` for a type with no row, for a property the declaration does not carry, for a
/// value that is not text, and for every value its pattern matches.
pub(super) fn value_outside_its_pattern(resource: &StoredResource) -> Option<PropertyFailure> {
    PATTERNED_PROPERTIES
        .iter()
        .filter(|row| row.resource_type == resource.resource_type)
        .find_map(|row| {
            let (at, rejected) = judged_values(&resource.properties, row.property)
                .into_iter()
                .find(|(_, declared)| !admits(row.pattern, declared))?;
            let at: Vec<&str> = at.iter().map(String::as_str).collect();
            Some(PropertyFailure {
                logical_id: resource.logical_id.clone(),
                resource_type: resource.resource_type.clone(),
                reason: pattern_failure_reason(
                    super::reported_value(&resource.resource_type, rejected),
                    row.pattern,
                ),
                path: super::member_path(&resource.logical_id, &at),
            })
        })
}

/// Whether `pattern` ADMITS `value`, by the ONE evaluation of a published expression this tree
/// holds ([`::service_common::unapplied_patterns::modelled_pattern_admits`]): a search over the
/// value, by the engine that reads the expression, and every value where no engine reads it at
/// all.
///
/// A schema states the expressions its owning API publishes, and those carry look-around that
/// the linear-time engine does not read — so evaluating them anywhere else would silently admit
/// every value of the properties whose form matters most. What one evaluation may spend is
/// bounded THERE, by that module's own step budget, and the expression handed to it is this
/// catalogue's own static text rather than anything a request carries; how many evaluations one
/// declaration can ask for is bounded by the stage's own walk of a member chain
/// ([`super::MAX_JUDGED_VALUES`]), so the work one request buys
/// is a product of two constants rather than of a constant and a value a template chooses.
fn admits(pattern: &'static str, value: &str) -> bool {
    ::service_common::unapplied_patterns::modelled_pattern_admits(pattern, value)
}

/// Every TEXT a declaration carries at a row's member chain, each with the path it sits at
/// under the resource's property document — the chain's own members, with the INDEX of an
/// element where the chain stepped through a list.
///
/// The walk is the stage's one walk of a member chain ([`super::judged_at`]), which bounds how
/// many values one declaration is judged on and where each of them sits; what this adds is
/// which of those values this question is about. A member the declaration does not carry, a
/// value that is not text and a chain that runs into a value no member can be read off
/// contribute nothing: none of them is a spelling this stage judges. The bound is charged
/// against every value the chain NAMES rather than against the spellings alone, so a list
/// carrying values of other types judges fewer spellings than its length — which leaves those
/// ADMITTED, the reading every value this stage cannot decide about gets.
pub(super) fn judged_values<'a>(document: &'a Value, chain: &[&str]) -> Vec<(Vec<String>, &'a str)> {
    super::judged_at(document, chain)
        .into_iter()
        .filter_map(|(at, declared)| declared.as_str().map(|declared| (at, declared)))
        .collect()
}

/// How CloudFormation reports a property value outside the pattern its type's schema states:
/// fixed text around the value that was judged and the pattern it does not match.
///
/// Both variable parts come from what was judged — the pattern is the one its property is held
/// to ([`PATTERNED_PROPERTIES`]) and the value the declaration's own, as far as a failure of
/// this stage may say it ([`super::reported_value`]) — and the value is named here, unlike the
/// set-membership failure this stage also reports, because a spelling is only actionable
/// beside the form it had to be in.
pub fn pattern_failure_reason(value: &str, pattern: &str) -> String {
    format!("Property value [{value}] does not match pattern: {pattern}")
}

#[cfg(test)]
mod tests {
    use super::*;
    use super::super::MAX_JUDGED_VALUES;
    use ::serde_json::json;
    use ::std::collections::BTreeMap;

    /// One declared resource of `resource_type` carrying `properties`.
    fn declared(logical_id: &str, resource_type: &str, properties: Value) -> StoredResource {
        StoredResource {
            logical_id: logical_id.to_string(),
            resource_type: resource_type.to_string(),
            physical_resource_id: format!("{logical_id}-physical"),
            attributes: BTreeMap::new(),
            metadata: None,
            properties,
            materialised: false,
            deletion_policy: None,
            update_replace_policy: None,
        }
    }

    /// The values every row is probed with: text over characters no alphabet of a resource
    /// property's schema carries, and the empty value a form stating a minimum does not admit.
    ///
    /// No ONE value is outside every published form — one row admits any non-empty line and
    /// another admits the empty value that clears it — so what a row is held to is refusing at
    /// least one of them, which a pattern no engine read could not do.
    const PROBES: &[&str] = &["\u{7}\u{7}", ""];

    /// RULE: what the stage judges against one row is bounded by [`MAX_JUDGED_VALUES`] and not
    /// by how many elements the declaration carries, so a value past that bound is left
    /// ADMITTED rather than deciding how much work one request buys.
    ///
    /// The bound is a LIMIT of the stage and is pinned so it is visible rather than only
    /// commented: a rejected value inside it fails the validation, the same value past it does
    /// not, and the service that owns the resource is what judges it then.
    #[test]
    fn the_values_judged_against_one_row_are_bounded() {
        let title = |title: &str| json!({"Title": title});
        let rows: &[(&str, usize, bool)] = &[
            ("the rejected title is the last one inside the bound", MAX_JUDGED_VALUES - 1, true),
            ("the rejected title is the first one past the bound", MAX_JUDGED_VALUES, false),
        ];
        for (case, at, refused) in rows {
            let mut widgets: Vec<Value> = (0..*at).map(|_| title("Monthly Spend")).collect();
            widgets.push(title(" Leading Space"));
            let resources = vec![declared(
                "R",
                crate::state::template::BCM_DASHBOARD,
                json!({"Name": "Cost Overview", "Widgets": widgets}),
            )];
            let failed = super::super::failure(&resources);
            assert_eq!(
                failed.is_some(),
                *refused,
                "case {case:?}: expected the stage to report a failure: {refused}, got {failed:?}"
            );
        }
    }

    /// Every row of the catalogue really CONSTRAINS: an expression no engine of this tree reads
    /// admits every value, so a row whose pattern went unread would refuse nothing at all.
    #[test]
    fn every_stated_pattern_constrains_its_property() {
        for row in PATTERNED_PROPERTIES {
            let refused = PROBES.iter().any(|probe| !admits(row.pattern, probe));
            assert!(
                refused,
                "case {:?} of {}: expected the pattern {:?} to refuse one of {PROBES:?}, it \
                 admits every one",
                row.property, row.resource_type, row.pattern
            );
        }
    }

    /// One row of the table below: the case, the resource type declared, the property document
    /// it carries, and the `(member chain, path under the property document, value)` the stage
    /// must report — `None` where every judged value is one its pattern admits.
    type PatternRow<'a> = (
        &'a str,
        &'a str,
        Value,
        Option<(&'a [&'a str], &'a str, &'a str)>,
    );

    /// The type whose nested rows the table below is also about.
    const BCM_DASHBOARD: &str = crate::state::template::BCM_DASHBOARD;

    /// RULE: a value outside the pattern its type's schema states for the property fails the
    /// validation, quoting that value and that pattern at the path of the member that carries
    /// it; a value the pattern matches does not. The rows span the catalogue — types and
    /// properties the report that led here did not cite among them — the depth the judged member
    /// sits at (a property of the resource, a member inside a list-valued property), where in
    /// the list it sits, and the forms a value can take.
    #[test]
    fn a_value_outside_its_pattern_fails_the_validation() {
        // (case, type, properties, the (chain, path, value) the stage must report — `None` for a
        // declaration every judged value of which its pattern matches).
        let rows: &[PatternRow] = &[
            (
                "a pipeline whose role is no ARN at all",
                "AWS::CodePipeline::Pipeline",
                json!({"RoleArn": "not-an-arn"}),
                Some((&["RoleArn"], "RoleArn", "not-an-arn")),
            ),
            (
                "a pipeline whose role ARN names another service",
                "AWS::CodePipeline::Pipeline",
                json!({"RoleArn": "arn:aws:codepipeline:us-east-1:000000000000:role/p"}),
                Some((
                    &["RoleArn"],
                    "RoleArn",
                    "arn:aws:codepipeline:us-east-1:000000000000:role/p",
                )),
            ),
            (
                "a pipeline whose role ARN carries no account",
                "AWS::CodePipeline::Pipeline",
                json!({"RoleArn": "arn:aws:iam:::role/p"}),
                Some((&["RoleArn"], "RoleArn", "arn:aws:iam:::role/p")),
            ),
            (
                "a pipeline whose role ARN is of a partition the form admits",
                "AWS::CodePipeline::Pipeline",
                json!({"RoleArn": "arn:aws-us-gov:iam::000000000000:role/p"}),
                None,
            ),
            (
                "a pipeline whose name is over the alphabet its schema states",
                "AWS::CodePipeline::Pipeline",
                json!({"Name": "release.pipeline-1_2@ci",
                       "RoleArn": "arn:aws:iam::000000000000:role/p"}),
                None,
            ),
            (
                "a pipeline whose name is over no character that alphabet admits",
                "AWS::CodePipeline::Pipeline",
                json!({"Name": "!!!", "RoleArn": "arn:aws:iam::000000000000:role/p"}),
                Some((&["Name"], "Name", "!!!")),
            ),
            (
                "a function whose execution role is a role NAME rather than an ARN",
                "AWS::Lambda::Function",
                json!({"Role": "my-exec-role"}),
                Some((&["Role"], "Role", "my-exec-role")),
            ),
            (
                "a function whose execution role ARN is of this account",
                "AWS::Lambda::Function",
                json!({"Role": "arn:aws:iam::000000000000:role/service-role/exec"}),
                None,
            ),
            (
                "a pipe whose role ARN carries an account of the wrong width",
                "AWS::Pipes::Pipe",
                json!({"RoleArn": "arn:aws:iam::123:role/pipe",
                       "Source": "arn:aws:sqs:us-east-1:000000000000:q",
                       "Target": "arn:aws:sqs:us-east-1:000000000000:t"}),
                Some((&["RoleArn"], "RoleArn", "arn:aws:iam::123:role/pipe")),
            ),
            (
                "a pipe whose role ARN is well formed",
                "AWS::Pipes::Pipe",
                json!({"RoleArn": "arn:aws:iam::000000000000:role/pipe",
                       "Source": "arn:aws:sqs:us-east-1:000000000000:q",
                       "Target": "arn:aws:sqs:us-east-1:000000000000:t"}),
                None,
            ),
            (
                "a pipeline whose role reads an attribute that has not landed yet",
                "AWS::CodePipeline::Pipeline",
                json!({"RoleArn": Value::Null}),
                None,
            ),
            (
                "a pipeline carrying a structure where the schema states a string",
                "AWS::CodePipeline::Pipeline",
                json!({"RoleArn": {"Fn::GetAtt": ["Role", "Arn"]}}),
                None,
            ),
            (
                "a type this stage knows no pattern of",
                "AWS::SNS::Topic",
                json!({"TopicName": "not a topic name"}),
                None,
            ),
            (
                "a cluster whose HSM type is no HSM type at all",
                crate::state::template::CLOUDHSM_CLUSTER,
                json!({"HsmType": "not-a-valid-hsm-type", "SubnetIds": ["subnet-0a1b2c3d"]}),
                Some((&["HsmType"], "HsmType", "not-a-valid-hsm-type")),
            ),
            (
                "a cluster whose HSM type names the family without a size",
                crate::state::template::CLOUDHSM_CLUSTER,
                json!({"HsmType": "hsm2m", "SubnetIds": ["subnet-0a1b2c3d"]}),
                Some((&["HsmType"], "HsmType", "hsm2m")),
            ),
            (
                "a cluster whose HSM type is one the form admits",
                crate::state::template::CLOUDHSM_CLUSTER,
                json!({"HsmType": "hsm2m.medium", "SubnetIds": ["subnet-0a1b2c3d"]}),
                None,
            ),
            (
                "a cluster whose HSM type is the earlier generation, which the form admits too",
                crate::state::template::CLOUDHSM_CLUSTER,
                json!({"HsmType": "hsm1.medium", "SubnetIds": ["subnet-0a1b2c3d"]}),
                None,
            ),
            // The nested rows: a member of a LIST-valued property, whose pattern carries the
            // look-around no linear-time engine reads.
            (
                "a dashboard whose only widget's title starts with a space",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview",
                       "Widgets": [{"Title": " Leading Space"}]}),
                Some((
                    &["Widgets", "Title"],
                    "Widgets/0/Title",
                    " Leading Space",
                )),
            ),
            (
                "a dashboard whose second widget's title carries two consecutive spaces",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview",
                       "Widgets": [{"Title": "Monthly Spend"},
                                   {"Title": "Monthly  Spend"}]}),
                Some((
                    &["Widgets", "Title"],
                    "Widgets/1/Title",
                    "Monthly  Spend",
                )),
            ),
            (
                "a dashboard whose widget title is one character, below the form's two",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview", "Widgets": [{"Title": "S"}]}),
                Some((&["Widgets", "Title"], "Widgets/0/Title", "S")),
            ),
            (
                "a dashboard whose widget titles are each over the alphabet its schema states",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview",
                       "Widgets": [{"Title": "Monthly Spend"},
                                   {"Title": "Usage_By-Service 2"}]}),
                None,
            ),
            (
                "a dashboard whose widget description carries a character the form forbids",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview",
                       "Widgets": [{"Title": "Monthly Spend",
                                    "Description": "Spend by \"service\""}]}),
                Some((
                    &["Widgets", "Description"],
                    "Widgets/0/Description",
                    "Spend by \"service\"",
                )),
            ),
            (
                "a dashboard whose widget description is over the punctuation it admits",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview",
                       "Widgets": [{"Title": "Monthly Spend",
                                    "Description": "Spend & usage: by service, 100%!"}]}),
                None,
            ),
            (
                "a dashboard whose own name starts with a digit",
                BCM_DASHBOARD,
                json!({"Name": "2026 Costs", "Widgets": [{"Title": "Monthly Spend"}]}),
                Some((&["Name"], "Name", "2026 Costs")),
            ),
            (
                "a dashboard whose own description carries two consecutive spaces",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview",
                       "Description": "Spend  and usage.",
                       "Widgets": [{"Title": "Monthly Spend"}]}),
                Some((&["Description"], "Description", "Spend  and usage.")),
            ),
            (
                "a dashboard that declares no widget at all, so no title is judged",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview", "Widgets": []}),
                None,
            ),
            (
                "a dashboard whose widget states no title",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview", "Widgets": [{"Description": "No title."}]}),
                None,
            ),
            (
                "a dashboard whose widget title has not landed yet",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview", "Widgets": [{"Title": {"Ref": "Title"}}]}),
                None,
            ),
            (
                "a dashboard whose widget list is an unevaluated expression",
                BCM_DASHBOARD,
                json!({"Name": "Cost Overview", "Widgets": {"Fn::ImportValue": "Widgets"}}),
                None,
            ),
            // A type that carries CREDENTIALS among its properties: the same judgement, and
            // the value reported as far as such a document may say it
            // ([`super::super::reported_value`], which the expectation below applies too).
            (
                "a hypervisor whose name carries a character its alphabet does not admit",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "10.0.0.5", "Name": "a_hypervisor"}),
                Some((&["Name"], "Name", "a_hypervisor")),
            ),
            (
                "a hypervisor whose name is over letters, digits and the hyphen",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "10.0.0.5", "Name": "vcenter-01"}),
                None,
            ),
            (
                "a hypervisor whose host is the empty value its form does not admit",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "", "Name": "empty-host"}),
                Some((&["Host"], "Host", "")),
            ),
            (
                "a hypervisor whose encryption key is no key ARN or alias",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "10.0.0.5", "Name": "bad-key",
                       "KmsKeyArn": "not-a-key-arn"}),
                Some((&["KmsKeyArn"], "KmsKeyArn", "not-a-key-arn")),
            ),
            (
                "a hypervisor whose encryption key is a bare alias, which its form admits",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "10.0.0.5", "Name": "key-alias",
                       "KmsKeyArn": "alias/backup-gateway"}),
                None,
            ),
            (
                "a hypervisor whose log group is no log-group ARN",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "10.0.0.5", "Name": "bad-log-group",
                       "LogGroupArn": "not-a-log-group-arn"}),
                Some((&["LogGroupArn"], "LogGroupArn", "not-a-log-group-arn")),
            ),
            (
                "a hypervisor whose log group is the empty value its form admits",
                crate::state::template::BACKUP_GATEWAY_HYPERVISOR,
                json!({"Host": "10.0.0.5", "Name": "unlogged", "LogGroupArn": ""}),
                None,
            ),
        ];
        for (case, resource_type, properties, reported) in rows {
            let resources = vec![declared("R", resource_type, properties.clone())];
            let failed = super::super::failure(&resources);
            let expected = reported.map(|(property, path, value)| {
                let pattern = PATTERNED_PROPERTIES
                    .iter()
                    .find(|row| row.resource_type == *resource_type && row.property == property)
                    .map(|row| row.pattern)
                    .unwrap_or_default();
                (
                    pattern_failure_reason(
                        super::super::reported_value(resource_type, value),
                        pattern,
                    ),
                    format!("/Resources/R/Properties/{path}"),
                )
            });
            assert_eq!(
                failed
                    .as_ref()
                    .map(|failure| (failure.reason.clone(), failure.path.clone())),
                expected,
                "case {case:?}: expected the failure {expected:?}, got {failed:?}"
            );
        }
    }
}
