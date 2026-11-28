Issue-Severity: MEDIUM
Issue-Gap-Class: wrong-behavior
Issue-Commit: e2b1e957d16e5804a66029c01544c489561bd0fd

# ListConfigurationCheckOperations refuses three of the filter names its own catalogue rule admits

_Filed: 2026-11-28T08:10:42Z_
_Operation: ListConfigurationCheckOperations | Class: wrong-behavior_

> _Note: this issue was identified against the code base at commit `e2b1e957d16e5804a66029c01544c489561bd0fd`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListConfigurationCheckOperations` validates its `Filters` member against `CONFIGURATION_CHECK_FILTER_NAMES`, and the crate states the RULE that constant is built from in two places: "the string-valued members the model gives `ConfigurationCheckOperation`, which is the shape that listing returns". The model gives `ConfigurationCheckOperation` SEVEN string-valued members (`Id`, `ApplicationId`, `Status`, `StatusMessage`, `ConfigurationCheckId`, `ConfigurationCheckName`, `ConfigurationCheckDescription` — the last three target `String`, the first two `String` and `Status`/`ConfigurationCheckId` string enums). The constant, and the `filter_attribute` mapping under the same doc sentence, hold only FOUR of them.

The consequence is observable: a request to `/list-configuration-check-operations` carrying `Filters: [{"Name": "ConfigurationCheckName", "Value": "...", "Operator": "Equals"}]` is refused with `ValidationException` (HTTP 400) `"Filter Name must be one of Id, ApplicationId, Status, ConfigurationCheckId, but was ConfigurationCheckName"`, even though the crate's own stated rule for that catalogue admits it — and the listing does report that member in every row it returns (`StoredConfigurationCheckOperation::view` sets `configuration_check_name` / `configuration_check_description` / `status_message`). The same goes for `StatusMessage` and `ConfigurationCheckDescription`.

The sibling catalogues show the crate knows how to state a narrower rule when it means one: `OPERATION_FILTER_NAMES` is documented as "the identifying and status members the model gives `Operation`", deliberately excluding `Operation$StatusMessage`, and its six entries match that wording exactly. The configuration-check catalogue instead claims the whole string-valued set and delivers a subset, so the doc and the code under it disagree about which filters the listing offers.

Either the catalogue is missing the three members, or the two doc sentences overstate the rule they are built from; as the crate stands the two halves contradict each other.

## Evidence

The stated rule and the constant, `services/service-com-amazonaws-ssmsap-ssmsap/src/state.rs:4003-4008`:
```rust
/// The `Name` values a `ListConfigurationCheckOperations` filter may carry: the
/// string-valued members the model gives `ConfigurationCheckOperation`, which is the
/// shape that listing returns. Filter names are case-sensitive, as `Filter$Name`'s
/// documentation states. See [`StoredConfigurationCheckOperation::filter_attribute`].
pub const CONFIGURATION_CHECK_FILTER_NAMES: &[&str] =
    &["Id", "ApplicationId", "Status", "ConfigurationCheckId"];
```

The same claim over the mapping, `services/service-com-amazonaws-ssmsap-ssmsap/src/state/configuration_checks.rs:975-991`:
```rust
    /// The value a `ListConfigurationCheckOperations` filter whose `Name` is `name`
    /// compares against, or `None` when this record has no such attribute.
    ///
    /// The names are the string-valued members the model gives
    /// `ConfigurationCheckOperation` — `Id`, `ApplicationId`, `Status`,
    /// `ConfigurationCheckId` — which is the shape that listing returns, so a filter
    /// names something the response itself reports. See
    /// [`CONFIGURATION_CHECK_FILTER_NAMES`].
    pub fn filter_attribute(&self, name: &str) -> Option<&str> {
        match name {
            "Id" => Some(self.id.as_str()),
            "ApplicationId" => Some(self.application_id.as_str()),
            "Status" => Some(self.status.as_str()),
            "ConfigurationCheckId" => Some(self.check_id.as_str()),
            _ => None,
        }
    }
