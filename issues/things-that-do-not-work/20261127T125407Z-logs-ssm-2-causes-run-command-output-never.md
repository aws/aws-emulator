Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__ssm.run-command-output-streams-into-the-named-log-group-split-by-stdout-and-stderr logs__ssm.send-command-output-log-group-outside-the-log-group-name-grammar-rejected
Issue-Key: logs__ssm.run-command-output-streams-into-the-named-log-group-split-by-stdout-and-stderr|_poll#25|AssertionError|raise AssertionError(f"timed out waiting for {what}; last read {last!r}")
Issue-Key: logs__ssm.send-command-output-log-group-outside-the-log-group-name-grammar-rejected|_refusal#11|AssertionError|raise AssertionError(f"expected a refusal, the call succeeded: {got!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# logs__ssm: 2 causes — Run Command output never reaches its CloudWatch log group, and SendCommand accepts a log-group name outside the grammar

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
Cause 1 of 2: CloudWatch output is never delivered.
AWS: with CloudWatchOutputConfig.CloudWatchOutputEnabled true, each invocation's stdout and stderr are streamed into the named log group as separate streams (…/stdout and …/stderr), readable through GetLogEvents/FilterLogEvents.
Emulator: the log group receives nothing — the poll for the run's stdout line times out with an empty last read.
Why: the documented way to collect Run Command output is inert; a client watching the log group sees a successful command with no output anywhere.
Fix: the CloudWatch-output delivery path of the Run Command invocation runner (services/service-com-amazonaws-ssm-amazonssm/src/run_command/…) into the CloudWatch Logs crate.
Traces: logs__ssm.run-command-output-streams-into-the-named-log-group-split-by-stdout-and-stderr.

Cause 2 of 2: SendCommand accepts a CloudWatchLogGroupName that cannot be a log-group name.
AWS: the log-group name must satisfy the log-group grammar ([\.\-_/#A-Za-z0-9]+); a name with a space or '!' is refused (HTTP 400) and no command is created.
Emulator: returns 200 with a full Command record whose CloudWatchOutputConfig.CloudWatchLogGroupName is `sim2real-xsvc-rcname-<id> out!` — a name CloudWatch Logs can never hold.
Why: the command is accepted and its output can never be delivered anywhere, with no error at any point.
Fix: validate CloudWatchOutputConfig.CloudWatchLogGroupName in ops/send_command.rs against the log-group grammar.
Traces: logs__ssm.send-command-output-log-group-outside-the-log-group-name-grammar-rejected.

## Evidence
Cause 1: "timed out waiting for the quiet run's stdout line; last read ''".
Cause 2: "expected a refusal, the call succeeded: {'CommandId': 'ded2de72-…', 'CloudWatchOutputConfig': {'CloudWatchLogGroupName': 'sim2real-xsvc-rcname-7467cad248f6 out!', 'CloudWatchOutputEnabled': True}, 'Status': 'Pending', …}" (HTTP 200).

## Steps to Reproduce
1. SendCommand AWS-RunShellScript to a managed node with CloudWatchOutputConfig{CloudWatchOutputEnabled: true, CloudWatchLogGroupName: <group>}; poll the group's streams for the command's stdout and stderr.
2. SendCommand with CloudWatchLogGroupName '<name> out!' and compare with AWS's ValidationException for the log-group pattern.
