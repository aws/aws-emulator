Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: lambda.create-function-application-log-level-outside-enum-rejected lambda.create-function-system-log-level-outside-enum-rejected lambda.create-function-tracing-mode-outside-enum-rejected lambda.invoke-log-type-outside-enum-rejected lambda.layer-publish-unknown-storage-mode-rejected-mints-no-version lambda.update-function-code-unknown-storage-mode-rejected lambda.function-url-unknown-invoke-mode-rejected lambda.delete-layer-version-malformed-layer-name-rejected lambda.delete-resource-policy-off-pattern-arn-rejected lambda.function-url-numeric-qualifier-in-name-rejected lambda.create-alias-weight-key-not-a-version-number-rejected lambda.create-alias-routing-weight-version-key-over-length-rejected lambda.create-function-dead-letter-target-arn-over-length-rejected lambda.create-function-environment-key-leading-digit-rejected lambda.create-function-role-arn-past-1600-characters-rejected lambda.create-function-kms-key-arn-over-10000-chars-rejected lambda.durable-kms-key-arn-over-length-rejected lambda.publish-layer-version-license-info-over-512-rejected lambda.add-permission-function-name-over-256-chars-rejected lambda.delete-resource-policy-arn-past-256-characters-reaches-alias-lookup-rejected lambda.recursion-config-read-with-non-matching-function-name-rejected lambda.get-function-recursion-config-name-over-140-chars-rejected lambda.layer-get-by-arn-over-2048-chars-rejected lambda.publish-layer-version-s3-bucket-trailing-dot-rejected lambda.update-code-s3-object-version-length-bounds-rejected lambda.publish-layer-version-unknown-compatible-runtime-rejected lambda.function-url-cors-allow-header-with-line-break-rejected lambda.function-url-cors-allow-method-with-line-break-rejected lambda.function-url-cors-expose-header-with-line-break-rejected lambda.function-url-cors-origin-with-line-break-rejected lambda.function-url-cors-expose-header-over-1024-characters-rejected lambda.function-url-cors-max-age-above-one-day-rejected lambda.delete-layer-version-name-over-140-characters-rejected
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda.create-function-application-log-level-outside-enum-rejected|run#15|AssertionError|assert got == want, f"ApplicationLogLevel VERBOSE was answered {got!r}, expected {want!r}"
Issue-Key: lambda.create-function-system-log-level-outside-enum-rejected|run#15|AssertionError|assert got == expected, f"SystemLogLevel TRACE answered {got!r}, expected {expected!r}"
Issue-Key: lambda.create-function-tracing-mode-outside-enum-rejected|run#11|AssertionError|assert got == want, f"TracingConfig mode 'Passthrough' answered {got!r}, expected {want!r}"
Issue-Key: lambda.invoke-log-type-outside-enum-rejected|run#15|AssertionError|assert got == want, f"LogType 'tail' answered {got!r}, expected {want!r}"
Issue-Key: lambda.layer-publish-unknown-storage-mode-rejected-mints-no-version|run#7|AssertionError|assert got == expected, f"S3ObjectStorageMode 'MOVE' was answered {got!r}, expected {expected!r}"
Issue-Key: lambda.update-function-code-unknown-storage-mode-rejected|run#25|AssertionError|assert got == ("ValidationException", (
Issue-Key: lambda.function-url-unknown-invoke-mode-rejected|run#7|AssertionError|assert got == (
Issue-Key: lambda.delete-layer-version-malformed-layer-name-rejected|run#9|AssertionError|assert got == want, f"layer name {bad!r}: expected {want!r}, got {got!r}"
Issue-Key: lambda.delete-resource-policy-off-pattern-arn-rejected|run#14|AssertionError|assert got == want, f"ResourceArn given as a bare name answered {got!r}, expected {want!r}"
Issue-Key: lambda.function-url-numeric-qualifier-in-name-rejected|run#23|AssertionError|assert got == want, f"FunctionName with a version qualifier: expected {want!r}, got {got!r}"
Issue-Key: lambda.create-alias-weight-key-not-a-version-number-rejected|run#18|AssertionError|assert got == expected, f"a '$LATEST' weight key gave {got!r}, expected {expected!r}"
Issue-Key: lambda.create-alias-routing-weight-version-key-over-length-rejected|run#25|AssertionError|assert got == expected, f"a {len(long_key)}-digit key answered {got!r}"
Issue-Key: lambda.create-function-dead-letter-target-arn-over-length-rejected|_refusal#15|AssertionError|raise AssertionError(f"expected a refusal, but the call succeeded: {result!r}")
Issue-Key: lambda.create-function-environment-key-leading-digit-rejected|_refusal#12|AssertionError|raise AssertionError(f"the call was expected to be refused but succeeded: {result!r}")
Issue-Key: lambda.create-function-role-arn-past-1600-characters-rejected|_refusal#13|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: lambda.create-function-kms-key-arn-over-10000-chars-rejected|run#11|AssertionError|assert "KMSKeyArn" not in config, (
Issue-Key: lambda.durable-kms-key-arn-over-length-rejected|run#11|AssertionError|assert stored.get("ExecutionTimeout") == 600 and "KMSKeyArn" not in stored, (
Issue-Key: lambda.publish-layer-version-license-info-over-512-rejected|run#21|AssertionError|assert "LicenseInfo" not in empty, (
Issue-Key: lambda.add-permission-function-name-over-256-chars-rejected|run#20|AssertionError|assert got == ("ResourceNotFoundException", f"Cannot find alias arn: {at_bound}"), (
Issue-Key: lambda.delete-resource-policy-arn-past-256-characters-reaches-alias-lookup-rejected|run#21|AssertionError|assert got_at_limit == (
Issue-Key: lambda.recursion-config-read-with-non-matching-function-name-rejected|run#20|AssertionError|assert got == ("ResourceNotFoundException", "The resource you requested does not exist."), (
Issue-Key: lambda.get-function-recursion-config-name-over-140-chars-rejected|run#15|AssertionError|assert missing == (
Issue-Key: lambda.layer-get-by-arn-over-2048-chars-rejected|run#12|AssertionError|assert got_at == expected_at, (
Issue-Key: lambda.publish-layer-version-s3-bucket-trailing-dot-rejected|run#14|AssertionError|assert got == ("ValidationException", (
Issue-Key: lambda.update-code-s3-object-version-length-bounds-rejected|run#32|AssertionError|assert got == want, f"a 1024-character S3ObjectVersion was answered {got!r}, expected {want!r}"
Issue-Key: lambda.publish-layer-version-unknown-compatible-runtime-rejected|run#13|AssertionError|assert (code, head, sep) == ("ValidationException", expected_head, "enum value set: ["), (
Issue-Key: lambda.function-url-cors-allow-header-with-line-break-rejected|_rejection#4|AssertionError|raise AssertionError(f"the call should have been refused; it returned {result!r}")
Issue-Key: lambda.function-url-cors-allow-method-with-line-break-rejected|_refusal#5|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {answer!r}")
Issue-Key: lambda.function-url-cors-expose-header-with-line-break-rejected|_refusal#10|AssertionError|raise AssertionError(f"expected the call to be refused, but it returned {result!r}")
Issue-Key: lambda.function-url-cors-origin-with-line-break-rejected|_refusal#9|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {out!r}")
Issue-Key: lambda.function-url-cors-expose-header-over-1024-characters-rejected|_get_served#17|Timeout|time.sleep(min(2 ** attempt, 15))
Issue-Key: lambda.function-url-cors-max-age-above-one-day-rejected|run#17|AssertionError|assert status == 200, f"preflight should be answered 200, got {status!r} {headers!r}"
Issue-Key: lambda.delete-layer-version-name-over-140-characters-rejected|run#11|AssertionError|assert got == (
Issue-Commit: 35f7de95e926ecd075d4f0c5efe73729faf5e0c1

