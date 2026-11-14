Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation.cli-script-cfn-deploy-and-verify cloudformation.deep-cloudformation-2-deployment-mode-express cloudformation.deep-cloudformation-3-cfn-flight-export-name-collision-rejected-then-recovers-after-owner-deleted cloudformation.export-value-update-refused-while-imported cloudformation.rollback-complete-stack-cannot-be-updated cloudformation.template-body-unicode-round-trip cloudformation.type-activation-toggle-lifecycle
Issue-Shape-Id: com.amazonaws.cloudformation#CloudFormation
Issue-Key: cloudformation.cli-script-cfn-deploy-and-verify|_run_shell#6|Timeout|result = subprocess.run(
Issue-Key: cloudformation.deep-cloudformation-2-deployment-mode-express|_failed_create#3|AssertionError|assert ("A", "CREATE_COMPLETE") in events, (
Issue-Key: cloudformation.deep-cloudformation-3-cfn-flight-export-name-collision-rejected-then-recovers-after-owner-deleted|run#15|AssertionError|assert status == "ROLLBACK_COMPLETE", f"rival stack ended {status!r}, expected 'ROLLBACK_COMPLETE'"
Issue-Key: cloudformation.export-value-update-refused-while-imported|run#21|AssertionError|assert consumer_display == shared_v1, (
Issue-Key: cloudformation.rollback-complete-stack-cannot-be-updated|run#15|AssertionError|assert final["StackStatus"] == "ROLLBACK_COMPLETE", final["StackStatus"]
Issue-Key: cloudformation.template-body-unicode-round-trip|run#15|AssertionError|assert parsed["Description"] == expected_description, (
Issue-Key: cloudformation.type-activation-toggle-lifecycle|run#4|AssertionError|raise AssertionError("no public third-party RESOURCE extensions available to activate")
Issue-Commit: eaacfa963fb5bc737a9bad8d07574a87d1e0bb31

# cloudformation: 7 net-new root causes — CLI deploy never converges, no per-resource stack events, export-name collision not enforced, Fn::ImportValue never reaches resources, WaitCondition timeout never fails the stack, GetTemplate skips AWS's ASCII coercion, and the public third-party type catalog is empty

> _Note: this issue was identified against the code base at commit `eaacfa963fb5bc737a9bad8d07574a87d1e0bb31`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Seven net-new root causes in the `cloudformation` service, each grounded in a failing behavioral check that the replay report marked neither ALREADY FILED nor OWNED. The emulator is serving (94/411 checks pass, no bootstrap signal), so these are fidelity divergences, not a bootstrap gap. Model citations are from services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/com-amazonaws-cloudformation-cloudformation.json.

=== Cause 1 of 7: a CLI-driven CreateStack -> wait -> use -> DeleteStack workflow never converges ===
Failing check id: cloudformation.cli-script-cfn-deploy-and-verify.
AWS: Smithy: CreateStack(StackName required; TemplateBody @length min 1) returns StackId and the stack advances CREATE_IN_PROGRESS -> CREATE_COMPLETE; DescribeStacks (no modeled errors) is the read the `aws cloudformation wait stack-create-complete` waiter polls, and DeleteStack drives the stack to DELETE_COMPLETE for the `stack-delete-complete` waiter. For a one-resource AWS::SQS::Queue template real CloudFormation settles in seconds, so both waiters return promptly and the stack's Outputs then expose the queue URL.
Emulator: the check is cut off by the harness ('Error: Check timed out after 60.0s') while the bash script is still inside `aws cloudformation wait stack-create-complete`; the emulator answers the DescribeStacks polls but never publishes the terminal status the waiter is waiting for, so the script never reads the QueueUrl output, never sends/receives through the deployed queue and never deletes the stack.
Why: every scripted/CI deploy that uses a standard CLI or SDK waiter hangs instead of completing, and nothing downstream of the deploy can be exercised at all. This is a convergence failure, not a wrong value.
Note on existing issues: open issue 20261025T015306Z-cloudformation-long-running-operations-never-converge-23-behavioral.md describes the same convergence family but does not carry this check's replay key. This check was also filed before as 20261025T024422Z-cloudformation-2-net-new-root-causes-a-cli.md and that issue is now marked FIXED, yet the identical check still times out — the fix did not land for this path. It is not a refusal (no error code is returned; the call simply never converges), so it is restated here as a live divergence with its replay key.
Fix: the asynchronous stack-operation driver in the cloudformation service crate — the task that advances a CreateStack/DeleteStack operation through its status machine, and the status each DescribeStacks read publishes.
Traces: cloudformation.cli-script-cfn-deploy-and-verify.

