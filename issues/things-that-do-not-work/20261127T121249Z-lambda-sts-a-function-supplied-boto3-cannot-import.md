Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__sts.lambda-layer-boto3-shadows-the-runtime-sdk-and-reaches-sts-by-default lambda__sts.lambda-vendored-boto3-in-the-zip-reaches-sts-by-default-endpoint-resolution lambda__sts.lambda-node-sigv4-signed-post-to-the-sts-regional-host-returns-the-role-session lambda__sts.lambda-node-sigv4-post-signed-with-a-wrong-secret-is-rejected-by-sts
Issue-Key: lambda__sts.lambda-layer-boto3-shadows-the-runtime-sdk-and-reaches-sts-by-default|_invoke#8|AssertionError|assert "FunctionError" not in response, (
Issue-Key: lambda__sts.lambda-vendored-boto3-in-the-zip-reaches-sts-by-default-endpoint-resolution|_invoke#6|AssertionError|assert "FunctionError" not in response, (
Issue-Key: lambda__sts.lambda-node-sigv4-signed-post-to-the-sts-regional-host-returns-the-role-session|_invoke#5|AssertionError|assert "FunctionError" not in response, (
Issue-Key: lambda__sts.lambda-node-sigv4-post-signed-with-a-wrong-secret-is-rejected-by-sts|_invoke#5|AssertionError|assert "FunctionError" not in response, (
Issue-Commit: 7c686d71091806786cdd6101a8e36a3d7eadfcb8

# lambda__sts: a function-supplied boto3 cannot import because the python runtime's http.client is incomplete, and STS ignores Accept: application/json and answers XML (2 causes)

