// SPDX-License-Identifier: CC0-1.0

//! How this service states a request it refuses at its boundary, end to end: the clause a
//! broken modelled constraint is reported with, over a TABLE of inputs rather than the one
//! request a recorded unit happened to send.
//!
//! Four rules are pinned here, each over several operations, members, constraint classes
//! and list positions:
//!
//! 1. A refusal of a value outside an enumerated type lists the values the service
//!    ACCEPTS — which for a filter key whose published set has outgrown this checkout's
//!    model is more than the model declares — in the order the service lists them, which
//!    is neither the model's declaration order nor lexicographic.
//! 2. A clause quotes the value it refused, except in the operations whose refusals state
//!    none: those name the member and the requirement alone, whatever constraint class was
//!    broken and whatever position in a list the member sat at.
//! 3. A clause stated at a COLLECTION member quotes the collection the caller submitted,
//!    rendered `[<element>, …]` — never a count of its elements, which no caller sent.
//! 4. A managed-node filter value outside the set its key publishes is the modelled
//!    filter-value error of the operation that read it, naming the key and the values it
//!    accepts as the API publishes them, whichever operator the filter named.
//!
//! Every case goes through the front door a client uses — the shared harness in
//! `all/parameter_store.rs` ([`router`], [`call`]) dispatches `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` through the multi-service router.

use serde_json::{json, Value};

use service_com_amazonaws_ssm_amazonssm::ops::types::{
    ASSOCIATION_STATUS_NAME_VALUES, PING_STATUS_VALUES, PLATFORM_TYPE_VALUES,
    RESOURCE_TYPE_VALUES, SOURCE_TYPE_VALUES,
};
use service_com_amazonaws_ssm_amazonssm::state::node_filter_key_value_set;

use crate::parameter_store::{call, error_code, error_message, router};

/// The envelope a refusal of ONE broken constraint carries, as the framework writes it.
fn one_violation(clause: &str) -> String {
    format!("1 validation error detected: {clause}")
}

/// The clause a refusal states for a value outside an enumerated type, with the value
/// QUOTED: the subject, the member path and the set the member admits.
fn enum_clause(value: &str, path: &str, set: &str) -> String {
    format!(
        "Value '{value}' at '{path}' failed to satisfy constraint: Member must satisfy enum \
         value set: [{set}]"
    )
}

/// The same clause with NO value: the members whose refusals state none.
fn valueless_enum_clause(path: &str, set: &str) -> String {
    format!(
        "Value at '{path}' failed to satisfy constraint: Member must satisfy enum value set: \
         [{set}]"
    )
}

