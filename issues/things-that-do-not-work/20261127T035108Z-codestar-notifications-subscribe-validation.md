Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 588d12f22e65a5cd40d785b9552220e3326aeffc

# Target.TargetType is stored and reported unchecked, though the model documents the three values it can hold and this crate enforces every other doc-only value rule

_Filed: 2026-11-27T03:51:08Z_
_Operation: Subscribe | Class: validation_

> _Note: this issue was identified against the code base at commit `588d12f22e65a5cd40d785b9552220e3326aeffc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`Target$TargetType` is a pattern-only string in the model, and its documentation enumerates the values it can carry: SNS, AWSChatbotSlack, AWSChatbotMicrosoftTeams. The one place this crate turns wire targets into stored targets — `crate::state::rule_targets_from` — checks only that the two members are PRESENT and non-empty; it never compares the type to that documented set. So `Subscribe` (and the `CreateNotificationRule` / `UpdateNotificationRule` paths that call the same helper) accepts any `^[A-Za-z]+$` word as a target type, persists it on the rule and in the account-wide target registry, and then reports it back through `DescribeNotificationRule$Targets[].TargetType` and `ListTargets$Targets[].TargetType`, where it even becomes a matchable value for the `TARGET_TYPE` filter.

This is the one doc-only value restriction the crate skips. The crate already enforces exactly this class of rule for the other two members whose documentation narrows a pattern-only string: `Resource` (refused unless its ARN names one of `SUPPORTED_RESOURCE_SERVICES`) and every `EventTypeIds` entry (refused unless the published catalog holds it), both with the same stated rationale — "the service, not the model, restricts the type". `TargetType` carries the same kind of documentation and gets no check at all, so a rule can be wired to a target type the service does not publish, and `ListTargets` can report a target whose `TargetType` is outside the documented set.

## Evidence

Model, `Target$TargetType` (`smithy_query(query="shape",shape="Target")`): "The target type. Can be an Amazon Q Developer in chat applications topic or Amazon Q Developer in chat applications client." followed by "Amazon Q Developer in chat applications topics are specified as `SNS`.", "Amazon Q Developer in chat applications clients are specified as `AWSChatbotSlack`.", "Amazon Q Developer in chat applications clients for Microsoft Teams are specified as `AWSChatbotMicrosoftTeams`." The member carries only `@pattern ^[A-Za-z]+$` (no enum), so nothing the framework runs restricts the value.

The code that stores it — `src/state.rs:759-775` (`rule_targets_from`):
```
        let target_type = target
            .target_type
            .as_deref()
            .filter(|value| !value.is_empty())
            .ok_or_else(|| {
                format!("Target.TargetType is required for every target (address {address})")
            })?;
```
Presence is the ONLY test; the value is then written verbatim (`target_type: target_type.to_string()`).

Its one caller on this operation — `src/ops/subscribe.rs:166-170`:
```
        let target = st::rule_targets_from(std::slice::from_ref(&input.target))
            .map_err(misconfigured)?
```
and the stored value reaches the registry at `src/state.rs:156-161` (`entry.target_type = target.target_type.clone();`), is reported by `src/ops/describe_notification_rule.rs:213` (`target_type: Some(target.target_type.clone())`) and by `src/ops/list_targets.rs:201` (`target_type: Some(entry.target_type.clone())`), and is matched as a filter value at `src/ops/list_targets.rs:96` (`"TARGET_TYPE" => entry.target_type == value`).

The sibling rule this path skips — `src/state.rs:465-490`:
```
/// The service — not the model — restricts the type: `NotificationRuleResource`'s pattern
/// (`smithy_query(query="shape",shape="NotificationRuleResource")`) accepts ANY service's
/// ARN, while the member documents "Supported resources include pipelines in
/// CodePipeline, ..."
pub const SUPPORTED_RESOURCE_SERVICES: &[&str] =
    &["codebuild", "codecommit", "codedeploy", "codepipeline"];
...
pub fn resource_type_is_supported(resource: &str) -> bool {
```
applied at `src/ops/create_notification_rule.rs:209-216` together with `st::event_type_ids_violation`.

The crate's own words that these are the only two such rules — `tests/all/create_rule_input_validation.rs:3`: "//! The two input rules the MODEL cannot express but the SERVICE enforces, over the wire."

And the crate's own statement of the documented target-type set — `tests/all/list_targets.rs:28-33`:
```
/// The target types this suite subscribes, which are `TargetSummary$TargetType`'s own
/// documented values ("Amazon Q Developer in chat applications topics are specified as
/// `SNS`", clients for Slack as `AWSChatbotSlack`, for Microsoft Teams as
/// `AWSChatbotMicrosoftTeams`) ...
const TARGET_TYPES: [&str; 3] = ["SNS", "AWSChatbotSlack", "AWSChatbotMicrosoftTeams"];
```
No test in `tests/all/` subscribes a target type outside those three, so nothing pins the permissive behaviour: grep for `TargetType` across `tests/all/` yields only `SNS`, `AWSChatbotSlack`, `AWSChatbotMicrosoftTeams` (plus the absent-member cases at `tests/all/notification_rule_lifecycle.rs:1325`).

## Steps to Reproduce

1. POST /createNotificationRule with Name "target-type-probe", Resource arn:aws:codecommit:us-east-1:123456789012:MyRepo, DetailType BASIC, EventTypeIds ["codecommit-repository-comments-on-commits"], Targets [] -> 200, note the returned Arn.
2. POST /subscribe with {"Arn": <that Arn>, "Target": {"TargetType": "Banana", "TargetAddress": "arn:aws:sns:us-east-1:123456789012:topic"}} -> 200 (no ValidationException/ConfigurationException), although `Target$TargetType`'s documentation lists only SNS, AWSChatbotSlack and AWSChatbotMicrosoftTeams.
3. POST /describeNotificationRule with that Arn -> Targets[0].TargetType is "Banana", persisted on the rule.
4. POST /listTargets with {} -> the target is reported with TargetType "Banana"; POST /listTargets with Filters [{"Name": "TARGET_TYPE", "Value": "Banana"}] -> it matches, so the undocumented type is a first-class filterable value in the registry.
5. Compare with the same handler's treatment of the other two doc-only restrictions: POST /createNotificationRule with Resource arn:aws:s3:::bucket -> 400 ValidationException (src/ops/create_notification_rule.rs:209), and with an uncatalogued EventTypeIds entry -> 400 ValidationException (src/ops/create_notification_rule.rs:214).
