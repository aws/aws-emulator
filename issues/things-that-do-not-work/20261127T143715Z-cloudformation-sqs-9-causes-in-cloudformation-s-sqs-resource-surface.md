Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__sqs.cfn-change-set-fifo-queue-toggle-classified-as-replacement cloudformation__sqs.deploy-cfn-fifoqueue-change-replaces-standard-queue-with-new-fifo-queue cloudformation__sqs.cfn-drift-detects-queue-tag-and-redrive-policy-changes cloudformation__sqs.cfn-queue-policy-on-missing-queue-fails-create cloudformation__sqs.cfn-queue-update-dropping-optional-properties-restores-defaults cloudformation__sqs.update-dropping-visibility-timeout-restores-the-default cloudformation__sqs.deploy-cfn-queueinlinepolicy-target-queue-gets-no-stack-or-system-tags cloudformation__sqs.deploy-queueinlinepolicy-queue-change-moves-the-policy-by-replacement cloudformation__sqs.deploy-stack-tag-update-retags-the-queue-and-leaves-its-inline-policy-untouched cloudformation__sqs.deploy-cfn-repeated-tag-update-of-queueinlinepolicy-stack-rejected cloudformation__sqs.deploy-cfn-stack-tag-update-retags-queue-but-leaves-queueinlinepolicy-unupdated cloudformation__sqs.deploy-import-of-queuepolicy-rejected-while-its-queue-imports cloudformation__sqs.deploy-stack-policy-denying-replace-rejects-queueinlinepolicy-queue-change cloudformation__sqs.join-with-conditional-no-value-element-keeps-remaining-elements
Issue-Key: cloudformation__sqs.cfn-change-set-fifo-queue-toggle-classified-as-replacement|run#11|AssertionError|assert (flip["Action"], flip.get("Replacement")) == ("Modify", "True"), (
Issue-Key: cloudformation__sqs.deploy-cfn-fifoqueue-change-replaces-standard-queue-with-new-fifo-queue|run#17|AssertionError|assert [(c["LogicalResourceId"], c["Action"], c["Replacement"]) for c in changes] == [
Issue-Key: cloudformation__sqs.cfn-drift-detects-queue-tag-and-redrive-policy-changes|run#13|AssertionError|assert tagged["status"] == "MODIFIED" and any(p.startswith("/Tags") for p in tagged["paths"]), (
Issue-Key: cloudformation__sqs.cfn-queue-policy-on-missing-queue-fails-create|run#8|AssertionError|assert status == "ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__sqs.cfn-queue-update-dropping-optional-properties-restores-defaults|run#15|AssertionError|assert status == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__sqs.update-dropping-visibility-timeout-restores-the-default|run#14|AssertionError|assert updated["StackStatus"] == "UPDATE_COMPLETE", (
Issue-Key: cloudformation__sqs.deploy-cfn-queueinlinepolicy-target-queue-gets-no-stack-or-system-tags|run#20|AssertionError|assert grant["PhysicalResourceId"] == target_url, (
Issue-Key: cloudformation__sqs.deploy-queueinlinepolicy-queue-change-moves-the-policy-by-replacement|run#11|AssertionError|assert physical("Grant") == first_url, "the inline policy's physical id must be the first url"
Issue-Key: cloudformation__sqs.deploy-stack-tag-update-retags-the-queue-and-leaves-its-inline-policy-untouched|run#11|AssertionError|assert resources["OrdersPolicy"] == queue_url, (
Issue-Key: cloudformation__sqs.deploy-cfn-repeated-tag-update-of-queueinlinepolicy-stack-rejected|run#21|AssertionError|assert policy_row["ResourceStatus"] == "CREATE_COMPLETE", (
Issue-Key: cloudformation__sqs.deploy-cfn-stack-tag-update-retags-queue-but-leaves-queueinlinepolicy-unupdated|run#21|AssertionError|assert touched == {"Owned"}, (
Issue-Key: cloudformation__sqs.deploy-import-of-queuepolicy-rejected-while-its-queue-imports|_refusal#9|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: cloudformation__sqs.deploy-stack-policy-denying-replace-rejects-queueinlinepolicy-queue-change|run#16|AssertionError|assert status == "UPDATE_ROLLBACK_COMPLETE", (
Issue-Key: cloudformation__sqs.join-with-conditional-no-value-element-keeps-remaining-elements|run#12|AssertionError|assert outputs == {"Joined": expected_name}, (
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# 9 causes in CloudFormation's SQS resource surface: create-only classification, drift, QueuePolicy/QueueInlinePolicy handling, stack policy, defaults restore and Fn::Join

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateChangeSet

## Description
Nine causes.

Cause 1 of 9: changing or dropping AWS::SQS::Queue's create-only `FifoQueue` property is classified as an in-place Modify.
AWS: `FifoQueue` is create-only — SQS has no API to convert a queue between standard and FIFO (Smithy: com.amazonaws.sqs SetQueueAttributes refuses it). CloudFormation therefore reports the change as `Action=Modify, Replacement=True` with the change detail's `RequiresRecreation=Always`, and executing it creates a new queue and deletes the old one.
Emulator: the change set reports `Replacement='False'` with `Details: [{Target: {Attribute: Properties, Name: FifoQueue, RequiresRecreation: 'Never'}}]`, and the queue's physical id is unchanged.
Why: the plan tells the client its queue and messages survive when AWS would replace the queue; a review gate keyed on Replacement passes a destructive change.
Fix: the AWS::SQS::Queue resource schema's create-only property set and the change-set classifier that derives Replacement/RequiresRecreation from it.
Traces: cloudformation__sqs.cfn-change-set-fifo-queue-toggle-classified-as-replacement, cloudformation__sqs.deploy-cfn-fifoqueue-change-replaces-standard-queue-with-new-fifo-queue.

Cause 2 of 9: drift detection does not see out-of-band changes to a queue's declared Tags or RedrivePolicy.
AWS: DetectStackDrift compares each declared property against the live resource; after a declared tag value is changed out of band the resource's drift status is MODIFIED and PropertyDifferences names the differing path.
Emulator: drift reads `{'status': 'IN_SYNC', 'paths': []}` after the declared tag `team=blue` was changed to `red` out of band.
Why: drift detection is the client's only report that reality left the template; reporting IN_SYNC hides every such change.
Fix: the stack drift-detection comparator for AWS::SQS::Queue (Tags and RedrivePolicy property readers).
Traces: cloudformation__sqs.cfn-drift-detects-queue-tag-and-redrive-policy-changes.

Cause 3 of 9: an AWS::SQS::QueuePolicy naming a queue that does not exist reaches CREATE_COMPLETE.
AWS: the QueuePolicy handler calls SetQueueAttributes on the named queue URL; a queue that does not exist is refused `AWS.SimpleQueueService.NonExistentQueue` / `The specified queue does not exist.` (Smithy: com.amazonaws.sqs SetQueueAttributes error QueueDoesNotExist, awsQueryError `AWS.SimpleQueueService.NonExistentQueue`), so the resource is CREATE_FAILED and the stack rolls back.
Emulator: the resource ends CREATE_COMPLETE and the stack succeeds, having written nothing.
Why: the stack reports success while the policy it was deployed to attach does not exist anywhere — the client believes a queue is protected when it is not.
Fix: the AWS::SQS::QueuePolicy resource handler's create path (it must surface SetQueueAttributes' refusal).
Traces: cloudformation__sqs.cfn-queue-policy-on-missing-queue-fails-create.

Cause 4 of 9: an update that stops declaring an optional queue property does not restore SQS's default; the handler refuses the update.
AWS: when an update no longer declares an optional property, CloudFormation resets it to the service default — for AWS::SQS::Queue, VisibilityTimeout returns to 30 seconds — and the stack reaches UPDATE_COMPLETE.
Emulator: the update ends `UPDATE_ROLLBACK_COMPLETE` with the queue's UPDATE_FAILED reason `Resource handler returned message: "this emulator does not reset the VisibilityTimeout property of an existing AWS::SQS::Queue that an update no longer declares to Amazon SQS's documented default; the resource was left as it is." (HandlerErrorCode: InvalidRequest)`.
Why: a client cannot un-set any optional queue property; the stack rolls back and the stale value stays live.
Fix: the AWS::SQS::Queue resource handler's update path (undeclared optional properties must be reset to their documented defaults).
Traces: cloudformation__sqs.cfn-queue-update-dropping-optional-properties-restores-defaults, cloudformation__sqs.update-dropping-visibility-timeout-restores-the-default.