=== Cause 2 of 7: DescribeStackEvents emits no per-resource events, and a resource-level create failure never fails the stack (DeploymentConfig.Mode has no effect) ===
Failing check id: cloudformation.deep-cloudformation-2-deployment-mode-express.
AWS: Smithy: DescribeStackEvents returns a paginated StackEvents list of StackEvent, whose members include LogicalResourceId ('the logical name of the resource specified in the template'), ResourceType, ResourceStatus (enum: CREATE_IN_PROGRESS/CREATE_COMPLETE/CREATE_FAILED/DELETE_COMPLETE/...) and ResourceStatusReason. Real CloudFormation emits one event per resource per transition, so a template with resources A and B yields ('A','CREATE_IN_PROGRESS'), ('A','CREATE_COMPLETE'), ('B','CREATE_IN_PROGRESS') and ('B','CREATE_FAILED') alongside the stack-level events. Recorded behavior for the failure itself: a second AWS::SSM::Parameter whose Name is {'Ref':'A'} collides with the first at provisioning time, so B reaches CREATE_FAILED after A is CREATE_COMPLETE and the stack settles per CreateStack's DeploymentConfig — Mode=EXPRESS stops at CREATE_FAILED with StackStatusReason 'The following resource(s) failed to create via Express Mode: [B]. ' and leaves A in place, Mode=STANDARD (and EXPRESS with DisableRollback=false) rolls back to ROLLBACK_COMPLETE and deletes A.
Emulator: DescribeStackEvents for that stack returns only stack-level events — the whole event set is [(<stack-name>,'CREATE_IN_PROGRESS'), (<stack-name>,'CREATE_COMPLETE')]. There is no event whose LogicalResourceId is a template logical id, the colliding resource B never fails, and the stack reports CREATE_COMPLETE instead of CREATE_FAILED/ROLLBACK_COMPLETE, so DeploymentConfig.Mode is unobservable.
Why: a client cannot tell which resource failed or why (the per-resource event stream is the only place CloudFormation explains a failure), and worse, a template that real AWS refuses to provision is reported as a fully successful stack while half its declared resources do not exist.
Note on existing issues: open issues 20261025T053459Z-cloudformation-emits-no-per-resource-stack-events-and.md and 20261025T015306Z-cloudformation-stack-events-and-resource-views-are-incomplete.md describe neighbouring event-view gaps but neither carries this check's replay key.
Fix: the per-resource provisioning path of the stack driver (emit a StackEvent per resource transition, with LogicalResourceId/ResourceType/ResourceStatus/ResourceStatusReason) and the failure/rollback branch that honours CreateStack.DeploymentConfig, in the cloudformation service crate's stack state and DescribeStackEvents handler.
Traces: cloudformation.deep-cloudformation-2-deployment-mode-express.

