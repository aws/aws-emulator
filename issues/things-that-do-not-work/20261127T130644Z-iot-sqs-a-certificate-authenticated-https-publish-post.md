Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: iot__sqs.https-publish-outside-the-certificate-policy-is-rejected-and-never-reaches-the-rule-queue
Issue-Key: iot__sqs.https-publish-outside-the-certificate-policy-is-rejected-and-never-reaches-the-rule-queue|_first_delivery#30|AssertionError|assert answer[0] == 200, f"the granted-topic publish answered {answer!r}"
Issue-Commit: eef1405ac0806ef267777bba6aefe2b3b0754197

# iot__sqs: a certificate-authenticated HTTPS publish (POST /topics/{topic}) at the published iot:Data-ATS address answers 404 UnknownService, so the policy-scoped publish and its topic-rule delivery never reach a verdict

> _Note: this issue was identified against the code base at commit `eef1405ac0806ef267777bba6aefe2b3b0754197`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Publish

## Description
One net-new root cause, found by the behavioral suite under the `iot__sqs` service heading.

Cause 1 of 1: the mTLS HTTPS publish route of the IoT data plane is not served at the published iot:Data-ATS address — a device's POST to /topics/{topic} with its X.509 certificate as the TLS client identity is answered 404 UnknownService instead of being accepted (or refused on policy grounds).
AWS: Smithy iot-data Publish is POST /topics/{topic} with the message as the httpPayload and qos/retain/contentType/responseTopic as query parameters; it answers HTTP 200 with an empty body and models InvalidRequestException (400), UnauthorizedException (401), MethodNotAllowedException (405) and ThrottlingException (429). A device authenticates to this route with its certificate over mutual TLS on the account's iot:Data-ATS host: no SigV4 credential scope and no X-Amz-Target are present, and the operation is resolved from the URI path alone. Recorded behavior for this scenario: a publish to a topic the certificate's attached policy GRANTS answers 200 and the message is ingested by the matching topic rule (so the rule's SQS action delivers it), while a publish to a topic OUTSIDE the policy is refused 403 AccessDeniedException and never reaches the rule's queue.
Emulator: the granted-topic publish — the positive control the whole check rests on — is answered HTTP 404 with body {"__type": "UnknownService", "message": "could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)"}. The front door tries to resolve a service from signing metadata a certificate-authenticated device does not send, so the route is unreachable and neither the authorized publish nor the out-of-policy refusal can be observed.
Why: a device that holds only a certificate (the normal IoT identity) cannot publish over HTTPS at all; every client built on the certificate-authenticated data plane fails at its first call with an error that names no IoT operation, and the per-topic authorization decision that protects a fleet is never exercised. Open issue 20261110T222008Z-iot-the-published-data-plane-address-serves-no.md reports this 404 under the `iot` heading and open issue 20261123T215116Z-ec2-iot-a-mutual-tls-iot-publish-from.md reports it under `ec2__iot`; neither carries this check, which is reported under `iot__sqs` and additionally covers the rules-engine delivery leg.
Fix: the mTLS listener's request routing — resolve iot-data operations from the URI path (and the presented client certificate) rather than from SigV4/X-Amz-Target, bind POST /topics/{topic} on it, and evaluate the certificate's attached policies to answer 200 for a granted topic and 403 AccessDeniedException for one outside the policy (the emulator's data-plane listener/router plus services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/src/ops/publish.rs and the policy-evaluation path it must call).
Traces: iot__sqs.https-publish-outside-the-certificate-policy-is-rejected-and-never-reaches-the-rule-queue.

## Evidence
Replay of suite simparity-behavioral-aws-v1 against the emulator built from this checkout. Expected = real-AWS behavior the check encodes; Actual = the emulator's answer. The host, port and generated names below are one run's values; they are not part of the requirement.

iot__sqs.https-publish-outside-the-certificate-policy-is-rejected-and-never-reaches-the-rule-queue — CreateKeysAndCertificate, a policy granting iot:Publish on one topic only, AttachPolicy, an SQS queue plus a topic rule with an sqs action, DescribeEndpoint(iot:Data-ATS), then a `requests` POST to https://<data-host>:8443/topics/<granted topic>?qos=1 carrying the device certificate and key as the TLS client identity. Expected HTTP 200 (empty body) followed by the message appearing on the rule's queue, and a later POST to a topic outside the policy refused 403 AccessDeniedException with nothing reaching the queue. Actual (404, {'__type': 'UnknownService', 'message': 'could not resolve a target service from the request (no SigV4 credential scope, X-Amz-Target, or Host header)'}). AssertionError at check lines 214/117: "the granted-topic publish answered (404, ...)".

## Steps to Reproduce
1. git clone the repo and build/boot the emulator from this checkout.
2. Replay suite simparity-behavioral-aws-v1 and observe iot__sqs.https-publish-outside-the-certificate-policy-is-rejected-and-never-reaches-the-rule-queue failing.
3. Equivalently, by hand: iot CreateKeysAndCertificate; CreatePolicy granting iot:Publish on arn:aws:iot:<region>:<account>:topic/<granted topic> only; AttachPolicy to the certificate; sqs CreateQueue and iot CreateTopicRule with an sqs action on the granted topic; DescribeEndpoint(iot:Data-ATS); then POST https://<that host>:8443/topics/<granted topic>?qos=1 with the certificate/key as the TLS client identity — observe 404 UnknownService instead of 200, and repeat for a topic outside the policy, which must answer 403 AccessDeniedException. Confirm with sqs ReceiveMessage that the granted publish reaches the rule's queue and the out-of-policy one does not.
