Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: appconfig__lambda.lambda-validator-content-must-be-a-function-arn-rejected
Issue-Key: appconfig__lambda.lambda-validator-content-must-be-a-function-arn-rejected|_refusal#16|AssertionError|raise AssertionError(f"the call was expected to be refused but returned {result!r}")
Issue-Commit: af007de286377294faf8721f963c7a04ad7dcdf8

# appconfig__lambda: CreateConfigurationProfile accepts a LAMBDA validator whose Content is not a function ARN, creating a profile AWS refuses with BadRequestException

> _Note: this issue was identified against the code base at commit `af007de286377294faf8721f963c7a04ad7dcdf8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationProfile

## Description
Cause 1 of 1 — a LAMBDA-type validator's Content is not validated as an ARN (failing check: appconfig__lambda.lambda-validator-content-must-be-a-function-arn-rejected).
AWS: a Validator's Content is "Either the JSON Schema content or the Amazon Resource Name (ARN) of an Lambda function" (Smithy: Validator$Content documentation; CreateConfigurationProfile Validators -> ValidatorList, errors BadRequestException 400, ResourceNotFoundException 404, ServiceQuotaExceededException 402, InternalServerException 500). When Type is LAMBDA, AppConfig parses Content as a Lambda function ARN, so JSON-Schema text there is refused 400 BadRequestException and NO configuration profile is created. Recorded message, whose fixed text is "ARN " / " is invalid due to not containing a twelve digit account ID." with the offending Content echoed verbatim from the request: "ARN <content> is invalid due to not containing a twelve digit account ID."
Emulator: the call returns HTTP 201 and a ConfigurationProfile body with Validators [{"Type": "LAMBDA", "Content": "<the JSON schema text>"}], and the profile then shows up in ListConfigurationProfiles. The same request with Content set to a real Lambda function ARN is (correctly) accepted and stored as sent, so the Type=LAMBDA branch performs no ARN validation at all.
Why: a client relying on AWS to reject a mis-typed validator (schema text pasted into a LAMBDA validator, or an ARN from the wrong account) gets a profile that looks valid but can never invoke a validator function; the rejection path it codes against (400 BadRequestException, nothing created) never fires, and its resource inventory diverges from AWS's. Note this is distinct from the open issue 20261027T064453Z-appconfig-lambda-appconfig-startdeployment-still-deploys-content-a-profile-s.md, which is about the validator never being INVOKED at deployment time; this one is about CreateConfigurationProfile not validating the validator's Content at creation time.
Fix: CreateConfigurationProfile's input validation (the Validators handling in its handler) must, for Type=LAMBDA, require Content to be a Lambda function ARN and otherwise return BadRequestException (400) with the message above, creating no profile.
Traces: appconfig__lambda.lambda-validator-content-must-be-a-function-arn-rejected.

## Evidence
Expected: ('BadRequestException', "ARN <validator content> is invalid due to not containing a twelve digit account ID.") with HTTP 400 and the application still holding only the previously created profile.
Actual: HTTP 201 and a created profile — AssertionError "the call was expected to be refused but returned {'ResponseMetadata': {'HTTPStatusCode': 201, ...}, 'ApplicationId': ..., 'Id': ..., 'Name': 'sim2real-xsvc-acv-l-...', 'LocationUri': 'hosted', 'Validators': [{'Type': 'LAMBDA', 'Content': '{\"$schema\": \"http://json-schema.org/draft-07/schema#\", \"type\": \"object\", \"properties\": {\"limit\": {\"type\": \"integer\"}}, \"required\": [\"limit\"]}'}], 'Type': 'AWS.Freeform'}" (ids and generated names are per-run values, not requirements).

## Steps to Reproduce
1. Create a Lambda function (any runtime/handler) and an AppConfig application.
2. CreateConfigurationProfile with LocationUri="hosted" and Validators=[{"Type":"LAMBDA","Content":<the function's ARN>},{"Type":"JSON_SCHEMA","Content":<schema text>}] — this succeeds on both AWS and the emulator and must read back via GetConfigurationProfile exactly as sent.
3. CreateConfigurationProfile again in the same application with Validators=[{"Type":"LAMBDA","Content":<JSON schema text, no twelve-digit account id>}].
   Expect BadRequestException 400 "ARN <content> is invalid due to not containing a twelve digit account ID." and no new profile; the emulator returns 201 and creates the profile.
4. ListConfigurationProfiles for the application: AWS lists only the profile from step 2.
