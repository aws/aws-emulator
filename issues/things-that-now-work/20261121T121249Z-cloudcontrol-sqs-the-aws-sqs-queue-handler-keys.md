Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue
Issue-Key: cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api|run#10|AssertionError|assert sqs.get_queue_url(QueueName=queue_name)["QueueUrl"] == identifier, (
Issue-Key: cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found|_poll_until#7|AssertionError|raise AssertionError(
Issue-Key: cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue|_poll_until#8|Timeout|time.sleep(interval_s)
Issue-Commit: a3e7427fc33f71baf84855dcb1036df3c2b77cc6

# cloudcontrol__sqs: the AWS::SQS::Queue handler keys and reports a synthesized real-AWS-host QueueUrl, and GetResource/ListResources never resolve a queue SQS itself created

> _Note: this issue was identified against the code base at commit `a3e7427fc33f71baf84855dcb1036df3c2b77cc6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetResource

## Description
Cause 1 of 2: Cloud Control's AWS::SQS::Queue handler reports a synthesized https://sqs.<region>.amazonaws.com/<account-id>/<queue-name> as the resource identifier of a queue it just created, instead of the QueueUrl SQS returned, so ProgressEvent.Identifier (and the ResourceDescription.Identifier built from it) names a queue SQS does not resolve.
AWS: Cloud Control documents the AWS::SQS::Queue primary identifier as the queue's QueueUrl, and Smithy (sqs): CreateQueueResult.QueueUrl / GetQueueUrlResult.QueueUrl is that queue's URL and the exact value every data-plane operation requires. The host is environment-specific and is not the requirement; the invariant is that the identifier Cloud Control reports for a queue and the URL SQS reports for the same queue are the SAME string, served by the endpoint being talked to. Recorded behavior only for the concrete host.
Emulator: after CreateResource(TypeName=AWS::SQS::Queue) the identifier is 'https://sqs.us-west-2.amazonaws.com/<account>/<queue>' while sqs GetQueueUrl for that name answers the emulator's served listener — 'the two services are not describing the same resource'.
Why: the identifier is the only handle a Cloud Control client gets back; used against SQS it addresses an unserved host, and every later GetResource/UpdateResource/DeleteResource keyed on the real handle misses.
Fix: the Cloud Control AWS::SQS::Queue create/read handler's identifier resolution (take the QueueUrl the CreateQueue call returned).
Traces: cloudcontrol__sqs.cloudcontrol-lifecycle-visible-to-native-sqs-api.

Cause 2 of 2: Cloud Control does not read through to the owning service: GetResource (and ListResources) for a queue created with SQS's own CreateQueue never resolves — polled for 30 s with the queue's own served QueueUrl as Identifier, every call refused. The queue was readable through SQS GetQueueAttributes throughout, so the resource exists.
AWS: Cloud Control's resource operations operate on the live resource through the type's handler regardless of how it was created (Cloud Control API resource-operations documentation): GetResource for an existing AWS::SQS::Queue returns a ResourceDescription whose Identifier is the queue URL supplied and whose Properties carry the owning service's values (QueueName, VisibilityTimeout, Arn), and ListResources enumerates it. Only once the resource is gone is ResourceNotFoundException correct. Recorded behavior only.
Emulator: GetResource with the served QueueUrl of a natively created queue is refused for the whole 30 s budget ('last observation None'), so the subsequent check of the post-delete refusal can never be reached; the companion read/write-through check exhausts its 60 s budget on the same read. Hypothesis: the handler resolves only identifiers it minted itself in the synthesized form of Cause 1, so an identifier that is SQS's served URL — the only URL SQS will hand a client — matches nothing.
Why: Cloud Control is advertised as a uniform API over live resources; a client that creates a queue with SQS (or adopts an existing estate) cannot read, update or delete it through Cloud Control at all, and a generic provider built on Cloud Control sees an empty/partial account.
Fix: the Cloud Control AWS::SQS::Queue read/list handler (resolve the identifier against SQS's live queue state rather than Cloud Control's own request store, and normalize both URL forms of an identifier).
Traces: cloudcontrol__sqs.native-sqs-queue-delete-leaves-cloudcontrol-not-found, cloudcontrol__sqs.cloudcontrol-reads-and-writes-through-to-a-natively-created-queue.

Open issues on the same service name different causes (20261110T221719Z-cloudcontrol-the-aws-sqs-queue-handler-not-found.md on the not-found message's identifier rendering, 20261115T161642Z-cloudcontrol-sqs-the-queuedoesnotexist-refusal-message-is-the-legacy.md on SQS's refusal wording); neither carries these checks' replay keys.

## Evidence
Cause 1 — cloudcontrol-lifecycle-visible-to-native-sqs-api [mismatch]:

    AssertionError: cloudcontrol reports identifier 'https://sqs.us-west-2.amazonaws.com/000000000000/<queue>'; SQS resolves the queue name to a different URL, so the two services are not describing the same resource

Cause 2 — native-sqs-queue-delete-leaves-cloudcontrol-not-found [mismatch]:

    AssertionError: waited 30.0s for Cloud Control GetResource to resolve '<queue>', a queue it never created and it never happened; last observation None

  The call under poll is cc.get_resource(TypeName='AWS::SQS::Queue', Identifier=<the QueueUrl sqs.create_queue returned>); SQS GetQueueAttributes on the same URL succeeded immediately before. cloudcontrol-reads-and-writes-through-to-a-natively-created-queue [mismatch] ends as 'Check timed out after 60.0s' on the same read-through.

Expected (AWS): identifier equal to SQS's QueueUrl; GetResource returns ResourceDescription{Identifier=<that URL>, Properties with QueueName/VisibilityTimeout/Arn matching SQS}, ListResources enumerates it, and ResourceNotFoundException only after the queue is deleted.
Actual (emulator): a synthesized real-AWS-host identifier, and no resolution at all for a natively created queue. (Hosts, ports and queue names are per-run values.)

## Steps to Reproduce
Cause 1: cloudcontrol CreateResource(TypeName='AWS::SQS::Queue', DesiredState={QueueName, VisibilityTimeout, Tags}); wait for SUCCESS; compare ProgressEvent.Identifier with sqs GetQueueUrl(QueueName). Expected equal; observed Cloud Control reports https://sqs.<region>.amazonaws.com/<account>/<name>.
Cause 2: sqs CreateQueue(QueueName, Attributes={VisibilityTimeout: 53}); confirm GetQueueAttributes answers; then cloudcontrol GetResource(TypeName='AWS::SQS::Queue', Identifier=<the QueueUrl CreateQueue returned>) and ListResources(TypeName='AWS::SQS::Queue'). Expected: the resource is described/enumerated with SQS's values. Observed: GetResource is refused for the full 30 s poll and the queue never appears.
