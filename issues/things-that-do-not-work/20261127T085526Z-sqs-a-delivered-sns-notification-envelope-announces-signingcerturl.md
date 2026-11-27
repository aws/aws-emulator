Issue-Source: trace-sweep-divergence
Issue-Trace: sns-sqs-subscription-defaults-envelope-then-raw-delivery-queue-policy-cleared-and-not-found
Issue-Step: task#9
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: sns-sqs-subscription-defaults-envelope-then-raw-delivery-queue-policy-cleared-and-not-found
Issue-Shape-Id: com.amazonaws.sns#AmazonSimpleNotificationService
Issue-Key: sqs|ReceiveMessage|2xx->2xx|$.Messages[*].Body
Issue-Commit: 91b993ae414910f3a059c9a9af3882a70940e0b6

# sqs: a delivered SNS notification envelope announces SigningCertURL/UnsubscribeURL at an authority the receiving client is not reached on (1 cause)

> _Note: this issue was identified against the code base at commit `91b993ae414910f3a059c9a9af3882a70940e0b6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ReceiveMessage

## Description
One net-new root cause. REOPENS the RESOLVED issue 20261120T082601Z-sqs-the-sns-notification-delivered-into-a-subscribed.md ("sqs: the SNS notification delivered into a subscribed queue announces a SigningCertURL host the receiving client is not reached on") — a resolved issue covers nothing, and its fix only moved the announced authority: the envelope now names a loopback authority whose port is NOT the port the receiving client reaches this emulator on, so the same member is still unfollowable. Also related and NOT covering this step (neither carries this fixture/step as a replay key): the OPEN issue 20261125T032512Z-sns-the-signed-notification-envelope-announces-signingcerturl-at.md, which is a BEHAVIORAL issue under the `sns` heading grounded in nine check ids and describes the authority as the real regional host sns.<region>.amazonaws.com (the state this checkout has since moved off of), and 20261121T105340Z-iam-lambda-sns-sqs-an-sns-https-subscription.md (the https push surface). This finding is the WIRE corpus observation on the plain SQS-subscription envelope path, under service token `sqs`.

=== Cause 1 of 1: the Notification envelope delivered into a subscribed SQS queue announces SigningCertURL and UnsubscribeURL on an authority that is not the address the receiving client reaches this emulator at ===

AWS:      Smithy (com.amazonaws.sqs#AmazonSQS) models ReceiveMessage's Message.Body only as an opaque String ("The message's contents (not URL-encoded)") — the SNS notification JSON carried inside it is recorded behavior only. The recording shows the envelope's member set Type="Notification", MessageId, TopicArn, Message, Timestamp, SignatureVersion="1", Signature, SigningCertURL and UnsubscribeURL, with SigningCertURL of the form https://<sns endpoint authority>/SimpleNotificationService-<32 hex characters>.pem on the default https port (no port component) and UnsubscribeURL of the form https://<the SAME authority>/?Action=Unsubscribe&SubscriptionArn=<the subscription's ARN>. Against real AWS that authority is the Region's SNS endpoint, sns.<region>.amazonaws.com. The requirement the recording states, stripped of per-environment values, is twofold and both halves must hold at once: (a) every URL member of ONE envelope names ONE authority, and (b) that authority is the one the RECIPIENT of the envelope reaches this service at and is answered on — because the AWS Developer Guide section "Verifying the signatures of Amazon SNS messages" tells a subscriber to RETRIEVE SigningCertURL and extract the public key from the returned certificate, and UnsubscribeURL is a link the recipient FOLLOWS. The concrete host name, the port, the 32-hex certificate id, the Timestamp and the Signature bytes are per-run/per-environment values and are not part of the requirement; the authority-is-reachable-by-the-reader rule is.

Emulator: the envelope is delivered with an authority that is neither the recorded regional endpoint nor an address the client is reached on. The replay reports the body mismatch on $.Messages[0].Body classified `host_not_own`: the emulator served SigningCertURL = https://127.0.0.1:44391/SimpleNotificationService-<32 hex>.pem and UnsubscribeURL = https://127.0.0.1:44391/?Action=Unsubscribe&SubscriptionArn=<the subscription ARN>, while the client that performed the ReceiveMessage in that very run reaches the emulator's data listener on a different port (:46327 in the replay's emulator_detail). The source of the value in this checkout is services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/state/notification_signature.rs lines 390-406: `announced_authority(ctx, region, Recipient::ClientOfTheEmulator)` returns `service_common::data_plane_addressing::served_endpoint_authority(ctx.endpoint().as_str())` and falls back to `format!("sns.{region}.amazonaws.com")`, and the value it produced for this delivery is a host:port the receiving client is not served on. Hypothesis, since the replay shows only the published value: the authority is taken from the endpoint recorded for the context that PUBLISHED the message (or from a non-data listener / an earlier binding) rather than from the data endpoint every client of this emulator is answered on, so a client polling on the data port is handed a stale sibling address. Everything else in the envelope matched: Type, MessageId, TopicArn, Message and SignatureVersion are as recorded, and the raw-delivery and queue-policy steps of the same fixture passed.

Why:      a client doing exactly what AWS documents — read SigningCertURL out of the delivered body, GET it, and verify the RSASSA-PKCS1-v1_5 Signature over SNS's canonical string — is handed an address nothing answers it on, so the fetch fails and EVERY notification is unverifiable even though Signature and SignatureVersion are present and internally consistent; the client cannot tell an authentic notification from a forged one, which is the whole purpose of those members. The same wrong authority makes UnsubscribeURL a dead link, so the one-click unsubscribe path AWS guarantees in every notification is unusable. Because the member is syntactically a well-formed https URL, the failure surfaces only as a connection error deep inside the subscriber's verification code rather than as an SNS or SQS error.

Fix:      the SNS notification/confirmation envelope's URL-member addressing — `announced_authority` in state/notification_signature.rs (the `Recipient::ClientOfTheEmulator` arm and the `served_endpoint_authority` lookup it delegates to) and the sibling SubscribeURL / UnsubscribeURL construction on the publish fan-out path — must resolve the announced authority to the data endpoint the RECIPIENT of this document is served at, the same host:port that answers the ReceiveMessage which returns the body, and must keep both members on that one authority so the document still names one origin. A per-resource or per-listener authority is acceptable only if a GET of /SimpleNotificationService-<32 hex>.pem on it is answered 200 with the PEM the Signature verifies under.

Traces:   fixture sns-sqs-subscription-defaults-envelope-then-raw-delivery-queue-policy-cleared-and-not-found step task#9 (ReceiveMessage, verdict mismatch, 200 vs 200, key $.Messages[0].Body). Code read in this checkout: services/service-com-amazonaws-sns-amazonsimplenotificationservice/src/state/notification_signature.rs lines 360-406 (the one-authority doc contract and `announced_authority`), 408-423 (`addresses_certificate`, `published_certificate`).

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 44 fixtures replayed, 370 steps, 369 passed, 1 mismatched, 0 errored, 0 blocked, score 99.7%, bootstrap_signal false, emulator_up true ("/__admin/health answered 200 and the :46327 data listener accepts"). The single divergence, verbatim:

  service=sqs | operation=ReceiveMessage | verdict=mismatch | expected_status=200 actual_status=200
  fixture_id=sns-sqs-subscription-defaults-envelope-then-raw-delivery-queue-policy-cleared-and-not-found
  step_key=task#9
  divergence_key=service=sqs|ReceiveMessage|status=200->200|keys=$.Messages[0].Body
  detail: $.Messages[0].Body: host_not_own
  stub_path=""  owned_by=[]  affected_count=1

Expected (recorded AWS) $.Messages[0].Body:
  {
    "Type" : "Notification",
    "MessageId" : "<message id>",
    "TopicArn" : "arn:aws:sns:us-west-2:000000000000:<topic>",
    "Message" : "sim2real-probe-message-envelope",
    "Timestamp" : "<iso-8601>",
    "SignatureVersion" : "1",
    "Signature" : "<base64>",
    "SigningCertURL" : "https://sns.us-west-2.amazonaws.com/SimpleNotificationService-<32 hex>.pem",
    "UnsubscribeURL" : "https://sns.us-west-2.amazonaws.com/?Action=Unsubscribe&SubscriptionArn=arn:aws:sns:us-west-2:000000000000:<topic>:<sub id>"
  }

Actual (emulator), same members, same MessageId/TopicArn/Message/SignatureVersion, differing only in the authority of the two URL members:
    "SigningCertURL" : "https://127.0.0.1:44391/SimpleNotificationService-<32 hex>.pem",
    "UnsubscribeURL" : "https://127.0.0.1:44391/?Action=Unsubscribe&SubscriptionArn=arn:aws:sns:us-west-2:000000000000:<topic>:<sub id>"

The harness's verdict on that member is `host_not_own`: the announced host:port is not one this emulator is reached on — the client that issued this ReceiveMessage is served on the data listener :46327, not :44391. The Timestamp and Signature bytes also differ between the two bodies; those are per-run values and are NOT part of this finding. Likewise the literal port numbers (44391, 46327), the loopback address, the 32-hex certificate id, the topic name, the message id and the subscription id are per-run values — the requirement is that the envelope's URL members name one authority and that it be the authority the recipient of the envelope reaches this service at.

No status or error-code divergence is involved: the step is a 200 whose body carries an unfollowable address.

## Steps to Reproduce
1. Build and boot the emulator from this checkout; note the data endpoint your client is served on (host:port).
2. sqs CreateQueue --queue-name q; sns CreateTopic --name t; set the queue policy so sns:SendMessage from the topic is allowed.
3. sns Subscribe --topic-arn <t> --protocol sqs --notification-endpoint <queue ARN> with RawMessageDelivery left at its default (off), so the signing members survive into the body.
4. sns Publish --topic-arn <t> --message sim2real-probe-message-envelope.
5. sqs ReceiveMessage --queue-url <q> and parse Messages[0].Body as JSON. Read "SigningCertURL" and "UnsubscribeURL". Against AWS both name one authority — the Region's SNS endpoint on the default https port — and that authority is one the receiving client reaches the service at. Against the emulator both read https://127.0.0.1:<some other port>/..., a port the client performing this ReceiveMessage is not served on.
6. Follow the member the way the Developer Guide tells a subscriber to: GET the announced SigningCertURL. Against AWS it answers 200 with a PEM containing BEGIN CERTIFICATE whose RSA public key verifies the envelope's Signature over SNS's canonical string. Against the emulator nothing answers that authority, so the notification cannot be verified at all.
7. GET the announced UnsubscribeURL: against AWS it reaches the service and removes the subscription; against the emulator it reaches nothing.
8. Check the one-origin invariant after the fix: SigningCertURL and UnsubscribeURL of the SAME body must name one authority, and that authority must be the host:port this client is served on (a GET of /SimpleNotificationService-<32 hex>.pem on it must answer 200). Ignore the certificate id, Timestamp and Signature bytes — per-run values.
9. Or re-replay fixture sns-sqs-subscription-defaults-envelope-then-raw-delivery-queue-policy-cleared-and-not-found, step task#9.
