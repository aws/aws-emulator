Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: medialive__ssm.credentialed-pull-source-without-url-rejected
Issue-Key: medialive__ssm.credentialed-pull-source-without-url-rejected|_bad_request#7|AssertionError|raise AssertionError(f"the input was created: {out.get('Input', out)!r}")
Issue-Commit: a24aa969c7ede1b0812774a0cc80ce835cb01596

# medialive__ssm: CreateInput accepts a credentialed URL_PULL source with no Url

> _Note: this issue was identified against the code base at commit `a24aa969c7ede1b0812774a0cc80ce835cb01596`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateInput

## Description
Cause 1 of 1: a URL_PULL input source carrying only credentials is accepted.
AWS: for a URL_PULL input every Sources entry must carry a Url; a source that supplies Username/PasswordParam without a Url is refused (HTTP 400, a BadRequestException naming the missing member) and no input is created.
Emulator: returns the created input — Type 'URL_PULL', State 'DETACHED', with Sources [{'PasswordParam': '<param>', 'Username': 'edge'}, {'PasswordParam': '<param>', 'Username': 'edge'}] and no Url in either entry.
Why: an input that can never pull anything exists and is attachable to a channel; the configuration error surfaces (if at all) only when the channel starts, far from the call that caused it.
Fix: validate Sources[].Url (required for URL_PULL) in the MediaLive CreateInput handler before creating the input.
Traces: medialive__ssm.credentialed-pull-source-without-url-rejected.

## Evidence
'the input was created: {Arn: arn:aws:medialive:us-west-2:000000000000:input:1000000, Id: 1000000, Type: URL_PULL, State: DETACHED, Sources: [{PasswordParam: sim2real-xsvc-nourl-…, Username: edge}, {PasswordParam: …, Username: edge}], …}'.

## Steps to Reproduce
PutParameter a SecureString for the password, then CreateInput(Type='URL_PULL', Sources=[{Username:'edge', PasswordParam:'<param>'}] twice) with no Url in either source. AWS refuses; the emulator returns a created input.
