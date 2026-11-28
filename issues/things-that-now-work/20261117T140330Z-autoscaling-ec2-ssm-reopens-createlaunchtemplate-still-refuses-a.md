Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: autoscaling__ec2__ssm.launch-template-resolve-ssm-alias-launch-blocked-while-parameter-is-absent
Issue-Key: ec2|CreateLaunchTemplate|SsmInvalidParameter
Issue-Commit: 845c80d629e2b6091bbba6d5ecc9b7e37b15e9ab

# autoscaling__ec2__ssm: REOPENS — CreateLaunchTemplate still refuses a resolve:ssm: ImageId (SsmInvalidParameter), now for a user-created parameter that exists

> _Note: this issue was identified against the code base at commit `845c80d629e2b6091bbba6d5ecc9b7e37b15e9ab`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateLaunchTemplate

## Description
REOPENS 20261106T125205Z-autoscaling-ec2-ecs-createlaunchtemplate-refuses-a-resolve-ssm.md: its fix left the call refused.

Cause 1 of 1: EC2 CreateLaunchTemplate refuses a LaunchTemplateData.ImageId in the documented resolve:ssm: alias form even when the named parameter exists in the caller's own account.
AWS: Smithy com.amazonaws.ec2#CreateLaunchTemplate, input LaunchTemplateData (RequestLaunchTemplateData) member ImageId, whose modeled documentation states the ID may instead be "a Systems Manager parameter, using one of the following formats. The Systems Manager parameter will resolve to an AMI ID on launch" and lists resolve:ssm:<parameter-name> among them. Recorded behavior: with ImageId="resolve:ssm:<a String parameter of DataType aws:ec2:image holding a valid ami- id>" AWS answers HTTP 200 with the LaunchTemplate element, stores the alias verbatim, and resolves it only at launch time; an instance launched from it carries the AMI id the parameter held. The refusal "The following aliases are invalid: <name>" belongs on the LAUNCH path — when the parameter has since been deleted, the Auto Scaling activity records StatusCode Failed with StatusMessage "The following aliases are invalid: <parameter name>. Launching EC2 instance failed." (fixed text around the parameter name from the template).
Emulator: the create itself is refused, HTTP 400 with Error.Code SsmInvalidParameter and message "The following aliases are invalid: <parameter name>", and it keeps being refused for the whole 30 s window the check allows for parameter propagation — even though the check created that parameter with PutParameter(DataType="aws:ec2:image") immediately before, so the alias names a parameter that exists in the emulator's own store. The prior fix for this operation evidently addressed only the AWS-published /aws/service/... namespace; the eager resolution at create time remains.
Why: the documented idiom of pinning an AMI by SSM alias is still unusable, so nothing downstream of a launch template can be exercised: the ASG launch that must resolve the alias, the failed scaling activity recorded when the parameter disappears, and the repaired scale-out after the parameter is restored are all unreachable.
Fix: the CreateLaunchTemplate handler and its ImageId validation in the ec2 service crate — accept every resolve:ssm: form (name, name:version, name:label, ARN), store the alias unresolved in the launch-template version, and resolve it on the launch path, where an unresolvable alias becomes the launch failure above.
Traces: autoscaling__ec2__ssm.launch-template-resolve-ssm-alias-launch-blocked-while-parameter-is-absent.

## Evidence
Behavioral suite, service heading `### autoscaling__ec2__ssm`, check launch-template-resolve-ssm-alias-launch-blocked-while-parameter-is-absent.

The check first resolves an AMI id from the public AL2023 parameter, then:

    ssm.put_parameter(Name=param_name, Value=ami_id, Type="String", DataType="aws:ec2:image")
    ec2.create_launch_template(
        LaunchTemplateName=<fresh name>,
        LaunchTemplateData={"ImageId": f"resolve:ssm:{param_name}", "InstanceType": "t3.micro"}, ...)

retrying only SsmInvalidParameter for 30 s. Replay:

    botocore.exceptions.ClientError: An error occurred (SsmInvalidParameter) when calling the CreateLaunchTemplate operation: The following aliases are invalid: sim2real-lthoame-param-78550492d3ad

Expected (AWS): HTTP 200 with the LaunchTemplate element; the alias is stored unresolved. The later, intended refusal is the Auto Scaling activity StatusMessage "The following aliases are invalid: <param>. Launching EC2 instance failed." after the parameter is deleted — never reached here.

The parameter/template names and the AMI id are per-run values; the requirement is that the create succeeds with 200 for an existing parameter.

## Steps to Reproduce
1. Build and boot the emulator from this checkout.
2. ssm PutParameter Name=<fresh name>, Value=<a valid ami- id>, Type="String", DataType="aws:ec2:image".
3. ec2 CreateLaunchTemplate with LaunchTemplateName=<fresh name> and LaunchTemplateData={"ImageId": "resolve:ssm:<that parameter name>", "InstanceType": "t3.micro"}.
4. Observe HTTP 400 SsmInvalidParameter "The following aliases are invalid: <parameter name>", repeatedly for at least 30 s; AWS answers 200 and DescribeLaunchTemplateVersions reports ImageId exactly as submitted.
5. Re-replay the check listed in trace_ids.
