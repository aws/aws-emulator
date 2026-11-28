// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP tests for the telemetry-pipeline slice of this service.
//!
//! Every case drives the real wire path — the production `MultiServiceRouter`,
//! restJson1 serialization, the modelled-constraint walk and the handlers — through
//! the ONE harness this test crate has (`telemetry_rules`), so what is asserted is
//! what a signed AWS SDK request observes.
//!
//! The rules under test:
//!
//! 1. `CreateTelemetryPipeline` stores ONE pipeline record, keyed by the region the
//!    request was signed for and the name, and answers its ARN; every member
//!    `GetTelemetryPipeline` reports — name, ARN, configuration body, tags and
//!    timestamps — comes from that record, whatever the configuration looks like;
//! 2. a pipeline is `CREATING` until the transition the create started comes due and
//!    `ACTIVE` from then on, so a read straight after the create still sees
//!    `CREATING` and no read ever has to write;
//! 3. the configuration body is persisted and reported back VERBATIM whatever it
//!    contains, for every body the modelled `Body` length (1..24000) admits, and a
//!    body outside that length is a 400 `ValidationException` that stores nothing:
//!    this service's model carries no schema for the sources, processors and sinks a
//!    pipeline definition names, and `ValidateTelemetryPipelineConfiguration` — the
//!    operation documented to report per-field configuration errors — is a separate
//!    scaffold stub with its own issue, so the create invents no verdict about the
//!    document's contents;
//! 4. a pipeline name identifies a pipeline within the region the request was signed
//!    for, so a second create of the same name there is a 409 and the same name in
//!    another region is a different pipeline;
//! 5. an identifier resolves as a bare name or as the full ARN, and anything that
//!    names no stored pipeline is a modelled 404.

use std::time::Duration;

use serde_json::{json, Value};
use service_com_amazonaws_observabilityadmin_observabilityadmin::ops::types::TELEMETRY_PIPELINE_STATUS_VALUES;
use service_com_amazonaws_observabilityadmin_observabilityadmin::state::{
    is_minted_resource_id, now_epoch_millis, pipeline_arn,
    ComAmazonawsObservabilityadminObservabilityadminState, StoredTelemetryPipeline,
    MAX_STORED_PIPELINE_RECORDS, MAX_TELEMETRY_PIPELINES, PIPELINE_ACTIVATION_DELAY_MS,
};

use crate::telemetry_rules::{call, emulator, error_code, Emulator, ACCOUNT};

/// The region most requests below sign for; the ARNs they mint carry exactly this one.
const HOME_REGION: &str = "us-west-2";

/// A second region, so "unique within the account" can be shown to be per-region.
const OTHER_REGION: &str = "eu-west-1";

/// Long enough after a create for the `CREATING` -> `ACTIVE` transition to have come
/// due, whatever scheduling delay the test run adds on top.
fn past_activation_delay() -> Duration {
    Duration::from_millis(PIPELINE_ACTIVATION_DELAY_MS as u64 + 250)
}

async fn create(emulator: &Emulator, region: &str, body: &Value) -> (u16, Value) {
    call(emulator, region, "/CreateTelemetryPipeline", body).await
}

async fn get(emulator: &Emulator, region: &str, identifier: &str) -> (u16, Value) {
    call(
        emulator,
        region,
        "/GetTelemetryPipeline",
        &json!({ "PipelineIdentifier": identifier }),
    )
    .await
}

/// The `Arn` a successful create reported.
fn created_arn(body: &Value) -> String {
    body.get("Arn")
        .and_then(Value::as_str)
        .unwrap_or("<no Arn>")
        .to_string()
}

/// The ARN the service must mint for the pipeline id `id` in `region`.
fn arn_of(region: &str, id: &str) -> String {
    format!("arn:aws:observabilityadmin:{region}:{ACCOUNT}:telemetry-pipeline/{id}")
}

/// The id inside a pipeline ARN, having asserted the ARN is one this service mints:
/// the resource type its ARNs carry, and then the opaque id the service assigned the
/// pipeline (asked of the service's own predicate, so the test cannot drift from the
/// minting site).
fn minted_id(case: &str, region: &str, arn: &str) -> String {
    let prefix = arn_of(region, "");
    let Some(id) = arn.strip_prefix(&prefix) else {
        panic!("case {case:?}: expected a pipeline ARN under {prefix}, got {arn:?}");
    };
    assert!(
        is_minted_resource_id(id),
        "case {case:?}: expected the service-assigned pipeline id to be an opaque \
         hexadecimal token, got {id:?}"
    );
    id.to_string()
}

