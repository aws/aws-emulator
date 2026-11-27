Issue-Source: trace-sweep-divergence
Issue-Trace: firehose-create-delivery-stream-prefix-expression-rules
Issue-Step: task#7
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: firehose-create-delivery-stream-prefix-expression-rules
Issue-Shape-Id: com.amazonaws.firehose#Firehose_20150804
Issue-Key: firehose|DescribeDeliveryStream|4xx->2xx|$.__type
Issue-Key: firehose|DescribeDeliveryStream|4xx->2xx|$.message
Issue-Key: firehose|DescribeDeliveryStream|4xx->2xx|status
Issue-Key: firehose|DeleteDeliveryStream|4xx->2xx|status
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# firehose serves a delivery stream AWS never created, so Describe and Delete answer 200 instead of ResourceNotFoundException

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeDeliveryStream

## Description
Two net-new root causes with one shared fault: the emulator keeps a delivery stream that AWS refused to create, so the follow-up lookups of that name — which AWS answers with a not-found refusal, because the name was never registered — are answered as if the stream existed. firehose is bootstrapped (get_bootstrap_status reports service-com-amazonaws-firehose-firehose-20150804), so these are fidelity findings, not bootstrap gaps.

Context for a fixer: the accompanying CreateDeliveryStream 400->200 cluster in this same fixture is already OWNED by 20261124T080946Z-firehose-iceberg-kfi-create-and-update-validation-diverges.md and the replay report marked it so, so it is not re-filed here. The two causes below carry their OWN replay keys, which that issue does not, and they are the state-side consequence: even once the create validation is restored, the lookup paths must answer not-found for a name no accepted create ever registered. The other prefix-expression issue, 20261127T120636Z-firehose-s3-3-net-new-causes-in-custom.md, concerns how an ACCEPTED prefix expression is EVALUATED at delivery time, not whether an invalid one is registered.

