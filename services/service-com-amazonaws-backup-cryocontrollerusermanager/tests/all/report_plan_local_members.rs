// SPDX-License-Identifier: CC0-1.0

//! The members a REPORT PLAN is reported with, and the local declaration the one AWS
//! carries beyond the published model snapshot needs.
//!
//! RULE: every site that reports a report plan reports it through ONE shape — the
//! members the snapshot declares for `ReportPlan`, under the names it declares them
//! with, plus the `ReportTiming` block AWS carries and the snapshot does not declare
//! (see `smithy-model/LOCAL-ADDITIONS.md`). So `DescribeReportPlan` and the entries of
//! `ListReportPlans` report the same body for one plan, that body carries no member
//! outside the set, and the timing it carries is a report frequency in hours and an hour
//! of the day.
//!
//! The rows differ along what the rule generalises over: whether the create states a
//! description, the report template the plan is built from, the delivery formats it
//! names (none, one, both), and how many frameworks its setting covers.
//!
//! The last test pins the LOCAL DECLARATION the extra member needs: the shape is
//! generated from the overlay model `smithy-model/local-additions.json` rather than from
//! the snapshot, so everything that overlay restates of the snapshot is compared against
//! it. A refreshed snapshot that declares the member itself, adds one the overlay does
//! not restate, or changes a target or a constraint fails there rather than silently
//! narrowing what this service reports.
//!
//! Every case drives real HTTP through the router; the harness is the shared one in
//! `vault_and_backup_job.rs`.

use serde_json::{json, Map, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::vault_and_backup_job::{call, router};

/// The shape the local declaration is of, and the member it adds to it.
const REPORT_PLAN_SHAPE: &str = "com.amazonaws.backup#ReportPlan";
const LOCAL_MEMBER: &str = "ReportTiming";

/// The shape the overlay adds for that member, which the snapshot declares nowhere.
const LOCAL_SHAPE: &str = "com.amazonaws.backup#ReportTiming";

/// The members of the added block, as the overlay declares them.
const TIMING_MEMBERS: [&str; 2] = ["ReportFrequencyHours", "ReportStartHour"];

/// The report templates a report may be built from, as the model lists them
/// (`smithy_query` shape `ReportSetting`, member `ReportTemplate`).
const REPORT_TEMPLATES: [&str; 6] = [
    "RESOURCE_COMPLIANCE_REPORT",
    "CONTROL_COMPLIANCE_REPORT",
    "BACKUP_JOB_REPORT",
    "COPY_JOB_REPORT",
    "RESTORE_JOB_REPORT",
    "SCAN_JOB_REPORT",
];

/// The `shapes` object of one model file of this crate, keyed by shape id.
fn shapes_of(path: &str) -> Map<String, Value> {
    let text = std::fs::read_to_string(format!("{}/{path}", env!("CARGO_MANIFEST_DIR")))
        .unwrap_or_else(|error| panic!("case {path}: the model file reads back: {error}"));
    let parsed: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {path}: the model file parses: {error}"));
    parsed["shapes"]
        .as_object()
        .unwrap_or_else(|| panic!("case {path}: a Smithy model declares shapes"))
        .clone()
}

/// The published snapshot this crate's wire shapes come from.
fn snapshot() -> Map<String, Value> {
    shapes_of("smithy-model/com-amazonaws-backup-cryocontrollerusermanager.json")
}

/// The overlay model the local `ReportPlan` declaration is generated from.
fn overlay() -> Map<String, Value> {
    shapes_of("smithy-model/local-additions.json")
}

/// `declaration` with every documentation trait dropped, recursively — and with a traits
/// object left empty by that dropped too, so a shape the snapshot documents and nothing
/// else compares equal to the same shape restated with no traits at all.
///
/// Documentation is prose: the overlay restates what codegen READS — targets, types and
/// constraint traits — so comparing declarations has to ignore the one trait that carries
/// no behaviour.
fn without_docs(declaration: &Value) -> Value {
    match declaration {
        Value::Object(members) => Value::Object(
            members
                .iter()
                .filter(|(key, _)| key.as_str() != "smithy.api#documentation")
                .map(|(key, value)| (key.clone(), without_docs(value)))
                .filter(|(key, value)| {
                    key.as_str() != "traits" || !value.as_object().is_some_and(Map::is_empty)
                })
                .collect(),
        ),
        Value::Array(items) => Value::Array(items.iter().map(without_docs).collect()),
        other => other.clone(),
    }
}

/// The members one shape of a model declares, keyed by member name.
fn members_of(shapes: &Map<String, Value>, shape_id: &str) -> Map<String, Value> {
    shapes[shape_id]["members"]
        .as_object()
        .unwrap_or_else(|| panic!("case {shape_id}: the shape declares members"))
        .clone()
}

/// The names a read of a report plan may report: the members the SNAPSHOT declares for
/// `ReportPlan`, read off the model rather than copied here, plus the one the overlay
/// adds.
fn reportable_members() -> Vec<String> {
    let mut names: Vec<String> = members_of(&snapshot(), REPORT_PLAN_SHAPE)
        .keys()
        .cloned()
        .collect();
    names.push(LOCAL_MEMBER.to_string());
    names
}

/// `POST /audit/report-plans`, as the model binds the create.
async fn create_plan(router: &MultiServiceRouter, body: &Value) -> (u16, Value) {
    call(router, "POST", "/audit/report-plans", Some(&body.to_string())).await
}

/// The `ReportPlan` body `DescribeReportPlan` answers with for `name`.
async fn described_plan(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "GET", &format!("/audit/report-plans/{name}"), None).await;
    assert_eq!(
        status, 200,
        "DescribeReportPlan {name}: expected 200, got {status} {body:?}"
    );
    body["ReportPlan"].clone()
}