=== Cause 3 of 7: Export.Name is not a single-owner namespace — a stack exporting a name another stack already owns reaches CREATE_COMPLETE ===
Failing check id: cloudformation.deep-cloudformation-3-cfn-flight-export-name-collision-rejected-then-recovers-after-owner-deleted.
AWS: Smithy: ListExports returns Export{ExportingStackId, Name, Value}, documented as 'the exported output values for a stack', and Export.Name is the region-wide key Fn::ImportValue resolves. Recorded behavior: a second stack whose Outputs export a name an existing stack already owns is accepted by CreateStack (the collision is asynchronous), then fails and rolls back to ROLLBACK_COMPLETE, and the rollback event for the stack itself carries ResourceStatusReason 'Export with name <ExportName> is already exported by stack <OwnerStackName>. Rollback requested by user.' — fixed text apart from the export name from the request's template and the owning stack's name taken from emulator state. The export stays with its first owner (ListExports keeps the owner's StackId and value); once the owner is deleted the identical request succeeds and takes the export over.
Emulator: the rival stack settles at CREATE_COMPLETE. No refusal, no ROLLBACK_IN_PROGRESS event, no ResourceStatusReason naming the owner.
Why: two stacks silently own one export name, so Fn::ImportValue in any consumer resolves to whichever value the emulator happens to keep; the single-owner invariant clients rely on for cross-stack wiring (and the recovery-after-delete behaviour) does not exist.
Fix: export registration in the cloudformation service's stack state — reject a duplicate Export.Name at provisioning time, fail and roll the stack back, and record the quoted ResourceStatusReason on the rollback event.
Traces: cloudformation.deep-cloudformation-3-cfn-flight-export-name-collision-rejected-then-recovers-after-owner-deleted.

=== Cause 4 of 7: Fn::ImportValue never reaches the resource property that references it ===
Failing check id: cloudformation.export-value-update-refused-while-imported.
AWS: Smithy: ListExports/Export documents Name as the value 'to import the associated value into other stacks' with Fn::ImportValue. Recorded behavior: a stack whose AWS::SNS::Topic DisplayName is {'Fn::ImportValue': <export-name>} creates a topic whose DisplayName, read back at SNS (GetTopicAttributes — an independent read, not CloudFormation's view), equals the exporting stack's exported output value.
Emulator: the importing stack reaches CREATE_COMPLETE but the topic's DisplayName comes back as the empty string '' instead of the imported value, so the import was resolved to nothing and silently dropped into the resource property. The dependent half of the check (an export value cannot be changed while another stack imports it) is therefore never reached.
Why: any cross-stack composition is broken in the most dangerous way — the consumer stack reports success while the property that should carry the imported value is empty, so clients get a resource configured with nothing rather than an error.
Note on existing issues: open issue 20261025T015306Z-cloudformation-template-transforms-and-intrinsics-are-not-processed.md covers neighbouring intrinsic gaps but does not carry this check's replay key.
Fix: intrinsic-function resolution in the cloudformation template processor — resolve Fn::ImportValue against the export registry when materializing resource properties, and fail the stack when the name is unresolvable rather than substituting an empty string.
Traces: cloudformation.export-value-update-refused-while-imported.

=== Cause 5 of 7: an AWS::CloudFormation::WaitCondition that times out never fails the stack, so ROLLBACK_COMPLETE is unreachable and UpdateStack's ROLLBACK_COMPLETE guard is unenforced ===
Failing check id: cloudformation.rollback-complete-stack-cannot-be-updated.
AWS: Recorded behavior: a template whose only resource is an AWS::CloudFormation::WaitCondition with Timeout=10 and no signal fails that resource when the timeout elapses; the stack rolls back and settles at ROLLBACK_COMPLETE. Smithy: UpdateStack(StackName required) models only InsufficientCapabilitiesException and TokenAlreadyExistsException, so the state-machine refusal arrives as the service-common ValidationError: error code 'ValidationError' at HTTP 400 with message 'Stack:<StackId> is in ROLLBACK_COMPLETE state and can not be updated.' — fixed text apart from <StackId>, which is the target stack's id from emulator state. The stack stays in ROLLBACK_COMPLETE afterwards and only DeleteStack is permitted.
Emulator: the wait-condition stack settles at CREATE_COMPLETE (assertion: 'AssertionError: CREATE_COMPLETE' where ROLLBACK_COMPLETE was required), so the timeout is never honoured and no stack can be driven into ROLLBACK_COMPLETE by this route; the ValidationError guard on UpdateStack is consequently never exercised. The check's positive control (create + update a healthy SNS-topic stack) passed, so UpdateStack itself works — the divergence is the missing failure/rollback.
Why: a client cannot rely on wait conditions as deployment barriers (a stack that should have failed reports success), and a stack that real AWS would only allow to be deleted would here accept an update.
Note on existing issues: open issue 20261025T015306Z-cloudformation-never-rolls-a-failed-stack-back-failed.md describes the same rollback family but does not carry this check's replay key.
Fix: the AWS::CloudFormation::WaitCondition provider and its timeout in the stack driver, plus the stack-status guard in the UpdateStack handler that must refuse a ROLLBACK_COMPLETE stack with the quoted ValidationError.
Traces: cloudformation.rollback-complete-stack-cannot-be-updated.

=== Cause 6 of 7: GetTemplate preserves non-ASCII code points that real CloudFormation coerces to '?' ===
Failing check id: cloudformation.template-body-unicode-round-trip.
AWS: Smithy: GetTemplate returns TemplateBody (@length min 1) for the named stack. Recorded behavior: CreateStack stores the template lossily — every non-ASCII code point in the template's Description and Metadata strings (CJK, Cyrillic, accented Latin, em-dash, emoji) comes back from GetTemplate as a literal '?', one replacement per code point. The requirement is the coercion rule, not any one string: ASCII code points survive, every code point above 0x7F becomes '?'.
Emulator: GetTemplate returns the original unicode verbatim, e.g. Description '\u90e8\u7f72\u30c6\u30f3\u30d7\u30ec\u30fc\u30c8 \ud83d\ude80 \u2014 caf\u00e9 na\u00efve, Z\u00fcrich, \u041c\u043e\u0441\u043a\u0432\u0430' where AWS returns '???????? ? ? caf? na?ve, Z?rich, ??????'; the same holds for Metadata string values.
Why: the emulator is more permissive than AWS. A client that round-trips a template through CreateStack/GetTemplate and compares, hashes or re-deploys it sees a faithful body here and a '?'-folded body in production — template drift, changed hashes and diffs that only appear against real AWS.
Note on existing issues: the open unicode issue 20261025T015306Z-cloudformation-is-not-region-scoped-stack-names-and.md covers stack TAG values, not the stored template body/Metadata, and does not carry this check's replay key.
Fix: the template-store write path in the cloudformation service (apply AWS's ASCII coercion to the stored Description/Metadata strings) and the GetTemplate serializer that returns it.
Traces: cloudformation.template-body-unicode-round-trip.

=== Cause 7 of 7: ListTypes exposes no PUBLIC third-party RESOURCE extensions, so the whole ActivateType/DeactivateType lifecycle is unreachable ===
Failing check id: cloudformation.type-activation-toggle-lifecycle.
AWS: Smithy: ListTypes(Visibility, Type, Filters, MaxResults 1..100, paginated on TypeSummaries) returns TypeSummary entries carrying TypeName, PublisherId, TypeArn and IsActivated; ActivateType(Type=RESOURCE, TypeName, PublisherId, ExecutionRoleArn) returns the private Arn of the account's activated copy. Recorded behavior: ListTypes(Visibility=PUBLIC, Filters.Category=THIRD_PARTY, Type=RESOURCE) is sparse but non-empty — the first match can sit ~2000 items deep behind NextToken-bearing pages — and from such an entry ActivateType yields an Arn that DescribeType resolves with Visibility='PRIVATE'; DeactivateType then flips DeprecatedStatus to 'DEPRECATED' and drops it from ListTypes(Visibility=PRIVATE, Filters.Category=ACTIVATED) without deleting the registration.
Emulator: a 60-page paginated scan of ListTypes(Visibility=PUBLIC, Filters.Category=THIRD_PARTY, Type=RESOURCE) returns no entry with both TypeName and PublisherId at all ('AssertionError: no public third-party RESOURCE extensions available to activate'), so the registry's public catalog is empty and nothing can be activated.
Why: no client can discover or activate a public third-party extension, and the activation state machine (PRIVATE visibility, ACTIVATED listing membership, the DEPRECATED flip on deactivate) cannot be observed. The public catalog is AWS-global data, not account-specific, so an empty answer here is an emulator state gap rather than an artifact of the test account.
Note on existing issues: 20261025T015306Z-cloudformation-listtypes-stub.md is marked FIXED — ListTypes now answers (HTTP 200, no error code), it just answers with an empty catalog for this filter combination, which is net-new work layered on that fix.
Fix: the type-registry state in the cloudformation service crate (seed/serve the PUBLIC THIRD_PARTY catalog that ListTypes pages over) and the ActivateType/DeactivateType/DescribeType handlers that must then expose Visibility=PRIVATE, ACTIVATED-listing membership and the DeprecatedStatus flip.
Traces: cloudformation.type-activation-toggle-lifecycle.

## Evidence
Replayed suite simparity-behavioral-ga-prod-v1 against the booted emulator: 94/411 passed (22.9%); cloudformation 83/248. bootstrap_signal was false and 94 checks pass, so the emulator is serving and these are fidelity divergences. All seven failing checks below were listed by the report WITHOUT an 'ALREADY FILED' or 'OWNED by' marker.

Cause 1 — cloudformation.cli-script-cfn-deploy-and-verify, verdict [mismatch]:
  Expected: `aws cloudformation create-stack` on a one-resource AWS::SQS::Queue template followed by `aws cloudformation wait stack-create-complete` returns promptly; DescribeStacks then exposes the QueueUrl output; a probe message round-trips through the queue; `delete-stack` + `wait stack-delete-complete` likewise return and SQS GetQueueUrl then 404s.
  Actual: 'Error: Check timed out after 60.0s' with the script still inside `wait stack-create-complete`. No URL line and no BODY line were ever printed.

Cause 2 — cloudformation.deep-cloudformation-2-deployment-mode-express, verdict [mismatch]:
  Expected: event set contains ('A','CREATE_COMPLETE') and ('B','CREATE_FAILED'); under DeploymentConfig Mode=EXPRESS the stack is CREATE_FAILED with StackStatusReason 'The following resource(s) failed to create via Express Mode: [B]. ' and parameter A survives at SSM; under Mode=STANDARD the stack is ROLLBACK_COMPLETE and SSM GetParameter on A answers ParameterNotFound.
  Actual: AssertionError: '[express] A must be created before B fails, or its fate proves nothing; events=[('sim2real-behav-dmode-express-ee973e146fa5','CREATE_COMPLETE'), ('sim2real-behav-dmode-express-ee973e146fa5','CREATE_IN_PROGRESS')]' — the only events are the two stack-level ones, and the stack reports CREATE_COMPLETE for a template whose second resource must fail.

Cause 3 — cloudformation.deep-cloudformation-3-cfn-flight-export-name-collision-rejected-then-recovers-after-owner-deleted, verdict [mismatch]:
  Expected: the rival stack settles ROLLBACK_COMPLETE and its ROLLBACK_IN_PROGRESS event's ResourceStatusReason is 'Export with name <export> is already exported by stack <owner>. Rollback requested by user.'; ListExports still shows the owner's StackId and 'owner-value'.
  Actual: AssertionError: "rival stack ended 'CREATE_COMPLETE', expected 'ROLLBACK_COMPLETE'".

Cause 4 — cloudformation.export-value-update-refused-while-imported, verdict [mismatch]:
  Expected: the importer stack's AWS::SNS::Topic DisplayName, read at SNS, equals the exported value (e.g. 'shared-v1-<suffix>').
  Actual: AssertionError: "importer's topic DisplayName: expected the imported export value 'shared-v1-80c8e3fbebcf', got ''; the Fn::ImportValue never reached the resource".

Cause 5 — cloudformation.rollback-complete-stack-cannot-be-updated, verdict [mismatch]:
  Expected: the WaitCondition (Timeout=10, never signalled) stack settles ROLLBACK_COMPLETE; UpdateStack against it is then refused with code 'ValidationError' and message 'Stack:<StackId> is in ROLLBACK_COMPLETE state and can not be updated.'; the stack remains ROLLBACK_COMPLETE.
  Actual: AssertionError: 'CREATE_COMPLETE' — the stack settled CREATE_COMPLETE, so the rejection path was never reached. The positive control (create then update a healthy SNS-topic stack to UPDATE_COMPLETE) passed.

Cause 6 — cloudformation.template-body-unicode-round-trip, verdict [mismatch]:
  Expected (GetTemplate after CreateStack): Description '???????? ? ? caf? na?ve, Z?rich, ??????' and the same one-for-one '?' folding in Metadata.sim2real.note and .arrow.
  Actual: AssertionError: 'Description did not match real AWS's ASCII-coercion contract: "\u90e8\u7f72\u30c6\u30f3\u30d7\u30ec\u30fc\u30c8 \ud83d\ude80 \u2014 caf\u00e9 na\u00efve, Z\u00fcrich, \u041c\u043e\u0441\u043a\u0432\u0430" != "???????? ? ? caf? na?ve, Z?rich, ??????"' — the body came back byte-identical.

Cause 7 — cloudformation.type-activation-toggle-lifecycle, verdict [mismatch]:
  Expected: ListTypes(Visibility=PUBLIC, Filters.Category=THIRD_PARTY, Type=RESOURCE) yields at least one TypeSummary with TypeName and PublisherId; ActivateType with an ExecutionRoleArn returns an Arn whose DescribeType reports Visibility='PRIVATE'; the Arn appears in ListTypes(PRIVATE/ACTIVATED) as TypeArn; after DeactivateType, DescribeType still resolves with DeprecatedStatus='DEPRECATED' and the Arn is gone from the ACTIVATED listing.
  Actual: AssertionError: 'no public third-party RESOURCE extensions available to activate' after a 60-page scan at MaxResults=100.

## Steps to Reproduce
Cause 1 (cloudformation.cli-script-cfn-deploy-and-verify):
1. Boot the emulator, point the AWS CLI at it.
2. aws cloudformation create-stack --stack-name <n> --template-body '{"Resources":{"Q":{"Type":"AWS::SQS::Queue","Properties":{"QueueName":"<q>"}}},"Outputs":{"QueueUrl":{"Value":{"Ref":"Q"}}}}'
3. aws cloudformation wait stack-create-complete --stack-name <n> — it does not return; poll DescribeStacks and observe the stack never publishes CREATE_COMPLETE.

Cause 2 (cloudformation.deep-cloudformation-2-deployment-mode-express):
1. CreateStack with DeploymentConfig={'Mode':'EXPRESS'} and a template of two AWS::SSM::Parameter resources, A with Name=<p> and B with Name={'Ref':'A'} (so B collides with A at provisioning time).
2. Wait for a terminal status and call DescribeStackEvents (paginated).
3. Observe the event set contains only (<stack-name>,'CREATE_IN_PROGRESS') and (<stack-name>,'CREATE_COMPLETE') — no ('A','CREATE_COMPLETE'), no ('B','CREATE_FAILED') — and the stack is CREATE_COMPLETE rather than CREATE_FAILED. Repeat with Mode=STANDARD and observe no ROLLBACK_COMPLETE and no deletion of A at SSM.

Cause 3 (export-name collision):
1. CreateStack 'owner' with Outputs {'Shared':{'Value':'owner-value','Export':{'Name':<e>}}} and wait for CREATE_COMPLETE; confirm ListExports maps <e> to the owner's StackId.
2. CreateStack 'rival' with the identical Export Name and a different value.
3. Observe the rival settles CREATE_COMPLETE instead of ROLLBACK_COMPLETE, and no event carries ResourceStatusReason 'Export with name <e> is already exported by stack <owner>. Rollback requested by user.'

Cause 4 (Fn::ImportValue):
1. CreateStack an exporter whose Outputs export <e> with value <v>; confirm ListExports shows <v>.
2. CreateStack an importer whose AWS::SNS::Topic has DisplayName={'Fn::ImportValue':<e>}; wait for CREATE_COMPLETE.
3. Resolve the topic's physical id via DescribeStackResources and call SNS GetTopicAttributes: DisplayName is '' instead of <v>.

Cause 5 (WaitCondition timeout / ROLLBACK_COMPLETE guard):
1. CreateStack with an AWS::CloudFormation::WaitCondition (Timeout=10) and its handle, and send no signal.
2. Wait for a terminal status: the stack reports CREATE_COMPLETE instead of ROLLBACK_COMPLETE.
3. (Blocked by step 2) UpdateStack against a ROLLBACK_COMPLETE stack and observe whether it is refused with 'ValidationError' / 'Stack:<StackId> is in ROLLBACK_COMPLETE state and can not be updated.'

Cause 6 (template unicode coercion):
1. CreateStack with TemplateBody whose Description and Metadata contain non-ASCII code points (CJK, Cyrillic, accented Latin, em-dash, emoji), sent with ensure_ascii=False.
2. Wait for CREATE_COMPLETE, then GetTemplate and parse TemplateBody.
3. Observe Description/Metadata come back with the original code points instead of one literal '?' per non-ASCII code point.

Cause 7 (public third-party type catalog):
1. Page ListTypes(Visibility='PUBLIC', Filters={'Category':'THIRD_PARTY'}, Type='RESOURCE', MaxResults=100) following NextToken.
2. Observe no TypeSummary carrying both TypeName and PublisherId is ever returned, so ActivateType/DeactivateType/DescribeType cannot be exercised.
