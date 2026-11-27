// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for an accelerator's flow-log attributes, driven over the wire.
//!
//! Every call goes through the real router, using the harness in
//! [`super::accelerator_lifecycle`] (`POST /` with
//! `X-Amz-Target: GlobalAccelerator_V20180706.<Operation>`, `awsJson1_1` body), so these
//! exercise routing, deserialization, constraint validation, the handler and the response
//! envelope — not just the handler function.
//!
//! The rules under test are stated once each, as a table or an invariant over a SET of
//! inputs; a recorded request is one row.
//!
//! A reference to an accelerator this store does not hold is answered at HTTP 400 — the
//! status this service answers that error class with, pinned by
//! [`super::not_found_error_status`].

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::accelerator_lifecycle::{arn_of, call, create_accelerator, emulator, error_code, ok};
use super::input_refusals::error_message;

/// `DescribeAcceleratorAttributes` on `accelerator_arn`, requiring a 200.
async fn describe(router: &MultiServiceRouter, accelerator_arn: &str) -> Value {
    ok(
        router,
        "DescribeAcceleratorAttributes",
        json!({ "AcceleratorArn": accelerator_arn }),
    )
    .await["AcceleratorAttributes"]
        .clone()
}

/// `UpdateAcceleratorAttributes` on `accelerator_arn` with the members a row names.
async fn update(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
    members: &Value,
) -> (u16, Value) {
    let mut body = json!({ "AcceleratorArn": accelerator_arn });
    for (key, value) in members.as_object().expect("a JSON object of members") {
        body[key] = value.clone();
    }
    call(router, "UpdateAcceleratorAttributes", body).await
}

/// The three members of `AcceleratorAttributes`, as a row states them: an absent
/// member is `Value::Null`, which is also how the envelope omits it.
fn attributes(enabled: bool, bucket: Option<&str>, prefix: Option<&str>) -> Value {
    json!({
        "FlowLogsEnabled": enabled,
        "FlowLogsS3Bucket": bucket,
        "FlowLogsS3Prefix": prefix,
    })
}

/// The attributes a response carries, in the shape [`attributes`] builds, so a row's
/// expectation and the wire answer compare member by member.
fn observed(body: &Value) -> Value {
    json!({
        "FlowLogsEnabled": body.get("FlowLogsEnabled").cloned().unwrap_or(Value::Null),
        "FlowLogsS3Bucket": body.get("FlowLogsS3Bucket").cloned().unwrap_or(Value::Null),
        "FlowLogsS3Prefix": body.get("FlowLogsS3Prefix").cloned().unwrap_or(Value::Null),
    })
}

#[tokio::test]
async fn a_fresh_accelerator_reports_flow_logs_off_and_no_destination() {
    let router = emulator();

    // One row per accelerator shape the create path can produce, so the reported
    // defaults are not a property of one particular create request.
    let rows: [(&str, Value); 3] = [
        ("plain", json!({})),
        ("disabled", json!({ "Enabled": false })),
        ("dual-stack", json!({ "IpAddressType": "DUAL_STACK" })),
    ];

    for (case, extra) in rows {
        let mut body = json!({ "Name": format!("attr-{case}"), "IdempotencyToken": format!("tok-{case}") });
        for (key, value) in extra.as_object().expect("a JSON object of members") {
            body[key] = value.clone();
        }
        let arn = arn_of(&ok(&router, "CreateAccelerator", body).await["Accelerator"]);

        let reported = describe(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            attributes(false, None, None),
            "case {case}: expected flow logs off with no destination, got {reported}"
        );
    }
}