# lambda: 10 net-new causes in the control-plane validation surface — enum value sets, ARN/name patterns and member constraints that are never enforced

> _Note: this issue was identified against the code base at commit `35f7de95e926ecd075d4f0c5efe73729faf5e0c1`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateFunction

## Description
Ten distinct net-new root causes in the Lambda control plane's request validation. All ten are about the REFUSAL a client sees for a bad request: the error code, the message text, or whether a refusal happens at all. An open issue 20261127T155721Z-lambda-logs-lambda-validation-enum-value-sets-are-listed-alphabetically.md describes the same family under the lambda__logs heading; it does not carry any of the replay keys below, so these are filed here.

Cause 1 of 10: the enum value set printed in a ValidationException message is emitted in the wrong order.
AWS: when a string member targeting an enum is off the enum, Lambda answers 400 ValidationException with the message `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]`. The fixed text is that sentence; `<value>`, `<memberPath>` come from the request and `<values>` is the service's own fixed ordering for that enum. Smithy: ApplicationLogLevel, SystemLogLevel, TracingMode, LogType, S3ObjectStorageMode, InvokeMode are enums; the recorded AWS answers show orderings that are NOT the model declaration order (e.g. ApplicationLogLevel is recorded as `[FATAL, TRACE, WARN, ERROR, INFO, DEBUG]`, SystemLogLevel as `[WARN, INFO, DEBUG]`, LogType as `[Tail, None]`, S3ObjectStorageMode as `[REFERENCE, COPY]`).
Emulator: emits the model's declaration order instead — `[TRACE, DEBUG, INFO, WARN, ERROR, FATAL]`, `[DEBUG, INFO, WARN]`, `[None, Tail]`, `[COPY, REFERENCE]` — and for tracingConfig.mode and invokeMode answers a value set that does not match the recording at all.
Why: a client that asserts on, logs, or diffs the refusal message gets a different string for every one of these members, and SDK test suites that pin the message fail.
Fix: the shared enum-validation helper that renders `Member must satisfy enum value set:` for the lambda service — the value list must be a per-enum fixed sequence taken from AWS, not the model/alphabetical order.
Traces: lambda.create-function-application-log-level-outside-enum-rejected, lambda.create-function-system-log-level-outside-enum-rejected, lambda.create-function-tracing-mode-outside-enum-rejected, lambda.invoke-log-type-outside-enum-rejected, lambda.layer-publish-unknown-storage-mode-rejected-mints-no-version, lambda.update-function-code-unknown-storage-mode-rejected, lambda.function-url-unknown-invoke-mode-rejected.