Cause 5 of 9: AWS::SQS::QueueInlinePolicy's physical id is a stack-derived name instead of the URL of the queue it attaches to.
AWS: AWS::SQS::QueueInlinePolicy's primary identifier is the queue it configures, so its PhysicalResourceId (and `Ref`) is that queue's URL; when the policy moves to another queue the physical id becomes the new queue's URL.
Emulator: the physical id reads `<stack>-<logical-id>-<suffix>` (e.g. `sim2real-xsvc-qinline-2fc902fdd53b-OrdersPolicy-4A7210D22E85`), not a queue URL, in every one of the three checks.
Why: a client that resolves the policy resource — to find which queue it governs, or to pass it on with Ref — gets a string that names no SQS resource, and the replacement-on-queue-change is unobservable.
Fix: the AWS::SQS::QueueInlinePolicy resource handler's primary-identifier/Ref projection.
The open issue 20261109T185037Z-cloudformation-sqs-6-net-new-causes-aws-sqs-queue-read.md reports the same physical-id defect for one other check; it does not carry these replay keys.
Traces: cloudformation__sqs.deploy-cfn-queueinlinepolicy-target-queue-gets-no-stack-or-system-tags, cloudformation__sqs.deploy-queueinlinepolicy-queue-change-moves-the-policy-by-replacement, cloudformation__sqs.deploy-stack-tag-update-retags-the-queue-and-leaves-its-inline-policy-untouched.

Cause 6 of 9: a stack-tag-only update updates the non-taggable AWS::SQS::QueueInlinePolicy resource.
AWS: a change to the stack's tags is propagated only to resources that support tagging. AWS::SQS::QueueInlinePolicy is not taggable, so the update touches only the queue: the policy resource emits no UPDATE events and keeps its previous status.
Emulator: the policy resource reads `UPDATE_COMPLETE` after a tag-only update, and the update's events name both resources (`['Access', 'Owned']`) rather than only the taggable queue.
Why: the client's event stream and change plan claim a resource was reconfigured when nothing about it changed; a repeated tag update that AWS answers with "no updates" instead performs work.
Fix: the stack-tag propagation step's taggability filter, and the AWS::SQS::QueueInlinePolicy handler's update classification.
Traces: cloudformation__sqs.deploy-cfn-repeated-tag-update-of-queueinlinepolicy-stack-rejected, cloudformation__sqs.deploy-cfn-stack-tag-update-retags-queue-but-leaves-queueinlinepolicy-unupdated.

Cause 7 of 9: CreateChangeSet with `ChangeSetType=IMPORT` accepts a resource type that cannot be imported.
AWS: only resource types whose schema declares import support may appear in an import change set; AWS::SQS::QueuePolicy may not, so CreateChangeSet is refused with a `ValidationError` naming the unimportable resource and no change set is created.
Emulator: the call succeeds and returns a change-set Id and StackId.
Why: the client gets a change set it cannot usefully execute and no signal that its import list is invalid; the refusal AWS uses to stop an unsafe import never happens.
Fix: the CreateChangeSet import path's per-resource-type import-support validation.
Traces: cloudformation__sqs.deploy-import-of-queuepolicy-rejected-while-its-queue-imports.

Cause 8 of 9: a stack policy denying `Update:Replace` does not roll back an update that replaces a resource.
AWS: when a stack policy statement denies `Update:Replace` for a logical id, an update that would replace it fails with the stack-policy denial and the stack ends UPDATE_ROLLBACK_COMPLETE.
Emulator: the stack ends `UPDATE_COMPLETE` — the denied replacement is performed.
Why: the stack policy is a client's guard against destructive updates; ignoring it means a protected resource is replaced (and its messages lost) with no error.
Fix: the stack-policy evaluation in the update executor for the Update:Replace action.
Traces: cloudformation__sqs.deploy-stack-policy-denying-replace-rejects-queueinlinepolicy-queue-change.

