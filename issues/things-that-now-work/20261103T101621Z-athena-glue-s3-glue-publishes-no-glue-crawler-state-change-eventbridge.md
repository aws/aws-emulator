Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive
Issue-Key: athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive|_start_crawler_with_retry#27|Timeout|time.sleep(2)
Issue-Commit: 5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a

# Glue publishes no "Glue Crawler State Change" EventBridge event, so a crawler-driven rule → Firehose → S3 archive pipeline receives nothing

> _Note: this issue was identified against the code base at commit `5d4a88d9a344cd4338be4fa17dd8ed09c8bb6d4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
One net-new root cause (Cause 1 of 1).

Cause 1 of 1: a Glue crawler run never reaches a terminal Succeeded state and Glue never publishes the crawler state-change event onto the default EventBridge bus, so nothing is ever delivered to a rule that matches it.

AWS: recorded behavior only — the Glue Smithy model describes the control plane (StartCrawler / GetCrawler) and models no eventing, so the event contract is taken from the recording. `StartCrawler` accepts a crawler whose Role trusts glue.amazonaws.com and whose S3 target exists and returns 200 with an empty body; the crawler leaves State=RUNNING, and on completion returns to State=READY with `Crawler.LastCrawl.Status` = "SUCCEEDED". At that transition Glue publishes one event onto the account's `default` event bus whose `source` is the fixed text `aws.glue`, whose `detail-type` is the fixed text `Glue Crawler State Change`, and whose `detail` carries `crawlerName` (the crawler name from the request) and `state` = `Succeeded`. A rule whose EventPattern matches that source/detail-type/detail then delivers the raw event envelope to its target — here a Firehose DirectPut delivery stream — which archives the JSON envelope as an object under the configured S3 prefix. Every variable part (crawler name, bucket names, stream name, event id/time) comes from the request and from state; the quoted `source`, `detail-type` and `state` strings are fixed text.

Emulator: the check never completes. The replay reports `Error: Check timed out after 60.0s`: the crawler never leaves its start state, so `_wait_for_crawler_state(..., {"READY"})` and the follow-on `_wait_for_archived_event(...)` both spin until the harness window is exhausted, and no object is ever archived under the `glue-events/` prefix. No crawler state-change envelope is observable anywhere in the pipeline.

Why: a client that wires crawler completion to downstream work — an EventBridge rule that kicks off ingestion, an archive of crawl outcomes, an alarm on failed crawls — sees nothing happen at all. The rule, the target and the delivery stream all report healthy, so the break is silent: the pipeline simply never fires, and a user debugging it has no refusal and no event to look at.

Fix: Glue's crawler execution path and its event publisher — the StartCrawler handler and the crawler state machine that must drive State RUNNING → READY and set `LastCrawl.Status`, plus the emission of the `aws.glue` / `Glue Crawler State Change` envelope onto the default bus at that transition. Related but NOT covering this check: open issue `20261030T063753Z-glue-startcrawler-refuses-every-crawl-501-notimplemented-so.md` records that StartCrawler refuses crawls with 501 NotImplemented; that issue is about the refusal of the call, while this check is grounded in the absent state transition and the absent state-change event, and the replay did not attribute it to that issue.

Traces: athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive. Handler/state to inspect: services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs and the crawler record in that crate's src/state.

## Evidence
Expected (AWS): start_crawler returns 200; get_crawler eventually reports State=READY with Crawler.LastCrawl.Status == "SUCCEEDED"; an object appears under s3://<archive-bucket>/glue-events/ whose JSON body has source == "aws.glue", detail-type == "Glue Crawler State Change", detail.crawlerName == <the crawler name> and detail.state == "Succeeded".

Actual (emulator): the crawler never reaches READY/SUCCEEDED and no object is ever archived under glue-events/. The replay verdict for the check is:

  [mismatch] athena__glue__s3.glue-crawler-state-change-event-delivered-to-target-positive
  Error: Check timed out after 60.0s

No event envelope, no refusal, and no failed-crawl status is produced — the pipeline is silent end to end.

## Steps to Reproduce
1. Create an S3 data bucket and put an object under `data/` in it; create a second S3 bucket to archive events into.
2. Create an IAM role trusted by glue.amazonaws.com with read access to the data bucket.
3. glue.create_database, then glue.create_crawler with Targets={"S3Targets":[{"Path":"s3://<data-bucket>/data/"}]} using that role.
4. Create a Firehose DirectPut delivery stream with an S3DestinationConfiguration pointing at the archive bucket with Prefix="glue-events/"; wait for it to be ACTIVE.
5. events.put_rule with EventPattern {"source":["aws.glue"],"detail-type":["Glue Crawler State Change"],"detail":{"crawlerName":[<crawler>],"state":["Succeeded"]}} and events.put_targets pointing at the delivery stream ARN.
6. glue.start_crawler(Name=<crawler>), then poll glue.get_crawler until State=="READY".
7. Observe: the crawler never reaches READY/LastCrawl.Status=="SUCCEEDED", and list_objects_v2 on the archive bucket under glue-events/ stays empty.
