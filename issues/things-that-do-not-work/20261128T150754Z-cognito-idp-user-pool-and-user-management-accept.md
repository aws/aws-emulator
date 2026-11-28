Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cognito-idp.admin-delete-required-attribute-rejected cognito-idp.admin-delete-user-attributes-required-attribute-rejected cognito-idp.developer-only-attribute-refuses-a-self-service-update cognito-idp.update-user-pool-omitting-policies-keeps-the-enforced-password-policy cognito-idp.create-user-pool-schema-min-length-past-131072-chars-rejected cognito-idp.managed-login-branding-asset-extension-mismatching-image-rejected cognito-idp.managed-login-branding-asset-extension-not-allowed-for-category-rejected cognito-idp.managed-login-branding-invalid-settings-value-rejected cognito-idp.ui-customization-image-file-over-100-kib-rejected cognito-idp.sign-up-rejects-auto-verify-email-for-a-user-without-an-email cognito-idp.verify-user-attribute-moves-email-alias-from-its-holder cognito-idp.verify-user-attribute-email-alias-held-by-another-user-rejected cognito-idp.verify-user-attribute-invalid-parameter-rejected cognito-idp.admin-initiate-auth-deleted-client-secret-hash-rejected-while-the-added-secret-signs-in
Issue-Shape-Id: com.amazonaws.cognitoidentityprovider#AWSCognitoIdentityProviderService
Issue-Key: cognito-idp.admin-delete-required-attribute-rejected|run#9|AssertionError|assert got == refused, f"deleting required attribute name: expected {refused!r}, got {got!r}"
Issue-Key: cognito-idp.admin-delete-user-attributes-required-attribute-rejected|run#9|AssertionError|assert got == ("InvalidParameterException", (
Issue-Key: cognito-idp.developer-only-attribute-refuses-a-self-service-update|run#5|AssertionError|assert "dev:custom:plan" in names, (
Issue-Key: cognito-idp.update-user-pool-omitting-policies-keeps-the-enforced-password-policy|run#20|AssertionError|assert seen == custom, f"after an update omitting Policies the policy reads {seen!r}, expected {custom!r}"
Issue-Key: cognito-idp.create-user-pool-schema-min-length-past-131072-chars-rejected|run#10|AssertionError|assert limits == {"MinLength": "1", "MaxLength": "64"}, (
Issue-Key: cognito-idp.managed-login-branding-asset-extension-mismatching-image-rejected|_refusal#8|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cognito-idp.managed-login-branding-asset-extension-not-allowed-for-category-rejected|_refusal#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cognito-idp.managed-login-branding-invalid-settings-value-rejected|_refusal#7|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cognito-idp.ui-customization-image-file-over-100-kib-rejected|_refusal#16|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: cognito-idp.sign-up-rejects-auto-verify-email-for-a-user-without-an-email|_refusal#26|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: cognito-identity-provider|VerifyUserAttribute|AliasExistsException
Issue-Key: cognito-idp.verify-user-attribute-email-alias-held-by-another-user-rejected|run#50|AssertionError|assert got == ("AliasExistsException", "User account already exists for the alias."), (
Issue-Key: cognito-idp.verify-user-attribute-invalid-parameter-rejected|run#14|AssertionError|assert got == (
Issue-Key: cognito-idp.admin-initiate-auth-deleted-client-secret-hash-rejected-while-the-added-secret-signs-in|run#27|AssertionError|assert "ClientSecret" not in described, (
Issue-Commit: 7eb46ae4754f02cb00b49bce755f449d67bad34e

# cognito-idp: user-pool and user management accept or refuse the wrong things (10 causes: required attributes, dev: attributes, policy update, schema, branding, UI customization, sign-up, aliases, client secret)

> _Note: this issue was identified against the code base at commit `7eb46ae4754f02cb00b49bce755f449d67bad34e`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
AdminDeleteUserAttributes

## Description
Ten defects in user-pool/user management.

Cause 1 of 10: the required-attribute deletion refusal is worded in emulator terms.
AWS: deleting a required attribute is refused InvalidParameterException 'Invalid user attributes: <oidc path>: The attribute <oidc path> is required' — e.g. 'Invalid user attributes: name.formatted: The attribute name.formatted is required' for 'name' and 'Invalid user attributes: name.givenName: The attribute name.givenName is required' for 'given_name'. The fixed text is "Invalid user attributes: <path>: The attribute <path> is required"; the path comes from the attribute named in the request.
Emulator: answers 'user.<attr>: Required attribute cannot be deleted.\n' (with a trailing newline).
Why: clients matching the documented message, and humans reading it, see text AWS never sends.
Fix: the AdminDeleteUserAttributes/DeleteUserAttributes required-attribute check's message (and the attribute-path mapping).
Traces: cognito-idp.admin-delete-required-attribute-rejected, cognito-idp.admin-delete-user-attributes-required-attribute-rejected.