/// A `TelemetryPipeline` member of a `GetTelemetryPipeline` response.
fn member<'a>(response: &'a Value, name: &str) -> Option<&'a Value> {
    response.get("Pipeline").and_then(|pipeline| {
        pipeline
            .get(name)
            .filter(|value| !matches!(value, Value::Null))
    })
}

/// One well-formed pipeline configuration. The rows differ along what the round-trip
/// rule generalises over: whether the document nests its sections under a named
/// pipeline section or writes them at the top level, how many processors and sinks it
/// declares, whether it chains two pipeline sections, and whether it is written in
/// YAML block style or in the JSON that YAML 1.2 is a superset of.
fn valid_configurations() -> Vec<(&'static str, &'static str)> {
    vec![
        (
            "top-level source and sink, no processors",
            "source:\n  \
               cloudwatch_logs:\n    \
                 log_group: /aws/lambda/example\n\
             sink:\n  \
               - cloudwatch_logs:\n      \
                   log_group: /aws/telemetry/example\n",
        ),
        (
            "a named pipeline section with a version and two processors",
            "version: \"2\"\n\
             log-pipeline:\n  \
               source:\n    \
                 s3:\n      \
                   bucket: example-telemetry\n  \
               processor:\n    \
                 - grok:\n        \
                     match: {}\n    \
                 - date:\n        \
                     from_time_received: true\n  \
               sink:\n    \
                 - cloudwatch_logs:\n        \
                     log_group: /aws/telemetry/example\n",
        ),
        (
            "two sinks",
            "source:\n  \
               http: {}\n\
             sink:\n  \
               - cloudwatch_logs: {}\n  \
               - s3:\n      \
                   bucket: example-archive\n",
        ),
        (
            "a sub-pipeline chain: two sections, one feeding the other",
            "version: \"2\"\n\
             entry-pipeline:\n  \
               source:\n    \
                 s3:\n      \
                   bucket: example-telemetry\n  \
               sink:\n    \
                 - pipeline:\n        \
                     name: enrich-pipeline\n\
             enrich-pipeline:\n  \
               source:\n    \
                 pipeline:\n      \
                   name: entry-pipeline\n  \
               sink:\n    \
                 - cloudwatch_logs: {}\n",
        ),
        (
            "JSON, which YAML 1.2 is a superset of",
            "{\"source\": {\"http\": {}}, \"sink\": [{\"cloudwatch_logs\": {}}]}",
        ),
        (
            "plugins named with no settings at all",
            "source:\n  http:\nsink:\n  - cloudwatch_logs:\n",
        ),
        (
            // Hyphens are pervasive in real configuration VALUES (region codes, ARNs,
            // hyphenated resource names) and none of them opens a YAML node, so a
            // line full of them is a configuration this service stores.
            "a flow sequence of hyphen-rich ARNs and region codes",
            "source:\n  \
               cloudwatch_logs:\n    \
                 log_groups: [arn:aws:logs:us-east-1:000000000000:log-group:my-log-group-one, \
                              arn:aws:logs:eu-west-1:000000000000:log-group:my-log-group-two, \
                              arn:aws:logs:ap-south-1:000000000000:log-group:my-log-group-three]\n\
             sink:\n  \
               - cloudwatch_logs:\n      \
                   log_group: /aws/telemetry/my-central-log-group\n",
        ),
    ]
}

