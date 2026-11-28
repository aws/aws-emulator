Issue-Source: trace-sweep-divergence
Issue-Trace: lambda-code-signing-config-enforcement
Issue-Step: task#13
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: lambda-code-signing-config-enforcement lambda-function-codesigning-attach-detach-lifecycle lambda-function-codesigning-attach-error-paths lambda-function-empty-description-explicit-defaults-read-back-and-not-found
Issue-Shape-Id: com.amazonaws.lambda#AWSGirApiService
Issue-Key: lambda|GetFunctionCodeSigningConfig|2xx->2xx|$
Issue-Commit: 2d48989723ffc52951945f21d9774f4aa78dfaff

# lambda: GetFunctionCodeSigningConfig answers 200 with an empty body instead of the modeled members

> _Note: this issue was identified against the code base at commit `2d48989723ffc52951945f21d9774f4aa78dfaff`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetFunctionCodeSigningConfig

## Description
Replay of the simparity-wire-aws-v1 wire corpus against the emulator built from this checkout surfaces ONE net-new, unowned lambda cluster (every other cluster in the run is owned by an open issue or blocked by an unimplemented-operation stub). It is a RESPONSE PAYLOAD projection divergence on the code-signing read: no status and no error-code divergence (200 -> 200).

=== Cause 1 of 1: GetFunctionCodeSigningConfig returns a wholly empty JSON body where the recorded 200 carries both modeled members (fixture lambda-code-signing-config-enforcement, step task#13) ===
AWS: Smithy GetFunctionCodeSigningConfig (services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json) binds GET /2020-06-30/functions/{FunctionName}/code-signing-config with http code 200 and is @readonly; its output shape GetFunctionCodeSigningConfigResponse has exactly two members, both @required: CodeSigningConfigArn (target CodeSigningConfigArn, @length 0..200, pattern ^arn:(aws[a-zA-Z-]*)?:lambda:<region>:<account>:code-signing-config:csc-[a-z0-9]{17}$) and FunctionName (target FunctionName, @length 1..140). Its modeled errors are CodeSigningConfigNotFoundException (404), ResourceNotFoundException (404), InvalidParameterValueException (400), TooManyRequestsException (429) and ServiceException (500) — none of which applies at these steps, which all legitimately answer 200. Recorded behavior only for the body's shape: at all five affected steps the recorded 200 body is a JSON object that CONTAINS both member keys, each with the JSON value null — the fixture's expectation is {'CodeSigningConfigArn': None, 'FunctionName': None}, i.e. the keys are present and their values are the only thing not asserted. The recorded response is therefore NOT an empty object: AWS renders both modeled members of this read even when the function under test holds no code-signing association, and signals "no association" by their values rather than by omitting them.
Emulator: answers 200 with a completely empty JSON body — the comparator reports `$: empty_body (expected {'CodeSigningConfigArn': None, 'FunctionName': None}, got {})` — so both required members are absent from the wire payload rather than present-and-null. This is the mirror of the previously reported over-reporting of this read (open issue 20261117T041130Z-lambda-function-url-instants-and-the-getfunctioncodesigningconfig-read.md, Cause 3, which reported the opposite direction: FunctionName echoed from the request path where the recording carries null). That issue does NOT cover this cluster — the replay reports this cluster's owned_by empty, and its divergence key is on `$` (the whole body) rather than on `$.FunctionName` — so it is filed here as net-new and named for cross-reference.
Why: both members are @required in the model, so a strict generated client (any AWS SDK deserializing GetFunctionCodeSigningConfigResponse) that requires the members to be present fails to deserialize, or materializes defaults AWS never sent, on every code-signing read of a function with no attached configuration. A client that distinguishes "member present with a null value" from "member absent" — the distinction AWS's recorded body makes — cannot make it against this emulator, and tooling that walks the response object's keys to decide whether a code-signing configuration is attached sees no keys at all rather than the explicit nulls AWS returns.
Fix: the GetFunctionCodeSigningConfig handler's response projection in the lambda code-signing read path (the operation's serializer/handler, not state): emit the response object with BOTH modeled members always rendered — CodeSigningConfigArn and FunctionName — carrying the values the function's association provides, instead of suppressing absent members and collapsing the body to {}. The recorded contract is "the two keys are always serialized"; only their values vary with whether a configuration is attached.
Traces: lambda-code-signing-config-enforcement#task#13 (representative, GetFunctionCodeSigningConfig, 200 -> 200, key $); the same cluster also covers lambda-function-codesigning-attach-detach-lifecycle#task#8, lambda-function-codesigning-attach-error-paths#task#5, lambda-function-codesigning-attach-error-paths#task#7 and lambda-function-empty-description-explicit-defaults-read-back-and-not-found#task#4.

