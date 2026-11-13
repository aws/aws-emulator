Issue-Source: trace-sweep-divergence
Issue-Trace: deadline-limit-lifecycle
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: deadline-limit-lifecycle
Issue-Shape-Id: com.amazonaws.deadline#Deadline
Issue-Key: deadline|GetLimit|2xx->2xx|$.createdAt
Issue-Key: deadline|GetLimit|4xx->4xx|$.message
Issue-Key: deadline|GetLimit|4xx->2xx|$.message
Issue-Key: deadline|GetLimit|4xx->2xx|$.resourceId
Issue-Key: deadline|GetLimit|4xx->2xx|$.resourceType
Issue-Key: deadline|GetLimit|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: deadline|GetLimit|4xx->2xx|status
Issue-Commit: 435579c3f4f417d2ee68a6d86543c2b3ca676fd8

# deadline GetLimit diverges on three counts: epoch-seconds createdAt, reworded 404, and a deleted limit still readable

> _Note: this issue was identified against the code base at commit `435579c3f4f417d2ee68a6d86543c2b3ca676fd8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetLimit

## Description
THREE independent defects on the deadline limit read/delete path, all observed replaying the limit lifecycle fixture. A fixer must close ALL THREE before submitting.

Cause 1 of 3 - createdAt is serialized as an epoch-seconds JSON NUMBER instead of a date-time string (fixture deadline-limit-lifecycle, step task#3).
AWS: Smithy: GetLimit is GET /2023-10-12/farms/{farmId}/limits/{limitId}, success 200; its output member createdAt targets CreatedAt, is @required, type timestamp, @timestampFormat("date-time") (updatedAt targets UpdatedAt, also date-time). The service speaks restJson1, so a date-time timestamp is carried on the wire as an ISO-8601/RFC-3339 STRING ("<yyyy>-<MM>-<dd>T<HH>:<mm>:<ss>Z"), never a bare JSON number. The recording asserts that member by SHAPE, not by value, because a per-run instant is not a requirement.
Emulator: the 200 body carries $.createdAt as a bare JSON number of epoch seconds with a fractional part. Every other asserted member of that 200 matches.
Why: every AWS SDK deserializes the member per the model, so a number where a date-time string is modeled either fails to parse or is silently reinterpreted; any client that displays, compares or does date math on createdAt/updatedAt gets a wrong or absent instant.
Fix: the operation's response serialization of timestamp members - the generated output schema / serializer timestampFormat wiring, not the handler (the handler already builds a correctly typed DateTime). Hypothesis: the serializer falls back to restJson1's protocol-default epoch-seconds encoding because @timestampFormat("date-time") on CreatedAt/UpdatedAt is not honored at serialization time. This is the same serialization gap already filed for GetStorageProfile (open issue 20261030T195531Z-deadline-getstorageprofile-diverges-on-three-counts-epoch-seconds.md); that issue carries no replay key for this fixture step, so the GetLimit occurrence is filed here.
Traces: deadline-limit-lifecycle#task#3.

Cause 2 of 3 - the limit 404 uses the emulator's own wording instead of AWS's ResourceNotFoundException sentence (fixture deadline-limit-lifecycle, step task#4).
AWS: Smithy: GetLimit models ResourceNotFoundException (httpError 404) whose required members are message, resourceId and resourceType; the model documents resourceType only as "The type of the resource that couldn't be found" and enumerates no values. Recorded behavior: a GET for a syntactically valid (^limit-[0-9a-f]{32}$) but non-existent limit answers HTTP 404 ResourceNotFoundException with message 'Resource of type limit with id <limitId> does not exist.' The FIXED text of that message is 'Resource of type <resourceType> with id <resourceId> does not exist.' including the trailing period; the VARIABLE parts are the resource kind ('limit') and the limitId taken verbatim from the request. resourceType and resourceId already match and are not part of this cause.
Emulator: same status (404) and same error code, but message "The limit <limitId> can't be found." - i.e. the template "The {resource_type} {resource_id} can't be found."
Why: the error message is part of the contract. A client, fixture or support tool that matches AWS's sentence sees no match and falls through to a generic error branch, so not-found handling that works against AWS breaks here.
Fix: the service's single-homed refusal builder (crate::state::not_found) - change the template there, not its call sites. Related but NOT covering this: open issues 20261029T172830Z-deadline-resourcenotfoundexception-message-is-the-emulator-s-own.md (filed for GetFarm) and 20261031T070507Z-deadline-fleet-resourcenotfoundexception-message-is-the-emulator-s.md (fleets) report the same wording defect for other resources; neither carries a replay key for this fixture step, so the GetLimit occurrence is filed here.
Traces: deadline-limit-lifecycle#task#4.

Cause 3 of 3 - a limit remains readable after it has been deleted: 200 where AWS answers 404 (fixture deadline-limit-lifecycle, step task#6).
AWS: Smithy: DeleteLimit is DELETE /2023-10-12/farms/{farmId}/limits/{limitId}, @idempotent, success 200 with empty output. Once it succeeds the limit no longer exists, so GetLimit on that id answers HTTP 404 ResourceNotFoundException - body with message 'Resource of type limit with id <limitId> does not exist.', resourceType 'limit', resourceId the id from the request - plus the restJson1 error header x-amzn-errortype naming the exception. That 404 is what the recording shows for the read following the delete.
Emulator: the same read answers HTTP 200 with the limit body still intact: no message, no resourceType, no resourceId, no x-amzn-errortype header. The delete left the limit in state.
Why: a client cannot tell a deleted resource from a live one. Deletion appears to silently fail, so teardown loops never converge, re-creation and uniqueness checks see a phantom limit, and any workflow that polls for 404 after delete hangs.
Fix: the delete path's state mutation - the limit must be removed from its farm's limits map so the read path's existing not-found branch fires. DeleteLimit is still an unimplemented scaffold stub in this checkout (its handler returns Scaffolded::not_implemented), so standing that operation up must also make this subsequent read answer the modeled 404, which is the divergence recorded here.
Traces: deadline-limit-lifecycle#task#6.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, bootstrap_signal=false, score 49.3%, 71 steps):

Cause 1 - fixture deadline-limit-lifecycle step task#3, GetLimit, expected 200 / actual 200:
  "$.createdAt: value_mismatch (expected '<shaped-not-asserted>', got 1793436115.2920616)"
The expected value is asserted by shape (a date-time string); the emulator emitted a JSON number of epoch seconds. That 1793436115.2920616 is this run's instant and is not itself a requirement. Model evidence: smithy_query(operation="GetLimit") -> output createdAt {target CreatedAt, required, type timestamp, timestampFormat "date-time"}, updatedAt likewise; smithy_query(shape="CreatedAt") -> timestamp, timestampFormat "date-time". Code: services/service-com-amazonaws-deadline-deadline/src/ops/get_limit.rs:199-201 sets created_at: crate::state::timestamp(limit.created_at) / updated_at: limit.updated_at.map(crate::state::timestamp), and src/state.rs:666-668 returns an aws_smithy_types::DateTime - so the date-time intent is lost downstream of the handler, in serialization.

Cause 2 - fixture deadline-limit-lifecycle step task#4, GetLimit, expected 404 / actual 404:
  "$.message: value_mismatch (expected 'Resource of type limit with id limit-ee01db9f44748ee130163a0ab6772ed5 does not exist.', got \"The limit limit-ee01db9f44748ee130163a0ab6772ed5 can't be found.\")"
The limit- id shown is that run's value, not a requirement; the requirement is the sentence template 'Resource of type <resourceType> with id <resourceId> does not exist.' with the resource kind and the request's limitId filled in. Code: src/state.rs:670-681 - not_found() formats message: format!("The {resource_type} {resource_id} can't be found."); the resourceType constant is already correct (src/state.rs:96-100, RESOURCE_TYPE_LIMIT = "limit"), and the replay reports no resourceType/resourceId mismatch for this step. Call site: src/ops/get_limit.rs:188-193. Model evidence: smithy_query(shape="ResourceNotFoundException") -> httpError 404, errorKind client, required members message/resourceId/resourceType.

Cause 3 - fixture deadline-limit-lifecycle step task#6, GetLimit, expected 404 / actual 200:
  "status: expected 404, got 200"
  "$.message: missing (expected 'Resource of type limit with id limit-a2b01ac903c10198df27ae22573e24bc does not exist.', got None)"
  "$.resourceType: missing (expected 'limit', got None)"
  "$.resourceId: missing (expected 'limit-a2b01ac903c10198df27ae22573e24bc', got None)"
  "$headers.x-amzn-errortype: missing (expected '<shaped-not-asserted>', got None)"
Model evidence: smithy_query(operation="DeleteLimit") -> DELETE /2023-10-12/farms/{farmId}/limits/{limitId}, idempotent, success 200, empty output; GetLimit models ResourceNotFoundException (404). Code: the read's not-found branch exists (src/ops/get_limit.rs:188-193) and fires only when the limit is absent from the farm's limits map (src/state.rs:630-634, ComAmazonawsDeadlineDeadlineState::limit), so the gap is the removal on delete; src/ops/delete_limit.rs:140-158 is still the scaffold stub returning crate::scaffolded::Scaffolded::not_implemented("DeleteLimit").

## Steps to Reproduce
Replay fixture deadline-limit-lifecycle of trace set simparity-wire-aws-v1 against the emulator, or by hand against the deadline endpoint:

1. deadline.create_farm(displayName=...) -> farmId; deadline.create_limit(farmId=farmId, displayName=..., amountRequirementName=..., maxCount=...) -> limitId.
2. (Cause 1, step task#3) deadline.get_limit(farmId=farmId, limitId=limitId). Observed: 200 with "createdAt" as a bare JSON number of epoch seconds (1793436115.2920616 in the recorded run). Expected: 200 with createdAt (and updatedAt when present) as an ISO-8601 date-time STRING, per @timestampFormat("date-time").
3. (Cause 2, step task#4) deadline.get_limit(farmId=farmId, limitId=<a syntactically valid ^limit-[0-9a-f]{32}$ id never created>). Observed: 404 ResourceNotFoundException with message "The limit <id> can't be found.". Expected: 404 ResourceNotFoundException with message 'Resource of type limit with id <id> does not exist.' (resourceType 'limit' and resourceId <id> already correct).
4. (Cause 3, step task#6) deadline.delete_limit(farmId=farmId, limitId=limitId), then deadline.get_limit(farmId=farmId, limitId=limitId). Observed: the read answers 200 with the limit body, no error members and no x-amzn-errortype header. Expected: 404 ResourceNotFoundException with message 'Resource of type limit with id <id> does not exist.', resourceType 'limit', resourceId <id>, and an x-amzn-errortype header naming the exception. (DeleteLimit is currently an unimplemented stub, so the delete itself answers 501 here - standing it up must also make this read answer the modeled 404.)
