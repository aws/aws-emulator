Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys
Issue-Key: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys|_run_cli#13|AssertionError|assert result.returncode == 0, (
Issue-Commit: 9d6dbefbeeb79c5752051e10ea3d5ba278a8ab35

# cloudformation: the AWS::Lex::Bot resource create is refused on valueSelectionSetting.resolutionStrategy, so the 39-stack CDK reference estate cannot deploy

> _Note: this issue was identified against the code base at commit `9d6dbefbeeb79c5752051e10ea3d5ba278a8ab35`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
Cause 1 of 1: CloudFormation's AWS::Lex::Bot resource provider refuses the bot's slot-type value-selection setting, failing the LexBot stack of the CDK reference estate and aborting `cdk deploy --all` for the whole 39-stack app.
Failing check: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys.

AWS:      recorded behavior only for the estate as a whole — the check asserts `cdk deploy --all` exits 0 and that every one of its 39 stacks (including the one named LexBot, whose resource is `AWS::Lex::Bot BookTripTemplateBot`) reaches CREATE_COMPLETE, after which the estate serves and is destroyed. For the refused property specifically: CloudFormation's AWS::Lex::Bot type takes BotLocales[].SlotTypes[].ValueSelectionSetting.ResolutionStrategy in CloudFormation's own enum spelling (the documented allowed values are the upper-snake forms ORIGINAL_VALUE | TOP_RESOLUTION | CONCATENATION), while the Lex Models V2 API takes the PascalCase enum — Smithy (lexmodelsv2): SlotValueSelectionSetting.resolutionStrategy is @required and targets SlotValueResolutionStrategy, an enum whose values are exactly [OriginalValue, TopResolution, Concatenation]. AWS therefore accepts the CDK-generated template and creates the bot and its slot types; the two spellings never meet on the wire because the resource provider translates between them. Hypothesis: the emulator's provider forwards the template's ResolutionStrategy value verbatim into the lexmodelsv2 slot-type create, where the framework's enum validator rejects it — the emulator's own message quotes the API enum set as the admissible one, which is what a pass-through produces.

Emulator: the LexBot stack's resource fails during create and the deploy aborts. The CLI output tail ends: "❌  sim2real-ref-<token>-LexBot failed to deploy / ‣ DeploymentError: Resource updates failed: sim2real-ref-<token>-LexBot/BookTripTemplateBot  (AWS::Lex::Bot BookTripTemplateBot) / Resource handler returned message: \"1 validation error detected: Value at 'valueSelectionSetting.resolutionStrategy' failed to satisfy constraint: Member must satisfy enum value set: [OriginalValue, TopResolution, Concatenation]\" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)" and `cdk deploy --all` exits 1 instead of 0. In that refusal the fixed text is AWS's constraint-violation template "<n> validation error(s) detected: Value at '<member path>' failed to satisfy constraint: Member must satisfy enum value set: [<values>]"; the member path, the enum value list and the HandlerErrorCode are the emulator's state/model, and the stack name, logical id and RequestToken are per-run values and no part of the requirement. The requirement being broken is that the resource is CREATED, not that any particular message is served.

Why:      a client cannot deploy a CDK/CloudFormation app containing a Lex bot at all. Because `cdk deploy --all` aborts on the first failing stack, every other stack of the estate is left unprovisioned and nothing downstream of the deploy (the functions, REST/WebSocket APIs, state machines, event chains, ECS service, database and pipelines the estate serves, and the destroy path) can run. A developer sees a CloudFormation resource-handler validation error naming an enum set they never used, with no way to reconcile it against the documented CloudFormation property values.

Fix:      the CloudFormation crate's AWS::Lex::Bot resource provider — its property translation for BotLocales[].SlotTypes[].ValueSelectionSetting (map CloudFormation's ResolutionStrategy spelling onto the lexmodelsv2 SlotValueResolutionStrategy enum before calling the Lex create path, and likewise for the sibling Lex enum-valued properties), so a template carrying the documented CloudFormation values provisions the bot instead of failing the resource.

Related open issues that do NOT cover this check (named so a fixer can tell them apart; none carries this check's replay key): 20261109T170659Z-cloudformation-lexv2-models-2-net-new-causes-aws.md (AWS::Lex::Bot stub-provisioned, Ref/PhysicalResourceId = template Name), 20261117T134552Z-cloudformation-lexv2-models-a-create-complete-aws-lex.md (value-less stack Outputs) and 20261118T115344Z-cloudformation-lexv2-models-aws-lex-botalias-fails-to.md (BotAlias create failure). This failure is the Bot resource's own property validation refusing a documented CloudFormation enum value.

Traces:   apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys.

## Evidence
Failing check: [mismatch] apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys

Expected: `cdk deploy --all --require-approval never --concurrency 10` exits 0 (region us-east-1), and DescribeStacks reports StackStatus CREATE_COMPLETE for each of the 39 stacks, LexBot among them.

Actual: AssertionError: `cdk deploy --all` exited 1, expected 0; output tail: '... ❌  sim2real-ref-25bb248ad899-LexBot failed to deploy\n‣ DeploymentError: Resource updates failed:\nsim2real-ref-25bb248ad899-LexBot/BookTripTemplateBot  (AWS::Lex::Bot BookTripTemplateBot)\n  Resource handler returned message: "1 validation error detected: Value at \'valueSelectionSetting.resolutionStrategy\'\n  failed to satisfy constraint: Member must satisfy enum value set: [OriginalValue, TopResolution, Concatenation]"\n  (RequestToken: ecb0ee2c-3ba2-5995-b192-c68ba951bad2, HandlerErrorCode: InvalidRequest)'

Smithy cited (lexmodelsv2): SlotValueResolutionStrategy = enum [OriginalValue, TopResolution, Concatenation]; SlotValueSelectionSetting.resolutionStrategy is @required and targets it. CloudFormation's AWS::Lex::Bot documents ResolutionStrategy's allowed values in the upper-snake spelling (ORIGINAL_VALUE | TOP_RESOLUTION | CONCATENATION), which is what the CDK L1 construct emits.

The stack-name token 'sim2real-ref-25bb248ad899', the logical id 'BookTripTemplateBot' and the RequestToken are one run's values; the requirement is that an AWS::Lex::Bot declaring a slot type with a documented ResolutionStrategy is provisioned and its stack reaches CREATE_COMPLETE.

## Steps to Reproduce
1. Point the CDK at the emulator in us-east-1 and deploy the reference-estate app the check stages (`cdk bootstrap`, then `cdk deploy --all --ci --no-version-reporting --no-notices --require-approval never`).
2. Observe the CLI exits 1 with '❌ <prefix>-LexBot failed to deploy' and the AWS::Lex::Bot resource's handler message '1 validation error detected: Value at \'valueSelectionSetting.resolutionStrategy\' failed to satisfy constraint: Member must satisfy enum value set: [OriginalValue, TopResolution, Concatenation]' (HandlerErrorCode InvalidRequest); the remaining 38 stacks are never created.
3. Minimal equivalent: CreateStack with a template declaring an IAM role trusted by lexv2.amazonaws.com and an AWS::Lex::Bot whose BotLocales[0].SlotTypes[0].ValueSelectionSetting = {"ResolutionStrategy": "ORIGINAL_VALUE"} (also try TOP_RESOLUTION and CONCATENATION). AWS creates the bot and the stack reaches CREATE_COMPLETE; the emulator fails the Bot resource with the enum-constraint message above and rolls the stack back.
4. Re-replay apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys to confirm the fix.
