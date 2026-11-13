// SPDX-License-Identifier: CC0-1.0

//! End-to-end cover for the SIGNATURE a delivered notification carries, driven over the
//! wire through the real router with SQS mounted beside SNS, so the envelope is read where a
//! subscriber reads it: on the queue.
//!
//! The rule asserted here: every notification SNS delivers in its JSON envelope carries
//! `SignatureVersion`, `Signature` and `SigningCertURL`, where the version is the topic's own
//! `SignatureVersion` attribute ("By default SignatureVersion is set to 1",
//! `SetTopicAttributesInput$AttributeName`), the certificate URL leads to the X.509
//! certificate of the key that signed it, and the signature is the RSA signature of the
//! canonical string built from the delivered `Message`, `MessageId`, `Subject` (if present),
//! `Timestamp`, `TopicArn` and `Type` — over SHA-1 for version 1 and SHA-256 for version 2
//! (Amazon SNS Developer Guide, "Verifying the signatures of Amazon SNS messages"). A RAW
//! delivery carries no envelope at all, so it carries no signature either.
//!
//! Every assertion is made the way a verifying subscriber makes it: the certificate is
//! FETCHED from the URL the notification carried, the canonical string is rebuilt from the
//! members that arrived, and the signature is checked against the certificate's public key
//! with the digest the declared version selects. The rows differ along what the rule
//! generalises over: the version the topic asks for (none, each published value, and back
//! again), whether the message carried a subject, and whether it carried attributes.

use serde_json::Value;
use x509_cert::der::DecodePem;

use super::sqs_subscription_delivery::{
    drain, element, emulator, fetch, given_queue, sns, Emulator,
};

/// The members of a delivered `Notification` that are signed, in the order the canonical
/// string carries them (Developer Guide, "Verifying the signatures of Amazon SNS messages").
const SIGNED_MEMBERS: &[&str] = &[
    "Message",
    "MessageId",
    "Subject",
    "Timestamp",
    "TopicArn",
    "Type",
];

/// One row of the signature table: the case, the `SignatureVersion` the topic is set to
/// (`None` leaves the topic as created), the subject the publish carries, and the version the
/// delivered notification must declare.
struct Row {
    case: &'static str,
    asked_for: Option<&'static str>,
    subject: Option<&'static str>,
    attributed: bool,
    version: &'static str,
}

/// The canonical string a subscriber rebuilds from the notification it received.
fn canonical_string(envelope: &Value) -> String {
    let mut canonical = String::new();
    for member in SIGNED_MEMBERS {
        let Some(value) = envelope.get(*member).and_then(Value::as_str) else {
            continue;
        };
        canonical.push_str(member);
        canonical.push('\n');
        canonical.push_str(value);
        canonical.push('\n');
    }
    canonical
}

/// Whether `signature` is the signature of `signed` under the public key of the certificate
/// `certificate_pem`, with the digest `version` selects — the check the documented
/// verification pattern performs.
fn verifies(certificate_pem: &str, version: &str, signed: &str, signature: &[u8]) -> bool {
    let certificate = x509_cert::Certificate::from_pem(certificate_pem)
        .unwrap_or_else(|error| panic!("the published certificate must parse: {error}"));
    let public_key = certificate
        .tbs_certificate
        .subject_public_key_info
        .subject_public_key
        .raw_bytes();
    // "If the SignatureVersion is 1, Signature is a Base64-encoded SHA1withRSA signature …
    // If the SignatureVersion is 2, … SHA256withRSA" — the two verification algorithms a
    // subscriber picks between on that member.
    match version {
        "2" => ::aws_lc_rs::signature::UnparsedPublicKey::new(
            &::aws_lc_rs::signature::RSA_PKCS1_2048_8192_SHA256,
            public_key,
        )
        .verify(signed.as_bytes(), signature)
        .is_ok(),
        _ => ::aws_lc_rs::signature::UnparsedPublicKey::new(
            &::aws_lc_rs::signature::RSA_PKCS1_1024_8192_SHA1_FOR_LEGACY_USE_ONLY,
            public_key,
        )
        .verify(signed.as_bytes(), signature)
        .is_ok(),
    }
}

/// The certificate published at `url`, fetched through this emulator as a subscriber would.
async fn certificate_at(emulator: &Emulator, case: &str, url: &str) -> String {
    let (status, body) = fetch(emulator, url).await;
    assert_eq!(
        status, 200,
        "case {case:?}: the SigningCertURL must lead to the certificate, got {status} / {body}"
    );
    assert!(
        body.starts_with("-----BEGIN CERTIFICATE-----"),
        "case {case:?}: the SigningCertURL must serve a PEM certificate, got {body}"
    );
    body
}

