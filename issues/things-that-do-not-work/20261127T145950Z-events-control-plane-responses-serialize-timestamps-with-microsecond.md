Issue-Source: trace-sweep-divergence
Issue-Trace: create-connection-header-parameters-count-bound
Issue-Step: task#1
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: create-connection-header-parameters-count-bound events-api-destination-invocation-rate-limit-floor events-create-api-destination-name-character-grammar events-create-connection-body-parameters-count-bound create-event-bus-log-level-outside-enum-rejected events-create-event-bus-dead-letter-arn-length-bound events-create-event-bus-kms-key-identifier-length-bound events-delete-event-bus-name-length-bounds events-update-event-bus-log-level-enum update-event-bus-name-at-256-characters-and-one-over events-api-destination-create-members-round-trip events-update-api-destination-invocation-endpoint-escapes events-update-api-destination-rate-limit-minimum events-archive-and-replay-listings-limit-at-maximum-and-one-over events-create-archive-event-source-arn-length-bound events-list-archives-state-filter-enum list-archives-name-prefix-pattern-slash-refused events-cancel-replay-name-length events-create-archive-description-length-bound events-start-replay-description-length events-start-replay-filter-arn-member-length-bound events-start-replay-filter-rule-and-destination-bus-must-exist events-update-archive-description-length events-list-api-destinations-next-token-length events-list-connections-name-prefix-character-set events-list-connections-name-prefix-length-bounds list-event-buses-name-prefix-pattern events-put-permission-action-length-bound events-put-permission-event-bus-name-length-bounds
Issue-Shape-Id: com.amazonaws.eventbridge#AWSEvents
Issue-Key: events|CreateConnection|2xx->2xx|$.CreationTime
Issue-Key: events|CreateConnection|2xx->2xx|$.LastModifiedTime
Issue-Key: events|DescribeConnection|2xx->2xx|$.CreationTime
Issue-Key: events|DescribeConnection|2xx->2xx|$.LastAuthorizedTime
Issue-Key: events|DescribeConnection|2xx->2xx|$.LastModifiedTime
Issue-Key: events|DeleteConnection|2xx->2xx|$.CreationTime
Issue-Key: events|DeleteConnection|2xx->2xx|$.LastAuthorizedTime
Issue-Key: events|DeleteConnection|2xx->2xx|$.LastModifiedTime
Issue-Key: events|DescribeEventBus|2xx->2xx|$.CreationTime
Issue-Key: events|DescribeEventBus|2xx->2xx|$.LastModifiedTime
Issue-Key: events|CreateApiDestination|2xx->2xx|$.CreationTime
Issue-Key: events|CreateApiDestination|2xx->2xx|$.LastModifiedTime
Issue-Key: events|DescribeApiDestination|2xx->2xx|$.CreationTime
Issue-Key: events|DescribeApiDestination|2xx->2xx|$.LastModifiedTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[*].CreationTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[*].LastModifiedTime
Issue-Key: events|ListArchives|2xx->2xx|$.Archives[*].CreationTime
Issue-Key: events|CreateArchive|2xx->2xx|$.CreationTime
Issue-Key: events|StartReplay|2xx->2xx|$.ReplayStartTime
Issue-Key: events|DescribeReplay|2xx->2xx|$.ReplayEndTime
Issue-Key: events|DescribeReplay|2xx->2xx|$.ReplayStartTime
Issue-Key: events|DescribeArchive|2xx->2xx|$.CreationTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-a-fb832fd9"].CreationTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-a-fb832fd9"].LastModifiedTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-b-3edc90f6"].CreationTime
Issue-Key: events|ListApiDestinations|2xx->2xx|$.ApiDestinations[Name="sim2real-probe-events-pgtok-b-3edc90f6"].LastModifiedTime
Issue-Key: events|ListConnections|2xx->2xx|$.Connections[*].CreationTime
Issue-Key: events|ListConnections|2xx->2xx|$.Connections[*].LastAuthorizedTime
Issue-Key: events|ListConnections|2xx->2xx|$.Connections[*].LastModifiedTime
Issue-Key: events|UpdateApiDestination|2xx->2xx|$.CreationTime
Issue-Key: events|UpdateApiDestination|2xx->2xx|$.LastModifiedTime
Issue-Key: events|UpdateArchive|2xx->2xx|$.CreationTime
Issue-Key: events|ListEventBuses|2xx->2xx|$.EventBuses[*].CreationTime
Issue-Key: events|ListEventBuses|2xx->2xx|$.EventBuses[*].LastModifiedTime
Issue-Key: events|DescribeEventBus|2xx->2xx|$.Policy
Issue-Commit: f9b7321d6a22920f7cbed43b33d059dedecbc1ee

# events: control-plane responses serialize timestamps with microsecond precision and the bus policy principal as a bare account id

> _Note: this issue was identified against the code base at commit `f9b7321d6a22920f7cbed43b33d059dedecbc1ee`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConnection

## Description
Two response-serialization faults in the events control plane. 18 replayed divergence clusters, 2 root causes.