#[tokio::test]
async fn a_create_persists_the_pipeline_every_read_reports() {
    let emulator = emulator();

    // The rows span the configuration forms above and, independently, the name
    // lengths the model admits (3..28) and whether the create carried tags at all.
    for (index, (case, body)) in valid_configurations().into_iter().enumerate() {
        let name = match index % 3 {
            0 => format!("pip{index}"),
            // A name of the modelled maximum length (28 characters).
            1 => format!("pipeline-{index}-{}", "x".repeat(28 - 10 - index.to_string().len())),
            _ => format!("telemetry-pipeline-{index}"),
        };
        assert!(
            (3..=28).contains(&name.len()),
            "case {case:?}: the test's own name must satisfy the modelled 3..28 length, \
             got {name:?} of length {}",
            name.len()
        );
        let tags = (index % 2 == 0).then(|| json!({ "team": "observability", "tier": "1" }));

        let mut request = json!({ "Name": name, "Configuration": { "Body": body } });
        if let Some(tags) = &tags {
            request["Tags"] = tags.clone();
        }
        let (status, created) = create(&emulator, HOME_REGION, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateTelemetryPipeline to answer 200, \
             got {status} {created:?}"
        );
        let arn = created_arn(&created);
        let id = minted_id(case, HOME_REGION, &arn);
        assert_eq!(
            arn,
            arn_of(HOME_REGION, &id),
            "case {case:?}: CreateTelemetryPipeline minted the wrong ARN"
        );

        // The identifier resolves as a bare name AND as the full ARN — the two forms
        // `PipelineIdentifier`'s documentation names — and both must report the same
        // persisted record.
        for identifier in [name.clone(), arn.clone()] {
            let (status, got) = get(&emulator, HOME_REGION, &identifier).await;
            assert_eq!(
                status, 200,
                "case {case:?}: expected GetTelemetryPipeline({identifier}) to answer 200, \
                 got {status} {got:?}"
            );
            assert_eq!(
                member(&got, "Name").and_then(Value::as_str),
                Some(name.as_str()),
                "case {case:?}: GetTelemetryPipeline({identifier}) reported the wrong Name: {got:?}"
            );
            assert_eq!(
                member(&got, "Arn").and_then(Value::as_str),
                Some(arn.as_str()),
                "case {case:?}: GetTelemetryPipeline({identifier}) reported the wrong Arn: {got:?}"
            );
            assert_eq!(
                member(&got, "Configuration")
                    .and_then(|configuration| configuration.get("Body"))
                    .and_then(Value::as_str),
                Some(body),
                "case {case:?}: GetTelemetryPipeline({identifier}) did not report the \
                 configuration body that was created: {got:?}"
            );
            assert_eq!(
                member(&got, "Tags"),
                Some(tags.clone().unwrap_or_else(|| json!({}))).as_ref(),
                "case {case:?}: GetTelemetryPipeline({identifier}) reported the wrong Tags: {got:?}"
            );
            let status_value = member(&got, "Status")
                .and_then(Value::as_str)
                .unwrap_or("<no Status>")
                .to_string();
            assert!(
                TELEMETRY_PIPELINE_STATUS_VALUES.contains(&status_value.as_str()),
                "case {case:?}: Status must be one of {TELEMETRY_PIPELINE_STATUS_VALUES:?}, \
                 got {status_value:?}"
            );
            // This read follows the create closely, so the status is either the
            // transient one or the terminal one the transition moves to — never any
            // other modelled value. The dedicated transition test pins which is which
            // at a known instant; asserting both here keeps the table free of a
            // wall-clock race on a loaded machine.
            assert!(
                matches!(status_value.as_str(), "CREATING" | "ACTIVE"),
                "case {case:?}: a freshly created pipeline is CREATING or ACTIVE, \
                 got {status_value:?}"
            );

            let created_at = member(&got, "CreatedTimeStamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            let updated_at = member(&got, "LastUpdateTimeStamp")
                .and_then(Value::as_i64)
                .unwrap_or_default();
            // Epoch milliseconds: any real clock is far past this, and a create sets
            // both stamps to the same instant.
            assert!(
                created_at > 1_600_000_000_000,
                "case {case:?}: expected an epoch-millisecond CreatedTimeStamp, got {created_at}"
            );
            assert_eq!(
                created_at, updated_at,
                "case {case:?}: a create sets LastUpdateTimeStamp to CreatedTimeStamp"
            );
        }
    }
}

#[tokio::test]
async fn a_pipeline_is_creating_until_the_transition_the_create_started_comes_due() {
    let emulator = emulator();
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "Name": "activation-span",
            "Configuration": { "Body": "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n" }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");

    // The span is never zero, so the caller that read straight after the create still
    // observes the transient status and the reason that goes with it.
    let (status, creating) = get(&emulator, HOME_REGION, "activation-span").await;
    assert_eq!(status, 200, "expected 200, got {status} {creating:?}");

    // The span the create started is NON-ZERO and is what the status is derived
    // from: the stored due instant is strictly after the create instant, so a caller
    // reading inside it observes the transient status. Reading the record's own due
    // instant is what makes the assertion below independent of how long this test
    // took to issue its read.
    let (created_ms, active_at_ms) = {
        let state = emulator
            .engine
            .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
        let store = state.lock();
        let record = store
            .pipeline(HOME_REGION, "activation-span")
            .expect("the create stored a pipeline record");
        (record.created_timestamp_ms, record.active_at_ms)
    };
    assert!(
        active_at_ms > created_ms,
        "the CREATING -> ACTIVE span must be non-zero: created {created_ms}, due {active_at_ms}"
    );

    let status_value = member(&creating, "Status")
        .and_then(Value::as_str)
        .unwrap_or("<no Status>")
        .to_string();
    let reason_while_creating = member(&creating, "StatusReason")
        .and_then(|reason| reason.get("Description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    if now_epoch_millis() < active_at_ms {
        assert_eq!(
            status_value, "CREATING",
            "a read inside the span reports CREATING, got {creating:?}"
        );
        assert!(
            !reason_while_creating.is_empty(),
            "a CREATING pipeline reports why, got {creating:?}"
        );
    }
    let created_at = member(&creating, "CreatedTimeStamp")
        .and_then(Value::as_i64)
        .unwrap_or_default();

    tokio::time::sleep(past_activation_delay()).await;

    let (status, active) = get(&emulator, HOME_REGION, "activation-span").await;
    assert_eq!(status, 200, "expected 200, got {status} {active:?}");
    assert_eq!(
        member(&active, "Status").and_then(Value::as_str),
        Some("ACTIVE"),
        "once the transition comes due the pipeline is ACTIVE, got {active:?}"
    );
    let active_reason = member(&active, "StatusReason")
        .and_then(|reason| reason.get("Description"))
        .and_then(Value::as_str)
        .unwrap_or_default()
        .to_string();
    assert!(
        !active_reason.is_empty(),
        "every status a pipeline reports carries the reason for it, got {active:?}"
    );
    if status_value == "CREATING" {
        assert_ne!(
            active_reason, reason_while_creating,
            "the reason describes the status the pipeline is IN, so the ACTIVE one is not the \
             CREATING one, got {active:?}"
        );
    }
    // The clock moved the status; it did not re-create the resource.
    assert_eq!(
        member(&active, "CreatedTimeStamp").and_then(Value::as_i64),
        Some(created_at),
        "the transition leaves CreatedTimeStamp alone, got {active:?}"
    );
    assert_eq!(
        member(&active, "LastUpdateTimeStamp").and_then(Value::as_i64),
        Some(created_at),
        "nothing updated the pipeline, so LastUpdateTimeStamp is the create instant: {active:?}"
    );
}

/// The configuration body is stored and reported back VERBATIM for every body the
/// modelled `Body` length admits, and a body outside that length stores nothing.
///
/// The rows span what the rule generalises over — the SHAPE of the document (a
/// pipeline definition, a bare scalar, a YAML sequence, JSON, an unterminated flow
/// collection, bytes that are not YAML at all, leading/trailing whitespace that a
/// parser would discard) and its LENGTH, on each published bound of
/// `TelemetryPipelineConfiguration$Body` (`length: min 1, max 24000`, as
/// `smithy_query(query="shape",shape="TelemetryPipelineConfiguration")` reports) and
/// one character past each.
#[tokio::test]
async fn a_configuration_body_round_trips_verbatim_within_the_modelled_length() {
    let emulator = emulator();

    // `Body` is 1..24000 CHARACTERS: the two accepted rows sit exactly on those
    // bounds, the two refused rows one character outside them.
    let shortest = "x".to_string();
    let longest = "y".repeat(24_000);
    let too_long = "z".repeat(24_001);

    let rows: Vec<(&str, &str, u16)> = vec![
        (
            "a pipeline definition in YAML block style",
            "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n",
            200,
        ),
        (
            "the same definition written as JSON, which YAML 1.2 is a superset of",
            "{\"source\": {\"http\": {}}, \"sink\": [{\"cloudwatch_logs\": {}}]}",
            200,
        ),
        ("a YAML sequence rather than a mapping", "- source\n- sink\n", 200),
        ("an unterminated flow collection", "source: [cloudwatch_logs", 200),
        ("bytes that are not YAML at all", "\t\tsource: {{{ !!", 200),
        (
            "leading and trailing whitespace a parser would discard",
            "\n\n  source:\n    http: {}\n  sink:\n    - s3: {}\n\n  ",
            200,
        ),
        ("exactly the 1-character minimum", shortest.as_str(), 200),
        ("exactly the 24000-character maximum", longest.as_str(), 200),
        ("one character past the 24000 maximum", too_long.as_str(), 400),
        ("the empty body, one character short of the minimum", "", 400),
    ];

    for (index, (case, body, expected)) in rows.into_iter().enumerate() {
        let name = format!("verbatim-{index}");
        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "Name": name, "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} (body of {} characters): expected {expected}, got {status} \
             {response:?}",
            body.chars().count()
        );

        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
            let (status, got) = get(&emulator, HOME_REGION, &name).await;
            assert_eq!(
                status, 404,
                "case {case:?}: a refused create stores nothing, got {status} {got:?}"
            );
            continue;
        }

        let (status, got) = get(&emulator, HOME_REGION, &name).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected GetTelemetryPipeline to answer 200, got {status} {got:?}"
        );
        let reported = member(&got, "Configuration")
            .and_then(|configuration| configuration.get("Body"))
            .and_then(Value::as_str)
            .unwrap_or_default();
        assert_eq!(
            reported, body,
            "case {case:?}: the stored body must be reported back verbatim, got {reported:?}"
        );
    }
}

