Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp__lambda.verify-auth-challenge-rejects-an-answer-unless-respond-client-metadata-vouches-for-it
Issue-Key: cognito-idp__lambda.verify-auth-challenge-rejects-an-answer-unless-respond-client-metadata-vouches-for-it|run#25|AssertionError|assert bare == ("NotAuthorizedException", "Incorrect username or password."), (
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# cognito-idp: AdminInitiateAuth/InitiateAuth with AuthFlow=CUSTOM_AUTH answers NotImplemented, so the VerifyAuthChallenge contract cannot be exercised

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminInitiateAuth

## Description
Cause 1 of 1: the CUSTOM_AUTH authentication flow is a scaffold stub.
AWS: AdminInitiateAuth and InitiateAuth accept `AuthFlow=CUSTOM_AUTH` and drive the DefineAuthChallenge / CreateAuthChallenge / VerifyAuthChallengeResponse trigger chain; a challenge answer offered without the respond-time ClientMetadata the VerifyAuthChallenge trigger requires is refused by that trigger, not by the service. Smithy: AuthFlowType includes CUSTOM_AUTH and AdminInitiateAuth models no `NotImplemented` error — the error the emulator returns is not in the operation's error set at all.
Emulator: answers `('NotImplemented', 'operation AdminInitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator')`, so no custom-auth challenge can be started and the trigger contract under test is never reached.
Why: a client cannot exercise custom authentication at all, and the error code it receives is outside the operation's modelled errors, so its SDK surfaces an unmodelled exception rather than a Cognito one.
Fix: CUSTOM_AUTH support in the AdminInitiateAuth / InitiateAuth / RespondToAuthChallenge handlers of services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice, and the challenge-trigger chain they drive.
Traces: cognito-idp__lambda.verify-auth-challenge-rejects-an-answer-unless-respond-client-metadata-vouches-for-it.

Related open issues describe the same stub from other checks but carry none of this check's replay key: 20261024T114943Z-cognito-idp-lambda-the-pool-s-jwks-document.md and 20261025T014422Z-apigateway-cognito-idp-lambda-the-defineauthchallenge-trigger-chain.md.

## Evidence
`AssertionError: an answer with no respond-time metadata gave ('NotImplemented', 'operation AdminInitiateAuth with AuthFlow=CUSTOM_AUTH is scaffolded but not implemented in this emulator')` — expected the VerifyAuthChallenge trigger to refuse the answer with a Cognito error.

## Steps to Reproduce
Create a user pool with DefineAuthChallenge, CreateAuthChallenge and VerifyAuthChallengeResponse triggers, then `aws cognito-idp admin-initiate-auth --auth-flow CUSTOM_AUTH --auth-parameters USERNAME=<user>`; the emulator answers NotImplemented instead of issuing the first challenge. Replay check cognito-idp__lambda.verify-auth-challenge-rejects-an-answer-unless-respond-client-metadata-vouches-for-it.
