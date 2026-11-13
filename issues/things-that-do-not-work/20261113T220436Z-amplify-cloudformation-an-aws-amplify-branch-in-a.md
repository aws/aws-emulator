Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete
Issue-Key: amplify__cloudformation|GetBranch|NotFoundException
Issue-Commit: fa605e3beb5560d80c09dff54081711248679782

# amplify__cloudformation: an AWS::Amplify::Branch in a CREATE_COMPLETE stack is never created in Amplify — GetBranch answers NotFoundException for the branch the stack published

> _Note: this issue was identified against the code base at commit `fa605e3beb5560d80c09dff54081711248679782`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetBranch

## Description
Cause 1 of 1: a CloudFormation stack declaring an AWS::Amplify::Branch settles CREATE_COMPLETE and publishes the branch's Ref, but the branch does not exist in Amplify.
AWS: recorded behavior only — a stack whose AWS::Amplify::Branch names AppId (here Fn::GetAtt App.AppId) and BranchName reaches CREATE_COMPLETE only once the branch exists, and an independent Amplify read confirms it: GetBranch(appId=<the stack's AppId output>, branchName=<the template's BranchName>) answers 200 with branch.branchArn equal to the stack's Ref output for that resource, branch.branchName equal to the template's BranchName, and branch.tags carrying the template's own tag, the stack-propagated tags (aws:cloudformation:stack-name, aws:cloudformation:stack-id) and aws:cloudformation:logical-id set to the branch's logical id. Only after the branch is removed from the template (UpdateStack) does GetBranch refuse, with 404 NotFoundException and the message 'Branch <branchName> not found.' — fixed text 'Branch ' / ' not found.' around the branch name taken from the request.
Emulator: immediately after the stack reports CREATE_COMPLETE, GetBranch on exactly that appId and BranchName answers 404 NotFoundException 'Branch main not found.' ('main' is this run's BranchName). The branch resource is reported as created and its Ref output is published, but no branch was registered in Amplify state, so the check dies on the first read of the resource the stack claims to have created.
Why: a client that deploys its Amplify hosting topology through CloudFormation gets a stack that says the branch exists while every Amplify call about it — GetBranch, ListBranches, UpdateBranch, CreateDeployment/StartDeployment against that branch — refuses with not-found. Nothing downstream of the deploy can work, and the stack reports no failure a pipeline could act on. Note the open issue 20261109T123922Z-amplify-cloudformation-stack-outputs-carry-no-outputvalue-and.md names this same check, but for a different cause (DescribeStacks Outputs carrying no OutputValue, which this replay got past) and for the inverse branch cause (a branch whose AppId does not exist being provisioned); neither covers a validly declared branch not being created.
Fix: the AWS::Amplify::Branch CloudFormation resource handler's create path — it must resolve AppId and actually create the branch in amplify's state (with the template, stack and logical-id tags) before the resource reaches CREATE_COMPLETE, and must fail the resource (and roll the stack back) when it cannot.
Traces: amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete

## Evidence
amplify__cloudformation.deploy-amplify-app-and-branch-round-trip-and-delete:
  the stack reached CREATE_COMPLETE and its App outputs (Ref/AppId/AppName/DefaultDomain) matched Amplify's own values, then:
  expected: amp.get_branch(appId=<stack AppId>, branchName=<template BranchName>) -> 200 with branchArn == the stack's BranchArn Ref output, branchName == the template's BranchName, and tags == {<template tag>, sim2real:<stack>, aws:cloudformation:stack-name:<stack>, aws:cloudformation:stack-id:<stack id>, aws:cloudformation:logical-id:'Branch'}
  actual:   botocore.errorfactory.NotFoundException: An error occurred (NotFoundException) when calling the GetBranch operation: Branch main not found.
  ('main' is the BranchName this run declared; the requirement is the message template 'Branch <branchName> not found.', which must only be answered AFTER the branch is removed from the stack.)

## Steps to Reproduce
1. CreateStack with a template holding an AWS::Amplify::App (Name, Tags) and an AWS::Amplify::Branch whose AppId is Fn::GetAtt [App, AppId] and whose BranchName is 'main', plus Outputs for Ref App, GetAtt App.AppId and Ref Branch.
2. Wait for CREATE_COMPLETE and read the AppId and BranchArn outputs.
3. Call amplify GetBranch(appId=<that AppId>, branchName='main'): expect 200 with the branch (branchArn == the Ref output, the CloudFormation tags present), observe 404 NotFoundException 'Branch main not found.'.
4. Also ListBranches(appId=<that AppId>) — the branch the stack claims to have created is absent.
5. Re-replay the check in trace_ids.
