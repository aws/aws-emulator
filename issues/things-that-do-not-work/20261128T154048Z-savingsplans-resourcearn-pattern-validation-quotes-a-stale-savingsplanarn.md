Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: savingsplans.tag-resource-malformed-arn-rejected
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans.tag-resource-malformed-arn-rejected|run#12|AssertionError|assert err["Message"] == (
Issue-Commit: 2ad04cd39860f70eb0b7d33f623aafe0280db657

# savingsplans resourceArn pattern validation quotes a stale SavingsPlanArn regex and rejects every non-default partition ARN

> _Note: this issue was identified against the code base at commit `2ad04cd39860f70eb0b7d33f623aafe0280db657`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
TagResource

## Description
Behavioral replay of the savingsplans service (simparity-behavioral-aws-v1, 20/21 passed) reports ONE net-new root cause in the savingsplans crate (services/service-com-amazonaws-savingsplans-awssavingsplan).

Cause 1 of 1: the `resourceArn` constraint this service enforces — and the ValidationException message it renders from that constraint — uses a SavingsPlanArn regex whose partition field is the bare literal `aws`, where AWS's current SavingsPlanArn pattern admits a suffixed partition. Failing check: savingsplans.tag-resource-malformed-arn-rejected.

AWS: Smithy (com.amazonaws.savingsplans#TagResource, input TagResourceRequest.resourceArn -> com.amazonaws.savingsplans#SavingsPlanArn, @required, @pattern; modeled errors ValidationException (HTTP 400), ResourceNotFoundException (404), ServiceQuotaExceededException (402), InternalServerException (500)). The checked-in model in this repo spells that pattern `^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$` — the partition is the literal `aws` with no alternative. The recorded AWS behavior shows the live service enforcing and QUOTING a wider partition group: `^arn:aws([-a-z]*)?:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$`, i.e. `aws` optionally followed by a `-`-suffixed partition name (`aws-us-gov`, `aws-cn`, `aws-iso-b`, ...). So the checked-in pattern is STALE relative to the service AWS runs, and this is recorded behavior for the regex text: AWS refuses a malformed resourceArn with HTTP 400, error code `ValidationException`, and the standard boundary-validation sentence `1 validation error detected: Value '<the resourceArn as sent>' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: <the SavingsPlanArn pattern>`. In that message the FIXED text is `1 validation error detected: Value '`, `' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ` and the quoted pattern itself (the pattern is a property of the shape, not of the request); the only VARIABLE part is the rejected value, echoed verbatim from the request's resourceArn. The recording's value `not-an-arn` is a request value filling that template, not part of the requirement. Correspondingly, a well-formed ARN that matches the pattern but names no existing plan must get past pattern validation and be refused at lookup with `ResourceNotFoundException` and the message `Resource with resourceArn: <arn> doesn't exist` — which this emulator already does correctly.

Emulator: HTTP 400 with code `ValidationException` (status and code agree) but the message quotes the stale regex: `1 validation error detected: Value 'not-an-arn' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\d{1}|):(\d{12}):savingsplan\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$` — the `aws([-a-z]*)?` partition group is rendered as a bare `aws`. The whole surrounding sentence, the `originalRequest.resourceArn` path and the echoed value all match; only the quoted pattern diverges. The divergence is not merely cosmetic: the same regex is what the boundary validator ENFORCES, so any resourceArn in a non-default partition (`arn:aws-us-gov:savingsplans::<account>:savingsplan/<uuid>`, `arn:aws-cn:...`) is rejected as malformed by this emulator although AWS's pattern admits it, and the crate's own shared partition check hard-anchors the same assumption (state.rs `ARN_PARTITION = "aws"` / `is_aws_partition`, which documents itself as mirroring the literal-`aws` group). Every operation taking a `resourceArn` of this shape inherits both effects (TagResource, UntagResource, ListTagsForResource) as does any output member targeting SavingsPlanArn.

Why: a client that asserts on the refusal — an SDK test, a validation-error parser, or a user-facing form that surfaces the quoted constraint so a caller can correct the ARN — is handed a regex that is not the service's, and one that is strictly NARROWER, so it will conclude a legitimate GovCloud or China-partition Savings Plan ARN is malformed. Worse, a client running against a non-default partition cannot tag, untag or list tags on any plan at all: the call is refused 400 ValidationException at the boundary before any handler runs, where AWS accepts it and proceeds to the resource lookup. Because the status and error code are right, nothing distinguishes this from a genuinely malformed ARN.

Fix: the SavingsPlanArn pattern constraint used for this service's request validation and quoted in the validator's pattern message — the `SavingsPlanArn` shape's `@pattern` in this crate's Smithy model and whatever the boundary validator renders the message from — must carry AWS's current partition group `aws([-a-z]*)?` rather than the bare `aws`, so the enforced set and the quoted text move together; and the crate's shared ARN partition check must accept a suffixed partition instead of anchoring the single literal. File and line citations are under Traces.

Traces: behavioral check savingsplans.tag-resource-malformed-arn-rejected (assertion at check_savingsplans_tag_resource_malformed_arn_rejected.py line 38). Code read while attributing the cause (citations only, no patch): services/service-com-amazonaws-savingsplans-awssavingsplan/smithy-model/com-amazonaws-savingsplans-awssavingsplan.json shape com.amazonaws.savingsplans#SavingsPlanArn (pattern begins `^arn:aws:[a-z]+:`); src/ops/tag_resource.rs doc comment lines ~57-59 (echoes the same stale pattern) and handle() line ~146 (defers the ARN-shape refusal to the boundary validator, then only names the error); src/state.rs ARN_PARTITION line ~792 and is_aws_partition line ~800 (partition anchored to the literal `aws`), savings_plan_id_from_arn ~line 804, savings_plan_id_or_refusal line ~847, resource_not_found_message line ~860.

Not covered by the open savingsplans issues: 20261122T071534Z-savingsplans-listtagsforresource-drops-the-reserved-aws-prefixed-tag.md concerns reserved `aws:`-prefixed TAG KEYS being dropped, and 20261122T152448Z-savingsplans-rate-card-and-plan-projection-serve-rows.md concerns Describe* body content; neither touches the resourceArn pattern constraint or its validation message. The positive control in this same check (TagResource then ListTagsForResource on a real plan with key `sim2real-env`) PASSED, and the ResourceNotFoundException branch PASSED.

## Evidence
Suite summary: Total 20/21 passed (95.2%); Mismatched 1, Errored 0, Skipped 0, Unloadable 0. Per-service: savingsplans 20/21 (95%), 1 failed. bootstrap_signal not set — the emulator is serving and 20 savingsplans checks passed, so this is a fidelity finding, not a bootstrap gap. The failing check is marked neither "ALREADY FILED" nor "OWNED by".

Failing check: [mismatch] savingsplans.tag-resource-malformed-arn-rejected
Assertion detail (verbatim):
  AssertionError: unexpected tag_malformed message: "1 validation error detected: Value 'not-an-arn' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\\d{1}|):(\\d{12}):savingsplan\\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$"

Expected (AWS, as the check asserts):
  Code    = ValidationException (HTTP 400)
  Message = "1 validation error detected: Value 'not-an-arn' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws([-a-z]*)?:[a-z]+:([a-z]{2}-[a-z]+-\\d{1}|):(\\d{12}):savingsplan\\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$"

Actual (emulator):
  Code    = ValidationException (HTTP 400)  <- agrees
  Message = identical EXCEPT the quoted pattern's partition group: `^arn:aws:[a-z]+:` where AWS quotes `^arn:aws([-a-z]*)?:[a-z]+:`.

Diff, isolated: expected `arn:aws([-a-z]*)?:` vs actual `arn:aws:` — the optional `-`-suffixed partition alternative is missing. Everything else (the `1 validation error detected:` prefix, the echoed value `not-an-arn`, the `originalRequest.resourceArn` path, the `Member must satisfy regular expression pattern:` clause, the region alternative `([a-z]{2}-[a-z]+-\\d{1}|)`, the 12-digit account group and the UUIDv1-5 resource group) matches byte-for-byte.

Fixed vs variable in the required message: FIXED = the sentence scaffolding and the quoted pattern (a property of the SavingsPlanArn shape). VARIABLE = only the rejected value, echoed from the request's resourceArn; `not-an-arn` is that request value and is not itself the requirement. No request ids, hosts, ports, timestamps or generated plan/offering ids appear in either side of the comparison.

Model evidence (smithy_query, services/service-com-amazonaws-savingsplans-awssavingsplan/smithy-model/com-amazonaws-savingsplans-awssavingsplan.json):
  SavingsPlanArn = {"type":"string","pattern":"^arn:aws:[a-z]+:([a-z]{2}-[a-z]+-\\d{1}|):(\\d{12}):savingsplan\\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$"}
  TagResourceRequest.resourceArn -> SavingsPlanArn, required=true, same pattern.
So the emulator is faithfully echoing a STALE checked-in pattern; the recorded live service enforces and quotes the wider partition group.

Parts of the same check that PASSED and must not regress: TagResource with a well-formed ARN of an existing plan succeeds and ListTagsForResource reflects `sim2real-env=behavioral-check`; a well-formed ARN naming a nonexistent plan is refused with ResourceNotFoundException and the message `Resource with resourceArn: <arn> doesn't exist`.

## Steps to Reproduce
1. Clone the repo and let the behavioral suite build and boot the emulator from that checkout, then replay simparity-behavioral-aws-v1. The summary must show the savingsplans service serving (20/21 checks pass, bootstrap_signal not set).
2. Replay check savingsplans.tag-resource-malformed-arn-rejected. It first calls sts.GetCallerIdentity, buys a 1-year / No Upfront / USD Compute Savings Plan with purchaseTime 200 days out (so it stays `queued` and can be cleaned up with DeleteQueuedSavingsPlan), tags it through its real ARN and reads the tags back — all of which pass today.
3. Call savingsplans TagResource with resourceArn="not-an-arn" and tags={"k":"v"}. Expect HTTP 400 / ValidationException whose Message is `1 validation error detected: Value 'not-an-arn' at 'originalRequest.resourceArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws([-a-z]*)?:[a-z]+:([a-z]{2}-[a-z]+-\\d{1}|):(\\d{12}):savingsplan\\/([0-9a-f]{8}-[0-9a-f]{4}-[1-5][0-9a-f]{3}-[89ab][0-9a-f]{3}-[0-9a-f]{12})$`. The emulator answers the same status and code but quotes `^arn:aws:[a-z]+:...` — the `([-a-z]*)?` partition group is absent — and the check fails on the message assertion.
4. Confirm the enforcement half of the same cause: call TagResource with a resourceArn in a suffixed partition, e.g. `arn:aws-us-gov:savingsplans::<account-id>:savingsplan/<uuid>`. AWS's pattern admits it, so it must get past boundary validation and be refused at lookup with ResourceNotFoundException / `Resource with resourceArn: <arn> doesn't exist`; the emulator refuses it at the boundary with ValidationException instead. The same must hold for UntagResource and ListTagsForResource, which share the shape.
5. After the fix, re-replay savingsplans.tag-resource-malformed-arn-rejected: it must pass end to end — the positive TagResource/ListTagsForResource control, the malformed-ARN ValidationException with AWS's exact message and pattern, and the well-formed-but-missing-ARN ResourceNotFoundException with its exact message.