/// RULE 1: the value set a refusal lists, and the order it lists it in, over every
/// enumerated member a recorded refusal of this service pins — and one the recordings do
/// not cover, to show the order is the service's for every type rather than for a table of
/// them.
///
/// The accepted sets and their order are written out here, independently of the service, so
/// the test pins them rather than echoing whatever the service holds.
#[tokio::test]
async fn an_enum_value_set_is_listed_as_the_service_lists_it() {
    let router = router();

    // (case, operation, body, the clause the refusal states)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "a cloud-connector filter key: the service accepts two keys the model omits",
            "ListCloudConnectors",
            json!({
                "Filters": [{ "FilterKey": "NotARealFilterKey", "FilterValues": ["x"] }],
            }),
            enum_clause(
                "NotARealFilterKey",
                "filters.1.member.filterKey",
                "TenantId, OrganizationId, ProjectId, SubscriptionId",
            ),
        ),
        (
            "a command filter key: the service accepts the command and node ids too",
            "ListCommandInvocations",
            json!({ "Filters": [{ "key": "NotARealCommandFilterKey", "value": "x" }] }),
            enum_clause(
                "NotARealCommandFilterKey",
                "filters.1.member.key",
                "CommandId, DocumentName, ExecutionStage, InvokedBefore, InstanceId, \
                 InvokedAfter, Status",
            ),
        ),
        (
            "an inventory filter type: the model declares every value, in another order",
            "GetInventory",
            json!({
                "Filters": [{
                    "Key": "AWS:InstanceInformation.PlatformType",
                    "Values": ["Linux"],
                    "Type": "Contains",
                }],
            }),
            enum_clause(
                "Contains",
                "filters.1.member.type",
                "NotEqual, BeginWith, Equal, LessThan, GreaterThan, Exists",
            ),
        ),
        (
            // `com.amazonaws.ssm#ParameterType`
            // (`smithy_query(query="shape",shape="ParameterType")`), a type no recorded
            // refusal of this service covers.
            "a parameter type: a member the recordings do not cover",
            "PutParameter",
            json!({ "Name": "/test/type", "Value": "v", "Type": "Secret" }),
            enum_clause(
                "Secret",
                "type",
                "SecureString, StringList, String",
            ),
        ),
        (
            // The 42 keys the service accepts, listed in the order it lists them: the 37
            // `OpsItemFilterKey` declares plus the Identity Center instance, the
            // organization and organizational unit, and the assignee's type and id.
            "an OpsItem filter key: the widest accepted set this service publishes",
            "DescribeOpsItems",
            json!({
                "OpsItemFilters": [{
                    "Key": "NotARealFilterKey",
                    "Values": ["x"],
                    "Operator": "Equal",
                }],
            }),
            enum_clause(
                "NotARealFilterKey",
                "opsItemFilters.1.member.key",
                "IdcInstanceArn, OpsItemId, ActualStartTime, ChangeRequestByApproverArn, \
                 AccessRequestByApproverId, AccessRequestBySourceOpsItemId, \
                 AccessRequestByRequesterArn, OperationalDataKey, PlannedEndTime, Priority, \
                 ChangeRequestByTargetsResourceGroup, ChangeRequestByApproverName, AccountId, \
                 AssigneeType, InsightByType, ResourceId, AssigneeId, \
                 AccessRequestBySourceAccountId, PlannedStartTime, AccessRequestByIsReplica, \
                 LastModifiedTime, OrganizationId, ChangeRequestByTemplate, \
                 ChangeRequestByRequesterName, AccessRequestBySourceRegion, AutomationId, \
                 Status, AccessRequestByRequesterId, OperationalDataValue, CreatedTime, \
                 AccessRequestByApproverArn, ChangeRequestByRequesterArn, Title, \
                 ActualEndTime, Source, AccessRequestByTargetResourceId, OperationalData, \
                 OrganizationalUnitId, Severity, Category, OpsItemType, CreatedBy",
            ),
        ),
    ];

    for (case, operation, body, clause) in cases {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: a value outside an enumerated type is refused, got {status} \
             {response}"
        );
        assert_eq!(
            error_code(&response),
            "ValidationException",
            "case {case:?}: the refusal is a ValidationException, got {response}"
        );
        let expected = one_violation(&clause);
        let stated = error_message(&response);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE 2: the operations whose clauses state no value state none for a value outside an