```

What the model actually gives that shape (`smithy_query(query="shape", shape="ConfigurationCheckOperation")`) — three further string-valued members the catalogue omits:
```json
"StatusMessage": {"target": "String", "doc": "<p>A message providing additional details about the status of the configuration check operation.</p>"},
"ConfigurationCheckName": {"target": "String", "doc": "<p>The name of the configuration check that was performed.</p>"},
"ConfigurationCheckDescription": {"target": "String", "doc": "<p>A description of the configuration check that was performed.</p>"}
```
and the listing does report them — `src/state/configuration_checks.rs:950-954`:
```rust
            status_message: self.status_message.clone(),
            configuration_check_id: Some(self.check_id.clone()),
            configuration_check_name: catalogued.map(|entry| entry.name.to_string()),
            configuration_check_description: catalogued
                .map(|entry| entry.description.to_string()),
```

The handler that turns the mismatch into a 400, `src/ops/list_configuration_check_operations.rs:192-197`:
```rust
        crate::state::validate_filters(
            &filters,
            crate::state::FilterOperators::Modelled,
            crate::state::CONFIGURATION_CHECK_FILTER_NAMES,
        )
        .map_err(ListConfigurationCheckOperationsErrorModeled::ValidationException)?;
```
refusing through `src/state.rs:4162` -> `value_within_catalogue` (`src/state.rs:3356-3360`):
```rust
        value_within_catalogue("Filter Name", allowed_names, &filter.name)?;
...
            "{member} must be one of {}, but was {value}",
```

The sibling catalogue whose doc and contents DO agree, `src/state.rs:4010-4021`:
```rust
/// The `Name` values a `ListOperations` filter may carry: the identifying and status
/// members the model gives `Operation`, which is the shape that listing returns.
pub const OPERATION_FILTER_NAMES: &[&str] = &[
    "Id",
    "Type",
    "Status",
    "ResourceType",
    "ResourceId",
    "ResourceArn",
];
```

Test coverage: `tests/all/filter_validation.rs:59-62` reads each listing's names off the crate constant rather than hard-coding them —
```rust
    /// The `Name`s it filters by, read off this crate's own catalogue for it rather than
    /// written out here, so a name added to either catalogue is exercised instead of
    /// going untested.
    pub(crate) names: &'static [&'static str],
```
and `tests/all/filter_validation.rs:184` binds `names: CONFIGURATION_CHECK_FILTER_NAMES`, with `refused_names()` (`:122-141`) drawing refusals only from the OTHER listings' catalogues. No test hard-codes the four names, and no other catalogue carries `StatusMessage`, `ConfigurationCheckName` or `ConfigurationCheckDescription`, so reconciling the catalogue with the stated rule leaves the whole suite green.

## Steps to Reproduce

1. POST /register-application signed for us-east-1 with a valid HANA registration (ApplicationId "HDB-APP", Sid, SapInstanceNumber and one credential); wait one TRANSITION_SPAN so it settles ACTIVATED / SUCCESS.
2. POST /start-configuration-checks {"ApplicationId":"HDB-APP","ConfigurationCheckIds":["SAP_CHECK_01"]} — 200.
3. POST /list-configuration-check-operations {"ApplicationId":"HDB-APP"} — 200; each reported ConfigurationCheckOperation carries ConfigurationCheckName "SAP EC2 Instance Type Selection" and a ConfigurationCheckDescription.
4. POST /list-configuration-check-operations {"ApplicationId":"HDB-APP","Filters":[{"Name":"ConfigurationCheckName","Value":"SAP EC2 Instance Type Selection","Operator":"Equals"}]} — actual: 400 ValidationException "Filter Name must be one of Id, ApplicationId, Status, ConfigurationCheckId, but was ConfigurationCheckName". Expected per src/state.rs:4003-4006 and src/state/configuration_checks.rs:978-980 ("the string-valued members the model gives ConfigurationCheckOperation"): the filter is applied, since ConfigurationCheckName is a string-valued member of that shape and is reported in the response.
5. Repeat with {"Name":"StatusMessage"} and {"Name":"ConfigurationCheckDescription"} — both refused the same way, although both are string-valued members of the returned shape.
6. Contrast POST /list-operations {"ApplicationId":"HDB-APP","Filters":[{"Name":"ResourceArn",...}]} — accepted, because OPERATION_FILTER_NAMES (src/state.rs:4014-4021) matches its own narrower doc sentence exactly.