#[tokio::test]
async fn an_update_sets_only_the_members_it_carries_and_describe_reports_them() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "attr-patch", "tok-patch").await);

    // Applied in order to ONE accelerator: each row's expectation is the whole
    // configuration the update leaves behind, so a member a row omits is asserted to
    // keep the value the row before it set.
    let rows: [(&str, Value, Value); 6] = [
        (
            "turn on with a destination",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "flow-logs-bucket",
                "FlowLogsS3Prefix": "prefix/",
            }),
            attributes(true, Some("flow-logs-bucket"), Some("prefix/")),
        ),
        (
            // The doc's own example prefix: "If you specify slash (/) for the S3
            // bucket prefix" (`UpdateAcceleratorAttributes$FlowLogsS3Prefix`).
            "prefix only",
            json!({ "FlowLogsS3Prefix": "/" }),
            attributes(true, Some("flow-logs-bucket"), Some("/")),
        ),
        (
            "bucket only",
            json!({ "FlowLogsS3Bucket": "other-bucket" }),
            attributes(true, Some("other-bucket"), Some("/")),
        ),
        (
            "flag only, off",
            json!({ "FlowLogsEnabled": false }),
            attributes(false, Some("other-bucket"), Some("/")),
        ),
        (
            "nothing but the ARN",
            json!({}),
            attributes(false, Some("other-bucket"), Some("/")),
        ),
        (
            "flag only, back on over the stored destination",
            json!({ "FlowLogsEnabled": true }),
            attributes(true, Some("other-bucket"), Some("/")),
        ),
    ];

    for (case, members, expected) in rows {
        let (status, body) = update(&router, &arn, &members).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        let updated = body["AcceleratorAttributes"].clone();
        assert_eq!(
            observed(&updated),
            expected,
            "case {case}: expected the update to answer {expected}, got {updated}"
        );

        // The read-back: what the update answered is what the store now holds.
        let reported = describe(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            expected,
            "case {case}: expected describe to report {expected}, got {reported}"
        );
    }
}

#[tokio::test]
async fn enabling_flow_logs_without_a_destination_is_refused_and_changes_nothing() {
    let router = emulator();

    // "If the value is true, FlowLogsS3Bucket and FlowLogsS3Prefix must be specified"
    // (`UpdateAcceleratorAttributes$FlowLogsEnabled`), so every row below leaves one of
    // the two unset — including the empty string, which names no bucket and no prefix.
    //
    // The third column is the WHOLE message the refusal must carry: a sentence of fixed
    // text naming the missing destination, with nothing from the request interpolated
    // into it — so a row varying the other member's value, or the accelerator it is
    // called on, must not change a character of it.
    let bucket_missing = "Flow logs s3 bucket should not be empty.";
    let prefix_missing = "Flow logs s3 prefix should not be empty.";
    let rows: [(&str, Value, &str); 7] = [
        (
            "flag alone",
            json!({ "FlowLogsEnabled": true }),
            bucket_missing,
        ),
        (
            "bucket, no prefix",
            json!({ "FlowLogsEnabled": true, "FlowLogsS3Bucket": "flow-logs-bucket" }),
            prefix_missing,
        ),
        (
            "prefix, no bucket",
            json!({ "FlowLogsEnabled": true, "FlowLogsS3Prefix": "prefix/" }),
            bucket_missing,
        ),
        (
            // A longer prefix: the message is fixed text, so it is the same sentence.
            "another prefix, no bucket",
            json!({ "FlowLogsEnabled": true, "FlowLogsS3Prefix": "logs/2026/01/" }),
            bucket_missing,
        ),
        (
            "empty bucket",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "",
                "FlowLogsS3Prefix": "prefix/",
            }),
            bucket_missing,
        ),
        (
            "empty prefix",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "flow-logs-bucket",
                "FlowLogsS3Prefix": "",
            }),
            prefix_missing,
        ),
        (
            // Both missing: the bucket is the destination, so it is named first.
            "both empty",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "",
                "FlowLogsS3Prefix": "",
            }),
            bucket_missing,
        ),
    ];

    for (case, members, expected_message) in rows {
        // An accelerator name takes only alphanumerics, periods and hyphens
        // (`CreateAccelerator$Name`), so the row's label is slugified into one.
        let slug: String = case
            .chars()
            .map(|c| if c.is_ascii_alphanumeric() { c } else { '-' })
            .collect();
        let arn = arn_of(&create_accelerator(&router, &format!("attr-{slug}"), &slug).await);
        let (status, body) = update(&router, &arn, &members).await;
        assert_eq!(
            status, 400,
            "case {case}: expected HTTP 400, got {status} with {body}"
        );
        assert_eq!(
            error_code(&body),
            "InvalidArgumentException",
            "case {case}: expected InvalidArgumentException, got {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case}: expected the refusal to read {expected_message:?}, got {message:?}"
        );

        // A refused update leaves the configuration exactly as it was.
        let reported = describe(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            attributes(false, None, None),
            "case {case}: expected the refused update to change nothing, got {reported}"
        );
    }
}

