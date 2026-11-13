Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete
Issue-Key: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete|run#20|AssertionError|assert out["AppArn"] == app_arn, f"Application Ref {out['AppArn']!r} != {app_arn!r}"
Issue-Commit: a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c

# Ref on AWS::ApplicationInsights::Application returns a generated physical id instead of the application ARN

> _Note: this issue was identified against the code base at commit `a1ec1c78d82f4b31ace8b5486cc3f03e8393d01c`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new cause behind the surviving application-insights__cloudformation__resource-groups failure. The stack now reaches CREATE_COMPLETE and the Resource Groups group is registered (so the ListGroups cause of the open issue 20261109T123922Z-application-insights-cloudformation-reso-application-insights-cloudformation-resource-groups-a-stack-created.md no longer fires here; its second cause, the missing-resource-group rollback, is still owned by that issue and is NOT re-filed). What diverges now is the identifier CloudFormation publishes for the Application resource.

Cause 1 of 1: the physical resource id (and therefore `Ref`) of an AWS::ApplicationInsights::Application is a generated stack-derived name rather than the application ARN.
AWS: the CloudFormation resource reference for AWS::ApplicationInsights::Application (https://docs.aws.amazon.com/AWSCloudFormation/latest/TemplateReference/aws-resource-applicationinsights-application.html) states that `Ref` returns the ApplicationARN, i.e. the resource's physical id IS the ARN `arn:<partition>:applicationinsights:<region>:<account-id>:application/resource-group/<ResourceGroupName>`. Every part after the fixed `application/resource-group/` prefix is taken from the stack's partition/region/account and the resource's resolved `ResourceGroupName` property; `Fn::GetAtt ApplicationARN` yields the same string. Smithy: the applicationinsights model carries no ARN member for this (the ARN form is a CloudFormation/Application Insights identity rule, not a modelled field) — recorded behavior plus the template reference only.
Emulator: the `Ref` of the App resource resolves to a generated `<stack-name>-<logical-id>-<random-suffix>` physical id — observed as `'sim2real-aiapp-<suffix>-App-<suffix>'` where the ARN `arn:aws:applicationinsights:<region>:<account>:application/resource-group/<group-name>` was expected. (The literal names/suffixes in that observation are per-run values, not part of the requirement; the requirement is the ARN template above.) This is the same generic fallback id the emulator mints for resource types whose primary identifier is not modelled.
Why: a template that exports the application ARN, or feeds `Ref App` / `Fn::GetAtt App.ApplicationARN` into an IAM policy resource, a tag call, an SSM parameter or any applicationinsights API, receives a string that is not an ARN and identifies nothing. Deploy-then-use wiring built on the documented Ref silently breaks, and the output a client reads back cannot be used to address the application.
Fix: the AWS::ApplicationInsights::Application resource handler's create path in the CloudFormation resource-type registry — it must return the application ARN (built from the stack's partition, region, account and the resolved ResourceGroupName) as the resource's physical resource id, so Ref and GetAtt ApplicationARN both resolve to it, instead of falling back to the generated stack-derived name.
Traces: application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete

## Evidence
Check application-insights__cloudformation__resource-groups.deploy-applicationinsights-application-round-trip-and-delete

Template: AWS::ResourceGroups::Group `Group` + AWS::ApplicationInsights::Application `App` (DependsOn Group, ResourceGroupName = {"Ref": "Group"}, AutoConfigurationEnabled false), Output `AppArn` = {"Ref": "App"}.

Expected (AWS): Outputs.AppArn == arn:<partition>:applicationinsights:<region>:<account>:application/resource-group/<group-name>
Actual (emulator): Outputs.AppArn == '<stack-name>-App-<random-suffix>' (recorded run: 'sim2real-aiapp-1f8e45e8f26a-App-0AE525A7D546' vs expected 'arn:aws:applicationinsights:us-west-2:000000000000:application/resource-group/sim2real-aigrp-0a5ca6e3082b')

AssertionError: Application Ref '<generated id>' != '<application ARN>'  (run.py BODY phase, assert out["AppArn"] == app_arn)

The stack itself reached CREATE_COMPLETE and the group's Ref/GetAtt Arn and GetGroupQuery assertions all passed, so only the Application's identifier diverges.

## Steps to Reproduce
1. CreateStack with a template declaring an AWS::ResourceGroups::Group (tag-query, TAG_FILTERS_1_0) and an AWS::ApplicationInsights::Application whose ResourceGroupName is {"Ref": "Group"} and AutoConfigurationEnabled false, plus an Output AppArn = {"Ref": "App"}.
2. Wait for CREATE_COMPLETE, then DescribeStacks and read Outputs.AppArn.
3. Observe a generated '<stack-name>-App-<suffix>' physical id instead of 'arn:<partition>:applicationinsights:<region>:<account>:application/resource-group/<group-name>'. ListStackResources shows the same value as the App resource's PhysicalResourceId.
4. Re-replay the check in trace_ids.
