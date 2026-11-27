Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: eventbridgev2__iam__sqs.put-events-before-bus-active-refused-or-published-and-replay-shows-which
Issue-Key: eventbridgev2__iam__sqs|PutEvents|InvalidStateException
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# eventbridgev2__iam__sqs: PutEvents to a bus that is not yet ACTIVE is refused with an emulator-authored InvalidStateException message instead of AWS's fixed 'EventBus is in invalid state for PutEventsPutRawEvents. Must be in one of [ACTIVE, UPDATING, UPDATE_FAILED]'

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
PutEvents

## Description
Cause 1 of 1: the InvalidStateException message PutEvents serves for a bus outside the publishable states is the emulator's own prose. Failing check: eventbridgev2__iam__sqs.put-events-before-bus-active-refused-or-published-and-replay-shows-which.

AWS: PutEvents against an event bus that is not in a publishable state is refused with the modelled InvalidStateException (Smithy: PutEvents errors include InvalidStateException, errorKind client, httpError 409; shape doc: 'The resource is not in a state that allows the operation. For example, an event bus that is still being created cannot accept events.'). The message is a fixed sentence that names the operation group and enumerates the permitted states, with NO request- or state-derived values in it: `EventBus is in invalid state for PutEventsPutRawEvents. Must be in one of [ACTIVE, UPDATING, UPDATE_FAILED]`. Every token of that text is fixed — the bus ARN, the bus name and the bus's current state do not appear.

Emulator: it refuses with the right code but a re-worded message that interpolates state the AWS message does not carry: `event bus arn:aws:events:<region>:<account>:event-busv2/<bus-name>/<bus-id> is in state CREATING and cannot accept events` — lower-case prose, the bus ARN spliced in, the current state named, and the permitted-state list absent. (The ARN, name, id and the particular state in that observed string are per-run values, not part of any requirement; the requirement is that none of them appear at all.)

Why: a client that classifies this refusal by its message — matching the `Must be in one of [...]` list to decide whether to wait and retry, or surfacing AWS's sentence to an operator — takes the wrong branch or shows the wrong text against the emulator. Because the emulator's message is strictly more specific, a client written against it would also break on real AWS.

Fix: PutEvents' (and PutRawEvents') bus-state guard in the eventbridgev2 crate — emit the fixed AWS sentence for the InvalidStateException it already raises, rather than composing one from the bus ARN and its current state.

Traces: eventbridgev2__iam__sqs.put-events-before-bus-active-refused-or-published-and-replay-shows-which.

## Evidence
Expected (AWS): ('InvalidStateException', 'EventBus is in invalid state for PutEventsPutRawEvents. Must be in one of [ACTIVE, UPDATING, UPDATE_FAILED]') — HTTP 409, message entirely fixed text.

Actual (emulator): ('InvalidStateException', 'event bus arn:aws:events:us-west-2:000000000000:event-busv2/sim2real-xsvc-ebv2-young-72874fc7c130/adp7diojoqv4t693w1mk0b4qp is in state CREATING and cannot accept events').

Suite output: AssertionError: PutEvents before the bus was ACTIVE was refused with ('InvalidStateException', 'event bus arn:…:event-busv2/<name>/<id> is in state CREATING and cannot accept events'), expected ('InvalidStateException', 'EventBus is in invalid state for PutEventsPutRawEvents. Must be in one of [ACTIVE, UPDATING, UPDATE_FAILED]').

Breakdown: fixed text required = "EventBus is in invalid state for PutEventsPutRawEvents. Must be in one of [ACTIVE, UPDATING, UPDATE_FAILED]"; variable parts = none. The emulator's string instead injects two values from its own state (the bus ARN and the bus's current state) and omits the permitted-state list. The error code and its 409 class match the model.

## Steps to Reproduce
1. eventbridgev2.CreateEventBus(Name=<name>, ClientToken=<token>) and do NOT wait for State to reach ACTIVE (DescribeEventBus reports CREATING).
2. Immediately call PutEvents(EventBusArn=<the new bus ARN>, Entries=[{Source:'sim2real.lifecycle', DetailType:..., Detail:'{...}'}]).
   -> Observed: InvalidStateException with 'event bus <arn> is in state CREATING and cannot accept events'.
   -> Expected: InvalidStateException with exactly 'EventBus is in invalid state for PutEventsPutRawEvents. Must be in one of [ACTIVE, UPDATING, UPDATE_FAILED]'.
