Issue-Source: trace-sweep-divergence
Issue-Trace: opensearchserverless-vpc-endpoint-errors
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: opensearchserverless-vpc-endpoint-errors
Issue-Shape-Id: com.amazonaws.opensearchserverless#OpenSearchServerless
Issue-Key: aoss|CreateVpcEndpoint|4xx->4xx|$.message
Issue-Commit: ff0a2433ea076228f93b8b79ef2b1e947e770a3c

# aoss: CreateVpcEndpoint's absent-VPC ValidationException states an emulator-authored sentence

> _Note: this issue was identified against the code base at commit `ff0a2433ea076228f93b8b79ef2b1e947e770a3c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateVpcEndpoint

## Description
This issue carries 1 net-new cause on the `aoss` (OpenSearch Serverless) managed-VPC-endpoint surface.

--- Cause 1 of 1: the ValidationException message for a `vpcId` that names no VPC is the emulator's own sentence, not OpenSearch Serverless's ---

AWS: `CreateVpcEndpoint` creates an OpenSearch Serverless-managed interface VPC endpoint. Smithy (services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/com-amazonaws-opensearchserverless-opensearchserverless.json, OpenSearchServerless, awsJson1_0, sigv4Name aoss): input `name` (VpcEndpointName, @length 3..32, @pattern ^[a-z][a-z0-9-]+$) required, `vpcId` (VpcId, @length 1..255, @pattern ^vpc-[0-9a-z]*$) required, `subnetIds` (SubnetIds, @length min 1) required, `securityGroupIds` (@length 1..5), `clientToken` (@idempotencyToken, @length 1..512); errors ValidationException (@httpError 400, "Thrown when the HTTP request contains invalid input or is missing required input."), ConflictException (409), ServiceQuotaExceededException (402), InternalServerException (500). The model fixes the refusal CLASS but states no sentence for it, and `ValidationException` carries a single member, `message` (String) — no field/member list — so the message text is the ONLY place the refusal says which input was bad. For the text itself this is recorded behavior only: the recording pins the refusal at HTTP 400 with a `message` member and asserts that member's SHAPE while eliding its literal value (`<shaped-not-asserted>`), and the replay reports the emulator's sentence as not satisfying it. The requirement is therefore AWS's own wording for this error class — its fixed text exactly as the service words a `vpcId` that resolves to no VPC in the caller's account and Region, with the single variable part (the VPC id) interpolated from the request.

Emulator: the step is answered HTTP 400 (the status and the `ValidationException` class are correct — this is a message-text divergence only) with `message` = "The Vpc with Id 'vpc-00000000000000000' does not exist.". Replay detail: `$.message: value_mismatch (expected '<shaped-not-asserted>', got "The Vpc with Id 'vpc-00000000000000000' does not exist.")`; divergence_key `service=aoss|CreateVpcEndpoint|status=400->400|keys=$.message`. In that sentence the fixed text is "The Vpc with Id '" + "' does not exist." and the only variable part is the VPC id copied from the request — the recorded id `vpc-00000000000000000` is a per-run request value and is NOT part of the requirement. The sentence is minted by `vpc_absent_message` in services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state/ec2_network.rs and reached from the placement gate in services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_vpc_endpoint.rs. Its own doc comment concedes the wording is not grounded in any recording ("NOT re-grounded against the recording: the wire corpus elides this refusal's recorded `message` (`<shaped-not-asserted>`) ... left as it stands rather than replaced by a guess"): it is the crate's generic `The <Kind> with Id '<id>' does not exist.` form, reused from the absent-collection sentence and reading like Amazon EC2's `InvalidVpcID.NotFound` phrasing rather than an OpenSearch Serverless `ValidationException`.

Why: a client relying on AWS's behavior gets the right status and error class but the wrong sentence, which is the one part of this refusal that identifies the offending input. SDK integration tests and acceptance suites that assert the refusal text, operator runbooks and log/alarm patterns written against AWS's wording, and any client that parses the message to tell an absent `vpcId` from an absent `subnetIds` entry (both of which the emulator words with the SAME template, differing only in the resource word) all diverge — silently, because status, error shape and code all look right.

Fix: `CreateVpcEndpoint`'s refusal path — specifically the message template `vpc_absent_message` (and the sibling `subnet_absent_message` that shares its form) in the service's `state::ec2_network` module, as consumed by the placement gate of the `CreateVpcEndpoint` handler and, through `unresolved_network`, by `UpdateVpcEndpoint`'s. The template's fixed text must be AWS's wording for an OpenSearch Serverless `ValidationException` about a `vpcId` that names no VPC, with the VPC id taken from the request; the gate's placement logic itself (resolve the VPC against EC2 before minting an id) is already in the right place.

Traces: `opensearchserverless-vpc-endpoint-errors#task#1` (expected 400 ValidationException with the recorded message shape, got 400 with the emulator-authored sentence).

