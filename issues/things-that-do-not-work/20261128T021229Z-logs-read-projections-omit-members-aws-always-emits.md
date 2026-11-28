Issue-Source: trace-sweep-divergence
Issue-Trace: logs-query-definition-name-length-bounds
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: logs-query-definition-name-length-bounds logs-query-definition-query-string-length-bounds logs-query-definition-parameter-count-bound logs-query-definition-parameter-description-length-bounds logs-query-definition-parameter-default-value-length-bounds logs-query-definition-parameter-name-length-bounds logs-query-definition-parameter-name-pattern describe-resource-policies-policy-scope-values logs-associate-kms-key-resource-identifier-character-class logs-transformer-csv-column-count-bound logs-transformer-parse-to-ocsf-version-enum put-transformer-datetime-target-length-bounds
Issue-Shape-Id: com.amazonaws.cloudwatchlogs#Logs_20140328
Issue-Key: logs|DescribeQueryDefinitions|2xx->2xx|$.queryDefinitions[*].logGroupNames
Issue-Key: logs|DescribeQueryDefinitions|2xx->2xx|$.queryDefinitions[*].parameters
Issue-Key: logs|PutResourcePolicy|2xx->2xx|$.revisionId
Issue-Key: logs|DescribeResourcePolicies|2xx->2xx|$.resourcePolicies[*].revisionId
Issue-Key: logs|DescribeLogGroups|2xx->2xx|$.logGroups[*].kmsKeyId
Issue-Key: logs|GetTransformer|2xx->2xx|$.logGroupIdentifier
Issue-Key: logs|GetTransformer|2xx->2xx|$.transformerConfig[*].csv.delimiter
Issue-Key: logs|GetTransformer|2xx->2xx|$.transformerConfig[*].csv.quoteCharacter
Issue-Key: logs|GetTransformer|2xx->2xx|$.transformerConfig[*].parseToOCSF.mappingVersion
Issue-Key: logs|GetTransformer|2xx->2xx|$.transformerConfig[*].dateTimeConverter.overwriteIfExists
Issue-Commit: d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3

# logs read projections omit members AWS always emits and mint emulator-shaped revision ids

