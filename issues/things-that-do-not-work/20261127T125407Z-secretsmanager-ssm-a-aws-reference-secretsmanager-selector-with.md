Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager__ssm.parameter-store-reference-refuses-a-stage-selector-with-other-symbols
Issue-Key: secretsmanager__ssm.parameter-store-reference-refuses-a-stage-selector-with-other-symbols|run#14|AssertionError|assert bad == _REF_SYNTAX, f"a selector whose label holds ':' gave {bad!r}, expected {_REF_SYNTAX!r}"
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# secretsmanager__ssm: a /aws/reference/secretsmanager selector with an invalid symbol is refused ParameterNotFound instead of ValidationException

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetParameter

## Description
Cause 1 of 1: an unparsable version/label selector on a Secrets Manager reference is treated as a missing secret.
AWS: GetParameter on `/aws/reference/secretsmanager/<secret>` accepts `<name>:<version-or-label>`; a selector whose label holds a character the syntax does not allow (here a second ':') is a SYNTAX error, refused with `ValidationException` — `Invalid parameter name. Please use correct syntax for referencing a version/label  <name>:<version/label>` (fixed text, including its double space, as AWS words it).
Emulator: answers `ParameterNotFound` — `An error occurred (ParameterNotFound) when referencing Secrets Manager: Secret aws/reference/secretsmanager/<secret>:<label> not found. Secrets Manager can't find the specified secret.`
Why: a malformed reference is reported as an absent secret, so the caller searches for a secret that exists rather than fixing the selector; code that treats ParameterNotFound as 'create it' takes the wrong branch.
Fix: parse and validate the version/label selector before resolving the secret in the GetParameter Secrets Manager reference path (services/service-com-amazonaws-ssm-amazonssm/src/ops/get_parameter.rs).
Traces: secretsmanager__ssm.parameter-store-reference-refuses-a-stage-selector-with-other-symbols.

## Evidence
got ('ParameterNotFound', "An error occurred (ParameterNotFound) when referencing Secrets Manager: Secret aws/reference/secretsmanager/sim2real-behav-refsel-c7378d1c92d3:colon not found. Secrets Manager can't find the specified secret."), expected ('ValidationException', 'Invalid parameter name. Please use correct syntax for referencing a version/label  <name>:<version/label>').

## Steps to Reproduce
Create a secret, then GetParameter Name='/aws/reference/secretsmanager/<secret>:<label containing ":">' and compare the error code and message with AWS's.
