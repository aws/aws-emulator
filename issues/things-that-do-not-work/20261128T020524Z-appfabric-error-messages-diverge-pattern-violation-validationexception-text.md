Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appfabric.start-ingestion-uppercase-uuid-identifier-rejected appfabric.stop-ingestion-uppercase-bundle-identifier-rejected appfabric.start-user-access-tasks-email-without-at-sign-rejected appfabric.start-user-access-tasks-deleted-bundle-rejected appfabric.tag-resource-on-deleted-app-bundle-rejected
Issue-Shape-Id: com.amazonaws.appfabric#FabricFrontEndService
Issue-Key: appfabric.start-ingestion-uppercase-uuid-identifier-rejected|run#11|AssertionError|assert got == expected, f"an uppercase ingestion id answered {got!r}, expected {expected!r}"
Issue-Key: appfabric.stop-ingestion-uppercase-bundle-identifier-rejected|run#15|AssertionError|assert upper == want, f"an uppercase bundle UUID answered {upper!r}, expected {want!r}"
Issue-Key: appfabric.start-user-access-tasks-email-without-at-sign-rejected|run#12|AssertionError|assert got == expected, f"an address without '@' answered {got!r}, expected {expected!r}"
Issue-Key: appfabric.start-user-access-tasks-deleted-bundle-rejected|run#14|AssertionError|assert got == expected, f"StartUserAccessTasks on the deleted bundle answered {got!r}, expected {expected!r}"
Issue-Key: appfabric.tag-resource-on-deleted-app-bundle-rejected|run#17|AssertionError|assert tagged == ("ResourceNotFoundException", "The app bundle ID was not found."), (
Issue-Commit: c05681c6a7a1f98eaadd6408209ad7859bc7b6dc

# AppFabric error messages diverge: pattern-violation ValidationException text and ResourceNotFoundException text on a deleted app bundle

> _Note: this issue was identified against the code base at commit `c05681c6a7a1f98eaadd6408209ad7859bc7b6dc`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
StartIngestion

## Description
Three distinct net-new root causes in the AppFabric emulator, all in how refusals are worded. Error codes and HTTP statuses match AWS; the Message strings do not, so a client that reads the message (or a test that asserts on it) diverges. An open issue, 20261106T165607Z-appfabric-connectappauthorization-never-validates-the-credential-a-failing.md, concerns ConnectAppAuthorization credential validation and covers none of these.

Cause 1 of 3 — the pattern-violation ValidationException message is rendered with the regex anchors left in, and (for non-sensitive members) with the rejected value echoed.
AWS: Smithy (services/service-com-amazonaws-appfabric-fabricfrontendservice): StartIngestion.ingestionIdentifier, StopIngestion.appBundleIdentifier -> shape Identifier, @pattern "^arn:.+$|^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$"; StartUserAccessTasks.email -> shape Email, @pattern "^[a-zA-Z0-9.!#$%&’*+/=?^_`{|}~-]+@[a-zA-Z0-9-]+(?:\\.[a-zA-Z0-9-]+)*$", @sensitive; modeled error ValidationException (httpError 400). Recorded behavior: AWS answers 400 ValidationException with the fixed text "1 validation error detected: Value at '<member>' failed to satisfy constraint: Member must satisfy regular expression pattern: <pattern>" where the only variable parts are the member name taken from the request (ingestionIdentifier / appBundleIdentifier / email) and the modeled pattern with its outer anchors removed — the leading '^' and the trailing '$' of the whole expression are stripped, so the identifier pattern is reported as "arn:.+$|^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}" and the email pattern as "[a-zA-Z0-9.!#$%&’*+/=?^_`{|}~-]+@[a-zA-Z0-9-]+(?:\\.[a-zA-Z0-9-]+)*". The rejected value is NOT quoted in the message for any of these three members (the generic AWS form "Value '<x>' at '<member>' ..." is not what AppFabric emits here).
Emulator: emits the modeled pattern verbatim including the outer '^' and '$', and for the two Identifier members additionally prefixes the rejected value, e.g. "1 validation error detected: Value '793C021B-6D17-4364-83B3-C0CCB246E721' at 'ingestionIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.+$|^[a-f0-9]{8}-...-[a-f0-9]{12}$". For email it omits the value but still keeps the anchors.
Why: the code (ValidationException) and status (400) are right, but any client, SDK waiter, log assertion or human-facing surface that reads the message sees text AWS never produces; and echoing the identifier back is an extra divergence on members AWS reports without a value (email is @sensitive and must never be echoed at all).
Fix: the shared @pattern validation-message formatter used by the AppFabric input validation layer — strip the expression's outer anchors when rendering, and do not quote the offending value for these members.
Traces: appfabric.start-ingestion-uppercase-uuid-identifier-rejected, appfabric.stop-ingestion-uppercase-bundle-identifier-rejected, appfabric.start-user-access-tasks-email-without-at-sign-rejected.

Cause 2 of 3 — StartUserAccessTasks on a deleted app bundle is refused with the wrong message.
AWS: Smithy: StartUserAccessTasks POST /useraccess/start, success 201, modeled error ResourceNotFoundException (httpError 404). Recorded behavior: after DeleteAppBundle, the same StartUserAccessTasks request answers 404 ResourceNotFoundException with the fixed message "Fabric has been disabled by your IT Admin." (no variable parts), while GetAppBundle on the same ARN answers "The app bundle ID was not found." — i.e. this operation has its own, distinct not-found wording.
Emulator: answers ResourceNotFoundException with "The specified app bundle ID is not valid" instead.
Why: a client distinguishing "the bundle is gone" from "AppFabric is disabled for this account" by message gets the wrong classification, and the emulator's text is not an AWS message at all.
Fix: the StartUserAccessTasks handler's bundle-lookup failure path (its not-found message must not share the generic bundle-identifier message).
Traces: appfabric.start-user-access-tasks-deleted-bundle-rejected.

