Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue__sqs.glue-crawler-event-queue-batch-of-multiple-paths-positive glue__sqs.glue-crawler-sqs-dlq-receives-malformed-event-rejected glue__sqs.deploy-glue-crawler-role-trust-policy-positive glue__sqs.deploy-glue-crawler-with-sqs-event-queue-positive glue__sqs.deploy-stack-update-changes-sqs-queue-arn-positive
Issue-Key: glue|GetPartitions|EntityNotFoundException
Issue-Key: glue__sqs.glue-crawler-sqs-dlq-receives-malformed-event-rejected|run#29|AssertionError|assert remaining == 0, f"malformed message should have been drained from the event queue, {remaining} left"
Issue-Key: glue__sqs.deploy-glue-crawler-role-trust-policy-positive|_poll_until#43|Timeout|last = probe()
Issue-Key: glue__sqs.deploy-glue-crawler-with-sqs-event-queue-positive|_poll_until#30|Timeout|last = probe()
Issue-Key: glue__sqs.deploy-stack-update-changes-sqs-queue-arn-positive|_poll_until#31|Timeout|last = probe()
Issue-Commit: e18aca9896c14b293d2c6f422ce254ac7004d2d3

# glue__sqs: 2 net-new causes — an event-mode crawler neither deletes the S3 event messages it reads nor catalogues the tables for the crawled paths, and three CFN-deployed crawler/event-queue flows never reach a verdict

