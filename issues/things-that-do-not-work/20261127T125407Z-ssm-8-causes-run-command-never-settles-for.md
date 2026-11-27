Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ssm.command-to-an-unmatched-tag-completes-success-with-no-instances-in-tag ssm.tag-targeted-command-matching-no-instance-completes-as-success ssm.cancel-command-malformed-instance-id-rejected ssm.cancel-command-ends-an-in-progress-shell-script-as-cancelled ssm.command-invocation-reports-success-with-the-script-output-the-node-computed ssm.send-command-max-errors-zero-withholds-the-remaining-target ssm.send-command-notification-arn-of-another-service-rejected ssm.send-command-notification-config-without-topic-rejected ssm.send-command-repeated-instance-id-rejected ssm.send-command-stale-document-hash-rejected ssm.send-command-unminted-document-version-rejected
Issue-Shape-Id: com.amazonaws.ssm#AmazonSSM
Issue-Key: ssm.command-to-an-unmatched-tag-completes-success-with-no-instances-in-tag|_wait_for#19|AssertionError|raise AssertionError(f"{what} did not happen within {budget_s:.0f}s; last read {value!r}")
Issue-Key: ssm.tag-targeted-command-matching-no-instance-completes-as-success|_settle#6|AssertionError|raise AssertionError(f"command still {cmd['Status']!r} after {budget_s:.0f}s")
Issue-Key: ssm.cancel-command-malformed-instance-id-rejected|_settled#9|AssertionError|raise AssertionError(f"the command did not settle; last ListCommands record {got!r}")
Issue-Key: ssm.cancel-command-ends-an-in-progress-shell-script-as-cancelled|run#22|AssertionError|assert cancelled["Status"] == "Cancelled", (
Issue-Key: ssm.command-invocation-reports-success-with-the-script-output-the-node-computed|run#13|AssertionError|assert (out["Status"], out["StatusDetails"], out["ResponseCode"]) == ("Success", "Success", 0), (
Issue-Key: ssm.send-command-max-errors-zero-withholds-the-remaining-target|run#16|AssertionError|assert details == ["Failed", "Terminated"], (
Issue-Key: ssm.send-command-notification-arn-of-another-service-rejected|run#10|AssertionError|assert (code, message) == (
Issue-Key: ssm.send-command-notification-config-without-topic-rejected|run#10|AssertionError|assert (code, message) == (
Issue-Key: ssm.send-command-repeated-instance-id-rejected|run#6|AssertionError|assert got == ("DuplicateInstanceId", "Duplicate instanceIds not allowed."), (
Issue-Key: ssm.send-command-stale-document-hash-rejected|run#12|AssertionError|assert got == ("InvalidDocument", "Document hash does not match"), f"stale hash gave {got!r}"
Issue-Key: ssm.send-command-unminted-document-version-rejected|run#9|AssertionError|assert got == ("InvalidDocumentVersion", ""), (
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# ssm: 8 causes — Run Command never settles for tag targets, CancelCommand does not cancel, MaxErrors=0 never Terminates, and the SendCommand refusal codes/messages diverge

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
SendCommand

## Description
Eight net-new causes in SSM Run Command.

Cause 1 of 8: a command whose targets match no managed node never reaches a terminal status.
AWS: SendCommand with a tag target that matches nothing is accepted and the command settles `Success` with TargetCount 0 (it is not left Pending); the same holds for a command whose InstanceIds are refused at validation time.
Emulator: ListCommands/GetCommandInvocation keep reporting `Status: 'Pending', StatusDetails: 'Pending'`, TargetCount 0, past the poll window.
Why: any caller that waits for a terminal status hangs; the documented empty-target outcome is unobservable.
Fix: the command state machine that settles a command with no matching targets (services/service-com-amazonaws-ssm-amazonssm/src/run_command/…).
Traces: ssm.command-to-an-unmatched-tag-completes-success-with-no-instances-in-tag, ssm.tag-targeted-command-matching-no-instance-completes-as-success, ssm.cancel-command-malformed-instance-id-rejected.

Cause 2 of 8: CancelCommand does not cancel a running invocation.
AWS: cancelling an in-progress shell script ends the invocation `Cancelled`.
Emulator: the invocation ends `Success` / StatusDetails `Success` — the cancel is a no-op.
Why: a client cannot stop a long-running command, and the reported outcome claims work completed that was cancelled.
Fix: ops/cancel_command.rs and the invocation runner's cancellation path.
Traces: ssm.cancel-command-ends-an-in-progress-shell-script-as-cancelled.

