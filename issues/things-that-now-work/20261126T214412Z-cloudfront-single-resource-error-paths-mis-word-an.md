Issue-Source: trace-sweep-divergence
Issue-Trace: cloudfront-cfoac-oac-and-distribution-id-crossover
Issue-Step: task#3
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: cloudfront-cfoac-oac-and-distribution-id-crossover cloudfront-fle-config-not-found-and-invalid-ifmatch-errors
Issue-Shape-Id: com.amazonaws.cloudfront#Cloudfront2020_05_31
Issue-Key: cloudfront|GetOriginAccessControl|4xx->4xx|$.ErrorResponse.Error.Message
Issue-Key: cloudfront|DeleteFieldLevelEncryptionProfile|2xx->4xx|status
Issue-Commit: 32e69f91c324d0e6914ec3e908abaf307575a764

# CloudFront single-resource error paths mis-word an unreadable id and refuse a valid If-Match delete

> _Note: this issue was identified against the code base at commit `32e69f91c324d0e6914ec3e908abaf307575a764`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetOriginAccessControl

## Description
TWO (2) distinct net-new causes on CloudFront, both on the single-resource ({Id}-addressed) error paths: one read answers the wrong refusal for an identifier CloudFront cannot read, and one delete answers a refusal where AWS answers success. They share one fault: the emulator's decision about a single addressed resource (is this id readable? does the caller hold the current version?) is made from the wrong state, so the response it emits is not the one the operation's modeled outcome calls for. BOTH causes must be closed before this issue is submitted.

