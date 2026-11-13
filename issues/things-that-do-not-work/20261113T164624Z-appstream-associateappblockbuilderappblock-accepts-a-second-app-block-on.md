Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.appblockbuilder-appblock-association-lifecycle
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.appblockbuilder-appblock-association-lifecycle|_error#13|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Commit: ad929e8a332c1e810b12039ddf4f335962289812

# appstream AssociateAppBlockBuilderAppBlock accepts a second app block on a builder that already has one (HTTP 200) instead of refusing it OperationNotPermittedException (1 cause)

> _Note: this issue was identified against the code base at commit `ad929e8a332c1e810b12039ddf4f335962289812`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAppBlockBuilderAppBlock

## Description
One net-new root cause in the AppStream app-block-builder association surface, grounded in the failing behavioral check listed under Traces.

Context on related open issues (neither covers this cause): 20261106T134718Z-appstream-associateappblockbuilderappblock-stub.md and 20261109T175829Z-appstream-associateappblockbuilderappblock-answers-501-notimplemented-instead-of-refusing.md both describe the operation while it was still a 501 scaffold stub (the second also records the CUSTOM-packaging refusal). The handler is now implemented and answers 200, and the divergence recorded here is a DIFFERENT, previously unreachable gate — the one-app-block-per-builder cardinality rule — which neither issue describes and whose replay key neither carries. This is not a reopen: the call is not refused, it over-permissively succeeds.

Cause 1 of 1: AssociateAppBlockBuilderAppBlock does not enforce the one-associated-app-block-per-app-block-builder limit.
AWS: Smithy: AssociateAppBlockBuilderAppBlock(AppBlockArn: Arn required, AppBlockBuilderName: Name required) models ConcurrentModificationException, InvalidParameterCombinationException, LimitExceededException, OperationNotPermittedException (all httpError 400) and ResourceNotFoundException (404); OperationNotPermittedException is documented "The attempted operation is not permitted." The cardinality rule itself is recorded behavior only (the model states no cardinality constraint): an app block builder holds AT MOST ONE associated app block, so a request to associate app block B while app block A is still associated with that builder is refused with HTTP 400, error code OperationNotPermittedException, message `The selected App block builder can not be associated with additional App block`. That message is entirely FIXED text — it takes no name, ARN or value from the request or from state. The refusal has NO effect: an independent DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=<builder>) still reports exactly the one pre-existing association (A), never B. The rule is a live cardinality check, not a permanent ban: after DisassociateAppBlockBuilderAppBlock(AppBlockArn=<A>, AppBlockBuilderName=<builder>) the builder reads back with no associations, and the IDENTICAL associate-B call that was just refused then SUCCEEDS with HTTP 200 and the builder reads back associated with B only. Positive control: associating the first app block onto a bare builder succeeds.
Emulator: the second associate SUCCEEDS with HTTP 200 and returns an AppBlockBuilderAppBlockAssociation echoing B's AppBlockArn and the builder name, so the builder ends up carrying TWO associations at once — a state AWS never produces. Recorded replay: `expected the call to be rejected, but it SUCCEEDED and returned {'AppBlockBuilderAppBlockAssociation': {'AppBlockArn': ..., 'AppBlockBuilderName': ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}`. The handler checks only that both endpoints exist, that the app block's PackagingType is APPSTREAM2, and a store-size ceiling, and then inserts the (builder, arn) edge into a SET — a set keyed by the pair permits any number of distinct app blocks per builder, so no cardinality gate exists anywhere on the path.
Why: a client that relies on AWS's cardinality rule gets the opposite answer in both directions. It sees no 400 OperationNotPermittedException where AWS refuses, so code that catches that error to detach the current app block before attaching a new one never runs, and the builder silently accumulates multiple associations; a subsequent read returns a set of associations that no real builder can be in, and any logic that assumes "a builder has one app block" (picking associations[0], or diffing desired vs actual) diverges from AWS. The restore half of the rule is equally unverifiable: nothing distinguishes "refused because the slot is taken" from "always allowed", so the disassociate-then-associate workflow is untested against the emulator.
Fix: the AssociateAppBlockBuilderAppBlock handler (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/associate_app_block_builder_app_block.rs, handle() around lines 206-231, where the edge is inserted after only the packaging and store-ceiling checks) — before inserting, look up whether the named builder already has an association to a DIFFERENT AppBlockArn and, if so, refuse with the modeled OperationNotPermittedException and AWS's fixed sentence, touching no state. Re-associating the SAME pair must stay non-refusing/idempotent as today, and DisassociateAppBlockBuilderAppBlock must free the slot so the identical associate then succeeds and projects from DescribeAppBlockBuilderAppBlockAssociations.
Traces: appstream.appblockbuilder-appblock-association-lifecycle.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (appstream 19/24 passed).