Cause 2 of 10: the regular-expression text quoted in a pattern ValidationException is not AWS's pattern.
AWS: a pattern violation answers 400 ValidationException `1 validation error detected: Value '<value>' at '<memberPath>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>`, where `<pattern>` is verbatim the service's own pattern for that member. Recorded: layerName is `(arn:(aws[a-zA-Z-]*)?:lambda:(eusc-)?[a-z]{2}((-gov)|(-iso([a-z]?)))?-[a-z]+-\d{1}:\d{12}:layer:[a-zA-Z0-9-_]{1,140}|[a-zA-Z0-9-_]{1,140})`; resourceArn is `arn:...:(function:[a-zA-Z0-9-_]+(:(\$LATEST(\.PUBLISHED)?|[a-zA-Z0-9-_])+)?|runtime/[a-zA-Z0-9-_./]+)`.
Emulator: quotes a different pattern for the same member — layerName without the {1,140} bounds, resourceArn with the runtime/function alternatives inverted and `runtime/` unable to match dots and slashes. For CreateFunctionUrlConfig the emulator validates a split-out `qualifier` member (`Value '1' at 'qualifier' ... pattern: ((?!^\d+$)^[0-9a-zA-Z-_]+$)`) where AWS validates the whole `functionName` member against NamespacedFunctionName.
Why: the message names a member that does not exist in the request and quotes a pattern the client cannot match against its own input, so error handling keyed on the member path misroutes.
Fix: the pattern strings and the member paths in the validation of DeleteLayerVersion, DeleteResourcePolicy and CreateFunctionUrlConfig handlers.
Traces: lambda.delete-layer-version-malformed-layer-name-rejected, lambda.delete-resource-policy-off-pattern-arn-rejected, lambda.function-url-numeric-qualifier-in-name-rejected, lambda.delete-layer-version-name-over-140-characters-rejected.

