Issue-Source: trace-sweep-divergence
Issue-Trace: personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected
Issue-Step: task#2
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected personalize-runtime-personalized-ranking-foreign-region-and-partial-arns-rejected personalize-runtime-recommendations-foreign-region-and-partial-arns-rejected
Issue-Shape-Id: com.amazonaws.personalizeruntime#AmazonPersonalizeRuntime
Issue-Key: personalize-runtime|GetActionRecommendations|4xx->4xx|$.message
Issue-Key: personalize-runtime|GetPersonalizedRanking|4xx->4xx|$.message
Issue-Key: personalize-runtime|GetRecommendations|4xx->4xx|$.message
Issue-Commit: 7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21

# personalize-runtime reports a malformed or foreign ARN as unresolvable instead of invalid, on all three inference operations

> _Note: this issue was identified against the code base at commit `7bfd99e3999e8b9fa7ee2b4f0c70a66ae974be21`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetActionRecommendations

## Description
THREE net-new causes, one shared fault. Amazon Personalize Runtime has two distinct refusals for an ARN-carrying member, and both are InvalidInputException / HTTP 400, so the MESSAGE is the only observable that separates them: an ARN that is not a well-formed reference at all is refused as INVALID ("The given ARN is invalid: <arn>" / "The given <kind> ARN is invalid: <arn>"), while a well-formed reference of the right resource type that the service does not hold, or that has not reached ACTIVE, is refused as UNRESOLVABLE ("<arn> does not exist or is not active yet."). This emulator decides which of the two applies from the ARN's RESOURCE TYPE ONLY. Any ARN whose resource part merely starts with the right type token therefore passes the validity gate and falls through to the existence lookup, so a PARTIAL ARN (one whose resource id after '<type>/' is empty) and an ARN naming a region or account other than the one serving the request are both reported as if the referenced resource were simply absent or still being created. The two refusals are not interchangeable: only the unresolvable one describes a condition that can change by itself, and the crate's own state module says so (services/service-com-amazonaws-personalizeruntime-amazonpersonalizeruntime/src/state.rs:17-36, :689-699). The three causes below are the same classification bug reached through each operation's own ARN member, and each operation's expected invalid-ARN wording differs, so each needs its own message.

