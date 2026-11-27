// SPDX-License-Identifier: CC0-1.0

//! The RECORDS of one investigation — `ListInvestigationRecords` — end to end over the
//! wire (the harness lives in the `detector_lifecycle` suite).
//!
//! The rules under test, each asserted over a TABLE of inputs rather than over one
//! recorded call:
//!
//! 1. "Each record is one step in the reasoning that GuardDuty followed to reach the
//!    results of the investigation"
//!    (`smithy_query(query="doc",shape="ListInvestigationRecords")`), so the records are
//!    the steps the pass took over the evidence it read: it was asked, it fetched the
//!    findings, it noted each one, it assessed them, it recommended, it answered, and it
//!    reports how long that took. Each record names itself once and the list is in the
//!    order the steps were created, oldest first.
//! 2. "Records are returned only when the status is COMPLETED. If the status is RUNNING
//!    or FAILED, the response contains an empty list of records and no NextToken", and a
//!    `NextToken` handed to an investigation that is not COMPLETED is refused
//!    (the `Status` and `NextToken` members,
//!    `smithy_query(operation="ListInvestigationRecords")`).
//! 3. "The types of records to return … if you don't specify this parameter, GuardDuty
//!    returns records of all types" (the `RecordTypes` member): the response carries
//!    exactly the records of the asked-for types.
//! 4. The listing pages over one total order: every record is reported exactly once,
//!    whatever `MaxResults` the walk uses, and the last page carries no token.
//! 5. The records are the ones the PASS produced: a later read, and a read taken after
//!    the account's findings have changed, report the same steps with the same
//!    identifiers.

use std::collections::BTreeSet;
use std::time::Duration;

use serde_json::{json, Value};

use crate::detector_lifecycle::{
    all_finding_ids, call, error_code, investigation_detector, new_router, Router,
    ABSENT_DETECTOR_ID,
};
use service_com_amazonaws_guardduty_guarddutyapiservice::state::INVESTIGATION_RUNNING;

/// How long a poll may wait for the clock to end an analysis before the test gives up.
/// Generous on purpose: the loop ends on the CONDITION it looks for, never on a duration.
const ENDED_DEADLINE: Duration = Duration::from_secs(30);

/// How long the poll pauses between two reads.
const POLL_INTERVAL: Duration = Duration::from_millis(25);

/// The `RecordType` members (`smithy_query(shape="RecordType")`).
const RECORD_TYPES: [&str; 5] = ["UPDATE", "FETCH", "OBSERVATION", "ANALYSIS", "METADATA"];

/// The modelled maximum of `RecordId` (`len 1..64`, `smithy_query(shape="RecordId")`).
const RECORD_ID_MAX: usize = 64;

/// How many records a completed pass reports BESIDE one per finding it analysed: the
/// request it answered, the fetch it made, the assessment it derived, the
/// recommendations it made, its answer, and its timing.
const STEPS_BESIDE_THE_FINDINGS: usize = 6;

/// Create one sample finding per entry of `types`.
async fn create_samples(router: &Router, detector_id: &str, types: &[&str], case: &str) {
    if types.is_empty() {
        return;
    }
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/findings/create"),
        Some(json!({"findingTypes": types})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-sample-findings expected 200, got {status} {body:?}"
    );
}

/// Initiate one investigation with `prompt` and return its id.
async fn initiate(router: &Router, detector_id: &str, prompt: &str, case: &str) -> String {
    let (status, body) = call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation"),
        Some(json!({"triggerPrompt": prompt})),
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: create-investigation expected 200, got {status} {body:?}"
    );
    body["investigationId"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case:?}: expected an investigationId, got {body:?}"))
        .to_string()
}

/// One `ListInvestigationRecords` call with `body` passed verbatim.
async fn list_records_raw(router: &Router, detector_id: &str, body: Value) -> (u16, Value) {
    call(
        router,
        "POST",
        &format!("/detector/{detector_id}/investigation/records"),
        Some(body),
    )
    .await
}

