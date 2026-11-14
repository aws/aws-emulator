Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-ga-prod-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption
Issue-Key: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule|run#23|Timeout|time.sleep(10)
Issue-Key: s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers|run#24|Timeout|time.sleep(10)
Issue-Key: s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption|run#33|Timeout|time.sleep(10)
Issue-Commit: 65fd8797851bb5c5937d1ea0792f5c718c73df47

# s3__ses__sesv2: SES email receiving never runs — mail accepted by SendEmail for a recipient matched by the active receipt rule set is never stored in the S3Action bucket, so every inbound S3-delivery scenario hangs

> _Note: this issue was identified against the code base at commit `65fd8797851bb5c5937d1ea0792f5c718c73df47`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
CreateReceiptRule

## Description
Cause 1 of 1: the emulator has no inbound (receiving) pipeline. Mail that SESv2 SendEmail accepts for a recipient on the account's own verified domain is never matched against the ACTIVE receipt rule set, and the rule's S3Action never writes the raw message into its bucket, so every check that waits for the received object polls until the suite budget expires.

AWS: Smithy (com.amazonaws.ses): CreateReceiptRuleSet / CreateReceiptRule / SetActiveReceiptRuleSet build the account's single active rule set; ReceiptRule carries Recipients ("The recipient domains and email addresses that the receipt rule applies to"), Actions, Enabled and ScanEnabled ("If true, then messages that this receipt rule applies to are scanned for spam and viruses"). S3Action is documented as: "When included in a receipt rule, this action saves the received message to an Amazon S3 bucket", with required BucketName, an ObjectKeyPrefix ("similar to a directory name") and an optional KmsKeyArn whose doc states the mail "is encrypted by Amazon SES using the Amazon S3 encryption client before the mail is saved". So for mail delivered to a recipient the active rule matches, AWS writes one object under ObjectKeyPrefix holding the raw MIME message (plus the one-off AMAZON_SES_SETUP_NOTIFICATION object a freshly activated rule set produces); with ScanEnabled true the stored message additionally carries Received-SPF, Authentication-Results (dkim=pass for mail signed by the verified domain), X-SES-Spam-Verdict and X-SES-Virus-Verdict headers; with KmsKeyArn the object is the S3 encryption client envelope (x-amz-wrap-alg=kms metadata and an x-amz-matdesc naming the CMK and the triggering rule) rather than plaintext. Recorded behavior: all three checks complete against real AWS inside their own 300s/600s delivery budgets.

Emulator: the whole setup path succeeds — the stand-in domain verifies (DKIM SUCCESS), CreateReceiptRuleSet/CreateReceiptRule (including the KmsKeyArn variant)/SetActiveReceiptRuleSet all return 200, and sesv2 SendEmail returns 200 with a MessageId — but nothing is ever delivered: s3 ListObjectsV2 on the configured prefix (inbox/, scan/, the KMS check's prefix) returns no Contents on every poll, and all three checks end as "Check timed out after 60.0s" inside that first delivery poll loop. The SESv2 SendEmail handler documents its own scope as appending the message to "this service's outbound-message log" — there is no hand-off to the SES receiving side, no receipt-rule matching, and consequently no S3 write, no scan headers and no KMS envelope.

Why: a client that uses SES email receiving (the documented inbound-to-S3 pattern) sees every control-plane call succeed and then blocks forever waiting for a message that never arrives — no bounded error, just a hang. Anything built on the stored object (scan verdicts/authentication results, encrypted-at-rest inbound archives, downstream processing keyed off the S3 object) cannot be emulated or tested at all.

Fix: the receiving pipeline. The SES v1 crate must evaluate the active receipt rule set against the recipients of a message the account sends to one of its own verified domains and execute the matched rule's actions — the receipt-rule/active-rule-set state and action execution in services/service-com-amazonaws-ses-simpleemailservice/src/ (ops/create_receipt_rule.rs, ops/set_active_receipt_rule_set.rs and the crate's state module) — with the outbound send path handing the assembled message over: services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs (and the v1 SendEmail/SendRawEmail equivalents). The S3Action execution owns the object key (ObjectKeyPrefix + message id), the raw MIME body, the ScanEnabled header stamping and the KmsKeyArn envelope.

