Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: secretsmanager.batch-get-secret-value-eleven-filters-rejected secretsmanager.batch-get-secret-value-filter-with-eleven-values-rejected secretsmanager.batch-get-secret-value-filter-value-outside-pattern-rejected secretsmanager.batch-get-secret-value-next-token-past-4096-characters-rejected-as-invalid-token secretsmanager.batch-get-secret-value-secret-id-longer-than-2048-rejected secretsmanager.untag-resource-tag-key-longer-than-128-rejected secretsmanager.put-secret-value-malformed-rotation-token-rejected-and-value-unchanged secretsmanager.list-secrets-uppercase-sort-order-rejected secretsmanager.delete-resource-policy-secret-id-of-2048-characters-rejected
Issue-Shape-Id: com.amazonaws.secretsmanager#secretsmanager
Issue-Key: secretsmanager.batch-get-secret-value-eleven-filters-rejected|run#18|AssertionError|assert got == expected, f"eleven filters must be refused as {expected!r}, got {got!r}"
Issue-Key: secretsmanager.batch-get-secret-value-filter-with-eleven-values-rejected|run#14|AssertionError|assert got == expected, f"a filter with eleven values must be refused as {expected!r}, got {got!r}"
Issue-Key: secretsmanager.batch-get-secret-value-filter-value-outside-pattern-rejected|run#13|AssertionError|assert (code, message) == expected, f"a filter value with '#' gave {(code, message)!r}"
Issue-Key: secretsmanager.batch-get-secret-value-next-token-past-4096-characters-rejected-as-invalid-token|run#8|AssertionError|assert got == _STALE, f"a {length}-character NextToken gave {got!r}, expected {_STALE!r}"
Issue-Key: secretsmanager.batch-get-secret-value-secret-id-longer-than-2048-rejected|run#11|AssertionError|assert errors == [(True, "ValidationException", invalid_name)], (
Issue-Key: secretsmanager.untag-resource-tag-key-longer-than-128-rejected|run#12|AssertionError|assert got == expected, f"a 129-character tag key must be refused as {expected!r}, got {got!r}"
Issue-Key: secretsmanager.put-secret-value-malformed-rotation-token-rejected-and-value-unchanged|run#13|AssertionError|assert msg == want, f"pattern refusal message was {msg!r}, expected {want!r}"
Issue-Key: secretsmanager.list-secrets-uppercase-sort-order-rejected|run#12|AssertionError|assert got == want, f"SortOrder 'ASC' gave {got!r}, expected {want!r}"
Issue-Key: secretsmanager.delete-resource-policy-secret-id-of-2048-characters-rejected|run#10|AssertionError|assert got == oversize, f"a {length}-character SecretId gave {got!r}, expected {oversize!r}"
Issue-Commit: c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6

# secretsmanager: 9 net-new causes — BatchGetSecretValue refusals take the wrong error class, and validation-message rendering drops the value segment, the 64-character elision, the regex anchors and the enum order

