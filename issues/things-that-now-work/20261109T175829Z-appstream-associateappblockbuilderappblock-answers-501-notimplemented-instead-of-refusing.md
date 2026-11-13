Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appstream.appblockbuilder-association-incompatible-packaging-rejected
Issue-Shape-Id: com.amazonaws.appstream#PhotonAdminProxyService
Issue-Key: appstream.appblockbuilder-association-incompatible-packaging-rejected|run#13|AssertionError|assert code == "InvalidParameterValueException", (
Issue-Commit: db165e9afd311b39a9697deff319fb6ff63a6914

# appstream AssociateAppBlockBuilderAppBlock answers 501 NotImplemented instead of refusing a CUSTOM-packaged app block with InvalidParameterValueException (1 cause)

> _Note: this issue was identified against the code base at commit `db165e9afd311b39a9697deff319fb6ff63a6914`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateAppBlockBuilderAppBlock

## Description
One net-new root cause in the AppStream app-block-builder association surface, grounded in a failing behavioral check of this suite.

Note on overlap: open issue 20261106T134718Z-appstream-app-block-builder-divergences-createappblockbuilder-accepts-a.md describes a similar cause, and open issue 20261106T134718Z-appstream-associateappblockbuilderappblock-stub.md records only that the handler is a scaffold stub. Neither carries this check's current replay key (the suite reports it as neither ALREADY FILED nor OWNED), so the semantics below are restated here against the current checkout so a fixer has the gate and its wording in one place.

Cause 1 of 1: AssociateAppBlockBuilderAppBlock has no implementation, so the packaging-compatibility refusal, its no-effect guarantee, and the compatible-path association read are all unreachable.

AWS:      Smithy: AssociateAppBlockBuilderAppBlock requires AppBlockArn (target Arn, pattern ^arn:aws(?:-cn|-iso-b|-iso|-us-gov)?:...$) and AppBlockBuilderName (target Name, pattern ^[a-zA-Z0-9][a-zA-Z0-9_.-]{0,100}$); it returns AppBlockBuilderAppBlockAssociation and models ConcurrentModificationException, InvalidParameterCombinationException, LimitExceededException, OperationNotPermittedException (all httpError 400) and ResourceNotFoundException (404). The packaging rule itself is recorded behavior only — the model names no InvalidParameterValueException for this operation. Recorded behavior: associating an app block whose PackagingType is CUSTOM (such a block is installed by its setup script and needs no builder) is refused with HTTP 400, error code InvalidParameterValueException, message `The selected App block does not support the association operation`. That message is entirely FIXED text: it takes no name, ARN, id or value from the request or from state. The refusal has NO effect — DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=<builder>) then returns an EMPTY AppBlockBuilderAppBlockAssociations list (the key present with zero entries, per the check's assertion). Positive control: an APPSTREAM2-packaged app block associates with HTTP 200 and then appears in that same list with its AppBlockArn.
Emulator: every call to the operation answers HTTP 501, error code NotImplemented, message `operation AssociateAppBlockBuilderAppBlock is scaffolded but not implemented in this emulator`. The check asserted the error code of the CUSTOM association and got 'NotImplemented' where AWS gives 'InvalidParameterValueException', so the body of the check never reaches either the empty-list effect assertion or the APPSTREAM2 positive control.
Why:      501/NotImplemented is not one of the operation's modeled errors, so no generated client's error handling matches it: a caller that catches the modeled 400 classes (or that branches on InvalidParameterValueException to fall back to a CUSTOM-packaging install path) sees an unmodeled transport-level failure instead and cannot attach any app block to a builder at all. The packaging gate is what keeps an unusable builder/app-block pairing from existing; if the stub is later filled in without it, a CUSTOM block would associate with HTTP 200 and leave the builder in a state real AppStream never produces — a silent overly-permissive accept replacing today's loud 501.
Fix:      the AssociateAppBlockBuilderAppBlock handler and its validation (services/service-com-amazonaws-appstream-photonadminproxyservice/src/ops/associate_app_block_builder_app_block.rs): resolve the app block by ARN and the builder by name (404 ResourceNotFoundException when either is absent), refuse a CUSTOM PackagingType with HTTP 400 InvalidParameterValueException carrying AWS's fixed sentence, persist nothing on the refused path, and persist plus project the association from DescribeAppBlockBuilderAppBlockAssociations on the APPSTREAM2 path.
Traces:   appstream.appblockbuilder-association-incompatible-packaging-rejected.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout (appstream 9/24 passed; overall 17/43).

[mismatch] appstream.appblockbuilder-association-incompatible-packaging-rejected
  check_appstream_appblockbuilder_association_incompatible_packaging_rejected.py line 53:
    assert code == "InvalidParameterValueException"
  AssertionError: expected InvalidParameterValueException associating a CUSTOM-packaged app block, got 'NotImplemented'

Expected (AWS):
  HTTP 400, Error.Code = InvalidParameterValueException,
  Error.Message = "The selected App block does not support the association operation" (all fixed text, no request/state values),
  then DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=<builder>) -> AppBlockBuilderAppBlockAssociations == [],
  and for an APPSTREAM2-packaged app block: HTTP 200 and the AppBlockArn present in that list.

Actual (emulator):
  HTTP 501, Error.Code = NotImplemented,
  Error.Message = "operation AssociateAppBlockBuilderAppBlock is scaffolded but not implemented in this emulator".
  501/NotImplemented is not among the operation's modeled errors (ConcurrentModificationException, InvalidParameterCombinationException, LimitExceededException, OperationNotPermittedException = 400; ResourceNotFoundException = 404).

Builder names, app-block names, bucket names, ARNs, account ids and timestamps in the run are per-run values; the requirements are the HTTP status, the error code, the fixed message text and the empty-list effect.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. Create a VPC with two subnets and an S3 bucket holding the package and setup-script objects.
3. CreateAppBlockBuilder(Name=B, Platform=WINDOWS_SERVER_2019, InstanceType=stream.standard.small, VpcConfig={SubnetIds:[s1,s2]}).
4. CreateAppBlock(Name=A, PackagingType=CUSTOM, SourceS3Location={S3Bucket:<bucket>,S3Key:pkg/incompat.zip}, SetupScriptDetails={ScriptS3Location:{...}, ExecutablePath:setup.ps1, TimeoutInSeconds:60}).
5. AssociateAppBlockBuilderAppBlock(AppBlockArn=<A arn>, AppBlockBuilderName=B).
   Emulator: HTTP 501 NotImplemented "operation AssociateAppBlockBuilderAppBlock is scaffolded but not implemented in this emulator".
   AWS: HTTP 400 InvalidParameterValueException "The selected App block does not support the association operation".
6. DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=B): AWS returns an empty AppBlockBuilderAppBlockAssociations list (the refused associate created nothing).
7. CreateAppBlock(Name=A2, PackagingType=APPSTREAM2, SourceS3Location={...}) then AssociateAppBlockBuilderAppBlock(AppBlockArn=<A2 arn>, AppBlockBuilderName=B): AWS returns HTTP 200 and <A2 arn> then appears in DescribeAppBlockBuilderAppBlockAssociations(AppBlockBuilderName=B).
8. Or replay the check in trace_ids.
