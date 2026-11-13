Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-mfa-preference-and-auth-factors cognito-idp.admin-set-user-mfa-preference-concurrent-conflicting-writes-last-write-wins cognito-idp.auth-session-validity-expires-the-challenge-session-while-a-longer-lived-twin-survives cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates cognito-idp.e2e-remembered-device-skips-mfa-on-next-login cognito-idp.self-service-mfa-preference-enable-and-disable
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-identity-provider|AssociateSoftwareToken|NotImplemented
Issue-Commit: 9d990885182cf7376ea90557ef044b425999cd93

# Implement cognito-idp AssociateSoftwareToken: unimplemented scaffold stub

> _Note: this issue was identified against the code base at commit `9d990885182cf7376ea90557ef044b425999cd93`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AssociateSoftwareToken

## Description
`AssociateSoftwareToken` is an unmodified scaffold stub, so it answers HTTP 501 and persists nothing. Implement it in `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/associate_software_token.rs`: replace the `Err(crate::scaffolded::Scaffolded::not_implemented(..))` with a real handler that reads and writes the service's state, then DELETE that file's `//! SCAFFOLDED, NOT IMPLEMENTED:` header line.

That header line is the stub census — the one record of which operations are still scaffolds. Deleting it without implementing the handler, or leaving it in place on a handler you did implement, corrupts that record rather than closing this issue. A stub's 501 is also CORRECT for a stub: this issue asks you to implement the operation, never to change the error it returns while unimplemented.

6 recorded unit(s) fail on their `AssociateSoftwareToken` call, so each one unblocks only when `AssociateSoftwareToken` works: `cognito-idp.admin-mfa-preference-and-auth-factors`, `cognito-idp.admin-set-user-mfa-preference-concurrent-conflicting-writes-last-write-wins`, `cognito-idp.auth-session-validity-expires-the-challenge-session-while-a-longer-lived-twin-survives`, `cognito-idp.e2e-device-remember-skips-mfa-then-forget-reinstates`, `cognito-idp.e2e-remembered-device-skips-mfa-on-next-login`, `cognito-idp.self-service-mfa-preference-enable-and-disable`.

## Evidence
The handler carries the scaffold marker `//! SCAFFOLDED, NOT IMPLEMENTED:` at the top of `services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/src/ops/associate_software_token.rs`. Replaying the corpus, the emulator refused the `AssociateSoftwareToken` call: File "context.py", line 123, in wrapper
    return func(*args, **kwargs)
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
botocore.exceptions.ClientError: An error occurred (NotImplemented) when calling the AssociateSoftwareToken operation: operation AssociateSoftwareToken is scaffolded but not implemented in this emulator

## Steps to Reproduce
Boot the cognito-idp emulator
Call AssociateSoftwareToken and observe HTTP 501 with no state persisted
Replay any of the blocked units above and observe it fail on that refusal
