Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: connectparticipant.cancel-participant-authentication-unissued-session-rejected
Issue-Shape-Id: com.amazonaws.connectparticipant#AmazonConnectParticipantServiceLambda
Issue-Key: connectparticipant|DescribeContact|NotImplemented
Issue-Commit: 1d46c777556b138301c0ca0bdf752665685bd84a

# connectparticipant: the CancelParticipantAuthentication unissued-session check cannot reach the participant service — connect DescribeContact is an unimplemented 501 scaffold stub

> _Note: this issue was identified against the code base at commit `1d46c777556b138301c0ca0bdf752665685bd84a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DescribeContact

## Description
Cause 1 of 1: the behavioral check that grades CancelParticipantAuthentication's refusal of a SessionId no authentication flow issued never reaches connectparticipant at all — its setup calls Amazon Connect's DescribeContact, which this emulator answers with HTTP 501 NotImplemented, so the check errors out and every assertion about the participant service is left ungraded. Failing check: connectparticipant.cancel-participant-authentication-unissued-session-rejected.

AWS: Amazon Connect models DescribeContact as a normal control-plane read ("Describes the specified contact"); it answers 200 with the contact's Contact structure, or refuses a bad input/absent contact with that operation's modeled errors. It never answers 501, and NotImplemented is not an error any AWS service returns for a published operation. Downstream, connectparticipant's CancelParticipantAuthentication (POST /participant/cancel-authentication, service-common ValidationException httpError 400 with Message required) must refuse a well-formed SessionId that no Authenticate Customer flow issued with the fixed sentence "Specified request parameter(s) are invalid." — whole message fixed text, naming neither the session nor the contact (recorded behavior; the check's own constant records this expectation and it is what the suite will grade once the call can be reached).

Emulator: DescribeContact returns HTTP 501 with code NotImplemented and the message "operation DescribeContact is scaffolded but not implemented in this emulator". services/service-com-amazonaws-connect-amazonconnectservice/src/ops/describe_contact.rs still carries the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker (lines 3-12) and its handle() body returns the scaffold Err(..), so no behavior is asserted for the operation.

Why: a client cannot read back a contact it just started, which is the ordinary way to learn a chat's state, and here it also makes a whole negative path of the participant service unreachable: the authentication-session refusal class of CancelParticipantAuthentication is never exercised, so a wrong status, code or message there would go unnoticed. The 501 itself is not an AWS response, so an SDK sees an unmodeled error for a documented call.

Fix: implement DescribeContact in services/service-com-amazonaws-connect-amazonconnectservice/src/ops/describe_contact.rs (reading the contact from the connect crate's state and serializing the modeled Contact, refusing an unknown contact with the operation's modeled not-found error), and remove the SCAFFOLDED marker line. Once the check can proceed, verify connectparticipant's CancelParticipantAuthentication refusal for an unissued SessionId carries 400 ValidationException with AWS's fixed sentence.

Traces: connectparticipant.cancel-participant-authentication-unissued-session-rejected (listed under the suite's `connectparticipant` heading; the blocking operation lives in the connect crate). Note: the suite separately reports a CreateIntegrationAssociation scaffold stub as already filed for you; THIS check is blocked by a different operation, DescribeContact, and was not marked already filed or owned.

## Evidence
Behavioral suite simparity-behavioral-aws-v1, service connectparticipant: 18/34 passed (53%), 16 failed.

[error] connectparticipant.cancel-participant-authentication-unissued-session-rejected
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the DescribeContact operation: operation DescribeContact is scaffolded but not implemented in this emulator

Expected: DescribeContact answers HTTP 200 with the contact's Contact structure (or one of its modeled refusals). Actual: HTTP 501, code NotImplemented — a response class AWS does not emit for a published operation, so the check aborts in setup.

The check's own expectation for the operation it was written to grade, left ungraded by this abort: CancelParticipantAuthentication with a well-formed SessionId that no authentication flow issued must answer HTTP 400 ValidationException with the fixed message 'Specified request parameter(s) are invalid.' (the check records that sentence as "AWS's message for a SessionId of the right length that no authentication flow issued").

Code read for grounding: services/service-com-amazonaws-connect-amazonconnectservice/src/ops/describe_contact.rs lines 3-12 carry the `//! SCAFFOLDED, NOT IMPLEMENTED:` marker; the operation's handle() returns the scaffold error channel's NotImplemented.

## Steps to Reproduce
1. Boot the emulator from this checkout and point connect + connectparticipant clients at it.
2. Create a Connect instance, publish a contact flow, and StartChatContact to get a ContactId.
3. Call connect DescribeContact(InstanceId=<instance>, ContactId=<contact>) — it answers HTTP 501 with code NotImplemented and the message 'operation DescribeContact is scaffolded but not implemented in this emulator'. AWS answers HTTP 200 with the contact's Contact structure.
4. Implement the operation, then re-replay connectparticipant.cancel-participant-authentication-unissued-session-rejected: it must get past setup and grade CancelParticipantAuthentication's refusal of an unissued SessionId as HTTP 400 ValidationException with message 'Specified request parameter(s) are invalid.'.
