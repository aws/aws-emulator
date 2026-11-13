Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.appblockbuilder-invalid-role-rejected appstream.appblockbuilder-association-incompatible-packaging-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.appblockbuilder-invalid-role-rejected|_error#7|AssertionError|raise AssertionError(f"expected the call to be rejected, but it SUCCEEDED and returned {result!r}")
Issue-Key: appstream.appblockbuilder-association-incompatible-packaging-rejected|run#14|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Commit: e5ca3506b31b6433866069965c2564d0cdcea940

# appstream app-block-builder divergences: CreateAppBlockBuilder accepts a non-assumable IamRoleArn (200) instead of InvalidRoleException, and AssociateAppBlockBuilderAppBlock answers 501 NotImplemented instead of refusing a CUSTOM-packaged app block (2 causes)

> _Note: this issue was identified against the code base at commit `e5ca3506b31b6433866069965c2564d0cdcea940`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateAppBlockBuilder

## Description
Two net-new root causes in the AppStream app-block-builder surface. Both are grounded in failing behavioral checks of this suite (ids under each cause). Related open issue 20261029T194603Z-appstream-answers-501-notimplemented-where-aws-performs-a.md recorded cause 1 while CreateAppBlockBuilder was still a 501 stub; the handler is now implemented and answers 200, so the divergence has MOVED (an overly-permissive accept, not a 501) and the check now fails on a different assertion — it is restated here with the behavior the implemented handler must carry.

Cause 1 of 2: CreateAppBlockBuilder does not gate IamRoleArn on assumability.
AWS: Smithy: CreateAppBlockBuilder.IamRoleArn targets Arn (pattern ^arn:aws(?:-cn|-iso-b|-iso|-us-gov)?:...$) and the operation models InvalidRoleException (httpError 400) — "The specified role is invalid." A syntactically valid ARN passes the modeled pattern, so assumability is a handler gate. Recorded behavior: a request whose IamRoleArn names a role that does not exist / AppStream cannot assume is refused with HTTP 400, error code InvalidRoleException, message `WorkSpaces Applications encountered an error because your IAM role '<IamRoleArn>' is invalid`. Fixed text: "WorkSpaces Applications encountered an error because your IAM role '" ... "' is invalid" (ARN in single quotes, no trailing period); the only variable part is <IamRoleArn>, echoed from the request. The refusal has no effect: no builder is created, so DescribeAppBlockBuilders(Names=[<Name>]) then answers HTTP 404 ResourceNotFoundException with message `The appblockbuilder <Name> for account <accountId> does not exist.` (fixed text "The appblockbuilder ", " for account ", " does not exist."; variable parts <Name> from the request and <accountId> the caller's account).
Emulator: the call SUCCEEDS with HTTP 200 and returns the AppBlockBuilder (State STOPPED) with the bogus IamRoleArn echoed back, and the builder is persisted and describable. services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/create_app_block_builder.rs builds the AppBlockBuilderRecord straight from the input (iam_role_arn: input.iam_role_arn) with only a duplicate-name and a store-size check before it — there is no role validation anywhere in handle().
Why: a client that provisions a builder with a mistyped or not-yet-created role gets a 200 and a builder that can never stream, instead of the modeled 400 it is written to handle; worse, the suite's own retry loops (and real callers) retry CreateAppBlockBuilder on InvalidRoleException while an IAM role propagates, and an unconditional 200 turns a misconfiguration into a silently broken resource. Negative-class coverage for this operation is absent, so nothing distinguishes a good role from a nonexistent one.
Fix: the CreateAppBlockBuilder handler — add the IamRoleArn assumability gate (reject an ARN that names no role resolvable in the emulator's IAM state) returning InvalidRoleException with AWS's sentence, and ensure the rejected request persists nothing so the subsequent DescribeAppBlockBuilders answers ResourceNotFoundException.
Traces: appstream.appblockbuilder-invalid-role-rejected.

Cause 2 of 2: AssociateAppBlockBuilderAppBlock does not exist, so the packaging-compatibility refusal and the association read are both unobservable.
AWS: Smithy: AssociateAppBlockBuilderAppBlock requires AppBlockArn and AppBlockBuilderName and models ConcurrentModificationException, InvalidParameterCombinationException, LimitExceededException, OperationNotPermittedException (all 400) and ResourceNotFoundException (404). The packaging rule itself is recorded behavior only (the model does not name InvalidParameterValueException for this operation): associating an app block whose PackagingType is CUSTOM (such a block needs no builder) is refused with HTTP 400, error code InvalidParameterValueException, message `The selected App block does not support the association operation` — that is entirely fixed text, with no value taken from the request. The refusal has no effect: DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=<Name>) then returns an EMPTY AppBlockBuilderAppBlockAssociations list. Positive control: an APPSTREAM2-packaged app block associates with HTTP 200 and then appears in that same list with its AppBlockArn.
Emulator: every call answers HTTP 501, error code NotImplemented, message `operation AssociateAppBlockBuilderAppBlock is scaffolded but not implemented in this emulator` (handler services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/associate_app_block_builder_app_block.rs is a scaffold stub). 501/NotImplemented is not one of the operation's modeled errors, so no client's error handling matches it, and neither the CUSTOM refusal nor the APPSTREAM2 success is reachable.
Why: a client cannot attach an app block to a builder at all, and the packaging gate that keeps an unusable pairing from being created is missing — once the stub is filled in without that gate, a CUSTOM block would associate and the builder would be left in a state AWS never produces. Overlap note: a scaffold-stub issue is filed for this handler covering only "the handler is a stub"; this cause records the request-validation semantics (the CUSTOM-packaging refusal, its exact code/message, and the empty-list effect) that issue does not describe, and it is grounded in a further failing check whose replay key that issue does not carry.
Fix: the AssociateAppBlockBuilderAppBlock handler — look up builder and app block, refuse a CUSTOM PackagingType with InvalidParameterValueException and AWS's sentence, persist the association only on the compatible path, and project it from DescribeAppBlockBuilderAppBlockAssociations.
Traces: appstream.appblockbuilder-association-incompatible-packaging-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator (appstream 9/24 passed).

1) [mismatch] appstream.appblockbuilder-invalid-role-rejected
   AssertionError (in _error): "expected the call to be rejected, but it SUCCEEDED and returned {'AppBlockBuilder': {'Arn': 'arn:aws:appstream:<region>:<account>:app-block-builder/<name>', 'Name': '<name>', 'Platform': 'WINDOWS_SERVER_2019', 'InstanceType': 'stream.standard.small', 'IamRoleArn': 'arn:aws:iam::<account>:role/<nonexistent-role>', 'VpcConfig': {...}, 'State': 'STOPPED', 'CreatedTime': ...}, 'ResponseMetadata': {'HTTPStatusCode': 200, ...}}"
   Expected: HTTP 400, code InvalidRoleException, message "WorkSpaces Applications encountered an error because your IAM role '<IamRoleArn>' is invalid"; then DescribeAppBlockBuilders(Names=[<name>]) -> HTTP 404 ResourceNotFoundException "The appblockbuilder <name> for account <accountId> does not exist."
   Handler evidence: create_app_block_builder.rs handle() (lines ~189-257) performs only a duplicate-name check and a MAX_RECORDS_PER_STORE check, then inserts the record with iam_role_arn: input.iam_role_arn.