#[tokio::test]
async fn a_delivered_notification_is_signed_under_the_version_the_topic_asks_for() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "signed")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "signed-queue").await;
    sns(
        &emulator,
        "Subscribe",
        &[
            ("TopicArn", &topic),
            ("Protocol", "sqs"),
            ("Endpoint", &arn),
        ],
    )
    .await;

    let rows = &[
        Row {
            case: "the topic default",
            asked_for: None,
            subject: None,
            attributed: false,
            version: "1",
        },
        Row {
            case: "the default, with a subject",
            asked_for: None,
            subject: Some("an order"),
            attributed: false,
            version: "1",
        },
        Row {
            case: "version 1 asked for explicitly",
            asked_for: Some("1"),
            subject: None,
            attributed: true,
            version: "1",
        },
        Row {
            case: "version 2, which switches the digest to SHA-256",
            asked_for: Some("2"),
            subject: Some("an order"),
            attributed: true,
            version: "2",
        },
        Row {
            case: "back to version 1 on the same topic",
            asked_for: Some("1"),
            subject: None,
            attributed: false,
            version: "1",
        },
    ];

    let mut certificate_urls: Vec<String> = Vec::new();
    for row in rows {
        if let Some(asked_for) = row.asked_for {
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", "SignatureVersion"),
                    ("AttributeValue", asked_for),
                ],
            )
            .await;
        }
        let case = row.case;
        let mut params: Vec<(&str, &str)> =
            vec![("TopicArn", topic.as_str()), ("Message", "an event")];
        if let Some(subject) = row.subject {
            params.push(("Subject", subject));
        }
        if row.attributed {
            params.push(("MessageAttributes.entry.1.Name", "kind"));
            params.push(("MessageAttributes.entry.1.Value.DataType", "String"));
            params.push(("MessageAttributes.entry.1.Value.StringValue", "created"));
        }
        sns(&emulator, "Publish", &params).await;

        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected one delivered message, got {delivered:?}"
        );
        let envelope: Value = serde_json::from_str(&delivered[0].0).unwrap_or_else(|error| {
            panic!("case {case:?}: the body is not the SNS envelope ({error})")
        });

        let declared = envelope.get("SignatureVersion").and_then(Value::as_str);
        assert_eq!(
            declared,
            Some(row.version),
            "case {case:?}: expected SignatureVersion {:?}, got {declared:?} in {envelope}",
            row.version
        );
        let certificate_url = envelope
            .get("SigningCertURL")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the envelope must carry a SigningCertURL, got {envelope}")
            })
            .to_string();
        let signature = envelope
            .get("Signature")
            .and_then(Value::as_str)
            .unwrap_or_else(|| {
                panic!("case {case:?}: the envelope must carry a Signature, got {envelope}")
            });
        let signature = ::aws_smithy_types::base64::decode(signature).unwrap_or_else(|error| {
            panic!("case {case:?}: the Signature must be base64 ({error}): {envelope}")
        });

        let certificate = certificate_at(&emulator, case, &certificate_url).await;
        let signed = canonical_string(&envelope);
        assert!(
            verifies(&certificate, row.version, &signed, &signature),
            "case {case:?}: the signature must verify against the published certificate over \
             the canonical string {signed:?}, got {envelope}"
        );
        // The signature is of THIS message: one byte more and it does not verify, so the
        // member is a signature rather than a constant that happens to decode.
        assert!(
            !verifies(
                &certificate,
                row.version,
                &format!("{signed}tampered\n"),
                &signature
            ),
            "case {case:?}: a signature of another string must not verify, got {envelope}"
        );
        certificate_urls.push(certificate_url);
    }

    // One key per emulator: every notification names the same certificate — the path is the
    // document's own name, so a subscriber that cached it keeps verifying. (What follows the
    // `?` is the signature that gets the fetch back to this emulator, which is minted per
    // URL.)
    let paths: Vec<&str> = certificate_urls
        .iter()
        .map(|url| url.split('?').next().unwrap_or(url))
        .collect();
    let first = paths.first().copied().unwrap_or_default();
    assert!(
        paths.iter().all(|path| *path == first),
        "every notification must name the same signing certificate, got {paths:?}"
    );
}

#[tokio::test]
async fn a_raw_delivery_carries_no_signature_members() {
    let emulator = emulator();
    let topic = element(
        &sns(&emulator, "CreateTopic", &[("Name", "raw-signed")]).await,
        "TopicArn",
    );
    let (url, arn) = given_queue(&emulator, "raw-signed-queue").await;
    let subscription = element(
        &sns(
            &emulator,
            "Subscribe",
            &[
                ("TopicArn", &topic),
                ("Protocol", "sqs"),
                ("Endpoint", &arn),
                ("ReturnSubscriptionArn", "true"),
            ],
        )
        .await,
        "SubscriptionArn",
    );
    sns(
        &emulator,
        "SetSubscriptionAttributes",
        &[
            ("SubscriptionArn", &subscription),
            ("AttributeName", "RawMessageDelivery"),
            ("AttributeValue", "true"),
        ],
    )
    .await;

    // Rows differ in the version the topic signs under: a raw delivery carries the published
    // message exactly, whichever version the topic asks for, because it carries no SNS
    // metadata at all.
    for (case, version) in [
        ("the topic default", None),
        ("version 1", Some("1")),
        ("version 2", Some("2")),
    ] {
        if let Some(version) = version {
            sns(
                &emulator,
                "SetTopicAttributes",
                &[
                    ("TopicArn", &topic),
                    ("AttributeName", "SignatureVersion"),
                    ("AttributeValue", version),
                ],
            )
            .await;
        }
        let message = r#"{"orderId":"A-1"}"#;
        sns(
            &emulator,
            "Publish",
            &[("TopicArn", &topic), ("Message", message)],
        )
        .await;
        let delivered = drain(&emulator, &url).await;
        assert_eq!(
            delivered.len(),
            1,
            "case {case:?}: expected one delivered message, got {delivered:?}"
        );
        assert_eq!(
            delivered[0].0, message,
            "case {case:?}: a raw delivery carries the published message exactly, got {:?}",
            delivered[0].0
        );
    }
}