/// `ListInvestigationRecords` for `id`, with `body` merged over the investigation id.
async fn list_records(
    router: &Router,
    detector_id: &str,
    id: &str,
    extra: Value,
    case: &str,
) -> Value {
    let mut body = json!({"investigationId": id});
    for (key, value) in extra.as_object().into_iter().flatten() {
        body[key] = value.clone();
    }
    let (status, got) = list_records_raw(router, detector_id, body.clone()).await;
    assert_eq!(
        status, 200,
        "case {case:?}: list-investigation-records {body} expected 200, got {status} {got:?}"
    );
    got
}

/// One `GetInvestigation` read.
async fn read(router: &Router, detector_id: &str, id: &str, case: &str) -> Value {
    let (status, body) = call(
        router,
        "GET",
        &format!("/detector/{detector_id}/investigation/{id}"),
        None,
    )
    .await;
    assert_eq!(
        status, 200,
        "case {case:?}: get-investigation expected 200, got {status} {body:?}"
    );
    body["investigation"].clone()
}

/// Read the investigation until its pass has ended, or until [`ENDED_DEADLINE`] passes.
async fn ended(router: &Router, detector_id: &str, id: &str, case: &str) -> Value {
    let deadline = std::time::Instant::now() + ENDED_DEADLINE;
    loop {
        let investigation = read(router, detector_id, id, case).await;
        if investigation["status"] != json!(INVESTIGATION_RUNNING) {
            return investigation;
        }
        assert!(
            std::time::Instant::now() < deadline,
            "case {case:?}: the analysis never ended within {ENDED_DEADLINE:?}, last read \
             {investigation}"
        );
        tokio::time::sleep(POLL_INTERVAL).await;
    }
}

/// The records of one response, as a list.
fn records(body: &Value, case: &str) -> Vec<Value> {
    body["records"]
        .as_array()
        .unwrap_or_else(|| panic!("case {case:?}: expected a records list, got {body:?}"))
        .clone()
}

/// The records of every type a record carries, in the order reported.
fn types_of(records: &[Value]) -> Vec<String> {
    records
        .iter()
        .map(|record| record["type"].as_str().unwrap_or_default().to_string())
        .collect()
}

/// The payload a record's `supportingData` carries for `member`.
///
/// The union member is matched case-insensitively on purpose: how a union member is
/// KEYED on the wire is the framework serializer's business, not this operation's, and
/// what this suite asserts is that the right payload accompanies the right step.
fn supporting(record: &Value, member: &str) -> Value {
    record["supportingData"]
        .as_object()
        .into_iter()
        .flatten()
        .find(|(key, _)| key.eq_ignore_ascii_case(member))
        .map(|(_, value)| value.clone())
        .unwrap_or(Value::Null)
}

/// The one record of `record_type` in `records`, failing when there is not exactly one.
fn only_record<'a>(records: &'a [Value], record_type: &str, case: &str) -> &'a Value {
    let found: Vec<&Value> = records
        .iter()
        .filter(|record| record["type"].as_str() == Some(record_type))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one {record_type} record, got {:?}",
        types_of(records)
    );
    found[0]
}

// ---------------------------------------------------------------------------
// Rule 1.
// ---------------------------------------------------------------------------