/// The `ListReportPlans` entry for `name`.
async fn listed_plan(router: &MultiServiceRouter, name: &str) -> Value {
    let (status, body) = call(router, "GET", "/audit/report-plans", None).await;
    assert_eq!(
        status, 200,
        "ListReportPlans: expected 200, got {status} {body:?}"
    );
    body["ReportPlans"]
        .as_array()
        .cloned()
        .unwrap_or_default()
        .into_iter()
        .find(|entry| entry["ReportPlanName"] == json!(name))
        .unwrap_or_else(|| panic!("ListReportPlans never reported the plan {name}: {body:?}"))
}

/// One create request: the plan's name, the template its report is built from, the
/// delivery formats it names (`None` leaves the member off), the frameworks its setting
/// covers and its description when it states one.
fn plan_request(
    name: &str,
    template: &str,
    formats: Option<&[&str]>,
    frameworks: usize,
    description: Option<&str>,
) -> Value {
    let mut setting = json!({"ReportTemplate": template});
    if frameworks > 0 {
        let arns: Vec<String> = (0..frameworks)
            .map(|index| format!("arn:aws:backup:us-west-2:000000000000:framework:covered-{index}"))
            .collect();
        setting["FrameworkArns"] = json!(arns);
    }
    let mut channel = json!({"S3BucketName": "audit-reports"});
    if let Some(formats) = formats {
        channel["Formats"] = json!(formats);
    }
    let mut body = json!({
        "ReportPlanName": name,
        "ReportSetting": setting,
        "ReportDeliveryChannel": channel,
    });
    if let Some(description) = description {
        body["ReportPlanDescription"] = json!(description);
    }
    body
}

/// The create bodies every wire case below runs.
fn rows() -> Vec<(&'static str, Value)> {
    vec![
        (
            "no description, no formats, nothing covered",
            plan_request("plan_members_bare", REPORT_TEMPLATES[2], None, 0, None),
        ),
        (
            "a description, one delivery format",
            plan_request(
                "plan_members_described",
                REPORT_TEMPLATES[0],
                Some(&["CSV"]),
                1,
                Some("the compliance of every covered resource"),
            ),
        ),
        (
            "both delivery formats, two frameworks covered",
            plan_request(
                "plan_members_both_formats",
                REPORT_TEMPLATES[1],
                Some(&["CSV", "JSON"]),
                2,
                Some("control compliance"),
            ),
        ),
        (
            "a restore-job report delivering in JSON alone",
            plan_request(
                "plan_members_restore",
                REPORT_TEMPLATES[4],
                Some(&["JSON"]),
                0,
                None,
            ),
        ),
    ]
}

/// The timing block of a reported plan, asserted to be a block of the two members the
/// overlay declares, each an integer, the frequency a count of hours and the start an
/// hour of the day.
fn assert_timing(case: &str, site: &str, plan: &Value) {
    let timing = &plan[LOCAL_MEMBER];
    let reported = timing
        .as_object()
        .unwrap_or_else(|| panic!("case {case:?}: {site} must report {LOCAL_MEMBER}, got {plan:?}"));
    let mut names: Vec<&str> = reported.keys().map(String::as_str).collect();
    names.sort_unstable();
    let mut expected: Vec<&str> = TIMING_MEMBERS.to_vec();
    expected.sort_unstable();
    assert_eq!(
        names, expected,
        "case {case:?}: {site} must report {LOCAL_MEMBER} as the two declared members, got \
         {timing:?}"
    );

    let frequency = timing[TIMING_MEMBERS[0]].as_i64();
    assert!(
        frequency.is_some_and(|hours| hours >= 1),
        "case {case:?}: {site} must report a report frequency of at least one hour, got {:?}",
        timing[TIMING_MEMBERS[0]]
    );
    let start_hour = timing[TIMING_MEMBERS[1]].as_i64();
    assert!(
        start_hour.is_some_and(|hour| (0..24).contains(&hour)),
        "case {case:?}: {site} must report a start hour of the day, got {:?}",
        timing[TIMING_MEMBERS[1]]
    );
}

