// SPDX-License-Identifier: CC0-1.0

//! The two rules a published default-quota row obeys ACROSS A WHOLE CLASS, rather than
//! one row at a time.
//!
//! * A service that publishes a quota per API operation (or per throttling category)
//!   words one sentence for the class and names the row's own subject in it: so for every
//!   row of such a class, the `Description` is that class's sentence with the subject the
//!   row's own `QuotaName` states, and where the class is metered, the `Period` and the
//!   `Resource` dimension of the `UsageMetric` name that same subject's window and call
//!   count. A row of the class whose text or metric was authored on its own fails here,
//!   which is what keeps one row from stating another quota's rule.
//! * A `ServiceName` is the display name of the `ServiceCode`, not of a row: every row of
//!   both quota listings, and every single read, reports the one name `ListServices`
//!   reports for that code.
//!
//! Both rules are asserted over EVERY row the covered services publish — the tables below
//! name classes and service codes, never quota codes — so a row added to a class later is
//! held to the same rule. The classes span a rate published with its window in its own
//! name and no metric, a rate metered with an `AWS/Usage` call count over a one-second
//! window, a per-operation throttle, a count of running jobs, a class whose sentence names
//! the account and the Region, and one that names the throttled operation after its own
//! phrase rather than before it; the service codes span a display name carrying a
//! parenthesised code, one carrying a parenthesised qualifier, plain product names, and two
//! services this issue never cited.
//!
//! The harness (`client`, `emulator`, `default_quotas`, `service_inventory`, `walk_pages`)
//! is the one `quota_workflow.rs` owns.

use serde_json::{json, Value};

use crate::quota_workflow::{client, default_quotas, emulator, service_inventory, walk_pages};

/// The modelled ceiling on a `Description`: `QuotaDescription` carries `@length 0..350`
/// (`smithy_query(query="shape", shape="ServiceQuota")`, member `Description`).
const DESCRIPTION_MAX: usize = 350;

