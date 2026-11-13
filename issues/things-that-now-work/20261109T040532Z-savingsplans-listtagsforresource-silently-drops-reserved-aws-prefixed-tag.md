Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-queued-plan-delete-lifecycle
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-queued-plan-delete-lifecycle
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws:tag:domain
Issue-Commit: 264726cd210189fc14dd0f74a077b22d6a361cf8

# savingsplans ListTagsForResource silently drops reserved aws-prefixed tag keys

> _Note: this issue was identified against the code base at commit `264726cd210189fc14dd0f74a077b22d6a361cf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Replay of the savingsplans wire corpus (simparity-wire-aws-v1, 6 fixtures / 27 steps) leaves ONE divergence cluster unowned by any open issue: a tag key carrying the reserved 'aws:' prefix is accepted on write but never returned by ListTagsForResource. The cluster has two members (task#5 and task#7 of the same fixture) and ONE root cause.

Cause 1 of 1: ListTagsForResource omits tag keys whose name begins with the reserved 'aws:' prefix.
AWS: Smithy: ListTagsForResource input resourceArn (SavingsPlanArn, @required, @pattern '^arn:aws([-a-z]*)?:[a-z]+:(<region>|):<account>:savingsplan/<uuid>$'), output tags -> com.amazonaws.savingsplans#TagMap (map of TagKey -> TagValue, neither carrying any @length or @pattern constraint, so the model places NO restriction on key spelling); errors ResourceNotFoundException (404), ValidationException (400), InternalServerException (500). The model says nothing about reserved prefixes, so for this behavior: recorded behavior only. The recording shows AWS answering HTTP 200 with the plan's tag set returned VERBATIM, including a key of the form 'aws:tag:<name>' with the value that was written on the plan. AWS either preserves the key exactly as supplied or refuses the write; it does not accept a tag write and then hide the tag on read.
Emulator: HTTP 200 with the correct status and envelope, but the tags map OMITS the 'aws:'-prefixed entry entirely: replay reports `$.tags.aws:tag:domain: missing (expected 'prod', got None)`. The preceding write that set that key returned success, so the tag is lost silently rather than rejected. Hypothesis: the tag-ingestion path filters or drops keys with the reserved 'aws:' prefix before merging them into the plan record; the crate's check_tags validation only enforces count/length, so nothing refuses the write.
Why: a client that tags a savings plan and reads the tags back sees a tag it successfully wrote disappear. Cost-allocation reporting, tag-based filtering and tag-conditioned authorization all evaluate against a tag set that is missing entries, and because the write reported 200 the client has no signal that state it asked for was never kept. Read-after-write consistency on the tag set is broken for a whole class of keys.
Fix: the tag write path and its projection — the TagResource handler plus CreateSavingsPlan's tags ingestion into the plan record's tag set, and the ListTagsForResource handler that projects it. Store every supplied key verbatim, including reserved-prefix keys, and return them unfiltered on read; if the emulator intends to reject reserved prefixes, it must fail the write with the modeled ValidationException instead of accepting and discarding it.
Traces: savingsplans-queued-plan-delete-lifecycle#task#5 (same cause recurs at savingsplans-queued-plan-delete-lifecycle#task#7).

Related open issue, which does NOT cover this: 20261029T203239Z-savingsplans-handlers-diverge-from-recorded-aws-answers-in.md describes seven savingsplans causes and its cause 4 is about the same behavior, but its recorded replay key is truncated to 'savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws', which does not match this cluster's key 'service=savingsplans|ListTagsForResource|status=200->200|keys=$.tags.aws:tag:domain'. The replay therefore reports this cluster as owned_by [] — unowned — so its replay gate is not carried by that issue and it is filed here with the exact fixture/step binding.

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=27, passed=19, mismatched=8, errored=0, blocked=0, score_pct=70.4, divergence_count=7. Six of the seven clusters carry owned_by=['20261029T203239Z-savingsplans-handlers-diverge-from-recorded-aws-answers-in.md']; this cluster carries owned_by=[] and stub_path=''.

Cause 1 — fixture savingsplans-queued-plan-delete-lifecycle, step task#5, operation ListTagsForResource, service savingsplans.
  divergence_key: service=savingsplans|ListTagsForResource|status=200->200|keys=$.tags.aws:tag:domain
  expected_status: 200, actual_status: 200 (the status and error code are correct; only the body diverges)
  detail: $.tags.aws:tag:domain: missing (expected 'prod', got None)
  affected (2 members): savingsplans-queued-plan-delete-lifecycle#task#5, savingsplans-queued-plan-delete-lifecycle#task#7

The requirement is structural, not literal: the response's tags map must contain EVERY key written on the plan, spelled exactly as supplied. In the recording the key is 'aws:tag:domain' and the value 'prod' — those are the fixture's values filling the template; the requirement is that a key beginning with 'aws:' survives the write/read round-trip with its value intact, not that this particular key/value pair appear.

Model evidence used: smithy_query operation=ListTagsForResource,TagResource and shape=TagMap,TagKey,TagValue on services/service-com-amazonaws-savingsplans-awssavingsplan/smithy-model/com-amazonaws-savingsplans-awssavingsplan.json. TagMap is an unconstrained map of TagKey->TagValue (no @length, no @pattern on either), so no modeled constraint licenses dropping a key; TagResource declares ValidationException (400) and ServiceQuotaExceededException (402) as its refusal paths, so refusal — not silent discard — is the modeled alternative to storing the tag.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus (6 savingsplans fixtures, 27 steps).
3. Replay fixture savingsplans-queued-plan-delete-lifecycle and compare step task#5: the lifecycle writes a tag whose key uses the reserved 'aws:' prefix (an 'aws:tag:<name>' key) on the plan and the write returns 200, then calls ListTagsForResource on the plan's ARN.
4. Observe the 200 response body: the tags map comes back without the 'aws:'-prefixed entry (replay: $.tags.aws:tag:domain expected 'prod', got None), while the recorded AWS answer returns it with its written value.
5. Compare step task#7 of the same fixture, which repeats ListTagsForResource later in the lifecycle and shows the identical loss.
6. After the fix, re-replay savingsplans-queued-plan-delete-lifecycle; both task#5 and task#7 must pass, i.e. ListTagsForResource must echo every key written on the plan verbatim (or the write must have been refused with ValidationException instead of returning success).