/// The modelled `Name` length (3..28), on each published bound and one past it.
#[tokio::test]
async fn a_name_outside_the_modelled_length_is_refused() {
    let emulator = emulator();
    let body = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";

    let rows: Vec<(&str, String, u16)> = vec![
        ("one character short of the 3-character minimum", "ab".to_string(), 400),
        ("exactly the 3-character minimum", "abc".to_string(), 200),
        ("exactly the 28-character maximum", "a".repeat(28), 200),
        ("one past the 28-character maximum", "b".repeat(29), 400),
    ];

    for (case, name, expected) in rows {
        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "Name": name, "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, expected,
            "case {case:?} (name of length {}): expected {expected}, got {status} {response:?}",
            name.len()
        );
        if expected == 400 {
            assert_eq!(
                error_code(&response),
                "ValidationException",
                "case {case:?}: expected ValidationException, got {response:?}"
            );
        }
    }
}

/// Both caps are refused before a record is allocated: the per-region budget, and
/// the store-wide one that dominates it because the region a record is keyed by comes
/// from the caller's own credential scope. The store is pre-filled directly through
/// the shared engine, so the test states the cap rather than paying for thousands of
/// requests to reach it.
#[tokio::test]
async fn a_create_past_either_cap_is_refused_and_stores_nothing() {
    let body = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";

    for (case, region_of, records) in [
        ("the per-region budget is full", HOME_REGION, MAX_TELEMETRY_PIPELINES),
        (
            "the store-wide cap is reached from other regions",
            OTHER_REGION,
            MAX_STORED_PIPELINE_RECORDS,
        ),
    ] {
        let emulator = emulator();
        {
            let state = emulator
                .engine
                .state::<ComAmazonawsObservabilityadminObservabilityadminState>();
            let mut store = state.lock();
            for index in 0..records {
                // Spread the store-wide rows over many regions, so the per-region
                // budget is NOT what refuses that case.
                let region = if region_of == HOME_REGION {
                    HOME_REGION.to_string()
                } else {
                    format!("{OTHER_REGION}-{}", index / 10)
                };
                let name = format!("filler-{index}");
                let id = format!("filler{index:026}");
                store.insert_pipeline(StoredTelemetryPipeline {
                    id: id.clone(),
                    name: name.clone(),
                    region: region.clone(),
                    arn: pipeline_arn(&region, &id),
                    configuration_body: body.to_string(),
                    created_timestamp_ms: 1_700_000_000_000,
                    last_update_timestamp_ms: 1_700_000_000_000,
                    active_at_ms: 1_700_000_000_000,
                    transition: Default::default(),
                    tags: Default::default(),
                    last_refresh_started_ms: None,
                    refresh_due_ms: 0,
                });
            }
        }

        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "Name": "one-too-many", "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, 402,
            "case {case:?}: expected 402, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ServiceQuotaExceededException",
            "case {case:?}: expected ServiceQuotaExceededException, got {response:?}"
        );
        assert_eq!(
            response.get("ResourceType").and_then(Value::as_str),
            Some("Pipeline"),
            "case {case:?}: the refusal names the resource type: {response:?}"
        );
        assert_eq!(
            response.get("ServiceCode").and_then(Value::as_str),
            Some("observabilityadmin"),
            "case {case:?}: the refusal names the service: {response:?}"
        );

        let (status, got) = get(&emulator, HOME_REGION, "one-too-many").await;
        assert_eq!(
            status, 404,
            "case {case:?}: a refused create allocates nothing, got {status} {got:?}"
        );
    }
}