2) [mismatch] appstream.appblockbuilder-association-incompatible-packaging-rejected
   AssertionError: "expected InvalidParameterValueException associating a CUSTOM-packaged app block, got 'NotImplemented'"
   Expected: HTTP 400, code InvalidParameterValueException, message "The selected App block does not support the association operation"; then DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=<builder>) -> [] ; and an APPSTREAM2-packaged app block associates 200 and appears in that list.
   Actual: HTTP 501 NotImplemented "operation AssociateAppBlockBuilderAppBlock is scaffolded but not implemented in this emulator" (associate_app_block_builder_app_block.rs scaffold stub).

ARNs, account numbers, builder/app-block names and timestamps above are per-run placeholders; the requirements are the HTTP status, the error code and the message templates.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Cause 1: create a VPC with two subnets, then CreateAppBlockBuilder(Name=B, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, VpcConfig={SubnetIds:[s1,s2]}, IamRoleArn=arn:aws:iam::<account>:role/does-not-exist). Emulator returns HTTP 200 with the builder; AWS returns HTTP 400 InvalidRoleException "WorkSpaces Applications encountered an error because your IAM role '<arn>' is invalid". Then DescribeAppBlockBuilders(Names=[B]): emulator describes the builder; AWS answers 404 ResourceNotFoundException "The appblockbuilder B for account <accountId> does not exist."
3. Cause 2: CreateAppBlockBuilder(Name=B2, ...) without a role; CreateAppBlock(Name=A, PackagingType=CUSTOM, SourceS3Location=..., SetupScriptDetails=...); AssociateAppBlockBuilderAppBlock(AppBlockArn=<A arn>, AppBlockBuilderName=B2). Emulator answers 501 NotImplemented; AWS answers 400 InvalidParameterValueException "The selected App block does not support the association operation" and DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=B2) returns an empty list. Repeat with an APPSTREAM2-packaged app block: AWS returns 200 and the association is listed.
4. Or replay the two checks in trace_ids.