Cause 1 of 2: every control-plane timestamp is serialized as an epoch-seconds float carrying microsecond precision.
AWS: these members all target the Timestamp shape and EventBridge speaks awsJson1_1, whose default timestamp encoding is epoch-seconds (Smithy: service AWSEvents, protocols [awsJson1_1]; CreateConnection/DescribeConnection/DeleteConnection CreationTime, LastModifiedTime, LastAuthorizedTime; CreateEventBus/DescribeEventBus/ListEventBuses CreationTime, LastModifiedTime; CreateApiDestination/DescribeApiDestination/UpdateApiDestination/ListApiDestinations CreationTime, LastModifiedTime; CreateArchive/DescribeArchive/UpdateArchive/ListArchives CreationTime; StartReplay/DescribeReplay ReplayStartTime, ReplayEndTime). The precision AWS actually serves on the wire is at most milliseconds in the fractional part - recorded behavior only for that limit; the model states only the type.
Emulator: emits the raw clock value with 6-7 fractional digits on every one of those members, e.g. CreationTime 1795790861.212797 and LastModifiedTime 1795790861.212797 on CreateConnection, ReplayStartTime 1795790861.560619 on StartReplay, ListApiDestinations entries with CreationTime 1795790926.3490767. Replay marks each as 'value_mismatch (expected <shaped-not-asserted>, got <microsecond float>)' - the fixture asserts the shape of the value, not the instant, and the shape is wrong.
Why: a client that re-serializes, hashes, caches or diffs these values, or that validates the response against the API shape, sees a precision real EventBridge never emits; equality against a value it previously stored (or against a CloudTrail/AWS-SDK-formatted timestamp) fails, and any consumer that assumes millisecond-resolution epochs mis-parses the tail digits.
Fix: the shared timestamp serializer used by the events response serializers - one place; every operation listed above inherits it.
Traces: create-connection-header-parameters-count-bound#task#1 (CreateConnection), #task#2 (DescribeConnection), #task#5 (DeleteConnection); create-event-bus-log-level-outside-enum-rejected#task#4 (DescribeEventBus); events-api-destination-create-members-round-trip#task#6, #task#7, #task#8; events-archive-and-replay-listings-limit-at-maximum-and-one-over#task#3 (ListArchives); events-cancel-replay-name-length#setup#2 (CreateArchive), #task#5 (StartReplay), #task#7 (DescribeReplay); events-create-archive-description-length-bound#task#3 (DescribeArchive); events-list-api-destinations-next-token-length#task#8; events-list-connections-name-prefix-character-set#task#2; events-update-api-destination-invocation-endpoint-escapes#task#7; events-update-archive-description-length#task#4; list-event-buses-name-prefix-pattern#task#2; events-put-permission-action-length-bound#task#5.

Cause 2 of 2: the resource policy minted by PutPermission renders the granted principal as a bare 12-digit account id instead of that account's root-user ARN.
AWS: PutPermission accepts Principal as a 12-digit account id or '*' (Smithy: PutPermission.Principal @length 1..12 @pattern ^(\d{12}|\*)$) and stores a statement whose Principal.AWS is the account's root-user ARN, so a following DescribeEventBus returns Principal {"AWS":"arn:aws:iam::<account-id>:root"} - recorded behavior for the expansion; the fixed part is the template arn:aws:iam::<account-id>:root and the only variable part is the account id taken from the request's Principal.
Emulator: returns the statement with Principal {"AWS":"<account-id>"} - the request value stored verbatim, with no ARN expansion.
Why: a client that reads the bus policy back to diff it against the IAM policy document it intended (or that re-submits the policy through PutPermission's Policy member, or renders it in a console/IaC drift check) sees a principal form AWS never returns, so every comparison and every policy-equality test reports drift that does not exist.
Fix: PutPermission's policy-statement construction in the events event-bus state (the Principal field of the generated statement), which DescribeEventBus then serializes.
Traces: events-put-permission-action-length-bound#task#5 (DescribeEventBus; the same cluster also carries Cause 1's timestamps).

## Evidence
Cause 1 (replay detail, CreateConnection, create-connection-header-parameters-count-bound#task#1, HTTP 200 both sides): "$.CreationTime: value_mismatch (expected '<shaped-not-asserted>', got 1795790861.212797)"; "$.LastModifiedTime: value_mismatch (expected '<shaped-not-asserted>', got 1795790861.212797)". Same shape of failure on DescribeConnection (+LastAuthorizedTime), DescribeEventBus, Create/Describe/Update/ListApiDestinations, Create/Describe/Update/ListArchives, StartReplay ($.ReplayStartTime: got 1795790861.560619) and DescribeReplay ($.ReplayEndTime: got 1795790864.565592). Expected: an epoch-seconds value whose fractional part does not exceed millisecond resolution. Actual: 6-7 fractional digits.

Cause 2 (replay detail, DescribeEventBus, events-put-permission-action-length-bound#task#5, HTTP 200 both sides): "$.Policy: value_mismatch (expected '{"Version":"2012-10-17","Statement":[{"Sid":"grant-own-account","Effect":"Allow","Principal":{"AWS":"arn:aws:iam::000000000000:root"},"Action":"events:PutEvents","Resource":"arn:aws:events:us-west-2:000000000000:event-bus/<bus>"}]}', got '{..."Principal":{"AWS":"000000000000"}...}')". Only the Principal.AWS rendering differs; account id, Sid, Action and Resource are the request's own values.

## Steps to Reproduce
1. CreateConnection (or CreateEventBus / CreateApiDestination / CreateArchive / StartReplay), then read it back with the matching Describe*/List* call; inspect the fractional digits of CreationTime, LastModifiedTime, LastAuthorizedTime, ReplayStartTime, ReplayEndTime - the emulator returns microsecond-precision epoch floats.
2. CreateEventBus, then PutPermission(EventBusName=<bus>, StatementId='grant-own-account', Action='events:PutEvents', Principal=<own 12-digit account id>), then DescribeEventBus(<bus>) and read $.Policy - the statement's Principal.AWS is the bare account id instead of arn:aws:iam::<account-id>:root.
3. Re-replay the fixtures listed under Traces.