/// enumerated type, for an absent member and for a number outside its range — while every
/// other operation's clause quotes what the caller sent.
#[tokio::test]
async fn the_valueless_operations_state_no_value() {
    let router = router();

    /// `SessionState` as the model declares it, which is also the order it is listed in.
    const SESSION_STATE: &str = "Active, History";

    /// `InstancePropertyFilterOperator`, listed as this service lists it.
    const NODE_FILTER_OPERATOR: &str = "NotEqual, BeginWith, Equal, LessThan, GreaterThan";

    /// `StepExecutionFilterKey` (`smithy_query(query="shape",shape="StepExecutionFilterKey")`
    /// declares the nine values), listed as this service lists them — which is neither the
    /// model's declaration order nor lexicographic.
    const STEP_EXECUTION_FILTER_KEY: &str =
        "ParentStepIteration, StepExecutionId, ParentStepIteratorValue, StartTimeAfter, \
         StartTimeBefore, Action, StepName, ParentStepExecutionId, StepExecutionStatus";

    /// A well-formed `AutomationExecutionId` (the modelled length is exactly 36), so the
    /// rows below break ONE constraint each: the boundary refusal happens before anything
    /// looks for an execution of that id.
    const AN_EXECUTION_ID: &str = "99999999-8888-7777-6666-555555555555";

    /// One well-formed managed-node filter, for the cases that refuse a LATER element.
    fn node_filter(operator: &str) -> Value {
        json!({ "Key": "PlatformTypes", "Values": ["Windows"], "Operator": operator })
    }

    // (case, operation, body, the clause the refusal states)
    let cases: Vec<(&str, &str, Value, String)> = vec![
        (
            "a session state outside its enumerated type",
            "DescribeSessions",
            json!({ "State": "active" }),
            valueless_enum_clause("state", SESSION_STATE),
        ),
        (
            "another value outside the same type",
            "DescribeSessions",
            json!({ "State": "Closed" }),
            valueless_enum_clause("state", SESSION_STATE),
        ),
        (
            "the same member absent altogether",
            "DescribeSessions",
            json!({ "MaxResults": 10 }),
            "Value at 'state' failed to satisfy constraint: Member must not be null".to_string(),
        ),
        (
            "a page size over the same operation's maximum",
            "DescribeSessions",
            json!({ "State": "Active", "MaxResults": 201 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
             than or equal to 200"
                .to_string(),
        ),
        (
            "a managed-node filter operator in the first element",
            "DescribeInstanceProperties",
            json!({ "FiltersWithOperator": [node_filter("Contains")] }),
            valueless_enum_clause(
                "filtersWithOperator.1.member.operator",
                NODE_FILTER_OPERATOR,
            ),
        ),
        (
            "the same member in the third element",
            "DescribeInstanceProperties",
            json!({
                "FiltersWithOperator": [
                    node_filter("Equal"),
                    node_filter("NotEqual"),
                    node_filter("StartsWith"),
                ],
            }),
            valueless_enum_clause(
                "filtersWithOperator.3.member.operator",
                NODE_FILTER_OPERATOR,
            ),
        ),
        (
            "a page size over the managed-node describe's maximum",
            "DescribeInstanceProperties",
            json!({ "MaxResults": 1001 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
             than or equal to 1000"
                .to_string(),
        ),
        (
            "an empty filter value list on the node describe",
            "DescribeInstanceInformation",
            json!({ "Filters": [{ "Key": "PlatformTypes", "Values": [] }] }),
            "Value at 'filters.1.member.values' failed to satisfy constraint: Member must \
             have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "the same list empty in the second element",
            "DescribeInstanceInformation",
            json!({
                "Filters": [
                    { "Key": "PlatformTypes", "Values": ["Windows"] },
                    { "Key": "PingStatus", "Values": [] },
                ],
            }),
            "Value at 'filters.2.member.values' failed to satisfy constraint: Member must \
             have length greater than or equal to 1"
                .to_string(),
        ),
        (
            "a page size under the node describe's minimum",
            "DescribeInstanceInformation",
            json!({ "MaxResults": 4 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value \
             greater than or equal to 5"
                .to_string(),
        ),
        (
            "a page size over an operation that quotes its values",
            "GetInventory",
            json!({ "MaxResults": 51 }),
            "Value '51' at 'maxResults' failed to satisfy constraint: Member must have value \
             less than or equal to 50"
                .to_string(),
        ),
        (
            "an inventory filter type, which quotes the same rejected value",
            "GetInventory",
            json!({
                "Filters": [{
                    "Key": "AWS:InstanceInformation.PlatformType",
                    "Values": ["Linux"],
                    "Type": "Contains",
                }],
            }),
            enum_clause(
                "Contains",
                "filters.1.member.type",
                "NotEqual, BeginWith, Equal, LessThan, GreaterThan, Exists",
            ),
        ),
        (
            "a command filter key, which quotes the key it refused",
            "ListCommandInvocations",
            json!({ "Filters": [{ "key": "NotAKey", "value": "x" }] }),
            enum_clause(
                "NotAKey",
                "filters.1.member.key",
                "CommandId, DocumentName, ExecutionStage, InvokedBefore, InstanceId, \
                 InvokedAfter, Status",
            ),
        ),
        (
            "a step-execution filter key in the first element",
            "DescribeAutomationStepExecutions",
            json!({
                "AutomationExecutionId": AN_EXECUTION_ID,
                "Filters": [{ "Key": "NotARealFilterKey", "Values": ["x"] }],
            }),
            valueless_enum_clause("filters.1.member.key", STEP_EXECUTION_FILTER_KEY),
        ),
        (
            "the same member in the second element",
            "DescribeAutomationStepExecutions",
            json!({
                "AutomationExecutionId": AN_EXECUTION_ID,
                "Filters": [
                    { "Key": "StepName", "Values": ["stand"] },
                    { "Key": "StepExecutionStatuses", "Values": ["Cancelled"] },
                ],
            }),
            valueless_enum_clause("filters.2.member.key", STEP_EXECUTION_FILTER_KEY),
        ),
        (
            "a page size over the step report's maximum",
            "DescribeAutomationStepExecutions",
            json!({ "AutomationExecutionId": AN_EXECUTION_ID, "MaxResults": 51 }),
            "Value at 'maxResults' failed to satisfy constraint: Member must have value less \
             than or equal to 50"
                .to_string(),
        ),
    ];

    for (case, operation, body, clause) in cases {
        let (status, response) = call(&router, operation, &body).await;
        assert_eq!(
            status, 400,
            "case {case:?}: the request is refused, got {status} {response}"
        );
        let expected = one_violation(&clause);
        let stated = error_message(&response);
        assert_eq!(
            stated, expected,
            "case {case:?}: expected {expected:?}, got {stated:?}"
        );
    }
}

/// RULE 3: a clause stated at a collection member quotes the collection the caller
/// submitted — every element of it, rendered as a list — rather than a count of its
/// elements.
///
/// The rows differ along the two operations whose `Names` list is bounded, how far past the
/// bound the list is, and what the names look like; the row at the bound itself is served
/// rather than refused, so the clause is about the bound and not about the member.
#[tokio::test]
async fn a_collection_over_its_bound_quotes_the_collection() {
    let router = router();

    /// The published maximum of `ParameterNameList` as `GetParameters$Names` and
    /// `DeleteParameters$Names` bind it (`smithy_query(query="operation",
    /// operation="GetParameters,DeleteParameters")`: `length 1..10`).
    const MAX_NAMES: usize = 10;

    /// `count` parameter names in the shape `name` spells, hierarchical or flat.
    fn names(shape: &str, count: usize) -> Vec<String> {
        (1..=count).map(|n| shape.replace('#', &n.to_string())).collect()
    }

    // (case, the names the request carries)
    let over_the_bound: Vec<(&str, Vec<String>)> = vec![
        ("one name past the bound", names("/db/param-#", MAX_NAMES + 1)),
        ("flat names", names("param#", MAX_NAMES + 2)),
        (
            "names with the characters a name may carry",
            names("/team.a_b-c/#", MAX_NAMES + 5),
        ),
        ("far past the bound", names("/deep/tree/of/names/#", MAX_NAMES * 3)),
    ];

    for operation in ["GetParameters", "DeleteParameters"] {
        for (case, asked) in &over_the_bound {
            let (status, response) =
                call(&router, operation, &json!({ "Names": asked })).await;
            assert_eq!(
                status, 400,
                "case {operation} {case:?}: a list past its bound is refused, got {status} \
                 {response}"
            );
            let expected = one_violation(&format!(
                "Value '[{}]' at 'names' failed to satisfy constraint: Member must have \
                 length less than or equal to {MAX_NAMES}",
                asked.join(", ")
            ));
            let stated = error_message(&response);
            assert_eq!(
                stated, expected,
                "case {operation} {case:?}: expected {expected:?}, got {stated:?}"
            );
            assert!(
                !stated.contains(" items"),
                "case {operation} {case:?}: a clause states no count of elements, got \
                 {stated:?}"
            );
        }

        // The bound itself is served: the clause above is about the length and not about
        // the member.
        let at_the_bound = names("/db/param-#", MAX_NAMES);
        let (status, response) =
            call(&router, operation, &json!({ "Names": at_the_bound })).await;
        assert_eq!(
            status, 200,
            "case {operation} at the bound: a list of {MAX_NAMES} names is served, got \
             {status} {response}"
        );
    }
}

/// RULE 4: a managed-node filter value outside the set its key publishes is refused naming
/// the KEY and the values it accepts, in the order the service publishes that key's values,
/// on both node describes and whichever operator the filter named.
#[tokio::test]
async fn a_filter_value_outside_its_published_set_names_the_key_and_the_set() {
    let router = router();

    // (the filter key, the values it publishes, in the order the service lists them: the
    // modelled enum's own declaration for `AssociationStatus`
    // (`smithy_query(query="shape", shape="AssociationStatusName")`) and the
    // `InstanceInformationStringFilter$Key` doc's published list for the others
    // (`smithy_query(query="doc", shape="InstanceInformationStringFilter", member="Key")`))
    let published: &[(&str, &str)] = &[
        ("AssociationStatus", "Pending, Success, Failed"),
        ("PingStatus", "Online, ConnectionLost, Inactive"),
        ("PlatformTypes", "Windows, Linux, MacOS"),
        ("ResourceType", "EC2Instance, ManagedInstance"),
        (
            "SourceTypes",
            "AWS::EC2::Instance, AWS::SSM::ManagedInstance, AWS::IoT::Thing, \
             Microsoft.Compute/virtualMachines, compute.googleapis.com/Instance",
        ),
    ];

    // (the operation, the member its string filters travel in, the modelled error it
    // refuses a filter value with)
    let describes: &[(&str, &str, &str)] = &[
        (
            "DescribeInstanceProperties",
            "FiltersWithOperator",
            "InvalidInstancePropertyFilterValue",
        ),
        (
            "DescribeInstanceInformation",
            "Filters",
            "InvalidInstanceInformationFilterValue",
        ),
    ];

    for (key, set) in published {
        let first = set.split(", ").next().expect("a published set is not empty");
        // (case, the values the filter carries)
        let refused: &[(&str, &str)] = &[
            ("a value no node can hold", "NotAPublishedValue"),
            ("a published value in another case", &first.to_lowercase()),
            ("a prefix of a published value", &first[..1]),
        ];
        for (operation, member, error) in describes {
            for (case, value) in refused {
                // `DescribeInstanceProperties` carries an operator per filter; the sibling
                // describe's filters carry none, so the loop covers every operator the one
                // that has them accepts.
                let operators: &[Option<&str>] = if *member == "FiltersWithOperator" {
                    &[
                        Some("Equal"),
                        Some("NotEqual"),
                        Some("BeginWith"),
                        Some("LessThan"),
                        Some("GreaterThan"),
                    ]
                } else {
                    &[None]
                };
                for operator in operators {
                    let mut filter = json!({ "Key": key, "Values": [value] });
                    if let Some(operator) = operator {
                        filter["Operator"] = json!(operator);
                    }
                    let body = json!({ (*member): [filter] });
                    let (status, response) = call(&router, operation, &body).await;
                    assert_eq!(
                        status, 400,
                        "case {operation} {key} {case:?} {operator:?}: the filter is refused, \
                         got {status} {response}"
                    );
                    assert_eq!(
                        error_code(&response),
                        *error,
                        "case {operation} {key} {case:?} {operator:?}: the refusal is the \
                         modelled one, got {response}"
                    );
                    let expected = format!("{key} filter is invalid, valid values are {set}");
                    let stated = error_message(&response);
                    assert_eq!(
                        stated, expected,
                        "case {operation} {key} {case:?} {operator:?}: expected {expected:?}, \
                         got {stated:?}"
                    );
                }
            }
        }
    }
}

/// The published sets rule 4 lists are the SETS the model's own enums declare: the listing
/// order is the order the service publishes each key's values in, the membership is the
/// generated value constant, so a model revision cannot leave the published listing behind.
#[test]
fn the_published_filter_value_sets_are_the_models_enums() {
    // (the filter key, the modelled enum the property it filters targets)
    let modelled: &[(&str, &[&str])] = &[
        ("AssociationStatus", ASSOCIATION_STATUS_NAME_VALUES),
        ("PingStatus", PING_STATUS_VALUES),
        ("PlatformTypes", PLATFORM_TYPE_VALUES),
        ("ResourceType", RESOURCE_TYPE_VALUES),
        ("SourceTypes", SOURCE_TYPE_VALUES),
    ];
    for (key, declared) in modelled {
        let published = node_filter_key_value_set(key)
            .unwrap_or_else(|| panic!("case {key}: the key publishes a value set"));
        let mut sorted: Vec<&str> = published.to_vec();
        sorted.sort_unstable();
        assert_eq!(
            sorted, *declared,
            "case {key}: the published set is the modelled enum, got {published:?}"
        );
    }
    assert!(
        node_filter_key_value_set("IamRole").is_none(),
        "case IamRole: a key whose values are free text publishes no set"
    );
}
