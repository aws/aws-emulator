Issue-Source: trace-sweep-divergence
Issue-Trace: iot-certificate-provider-error-paths
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: iot-certificate-provider-error-paths iot-certificate-provider-lifecycle iot-stream-error-paths iot-stream-lifecycle iot-stream-lifecycle-and-errors iot-dynamic-thing-group-lifecycle iot-dynamic-thing-group-workflow
Issue-Shape-Id: com.amazonaws.iot#AWSIotService
Issue-Key: iot|CreateCertificateProvider|4xx->2xx|$.message
Issue-Key: iot|CreateCertificateProvider|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateCertificateProvider|4xx->2xx|status
Issue-Key: iot|DeleteCertificateProvider|2xx->4xx|$
Issue-Key: iot|DeleteCertificateProvider|2xx->4xx|status
Issue-Key: iot|DescribeCertificateProvider|2xx->2xx|$.lambdaFunctionArn
Issue-Key: iot|CreateStream|4xx->2xx|$.message
Issue-Key: iot|CreateStream|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateStream|4xx->2xx|status
Issue-Key: iot|DeleteStream|4xx->2xx|$
Issue-Key: iot|DeleteStream|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|DeleteStream|4xx->2xx|status
Issue-Key: iot|CreateDynamicThingGroup|4xx->2xx|$.message
Issue-Key: iot|CreateDynamicThingGroup|4xx->2xx|$headers.x-amzn-errortype
Issue-Key: iot|CreateDynamicThingGroup|4xx->2xx|status
Issue-Commit: a6ac5f2812657951a989dc8ceb784d022fa28a4a

# iot dependent-resource pre-flight is missing on creates and updates are not persisted: certificate providers, streams and dynamic thing groups (6 causes)