/// One CLASS of published rows: how a row of it is recognised by its `QuotaName`, and
/// what that name's subject implies about the rest of the row.
///
/// SOURCE: AWS's published Service Quotas data for the service code the class belongs to,
/// as `src/state/quota_rows/<service>.rs` carries it — one constructor per class there,
/// which is the implementation of exactly this rule.
struct Class {
    /// The `ServiceCode` whose rows belong to the class.
    service_code: &'static str,
    /// What a row's `QuotaName` starts with, and what it ends with: between them sits the
    /// SUBJECT — the API operation, or the throttling category, the quota is published
    /// for.
    name_prefix: &'static str,
    name_suffix: &'static str,
    /// A plane suffix the name may carry after [`Class::name_suffix`], which names the
    /// plane the subject's quota is served on rather than part of the subject.
    name_plane: &'static str,
    /// The sentence the service publishes for the class, with `{}` where the subject goes.
    /// The second form is the one a row of the class carries when AWS publishes the quota
    /// as NOT adjustable, for a class that words the two differently.
    sentence: &'static str,
    fixed_sentence: &'static str,
    /// `(PeriodValue, PeriodUnit)` for a class published over a window, and the
    /// `AWS/Usage` `CallCount` the subject's calls are metered with, when it carries one.
    period: Option<(i64, &'static str)>,
    metered: bool,
    /// The `Service` dimension the class's call count is published under.
    usage_service: &'static str,
}

/// The classes asserted here.
const CLASSES: &[Class] = &[
    // Amazon Cognito Federated Identities publishes a request rate per identity-pool API
    // operation, with the window stated in the quota's own text and no metric block.
    Class {
        service_code: "cognito-identity",
        name_prefix: "Rate of ",
        name_suffix: " requests",
        name_plane: "",
        sentence: "The maximum number of {} requests that you can make per second.",
        fixed_sentence: "The maximum number of {} requests that you can make per second.",
        period: None,
        metered: false,
        usage_service: "",
    },
    // Amazon Cognito User Pools throttles by CATEGORY: a one-second window, the category's
    // own call count, and two sentences — the provisionable one for a rate AWS lets you
    // request an increase for, the shared-quota one for a fixed rate.
    Class {
        service_code: "cognito-idp",
        name_prefix: "Rate of ",
        name_suffix: " requests",
        name_plane: " per account",
        sentence: "The maximum combined call rate (requests per second) you can request for all \
                   API operations in the {} category. Provision your rate limit within this quota \
                   from the Amazon Cognito console or APIs. For the list of included operations, \
                   see https://docs.aws.amazon.com/cognito/latest/developerguide/\
                   limits.html#category_operations.",
        fixed_sentence: "The maximum combined call rate (requests per second) for all API \
                         operations in the {} category. All operations within a category share \
                         the same quota. For the list of included operations, see \
                         https://docs.aws.amazon.com/cognito/latest/developerguide/\
                         limits.html#category_operations.",
        period: Some((1, "SECOND")),
        metered: true,
        usage_service: "Cognito User Pool",
    },
    // Amazon Comprehend publishes a per-operation throttle in transactions per second...
    Class {
        service_code: "comprehend",
        name_prefix: "",
        name_suffix: " throttle limit in transactions per second",
        name_plane: "",
        sentence: "The maximum number of {} requests allowed per account per second, in the \
                   current Region",
        fixed_sentence: "The maximum number of {} requests allowed per account per second, in the \
                         current Region",
        period: None,
        metered: false,
        usage_service: "",
    },
    // ... and, for an asynchronous operation, the number of jobs it may have running.
    Class {
        service_code: "comprehend",
        name_prefix: "",
        name_suffix: " max active jobs",
        name_plane: "",
        sentence: "The maximum number of active jobs for API {} allowed per account, in the \
                   current Region",
        fixed_sentence: "The maximum number of active jobs for API {} allowed per account, in the \
                         current Region",
        period: None,
        metered: false,
        usage_service: "",
    },
    // AWS Certificate Manager publishes a request rate per API operation, over a
    // one-second window and metered with that operation's own call count.
    Class {
        service_code: "acm",
        name_prefix: "Rate of ",
        name_suffix: " API requests",
        name_plane: "",
        sentence: "The maximum number of {} API requests per second.",
        fixed_sentence: "The maximum number of {} API requests per second.",
        period: Some((1, "SECOND")),
        metered: true,
        usage_service: "Certificate Manager",
    },
    // AWS Private CA words its per-operation rate as the requests an account may perform
    // in a Region each second, and publishes no metric block for it.
    Class {
        service_code: "acm-pca",
        name_prefix: "Rate of ",
        name_suffix: " requests",
        name_plane: "",
        sentence: "The maximum number of {} requests that can be performed in an account in \
                   the current Region each second.",
        fixed_sentence: "The maximum number of {} requests that can be performed in an account \
                         in the current Region each second.",
        period: Some((1, "SECOND")),
        metered: false,
        usage_service: "",
    },
    // AWS Compute Optimizer Automation names the throttled operation after its own
    // phrase rather than before it.
    Class {
        service_code: "aco-automation",
        name_prefix: "Throttle rate limit for ",
        name_suffix: "",
        name_plane: "",
        sentence: "Maximum number of {} requests you can make per second per account in the \
                   current region.",
        fixed_sentence: "Maximum number of {} requests you can make per second per account in \
                         the current region.",
        period: Some((1, "SECOND")),
        metered: false,
        usage_service: "",
    },
    // AWS Agent Registry states the per-second window in the quota's own name and meters
    // the operation's call count; its sentence names the account and the Region.
    Class {
        service_code: "agent-registry",
        name_prefix: "",
        name_suffix: " requests per second",
        name_plane: "",
        sentence: "The maximum number of {} API requests per second per account per Region",
        fixed_sentence: "The maximum number of {} API requests per second per account per \
                         Region",
        period: Some((1, "SECOND")),
        metered: true,
        usage_service: "Agent Registry",
    },
];

/// The rows a class service publishes that belong to NO rate class: a COUNT of a
/// resource rather than a rate, so such a row is published with the quota's own text and
/// with neither a window nor a call count. Named by `(ServiceCode, QuotaName)`, so a row
/// whose name states a rate can never be excused by this table.
///
/// SOURCE: AWS's published Service Quotas data for the service code, as
/// `src/state/quota_rows/cognito_idp.rs` carries it: Amazon Cognito User Pools publishes
/// a count of the account's user pools and a count of one pool's resource servers beside
/// its category rates.
const COUNT_ROWS: &[(&str, &str)] = &[
    ("cognito-idp", "Resource servers per user pool"),
    ("cognito-idp", "User pools per account"),
];

/// The display name AWS's integrated-service inventory reports for a `ServiceCode`.
///
/// SOURCE: AWS's own Service Quotas service catalog (what `aws service-quotas
/// list-services` reports), as `src/state.rs::SEED_SERVICE_CATALOG` carries it. The rows
/// span a name carrying a parenthesised service code, one carrying a parenthesised
/// qualifier, and plain product names; `es` and `monitoring` are codes this suite's issue
/// never cited.
const DISPLAY_NAMES: &[(&str, &str)] = &[
    ("cognito-identity", "Amazon Cognito Federated Identities"),
    ("comprehend", "Amazon Comprehend"),
    ("dax", "Amazon DynamoDB Accelerator (DAX)"),
    ("directconnect", "AWS Direct Connect"),
    ("dms", "AWS Database Migration Service (AWS DMS)"),
    ("docdb", "Amazon DocumentDB (with MongoDB compatibility)"),
    ("es", "Amazon OpenSearch Service"),
    ("monitoring", "Amazon CloudWatch"),
    ("quicksight", "Amazon QuickSight"),
];

/// The subject of `quota_name` within `class`, and whether the name states the class's
/// plane — `None` when the name does not belong to the class at all.
fn subject<'name>(class: &Class, quota_name: &'name str) -> Option<&'name str> {
    let body = quota_name.strip_prefix(class.name_prefix)?;
    let body = if class.name_plane.is_empty() {
        body
    } else {
        body.strip_suffix(class.name_plane).unwrap_or(body)
    };
    body.strip_suffix(class.name_suffix)
}

