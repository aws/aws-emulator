Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: gameliftstreams.associate-applications-over-fifty-identifiers-rejected gameliftstreams.create-application-log-paths-without-output-uri-rejected gameliftstreams.create-stream-url-location-outside-stream-group-rejected gameliftstreams.delete-stream-group-while-activating-rejected gameliftstreams.disassociate-default-application-promotes-remaining-one gameliftstreams.list-stream-groups-unissued-next-token-rejected gameliftstreams.list-stream-urls-foreign-next-token-rejected gameliftstreams.list-stream-urls-lists-new-urls-and-follows-status gameliftstreams.revoke-stream-url-refuses-malformed-input-and-reports-unknown-url gameliftstreams.start-stream-session-client-token-reuse-with-changed-input-rejected gameliftstreams.start-stream-session-location-outside-stream-group-rejected gameliftstreams.start-stream-session-without-client-connection-unassociated-application-rejected gameliftstreams.stream-url-revoked-by-stream-group-deletion gameliftstreams.terminated-stream-session-reports-api-terminated-and-leaves-active-listing gameliftstreams.update-stream-group-capacity-not-tenancy-multiple-rejected gameliftstreams.update-stream-group-while-activating-rejected
Issue-Shape-Id: com.amazonaws.gameliftstreams#GameLiftStreams
Issue-Key: gameliftstreams.associate-applications-over-fifty-identifiers-rejected|run#11|AssertionError|assert _APP_ID.fullmatch(app_id), f"CreateApplication returned Id {app_id!r}"
Issue-Key: gameliftstreams.create-application-log-paths-without-output-uri-rejected|run#12|AssertionError|assert (code, _trim(message)) == (
Issue-Key: gameliftstreams.create-stream-url-location-outside-stream-group-rejected|run#20|AssertionError|assert message.startswith("Locations do not exist in stream group '"), (
Issue-Key: gameliftstreams.delete-stream-group-while-activating-rejected|_code_and_message#9|AssertionError|raise AssertionError(f"DeleteStreamGroup was expected to be refused but answered {answer!r}")
Issue-Key: gameliftstreams.disassociate-default-application-promotes-remaining-one|_application#8|AssertionError|assert _APP_ID.fullmatch(app_id), f"CreateApplication returned Id {app_id!r}"
Issue-Key: gameliftstreams.list-stream-groups-unissued-next-token-rejected|_refusal#12|AssertionError|raise AssertionError(f"the call was expected to be refused but answered {answer!r}")
Issue-Key: gameliftstreams.list-stream-urls-foreign-next-token-rejected|_refusal#12|AssertionError|raise AssertionError(f"expected the token to be refused, but the call returned {result!r}")
Issue-Key: gameliftstreams.list-stream-urls-lists-new-urls-and-follows-status|_wait_for#30|Timeout|time.sleep(pause)
Issue-Key: gameliftstreams.revoke-stream-url-refuses-malformed-input-and-reports-unknown-url|_shaped#2|AssertionError|assert isinstance(value, str) and re.fullmatch(pattern, value), (
Issue-Key: gameliftstreams.start-stream-session-client-token-reuse-with-changed-input-rejected|_shaped#3|AssertionError|assert isinstance(value, str) and re.fullmatch(pattern, value), (
Issue-Key: gameliftstreams.start-stream-session-location-outside-stream-group-rejected|_shaped#3|AssertionError|assert isinstance(value, str) and re.fullmatch(pattern, value), (
Issue-Key: gameliftstreams.start-stream-session-without-client-connection-unassociated-application-rejected|_shaped#3|AssertionError|assert isinstance(value, str) and re.fullmatch(pattern, value), (
Issue-Key: gameliftstreams.stream-url-revoked-by-stream-group-deletion|_wait#18|AssertionError|raise AssertionError(f"{what} did not happen in {budget_s:.0f}s; last read {seen!r}")
Issue-Key: gameliftstreams.terminated-stream-session-reports-api-terminated-and-leaves-active-listing|_shaped#3|AssertionError|assert isinstance(value, str) and re.fullmatch(pattern, value), (
Issue-Key: gameliftstreams.update-stream-group-capacity-not-tenancy-multiple-rejected|_rejection#11|AssertionError|raise AssertionError(f"the capacity update was accepted: {reply!r}")
Issue-Key: gameliftstreams.update-stream-group-while-activating-rejected|run#10|AssertionError|assert got == expected, f"update during activation was refused with {got!r}, expected {expected!r}"
Issue-Commit: 51355cfc1cfbb5e6f009c7f9575dfb9e65707db6