Cause 9 of 9: `Fn::Join` whose list contains an element resolving to `AWS::NoValue` yields null instead of joining the remaining elements.
AWS: an element that resolves to `AWS::NoValue` (e.g. via `Fn::If` with a false condition) is removed from the list and `Fn::Join` returns the delimiter-joined remaining elements.
Emulator: the stack's output is `None` — the whole intrinsic collapses to null.
Why: any template using a conditional element inside Fn::Join (a very common pattern for optional name parts) produces an empty output and hands downstream resources an empty string.
Fix: the Fn::Join intrinsic evaluator's handling of AWS::NoValue elements.
Traces: cloudformation__sqs.join-with-conditional-no-value-element-keeps-remaining-elements.

## Evidence
Cause 1: `AssertionError: dropping FifoQueue (a create-only property) classified as Modify/'False' with details {'FifoQueue': 'Never'}; expected a replacement`; and `expected one replacing Modify of Inbox; the change set listed [{... 'Replacement': 'False', 'Details': [{'Target': {'Attribute': 'Properties', 'Name': 'FifoQueue', 'RequiresRecreation': 'Never'}}]}]`.
Cause 2: `AssertionError: after the declared tag team=blue was changed to red out of band, drift reads {'status': 'IN_SYNC', 'paths': []}`.
Cause 3: `AssertionError: a QueuePolicy on the missing queue <queue-url> ended CREATE_COMPLETE; it must fail and roll back`.
Cause 4: `AssertionError: an update that only stops declaring optional queue properties ended UPDATE_ROLLBACK_COMPLETE; failures: [('Queue', 'UPDATE_FAILED', 'Resource handler returned message: "this emulator does not reset the VisibilityTimeout property ... the resource was left as it is." (HandlerErrorCode: InvalidRequest)')]`; and `update ended 'UPDATE_ROLLBACK_COMPLETE': 'The following resource(s) failed to update: [Queue]. '`.
Cause 5: `AssertionError: the inline policy's physical id must be the url of the queue it attaches to`; `the inline policy's physical id must be the first url`; `the inline policy's physical id 'sim2real-xsvc-qinline-2fc902fdd53b-OrdersPolicy-4A7210D22E85' is not its queue URL`.
Cause 6: `AssertionError: the tag change must not update the inline policy; it reads 'UPDATE_COMPLETE'`; `only the taggable queue may be updated by a tag change; events named ['Access', 'Owned']`.
Cause 7: `AssertionError: expected a refusal, but the call succeeded: {'Id': 'arn:aws:cloudformation:...:changeSet/...', 'StackId': 'arn:aws:cloudformation:...:stack/...', 'ResponseMetadata': {'HTTPStatusCode': 200 ...}}`.
Cause 8: `AssertionError: a denied replacement must roll the update back; the stack ended 'UPDATE_COMPLETE'`.
Cause 9: `AssertionError: outputs were {'Joined': None}, expected the joined string '<prefix>-tail'`.

## Steps to Reproduce
Each cause is reproduced by replaying its listed check id. In outline:
1. FifoQueue toggle: change set over a standard AWS::SQS::Queue whose template now sets/drops FifoQueue; inspect Replacement and RequiresRecreation.
2. Drift: deploy a queue with declared Tags/RedrivePolicy, change a declared tag with SQS TagQueue out of band, run DetectStackDrift.
3. QueuePolicy: deploy an AWS::SQS::QueuePolicy whose Queues names a non-existent queue URL.
4. Defaults: deploy a queue declaring VisibilityTimeout, then update the template so it no longer declares it.
5. QueueInlinePolicy id: deploy AWS::SQS::QueueInlinePolicy and read its PhysicalResourceId / Ref.
6. Tag-only update: update only the stack's tags on a stack containing a queue and a QueueInlinePolicy; inspect resource events.
7. Import: CreateChangeSet with ChangeSetType=IMPORT listing an AWS::SQS::QueuePolicy alongside a queue.
8. Stack policy: deny Update:Replace for the inline policy's logical id, then change the queue it attaches to.
9. Fn::Join: `{"Fn::Join": ["-", [{"Fn::If": [<false cond>, "head", {"Ref": "AWS::NoValue"}]}, "tail"]]}` as a stack Output.
