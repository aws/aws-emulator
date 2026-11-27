Issue-Severity: MEDIUM
Issue-Gap-Class: pagination
Issue-Commit: 11892c8a0fd001958f088427b8ee0a7691e93494

# ListOrganizationEnrollmentConfigurations defaults maxResults to 100 where the model documents 1,000

_Filed: 2026-11-27T22:53:59Z_
_Operation: ListOrganizationEnrollmentConfigurations | Class: pagination_

> _Note: this issue was identified against the code base at commit `11892c8a0fd001958f088427b8ee0a7691e93494`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`ListOrganizationEnrollmentConfigurationsRequest$maxResults` is documented with its own default: "If you don't specify a value, the default is 1,000." The handler sizes its page through the crate-wide `crate::state::page_size`, whose default is the single constant `DEFAULT_MAX_RESULTS = 100`. The configuration store itself holds up to `MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS = 1_000` targets, so a caller that sends no `maxResults` can hold 1,000 stored configurations and still be served only 100 of them plus a `nextToken`, instead of the 1,000-item page the member documents.

The divergence is not justified anywhere in the crate: `DEFAULT_MAX_RESULTS`'s doc comment reasons only from the `@range` ceilings of `ListAutomationRules`, `ListAutomationRulePreview` and `ListAccounts` and never mentions this member's documented default, the handler comment states only the ceiling (`1..=1000`), and the suite that pins the shared page-size rule claims to cover every paginated listing yet omits this operation. The operation's own test exercises `maxResults` at the range bounds (asserting HTTP status only) and never the defaulted page.

## Evidence

Smithy model (`smithy_query(query="shape",shape="ListOrganizationEnrollmentConfigurationsRequest")`), member `maxResults`:
  "range": {"min": 1, "max": 1000}, doc: "<p> The maximum number of organization enrollment configurations to return in a single response. If you don't specify a value, the default is 1,000. </p>"

Handler takes the shared default instead — services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/ops/list_organization_enrollment_configurations.rs:203-208:
    // The model bounds this operation's `maxResults` at 1..=1000, so a
    // caller may ask for a page larger than the default this service serves
    // a request that names none. One rule for every paginated listing here,
    // including the floor against a non-advancing zero page:
    // `crate::state::page_size`.
    let page_size = crate::state::page_size(input.max_results);

The one default, 100 — src/state.rs:135: `pub const DEFAULT_MAX_RESULTS: i32 = 100;`, whose doc (src/state.rs:125-134) justifies the number only from other operations: "Most of this service's `maxResults` members are bounded at `range(min: 1, max: 100)` (`ListAutomationRules`, `ListAutomationRulePreview`, …) … `ListAccounts` is bounded at `range(min: 1, max: 1000)` … so there a caller may ask for a larger page than the default." — this member's documented default is never addressed.

src/state.rs:4393-4397: `pub fn page_size(max_results: Option<i32>) -> usize { usize::try_from(max_results.unwrap_or(DEFAULT_MAX_RESULTS)).unwrap_or(1).max(1) }`

The store can hold ten default pages — src/state/organization_enrollment.rs:118: `pub const MAX_ORGANIZATION_ENROLLMENT_CONFIGURATIONS: usize = 1_000;`

Test coverage: tests/all/page_size_rule.rs:14 claims "Every paginated operation of this service is a row below", but `listings()` (tests/all/page_size_rule.rs:150-207) holds nine rows and ListOrganizationEnrollmentConfigurations is not one of them. The operation's own suite only checks the range bounds' HTTP status — tests/all/organization_enrollment_configurations.rs:746-763: `for (case, max_results, expected) in [("at-the-range-minimum", 1, 200), ("at-the-range-maximum", 1000, 200), ("below-the-range-minimum", 0, 400), ("above-the-range-maximum", 1001, 400)]` — so no test pins the page a request naming no `maxResults` is served.

## Steps to Reproduce

1. Start the emulator and opt the caller in with UpdateEnrollmentConfiguration (status=Active).
2. Call PutOrganizationEnrollmentConfiguration more than 100 times, each naming a distinct member-account or OU target with organizationRuleMode=AnyAllowed (e.g. 150 targets).
3. POST / with X-Amz-Target: ComputeOptimizerAutomationService.ListOrganizationEnrollmentConfigurations and body {} (no maxResults).
4. Count the entries in `organizationEnrollmentConfigurations`: the response carries 100 items and a `nextToken`.
5. Expected per the member's documentation ("If you don't specify a value, the default is 1,000"): all 150 stored configurations on that first page and no `nextToken`.
