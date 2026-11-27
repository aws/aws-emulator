Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__iam__s3.apigw-s3-integration-maps-object-response-headers apigateway__iam__s3.s3-proxy-put-convert-to-binary-decodes-base64-text-body apigateway__iam__s3.s3-proxy-put-raw-binary-body-stored-byte-exact apigateway__iam__s3.s3-proxy-get-binary-object-base64-encoded-into-json-template apigateway__iam__s3.s3-proxy-put-outside-role-policy-prefix-rejected-with-500
Issue-Key: apigateway__iam__s3.apigw-s3-integration-maps-object-response-headers|run#26|AssertionError|assert headers.get("etag") == head["ETag"], f"expected ETag {head['ETag']!r}, got {headers.get('etag')!r}"
Issue-Key: apigateway__iam__s3.s3-proxy-put-convert-to-binary-decodes-base64-text-body|run#32|AssertionError|assert decoded == raw, (
Issue-Key: apigateway__iam__s3.s3-proxy-put-raw-binary-body-stored-byte-exact|run#38|AssertionError|assert decoded == payload, (
Issue-Key: apigateway__iam__s3.s3-proxy-get-binary-object-base64-encoded-into-json-template|_until_200#27|Timeout|time.sleep(delay)
Issue-Key: apigateway__iam__s3.s3-proxy-put-outside-role-policy-prefix-rejected-with-500|run#39|AssertionError|assert msg.startswith(head_part) and msg.endswith(tail_part), (
Issue-Commit: cc8c6d4514b1c20009096c9d1c08086c7dabca5f

# apigateway: the AWS-S3 proxy integration drops response-header mappings, ignores contentHandling on the integration request, never answers 200 on a binary GET, and flattens S3's AccessDenied to "Access Denied"

> _Note: this issue was identified against the code base at commit `cc8c6d4514b1c20009096c9d1c08086c7dabca5f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutIntegrationResponse

## Description
Four net-new root causes in the REST API `AWS` integration whose backend is S3 (an "S3 proxy" method). All four are reached through a deployed REST API method with `type=AWS`, `uri=arn:aws:apigateway:<region>:s3:path/{bucket}/{key}` and a `credentials` role, called with SigV4.

=== Cause 1 of 4: a 200 integration response's responseParameters do not map S3's headers onto the method response ===
AWS: `PutIntegrationResponse`/`PutMethodResponse` let a method project a backend header onto the client response via `responseParameters` of the form `method.response.header.<Name>` -> `integration.response.header.<Name>`. For an S3 integration the object's `ETag`, `Content-Type`, `Content-Length` and `Last-Modified` are the backend headers, and the invoke response must carry them with the values S3 returned (Smithy: `PutIntegrationResponse.responseParameters` / `PutMethodResponse.responseParameters`, `MethodResponse.responseParameters`; API Gateway REST API developer guide, "Set up request and response data mappings"). The ETag value itself is the object's, taken from S3's `HeadObject` on the same key.
Emulator: the invoke response carries NO `etag` header at all (`headers.get("etag")` is `None`), although the mapping was declared and the object exists.
Why: a client that reads an object through an S3 proxy method cannot do conditional GETs, cache validation or integrity checks — every mapped header is simply absent, so the method looks like it returned a body with no metadata.
Fix: the integration-response stage of the REST API invoke path — the component that applies `responseParameters` to the backend response before serializing the method response.
Traces: apigateway__iam__s3.apigw-s3-integration-maps-object-response-headers.

=== Cause 2 of 4: `contentHandling` on the integration REQUEST is ignored, so the raw/base64 request body is stored verbatim ===
AWS: `contentHandling` on an integration takes `CONVERT_TO_BINARY` or `CONVERT_TO_TEXT` (Smithy: `Integration.contentHandling` / `PutIntegration.contentHandling`, enum `ContentHandlingStrategy` = `CONVERT_TO_BINARY | CONVERT_TO_TEXT`). With `CONVERT_TO_BINARY`, API Gateway base64-DECODES the request payload before handing it to the backend, so S3 stores the decoded bytes; the same conversion applies when a text media type carries a base64 payload that the method declares binary. The requirement is that the bytes S3 holds equal the DECODED payload, byte for byte, and that its `Content-Length` is the decoded length.
Emulator: the base64 TEXT is stored instead of the decoded bytes. A 176-byte decoded payload lands as 236 bytes beginning `iVBORw0KGgrIycrLzM3Oz9DR` (the base64 characters themselves), and a 1024-byte payload sent as `text/plain` lands as 1368 bytes beginning `AAECAwQFBgcICQoLDA0ODxAREhMUFRYX`. (Those lengths and prefixes are this run's payloads; the requirement is decoded-length and decoded-bytes equality for any payload.)
Why: every binary upload through an S3 proxy method is corrupted — the object a client later downloads is base64 text, not the file it PUT, so checksums, content types and image/PDF readers all fail downstream.
Fix: the integration-request body stage of the REST API invoke path — apply the integration's `contentHandling` (and the API's `binaryMediaTypes`) before forwarding the payload to the AWS backend.
Traces: apigateway__iam__s3.s3-proxy-put-convert-to-binary-decodes-base64-text-body, apigateway__iam__s3.s3-proxy-put-raw-binary-body-stored-byte-exact.

=== Cause 3 of 4: a GET through the S3 proxy with a JSON/`application/octet-stream` Accept never reaches 200 ===
AWS: recorded behavior only. A GET method on the S3 integration with an integration-response mapping template (`application/json` producing `{"image": "$input.body"}` over a base64-encoded object) must answer HTTP 200 with that single-member JSON document, and the sibling method with a binary `Accept` must answer 200 with the object's exact bytes. The recording shows both legs returning 200 on a deployed stage.
Emulator: neither leg ever reaches 200 — the check's retry loop (`_until_200`, exponential backoff) exhausts its 60 s budget without a single 200, so the check ends `Check timed out after 60.0s`.
Why: the read half of the S3 proxy pattern is unusable: a client gets a non-2xx forever for an object that exists and that the integration role may read.
Fix: the REST API invoke path for `type=AWS` S3 GET integrations — integration-response selection and the `application/json` mapping template applied to a binary backend body.
Traces: apigateway__iam__s3.s3-proxy-get-binary-object-base64-encoded-into-json-template.

=== Cause 4 of 4: S3's AccessDenied for the assumed integration role is the bare string "Access Denied" ===
AWS: when the integration's `credentials` role is assumed and its policy does not cover the key, S3 refuses with `AccessDenied` whose message names the assumed-role session and the refused resource: `User: arn:aws:sts::<account>:assumed-role/<role-name>/<session-name> is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/<key>" because no identity-based policy allows the s3:PutObject action`. Fixed text: `User: `, ` is not authorized to perform: s3:PutObject on resource: "`, `" because no identity-based policy allows the s3:PutObject action`. Variable parts taken from the request and state: the account id, the integration role's name, the session name API Gateway chose (any `[\w+=,.@-]{2,64}`), the bucket and the refused key. API Gateway then surfaces the 403 as its default 500, and `HeadObject` on the refused key answers 404.
Emulator: the S3 error body carries `Code=AccessDenied` but `Message` is exactly `Access Denied` — no principal, no action, no resource.
Why: a caller debugging an S3 proxy refusal cannot tell WHICH role, action or key was refused; tooling that parses the principal or the resource out of the message gets nothing.
Fix: the S3 authorization-refusal serializer — the component that words `AccessDenied` for an assumed-role caller on an object operation.
Traces: apigateway__iam__s3.s3-proxy-put-outside-role-policy-prefix-rejected-with-500.

