Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: d6ab12dd13fc1c951c3823d9af85ad5bf611db1c

# ListOperationEvents refuses the two ordering FilterOperator values its siblings accept, on a cited model doc that says nothing of the kind

_Filed: 2026-11-28T04:46:59Z_
_Operation: ListOperationEvents | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `d6ab12dd13fc1c951c3823d9af85ad5bf611db1c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListOperationEvents` is the only filtered listing of this crate that narrows the accepted `Filter$Operator` set to `Equals`: it passes `FilterOperators::Only(FILTER_OPERATOR_EQUALS)` to the shared `validate_filters`, while `ListApplications`, `ListOperations` and `ListConfigurationCheckOperations` all pass `FilterOperators::Modelled` (the model's whole `FilterOperator` catalogue).

The ground the crate gives for that divergence is a citation to the model: `OPERATION_EVENT_FILTER_NAMES`'s doc comment asserts that the names AND the single operator are "both as that operation documents its `Filters` member", and names the exact `smithy_query` that is supposed to show it. Running that query returns only "The filters of an operation." — the member documents neither a name set nor an operator restriction. `Filter$Operator` is modelled `@required` against the `FilterOperator` enum whose values are `Equals | GreaterThanOrEquals | LessThanOrEquals`, documented only as "The operator for the filter."

So a well-formed `ListOperationEvents` request carrying `Operator: "GreaterThanOrEquals"` — a value the model's enum declares, and a comparison this crate already implements in the one place every listing shares (`filter_matches_attribute`) — is refused with `ValidationException` 400, while the identical filter on `ListOperations` is served. The refusal is decided by a rule whose only stated justification does not exist in the model.

## Evidence

The crate's stated ground, services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:4023-4029:
```
/// The `Name` values a `ListOperationEvents` filter may carry, and the only operator it
/// accepts for them is [`FILTER_OPERATOR_EQUALS`] — both as that operation documents its
/// `Filters` member (`smithy_query(query="doc", shape="ListOperationEventsInput",
/// member="Filters")`). Filter names are case-sensitive, as `Filter$Name`'s documentation
/// states, so they are spelled here exactly as that member spells them. See
/// [`OperationEventRecord::filter_attribute`].
pub const OPERATION_EVENT_FILTER_NAMES: &[&str] = &["status", "resourceID", "resourceType"];
```

What that very query actually returns from the model:
`{"shape": "ListOperationEventsInput", "member": "Filters", "documentation": "<p>The filters of an operation.</p>"}`
and the operation's own doc carries no operator rule either: "Returns a list of operations events.</p> <p>Available parameters include <code>OperationID</code>, as well as optional parameters <code>MaxResults</code>, <code>NextToken</code>, and <code>Filters</code>."

The model's operator contract — `Filter$Operator`: `{"target": "FilterOperator", "required": true, "type": "enum", "doc": "<p>The operator for the filter. </p>"}`, `FilterOperator`: `{"type": "enum", "values": ["Equals", "GreaterThanOrEquals", "LessThanOrEquals"]}`.

The handler acting on it, services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_operation_events.rs:181-189:
```
        // This operation documents ONE operator for every filter it takes, so any other
        // is refused rather than silently applied as a comparison this listing does not
        // offer.
        crate::state::validate_filters(
            &filters,
            crate::state::FilterOperators::Only(crate::state::FILTER_OPERATOR_EQUALS),
            crate::state::OPERATION_EVENT_FILTER_NAMES,
        )
        .map_err(ListOperationEventsErrorModeled::ValidationException)?;
```

The sibling listing, services/service-com-amazonaws-ssmsap-ssmsap/src/ops/list_operations.rs:171-176:
```
        crate::state::validate_filters(
            &filters,
            crate::state::FilterOperators::Modelled,
            crate::state::OPERATION_FILTER_NAMES,
        )
        .map_err(ListOperationsErrorModeled::ValidationException)?;
```
(`ListApplications` src/ops/list_applications.rs:161-165 and `ListConfigurationCheckOperations` src/ops/list_configuration_check_operations.rs:192-197 do the same.)

The refusal itself, src/state.rs:4130-4135:
```
            Self::Only(only) => {
                if operator != *only {
                    return Err(validation(format!(
                        "Filter Operator must be {only}, but was {operator}"
                    )));
```
while the comparison the refusal denies is already implemented for every listing, src/state.rs:4204-4207:
```
        .any(|wanted| match filter.operator.as_str() {
            FILTER_OPERATOR_EQUALS => actual == wanted,
            FILTER_OPERATOR_GREATER_THAN_OR_EQUALS => actual >= wanted,
            FILTER_OPERATOR_LESS_THAN_OR_EQUALS => actual <= wanted,
```

Test coverage of the divergence: services/service-com-amazonaws-ssmsap-ssmsap/tests/all/filter_validation.rs:188-200 tabulates `ListOperationEvents` with `operators: FilterOperators::Only(FILTER_OPERATOR_EQUALS)` and `ListOperations` with `operators: FilterOperators::Modelled`; the table states no reason of its own, so the only stated ground is the state.rs citation above.

## Steps to Reproduce

1. RegisterApplication an application and keep the `OperationId` it answers with.
2. POST /list-operations with `{"ApplicationId": "<id>", "Filters": [{"Name": "Status", "Value": "SUCCESS", "Operator": "GreaterThanOrEquals"}]}` — served 200.
3. POST /list-operation-events with `{"OperationId": "<operation id>", "Filters": [{"Name": "status", "Value": "COMPLETED", "Operator": "GreaterThanOrEquals"}]}` — refused 400 `ValidationException` "Filter Operator must be Equals, but was GreaterThanOrEquals", although `FilterOperator` declares that value and `filter_matches_attribute` implements it.
4. Run `smithy_query(query="doc", shape="ListOperationEventsInput", member="Filters")` — the sentence the refusal is justified by ("documents ONE operator for every filter it takes") is absent: the member's whole documentation is "The filters of an operation."
