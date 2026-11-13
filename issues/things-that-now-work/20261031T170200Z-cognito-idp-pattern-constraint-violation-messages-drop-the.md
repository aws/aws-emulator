Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.delete-nonexistent-terms-fails cognito-idp.describe-terms-by-client-nonexistent-client-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.delete-nonexistent-terms-fails|run#7|AssertionError|assert message == (
Issue-Key: cognito-idp.describe-terms-by-client-nonexistent-client-rejected|run#8|AssertionError|assert message_bad_name == expected_bad_name, (
Issue-Commit: 58db3281dc5b97463dad6d5a03f3fef55333dd92

# cognito-idp: @pattern constraint-violation messages drop the regex anchors, so every "Member must satisfy regular expression pattern" refusal text diverges from AWS

> _Note: this issue was identified against the code base at commit `58db3281dc5b97463dad6d5a03f3fef55333dd92`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DeleteTerms

## Description
One net-new root cause in the cognito-idp request-validation message renderer.

Cause 1 of 1 — the validation-failure message quotes the modeled regular expression with its anchors stripped.

AWS: a string member that carries a Smithy `@pattern` and is given a value that does not match is refused, before any lookup, with HTTP 400 `InvalidParameterException` and the message AWS renders for a constraint violation:
`1 validation error detected: Value '<value>' at '<memberName>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`
The fixed text is `1 validation error detected: Value '`, `' at '`, `' failed to satisfy constraint: Member must satisfy regular expression pattern: `. The variable parts are (a) the rejected value, taken verbatim from the request, (b) the member name in lowerCamelCase as the wire names it (`termsId`, `termsName`), and (c) the pattern, which is the member's modeled `@pattern` reproduced CHARACTER-FOR-CHARACTER. Smithy (services/service-com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice/smithy-model): `TermsIdType` @pattern `^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$`; `TermsNameType` @pattern `^(terms-of-use|privacy-policy)$`; error `InvalidParameterException`. Both modeled patterns begin with `^` and end with `$`.

Emulator: it refuses with the right status and the right error code, but it re-serializes the pattern without the leading `^` and the trailing `$`. DeleteTerms with a non-v4 UUID answers `...Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}` (anchors gone), and DescribeTermsByClient with TermsName='not-a-valid-terms-name' answers `...Member must satisfy regular expression pattern: (terms-of-use|privacy-policy)` instead of `^(terms-of-use|privacy-policy)$`. The two failures are the same renderer seen through two different members, so this is one cause and not one per operation.

Why: the message is the only machine-readable description of the constraint a client gets back from a validation refusal. A client, SDK test or support tool that parses the message and re-applies the quoted regex locally gets a materially WEAKER rule than AWS stated: unanchored, `(terms-of-use|privacy-policy)` matches `xx-terms-of-use-yy` and the unanchored UUID pattern matches any string that merely CONTAINS a UUID, so a value real AWS refuses is accepted by the client-side re-check. Any assertion on the exact refusal text — the behavioural suite's included — fails against the emulator.

Fix: the shared request-validation / constraint-violation message formatter for this service (the component that turns a `@pattern` violation into the `InvalidParameterException` message) must emit the modeled pattern string verbatim, anchors included, rather than a normalised or anchor-trimmed form of it. The same formatter serves every patterned member in the service, so correcting it there fixes both failing members and prevents the rest of the service's patterned members from diverging the same way. Related (not the same defect): the already-fixed 20261027T110306Z-cognito-idp-the-terms-not-found-refusal-message.md covered the Terms NOT-FOUND wording, and the open 20261029T205621Z-cognito-idp-terms-branding-and-webauthn-refusals-serve.md covers Terms refusals served with the wrong HTTP status — neither covers the pattern text inside a validation message.

Traces: cognito-idp.delete-nonexistent-terms-fails, cognito-idp.describe-terms-by-client-nonexistent-client-rejected.

## Evidence
Verbatim from the replay.

cognito-idp.delete-nonexistent-terms-fails (DeleteTerms, TermsId='00000000-0000-0000-0000-000000000000'):
  expected message: "1 validation error detected: Value '00000000-0000-0000-0000-000000000000' at 'termsId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$"
  AssertionError: unexpected message "1 validation error detected: Value '00000000-0000-0000-0000-000000000000' at 'termsId' failed to satisfy constraint: Member must satisfy regular expression pattern: [0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}"

cognito-idp.describe-terms-by-client-nonexistent-client-rejected (DescribeTermsByClient, TermsName='not-a-valid-terms-name'):
  AssertionError: expected message "1 validation error detected: Value 'not-a-valid-terms-name' at 'termsName' failed to satisfy constraint: Member must satisfy regular expression pattern: ^(terms-of-use|privacy-policy)$", got "1 validation error detected: Value 'not-a-valid-terms-name' at 'termsName' failed to satisfy constraint: Member must satisfy regular expression pattern: (terms-of-use|privacy-policy)"

In both cases HTTP 400 and Error.Code=InvalidParameterException are correct; only the pattern inside Error.Message differs, by exactly the leading '^' and trailing '$'. The modeled patterns both carry those anchors (smithy_query shape TermsIdType/TermsNameType).

## Steps to Reproduce
1. CreateUserPool, then CreateUserPoolClient in it.
2. Call DeleteTerms(UserPoolId=<pool>, TermsId='00000000-0000-0000-0000-000000000000') — a well-formed UUID that is not a v4 UUID, so it fails the modeled TermsIdType @pattern.
   Expect: HTTP 400, Error.Code=InvalidParameterException, Error.Message="1 validation error detected: Value '00000000-0000-0000-0000-000000000000' at 'termsId' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[4][0-9a-fA-F]{3}-[89abAB][0-9a-fA-F]{3}-[0-9a-fA-F]{12}$".
   Observe: the same message with the leading '^' and trailing '$' removed from the pattern.
3. Call DescribeTermsByClient(UserPoolId=<pool>, ClientId=<client>, TermsName='not-a-valid-terms-name').
   Expect: HTTP 400, InvalidParameterException, "... Member must satisfy regular expression pattern: ^(terms-of-use|privacy-policy)$".
   Observe: "... Member must satisfy regular expression pattern: (terms-of-use|privacy-policy)".
4. Compare each returned pattern substring against the member's @pattern in the service's Smithy model — they must be byte-identical.