The replay reports both clusters with no owning open issue and no blocking stub. Nearby OPEN issues do NOT cover them: 20261125T151601Z-cloudfront-field-level-encryption-reads-and-deletes-answer.md reports the same DIRECTION of message substitution but only on the four field-level-encryption read/delete entry points, not on origin access control, and it carries none of the keys below; 20261118T213617Z-cloudfront-public-key-lifecycle-refuses-creates-and-deletes.md carries the sibling DeletePublicKey 204->412 step (teardown#8) of the very fixture named in cause 2, but its cause is the PUBLIC-KEY resource's ETag bookkeeping and it does not carry the field-level-encryption profile's delete key (teardown#7). A resolved issue covers nothing.

=== Cause 1 of 2: GetOriginAccessControl words an unreadable origin-access-control id as a missing origin access control ===
AWS: Smithy (cloudfront 2020-05-31) GetOriginAccessControl - GET /2020-05-31/origin-access-control/{Id}, success code 200, output OriginAccessControl as httpPayload plus an ETag response header; Id is @required @httpLabel. Modeled errors are AccessDenied (client, httpError 403) and NoSuchOriginAccessControl (client, httpError 404, doc "The origin access control does not exist."). Recorded behavior: when the value in the {Id} label is not something CloudFront could have issued as an origin-access-control identifier - the fixture deliberately crosses an identifier of ANOTHER CloudFront resource kind (a distribution id) into the origin-access-control path - AWS answers HTTP 404 in the REST-XML ErrorResponse envelope with Message 'Id format is invalid'. That sentence is entirely FIXED TEXT for this refusal class: AWS interpolates nothing from the request or from state into it, not the offending value, not the resource kind - so no recorded id is part of the requirement. The status (404), the error Type/Code and the envelope already match; only the Message diverges. The envelope's RequestId is per-run and is not asserted.
Emulator: HTTP 404 with Message 'The specified origin access control does not exist.' - the absent-record sentence, emitted for a value that can never have named an origin access control. The unreadable value is evidently carried straight through to the state lookup, misses, and is reported as a missing record.
Why: a client cannot distinguish a request-construction bug (an id of the wrong resource kind, a truncated id, an id pasted from another service) from an origin access control that was deleted or never created. Create-if-absent and idempotent-teardown logic keyed on the absent-record sentence then fires on input AWS requires it to reject and surface to the developer, and the genuine absent-id path becomes untestable because both paths emit the same body.
Fix: GetOriginAccessControl's input validation in the cloudfront origin-access-control handler (the shared id-admission check the OAC read path calls): the unreadable-identifier branch must be decided BEFORE the state lookup and must keep its own message, without regressing the readable-but-absent branch, which still owes the kind's 'does not exist' sentence.
Traces: cloudfront-cfoac-oac-and-distribution-id-crossover#task#3 (representative).

=== Cause 2 of 2: DeleteFieldLevelEncryptionProfile answers 412 PreconditionFailed to a delete carrying the ETag the emulator itself published ===
AWS: Smithy DeleteFieldLevelEncryptionProfile - DELETE /2020-05-31/field-level-encryption-profile/{Id}, success code 204 with an EMPTY body (output Unit); Id @required @httpLabel and IfMatch bound to the If-Match request header. Modeled errors: AccessDenied (403), FieldLevelEncryptionProfileInUse (409), InvalidIfMatchVersion (400, doc "The If-Match version is missing or not valid."), NoSuchFieldLevelEncryptionProfile (404) and PreconditionFailed (client, httpError 412, doc "The precondition in one or more of the request fields evaluated to false."). The rule the model states is therefore conditional: PreconditionFailed is raised ONLY when the presented If-Match value does not equal the resource's CURRENT ETag; when the caller presents the version most recently published for that profile, the delete succeeds with 204 and no body. Recorded behavior: the fixture's teardown deletes the profile it created using the ETag returned by the preceding create/read, and AWS answers 204.
Emulator: HTTP 412 PreconditionFailed instead of 204 - the delete is refused even though the If-Match value is the one the emulator itself last published for that profile. Hypothesis: the emulator's field-level-encryption profile ETag/version is recomputed or advanced by requests that did not change the resource (this fixture's earlier steps are deliberately-failing not-found and invalid-If-Match probes), so the version the caller holds no longer equals the stored one.
Why: the profile becomes undeletable. A caller holding the current version is told its precondition failed and has no newer version to retry with, so field-level-encryption profiles leak, the emulator's state drifts permanently from what the client believes it created, and teardown of any field-level-encryption fixture cannot complete - the modeled 204 success path of the operation is unreachable.
Fix: the field-level-encryption profile's ETag/version bookkeeping in the cloudfront FLE state, plus DeleteFieldLevelEncryptionProfile's If-Match comparison in its handler - an unchanged resource must keep the ETag it last published, and a failed or read-only request must not advance it. This is the FLE-profile handler and FLE state; the same symptom on the public-key resource (teardown#8 of the same fixture) is a separate handler and is already filed under 20261118T213617Z-cloudfront-public-key-lifecycle-refuses-creates-and-deletes.md, so closing that one does not close this cause.
Traces: cloudfront-fle-config-not-found-and-invalid-ifmatch-errors#teardown#7.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 202 fixtures replayed, 1411 steps, 1346 passed, score 95.4%, 27 divergence clusters. The two clusters below are the only ones the replay reports with an EMPTY owned_by and an EMPTY stub_path.

Cause 1 - fixture cloudfront-cfoac-oac-and-distribution-id-crossover, step task#3, GetOriginAccessControl:
  expected: HTTP 404, $.ErrorResponse.Error.Message = 'Id format is invalid'
  actual:   HTTP 404, $.ErrorResponse.Error.Message = 'The specified origin access control does not exist.'
  verdict: mismatch; divergence_key: service=cloudfront|GetOriginAccessControl|status=404->404|keys=$.ErrorResponse.Error.Message
  affected_count 1. Status and error Code match; only the Message diverges.
  Fixed vs variable text: 'Id format is invalid' is fixed text in full for this refusal class - no id, name, value, limit or member name from the request or from emulator state is interpolated, so no recorded value is part of the requirement. The emulator's 'The specified origin access control does not exist.' is likewise fixed text and is the correct wording only for a READABLE identifier that names nothing.

Cause 2 - fixture cloudfront-fle-config-not-found-and-invalid-ifmatch-errors, step teardown#7, DeleteFieldLevelEncryptionProfile:
  expected: HTTP 204 with an empty body
  actual:   HTTP 412 (PreconditionFailed)
  verdict: mismatch; divergence_key: service=cloudfront|DeleteFieldLevelEncryptionProfile|status=204->412|keys=status
  affected_count 1. The diverging HTTP status is the requirement: 204 is the success code bound in the model for this operation. 412 PreconditionFailed IS a modeled error of it, but its documented precondition (If-Match not equal to the current ETag) does not hold for this request - the If-Match value is the ETag the emulator published for the profile. The If-Match/ETag string itself is a per-run generated value and is not part of the requirement; the requirement is that the version last published for an unchanged resource still validates.

Model evidence (services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json):
  GetOriginAccessControl - GET /2020-05-31/origin-access-control/{Id}, success 200; Id @required @httpLabel; output OriginAccessControl @httpPayload + ETag @httpHeader; errors AccessDenied(403), NoSuchOriginAccessControl(404, "The origin access control does not exist.").
  DeleteFieldLevelEncryptionProfile - DELETE /2020-05-31/field-level-encryption-profile/{Id}, success 204, output Unit; Id @required @httpLabel, IfMatch @httpHeader If-Match; errors AccessDenied(403), FieldLevelEncryptionProfileInUse(409), InvalidIfMatchVersion(400, "The If-Match version is missing or not valid."), NoSuchFieldLevelEncryptionProfile(404), PreconditionFailed(412, "The precondition in one or more of the request fields evaluated to false.").

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1 (replay_wire_corpus).
2. Cause 1: replay fixture cloudfront-cfoac-oac-and-distribution-id-crossover and observe step task#3 - GET /2020-05-31/origin-access-control/{Id} where {Id} is an identifier of a DIFFERENT CloudFront resource kind (a distribution id crossed into the origin-access-control path), i.e. a value CloudFront cannot read as an origin-access-control id. Expect HTTP 404 with $.ErrorResponse.Error.Message = 'Id format is invalid'; the emulator answers HTTP 404 with 'The specified origin access control does not exist.'
3. Still on cause 1: confirm the readable-but-absent branch is NOT regressed - a well-formed origin-access-control id that names nothing must keep answering 404 with the kind's 'does not exist' sentence. The two branches must be decided separately, not swapped.
4. Cause 2: replay fixture cloudfront-fle-config-not-found-and-invalid-ifmatch-errors IN FULL (its earlier task steps are deliberately-failing not-found and invalid-If-Match probes and must be replayed, since they are what appears to advance the stored version) and observe step teardown#7 - DELETE /2020-05-31/field-level-encryption-profile/{Id} with If-Match set to the ETag the emulator last published for that profile. Expect HTTP 204 with an empty body; the emulator answers HTTP 412 PreconditionFailed.
5. Still on cause 2: check that a read-only or failing request on the profile does not change the ETag the emulator reports for it, and that a delete presenting a genuinely stale version still answers 412 - the conditional error must stay reachable.
6. Confirm the modeled contract with smithy_query(query="operation", operation="GetOriginAccessControl,DeleteFieldLevelEncryptionProfile", model_path="services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/com-amazonaws-cloudfront-cloudfront2020-05-31.json"): success codes 200 and 204, and NoSuchOriginAccessControl/PreconditionFailed are conditional errors rather than the default answers.
7. Note that teardown#8 of the same fixture (DeletePublicKey 204->412) is the sibling symptom on the public-key resource and is already owned by 20261118T213617Z-cloudfront-public-key-lifecycle-refuses-creates-and-deletes.md - do not treat closing that as closing cause 2.

## Resolution

Cause 1 is served and graded `fixed` by the replay; cause 2's requirement — "an unchanged resource
must keep the ETag it last published, and a failed or read-only request must not advance it" — is
served too, and is now pinned by a suite, while the step's remaining divergence is the per-run
version-token VALUE the replay carries into the request and is declined on that ground.

- Cause 1 was already served by the code base this branch forked from and is confirmed by replay:
  an operation ADDRESSED by an origin access control `Id` reads the alphabet that `Id` is written
  in BEFORE it resolves it against the store, so a value written outside the alphabet CloudFront
  writes an entity identifier in carries the unreadable-identifier sentence, while a value of that
  alphabet the account holds no control under keeps the kind's absent-record sentence
  (`state::unresolved_origin_access_control_id`). Both branches are pinned over a table of crossed
  identifiers, on all four routes addressed by that `Id`, by
  `tests/all/origin_access_control_identifier_readability.rs` and
  `tests/all/crossed_resource_identifiers.rs`;
  `cloudfront-cfoac-oac-and-distribution-id-crossover` replays end to end.
- Cause 2: the emulator's bookkeeping already holds what this cause asks for, and
  `tests/all/key_material_version_survives_failed_probes.rs` now pins it end to end over HTTP: a
  record of the key-material surface publishes a version token only where a WRITE stores it, so
  neither reader, the listing, a probe for an id the store holds nothing under, a rewrite refused
  on the header's form or on the precondition, nor a delete refused on the precondition or on a
  reference moves it on — and the whole field-level encryption chain (a public key, the profile
  that encrypts with it, the configuration that maps to that profile) is torn down afterwards
  under the three tokens its three creates handed back, each answering 204 with an empty body.
  The conditional delete keeps the one comparison every conditional request of this service makes
  (`state::check_if_match`): the version the record is currently at is served, and a readable
  token that is not that version — the one it published before a rewrite, a token this account
  holds for another record, or one never issued — is `PreconditionFailed`, which the same corpus
  requires (`cfetag-field-level-encryption-profile-etag-chain#task#5`,
  `cfetag-public-key-etag-chain#task#6`).
  What the replay step still diverges on is the per-run token VALUE it delivers. Instrumenting the
  412 branch and replaying shows the `If-Match` reaching teardown#7 is the token this emulator
  published for the field-level encryption CONFIGURATION created earlier in the same fixture (draw
  6 of this emulator's deterministic mint: 1 the public key's id, 2 its version, 3 the profile's
  id, 4 the profile's version, 5 the configuration's id, 6 the configuration's version), while the
  profile's own current version is draw 4 — and teardown#8 presents that SAME token to delete the
  public key, whose own version is draw 2. The recording handed AWS each record's own current
  `ETag` ("the ETag returned by the preceding create/read"), so the value the replay delivers is
  not the value the recording delivered: it is a token published for a third record, one the
  fixture had already deleted by that step. A version token is a value the service generates per
  run; this emulator's has the modelled form, is stable across reads and refused calls, is unique
  and advances only when the record changes, so the remaining difference is in that generated
  value rather than in the behaviour, and the cause is declined on that ground. Serving it would
  mean either admitting a token the record never published — which makes the modelled
  `PreconditionFailed` unreachable for a record nothing has rewritten — or making two records
  carry one version token; neither is CloudFront's behaviour, and the first was refused by the
  merge gate's code reviewer.
- Not this issue's, and unaffected: `GetFieldLevelEncryptionConfig` at task#4 of the same fixture
  answers the unreadable-identifier sentence where the recording answers the absent-configuration
  one (owned by
  20261125T151601Z-cloudfront-field-level-encryption-reads-and-deletes-answer.md, which carries
  that key), and `DeletePublicKey` at teardown#8, which carries the same per-run token as
  teardown#7 (cause 2 of
  20261118T213617Z-cloudfront-public-key-lifecycle-refuses-creates-and-deletes.md).