#[tokio::test]
async fn a_name_identifies_one_pipeline_in_the_region_it_was_created_in() {
    let emulator = emulator();
    let first = "source:\n  http: {}\nsink:\n  - cloudwatch_logs: {}\n";
    let second = "source:\n  s3:\n    bucket: example\nsink:\n  - s3:\n      bucket: out\n";

    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({ "Name": "shared-name", "Configuration": { "Body": first } }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let home_arn = created_arn(&created);

    // A second create of the same name in the same region is a conflict, whatever it
    // carries, and leaves the stored configuration alone.
    for (case, body) in [
        ("the same configuration again", first),
        ("a different configuration under the same name", second),
    ] {
        let (status, response) = create(
            &emulator,
            HOME_REGION,
            &json!({ "Name": "shared-name", "Configuration": { "Body": body } }),
        )
        .await;
        assert_eq!(
            status, 409,
            "case {case:?}: expected 409, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ConflictException",
            "case {case:?}: expected ConflictException, got {response:?}"
        );
    }

    // The same name in another region is a different pipeline, with its own ARN and
    // its own configuration.
    let (status, elsewhere) = create(
        &emulator,
        OTHER_REGION,
        &json!({ "Name": "shared-name", "Configuration": { "Body": second } }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {elsewhere:?}");
    let other_arn = created_arn(&elsewhere);
    let other_id = minted_id("the other region's pipeline", OTHER_REGION, &other_arn);
    assert_eq!(
        other_arn,
        arn_of(OTHER_REGION, &other_id),
        "the other region's pipeline carries its own region in its ARN"
    );
    assert_ne!(
        home_arn, other_arn,
        "two regions' pipelines of one name are two resources"
    );

    for (region, arn, body) in [
        (HOME_REGION, home_arn.as_str(), first),
        (OTHER_REGION, other_arn.as_str(), second),
    ] {
        let (status, got) = get(&emulator, region, "shared-name").await;
        assert_eq!(status, 200, "expected 200 in {region}, got {status} {got:?}");
        assert_eq!(
            member(&got, "Arn").and_then(Value::as_str),
            Some(arn),
            "{region} reports its own pipeline: {got:?}"
        );
        assert_eq!(
            member(&got, "Configuration")
                .and_then(|configuration| configuration.get("Body"))
                .and_then(Value::as_str),
            Some(body),
            "{region} reports the configuration created there: {got:?}"
        );
    }

    // An ARN naming another region's pipeline resolves to nothing for this caller.
    let (status, got) = get(&emulator, HOME_REGION, &other_arn).await;
    assert_eq!(
        status, 404,
        "an ARN from {OTHER_REGION} names no pipeline in {HOME_REGION}, got {status} {got:?}"
    );
}

#[tokio::test]
async fn an_identifier_that_names_no_stored_pipeline_is_a_modelled_404() {
    let emulator = emulator();
    let (status, created) = create(
        &emulator,
        HOME_REGION,
        &json!({
            "Name": "stored-pipeline",
            "Configuration": { "Body": "source:\n  http: {}\nsink:\n  - s3: {}\n" }
        }),
    )
    .await;
    assert_eq!(status, 200, "expected 200, got {status} {created:?}");
    let stored_arn = created_arn(&created);

    let rows: Vec<(&str, String)> = vec![
        ("a name nothing was created under", "absent-pipeline".to_string()),
        (
            "a well-formed ARN for a pipeline that does not exist",
            arn_of(HOME_REGION, &"a".repeat(32)),
        ),
        (
            "the stored pipeline's ARN under another account",
            stored_arn.replace(ACCOUNT, "123456789012"),
        ),
        (
            "the stored pipeline's ARN under another service namespace",
            stored_arn.replace("observabilityadmin", "osis"),
        ),
    ];

    for (case, identifier) in rows {
        let (status, response) = get(&emulator, HOME_REGION, &identifier).await;
        assert_eq!(
            status, 404,
            "case {case:?}: expected 404, got {status} {response:?}"
        );
        assert_eq!(
            error_code(&response),
            "ResourceNotFoundException",
            "case {case:?}: expected ResourceNotFoundException, got {response:?}"
        );
    }
}
