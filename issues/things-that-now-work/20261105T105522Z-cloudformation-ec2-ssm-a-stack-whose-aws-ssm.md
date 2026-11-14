Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: cloudformation__ec2__ssm.cfn-ssm-automation-document-stops-ec2-instance-positive
Issue-Key: ssm|StartAutomationExecution|AutomationDefinitionNotFoundException
Issue-Commit: 8524570065cf0f73f283419a239f018e52a5ab39

# cloudformation__ec2__ssm: a stack whose AWS::SSM::Document reaches CREATE_COMPLETE never registers the document in SSM, so StartAutomationExecution on the stack's own output answers AutomationDefinitionNotFoundException

> _Note: this issue was identified against the code base at commit `8524570065cf0f73f283419a239f018e52a5ab39`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateStack

## Description
One net-new root cause.

Cause 1 of 1: a CloudFormation stack that declares an `AWS::SSM::Document` with `DocumentType: Automation` reports CREATE_COMPLETE and returns the document's name through a stack Output ({"Ref": "StopDoc"}), but the document is never registered in SSM state: the very next call, StartAutomationExecution with that exact output as DocumentName, is refused as if the document had never been created.

AWS: recorded behavior only for the end-to-end path — a stack that reaches CREATE_COMPLETE has created every declared resource, so an `AWS::SSM::Document` of type Automation is afterwards resolvable by name through SSM's Automation surface, StartAutomationExecution returns an AutomationExecutionId, the execution reaches `Success`, and its `aws:changeInstanceState` step with DesiredState `stopped` actually stops the named EC2 instance (DescribeInstances then reports state `stopped`). SSM's AutomationDefinitionNotFoundException exists for a document that does NOT exist; its recorded message is `Document <document-name> does not exist.` (fixed text `Document ` … ` does not exist.`, with the name taken from the request) — AWS does not answer it for a document the account just created.

Emulator: the stack create succeeds and both Outputs (InstanceId, DocumentName) are populated, then `ssm.start_automation_execution(DocumentName=<stack output>, Parameters={"InstanceId": [...]})` fails with HTTP 400 AutomationDefinitionNotFoundException: "An error occurred (AutomationDefinitionNotFoundException) when calling the StartAutomationExecution operation: Document <name> does not exist." The automation therefore never runs and the instance is never stopped.

Why: a client relying on AWS's behavior treats CREATE_COMPLETE as the guarantee that the declared document is usable — infrastructure pipelines create runbooks with CloudFormation and immediately invoke them. Here the stack lies: it reports success for a resource that does not exist in the service that owns it, so every downstream SSM Automation call fails with a not-found refusal for an id the stack itself just handed back, and no automation side effect (the instance stop) ever happens.

Fix: the CloudFormation resource provider for `AWS::SSM::Document` — its create path in the cloudformation crate's resource registry — must register the document (Name, DocumentType, Content) in the SSM crate's document state so SSM's Automation operations (StartAutomationExecution/GetAutomationExecution, and the document lookup they share) resolve it by the same name CloudFormation returns from {"Ref"}; the provider must fail the resource rather than report CREATE_COMPLETE when that registration does not happen. The `aws:changeInstanceState` step execution lives in SSM's automation-execution state and must drive the EC2 instance state, not just echo a status.

Traces (example): cloudformation__ec2__ssm.cfn-ssm-automation-document-stops-ec2-instance-positive.

## Evidence
Failing check cloudformation__ec2__ssm.cfn-ssm-automation-document-stops-ec2-instance-positive [error].
Replay detail, verbatim (tail):
  File "client.py", line 1094, in _make_api_call
    raise error_class(parsed_response, operation_name)
  botocore.errorfactory.AutomationDefinitionNotFoundException: An error occurred (AutomationDefinitionNotFoundException) when calling the StartAutomationExecution operation: Document sim2real-xsvc-stopdoc-499e698faeaf does not exist.
(the document name is a per-run generated value; the requirement is that the name CloudFormation returned resolves)

Expected (AWS): after the stack reaches CREATE_COMPLETE, StartAutomationExecution with DocumentName = the stack's DocumentName output returns an AutomationExecutionId; GetAutomationExecution reaches AutomationExecutionStatus `Success`; DescribeInstances on the stack's InstanceId output then reports State.Name `stopped`.
Actual (emulator): stack reaches CREATE_COMPLETE with both outputs present, StartAutomationExecution is refused 400 AutomationDefinitionNotFoundException "Document <name> does not exist.", so no execution id is minted, no execution reaches Success, and the instance stays running.

## Steps to Reproduce
1. git_clone the repo and run the behavioral suite (run_behavioral_suite) so the emulator is built and booted.
2. Replay check cloudformation__ec2__ssm.cfn-ssm-automation-document-stops-ec2-instance-positive: CreateStack with a template declaring an AWS::IAM::Role (ssm.amazonaws.com trust, ec2:StopInstances), an AWS::EC2::Instance (t3.micro in the test subnet, ImageId from the AL2023 SSM public parameter), and an AWS::SSM::Document of DocumentType `Automation` whose content has one `aws:changeInstanceState` step with DesiredState `stopped`; export Outputs InstanceId ({"Ref": "Instance"}) and DocumentName ({"Ref": "StopDoc"}).
3. Poll DescribeStacks until CREATE_COMPLETE and read both Outputs.
4. Call ssm.StartAutomationExecution(DocumentName=<DocumentName output>, Parameters={"InstanceId": [<InstanceId output>]}). It is refused 400 AutomationDefinitionNotFoundException "Document <name> does not exist." instead of returning an AutomationExecutionId.
5. (Unreachable today) GetAutomationExecution would have to reach Success and DescribeInstances report the instance `stopped`.
