Issue-Source: trace-sweep-divergence
Issue-Trace: route53profiles-profile-shares-dns-firewall-with-vpc
Issue-Step: task#24
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: route53profiles-profile-shares-dns-firewall-with-vpc
Issue-Shape-Id: com.amazonaws.route53profiles#Route53Profiles
Issue-Key: ssm|GetCommandInvocation|2xx->2xx|$.StandardErrorContent
Issue-Related: 20261114T235914Z-ssm-run-command-responses-diverge-sendcommand-and-getcommandinvocation.md
Issue-Related: 20261115T010213Z-ssm-getcommandinvocation-an-in-guest-dns-probe-invocation.md
Issue-Commit: af6e2ca0d6958cf434154027297611bddfae7bb3

# ssm GetCommandInvocation under Route 53 Profiles: the shipped shell command cannot execute (rc 127) and PluginName/Comment/CloudWatchOutputConfig are wrong

> _Note: this issue was identified against the code base at commit `af6e2ca0d6958cf434154027297611bddfae7bb3`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetCommandInvocation

## Description
2 net-new causes in the ssm Run Command read path, both exercised by the same recorded step: a Route 53 Profiles scenario shares a DNS Firewall rule group with a VPC, sends an AWS-RunShellScript command that resolves a name from inside a managed node, and polls GetCommandInvocation for the result. Both causes must be closed before this issue is submitted.

