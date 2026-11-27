Issue-Source: trace-sweep-divergence
Issue-Trace: api-batch-check-layer-availability-digest-failures
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: api-batch-check-layer-availability-digest-failures api-batch-get-image-identifier-failures
Issue-Shape-Id: com.amazonaws.ecr#AmazonEC2ContainerRegistry_V20150921
Issue-Key: ecr|BatchCheckLayerAvailability|2xx->2xx|$.failures[*].failureReason
Issue-Key: ecr|BatchGetImage|2xx->2xx|$.failures[*].failureReason
Issue-Commit: ae5d8a902292096d9ed4c36fc40ed704ee301e98

# ecr batch-operation failureReason text is emulator-authored instead of AWS wording

> _Note: this issue was identified against the code base at commit `ae5d8a902292096d9ed4c36fc40ed704ee301e98`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchCheckLayerAvailability

## Description
ECR's batch operations do not raise an error for a bad per-item identifier: they answer 200 and report the item under `failures[]` with a `failureCode` and a `failureReason`. The emulator picks the right status and the right `failureCode`, but writes its OWN prose into `failureReason` instead of the fixed text AWS uses for that failure class. The reason string is part of the response contract for these operations (it is the only human-readable explanation a client gets, since the call itself succeeded), so the wording is a requirement, not cosmetic. 2 net-new causes, both the same fault in two different batch handlers.

