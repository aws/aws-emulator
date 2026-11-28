Issue-Source: trace-sweep-divergence
Issue-Trace: s3-filter-prefix-byte-bound-by-configuration-kind
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: s3-filter-prefix-byte-bound-by-configuration-kind s3-itc-config-id-outside-the-allowed-characters-refused s3-inventory-report-prefix-byte-bounds
Issue-Shape-Id: com.amazonaws.s3#AmazonS3
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->2xx|$headers.content-length
Issue-Key: s3|PutBucketIntelligentTieringConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketIntelligentTieringConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketIntelligentTieringConfiguration|4xx->2xx|status
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketMetricsConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketMetricsConfigurations|2xx->2xx|$.ListMetricsConfigurationsResult.MetricsConfiguration
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketAnalyticsConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketAnalyticsConfigurations|2xx->2xx|$.ListBucketAnalyticsConfigurationsResult.AnalyticsConfiguration
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->2xx|$
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->2xx|$headers.content-length
Issue-Key: s3|PutBucketInventoryConfiguration|4xx->2xx|status
Issue-Key: s3|GetBucketInventoryConfiguration|4xx->2xx|$.Error
Issue-Key: s3|GetBucketInventoryConfiguration|4xx->2xx|status
Issue-Key: s3|ListBucketInventoryConfigurations|2xx->2xx|$.ListInventoryConfigurationsResult.InventoryConfiguration
Issue-Commit: 8e0150f548083d70861fe6ec8aea207c225cbc47

# s3 bucket analytics, metrics, inventory and intelligent-tiering configuration writes skip validation, are stored, and the paired reads diverge

