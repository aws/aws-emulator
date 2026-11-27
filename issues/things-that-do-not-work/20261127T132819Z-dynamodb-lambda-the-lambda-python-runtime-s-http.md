Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: dynamodb__lambda.lambda-vendored-boto3-conditional-put-on-an-existing-key-is-rejected
Issue-Key: dynamodb__lambda.lambda-vendored-boto3-conditional-put-on-an-existing-key-is-rejected|_invoke#12|AssertionError|assert "FunctionError" not in response, (
Issue-Commit: 803b8f0256cf78557f71fb793e40c4a31d070d39

# dynamodb__lambda: the Lambda Python runtime's http.client has no IncompleteRead, so a function bundling its own boto3 cannot import

> _Note: this issue was identified against the code base at commit `803b8f0256cf78557f71fb793e40c4a31d070d39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
Invoke

## Description
Cause 1 of 1: the Python runtime image used for Lambda invocations ships an `http.client` module without `IncompleteRead`, so any handler that imports a vendored/bundled boto3 fails at import time.
AWS: a Lambda deployment package that bundles its own boto3 imports cleanly on the managed Python runtime — `http.client.IncompleteRead` is part of the Python standard library, and botocore's `botocore.vendored`/`urllib3` import chain depends on it. The handler then runs and, for this check, surfaces DynamoDB's conditional-put refusal (`ConditionalCheckFailedException`, HTTP 400, `The conditional request failed`) to the caller.
Emulator: the invocation returns an unhandled error — `{'errorMessage': "Unable to import module 'index': cannot import name 'IncompleteRead' from 'http.client' (unknown location)", 'errorType': 'Runtime.ImportModuleError'}` — so the handler never runs and the DynamoDB behaviour it exercises is never reached.
Why: the very common pattern of pinning a boto3 version inside the deployment package is broken for every Python Lambda; the error names an import failure in the runtime's own standard library, which no customer code can fix.
Fix: the Lambda Python runtime component — its bundled standard library must provide the full `http.client` surface (including `IncompleteRead`) that botocore's import chain requires.
Traces: dynamodb__lambda.lambda-vendored-boto3-conditional-put-on-an-existing-key-is-rejected.

## Evidence
Expected: the handler imports its bundled boto3, issues a conditional PutItem on an existing key and reports the ConditionalCheckFailedException. Actual: `sim2real-sdkzipddb-fn-... raised 'Unhandled': {'errorMessage': "Unable to import module 'index': cannot import name 'IncompleteRead' from 'http.client' (unknown location)", 'errorType': 'Runtime.ImportModuleError', 'requestId': '', 'stackTrace': []}`.

## Steps to Reproduce
Create a Python Lambda whose deployment zip bundles boto3/botocore, with a handler that does `import boto3` and performs a conditional PutItem, then Invoke it.
