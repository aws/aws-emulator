Issue-Source: trace-sweep-divergence
Issue-Trace: appsync-api-key-lifecycle
Issue-Step: task#4
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: appsync-api-key-lifecycle
Issue-Shape-Id: com.amazonaws.appsync#AWSDeepdishControlPlaneService
Issue-Key: appsync|ListApiKeys|2xx->2xx|$.apiKeys[id="da2-mfocndbgbb2ebey6zzvrintaic"].deletes
Issue-Commit: d2ebcd8c12cc6501ce5e652d8f3a2ce865548576

# AppSync ListApiKeys returns wrong ApiKey deletes grace period

> _Note: this issue was identified against the code base at commit `d2ebcd8c12cc6501ce5e652d8f3a2ce865548576`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListApiKeys

## Description
1 cause.

Cause 1 of 1: ListApiKeys projects ApiKey$deletes with a shorter deletion grace period than AWS applies.

AWS: for a da2 key, ListApiKeys returns both the expiration time and the deletion time in seconds since the epoch, rounded down to the nearest hour (Smithy: ApiKey.deletes, target Long, doc "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour"; ApiKey doc for key version da2: "ListApiKeys returns the expiration time and deletion time in seconds"). The deletion time is derived from the key's expires value plus the service's fixed post-expiry retention window, so for a given expires value the deletes value is fully determined — it is not a free-running or per-run value. The recorded response shows deletes sitting a fixed 60-day (5,184,000 s) interval beyond the value the emulator computes, i.e. AWS retains the key for a longer window after expiry than the emulator does. HTTP status is 200 on both sides; this is a response-body value divergence only.

Emulator: the listed key's deletes is computed with a shorter retention offset than AWS's, so every element of $.apiKeys carries a deletion timestamp earlier than AWS would report for the same expires. In the replayed step the asserted key's deletes came back 1798761600 where the recording has 1803945600 (the concrete seconds values are that run's clock-derived instances of the rule, not the requirement; the requirement is the offset relationship between expires and deletes).

Why: a client that inventories keys with ListApiKeys and schedules rotation, re-creation or cleanup off deletes computes a reclamation date that does not exist on AWS. It will treat keys as already reclaimed while AWS still has them (so a "replacement" key is created needlessly and the old key stays live), or will skip a key it believed had more time. Any assertion that CreateApiKey's deletes and the deletes later reported by ListApiKeys agree with AWS's window also fails.

Fix: the shared API-key expiry/deletion arithmetic behind the ListApiKeys handler's response projection — the derivation that turns a stored expires into the projected deletes must use AWS's retention window (and the same derivation must be used by CreateApiKey/UpdateApiKey so all three agree).

Traces: appsync-api-key-lifecycle#task#4 (ListApiKeys, 200).

Related open issue that does NOT cover this step: 20261029T200818Z-appsync-api-key-deletion-window-and-tagging-responses.md describes the same deletion-window arithmetic and even lists a ListApiKeys replay key, but that key is bound to a different api-key id (da2-dq5e45sgwr3mamypsk3zabjbfx), so this replayed divergence (key id da2-mfocndbgbb2ebey6zzvrintaic) is reported as unowned and is not gated by it. A fixer correcting the shared derivation for that issue should re-replay this step too.

## Evidence
Replay of simparity-wire-aws-v1 against the emulator built from the checkout (emulator up, /__admin/health 200).

Fixture appsync-api-key-lifecycle, step task#4, operation ListApiKeys:
- status: expected 200, actual 200 (agrees).
- $.apiKeys[id="da2-mfocndbgbb2ebey6zzvrintaic"].deletes: value_mismatch — expected 1803945600, got 1798761600.
- Difference: 5,184,000 seconds = 60 days; the emulator's deletes is that much earlier than AWS's. Both values are hour-aligned, so only the retention offset differs, not the rounding.
- divergence_key: service=appsync|ListApiKeys|status=200->200|keys=$.apiKeys[id="da2-mfocndbgbb2ebey6zzvrintaic"].deletes; verdict mismatch; stub_path empty; owned_by empty.

Smithy evidence: ApiKey.deletes (Long) "The time after which the API key is deleted. The date is represented as seconds since the epoch, rounded down to the nearest hour"; ApiKey da2 doc "ListApiKeys returns the expiration time and deletion time in seconds".

## Steps to Reproduce
1. Clone the repository and boot the emulator built from that checkout.
2. Replay the simparity-wire-aws-v1 wire corpus; fixture appsync-api-key-lifecycle mismatches at step task#4.
3. By hand: CreateApiKey on a GraphQL API with an explicit expires value, then call ListApiKeys for that API and compare the returned deletes to expires. The emulator's deletes is a shorter fixed interval past expires than AWS's (60 days shorter in the recording); AWS's value is expires plus its fixed post-expiry retention window, hour-aligned.
4. Cross-check that CreateApiKey, UpdateApiKey and ListApiKeys all report the same deletes for the same key.
