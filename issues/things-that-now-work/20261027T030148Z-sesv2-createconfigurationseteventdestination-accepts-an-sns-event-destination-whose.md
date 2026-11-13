Issue-Severity: MEDIUM
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish
Issue-Key: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish|_error#15|AssertionError|raise AssertionError(
Issue-Commit: 464a2835635950cb0afe23fa38546fc133c528e8

# sesv2 CreateConfigurationSetEventDestination accepts an SNS event destination whose topic policy explicitly denies ses.amazonaws.com:Publish

> _Note: this issue was identified against the code base at commit `464a2835635950cb0afe23fa38546fc133c528e8`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateConfigurationSetEventDestination

## Description
ONE root cause. Cause 1 of 1: CreateConfigurationSetEventDestination performs no authorization probe against the SnsDestination topic, so a destination SES could never publish to is created.

AWS: Smithy: sesv2 CreateConfigurationSetEventDestination (POST /v2/email/configuration-sets/{ConfigurationSetName}/event-destinations) models exactly five errors — AlreadyExistsException (400), BadRequestException (400), LimitExceededException (400), NotFoundException (404), TooManyRequestsException (429) — and `EventDestinationDefinition$SnsDestination` carries a required `TopicArn`. SES validates at create time that it can publish to the named topic, and refuses a topic whose resource policy does not permit the `ses.amazonaws.com` service principal `sns:Publish`; of the modelled errors only `BadRequestException` fits "cannot publish to that topic". Expected refusal: HTTP 400, error code `BadRequestException`, message "Access denied to SNS topic <TOPIC ARN>. Verify the SNS topic policy grants Amazon SES the sns:Publish permission." — the fixed text is "Access denied to SNS topic <" / ">. Verify the SNS topic policy grants Amazon SES the sns:Publish permission." and the only variable part is the TopicArn taken from the request. Hypothesis: the check that grounds this issue is marked in-tree as a draft never replayed against real AWS, so the exact code and message above are its stated expectation rather than a recorded AWS response; what is NOT in doubt is that SES must refuse — or at minimum not silently accept — a destination it cannot publish to, and that the emulator performs no check of any kind. A fixer should confirm the code/message against AWS and keep the rule (refusal keyed on the topic policy) either way.

Emulator: the call succeeds. With the identical configuration set, destination name and MatchingEventTypes, the create against a topic whose policy carries a single statement {Sid, Effect: "Deny", Principal: {Service: "ses.amazonaws.com"}, Action: "SNS:Publish", Resource: <that topic>, Condition: {StringEquals: {"AWS:SourceAccount": <account>}}} returns 200 and the destination is then listed by GetConfigurationSetEventDestinations. The check's own helper raises on unexpected success: `AssertionError: CreateConfigurationSetEventDestination succeeded against an SNS topic whose policy explicitly denies ses.amazonaws.com:Publish; SES must refuse a destination it cannot publish to`. The policy was independently confirmed readable at SNS through GetTopicAttributes before SES was called, so the acceptance cannot be blamed on a policy that had not landed — the emulator stores `SnsDestination.TopicArn` as an opaque string and never evaluates the topic's resource policy.

Why: a client that wires bounce/complaint/send events to SNS is told the destination is live and healthy, then loses every event in the window before it notices nothing is arriving — the misconfiguration that create-time validation exists to surface is pushed to silent delivery-time loss, with no error anywhere in the control plane to diagnose it.

Fix: the CreateConfigurationSetEventDestination handler's validation (and the matching UpdateConfigurationSetEventDestination path) — resolve the SnsDestination TopicArn, evaluate the topic's resource policy for `sns:Publish` by the `ses.amazonaws.com` service principal, and refuse through the operation's already-modelled BadRequestException channel instead of storing the ARN unchecked.