Cause 3 of 10: CreateAlias RoutingConfig.AdditionalVersionWeights map keys are validated with the wrong error class and message.
AWS: a map key that is not a version number answers 400 ValidationException with `1 validation error detected: Value '{<key>=<weight>}' at 'routingConfig.additionalVersionWeights' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 1024, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [0-9]+]` — i.e. the whole map is quoted and all three key constraints are listed. An over-long key is refused by the same validation, before any function or alias lookup.
Emulator: answers InvalidParameterValueException with `Value '$LATEST' at 'routingConfig.additionalVersionWeights' failed to satisfy constraint: Map keys must satisfy regular expression pattern: ^[0-9]+$`, and for a 1025-digit key skips validation entirely and answers ResourceNotFoundException `Function not found: <arn>:<key>`.
Why: the client sees a non-validation error class (and a not-found for a purely malformed request), so retry/validation branches take the wrong path.
Fix: RoutingConfig map-key validation in the CreateAlias/UpdateAlias handlers, ahead of resource resolution.
Traces: lambda.create-alias-weight-key-not-a-version-number-rejected, lambda.create-alias-routing-weight-version-key-over-length-rejected.

Cause 4 of 10: modelled member constraints on CreateFunction are not enforced at all — the call succeeds.
AWS: CreateFunction refuses a request whose members violate their modelled constraints, with 400 ValidationException and no function created. Smithy: Environment variable names target EnvironmentVariableName with pattern `^[a-zA-Z]([a-zA-Z0-9_])+$` (so a leading digit is invalid); Role targets RoleArn with @length max 10000 and an IAM-role ARN pattern; DeadLetterConfig.TargetArn is a ResourceArn.
Emulator: answers 201 Created and creates the function. Recorded: `{'Environment': {'Variables': {'9LIVES': 'nine'}}}` accepted; a 1600+ character Role accepted and echoed; `DeadLetterConfig: {'TargetArn': ''}` accepted after an over-long value.
Why: a client relying on AWS to reject malformed configuration gets a function that exists with illegal configuration, and the later invoke fails in a way that cannot be traced back to the create call.
Fix: input validation in the CreateFunction handler (services/service-com-amazonaws-lambda-awsgirapiservice/src/ops/create_function.rs), applied before state is written.
Traces: lambda.create-function-environment-key-leading-digit-rejected, lambda.create-function-role-arn-past-1600-characters-rejected, lambda.create-function-dead-letter-target-arn-over-length-rejected.

Cause 5 of 10: an empty string for an optional ARN/text member is stored and echoed instead of leaving the member unset.
AWS: KMSKeyArn, LicenseInfo and the durable KMSKeyArn accept the empty string as 'no value' — the read-back omits the member (the function stays on the Lambda-managed key, the layer version carries no LicenseInfo). Smithy: KMSKeyArn @length min 0 with pattern `^(arn:...)|()$` — the empty alternative is explicitly the 'unset' encoding.
Emulator: persists and returns `KMSKeyArn: ''` on the function and on the durable config, and `LicenseInfo: ''` on the published layer version.
Why: a client that tests `if KMSKeyArn:` or compares read-back to its template sees a key configured where none is, and drift detection reports a difference that does not exist.
Fix: normalise empty-string optional members to absent in the CreateFunction / UpdateFunctionConfiguration / PublishLayerVersion handlers' state writes and in their serializers.
Traces: lambda.create-function-kms-key-arn-over-10000-chars-rejected, lambda.durable-kms-key-arn-over-length-rejected, lambda.publish-layer-version-license-info-over-512-rejected.

Cause 6 of 10: ResourceNotFoundException messages for qualified-name lookups use the wrong sentence.
AWS: when a FunctionName/ResourceArn carries a qualifier that names no alias, Lambda answers 404 ResourceNotFoundException `Cannot find alias arn: <arn>` — the fixed text is `Cannot find alias arn: ` followed by the fully qualified ARN built from the request. For a recursion-config read of an unknown function AWS answers with its own not-found sentence, not the function-ARN form the emulator emits.
Emulator: answers `Function not found: <arn>` for the alias case (AddPermission with a 256-character qualified name, DeleteResourcePolicy with a 256-character ARN) and `Function not found: <arn>` for GetFunctionRecursionConfig on an absent/over-long name.
Why: clients branch on the message to tell 'the function is missing' from 'the alias is missing'; both now read as a missing function, so alias-creation retries never fire.
Fix: the not-found message selection in the AddPermission, DeleteResourcePolicy and GetFunctionRecursionConfig handlers — pick the alias sentence when the request carried a qualifier.
Traces: lambda.add-permission-function-name-over-256-chars-rejected, lambda.delete-resource-policy-arn-past-256-characters-reaches-alias-lookup-rejected, lambda.recursion-config-read-with-non-matching-function-name-rejected, lambda.get-function-recursion-config-name-over-140-chars-rejected.