# gameliftstreams: 8 control-plane fidelity defects — 6-character resource ids, missing ACTIVATING delete guard, wrong ACTIVATING update error class, unvalidated NextToken, missing capacity-multiple validation, two wrong refusal messages, and stream URLs destroyed with their stream group

> _Note: this issue was identified against the code base at commit `51355cfc1cfbb5e6f009c7f9575dfb9e65707db6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateApplication, CreateStreamGroup, CreateStreamUrl, DeleteStreamGroup, UpdateStreamGroup, ListStreamGroups, ListStreamUrls, StartStreamSession

## Description
Eight independent root causes in the gameliftstreams emulator, all grounded in failing checks of the behavioral replay (gameliftstreams 41/72 passed, 31 failed). Each cause below must be closed before this issue is submitted; the checks that ground each one are named on its own Traces line.

=== Cause 1 of 8: minted application / stream-group / stream-URL ids are 6 characters long, where AWS ids carry a 9-character suffix ===
AWS: a GameLift Streams resource id is its prefix plus NINE characters. The Smithy model documents the form on every Identifier member, e.g. DeleteStreamGroup.Identifier: "Example ARN: arn:aws:gameliftstreams:us-west-2:111122223333:streamgroup/sg-1AB2C3De4. Example ID: sg-1AB2C3De4" — sg- plus 9 alphanumerics; applications are a- plus 9 (shape com.amazonaws.gameliftstreams#Id, length 1..32, pattern ^[a-zA-Z0-9-]+$, so the 9-character body is recorded/documented behavior, not a model constraint). The replayed checks encode exactly that: a-[A-Za-z0-9]{9} and sg-[A-Za-z0-9]{9}.
Emulator: ids come back with a 6-character body — CreateApplication answered Id 'a-ArahX1', 'a-CG5rS1', 'a-mQUZs1', 'a-hkcGd1', 'a-QTX641', 'a-0F5f41' and CreateStreamGroup answered Id 'sg-pJok61' (and Arn .../streamgroup/sg-3SACO1). The mint is 5 pseudo-random characters plus a base62 store counter (state.rs, ComAmazonawsGameliftstreamsGameliftstreamsState::mint_id), so the body is 5+N characters and only reaches 9 once the counter is 4 digits wide.
Why: every client, template and test that parses or validates a GameLift Streams id (a regex on a-/sg- ids, a fixed-width column, a CloudFormation/IaC assertion, log scraping) accepts the emulator's value and rejects — or mis-slices — AWS's, and vice versa. It is also the first assertion many checks make, so it masks everything downstream of resource creation.
Fix: the id mint in the service state (state.rs, mint_id) — produce a 9-character body for application, stream-group, stream-session and stream-URL ids while keeping intra-store uniqueness.
Traces: gameliftstreams.associate-applications-over-fifty-identifiers-rejected, gameliftstreams.disassociate-default-application-promotes-remaining-one, gameliftstreams.revoke-stream-url-refuses-malformed-input-and-reports-unknown-url, gameliftstreams.start-stream-session-client-token-reuse-with-changed-input-rejected, gameliftstreams.start-stream-session-location-outside-stream-group-rejected, gameliftstreams.start-stream-session-without-client-connection-unassociated-application-rejected, gameliftstreams.terminated-stream-session-reports-api-terminated-and-leaves-active-listing.

=== Cause 2 of 8: CreateApplication's ApplicationLogPaths-without-ApplicationLogOutputUri refusal is worded by the emulator, not as AWS words it ===
AWS: CreateApplication (Smithy: POST /applications, success 201, ValidationException httpError 400; ApplicationLogPaths is a FilePaths list length 0..10, ApplicationLogOutputUri length 0..1024 with pattern ^$|^s3://...) refuses the pair being set independently with ValidationException/HTTP 400 and the fixed message "'ApplicationLogOutputUri' and 'ApplicationLogPaths' cannot be set independently, they must both be specified or both be empty" — the sentence is entirely fixed text (it names the two MODEL member names and no request value), followed by this service's per-request " (Internal ID: <uuid>)" suffix, whose uuid is per-request and not a requirement.
Emulator: the code is right (ValidationException) but the message is its own: "ApplicationLogOutputUri is required when ApplicationLogPaths is set (1 path(s) given) (Internal ID: e7edeff7-...)" — different wording, and it interpolates the number of paths from the request, a value AWS never puts in this message.
Why: the refusal is asymmetric in the emulator (it reads as "the output URI is missing") where AWS states the symmetric rule, and a client or test that matches on the documented sentence does not match it. Message text is part of the contract for an error class.
Fix: the CreateApplication handler's log-configuration validation (ops/create_application.rs) — word the refusal exactly as above and drop the path count.
Traces: gameliftstreams.create-application-log-paths-without-output-uri-rejected.

=== Cause 3 of 8: the "location is not in the stream group" refusal is worded by the emulator and names the group's bare id instead of its ARN ===
AWS: CreateStreamUrl (POST /streamgroups/{Identifier}/streamurls, 201, ValidationException 400; Locations is a required @uniqueItems LocationList) and StartStreamSession refuse a Locations entry the addressed stream group does not hold with ValidationException/HTTP 400 and the message "Locations do not exist in stream group '<stream-group ARN>': <location names>". Fixed text: "Locations do not exist in stream group '" … "': "; variable parts: the full ARN of the stream group the request's Identifier resolves to (in single quotes), then the offending location name(s) from the request. Plus the service's " (Internal ID: <uuid>)" suffix.
Emulator: ValidationException with "stream group sg-KQEJr2 does not hold the requested Locations: us-east-1" — different sentence, and the group is named by bare id rather than by the resolved ARN (this service names resources by resolved ARN everywhere else).
Why: a client that reads the ARN out of the message to report which group refused the placement gets a bare id (or nothing), and any assertion on the documented wording fails. The same wording is required from both operations, so the two must not drift.
Fix: one shared refusal builder beside the other message builders in state.rs, used by the CreateStreamUrl and StartStreamSession handlers (ops/create_stream_url.rs, ops/start_stream_session.rs) in place of their local sentences.
Traces: gameliftstreams.create-stream-url-location-outside-stream-group-rejected.

=== Cause 4 of 8: DeleteStreamGroup accepts a delete of a stream group that is still ACTIVATING ===
AWS: a stream group may only be deleted once it has settled; a DeleteStreamGroup issued while the group is ACTIVATING is refused with ValidationException/HTTP 400 and the message "Stream group '<stream-group ARN>' is in ACTIVATING status" (fixed text "Stream group '" … "' is in " … " status"; variable parts: the resolved stream-group ARN and the group's current status), plus the " (Internal ID: <uuid>)" suffix. The group then survives the refusal and reaches ACTIVE, where the identical delete is accepted. DeleteStreamGroup models both ValidationException (400) and ConflictException (409); the recorded refusal of this state is the ValidationException above.
Emulator: the call SUCCEEDS — HTTP 204 with an empty body — and the activating group is moved straight to DELETING: ops/delete_stream_group.rs has no status guard at all (it only short-circuits a group already DELETING).
Why: a client that relies on the guard to protect a group it is still provisioning destroys it instead of getting a 400, and nothing surfaces the race; CloudFormation/IaC rollback paths that expect the refusal diverge.
Fix: a status gate in the DeleteStreamGroup handler (ops/delete_stream_group.rs), before any state is written, refusing any non-settled status with the ValidationException worded above.
Traces: gameliftstreams.delete-stream-group-while-activating-rejected.

=== Cause 5 of 8: UpdateStreamGroup on an ACTIVATING group refuses with ConflictException and its own message, where AWS answers ValidationException ===
AWS: UpdateStreamGroup (PATCH /streamgroups/{Identifier}, 200) on a group that is not yet settled is refused with ValidationException/HTTP 400 and the SAME sentence as Cause 4: "Stream group '<stream-group ARN>' is in ACTIVATING status" (fixed text as above; variable parts the resolved ARN and the current status), plus the internal-id suffix. The refused update must leave Description untouched, and the identical update must be accepted once the group is ACTIVE.
Emulator: answers ConflictException (HTTP 409) with "stream group sg-pdSzk1 is in ACTIVATING status; UpdateStreamGroup requires ACTIVE status" (ops/update_stream_group.rs, the group.status != ACTIVE gate). Both the error class/status and the message differ, and the group is named by bare id rather than resolved ARN.
Why: a client branching on the error code takes the wrong branch — ConflictException is commonly treated as retry-after-settling while ValidationException is surfaced to the user — and SDK retry/error-classification behavior differs between 409 and 400.
Fix: the UpdateStreamGroup handler's status gate (ops/update_stream_group.rs) — raise the ValidationException worded above (shared with Cause 4's builder), not ConflictException.
Traces: gameliftstreams.update-stream-group-while-activating-rejected.

=== Cause 6 of 8: NextToken is a plain resource id and is never checked against the tokens the operation issued, so a forged or foreign token is served instead of refused ===
AWS: ListStreamGroups and ListStreamUrls are @paginated with an opaque NextToken (Smithy: NextToken is a plain string bound to the NextToken query parameter; both operations model ValidationException httpError 400). A NextToken value the operation never issued — a forged opaque-looking string, or a token another operation (ListApplications) issued and still honours — is refused with ValidationException/HTTP 400 and the fixed message "'NextToken' pagination value does not match a previously returned value from the same operation" (no variable parts), plus the " (Internal ID: <uuid>)" suffix; a genuine token keeps working after the refusal.
Emulator: both calls are SERVED with HTTP 200. ListStreamGroups(MaxResults=1, NextToken="c2ltMnJlYWwtbmV2ZXItaXNzdWVkLWNvbnRpbnVhdGlvbg==") returned a first page plus NextToken='sg-3SACO1'; ListStreamUrls(MaxResults=1, NextToken=<a ListApplications token>) returned {'Items': []}. The cause is the shared pager: state.rs page_of_ids treats NextToken as the last id of the previous page ("after") and the handlers pass any string through, so the token is neither opaque nor issued-checked — and a token from another operation silently filters the result set.
Why: pagination loses its only integrity check. A client that crosses tokens between operations (or replays a stale/forged one) gets a silently truncated or empty listing from the emulator where AWS refuses it, so a paging bug passes here and loses data in production; the leaked token also exposes the internal id ordering.
Fix: issue opaque tokens and record them per operation in service state, and validate on input in the shared pager (state.rs page_of_ids) plus the List* handlers (ops/list_stream_groups.rs, ops/list_stream_urls.rs, and the other List* operations that share the pager), refusing an unrecognised token with the ValidationException worded above.
Traces: gameliftstreams.list-stream-groups-unissued-next-token-rejected, gameliftstreams.list-stream-urls-foreign-next-token-rejected.

=== Cause 7 of 8: UpdateStreamGroup accepts a capacity that is not a multiple of the stream class's streams-per-GPU tenancy ===
AWS: a stream class that runs more than one stream per GPU requires every capacity value for a location to be a multiple of that tenancy (gen4n_high runs two streams per GPU, so capacities must be even). An UpdateStreamGroup whose LocationConfigurations carries an odd MaximumCapacity on such a class is refused with ValidationException/HTTP 400 and the message "Capacity values for location '<LocationName>' must be multiples of <streams-per-GPU of the group's StreamClass>" (fixed text "Capacity values for location '" … "' must be multiples of " …; variable parts: the LocationName from the request and the tenancy of the stored StreamClass), plus the internal-id suffix — and the location keeps its previous capacity. The even neighbour is accepted and read back. (Model: UpdateStreamGroup models ValidationException 400; LocationConfiguration's capacity members are only @range(min: 0), so the multiple rule itself is recorded behavior.)
Emulator: the update is ACCEPTED with HTTP 200 and the odd value is stored — reply showed LocationStates [{'LocationName': 'us-west-2', 'MaximumCapacity': 1, ...}] on a gen4n_high group. The shared validator state.rs unstorable_location_configuration only rejects negative capacities; no tenancy-multiple rule exists on any path.
Why: capacity planning silently diverges — the emulator reports a capacity AWS would never grant, so a client that sizes a group from the emulator's answer over-commits (or sees capacity AWS refuses to provision) and never learns its request was invalid.
Fix: the shared location-configuration validation (state.rs unstorable_location_configuration, which already serves CreateStreamGroup/AddStreamGroupLocations/UpdateStreamGroup) — derive the tenancy from the group's StreamClass and refuse non-multiples with the message above, before any capacity is written.
Traces: gameliftstreams.update-stream-group-capacity-not-tenancy-multiple-rejected.

=== Cause 8 of 8: deleting a stream group destroys its stream URLs instead of leaving them listed as REVOKED/streamGroupDeleted ===
AWS: "if you delete the stream group or application that backs the stream URL, this operation updates the status of the stream URL to REVOKED" (GetStreamUrl documentation) and StreamUrlStatusReason declares streamGroupDeleted: "The stream group was deleted, which revoked the stream URL". So after the group is gone, each of its stream URLs is still reported by ListStreamUrls with Status=REVOKED, StatusReason="streamGroupDeleted", and still names its StreamGroupArn, ApplicationArn and UsageLimit (Smithy: ListStreamUrls GET /streamurls, 200, @paginated, optional Status and StreamGroupIdentifier query filters; StreamUrlStatus enum ACTIVE/EXPIRED/REVOKED/LIMIT_REACHED).
Emulator: once the delete completes the URLs VANISH — the check's listing of the deleted group's two URLs reads [] for the whole 300 s budget ("revocation of both of the deleted group's stream URLs did not happen in 24s; last read []"). The records live inside the stream-group record (state.rs StreamGroup::stream_urls) and state.rs materialize drops the whole group with retain(|_, group| !group.advance(now)), taking its stream URLs with it; the revoke_for(streamGroupDeleted) that StreamGroup::advance applies while the status is DELETING is therefore only observable during the short DELETING window and is lost when the record is reaped.
Why: a client that audits which URLs were invalidated by a group deletion, or that reads a URL's final status/StatusReason to explain to end users why a link stopped working, sees nothing at all instead of a REVOKED record; a "was it revoked or did it never exist" distinction becomes unanswerable.
Fix: retain stream-URL records independently of their stream group's record (move them out of the group record, or keep a revoked-URL store the reaper does not touch) and have the ListStreamUrls/GetStreamUrl read path report them with Status REVOKED and StatusReason streamGroupDeleted, including under the Status filter and with no StreamGroupIdentifier filter (state.rs materialize/StreamGroup::advance, ops/list_stream_urls.rs, ops/get_stream_url.rs).
Hypothesis: the same loss is what hangs gameliftstreams.list-stream-urls-lists-new-urls-and-follows-status, whose final stage waits for the surviving URL to read REVOKED after its stream group is deleted; that check ends as "Check timed out after 60.0s" and the step it was in is not named in the output, so re-replay it after this cause is fixed.
Traces: gameliftstreams.stream-url-revoked-by-stream-group-deletion, gameliftstreams.list-stream-urls-lists-new-urls-and-follows-status.

## Evidence
Suite: simparity-behavioral-aws-v1 — Total 41/74 (55.4%); gameliftstreams 41/72 (57%), 31 failed. Checks already covered by filed stub issues (ExportStreamSessionFiles, UpdateApplication, ListStreamSessions, ListStreamSessionsByAccount) and the cloudformation__gameliftstreams checks owned by 20261109T204356Z-cloudformation-gameliftstreams-aws-gameliftstreams-streamgroup-is-not-a.md are excluded here.

Cause 1 — id shape:
  expected: a-[A-Za-z0-9]{9} / sg-[A-Za-z0-9]{9} (model doc: "Example ID: sg-1AB2C3De4")
  actual:   AssertionError: CreateApplication returned Id 'a-ArahX1' (also 'a-CG5rS1', 'a-mQUZs1', 'a-hkcGd1', 'a-QTX641', 'a-0F5f41'); "the stream group id 'sg-pJok61' does not match sg-[A-Za-z0-9]{9}"; listing showed Arn '...:streamgroup/sg-3SACO1'
  source:   state.rs mint_id = prefix + 5 random chars + base62 mint counter

Cause 2 — CreateApplication log paths:
  expected: ('ValidationException', "'ApplicationLogOutputUri' and 'ApplicationLogPaths' cannot be set independently, they must both be specified or both be empty")
  actual:   ('ValidationException', 'ApplicationLogOutputUri is required when ApplicationLogPaths is set (1 path(s) given) (Internal ID: e7edeff7-109a-e1e1-9048-d749eec7d9f2)')

Cause 3 — Locations outside the stream group:
  expected: ValidationException, message starting "Locations do not exist in stream group '" and ending "': us-east-1"
  actual:   'stream group sg-KQEJr2 does not hold the requested Locations: us-east-1'

Cause 4 — DeleteStreamGroup while ACTIVATING:
  expected: ('ValidationException', "Stream group '<group-arn>' is in ACTIVATING status")
  actual:   HTTP 204 success — AssertionError: DeleteStreamGroup was expected to be refused but answered {'ResponseMetadata': {'HTTPStatusCode': 204, ...}}
  source:   ops/delete_stream_group.rs handle() has no status guard

Cause 5 — UpdateStreamGroup while ACTIVATING:
  expected: ('ValidationException', "Stream group 'arn:aws:gameliftstreams:us-west-2:000000000000:streamgroup/sg-pdSzk1' is in ACTIVATING status")
  actual:   ('ConflictException', 'stream group sg-pdSzk1 is in ACTIVATING status; UpdateStreamGroup requires ACTIVE status')  [HTTP 409]
  source:   ops/update_stream_group.rs ~line 352: group.status != ACTIVE -> ConflictException

Cause 6 — NextToken:
  expected: ('ValidationException', "'NextToken' pagination value does not match a previously returned value from the same operation")
  actual (ListStreamGroups, forged token 'c2ltMnJlYWwtbmV2ZXItaXNzdWVkLWNvbnRpbnVhdGlvbg=='): HTTP 200 with Items=[sg-3SACO1] and NextToken='sg-3SACO1'
  actual (ListStreamUrls, token issued by ListApplications): HTTP 200 with {'Items': []}
  source:   state.rs page_of_ids uses NextToken as the last-id cursor; no issued-token registry

Cause 7 — capacity multiple:
  expected: ('ValidationException', "Capacity values for location 'us-west-2' must be multiples of 2") for MaximumCapacity 1 and 3 on a gen4n_high group
  actual:   HTTP 200 accepted -> LocationStates [{'LocationName': 'us-west-2', 'MaximumCapacity': 1, 'Status': 'ACTIVE', ...}]
  source:   state.rs unstorable_location_configuration checks only for negative capacities

Cause 8 — stream URLs after the group is deleted:
  expected: both URLs still listed with Status='REVOKED', StatusReason='streamGroupDeleted', StreamGroupArn/ApplicationArn/UsageLimit preserved
  actual:   AssertionError: revocation of both of the deleted group's stream URLs did not happen in 24s; last read []
  source:   StreamUrl records live in StreamGroup::stream_urls; state.rs materialize reaps the group record with retain(|_, group| !group.advance(now))
  related:  gameliftstreams.list-stream-urls-lists-new-urls-and-follows-status ends "Check timed out after 60.0s"; its last stage waits for the surviving URL to read REVOKED after the group delete.

Model references used (com-amazonaws-gameliftstreams-gameliftstreams.json): Id (length 1..32, pattern ^[a-zA-Z0-9-]+$) and the "Example ID: sg-1AB2C3De4" documentation on Identifier; CreateApplication (POST /applications, 201, ValidationException 400; ApplicationLogPaths FilePaths length 0..10; ApplicationLogOutputUri length 0..1024, pattern ^$|^s3://...); CreateStreamUrl (POST /streamgroups/{Identifier}/streamurls, 201, ValidationException 400; Locations required @uniqueItems); DeleteStreamGroup (DELETE /streamgroups/{Identifier}, 204, ValidationException 400, ConflictException 409); UpdateStreamGroup (PATCH /streamgroups/{Identifier}, 200, ValidationException 400, ConflictException 409); ListStreamGroups / ListStreamUrls (GET, 200, @paginated NextToken/MaxResults, ValidationException 400; ListStreamUrls Status/StreamGroupIdentifier query filters); StreamUrlStatus (ACTIVE/EXPIRED/REVOKED/LIMIT_REACHED); StreamUrlStatusReason (userRevoked, revokedAndTerminatingSessions, revokedAndSessionsTerminated, streamGroupDeleted, applicationDeleted).

## Steps to Reproduce
Boot the emulator and point a botocore gameliftstreams client at it (region us-west-2), then:

Cause 1: create_application(Description='d', RuntimeEnvironment={'Type':'UBUNTU','Version':'22_04_LTS'}, ExecutablePath='run.sh', ApplicationSourceUri='s3://<bucket>/app/') and create_stream_group(Description='d', StreamClass='gen6n_small', LocationConfigurations=[{'LocationName':'us-west-2','MaximumCapacity':12}]) -> the returned Id bodies are 6 characters ('a-ArahX1', 'sg-pJok61'); AWS returns 9 ('a-1AB2C3De4' form).

Cause 2: create_application(Description='d', RuntimeEnvironment={'Type':'UBUNTU','Version':'22_04_LTS'}, ExecutablePath='serve.sh', ApplicationSourceUri='s3://<bucket>/bin', ApplicationLogPaths=['output/session.log'])  (no ApplicationLogOutputUri) -> compare Error.Message with the AWS sentence.

Cause 3: on an ACTIVE stream group in us-west-2 with a READY associated application, create_stream_url(Identifier=<group arn>, ApplicationIdentifier=<app arn>, Protocol='WebRTC', UrlExpiresAfterMinutes=30, Locations=['us-east-1']) -> compare Error.Message with "Locations do not exist in stream group '<group arn>': us-east-1".

Cause 4: create_stream_group(...) and, while get_stream_group reports ACTIVATING, delete_stream_group(Identifier=<arn>) -> emulator answers 204; AWS refuses with ValidationException "Stream group '<arn>' is in ACTIVATING status".

Cause 5: same setup, while ACTIVATING call update_stream_group(Identifier=<arn>, Description='renamed') -> emulator answers 409 ConflictException; AWS answers 400 ValidationException with the Cause 4 sentence.

Cause 6: with two stream groups present, list_stream_groups(MaxResults=1, NextToken='c2ltMnJlYWwtbmV2ZXItaXNzdWVkLWNvbnRpbnVhdGlvbg==') -> emulator serves a page; AWS refuses. Then take a NextToken from list_applications(MaxResults=1) and pass it to list_stream_urls(MaxResults=1, NextToken=<that token>) -> emulator serves {'Items': []}; AWS refuses with the same ValidationException.

Cause 7: create_stream_group(Description='d', StreamClass='gen4n_high', LocationConfigurations=[{'LocationName':'us-west-2','AlwaysOnCapacity':0,'MaximumCapacity':0}]); once ACTIVE, update_stream_group(Identifier=<arn>, LocationConfigurations=[{'LocationName':'us-west-2','AlwaysOnCapacity':0,'MaximumCapacity':1}]) -> emulator accepts and stores 1; AWS refuses with "Capacity values for location 'us-west-2' must be multiples of 2".

Cause 8: on an ACTIVE group with a READY associated application, create two stream URLs, then delete_stream_group and wait for the group to be gone; list_stream_urls(StreamGroupIdentifier=<group arn>) (and unfiltered with Status='REVOKED') -> emulator lists nothing; AWS lists both with Status='REVOKED' and StatusReason='streamGroupDeleted'.

Or replay the checks listed in trace_ids.