/// The class's sentence with `subject` named in it.
fn sentence(class: &Class, subject: &str, adjustable: bool) -> String {
    let template = if adjustable {
        class.sentence
    } else {
        class.fixed_sentence
    };
    template.replace("{}", subject)
}

/// Every row of every class reports the class's own sentence, window and call count for
/// the subject its name states.
#[tokio::test]
async fn every_row_of_a_class_reports_that_class_facts_for_its_own_subject() {
    let engine = emulator();
    let caller = client(&engine, "us-west-2");

    let mut checked = 0_usize;
    for class in CLASSES {
        for row in default_quotas(&caller, class.service_code).await {
            let name = row["QuotaName"].as_str().unwrap_or_default();
            let Some(subject) = subject(class, name) else {
                continue;
            };
            let case = format!(
                "{}/{} {name:?}",
                class.service_code,
                row["QuotaCode"].as_str().unwrap_or_default()
            );
            checked += 1;

            let adjustable = row["Adjustable"] == json!(true);
            assert_eq!(
                row["Description"],
                json!(sentence(class, subject, adjustable)),
                "case {case}: expected the sentence the service publishes for the class with \
                 this row's own subject named in it, got {row}"
            );
            match class.period {
                Some((value, unit)) => assert_eq!(
                    row["Period"],
                    json!({"PeriodValue": value, "PeriodUnit": unit}),
                    "case {case}: expected the window the class is published over, got {row}"
                ),
                None => assert!(
                    row.get("Period").is_none_or(Value::is_null),
                    "case {case}: this class states its window in the quota's own name and is \
                     published over none, got {row}"
                ),
            }
            if class.metered {
                assert_eq!(
                    row["UsageMetric"],
                    json!({
                        "MetricNamespace": "AWS/Usage",
                        "MetricName": "CallCount",
                        "MetricStatisticRecommendation": "Sum",
                        "MetricDimensions": {
                            "Service": class.usage_service,
                            "Class": "None",
                            "Resource": subject,
                            "Type": "API",
                        },
                    }),
                    "case {case}: expected the call count of this row's own subject, got {row}"
                );
            } else {
                assert!(
                    row.get("UsageMetric").is_none_or(Value::is_null),
                    "case {case}: this class publishes no usage metric, got {row}"
                );
            }
        }
    }

    assert!(
        checked >= 15,
        "case classes: the rule must be asserted over the whole of each class, got {checked} rows"
    );
}

/// Every `Description` a covered service publishes stays within the modelled ceiling, so
/// a class sentence cannot grow past what the member allows.
#[tokio::test]
async fn a_published_description_stays_within_the_modelled_length() {
    let engine = emulator();
    let caller = client(&engine, "eu-west-1");

    for (service_code, _) in DISPLAY_NAMES {
        for row in default_quotas(&caller, service_code).await {
            let case = format!(
                "{service_code}/{}",
                row["QuotaCode"].as_str().unwrap_or_default()
            );
            // A row this catalog carries no published text for reports no `Description`
            // member at all; the ceiling is a rule about the text a row DOES carry.
            let Some(description) = row["Description"].as_str() else {
                continue;
            };
            assert!(
                !description.is_empty() && description.chars().count() <= DESCRIPTION_MAX,
                "case {case}: a description is 1..={DESCRIPTION_MAX} characters, got {} in {row}",
                description.chars().count()
            );
        }
    }
}