Cause 7 of 10: GetLayerVersionByArn with a malformed/over-long ARN is answered as a missing resource instead of an invalid parameter.
AWS: an ARN whose layer-name segment is over the 140-character bound is a parameter error — 400 InvalidParameterValueException `Invalid layer version <arn>`, with `<arn>` the ARN from the request.
Emulator: answers 404 ResourceNotFoundException `The resource you requested does not exist.`
Why: the client cannot tell a typo in its ARN from a layer it has not published, so it retries a publish that will never help.
Fix: ARN parsing/validation in the GetLayerVersionByArn handler, ahead of the lookup.
Traces: lambda.layer-get-by-arn-over-2048-chars-rejected.

Cause 8 of 10: errors from the S3 source object are passed through instead of being mapped to the refusal AWS gives.
AWS: a bucket name with a trailing dot and an over-long S3ObjectVersion are refused before the object is fetched. The recorded answers quote the S3 condition AWS actually hits — for an invalid version id, InvalidParameterValueException `Error occurred while GetObjectVersion. S3 Error Code: InvalidArgument. S3 Error Message: Invalid version id specified`.
Emulator: forwards whatever its own S3 returns — `S3 Error Code: NoSuchBucket. S3 Error Message: The specified bucket does not exist` for the trailing-dot bucket, and `S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.` for the over-long version id.
Why: the client is told the object is absent when its request is malformed, so it uploads again rather than fixing the identifier.
Fix: validate S3Bucket/S3ObjectVersion in the PublishLayerVersion and UpdateFunctionCode handlers and map the S3 error class before serialising.
Traces: lambda.publish-layer-version-s3-bucket-trailing-dot-rejected, lambda.update-code-s3-object-version-length-bounds-rejected.

Cause 9 of 10: PublishLayerVersion's CompatibleRuntimes refusal lists a runtime set that is not AWS's.
AWS: an unknown compatible runtime answers 400 ValidationException `1 validation error detected: Value '[<runtime>]' at 'compatibleRuntimes' failed to satisfy constraint: Member must satisfy constraint: [Member must satisfy enum value set: [<runtimes>], Member must not be null]`, where `<runtimes>` is the service's published Runtime enum in its own fixed order.
Emulator: emits a runtime list of its own (ending `..., provided.al2023, nodejs26.x, python3.15, java8.al2023, java11.al2023, java17.al2023`) whose membership and ordering differ from the recording.
Why: the message is the only place a client learns which runtimes a layer may declare; a wrong list sends it to an unsupported runtime.
Fix: the Runtime enum value list used by the PublishLayerVersion validator.
Traces: lambda.publish-layer-version-unknown-compatible-runtime-rejected.

Cause 10 of 10: function-URL CORS member constraints are not enforced.
AWS: CreateFunctionUrlConfig/UpdateFunctionUrlConfig refuse CORS values that break their modelled constraints, with 400 ValidationException and no URL config created. Smithy: Cors.AllowHeaders/ExposeHeaders target Header (@length max 1024), Cors.AllowOrigins target Origin (@length 1..253), Cors.MaxAge @range max 86400; a header/method/origin containing a line break cannot satisfy those members (recorded behavior: AWS refuses each).
Emulator: answers 201 Created and echoes the illegal values back — `'AllowHeaders': ['x-trace-id', 'x-line\nbreak']`, `'AllowMethods': ['PU\nT']`, `'ExposeHeaders': ['X-Release-Marker\nX-Other']`, `'AllowOrigins': ['https://app.example.com\nhttps://admin.example.com']` — and over-long/over-max values are likewise accepted. The MaxAge-above-86400 check additionally never reaches its assertion because the preflight it then sends is answered 403 InvalidSignatureException instead of 200.
Why: a client gets a URL whose CORS headers contain header-injection payloads that AWS would have rejected at configure time, and a browser preflight then behaves differently against the emulator than against AWS.
Fix: Cors member validation in the CreateFunctionUrlConfig/UpdateFunctionUrlConfig handlers.
Traces: lambda.function-url-cors-allow-header-with-line-break-rejected, lambda.function-url-cors-allow-method-with-line-break-rejected, lambda.function-url-cors-expose-header-with-line-break-rejected, lambda.function-url-cors-origin-with-line-break-rejected, lambda.function-url-cors-expose-header-over-1024-characters-rejected, lambda.function-url-cors-max-age-above-one-day-rejected.

