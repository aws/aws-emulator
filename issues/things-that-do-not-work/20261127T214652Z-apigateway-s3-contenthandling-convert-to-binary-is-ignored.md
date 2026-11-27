Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__s3.rest-api-s3-put-convert-to-binary-stores-base64-decoded-object-bytes apigateway__s3.rest-api-s3-put-outside-role-prefix-rejected-and-writes-nothing
Issue-Key: apigateway__s3.rest-api-s3-put-convert-to-binary-stores-base64-decoded-object-bytes|run#26|AssertionError|assert decoded == blob, (
Issue-Key: apigateway__s3.rest-api-s3-put-outside-role-prefix-rejected-and-writes-nothing|run#26|AssertionError|assert stored == payload, f"granted object must hold the decoded bytes, got {stored[:24]!r}"
Issue-Commit: 9bfc3dcca28986b1e030db2b6d5b17f08a507141

# apigateway__s3: contentHandling CONVERT_TO_BINARY is ignored on an AWS-S3 proxy integration request, so the base64 text body is stored verbatim instead of decoded

> _Note: this issue was identified against the code base at commit `9bfc3dcca28986b1e030db2b6d5b17f08a507141`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegration

## Description
Cause 1 of 1: an AWS (S3 proxy) integration whose contentHandling is CONVERT_TO_BINARY stores the request body as the literal base64 text instead of its decoded bytes.
AWS: Smithy (com-amazonaws-apigateway-backplanecontrolservice) models PutIntegration.contentHandling as the ContentHandlingStrategy enum. With CONVERT_TO_BINARY on the integration REQUEST, API Gateway base64-DECODES a text request payload before forwarding it to S3, so the stored object holds exactly the decoded bytes — the object's length equals the decoded length and its leading bytes are the decoded ones (for a PNG payload, the PNG signature), not the base64 alphabet. This holds both for the plain write and for the admitted write in the role-prefix scenario.
Emulator: the object S3 holds is the undecoded base64 text — 372 bytes beginning `b'AP/+gGJsb2ItNWMwY2FmODUw'` where the 277 decoded bytes were expected, and `b'iQDDc2NvcGVkLTk3ZjU3Mjk3'` where the decoded bytes were expected for the granted write. The byte count is the base64 length, so no decoding step ran at all.
Why: every binary upload through an S3 proxy integration lands corrupted — a client that later GETs the object (or that relies on Content-Length, a checksum or a magic-number sniff) reads base64 text where AWS stores the original bytes, and nothing in the response signals it.
Fix: the execute-api AWS-integration request path — apply the integration's contentHandling (CONVERT_TO_BINARY base64-decodes, CONVERT_TO_TEXT base64-encodes) to the integration request payload before forwarding.
Traces: apigateway__s3.rest-api-s3-put-convert-to-binary-stores-base64-decoded-object-bytes, apigateway__s3.rest-api-s3-put-outside-role-prefix-rejected-and-writes-nothing

Note: the already-open issue 20261127T173710Z-apigateway-iam-s3-apigateway-the-aws-s3-proxy-integration-drops-response names contentHandling on the S3 proxy integration, but it is filed against the apigateway__iam__s3 service and does not carry either of these two checks, so it does not cover them.

## Evidence
rest-api-s3-put-convert-to-binary-stores-base64-decoded-object-bytes — 'the CONVERT_TO_BINARY object must hold the 277 decoded bytes, got 372 bytes starting b\'AP/+gGJsb2ItNWMwY2FmODUw\''.
rest-api-s3-put-outside-role-prefix-rejected-and-writes-nothing — 'granted object must hold the decoded bytes, got b\'iQDDc2NvcGVkLTk3ZjU3Mjk3\''.
In both cases the stored length is the base64 length of the payload rather than its decoded length, so contentHandling was not applied.

## Steps to Reproduce
1. Create an S3 bucket and an IAM role allowing s3:PutObject on it.
2. CreateRestApi (REGIONAL); PutMethod PUT on a resource; PutIntegration type=AWS, integrationHttpMethod=PUT, uri=.../s3/{bucket}/{key}, credentials=<role ARN>, contentHandling=CONVERT_TO_BINARY.
3. CreateDeployment + stage; PUT a text/plain body containing the base64 encoding of some binary payload (e.g. a small PNG) through the stage.
4. GetObject the key and compare its length and leading bytes with the decoded payload.
