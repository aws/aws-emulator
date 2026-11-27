Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sagemaker__secretsmanager.code-repository-parameter-arn-as-secret-rejected
Issue-Key: sagemaker__secretsmanager.code-repository-parameter-arn-as-secret-rejected|run#16|AssertionError|assert message.replace(param_arn, "<value>") == expected, f"unexpected create refusal message: {message!r}"
Issue-Commit: 33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5

# sagemaker__secretsmanager: the GitConfig.SecretArn pattern-violation message echoes the Smithy regex verbatim (leading ^, no trailing .*) instead of AWS's rendering

> _Note: this issue was identified against the code base at commit `33b472663ab106bbc2d7f1efbf90bc2bd7d78bc5`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateCodeRepository

## Description
=== Cause 1 of 1: CreateCodeRepository renders the SecretArn pattern-constraint refusal with the model's anchored regex instead of the pattern AWS quotes ===
AWS: Smithy — CreateCodeRepository.GitConfig is required; `GitConfig.SecretArn` targets `SecretArn` (string, length 1..2048, pattern `^arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:`), documented as "The Amazon Resource Name (ARN) of the Amazon Web Services Secrets Manager secret that contains the credentials used to access the git repository". A value that is not a Secrets Manager secret ARN — e.g. an SSM Parameter Store ARN — is refused with HTTP 400 `ValidationException` and the standard per-member validation sentence. Recorded behavior: the message is "1 validation error detected: Value '<submitted value>' at 'gitConfig.secretArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:.*". Fixed text: everything except `<submitted value>`, which is echoed from the request exactly once. Note how AWS renders the pattern: the leading `^` anchor is dropped and, because the constraint is not end-anchored, a trailing `.*` is appended — the same rendering convention the service uses for its other unanchored member patterns.
Emulator: the status (400) and code (ValidationException) are right, and the member path `gitConfig.secretArn` and echoed value are right, but the quoted pattern is the raw Smithy string: "... Member must satisfy regular expression pattern: ^arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:" — it keeps the `^` and omits the `.*`.
Why: the quoted pattern is the actionable part of this message: it is what a client (or a human reading a deploy log) copies to fix the value, and tooling that asserts on or parses the constraint text sees a different regex from the one AWS publishes. A leading `^` in a message AWS never anchors also makes emulator-authored text identifiable, so parity tests on the refusal wording fail even though the classification is correct.
Fix: the shared pattern-constraint violation renderer used by the CreateCodeRepository (and UpdateCodeRepository) input validation — it must normalise a Smithy `pattern` for display by stripping the `^`/`$` anchors and appending `.*` where the constraint is not end-anchored, rather than printing the trait value verbatim.
Traces: sagemaker__secretsmanager.code-repository-parameter-arn-as-secret-rejected.

## Evidence
Replay output, verbatim from the behavioral suite report, under `### sagemaker__secretsmanager`:

[mismatch] sagemaker__secretsmanager.code-repository-parameter-arn-as-secret-rejected
  assert message.replace(param_arn, "<value>") == expected
  AssertionError: unexpected create refusal message: "1 validation error detected: Value 'arn:aws:ssm:us-west-2:000000000000:parameter/sim2real-gitcred27e62c1fa695' at 'gitConfig.secretArn' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:"

Expected message (the check's own template; `<value>` is the submitted ARN, echoed once):
  "1 validation error detected: Value '<value>' at 'gitConfig.secretArn' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:.*"

Diff: the emulator prints a leading `^` and no trailing `.*`. The region, account and parameter name in the echoed value are per-run and are not part of the requirement. Error.Code 'ValidationException' and the 400 status matched; the sibling positive case (the same member holding a real Secrets Manager secret ARN from CreateSecret) was accepted, so only the rendering of the refusal diverges.

## Steps to Reproduce
Against the booted emulator with boto3/botocore pointed at it:
1. secretsmanager.create_secret(Name=<n>, SecretString='{"username": "john-doe", "password": "placeholder-value"}') and note its ARN.
2. Positive control: sagemaker.create_code_repository(CodeRepositoryName=<a>, GitConfig={'RepositoryUrl': 'https://example.com/sim2real/model-training.git', 'SecretArn': <that secret ARN>}) — accepted.
3. sagemaker.create_code_repository(CodeRepositoryName=<b>, GitConfig={'RepositoryUrl': 'https://example.com/sim2real/model-training.git', 'SecretArn': 'arn:aws:ssm:<region>:<account>:parameter/<name>'}).
4. Read Error.Message of the resulting ValidationException. Observe the quoted pattern is `^arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:` instead of `arn:aws[a-z\-]*:secretsmanager:[a-z0-9\-]*:[0-9]{12}:secret:.*`.
