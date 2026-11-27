Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys
Issue-Key: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys|_run_cli#13|AssertionError|assert result.returncode == 0, (
Issue-Commit: 65ee76328c73fce07b2a3bd288dbf2c587c2316a

# apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions: the CDK reference estate cannot deploy — the LexBot stack's AWS::Lex::Bot is refused "Intent with name FallbackIntent already exists. Use another name."

> _Note: this issue was identified against the code base at commit `65ee76328c73fce07b2a3bd288dbf2c587c2316a`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack (AWS::Lex::Bot) / CreateIntent

## Description
This issue carries 1 net-new root cause in the apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions behavioral service. The failing check was not marked ALREADY FILED and is not OWNED by any open issue. Open issues that look adjacent but do NOT carry this check's replay key: 20261109T170659Z-cloudformation-lexv2-models-2-net-new-causes-aws.md, 20261118T115344Z-cloudformation-lexv2-models-aws-lex-botalias-fails-to.md, 20261124T222528Z-cloudformation-lexv2-models-3-net-new-causes-an.md, 20261122T231140Z-cloudformation-cloudfront-dynamodb-ec2-e-cloudformation-the-cdk-compute-data-estate-cannot-deploy.md — they are named only so a fixer can see the family; none of them covers this check.

=== Cause 1 of 1: a template-declared FallbackIntent in an AWS::Lex::Bot locale is refused as a duplicate, so the whole CDK estate deploy fails ===
AWS: AWS::Lex::Bot provisions a bot together with its BotLocales and each locale's declared Intents. A Lex V2 locale must declare its fallback intent (an intent named FallbackIntent whose ParentIntentSignature is AMAZON.FallbackIntent) — that is the only accepted shape for a CFN-defined locale, and AWS's resource handler provisions it as the locale's fallback intent rather than rejecting it. The stack therefore reaches CREATE_COMPLETE and `cdk deploy --all` exits 0. Smithy (lexmodelsv2): CreateIntent PUT /bots/{botId}/botversions/{botVersion}/botlocales/{localeId}/intents, intentName @length 1-100 @pattern ^([0-9a-zA-Z][_-]?){1,100}$, parentIntentSignature IntentSignature; modeled errors ConflictException (409), PreconditionFailedException (412), ServiceQuotaExceededException (402), ThrottlingException (429), ValidationException (400), InternalServerException (500). A duplicate-name ConflictException is correct only for a name the locale already holds as a *distinct* caller-created intent — recorded behavior for the fallback intent is that the template's FallbackIntent is accepted, not refused.
Emulator: `cdk deploy --all` exits 1. The stack <prefix>-LexBot fails with a DeploymentError on resource BookTripTemplateBot (AWS::Lex::Bot): 'Resource handler returned message: "Intent with name FallbackIntent already exists. Use another name." (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)'. In that message the fixed text is "Intent with name " / " already exists. Use another name." and the variable part is the intent name taken from the request (FallbackIntent here); the RequestToken and the stack-name prefix are per-run values and are not part of the requirement. Hypothesis: the emulator's Lex::Bot handler (or the locale-creation path underneath it) auto-creates a FallbackIntent for every new bot locale and then issues CreateIntent for the template-declared FallbackIntent, which collides with the auto-created one and is refused as an already-existing name.
Why: a client cannot deploy any CloudFormation/CDK estate that contains a Lex V2 bot — the only locale shape AWS accepts (one that declares FallbackIntent) is exactly the one the emulator refuses. Because `cdk deploy --all` aborts on the first failed stack, the other 38 stacks of the reference estate and every serve/destroy assertion after them never run, so a large, otherwise-working surface is unreachable behind one refusal.
Fix: the AWS::Lex::Bot CloudFormation resource handler's locale/intent provisioning, and the duplicate-name check in the lexmodelsv2 CreateIntent handler it calls: a locale's built-in/auto-provisioned FallbackIntent must not be treated as a pre-existing distinct intent when the template declares that same fallback intent (reconcile/update it instead of returning the conflict).
Traces: apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys

## Evidence
Suite: simparity-behavioral-aws-v1, service apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions — 0/1 passed (0%), 1 failed (not filed, not owned).

Cause 1 — [mismatch] apigateway__cloudformation__dynamodb__lambda__s3__ssm__stepfunctions.cdk-reference-estate-deploys-serves-and-destroys
  Expected: `cdk deploy --all` of the 39-stack reference app exits 0; every stack, including the LexBot stack whose AWS::Lex::Bot declares a BotLocale containing a FallbackIntent (ParentIntentSignature AMAZON.FallbackIntent), reaches CREATE_COMPLETE.
  Actual: `cdk deploy --all` exited 1. Replay output tail:
    "❌  <prefix>-LexBot failed to deploy
     ‣ DeploymentError: Resource updates failed:
     <prefix>-LexBot/BookTripTemplateBot  (AWS::Lex::Bot BookTripTemplateBot)
       Resource handler returned message: \"Intent with name FallbackIntent already exists. Use another name.\" (RequestToken: <uuid>, HandlerErrorCode: InvalidRequest)"
  Assertion raised: AssertionError: `cdk deploy --all` exited 1, expected 0 (check file line 1152, _run_cli).
  The deploy aborts there, so the estate's serve and destroy phases are never exercised.

## Steps to Reproduce
1. Create a CloudFormation stack (or `cdk deploy` a stack) containing an AWS::Lex::Bot whose DataPrivacy/IdleSessionTTLInSeconds/RoleArn are set and whose BotLocales contains one locale (e.g. LocaleId "en_US") whose Intents list includes both a normal intent and an intent named "FallbackIntent" with ParentIntentSignature "AMAZON.FallbackIntent" — the shape AWS requires for a CFN-defined locale.
2. Observe the stack event for the AWS::Lex::Bot resource: AWS reaches CREATE_COMPLETE; the emulator reports CREATE_FAILED with handler message "Intent with name FallbackIntent already exists. Use another name." and HandlerErrorCode InvalidRequest.
3. Equivalent direct-API reproduction: CreateBot, then CreateBotLocale for en_US, then CreateIntent with intentName=FallbackIntent and parentIntentSignature=AMAZON.FallbackIntent against that locale — the emulator refuses it as an existing name instead of provisioning the locale's fallback intent.
