Issue-Source: trace-sweep-divergence
Issue-Trace: codestar-connections-repository-link-id-and-connection-validation
Issue-Step: task#8
Issue-Severity: MEDIUM
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: codestar-connections-repository-link-id-and-connection-validation
Issue-Shape-Id: com.amazonaws.codestarconnections#CodeStar_connections_20191201
Issue-Key: codestar-connections|UpdateRepositoryLink|4xx->4xx|$.Message
Issue-Commit: 72f9738f7767eb7284df38f15b7bafac7c93f5ce

# UpdateRepositoryLink dangling-ConnectionArn error message is emulator-authored, not AWS wording

> _Note: this issue was identified against the code base at commit `72f9738f7767eb7284df38f15b7bafac7c93f5ce`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
UpdateRepositoryLink

## Description
This issue has ONE cause.

Cause 1 of 1: UpdateRepositoryLink with a syntactically valid ConnectionArn that names no connection in the account (fixture codestar-connections-repository-link-id-and-connection-validation, step task#8).

AWS: UpdateRepositoryLink validates the optional ConnectionArn member before applying the update and rejects a reference to a non-existent connection with InvalidInputException (Smithy: UpdateRepositoryLink input RepositoryLinkId required, ConnectionArn target ConnectionArn @length 0..256 pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$; modeled errors include InvalidInputException httpError 400, ResourceNotFoundException httpError 404; InvalidInputException carries a single Message member of ErrorMessage). The recorded response is HTTP 400 with Message 'Connection <connectionArn> does not exist'. Fixed text: 'Connection ' followed by ' does not exist'. The only variable part is <connectionArn>, copied verbatim from the request's ConnectionArn. The message contains no 'The input is not valid.' preamble, does not name the input member, and says nothing about the account.

Emulator: returns the correct HTTP 400 and the correct error code, but Message 'The input is not valid. ConnectionArn names no connection in this account: <connectionArn>' — it prefixes the InvalidInputException documentation sentence, names the ConnectionArn member, and re-words the condition as 'names no connection in this account'. Only $.Message diverges.

Why: a client that branches on, logs or surfaces the error message to distinguish 'the referenced connection is gone' from 'the ARN is malformed' never matches AWS's 'Connection ... does not exist' template, so retry/repair logic and CLI output built against real CodeConnections behave differently against the emulator.

Fix: the UpdateRepositoryLink handler's connection-existence validation in the codestar-connections service crate (the branch that builds the InvalidInputException). Stop using the exception's generic documentation sentence as a message prefix and emit AWS's template with only the request's ConnectionArn substituted. The status and error code are already correct and must not change.

Traces: codestar-connections-repository-link-id-and-connection-validation#task#8.

Relation to already-filed work: open issue 20261029T175651Z-codestar-connections-error-messages-diverge-from-aws-wording.md covers the same class of defect on DeleteHost, GetRepositoryLink and CreateRepositoryLink, and its replay key does NOT cover the UpdateRepositoryLink step, so this cause is net-new and must be closed separately (the CreateRepositoryLink path it does cover is a sibling code path, so a shared validation helper may fix both).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout (emulator_up=true, 98 steps, 93 passed, score 94.9%).

Divergence cluster service=codestar-connections|UpdateRepositoryLink|status=400->400|keys=$.Message
Fixture: codestar-connections-repository-link-id-and-connection-validation, step task#8
Expected status 400, actual status 400 (status and error code agree).
$.Message value_mismatch:
  expected: 'Connection arn:aws:codestar-connections:us-west-2:000000000000:connection/00000000-0000-0000-0000-000000000000 does not exist'
  actual:   'The input is not valid. ConnectionArn names no connection in this account: arn:aws:codestar-connections:us-west-2:000000000000:connection/00000000-0000-0000-0000-000000000000'

Message decomposition: fixed text is 'Connection ' + ' does not exist'; the ARN shown above is the fixture's request value filling the single variable slot and is not itself a requirement.

Model reference used: Smithy UpdateRepositoryLink (input RepositoryLinkId required; ConnectionArn @length 0..256, pattern ^arn:aws(-[\w]+)*:.+:.+:[0-9]{12}:.+$; errors AccessDeniedException 403, ConditionalCheckFailedException 409, InternalServerException 503, InvalidInputException 400, ResourceNotFoundException 404, ThrottlingException 429, UpdateOutOfSyncException 409).

## Steps to Reproduce
1. Boot the emulator from this checkout and replay trace set simparity-wire-aws-v1.
2. Replay fixture codestar-connections-repository-link-id-and-connection-validation up to step task#8: create a repository link, then call UpdateRepositoryLink with that RepositoryLinkId and a well-formed ConnectionArn that names no connection in the account.
3. Observe HTTP 400 with error code InvalidInputException (correct) and compare $.Message against AWS's template 'Connection <connectionArn> does not exist', where <connectionArn> is the request's ConnectionArn.
4. The emulator instead returns 'The input is not valid. ConnectionArn names no connection in this account: <connectionArn>'. The fix is correct when the Message is exactly AWS's template with no documentation-sentence prefix and no member name.
