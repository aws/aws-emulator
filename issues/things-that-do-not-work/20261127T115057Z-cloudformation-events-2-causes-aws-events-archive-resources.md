Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__events.cfn-archive-update-after-backing-record-deleted-out-of-band-rejected cloudformation__events.deploy-cfn-archive-name-held-by-stack-rejects-direct-create cloudformation__events.deploy-cfn-archive-rename-replaces-archive cloudformation__events.cfn-events-rule-carries-its-declared-and-stack-tags
Issue-Key: cloudformation__events|DeleteArchive|ResourceNotFoundException
Issue-Key: cloudformation__events.deploy-cfn-archive-name-held-by-stack-rejects-direct-create|_failure#18|AssertionError|raise AssertionError(f"expected the call to be refused, it returned {answer!r}")
Issue-Key: cloudformation__events|DescribeArchive|ResourceNotFoundException
Issue-Key: cloudformation__events.cfn-events-rule-carries-its-declared-and-stack-tags|run#14|AssertionError|assert tags_.get("team") == "payments", (
Issue-Commit: 7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8

# cloudformation__events: 2 causes — AWS::Events::Archive resources are never provisioned in EventBridge, and a rule's declared/stack Tags are not applied

> _Note: this issue was identified against the code base at commit `7e5e0efa347a2e327a9b3588e8ef7ff7fc53eee8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 2: a stack that declares AWS::Events::Archive reports the resource created, but no archive exists in EventBridge. Every call that names the archive afterwards is refused ResourceNotFoundException, and the name is not held against a direct CreateArchive.
AWS: CREATE_COMPLETE for an AWS::Events::Archive means EventBridge holds an archive of that name: DescribeArchive/DeleteArchive on it succeed, and a direct events:CreateArchive for the same name is refused with ResourceAlreadyExistsException (the name is taken for the account+region). A rename (change of ArchiveName) is a replacement: the new name exists and is describable when the update completes. Smithy (eventbridge): CreateArchive errors include ResourceAlreadyExistsException; DescribeArchive/DeleteArchive error ResourceNotFoundException with the message form `Archive <name> does not exist.`
Emulator: DescribeArchive and DeleteArchive on the stack's archive are refused `ResourceNotFoundException: Archive <name> does not exist.` (the stack-created archive is absent), and a direct CreateArchive for the name a stack holds returns 200 with an ArchiveArn instead of refusing.
Why: a client cannot replay, describe or delete an archive its stack claims to own, and two owners can mint the same archive name — CFN-managed event archiving is unusable and drifts silently from the stack.
Fix: the CloudFormation resource provider for AWS::Events::Archive (create/update/replace path) must create the archive in the events state, and the events CreateArchive handler must treat an existing name as taken.
Traces: cloudformation__events.cfn-archive-update-after-backing-record-deleted-out-of-band-rejected, cloudformation__events.deploy-cfn-archive-name-held-by-stack-rejects-direct-create, cloudformation__events.deploy-cfn-archive-rename-replaces-archive.

Cause 2 of 2: the Tags a template declares on an AWS::Events::Rule (and the stack-level tags) are not on the created rule.
AWS: tags declared in a resource's Tags property, plus the stack's own tags, are applied to the created rule; events:ListTagsForResource on the rule ARN returns them.
Emulator: ListTagsForResource on the stack's rule returns `{}` — neither the declared tag (team=payments in the recording) nor the stack tags are present.
Why: tag-based cost allocation, access control (aws:ResourceTag conditions) and tag-driven discovery all fail for CFN-managed rules.
Fix: the AWS::Events::Rule resource provider's tag propagation (it must pass Tags through to events PutRule / TagResource).
Traces: cloudformation__events.cfn-events-rule-carries-its-declared-and-stack-tags.

## Evidence
Cause 1 — `botocore.errorfactory.ResourceNotFoundException: An error occurred (ResourceNotFoundException) when calling the DeleteArchive operation: Archive <name> does not exist.` and `... when calling the DescribeArchive operation: Archive <name>-one does not exist.`; and `AssertionError: expected the call to be refused, it returned {'ArchiveArn': 'arn:aws:events:<region>:<acct>:archive/<name>-store', 'State': 'ENABLED', ...}` where AWS refuses the direct create ResourceAlreadyExistsException.
Cause 2 — `AssertionError: ListTagsForResource on the stack's rule returned {}; the template's Tags [team=payments] must be on the rule`.

## Steps to Reproduce
Cause 1: CreateStack with an AWS::Events::Archive (ArchiveName=X, SourceArn a bus ARN); after CREATE_COMPLETE call events DescribeArchive(ArchiveName=X) — refused ResourceNotFoundException; call events CreateArchive(ArchiveName=X) — accepted 200. Then UpdateStack changing ArchiveName and DescribeArchive the new name.
Cause 2: CreateStack with an AWS::Events::Rule carrying Tags [{team,payments}] and stack-level tags; call events ListTagsForResource on the rule ARN.
