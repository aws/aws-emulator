// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for the two log-group properties `DescribeLogGroups` projects from
//! state that the mutating operations write: `bearerTokenAuthenticationEnabled` (moved by
//! `PutBearerTokenAuthentication`) and `kmsKeyId` (moved by `AssociateKmsKey` /
//! `DisassociateKmsKey` and seeded by `CreateLogGroup`).
//!
//! Every case drives the service the way a client does — `POST /` with
//! `X-Amz-Target: Logs_20140328.<Operation>` through the router that mounts it — so a
//! response that is not computed from the shared store cannot pass.
//!
//! The tables below span the classes the rule generalises over: the forms a log group is
//! named by (name, either ARN form), the log group classes, the sequences of puts and
//! associations applied to a group, and the forms a `kmsKeyId` takes. Any single group,
//! key or flag value a recording happens to carry is one row.

use serde_json::{json, Value};

use crate::harness::{Logs, REGION};

/// Log group names spanning the forms `LogGroupName`'s pattern
/// (`^[\.\-_/#A-Za-z0-9]+$`) admits: a path, a dotted name, a single character, and one
/// carrying the punctuation `ResourceIdentifier`'s pattern (`^[\w+=/:,.@\-\*]*$`) also
/// admits, so the same name can be named by an ARN.
const NAME_FORMS: [&str; 4] = [
    "/aws/lambda/bearer-fn",
    "service.api.Bearer",
    "b",
    "mixed-1_2.3/4",
];

/// The log group classes `CreateLogGroup` accepts, from the model's `LogGroupClass` enum
/// (`DELIVERY | INFREQUENT_ACCESS | STANDARD`).
const LOG_GROUP_CLASSES: [&str; 3] = ["DELIVERY", "INFREQUENT_ACCESS", "STANDARD"];

/// The members a `DescribeLogGroups` projection of a plainly created log group carries,
/// from the `LogGroup` shape: every member whose value such a group has, which is every
/// one except the ones it has not been given (`kmsKeyId`, `retentionInDays`,
/// `dataProtectionStatus`, `inheritedProperties`).
const DEFAULT_PROJECTION: [&str; 9] = [
    "arn",
    "bearerTokenAuthenticationEnabled",
    "creationTime",
    "deletionProtectionEnabled",
    "logGroupArn",
    "logGroupClass",
    "logGroupName",
    "metricFilterCount",
    "storedBytes",
];

/// KMS key identifiers spanning several KEY ARNs — the form `kmsKeyId` takes ("The Amazon
/// Resource Name (ARN) of the KMS key to use when encrypting log data"), in the ARN shape
/// `engine::emulator_arn` builds for this emulator's account. No other identifier form names a
/// key here, which `tests/all/kms_key_resolution.rs` pins as the refusal it is.
fn kms_key_ids() -> [String; 3] {
    [
        engine::emulator_arn("kms", REGION, "key/1234abcd-12ab-34cd-56ef-1234567890ab"),
        engine::emulator_arn("kms", REGION, "key/fedcba98-76fe-54dc-32ba-0987654321fe"),
        engine::emulator_arn("kms", REGION, "key/0f1e2d3c-4b5a-6978-8796-a5b4c3d2e1f0"),
    ]
}

/// The account's query-insights result resource, the non-log-group target
/// `AssociateKmsKey` documents: `arn:aws:logs:REGION:ACCOUNT_ID:query-result:*`.
fn query_result_arn() -> String {
    engine::emulator_arn("logs", REGION, "query-result:*")
}

/// The `bearerTokenAuthenticationEnabled` the service reports for one log group. `None`
/// means the member is absent — which the defect this suite pins looked exactly like.
async fn bearer_of(logs: &Logs, identifier: &str) -> Option<bool> {
    logs.describe_group(identifier)
        .await
        .unwrap_or_else(|| panic!("case {identifier}: the log group must still exist"))
        .get("bearerTokenAuthenticationEnabled")
        .and_then(Value::as_bool)
}

/// The `kmsKeyId` the service reports for one log group, or `None` when it carries none.
async fn kms_key_of(logs: &Logs, identifier: &str) -> Option<String> {
    logs.describe_group(identifier)
        .await
        .unwrap_or_else(|| panic!("case {identifier}: the log group must still exist"))
        .get("kmsKeyId")
        .and_then(Value::as_str)
        .map(ToString::to_string)
}

