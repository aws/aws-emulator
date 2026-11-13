Issue-Source: trace-sweep-divergence
Issue-Trace: cloudwatch-mstx-traj-query-protocol-route
Issue-Step: task#2
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudwatch-mstx-traj-query-protocol-route cloudwatch-mstx-traj-get-name-mistakes cloudwatch-mstx-traj-stale-name-after-delete cloudwatch-mstx-traj-wrong-region-default
Issue-Shape-Id: com.amazonaws.cloudwatch#GraniteServiceVersion20100801
Issue-Key: monitoring|GetMetricStream|2xx->4xx|$.GetMetricStreamResponse
Issue-Key: monitoring|GetMetricStream|2xx->4xx|$headers.content-type
Issue-Key: monitoring|GetMetricStream|2xx->4xx|status
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$.ErrorResponse
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$headers.content-type
Issue-Key: monitoring|GetMetricStream|4xx->4xx|status
Issue-Key: monitoring|ListMetricStreams|4xx->4xx|$.ErrorResponse
Issue-Key: monitoring|ListMetricStreams|4xx->4xx|$headers.content-type
Issue-Key: monitoring|GetMetricStream|4xx->4xx|$.Message
Issue-Commit: 7923db9e5c097e7a9eee0c015b735053c707d1d4

# CloudWatch metric stream operations refuse awsQuery and emit empty 404 Message

> _Note: this issue was identified against the code base at commit `7923db9e5c097e7a9eee0c015b735053c707d1d4`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetMetricStream

## Description
This issue bundles FIVE causes. Four share the fault that the metric-stream operations do not serve the awsQuery form-encoded protocol at all; the fifth is the related defect that the JSON 404 body carries a member AWS does not send. There are 5 causes to close.

AWS background for causes 1-4: Smithy service GraniteServiceVersion20100801 (sdkId CloudWatch, endpointPrefix monitoring) declares protocols [awsJson1_0, awsQuery, rpcv2Cbor] with protocol modifier awsQueryCompatible and xmlNamespace http://monitoring.amazonaws.com/doc/2010-08-01/. A form-encoded Action=<Op>&Version=2010-08-01 request is therefore a first-class way to call every bound operation, and AWS answers it with Content-Type text/xml and an XML envelope named after the operation (<Op>Response/<Op>Result, or ErrorResponse/Error for a failure). The emulator answers such a request with 400 and Content-Type application/x-amz-json-1.0 - it never leaves the JSON protocol.

Related open issue: "CloudWatch awsQuery form-encoded protocol is not served at all" (20261029T203422Z-monitoring-cloudwatch-awsquery-form-encoded-protocol-is-not-served.md) carries the replay keys for the alarm-operation awsQuery clusters (cwq-query-protocol-alarm-round-trip-and-errors). It does NOT carry the four metric-stream keys below, so they are filed net-new; a fixer closing the alarm issue will not have exercised GetMetricStream or ListMetricStreams over awsQuery.

Cause 1 of 5: GetMetricStream over awsQuery, success path.
  AWS: 200 with Content-Type text/xml and body <GetMetricStreamResponse><GetMetricStreamResult>...</GetMetricStreamResult><ResponseMetadata><RequestId>...</RequestId></ResponseMetadata></GetMetricStreamResponse>. The result serialises every GetMetricStreamOutput member in awsQuery form - scalars as text (IncludeLinkedAccountsMetrics as the string "false", OutputFormat "json", State "running") and lists as <member> elements, including the nested StatisticsConfigurations/AdditionalStatistics and ExcludeFilters/MetricNames members. The ARNs, names, RequestId and the Creation/LastUpdate dates in the recording are per-run values; the requirement is the XML envelope, the content type, and the member set.
  Emulator: 400 with Content-Type application/x-amz-json-1.0 and no GetMetricStreamResponse element at all.
  Why: any SDK or tool pinned to the query protocol (older AWS SDKs, aws-cli with a query-protocol service model, hand-rolled SigV4 form posts) cannot read a metric stream at all.
  Fix: the CloudWatch request router / protocol dispatch and the awsQuery response serializer for GetMetricStream.
  Traces: cloudwatch-mstx-traj-query-protocol-route#task#2.

Cause 2 of 5: GetMetricStream over awsQuery, not-found path.
  AWS: 404 with Content-Type text/xml and body <ErrorResponse><Error><Type>Sender</Type><Code>ResourceNotFoundException</Code><Message/></Error><RequestId>...</RequestId></ErrorResponse>. Smithy: GetMetricStream models ResourceNotFoundException with httpError 404 and awsQueryError code ResourceNotFoundException / httpResponseCode 404; the recorded Message is empty.
  Emulator: 400 with application/x-amz-json-1.0 and no ErrorResponse envelope - the status is wrong as well as the protocol.
  Why: a query-protocol client cannot distinguish "no such stream" from a malformed request, because every outcome comes back 400 in the wrong wire format.
  Fix: the awsQuery error serializer and the error-to-status mapping for GetMetricStream.
  Traces: cloudwatch-mstx-traj-query-protocol-route#task#3.