Cause 2 of 10: developer-only attributes are named without the custom: segment.
AWS: a developer-only custom attribute appears in the pool schema as 'dev:custom:<name>' and refuses a self-service update.
Emulator: the schema carries 'dev:plan' instead of 'dev:custom:plan'.
Why: clients reading or writing the attribute by its documented name cannot find it.
Fix: the schema naming for DeveloperOnlyAttribute in CreateUserPool/DescribeUserPool.
Traces: cognito-idp.developer-only-attribute-refuses-a-self-service-update.

Cause 3 of 10: UpdateUserPool without Policies resets the password policy.
AWS: an UpdateUserPool that omits Policies keeps the policy already in force (MinimumLength 12, RequireSymbols false in the recording).
Emulator: resets it to the service defaults (MinimumLength 8 with all Require* true).
Why: any partial pool update silently loosens or tightens password rules for every user.
Fix: UpdateUserPool must treat an omitted Policies as 'unchanged' in the pool state write.
Traces: cognito-idp.update-user-pool-omitting-policies-keeps-the-enforced-password-policy.

Cause 4 of 10: a zero-padded schema constraint is stored verbatim.
AWS: refuses a StringAttributeConstraints.MinLength whose zero-padded value exceeds the bound instead of storing it.
Emulator: stores the padded string as given ('00000...').
Why: DescribeUserPool reports a constraint no AWS pool can hold, and the bound is never enforced.
Fix: numeric parsing/validation of schema string constraints in CreateUserPool.
Traces: cognito-idp.create-user-pool-schema-min-length-past-131072-chars-rejected.

Cause 5 of 10: managed login branding accepts invalid assets and settings.
AWS: CreateManagedLoginBranding refuses an asset whose Extension does not match the image bytes, an extension not allowed for the asset Category (PNG for FAVICON_SVG), and an unknown Settings value (recorded behavior only: all three recordings show a ClientError).
Emulator: answers HTTP 200 and stores the branding.
Why: branding state the console would reject is accepted, and callers never see the validation errors.
Fix: asset/settings validation in the CreateManagedLoginBranding handler.
Traces: cognito-idp.managed-login-branding-asset-extension-mismatching-image-rejected, cognito-idp.managed-login-branding-asset-extension-not-allowed-for-category-rejected, cognito-idp.managed-login-branding-invalid-settings-value-rejected.

Cause 6 of 10: SetUICustomization accepts an image above the size limit.
AWS: refuses an ImageFile larger than 100 KiB (recorded behavior only: the recording shows a ClientError).
Emulator: answers HTTP 200 with a UICustomization carrying the stored ImageUrl.
Why: the documented 100 KiB limit does not exist.
Fix: ImageFile size validation in the SetUICustomization handler.
Traces: cognito-idp.ui-customization-image-file-over-100-kib-rejected.

Cause 7 of 10: SignUp accepts auto-verified email for a user without an email.
AWS: refuses the SignUp when email auto-verification is required but the request carries no email attribute (recorded behavior only: the recording shows a ClientError).
Emulator: answers HTTP 200 with UserConfirmed=true and a UserSub.
Why: users are created confirmed with no verifiable contact, which AWS prevents.
Fix: the SignUp required/auto-verified attribute validation.
Traces: cognito-idp.sign-up-rejects-auto-verify-email-for-a-user-without-an-email.

Cause 8 of 10: email-alias ownership on VerifyUserAttribute is judged wrongly.
AWS: verifying an address that another user holds UNVERIFIED moves the alias to the verifying user; it is refused only when another user holds it VERIFIED, and that refusal's message is not the generic AliasExists text used here.
Emulator: answers AliasExistsException 'An account with the given email already exists.' in both cases, so the move never happens.
Why: the documented alias-transfer behaviour is unavailable and the refusal message diverges.
Fix: alias ownership resolution in the VerifyUserAttribute handler.
Traces: cognito-idp.verify-user-attribute-moves-email-alias-from-its-holder, cognito-idp.verify-user-attribute-email-alias-held-by-another-user-rejected.

