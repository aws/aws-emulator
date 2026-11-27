Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 1a8d6da9d066bb6b81849373d117fc388454286a

# ListOrganizationalUnits and ListOrganizationEnrollmentConfigurations serve a 100-item default page, although both maxResults members document a default of 1,000

_Filed: 2026-11-27T21:48:02Z_
_Operation: ListOrganizationalUnits | Class: pagination_

> _Note: this issue was identified against the code base at commit `1a8d6da9d066bb6b81849373d117fc388454286a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Both of these listings take their page size from the service-wide `crate::state::page_size`, which defaults an absent `maxResults` to `DEFAULT_MAX_RESULTS = 100`. The model documents a different default for exactly these two operations: each one's `maxResults` is bounded `range(min: 1, max: 1000)` AND documents "If you don't specify a value, the default is 1,000." (every other paginated operation of this service documents 100, which is why the shared constant is right for them).

So a caller that names no `maxResults` is served 100 nodes/configurations plus a `nextToken` where the model says it is served up to 1,000 — the page contents and the presence of the continuation token both differ from what the member documents.

Nothing in the crate justifies the divergence for these two operations: the constant's own doc comment discusses only the `maxResults` CEILING, and names `ListAccounts` — whose member documents no default at all — as the single max-1000 case, while the two handlers' comments repeat that ceiling remark and never mention the documented default of 1,000. Nor does any test pin it: the suite that claims to cover "every paginated operation of this service" omits both of these listings from its row set.

## Evidence

Model, `smithy_query(query="shape",shape="ListOrganizationalUnitsRequest")`, member `maxResults`: `"range": {"min": 1, "max": 1000}`, doc "The maximum number of organizational units to return in a single response. If you don't specify a value, the default is 1,000."
Model, `smithy_query(query="shape",shape="ListOrganizationEnrollmentConfigurationsRequest")`, member `maxResults`: `"range": {"min": 1, "max": 1000}`, doc "The maximum number of organization enrollment configurations to return in a single response. If you don't specify a value, the default is 1,000."
(For contrast, `ListAccountsRequest$maxResults` is max 1000 but documents NO default, and every other listing documents "the default is 100".)

The code that serves 100 instead — `src/state.rs:125-135`:
```
/// The page size a paginated list uses when the caller sends no `maxResults`.
///
/// Most of this service's `maxResults` members are bounded at
/// `range(min: 1, max: 100)` (`ListAutomationRules`,
/// `ListAutomationRulePreview`, …), so for those the largest page a caller may
/// ask for is also the default; `ListAccounts` is bounded at
/// `range(min: 1, max: 1000)` … so there a
/// caller may ask for a larger page than the default. …
pub const DEFAULT_MAX_RESULTS: i32 = 100;
```
`src/state.rs:4393-4397`: `pub fn page_size(max_results: Option<i32>) -> usize { usize::try_from(max_results.unwrap_or(DEFAULT_MAX_RESULTS)) … }`
`src/ops/list_organizational_units.rs:213-218`: "The model bounds this operation's `maxResults` at 1..=1000, so a caller may ask for a page larger than the default this service serves a request that names none." followed by `let page_size = crate::state::page_size(input.max_results);`
`src/ops/list_organization_enrollment_configurations.rs:203-208`: the identical comment and `let page_size = crate::state::page_size(input.max_results);`

No test pins the default for either listing, although the suite claims completeness — `tests/all/page_size_rule.rs:14-15`: "Every paginated operation of this service is a row below, and each row is driven the way a client drives it" — while `listings()` (`tests/all/page_size_rule.rs:150-207`) enumerates only ListAccounts, ListAutomationRules, ListRecommendedActions, ListRecommendedActionSummaries, ListAutomationRulePreview, ListAutomationRulePreviewSummaries, ListAutomationEvents, ListAutomationEventSummaries and ListAutomationEventSteps — neither ListOrganizationalUnits nor ListOrganizationEnrollmentConfigurations.

## Steps to Reproduce

1. Mount this service beside the Organizations control plane and create an organization whose root holds more than 100 organizational units (so the hierarchy ListOrganizationalUnits reads has more than one default page of nodes).
2. UpdateEnrollmentConfiguration {"status":"Active"} to open the enrollment gate.
3. ListOrganizationalUnits {} (no maxResults) — documented: "If you don't specify a value, the default is 1,000", so up to 1,000 nodes come back. Actual: organizationalUnits holds 100 entries and a nextToken is minted.
4. PutOrganizationEnrollmentConfiguration for more than 100 distinct targets, then ListOrganizationEnrollmentConfigurations {} (no maxResults) — documented default 1,000; actual: 100 configurations plus a nextToken.