Not covered by what is already open: 20261026T065823Z-sesv2-sns-sqs-ses-publishes-no-events-to concerns OUTBOUND event publication to SNS event destinations, and the auto-filed SendRawEmail stub issue concerns only that one operation being a 501 scaffold; neither makes an accepted message arrive in a receipt rule's S3 bucket.

Traces: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule, s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers, s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption (all three time out in the loop that polls the S3Action bucket for the received object).

## Evidence
Expected (AWS): after SetActiveReceiptRuleSet on a rule set whose rule matches the recipient and carries S3Action{BucketName, ObjectKeyPrefix}, mail sent to that recipient appears as exactly one object under the prefix (excluding inbox/AMAZON_SES_SETUP_NOTIFICATION), whose body is the raw MIME message carrying the probe's Subject, the To recipient and a DKIM-Signature with d=<verified domain>. With ScanEnabled=true the stored message also carries Received-SPF, Authentication-Results (containing dkim=pass), X-SES-Spam-Verdict and X-SES-Virus-Verdict (each PASS or FAIL). With KmsKeyArn set, HeadObject metadata carries x-amz-wrap-alg=kms and an x-amz-matdesc whose kms_cmk_id is the configured key ARN and whose aws:ses:rule-name is the triggering rule, and the body is ciphertext (the plaintext token is not present).

Actual (emulator): sesv2 SendEmail returns 200 with a MessageId, then ListObjectsV2(Bucket=<rule bucket>, Prefix=<ObjectKeyPrefix>) returns an empty listing on every poll; no object — not even AMAZON_SES_SETUP_NOTIFICATION — is ever created, so the delivery wait never completes and the three checks are killed at the 60s budget with "Check timed out after 60.0s". No scan headers, no DKIM-Signature and no KMS envelope can be observed because no stored message exists.

Supporting emulator evidence: the SESv2 SendEmail handler (services/service-com-amazonaws-sesv2-simpleemailservice-v2/src/ops/send_email.rs, module doc) describes its behavior as appending the message to the service's outbound-message log only; nothing consults the SES receipt-rule state.

## Steps to Reproduce
1. Verify a domain for sending in SESv2 (CreateEmailIdentity + publish the Easy DKIM CNAMEs; the emulator reports DkimAttributes.Status=SUCCESS).
2. s3 CreateBucket(<bucket>) and PutBucketPolicy allowing ses.amazonaws.com s3:PutObject on arn:aws:s3:::<bucket>/inbox/*.
3. ses CreateReceiptRuleSet(RuleSetName=<set>) then ses CreateReceiptRule(RuleSetName=<set>, Rule={Name:..., Enabled:true, ScanEnabled:false, Recipients:[<user>@<domain>], Actions:[{S3Action:{BucketName:<bucket>, ObjectKeyPrefix:"inbox/"}}]}).
4. ses SetActiveReceiptRuleSet(RuleSetName=<set>).
5. sesv2 SendEmail(FromEmailAddress=<sender>@<domain>, Destination.ToAddresses=[<user>@<domain>], Content.Simple with a unique Subject) — returns 200 with a MessageId.
6. Poll s3 ListObjectsV2(Bucket=<bucket>, Prefix="inbox/"): it stays empty indefinitely instead of yielding one raw MIME object for the message.
7. Repeat with ScanEnabled=true (expect Received-SPF / Authentication-Results / X-SES-Spam-Verdict / X-SES-Virus-Verdict on the stored message) and with S3Action.KmsKeyArn set (expect an x-amz-wrap-alg=kms envelope); neither can be observed because no object is ever written.

Replay: s3__ses__sesv2.mail-to-a-verified-domain-is-received-into-s3-by-a-receipt-rule, s3__ses__sesv2.inbound-message-carries-authentication-and-scan-headers, s3__ses__sesv2.s3-action-object-key-prefix-and-kms-encryption
