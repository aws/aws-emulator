Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: config__lightsail.history-of-a-never-allocated-static-ip-rejected
Issue-Key: config__lightsail.history-of-a-never-allocated-static-ip-rejected|run#8|Timeout|time.sleep(_PAUSE_S)
Issue-Commit: 87d71c43287cb88d437f1b22d9a50457d3db170d

# config__lightsail: Config never discovers an allocated Lightsail static IP, so AWS::Lightsail::StaticIp has no inventory or history at all (1 cause)

> _Note: this issue was identified against the code base at commit `87d71c43287cb88d437f1b22d9a50457d3db170d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListDiscoveredResources

## Description
ONE root cause, grounded in one failing behavioral check. ONE CAUSE must be closed before submitting.

Not covered by an existing open issue: the open config issues concern recorder restart inertness (20261127T023436Z-config-sqs-a-stopped-then-restarted-configuration-recorder.md), the SQS queue recorded-under-a-recording question (20261122T033830Z-config-sqs-a-queue-created-under-a-recording.md) and request-validation renderings (20261022T190340Z..., 20261030T093351Z...); none of them is about which resource types the recorder observes.

--- Cause 1 of 1: a running configuration recorder never records AWS::Lightsail::StaticIp, so ListDiscoveredResources and GetResourceConfigHistory report nothing for an allocated static IP (check config__lightsail.history-of-a-never-allocated-static-ip-rejected).
AWS: AWS::Lightsail::StaticIp is a resource type Config records — the service's own model enumerates it (Smithy: com.amazonaws.configservice#ResourceType includes 'AWS::Lightsail::StaticIp', alongside AWS::Lightsail::Bucket, AWS::Lightsail::Disk and AWS::Lightsail::Certificate). With a recorder recording it, a static IP allocated through Lightsail's AllocateStaticIp becomes a discovered resource: ListDiscoveredResources(resourceType='AWS::Lightsail::StaticIp', resourceName=<name>, includeDeletedResources=true) reports one resourceIdentifier for it, and GetResourceConfigHistory(resourceType='AWS::Lightsail::StaticIp', resourceId=<that id>) returns configurationItems whose resourceName is that name. A resourceId Config never discovered is refused 400 ResourceNotDiscoveredException with 'Resource ' + the submitted resourceId + ' of resourceType:' + the submitted resourceType + ' is unknown or has not been discovered' (fixed text; both values come from the request) — that refusal is about the RESOURCE, not about the type, which is exactly the distinction the check makes by reading a sibling static IP's history successfully first.
Emulator: records nothing of the type. services/service-com-amazonaws-configservice-starlingdoveservice/src/state/recording.rs declares OBSERVABLE_RESOURCE_TYPES as exactly five entries — AWS::S3::Bucket, AWS::SQS::Queue, AWS::EC2::Instance, AWS::IAM::Role, AWS::IAM::Policy — and its module docs state the consequence: "A resource type no service of this emulator reports is simply not recorded". A recorder records the intersection of that set with its recording group, so even an allSupported recorder never writes a configuration item for a Lightsail static IP. The check's 2-second poll for the allocated IP's resource id therefore never ends and the check is killed before its refusal assertion is reached: "Error: Check timed out after 60.0s". (The check itself allows 900 s for Config to record; the type being unobservable means no budget would help.)
Why: a client cannot see Lightsail resources through Config at all — a Config-driven inventory or compliance flow over Lightsail silently reports an empty account instead of the resources that exist, and because the positive control is unobtainable, the ResourceNotDiscoveredException refusal cannot be told apart from "this type is never recorded here".
Fix: the recorder's observation pass — services/service-com-amazonaws-configservice-starlingdoveservice/src/state/recording.rs (OBSERVABLE_RESOURCE_TYPES plus a per-type observer module beside the existing observed_ec2.rs / observed_iam.rs): observe this account's allocated Lightsail static IPs through Lightsail's own API over the in-process callback (as the S3/SQS/EC2/IAM observers already do) and write configuration items under AWS::Lightsail::StaticIp, including the deletion item when the IP is released, so ListDiscoveredResources (with includeDeletedResources) and GetResourceConfigHistory report them while an id Config never discovered still earns the ResourceNotDiscoveredException above.

Traces (failing check ids, as examples): config__lightsail.history-of-a-never-allocated-static-ip-rejected.

## Evidence
Replay of simparity-behavioral-aws-v1 against the emulator built from this checkout: 169/185 passed (91.4%), bootstrap_signal=false, so the emulator was serving. Per-service line: config__lightsail 0/1 (0%), 1 failed. Both services are bootstrapped (get_bootstrap_status lists service-com-amazonaws-configservice-starlingdoveservice and service-com-amazonaws-lightsail-lightsail-20161128 under "bootstrapped"; only service-com-amazonaws-abusesignal-abusereportapi is pending).

config__lightsail.history-of-a-never-allocated-static-ip-rejected [mismatch]:
  Error: Check timed out after 60.0s
  What the check does: lightsail.allocate_static_ip(staticIpName=<held>), then polls every 2 s (own budget 900 s) for
    config.list_discovered_resources(resourceType='AWS::Lightsail::StaticIp', resourceName=<held>, includeDeletedResources=True)['resourceIdentifiers']
  to become non-empty, asserting "Config never recorded the allocated static IP" otherwise; then
    config.get_resource_config_history(resourceType='AWS::Lightsail::StaticIp', resourceId=<that id>) must name only <held> in configurationItems[].resourceName; and finally
    config.get_resource_config_history(resourceType='AWS::Lightsail::StaticIp', resourceId=<never-allocated name>) must be refused ('ResourceNotDiscoveredException', 'Resource <never> of resourceType:AWS::Lightsail::StaticIp is unknown or has not been discovered').
  expected: the allocated static IP appears as a discovered resource and has a readable history; the never-allocated id is refused as above.
  got:      resourceIdentifiers stays empty forever, so the check is killed in the poll and no assertion after it ever runs.
  Emulator grounding: src/state/recording.rs lines 52-64 — `pub const OBSERVABLE_RESOURCE_TYPES: [&str; 5] = [S3_BUCKET_RESOURCE_TYPE, SQS_QUEUE_RESOURCE_TYPE, observed_ec2::EC2_INSTANCE_RESOURCE_TYPE, observed_iam::IAM_ROLE_RESOURCE_TYPE, observed_iam::IAM_POLICY_RESOURCE_TYPE]`, with the module docs (lines 23-29) stating that a type no service of this emulator reports "is simply not recorded". No Lightsail observer exists, so the type produces no configuration item under any recording group.
  Model grounding: smithy_query(query='shape', shape='ResourceType', model_path='services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/com-amazonaws-configservice-starlingdoveservice.json') enumerates 'AWS::Lightsail::StaticIp', and GetResourceConfigHistory models ResourceNotDiscoveredException (client, httpError 400).

Per-run values (the static IP names from ctx.unique_name, the resource id Config assigns, timestamps, request ids) are not part of the requirement; the requirement is that an allocated static IP is discovered and has a history, and that a never-allocated id earns the 400 ResourceNotDiscoveredException message template above with the request's values filled in.

## Steps to Reproduce
1. git_clone the repo and build/boot the emulator from this checkout.
2. Replay the behavioral suite simparity-behavioral-aws-v1 and observe config__lightsail 0/1.
3. By hand: set up a running customer-managed configuration recorder (allSupported, or an inclusion group naming AWS::Lightsail::StaticIp) with a delivery channel; call lightsail AllocateStaticIp(staticIpName=<name>).
4. Poll config ListDiscoveredResources(resourceType='AWS::Lightsail::StaticIp', resourceName=<name>, includeDeletedResources=true) — observe resourceIdentifiers stays empty indefinitely instead of reporting the allocated static IP.
5. Call config GetResourceConfigHistory(resourceType='AWS::Lightsail::StaticIp', resourceId=<a never-allocated name>) and confirm the 400 ResourceNotDiscoveredException message is 'Resource <that name> of resourceType:AWS::Lightsail::StaticIp is unknown or has not been discovered'.
6. Re-replay config__lightsail.history-of-a-never-allocated-static-ip-rejected to confirm the fix.