## Evidence
Cause 1: expected `...enum value set: [FATAL, TRACE, WARN, ERROR, INFO, DEBUG]`, got `[TRACE, DEBUG, INFO, WARN, ERROR, FATAL]`; expected `[WARN, INFO, DEBUG]`, got `[DEBUG, INFO, WARN]`; expected `[Tail, None]`, got `[None, Tail]`; expected `[REFERENCE, COPY]`, got `[COPY, REFERENCE]`.
Cause 2: expected layerName pattern `...:layer:[a-zA-Z0-9-_]{1,140}|[a-zA-Z0-9-_]{1,140})`, got `...:layer:[a-zA-Z0-9-_]+)|[a-zA-Z0-9-_]+`; expected a `functionName` pattern violation, got `Value '1' at 'qualifier' ... pattern: ((?!^\d+$)^[0-9a-zA-Z-_]+$)`.
Cause 3: expected `('ValidationException', "1 validation error detected: Value '{$LATEST=1.0}' at 'routingConfig.additionalVersionWeights' failed to satisfy constraint: Map keys must satisfy constraint: [Member must have length less than or equal to 1024, Member must have length greater than or equal to 1, Member must satisfy regular expression pattern: [0-9]+]")`, got `('InvalidParameterValueException', "... Map keys must satisfy regular expression pattern: ^[0-9]+$")`; the 1025-digit key got `('ResourceNotFoundException', 'Function not found: <arn>:999...')`.
Cause 4: expected a 400 refusal with nothing created; got HTTP 201 with `'Environment': {'Variables': {'9LIVES': 'nine'}}` / a 1600-character `Role` / `'DeadLetterConfig': {'TargetArn': ''}`.
Cause 5: `got KMSKeyArn=''`; `read back {'KMSKeyArn': '', 'RetentionPeriodInDays': 14, 'ExecutionTimeout': 600}`; `version 2 was published with an empty LicenseInfo but reads ''`.
Cause 6: expected `('ResourceNotFoundException', 'Cannot find alias arn: <arn>')`, got `('ResourceNotFoundException', 'Function not found: arn:aws:lambda:<region>:<account>:function:<name>:<qualifier>')`.
Cause 7: expected `('InvalidParameterValueException', 'Invalid layer version <arn>')`, got `('ResourceNotFoundException', 'The resource you requested does not exist.')`.
Cause 8: expected `S3 Error Code: InvalidArgument. S3 Error Message: Invalid version id specified`, got `S3 Error Code: NoSuchVersion. S3 Error Message: The specified version does not exist.`; trailing-dot bucket got `S3 Error Code: NoSuchBucket. S3 Error Message: The specified bucket does not exist`.
Cause 9: emulator's `compatibleRuntimes` enum list differs in membership and order from the recorded AWS list.
Cause 10: expected a 400 refusal, got HTTP 201 and the illegal CORS value echoed (`'AllowMethods': ['PU\nT']`).

## Steps to Reproduce
Boot the emulator and replay the behavioral suite; the 33 check ids in trace_ids reproduce the ten causes. Minimal repro per cause: (1) `aws lambda create-function --logging-config ApplicationLogLevel=VERBOSE ...` and compare the enum list in the ValidationException message; (2) `aws lambda delete-layer-version --layer-name 'name.' --version-number 1` and compare the quoted pattern; (3) `aws lambda create-alias --routing-config AdditionalVersionWeights={'$LATEST'=1.0}`; (4) `aws lambda create-function --environment Variables={9LIVES=nine}` — expect a refusal, observe 201; (5) create a function with `--kms-key-arn ''` and read it back; (6) `aws lambda add-permission --function-name <256-char name:qualifier>`; (7) `aws lambda get-layer-version-by-arn --arn <2048-char layer arn>`; (8) `aws lambda update-function-code --s3-object-version <1024 chars>`; (9) `aws lambda publish-layer-version --compatible-runtimes nodejs23.x`; (10) `aws lambda create-function-url-config --cors 'AllowMethods=["PU\nT"]'` — expect a refusal, observe 201.
