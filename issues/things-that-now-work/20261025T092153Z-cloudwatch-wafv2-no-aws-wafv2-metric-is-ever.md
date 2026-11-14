Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions
Issue-Key: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions|_poll_until_steady#16|Timeout|time.sleep(interval_s)
Issue-Commit: dd185e64193b12242704f82dcc412d70cd33b9eb

# cloudwatch__wafv2: no AWS/WAFV2 metric is ever observable — a web-ACL-associated REST stage never answers, so BlockedRequests/CountedRequests under WebACL/Rule/Region are unreachable and the check hangs to the 60 s cap (1 cause)

> _Note: this issue was identified against the code base at commit `dd185e64193b12242704f82dcc412d70cd33b9eb`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateWebACL

## Description
Cause 1 of 1: once a WAFv2 web ACL is associated with a deployed API Gateway stage, requests to that stage never complete, so the WAF telemetry that depends on inspected requests is never produced and never readable at CloudWatch.
AWS: a REGIONAL web ACL associated with a REST API stage inspects every request and the stage answers with a bounded HTTP response — 403 for a path a Block rule matches, 200 for a path a Count-action rule matches (Count must not terminate the request), 403 for a rule that blocks with CloudWatchMetricsEnabled=false. WAF then publishes to the CloudWatch namespace AWS/WAFV2: BlockedRequests and CountedRequests, carrying the WebACL, Rule and Region dimensions together, with Sums equal to the exact number of requests each rule acted on, and datapoints whose CloudWatch Unit is "None"; a rule whose VisibilityConfig.CloudWatchMetricsEnabled is false publishes no datapoint at all even though it enforces identically. Smithy (com.amazonaws.wafv2): AssociateWebACL takes WebACLArn + ResourceArn (both required, ResourceArn @length 20..2048) and models WAFUnavailableEntityException/WAFInvalidParameterException/WAFNonexistentItemException at httpError 400; VisibilityConfig.CloudWatchMetricsEnabled and MetricName are the modeled switches for this emission. The per-case HTTP statuses and the metric namespace/dimension names are recorded behavior only (execute-api and the WAF metric pipeline are not in a Smithy model); what is structural is that every request to the associated stage produces an HTTP response, and that a rule with metrics enabled produces a series a customer can read back.
Emulator: the check is killed by the harness at its 60 s cap — detail is exactly "Check timed out after 60.0s". Its first observable condition is a steady window of 403s from GET <invoke_url>/quiet on the associated stage; no status is ever produced there, so none of the 7 blocked / 3 counted / 5 quiet-blocked requests is driven and no AWS/WAFV2 series is ever queried. Hypothesis: the hang is the same no-response data path that the open issue 20261025T081755Z-apigateway-wafv2-six-more-wafv2-at-the-edge.md records for six apigateway-scoped checks; the AWS/WAFV2 metric emission itself sits behind it and is therefore unverified rather than proven absent.
Why: a client fronted by WAF on an API Gateway stage gets no response at all instead of 200/403, and an operator gets no telemetry — without a series carrying WebACL + Rule + Region together a customer cannot tell which of several rules blocked their traffic, and CloudWatchMetricsEnabled cannot be distinguished from decoration (publishing nothing, publishing only a per-web-ACL total, and publishing regardless of the flag are all indistinguishable today).
Fix: the execute-api data plane's WAFv2 inspection path — a request to a stage with an associated web ACL must be evaluated and terminated with an HTTP response (403 on a terminating Block, 200 when only a Count rule matches) instead of being left open; behind it, the AWS/WAFV2 metric publisher that must emit BlockedRequests/CountedRequests under the WebACL/Rule/Region dimensions for rules with CloudWatchMetricsEnabled=true and nothing for those with it false.
Note on history: the fixed issue 20261024T065754Z-cloudwatch-wafv2-wafv2-cloudwatch-metric-dimensions-cannot-be-exercised-associatewebacl.md covered this same check when AssociateWebACL answered NotImplemented. AssociateWebACL no longer refuses the call — the check now reaches the data plane and hangs there instead — so this is the next, distinct blocker and not a refusal left behind by that fix. No open issue carries this check id.
Traces: cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions.

## Evidence
[mismatch] cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions — Error: Check timed out after 60.0s. No HTTP status line ever arrives from the web-ACL-associated stage, so the check's enforcement gate (a continuous 60 s window of 403s on GET <invoke_url>/quiet) never opens and CloudWatch is never read.

Expected (AWS): GET <invoke_url>/blocked -> 403 (Block rule, ENDS_WITH on the UriPath including the stage segment), GET <invoke_url>/counted -> 200 (Count rule must not terminate), GET <invoke_url>/quiet -> 403 (Block rule with CloudWatchMetricsEnabled=false). Then cloudwatch GetMetricStatistics on Namespace=AWS/WAFV2, MetricName=BlockedRequests, Dimensions WebACL=<acl>/Rule=<blocked rule metric>/Region=<region> settles at Sum 7; CountedRequests for the counted rule's metric settles at Sum 3; the datapoints carry Unit "None"; ListMetrics reports a dimension set containing WebACL, Rule and Region together; and the quiet rule's metric name yields no datapoint for either metric while its 403s are already proven.
Actual (emulator): nothing — no response from the stage within the 60 s harness cap, hence no metric read at all.

## Steps to Reproduce
1. Create a REGIONAL REST API with a {proxy+} resource and a MOCK integration returning 200 on ANY, and deploy it to stage 'prod'.
2. wafv2 CreateWebACL (Scope=REGIONAL, DefaultAction Allow) with three ByteMatchStatement/UriPath/ENDS_WITH rules: priority 0 '/blocked' Action Block with CloudWatchMetricsEnabled=true, priority 1 '/counted' Action Count with CloudWatchMetricsEnabled=true, priority 2 '/quiet' Action Block with CloudWatchMetricsEnabled=false (each with its own MetricName).
3. AssociateWebACL(WebACLArn=<acl arn>, ResourceArn=arn:aws:apigateway:<region>::/restapis/<api-id>/stages/prod), retrying WAFUnavailableEntityException.
4. GET <invoke_url>/quiet repeatedly and observe that no HTTP response is ever produced, so enforcement never becomes observable (the check is killed at 60 s here).
5. Had it answered: drive 7 GETs to /blocked, 3 to /counted, 5 to /quiet, then read Namespace AWS/WAFV2 BlockedRequests/CountedRequests under Dimensions WebACL/Rule/Region and ListMetrics for that WebACL.
6. Or replay cloudwatch__wafv2.blocked-and-counted-requests-carry-the-webacl-and-rule-dimensions.
