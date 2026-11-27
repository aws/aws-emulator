Issue-Severity: LOW
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: logs__redshift.enable-logging-rejected-while-provisioning-then-cloudwatch-log-group-created
Issue-Key: logs__redshift.enable-logging-rejected-while-provisioning-then-cloudwatch-log-group-created|run#15|AssertionError|assert message == "Logging cannot be enabled at this time. Please try again later.", (
Issue-Commit: 7300602cd0fc37394a6328fdd869591f4a36c031

# redshift EnableLogging while the cluster is still provisioning is refused with the wrong message

> _Note: this issue was identified against the code base at commit `7300602cd0fc37394a6328fdd869591f4a36c031`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
EnableLogging

## Description
Cause 1 of 1: `EnableLogging` called before the cluster is available is refused, but with the emulator's own sentence instead of AWS's.

AWS: enabling audit logging on a cluster that is not yet `available` is refused with Redshift's invalid-cluster-state error, whose message names the cluster and the state requirement in AWS's own wording. Smithy (redshift): `EnableLogging` models `InvalidClusterStateFault` (and `ClusterNotFoundFault`), so the refusal class is modelled; the exact sentence is recorded behavior. The fixed text is AWS's state sentence and the variable parts are the cluster identifier from the request and the cluster's current state from the emulator's own state.

Emulator: the message is `Cluster <cluster-identifier> is in state creating; it must be available.` — a locally invented sentence. The cluster identifier and the state `creating` are the correct variable values; the surrounding wording is not AWS's.

Why: a client that waits-and-retries keys on the error message (or presents it to a user) and gets text AWS never produces. Message-matching retry logic that works against AWS does not recognise this refusal, so the retry loop either gives up or treats a transient state as a permanent failure.

Fix: the EnableLogging handler's refusal for a non-available cluster — raise `InvalidClusterStateFault` with AWS's message wording, substituting the cluster identifier and state.

Traces: logs__redshift.enable-logging-rejected-while-provisioning-then-cloudwatch-log-group-created.

## Evidence
Actual message: "Cluster sim2real-behav-rs-log-e95141483a3f is in state creating; it must be available." (reported by the check as "unexpected message"). Expected: Redshift's own InvalidClusterState sentence for a cluster that is not available, with the cluster identifier and current state as its only variable parts.

## Steps to Reproduce
1. CreateCluster and immediately call EnableLogging with a LogDestinationType of cloudwatch while the cluster is still 'creating'.
2. Compare the refusal's Message with the one AWS produces for an invalid cluster state.