Cause 3 of 3 — the tagging operations on a deleted app bundle ARN are refused with a generic resource message instead of AppFabric's app-bundle message.
AWS: Smithy: TagResource POST /tags/{resourceArn} (resourceArn @pattern "^arn:.+$"), UntagResource, ListTagsForResource, each with modeled error ResourceNotFoundException (httpError 404). Recorded behavior: once the app bundle is deleted, TagResource, UntagResource and ListTagsForResource on its ARN all answer 404 ResourceNotFoundException with the fixed message "The app bundle ID was not found." (no variable parts) — the same wording GetAppBundle uses.
Emulator: TagResource answers ResourceNotFoundException with "The specified resource does not exist."; the check fails there, so the sibling UntagResource and ListTagsForResource paths that share the lookup should be corrected together.
Why: the message a client sees for a vanished app bundle depends on which operation it called, where AWS is uniform; message-based handling and user-facing text diverge.
Fix: the resource-ARN resolution used by the TagResource / UntagResource / ListTagsForResource handlers — resolve an appbundle/<uuid> ARN through the app-bundle lookup and report its not-found message.
Traces: appfabric.tag-resource-on-deleted-app-bundle-rejected.

## Evidence
Cause 1:
- start-ingestion-uppercase-uuid-identifier-rejected: expected ('ValidationException', "1 validation error detected: Value at 'ingestionIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: arn:.+$|^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}"); got ('ValidationException', "1 validation error detected: Value '793C021B-6D17-4364-83B3-C0CCB246E721' at 'ingestionIdentifier' failed to satisfy constraint: Member must satisfy regular expression pattern: ^arn:.+$|^[a-f0-9]{8}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{4}-[a-f0-9]{12}$"). (The quoted UUID is a per-run value filling the template; the requirement is that no value is quoted at all and the anchors are stripped.)
- stop-ingestion-uppercase-bundle-identifier-rejected: same divergence on member 'appBundleIdentifier' (value echoed, anchors kept). The lowercase-UUID and ARN controls in the same check pass: both reach the ingestion lookup and answer ('ResourceNotFoundException', 'The specified ingestion does not exist').
- start-user-access-tasks-email-without-at-sign-rejected: expected pattern text "[a-zA-Z0-9.!#$%&’*+/=?^_`{|}~-]+@[a-zA-Z0-9-]+(?:\\.[a-zA-Z0-9-]+)*"; got the same pattern wrapped in '^' ... '$'. Value correctly not echoed (email is @sensitive).

Cause 2:
- start-user-access-tasks-deleted-bundle-rejected: on the live bundle StartUserAccessTasks returns HTTP 201 with userAccessTasksList == [] (passes). After DeleteAppBundle, GetAppBundle answers ('ResourceNotFoundException', 'The app bundle ID was not found.') as expected, but StartUserAccessTasks answers ('ResourceNotFoundException', 'The specified app bundle ID is not valid'); expected ('ResourceNotFoundException', 'Fabric has been disabled by your IT Admin.').

Cause 3:
- tag-resource-on-deleted-app-bundle-rejected: TagResource/UntagResource on the live bundle take effect (passes); GetAppBundle after delete answers ('ResourceNotFoundException', 'The app bundle ID was not found.') (passes); TagResource on the deleted bundle ARN answers ('ResourceNotFoundException', 'The specified resource does not exist.'), expected ('ResourceNotFoundException', 'The app bundle ID was not found.').

## Steps to Reproduce
Region us-east-1 (AppFabric serves us-east-1, eu-west-1, ap-northeast-1).

Cause 1:
1. CreateAppBundle(clientToken=<uuid>) -> note appBundle.arn and its appbundle/<uuid> suffix.
2. StartIngestion(appBundleIdentifier=<bundle arn>, ingestionIdentifier=<a lowercase uuid4>) -> 404 ResourceNotFoundException 'The specified ingestion does not exist' (control, passes).
3. StartIngestion with the same uuid upper-cased -> compare the ValidationException Message: it must quote no value and report the pattern without its outer '^'/'$'.
4. StopIngestion(appBundleIdentifier=<bundle uuid>.upper(), ingestionIdentifier=<uuid4>) -> same check on member 'appBundleIdentifier'.
5. StartUserAccessTasks(appBundleIdentifier=<bundle arn>, email='user.example.com') -> ValidationException message must report the email pattern with anchors stripped.

Cause 2:
1. CreateAppBundle; StartUserAccessTasks(appBundleIdentifier=<arn>, email='user@example.com') -> 201, userAccessTasksList == [].
2. DeleteAppBundle(appBundleIdentifier=<arn>).
3. Repeat StartUserAccessTasks with the same arguments -> must be 404 ResourceNotFoundException 'Fabric has been disabled by your IT Admin.'

Cause 3:
1. CreateAppBundle(tags=[{key:'sim2real',...}]); TagResource/UntagResource/ListTagsForResource on the ARN while live (all succeed).
2. DeleteAppBundle(appBundleIdentifier=<arn>).
3. TagResource(resourceArn=<arn>, tags=[{key:'stage',value:'dead'}]), then UntagResource(resourceArn=<arn>, tagKeys=['sim2real']), then ListTagsForResource(resourceArn=<arn>) -> each must be 404 ResourceNotFoundException 'The app bundle ID was not found.'
