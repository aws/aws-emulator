Issue-Source: trace-sweep-divergence
Issue-Trace: savingsplans-queued-plan-delete-lifecycle
Issue-Step: task#5
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: savingsplans-queued-plan-delete-lifecycle
Issue-Shape-Id: com.amazonaws.savingsplans#AWSSavingsPlan
Issue-Key: savingsplans|ListTagsForResource|2xx->2xx|$.tags.aws:tag:domain
Issue-Commit: 22f94653eba30c921a9f5299a2c0aa92b971d5f1

# savingsplans drops reserved aws:-prefixed tag keys from the plan tag set

> _Note: this issue was identified against the code base at commit `22f94653eba30c921a9f5299a2c0aa92b971d5f1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListTagsForResource

## Description
Replay of the savingsplans wire corpus (simparity-wire-aws-v1, 6 fixtures / 27 steps) shows ONE net-new cause in the service-com-amazonaws-savingsplans-awssavingsplan crate; it must be closed before this issue is submitted.

Cause 1 of 1: ListTagsForResource — a tag key carrying the reserved 'aws:' prefix is accepted on write but never returned.

AWS: Smithy: ListTagsForResource input resourceArn (SavingsPlanArn, @required, pattern '^arn:aws([-a-z]*)?:[a-z]+:(<region>|):<account>:savingsplan/<uuid>$'), output tags (com.amazonaws.savingsplans#TagMap, key TagKey -> value TagValue, no length/pattern constraints on either); errors ResourceNotFoundException (404), ValidationException (400), InternalServerException (500). TagResource takes the same TagMap with tags @required. The model places NO restriction on tag keys, so for the reserved-prefix behavior this is recorded behavior only: the recording shows AWS storing and returning the plan's tag set VERBATIM, including a key spelled with the reserved 'aws:' prefix ('aws:tag:<name>' -> its value), on an HTTP 200 answer. The key is echoed exactly as it was written on the plan, and the write that set it succeeded.

Emulator: HTTP 200 (status matches) but the returned tags map OMITS the reserved-prefix entry entirely — the expected key/value pair is absent from the body (expected the written value, got None), while every non-reserved key on the same plan comes back. The preceding write returned success, so the tag is silently discarded rather than refused. Hypothesis: the tag-ingestion path filters keys beginning with 'aws:' before merging them into the plan record's tag set (the crate's check_tags in the service state validates only tag count and key/value lengths, so the drop happens outside it).

Why: a client that tags a Savings Plan and reads the tags back sees a tag it successfully wrote disappear. Tag-driven cost allocation, resource filtering and tag-based authorization all mis-evaluate against the truncated set, and because the write reported 200 the client has no way to detect that the state was never kept — the failure is silent and unrecoverable from the client side.

Fix: the tag write path and the ListTagsForResource projection — the TagResource handler and CreateSavingsPlan's tags ingestion into the plan record's tag set must store every supplied key verbatim, including reserved-prefix keys, and ListTagsForResource must project the stored set unfiltered; alternatively refuse the write outright (modeled ValidationException, 400) instead of accepting it and discarding the entry.

Related but not covering: open issue 20261029T203239Z-savingsplans-handlers-diverge-from-recorded-aws-answers-in.md discusses the savingsplans handler divergences generally, but the replay reports this cluster as unowned (owned_by empty) — its replay key for ListTagsForResource is recorded only as the truncated '$.tags.aws' and does not bind this cluster's '$.tags.aws:tag:<name>' key, so this divergence is filed here with its own fixture/step linkage.

Traces: savingsplans-queued-plan-delete-lifecycle#task#5 (same cause recurs in the same cluster at savingsplans-queued-plan-delete-lifecycle#task#7).

## Evidence
Replay summary: trace_set_id=simparity-wire-aws-v1, emulator_up=true, bootstrap_signal=false, steps_total=27, passed=19, mismatched=8, errored=0, blocked=0, score_pct=70.4, divergence_count=7.

Cluster (index 4): service=savingsplans, operation=ListTagsForResource, verdict=mismatch, divergence_key 'service=savingsplans|ListTagsForResource|status=200->200|keys=$.tags.aws:tag:domain', stub_path empty, owned_by empty (NOT already filed).
Expected status 200, actual status 200.
detail: "$.tags.aws:tag:domain: missing (expected 'prod', got None)" — i.e. the reserved-prefix tag key written earlier in the fixture is absent from the emulator's tags map while AWS returns it.
Fixed vs variable: the requirement is that the key is returned byte-for-byte as supplied and mapped to the value supplied; the particular key spelling and value in the recording ('aws:tag:domain' -> 'prod') are request values filling that template, not part of it.
affected (affected_count=1): savingsplans-queued-plan-delete-lifecycle#task#2 is the cluster's second member at savingsplans-queued-plan-delete-lifecycle#task#7 (same operation, same missing key).
No per-run values (request ids, clocks, generated plan ids) are part of this finding.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from that checkout; replay the simparity-wire-aws-v1 wire corpus (6 savingsplans fixtures, 27 steps).
2. Replay fixture savingsplans-queued-plan-delete-lifecycle, which purchases a plan and writes a tag set on it that includes a key with the reserved 'aws:' prefix (the write answers 200).
3. Compare step task#5 (ListTagsForResource on that plan's ARN): the response is 200, but the tags map must contain the reserved-prefix key with the value that was written; the emulator omits it.
4. Compare step task#7 (ListTagsForResource again, later in the lifecycle): same omission.
5. Re-replay the fixture after the fix; steps task#5 and task#7 must pass.