Traces: sesv2__sns.event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish. Note on history: this check previously failed earlier in its sequence and three open issues record those earlier blockers — 20261023T183615Z-sesv2-sns-the-ses-event-destination-sns-topic.md (CreateConfigurationSet refused 'AmbiguousService'), 20261025T220900Z-sesv2-sns-the-ses-event-destination-topic-policy.md (CreateConfigurationSet a 501 stub) and 20261026T024515Z-sesv2-sns-the-ses-event-destination-topic-policy-30dc4c.md (CreateConfigurationSetEventDestination a 501 stub). All three are still open and none covers this divergence: the call now SUCCEEDS, and the failure has moved to the missing topic-policy validation described here.

## Evidence
Replay report, service token `sesv2` (section `sesv2__sns`, 0/1 passed), check `event-destination-requires-a-topic-policy-that-actually-permits-ses-to-publish` — verdict `mismatch`, not marked ALREADY FILED and not marked OWNED by any open issue.

Expected: CreateConfigurationSetEventDestination against an SNS topic whose policy explicitly Denies ses.amazonaws.com `SNS:Publish` is refused HTTP 400 with error code `BadRequestException` and message "Access denied to SNS topic <TOPIC ARN>. Verify the SNS topic policy grants Amazon SES the sns:Publish permission." (fixed text apart from the request's TopicArn), leaving no destination behind — while the identical call against an allow-policy topic succeeds, and replacing the Deny with an Allow makes the identical call succeed.

Actual: the create against the deny-policy topic returns 200 and the destination is listed by GetConfigurationSetEventDestinations. Reported failure:

    File "check_sesv2__sns_event_destination_requires_a_topic_policy_that_actually_permits_ses_to_publish.py", line 120, in run
      code, message = _error(lambda: sesv2.create_configuration_set_event_destination(...))
    File "...", line 273, in _error
      raise AssertionError(...)
    AssertionError: CreateConfigurationSetEventDestination succeeded against an SNS topic whose policy explicitly denies ses.amazonaws.com:Publish; SES must refuse a destination it cannot publish to

The policy write was confirmed by an independent SNS GetTopicAttributes read (the check's `_put_policy` raises if the Deny statement never becomes visible), so the acceptance is a missing authorization probe, not a propagation race.

## Steps to Reproduce
1. sts:GetCallerIdentity to obtain the account id.
2. sesv2:CreateConfigurationSet(ConfigurationSetName=<cs>).
3. sns:CreateTopic twice, giving an allow topic and a deny topic.
4. On the allow topic, sns:SetTopicAttributes(AttributeName="Policy") with one statement {Sid: "sim2realSesPublish", Effect: "Allow", Principal: {Service: "ses.amazonaws.com"}, Action: "SNS:Publish", Resource: <allow topic>, Condition: {StringEquals: {"AWS:SourceAccount": <account>}}}; confirm it with sns:GetTopicAttributes.
5. sesv2:CreateConfigurationSetEventDestination(ConfigurationSetName=<cs>, EventDestinationName="sim2real-ed-policy-probe", EventDestination={Enabled: true, MatchingEventTypes: ["SEND","BOUNCE"], SnsDestination: {TopicArn: <allow topic>}}) — positive control, succeeds and is listed by GetConfigurationSetEventDestinations.
6. sesv2:DeleteConfigurationSetEventDestination for that name, so the negative leg uses the identical call.
7. On the deny topic, set the SAME statement with Effect "Deny" and confirm it with sns:GetTopicAttributes.
8. Repeat step 5 with SnsDestination.TopicArn = <deny topic>. EXPECT HTTP 400 BadRequestException "Access denied to SNS topic <deny topic>. Verify the SNS topic policy grants Amazon SES the sns:Publish permission." and no destination created. OBSERVE 200 and the destination present in GetConfigurationSetEventDestinations.
9. Restore leg: change the deny topic's statement back to Effect "Allow" and repeat step 8's call — it must succeed (this leg must keep working after the fix, so the refusal is keyed on the policy and not on the topic).