Related issues, neither of which covers this cause: the open `20261122T183952Z-aoss-opensearchserverless-access-policy-created-with-a-clienttoken-cannot.md` (GetAccessPolicy/DeleteAccessPolicy answer 400 — a different operation pair, and it owns the other two divergences of this replay by replay key); and the earlier aoss CreateVpcEndpoint wording/validation issues `20261029T135112Z-opensearchserverless-createvpcendpoint-error-response.md` and `20261029T105947Z-opensearchserverless-createvpcendpoint-validation.md`, which are RESOLVED (and so cover nothing) and in any case concern the subnet-list length refusal rendering and the repeated-id SET invariant, not the absent-VPC sentence. The replay reports this cluster with an EMPTY `owned_by`, i.e. no open issue carries its replay key.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (branch issues/finder-wi-53e5df88434043fa8efe6700a12a6990-1795718269339): emulator_up true (/__admin/health answered 200 and the data listener accepts), 23 fixtures replayed, 150 steps, 145 passed, 3 mismatched, 2 blocked, 0 errored, score 96.7%, 3 divergence clusters — two of which are already owned by an open issue by replay key, leaving this one net-new.

Cause 1 — fixture `opensearchserverless-vpc-endpoint-errors`, step `task#1`, operation CreateVpcEndpoint, verdict mismatch, owned_by [] (net-new), stub_path empty, affected_count 1:
  expected_status 400, actual_status 400  (the status and the ValidationException class MATCH)
  detail: "$.message: value_mismatch (expected '<shaped-not-asserted>', got \"The Vpc with Id 'vpc-00000000000000000' does not exist.\")"
  divergence_key: service=aoss|CreateVpcEndpoint|status=400->400|keys=$.message

Expected: HTTP 400 `ValidationException` whose `message` is OpenSearch Serverless's own sentence for a `vpcId` that names no VPC — the corpus elides the literal text and asserts the member's shape, and the emulator's value does not satisfy that assertion.
Actual: HTTP 400 `ValidationException` with `message` = "The Vpc with Id 'vpc-00000000000000000' does not exist." — the crate's generic `The <Kind> with Id '<id>' does not exist.` template.

The VPC id `vpc-00000000000000000` is the value this run's request carried and is NOT part of the requirement; the requirement is the template's fixed text (AWS's wording for this error class) plus the VPC id interpolated from the request.

Smithy evidence:
  CreateVpcEndpoint: input {name: VpcEndpointName required len 3..32 pattern ^[a-z][a-z0-9-]+$, vpcId: VpcId required len 1..255 pattern ^vpc-[0-9a-z]*$, subnetIds: SubnetIds required len min 1, securityGroupIds len 1..5, clientToken @idempotencyToken len 1..512}; output {createVpcEndpointDetail}; errors ConflictException(409), InternalServerException(500), ServiceQuotaExceededException(402), ValidationException(400); @idempotent.
  ValidationException: structure, client error, @httpError 400, members {message: String}, doc "Thrown when the HTTP request contains invalid input or is missing required input." — so `message` is the only channel that can name the offending input.

Emulator code the cause lands in:
  services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/state/ec2_network.rs — `vpc_absent_message` (line 101-103) formats `The Vpc with Id '{vpc_id}' does not exist.`; its doc (lines 98-100) states the sentence was NOT grounded against the recording because the corpus elides the recorded message.
  services/service-com-amazonaws-opensearchserverless-opensearchserverless/src/ops/create_vpc_endpoint.rs — lines 214-228 call `state::ec2_network::placement(...)` and lift `placement.refusal` straight into `ValidationException { message }`.

## Steps to Reproduce
1. Clone the repo and build/boot the emulator from this checkout; confirm the aoss listener answers (/__admin/health returned 200 during this replay).
2. Replay the wire fixture `opensearchserverless-vpc-endpoint-errors` from trace set `simparity-wire-aws-v1`.
3. Step `task#1` calls CreateVpcEndpoint (awsJson1_0, X-Amz-Target OpenSearchServerless.CreateVpcEndpoint) with a well-formed `name`, a `subnetIds` list, and a `vpcId` that satisfies VpcId's @length 1..255 and @pattern ^vpc-[0-9a-z]*$ but names no VPC in the emulator's EC2 state (the recording uses `vpc-00000000000000000`).
4. Observe HTTP 400 `ValidationException` — the correct status and class — with body `{"message": "The Vpc with Id '<the requested vpcId>' does not exist."}`.
5. Compare against the recording: the corpus asserts the shape of that `message` (its literal text elided as `<shaped-not-asserted>`) and the emulator's sentence does not satisfy it; replay reports `$.message: value_mismatch`, divergence_key `service=aoss|CreateVpcEndpoint|status=400->400|keys=$.message`.
6. Equivalent by hand: `aws opensearchserverless create-vpc-endpoint --name probe-endpoint --vpc-id vpc-00000000000000000 --subnet-ids subnet-0123456789abcdef0` against the emulator and read the message.
7. Re-replay `opensearchserverless-vpc-endpoint-errors#task#1` to confirm a fix. Note the same template also words the absent-subnet refusal (`subnet_absent_message`) and is shared with UpdateVpcEndpoint's placement gate, so check that path does not regress.