> _Note: this issue was identified against the code base at commit `7c686d71091806786cdd6101a8e36a3d7eadfcb8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Two net-new causes, both reached through lambda Invoke of a deployment package that talks to STS.

Cause 1 of 2: the in-process python runtime's standard library is incomplete, so a boto3 the FUNCTION supplies (in a layer, or vendored in the zip) cannot be imported.
AWS: Smithy: Invoke POST /2015-03-31/functions/{FunctionName}/invocations, FunctionError carried in the X-Amz-Function-Error header. Recorded behavior for the runtime: a managed python runtime is a complete CPython, so `http.client` exports `IncompleteRead` and an SDK the function ships — on the layer path /opt/python, which SHADOWS the runtime's own SDK, or vendored beside the handler in the zip — imports and runs. Invoke answers 200 with NO X-Amz-Function-Error and the handler's JSON document as Payload (here the STS identity the function resolved through default endpoint resolution).
Emulator: Invoke answers with FunctionError 'Unhandled' and payload `{"errorMessage": "Unable to import module '<handler module>': cannot import name 'IncompleteRead' from 'http.client' (unknown location)", "errorType": "Runtime.ImportModuleError", "requestId": "", "stackTrace": []}`. Fixed text: "Unable to import module '" … "': cannot import name 'IncompleteRead' from 'http.client' (unknown location)" with errorType `Runtime.ImportModuleError`; the module name is the function's own Handler module. The same failure occurs for the layer package and for the zip-vendored package, which is why both are one cause: the missing stdlib name, not the delivery mechanism.
Why: no function can bring its own SDK version. Layer shadowing and zip-vendoring are the two supported ways to pin an SDK, and both die at import, so the function never runs and the caller cannot distinguish "my package is broken" from "the runtime's stdlib is missing a name it never had to provide". Related: the open issue 20261105T081139Z-lambda-rds-secretsmanager-2-net-new-root-causes-e2a51e.md reports that the runtime supplies NO boto3 and no `_socket`; this cause is the converse case — the function supplies boto3 itself — and names a different missing stdlib symbol, `http.client.IncompleteRead`.
Fix: the Invoke execution path's python runtime environment — the interpreter executing the package must expose a complete CPython standard library (here `http.client` with `IncompleteRead`) so a function-supplied botocore/boto3 imports.
Traces: lambda__sts.lambda-layer-boto3-shadows-the-runtime-sdk-and-reaches-sts-by-default, lambda__sts.lambda-vendored-boto3-in-the-zip-reaches-sts-by-default-endpoint-resolution.

Cause 2 of 2: a SigV4 POST to the STS regional endpoint that asks for `Accept: application/json` is answered with an XML body.
AWS: recorded behavior only for the content negotiation (sts is an awsQuery service whose default serialization is XML). A POST of `Action=GetCallerIdentity&Version=2011-06-15` with `content-type: application/x-www-form-urlencoded; charset=utf-8` and `accept: application/json` is answered with a JSON document, so a client that parses the body as JSON succeeds — both for the success case and for the refusal case, where the JSON carries Error.Code `SignatureDoesNotMatch`, Error.Type `Sender` and Error.Message "The request signature we calculated does not match the signature you provided. Check your AWS Secret Access Key and signing method. Consult the service documentation for details." (fixed text, no variable part) at HTTP 403.
Emulator: the body begins with `<`, i.e. XML, regardless of the Accept header — the handler's `JSON.parse` of the response fails with `SyntaxError: unexpected token: '<'`. Both the correctly signed control request and the deliberately mis-signed one fail this way, so neither the 200 identity nor the 403 SignatureDoesNotMatch body can be read.
Why: a client that negotiates JSON — the normal choice for code that signs STS by hand, without an SDK's XML parser — cannot read ANY STS response: the success body and the error body are both unparseable, so the caller sees a parse error instead of its own identity or the signature refusal, and cannot tell a signing bug from a transport problem.
Fix: the sts front door's response serializer — honour `Accept: application/json` for the awsQuery protocol and serialize both the success and the error document as JSON, instead of always emitting XML.
Traces: lambda__sts.lambda-node-sigv4-signed-post-to-the-sts-regional-host-returns-the-role-session, lambda__sts.lambda-node-sigv4-post-signed-with-a-wrong-secret-is-rejected-by-sts.

## Evidence
Cause 1 — expected Invoke 200 with no FunctionError and the handler's STS identity document. Actual, verbatim from the replay:
- 'AssertionError: <fn> raised \'Unhandled\': {\'errorMessage\': "Unable to import module \'index\': cannot import name \'IncompleteRead\' from \'http.client\' (unknown location)", \'errorType\': \'Runtime.ImportModuleError\', \'requestId\': \'\', \'stackTrace\': []}' (boto3 delivered as a layer)
- the identical document for the zip-vendored boto3 function.

Cause 2 — expected a JSON body (200 with the role session's identity; 403 with Error.Code SignatureDoesNotMatch / Type Sender / the fixed message above). Actual, verbatim:
- 'AssertionError: <fn> raised \'Unhandled\': {\'errorMessage\': "unexpected token: \'<\'", \'errorType\': \'SyntaxError\', \'trace\': ["SyntaxError: unexpected token: \'<\'", \'    at <input>:1:1\', \'    at parse (native)\', \'    at exports.handler (/var/task/index.js:43:49)\']}' — index.js:43 is `JSON.parse(res.body)`; the body starts with \'<\'.
- the identical SyntaxError for the wrong-secret twin, so the 403 refusal body is unreadable too.

Function names and request ids above are per-run values and are not part of the requirement.

## Steps to Reproduce
Cause 1: publish a Lambda layer containing boto3/botocore under python/ (or vendor them beside the handler in the zip), create a python3.x function using it whose handler does `import boto3` and calls sts.get_caller_identity() through default endpoint resolution, then Invoke it. Expect 200 with no X-Amz-Function-Error and the identity document; observe FunctionError 'Unhandled' with Runtime.ImportModuleError "cannot import name 'IncompleteRead' from 'http.client' (unknown location)".
Cause 2: create a nodejs22.x function whose handler signs SigV4 by hand with crypto/https and POSTs 'Action=GetCallerIdentity&Version=2011-06-15' to sts.<region>.amazonaws.com with headers accept: application/json and content-type: application/x-www-form-urlencoded; charset=utf-8, then JSON.parse's the body. Invoke it — expect a JSON body it can parse (200 identity); observe SyntaxError "unexpected token: '<'". Re-invoke with the secret tampered — expect 403 and a JSON Error with Code SignatureDoesNotMatch, Type Sender and the fixed signature message; observe the same XML-body SyntaxError.