> _Note: this issue was identified against the code base at commit `a6ac5f2812657951a989dc8ceb784d022fa28a4a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCertificateProvider

## Description
Six net-new wire divergences sharing one fault: creates that AWS refuses because a resource they depend on is unusable (a Lambda function, an IAM role, the fleet index) are accepted, and the certificate-provider update is not persisted. Six causes must be closed.

Cause 1 of 6: CreateCertificateProvider accepts a lambdaFunctionArn that cannot be validated.
  AWS: Smithy: CreateCertificateProvider -> POST /certificate-providers/{certificateProviderName}; lambdaFunctionArn and accountDefaultForOperations are @required; modeled errors include InvalidRequestException (httpError 400). Recorded response: 400 with x-amzn-errortype 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/' and message 'Access denied during lambda validation: ' + <the lambdaFunctionArn from the request>. Fixed text: 'Access denied during lambda validation: '; variable part: the function ARN the request named. AWS invokes/validates the function before creating the provider.
  Emulator: 200 with a created provider; no message, no x-amzn-errortype.
  Why: the provider is reported ready while the function behind it does not exist or does not permit iot.amazonaws.com, so CreateCertificateFromCsr later fails with no diagnosable cause, and the operator never learns the ARN or its permission is wrong.
  Fix: CreateCertificateProvider's validation path — a Lambda existence/permission pre-flight ahead of state insertion.
  Traces: iot-certificate-provider-error-paths#task#1 (representative).

Cause 2 of 6: DeleteCertificateProvider refuses a provider name that does not exist instead of answering success.
  AWS: recorded behavior only — Smithy models ResourceNotFoundException (404) on DeleteCertificateProvider, but the recorded response for a never-created name is 200 with an empty body.
  Emulator: 404 with {'__type': 'ResourceNotFoundException', 'message': 'Certificate provider with name [<name>] not found'}.
  Why: idempotent cleanup that AWS lets succeed fails against the emulator, aborting a teardown path.
  Fix: the DeleteCertificateProvider handler's unknown-name path.
  Traces: iot-certificate-provider-error-paths#task#3.

Cause 3 of 6: UpdateCertificateProvider does not persist the new lambdaFunctionArn.
  AWS: Smithy: UpdateCertificateProvider -> PUT /certificate-providers/{certificateProviderName} with optional lambdaFunctionArn; DescribeCertificateProvider -> GET the same path with output member lambdaFunctionArn. Recorded response after updating the provider to a second function: DescribeCertificateProvider reports the NEW function ARN.
  Emulator: DescribeCertificateProvider still reports the ARN the provider was created with; the update is accepted but dropped.
  Why: a client that repoints a certificate provider at a new function and verifies the change is told the update did not take, and certificate issuance keeps running through the old function.
  Fix: the UpdateCertificateProvider handler's write into the certificate-provider state (and lastModifiedDate alongside it).
  Traces: iot-certificate-provider-lifecycle#task#11.

Cause 4 of 6: CreateStream accepts a roleArn that AWS IoT cannot assume.
  AWS: Smithy: CreateStream -> POST /streams/{streamId}; files and roleArn are @required; modeled errors include InvalidRequestException (httpError 400). Recorded response: 400 with message 'Permission denied when attempting to assume role ' + <the roleArn from the request> + '.' Fixed text: 'Permission denied when attempting to assume role ' and the trailing '.'; variable part: the role ARN from the request.
  Emulator: 200 with a created stream; no message and no error type header.
  Why: the OTA stream is reported ready while IoT cannot read the S3 files behind it, so the job that uses the stream fails later with an unrelated symptom.
  Fix: CreateStream's validation path — an assume-role pre-flight ahead of state insertion. Open issue 20261114T082236Z-iot-refusal-message-fidelity-7-causes-addthingtothinggroup-createstream.md covers CreateStream's duplicate-id CONFLICT wording from the behavioral corpus, not this role pre-flight and not this wire key.
  Traces: iot-stream-error-paths#task#4, iot-stream-lifecycle#task#3, iot-stream-lifecycle-and-errors#task#3.

Cause 5 of 6: DeleteStream answers 200 for a stream whose create AWS refused.
  AWS: Smithy: DeleteStream -> DELETE /streams/{streamId}, errors include ResourceNotFoundException (httpError 404). Recorded response: 404 with message 'Requested stream ' + <streamId> + ' does not exist in given account ' + <account id>. Fixed text: 'Requested stream ' and ' does not exist in given account '; variable parts: the stream id from the request and the caller's account id.
  Emulator: 200 with an empty body.
  Why: delete reports removing a stream that should never have existed, and the AWS-worded 404 (which names the account) is never produced.
  Fix: the DeleteStream handler's not-found path (plus cause 4, which is why the record exists).
  Traces: iot-stream-lifecycle#task#4, iot-stream-lifecycle-and-errors#task#4.

Cause 6 of 6: CreateDynamicThingGroup succeeds while fleet indexing is disabled.
  AWS: Smithy: CreateDynamicThingGroup -> POST /dynamic-thing-groups/{thingGroupName}; queryString is @required; modeled errors include ResourceNotFoundException (httpError 404) and InvalidQueryException (400). Recorded response when thing indexing has not been enabled: 404 with x-amzn-errortype 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/' and message 'AWS IoT Fleet Indexing is not enabled. Please enable index by calling UpdateIndexingConfiguration.' — entirely fixed text, no variable part. A dynamic group is backed by the fleet index, so AWS refuses until the index exists.
  Emulator: 200 with a created dynamic thing group.
  Why: the caller is told a query-backed group exists when AWS would have refused and told it to call UpdateIndexingConfiguration; the group then never populates, and the real misconfiguration is never surfaced.
  Fix: CreateDynamicThingGroup's validation path — require an enabled thing index (named indexName or the default) before creating the group.
  Traces: iot-dynamic-thing-group-lifecycle#task#1, iot-dynamic-thing-group-workflow#task#5.

## Evidence
Wire corpus simparity-wire-aws-v1 replayed against the emulator built from this checkout. All six clusters are verdict 'mismatch' with empty owned_by and empty stub_path.

Cause 1 — iot-certificate-provider-error-paths#task#1 (CreateCertificateProvider):
  expected status 400, got 200
  $.message: missing (expected 'Access denied during lambda validation: arn:aws:lambda:<region>:<account>:function:<fn>', got None)
  $headers.x-amzn-errortype: missing (expected 'InvalidRequestException:http://iot.amazonaws.com/doc/2015-10-02/', got None)

Cause 2 — iot-certificate-provider-error-paths#task#3 (DeleteCertificateProvider):
  expected status 200, got 404
  $: expected {}, got {'__type': 'ResourceNotFoundException', 'message': 'Certificate provider with name [<name>] not found'}

Cause 3 — iot-certificate-provider-lifecycle#task#11 (DescribeCertificateProvider), status 200:
  $.lambdaFunctionArn: expected the ARN of the SECOND function (…:function:<name>-fn2-…), got the ARN of the first (…:function:<name>-fn1-…)

Cause 4 — iot-stream-error-paths#task#4 (CreateStream):
  expected status 400, got 200
  $.message: missing (expected 'Permission denied when attempting to assume role arn:aws:iam::<account>:role/<role>.', got None)
  $headers.x-amzn-errortype: missing (expected a shaped error type, got None)
  also affected: iot-stream-lifecycle#task#3, iot-stream-lifecycle-and-errors#task#3

Cause 5 — iot-stream-lifecycle#task#4 (DeleteStream):
  expected status 404, got 200
  $: expected {'message': 'Requested stream <streamId> does not exist in given account <account>'}, got {}
  also affected: iot-stream-lifecycle-and-errors#task#4

Cause 6 — iot-dynamic-thing-group-lifecycle#task#1 (CreateDynamicThingGroup):
  expected status 404, got 200
  $.message: missing (expected 'AWS IoT Fleet Indexing is not enabled. Please enable index by calling UpdateIndexingConfiguration.', got None)
  $headers.x-amzn-errortype: missing (expected 'ResourceNotFoundException:http://iot.amazonaws.com/doc/2015-10-02/', got None)
  also affected: iot-dynamic-thing-group-workflow#task#5

Function ARNs, role ARNs, stream ids, provider names and account ids above are per-run probe values; the requirements are the statuses, error codes, the fixed text of each message plus the request/state values it interpolates, and that the update is reflected by the following read.

Smithy consulted: services/service-com-amazonaws-iot-awsiotservice/smithy-model/com-amazonaws-iot-awsiotservice.json — CreateCertificateProvider (POST /certificate-providers/{certificateProviderName}; lambdaFunctionArn + accountDefaultForOperations required; InvalidRequestException 400), UpdateCertificateProvider (PUT same path), DescribeCertificateProvider (GET same path; lambdaFunctionArn, lastModifiedDate), DeleteCertificateProvider (DELETE same path; ResourceNotFoundException 404), CreateStream (POST /streams/{streamId}; files + roleArn required; InvalidRequestException 400), DeleteStream (DELETE /streams/{streamId}; ResourceNotFoundException 404), CreateDynamicThingGroup (POST /dynamic-thing-groups/{thingGroupName}; queryString required; ResourceNotFoundException 404, InvalidQueryException 400).

## Steps to Reproduce
1. Boot the emulator built from this checkout and point an iot client at it.
2. Cause 1: CreateCertificateProvider(certificateProviderName=P, lambdaFunctionArn=<a function ARN that does not exist / does not allow iot.amazonaws.com>, accountDefaultForOperations=['CreateCertificateFromCsr']) — expect 400 InvalidRequestException 'Access denied during lambda validation: <arn>'.
3. Cause 2: DeleteCertificateProvider(<a provider name never created>) — expect 200 with an empty body.
4. Cause 3: create a provider pointing at function fn1, UpdateCertificateProvider(P, lambdaFunctionArn=<fn2>), then DescribeCertificateProvider(P) — expect fn2.
5. Cause 4: CreateStream(streamId=S, files=[{fileId:0, s3Location:{bucket,key}}], roleArn=<a role IoT cannot assume>) — expect 400 'Permission denied when attempting to assume role <arn>.'
6. Cause 5: DeleteStream(S) for a stream that does not exist — expect 404 'Requested stream <S> does not exist in given account <account>'.
7. Cause 6: with thing indexing NOT enabled, CreateDynamicThingGroup(thingGroupName=G, queryString='thingName:*') — expect 404 ResourceNotFoundException 'AWS IoT Fleet Indexing is not enabled. Please enable index by calling UpdateIndexingConfiguration.'
8. Re-replay the representative step iot-certificate-provider-error-paths#task#1 and every fixture#step in trace_ids.
