Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: glue.deploy-crawler-via-cloudformation glue.deploy-glue-crawler-via-cloudformation-runs-and-populates-catalog
Issue-Shape-Id: com.amazonaws.glue#AWSGlue
Issue-Key: glue.deploy-crawler-via-cloudformation|_start_crawler_with_retry#14|Timeout|time.sleep(interval_s)
Issue-Key: glue.deploy-glue-crawler-via-cloudformation-runs-and-populates-catalog|_start_crawler_with_retry#18|Timeout|time.sleep(5)
Issue-Commit: c3867ff5ad180ddb764d6f7e7211ec419ed4ca10

# Glue: a CloudFormation-deployed AWS::Glue::Crawler can never crawl — StartCrawler still answers 501 NotImplemented, so the deploy checks hang and no table is ever published (1 cause)

> _Note: this issue was identified against the code base at commit `c3867ff5ad180ddb764d6f7e7211ec419ed4ca10`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartCrawler

## Description
Cause 1 of 1: Glue StartCrawler is an unimplemented scaffold stub, so a crawler that CloudFormation deployed as AWS::Glue::Crawler can be read back through GetCrawler but can never be started, never crawls its S3 target and never publishes a table into the Data Catalog.

AWS: Smithy (services/service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json): StartCrawler takes Name (NameString, @length 1..255) as its only, required member and answers 200 with an EMPTY output shape; its modeled errors are exactly CrawlerRunningException, EntityNotFoundException and OperationTimeoutException (all client, HTTP 400) — neither a 501 status nor a NotImplemented code is a modeled outcome of this operation. Recorded behavior for the rest of the sequence these two checks assert: after an accepted start, GetCrawler.Crawler.State leaves READY and returns to READY with Crawler.LastCrawl present and LastCrawl.Status == "SUCCEEDED"; the crawl infers a schema from the bytes of the objects under the crawler's S3 target path and publishes a table into the crawler's DatabaseName, so GetTables(DatabaseName) lists at least one table whose StorageDescriptor.Location is under that s3:// prefix; and GetCrawlerMetrics(CrawlerNameList=[crawler]) reports TablesCreated + TablesUpdated >= 1.

