Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 927eef52ff1b7195638ad898bf452ad5f9c2d613

# channelName's `^.+$` is left to the walk the crate says admits CR/U+2028/U+2029, so an unmodelled channel name is stored and reported back

_Filed: 2026-11-14T01:34:08Z_
_Operation: CreateSlackChannelConfiguration | Class: validation_

> _Note: this issue was identified against the code base at commit `927eef52ff1b7195638ad898bf452ad5f9c2d613`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

This crate states, in its own state module, that a Smithy `@pattern` is an ECMA-262 expression whose `.` matches no line terminator, AND that the engine the framework's centralized constraint walk compiles a `@pattern` with reads `.` as every character except a line FEED alone — so carriage return, U+2028 and U+2029 are admitted by that walk "whatever the modelled expression says". The crate therefore applies the rule BY HAND for `channelRoleArn`'s `(.+)` (`is_role_name` over `PATTERN_DOT_EXCLUDED_CHARS`).

`channelName` is the other `.`-based pattern on the same two writing operations (`pattern ^.+$`), and nothing in the crate applies that rule to it: it is not in either input's `skip_validation` list, so the only check it gets is exactly the walk the crate says admits three of the four excluded characters, and `CreateSlackChannelConfiguration` stores the value verbatim (`channel_name: input.channel_name`). A create whose `channelName` carries a carriage return, U+2028 or U+2029 is therefore accepted (HTTP 200), persisted in `slack_channel_configurations`, and reported back by `ListSlackChannelConfigurations` (inside every `SlackChannelConfiguration`) and by `UpdateSlackChannelConfiguration` through an OUTPUT member that declares the same `^.+$` — the exact situation the crate's own test suite for the sibling member says must not be reachable ("a value the pattern does not admit could not be reported back through the shape it was stored in"). `UpdateSlackChannelConfiguration` merges and reports the same value on the same path (`src/ops/update_slack_channel_configuration.rs:304`), so the one fix covers both writers.

## Evidence

The rule the crate states, and the framework gap it states it against — services/service-com-amazonaws-supportapp-supportapp/src/state.rs:513-525:
```
/// The characters a modelled `@pattern`'s `.` does NOT match.
///
/// A Smithy `@pattern` is an ECMA-262 regular expression, and there `.` matches every
/// character except a LINE TERMINATOR: line feed, carriage return, line separator and
/// paragraph separator. A value carrying any of them therefore cannot be matched by a `.`
/// of such an expression, however many characters it has.
///
/// Stated here because the expression has to be applied by hand (see
/// [`channel_role_account_refusal`]), and because the engine the framework's centralized
/// constraint walk compiles a `@pattern` with reads `.` as every character except a line
/// FEED alone, so the other three would be admitted there whatever the modelled expression
/// says.
const PATTERN_DOT_EXCLUDED_CHARS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];
```

The path that DOES apply it — src/state.rs:534-539:
```
fn is_role_name(role_name: &str) -> bool {
    !role_name.is_empty()
        && !role_name
            .chars()
            .any(|character| PATTERN_DOT_EXCLUDED_CHARS.contains(&character))
}
```

The stated reason that path exists — tests/all/channel_role_name_pattern.rs:16-21:
```
//! Why the handler has to be the layer that says so: the SAME shape, with the same
//! `@pattern`, is an OUTPUT member — `UpdateSlackChannelConfiguration` answers with
//! `channelRoleArn` and `ListSlackChannelConfigurations` reports it inside every
//! `SlackChannelConfiguration` — so a value the pattern does not admit could not be reported
//! back through the shape it was stored in.
```

The member that is left to the walk: `smithy_query(query="shape",shape="channelName")` -> `{"length": {"min": 1, "max": 256}, "pattern": "^.+$"}`, and it is an OUTPUT member too — `smithy_query(query="shape",shape="SlackChannelConfiguration")` -> `"channelName": {"target": "channelName", "pattern": "^.+$"}`; OPERATIONS.md:95 lists `channelName` (string, len 1..256, pattern ^.+$) in UpdateSlackChannelConfiguration's **Output**.

Only `channelRoleArn` is taken off the boundary, so the walk is the sole check of `channelName` — src/ops/types/create_slack_channel_configuration_types.rs:22-25:
```
#[smithy_macros::smithy(
    shape = "com.amazonaws.supportapp#CreateSlackChannelConfigurationRequest",
    skip_validation = ["channelRoleArn"]
)]
```
(the same single-member list on src/ops/types/update_slack_channel_configuration_types.rs:19).

The handler stores the value with no form check — src/ops/create_slack_channel_configuration.rs:336: `channel_name: input.channel_name,`; the read paths that report it back — src/ops/list_slack_channel_configurations.rs:194-198 (`.map(crate::state::SlackChannelConfig::to_modeled)`) and src/ops/update_slack_channel_configuration.rs:371 (`channel_name: persisted.channel_name`).

Test coverage: the line-terminator rule is swept only for the role name — tests/all/channel_role_name_pattern.rs:53-58 `LINE_TERMINATORS` over `channelRoleArn`. A repo-wide grep for `2028`/`LINE_TERMINATORS` matches only that suite and src/state.rs:525, and the channelName rows in tests/all/slack_channel_configurations.rs:516-549 and tests/all/resource_preconditions.rs:82-106 carry only ordinary names ("aws-support", "a channel name with spaces", "n".repeat(256)), so nothing pins the current behaviour.

## Steps to Reproduce

1. Mount the Support App service with the Organizations service on one engine, authorize `T012ABCDEFG` in the console-authorization set, put the account in an organization, and POST /control/register-slack-workspace-for-organization with {"teamId":"T012ABCDEFG"} so the workspace precondition is satisfied.
2. POST /control/create-slack-channel-configuration with {"teamId":"T012ABCDEFG","channelId":"C01234A5BCD","channelName":"aws\rsupport" (a real carriage return; repeat with U+2028 and U+2029),"notifyOnCaseSeverity":"none","channelRoleArn":"arn:aws:iam::<emulator-account>:role/SupportAppRole"}.
3. Observe HTTP 200: `channelName` is not in either input's `skip_validation` list, so its `^.+$` is checked only by the walk src/state.rs:519-524 says admits carriage return, U+2028 and U+2029 — while the same character inside `channelRoleArn`'s role name is refused 403 by tests/all/channel_role_name_pattern.rs.
4. POST /control/list-slack-channel-configurations and /control/update-slack-channel-configuration for the same pair, and observe the stored `channelName` reported back with the embedded line terminator — through an output member whose declared `@pattern ^.+$` does not admit it.