> _Note: this issue was identified against the code base at commit `d7b90cb0555dc69b04a63d1b15d7fc089e6fcfe3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeQueryDefinitions

## Description
This issue bundles 9 causes that share ONE fault: the emulator's read projections emit only the members it happens to have stored, so members CloudWatch Logs always materialises — empty collections, server-applied defaults, the associated KMS key, the policy revision — are missing or invented. Every cause is a 200/200 body mismatch.

Cause 1 of 9: DescribeQueryDefinitions omits logGroupNames and parameters. AWS: each queryDefinitions entry always carries `logGroupNames` and `parameters`, as `[]` when the definition has none. Emulator: omits both. Why: a client iterating `logGroupNames` dereferences a null instead of an empty list. Fix: the DescribeQueryDefinitions projection must emit both members, empty when unset. Traces: logs-query-definition-name-length-bounds#task#4 (also logs-query-definition-query-string-length-bounds#task#3).

Cause 2 of 9: the same omission across multiple definitions in one page. AWS: `logGroupNames: []` on every entry. Emulator: omitted on entries 0 and 1. Why/Fix: as cause 1. Traces: logs-query-definition-parameter-count-bound#task#5.

Cause 3 of 9: the same omission on parameterised definitions. Why/Fix: as cause 1. Traces: logs-query-definition-parameter-default-value-length-bounds#task#4 (also task#6, logs-query-definition-parameter-name-length-bounds#task#4, #task#6, logs-query-definition-parameter-name-pattern#task#5).

Cause 4 of 9: PutResourcePolicy's revisionId. AWS: a resource policy's revision is a monotonic counter whose first value is `1`. Emulator: returns a 32-character all-zero string. Why: a client doing optimistic concurrency with expectedRevisionId cannot reason about ordering, and a revision that never changes defeats the check entirely. Fix: the resource-policy state must carry an incrementing revision and the projection must render it. Traces: describe-resource-policies-policy-scope-values#task#2.

Cause 5 of 9: DescribeResourcePolicies replays the same placeholder revision. AWS: `resourcePolicies[].revisionId` is `1` for a freshly written policy. Emulator: the 32-zero string. Why/Fix: follows from cause 4, in the DescribeResourcePolicies projection. Traces: describe-resource-policies-policy-scope-values#task#3.

Cause 6 of 9: DescribeLogGroups omits kmsKeyId after AssociateKmsKey. AWS: once a KMS key is associated, the log group is projected with `kmsKeyId` set to that key's ARN. Emulator: omits the member. Why: a client verifying encryption-at-rest configuration sees an unencrypted group, so an association that succeeded looks like it did not. Fix: AssociateKmsKey must record the key on the log-group record and DescribeLogGroups must project it. Traces: logs-associate-kms-key-resource-identifier-character-class#task#5.

Cause 7 of 9: GetTransformer omits the CSV processor's server-applied defaults. AWS: a `csv` processor is stored with its defaults materialised — `delimiter: ","` and `quoteCharacter: "\""` — and they are echoed on read. Emulator: omits both. Why: a client round-tripping the transformer config loses the delimiter and quote character and re-submits an under-specified processor. Fix: PutTransformer must materialise the csv defaults into state, and the GetTransformer projection must emit them. Traces: logs-transformer-csv-column-count-bound#task#3 (also task#5).

Cause 8 of 9: GetTransformer omits parseToOCSF.mappingVersion. AWS: a parseToOCSF processor is stored with the server-chosen `mappingVersion` (e.g. the mapping release corresponding to the requested ocsfVersion) and echoes it. Emulator: omits the member. Why/Fix: as cause 7. Traces: logs-transformer-parse-to-ocsf-version-enum#task#5 (also task#7).

Cause 9 of 9: GetTransformer omits dateTimeConverter.overwriteIfExists. AWS: the boolean default `false` is materialised and echoed. Emulator: omits the member. Why/Fix: as cause 7. Traces: put-transformer-datetime-target-length-bounds#task#3 (also task#6).

## Evidence
logs-query-definition-name-length-bounds#task#4 (200/200): expected queryDefinitions[0].logGroupNames [] and queryDefinitions[0].parameters [], actual both missing. Same on logs-query-definition-parameter-count-bound#task#5 (entries 0 and 1) and logs-query-definition-parameter-default-value-length-bounds#task#4.
describe-resource-policies-policy-scope-values#task#2: expected $.revisionId '1', actual '00000000000000000000000000000000'; #task#3: expected resourcePolicies[0].revisionId '1', actual the same 32-zero string.
logs-associate-kms-key-resource-identifier-character-class#task#5: expected logGroups[0].kmsKeyId 'arn:aws:kms:<region>:<account>:key/<key-id>', actual missing.
logs-transformer-csv-column-count-bound#task#3: expected transformerConfig[0].csv.delimiter ',' and .quoteCharacter '"', actual both missing.
logs-transformer-parse-to-ocsf-version-enum#task#5: expected transformerConfig[0].parseToOCSF.mappingVersion 'v1.5.0', actual missing.
put-transformer-datetime-target-length-bounds#task#3: expected transformerConfig[1].dateTimeConverter.overwriteIfExists false, actual missing.

## Steps to Reproduce
1. Put a query definition with no log groups and call DescribeQueryDefinitions (fixtures logs-query-definition-name-length-bounds#task#4, logs-query-definition-parameter-count-bound#task#5, logs-query-definition-parameter-default-value-length-bounds#task#4): logGroupNames and parameters are absent instead of [].
2. Call PutResourcePolicy then DescribeResourcePolicies (describe-resource-policies-policy-scope-values#task#2, #task#3): revisionId is a 32-zero placeholder instead of '1'.
3. Call AssociateKmsKey on a log group then DescribeLogGroups (logs-associate-kms-key-resource-identifier-character-class#task#5): kmsKeyId is absent.
4. Call PutTransformer with csv, parseToOCSF and dateTimeConverter processors and read it back with GetTransformer (logs-transformer-csv-column-count-bound#task#3, logs-transformer-parse-to-ocsf-version-enum#task#5, put-transformer-datetime-target-length-bounds#task#3): the server-applied defaults are missing.