> _Note: this issue was identified against the code base at commit `e18aca9896c14b293d2c6f422ce254ac7004d2d3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Two net-new root causes in the glue__sqs event-mode crawler path. Neither failing cluster is marked ALREADY FILED, and none of these five checks is OWNED by an open issue.

Cause 1 of 2: a crawler configured with an S3 target EventQueueArn does not consume its queue — it leaves the messages it processed on the event queue and creates no catalogue table for the crawled child prefixes.
AWS: Smithy (glue) CreateCrawler/UpdateCrawler -> S3Target{Path, EventQueueArn, DlqEventQueueArn} and RecrawlPolicy{RecrawlBehavior} with the CRAWL_EVENT_MODE value; GetPartitions refuses an absent table with EntityNotFoundException (errorKind client, httpError 400). Recorded behavior: after a full first crawl, an incremental (event-mode) crawl receives the queued S3 'ObjectCreated' notifications, DELETES each message it has consumed from the event queue (so ApproximateNumberOfMessages returns to 0), routes nothing to the DLQ for a message it merely cannot parse, and reflects each consumed path as a new partition on the catalogue table that covers that path — two messages for two distinct child prefixes produce exactly one new partition on each of the two tables.
Emulator: the crawl leaves the queue untouched and produces no such table. With two queued events for two child prefixes, glue GetPartitions for the expected table is refused with EntityNotFoundException and the Message 'Table <name> not found.' (<name> is the requested table name, observed as 'tablea'; the rest of the message is fixed text) — the crawl catalogued no table under that name. With one malformed message queued ahead of a well-formed one, the malformed message is still on the event queue after the crawl (1 message remaining) instead of having been drained.
Why: the whole S3-event-driven crawler pattern is silently inert. A client wiring S3 -> SQS -> Glue crawler sees neither the new partitions it depends on nor a drained queue, so downstream consumers re-read the same events forever and queries over the crawled location find no table; a malformed poison message is never removed, so the queue only grows.
Fix: the crawler run executor in the glue service — the event-mode crawl path must receive-and-delete from the configured EventQueueArn (and only route to DlqEventQueueArn for the documented cases) and must create/update one catalogue table per crawled include path with the discovered partitions. The open issue 20261111T052620Z-glue-sqs-3-net-new-causes-an-event.md records a related event-mode table-NAMING cause, but its replay set names neither of these two checks, so neither recorded unit is re-replayed by it.
Traces: glue__sqs.glue-crawler-event-queue-batch-of-multiple-paths-positive, glue__sqs.glue-crawler-sqs-dlq-receives-malformed-event-rejected.

Cause 2 of 2: three CloudFormation-deployed crawler/event-queue flows never reach any verdict — the check is cut off at the harness cap with no assertion ever evaluated.
AWS: recorded behavior only. On AWS each of these flows settles: the stack reaches CREATE_COMPLETE (and UPDATE_COMPLETE for the rewire case), the deployed crawler's event-mode run reaches a terminal state, the S3 event is consumed off the queue it is pointed at and the partition appears, so each check runs to a pass/fail decision well inside its budget.
Emulator: all three stop with 'Check timed out after 60.0s' — no AWS error, no assertion failure, just no progress to a verdict. The only evidence is the timeout itself, so the blocking step is not pinned by the replay output; the three checks all poll a deployed crawler's event-mode run and the queue behind it, which is the same surface Cause 1 shows inert, so that path is the first place to look.
Why: the three flows that cover the deployment-time wiring of crawler -> SQS (trust policy, first wiring, and a stack update that repoints the notification and crawler at a second queue) are unverifiable: a client cannot tell whether the emulator wires them at all, and the stack-update rewire in particular could be silently keeping events on the old queue.
Fix: whatever step of the CloudFormation-deployed crawler + SQS event path fails to settle — begin with the crawler run lifecycle and the SQS event consumption named in Cause 1, then the stack UPDATE that repoints S3 NotificationConfiguration and the crawler's EventQueueArn.
Traces: glue__sqs.deploy-glue-crawler-role-trust-policy-positive, glue__sqs.deploy-glue-crawler-with-sqs-event-queue-positive, glue__sqs.deploy-stack-update-changes-sqs-queue-arn-positive.

## Evidence
Replay output (behavioral suite, section ### glue__sqs):

Cause 1:
  [error] glue__sqs.glue-crawler-event-queue-batch-of-multiple-paths-positive
    botocore.errorfactory.EntityNotFoundException: An error occurred (EntityNotFoundException) when calling the GetPartitions operation: Table tablea not found.
    Expected (AWS): after one incremental crawl over the two queued S3 events, GetPartitions answers 200 for each of the two tables with exactly one new partition each.
    Actual (emulator): the table the crawl was expected to catalogue does not exist, so GetPartitions is refused ('tablea' is the request's table name; 'Table ' / ' not found.' is fixed text).
  [mismatch] glue__sqs.glue-crawler-sqs-dlq-receives-malformed-event-rejected
    AssertionError: malformed message should have been drained from the event queue, 1 left
    Expected (AWS): the crawler deletes the malformed message from its event queue (0 remaining), adds no partition for it and leaves the DLQ empty; the well-formed event queued after it adds one partition.
    Actual (emulator): the malformed message is still on the event queue after the crawl.

Cause 2:
  [mismatch] glue__sqs.deploy-glue-crawler-role-trust-policy-positive — Error: Check timed out after 60.0s
  [mismatch] glue__sqs.deploy-glue-crawler-with-sqs-event-queue-positive — Error: Check timed out after 60.0s
  [mismatch] glue__sqs.deploy-stack-update-changes-sqs-queue-arn-positive — Error: Check timed out after 60.0s
Expected (AWS): each flow settles and reaches an assertion verdict. Actual (emulator): no verdict is reached; the run is cut at the harness cap with no error returned by any API call.

## Steps to Reproduce
Cause 1:
1. Build and boot the emulator from this checkout (glue, s3 and sqs all serve).
2. Create an S3 bucket with two child prefixes under one include path (e.g. data/tablea/dt=.../ and data/tableb/dt=.../), an SQS event queue and a DLQ, and a crawler role allowed on them.
3. glue CreateCrawler with an S3Target carrying Path = the include path, EventQueueArn = the event queue and DlqEventQueueArn = the DLQ, plus RecrawlPolicy RecrawlBehavior = CRAWL_EVENT_MODE. Run a first full crawl to establish the tables.
4. Put one new object under each child prefix and enqueue the corresponding S3 ObjectCreated notifications; StartCrawler and wait for the crawl to finish.
5. glue GetPartitions for each expected table: expect 200 with one new partition each; observe EntityNotFoundException 'Table <name> not found.'
6. SQS GetQueueAttributes on the event queue: expect ApproximateNumberOfMessages 0 (the crawler deleted what it consumed); observe the messages still present.
7. Repeat with a malformed message queued ahead of a well-formed S3 event: expect the malformed one drained and the DLQ empty; observe 1 message left on the event queue.
8. Or re-replay: glue__sqs.glue-crawler-event-queue-batch-of-multiple-paths-positive, glue__sqs.glue-crawler-sqs-dlq-receives-malformed-event-rejected.

Cause 2:
1. Deploy the CloudFormation stack each check builds (S3 bucket with a NotificationConfiguration onto an SQS queue, the queue policy, the crawler role and an AWS::Glue::Crawler wired to that queue's ARN), then enqueue an S3 event and run the crawler in event mode.
2. For the rewire case, UpdateStack so both the bucket notification and the crawler point at a second queue, then enqueue a post-update event.
3. Expect each flow to settle and the assertions to be evaluated; observe the check cut off with no verdict.
4. Or re-replay: glue__sqs.deploy-glue-crawler-role-trust-policy-positive, glue__sqs.deploy-glue-crawler-with-sqs-event-queue-positive, glue__sqs.deploy-stack-update-changes-sqs-queue-arn-positive.