#[tokio::test]
async fn attributes_are_per_accelerator_and_survive_deleting_a_listener() {
    let router = emulator();

    let first = arn_of(&create_accelerator(&router, "attr-one", "tok-one").await);
    let second = arn_of(&create_accelerator(&router, "attr-two", "tok-two").await);
    let (status, body) = update(
        &router,
        &first,
        &json!({
            "FlowLogsEnabled": true,
            "FlowLogsS3Bucket": "one-bucket",
            "FlowLogsS3Prefix": "one/",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed first: expected HTTP 200, got {status} with {body}"
    );

    // One accelerator's attributes are its own.
    assert_eq!(
        observed(&describe(&router, &second).await),
        attributes(false, None, None),
        "case sibling: expected the other accelerator to keep its defaults"
    );

    // A listener comes and goes under the accelerator whose attributes are set; the
    // attributes belong to the accelerator, so neither call touches them.
    let listener_arn = ok(
        &router,
        "CreateListener",
        json!({
            "AcceleratorArn": first,
            "PortRanges": [{ "FromPort": 80, "ToPort": 80 }],
            "Protocol": "TCP",
            "IdempotencyToken": "lsn-attr",
        }),
    )
    .await["Listener"]["ListenerArn"]
        .as_str()
        .unwrap_or_default()
        .to_string();
    for case in ["with a listener", "after deleting it"] {
        if case == "after deleting it" {
            ok(
                &router,
                "DeleteListener",
                json!({ "ListenerArn": listener_arn }),
            )
            .await;
        }
        let reported = describe(&router, &first).await;
        assert_eq!(
            observed(&reported),
            attributes(true, Some("one-bucket"), Some("one/")),
            "case {case}: expected the accelerator's attributes unaffected, got {reported}"
        );
    }
}

#[tokio::test]
async fn attributes_of_an_accelerator_the_store_does_not_hold_are_refused() {
    let router = emulator();
    let live = arn_of(&create_accelerator(&router, "attr-live", "tok-live").await);
    let deleted = arn_of(&create_accelerator(&router, "attr-gone", "tok-gone").await);
    ok(
        &router,
        "UpdateAccelerator",
        json!({ "AcceleratorArn": deleted, "Enabled": false }),
    )
    .await;
    ok(
        &router,
        "DeleteAccelerator",
        json!({ "AcceleratorArn": deleted }),
    )
    .await;

    // Rows: an ARN of the right shape that was never minted, one whose accelerator was
    // deleted, a non-ARN string, and the empty string the model's `@length(0..255)`
    // admits.
    let rows: [(&str, String); 4] = [
        (
            "never minted",
            format!("{live}-nope"),
        ),
        ("deleted", deleted.clone()),
        ("not an ARN", "attr-live".to_string()),
        ("empty", String::new()),
    ];

    for (case, accelerator_arn) in rows {
        for operation in [
            "DescribeAcceleratorAttributes",
            "UpdateAcceleratorAttributes",
        ] {
            let (status, body) = call(
                &router,
                operation,
                json!({ "AcceleratorArn": accelerator_arn }),
            )
            .await;
            // The status this service answers a missing resource with: HTTP 400, the
            // rule `not_found_error_status` states for the error class.
            assert_eq!(
                status, 400,
                "case {case}/{operation}: expected HTTP 400, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                "AcceleratorNotFoundException",
                "case {case}/{operation}: expected AcceleratorNotFoundException, got {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message,
                format!("Accelerator {accelerator_arn} not found."),
                "case {case}/{operation}: expected the refusal to name the ARN, got {message:?}"
            );
        }
    }

    // The live accelerator is still readable, so the refusals above are about the ARN
    // the request named and not about the store.
    assert_eq!(
        observed(&describe(&router, &live).await),
        attributes(false, None, None),
        "case live: expected the remaining accelerator to still report its attributes"
    );
}

#[tokio::test]
async fn a_destination_is_mutable_while_flow_logs_are_off_and_then_enables_them() {
    let router = emulator();

    // The destination is constrained only while flow logs are ON, so while they are off
    // any bucket and prefix — the empty string included — is accepted, and the call that
    // turns them on over a stored destination is accepted too. One row per destination
    // shape, so the rule is not a property of one bucket name.
    let rows: [(&str, &str, &str); 6] = [
        ("plain", "flow-logs-bucket", "prefix/"),
        ("dotted bucket", "logs.example.bucket", "a/b/c/"),
        // The doc's own example prefix (`UpdateAcceleratorAttributes$FlowLogsS3Prefix`).
        ("slash prefix", "another-bucket", "/"),
        ("no bucket", "", "prefix/"),
        ("no prefix", "a-bucket", ""),
        ("neither", "", ""),
    ];

    for (index, (case, bucket, prefix)) in rows.into_iter().enumerate() {
        let arn = arn_of(
            &create_accelerator(
                &router,
                &format!("attr-off-{index}"),
                &format!("tok-off-{index}"),
            )
            .await,
        );

        let (status, body) = update(
            &router,
            &arn,
            &json!({ "FlowLogsS3Bucket": bucket, "FlowLogsS3Prefix": prefix }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {case}: expected setting a destination while off to be accepted, got {status} with {body}"
        );
        assert_eq!(
            observed(&describe(&router, &arn).await),
            attributes(false, Some(bucket), Some(prefix)),
            "case {case}: expected the stored destination with flow logs still off"
        );

        // Turning them on: accepted exactly when the stored destination names both a
        // bucket and a prefix.
        let (status, body) = update(&router, &arn, &json!({ "FlowLogsEnabled": true })).await;
        // The sentence names the part of the destination that is empty — the bucket first,
        // it being the destination the prefix sits under.
        let expected_message = if bucket.is_empty() {
            Some("Flow logs s3 bucket should not be empty.")
        } else if prefix.is_empty() {
            Some("Flow logs s3 prefix should not be empty.")
        } else {
            None
        };
        if let Some(expected_message) = expected_message {
            assert_eq!(
                status, 400,
                "case {case}: expected turning flow logs on with no destination to be refused, got {status} with {body}"
            );
            let message = error_message(&body);
            assert_eq!(
                message, expected_message,
                "case {case}: expected the refusal to read {expected_message:?}, got {message:?}"
            );
            continue;
        }
        assert_eq!(
            status, 200,
            "case {case}: expected turning flow logs on over the stored destination to be accepted, got {status} with {body}"
        );
        assert_eq!(
            observed(&describe(&router, &arn).await),
            attributes(true, Some(bucket), Some(prefix)),
            "case {case}: expected flow logs on with the stored destination"
        );
    }
}

/// A custom routing accelerator created with the members a row names, requiring a 200,
/// as its ARN.
async fn create_custom_routing_accelerator(
    router: &MultiServiceRouter,
    name: &str,
    token: &str,
    extra: Value,
) -> String {
    let mut body = json!({ "Name": name, "IdempotencyToken": token });
    for (key, value) in extra.as_object().expect("a JSON object of members") {
        body[key] = value.clone();
    }
    arn_of(&ok(router, "CreateCustomRoutingAccelerator", body).await["Accelerator"])
}

/// `DescribeCustomRoutingAcceleratorAttributes` on `accelerator_arn`, requiring a 200.
async fn describe_custom_routing_attributes(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
) -> Value {
    ok(
        router,
        "DescribeCustomRoutingAcceleratorAttributes",
        json!({ "AcceleratorArn": accelerator_arn }),
    )
    .await["AcceleratorAttributes"]
        .clone()
}

/// `UpdateCustomRoutingAcceleratorAttributes` on `accelerator_arn` with the members a row
/// names.
async fn update_custom_routing_attributes(
    router: &MultiServiceRouter,
    accelerator_arn: &str,
    members: &Value,
) -> (u16, Value) {
    let mut body = json!({ "AcceleratorArn": accelerator_arn });
    for (key, value) in members.as_object().expect("a JSON object of members") {
        body[key] = value.clone();
    }
    call(router, "UpdateCustomRoutingAcceleratorAttributes", body).await
}

/// RULE: a custom routing accelerator's attributes exist from the moment it is created,
/// and report flow logs off with no destination until something sets one — whatever the
/// create request carried, since the attributes are not among its members.
#[tokio::test]
async fn a_fresh_custom_routing_accelerator_reports_flow_logs_off_and_no_destination() {
    let router = emulator();

    // One row per shape the custom routing create path can produce — the optional
    // members it takes (`CreateCustomRoutingAccelerator`: Enabled, IpAddressType,
    // IpAddresses, Tags) — so the reported defaults are not a property of one create.
    let rows: [(&str, Value); 5] = [
        ("plain", json!({})),
        ("disabled", json!({ "Enabled": false })),
        ("explicit-ipv4", json!({ "IpAddressType": "IPV4" })),
        ("byoip", json!({ "IpAddresses": ["198.51.100.21"] })),
        ("tagged", json!({ "Tags": [{ "Key": "team", "Value": "net" }] })),
    ];

    for (case, extra) in rows {
        let arn = create_custom_routing_accelerator(
            &router,
            &format!("cra-attr-{case}"),
            &format!("cra-attr-{case}"),
            extra,
        )
        .await;

        let reported = describe_custom_routing_attributes(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            attributes(false, None, None),
            "case {case}: expected flow logs off with no destination, got {reported}"
        );
    }
}

/// RULE: an attributes update on a custom routing accelerator changes the members it
/// carries and leaves the rest of the stored configuration alone, and the matching
/// describe reports what was persisted — flow logs being refused while they name no
/// destination.
#[tokio::test]
async fn an_update_sets_only_the_members_it_carries_on_a_custom_routing_accelerator() {
    let router = emulator();
    let arn =
        create_custom_routing_accelerator(&router, "cra-attr-patch", "cra-attr-patch", json!({}))
            .await;

    // Applied in order to ONE accelerator: each row's expectation is the whole
    // configuration the update leaves behind, so a member a row omits is asserted to keep
    // the value the row before it set.
    let rows: [(&str, Value, Value); 6] = [
        (
            "turn on with a destination",
            json!({
                "FlowLogsEnabled": true,
                "FlowLogsS3Bucket": "cra-flow-logs",
                "FlowLogsS3Prefix": "cra/",
            }),
            attributes(true, Some("cra-flow-logs"), Some("cra/")),
        ),
        (
            // The doc's own example prefix: "If you specify slash (/) for the S3 bucket
            // prefix" (`UpdateCustomRoutingAcceleratorAttributes$FlowLogsS3Prefix`).
            "prefix only",
            json!({ "FlowLogsS3Prefix": "/" }),
            attributes(true, Some("cra-flow-logs"), Some("/")),
        ),
        (
            "bucket only",
            json!({ "FlowLogsS3Bucket": "cra-other-bucket" }),
            attributes(true, Some("cra-other-bucket"), Some("/")),
        ),
        (
            "flag only, off",
            json!({ "FlowLogsEnabled": false }),
            attributes(false, Some("cra-other-bucket"), Some("/")),
        ),
        (
            "nothing but the ARN",
            json!({}),
            attributes(false, Some("cra-other-bucket"), Some("/")),
        ),
        (
            "flag only, back on over the stored destination",
            json!({ "FlowLogsEnabled": true }),
            attributes(true, Some("cra-other-bucket"), Some("/")),
        ),
    ];

    for (case, members, expected) in rows {
        let (status, body) = update_custom_routing_attributes(&router, &arn, &members).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status} with {body}"
        );
        let updated = body["AcceleratorAttributes"].clone();
        assert_eq!(
            observed(&updated),
            expected,
            "case {case}: expected the update to answer {expected}, got {updated}"
        );

        // The read-back: what the update answered is what the store now holds, and a
        // repeated read reports it again.
        for read in 1..=2 {
            let reported = describe_custom_routing_attributes(&router, &arn).await;
            assert_eq!(
                observed(&reported),
                expected,
                "case {case}, read {read}: expected describe to report {expected}, \
                 got {reported}"
            );
        }
    }

    // "If the value is true, FlowLogsS3Bucket and FlowLogsS3Prefix must be specified"
    // (`UpdateCustomRoutingAcceleratorAttributes$FlowLogsEnabled`), so a destination
    // emptied under live flow logs is refused and changes nothing.
    let rows: [(&str, Value, &str); 3] = [
        (
            "empty bucket",
            json!({ "FlowLogsS3Bucket": "" }),
            "Flow logs s3 bucket should not be empty.",
        ),
        (
            "empty prefix",
            json!({ "FlowLogsS3Prefix": "" }),
            "Flow logs s3 prefix should not be empty.",
        ),
        (
            "both emptied",
            json!({ "FlowLogsS3Bucket": "", "FlowLogsS3Prefix": "" }),
            "Flow logs s3 bucket should not be empty.",
        ),
    ];
    for (case, members, expected_message) in rows {
        let (status, body) = update_custom_routing_attributes(&router, &arn, &members).await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "InvalidArgumentException"),
            "case {case}: expected the refusal, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case}: expected the refusal to read {expected_message:?}, got {message:?}"
        );
        let reported = describe_custom_routing_attributes(&router, &arn).await;
        assert_eq!(
            observed(&reported),
            attributes(true, Some("cra-other-bucket"), Some("/")),
            "case {case}: expected the refused update to change nothing, got {reported}"
        );
    }
}

