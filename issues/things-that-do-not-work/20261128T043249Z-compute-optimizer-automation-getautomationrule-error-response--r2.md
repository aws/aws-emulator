Issue-Severity: MEDIUM
Issue-Gap-Class: error-response
Issue-Commit: 14a20d4988acc078b454ec1aa206a20bf5810707

# The authorization gate's ARN check skips the rule-id character class, so a malformed rule id is answered 400 instead of 403

_Filed: 2026-11-28T04:32:49Z_
_Operation: GetAutomationRule | Class: error-response_

> _Note: this issue was identified against the code base at commit `14a20d4988acc078b454ec1aa206a20bf5810707`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`crate::state::names_automation_rule` is the gate `src/service.rs` uses to decide whether a caller-supplied resource string names an automation rule of this service; a string that does not is answered `AccessDeniedException` (403) BEFORE the modelled walk runs, precisely so a permissions failure is not reported as a bad parameter.

The function hand-rolls the model's `RuleArn` pattern `^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`, and its own doc says it checks that form ("a fixed prefix plus two character classes"). But it only checks that the rule-id segment is NON-EMPTY — the `[a-zA-Z0-9_-]+` character class is never applied. So a resource string such as `arn:aws:compute-optimizer::123456789012:automation-rule/bad id` (or `.../a:b`, or anything else with a character outside the class) passes the gate, falls through to the framework's pre-handler `@pattern` check, and is answered 400 `InvalidParameterValueException` — exactly the classification `src/service.rs` documents the gate as existing to prevent, and exactly the outcome `tests/all/resource_arn_refusal_order.rs` states can never happen.

It affects all six operations the gate covers (`GetAutomationRule`, `UpdateAutomationRule`, `DeleteAutomationRule`, `TagResource`, `UntagResource`, `ListTagsForResource`); `GetAutomationRule` is the first entry in the gate's own table.

## Evidence

The helper's doc states the rule the code under it breaks —
services/service-com-amazonaws-computeoptimizerautomation-computeoptimizerautomationservice/src/state.rs:341
```
/// Does `candidate` name an automation rule of this service at all?
///
/// The form is the model's `RuleArn`
/// (`^arn:aws:compute-optimizer::[0-9]{12}:automation-rule/[a-zA-Z0-9_-]+$`,
/// from `smithy_query(query="shape",shape="RuleArn")`): the service's own ARN
/// namespace, an EMPTY region segment, a 12-digit account and the
/// `automation-rule/<ruleId>` resource. A string that does not have it names no
/// resource of this service, whether or not a rule with that ARN exists.
///
/// Checked by hand rather than with a regex engine: the pattern is a fixed
/// prefix plus two character classes, and this runs on the dispatch path.
```
…but the code checks only the first class (the account digits) and never the rule-id class —
services/.../src/state.rs:363
```
    if account_id.len() != 12 || !account_id.bytes().all(|b| b.is_ascii_digit()) {
        return false;
    }
    let Some(rule_id) = resource.strip_prefix("automation-rule/") else {
        return false;
    };
    !rule_id.is_empty()
```

The gate that consumes it, and the classification it says it prevents —
services/.../src/service.rs:96
```
/// Authorization is evaluated against the resource string a request supplies
/// BEFORE the service parses that string, so a string that identifies no
/// resource of this service is refused as unauthorized rather than as a bad
/// parameter: no permission a caller can hold names a resource the request does
/// not name. The framework's pre-handler validation would otherwise reject the
/// `RuleArn` `@pattern` with a 400, which classifies a permissions failure as a
/// request bug.
```
services/.../src/service.rs:207
```
    ::service_common::addressed_resource::unaddressed_resource(
        protocol,
        req,
        &RESOURCE_MEMBERS,
        crate::state::names_automation_rule,
    )
```

The suite that states the rule being broken, and whose table has no row for a rule id outside the class —
services/.../tests/all/resource_arn_refusal_order.rs:12
```
//! A
//! string that names no automation rule of this service is therefore never
//! answered as a bad parameter — not even when another member of the same
//! request really does break a modelled constraint
```
services/.../tests/all/resource_arn_refusal_order.rs:59 lists "one row per way the modelled `RuleArn` form ... can fail" as exactly five forms — `no-arn-at-all`, `empty`, `another-resource-type`, `a-region-segment`, `an-account-that-is-not-twelve-digits` — none of which exercises the `[a-zA-Z0-9_-]+` rule-id segment.

## Steps to Reproduce

1. Start the emulator and enroll the caller (`UpdateEnrollmentConfiguration` with `status=Active`).
2. POST `/` with `X-Amz-Target: ComputeOptimizerAutomationService.GetAutomationRule` and body `{"ruleArn": "arn:aws:compute-optimizer::123456789012:automation-rule/bad id"}` (a space is outside the model's `[a-zA-Z0-9_-]+` rule-id class, so the string matches no `RuleArn` and names no rule of this service).
3. Observe HTTP 400 `InvalidParameterValueException` from the framework's `@pattern` walk.
4. Compare with `{"ruleArn": "arn:aws:compute-optimizer:us-east-1:123456789012:automation-rule/0123456789abcdef"}` (a region segment, another way the same pattern fails), which the gate catches and answers HTTP 403 `AccessDeniedException` — the two malformed ARNs get different classifications.
5. Repeat step 2 for `TagResource`/`UntagResource`/`ListTagsForResource` with the same string in `resourceArn`: same 400.