> _Note: this issue was identified against the code base at commit `8e0150f548083d70861fe6ec8aea207c225cbc47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutBucketIntelligentTieringConfiguration

## Description
10 net-new causes sharing ONE fault: the bucket sub-resource configuration writes (intelligent-tiering, metrics, analytics, inventory) do not validate their payload. An invalid configuration is accepted with the success status, persisted, and therefore returned by the paired Get* (200 instead of the not-found refusal) and by the paired List* (an extra configuration in the result). Three distinct validation rules are missed and each one produces a write/read/list triple. The Smithy model declares no error shapes for these operations, so the codes and messages below are recorded behavior only; the limits named in each message are service limits (fixed text), not values from a request.

Cause 1 of 10 — PutBucketIntelligentTieringConfiguration accepts a filter Prefix longer than the byte bound. AWS: a Filter Prefix over 1024 bytes is refused 400 with Code 'InvalidPrefix' and Message 'Filter Prefix cannot be longer than 1024 bytes.' (fixed text; 1024 is the limit, not a request value), and nothing is stored. Emulator: 204 No Content with an empty body and no content-length, and the configuration is stored. Why: a client that measures its prefix against the service limit by probing gets a false 'accepted', then builds a tiering rule AWS would never have. Fix: the PutBucketIntelligentTieringConfiguration input validation, before the state write. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#4, s3-itc-config-id-outside-the-allowed-characters-refused#task#2 (and #task#3, the Id character-set rule on the same handler).

Cause 2 of 10 — GetBucketIntelligentTieringConfiguration serves the configuration Cause 1 should have rejected. AWS: with no such configuration the read answers 404 with Code 'NoSuchConfiguration' and Message 'The specified configuration does not exist.' (fixed text). Emulator: 200 with the stored configuration and no Error element. Why: the read confirms a configuration that AWS says does not exist, so an idempotent 'create if absent' loop never creates it. Fix: follows from Cause 1 — the write must not persist; the read's not-found branch must answer 404 NoSuchConfiguration. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#5, s3-itc-config-id-outside-the-allowed-characters-refused#task#5.

Cause 3 of 10 — PutBucketMetricsConfiguration accepts a payload that does not validate against the schema. AWS: refused 400 with Code 'MalformedXML' and Message 'The XML you provided was not well-formed or did not validate against our published schema' (fixed text), nothing stored. Emulator: 204 No Content, stored. Why/Fix: as Cause 1, in the metrics-configuration validator. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#8.

Cause 4 of 10 — ListBucketMetricsConfigurations returns the configuration Cause 3 should have rejected. AWS: the result holds only the configurations that were accepted. Emulator: the rejected one is present in the list. Why: a reconciliation client sees state AWS would not have. Fix: follows from Cause 3. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#9.

Cause 5 of 10 — PutBucketAnalyticsConfiguration accepts a filter Prefix longer than the byte bound. AWS: 400 'InvalidPrefix' / 'Filter Prefix cannot be longer than 1024 bytes.', nothing stored — and the bound is counted in BYTES, so a multi-byte prefix that is short in characters still exceeds it. Emulator: 204 No Content, stored. Why/Fix: as Cause 1, in the analytics-configuration validator; the length check must be over encoded bytes. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#12 (and #task#13).

Cause 6 of 10 — GetBucketAnalyticsConfiguration serves the configuration Cause 5 should have rejected. AWS: 404 'NoSuchConfiguration' / 'The specified configuration does not exist.' Emulator: 200 with the configuration. Why/Fix: as Cause 2. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#14.

Cause 7 of 10 — ListBucketAnalyticsConfigurations returns the configuration Cause 5 should have rejected. AWS: only accepted configurations appear. Emulator: the over-bound one is listed alongside the valid one. Why/Fix: as Cause 4. Traces: s3-filter-prefix-byte-bound-by-configuration-kind#task#15.

Cause 8 of 10 — PutBucketInventoryConfiguration accepts an S3BucketDestination Prefix longer than its byte bound. AWS: refused 400 with Code 'InvalidS3DestinationBucketPrefix' and Message 'S3BucketDestination Prefix cannot be longer than 512 bytes.' (fixed text; 512 is the limit), nothing stored. Note the destination prefix bound (512) differs from the filter prefix bound (1024) of Causes 1/5, and is also counted in bytes. Emulator: 204 No Content, stored. Why/Fix: as Cause 1, in the inventory-configuration validator. Traces: s3-inventory-report-prefix-byte-bounds#task#4 (and #task#5, #task#6).

Cause 9 of 10 — GetBucketInventoryConfiguration serves the configuration Cause 8 should have rejected. AWS: 404 'NoSuchConfiguration' / 'The specified configuration does not exist.' Emulator: 200 with the configuration. Why/Fix: as Cause 2. Traces: s3-inventory-report-prefix-byte-bounds#task#7.

Cause 10 of 10 — ListBucketInventoryConfigurations returns the configuration Cause 8 should have rejected. AWS: only accepted configurations appear. Emulator: the over-bound one is listed. Why/Fix: as Cause 4. Traces: s3-inventory-report-prefix-byte-bounds#task#8.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator.
Representative s3-filter-prefix-byte-bound-by-configuration-kind#task#4 (PutBucketIntelligentTieringConfiguration): status expected 400, got 204; body empty_body — expected {'Error': {'Code': 'InvalidPrefix', 'Message': 'Filter Prefix cannot be longer than 1024 bytes.', ...}}, got {}; $headers.content-length missing.
Other causes, same shape:
- #task#5 GetBucketIntelligentTieringConfiguration: status expected 404 got 200; $.Error missing (expected Code 'NoSuchConfiguration', Message 'The specified configuration does not exist.').
- #task#8 PutBucketMetricsConfiguration: status expected 400 got 204; expected Error Code 'MalformedXML'.
- #task#9 ListBucketMetricsConfigurations: $.ListMetricsConfigurationsResult.MetricsConfiguration type_mismatch — the recording holds a single accepted configuration, the emulator returns a list that also contains the rejected 'edge' configuration.
- #task#12 PutBucketAnalyticsConfiguration: status expected 400 got 204; expected Error Code 'InvalidPrefix'.
- #task#14 GetBucketAnalyticsConfiguration: status expected 404 got 200; $.Error missing ('NoSuchConfiguration').
- #task#15 ListBucketAnalyticsConfigurations: the result additionally contains the rejected 'ascii-over' configuration.
- s3-inventory-report-prefix-byte-bounds#task#4 PutBucketInventoryConfiguration: status expected 400 got 204; expected Error Code 'InvalidS3DestinationBucketPrefix', Message 'S3BucketDestination Prefix cannot be longer than 512 bytes.'
- #task#7 GetBucketInventoryConfiguration: status expected 404 got 200; $.Error missing ('NoSuchConfiguration').
- #task#8 ListBucketInventoryConfigurations: the rejected configuration is listed.

## Steps to Reproduce
1. Replay fixture s3-filter-prefix-byte-bound-by-configuration-kind. At step task#4 the PutBucketIntelligentTieringConfiguration with an over-bound filter prefix must be refused 400 InvalidPrefix with no state change, not answered 204.
2. Still in that fixture, re-check task#5 (Get must be 404 NoSuchConfiguration), task#8/#9 (metrics write refused MalformedXML, list must not contain it), task#12/#14/#15 (analytics).
3. Replay s3-inventory-report-prefix-byte-bounds and check task#4 (refused InvalidS3DestinationBucketPrefix), task#7 (404 NoSuchConfiguration), task#8 (list excludes it).
4. Replay s3-itc-config-id-outside-the-allowed-characters-refused for the Id character-set rule on the same handler.
5. For every write, assert the byte-length bound is measured over encoded bytes (1024 for a filter Prefix, 512 for an inventory S3BucketDestination Prefix), and that a refused write leaves the configuration absent from both Get and List.
