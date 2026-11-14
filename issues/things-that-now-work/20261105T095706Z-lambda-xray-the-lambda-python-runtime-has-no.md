Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected
Issue-Key: lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store|run#9|AssertionError|assert "error" not in result, (
Issue-Key: lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected|run#13|AssertionError|assert "error" not in result, f"the traced Lambda raised: {result!r}"
Issue-Commit: 0d31b40a199e40e4d7162d56e6d27c869c9fd90e

# lambda__xray: the Lambda Python runtime has no _socket extension module, so every UDP X-Ray daemon datagram handler dies with Runtime.ImportModuleError and the AWS_XRAY_DAEMON_ADDRESS data plane is unobservable

> _Note: this issue was identified against the code base at commit `0d31b40a199e40e4d7162d56e6d27c869c9fd90e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
One net-new root cause, reached by 2 checks.

Cause 1 of 1: lambda Invoke executes the handler on a Python runtime whose CPython standard library is incomplete — the built-in `_socket` extension module is absent — so any handler module that does `import socket` fails to import and the function body never runs. Both X-Ray UDP-daemon checks are blocked by exactly this.

AWS: Invoke on a python3.12 function returns StatusCode 200 with NO FunctionError header and the handler's own JSON payload. The managed python3.x runtime is a complete CPython: `socket` (backed by the built-in `_socket` extension) is importable, and on a TracingConfig Mode=Active function the runtime also exports AWS_XRAY_DAEMON_ADDRESS (`<ipv4>:2000`) and a per-invocation `_X_AMZN_TRACE_ID` (`Root=1-<8hex>-<24hex>;Parent=<16hex>;Sampled=1`) so the handler can write segment datagrams to the daemon over UDP. Smithy (lambda, Invoke): the response carries StatusCode, FunctionError and Payload; FunctionError is set to "Unhandled" only when the handler itself faults, and the documented body for a genuine import failure is {"errorType": "Runtime.ImportModuleError", "errorMessage": "Unable to import module '<module>': <import error>"}. Nothing in the model or the docs sanctions a runtime that is missing core CPython extension modules. Recorded behavior confirms AWS returns the handler's payload here.

Emulator: both invocations return StatusCode=200 with FunctionError='Unhandled' and the body {"errorMessage": "Unable to import module 'lambda_function': No module named '_socket'", "errorType": "Runtime.ImportModuleError", "requestId": "<per-run id>", "stackTrace": []}. The fixed text of the message is `Unable to import module '<module>': No module named '<module>'`; `lambda_function` is the handler module from the request's Handler and `_socket` is the missing module — the requestId is a per-run value, not part of the contract. The emulator serves this reply, so the defect is the runtime it provides, not the check.

Why: a client that does what AWS documents — write a segment/subsegment datagram to AWS_XRAY_DAEMON_ADDRESS from inside a Mode=Active invocation — gets an unhandled import error instead of its result. Everything downstream is therefore unobservable: that a well-formed two-line-framed datagram reaches the trace store parented to the AWS::Lambda::Function segment and becomes queryable by annotation, and that a datagram with a malformed header line ({"format":"garbage"}) is dropped by the daemon while a well-formed datagram behind it on the same socket still lands. More generally, any customer handler using sockets (HTTP clients, DNS lookups, database drivers, the AWS X-Ray SDK itself) fails identically.

Fix: the Lambda invoke runtime — the in-process python runtime/sandbox the lambda service's Invoke path executes handlers in — must ship the CPython built-in extension modules, `_socket` among them, so `import socket` resolves and the handler's own payload is returned with no FunctionError. The X-Ray UDP receiver on the advertised daemon address must then accept the datagrams (first-line framing honored, malformed framing dropped without desyncing the socket).

Traces: lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store and lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected, both aborting at the same `assert "error" not in result` with the identical "No module named '_socket'" import error. Related but NOT covering these checks: open issue 20261105T083215Z-ec2-lambda-route53-the-lambda-python-runtime-has-no-socket-extension.md reports the same missing `_socket` for the ec2__lambda__route53 DNS vantage-point checks, and 20261105T074953Z-lambda-scheduler-the-python3-12-invoke-runtime-provides.md reports the same runtime missing boto3; neither carries these two checks' replay keys, and the X-Ray daemon leg above is not exercised by either.

## Evidence
Expected (AWS): lambda Invoke of the Mode=Active python3.12 function returns StatusCode 200 with no FunctionError and the handler's dict payload — {"raw_trace_header": "Root=1-<8hex>-<24hex>;Parent=<16hex>;Sampled=1", "daemon": "<ipv4>:2000", "root": ..., "parent": ..., "sampled": "1", "sent": true, "send_error": null} — after which the UDP-submitted subsegment is retrievable via BatchGetTraces under that Root and selectable by GetTraceSummaries with FilterExpression annotation.probe_tag = "<tag>".

Actual (emulator), identical in both checks:
  lambda__xray.lambda-udp-daemon-datagram-reaches-trace-store ->
    AssertionError: the traced Lambda raised: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named '_socket'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}} -- the datagram write must not fault the handler
  lambda__xray.lambda-udp-daemon-malformed-framing-datagram-rejected ->
    AssertionError: the traced Lambda raised: {'status_code': 200, 'error': 'Unhandled', 'payload': {'errorMessage': "Unable to import module 'lambda_function': No module named '_socket'", 'errorType': 'Runtime.ImportModuleError', 'requestId': '<per-run id>', 'stackTrace': []}}

No X-Ray assertion is ever reached: the failure occurs on the first Invoke, before any BatchGetTraces/GetTraceSummaries call.

## Steps to Reproduce
1. Create an IAM role assumable by lambda.amazonaws.com granting xray:PutTraceSegments and xray:PutTelemetryRecords.
2. CreateFunction with Runtime=python3.12, TracingConfig={"Mode":"Active"}, Handler=lambda_function.lambda_handler and a zip whose lambda_function.py begins `import json, os, socket, time`; wait for State=Active.
3. Invoke it (InvocationType=RequestResponse) with a payload naming a subsegment id/name/tag. The handler reads AWS_XRAY_DAEMON_ADDRESS and _X_AMZN_TRACE_ID and sendto()s b'{"format":"json","version":1}\n' + <segment JSON> to that address.
4. Expected: StatusCode 200, no FunctionError, payload with sent=true and send_error=null; then xray BatchGetTraces(TraceIds=[Root]) contains the submitted subsegment parented to the AWS::Lambda::Function segment, and GetTraceSummaries with FilterExpression 'annotation.probe_tag = "<tag>"' returns that Root.
5. Observed: StatusCode=200 with FunctionError='Unhandled' and {"errorType":"Runtime.ImportModuleError","errorMessage":"Unable to import module 'lambda_function': No module named '_socket'"} — the handler never executes.
6. Minimal equivalent: a handler module whose only statement is `import socket` fails to import.
