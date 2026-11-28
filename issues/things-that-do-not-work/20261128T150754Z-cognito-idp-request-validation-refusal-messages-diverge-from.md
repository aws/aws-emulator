Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.add-user-to-group-username-over-128-chars-rejected cognito-idp.admin-add-user-to-group-username-with-whitespace-rejected cognito-idp.admin-create-user-temporary-password-longer-than-256-characters-rejected cognito-idp.admin-initiate-auth-password-over-131072-chars-rejected cognito-idp.admin-initiate-auth-session-outside-length-bounds-rejected cognito-idp.admin-respond-to-auth-challenge-oversized-challenge-response-rejected cognito-idp.admin-respond-to-auth-challenge-response-value-over-maximum-rejected cognito-idp.admin-set-user-password-leading-space-rejected cognito-idp.admin-set-user-password-longer-than-256-characters-rejected cognito-idp.associate-software-token-malformed-access-token-rejected cognito-idp.change-password-previous-password-over-maximum-rejected cognito-idp.change-password-proposed-over-256-chars-rejected cognito-idp.delete-user-malformed-access-token-keeps-user cognito-idp.get-tokens-from-refresh-token-malformed-refresh-token-rejected cognito-idp.global-sign-out-access-token-outside-pattern-rejected cognito-idp.password-of-256-characters-signs-in-and-257-rejected cognito-idp.password-of-257-characters-rejected-and-256-signs-in cognito-idp.revoke-token-outside-token-alphabet-rejected cognito-idp.username-of-128-characters-signs-in-and-129-rejected cognito-idp.verify-software-token-five-digit-code-rejected-during-mfa-setup cognito-idp.verify-software-token-non-numeric-user-code-rejected cognito-idp.verify-software-token-session-non-digit-code-rejected cognito-idp.verify-software-token-seven-digit-code-rejected cognito-idp.initiate-auth-encoded-data-over-131072-chars-rejected cognito-idp.admin-confirm-sign-up-client-metadata-value-over-limit-rejected cognito-idp.password-inner-space-counts-as-a-symbol-and-signs-in cognito-idp.change-password-trailing-tab-rejected-inner-tab-accepted cognito-idp.admin-set-user-password-edge-whitespace-rejected cognito-idp.change-password-proposed-password-with-a-leading-space-rejected cognito-idp.change-password-previous-password-with-whitespace-rejected cognito-idp.admin-create-user-attribute-name-outside-pattern-rejected cognito-idp.admin-delete-user-attributes-control-character-name-rejected cognito-idp.admin-delete-user-attributes-name-over-32-chars-rejected cognito-idp.list-users-attributes-to-get-control-character-rejected cognito-idp.list-users-overlong-attribute-name-rejected cognito-idp.admin-list-devices-empty-pagination-token-rejected cognito-idp.admin-get-device-key-over-55-chars-rejected cognito-idp.list-resource-servers-empty-next-token-rejected cognito-idp.create-resource-server-scope-name-length-bound-rejected cognito-idp.create-user-pool-client-empty-write-attribute-rejected cognito-idp.create-user-pool-name-length-bounds-rejected cognito-idp.create-terms-empty-links-map-rejected cognito-idp.managed-login-branding-asset-bytes-over-one-million-rejected cognito-idp.app-client-fifty-first-allowed-scope-rejected cognito-idp.create-user-pool-client-allowed-oauth-scopes-count-bound-rejected cognito-idp.tag-resource-tag-key-over-128-characters-rejected cognito-idp.tag-resource-value-over-256-chars-rejected cognito-idp.untag-resource-tag-key-length-bound-rejected cognito-idp.create-user-pool-tag-value-past-256-chars-rejected cognito-idp.list-tags-for-resource-overlong-arn-rejected cognito-idp.admin-list-groups-for-user-next-token-longer-than-65535-characters-rejected cognito-idp.admin-list-groups-for-user-next-token-over-65535-chars-rejected cognito-idp.list-terms-next-token-over-131072-chars-rejected cognito-idp.admin-list-user-auth-events-unparseable-next-token-restarts-at-newest cognito-idp.create-terms-link-with-whitespace-rejected cognito-idp.create-terms-unknown-language-link-key-rejected cognito-idp.get-client-token-malformed-scope-rejected cognito-idp.get-client-token-fifty-first-scope-rejected-unless-repeated cognito-idp.initiate-auth-context-ip-address-over-131072-chars-rejected cognito-idp.add-user-pool-client-secret-custom-value-outside-length-rejected cognito-idp.supplied-client-secret-with-hyphens-signs-admin-sign-in cognito-idp.get-client-token-secret-outside-length-bounds-rejected
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.add-user-to-group-username-over-128-chars-rejected|run#14|AssertionError|assert got == want, f"a 129-character Username was answered {got!r}, expected {want!r}"
Issue-Key: cognito-idp.admin-add-user-to-group-username-with-whitespace-rejected|run#12|AssertionError|assert got == want, f"a Username with a space gave {got!r}, expected {want!r}"
Issue-Key: cognito-idp.admin-create-user-temporary-password-longer-than-256-characters-rejected|run#9|AssertionError|assert refused == ("InvalidParameterException",
Issue-Key: cognito-idp.admin-initiate-auth-password-over-131072-chars-rejected|run#13|AssertionError|assert (code, message) == ("InvalidParameterException", expected), (
Issue-Key: cognito-idp.admin-initiate-auth-session-outside-length-bounds-rejected|run#14|AssertionError|assert got == ("InvalidParameterException",
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-oversized-challenge-response-rejected|run#13|AssertionError|assert over == (
Issue-Key: cognito-idp.admin-respond-to-auth-challenge-response-value-over-maximum-rejected|run#15|AssertionError|assert got == want, f"{_MAX + 1}-character NEW_PASSWORD: expected {want!r}, got {got!r}"
Issue-Key: cognito-idp.admin-set-user-password-leading-space-rejected|run#10|AssertionError|assert got == ("InvalidParameterException", _PATTERN_REFUSAL), (
Issue-Key: cognito-idp.admin-set-user-password-longer-than-256-characters-rejected|run#18|AssertionError|assert got == ("InvalidParameterException",
Issue-Key: cognito-idp.associate-software-token-malformed-access-token-rejected|run#13|AssertionError|assert got == (
Issue-Key: cognito-idp.change-password-previous-password-over-maximum-rejected|run#14|AssertionError|assert got == want, f"{_MAX + 1}-character previous password: expected {want!r}, got {got!r}"
Issue-Key: cognito-idp.change-password-proposed-over-256-chars-rejected|run#11|AssertionError|assert got == (
Issue-Key: cognito-idp.delete-user-malformed-access-token-keeps-user|run#14|AssertionError|assert refused == ("InvalidParameterException", _TOKEN_PATTERN_REFUSAL), (
Issue-Key: cognito-idp.get-tokens-from-refresh-token-malformed-refresh-token-rejected|run#11|AssertionError|assert bad_shape == (
Issue-Key: cognito-idp.global-sign-out-access-token-outside-pattern-rejected|run#13|AssertionError|assert got == _PATTERN_REFUSAL, (
Issue-Key: cognito-idp.password-of-256-characters-signs-in-and-257-rejected|run#19|AssertionError|assert (code, msg) == (
Issue-Key: cognito-idp.password-of-257-characters-rejected-and-256-signs-in|run#16|AssertionError|assert got == (
Issue-Key: cognito-idp.revoke-token-outside-token-alphabet-rejected|run#14|AssertionError|assert (code, message) == (
Issue-Key: cognito-idp.username-of-128-characters-signs-in-and-129-rejected|run#18|AssertionError|assert (code, msg) == (
Issue-Key: cognito-idp.verify-software-token-five-digit-code-rejected-during-mfa-setup|run#16|AssertionError|assert got == (
Issue-Key: cognito-idp.verify-software-token-non-numeric-user-code-rejected|run#13|AssertionError|assert lettered == (
Issue-Key: cognito-idp.verify-software-token-session-non-digit-code-rejected|run#14|AssertionError|assert got == ("InvalidParameterException", _PATTERN_REFUSAL), (
Issue-Key: cognito-idp.verify-software-token-seven-digit-code-rejected|run#14|AssertionError|assert (code, message) == expected, f"a seven-digit code gave {(code, message)!r}"
Issue-Key: cognito-idp.initiate-auth-encoded-data-over-131072-chars-rejected|run#17|AssertionError|assert got == want, (
Issue-Key: cognito-idp.admin-confirm-sign-up-client-metadata-value-over-limit-rejected|run#16|AssertionError|assert over == want, (
Issue-Key: cognito-identity-provider|AdminSetUserPassword|InvalidParameterException
Issue-Key: cognito-idp.change-password-trailing-tab-rejected-inner-tab-accepted|run#15|AssertionError|assert refused == ("InvalidParameterException", PATTERN_REFUSAL), (
Issue-Key: cognito-identity-provider|ChangePassword|InvalidParameterException
Issue-Key: cognito-idp.change-password-previous-password-with-whitespace-rejected|run#11|AssertionError|assert (code, message) == ("NotAuthorizedException", "Incorrect username or password."), (
Issue-Key: cognito-idp.admin-create-user-attribute-name-outside-pattern-rejected|run#6|AssertionError|assert got == _pattern_refusal(bad_name), (
Issue-Key: cognito-idp.admin-delete-user-attributes-control-character-name-rejected|run#9|AssertionError|assert got == want, f"attribute name {bad!r}: expected {want!r}, got {got!r}"
Issue-Key: cognito-idp.admin-delete-user-attributes-name-over-32-chars-rejected|run#10|AssertionError|assert got == (
Issue-Key: cognito-idp.list-users-attributes-to-get-control-character-rejected|run#15|AssertionError|assert got == ("InvalidParameterException",
Issue-Key: cognito-idp.list-users-overlong-attribute-name-rejected|run#14|AssertionError|assert got == want, f"a 33-character attribute name answered {got!r}, want {want!r}"
Issue-Key: cognito-idp.admin-list-devices-empty-pagination-token-rejected|run#12|AssertionError|assert got == want, f"an empty PaginationToken was refused with {got!r}, expected {want!r}"
Issue-Key: cognito-idp.admin-get-device-key-over-55-chars-rejected|run#23|AssertionError|assert got == (
Issue-Key: cognito-idp.list-resource-servers-empty-next-token-rejected|run#14|AssertionError|assert got == ("InvalidParameterException", _EMPTY_TOKEN_MESSAGE), (
Issue-Key: cognito-idp.create-resource-server-scope-name-length-bound-rejected|run#20|AssertionError|assert got == want, f"an empty scope name gave {got!r}"
Issue-Key: cognito-idp.create-user-pool-client-empty-write-attribute-rejected|run#8|AssertionError|assert (code, message) == (
Issue-Key: cognito-idp.create-user-pool-name-length-bounds-rejected|run#14|AssertionError|assert message == expected, f"an empty PoolName gave message {message!r}"
Issue-Key: cognito-idp.create-terms-empty-links-map-rejected|run#9|AssertionError|assert (code, message) == ("InvalidParameterException", expected), (
Issue-Key: cognito-idp.managed-login-branding-asset-bytes-over-one-million-rejected|run#12|AssertionError|assert refused == expected, f"an asset of 1,000,001 bytes was answered with {refused!r}"
Issue-Key: cognito-idp.app-client-fifty-first-allowed-scope-rejected|run#12|AssertionError|assert got == ("LimitExceededException", limit_text), (
Issue-Key: cognito-idp.create-user-pool-client-allowed-oauth-scopes-count-bound-rejected|run#22|AssertionError|assert got == want, f"allowing {len(every_scope)} scopes gave {got!r}"
Issue-Key: cognito-idp.tag-resource-tag-key-over-128-characters-rejected|_refusal#8|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: cognito-idp.tag-resource-value-over-256-chars-rejected|run#6|AssertionError|assert got == (
Issue-Key: cognito-idp.untag-resource-tag-key-length-bound-rejected|run#11|AssertionError|assert got == want, (
Issue-Key: cognito-idp.create-user-pool-tag-value-past-256-chars-rejected|run#13|AssertionError|assert got == expected, (
Issue-Key: cognito-idp.list-tags-for-resource-overlong-arn-rejected|run#11|AssertionError|assert got == want, (
Issue-Key: cognito-idp.admin-list-groups-for-user-next-token-longer-than-65535-characters-rejected|run#16|AssertionError|assert got == ("InvalidParameterException",
Issue-Key: cognito-idp.admin-list-groups-for-user-next-token-over-65535-chars-rejected|run#17|AssertionError|assert (code, message) == (
Issue-Key: cognito-idp.list-terms-next-token-over-131072-chars-rejected|run#9|AssertionError|assert at_limit == want_token, (
Issue-Key: cognito-identity-provider|AdminListUserAuthEvents|InvalidParameterException
Issue-Key: cognito-idp.create-terms-link-with-whitespace-rejected|run#8|AssertionError|assert (code, message) == (
Issue-Key: cognito-idp.create-terms-unknown-language-link-key-rejected|run#9|AssertionError|assert got == want, f"an unlisted language key was refused with {got!r}, expected {want!r}"
Issue-Key: cognito-idp.get-client-token-malformed-scope-rejected|run#12|AssertionError|assert message == expected, f"a scope with a space gave message {message!r}"
Issue-Key: cognito-identity-provider|GetClientToken|InvalidParameterException
Issue-Key: cognito-identity-provider|InitiateAuth|InvalidParameterException
Issue-Key: cognito-idp.add-user-pool-client-secret-custom-value-outside-length-rejected|run#14|AssertionError|assert got == want, f"{n}-character client secret: expected {want!r}, got {got!r}"
Issue-Key: cognito-identity-provider|AddUserPoolClientSecret|InvalidParameterException
Issue-Key: cognito-idp.get-client-token-secret-outside-length-bounds-rejected|run#13|AssertionError|assert long == (
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: request-validation refusal messages diverge from AWS (10 causes: sensitive-value echo, password pattern, list aggregation, tags, pagination, terms, scopes, user-context, client secret)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminSetUserPassword

## Description
Ten distinct defects in how cognito-idp validates requests and words the resulting InvalidParameterException. All are 400 InvalidParameterException refusals whose message text (the requirement) differs from AWS's, or whose constraint is applied to the wrong characters.

Cause 1 of 10: validation messages echo the value of @sensitive members.
AWS: Smithy marks PasswordType, UsernameType, TokenModelType, SessionType, ClientSecretType and the software-token user code @sensitive. For a @sensitive member AWS omits the value from the constraint message: "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [\S]+" — fixed text with the member path and the constraint taken from the model, and NO value.
Emulator: renders "Value '<the submitted secret>' at 'password' failed to satisfy constraint: ...", i.e. it interpolates the sensitive value for password, previousPassword, proposedPassword, temporaryPassword, username, accessToken, refreshToken, token, session, userCode, challengeResponses values, userContextData.encodedData and clientMetadata values.
Why: a client that logs or surfaces the error leaks the caller's password/token, and message assertions never match AWS.
Fix: the shared request-validation message renderer must suppress the value for members whose target shape carries @sensitive.
Traces: cognito-idp.add-user-to-group-username-over-128-chars-rejected, cognito-idp.admin-add-user-to-group-username-with-whitespace-rejected, cognito-idp.admin-create-user-temporary-password-longer-than-256-characters-rejected, cognito-idp.admin-initiate-auth-password-over-131072-chars-rejected, cognito-idp.admin-initiate-auth-session-outside-length-bounds-rejected, cognito-idp.admin-respond-to-auth-challenge-oversized-challenge-response-rejected, cognito-idp.admin-set-user-password-leading-space-rejected, cognito-idp.admin-set-user-password-longer-than-256-characters-rejected, cognito-idp.associate-software-token-malformed-access-token-rejected, cognito-idp.change-password-previous-password-over-maximum-rejected, cognito-idp.change-password-proposed-over-256-chars-rejected, cognito-idp.delete-user-malformed-access-token-keeps-user, cognito-idp.get-tokens-from-refresh-token-malformed-refresh-token-rejected, cognito-idp.global-sign-out-access-token-outside-pattern-rejected, cognito-idp.password-of-256-characters-signs-in-and-257-rejected, cognito-idp.password-of-257-characters-rejected-and-256-signs-in, cognito-idp.revoke-token-outside-token-alphabet-rejected, cognito-idp.username-of-128-characters-signs-in-and-129-rejected, cognito-idp.verify-software-token-{five-digit,seven-digit,non-numeric-user-code,session-non-digit-code}-*, cognito-idp.initiate-auth-encoded-data-over-131072-chars-rejected, cognito-idp.admin-confirm-sign-up-client-metadata-value-over-limit-rejected.

