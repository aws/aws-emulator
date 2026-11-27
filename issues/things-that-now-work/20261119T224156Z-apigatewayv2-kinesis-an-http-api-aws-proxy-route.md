Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key
Issue-Key: apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key|_poll_http#26|TimeoutError|raise TimeoutError(
Issue-Commit: bea5cdf57d487a31229ef2924227deee2fe47ff2

# apigatewayv2__kinesis: an HTTP API AWS_PROXY route with IntegrationSubtype Kinesis-PutRecord answers 500 Internal Server Error instead of writing the record and returning ShardId/SequenceNumber

> _Note: this issue was identified against the code base at commit `bea5cdf57d487a31229ef2924227deee2fe47ff2`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutRecord

## Description
Cause 1 of 1: the execute-api data plane cannot dispatch the first-class AWS service integration subtype `Kinesis-PutRecord`, so a route bound to it is permanently a 500 and nothing is ever written to the stream.

AWS: an HTTP API route whose target is an AWS_PROXY integration with `IntegrationSubtype=Kinesis-PutRecord`, `PayloadFormatVersion=1.0`, a `CredentialsArn` granting `kinesis:PutRecord` on the stream, and `RequestParameters` mapping all three required parameters (StreamName, Data, PartitionKey — here Data and PartitionKey mapped from `$request.body.data` / `$request.body.pk`) resolves those mappings from the incoming request, calls Kinesis PutRecord under the integration's credentials, and passes PutRecord's answer through to the client. Smithy (apigatewayv2): CreateIntegration.IntegrationSubtype is a modeled member alongside IntegrationType=AWS_PROXY; the subtype catalogue is documentation-only (https://docs.aws.amazon.com/apigateway/latest/developerguide/http-api-develop-integrations-aws-services-reference.html) — recorded behavior only for the data-plane mapping. Smithy (kinesis): PutRecord output has required members ShardId and SequenceNumber (plus EncryptionType), so the client must get HTTP 200 and a body carrying a non-empty ShardId and SequenceNumber; and the record readable at that SequenceNumber must carry Data exactly equal to the mapped `$request.body.data` value and PartitionKey exactly equal to the mapped `$request.body.pk` value.

Emulator: the signed POST on the route answers HTTP 500 with body {"message":"Internal Server Error"} on every attempt for the whole 47 s poll budget — it never becomes 200. The fixed text of the diverging body is {"message":"Internal Server Error"} with no variable part, where AWS would answer a PutRecord-shaped JSON body (ShardId and SequenceNumber taken from the stream's state). Because the call never reaches Kinesis, no record lands on the stream either. The control-plane half of the same check now passes: CreateIntegration with RequestParameters={} is correctly refused BadRequestException "Operation: Kinesis-PutRecord requires enabling passthrough, or defining all of the following parameters in the parameter mapping: [StreamName, Data, PartitionKey]" (fixed text, with the subtype name and its declared parameter list as the variable parts), and GetIntegration returns PartitionKey stored as authored ("$request.body.pk"). Only the invoke-time dispatch is missing. The open issues 20261109T154749Z-apigatewayv2-kinesis-createintegration-accepts-a-kinesis-putrecord-aws.md and 20261118T133240Z-apigatewayv2-kinesis-apigatewayv2-the-kinesis-putrecord-subtype-refusal-lists-the.md are about that CreateIntegration refusal, not this data-plane 500; no filed issue carries this replay key.

Why: a client using the documented no-code HTTP API -> Kinesis ingest path loses every record silently behind an opaque 500. The caller cannot tell a transient server fault from a permanently broken route, so it retries indefinitely, and any producer that publishes through such a route writes nothing to the stream while appearing to be merely degraded.

Fix: the apigatewayv2 execute-api data-plane integration dispatcher for IntegrationType=AWS_PROXY with an IntegrationSubtype — the branch that resolves the subtype to its target service operation, evaluates each RequestParameters mapping expression ($request.body.<path> and literals) into that operation's input, assumes CredentialsArn, invokes the operation and serializes its output JSON back to the client. Today that branch has no arm for Kinesis-PutRecord and faults into the generic 500 responder.

Traces: check apigatewayv2__kinesis.aws-proxy-subtype-kinesis-putrecord-required-parameters-refused-else-lands-mapped-data-and-key (error: TimeoutError, last answer 500 '{"message":"Internal Server Error"}' from POST /put on the $default stage).

## Evidence
Expected: POST <stage-url>/put (SigV4-signed, AuthorizationType=AWS_IAM) with body {"data":"<marker>","pk":"<key>"} -> HTTP 200 and a PutRecord-shaped JSON body with non-empty ShardId and SequenceNumber; GetRecords at that SequenceNumber returns a record whose Data decodes to <marker> and whose PartitionKey equals <key>.

Actual: HTTP 500 with body {"message":"Internal Server Error"} on every poll until the check's 47 s budget expired; no record reaches the stream.

Suite error:
  TimeoutError: POST /put reaching the Kinesis-PutRecord integration (.../_aws/execute-api/<apiId>/put) never reached an accepted status within 47s. Last: 500 '{"message":"Internal Server Error"}'

The same check's negative half and the GetIntegration round-trip of the "$request.body.pk" mapping passed, so only invoke-time dispatch of the subtype diverges.

## Steps to Reproduce
1. kinesis: CreateStream(StreamModeDetails={StreamMode: ON_DEMAND}); wait for StreamStatus ACTIVE and ListShards to succeed.
2. iam: CreateRole trusting apigateway.amazonaws.com with an inline policy allowing kinesis:PutRecord on the stream ARN.
3. apigatewayv2: CreateApi(ProtocolType=HTTP); CreateIntegration(IntegrationType=AWS_PROXY, IntegrationSubtype="Kinesis-PutRecord", PayloadFormatVersion="1.0", CredentialsArn=<role>, RequestParameters={StreamName: <stream>, Data: "$request.body.data", PartitionKey: "$request.body.pk"}); CreateRoute(RouteKey="POST /put", Target=integrations/<id>, AuthorizationType=AWS_IAM); CreateStage(StageName="$default", AutoDeploy=true).
4. Send a SigV4-signed (service execute-api) POST to <stage-url>/put with Content-Type: application/json and body {"data":"<marker>","pk":"<key>"}.
5. Observe HTTP 500 {"message":"Internal Server Error"} instead of 200 with ShardId/SequenceNumber, and no record on the stream.