## Evidence
All four from one replay of the behavioral suite (service heading `### apigateway__iam__s3`, 2/7 passed).

[mismatch] apigateway__iam__s3.apigw-s3-integration-maps-object-response-headers
  AssertionError: expected ETag '"5d41402abc4b2a76b9719d911017c592"', got None
  (the ETag value is this run's object; the requirement is that the mapped `etag` header is present and equals S3's HeadObject ETag)

[mismatch] apigateway__iam__s3.s3-proxy-put-convert-to-binary-decodes-base64-text-body
  AssertionError: CONVERT_TO_BINARY should store the base64-decoded 176 bytes; S3 holds 236 bytes starting b'iVBORw0KGgrIycrLzM3Oz9DR'

[mismatch] apigateway__iam__s3.s3-proxy-put-raw-binary-body-stored-byte-exact
  AssertionError: a text/plain body should be base64-decoded before S3 stores it: stored 1368 bytes, first 32 b'AAECAwQFBgcICQoLDA0ODxAREhMUFRYX', expected the 1024 decoded bytes

[mismatch] apigateway__iam__s3.s3-proxy-get-binary-object-base64-encoded-into-json-template
  Error: Check timed out after 60.0s (the check's `_until_200` retry loop: `assert status == 200, f"the request never reached 200; last status ..."`)

[mismatch] apigateway__iam__s3.s3-proxy-put-outside-role-policy-prefix-rejected-with-500
  AssertionError: S3's AccessDenied should name the assumed integration role and the refused key; got 'Access Denied'
  Expected message head: `User: arn:aws:sts::<account>:assumed-role/<role>/` ; expected tail: ` is not authorized to perform: s3:PutObject on resource: "arn:aws:s3:::<bucket>/<key>" because no identity-based policy allows the s3:PutObject action`

## Steps to Reproduce
1. Create an IAM role trusted by apigateway.amazonaws.com, grant it s3 access scoped to one key prefix of a bucket, and create a REST API.
2. Cause 1: add `GET /obj` with `type=AWS`, `uri=arn:aws:apigateway:<region>:s3:path/{bucket}/{key}`, `credentials=<role arn>`; declare a 200 method response with `method.response.header.ETag` and a 200 integration response mapping `method.response.header.ETag -> integration.response.header.ETag`. Deploy, PUT an object, then SigV4-GET the method: the response must carry `etag` equal to S3's HeadObject ETag; observed: no `etag` header.
3. Cause 2: add `PUT /obj` with the same integration and `contentHandling=CONVERT_TO_BINARY`; send a base64 text body. S3 must hold the base64-DECODED bytes; observed: the base64 characters are stored. Repeat with a `text/plain` body declared binary via `binaryMediaTypes` — same divergence.
4. Cause 3: add `GET /img` whose 200 integration response has an `application/json` mapping template producing `{"image": "$input.body"}`, and `GET /blob` returning the raw bytes. SigV4-GET both on the deployed stage: both must answer 200; observed: neither ever returns 200.
5. Cause 4: SigV4-PUT a key OUTSIDE the role's granted prefix. API Gateway must answer 500 carrying S3's `AccessDenied` XML whose Message names `arn:aws:sts::<account>:assumed-role/<role>/<session>`, `s3:PutObject` and the refused `arn:aws:s3:::<bucket>/<key>`; observed Message is `Access Denied`. HeadObject on that key must be 404.