Cause 1 of 2: BatchCheckLayerAvailability, a layerDigest that is not of the form <algorithm>:<hex>.
  AWS: Smithy: BatchCheckLayerAvailability.layerDigests -> BatchedOperationLayerDigestList (required, 1..100) of BatchedOperationLayerDigest (string, length 0..1000 — the model imposes NO digest pattern, so the malformed-digest check is a handler/validation rule, not a constraint trait). Output member `failures` -> LayerFailureList of LayerFailure{layerDigest, failureCode: LayerFailureCode, failureReason: LayerFailureReason}; LayerFailureCode values are exactly [InvalidLayerDigest, MissingLayerDigest]. The model documents failureReason only as "The reason for the failure", so the exact text is recorded behavior only. The recorded response is HTTP 200 with failures[0].failureReason = "Invalid request parameters: layer digest does not match expected regex." This string is entirely FIXED text for the InvalidLayerDigest class — it interpolates nothing from the request, not even the offending digest (the digest is carried separately in failures[0].layerDigest).
  Emulator: HTTP 200 (correct) but failures[0].failureReason = "'29a539533a52add2666af72bc2719758ed46f9b9753f8806e47636af202be4a1' is not a layer digest of the form <algorithm>:<hex>". Wrong wording, and it additionally interpolates the submitted digest, which AWS's message for this class never does.
  Why: a client that branches or asserts on the reason text for a malformed digest (push tooling that distinguishes "client sent a bad digest" from the other layer failures, log scrapers, test suites pinned to AWS's reason) never matches, and because the emulator's variant embeds a per-request digest, no client can match it with a constant string at all.
  Fix: the BatchCheckLayerAvailability handler's per-digest validation, where the LayerFailure for failureCode InvalidLayerDigest is constructed — emit AWS's fixed reason text for the class and leave the offending digest in the `layerDigest` member only.
  Traces: api-batch-check-layer-availability-digest-failures#task#5 (also #task#6).

Cause 2 of 2: BatchGetImage, an imageIds entry carrying neither imageDigest nor imageTag.
  AWS: Smithy: BatchGetImage.imageIds -> ImageIdentifierList (required, 1..100) of ImageIdentifier{imageDigest, imageTag} — BOTH members are optional in the model, so "at least one must be present" is a handler rule reported per item. Output member `failures` -> ImageFailureList of ImageFailure{imageId, failureCode: ImageFailureCode, failureReason: ImageFailureReason}; ImageFailureCode includes MissingDigestAndTag. The exact reason text is recorded behavior only: the recorded response is HTTP 200 with failures[0].failureReason = "Invalid request parameters: both tag and digest cannot be null". Again wholly FIXED text — no request value is interpolated (the empty identifier is echoed in failures[0].imageId).
  Emulator: HTTP 200 (correct) but failures[0].failureReason = "Invalid request parameters: at least one of [imageDigest, imageTag] must be specified". Same leading "Invalid request parameters: " prefix, different and emulator-authored remainder.
  Why: the same mismatch as cause 1 — a client keyed to AWS's reason for an under-specified image identifier sees unknown prose and cannot distinguish this failure from any other 200-with-failures outcome by text.
  Fix: the BatchGetImage handler's per-imageId validation, where the ImageFailure for failureCode MissingDigestAndTag is constructed.
  Traces: api-batch-get-image-identifier-failures#task#7 (also #task#8, #task#9).

No open issue covers either cause: the open ECR message-text issues concern unrelated surfaces (e.g. 20261117T115047Z-ec2-ecr-ecs-ecr-s-imagenotfoundexception-for-a.md covers ImageNotFoundException's placeholder digest, not these batch failureReason strings).

## Evidence
Cause 1 — BatchCheckLayerAvailability, fixture api-batch-check-layer-availability-digest-failures step task#5 (cluster also covers task#6):
  status: expected 200, got 200 (OK)
  $.failures[0].failureReason: value_mismatch
    expected: 'Invalid request parameters: layer digest does not match expected regex.'
    actual:   "'29a539533a52add2666af72bc2719758ed46f9b9753f8806e47636af202be4a1' is not a layer digest of the form <algorithm>:<hex>"
  Fixed text (the requirement): the entire expected string. Request/state-derived parts: none — the emulator's inclusion of the submitted digest is itself part of the divergence.

Cause 2 — BatchGetImage, fixture api-batch-get-image-identifier-failures step task#7 (cluster also covers task#8, task#9):
  status: expected 200, got 200 (OK)
  $.failures[0].failureReason: value_mismatch
    expected: 'Invalid request parameters: both tag and digest cannot be null'
    actual:   'Invalid request parameters: at least one of [imageDigest, imageTag] must be specified'
  Fixed text (the requirement): the entire expected string. Request/state-derived parts: none.

Model evidence: Smithy LayerFailureCode = [InvalidLayerDigest, MissingLayerDigest]; Smithy ImageFailureCode includes MissingDigestAndTag; LayerFailure.failureReason / ImageFailure.failureReason are documented only as "The reason for the failure", so the text itself is grounded in the recording.

## Steps to Reproduce
1. Clone the repo and boot the emulator built from it; replay the wire corpus (replay_wire_corpus).
2. Cause 1: replay fixture api-batch-check-layer-availability-digest-failures, step task#5. It calls CreateRepository then BatchCheckLayerAvailability with a layerDigests entry that is a bare hex string with no '<algorithm>:' prefix (e.g. '29a539533a52add2666af72bc2719758ed46f9b9753f8806e47636af202be4a1'). Response is 200; read $.failures[0].failureCode (InvalidLayerDigest) and $.failures[0].failureReason — the reason is the emulator's own sentence rather than 'Invalid request parameters: layer digest does not match expected regex.'. Step task#6 of the same fixture repeats it.
3. Cause 2: replay fixture api-batch-get-image-identifier-failures, step task#7. It calls BatchGetImage with an imageIds entry that sets neither imageDigest nor imageTag. Response is 200; read $.failures[0].failureCode (MissingDigestAndTag) and $.failures[0].failureReason — the reason is 'Invalid request parameters: at least one of [imageDigest, imageTag] must be specified' instead of 'Invalid request parameters: both tag and digest cannot be null'. Steps task#8 and task#9 repeat it.
4. Re-replay both fixtures after the fix; every step in both clusters must pass.
