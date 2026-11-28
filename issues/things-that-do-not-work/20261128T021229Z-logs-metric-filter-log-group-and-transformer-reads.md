Issue-Source: trace-sweep-divergence
Issue-Trace: logs-put-transformer-split-string-delimiter-length-bounds
Issue-Step: task#5
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-put-transformer-split-string-delimiter-length-bounds logs-put-transformer-substitute-string-source-length-bounds logs-transformer-substitute-string-entry-count-bounds logsmovekeys-transformer-move-keys-entry-count-bounds logs-transformer-csv-column-count-bound logs-describe-log-groups-name-pattern-character-set logstagkey-create-log-group-tag-key-length-bounds logs-get-log-group-fields-identifier-arn-forms logs-metric-filter-namespace-reserved-characters logs-put-metric-filter-namespace-length-bounds logsdimlen-put-metric-filter-dimension-length-bounds logs-scheduled-query-log-group-identifier-count-bounds logs-scheduled-query-name-length-bound logs-scheduled-query-s3-destination-role-arn-length-bound logs-scheduled-query-s3-owner-account-id-pattern
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*]
Issue-Key: logs|GetLogGroupFields|2xx->2xx|$.logGroupFields[name="@aws.account"]
Issue-Key: logs|GetLogGroupFields|2xx->2xx|$.logGroupFields[name="@aws.region"]
Issue-Key: logs|GetLogGroupFields|2xx->2xx|$.logGroupFields[name="@data_format"]
Issue-Key: logs|GetLogGroupFields|2xx->2xx|$.logGroupFields[name="@data_source_name"]
Issue-Key: logs|GetLogGroupFields|2xx->2xx|$.logGroupFields[name="@data_source_type"]
Issue-Key: logs|GetLogGroupFields|2xx->2xx|$.logGroupFields[name="@log"]
Issue-Key: logs|DescribeMetricFilters|2xx->2xx|$.metricFilters[*]
Issue-Key: logs|DescribeMetricFilters|2xx->2xx|$.metricFilters[*].filterName
Issue-Key: logs|DescribeMetricFilters|2xx->2xx|$.metricFilters[*].metricTransformations[*].metricNamespace
Issue-Key: logs|DescribeMetricFilters|2xx->2xx|$.metricFilters[*].metricTransformations[*].dimensions.Field
Issue-Key: logs|CreateScheduledQuery|2xx->2xx|$.scheduledQueryArn
Issue-Related: 20261128T021229Z-logs-read-projections-omit-members-aws-always-emits.md
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs metric-filter, log-group and transformer reads project the wrong record identity and content

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetTransformer

## Description
This issue bundles 8 causes that share ONE fault: read operations project a record whose IDENTITY or whose stored CONTENT is not the one AWS would serve — the identifier is echoed in a normalised form the caller never sent, created records are missing from the listing, and the field/dimension catalogue served is the emulator's rather than the service's.

Cause 1 of 8: GetTransformer echoes logGroupIdentifier as a full ARN. AWS: the response's `logGroupIdentifier` echoes the identifier form the REQUEST used — a bare log-group name stays a bare name. Emulator: normalises it to `arn:aws:logs:<region>:<account>:log-group:<name>`. Why: a client that compares the echoed identifier with the one it sent, or uses it as a map key, never matches. Fix: the GetTransformer projection must echo the request's identifier verbatim instead of the resolved ARN. Traces: logs-put-transformer-split-string-delimiter-length-bounds#task#5 (also logs-put-transformer-substitute-string-source-length-bounds#task#3, #task#6, logs-transformer-substitute-string-entry-count-bounds#task#3, #task#6, logsmovekeys-transformer-move-keys-entry-count-bounds#task#5).

Cause 2 of 8: the same normalisation on the CSV transformer read path. AWS/Emulator: as cause 1. Why/Fix: as cause 1. Traces: logs-transformer-csv-column-count-bound#task#3 (also task#5).

Cause 3 of 8: DescribeLogGroups does not return a log group that was created. AWS: the created group is listed with arn (`...:log-group:<name>:*`), logGroupArn (`...:log-group:<name>`), logGroupClass STANDARD, logGroupName, metricFilterCount 0 and creationTime. Emulator: the page is empty. Why: a client cannot confirm creation, so any create-then-describe workflow breaks for names in that character range. Fix: the log-group index's name handling / DescribeLogGroups prefix matching. Traces: logs-describe-log-groups-name-pattern-character-set#task#4 (also logstagkey-create-log-group-tag-key-length-bounds#task#3).

Cause 4 of 8: GetLogGroupFields serves the wrong discovered-field set. AWS: the response includes the service's synthetic fields `@data_source_type`, `@data_source_name`, `@data_format`, `@aws.region` and `@aws.account`, each at `percent: 100`, and does NOT include `@log`. Emulator: omits all five and emits `@log` at percent 100. Why: a client enumerating queryable fields is shown a vocabulary CloudWatch Logs Insights would not accept and is missing the ones it would. Fix: GetLogGroupFields' synthetic-field catalogue. Traces: logs-get-log-group-fields-identifier-arn-forms#task#4.

Cause 5 of 8: DescribeMetricFilters serves the wrong filters after reserved-character namespaces. AWS: the filters created with namespaces containing `:`, `$` and `*` are all listed (entries for the colon, dollar and star variants, each with its metricTransformations and applyOnTransformedLogs false). Emulator: lists only the slash variant and nothing at indexes 1 and 3. Why: a client cannot see metric filters that AWS created, so reconciliation deletes or recreates them. Fix: follows from the PutMetricFilter namespace-pattern defect filed alongside, plus the DescribeMetricFilters projection. Traces: logs-metric-filter-namespace-reserved-characters#task#6.