Cause 1 of 3 — GetActionRecommendations (fixture personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected, step task#2).
AWS: Smithy: GetActionRecommendations.campaignArn targets Arn with @length max 256 and @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$; modeled errors InvalidInputException (httpError 400) and ResourceNotFoundException (404). Note the modeled pattern does NOT reject these ARNs — a trailing-slash partial ARN satisfies the final '.+' and a foreign region satisfies the '.*' region segment — so the service's own ARN validation is what refuses them, not constraint validation. Recorded behavior: HTTP 400 with message "The given campaign ARN is invalid: <campaignArn>". Fixed text: "The given campaign ARN is invalid: "; variable part: the campaignArn value exactly as the request sent it. This operation uses the KIND-QUALIFIED wording ("campaign ARN").
Emulator: HTTP 400 (status and error code match) but the message is the unresolvable one, "<campaignArn> does not exist or is not active yet." — i.e. the request is reported as naming a campaign that is absent or not yet ACTIVE.
Why: a client cannot tell a request it must FIX from a resource it should WAIT for. Retry/poll logic keyed on "does not exist or is not active yet" will loop forever on an ARN that can never resolve, and a client that surfaces the message to a user blames provisioning for a malformed ARN it passed in.
Fix: the ARN-resolution seam for campaignArn in GetActionRecommendations' validation path — the validity gate must reject an ARN that is not a complete reference for the serving partition/region/account BEFORE the registry existence lookup runs, rather than testing the resource-type token alone.
Traces: personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected#task#2, #task#3.

Cause 2 of 3 — GetPersonalizedRanking (fixture personalize-runtime-personalized-ranking-foreign-region-and-partial-arns-rejected, step task#2).
AWS: Smithy: GetPersonalizedRanking.campaignArn is @required, targets Arn with @length max 256 and @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$; modeled errors InvalidInputException (400), ResourceNotFoundException (404). Recorded behavior: HTTP 400 with message "The given ARN is invalid: <campaignArn>". Fixed text: "The given ARN is invalid: "; variable part: the ARN value from the request. This operation uses the UNQUALIFIED wording — no resource kind in the message — which is where it differs from Cause 1.
Emulator: HTTP 400 with the unresolvable message "<campaignArn> does not exist or is not active yet." instead.
Why: same as Cause 1 — a permanent request defect is reported as a transient provisioning state; additionally, a client matching on the documented invalid-ARN text to validate user input never sees it here.
Fix: the ARN-resolution seam for campaignArn/filterArn in GetPersonalizedRanking's validation path, which must emit the unqualified invalid-ARN message for an ARN that is not a complete reference, before any existence lookup.
Traces: personalize-runtime-personalized-ranking-foreign-region-and-partial-arns-rejected#task#2, #task#3.

Cause 3 of 3 — GetRecommendations (fixture personalize-runtime-recommendations-foreign-region-and-partial-arns-rejected, step task#3).
AWS: Smithy: GetRecommendations.campaignArn and recommenderArn (and filterArn) all target Arn with @length max 256 and @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$; modeled errors InvalidInputException (400), ResourceNotFoundException (404). Recorded behavior: HTTP 400 with message "The given ARN is invalid: <arn>" — fixed text "The given ARN is invalid: " plus the request's ARN value, unqualified by kind.
Emulator: HTTP 400 with the unresolvable message "<arn> does not exist or is not active yet." instead. This cluster covers three recorded steps, so every ARN-carrying member GetRecommendations accepts is affected the same way.
Why: same as above, and worse for GetRecommendations because the caller chooses between campaignArn and recommenderArn: a partial or foreign ARN in either member is reported as a missing resource, so the caller has no signal that the reference itself is wrong.
Fix: the ARN-resolution seam shared by GetRecommendations' campaignArn/recommenderArn/filterArn validation, same gate as Causes 1 and 2.
Traces: personalize-runtime-recommendations-foreign-region-and-partial-arns-rejected#task#3, #task#4, #task#5.

Related but NOT covering this: open issue 20261120T210359Z-personalize-runtime-getrecommendations-validation--r2 ("The registration seams never apply the crate's own require_resource_kind...") is about the WRITE side storing a resource under an ARN of another resource type. This issue is about the READ side's validity classification being too weak — the resource-type test passes, so a malformed or foreign ARN is never classified invalid at all. Fixing the seams does not change any of the three messages above.

## Evidence
Replay of trace set simparity-wire-aws-v1: 25 steps, 18 passed, 7 mismatched, score 72.0%. All 7 mismatched steps are these 3 clusters; status and error code match in every one, only $.message diverges.

Cause 1 — fixture personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected, step task#2, GetActionRecommendations, expected 400 / actual 400:
  $.message value_mismatch
  expected: 'The given campaign ARN is invalid: arn:aws:personalize:us-west-2:000000000000:campaign/'
  actual:   'arn:aws:personalize:us-west-2:000000000000:campaign/ does not exist or is not active yet.'
  cluster also covers: personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected#task#3
  divergence_key: service=personalize-runtime|GetActionRecommendations|status=400->400|keys=$.message

Cause 2 — fixture personalize-runtime-personalized-ranking-foreign-region-and-partial-arns-rejected, step task#2, GetPersonalizedRanking, expected 400 / actual 400:
  $.message value_mismatch
  expected: 'The given ARN is invalid: arn:aws:personalize:us-west-2:000000000000:campaign/'
  actual:   'arn:aws:personalize:us-west-2:000000000000:campaign/ does not exist or is not active yet.'
  cluster also covers: personalize-runtime-personalized-ranking-foreign-region-and-partial-arns-rejected#task#3
  divergence_key: service=personalize-runtime|GetPersonalizedRanking|status=400->400|keys=$.message

Cause 3 — fixture personalize-runtime-recommendations-foreign-region-and-partial-arns-rejected, step task#3, GetRecommendations, expected 400 / actual 400:
  $.message value_mismatch
  expected: 'The given ARN is invalid: arn:aws:personalize:us-west-2:000000000000:campaign/'
  actual:   'arn:aws:personalize:us-west-2:000000000000:campaign/ does not exist or is not active yet.'
  cluster also covers: personalize-runtime-recommendations-foreign-region-and-partial-arns-rejected#task#4, #task#5
  divergence_key: service=personalize-runtime|GetRecommendations|status=400->400|keys=$.message

The ARN value shown is the recorded run's; it is NOT a requirement. The requirement is the message TEMPLATE: the fixed text ('The given campaign ARN is invalid: ' for GetActionRecommendations, 'The given ARN is invalid: ' for the other two) followed by the ARN exactly as the request supplied it. The emulator's wording is its own unresolvable template ('<arn> does not exist or is not active yet.'), which the crate reserves for a well-formed reference of the right kind that the registry does not hold (src/state.rs:23-32).

Why the modeled constraints do not explain it: 'arn:aws:personalize:us-west-2:000000000000:campaign/' satisfies Arn's @pattern ^arn:([a-z\d-]+):personalize:.*:.*:.+$ (the resource segment 'campaign/' matches '.+') and is well under @length max 256, so constraint validation cannot be the gate — the service-level ARN validity check is. The emulator's own gate tests only the resource-type token (src/state.rs:739-753, require_resource_kind -> arn::resource_type(arn) == kind), which 'campaign/' and a foreign-region campaign ARN both satisfy, so both fall through to the existence lookup (src/state.rs:604-609) and are answered with Denied::unresolvable.

No cluster carried a stub_path or an owned_by, so none of these is already filed.

## Steps to Reproduce
1. Boot the emulator from this checkout and ensure the personalize-runtime data plane is serving (/__admin/health answered 200 during replay).
2. Register a campaign through the Personalize control-plane seam so the registry is non-empty and the refusal cannot be confused with an empty-state artifact.
3. Cause 3 — POST /recommendations with campaignArn set to a PARTIAL Personalize campaign ARN, i.e. one whose resource segment is the type token with an empty id ('...:campaign/'), then repeat with an ARN naming a region other than the one serving the request. Observe HTTP 400 InvalidInputException with message '<arn> does not exist or is not active yet.'; AWS answers HTTP 400 InvalidInputException 'The given ARN is invalid: <arn>'. Replay personalize-runtime-recommendations-foreign-region-and-partial-arns-rejected steps task#3, task#4, task#5.
4. Cause 2 — POST /personalize-ranking with the same partial and foreign-region campaignArn values, plus the required inputList and userId. Observe the same unresolvable message; AWS answers 'The given ARN is invalid: <arn>'. Replay personalize-runtime-personalized-ranking-foreign-region-and-partial-arns-rejected steps task#2, task#3.
5. Cause 1 — POST /action-recommendations with the same partial and foreign-region campaignArn values. Observe the same unresolvable message; AWS answers the KIND-QUALIFIED 'The given campaign ARN is invalid: <arn>'. Replay personalize-runtime-action-recommendations-foreign-region-and-partial-arns-rejected steps task#2, task#3.
6. Contrast with the refusal that IS correct today: a COMPLETE, correctly-typed campaign ARN for the serving region that was never registered must still answer '<arn> does not exist or is not active yet.' — that path must not regress while the invalid-ARN path is fixed.
7. Re-run replay_wire_corpus(); all 7 mismatched steps across the three fixtures must pass.
