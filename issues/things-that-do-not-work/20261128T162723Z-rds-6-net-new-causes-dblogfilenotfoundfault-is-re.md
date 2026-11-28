Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: rds.download-log-portion-of-unlisted-file-rejected rds.log-file-portion-rejects-a-log-file-the-instance-never-wrote rds.log-file-portion-answers-an-unissued-marker-with-no-data rds.log-file-portion-pages-follow-the-marker-with-no-line-lost rds.modify-instance-nonexistent-parameter-or-option-group-rejected rds.describe-db-instances-accepts-the-instance-arn rds.copy-tags-to-snapshot-copies-instance-tags-to-a-manual-snapshot rds.manual-snapshot-completion-event-recorded
Issue-Shape-Id: com.amazonaws.rds#AmazonRDSv19
Issue-Key: rds.download-log-portion-of-unlisted-file-rejected|run#18|AssertionError|code == "DBLogFileNotFoundFault"
Issue-Key: rds.log-file-portion-rejects-a-log-file-the-instance-never-wrote|run#8|AssertionError|assert got == ("DBLogFileNotFoundFault", f"DBLog File: {missing} is not found on the DB instance"), (
Issue-Key: rds.log-file-portion-answers-an-unissued-marker-with-no-data|run#9|AssertionError|assert isinstance(issued, str) and _MARKER_FORM.fullmatch(issued), (
Issue-Key: rds.log-file-portion-pages-follow-the-marker-with-no-line-lost|run#10|AssertionError|assert isinstance(nxt, str) and _MARKER_FORM.fullmatch(nxt), (
Issue-Key: rds.modify-instance-nonexistent-parameter-or-option-group-rejected|_error#9|AssertionError|raise AssertionError(f"expected the call to be refused, but it succeeded: {result!r}")
Issue-Key: rds|DescribeDBInstances|DBInstanceNotFound
Issue-Key: rds.copy-tags-to-snapshot-copies-instance-tags-to-a-manual-snapshot|run#13|AssertionError|assert marker in tags, f"snapshot {snap_id} tags are {tags!r}; the instance tag {marker!r} was not copied"
Issue-Key: rds.manual-snapshot-completion-event-recorded|run#14|AssertionError|assert "Manual snapshot created" in seen, (
Issue-Commit: f2a96a9f1fdbdd444ddfeaed949096426fea7442

# rds: 6 net-new causes — DBLogFileNotFoundFault is re-worded and DownloadDBLogFilePortion issues a non-positional Marker, ModifyDBInstance accepts a nonexistent parameter/option group, DescribeDBInstances refuses the instance ARN, CopyTagsToSnapshot is inert, and no 'Manual snapshot created' event is recorded

> _Note: this issue was identified against the code base at commit `f2a96a9f1fdbdd444ddfeaed949096426fea7442`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
DownloadDBLogFilePortion

## Description
Six net-new root causes on the RDS control plane, found by replaying the behavioral suite (simparity-behavioral-aws-v1) against the emulator built from this checkout. A fixer must close ALL SIX before submitting; each is numbered with its own failing check ids. None of these checks was reported blocked by an unimplemented stub, and none is marked OWNED by an open issue.

---
Cause 1 of 6: the DBLogFileNotFoundFault refusal of DownloadDBLogFilePortion is worded by the emulator instead of as AWS words it.
AWS: Smithy models DownloadDBLogFilePortion with required DBInstanceIdentifier and LogFileName and the error DBLogFileNotFoundFault (errorKind client, httpError 404, awsQueryError code 'DBLogFileNotFoundFault'). A LogFileName the instance never wrote — or one DescribeDBLogFiles does not list — is refused with that fault and the message "DBLog File: <LogFileName> is not found on the DB instance": fixed text "DBLog File: " and " is not found on the DB instance", with the single variable part being the log file name copied from the request. The instance identifier is NOT named.
Emulator: the code is right but the sentence is the emulator's own — "DB log file <LogFileName> not found for DB instance <DBInstanceIdentifier>." — different fixed text, a different word order, a trailing period, and an extra variable (the instance id) AWS does not include.
Why: a client that matches this message (log-collection agents and support tooling distinguish "this file has rotated away" from other 404s by the sentence under the shared fault code) matches nothing against the emulator, and a negative test asserting AWS's text cannot pass.
Fix: the refusal path of the DownloadDBLogFilePortion handler in the RDS crate (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/ — the download-DB-log-file-portion operation), where the DBLogFileNotFoundFault message is built.
Traces: rds.download-log-portion-of-unlisted-file-rejected, rds.log-file-portion-rejects-a-log-file-the-instance-never-wrote.