Cause 3 of 5: GetMetricStream over awsQuery, invalid-name path.
  AWS: 400 with Content-Type text/xml and <ErrorResponse><Error><Type>Sender</Type><Code>InvalidParameterValue</Code><Message>Invalid Name</Message></Error><RequestId>...</RequestId></ErrorResponse>. Smithy: GetMetricStream models InvalidParameterValueException with awsQueryError code InvalidParameterValue; "Invalid Name" is fixed text ending in the member name.
  Emulator: 400 (status happens to match) but Content-Type application/x-amz-json-1.0 and no ErrorResponse element, so neither Code nor Message is readable by a query client.
  Fix: the awsQuery error serializer for GetMetricStream.
  Traces: cloudwatch-mstx-traj-query-protocol-route#task#4.

Cause 4 of 5: ListMetricStreams over awsQuery, MaxResults out of range.
  AWS: 400 with Content-Type text/xml and <ErrorResponse><Error><Type>Sender</Type><Code>ValidationError</Code><Message>1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1</Message></Error>...</ErrorResponse>. Smithy: ListMetricStreams.MaxResults targets ListMetricStreamsMaxResults with @range min 1, max 500; the message is the standard range-constraint template with the sent value and member path interpolated.
  Emulator: 400 with application/x-amz-json-1.0 and no ErrorResponse element.
  Fix: the awsQuery error serializer for ListMetricStreams.
  Traces: cloudwatch-mstx-traj-query-protocol-route#task#5.

Cause 5 of 5: the JSON GetMetricStream 404 body carries an empty Message member AWS does not send.
  AWS: Smithy: GetMetricStream models ResourceNotFoundException at httpError 404. The recorded 404 JSON body has NO Message member - it is absent, not empty.
  Emulator: 404 (status correct) but the body includes "Message": "" - a present member with an empty string.
  Why: a client that tests for the presence of Message to decide whether a human-readable reason exists sees one and then renders an empty reason; strict body comparisons also fail. The requirement is absence of the member, which is not a per-run value.
  Fix: the ResourceNotFoundException serializer for GetMetricStream - omit Message rather than defaulting it to the empty string.
  Traces: cloudwatch-mstx-traj-get-name-mistakes#task#7 (plus cloudwatch-mstx-traj-stale-name-after-delete#task#4, #task#7, cloudwatch-mstx-traj-wrong-region-default#task#2).

## Evidence
Cause 1 - GetMetricStream awsQuery success: expected status 200, header content-type "text/xml" and a $.GetMetricStreamResponse envelope containing GetMetricStreamResult (Name, Arn, FirehoseArn, RoleArn, State "running", OutputFormat "json", IncludeLinkedAccountsMetrics "false", ExcludeFilters/member, StatisticsConfigurations/member, CreationDate, LastUpdateDate) plus ResponseMetadata.RequestId; actual status 400, content-type "application/x-amz-json-1.0", GetMetricStreamResponse absent.
Cause 2 - GetMetricStream awsQuery not-found: expected status 404, content-type "text/xml", $.ErrorResponse = {Error: {Type: "Sender", Code: "ResourceNotFoundException", Message: None}, RequestId: ...}; actual status 400, content-type "application/x-amz-json-1.0", ErrorResponse absent.
Cause 3 - GetMetricStream awsQuery invalid name: expected content-type "text/xml" and $.ErrorResponse = {Error: {Type: "Sender", Code: "InvalidParameterValue", Message: "Invalid Name"}, RequestId: ...}; actual content-type "application/x-amz-json-1.0", ErrorResponse absent (status 400 both).
Cause 4 - ListMetricStreams awsQuery MaxResults=0: expected content-type "text/xml" and $.ErrorResponse = {Error: {Type: "Sender", Code: "ValidationError", Message: "1 validation error detected: Value '0' at 'maxResults' failed to satisfy constraint: Member must have value greater than or equal to 1"}, RequestId: ...}; actual content-type "application/x-amz-json-1.0", ErrorResponse absent (status 400 both).
Cause 5 - GetMetricStream JSON 404: expected $.Message absent; actual $.Message present and equal to "" (status 404 both).

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (replay_wire_corpus).
2. Cause 1: fixture cloudwatch-mstx-traj-query-protocol-route, step task#2 - POST a form-encoded body Action=GetMetricStream&Version=2010-08-01&Name=<existing stream> to the monitoring endpoint. Expect 200 text/xml with a GetMetricStreamResponse envelope; observe 400 application/x-amz-json-1.0.
3. Cause 2: same fixture, step task#3 - the same form-encoded call for a stream that does not exist. Expect 404 text/xml ErrorResponse with Code ResourceNotFoundException; observe 400 JSON.
4. Cause 3: same fixture, step task#4 - form-encoded GetMetricStream with an invalid Name. Expect 400 text/xml ErrorResponse with Code InvalidParameterValue and Message "Invalid Name"; observe 400 JSON.
5. Cause 4: same fixture, step task#5 - form-encoded Action=ListMetricStreams with MaxResults=0. Expect 400 text/xml ErrorResponse with Code ValidationError; observe 400 JSON.
6. Cause 5: fixture cloudwatch-mstx-traj-get-name-mistakes, step task#7 - JSON GetMetricStream for an absent stream. Expect a 404 body with no Message member; observe "Message": "".