> _Note: this issue was identified against the code base at commit `c1d149b2bb5e60d0ae77bd59c6a4638e6165c0e6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
BatchGetSecretValue

## Description
9 net-new causes in Secrets Manager's refusal path, all reached on operations the emulator DOES serve. Causes 1-5 are BatchGetSecretValue: the handler hands every malformed input to the generic Smithy-constraint validator (or to the store lookup) instead of applying Secrets Manager's own pre-lookup checks, so the HTTP status is right but the error CODE — the thing a client branches on — is wrong. Causes 6-9 are the shared "1 validation error detected: ..." renderer and the enum clause it builds. A fixer must close all nine before submitting.

Related open issues that do NOT carry these checks' keys and do NOT cover them: 20261127T124058Z-secretsmanager-the-schema-validation-message-renderer-drops-the.md (same renderer, but its causes are ListSecrets.Filters, UntagResource.TagKeys, RemoveRegionsFromReplication, PutSecretValue.RotationToken and CreateSecret.KmsKeyId on the WIRE corpus), 20261127T124058Z-secretsmanager-secret-identifier-and-filter-validation-runs-after.md (SecretId grammar and Filter.Key enum, not Values count / value grammar / NextToken / the 2048 boundary) and 20261123T140313Z-secretsmanager-listsecrets-validationexception-for-an-unknown-sortby-lists.md (SortBy, not SortOrder). Causes 6, 7 and 9 below overlap those issues' subject matter on NEW members/boundaries and are filed here because the behavioural checks still fail.

The renderer template AWS emits, for reference in causes 1, 6, 7, 8:
  1 validation error detected: Value '<rendered value>' at '<memberPath>' failed to satisfy constraint: <constraint clause>
Fixed text: "1 validation error detected: Value '", "' at '", "' failed to satisfy constraint: " and the clause wording. Variable parts: the value from the request (rendered Java-toString style for collections, cut to its first 64 characters and followed by a space and "..." when longer), the member path from the model, and the limit/pattern/enum set from the model's constraint trait. No part of it comes from one run's values.

Cause 1 of 9: BatchGetSecretValue — the over-length Filters refusal omits the "Value '<...>' at '" segment.
AWS: Smithy BatchGetSecretValue.Filters -> FiltersListType @length {min:0,max:10}. Eleven filters are refused 400 ValidationException with message "1 validation error detected: Value '<first 64 characters of the rendered filter list> ...' at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10". The rendered list is Java toString form, "[Filter(key=<k>, values=[<v>, ...]), ...]".
Emulator: "1 validation error detected: Value at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10" — the whole value segment is absent.
Why: a client reading the 400 cannot see WHICH filters were rejected, and message assertions/log scrapers keyed on AWS's template never match.
Fix: the shared validation-message renderer used by the Secrets Manager operation validators — render the offending value for list/structure members, not only for scalars.
Traces: secretsmanager.batch-get-secret-value-eleven-filters-rejected.

Cause 2 of 9: BatchGetSecretValue — a single Filter carrying eleven Values is refused with the wrong error class and message.
AWS: Smithy Filter.Values -> FilterValuesStringList @length {min:1,max:10}. Secrets Manager refuses the over-count with its OWN error, not the schema message: 400, error code InvalidParameterException, message "The filter list has too many filters. You can include up to 10 filters." — entirely fixed text, no variable part. (Recorded behaviour: the model only states the @length bound; the code and wording come from the recording.)
Emulator: 400 ValidationException, message "1 validation error detected: Value '<rendered values> ...' at 'filters.1.member.values' failed to satisfy constraint: Member must have length less than or equal to 10".
Why: a client catching InvalidParameterException to trim its filter list never matches, and the reported member path points at a schema path AWS never emits for this case.
Fix: BatchGetSecretValue's filter validation — check the per-filter Values count ahead of generic schema validation and raise InvalidParameterException.
Traces: secretsmanager.batch-get-secret-value-filter-with-eleven-values-rejected.

Cause 3 of 9: BatchGetSecretValue — a filter value outside the filter grammar is refused with the wrong error class and message.
AWS: Smithy FilterValueStringType @length {0,512} @pattern "^\\!?[a-zA-Z0-9 :_@\\/\\+\\=\\.\\-\\!]*$". A value containing a character outside that set (e.g. '#') is refused 400 with error code InvalidParameterException and message "Invalid filter values format: <the rejected value>" — fixed text "Invalid filter values format: " plus the value copied verbatim from the request.
Emulator: 400 ValidationException, message "1 validation error detected: Value '[<value>]' at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 512, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ^\\!?[a-zA-Z0-9 :_@\\/\\+\\=\\.\\-\\!]*$]".
Why: same inverted branching as cause 2; a client is also told nothing about which value is at fault in AWS's own vocabulary.
Fix: BatchGetSecretValue's filter validation — grammar-check each filter value pre-lookup and raise InvalidParameterException with that message.
Traces: secretsmanager.batch-get-secret-value-filter-value-outside-pattern-rejected.

Cause 4 of 9: BatchGetSecretValue — a NextToken past the modelled ceiling is refused as a schema violation instead of an invalid pagination token.
AWS: Smithy BatchGetSecretValue.NextToken -> NextTokenType @length {min:1,max:4096}; BatchGetSecretValue models InvalidNextTokenException (400). A token of 4097 characters is refused 400 with error code InvalidNextTokenException and message "The pagination token is no longer valid." — fixed text, no variable part.
Emulator: 400 ValidationException, message "1 validation error detected: Value '<first 64 characters of the token> ...' at 'nextToken' failed to satisfy constraint: Member must have length less than or equal to 4096".
Why: an SDK paginator and most clients treat InvalidNextTokenException as "restart the scan from the beginning"; a ValidationException is a permanent client bug, so the emulator sends a paginating client down the wrong recovery path.
Fix: BatchGetSecretValue's pagination-token validation — classify any unusable NextToken (including an over-length one) as InvalidNextTokenException before generic schema validation.
Traces: secretsmanager.batch-get-secret-value-next-token-past-4096-characters-rejected-as-invalid-token.

Cause 5 of 9: BatchGetSecretValue — a SecretIdList entry at the 2048-character bound is classified as not-found instead of an invalid name.
AWS: Smithy BatchGetSecretValue.SecretIdList -> SecretIdListType @length {1,20} of SecretIdType @length {1,2048}; output Errors -> APIErrorListType of APIErrorType {SecretId, ErrorCode, Message}. An entry that is 2048 characters long is within the member bound, so the batch answers 200 and serves the other, existing secret; the bad entry appears in Errors as {SecretId: <the id echoed from the request>, ErrorCode: "ValidationException", Message: "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!"}. The message is entirely fixed text; only SecretId varies.
Emulator: 200 with the same single Errors entry, but ErrorCode "ResourceNotFoundException" and Message "Secrets Manager can't find the specified secret."
Why: the batch caller cannot distinguish a malformed id (fix the request) from a missing secret (provision it) — opposite remediations.
Fix: BatchGetSecretValue's per-entry resolution — apply the secret-name grammar/size rule to each SecretIdList entry before the store lookup and report ValidationException for a failure.
Traces: secretsmanager.batch-get-secret-value-secret-id-longer-than-2048-rejected.

Cause 6 of 9: UntagResource — the rendered value in the refusal message is never elided.
AWS: Smithy UntagResource.TagKeys -> TagKeyListType of TagKeyType @length {min:1,max:128}. A key list containing a 129-character key is refused 400 ValidationException with "1 validation error detected: Value '<the rendered key list cut to its first 64 characters> ...' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]". The cut — first 64 characters of the rendered list, then a space, then "..." — is part of the template.
Emulator: prints the rendered list in full, unbounded: "Value '[<both keys, 128 and 129 characters, in full>]' at 'tagKeys' ..." with the same trailing clause.
Why: AWS caps the echoed value; a client that logs or compares the message instead receives an unbounded echo of its own input, so the message never matches and the error body grows with the request.
Fix: the same shared renderer — apply the 64-character cut with the trailing " ..." sentinel to the rendered value. (20261127T124058Z-secretsmanager-the-schema-validation-message-renderer-drops-the.md describes the elision for the WIRE corpus; this behavioural check still fails and must be re-replayed.)
Traces: secretsmanager.untag-resource-tag-key-longer-than-128-rejected.

Cause 7 of 9: PutSecretValue — the pattern clause strips the regex anchors.
AWS: Smithy PutSecretValue.RotationToken -> RotationTokenType @length {36,256} @pattern "^[a-zA-Z0-9\\-]+$" @sensitive. A token with a character outside that set is refused 400 ValidationException with "1 validation error detected: Value at 'rotationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\-]+$" — AWS echoes the @pattern trait VERBATIM, and correctly omits the value because the member is @sensitive (the emulator gets the omission right).
Emulator: "... Member must satisfy regular expression pattern: [a-zA-Z0-9\\-]+" — the leading ^ and trailing $ are dropped.
Why: the echoed pattern is what a client uses to repair its input, and the unanchored form describes a strictly weaker rule than the one enforced, so a client that trusts it keeps sending rejected tokens.
Fix: the pattern clause of the shared renderer must emit the @pattern trait string unmodified.
Traces: secretsmanager.put-secret-value-malformed-rotation-token-rejected-and-value-unchanged.

Cause 8 of 9: ListSecrets — the SortOrder enum clause lists the accepted values in the wrong order.
AWS: Smithy ListSecrets.SortOrder -> SortOrderType, an enum. An out-of-domain value such as "ASC" is refused 400 ValidationException with "1 validation error detected: Value '<the rejected value>' at 'sortOrder' failed to satisfy constraint: Member must satisfy enum value set: [desc, asc]" — the set order is "[desc, asc]" (recorded behaviour; the on-disk model lists the members as asc then desc, so the order AWS prints is not the declaration order and must be taken from the recording). Fixed text is the template and "Member must satisfy enum value set: "; the variable part is the rejected value from the request.
Emulator: the same message but with "[asc, desc]".
Why: the enum clause is the authoritative list a client reads to correct its input, and a message-text comparison fails on the ordering.
Fix: the enum clause of the shared renderer — emit the accepted set in AWS's order for this member, not the model's declaration order. (20261123T140313Z-secretsmanager-listsecrets-validationexception-for-an-unknown-sortby-lists.md covers SortBy; this is SortOrder.)
Traces: secretsmanager.list-secrets-uppercase-sort-order-rejected.

Cause 9 of 9: DeleteResourcePolicy — a SecretId AT 2048 characters falls through to the lookup instead of being refused as over the size limit.
AWS: Smithy DeleteResourcePolicy.SecretId -> SecretIdType @required @length {min:1,max:2048}; the operation models InvalidParameterException (400). Secrets Manager's own size check refuses a SecretId of 2048 characters, and of 2049, before any lookup: 400, error code InvalidParameterException, message "SecretId exceeds size limit." — fixed text, no variable part. A shorter over-long id (513 characters) clears the size check and is refused instead with ValidationException "Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!".
Emulator: 400 with error code ResourceNotFoundException and message "Secrets Manager can't find the specified secret." for the 2048-character id — the identifier reaches the store, misses, and is reported as a missing resource.
Why: an over-long identifier is a permanent client bug AWS reports as an invalid parameter; told the secret does not exist, a client retries or provisions instead of truncating its id. The boundary matters too: the emulator's size rule, if any, admits 2048 where AWS refuses it.
Fix: DeleteResourcePolicy's input validation — enforce the SecretId size limit pre-lookup (2048 characters is already over it) and raise InvalidParameterException. (20261127T124058Z-secretsmanager-secret-identifier-and-filter-validation-runs-after.md describes the 2049-character case from the wire corpus; this behavioural check fails at 2048 and must be re-replayed.)
Traces: secretsmanager.delete-resource-policy-secret-id-of-2048-characters-rejected.

## Evidence
Behavioural suite simparity-behavioral-aws-v1, service heading '### secretsmanager' (220/236 passed). Verdicts verbatim from the failing list:

Cause 1 — secretsmanager.batch-get-secret-value-eleven-filters-rejected [mismatch]
  expected: ('ValidationException', "1 validation error detected: Value '[Filter(key=tag-key, values=[sim2real]), Filter(key=tag-value, v ...' at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10")
  actual:   ('ValidationException', "1 validation error detected: Value at 'filters' failed to satisfy constraint: Member must have length less than or equal to 10")

Cause 2 — secretsmanager.batch-get-secret-value-filter-with-eleven-values-rejected [mismatch]
  expected: ('InvalidParameterException', 'The filter list has too many filters. You can include up to 10 filters.')
  actual:   ('ValidationException', "1 validation error detected: Value '[sim2real-smval-<id>-north, sim2real-smval-<id>- ...' at 'filters.1.member.values' failed to satisfy constraint: Member must have length less than or equal to 10")

Cause 3 — secretsmanager.batch-get-secret-value-filter-value-outside-pattern-rejected [mismatch]
  expected: ('InvalidParameterException', 'Invalid filter values format: <tag-key value with a trailing #>')
  actual:   ('ValidationException', "1 validation error detected: Value '[<same value>]' at 'filters.1.member.values' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 512, Member must have length greater than or equal to 0, Member must satisfy regular expression pattern: ^\\!?[a-zA-Z0-9 :_@\\/\\+\\=\\.\\-\\!]*$]")

Cause 4 — secretsmanager.batch-get-secret-value-next-token-past-4096-characters-rejected-as-invalid-token [mismatch]
  expected: ('InvalidNextTokenException', 'The pagination token is no longer valid.')
  actual:   ('ValidationException', "1 validation error detected: Value 'QQQQ…(64 chars) ...' at 'nextToken' failed to satisfy constraint: Member must have length less than or equal to 4096")

Cause 5 — secretsmanager.batch-get-secret-value-secret-id-longer-than-2048-rejected [mismatch]
  expected Errors == [(id matches the 2048-char entry, 'ValidationException', 'Invalid name. Must be a valid name containing alphanumeric characters, or any of the following: -/_+=.@!')]
  actual   Errors == [(id matches, 'ResourceNotFoundException', "Secrets Manager can't find the specified secret.")]

Cause 6 — secretsmanager.untag-resource-tag-key-longer-than-128-rejected [mismatch]
  expected: ('ValidationException', "1 validation error detected: Value '[team-tttt…(64 chars total) ...' at 'tagKeys' failed to satisfy constraint: Member must satisfy constraint: [Member must have length less than or equal to 128, Member must have length greater than or equal to 1]")
  actual:   the same code and trailing clause, but the value segment carries both keys in full (128- and 129-character keys, no cut, no " ...").

Cause 7 — secretsmanager.put-secret-value-malformed-rotation-token-rejected-and-value-unchanged [mismatch]
  expected: "1 validation error detected: Value at 'rotationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: ^[a-zA-Z0-9\\-]+$"
  actual:   "1 validation error detected: Value at 'rotationToken' failed to satisfy constraint: Member must satisfy regular expression pattern: [a-zA-Z0-9\\-]+"

Cause 8 — secretsmanager.list-secrets-uppercase-sort-order-rejected [mismatch]
  expected: ('ValidationException', "1 validation error detected: Value 'ASC' at 'sortOrder' failed to satisfy constraint: Member must satisfy enum value set: [desc, asc]")
  actual:   ('ValidationException', "1 validation error detected: Value 'ASC' at 'sortOrder' failed to satisfy constraint: Member must satisfy enum value set: [asc, desc]")

Cause 9 — secretsmanager.delete-resource-policy-secret-id-of-2048-characters-rejected [mismatch]
  expected: ('InvalidParameterException', 'SecretId exceeds size limit.')
  actual:   ('ResourceNotFoundException', "Secrets Manager can't find the specified secret.")

Model citations (smithy_query on services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/com-amazonaws-secretsmanager-secretsmanager.json): BatchGetSecretValue.Filters @length{0,10}, .SecretIdList SecretIdListType @length{1,20} of SecretIdType @length{1,2048}, .NextToken NextTokenType @length{1,4096}, errors include InvalidNextTokenException and InvalidParameterException at httpError 400; output Errors -> APIErrorListType of APIErrorType{SecretId,ErrorCode,Message}; FilterValuesStringList @length{1,10} of FilterValueStringType @length{0,512} @pattern ^\!?[a-zA-Z0-9 :_@\/\+\=\.\-\!]*$; UntagResource.TagKeys TagKeyListType of TagKeyType @length{1,128}; PutSecretValue.RotationToken @length{36,256} @pattern ^[a-zA-Z0-9\-]+$ @sensitive; ListSecrets.SortOrder SortOrderType enum values [asc, desc]; DeleteResourcePolicy.SecretId @required @length{1,2048} with InvalidParameterException modelled.

## Steps to Reproduce
1. git_clone the repo and boot the emulator built from that checkout.
2. Replay the simparity-behavioral-aws-v1 suite and look under the '### secretsmanager' heading.
3. Cause 1: replay secretsmanager.batch-get-secret-value-eleven-filters-rejected — BatchGetSecretValue with 11 Filters; compare the message against the "Value '<rendered list cut at 64 chars> ...' at 'filters'" template.
4. Cause 2: replay secretsmanager.batch-get-secret-value-filter-with-eleven-values-rejected — one Filter with 11 Values; expect InvalidParameterException / 'The filter list has too many filters. You can include up to 10 filters.'.
5. Cause 3: replay secretsmanager.batch-get-secret-value-filter-value-outside-pattern-rejected — a tag-key filter value ending in '#'; expect InvalidParameterException / 'Invalid filter values format: <value>'.
6. Cause 4: replay secretsmanager.batch-get-secret-value-next-token-past-4096-characters-rejected-as-invalid-token — a 4097-character NextToken; expect InvalidNextTokenException / 'The pagination token is no longer valid.'.
7. Cause 5: replay secretsmanager.batch-get-secret-value-secret-id-longer-than-2048-rejected — SecretIdList = [a 2048-character absent id, one real secret]; expect 200, the real secret served, and one Errors entry with ErrorCode ValidationException and the 'Invalid name. ...' message.
8. Cause 6: replay secretsmanager.untag-resource-tag-key-longer-than-128-rejected — UntagResource with a 128- and a 129-character key; expect the rendered list cut at 64 characters with a trailing ' ...'.
9. Cause 7: replay secretsmanager.put-secret-value-malformed-rotation-token-rejected-and-value-unchanged — a RotationToken containing a character outside [a-zA-Z0-9-]; expect the anchored pattern ^[a-zA-Z0-9\-]+$ echoed verbatim.
10. Cause 8: replay secretsmanager.list-secrets-uppercase-sort-order-rejected — ListSecrets with SortOrder='ASC'; expect the enum set printed as [desc, asc].
11. Cause 9: replay secretsmanager.delete-resource-policy-secret-id-of-2048-characters-rejected — DeleteResourcePolicy with a 2048-character SecretId (and again with 2049); expect InvalidParameterException / 'SecretId exceeds size limit.' for both, and ValidationException / 'Invalid name. ...' for a 513-character one.
