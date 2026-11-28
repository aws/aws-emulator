Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.add-permission-naming-a-role-arn-or-a-session-arn lambda.python-function-scandir-and-walk-list-what-it-wrote-under-tmp lambda.python-function-write-outside-tmp-rejected-read-only lambda.lambda-unsigned-get-to-an-iam-function-url-from-another-function-is-forbidden lambda.create-function-second-file-system-config-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.add-permission-naming-a-role-arn-or-a-session-arn|run#39|AssertionError|assert not wrong, "; ".join(wrong)
Issue-Key: lambda.python-function-scandir-and-walk-list-what-it-wrote-under-tmp|run#6|AssertionError|assert "error" not in out, f"listing /tmp/{directory} failed in the function: {out!r}"
Issue-Key: lambda.python-function-write-outside-tmp-rejected-read-only|run#8|AssertionError|assert "error" not in out, f"listing {allowed_dir} failed in the function: {out!r}"
Issue-Key: lambda.lambda-unsigned-get-to-an-iam-function-url-from-another-function-is-forbidden|run#14|JSONDecodeError|assert (got["status"], json.loads(got["body"])) == (403, {"Message": "Forbidden"}), (
Issue-Key: lambda.create-function-second-file-system-config-rejected|run#11|AssertionError|assert got[1] == (
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# lambda: 4 further causes — assumed-role session principal accepted by AddPermission, Python /tmp directory entries lack is_junction, a function-URL IAM refusal is not JSON, and the FileSystemConfigs refusal text differs

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AddPermission

## Description
Four net-new root causes in the Lambda surface that do not belong to the validation, invoke-limit or durable clusters filed alongside this issue.

Cause 1 of 4: AddPermission accepts an assumed-role session ARN as the Principal, and the grant then authorises an invoke.
AWS: the Principal of a Lambda resource-policy statement may be an account, a service principal or an IAM role ARN; an STS assumed-role *session* ARN (`arn:aws:sts::<account>:assumed-role/<role>/<session>`) is not a valid Principal and AddPermission refuses it, so no invoke is authorised by such a statement.
Emulator: accepts the session-ARN AddPermission (`('OK', None)`) and then serves the invoke made with that session's credentials (`('OK', {'echo': 4})`).
Why: a client builds a policy that grants on the emulator and is refused at deploy time by AWS; worse, an authorisation that AWS would not have granted succeeds here, so a least-privilege test passes falsely.
Fix: Principal parsing/validation in the AddPermission handler (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/add_permission.rs) and the principal matcher used by the invoke authorisation path.
Traces: lambda.add-permission-naming-a-role-arn-or-a-session-arn.

Cause 2 of 4: the Python runtime's /tmp directory-entry objects are not os.DirEntry-compatible.
AWS: in the managed python runtime `os.scandir`/`os.walk` under /tmp yield real `os.DirEntry` objects, which since Python 3.12 expose `is_junction()`; a handler that walks /tmp (to list what it wrote, or to prove the rest of the filesystem is read-only) succeeds.
Emulator: the sandbox substitutes its own `_TmpEntry` object, and the handler fails with `AttributeError: '_TmpEntry' object has no attribute 'is_junction'`.
Why: any handler that enumerates /tmp with the standard library — a very common pattern for temp-file cleanup and for packaging steps — crashes on the emulator and works on AWS, and the crash masks whatever the handler was actually testing.
Fix: the /tmp filesystem shim in the python runtime sandbox — its directory entries must implement the full os.DirEntry interface for the runtime's Python version.
Traces: lambda.python-function-scandir-and-walk-list-what-it-wrote-under-tmp, lambda.python-function-write-outside-tmp-rejected-read-only.

Cause 3 of 4: an unsigned GET to an AWS_IAM function URL is answered with a body that is not JSON.
AWS: a function URL whose AuthType is AWS_IAM answers an unsigned request 403 Forbidden with a JSON body (`{"Message":"Forbidden"}`), so a caller can parse the refusal.
Emulator: the response body cannot be parsed as JSON at all — the check fails with `json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)`, i.e. an empty or non-JSON body for the refusal.
Why: a client (here, another Lambda function) that parses the refusal to decide whether to re-sign gets a parse error instead of a documented refusal.
Fix: the error serialisation of the function-URL front door for the unauthenticated/unsigned AWS_IAM case.
Traces: lambda.lambda-unsigned-get-to-an-iam-function-url-from-another-function-is-forbidden.

Cause 4 of 4: the FileSystemConfigs arity refusal does not use AWS's message.
AWS: CreateFunction with two FileSystemConfigs entries is refused 400 ValidationException; the recorded message for that list constraint is not the one the emulator emits.
Emulator: answers `1 validation error detected: Value at 'fileSystemConfigs' failed to satisfy constraint: Member must have length less than or equal to 1` — the member path and constraint sentence do not match the recording.
Why: a client keyed on the message/member path to point the user at the offending property reports the wrong field.
Fix: the FileSystemConfigs list-length validation message in the CreateFunction handler.
Traces: lambda.create-function-second-file-system-config-rejected.

## Evidence
Cause 1: `AssertionError: add-permission-session-arn: got ('OK', None); invoke-with-session-grant: got ('OK', {'echo': 4})` — both legs expected to be refused.
Cause 2: `AssertionError: listing /tmp/<dir> failed in the function: {'error': 'AttributeError', 'message': "'_TmpEntry' object has no attribute 'is_junction'"}` (both checks).
Cause 3: `json.decoder.JSONDecodeError: Expecting value: line 1 column 1 (char 0)` while decoding the 403 the function URL served to an unsigned GET.
Cause 4: `AssertionError: two FileSystemConfigs entries answered the message "1 validation error detected: Value at 'fileSystemConfigs' failed to satisfy constraint: Member must have length less than or equal to 1"`.

## Steps to Reproduce
Replay the 5 check ids in trace_ids. (1) `aws lambda add-permission --principal arn:aws:sts::<account>:assumed-role/<role>/<session> --action lambda:InvokeFunction ...`, then invoke with that session's credentials — both succeed, both must be refused. (2) Deploy a python3.12+ handler that calls `os.scandir('/tmp')` and touches `entry.is_junction()` and invoke it. (3) Create an AWS_IAM function URL and GET it unsigned from another function; try to JSON-decode the 403 body. (4) `aws lambda create-function --file-system-configs <two entries>` and compare the ValidationException message.
