Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudtrail__ssm.create-maintenance-window-refused-call-is-recorded-with-its-error-beside-the-accepted-one
Issue-Key: cloudtrail__ssm.create-maintenance-window-refused-call-is-recorded-with-its-error-beside-the-accepted-one|_events_for#15|AssertionError|raise AssertionError(
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# cloudtrail__ssm: SSM management calls are never recorded in CloudTrail event history — neither the accepted nor the refused CreateMaintenanceWindow appears

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
LookupEvents

## Description
Cause 1 of 1: no CloudTrail event-history record is written for SSM management calls.
AWS: every SSM management call is recorded in event history and returned by LookupEvents — the accepted CreateMaintenanceWindow and the refused one side by side, the refused record carrying its errorCode/errorMessage. Both appear within minutes of the call.
Emulator: LookupEvents returns neither: event history held 0 of the 2 CreateMaintenanceWindow events after 10 s of polling, so a refused call cannot be told from one that never happened.
Why: audit-driven clients (and any 'who called what, and did it fail' diagnosis) get an empty history for a service that was definitely called; the refusal's errorCode is unavailable anywhere after the fact.
Fix: the CloudTrail event-history writer must record SSM management calls (both outcomes), i.e. the SSM crate's audit hook into the CloudTrail crate's event store.
Traces: cloudtrail__ssm.create-maintenance-window-refused-call-is-recorded-with-its-error-beside-the-accepted-one.

## Evidence
'Event history held 0 of the 2 CreateMaintenanceWindow events after 10s' — one accepted CreateMaintenanceWindow and one refused call were made before the lookup.

## Steps to Reproduce
Call CreateMaintenanceWindow once with a valid request and once with a request SSM refuses, then poll CloudTrail LookupEvents with EventName='CreateMaintenanceWindow'. AWS returns both records (the refused one with errorCode); the emulator returns none.
