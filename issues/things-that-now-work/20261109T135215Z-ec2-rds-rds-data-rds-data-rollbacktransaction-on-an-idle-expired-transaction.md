Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: ec2__rds__rds-data.idle-transaction-write-is-rolled-back-and-rollback-on-expired-id-refused
Issue-Key: ec2__rds__rds-data.idle-transaction-write-is-rolled-back-and-rollback-on-expired-id-refused|run#21|AssertionError|assert rollback_error.get("Message") == _IDLE_REFUSAL[1], (
Issue-Commit: ae5156e659812a6269d5e057615ff3791f30822e

# rds-data RollbackTransaction on an idle-expired transaction answers the wrong message text for TransactionNotFoundException

> _Note: this issue was identified against the code base at commit `ae5156e659812a6269d5e057615ff3791f30822e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
RollbackTransaction

## Description
Cause 1 of 1: the refusal message for a transaction that timed out while idle is emulator-authored.

AWS: the Data API abandons a transaction left idle past its three-minute inactivity window and rolls back its writes; a later RollbackTransaction on that transactionId is refused with code TransactionNotFoundException and the message 'INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness' - that text is fixed for this error class (the 'INVALID_ARGUMENT: Transaction error: ' prefix plus the idleness clause), with no values taken from the request. Smithy: RollbackTransaction's modeled failures include TransactionNotFoundException; the message is the only thing that distinguishes an idleness expiry from the other termination causes.

Emulator: the same call is refused with the right code but the message 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction' - the wording AWS uses when a prior statement in the transaction failed, not when the transaction aged out while idle.

Why: a client that inspects the message to tell 'my transaction expired because I was idle' from 'my transaction was killed by a failed statement' - the only way to tell them apart, since both carry TransactionNotFoundException - retries or reports the wrong thing.

Fix: the rds-data transaction-expiry bookkeeping and the message its TransactionNotFoundException carries, in the RollbackTransaction/CommitTransaction handler path.

Traces: ec2__rds__rds-data.idle-transaction-write-is-rolled-back-and-rollback-on-expired-id-refused.

## Evidence
Expected: ('TransactionNotFoundException', 'INVALID_ARGUMENT: Transaction error: transaction has been terminated due to an idleness').
Actual message: 'INVALID_ARGUMENT: Transaction error: one previous call resulted in a termination of your transaction'.

## Steps to Reproduce
1. BeginTransaction on an Aurora cluster via rds-data and ExecuteStatement an INSERT inside it.
2. Leave the transaction idle past the 3-minute inactivity window.
3. Call RollbackTransaction with that transactionId and read the error message.