#[tokio::test]
async fn every_site_that_reports_a_report_plan_reports_the_modelled_members_and_its_timing() {
    let admitted = reportable_members();
    // The members a plan always carries: every read reports the plan's own identity, the
    // body it was stored with, where its deployment has got to, when it was stored and
    // when its report runs. The ones left out are those a plan may not have yet — the
    // description a create need not state, and the execution instants no report job of
    // the plan has reached.
    let always = [
        "ReportPlanArn",
        "ReportPlanName",
        "ReportSetting",
        "ReportDeliveryChannel",
        "DeploymentStatus",
        "CreationTime",
        LOCAL_MEMBER,
    ];

    for (case, body) in rows() {
        let router = router();
        let name = body["ReportPlanName"]
            .as_str()
            .expect("a case names its plan")
            .to_string();
        let (status, created) = create_plan(&router, &body).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected CreateReportPlan 200, got {status} {created:?}"
        );

        let described = described_plan(&router, &name).await;
        let listed = listed_plan(&router, &name).await;
        for (site, plan) in [("the DescribeReportPlan body", &described), ("the ListReportPlans entry", &listed)] {
            let reported = plan.as_object().unwrap_or_else(|| {
                panic!("case {case:?}: {site} must report a plan object, got {plan:?}")
            });
            let outside: Vec<&String> = reported
                .keys()
                .filter(|member| !admitted.contains(*member))
                .collect();
            assert!(
                outside.is_empty(),
                "case {case:?}: {site} may report only the members the model declares and the \
                 one the overlay adds, reported {outside:?} besides"
            );
            for member in always {
                assert!(
                    reported.get(member).is_some_and(|value| !value.is_null()),
                    "case {case:?}: {site} must report {member}, got {plan:?}"
                );
            }
            assert_eq!(
                reported.get("ReportPlanDescription").cloned(),
                body.get("ReportPlanDescription").cloned(),
                "case {case:?}: {site} must report the description the create stated, and none \
                 where it stated none, got {plan:?}"
            );
            assert_timing(case, site, plan);
        }

        // ONE projection of a stored plan: the two sites cannot disagree about it.
        assert_eq!(
            described, listed,
            "case {case:?}: the read and the listing must report one body for one plan, got \
             {described:?} and {listed:?}"
        );
    }
}

#[test]
fn the_overlay_restates_the_snapshots_report_plan_shape() {
    let snapshot = snapshot();
    let overlay = overlay();

    // Every member the snapshot declares is restated, at the SAME target: the overlay
    // generates the one `ReportPlan` type of this crate, so a member it dropped would
    // leave the wire a member short.
    let declared = members_of(&snapshot, REPORT_PLAN_SHAPE);
    let restated = members_of(&overlay, REPORT_PLAN_SHAPE);
    for (member, modelled) in &declared {
        let found = restated.get(member).unwrap_or_else(|| {
            panic!("case {member}: the overlay must restate every member the snapshot declares")
        });
        assert_eq!(
            without_docs(found),
            without_docs(modelled),
            "case {member}: expected the member restated as the snapshot declares it, got \
             {found:?}"
        );
    }

    // And it adds exactly the one member, so a snapshot that starts declaring it — or an
    // overlay that invents a second — fails here.
    let added: Vec<&String> = restated
        .keys()
        .filter(|member| !declared.contains_key(*member))
        .collect();
    assert_eq!(
        added,
        vec![LOCAL_MEMBER],
        "case the added member: expected exactly [{LOCAL_MEMBER}], got {added:?}"
    );

    // Every other shape the overlay carries either is the one it adds, which the snapshot
    // must declare nowhere, or is restated from the snapshot with the type and the
    // constraint traits the snapshot gives it.
    for (shape_id, found) in overlay.iter().filter(|(id, _)| *id != REPORT_PLAN_SHAPE) {
        if shape_id == LOCAL_SHAPE {
            assert!(
                snapshot.get(shape_id).is_none(),
                "the snapshot now declares {shape_id}, so the local declaration should give way \
                 to the modelled shape"
            );
            continue;
        }
        let modelled = snapshot.get(shape_id).unwrap_or_else(|| {
            panic!("case {shape_id}: the overlay may only restate shapes the snapshot declares")
        });
        assert_eq!(
            found["type"], modelled["type"],
            "case {shape_id}: expected the type {:?}, got {:?}",
            modelled["type"], found["type"]
        );
        // A structure the plan merely NESTS is restated by its type alone — its own
        // rendered type carries its member list — so only the shapes the overlay states
        // members for are compared on them.
        let traits = |declaration: &Value| match without_docs(&declaration["traits"]) {
            Value::Object(traits) if traits.is_empty() => Value::Null,
            other => other,
        };
        if found["type"] != json!("structure") {
            assert_eq!(
                traits(found),
                traits(modelled),
                "case {shape_id}: expected the traits the snapshot states for it, got {:?}",
                found["traits"]
            );
        }
    }

    // The shape the local declaration is of is restated, and the member it adds is still
    // one the snapshot does not declare.
    assert!(
        overlay.contains_key(REPORT_PLAN_SHAPE),
        "the overlay must restate {REPORT_PLAN_SHAPE}, the shape the local declaration is of"
    );
    assert!(
        !declared.contains_key(LOCAL_MEMBER),
        "the snapshot now declares {LOCAL_MEMBER} on {REPORT_PLAN_SHAPE}, so the local \
         declaration should give way to the modelled member"
    );
}
