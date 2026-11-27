Issue-Source: trace-sweep-divergence
Issue-Trace: ec2-unsigned-and-presigned-describe-requests
Issue-Step: task#1
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: ec2-unsigned-and-presigned-describe-requests
Issue-Shape-Id: com.amazonaws.ec2#AmazonEC2
Issue-Key: ec2|DescribeSecurityGroups|4xx->5xx|$.Response
Issue-Key: ec2|DescribeSecurityGroups|4xx->5xx|$headers.content-type
Issue-Key: ec2|DescribeSecurityGroups|4xx->5xx|status
Issue-Key: ec2|DescribeKeyPairs|4xx->5xx|$.Response
Issue-Key: ec2|DescribeKeyPairs|4xx->5xx|$headers.content-type
Issue-Key: ec2|DescribeKeyPairs|4xx->5xx|status
Issue-Key: ec2|DescribeSecurityGroups|2xx->4xx|$.DescribeSecurityGroupsResponse
Issue-Key: ec2|DescribeSecurityGroups|2xx->4xx|status
Issue-Commit: ba97c8568f94e8f0e1657393f851b27fdc51a8e0

# ec2 unsigned and presigned query requests answer a JSON 500 instead of the XML error envelope

> _Note: this issue was identified against the code base at commit `ba97c8568f94e8f0e1657393f851b27fdc51a8e0`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeSecurityGroups

## Description
The ec2 front door mishandles requests that are not SigV4-signed in a header: an unsigned query request faults with a 500 carrying a JSON body, and a validly presigned (query-string-credentialed) GET is refused 400 instead of being served. The shared fault is that the ec2 request pipeline's credential extraction only understands the Authorization header, and its failure path escapes as an untyped internal error in the wrong protocol envelope. This is a BUNDLE OF 3 CAUSES; all three must be closed.

Cause 1 of 3: unsigned DescribeSecurityGroups.
AWS: recorded behavior only (ec2 models no common auth error for this). A query request with no credentials at all is refused 400 with the ec2 query error envelope: Content-Type 'text/xml;charset=UTF-8', body <Response><Errors><Error><Code>MissingParameter</Code><Message>The request must contain the parameter AWSAccessKeyId</Message></Error></Errors><RequestID>..</RequestID></Response>. Fixed text: 'The request must contain the parameter AWSAccessKeyId'; no variable part.
Emulator: HTTP 500, no Response element at all, and Content-Type 'application/x-amz-json-1.1' — the ec2 service is a query/XML protocol service, so the JSON content type is wrong for every error it serves on this path.
Why: an SDK parsing the ec2 error envelope cannot read the body, so the caller sees an unparseable internal failure and SDK retry policy retries a 500 that will never succeed, instead of surfacing a non-retryable credential error.
Fix: the ec2 request pipeline's credential/authentication pre-check — emit the query-protocol XML error (400 MissingParameter) with the ec2 error content type, and make unauthenticated requests a modeled refusal rather than an internal error.
Traces: ec2-unsigned-and-presigned-describe-requests#task#1.

Cause 2 of 3: unsigned DescribeKeyPairs — the same fault on a second operation, confirming it is the shared pipeline and not one handler.
AWS: recorded behavior only; identical 400 MissingParameter / 'The request must contain the parameter AWSAccessKeyId' in the XML envelope with Content-Type 'text/xml;charset=UTF-8'.
Emulator: 500 with no Response body and Content-Type 'application/x-amz-json-1.1'.
Why: same as cause 1.
Fix: same pipeline pre-check; listed separately because it is a distinct recorded step a fixer must re-replay.
Traces: ec2-unsigned-and-presigned-describe-requests#task#2.

Cause 3 of 3: a presigned DescribeSecurityGroups GET (credentials in the query string) is refused.
AWS: recorded behavior only. A request carrying valid SigV4 presigned query parameters is SERVED: 200 with a DescribeSecurityGroupsResponse (here an empty securityGroupInfo).
Emulator: 400 — the response has no DescribeSecurityGroupsResponse element, so the presigned form is rejected as unauthenticated/malformed.
Why: presigned EC2 query URLs (the form used by delegated/browser callers and by AWS's own cross-service calls) cannot be served at all, so any workflow that hands out a presigned describe URL fails.
Fix: the ec2 request pipeline's credential extraction — accept SigV4 credentials presented as query-string parameters (X-Amz-Credential/X-Amz-Signature or the legacy AWSAccessKeyId/Signature form) in addition to the Authorization header.
Traces: ec2-unsigned-and-presigned-describe-requests#task#4.

## Evidence
Cause 1 — DescribeSecurityGroups: status expected 400, got 500; $.Response missing (expected {'Errors': {'Error': {'Code': 'MissingParameter', 'Message': 'The request must contain the parameter AWSAccessKeyId'}}, 'RequestID': <not asserted>}, got None); $headers.content-type expected 'text/xml;charset=UTF-8', got 'application/x-amz-json-1.1'.
Cause 2 — DescribeKeyPairs: identical divergence (status 400 vs 500, missing Response, content-type 'text/xml;charset=UTF-8' vs 'application/x-amz-json-1.1').
Cause 3 — DescribeSecurityGroups presigned: status expected 200, got 400; $.DescribeSecurityGroupsResponse missing (expected {'requestId': <not asserted>, 'securityGroupInfo': ''}, got None).
RequestId values in the recordings are per-run and not part of the requirement.

## Steps to Reproduce
1. Boot the emulator and replay the wire corpus (trace set simparity-wire-aws-v1).
2. Fixture ec2-unsigned-and-presigned-describe-requests step task#1: POST the ec2 query action DescribeSecurityGroups with no credentials — observe 500 + application/x-amz-json-1.1 instead of 400 MissingParameter in text/xml;charset=UTF-8.
3. Step task#2: the same with DescribeKeyPairs.
4. Step task#4: issue a SigV4-presigned DescribeSecurityGroups GET — observe 400 instead of a 200 DescribeSecurityGroupsResponse.