Emulator: every StartCrawler is refused by the scaffold. services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED` marker and its handle() body ends in `Err(crate::scaffolded::Scaffolded::not_implemented("StartCrawler"))`, which serializes as HTTP 501, Error.Code `NotImplemented`, Error.Message `operation StartCrawler is scaffolded but not implemented in this emulator` — the fixed text is "operation " + the operation name taken from the request + " is scaffolded but not implemented in this emulator"; no part of it comes from Glue state. Both checks wrap the start in their own retry helper (_start_crawler_with_retry), which treats ANY error as IAM-role propagation and retries for 60s, so the stub refusal never surfaces as a NotImplemented error to the suite and both are reported as `Error: Check timed out after 60.0s` instead — which is why neither is attributed to the StartCrawler stub cluster in this run. Downstream, the crawler stays READY forever, no LastCrawl is ever recorded, GetTables stays empty, and the metrics assertion is never reached.

Why: no client can catalog S3 data with a crawler against this emulator, and in particular a CloudFormation stack that deploys AWS::Glue::Crawler reaches CREATE_COMPLETE while the resource it created is inert — the deploy looks successful and the crawl it exists to perform cannot be invoked at all. Any pipeline that starts a crawl and polls GetCrawler hangs rather than finishing, and an SDK built against the model sees an unmodeled 501/NotImplemented where it expects success or one of the three modeled client errors, so its retry/error branching mis-fires.

Fix: the operation's handler, services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs (delete the SCAFFOLDED marker when it is implemented), plus the crawler-run state that GetCrawler reports from (the State transitions and the LastCrawl record in the Glue service state), the schema inference and table publication a crawl performs against its S3 target, and the counters GetCrawlerMetrics reports. Related open issues, named so a fixer sees the overlap rather than treating this as unrelated work: 20261024T015202Z-glue-startcrawler-stub.md names the unimplemented operation and this same file; 20261030T063753Z-glue-startcrawler-refuses-every-crawl-501-notimplemented-so.md carries the same cause grounded in the athena__glue__s3 checks; 20261029T184212Z-glue-crawlers-never-run-and-a-glue-job.md carries it grounded in the emr__glue__s3 checks. NONE of them carries the two glue.* replay keys in trace_ids above, so none covers these two checks. Note also 20261029T185330Z-glue-getcrawlermetrics-stub.md: GetCrawlerMetrics is itself a 501 stub, so glue.deploy-crawler-via-cloudformation will still fail on its metrics assertion until that one is closed too.

Traces: glue.deploy-crawler-via-cloudformation, glue.deploy-glue-crawler-via-cloudformation-runs-and-populates-catalog.

## Evidence
Both checks: `Error: Check timed out after 60.0s`.

Expected (AWS): StartCrawler(Name=<crawler>) -> HTTP 200, empty body; then GetCrawler(Name) shows State leaving READY and returning to READY with LastCrawl.Status == "SUCCEEDED"; GetTables(DatabaseName=<db>) lists >= 1 table and at least one StorageDescriptor.Location starts with s3://<bucket>/; GetCrawlerMetrics(CrawlerNameList=[<crawler>]).CrawlerMetricsList[0] has TablesCreated + TablesUpdated >= 1.

Actual (emulator): StartCrawler -> HTTP 501, Error.Code "NotImplemented", Error.Message "operation StartCrawler is scaffolded but not implemented in this emulator" (the same stub refusal the suite reports verbatim for the other StartCrawler-blocked checks in this run, summarised there as: UNIMPLEMENTED `StartCrawler` (services/service-com-amazonaws-glue-awsglue/src/ops/start_crawler.rs), which blocks 34 check(s) in this suite). Message anatomy: fixed text "operation " + <operation name from the request> + " is scaffolded but not implemented in this emulator"; no variable part comes from Glue state. The refusal is swallowed by each check's own 60s start-retry loop, so the suite records a check timeout rather than the 501.

Per-check replay detail:
  - glue.deploy-crawler-via-cloudformation — the stack (AWS::Glue::Database + AWS::Glue::Crawler with Targets.S3Targets=[{Path: s3://<bucket>/data/}]) reaches CREATE_COMPLETE and GetCrawler/GetDatabase answer, then _start_crawler_with_retry (6 attempts, 5s apart) consumes the whole check budget before _wait_crawler_finished, the GetCrawlerMetrics assertion and the GetTables assertion.
  - glue.deploy-glue-crawler-via-cloudformation-runs-and-populates-catalog — same shape via a single AWS::Glue::Crawler resource; blocked in _start_crawler_with_retry before the GetCrawler State==READY poll, the LastCrawl.Status=="SUCCEEDED" assertion and GetTables.

Bucket, database and crawler names in both checks are per-run generated values that fill the template; they are not part of the requirement.

## Steps to Reproduce
1. s3:CreateBucket and s3:PutObject(Key="data/sample.csv", Body="id,name,value\n1,alpha,10\n2,beta,20\n3,gamma,30\n").
2. iam:CreateRole trusting glue.amazonaws.com, with s3:GetObject + s3:ListBucket on that bucket (the checks also attach AWSGlueServiceRole / AmazonS3FullAccess).
3. cloudformation:CreateStack with an AWS::Glue::Database and an AWS::Glue::Crawler (Name, Role=<role arn>, DatabaseName, Targets={"S3Targets":[{"Path":"s3://<bucket>/data/"}]}); poll DescribeStacks to CREATE_COMPLETE.
4. glue:GetCrawler(Name=<crawler>) and glue:GetDatabase(Name=<db>) — both answer, so the deployed resources are addressable.
5. glue:StartCrawler(Name=<crawler>).
   Expected: HTTP 200 with an empty body.
   Observed: HTTP 501 NotImplemented "operation StartCrawler is scaffolded but not implemented in this emulator" on every attempt for 60s, so the check is killed by its budget.
6. Poll glue:GetCrawler(Name=<crawler>): State stays READY and no LastCrawl member ever appears; glue:GetTables(DatabaseName=<db>).TableList stays empty; glue:GetCrawlerMetrics(CrawlerNameList=[<crawler>]) never reports TablesCreated/TablesUpdated >= 1.
7. Re-replay both check ids in trace_ids.