/// Assert an operation modelled with a `Unit` output answered 200 with an empty body.
fn assert_unit(case: &str, body: &Value) {
    let members = match body {
        Value::Null => 0,
        Value::Object(map) => map.len(),
        other => panic!("case {case}: a Unit output is an empty body, got {other}"),
    };
    assert_eq!(
        members, 0,
        "case {case}: a Unit output carries no members, got {body}"
    );
}

/// Put a bearer-token authentication state through one identifier form.
async fn put_bearer(logs: &Logs, identifier: &str, enabled: bool) {
    let body = logs
        .ok(
            "PutBearerTokenAuthentication",
            &json!({
                "logGroupIdentifier": identifier,
                "bearerTokenAuthenticationEnabled": enabled,
            }),
        )
        .await;
    assert_unit(&format!("{identifier} enabled={enabled}"), &body);
}

/// A freshly created log group reports `bearerTokenAuthenticationEnabled` as an explicit
/// `false` — the member is present, not absent — in every log group class, and the rest
/// of the default projection is exactly the documented member set.
#[tokio::test]
async fn a_new_log_group_reports_the_whole_default_projection() {
    let logs = Logs::new();

    for (index, class) in LOG_GROUP_CLASSES.into_iter().enumerate() {
        let name = format!("{}/default-{index}", NAME_FORMS[index % NAME_FORMS.len()]);
        logs.ok(
            "CreateLogGroup",
            &json!({"logGroupName": name, "logGroupClass": class}),
        )
        .await;

        let group = logs
            .describe_group(&name)
            .await
            .unwrap_or_else(|| panic!("case {class}: the log group was just created"));
        let object = group
            .as_object()
            .unwrap_or_else(|| panic!("case {class}: a log group is an object, got {group}"));

        let mut members: Vec<&str> = object.keys().map(String::as_str).collect();
        members.sort_unstable();
        assert_eq!(
            members,
            DEFAULT_PROJECTION.to_vec(),
            "case {class}: the default projection is the documented member set, got {group}"
        );

        // "Indicates whether bearer token authentication is enabled for this log group":
        // a group nobody has enabled it on reports `false`, never a missing member.
        assert_eq!(
            object.get("bearerTokenAuthenticationEnabled"),
            Some(&Value::Bool(false)),
            "case {class}: a new group reports bearer-token auth as an explicit false, got {group}"
        );
        assert_eq!(
            object.get("deletionProtectionEnabled"),
            Some(&Value::Bool(false)),
            "case {class}: a new group reports deletion protection as an explicit false, got {group}"
        );
        assert_eq!(
            object.get("logGroupClass").and_then(Value::as_str),
            Some(class),
            "case {class}: the group reports the class it was created with, got {group}"
        );
    }
}

/// The reported flag follows the puts applied to the group, through every identifier form
/// a put can name it by, and a repeated put of the state it is already in changes nothing.
#[tokio::test]
async fn the_bearer_flag_follows_the_put_through_every_identifier_form() {
    let logs = Logs::new();

    // identifier form × the state the group is driven through. `LogGroupIdentifier`'s
    // pattern (`^[\w#+=/:,.@-]*$`) admits no `*`, so the `LogGroup$arn` form — the one
    // carrying the trailing `:*` — is not an identifier a put can name; the refusals
    // test pins that.
    for (index, form) in ["name", "logGroupArn"].into_iter().enumerate() {
        let name = format!("{}/bearer-{index}", NAME_FORMS[index % NAME_FORMS.len()]);
        logs.ok("CreateLogGroup", &json!({"logGroupName": name}))
            .await;
        let before = logs
            .describe_group(&name)
            .await
            .expect("the group was just created");
        let (_arn, log_group_arn) = logs.group_arns(&name).await;
        let identifier = if form == "name" {
            name.clone()
        } else {
            log_group_arn
        };

        // Both transitions, and each state put twice in a row, so a repeated put is
        // pinned as a no-op rather than a toggle.
        for enabled in [true, true, false, false, true, false] {
            let case = format!("{form} enabled={enabled}");
            put_bearer(&logs, &identifier, enabled).await;

            // Read back through the NAME, so the flag is stored on the group and not
            // under the string that named it.
            assert_eq!(
                bearer_of(&logs, &name).await,
                Some(enabled),
                "case {case}: a put records exactly the state it names"
            );

            let after = logs
                .describe_group(&name)
                .await
                .expect("a put does not delete the group");
            for member in DEFAULT_PROJECTION
                .into_iter()
                .filter(|member| *member != "bearerTokenAuthenticationEnabled")
            {
                assert_eq!(
                    after.get(member),
                    before.get(member),
                    "case {case}: a put leaves {member} unchanged, got {after}"
                );
            }
        }
    }
}

