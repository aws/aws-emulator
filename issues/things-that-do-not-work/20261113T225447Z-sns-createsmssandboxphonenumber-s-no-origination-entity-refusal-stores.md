Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sns.verify-sms-sandbox-wrong-otp-rejected
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sns.verify-sms-sandbox-wrong-otp-rejected|_sandbox_status#7|AssertionError|raise AssertionError(f"phone number {phone_number} never appeared in sandbox list")
Issue-Commit: 23d7c3ca3e68cb70ca93342274fc19d1394baa26

# sns: CreateSMSSandboxPhoneNumber's no-origination-entity refusal stores nothing, so the destination number never reaches Pending in ListSMSSandboxPhoneNumbers and the sandbox verification flow cannot be exercised

> _Note: this issue was identified against the code base at commit `23d7c3ca3e68cb70ca93342274fc19d1394baa26`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateSMSSandboxPhoneNumber

## Description
REOPENS 20261030T200727Z-sns-5-wire-divergences-the-opt-out-check.md: its fix left the call refused.

ONE net-new root cause of service `sns`, grounded in the single failing check in trace_ids. The check is not marked ALREADY FILED or OWNED in the replay report.

=== Cause 1 of 1: the UserError refusal is taken BEFORE the destination number is registered, so no pending sandbox record ever exists ===
AWS: Smithy (com.amazonaws.sns#CreateSMSSandboxPhoneNumber) declares UserErrorException (400 / `UserError`) beside InvalidParameter/OptedOut/Throttled, and documents the operation as "Adds a destination phone number to an Amazon Web Services account in the SMS sandbox AND sends a one-time password (OTP) to that phone number" — two effects, the registration and the send. ListSMSSandboxPhoneNumbers reports each registered number with `Status` (Pending until VerifySMSSandboxPhoneNumber succeeds). Recorded behavior: on an account that holds no origination entity able to send the OTP, AWS raises the 400 UserError ('No origination entities available to send') for the SEND, but the destination number is STILL registered and reads back from ListSMSSandboxPhoneNumbers with Status 'Pending' — the failure is send-only. That is exactly what the check relies on: it tolerates a UserError from the create and then confirms Pending through an independent list read before verifying with a wrong OTP.
Emulator: the refusal is raised ahead of any write. services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/create_sms_sandbox_phone_number.rs checks `state::sms_origination::can_send_sms(...)` and returns UserErrorException with `NO_ORIGINATION_ENTITIES` before `store.sandbox_phone_numbers.insert(...)` is reached — its own comment states "nothing is stored". The number therefore never appears in ListSMSSandboxPhoneNumbers, and the whole sandbox surface downstream of registration (ListSMSSandboxPhoneNumbers status, VerifySMSSandboxPhoneNumber's wrong-OTP VerificationException, DeleteSMSSandboxPhoneNumber) is unreachable.
Why: the SMS sandbox is the only path by which a new account can send SMS at all, and in the default (no origination identity) state the emulator now makes it a dead end: a client registers a destination, is handed an error AWS also returns, and then finds the number absent from the sandbox list — so it cannot tell "registration failed" from "OTP not sent", cannot verify, and cannot delete. Every test of the verification state machine is blocked.
Fix: the CreateSMSSandboxPhoneNumber handler's ordering — register (or refresh) the destination number as Pending in the store, then report the send failure as UserErrorException 400 'No origination entities available to send'. The refusal introduced for the wire recording must stay; what must change is that it no longer discards the registration.
Traces: sns.verify-sms-sandbox-wrong-otp-rejected.

## Evidence
sns.verify-sms-sandbox-wrong-otp-rejected [mismatch]:
  File "check_sns_verify_sms_sandbox_wrong_otp_rejected.py", line 53, in run — status = _sandbox_status(sns, pending_number)
  File "check_sns_verify_sms_sandbox_wrong_otp_rejected.py", line 148, in _sandbox_status
  AssertionError: phone number +447700900193 never appeared in sandbox list
(the number is drawn per run from the Ofcom fictional-use range +447700900xxx; the specific digits are a per-run value.)
Expected: after CreateSMSSandboxPhoneNumber — whether it answered 200 or raised the documented UserError 'No origination entities available to send' — ListSMSSandboxPhoneNumbers reports that number with Status 'Pending', VerifySMSSandboxPhoneNumber with a wrong OTP is then refused with VerificationException and leaves the status at Pending, and a number never added is refused with a distinct error class.
Actual: the number is absent from ListSMSSandboxPhoneNumbers entirely, so the Pending precondition cannot be established and nothing after it runs.
Code fact: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/ops/create_sms_sandbox_phone_number.rs returns UserErrorException(NO_ORIGINATION_ENTITIES) before the sandbox_phone_numbers insert.

## Steps to Reproduce
1. Against a freshly booted emulator (no origination entity seeded), call CreateSMSSandboxPhoneNumber(PhoneNumber='+447700900193') — it answers 400 UserError 'No origination entities available to send'.
2. Call ListSMSSandboxPhoneNumbers — the number is absent; against AWS it is listed with Status 'Pending'.
3. With the number present (as AWS has it), VerifySMSSandboxPhoneNumber(PhoneNumber=<that number>, OneTimePassword=<a wrong code>) must be refused with VerificationException and leave Status 'Pending', and VerifySMSSandboxPhoneNumber for a number never added must be a distinct error class.
4. Re-replay sns.verify-sms-sandbox-wrong-otp-rejected.
