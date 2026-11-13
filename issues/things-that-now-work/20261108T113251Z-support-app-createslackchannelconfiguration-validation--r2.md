Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e17a7ccc5cd6a075db896e119c91d98502826237

# channel_role_account_refusal admits a channelRoleArn the member's own `(.+)` rejects, so an unmodelled value is stored and reported back

_Filed: 2026-11-08T11:32:51Z_
_Operation: CreateSlackChannelConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `e17a7ccc5cd6a075db896e119c91d98502826237`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`channelRoleArn` is taken OFF the request boundary on both operations that write it (`skip_validation = ["channelRoleArn"]`), so the member's whole constraint set — `@length` AND `@pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$` — stops being enforced by the framework, and the crate takes over judging the value itself. `crate::state::channel_role_arn_length_refusal` reproduces the length half, and its doc says the length maximum is "the one rule of this member the request boundary no longer applies"; `crate::state::channel_role_account_refusal` is supposed to reproduce the rest, and its doc says the test it applies is "a non-empty role name — exactly what the member's `(.+)` admits after `role/`".

The code does not implement that: it tests `!role_name.is_empty()`, which admits strictly more than `(.+)` anchored by `$` does. A role name containing a line terminator (`arn:aws:iam::<account>:role/Support\nApp`) is non-empty, so the handler accepts it, persists it in `slack_channel_configurations` and answers HTTP 200 — yet `(.+)$` does not match a string containing a newline, so the value is outside the member's modelled form. The same `roleArn` shape, with the same `@pattern`, is an OUTPUT member: `UpdateSlackChannelConfiguration` reports `channelRoleArn` back, and `ListSlackChannelConfigurations` reports it inside every `SlackChannelConfiguration`. The crate's own stated reason for applying the length bound in the handler is exactly that "the same member reports the stored value back on `UpdateSlackChannelConfiguration` and `ListSlackChannelConfigurations`" — the pattern half of the member was never given the same treatment, so the store can hold, and the two read paths can report, a `channelRoleArn` the model forbids on the wire.

The helper is reached from `CreateSlackChannelConfiguration` (filed here, the first operation whose handler calls it) and from `UpdateSlackChannelConfiguration`, which calls it on the same value with the same consequence.

## Evidence

The rule the crate says it implements — services/service-com-amazonaws-supportapp-supportapp/src/state.rs:496-504:
```
/// A service acting for one account cannot be handed a role another account owns, so the
/// test is the one the member's own form states: the value has to be an IAM role ARN of
/// the caller's account NAMING a role, which is that form with the account field filled
/// in from [`::engine::emulator_account_id`] and a non-empty role name — exactly what the
/// member's `(.+)` admits after `role/`. It therefore needs no ARN reading of its own and
/// it fails CLOSED — anything that is not a role ARN of this account, whatever else it
/// is, is a role this service cannot pass.
```

The code under that comment — src/state.rs:505-511:
```
pub fn channel_role_account_refusal(role_arn: &str) -> Option<String> {
    let own_account_role = format!("arn:aws:iam::{}:role/", ::engine::emulator_account_id());
    let names_a_role_of_this_account = role_arn
        .strip_prefix(&own_account_role)
        .is_some_and(|role_name| !role_name.is_empty());
    (!names_a_role_of_this_account).then(|| CROSS_ACCOUNT_PASS_ROLE_MESSAGE.to_string())
}
```
`!role_name.is_empty()` is NOT "exactly what the member's `(.+)` admits": `(.+)` anchored with `$` does not admit a value containing a line terminator, `!is_empty()` does.

The boundary that no longer applies the pattern — src/ops/types/create_slack_channel_configuration_types.rs:14-25:
```
// `skip_validation = ["channelRoleArn"]`: AWS does not enforce `roleArn`'s `@pattern` on this
// member at the request boundary. ... The member's form is therefore judged in
// the handler, which is the only place that answer can be given;
#[smithy_macros::smithy(
    shape = "com.amazonaws.supportapp#CreateSlackChannelConfigurationRequest",
    skip_validation = ["channelRoleArn"]
)]
```
and the same list on src/ops/types/update_slack_channel_configuration_types.rs:17-20.

Why a stored value matters — src/state.rs:529-534 (doc of `channel_role_arn_length_refusal`):
```
/// The member's declared `@length` maximum is a bound on what the API can carry — the
/// same member reports the stored value back on `UpdateSlackChannelConfiguration` and
/// `ListSlackChannelConfigurations` — and it is the one rule of this member the request
/// boundary no longer applies, because taking the value into the pass-role step took the
/// whole member off that boundary
```

The member the stored value is reported on — services/service-com-amazonaws-supportapp-supportapp/OPERATIONS.md:95 (UpdateSlackChannelConfiguration **Output**):
```
- **Output:** `teamId` ... `channelRoleArn` (string, len 31..2048, pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$)
```

Handlers that reach the helper: src/ops/create_slack_channel_configuration.rs:224-235 (`crate::state::channel_role_account_refusal(&input.channel_role_arn)`) and src/ops/update_slack_channel_configuration.rs:245-258 (`.and_then(crate::state::channel_role_account_refusal)`).

Test coverage: tests/all/channel_role_pass_role_form.rs:9-12 states the rule as "a value that does not name an IAM role of the CALLING account is refused with the modelled `AccessDeniedException` (HTTP 403) and AWS's wording for that refusal — whatever form it has, the member's own `@pattern` and `@length` included", and the refused table `values_naming_no_role_of_this_account()` (lines 57-102) covers nine forms (no ARN, another service's ARN, `user/` instead of `role/`, a non-twelve-digit account, an empty account segment, below the length minimum, a foreign account, an empty role name, an upper-cased scheme). No row carries a role name containing a line terminator, so this divergence is unpinned; tests/all/channel_role_account.rs covers only the account segment.

## Steps to Reproduce

1. Mount the Support App service with the Organizations service on one engine, authorize `T012ABCDEFG` in the console-authorization set and POST /control/register-slack-workspace-for-organization with `{"teamId":"T012ABCDEFG"}` so the workspace precondition is satisfied.
2. POST /control/create-slack-channel-configuration with `{"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","notifyOnCaseSeverity":"none","channelRoleArn":"arn:aws:iam::000000000000:role/Support\nApp"}` (the JSON escape `\n` puts a real newline inside the role name; substitute the emulator's account id).
3. Observe HTTP 200 — the prefix test in `channel_role_account_refusal` passes because the role name is non-empty — where every sibling row of tests/all/channel_role_pass_role_form.rs that leaves the member's form is answered 403 `AccessDeniedException`.
4. POST /control/list-slack-channel-configurations (and /control/update-slack-channel-configuration for the same pair) and observe `channelRoleArn` reported back with the embedded newline — a value the output member's `@pattern ^arn:aws:iam::[0-9]{12}:role/(.+)$` does not admit.