Cause 1 of 2: DescribeDeliveryStream on a name whose CreateDeliveryStream AWS rejected returns 200 with a full DeliveryStreamDescription instead of the modeled not-found refusal.
AWS: Smithy: DescribeDeliveryStream's only modeled error is com.amazonaws.firehose#ResourceNotFoundException (errorKind client, httpError 400), and its input DeliveryStreamName is @required with @length min 1 max 64 and @pattern ^[a-zA-Z0-9_.-]+$. Message text is recorded behavior only: a name that does not exist in the caller's account is refused with HTTP 400, __type "ResourceNotFoundException" and the message template "Firehose under account <accountId> not found." — fixed text "Firehose under account " + " not found." (terminal period included) with ONE variable part, the 12-digit account id of the calling credentials (the recording shows 000000000000 because that is the emulator's account; the account id is a per-environment value, not part of the requirement). AWS reaches this state because the preceding CreateDeliveryStream, carrying an invalid custom S3 prefix expression, was refused — so the name was never registered.
Emulator: the same DescribeDeliveryStream returns HTTP 200 with a DeliveryStreamDescription body and no __type or message at all.
Why: existence is the primary thing a client asks Describe. A caller that polls Describe to confirm whether a create succeeded concludes it did; orchestration that treats ResourceNotFoundException as "safe to create" never retries, and a validation test that asserts the rejected name is absent passes against the emulator for the wrong reason. Note that even on the paths where the emulator DOES refuse, its not-found message is independently misworded (see 20261028T075033Z); here no refusal is produced at all.
Fix: the firehose delivery-stream state — a create that is refused must leave NO stream registered, and DescribeDeliveryStream's state lookup must raise ResourceNotFoundException with the AWS template for an unregistered name.
Traces: firehose-create-delivery-stream-prefix-expression-rules#task#7, and the identical mismatch at task#9, task#11, task#13, task#15 and task#17 of that fixture (one cluster, affected_count 6 — one describe per rejected prefix form).

Cause 2 of 2: DeleteDeliveryStream on that same never-created name returns 200 instead of a 400 refusal.
AWS: Smithy: DeleteDeliveryStream models ResourceNotFoundException (client, httpError 400) and ResourceInUseException (client, httpError 400); its output is empty, so a 200 is indistinguishable from a successful delete. Recorded behavior: deleting a name that was never registered is refused with HTTP 400 — the step records the refusal status, and ResourceNotFoundException is the operation's modeled not-found error, whose message follows the same "Firehose under account <accountId> not found." template as Cause 1.
Emulator: the delete is answered 200, because the phantom stream from the refused create is present and gets removed.
Why: delete is not idempotent in AWO Firehose — a 200 tells a client the named stream existed and has been torn down. Cleanup logic that counts deleted resources, or that distinguishes "already gone" from "deleted now" by catching ResourceNotFoundException, draws the opposite conclusion against the emulator, and the divergence masks the create-validation gap rather than surfacing it.
Fix: same component as Cause 1 — with the refused create leaving no state, DeleteDeliveryStream's lookup must raise ResourceNotFoundException (HTTP 400) for the unregistered name.
Traces: firehose-create-delivery-stream-prefix-expression-rules#teardown#20, and the same mismatch at teardown#22, teardown#24, teardown#26, teardown#28 and teardown#30 of that fixture (one cluster, affected_count 6).

## Evidence
Replayed against the emulator booted from this checkout; both are verdict=mismatch in fixture firehose-create-delivery-stream-prefix-expression-rules.

Cause 1 — step task#7, operation DescribeDeliveryStream, expected_status 400, actual_status 200: "status: expected 400, got 200"; "$.__type: missing (expected 'ResourceNotFoundException', got None)"; "$.message: missing (expected 'Firehose under account 000000000000 not found.', got None)". divergence_key service=firehose|DescribeDeliveryStream|status=400->200|keys=$.__type,$.message,status; affected_count 6 (task#7, task#9, task#11, task#13, task#15, task#17). In the expected message, "Firehose under account " and " not found." are the fixed text and the 12-digit account id is the only variable part, taken from the calling credentials.

Cause 2 — step teardown#20, operation DeleteDeliveryStream, expected_status 400, actual_status 200: "status: expected 400, got 200". divergence_key service=firehose|DeleteDeliveryStream|status=400->200|keys=status; affected_count 6 (teardown#20, #22, #24, #26, #28, #30). Only the status is asserted on these steps; the error code attribution (ResourceNotFoundException) is from the Smithy model's error list for DeleteDeliveryStream.

Upstream context (NOT re-filed, already owned): the CreateDeliveryStream steps of this fixture (task#6/8/10/12/14/16) return 200 where AWS returns 400 InvalidArgumentException; that cluster is marked owned_by 20261124T080946Z-firehose-iceberg-kfi-create-and-update-validation-diverges.md. The generated stream names and the 000000000000 account id are per-run/per-environment values.

## Steps to Reproduce
1. Build and boot the emulator from this checkout and point a firehose client at its data listener.
2. Call CreateDeliveryStream with DeliveryStreamType=DirectPut and an ExtendedS3DestinationConfiguration whose Prefix is an invalid custom prefix expression (one of the malformed !{...} forms AWS refuses with 400 InvalidArgumentException). The emulator answers 200 and registers the stream.
3. Cause 1: call DescribeDeliveryStream with that same DeliveryStreamName — the emulator returns 200 with a DeliveryStreamDescription, where AWS returns HTTP 400 with __type ResourceNotFoundException and message "Firehose under account <accountId> not found." because the create was refused and the name was never registered.
4. Cause 2: call DeleteDeliveryStream with that name — the emulator returns 200 (having deleted the phantom stream), where AWS returns HTTP 400 ResourceNotFoundException.
5. Or replay the wire corpus and re-run the fixture steps listed in Issue-Trace/Issue-Step and Issue-Trace-Ids.
