Issue-Severity: MEDIUM
Issue-Gap-Class: validation
Issue-Commit: 6b1652c669c36440eab65de28fac2314531b273d

# CreatePermissionSet stores a SessionDuration outside the service's 1-12 hour window (and negative durations), and accepts reserved aws: / duplicate tag keys

_Filed: 2026-10-24T20:09:00Z_
_Operation: CreatePermissionSet | Class: validation_

> _Note: this issue was identified against the code base at commit `6b1652c669c36440eab65de28fac2314531b273d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Description

Two refusals the service makes but the handler does not, both of which the model cannot express so the boundary validator passes the request:

1. `SessionDuration`. The modelled `Duration` shape is only an ISO-8601 SHAPE check — `^(-?)P(?=\d|T\d)(?:(\d+)Y)?(?:(\d+)M)?(?:(\d+)([DW]))?(?:T(?:(\d+)H)?(?:(\d+)M)?(?:(\d+(?:\.\d+)?)S)?)?$` — so it admits `PT24H`, `P1Y`, `PT0S` and even the NEGATIVE `-PT1H`. IAM Identity Center bounds a permission set's session duration to between 1 hour and 12 hours and refuses anything outside it with `ValidationException`. Expected: `{"InstanceArn":...,"Name":"X","SessionDuration":"PT24H"}` is refused (400 `ValidationException`) and nothing is stored. Actual: the handler copies `input.session_duration` straight onto the record (create_permission_set.rs line 268) and both the create response and every later `DescribePermissionSet` echo `PT24H` / `-PT1H` as the permission set's session duration — a value no real permission set can hold.

2. `Tags`. The handler stores `input.tags.unwrap_or_default()` verbatim (line 270). The modelled `TagKey` pattern `^([\p{L}\p{Z}\p{N}_.:/=+\-@]*)$` admits `:`, so `aws:`-prefixed keys reach the handler, and nothing rejects two entries with the same `Key`. AWS reserves the `aws:` tag-key prefix for its own use and refuses a caller-supplied key in that namespace, and refuses a tag list carrying the same key twice, with `ValidationException`. Expected: both requests are refused and no permission set is created. Actual: both are accepted, a permission set is minted, and the record keeps the reserved/duplicated keys.

The same unvalidated `Tags` path exists in the sibling create handlers (create_instance.rs line 266, create_application.rs line 312, create_trusted_token_issuer.rs line 293) — the fix belongs in one shared validator rather than only in this handler.

## Evidence

Spec/model: `smithy_query(query="shape", shape="Duration")` shows the pattern above — a form check only, with no range, and with an optional leading `-`; `smithy_query(query="shape", shape="TagKey")` shows `length 1..128` + a pattern that permits `:`, so neither the 1-12 hour window nor the reserved-prefix rule is expressible in the model and both must be enforced in the handler. Well-established service behaviour: IAM Identity Center documents a permission-set session duration of 1 hour to 12 hours (the API refuses longer or non-positive values with `ValidationException`), and AWS-wide tagging rules reserve the `aws:` prefix and refuse duplicate keys in one tag list. `CreatePermissionSet` models `ValidationException` (HTTP 400), so the refusal channel already exists.

Handler: services/service-com-amazonaws-ssoadmin-swbexternalservice/src/ops/create_permission_set.rs — the only checks before the insert are the account-instance surface withdrawal (lines 193-205), instance existence (210-223), name uniqueness (226-234) and the emulator's store ceiling (239-246). `session_duration`, `relay_state` and `tags` are moved onto the record unexamined (lines 262-276). `grep -i "session duration|aws:|tag_key"` over src/ returns no validation for either rule.

Test coverage: none. services/service-com-amazonaws-ssoadmin-swbexternalservice/tests/all/ssoadmin_permission_sets.rs only uses in-window values (`session_duration: Some("PT1H")`, `"PT2H"` at line 333/506) and asserts the echo; no test sends `PT24H`, `-PT1H`, an `aws:`-prefixed tag key or a duplicated tag key.

## Steps to Reproduce

1. Start the emulator and read the account's instance ARN with `X-Amz-Target: SWBExternalService.ListInstances`; enable permission sets on it if it is an account instance (`UpdateInstance` with `{"InstanceArn":"<instance>","PermissionSetsEnabled":true}`).
2. POST / with `X-Amz-Target: SWBExternalService.CreatePermissionSet` and body `{"InstanceArn":"<instance>","Name":"TooLong","SessionDuration":"PT24H"}` — the emulator answers 200 and the returned `PermissionSet.SessionDuration` is `PT24H`, where AWS refuses the request (400 `ValidationException`, session duration must be between 1 and 12 hours).
3. Repeat with `"SessionDuration":"-PT1H"` and with `"SessionDuration":"PT0S"` — both are accepted and echoed back by `DescribePermissionSet`.
4. POST / with `X-Amz-Target: SWBExternalService.CreatePermissionSet` and body `{"InstanceArn":"<instance>","Name":"ReservedTag","Tags":[{"Key":"aws:cloudformation:stack-name","Value":"x"}]}` — 200 and a permission set is minted, where AWS refuses a caller-supplied `aws:`-prefixed tag key with `ValidationException`.
5. POST / with the same target and `{"InstanceArn":"<instance>","Name":"DupTag","Tags":[{"Key":"team","Value":"a"},{"Key":"team","Value":"b"}]}` — 200, where AWS refuses the duplicated tag key.
6. Confirm with `ListPermissionSets` that each refused-by-AWS request nevertheless left a stored permission set behind.
