Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: e4aadb2d083ccc600e0c6001a4f71b730596bf57

# accountAlias's `^[\w\- ]+$` is left to the walk the crate says does not implement the modelled expression, so a non-ASCII alias is stored and reported back

_Filed: 2026-11-14T05:20:09Z_
_Operation: PutAccountAlias | Class: validation_

> _Note: this issue was identified against the code base at commit `e4aadb2d083ccc600e0c6001a4f71b730596bf57`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

`PutAccountAlias` stores the request's `accountAlias` verbatim and relies entirely on the framework's centralized constraint walk for the member's declared `@pattern ^[\w\- ]+$`. Its own comment asserts that the boundary "already refused anything outside of" that pattern — but this crate's state module states the opposite about that very layer: a Smithy `@pattern` is an ECMA-262 expression, and the engine the walk compiles a `@pattern` with does NOT implement that expression's semantics, so values are "admitted there whatever the modelled expression says". That is exactly why the crate applies the other hand-checked member's expression itself (`is_role_name` over `PATTERN_DOT_EXCLUDED_CHARS` for `channelRoleArn`'s `(.+)`).

The divergence bites `accountAlias` through `\w`: in the ECMA-262 language a Smithy `@pattern` is written in, `\w` is the ASCII class `[A-Za-z0-9_]`, while the walk compiles the pattern with the Rust `regex` crate, whose `\w` is Unicode-aware. So `PutAccountAlias {accountAlias: "Jürgen"}` (or "日本", "ñ", any non-ASCII word character) is accepted with HTTP 200, persisted into `state.account_alias`, and then reported back by `GetAccountAlias` through an OUTPUT member that declares the same `^[\w\- ]+$` — a value that member's declared pattern does not admit. The alias trio is the one resource of this service whose only form check is that walk: no `skip_validation` entry exists for `PutAccountAliasRequest`, and no handler-side charset rule is applied anywhere on the alias path.

This is the same defect class the crate already answers by hand for `channelRoleArn`; here nothing answers it, and the handler comment claiming the boundary did is the inconsistency.

## Evidence

The model: `smithy_query(query="shape",shape="awsAccountAlias")` -> `{"shape_id": "com.amazonaws.supportapp#awsAccountAlias", "type": "string", "length": {"min": 1, "max": 30}, "pattern": "^[\\w\\- ]+$"}`, and the SAME shape is the output member: `smithy_query(query="shape",shape="GetAccountAliasResult")` -> `"accountAlias": {"target": "awsAccountAlias", "length": {"min": 1, "max": 30}, "pattern": "^[\\w\\- ]+$"}` (also OPERATIONS.md:58 `accountAlias` (string, len 1..30, pattern ^[\w\- ]+$)).

Side 1 — the handler's claim, services/service-com-amazonaws-supportapp-supportapp/src/ops/put_account_alias.rs:153-158:
```
        // "Creates or updates an individual alias for each Amazon Web Services account
        // ID": one alias per account, so a later put replaces the stored value rather
        // than adding a second one. `accountAlias` is `@required`, `len 1..30` and
        // `pattern ^[\w\- ]+$`, all of which the boundary validator already refused
        // anything outside of.
        state.lock().account_alias = Some(input.account_alias);
```

Side 2 — the same crate, stating that the boundary walk does NOT implement the modelled expression, which is why the crate applies such a class by hand, src/state.rs:513-525:
```
/// The characters a modelled `@pattern`'s `.` does NOT match.
///
/// A Smithy `@pattern` is an ECMA-262 regular expression, and there `.` matches every
/// character except a LINE TERMINATOR: line feed, carriage return, line separator and
/// paragraph separator. ...
///
/// Stated here because the expression has to be applied by hand (see
/// [`channel_role_account_refusal`]), and because the engine the framework's centralized
/// constraint walk compiles a `@pattern` with reads `.` as every character except a line
/// FEED alone, so the other three would be admitted there whatever the modelled expression
/// says.
const PATTERN_DOT_EXCLUDED_CHARS: [char; 4] = ['\n', '\r', '\u{2028}', '\u{2029}'];
```
and the path that DOES apply its expression by hand, src/state.rs:534-539:
```
fn is_role_name(role_name: &str) -> bool {
    !role_name.is_empty()
        && !role_name
            .chars()
            .any(|character| PATTERN_DOT_EXCLUDED_CHARS.contains(&character))
}
```

The engine the walk uses, vendor/smithy-schema-server/src/constraints.rs:338-341 and :474:
```
/// Applies to string members. The `regex` is the Smithy pattern verbatim
/// (e.g. `^[a-zA-Z0-9_.-]+$`); Smithy pattern semantics are a *find* (the
/// regex must match somewhere in the value), which is exactly
/// [`regex::Regex::is_match`].
...
    let Ok(re) = regex::Regex::new(regex) else {
```
and what that means for `\w`, stated in this tree about the same walk — services/service-com-amazonaws-bcmpricingcalculator-awsbcmpricingcalculator/src/state.rs:503-507: "the modelled key pattern `^[\w\s:+=@/-]+$` is matched with a Unicode-aware `\w`, so such a key passes validation and reaches this helper"; and the ECMA-262 reading — services/service-com-amazonaws-datazone-datazone/src/state.rs:86: "`@pattern` is an ECMA-262 expression, where `\w` is the ASCII class `[A-Za-z0-9_]`."

Nothing takes the member off the boundary for a handler check either — src/ops/types/put_account_alias_types.rs:14 is a plain `#[smithy_macros::smithy(shape = "com.amazonaws.supportapp#PutAccountAliasRequest")]` with no `skip_validation` (contrast src/ops/types/create_slack_channel_configuration_types.rs:24 `skip_validation = ["channelRoleArn"]`).

Test coverage: nothing pins a non-ASCII alias. Every alias row is ASCII — tests/all/slack_channel_configurations.rs:1319-1329 (`"my-team"`, `"Support Team 1"`, `"under_score"`, `"0123456789"`, `"a".repeat(min)`, `"z".repeat(max)`), tests/all/resource_preconditions.rs:349-372, tests/all/channel_configuration_pair_lookup.rs:84-147 (`"support-team"`, `"aws support app"`, `"a"`, `"Support_App-1"`) — and the only `accountAlias` pattern-refusal row uses an ASCII character outside the class, tests/all/error_message_templates.rs:483-490: `json!({ "accountAlias": "bad alias!" })`.

## Steps to Reproduce

1. Mount the Support App service on the shared engine and sign a request with credentials the IAM gate allows.
2. POST /control/put-account-alias with {"accountAlias":"Jürgen"} (repeat with "日本" or "niño").
3. Observe HTTP 200: the only check of `^[\w\- ]+$` is the walk, which compiles the pattern with `regex::Regex` (vendor/smithy-schema-server/src/constraints.rs:338-341) whose `\w` is Unicode-aware, so the value is stored verbatim by src/ops/put_account_alias.rs:158 — while the ASCII-only row "bad alias!" is refused 400 by the same walk (tests/all/error_message_templates.rs:483-490).
4. POST /control/get-account-alias and observe the non-ASCII alias reported back in `accountAlias`, an output member whose declared `@pattern ^[\w\- ]+$` (ECMA-262 `\w` = [A-Za-z0-9_]) does not admit it — the situation the crate refuses to allow for the sibling member `channelRoleArn` (src/state.rs:534-539).
5. Contrast the handler's own comment at src/ops/put_account_alias.rs:155-157, which asserts the boundary validator "already refused anything outside of" that pattern, with src/state.rs:519-524, which says the walk admits values "whatever the modelled expression says".