/// A `ServiceName` is the display name of the service CODE: the one `ListServices`
/// reports, carried identically by every row of both quota listings and by both single
/// reads.
#[tokio::test]
async fn a_service_name_is_the_display_name_of_the_code_on_every_row() {
    let engine = emulator();
    let caller = client(&engine, "ap-south-1");

    let inventory = service_inventory(&caller).await;
    for (service_code, display_name) in DISPLAY_NAMES {
        let listed = inventory
            .iter()
            .find(|service| service["ServiceCode"] == json!(service_code))
            .cloned()
            .unwrap_or(Value::Null);
        assert_eq!(
            listed["ServiceName"],
            json!(display_name),
            "case {service_code}: ListServices reports the service's display name, got {listed}"
        );

        for operation in ["ListAWSDefaultServiceQuotas", "ListServiceQuotas"] {
            let rows = walk_pages(
                &caller,
                operation,
                &json!({"ServiceCode": service_code}),
                "Quotas",
                None,
            )
            .await;
            assert!(
                !rows.is_empty(),
                "case {service_code}: {operation} must publish this service's rows"
            );
            for row in &rows {
                assert_eq!(
                    row["ServiceName"],
                    json!(display_name),
                    "case {service_code}: {operation} must report the code's one display name, \
                     got {row}"
                );
            }

            // And the single reads of the first row resolve the same name.
            let quota_code = rows[0]["QuotaCode"].clone();
            for read in ["GetAWSDefaultServiceQuota", "GetServiceQuota"] {
                let record = caller
                    .ok(
                        read,
                        json!({"ServiceCode": service_code, "QuotaCode": quota_code}),
                    )
                    .await["Quota"]
                    .clone();
                assert_eq!(
                    record["ServiceName"],
                    json!(display_name),
                    "case {service_code}: {read} must report the code's one display name, got \
                     {record}"
                );
            }
        }
    }
}

/// The classes above cover every row the services that publish them have: a row outside
/// every class would be one whose text no class rule holds, which is how a hand-authored
/// row slipped in.
#[tokio::test]
async fn a_class_service_publishes_no_row_outside_its_classes() {
    let engine = emulator();
    let caller = client(&engine, "us-east-2");

    let mut service_codes: Vec<&str> = CLASSES.iter().map(|class| class.service_code).collect();
    service_codes.dedup();

    for service_code in service_codes {
        for row in default_quotas(&caller, service_code).await {
            let name = row["QuotaName"].as_str().unwrap_or_default();
            let code = row["QuotaCode"].as_str().unwrap_or_default();
            let matched = CLASSES
                .iter()
                .filter(|class| class.service_code == service_code)
                .any(|class| subject(class, name).is_some());
            if COUNT_ROWS.contains(&(service_code, name)) {
                // A count is published over no window and with no call count: the class
                // rules above are about the rates, so the exemption is only ever a row
                // AWS publishes as a count. Its `Description` is the quota's own text,
                // which this catalog publishes only where it can cite it — never wording
                // authored here — so a count carries none.
                assert!(
                    !matched,
                    "case {service_code}/{code}: a count is not a rate, got {name:?}"
                );
                assert!(
                    row.get("Description").is_none_or(Value::is_null),
                    "case {service_code}/{code}: this catalog cites no text for a count, so it \
                     publishes none, got {row}"
                );
                assert!(
                    row.get("Period").is_none_or(Value::is_null)
                        && row.get("UsageMetric").is_none_or(Value::is_null),
                    "case {service_code}/{code}: a count is published over no window and with \
                     no call count, got {row}"
                );
                continue;
            }
            assert!(
                matched,
                "case {service_code}/{code}: every row of this service belongs to one of its \
                 published classes, got {name:?}"
            );
        }
    }
}

/// A caller that reads a class row through the APPLIED plane sees the same subject's
/// facts: the class's sentence travels with the quota code, not with the listing.
#[tokio::test]
async fn an_applied_read_of_a_class_row_reports_the_same_class_facts() {
    let engine = emulator();
    let caller = client(&engine, "eu-central-1");

    for class in CLASSES {
        for row in default_quotas(&caller, class.service_code).await {
            let name = row["QuotaName"].as_str().unwrap_or_default();
            let Some(subject) = subject(class, name) else {
                continue;
            };
            let case = format!(
                "{}/{}",
                class.service_code,
                row["QuotaCode"].as_str().unwrap_or_default()
            );
            let applied = caller
                .ok(
                    "GetServiceQuota",
                    json!({
                        "ServiceCode": class.service_code,
                        "QuotaCode": row["QuotaCode"].clone(),
                    }),
                )
                .await["Quota"]
                .clone();
            assert_eq!(
                (
                    applied["QuotaName"].clone(),
                    applied["Description"].clone(),
                    applied["Value"].clone()
                ),
                (
                    json!(name),
                    json!(sentence(class, subject, row["Adjustable"] == json!(true))),
                    row["Value"].clone()
                ),
                "case {case}: the applied read resolves the same class row, got {applied}"
            );
        }
    }
}
