Issue-Source: trace-sweep-divergence
Issue-Trace: amplifybackend-token-session-refusals
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: amplifybackend-token-session-refusals
Issue-Shape-Id: com.amazonaws.amplifybackend#AmplifyBackend
Issue-Key: amplifybackend|CreateToken|2xx->2xx|$.challengeCode
Issue-Key: amplifybackend|CreateToken|2xx->2xx|$.sessionId
Issue-Key: amplifybackend|CreateToken|2xx->2xx|$.ttl
Issue-Key: amplifybackend|CreateToken|2xx->2xx|$headers.content-type
Issue-Key: amplifybackend|CreateToken|2xx->2xx|$headers.etag
Issue-Key: amplifybackend|GetToken|2xx->2xx|$headers.content-type
Issue-Key: amplifybackend|GetToken|2xx->2xx|$headers.etag
Issue-Key: amplifybackend|DeleteToken|2xx->2xx|$headers.content-type
Issue-Key: amplifybackend|DeleteToken|2xx->2xx|$headers.etag
Issue-Commit: 0e14ec34ba796ebb423716588b97f90ad61f6019

# AmplifyBackend token-session responses diverge: CreateToken field formats and missing JSON envelope headers

> _Note: this issue was identified against the code base at commit `0e14ec34ba796ebb423716588b97f90ad61f6019`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateToken, GetToken, DeleteToken

## Description
Two root causes, 3 wire divergence clusters (4 fixture steps) on the AmplifyBackend Admin-UI challenge-token path. All four steps answer the right HTTP status (200), so this is purely a response-content/envelope fidelity gap on the SUCCESS path. Each cause is a separate fix.

NOTE ON AN EXISTING ISSUE: the open issue 20261022T142056Z-amplifybackend-createbackend-createbackendauth-getbackendjob-error-contract-wrong-status.md mentions the same content-type/ETag envelope symptom in passing (its Cause 3), but it is keyed to the ERROR paths of CreateBackend / CreateBackendAuth / GetBackendJob and its Issue-Key / Issue-Trace-Ids lines name none of the token-operation clusters below; the replay reported owned_by=[] for all three clusters here, so a fix validated only against that issue will never be exercised by these steps. Cause 1 below is not described there at all.