Cause 9 of 10: verifying an attribute the user does not hold reports a code error.
AWS: refuses InvalidParameterException (the attribute cannot be verified) rather than reporting the code.
Emulator: answers ExpiredCodeException 'Invalid code provided, please request a code again.'
Why: the caller retries code delivery for a request that can never succeed.
Fix: attribute presence validation before code comparison in VerifyUserAttribute.
Traces: cognito-idp.verify-user-attribute-invalid-parameter-rejected.

Cause 10 of 10: a deleted client secret stays readable and usable.
AWS: after DeleteUserPoolClientSecret the client no longer reports that ClientSecret; a SECRET_HASH signed with the deleted secret is refused while the newly added secret signs in.
Emulator: DescribeUserPoolClient still returns a ClientSecret, and it is not the newly added one.
Why: a rotated-away secret keeps working, defeating secret rotation.
Fix: the app-client secret state and the DescribeUserPoolClient serializer.
Traces: cognito-idp.admin-initiate-auth-deleted-client-secret-hash-rejected-while-the-added-secret-signs-in.

## Evidence
Cause 1 — "deleting required attribute name: expected ('InvalidParameterException', 'Invalid user attributes: name.formatted: The attribute name.formatted is required'), got ('InvalidParameterException', 'user.name: Required attribute cannot be deleted.\\n')"; given_name variant expects 'Invalid user attributes: name.givenName: The attribute name.givenName is required'.
Cause 2 — "the developer-only attribute is not in the schema as dev:custom:plan: ['sub', 'dev:plan', 'name', ...]".
Cause 3 — "after an update omitting Policies the policy reads {'MinimumLength': 8, 'RequireUppercase': True, 'RequireLowercase': True, 'RequireNumbers': True, 'RequireSymbols': True}, expected {'MinimumLength': 12, ..., 'RequireSymbols': False}".
Cause 4 — "the zero-padded MinLength was stored as {'MinLength': '000000...'}".
Cause 5 — "expected the call to be refused, but it succeeded: {'ManagedLoginBranding': {... 'Assets': [{'Category': 'PAGE_FOOTER_LOGO', 'Extension': 'PNG', 'Bytes': b'<svg ...'}] ...}}"; FAVICON_SVG with Extension 'PNG' and Settings {'spacingDensity': 'ROOMY'} also returned 200.
Cause 6 — "expected a refusal, but the call succeeded: {'UICustomization': {... 'ImageUrl': '.../assets/login_logo.png' ...}}".
Cause 7 — "expected the call to be refused, but it succeeded: {'UserConfirmed': True, 'UserSub': '72f6ae52-...'}".
Cause 8 — "verifying an address another user holds as a verified alias gave ('AliasExistsException', 'An account with the given email already exists.')"; the alias-move leg raised botocore.errorfactory.AliasExistsException on VerifyUserAttribute.
Cause 9 — "verifying a phone number the user lacks answered ('ExpiredCodeException', 'Invalid code provided, please request a code again.')".
Cause 10 — "DescribeUserPoolClient still returns a ClientSecret after the generated secret was deleted; it equals the deleted one: False, the added one: True".

## Steps to Reproduce
1. Replay the check ids in trace_ids.
2. Cause 1: AdminCreateUser with name/given_name required, then AdminDeleteUserAttributes(['name']) and (['given_name']).
3. Cause 2: CreateUserPool with Schema=[{Name:'plan', DeveloperOnlyAttribute:true}] and read DescribeUserPool's schema.
4. Cause 3: CreateUserPool with Policies.PasswordPolicy MinimumLength 12 / RequireSymbols false, then UpdateUserPool without Policies and DescribeUserPool.
5. Cause 5/6: CreateManagedLoginBranding with mismatched asset bytes/extension and with an unknown Settings value; SetUICustomization with a >100 KiB ImageFile.
6. Cause 7: pool with AutoVerifiedAttributes=['email'] and required email, SignUp without an email attribute.
7. Cause 8/9: two users in an email-alias pool, VerifyUserAttribute the shared address (unverified and verified holder); VerifyUserAttribute phone_number for a user without one.
8. Cause 10: CreateUserPoolClient with GenerateSecret, AddUserPoolClientSecret, DeleteUserPoolClientSecret, then DescribeUserPoolClient and AdminInitiateAuth with each SECRET_HASH.