Cause 3 of 8: the managed-node shell is missing the `rev` program a command invocation uses.
AWS: an AWS-RunShellScript invocation on a managed node runs the node's shell utilities and returns their stdout with ResponseCode 0.
Emulator: the invocation ends `Failed`/`Failed` with ResponseCode 127 and stderr `sh: rev: command not found`.
Why: a document that uses a standard shell utility fails on the emulator's node, so the computed output a client reads back is never produced.
Fix: the managed-node guest program set used by AWS-RunShellScript.
Traces: ssm.command-invocation-reports-success-with-the-script-output-the-node-computed.

Cause 4 of 8: MaxErrors=0 does not withhold the remaining target.
AWS: with MaxErrors '0', the first failed invocation stops delivery — the remaining target's invocation is reported `Terminated`, so the details are one Failed and one Terminated.
Emulator: both invocations report `Failed`.
Why: a client cannot distinguish a target that failed from one that was never attempted, and the error budget appears not to apply.
Fix: the MaxErrors accounting in the command scheduler.
Traces: ssm.send-command-max-errors-zero-withholds-the-remaining-target.

Cause 5 of 8: the InvalidNotificationConfig message for a non-SNS NotificationArn is emulator-authored.
AWS: Smithy SendCommand lists InvalidNotificationConfig; an ARN of another service is refused with AWS's wording for that class (fixed text, with the submitted ARN filled in).
Emulator: `NotificationArn '<arn>' must be the ARN of an Amazon Simple Notification Service topic`.
Why: message-matching clients and operators see text AWS does not send.
Fix: the NotificationConfig validator in ops/send_command.rs.
Traces: ssm.send-command-notification-arn-of-another-service-rejected.

Cause 6 of 8: a NotificationConfig without a NotificationArn is refused with the wrong error class.
AWS: the missing topic is an InvalidNotificationConfig (HTTP 400) naming the config.
Emulator: `InvalidInstanceId` — `Instances not in a valid state for account`, a refusal about the targets rather than the notification config.
Why: the caller is sent to debug its instances when its notification config is at fault.
Fix: order the NotificationConfig validation before target resolution in ops/send_command.rs.
Traces: ssm.send-command-notification-config-without-topic-rejected.

Cause 7 of 8: DuplicateInstanceId is returned with an empty message.
AWS: Smithy SendCommand lists DuplicateInstanceId; AWS sends a message describing the repeated id.
Emulator: code `DuplicateInstanceId` with message ''.
Why: the refusal says nothing about which id repeated.
Fix: the InstanceIds validator in ops/send_command.rs.
Traces: ssm.send-command-repeated-instance-id-rejected.

Cause 8 of 8: InvalidDocument / InvalidDocumentVersion messages for a stale hash and an unminted version are emulator-authored.
AWS: both are modelled errors on SendCommand (InvalidDocument, InvalidDocumentVersion) with AWS's own wording; the document name, hash and version in the text come from the request.
Emulator: `Document <name> doesn't have Sha256 hash '<hash>'` and `Document <name> has no version '<n>'`.
Why: clients keyed on AWS's text mis-handle a stale document hash or a version that was never minted.
Fix: the document hash/version checks in ops/send_command.rs.
Traces: ssm.send-command-stale-document-hash-rejected, ssm.send-command-unminted-document-version-rejected.

## Evidence
Cause 1: last ListCommands record ('Pending','Pending',0,0,0); "the command reaching a terminal status did not happen within 27s"; "command still 'Pending' after 29s".
Cause 2: "a cancelled running script's invocation ended 'Success' / 'Success'".
Cause 3: invocation ended 'Failed'/'Failed' code 127: 'sh: rev: command not found\n'.
Cause 4: MaxErrors=0 invocation details were ['Failed','Failed'], expected one Failed and one Terminated.
Cause 5: got ('InvalidNotificationConfig', "NotificationArn 'arn:aws:sqs:us-west-2:000000000000:…' must be the ARN of an Amazon Simple Notification Service topic").
Cause 6: got ('InvalidInstanceId', 'Instances not in a valid state for account') for a NotificationConfig with no NotificationArn.
Cause 7: got ('DuplicateInstanceId', '').
Cause 8: got ('InvalidDocument', "Document sim2real-cmdhash-… doesn't have Sha256 hash '22e0…'") and ('InvalidDocumentVersion', "Document sim2real-cmdver-… has no version '2'").

## Steps to Reproduce
1. SendCommand AWS-RunShellScript to a tag target matching no instance; poll ListCommands/ListCommandInvocations for a terminal status.
2. SendCommand a long-running script to a managed node, CancelCommand it, read the invocation status.
3. SendCommand a script that pipes through `rev` on a managed node.
4. SendCommand to two nodes with MaxErrors '0' where the first fails; read both invocation details.
5. SendCommand with NotificationConfig.NotificationArn set to an SQS ARN, and with NotificationConfig carrying no NotificationArn.
6. SendCommand with the same InstanceId twice; with a stale DocumentHash; with DocumentVersion '2' of a one-version document.