/// RULE: an accelerator's attributes are its own and are reached only through the
/// operations of its own API family — an ARN of the other family names a resource that
/// exists and is served elsewhere, and an ARN that names nothing is absent.
#[tokio::test]
async fn the_attributes_of_each_accelerator_kind_are_served_by_its_own_api_family() {
    let router = emulator();

    let custom = create_custom_routing_accelerator(&router, "cra-own", "cra-own", json!({})).await;
    let sibling =
        create_custom_routing_accelerator(&router, "cra-sibling", "cra-sibling", json!({})).await;
    let standard = arn_of(&create_accelerator(&router, "std-own", "std-own").await);

    // Each family's own update sets its own accelerator's destination.
    let (status, body) = update_custom_routing_attributes(
        &router,
        &custom,
        &json!({
            "FlowLogsEnabled": true,
            "FlowLogsS3Bucket": "custom-bucket",
            "FlowLogsS3Prefix": "custom/",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed custom: expected HTTP 200, got {status} with {body}"
    );
    let (status, body) = update(
        &router,
        &standard,
        &json!({
            "FlowLogsEnabled": true,
            "FlowLogsS3Bucket": "standard-bucket",
            "FlowLogsS3Prefix": "standard/",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed standard: expected HTTP 200, got {status} with {body}"
    );

    // One accelerator's attributes are its own: neither its sibling in the same family
    // nor the accelerator of the other family took them on.
    assert_eq!(
        observed(&describe_custom_routing_attributes(&router, &sibling).await),
        attributes(false, None, None),
        "case sibling: expected the other custom routing accelerator to keep its defaults"
    );
    assert_eq!(
        observed(&describe_custom_routing_attributes(&router, &custom).await),
        attributes(true, Some("custom-bucket"), Some("custom/")),
        "case custom: expected the custom routing accelerator's own destination"
    );
    assert_eq!(
        observed(&describe(&router, &standard).await),
        attributes(true, Some("standard-bucket"), Some("standard/")),
        "case standard: expected the standard accelerator's own destination"
    );

    // The cross-family and absent-resource refusals, per operation. The wording is the
    // family's, pinned over the whole class by [`super::cross_family_resolution`]; the
    // status this service answers the error class with is pinned by
    // [`super::not_found_error_status`].
    let unknown = format!("{custom}-gone");
    let rows: Vec<(&str, &str, &str, String)> = vec![
        (
            "a standard accelerator through the custom routing describe",
            "DescribeCustomRoutingAcceleratorAttributes",
            &standard,
            format!(
                "Only Custom Routing resources are supported by this custom routing API; got \
                 standard accelerator: {standard}"
            ),
        ),
        (
            "a standard accelerator through the custom routing update",
            "UpdateCustomRoutingAcceleratorAttributes",
            &standard,
            format!(
                "Only Custom Routing resources are supported by this custom routing API; got \
                 standard accelerator: {standard}"
            ),
        ),
        (
            "a custom routing accelerator through the standard describe",
            "DescribeAcceleratorAttributes",
            &custom,
            format!(
                "Custom Routing resources must use the Custom Routing APIs; got custom routing \
                 resource: {custom}"
            ),
        ),
        (
            "a custom routing accelerator through the standard update",
            "UpdateAcceleratorAttributes",
            &custom,
            format!(
                "Custom Routing resources must use the Custom Routing APIs; got custom routing \
                 resource: {custom}"
            ),
        ),
        (
            "an ARN that names nothing, through the custom routing describe",
            "DescribeCustomRoutingAcceleratorAttributes",
            &unknown,
            format!("Accelerator {unknown} not found."),
        ),
        (
            "an ARN that names nothing, through the custom routing update",
            "UpdateCustomRoutingAcceleratorAttributes",
            &unknown,
            format!("Accelerator {unknown} not found."),
        ),
        (
            // The empty string the model's `@length(0..255)` admits names nothing.
            "the empty ARN, through the custom routing describe",
            "DescribeCustomRoutingAcceleratorAttributes",
            "",
            "Accelerator  not found.".to_string(),
        ),
    ];

    for (case, operation, accelerator_arn, expected_message) in rows {
        let (status, body) = call(
            &router,
            operation,
            json!({ "AcceleratorArn": accelerator_arn }),
        )
        .await;
        assert_eq!(
            (status, error_code(&body).as_str()),
            (400, "AcceleratorNotFoundException"),
            "case {case}: expected this API family not to serve the ARN, got {status} with {body}"
        );
        let message = error_message(&body);
        assert_eq!(
            message, expected_message,
            "case {case}: expected the refusal to read {expected_message:?}, got {message:?}"
        );
    }

    // And the refused calls left every accelerator's attributes as they were.
    assert_eq!(
        observed(&describe_custom_routing_attributes(&router, &custom).await),
        attributes(true, Some("custom-bucket"), Some("custom/")),
        "case custom after the refusals: expected its destination unchanged"
    );
    assert_eq!(
        observed(&describe(&router, &standard).await),
        attributes(true, Some("standard-bucket"), Some("standard/")),
        "case standard after the refusals: expected its destination unchanged"
    );
}

#[tokio::test]
async fn describing_attributes_does_not_change_what_a_later_caller_sees() {
    let router = emulator();
    let arn = arn_of(&create_accelerator(&router, "attr-read", "tok-read").await);
    let (status, body) = update(
        &router,
        &arn,
        &json!({
            "FlowLogsEnabled": true,
            "FlowLogsS3Bucket": "read-bucket",
            "FlowLogsS3Prefix": "read/",
        }),
    )
    .await;
    assert_eq!(
        status, 200,
        "case seed: expected HTTP 200, got {status} with {body}"
    );

    let first = describe(&router, &arn).await;
    for read in 2..=4 {
        let again = describe(&router, &arn).await;
        assert_eq!(
            observed(&again),
            observed(&first),
            "case read {read}: expected a repeated describe to report {first}, got {again}"
        );
    }
}