---
Cause 2 of 6: DownloadDBLogFilePortion's Marker is not a log position — it is a page counter.
AWS: Smithy models DownloadDBLogFilePortion's output Marker (String) together with AdditionalDataPending, and the operation as paginated with inputToken/outputToken Marker. The Marker AWS issues is a position inside the log file written as two decimal integers joined by a colon, "<digits>:<digits>"; it is fed back as the request Marker to continue exactly where the previous page stopped, and a Marker the service never issued is answered with no data rather than a refusal.
Emulator: the first page's Marker is "1" — a bare page ordinal that does not match AWS's <digits>:<digits> form at all (both checks fail on the Marker's shape before they can grade continuation or the unissued-marker behaviour).
Why: the Marker is an opaque-but-structured token clients carry across calls and persist between runs (log tailing resumes from the last marker). A caller that validates or parses it rejects the emulator's value, and any client or test that compares/stores positions sees a counter that cannot express a position within a file, so paging correctness (no line lost, no line repeated) cannot be established.
Fix: the pagination/serialization of the DownloadDBLogFilePortion handler and the log-file state it reads — issue the Marker as the <digits>:<digits> position form, continue a page from a Marker of that form, and answer a well-formed Marker the service never issued with an empty LogFileData and no refusal.
Traces: rds.log-file-portion-answers-an-unissued-marker-with-no-data, rds.log-file-portion-pages-follow-the-marker-with-no-line-lost.

---
Cause 3 of 6: ModifyDBInstance accepts a DBParameterGroupName (and OptionGroupName) that does not exist.
AWS: Smithy models ModifyDBInstance with the errors DBParameterGroupNotFoundFault (client, 404, awsQueryError code 'DBParameterGroupNotFound') and OptionGroupNotFoundFault (client, 404). Naming a group that does not exist is refused on the ModifyDBInstance call: code DBParameterGroupNotFound with message "DBParameterGroup not found: <name>" (fixed text "DBParameterGroup not found: ", variable the requested name), and code OptionGroupNotFoundFault with "Specified OptionGroupName: <name> not found." (fixed text "Specified OptionGroupName: " and " not found."). The instance keeps the groups it had.
Emulator: the call is answered HTTP 200 with a normal ModifyDBInstance result; the response already shows the instance in status 'modifying' with DBParameterGroups = [{DBParameterGroupName: '<the nonexistent name>', ParameterApplyStatus: 'applying'}] — the emulator attaches a group that was never created instead of refusing.
Why: a client gets success for a request AWS rejects, and the emulator's state now references a parameter group that does not exist — so later reads of the instance report a phantom group and the DBParameterGroupNotFound/OptionGroupNotFoundFault paths are unreachable. A typo'd group name in a deploy script silently "succeeds".
Fix: the input-validation stage of the ModifyDBInstance handler (services/service-com-amazonaws-rds-amazonrdsv19/src/ops/ — the modify-DB-instance operation), before any state mutation: resolve DBParameterGroupName and OptionGroupName against the stored groups for the request's region and raise the modeled not-found faults with the sentences above.
Traces: rds.modify-instance-nonexistent-parameter-or-option-group-rejected.

---
Cause 4 of 6: DescribeDBInstances refuses the instance's own ARN as DBInstanceIdentifier.
AWS: Smithy documents DescribeDBInstancesMessage.DBInstanceIdentifier as "The user-supplied instance identifier or the Amazon Resource Name (ARN) of the DB instance. If this parameter is specified, information from only the specific DB instance is returned. This parameter isn't case-sensitive." So an ARN of an existing instance returns that one instance; DBInstanceNotFoundFault (404) is for an identifier that matches no instance.
Emulator: passing the ARN the emulator itself published as DBInstance.DBInstanceArn is refused with {'Type': 'Sender', 'Code': 'DBInstanceNotFound', 'Message': 'DBInstance <arn> not found.'} — only the bare identifier form is resolved.
Why: a valid call is refused. Code that carries ARNs (tag/Resource Groups output, CloudFormation/CDK outputs, cross-service handoff) and then describes the resource gets a 404 for an instance that exists; the refusal also misreports the state of the world, so retry/creation logic may try to create a duplicate.
Fix: the identifier resolution of the DescribeDBInstances handler (and the shared lookup in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs it uses): accept either a bare identifier or an RDS db ARN, case-insensitively, resolving the ARN to the instance it names in the request's region.
Traces: rds.describe-db-instances-accepts-the-instance-arn.

---
Cause 5 of 6: CopyTagsToSnapshot is inert — a manual snapshot of an instance with CopyTagsToSnapshot=true carries none of the instance's tags.
AWS: Smithy models CopyTagsToSnapshot on CreateDBInstance/ModifyDBInstance and on the DBInstance shape; when it is true, the tags of the source DB instance are copied onto snapshots created from it, so the snapshot's ListTagsForResource / DBSnapshot tag list contains the instance's user tags in addition to any given on CreateDBSnapshot.
Emulator: the snapshot is created and becomes available, but its tag list is empty ([]) — the instance tag (in the recording {'Key': 'sim2real-cost-centre', 'Value': 'c-42'}; the key/value are request values, the requirement is that every instance tag appears) was not copied.
Why: tags are how cost allocation, backup-policy automation and tag-based IAM/Backup selection find snapshots. With no tags copied, a snapshot that AWS would have attributed to a cost centre or selected by a tag-based policy is invisible to all of it, and the CopyTagsToSnapshot setting is echoed in describes while doing nothing.
Fix: the snapshot-creation path (the CreateDBSnapshot handler and the snapshot record in services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs): when the source instance has CopyTagsToSnapshot=true, seed the snapshot's tag set from the instance's tags, merged with the request's Tags.
Traces: rds.copy-tags-to-snapshot-copies-instance-tags-to-a-manual-snapshot.

---
Cause 6 of 6: a completed manual snapshot records no "Manual snapshot created" event.
AWS: recorded behavior only for the event text — Smithy models DescribeEvents with SourceType 'db-snapshot' and Event.Message, but not the message catalogue. AWS records two events for a manual snapshot: "Creating manual snapshot" when it starts and "Manual snapshot created" once the snapshot reaches available (RDS event category 'creation' / event id RDS-EVENT-0042); the fixed text of each is the whole sentence, and the SourceIdentifier is the snapshot identifier from the request.
Emulator: once the snapshot is available, DescribeEvents for that snapshot reports only ['Creating manual snapshot'] — the completion event is never appended.
Why: the completion event is the documented signal that a snapshot finished; event-subscription/EventBridge consumers and runbooks that wait for "Manual snapshot created" before continuing (copy to another region, start an export) never fire, so the workflow stalls while the snapshot is in fact available.
Fix: the snapshot state transition to available in the RDS state module (services/service-com-amazonaws-rds-amazonrdsv19/src/state.rs, the DB-snapshot record and the event log DescribeEvents reads) — record the completion event with AWS's sentence when the snapshot becomes available, as the start event is already recorded.
Traces: rds.manual-snapshot-completion-event-recorded.

Open rds issues a fixer should read first that do NOT cover these causes (the replay bound none of these checks to them): 20261111T084329Z-rds-ten-more-control-plane-and-data-plane.md (covers DescribeDBLogFiles ordering, not the not-found wording or the Marker form), 20261105T224225Z-rds-2-net-new-causes-every-db-instance.md and 20261123T060821Z-rds-3-net-new-behavioral-causes-modifydbsnapshot-answers.md (snapshot lifecycle/ModifyDBSnapshot, not tag copying or the completion event), 20261126T120718Z-rds-describedbparameters-reports-a-caller-set-parameter-in.md (parameter reads, not group resolution on ModifyDBInstance).

## Evidence
Behavioral suite (simparity-behavioral-aws-v1) replayed against the emulator built from this checkout: 613/752 passed (81.5%); service rds 348/438 (79%), 90 failed. The failures below are the ones the report returns with no owning issue and no blocking stub. Generated instance/snapshot/log names, ARstN account digits and timestamps are per-run values, not requirements.

Cause 1 — [mismatch] rds.download-log-portion-of-unlisted-file-rejected:
  assert code == "DBLogFileNotFoundFault" and message.startswith("DBLog File: ") and message.endswith(" is not found on the DB instance")
  AssertionError: downloading an unlisted file answered ('DBLogFileNotFoundFault', 'DB log file error/sim2real-logref-9eeed5eb3812.log not found for DB instance sim2real-logref-9eeed5eb3812.')
[mismatch] rds.log-file-portion-rejects-a-log-file-the-instance-never-wrote:
  assert got == ("DBLogFileNotFoundFault", f"DBLog File: {missing} is not found on the DB instance")
  AssertionError: downloading a log file the instance never wrote gave ('DBLogFileNotFoundFault', 'DB log file error/sim2real-never-written-18e4c3e4e2b3.log not found for DB instance sim2real-logmiss-905e9b9c7065.')

Cause 2 — [mismatch] rds.log-file-portion-answers-an-unissued-marker-with-no-data:
  _MARKER_FORM = re.compile(r"[0-9]+:[0-9]+")  # "The position RDS issues as a Marker: two decimal integers joined by a colon."
  AssertionError: the first page's Marker '1' is not of the form <digits>:<digits>
[mismatch] rds.log-file-portion-pages-follow-the-marker-with-no-line-lost:
  AssertionError: page 1 carried the Marker '1', not of the form <digits>:<digits>

Cause 3 — [mismatch] rds.modify-instance-nonexistent-parameter-or-option-group-rejected:
  graded expectations: ("DBParameterGroupNotFound", "DBParameterGroup not found: <name>") and ("OptionGroupNotFoundFault", "Specified OptionGroupName: <name> not found.")
  AssertionError: expected the call to be refused, but it succeeded: {'DBInstance': {... 'DBInstanceStatus': 'modifying', 'DBParameterGroups': [{'DBParameterGroupName': 'sim2real-behav-nopg-c6fc40a07bd5', 'ParameterApplyStatus': 'applying'}], ...}}

Cause 4 — [mismatch] rds.describe-db-instances-accepts-the-instance-arn:
  AssertionError: DescribeDBInstances refused the instance's own ARN 'arn:aws:rds:us-west-2:000000000000:db:sim2real-behav-inst-ba855694907e': {'Type': 'Sender', 'Code': 'DBInstanceNotFound', 'Message': 'DBInstance arn:aws:rds:us-west-2:000000000000:db:sim2real-behav-inst-ba855694907e not found.'}

Cause 5 — [mismatch] rds.copy-tags-to-snapshot-copies-instance-tags-to-a-manual-snapshot:
  assert marker in tags, f"snapshot {snap_id} tags are {tags!r}; the instance tag {marker!r} was not copied"
  AssertionError: snapshot sim2real-behav-snap-e34bff51e7a7 tags are []; the instance tag {'Key': 'sim2real-cost-centre', 'Value': 'c-42'} was not copied

Cause 6 — [mismatch] rds.manual-snapshot-completion-event-recorded:
  assert "Manual snapshot created" in seen
  AssertionError: snapshot sim2real-behav-snap-a8c6cb15f0e1 is available but its events are ['Creating manual snapshot']: no 'Manual snapshot created'

## Steps to Reproduce
1. Cause 1: create a DB instance, wait for available, call DescribeDBLogFiles to learn which files exist, then DownloadDBLogFilePortion with a LogFileName that is not listed. AWS: 404 DBLogFileNotFoundFault, message "DBLog File: <name> is not found on the DB instance". Emulator: same code, message "DB log file <name> not found for DB instance <instance id>.". Checks: rds.download-log-portion-of-unlisted-file-rejected, rds.log-file-portion-rejects-a-log-file-the-instance-never-wrote.
2. Cause 2: DownloadDBLogFilePortion a listed log file with NumberOfLines small enough to page. AWS: output Marker of the form <digits>:<digits> which, fed back as the request Marker, continues after the last line returned; a well-formed Marker the service never issued returns no data and no error. Emulator: the first page's Marker is '1'. Checks: rds.log-file-portion-pages-follow-the-marker-with-no-line-lost, rds.log-file-portion-answers-an-unissued-marker-with-no-data.
3. Cause 3: on an available instance call ModifyDBInstance(DBParameterGroupName=<never created>, ApplyImmediately=True), then ModifyDBInstance(OptionGroupName=<never created>, ApplyImmediately=True). AWS: 404 DBParameterGroupNotFound "DBParameterGroup not found: <name>" and OptionGroupNotFoundFault "Specified OptionGroupName: <name> not found.", with the instance's groups unchanged. Emulator: both calls return 200 and the phantom group is attached. Check: rds.modify-instance-nonexistent-parameter-or-option-group-rejected.
4. Cause 4: create an instance, read DBInstance.DBInstanceArn from the describe, then call DescribeDBInstances(DBInstanceIdentifier=<that ARN>). AWS: the one instance. Emulator: 404 DBInstanceNotFound. Check: rds.describe-db-instances-accepts-the-instance-arn.
5. Cause 5: create an instance with CopyTagsToSnapshot=True and a user tag, wait for available, CreateDBSnapshot, wait for available, then read the snapshot's tags (ListTagsForResource on the snapshot ARN / DescribeDBSnapshots). AWS: the instance's tags are present. Emulator: []. Check: rds.copy-tags-to-snapshot-copies-instance-tags-to-a-manual-snapshot.
6. Cause 6: after the same snapshot reaches available, DescribeEvents(SourceType='db-snapshot', SourceIdentifier=<snapshot id>). AWS: both 'Creating manual snapshot' and 'Manual snapshot created'. Emulator: only the first. Check: rds.manual-snapshot-completion-event-recorded.
7. Re-replay the check ids in trace_ids to confirm a fix.