## Evidence
Replay summary (corpus simparity-wire-aws-v1): emulator_up true, bootstrap_signal false, steps_total 1889, steps_passed 1848, steps_mismatched 41, steps_errored 0, steps_blocked 0, score_pct 97.8, divergence_count 24. Of the 24 clusters, 23 carry a non-empty owned_by (already-filed open issues) or a stub path; the cluster below is the only unowned one.

Cause 1 — fixture lambda-code-signing-config-enforcement, step task#13, operation GetFunctionCodeSigningConfig, verdict mismatch, expected_status 200, actual_status 200,
divergence_key `service=lambda|GetFunctionCodeSigningConfig|status=200->200|keys=$`:
  detail: `$: empty_body (expected {'CodeSigningConfigArn': None, 'FunctionName': None}, got {})`
  expected body shape: a JSON object carrying BOTH modeled keys, CodeSigningConfigArn and FunctionName, each with a null value (the keys are the requirement; the values are not asserted)
  actual body: `{}` — neither required member serialized
  affected (affected_count 5): lambda-code-signing-config-enforcement#task#13, lambda-function-codesigning-attach-detach-lifecycle#task#8, lambda-function-codesigning-attach-error-paths#task#5, lambda-function-codesigning-attach-error-paths#task#7, lambda-function-empty-description-explicit-defaults-read-back-and-not-found#task#4
  owned_by: [] (unowned), stub_path: "" (not blocked)

Model grounding (smithy_query on services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/com-amazonaws-lambda-awsgirapiservice.json): GetFunctionCodeSigningConfig — http {method GET, uri /2020-06-30/functions/{FunctionName}/code-signing-config, code 200}, readonly; output GetFunctionCodeSigningConfigResponse { CodeSigningConfigArn (required, length 0..200, pattern ^arn:(aws[a-zA-Z-]*)?:lambda:<region>:<account>:code-signing-config:csc-[a-z0-9]{17}$), FunctionName (required, length 1..140) }; errors CodeSigningConfigNotFoundException 404, ResourceNotFoundException 404, InvalidParameterValueException 400, TooManyRequestsException 429, ServiceException 500.

Related open issue that does NOT cover this cluster: 20261117T041130Z-lambda-function-url-instants-and-the-getfunctioncodesigningconfig-read.md (its Cause 3 reports the opposite direction on the same read — FunctionName echoed where the recording carries null — and the replay reports this cluster unowned).

## Steps to Reproduce
1. git_clone the project repo and boot the emulator built from that checkout.
2. Call replay_wire_corpus() on corpus simparity-wire-aws-v1 and look for the single cluster with an empty owned_by and an empty stub_path (service=lambda, operation GetFunctionCodeSigningConfig, divergence_key `service=lambda|GetFunctionCodeSigningConfig|status=200->200|keys=$`).
3. Replay fixture lambda-code-signing-config-enforcement up to step task#13: GET /2020-06-30/functions/{FunctionName}/code-signing-config for the function under test. The response is 200 in both cases; inspect the body. The recorded body is a JSON object containing both keys CodeSigningConfigArn and FunctionName (values null); observe the emulator answer `{}` with neither member serialized.
4. The same empty body appears at lambda-function-codesigning-attach-detach-lifecycle#task#8, lambda-function-codesigning-attach-error-paths#task#5, lambda-function-codesigning-attach-error-paths#task#7 and lambda-function-empty-description-explicit-defaults-read-back-and-not-found#task#4.
5. After the fix, re-replay ALL FOUR fixtures (lambda-code-signing-config-enforcement, lambda-function-codesigning-attach-detach-lifecycle, lambda-function-codesigning-attach-error-paths, lambda-function-empty-description-explicit-defaults-read-back-and-not-found) and confirm all five steps pass, not just the representative.