Cause 1 of 2 - the invocation never succeeds because the managed node's guest cannot run the interpreter the command ships (fixture route53profiles-profile-shares-dns-firewall-with-vpc, step task#24, HTTP 200 vs 200).
AWS: Smithy (com.amazonaws.ssm, GetCommandInvocation -> GetCommandInvocationResult) models Status : CommandInvocationStatus (enum), StatusDetails : StatusDetails (@length 0..100), ResponseCode : ResponseCode (@default 0, integer), StandardOutputContent (@length 0..24000) and StandardErrorContent (@length 0..8000). For an AWS-RunShellScript invocation whose guest command exits 0, AWS reports Status 'Success', StatusDetails 'Success', ResponseCode 0, StandardOutputContent carrying exactly what the command wrote to stdout and StandardErrorContent empty. The recording shows that: the in-guest DNS probe prints one newline-terminated line naming the resolved CNAME target and the alias list it was asked about. The concrete host names and the per-run probe suffix in the recorded line are values taken from the request's script and the scenario's firewall configuration - the requirement is that the program the request ships actually runs on the node and its stdout is returned verbatim, not that one recording's host names appear.
Emulator: the invocation reaches a terminal failure instead - Status 'Failed', StatusDetails 'Failed', ResponseCode 127, StandardOutputContent '' and StandardErrorContent 'sh: python3: command not found\n'. 127 is the POSIX shell's 'command not found' exit status, and the stderr text names the missing program: the emulator's managed-node guest has no python3 interpreter, so the shipped shell command exits before producing any output. The replay's poll on Status hit 'Failed' (a fail_on value) after 2 attempts, so the scenario can never converge.
Why: the entire point of this read is to collect the output of work done on the node. A client that sends a command and polls for its result is told the command definitively failed with an exit status its own script never produced, gets no stdout, and a Failed invocation is not retryable by the caller. Any flow gated on the invocation succeeding - here, observing which answer a DNS Firewall rule group shared through a Route 53 Profile returns inside a VPC - terminates in a false failure instead of a readable answer.
Fix: the ssm Run Command guest-execution path (the invocation executor that runs an AWS-RunShellScript document's commands inside a managed node) and the invocation state it writes - the node's guest must provide the interpreters/programs an AWS-RunShellScript payload invokes, and the invocation record GetCommandInvocation projects must carry the command's real captured stdout/stderr and real exit status instead of a 127/Failed invocation with empty stdout.
Traces: route53profiles-profile-shares-dns-firewall-with-vpc#task#24.

Cause 2 of 2 - the same 200 response reports the wrong PluginName and omits members AWS always serializes (same fixture, same step task#24, HTTP 200 vs 200).
AWS: Smithy models GetCommandInvocationResult.PluginName : CommandPluginName (@length min 4), Comment : Comment (@length 0..100) and CloudWatchOutputConfig : CloudWatchOutputConfig (members CloudWatchLogGroupName, CloudWatchOutputEnabled @default false). PluginName is the invoked document's plugin/action name - for AWS-RunShellScript that is 'aws:runShellScript' - and GetCommandInvocationRequest accepts PluginName : CommandPluginName (@length min 4) back on input, which is why the returned label must be the document's plugin name. When the command set neither a comment nor CloudWatch output, AWS still serializes Comment as the empty string and CloudWatchOutputConfig as the object with its defaulted members (empty log-group name, CloudWatchOutputEnabled false).
Emulator: PluginName is 'runShellScriptOnLinux' - an internal platform-specific label, not the document's plugin name - and Comment and CloudWatchOutputConfig are absent from the body (JSON null / key not serialized).
Why: callers feed PluginName back into GetCommandInvocation and key per-plugin output (S3 prefixes, CloudWatch Logs streams) by it, so a label the service will not accept on input breaks that round trip and per-plugin output lookups resolve to nothing. Clients that model Comment/CloudWatchOutputConfig as always present decode null and either fail to deserialize or conclude the command was accepted with output settings it did not have.
Fix: the ssm GetCommandInvocation handler/serializer - derive PluginName from the invoked document's plugin/action name, and project Comment and CloudWatchOutputConfig from the stored command record unconditionally instead of skipping None.
Traces: route53profiles-profile-shares-dns-firewall-with-vpc#task#24.

Related open issues, none of which carries this replay key, so none covers these causes: 20261115T010213Z-ssm-getcommandinvocation-an-in-guest-dns-probe-invocation.md (the same two faults, but keyed to fixture route53resolver-dns-firewall-override-answers-in-vpc#task#21 and to a guest failure that returned no stderr at all), 20261114T235914Z-ssm-run-command-responses-diverge-sendcommand-and-getcommandinvocation.md (the same PluginName/Comment/CloudWatchOutputConfig projection fault, keyed to ec2-instance-connect-pushed-key-served-by-instance-metadata#task#15 and to a Success invocation), 20261115T000000Z-ec2-ssm-a-managed-nodes-guest-has-no-network-programs.md and 20261119T000635Z-ec2-ssm-an-aws-runshellscript-invocation-s-standardoutputcontent.md. The other divergence in this replay (route53profiles GetProfileResourceAssociation, fixture route53profiles-profile-error-paths#task#5) is already owned by 20261104T173229Z-route53profiles-getprofileresourceassociation-refuses-an-unknown-resource-association-id.md and is not refiled here.

## Evidence
Replay of fixture route53profiles-profile-shares-dns-firewall-with-vpc (trace set simparity-wire-aws-v1) against the emulator; 59 steps replayed, 57 passed, 2 diverged, and this is the one divergence no open issue owns (owned_by: [], stub_path: "").

Step task#24, ssm GetCommandInvocation, expected HTTP 200 / actual HTTP 200, body mismatch:
  $.Status: expected 'Success', got 'Failed'
  $.StatusDetails: expected 'Success', got 'Failed'
  $.ResponseCode: expected 0, got 127
  $.StandardOutputContent: expected the stdout of the in-guest DNS probe the command shipped - one newline-terminated line naming the resolved CNAME target and the alias list queried (recording: "cname example.com aliases ['sim2real-probe-r53p-blocked-62a70ef8.example.org']\n"; the host names and the probe-run suffix are per-run values from the request/scenario, the fixed part is that the program's stdout is returned verbatim), got ''
  $.StandardErrorContent: expected '' (empty), got 'sh: python3: command not found\n'
  $.Status (poll): 'Failed' is in the step's fail_on set after 2 attempt(s) - expected 'Success', got 'Failed'
  $.PluginName: expected 'aws:runShellScript', got 'runShellScriptOnLinux'
  $.Comment: expected '' (empty string), got None
  $.CloudWatchOutputConfig: expected {'CloudWatchLogGroupName': '', 'CloudWatchOutputEnabled': False}, got None

The emulator's own stderr text names the cause of the 127: the guest has no python3 on PATH.

Model evidence: Smithy com.amazonaws.ssm GetCommandInvocation -> GetCommandInvocationResult with members Status : CommandInvocationStatus (enum), StatusDetails : StatusDetails (@length 0..100), ResponseCode : ResponseCode (@default 0, integer), StandardOutputContent (@length 0..24000), StandardErrorContent (@length 0..8000), PluginName : CommandPluginName (@length min 4), Comment : Comment (@length 0..100), CloudWatchOutputConfig : CloudWatchOutputConfig; GetCommandInvocationRequest also accepts PluginName : CommandPluginName (@length min 4). CloudWatchOutputConfig.CloudWatchOutputEnabled is @default false.

## Steps to Reproduce
1. Replay wire fixture route53profiles-profile-shares-dns-firewall-with-vpc from trace set simparity-wire-aws-v1 against the emulator.
2. Reach the step that shares the scenario's DNS Firewall rule group with the VPC and then sends ssm SendCommand for AWS-RunShellScript against the scenario's managed node, shipping the in-guest DNS probe (a python3 one-liner), with no Comment and no CloudWatch output options set. Note the returned CommandId.
3. Step task#24: poll ssm GetCommandInvocation with that CommandId and the node's InstanceId until a terminal status.
4. Observe the invocation reaches Status 'Failed' / StatusDetails 'Failed' with ResponseCode 127, empty StandardOutputContent and StandardErrorContent 'sh: python3: command not found', instead of Status/StatusDetails 'Success', ResponseCode 0, the probe's stdout line and empty stderr; the step's poll fails on 'Failed' after 2 attempts.
5. In the same 200 response, observe PluginName is 'runShellScriptOnLinux' rather than the document's plugin name 'aws:runShellScript', and that Comment and CloudWatchOutputConfig are absent (null) rather than '' and {"CloudWatchLogGroupName": "", "CloudWatchOutputEnabled": false}.