Cause 6 of 8: DescribeMetricFilters serves the wrong filter at index 0. AWS: the listing is ordered so that the filter named `<prefix>-ok-<id>` is first and carries its long `Payments/Checkout/...` namespace. Emulator: puts the `-empty-` filter first with an empty namespace and pushes the `-ok-` filter to index 1. Why: a client reading the first page sees a different filter than AWS serves, and a filter with an empty namespace AWS would not have stored. Fix: the DescribeMetricFilters ordering and the stored namespace (see the namespace-pattern issue). Traces: logs-put-metric-filter-namespace-length-bounds#task#5.

Cause 7 of 8: DescribeMetricFilters loses a dimension value. AWS: the first listed filter is `<prefix>-ok-<id>` and its metricTransformations[0].dimensions carries the `Field` key with its long `$.<...>` value. Emulator: lists the `-key-` filter first and omits the `Field` dimension entirely. Why: a client reading dimensions back cannot reconstruct the filter it created. Fix: as cause 6, plus dimension-map round-tripping. Traces: logsdimlen-put-metric-filter-dimension-length-bounds#task#5.

Cause 8 of 8: DescribeMetricFilters returns a filter AWS does not list. AWS: no filter is listed at that index. Emulator: lists the `-key-` filter with a 255-character dimension KEY. Why: a filter with a dimension key beyond AWS's bound exists only here. Fix: the dimension-key bound in PutMetricFilter plus the projection. Traces: logsdimlen-put-metric-filter-dimension-length-bounds#task#7.

Cause 9 (bundled under cause 8's fault, identity half): CreateScheduledQuery's scheduledQueryArn does not match the recorded ARN shape. AWS: recorded behavior only — the recorded value is shape-redacted, and the emulator's `arn:aws:logs:<region>:<account>:scheduled-query:<name>` fails that shape check. Hypothesis: the service mints the scheduled-query resource segment from a generated id rather than echoing the caller's name. Why: a client that stores the returned ARN and later resolves it diverges. Fix: CreateScheduledQuery's ARN minting in the scheduled-query state. Traces: logs-scheduled-query-log-group-identifier-count-bounds#task#7 (also logs-scheduled-query-name-length-bound#task#6, logs-scheduled-query-s3-destination-role-arn-length-bound#task#6, logs-scheduled-query-s3-owner-account-id-pattern#task#7).

## Evidence
logs-put-transformer-split-string-delimiter-length-bounds#task#5 (200/200): expected $.logGroupIdentifier 'sim2real-probe-logs-splitdelim-<id>' (the name as sent), actual 'arn:aws:logs:<region>:<account>:log-group:sim2real-probe-logs-splitdelim-<id>'. Same on logs-transformer-csv-column-count-bound#task#3.
logs-describe-log-groups-name-pattern-character-set#task#4: expected logGroups[0] = {arn: '...:log-group:<name>:*', logGroupArn: '...:log-group:<name>', logGroupClass: 'STANDARD', logGroupName: <name>, metricFilterCount: 0, creationTime: <shaped>}, actual None.
logs-get-log-group-fields-identifier-arn-forms#task#4: expected @data_source_type, @data_source_name, @data_format, @aws.region, @aws.account each at percent 100 and no @log; actual only @log at percent 100.
logs-metric-filter-namespace-reserved-characters#task#6: expected the '-colon', '-dollar' and '-star' filters, actual only the '-slash' filter.
logs-put-metric-filter-namespace-length-bounds#task#5: expected metricFilters[0].filterName '<prefix>-ok-<id>' with the long namespace, actual '<prefix>-empty-<id>' with namespace '' and the '-ok-' filter at index 1.
logsdimlen-put-metric-filter-dimension-length-bounds#task#5: expected metricFilters[0].metricTransformations[0].dimensions.Field '$.aaa...', actual missing; #task#7: expected no filter at index 0, actual the '-key-' filter with a 255-character dimension key.
logs-scheduled-query-log-group-identifier-count-bounds#task#7: expected $.scheduledQueryArn <shaped-not-asserted>, actual 'arn:aws:logs:<region>:<account>:scheduled-query:<name>' fails that shape.

## Steps to Reproduce
1. Call PutTransformer with a bare log-group NAME, then GetTransformer with the same name (logs-put-transformer-split-string-delimiter-length-bounds#task#5, logs-transformer-csv-column-count-bound#task#3): logGroupIdentifier comes back as an ARN.
2. Create a log group whose name uses the full legal character set and call DescribeLogGroups (logs-describe-log-groups-name-pattern-character-set#task#4): the page is empty.
3. Call GetLogGroupFields (logs-get-log-group-fields-identifier-arn-forms#task#4): the synthetic-field set is '@log' instead of AWS's five @data_source_*/@aws.* fields.
4. Create metric filters with reserved-character namespaces, boundary namespaces and boundary dimension keys/values, then call DescribeMetricFilters (logs-metric-filter-namespace-reserved-characters#task#6, logs-put-metric-filter-namespace-length-bounds#task#5, logsdimlen-put-metric-filter-dimension-length-bounds#task#5 and #task#7): the listing's membership, order and dimension content diverge.
5. Call CreateScheduledQuery (logs-scheduled-query-log-group-identifier-count-bounds#task#7): the returned scheduledQueryArn fails the recorded ARN shape.