/// The reported steps are the ones the pass took over the evidence it read. The rows
/// differ along how much evidence that was — none, one finding, several over several
/// resource types — and along whether the prompt narrowed the pass to one finding.
#[tokio::test]
async fn the_records_are_the_steps_the_pass_took() {
    // (case, the sample finding types the detector holds, whether the prompt names one)
    let rows: [(&str, &[&str], bool); 4] = [
        ("a detector holding no findings", &[], false),
        ("a detector holding one finding", &["Recon:EC2/Portscan"], false),
        (
            "a detector holding three findings over three resource types",
            &[
                "AttackSequence:IAM/CompromisedCredentials",
                "Policy:S3/BucketAnonymousAccessGranted",
                "Impact:EC2/PortSweep",
            ],
            false,
        ),
        (
            "a prompt naming one of three findings",
            &[
                "AttackSequence:IAM/CompromisedCredentials",
                "Policy:S3/BucketAnonymousAccessGranted",
                "Impact:EC2/PortSweep",
            ],
            true,
        ),
    ];

    for (case, finding_types, name_a_finding) in rows {
        let router = new_router();
        let detector_id = investigation_detector(&router).await;
        create_samples(&router, &detector_id, finding_types, case).await;
        let held = all_finding_ids(&router, &detector_id, 50).await;
        let (prompt, analysed) = match name_a_finding {
            true => (
                format!("Investigate the finding {}", held[0]),
                vec![held[0].clone()],
            ),
            false => (
                "Investigate the threat posture here".to_string(),
                held.clone(),
            ),
        };

        let id = initiate(&router, &detector_id, &prompt, case).await;
        let investigation = ended(&router, &detector_id, &id, case).await;
        let body = list_records(&router, &detector_id, &id, json!({}), case).await;

        assert_eq!(
            body["investigationId"],
            json!(id),
            "case {case:?}: expected the investigation the request named, got {body:?}"
        );
        assert_eq!(
            body["status"], investigation["status"],
            "case {case:?}: the records and the investigation report one status, got {body:?}"
        );
        let reported = records(&body, case);
        assert_eq!(
            reported.len(),
            analysed.len() + STEPS_BESIDE_THE_FINDINGS,
            "case {case:?}: expected one step per analysed finding plus \
             {STEPS_BESIDE_THE_FINDINGS}, got {:?}",
            types_of(&reported)
        );

        // Every record names itself once, carries a type of the enum and a title, and
        // comes no earlier than the step before it.
        let mut seen: BTreeSet<String> = BTreeSet::new();
        let mut previous = 0.0_f64;
        for record in &reported {
            let record_id = record["recordId"]
                .as_str()
                .unwrap_or_else(|| panic!("case {case:?}: expected a recordId, got {record:?}"));
            assert!(
                !record_id.is_empty() && record_id.len() <= RECORD_ID_MAX,
                "case {case:?}: a recordId is 1..{RECORD_ID_MAX} characters, got {record_id:?}"
            );
            assert!(
                seen.insert(record_id.to_string()),
                "case {case:?}: {record_id:?} names two records, got {reported:?}"
            );
            let record_type = record["type"].as_str().unwrap_or_default();
            assert!(
                RECORD_TYPES.contains(&record_type),
                "case {case:?}: {record_type:?} is not a RecordType, got {record:?}"
            );
            assert!(
                record["title"].as_str().is_some_and(|title| !title.is_empty()),
                "case {case:?}: every step carries a title, got {record:?}"
            );
            let created_at = record["createdAt"]
                .as_f64()
                .unwrap_or_else(|| panic!("case {case:?}: expected a createdAt, got {record:?}"));
            assert!(
                created_at >= previous,
                "case {case:?}: the records are oldest first, so {created_at} cannot precede \
                 {previous}, got {reported:?}"
            );
            previous = created_at;
        }
        assert_eq!(
            types_of(&reported).first().map(String::as_str),
            Some("UPDATE"),
            "case {case:?}: the first step is the request that started the investigation, got {:?}",
            types_of(&reported)
        );

        // What the pass FETCHED: the findings it analysed, by their own ids.
        let fetch = only_record(&reported, "FETCH", case);
        let fetched = supporting(fetch, "fetch");
        assert_eq!(
            fetched["fetchType"],
            json!("FINDINGS"),
            "case {case:?}: the pass retrieves GuardDuty findings, got {fetch:?}"
        );
        assert_eq!(
            fetched["result"]["totalCount"],
            json!(analysed.len()),
            "case {case:?}: expected the analysed findings to be counted, got {fetch:?}"
        );
        let item_ids: BTreeSet<String> = fetched["result"]["itemIds"]
            .as_array()
            .into_iter()
            .flatten()
            .map(|value| value.as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            item_ids,
            analysed.iter().cloned().collect::<BTreeSet<String>>(),
            "case {case:?}: expected the ids of the findings the pass read, got {fetch:?}"
        );

        // One OBSERVATION per analysed finding, each naming a finding the pass read.
        let observed: Vec<&Value> = reported
            .iter()
            .filter(|record| {
                supporting(record, "observation")["observationType"] == json!("FINDING_DETAIL")
            })
            .collect();
        assert_eq!(
            observed.len(),
            analysed.len(),
            "case {case:?}: expected one noted finding per analysed finding, got {reported:?}"
        );
        for record in observed {
            let detail = supporting(record, "observation")["findingDetail"].clone();
            let finding_id = detail["findingId"].as_str().unwrap_or_default().to_string();
            assert!(
                analysed.contains(&finding_id),
                "case {case:?}: {finding_id:?} is not a finding the pass read, got {detail:?}"
            );
            assert!(
                !detail["findingType"].is_null() && !detail["severity"].is_null(),
                "case {case:?}: a noted finding carries its type and severity, got {detail:?}"
            );
        }

        // The ASSESSMENT it derived, which is the one the investigation reports.
        let analysis = only_record(&reported, "ANALYSIS", case);
        let assessment = supporting(analysis, "analysis");
        assert_eq!(
            assessment["analysisType"],
            json!("THREAT_ASSESSMENT"),
            "case {case:?}: expected the threat assessment, got {analysis:?}"
        );
        assert_eq!(
            assessment["threatAssessment"]["assessment"], investigation["risk"],
            "case {case:?}: the record and the investigation report one assessment, got \
             {assessment:?} and {investigation:?}"
        );

        // And how long it took, which is the step the pass ends with.
        let metadata = only_record(&reported, "METADATA", case);
        let took = supporting(metadata, "metadata")["totalTimeMilliseconds"]
            .as_i64()
            .unwrap_or_else(|| {
                panic!("case {case:?}: expected the time the pass took, got {metadata:?}")
            });
        assert!(
            took >= 0,
            "case {case:?}: expected a duration, got {took} in {metadata:?}"
        );
        assert_eq!(
            types_of(&reported).last().map(String::as_str),
            Some("METADATA"),
            "case {case:?}: the timing is the last step, got {:?}",
            types_of(&reported)
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 2.
// ---------------------------------------------------------------------------

/// An investigation whose pass has not ended has reasoned nothing, so it reports no
/// step and no position in a list of them — and a request that names a position is
/// refused.
#[tokio::test]
async fn a_running_investigation_reports_no_records() {
    let case = "an investigation read straight after it was initiated";
    let router = new_router();
    let detector_id = investigation_detector(&router).await;
    create_samples(&router, &detector_id, &["Recon:EC2/Portscan"], case).await;
    let id = initiate(&router, &detector_id, "Investigate this account", case).await;

    let body = list_records(&router, &detector_id, &id, json!({}), case).await;
    assert_eq!(
        body["status"],
        json!(INVESTIGATION_RUNNING),
        "case {case:?}: expected the transient status, got {body:?}"
    );
    assert_eq!(
        body["records"],
        json!([]),
        "case {case:?}: a pass that has not ended reports no step, got {body:?}"
    );
    assert!(
        body["nextToken"].is_null(),
        "case {case:?}: an empty list has no next page, got {body:?}"
    );

    // A position in a list the investigation does not have yet is refused.
    let (status, refused) = list_records_raw(
        &router,
        &detector_id,
        json!({"investigationId": id, "nextToken": "0123456789abcdef00000000"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a nextToken for a running investigation is refused, got {status} \
         {refused:?}"
    );
    assert!(
        refused["message"].as_str().is_some_and(|m| !m.is_empty()),
        "case {case:?}: expected a refusal message, got {refused:?}"
    );

    // Once the pass has ended, the same investigation reports its steps.
    ended(&router, &detector_id, &id, case).await;
    let body = list_records(&router, &detector_id, &id, json!({}), case).await;
    assert!(
        !records(&body, case).is_empty(),
        "case {case:?}: an ended pass reports its steps, got {body:?}"
    );
}

/// An investigation this detector does not hold is the sub-resource refusal, and a
/// detector the store does not hold is the detector family's own.
#[tokio::test]
async fn an_absent_investigation_and_an_absent_detector_are_refused_apart() {
    let case = "the absences a records request can name";
    let router = new_router();
    let detector_id = investigation_detector(&router).await;

    let (status, refused) = list_records_raw(
        &router,
        &detector_id,
        json!({"investigationId": "0123456789abcdef0123456789abcdef"}),
    )
    .await;
    assert_eq!(
        status, 404,
        "case {case:?}: an investigation no create minted is not found, got {status} {refused:?}"
    );
    assert_eq!(
        error_code(&refused),
        "ResourceNotFoundException",
        "case {case:?}: expected the modelled not-found class, got {refused:?}"
    );

    let (status, refused) = list_records_raw(
        &router,
        ABSENT_DETECTOR_ID,
        json!({"investigationId": "0123456789abcdef0123456789abcdef"}),
    )
    .await;
    assert_eq!(
        status, 400,
        "case {case:?}: a detector this account does not hold is the detector refusal, got \
         {status} {refused:?}"
    );
}

// ---------------------------------------------------------------------------
// Rules 3 and 4.
// ---------------------------------------------------------------------------

/// `RecordTypes` selects the steps of the asked-for types, and the walk pages over the
/// selection exactly once however small the page is.
#[tokio::test]
async fn the_records_can_be_selected_by_type_and_paged() {
    let case = "a pass over three findings";
    let router = new_router();
    let detector_id = investigation_detector(&router).await;
    create_samples(
        &router,
        &detector_id,
        &[
            "AttackSequence:IAM/CompromisedCredentials",
            "Policy:S3/BucketAnonymousAccessGranted",
            "Impact:EC2/PortSweep",
        ],
        case,
    )
    .await;
    let id = initiate(&router, &detector_id, "Investigate this account", case).await;
    ended(&router, &detector_id, &id, case).await;

    let all = records(
        &list_records(&router, &detector_id, &id, json!({}), case).await,
        case,
    );
    let all_types = types_of(&all);

    // Rule 3: one row per member of the enum, plus a row asking for two of them.
    let mut asks: Vec<Vec<&str>> = RECORD_TYPES.iter().map(|asked| vec![*asked]).collect();
    asks.push(vec!["FETCH", "METADATA"]);
    for asked in &asks {
        let row = format!("{case}, recordTypes {asked:?}");
        let body = list_records(
            &router,
            &detector_id,
            &id,
            json!({"recordTypes": asked, "maxResults": 50}),
            &row,
        )
        .await;
        let selected = types_of(&records(&body, &row));
        let expected: Vec<String> = all_types
            .iter()
            .filter(|reported| asked.contains(&reported.as_str()))
            .cloned()
            .collect();
        assert_eq!(
            selected, expected,
            "case {row:?}: expected the steps of the asked-for types, in the order the pass took \
             them, got {body:?}"
        );
    }

    // Rule 4: every record exactly once, in one order, whatever the page size.
    for page in [1_usize, 2, 50] {
        let row = format!("{case}, maxResults {page}");
        let mut walked: Vec<String> = Vec::new();
        let mut token: Option<String> = None;
        for _ in 0..all.len() + 2 {
            let mut extra = json!({"maxResults": page});
            if let Some(token) = token.as_deref() {
                extra["nextToken"] = json!(token);
            }
            let body = list_records(&router, &detector_id, &id, extra, &row).await;
            let reported = records(&body, &row);
            assert!(
                reported.len() <= page,
                "case {row:?}: a page carries at most the maximum asked for, got {body:?}"
            );
            walked.extend(
                reported
                    .iter()
                    .map(|record| record["recordId"].as_str().unwrap_or_default().to_string()),
            );
            token = body["nextToken"].as_str().map(str::to_string);
            if token.is_none() {
                break;
            }
        }
        assert!(
            token.is_none(),
            "case {row:?}: the walk must end with no token, got {token:?}"
        );
        let expected: Vec<String> = all
            .iter()
            .map(|record| record["recordId"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            walked, expected,
            "case {row:?}: expected every step exactly once, in one order"
        );
    }
}

// ---------------------------------------------------------------------------
// Rule 5.
// ---------------------------------------------------------------------------

/// The steps are the ones the pass produced: reading them again, and reading them after
/// the account's findings have changed, reports the same records.
#[tokio::test]
async fn the_records_are_the_ones_the_pass_produced() {
    let case = "records read again after the account's findings changed";
    let router = new_router();
    let detector_id = investigation_detector(&router).await;
    create_samples(&router, &detector_id, &["Recon:EC2/Portscan"], case).await;
    let id = initiate(&router, &detector_id, "Investigate this account", case).await;
    ended(&router, &detector_id, &id, case).await;

    let served = list_records(&router, &detector_id, &id, json!({}), case).await;
    create_samples(&router, &detector_id, &["Backdoor:EC2/C&CActivity.B"], case).await;
    for round in 0..2 {
        let again = list_records(&router, &detector_id, &id, json!({}), case).await;
        assert_eq!(
            again, served,
            "case {case:?} (read {round}): expected the steps the pass produced, got {again:?}"
        );
    }
}