=== Cause 1 of 2: CreateToken mints SessionId / ChallengeCode / Ttl in formats AWS does not use ===
AWS: Smithy models CreateToken as POST /backend/{AppId}/challenge, success 200, output AppId (jsonName appId), ChallengeCode (jsonName challengeCode), SessionId (jsonName sessionId), Ttl (jsonName ttl) — all bare __string with no pattern or length constraint, so the FORMAT is recorded behavior only. The recording asserts these three members by SHAPE (expected '<shaped-not-asserted>'), i.e. the exact secret is per-run but its form is the contract, and all three shape checks fail against the emulator. SessionId is documented as "A unique ID provided when creating a new challenge token" and the recorded value has the form of an opaque service-minted unique id (a UUID — the emulator's own state code asserts the same thing at services/service-com-amazonaws-amplifybackend-amplifybackend/src/state.rs:942, "A session id is a fresh UUID per mint", which its minting function contradicts). Ttl is documented as "The expiry time for the one-time generated token code" — an expiry time, and the recorded value's shape is not a small seconds-count. Hypothesis: the recorded Ttl is the token's expiry INSTANT rather than its lifetime SPAN, which is what the member's documented wording ("expiry time") says and what the shape check rejects in the emulator's value.
Emulator: CreateToken reports sessionId built from the emulator's own credential-store id plus a monotonic counter — the generic form '<emulator-credential-id-lowercased>-<16 hex digits of a per-process sequence>' (state.rs mint_session_id, format!("{}-{sequence:016x}")), which is neither opaque nor UUID-shaped and leaks the word 'emulator' plus a guessable, process-restart-resetting counter into a value the service treats as unguessable secret addressing. challengeCode is 20 characters of the credential store's 64-symbol alphabet (CHALLENGE_CODE_CHARS, state.rs:679) rather than the recorded code's shape. ttl is the LIFETIME in whole seconds as a decimal string — the constant CHALLENGE_TOKEN_TTL (300s) rendered by ChallengeToken::ttl() — rather than an expiry time.
Why: a client that validates or parses these fields breaks. sessionId is path-embedded by GetToken/DeleteToken (/backend/{AppId}/challenge/{SessionId}) and is treated by Admin-UI clients as an opaque unique id of a known form; a counter-derived id that resets to its first value on every emulator restart makes two different tokens collide across restarts and makes somebody else's challenge code a guessable address. A client that reads ttl as an expiry time (as the model documents it) and the emulator as a 300-second span computes an expiry in 1970, so it either discards a live token immediately or never expires one.
Fix: CreateToken's handler and the token state it mints from — services/service-com-amazonaws-amplifybackend-amplifybackend/src/ops/create_token.rs (the CreateTokenOutput it builds at the end of handle) and crate::state::mint_session_id / ChallengeToken::ttl / CHALLENGE_CODE_CHARS in src/state.rs: mint an opaque unique session id of the form AWS returns rather than one derived from an emulator credential id and a visible sequence, and report Ttl as the member AWS documents (the expiry time) in the recorded form. GetToken's read path already round-trips what CreateToken stored, so fixing the mint fixes both.
Traces: amplifybackend-token-session-refusals#task#2 (CreateToken; $.sessionId, $.challengeCode, $.ttl all value_mismatch against the recorded shape).

=== Cause 2 of 2: every token-operation response omits the charset on Content-Type and sends no ETag ===
AWS: recorded behavior only — Smithy declares no httpHeader bindings for CreateTokenResponse / GetTokenResponse / DeleteTokenResponse, so the envelope is not modelled. Every recorded AmplifyBackend 200 response on this path carries Content-Type 'application/json; charset=utf-8' (fixed text, no variable part) and an ETag header whose VALUE is body-derived and asserted shaped-only, so only the header's PRESENCE is the behavior, never one run's literal entity tag.
Emulator: answers Content-Type 'application/json' — the charset parameter is absent — and sends no ETag header at all, on CreateToken, GetToken and DeleteToken alike. The status (200) and, apart from Cause 1, the JSON document are correct; only the envelope diverges, and it diverges identically for all three operations, which places it in the shared restJson1 response serializer rather than in any one handler.
Why: a client that selects a decoder on the full Content-Type value (exact match on 'application/json; charset=utf-8', common in strict HTTP layers and in recorded-response assertions) rejects or mis-decodes a response real AWS would have it accept. A client or cache that conditionalises on ETag (If-None-Match) can never revalidate a challenge-token read, so it refetches unconditionally and any caching layer written against AWS silently degrades.
Fix: the service's shared restJson1 response serializer / response-header layer (the component that writes the success envelope for every operation of services/service-com-amazonaws-amplifybackend-amplifybackend, reached from src/service.rs) — emit Content-Type with the charset parameter and compute a body-derived ETag on success responses, rather than patching each operation handler.
Traces: amplifybackend-token-session-refusals#task#2 (CreateToken), #task#3 (GetToken), #task#6 and #task#10 (DeleteToken) — the same two header keys in all three clusters.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (summary: 45 steps, 25 passed, 20 mismatched, 4 diverged, score 55.6%; emulator_up=true, bootstrap_signal=false). All three clusters reported stub_path="" and owned_by=[], i.e. neither blocked by an unimplemented operation nor owned by an open issue.

Cluster 1 — fixture amplifybackend-token-session-refusals, step task#2, CreateToken, expected 200 / actual 200:
  $.sessionId: value_mismatch (expected '<shaped-not-asserted>', got 'emulatorasrnfkizilsz-0000000000000000')  <- recorded shape is an opaque unique id; the emulator's value is '<credential-id>-<16 hex sequence digits>', the sequence starting at 0 for the process
  $.challengeCode: value_mismatch (expected '<shaped-not-asserted>', got 'uT2tZeUuFk4pwiiuxJbu')  <- 20 chars of the credential store's alphabet; the literal secret is per-run, the FORM is what diverges
  $.ttl: value_mismatch (expected '<shaped-not-asserted>', got '300')  <- the emulator reports CHALLENGE_TOKEN_TTL as a seconds SPAN; the model documents Ttl as "The expiry time"
  $headers.content-type: value_mismatch (expected 'application/json; charset=utf-8', got 'application/json')
  $headers.etag: missing (expected '<shaped-not-asserted>', got None)
  divergence_key: service=amplifybackend|CreateToken|status=200->200|keys=$.challengeCode,$.sessionId,$.ttl,$headers.content-type,$headers.etag

Cluster 2 — fixture amplifybackend-token-session-refusals, step task#3, GetToken, expected 200 / actual 200:
  $headers.content-type: value_mismatch (expected 'application/json; charset=utf-8', got 'application/json')
  $headers.etag: missing (expected '<shaped-not-asserted>', got None)
  (body members round-trip correctly, including the empty read-path Ttl)

Cluster 3 — fixture amplifybackend-token-session-refusals, step task#6, DeleteToken, expected 200 / actual 200, affected_count 2 (also task#10):
  $headers.content-type: value_mismatch (expected 'application/json; charset=utf-8', got 'application/json')
  $headers.etag: missing (expected '<shaped-not-asserted>', got None)

Model citations (services/service-com-amazonaws-amplifybackend-amplifybackend/smithy-model/com-amazonaws-amplifybackend-amplifybackend.json):
  CreateToken  POST /backend/{AppId}/challenge, 200 -> {appId, challengeCode, sessionId, ttl} (all __string, unconstrained); errors BadRequestException 400 / GatewayTimeoutException 504 / NotFoundException 404 / TooManyRequestsException 429.
  GetToken     GET /backend/{AppId}/challenge/{SessionId}, 200 -> same four members; same errors.
  DeleteToken  POST /backend/{AppId}/challenge/{SessionId}/remove, 200 -> {isSuccess (__boolean)}; same errors.
  No member carries a pattern, length or timestampFormat trait and no response member is httpHeader-bound, so both the field formats and the envelope headers are recorded behavior only.

Code citations for the emulator side: src/ops/create_token.rs lines 226-238 (session id minted, ttl() reported); src/state.rs mint_session_id (format!("{}-{sequence:016x}")), CHALLENGE_TOKEN_TTL = Duration::from_secs(300), CHALLENGE_CODE_CHARS = 20, ChallengeToken::ttl() (expires_at - created_at, as_secs), and the comment at state.rs:942 claiming session ids are UUIDs.

## Steps to Reproduce
Boot the emulator from this checkout and replay fixture amplifybackend-token-session-refusals:

1. task#2 — POST /backend/{AppId}/challenge (CreateToken) for a registered Amplify app id. Inspect the JSON body: sessionId comes back as '<emulator-credential-id>-<16 hex digits>' with the sequence starting at 0 after every emulator restart instead of an opaque AWS-shaped unique id; ttl comes back as the decimal lifetime '300' instead of the documented expiry time; challengeCode comes back as 20 characters of the emulator credential store's alphabet instead of the recorded code form (Cause 1).
2. Repeat step 1 after restarting the emulator: the same sessionId ('...-0000000000000000') is minted again, so two distinct tokens share an address.
3. task#3 — GET /backend/{AppId}/challenge/{SessionId} (GetToken) with the session id from step 1. Body members round-trip, but inspect the response headers (Cause 2).
4. task#6 and task#10 — POST /backend/{AppId}/challenge/{SessionId}/remove (DeleteToken). Status is 200 with isSuccess, headers again diverge.
5. On every one of those responses, check the envelope: Content-Type is 'application/json' instead of 'application/json; charset=utf-8', and no ETag response header is present (its value is body-derived, so assert presence only) (Cause 2).