/// The flag belongs to the group it was put on: enabling it on one group leaves every
/// other group's flag where it was.
#[tokio::test]
async fn the_bearer_flag_is_per_log_group() {
    let logs = Logs::new();
    let groups = ["bearer-a", "bearer.b", "bearer/c", "bearer#d"];
    for group in groups {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    // Enable on every other group, so an enabled and a disabled group sit on both sides
    // of each other in name order.
    let enabled: Vec<&str> = groups.iter().copied().step_by(2).collect();
    for group in &enabled {
        put_bearer(&logs, group, true).await;
    }

    for group in groups {
        assert_eq!(
            bearer_of(&logs, group).await,
            Some(enabled.contains(&group)),
            "case {group}: a put on one group changes no other group's flag"
        );
    }

    // Every group is still reported with the member present when they are described
    // together rather than one at a time.
    let page = logs.ok("DescribeLogGroups", &json!({})).await;
    let described = page["logGroups"]
        .as_array()
        .unwrap_or_else(|| panic!("DescribeLogGroups must return logGroups, got {page}"));
    assert_eq!(
        described.len(),
        groups.len(),
        "case the whole page: every created group is described, got {page}"
    );
    for group in described {
        let name = group["logGroupName"].as_str().unwrap_or_default();
        assert_eq!(
            group.get("bearerTokenAuthenticationEnabled").and_then(Value::as_bool),
            Some(enabled.contains(&name)),
            "case {name} on the whole page: the flag is projected for every group, got {group}"
        );
    }
}

/// An association records the key on the log group, a later association overrides it, and
/// a disassociation removes the member again — through every target form, for every key
/// form, and whether or not the group was created with a key.
#[tokio::test]
async fn the_kms_key_follows_the_association_through_every_target_form() {
    let logs = Logs::new();
    let keys = kms_key_ids();

    // target form × created-with-a-key. `resourceIdentifier` names a log group by ARN,
    // in either ARN form, and `logGroupName` names it by name.
    let cases: [(&str, bool); 4] = [
        ("logGroupName", false),
        ("logGroupName", true),
        ("resourceIdentifier:logGroupArn", false),
        ("resourceIdentifier:arn", true),
    ];

    for (index, (form, created_with_key)) in cases.into_iter().enumerate() {
        let name = format!("{}/kms-{index}", NAME_FORMS[index % NAME_FORMS.len()]);
        let case = format!("{form} created_with_key={created_with_key}");
        let create = if created_with_key {
            json!({"logGroupName": name, "kmsKeyId": keys[2]})
        } else {
            json!({"logGroupName": name})
        };
        logs.ok("CreateLogGroup", &create).await;

        // "The Amazon Resource Name (ARN) of the KMS key to use when encrypting log
        // data": the key a group was created with is the key it reports.
        assert_eq!(
            kms_key_of(&logs, &name).await,
            created_with_key.then(|| keys[2].clone()),
            "case {case}: a new group reports the key it was created with"
        );

        let (arn, log_group_arn) = logs.group_arns(&name).await;
        let target = |body: Value| -> Value {
            let mut body = body;
            let map = body.as_object_mut().expect("the request body is an object");
            match form {
                "logGroupName" => {
                    map.insert("logGroupName".to_string(), json!(name));
                }
                "resourceIdentifier:logGroupArn" => {
                    map.insert("resourceIdentifier".to_string(), json!(log_group_arn));
                }
                _ => {
                    map.insert("resourceIdentifier".to_string(), json!(arn));
                }
            }
            body
        };

        // Each key form in turn, so "Associating a KMS key with a log group overrides any
        // existing associations between the log group and a KMS key" is pinned, not just
        // the first association.
        for key in &keys {
            let body = logs
                .ok("AssociateKmsKey", &target(json!({"kmsKeyId": key})))
                .await;
            assert_unit(&format!("{case} key={key}"), &body);
            assert_eq!(
                kms_key_of(&logs, &name).await.as_deref(),
                Some(key.as_str()),
                "case {case} key={key}: the associated key is reported on the group"
            );
            // The same object carries the bearer flag: one missing member never hides
            // another.
            assert_eq!(
                bearer_of(&logs, &name).await,
                Some(false),
                "case {case} key={key}: the group still reports its bearer-token flag"
            );
        }

        // "stop using the KMS key to encrypt future log events ingested and stored in the
        // log group": the member goes away with the association.
        let body = logs
            .ok("DisassociateKmsKey", &target(json!({})))
            .await;
        assert_unit(&format!("{case} disassociate"), &body);
        assert_eq!(
            kms_key_of(&logs, &name).await,
            None,
            "case {case}: no key is reported once it is disassociated"
        );

        // A disassociate of a group that carries no key is accepted and leaves it in that
        // state.
        logs.ok("DisassociateKmsKey", &target(json!({}))).await;
        assert_eq!(
            kms_key_of(&logs, &name).await,
            None,
            "case {case}: a repeated disassociate leaves the group without a key"
        );
    }
}

/// An association belongs to the group it names, and the account's query-result target is
/// a target of its own: associating a key with it touches no log group.
#[tokio::test]
async fn the_kms_key_is_per_target() {
    let logs = Logs::new();
    let keys = kms_key_ids();
    let groups = ["kms-a", "kms.b", "kms/c"];
    for group in groups {
        logs.ok("CreateLogGroup", &json!({"logGroupName": group}))
            .await;
    }
    logs.ok(
        "AssociateKmsKey",
        &json!({"logGroupName": groups[1], "kmsKeyId": keys[0]}),
    )
    .await;

    for group in groups {
        assert_eq!(
            kms_key_of(&logs, group).await,
            (group == groups[1]).then(|| keys[0].clone()),
            "case {group}: an association on one group changes no other group's key"
        );
    }

    // "Specify the resourceIdentifier parameter with a query-result resource, to use that
    // key to encrypt the stored results of all future StartQuery operations in the
    // account" — an account-scoped target, not a log group.
    let query_result = query_result_arn();
    for (operation, body) in [
        (
            "AssociateKmsKey",
            json!({"resourceIdentifier": query_result, "kmsKeyId": keys[1]}),
        ),
        (
            "DisassociateKmsKey",
            json!({"resourceIdentifier": query_result}),
        ),
    ] {
        let response = logs.ok(operation, &body).await;
        assert_unit(&format!("{operation} on the query-result resource"), &response);
        for group in groups {
            assert_eq!(
                kms_key_of(&logs, group).await,
                (group == groups[1]).then(|| keys[0].clone()),
                "case {operation} on the query-result resource: {group}'s key is untouched"
            );
        }
    }
}

/// `PutBearerTokenAuthentication` on a log group that does not exist is refused with
/// `ResourceNotFoundException`, in every identifier form, and creates nothing.
#[tokio::test]
async fn a_bearer_put_on_an_unknown_log_group_is_refused() {
    let logs = Logs::new();
    let present = "present.bearer.group";
    logs.ok("CreateLogGroup", &json!({"logGroupName": present}))
        .await;
    let (arn, log_group_arn) = logs.group_arns(present).await;

    let unknown = [
        ("a name nobody created", "no-such-group".to_string()),
        (
            "a name that differs only in case",
            "Present.Bearer.Group".to_string(),
        ),
        (
            "an ARN form naming no group",
            log_group_arn.replace(present, "absent/group"),
        ),
    ];
    for (case, identifier) in unknown {
        for enabled in [true, false] {
            let code = logs
                .err(
                    "PutBearerTokenAuthentication",
                    &json!({
                        "logGroupIdentifier": identifier,
                        "bearerTokenAuthenticationEnabled": enabled,
                    }),
                )
                .await;
            assert_eq!(
                code, "ResourceNotFoundException",
                "case {case} ({identifier}) enabled={enabled}: the flag is set on a group that exists, got {code}"
            );
        }
        assert_eq!(
            logs.describe_group(&identifier).await,
            None,
            "case {case} ({identifier}): a refused put does not create the group"
        );
    }

    // `LogGroupIdentifier`'s pattern admits no `*`, so the `LogGroup$arn` form is refused
    // by the modelled constraint before this operation's own checks run — as this
    // operation's own `InvalidParameterException`.
    let (status, body) = logs
        .call(
            "PutBearerTokenAuthentication",
            &json!({"logGroupIdentifier": arn, "bearerTokenAuthenticationEnabled": true}),
        )
        .await;
    assert_eq!(
        status, 400,
        "case the wildcard ARN form ({arn}): it is outside logGroupIdentifier's pattern, got {status} {body}"
    );
    assert_eq!(
        crate::harness::error_code(&body),
        "InvalidParameterException",
        "case the wildcard ARN form ({arn}): a modelled constraint refusal, got {body}"
    );

    // The group that does exist is untouched by the refusals.
    assert_eq!(
        bearer_of(&logs, present).await,
        Some(false),
        "case after the refusals: a refused put persists nothing"
    );
}

/// A key association naming no resolvable target, or no key, is refused — and the refusal
/// persists nothing.
#[tokio::test]
async fn a_key_association_with_a_bad_target_or_key_is_refused() {
    let logs = Logs::new();
    let keys = kms_key_ids();
    let group = "encrypted.group";
    logs.ok(
        "CreateLogGroup",
        &json!({"logGroupName": group, "kmsKeyId": keys[0]}),
    )
    .await;
    let (_arn, log_group_arn) = logs.group_arns(group).await;
    let query_result = query_result_arn();

    // "you must specify either the resourceIdentifier parameter or the logGroup
    // parameter, but you can't specify both", and a resourceIdentifier that names
    // neither a log group nor the query-result resource names no target.
    //
    // The two forms differ in WHAT is wrong: a request naming both target members, or
    // neither, breaks the rule relating the two members and is refused as a request
    // validation; a resourceIdentifier that resolves to no resource is a bad value, which
    // is the operation's own modelled parameter error.
    let bad_targets = [
        ("neither parameter", json!({}), "ValidationException"),
        (
            "both parameters",
            json!({"logGroupName": group, "resourceIdentifier": log_group_arn}),
            "ValidationException",
        ),
        (
            "a resourceIdentifier naming another service's resource",
            json!({"resourceIdentifier": engine::emulator_arn("s3", "", "a-bucket")}),
            "InvalidParameterException",
        ),
        (
            "a resourceIdentifier naming a log stream",
            json!({"resourceIdentifier": engine::emulator_arn(
                "logs",
                REGION,
                &format!("log-group:{group}:log-stream:a-stream"),
            )}),
            "InvalidParameterException",
        ),
        (
            "a resourceIdentifier that is no ARN at all",
            json!({"resourceIdentifier": group}),
            "InvalidParameterException",
        ),
    ];
    for (case, target, expected) in bad_targets {
        let mut associate = target.clone();
        associate
            .as_object_mut()
            .expect("the request body is an object")
            .insert("kmsKeyId".to_string(), json!(keys[1]));
        for (operation, body) in [
            ("AssociateKmsKey", associate),
            ("DisassociateKmsKey", target),
        ] {
            let code = logs.err(operation, &body).await;
            assert_eq!(
                code, expected,
                "case {operation} with {case}: a target naming nothing is refused, got {code}"
            );
        }
    }

    // A target that resolves to a log group nobody created is a missing resource, not a
    // bad parameter.
    for (operation, body) in [
        (
            "AssociateKmsKey",
            json!({"logGroupName": "no-such-group", "kmsKeyId": keys[1]}),
        ),
        (
            "DisassociateKmsKey",
            json!({"resourceIdentifier": log_group_arn.replace(group, "absent/group")}),
        ),
    ] {
        let code = logs.err(operation, &body).await;
        assert_eq!(
            code, "ResourceNotFoundException",
            "case {operation} on an unknown log group: refused as a missing resource, got {code}"
        );
    }

    // `kmsKeyId` is "the ARN of the KMS key": a value naming no key is refused. `KmsKeyId`
    // is modelled `@length 0..256`, so an empty string reaches this handler.
    let bad_keys = [
        ("an empty key id", String::new()),
        ("a blank key id", "   ".to_string()),
        (
            "an ARN of another service",
            engine::emulator_arn("dynamodb", REGION, "table/Orders"),
        ),
        (
            "a KMS ARN naming no resource",
            engine::emulator_arn("kms", REGION, ""),
        ),
    ];
    for (case, key) in bad_keys {
        let code = logs
            .err(
                "AssociateKmsKey",
                &json!({"logGroupName": group, "kmsKeyId": key}),
            )
            .await;
        assert_eq!(
            code, "InvalidParameterException",
            "case {case} ({key:?}): a kmsKeyId naming no key is refused, got {code}"
        );
    }

    // Every refusal above left the key the group was created with in place, and the
    // query-result target unassociated by any of them.
    assert_eq!(
        kms_key_of(&logs, group).await.as_deref(),
        Some(keys[0].as_str()),
        "case after the refusals: a refused association persists nothing"
    );
    logs.ok(
        "DisassociateKmsKey",
        &json!({"resourceIdentifier": query_result}),
    )
    .await;
}