[mismatch] appstream.appblockbuilder-appblock-association-lifecycle
  check_appstream_appblockbuilder_appblock_association_lifecycle.py line 61 -> _error() line 213:
  AssertionError: expected the call to be rejected, but it SUCCEEDED and returned
  {'AppBlockBuilderAppBlockAssociation': {'AppBlockArn': 'arn:aws:appstream:<region>:<account>:app-block/<appblock-b-name>', 'AppBlockBuilderName': '<builder-name>'}, 'ResponseMetadata': {'HTTPStatusCode': 200, 'HTTPHeaders': {'content-type': 'application/x-amz-json-1.1', ...}, 'RetryAttempts': 0}}

  Expected (AWS): HTTP 400, Error.Code = OperationNotPermittedException, Error.Message = "The selected App block builder can not be associated with additional App block" (fixed text in full; no variable part). Effect absent: DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=<builder>) still returns only {A}. Then DisassociateAppBlockBuilderAppBlock(A) -> that read returns [] , and the identical AssociateAppBlockBuilderAppBlock(B) -> HTTP 200 with the read then returning only {B}.
  Actual (emulator): HTTP 200 with the association for B; the builder now reports both A and B.

  Handler evidence: services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/associate_app_block_builder_app_block.rs handle() performs a builder-exists check (lines ~164-179), an app-block-exists check (~180-191), a PackagingType != APPSTREAM2 refusal (~199-204) and a MAX_RECORDS_PER_STORE ceiling (~214-230), then `state.app_block_builder_app_block_associations.insert(edge)` (line 231) where `edge = (builder_name, app_block_arn)` — a per-pair set, so a builder may hold unlimited distinct app blocks.

ARNs, account numbers, builder and app-block names and timestamps above are per-run placeholders; the requirements are the HTTP status, the error code, the fixed message text and the read-back effects.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create a VPC with two subnets and an S3 asset bucket; CreateAppBlockBuilder(Name=B, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, VpcConfig={SubnetIds:[s1,s2]}).
3. CreateAppBlock(Name=A1, PackagingType=APPSTREAM2, SourceS3Location={S3Bucket:<bucket>, S3Key:src/a.zip}) and CreateAppBlock(Name=A2, PackagingType=APPSTREAM2, SourceS3Location={S3Bucket:<bucket>, S3Key:src/b.zip}); note their Arns as A and B.
4. Positive control: AssociateAppBlockBuilderAppBlock(AppBlockArn=A, AppBlockBuilderName=B) -> HTTP 200; DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=B) reports {A}.
5. The divergence: AssociateAppBlockBuilderAppBlock(AppBlockArn=B_arn, AppBlockBuilderName=B) while A is still attached. Emulator answers HTTP 200 and returns the association; AWS answers HTTP 400 OperationNotPermittedException "The selected App block builder can not be associated with additional App block" and leaves the builder associated with A only.
6. DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=B): emulator reports both A and B_arn; AWS reports only A.
7. Restore half: DisassociateAppBlockBuilderAppBlock(AppBlockArn=A, AppBlockBuilderName=B) -> the read returns []; then the identical AssociateAppBlockBuilderAppBlock(AppBlockArn=B_arn, AppBlockBuilderName=B) must succeed with HTTP 200 and the read must report only B_arn.
8. Or replay the check in trace_ids.
