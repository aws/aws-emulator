Issue-Source: trace-sweep-divergence
Issue-Trace: socialmessaging-call-permission-and-call-settings-refusals
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: socialmessaging-call-permission-and-call-settings-refusals socialmessaging-calling-operations-unlinked-phone-number
Issue-Shape-Id: com.amazonaws.socialmessaging#SocialMessaging
Issue-Key: social-messaging|GetWhatsAppCallPermission|4xx->4xx|$.message
Issue-Key: social-messaging|SendWhatsAppCallEvent|4xx->4xx|$.message
Issue-Commit: 388bf0aeef5b87cdb8b8e5fe5948d049b57af517

# social-messaging validation messages insert a value segment AWS omits

> _Note: this issue was identified against the code base at commit `388bf0aeef5b87cdb8b8e5fe5948d049b57af517`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetWhatsAppCallPermission

## Description
The shared constraint-validation error formatter used by this service's request walk (deserialize -> validate -> handle) always renders a "Value <rendered-value> at '<member>'" segment into the standard AWS validation message. AWS omits that value segment in two cases: when the offending member is @sensitive, and when the violation is a missing (null) required member. Both appear in this corpus, so this is ONE formatter fault with 2 causes.

Cause 1 of 2: GetWhatsAppCallPermission - pattern violation on a @sensitive member echoes the value.
AWS: Smithy GetWhatsAppCallPermission.destinationPhoneNumber -> WhatsAppDestinationPhoneNumber, @length 1..20, @pattern ^\+[1-9]\d{1,14}$, @sensitive; modeled client error ValidationException (HTTP 400). The recorded refusal is HTTP 400 with message "1 validation error detected: Value at 'destinationPhoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: \+[1-9]\d{1,14}". Fixed text of this error class: "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>"; the variable parts are the member name taken from the request's rejected member and the pattern taken from the model. Because the member is @sensitive, the rejected value is NOT part of the message at all. Hypothesis: this is the standard AWS framework behavior of suppressing @sensitive values in validation text.
Emulator: answers HTTP 400 but with "1 validation error detected: Value '12065550142' at 'destinationPhoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: \+[1-9]\d{1,14}" - it splices the quoted rejected phone number into the message.
Why: a client gets a different message string than AWS produces for the same refusal, so message-matching and log-redaction logic diverges; worse, the emulator echoes a value the model marks @sensitive back over the wire, so recordings and logs taken against the emulator contain data AWS deliberately withholds.
Fix: the shared constraint-validation message formatter invoked by the service's validate stage - suppress the "Value <v> " segment whenever the violated member (or its target shape) carries @sensitive, emitting "Value at '<member>' ..." instead.
Traces: socialmessaging-call-permission-and-call-settings-refusals#task#2.

Cause 2 of 2: SendWhatsAppCallEvent - missing required member rendered as the literal "null".
AWS: Smithy SendWhatsAppCallEvent.callEvent -> WhatsAppCallEventBlob, @required, @length 1..2048000, @sensitive; modeled client error ValidationException (HTTP 400). The recorded refusal is HTTP 400 with message "1 validation error detected: Value at 'callEvent' failed to satisfy constraint: Member must not be null". Fixed text of this error class: "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must not be null"; the only variable part is the member name, taken from the required member the request omitted. There is no value to report for an absent member, so no value segment is emitted.
Emulator: answers HTTP 400 but with "1 validation error detected: Value null at 'callEvent' failed to satisfy constraint: Member must not be null" - it renders the absent member as the literal token null inside the value segment.
Why: clients and test suites that assert on the AWS refusal text for an omitted required member see an extra "null " token and treat the response as an unknown error shape.
Fix: the same shared constraint-validation message formatter - a required/not-null violation must emit no value segment rather than rendering the absent value.
Traces: socialmessaging-calling-operations-unlinked-phone-number#task#2.

No existing open issue covers either cause (list_existing_issues finds no social-messaging issue).

## Evidence
Cause 1 (fixture socialmessaging-call-permission-and-call-settings-refusals, step task#2, GetWhatsAppCallPermission): status 400 expected, 400 actual - OK. $.message expected "1 validation error detected: Value at 'destinationPhoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: \+[1-9]\d{1,14}"; actual "1 validation error detected: Value '12065550142' at 'destinationPhoneNumber' failed to satisfy constraint: Member must satisfy regular expression pattern: \+[1-9]\d{1,14}". ('12065550142' is the recorded request's value, not a requirement - the requirement is that NO value segment appears for a @sensitive member.)

Cause 2 (fixture socialmessaging-calling-operations-unlinked-phone-number, step task#2, SendWhatsAppCallEvent): status 400 expected, 400 actual - OK. $.message expected "1 validation error detected: Value at 'callEvent' failed to satisfy constraint: Member must not be null"; actual "1 validation error detected: Value null at 'callEvent' failed to satisfy constraint: Member must not be null".

Model citations (services/service-com-amazonaws-socialmessaging-socialmessaging/smithy-model/com-amazonaws-socialmessaging-socialmessaging.json): GetWhatsAppCallPermission.destinationPhoneNumber {pattern ^\+[1-9]\d{1,14}$, sensitive: true}; SendWhatsAppCallEvent.callEvent {required: true, sensitive: true}; ValidationException httpError 400.

## Steps to Reproduce
1. Boot the emulator from this checkout and replay the wire corpus (replay_wire_corpus).
2. Cause 1: POST /v1/whatsapp/call/permission/get (GetWhatsAppCallPermission) with a valid originationPhoneNumberId and a destinationPhoneNumber that violates ^\+[1-9]\d{1,14}$ (e.g. digits with no leading '+'). Observe the 400 body's message contains "Value '<the number>' at 'destinationPhoneNumber'" instead of "Value at 'destinationPhoneNumber'".
3. Cause 2: POST /v1/whatsapp/call/event (SendWhatsAppCallEvent) omitting the required callEvent member. Observe the 400 body's message contains "Value null at 'callEvent'" instead of "Value at 'callEvent'".
4. Replay fixtures socialmessaging-call-permission-and-call-settings-refusals (task#2) and socialmessaging-calling-operations-unlinked-phone-number (task#2) to confirm both causes are closed.