Cause 2 of 10: the password pattern is applied to inner whitespace.
AWS: a password containing an inner space or tab is accepted (AdminSetUserPassword/ChangePassword store it, it signs in, and a space counts toward the symbol requirement); only leading or trailing whitespace is refused, with the Cause-1 message naming the member and pattern [\S]+. A password with no symbol is refused InvalidPasswordException "Password does not conform to policy: Password must have symbol characters".
Emulator: refuses any password containing whitespace anywhere (InvalidParameterException, pattern [\S]+), so inner-space/inner-tab passwords that AWS accepts cannot be set and the positive leg of each check never runs.
Why: real user passwords with spaces are rejected, and the policy-symbol rule is unreachable.
Fix: the password validation in the AdminSetUserPassword/ChangePassword/SignUp handlers must reject only leading/trailing whitespace, then apply the password policy.
Traces: cognito-idp.password-inner-space-counts-as-a-symbol-and-signs-in, cognito-idp.change-password-trailing-tab-rejected-inner-tab-accepted, cognito-idp.admin-set-user-password-edge-whitespace-rejected, cognito-idp.change-password-proposed-password-with-a-leading-space-rejected, cognito-idp.change-password-previous-password-with-whitespace-rejected.

Cause 3 of 10: attribute-name list constraints are reported per member and with the wrong pattern.
AWS: for a list of attribute names it reports the whole list once and aggregates the member constraints: "1 validation error detected: Value '[<members, comma-joined>]' at 'userAttributeNames' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 32, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\p{L}\p{M}\p{S}\p{N}\p{P}]+]" (same shape for 'attributesToGet' and for AdminCreateUser's 'userAttributes.N.member.name'). A 32-character unknown name instead gives "Invalid user attributes: Type for attribute {<name>} could not be determined".
Emulator: reports a per-member path ('userAttributeNames.1.member', 'attributesToGet.1.member'), a single constraint, and the pattern [\p{L}\p{M}\p{S}\p{N}\p{P}\t\n\r ]+ — so tabs/newlines/spaces pass where AWS refuses them.
Why: callers that match on the attribute-name refusal see a different member path and a different pattern, and names containing control/whitespace characters are accepted.
Fix: the list-member validation path for attribute-name lists (aggregate rendering + the no-whitespace pattern).
Traces: cognito-idp.admin-create-user-attribute-name-outside-pattern-rejected, cognito-idp.admin-delete-user-attributes-control-character-name-rejected, cognito-idp.admin-delete-user-attributes-name-over-32-chars-rejected, cognito-idp.list-users-attributes-to-get-control-character-rejected, cognito-idp.list-users-overlong-attribute-name-rejected.

Cause 4 of 10: non-sensitive values are dropped, collections are rendered as prose, and multiple errors come out in the wrong order.
AWS: for a non-sensitive member the value IS part of the message, including the empty string: "2 validation errors detected: Value '' at 'deviceKey' failed to satisfy constraint: Member must have length greater than or equal to 1; Value '' at 'deviceKey' failed to satisfy constraint: Member must satisfy regular expression pattern: [\w-]+_[0-9a-f-]+". For paginationToken AWS orders the pattern error before the length error. Collection bounds are reported against the collection with its members listed, never as prose.
Emulator: omits the value ("Value at 'deviceKey'", "Value at 'poolName'", "Value at 'paginationToken'", "Value at 'nextToken'", "Value at 'writeAttributes.1.member'", "Value at 'scopes.1.member.scopeName'"), emits the two errors in the opposite order for paginationToken, and invents prose values such as "Value '0 entries' at 'links'" and "Value '1000001 bytes' at 'assets.1.member.bytes'".
Why: every client assertion on these refusals fails, and the prose forms are not valid AWS messages at all.
Fix: the same validation message renderer: keep the value for non-sensitive members, render collection/blob bounds against the collection, and emit errors in AWS's order.
Traces: cognito-idp.admin-list-devices-empty-pagination-token-rejected, cognito-idp.admin-get-device-key-over-55-chars-rejected, cognito-idp.list-resource-servers-empty-next-token-rejected, cognito-idp.create-resource-server-scope-name-length-bound-rejected, cognito-idp.create-user-pool-client-empty-write-attribute-rejected, cognito-idp.create-user-pool-name-length-bounds-rejected, cognito-idp.create-terms-empty-links-map-rejected, cognito-idp.managed-login-branding-asset-bytes-over-one-million-rejected, cognito-idp.app-client-fifty-first-allowed-scope-rejected, cognito-idp.create-user-pool-client-allowed-oauth-scopes-count-bound-rejected.

Cause 5 of 10: tag validation is missing and tag refusals are worded wrongly.
AWS: TagResource enforces the tag-key bound (max 128 characters, Smithy TagKeysType member) and refuses an over-long key with a 400 InvalidParameterException validation message; tag-value refusals name the map entry and are worded as AWS's validation message (ListTagsForResource's over-long ResourceArn refusal is 112 characters SHORTER than the emulator's while sharing the same head "1 validation error detected: Value 'arn:aws:cognito-idp:...").
Emulator: TagResource accepts a 129-character tag key with HTTP 200; the value refusals carry a message that differs from AWS's (extra text, echoed values) for TagResource, UntagResource, CreateUserPool UserPoolTags and ListTagsForResource.
Why: a template or client that relies on tag limits being enforced sees state AWS would never hold, and error matching fails.
Fix: tag validation in the TagResource/UntagResource/CreateUserPool handlers plus the validation message renderer for map members.
Traces: cognito-idp.tag-resource-tag-key-over-128-characters-rejected, cognito-idp.tag-resource-value-over-256-chars-rejected, cognito-idp.untag-resource-tag-key-length-bound-rejected, cognito-idp.create-user-pool-tag-value-past-256-chars-rejected, cognito-idp.list-tags-for-resource-overlong-arn-rejected.

Cause 6 of 10: pagination tokens are judged before their declared bounds, and the opaque-token message is wrong.
AWS: NextToken/PaginationKey carry modelled bounds (PaginationKey length 1..131072, pattern [\S]+). A token outside the bound is refused as a validation error naming the member; a token inside the bound but unparseable is refused InvalidParameterException "Invalid pagination token provided". AdminListUserAuthEvents treats an unparseable NextToken by restarting at the newest page rather than refusing.
Emulator: answers "Invalid pagination token given." for every bad token — over-long tokens never meet the length constraint, the opaque-token wording differs from AWS's, and AdminListUserAuthEvents refuses instead of restarting.
Why: paginating clients get a refusal where AWS pages, and token-bound errors are unmatchable.
Fix: validate the pagination members against their modelled bounds before decoding, correct the opaque-token message, and make AdminListUserAuthEvents restart.
Traces: cognito-idp.admin-list-groups-for-user-next-token-longer-than-65535-characters-rejected, cognito-idp.admin-list-groups-for-user-next-token-over-65535-chars-rejected, cognito-idp.list-terms-next-token-over-131072-chars-rejected, cognito-idp.admin-list-user-auth-events-unparseable-next-token-restarts-at-newest.

Cause 7 of 10: CreateTerms Links validation is hand-written instead of modelled.
AWS: refuses an unlisted language key with the map-key constraint message "1 validation error detected: Value '{<key>=<value>}' at 'links' failed to satisfy constraint: Map keys must satisfy constraint: [Member must satisfy regular expression pattern: ^cognito:(default|english|french|spanish|german|bahasa-indonesia|italian|japanese|korean|portuguese-brazil|chinese-(simplified|traditional)|dutch)$]", and applies the modelled value pattern to the link value.
Emulator: answers a bespoke sentence ("Links carries a language managed login does not localize: cognito:flemish.") and applies a value pattern that differs from AWS's for a link containing a space.
Why: clients matching the documented validation error see unrecognised text.
Fix: CreateTerms request validation (map key/value constraints and their rendering).
Traces: cognito-idp.create-terms-unknown-language-link-key-rejected, cognito-idp.create-terms-link-with-whitespace-rejected.

Cause 8 of 10: GetClientToken scope validation.
AWS: applies the modelled scope pattern (a scope containing a space is refused with AWS's pattern, which differs from the emulator's [\x21\x23-\x5B\x5D-\x7E]+ rendering) and counts DISTINCT scopes against the 50-scope bound, so a 51-entry list that repeats a scope is accepted.
Emulator: refuses the 51-entry list outright with a list validation error that interpolates every scope, and the malformed-scope message differs from AWS's.
Why: a legitimate token request with a repeated scope is refused.
Fix: GetClientToken scope validation (dedupe before the bound) and its message.
Traces: cognito-idp.get-client-token-fifty-first-scope-rejected-unless-repeated, cognito-idp.get-client-token-malformed-scope-rejected.

Cause 9 of 10: UserContextData is interpreted before it is validated.
AWS: an over-long UserContextData.IpAddress (131073 characters) is refused with the length validation error naming 'userContextData.ipAddress'.
Emulator: answers InvalidParameterException "Cannot use IP Address passed into UserContextData", a semantic refusal raised before the member bound is checked.
Why: the caller is told the field is unusable rather than that it is too long.
Fix: run InitiateAuth input validation before the UserContextData semantic checks.
Traces: cognito-idp.initiate-auth-context-ip-address-over-131072-chars-rejected.

Cause 10 of 10: app-client secret validation.
AWS: AddUserPoolClientSecret refuses a 23-character secret with InvalidParameterException "Client secret must be at least 24 characters long." and ACCEPTS a secret containing hyphens (which then signs ADMIN_USER_PASSWORD_AUTH); an out-of-bounds secret on GetClientToken is refused as a validation error naming 'clientSecret'.
Emulator: answers the generic Smithy length message for the short secret, refuses the hyphenated secret against pattern [\w+]+, and answers NotAuthorizedException "Invalid client or secret" for an out-of-bounds GetClientToken secret.
Why: callers cannot supply the secrets AWS accepts, and the refusal class/message for a malformed secret is wrong (authentication failure vs. validation failure).
Fix: AddUserPoolClientSecret / GetClientToken secret validation and messages.
Traces: cognito-idp.add-user-pool-client-secret-custom-value-outside-length-rejected, cognito-idp.supplied-client-secret-with-hyphens-signs-admin-sign-in, cognito-idp.get-client-token-secret-outside-length-bounds-rejected.

## Evidence
Cause 1 — AdminSetUserPassword, leading-space password. Expected ('InvalidParameterException', "1 validation error detected: Value at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\S]+"); got "... Value ' riverstone' at 'password' ...". Same divergence for 'username', 'accessToken', 'refreshToken', 'session', 'userCode', 'previousPassword', 'proposedPassword', 'temporaryPassword', 'userContextData.encodedData'.
Cause 2 — AdminSetUserPassword with 'orchard lane' (inner space): AWS stores it and it signs in; emulator raised InvalidParameterException "... Value 'orchard lane' at 'password' failed to satisfy constraint: Member must satisfy regular expression pattern: [\\S]+". ChangePassword with an inner tab is accepted by AWS, refused by the emulator; a trailing tab must be refused with the value-less message.
Cause 3 — AdminDeleteUserAttributes(['custom:ti\x01er']) expected "1 validation error detected: Value '[custom:ti\x01er]' at 'userAttributeNames' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 32, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [\\p{L}\\p{M}\\p{S}\\p{N}\\p{P}]+]"; got "... Value 'custom:ti\x01er' at 'userAttributeNames.1.member' ... pattern: [\\p{L}\\p{M}\\p{S}\\p{N}\\p{P}\\t\\n\\r ]+".
Cause 4 — AdminGetDevice(DeviceKey="") expected "2 validation errors detected: Value '' at 'deviceKey' ... greater than or equal to 1; Value '' at 'deviceKey' ... pattern: [\\w-]+_[0-9a-f-]+"; got the same two errors with the value dropped. AdminListDevices(PaginationToken="") expected the pattern error first, then the length error, both carrying "Value ''"; got length first with no value. CreateTerms(Links={}) got "Value '0 entries' at 'links'"; CreateManagedLoginBranding got "Value '1000001 bytes' at 'assets.1.member.bytes'".
Cause 5 — TagResource with a 129-character key answered HTTP 200 {} instead of a refusal. ListTagsForResource on a 2049-character ARN answered a 2184-character message where AWS's is 2072 characters.
Cause 6 — AdminListGroupsForUser with a 65536-character NextToken answered ('InvalidParameterException','Invalid pagination token given.') instead of the length validation error; ListTerms expected 'Invalid pagination token provided'; AdminListUserAuthEvents refused an unparseable token instead of restarting at the newest event.
Cause 7 — CreateTerms with key 'cognito:flemish' answered 'Links carries a language managed login does not localize: cognito:flemish.' instead of the map-keys constraint message.
Cause 8 — GetClientToken with 51 scopes (one repeated) raised InvalidParameterException listing all scopes; AWS accepts it.
Cause 9 — InitiateAuth with a 131073-character UserContextData.IpAddress raised 'Cannot use IP Address passed into UserContextData'.
Cause 10 — AddUserPoolClientSecret('a610f187fee4a7add437891') expected 'Client secret must be at least 24 characters long.', got "1 validation error detected: Value 'a610f187fee4a7add437891' at 'clientSecret' failed to satisfy constraint: Member must have length greater than or equal to 24"; AddUserPoolClientSecret('Kq7v-Rm2x-Tz9p-Lw4n-Bc8d-Hf3j') was refused against pattern [\\w+]+ though AWS accepts it.

## Steps to Reproduce
1. Replay the check ids listed in trace_ids against the emulator (each creates its own user pool/app client).
2. Cause 1: call AdminSetUserPassword with Password=' riverstone' and compare the Error.Message with AWS's value-less form.
3. Cause 2: AdminSetUserPassword with Password='Zt6!mRq2 Hw', Permanent=true, then InitiateAuth USER_PASSWORD_AUTH with the same password — AWS issues tokens.
4. Cause 3: AdminDeleteUserAttributes with UserAttributeNames=['custom:ti\x01er'].
5. Cause 4: AdminGetDevice with DeviceKey='' and AdminListDevices with PaginationToken=''.
6. Cause 5: TagResource on a user-pool ARN with a 129-character tag key.
7. Cause 6: AdminListGroupsForUser with a 65536-character NextToken; AdminListUserAuthEvents with a garbage NextToken.
8. Cause 7: CreateTerms with Links={'cognito:flemish': 'https://...'}.
9. Cause 8: GetClientToken with 51 scope entries where one repeats.
10. Cause 9: InitiateAuth with UserContextData.IpAddress of 131073 characters.
11. Cause 10: AddUserPoolClientSecret with a 23-character secret, then with 'Kq7v-Rm2x-Tz9p-Lw4n-Bc8d-Hf3j'.
