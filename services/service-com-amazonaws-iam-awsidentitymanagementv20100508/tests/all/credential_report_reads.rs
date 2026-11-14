// SPDX-License-Identifier: CC0-1.0

//! `GetCredentialReport`, end to end over the awsQuery wire.
//!
//! Every test drives a `MultiServiceRouter` holding this service over one shared
//! `InMemoryEngine`, exactly as the production router does, so what is asserted is the HTTP
//! behaviour (status, awsQuery error code, XML members, the `text/csv` artifact a client
//! decodes) a client on the socket sees. The awsQuery harness is the one
//! `tests/all/user_lifecycle.rs` owns and the other suites in this crate share, and the
//! `GenerateCredentialReport` call and the clock rewind are the ones
//! `tests/all/credential_report_generation.rs` owns.
//!
//! The rules being asserted:
//!
//! * the action is ROUTED — a bound operation of this service answers it rather than the
//!   dispatcher's `InvalidAction`;
//! * what a read answers is the state of the account's ONE credential report, read off the
//!   generation record and the clock: no generation on record is refused `ReportNotPresent`
//!   (410), a generation not yet due `ReportInProgress` (404), a report inside its validity
//!   window is SERVED as `text/csv` with the instant it was generated, and a report past
//!   that window is refused `ReportExpired` (410);
//! * a report is a SNAPSHOT: it is rendered once, so two reads answer the same bytes and
//!   the same `GeneratedTime` whatever happens to the account in between, and a fresh
//!   generation is what produces a new one;
//! * the artifact is rendered from the account's OWN credentials — the published column
//!   list, the account's root user, and one row per IAM user reporting that user's console
//!   password, MFA devices, access keys and signing certificates;
//! * a caller whose identity-based policies do not allow the action is refused.
//!
//! Each suite is a TABLE whose rows differ along what those rules generalise over: where
//! the clock stands relative to the generation's due instant and to the report's validity
//! window — including the instant on each side of both — and which credentials a user
//! holds.

use aws_smithy_types::DateTime;
use aws_smithy_types::date_time::Format;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::ops::types::REPORT_FORMAT_TYPE_VALUES;
use service_com_amazonaws_iam_awsidentitymanagementv20100508::state;
use smithy_server_runtime::MultiServiceRouter;

use crate::credential_report_generation::{generate, rewind};
use crate::identity_policy_enforcement::{principal, statement};
use crate::signing_certificates::certificate_pem;
use crate::user_lifecycle::{
    HARNESS_REGION, account_id, assert_access_denied_message, call, call_signed_by, element,
    elements, error_code, router_with_engine,
};

/// The IAM user the account starts out holding: the one this emulator's own long-term
/// credential belongs to, which every report of a fresh account carries a row for.
const AMBIENT_USER: &str = "emulator";

/// The header row a credential report carries, written out as the text a client reads.
///
/// Source: the report-content column list in "Getting credential reports for your Amazon
/// Web Services account" (IAM User Guide), in the published order. Stated here as a
/// literal, independently of the constant the implementation renders from, so this suite
/// pins the columns rather than re-reading the code's own list back.
const PUBLISHED_HEADER: &str = "user,arn,user_creation_time,password_enabled,\
                               password_last_used,password_last_changed,\
                               password_next_rotation,mfa_active,access_key_1_active,\
                               access_key_1_last_rotated,access_key_1_last_used_date,\
                               access_key_1_last_used_region,access_key_1_last_used_service,\
                               access_key_2_active,access_key_2_last_rotated,\
                               access_key_2_last_used_date,access_key_2_last_used_region,\
                               access_key_2_last_used_service,cert_1_active,\
                               cert_1_last_rotated,cert_2_active,cert_2_last_rotated";

/// How far short of the report's validity window the inside-the-window case stands.
///
/// Generous on purpose: the case is read over a real HTTP call while the host's clock runs,
/// so a margin of a second or two would make the outcome depend on how fast the machine is.
/// A minute is far inside the window the service publishes
/// (`state::CREDENTIAL_REPORT_VALIDITY_SECONDS`) and still one rewind away from its end.
const WINDOW_MARGIN_SECONDS: i64 = 60;

/// One `GetCredentialReport` call, presented as signed by `akid` when a row names one. The
/// operation takes no input, so the call carries no form parameter beyond the action.
async fn get(router: &MultiServiceRouter, akid: Option<&str>) -> (u16, String) {
    call_signed_by(router, akid, "GetCredentialReport", &[]).await
}

/// The report a served read carried: the format it named, the instant it reported, and the
/// decoded `text/csv` artifact.
#[derive(Debug, PartialEq, Eq)]
struct Served {
    format: String,
    generated_time: String,
    csv: String,
}

/// Assert the read was SERVED and answer what it carried: the one `ReportFormat` the model
/// declares, a `GeneratedTime`, and a `Content` blob that decodes to the report.
fn served(case: &str, status: u16, body: &str) -> Served {
    assert_eq!(
        status, 200,
        "case {case:?}: expected the report to be served, got {status}: {body}"
    );
    let format = element(body, "ReportFormat");
    assert!(
        REPORT_FORMAT_TYPE_VALUES.contains(&format.as_str()),
        "case {case:?}: expected a format ReportFormatType declares, got {format:?}: {body}"
    );
    let generated_time = element(body, "GeneratedTime");
    assert!(
        DateTime::from_str(&generated_time, Format::DateTime).is_ok(),
        "case {case:?}: expected a modelled timestamp, got {generated_time:?}: {body}"
    );
    let content = element(body, "Content");
    let bytes = ::aws_smithy_types::base64::decode(&content)
        .unwrap_or_else(|error| panic!("case {case:?}: Content is not base64: {error}: {body}"));
    let csv = String::from_utf8(bytes)
        .unwrap_or_else(|error| panic!("case {case:?}: the report is not UTF-8: {error}"));
    Served {
        format,
        generated_time,
        csv,
    }
}

/// Assert the read was REFUSED with `expected_status` and `expected_code`, and that the
/// refusal carries no message — the wording the one measured refusal of this family has.
fn refused(case: &str, status: u16, body: &str, expected_status: u16, expected_code: &str) {
    assert_eq!(
        status, expected_status,
        "case {case:?}: expected {expected_status}, got {status}: {body}"
    );
    assert_eq!(
        error_code(body),
        expected_code,
        "case {case:?}: expected {expected_code}, got {:?}: {body}",
        error_code(body)
    );
    let messages = elements(body, "Message");
    assert!(
        messages.iter().all(String::is_empty),
        "case {case:?}: expected the refusal to carry no message, got {messages:?}: {body}"
    );
}

/// The rows of a report, header first, read back with the same codec that wrote them — so a
/// cell that had to be quoted is asserted as the value a client reads rather than as the
/// bytes around it.
fn rows(case: &str, csv: &str) -> Vec<Vec<String>> {
    let mut reader = ::csv::ReaderBuilder::new()
        .has_headers(false)
        .from_reader(csv.as_bytes());
    reader
        .records()
        .map(|record| {
            record
                .unwrap_or_else(|error| panic!("case {case:?}: the report is not CSV: {error}"))
                .iter()
                .map(ToString::to_string)
                .collect()
        })
        .collect()
}

/// The row reporting `user`, and the assertion that the report carries exactly one.
fn row_for<'a>(case: &str, rows: &'a [Vec<String>], user: &str) -> &'a Vec<String> {
    let found: Vec<&Vec<String>> = rows
        .iter()
        .skip(1)
        .filter(|row| row.first().map(String::as_str) == Some(user))
        .collect();
    assert_eq!(
        found.len(),
        1,
        "case {case:?}: expected exactly one row for {user:?}, got {found:?}"
    );
    found[0]
}

/// The cell `column` carries in `row`, where `column` is one of the published columns.
fn cell<'a>(case: &str, row: &'a [String], column: &str) -> &'a str {
    let at = state::CREDENTIAL_REPORT_COLUMNS
        .iter()
        .position(|published| *published == column)
        .unwrap_or_else(|| panic!("case {case:?}: {column:?} is not a published column"));
    row.get(at)
        .unwrap_or_else(|| panic!("case {case:?}: no {column:?} cell in {row:?}"))
        .as_str()
}

/// Assert `value` is an instant as a report spells one: whole seconds with the numeric zero
/// UTC offset.
fn assert_instant(case: &str, column: &str, value: &str) {
    let Some(civil) = value.strip_suffix("+00:00") else {
        panic!("case {case:?}: expected {column} to carry an ISO 8601 UTC instant, got {value:?}")
    };
    assert!(
        DateTime::from_str(&format!("{civil}Z"), Format::DateTime).is_ok(),
        "case {case:?}: expected {column} to carry a readable instant, got {value:?}"
    );
}

/// Bring the account's report to the instant it becomes readable: start a generation and
/// stand the clock at its due instant.
async fn generate_and_wait(router: &MultiServiceRouter, engine: &engine::InMemoryEngine) {
    let (status, body) = generate(router, None).await;
    assert_eq!(
        status, 200,
        "the fixture GenerateCredentialReport must succeed, got {status}: {body}"
    );
    rewind(engine, state::CREDENTIAL_REPORT_GENERATION_SECONDS);
}

/// RULE: the action is a bound operation of this service, so a call reaches a handler and is
/// answered with one of the conditions the operation models rather than with the
/// dispatcher's `InvalidAction` for an action the service does not have — and the condition
/// a read of an account that never generated a report meets is that it has none.
#[tokio::test]
async fn the_action_is_routed_rather_than_answered_as_an_unknown_action() {
    let (router, _engine) = router_with_engine();
    let (status, body) = get(&router, None).await;
    assert!(
        !body.contains("InvalidAction"),
        "expected the action to be routed rather than answered as unknown, got {body}"
    );
    refused(
        "an account that has never generated a report",
        status,
        &body,
        410,
        "ReportNotPresent",
    );
}

/// RULE: what a read answers is the state of the account's one credential report, read off
/// the generation record and the clock — so the rows are where the clock stands: before the
/// generation is due, at it, inside the report's validity window, and on each side of that
/// window's end. Every one of the three modelled refusals is reached, and the served legs
/// in between carry the artifact.
#[tokio::test]
async fn the_clock_and_the_held_generation_are_what_a_read_answers() {
    let (router, engine) = router_with_engine();

    // No generation on record: there is no report to read.
    let (status, body) = get(&router, None).await;
    refused("no generation on record", status, &body, 410, "ReportNotPresent");

    // A generation that is not due yet is still being made, and says so however many times
    // it is asked — the span is stored rather than slept for, so a caller that reads
    // straight after starting the generation sees it in flight.
    let (status, body) = generate(&router, None).await;
    assert_eq!(status, 200, "the generation must start, got {status}: {body}");
    for attempt in 0..2 {
        let case = format!("a generation still running, attempt {attempt}");
        let (status, body) = get(&router, None).await;
        refused(&case, status, &body, 404, "ReportInProgress");
    }

    // One second short of due: still being made. The span is the emulator's own
    // (`state::CREDENTIAL_REPORT_GENERATION_SECONDS`), so the row is written against it
    // rather than against a literal.
    rewind(&engine, state::CREDENTIAL_REPORT_GENERATION_SECONDS - 1);
    let (status, body) = get(&router, None).await;
    refused("one second short of due", status, &body, 404, "ReportInProgress");

    // Due: the report is served.
    rewind(&engine, 1);
    let (status, body) = get(&router, None).await;
    let first = served("the generation has become due", status, &body);

    // A report is a SNAPSHOT: a second read answers the same bytes and the same instant,
    // and so does a read taken after the account's users have changed — the report is not
    // regenerated by being read.
    let (status, body) = get(&router, None).await;
    assert_eq!(
        served("an immediate second read", status, &body),
        first,
        "two reads of one report must answer the same report"
    );
    let (status, body) = call(&router, "CreateUser", &[("UserName", "added-later")]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");
    let (status, body) = get(&router, None).await;
    assert_eq!(
        served("a read after the account changed", status, &body),
        first,
        "a read must answer the report that was generated, not a fresh one"
    );

    // Inside the validity window, a margin short of its end: still served, and still the
    // same report. The ARTIFACT is what is compared here: this suite stands the clock by
    // moving the generation record into the past, so the instant the record names moves with
    // it, while a real clock advancing leaves it where it was.
    //
    // The margin is generous rather than one second ([`WINDOW_MARGIN_SECONDS`]): the read
    // below is a real HTTP call and the host's own clock keeps running while it is served, so
    // a case standing one second short of the end answers differently on a loaded or slower
    // machine. The window's far side is still measured exactly, by the rewind that follows.
    rewind(
        &engine,
        state::CREDENTIAL_REPORT_VALIDITY_SECONDS
            - state::CREDENTIAL_REPORT_GENERATION_SECONDS
            - WINDOW_MARGIN_SECONDS,
    );
    let (status, body) = get(&router, None).await;
    assert_eq!(
        served("inside the window, short of its end", status, &body).csv,
        first.csv,
        "a report inside its window must still be the one that was generated"
    );

    // From the window's end: the report has expired, and a fresh generation is what the
    // account needs. Time only ever moves the record further past the end, so this side of
    // the boundary is exact however long the call takes.
    rewind(&engine, WINDOW_MARGIN_SECONDS);
    let (status, body) = get(&router, None).await;
    refused("the report has expired", status, &body, 410, "ReportExpired");

    let (status, body) = generate(&router, None).await;
    assert_eq!(
        status, 200,
        "an expired report must be regenerable, got {status}: {body}"
    );
    let (status, body) = get(&router, None).await;
    refused("the fresh generation is running", status, &body, 404, "ReportInProgress");
    rewind(&engine, state::CREDENTIAL_REPORT_GENERATION_SECONDS);
    let (status, body) = get(&router, None).await;
    let second = served("the fresh report", status, &body);
    assert_ne!(
        second.csv, first.csv,
        "a fresh generation must render the account as it now stands"
    );
    assert!(
        second.csv.contains("added-later"),
        "the fresh report must carry the user added since the first one, got {:?}",
        second.csv
    );
}

/// What a cell of a row is expected to carry.
#[derive(Debug)]
enum Expect {
    /// Exactly this text — one of the published literals, or a value built from the
    /// account.
    Literal(String),
    /// An instant, whatever the clock made it.
    Instant,
}

/// One row of the content table: the user it is about, and what some of its cells carry.
struct Row {
    /// The `user` cell, which is the user's friendly name.
    user: &'static str,
    /// The cells this row is about: the published column name and what it must carry.
    cells: Vec<(&'static str, Expect)>,
}

/// `value` as an expected literal cell.
fn literal(value: &str) -> Expect {
    Expect::Literal(value.to_string())
}

/// RULE: the artifact is rendered from the account's OWN credentials — the published column
/// list, the account's root user, and one row per IAM user reporting what the account holds
/// for that user — so the rows below differ along which credentials a user holds: none, a
/// console password, one or two access keys (one of them deactivated), a key that has been
/// used, an MFA device of either kind, a signing certificate, and a name that has to be
/// quoted to survive the format.
#[tokio::test]
async fn the_report_is_rendered_from_the_accounts_own_credentials() {
    let (router, engine) = router_with_engine();

    // A user holding nothing at all.
    let plain = "plain-user";
    let (status, body) = call(&router, "CreateUser", &[("UserName", plain)]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");

    // A user holding a console password.
    let with_password = "password-user";
    let (status, body) = call(&router, "CreateUser", &[("UserName", with_password)]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");
    let (status, body) = call(
        &router,
        "CreateLoginProfile",
        &[("UserName", with_password), ("Password", "Sup3r-secret!")],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture CreateLoginProfile must succeed, got {status}: {body}"
    );

    // A user holding ONE access key, which has signed a call the account recorded.
    let with_used_key = "used-key-user";
    let signing_key = principal(
        &router,
        "a user whose key signs a call",
        with_used_key,
        &[("list".to_string(), statement("Allow", "iam:ListUsers", "*"))],
    )
    .await;
    let (status, body) = call_signed_by(&router, Some(&signing_key), "ListUsers", &[]).await;
    assert_eq!(
        status, 200,
        "the fixture signed call must succeed, got {status}: {body}"
    );

    // A user holding one DEACTIVATED access key, so the slot is reported inactive while it
    // still dates the key.
    let with_inactive_key = "inactive-key-user";
    let (status, body) = call(&router, "CreateUser", &[("UserName", with_inactive_key)]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");
    let (status, body) = call(&router, "CreateAccessKey", &[("UserName", with_inactive_key)]).await;
    assert_eq!(
        status, 200,
        "the fixture CreateAccessKey must succeed, got {status}: {body}"
    );
    let inactive_key = element(&body, "AccessKeyId");
    let (status, body) = call(
        &router,
        "UpdateAccessKey",
        &[
            ("UserName", with_inactive_key),
            ("AccessKeyId", inactive_key.as_str()),
            ("Status", "Inactive"),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture UpdateAccessKey must succeed, got {status}: {body}"
    );

    // A user holding BOTH access keys the quota admits, so both slots are filled.
    let with_two_keys = "two-key-user";
    let (status, body) = call(&router, "CreateUser", &[("UserName", with_two_keys)]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");
    for _ in 0..state::MAX_ACCESS_KEYS_PER_USER {
        let (status, body) = call(&router, "CreateAccessKey", &[("UserName", with_two_keys)]).await;
        assert_eq!(
            status, 200,
            "the fixture CreateAccessKey must succeed, got {status}: {body}"
        );
    }

    // A user holding a virtual MFA device, and one holding a FIDO security key. The security
    // key is enrolled by no operation at all, so both devices are written into the store the
    // handler reads, as `tests/all/mfa_device_reads.rs` writes one — what a report reads is the
    // device a user holds, whichever call put it there.
    let with_virtual_mfa = "virtual-mfa-user";
    let with_fido = "fido-user";
    for user_name in [with_virtual_mfa, with_fido] {
        let (status, body) = call(&router, "CreateUser", &[("UserName", user_name)]).await;
        assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");
    }
    enable_virtual_mfa_device(&engine, with_virtual_mfa);
    enroll_fido_security_key(&engine, with_fido);

    // A user holding an X.509 signing certificate.
    let with_certificate = "certificate-user";
    let (status, body) = call(&router, "CreateUser", &[("UserName", with_certificate)]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");
    let pem = certificate_pem(
        &rcgen::PKCS_ECDSA_P256_SHA256,
        (2026, 2027),
        "credential-report",
    );
    let (status, body) = call(
        &router,
        "UploadSigningCertificate",
        &[
            ("UserName", with_certificate),
            ("CertificateBody", pem.as_str()),
        ],
    )
    .await;
    assert_eq!(
        status, 200,
        "the fixture UploadSigningCertificate must succeed, got {status}: {body}"
    );

    // A name that carries the comma `userNameType`'s own alphabet (`^[\w+=,.@-]+$`) admits,
    // which the format has to quote.
    let comma_name = "quoted,name";
    let (status, body) = call(&router, "CreateUser", &[("UserName", comma_name)]).await;
    assert_eq!(status, 200, "the fixture CreateUser must succeed, got {status}: {body}");

    generate_and_wait(&router, &engine).await;
    let (status, body) = get(&router, None).await;
    let report = served("the account's credentials", status, &body);
    let rows = rows("the account's credentials", &report.csv);

    // The header is the published column list, written out here as a literal rather than
    // read off the implementation's own constant, so the check is independent of the code
    // under test. Source: the report-content column list in "Getting credential reports for
    // your Amazon Web Services account" (IAM User Guide) — the first three columns and
    // `password_enabled` among them are what the recorded report shows.
    let header = report.csv.lines().next().unwrap_or_default();
    assert_eq!(
        header, PUBLISHED_HEADER,
        "the report must carry the published columns, got {header:?}"
    );
    assert!(
        header.starts_with("user,arn,user_creation_time"),
        "the header must begin at the user columns, got {header:?}"
    );
    assert!(
        header.contains("password_enabled"),
        "the header must carry the password_enabled column, got {header:?}"
    );

    // One row per principal the account holds: its root user, the user this emulator's own
    // credential belongs to, and every user created above.
    let reported: Vec<&String> = rows
        .iter()
        .skip(1)
        .map(|row| row.first().expect("a user cell"))
        .collect();
    let mut expected_users = vec![
        state::CREDENTIAL_REPORT_ROOT_USER.to_string(),
        AMBIENT_USER.to_string(),
        plain.to_string(),
        with_password.to_string(),
        with_used_key.to_string(),
        with_inactive_key.to_string(),
        with_two_keys.to_string(),
        with_virtual_mfa.to_string(),
        with_fido.to_string(),
        with_certificate.to_string(),
        comma_name.to_string(),
    ];
    expected_users.sort();
    let mut actual_users: Vec<String> = reported.into_iter().cloned().collect();
    actual_users.sort();
    assert_eq!(
        actual_users, expected_users,
        "the report must carry one row per principal the account holds"
    );

    // Every row carries one cell per published column.
    for row in &rows {
        assert_eq!(
            row.len(),
            state::CREDENTIAL_REPORT_COLUMNS.len(),
            "every row must carry one cell per column, got {row:?}"
        );
    }

    // The root user's row: the account's root ARN, and the password columns that do not
    // apply to it.
    let root = row_for("the root user", &rows, state::CREDENTIAL_REPORT_ROOT_USER);
    assert_eq!(
        cell("the root user", root, "arn"),
        &format!("arn:aws:iam::{}:root", account_id()),
        "the root row must carry this account's root ARN, got {root:?}"
    );

    let table = vec![
        Row {
            user: state::CREDENTIAL_REPORT_ROOT_USER,
            cells: vec![
                ("password_enabled", literal("not_supported")),
                ("password_last_used", literal("no_information")),
                ("password_last_changed", literal("not_supported")),
                ("password_next_rotation", literal("not_supported")),
                ("mfa_active", literal("false")),
                ("access_key_1_active", literal("false")),
                ("access_key_1_last_rotated", literal("N/A")),
                ("cert_1_active", literal("false")),
                ("user_creation_time", Expect::Instant),
            ],
        },
        Row {
            user: plain,
            cells: vec![
                ("user_creation_time", Expect::Instant),
                ("password_enabled", literal("false")),
                ("password_last_used", literal("N/A")),
                ("password_last_changed", literal("N/A")),
                ("password_next_rotation", literal("N/A")),
                ("mfa_active", literal("false")),
                ("access_key_1_active", literal("false")),
                ("access_key_1_last_rotated", literal("N/A")),
                ("access_key_1_last_used_date", literal("N/A")),
                ("access_key_1_last_used_region", literal("N/A")),
                ("access_key_1_last_used_service", literal("N/A")),
                ("access_key_2_active", literal("false")),
                ("cert_1_active", literal("false")),
                ("cert_1_last_rotated", literal("N/A")),
                ("cert_2_active", literal("false")),
            ],
        },
        Row {
            user: with_password,
            cells: vec![
                ("password_enabled", literal("true")),
                // The password exists and has never been used to sign in.
                ("password_last_used", literal("no_information")),
                ("password_last_changed", Expect::Instant),
                // The account's password policy requires no rotation, which is the
                // documented default.
                ("password_next_rotation", literal("N/A")),
            ],
        },
        Row {
            user: with_used_key,
            cells: vec![
                ("access_key_1_active", literal("true")),
                ("access_key_1_last_rotated", Expect::Instant),
                // The key signed a call the account recorded, so its usage is read off
                // that record rather than reported as never used.
                ("access_key_1_last_used_date", Expect::Instant),
                ("access_key_1_last_used_region", literal(HARNESS_REGION)),
                ("access_key_1_last_used_service", literal("iam")),
                // The user holds no second key, so that slot is empty.
                ("access_key_2_active", literal("false")),
                ("access_key_2_last_rotated", literal("N/A")),
                ("access_key_2_last_used_date", literal("N/A")),
            ],
        },
        Row {
            user: with_inactive_key,
            cells: vec![
                ("access_key_1_active", literal("false")),
                ("access_key_1_last_rotated", Expect::Instant),
                ("access_key_1_last_used_date", literal("N/A")),
                ("access_key_2_active", literal("false")),
            ],
        },
        Row {
            user: with_two_keys,
            cells: vec![
                ("access_key_1_active", literal("true")),
                ("access_key_1_last_rotated", Expect::Instant),
                ("access_key_2_active", literal("true")),
                ("access_key_2_last_rotated", Expect::Instant),
            ],
        },
        Row {
            user: with_virtual_mfa,
            cells: vec![("mfa_active", literal("true"))],
        },
        Row {
            user: with_fido,
            cells: vec![("mfa_active", literal("true"))],
        },
        Row {
            user: with_certificate,
            cells: vec![
                ("cert_1_active", literal("true")),
                ("cert_1_last_rotated", Expect::Instant),
                ("cert_2_active", literal("false")),
                ("cert_2_last_rotated", literal("N/A")),
            ],
        },
        Row {
            // The quoted name survives the format: what a client reads back is the name
            // the account holds.
            user: comma_name,
            cells: vec![
                ("arn", literal(&format!("arn:aws:iam::{}:user/{comma_name}", account_id()))),
                ("user_creation_time", Expect::Instant),
            ],
        },
    ];

    for Row { user, cells } in table {
        let row = row_for(user, &rows, user);
        for (column, expected) in cells {
            let actual = cell(user, row, column);
            match expected {
                Expect::Literal(value) => assert_eq!(
                    actual, value,
                    "case {user:?}: expected {column} to carry {value:?}, got {actual:?}"
                ),
                Expect::Instant => assert_instant(user, column, actual),
            }
        }
    }
}

/// Enable a virtual MFA device for `user_name`, in the service's OWN store.
///
/// The fact a report reads is a device a user holds, so it is written into the store the
/// handler reads rather than into a second copy of it; what `EnableMFADevice` does with a
/// presented pair of pass codes is its own suite's subject
/// (`tests/all/virtual_mfa_device_enablement.rs`).
fn enable_virtual_mfa_device(engine: &engine::InMemoryEngine, user_name: &str) {
    let serial_number = state::virtual_mfa_device_arn(state::DEFAULT_PATH, user_name);
    let store = state::service_state(engine).expect("the IAM store");
    store.lock().virtual_mfa_devices.insert(
        serial_number,
        state::VirtualMfaDeviceRecord {
            device_name: user_name.to_string(),
            path: state::DEFAULT_PATH.to_string(),
            base32_string_seed: "ORSXG5BNMRQXIYI".to_string(),
            user_key: Some(state::name_key(user_name)),
            enable_date: Some(DateTime::from_secs(0)),
            tags: None,
        },
    );
}

/// Enroll a FIDO security key for `user_name`, in that same store and for the same reason:
/// no operation of this service enrolls one.
fn enroll_fido_security_key(engine: &engine::InMemoryEngine, user_name: &str) {
    let serial_number = format!("arn:aws:iam::{}:u2f/user/{user_name}/key", account_id());
    let store = state::service_state(engine).expect("the IAM store");
    store.lock().fido_security_keys.insert(
        serial_number.clone(),
        state::FidoSecurityKey {
            serial_number,
            user_key: state::name_key(user_name),
            enable_date: DateTime::from_secs(0),
            certifications: Default::default(),
        },
    );
}

/// RULE: IAM decides the read against the caller's identity-based policies before it
/// reports anything, so a caller no policy allows the action for is refused with HTTP 403
/// `AccessDenied` naming the principal, the action and the account-wide resource — and the
/// same read under a statement that names its action is served.
#[tokio::test]
async fn a_caller_whose_policies_do_not_allow_the_read_is_refused() {
    // Rows: a caller whose only grant is for ANOTHER action, and one explicitly denied this
    // action by a second statement.
    let rows = vec![
        (
            "no allow at all",
            vec![("unrelated".to_string(), statement("Allow", "iam:ListUsers", "*"))],
            "because no identity-based policy allows the iam:GetCredentialReport action".to_string(),
        ),
        (
            "an explicit deny over the allow",
            vec![
                ("allow".to_string(), statement("Allow", "iam:GetCredentialReport", "*")),
                ("deny".to_string(), statement("Deny", "iam:GetCredentialReport", "*")),
            ],
            "with an explicit deny in an identity-based policy".to_string(),
        ),
    ];

    for (row, (case, policies, reason)) in rows.into_iter().enumerate() {
        let (router, engine) = router_with_engine();
        generate_and_wait(&router, &engine).await;
        let user_name = format!("report-reader{row}");
        let access_key_id = principal(&router, case, &user_name, &policies).await;
        let (status, body) = get(&router, Some(&access_key_id)).await;
        assert_eq!(status, 403, "case {case:?}: expected 403, got {status}: {body}");
        assert_eq!(
            error_code(&body),
            "AccessDenied",
            "case {case:?}: expected AccessDenied, got {:?}: {body}",
            error_code(&body)
        );
        assert_access_denied_message(
            case,
            &element(&body, "Message"),
            &format!("arn:aws:iam::{}:user/{user_name}", account_id()),
            "iam:GetCredentialReport",
            state::authorization::ACCOUNT_WIDE_RESOURCE,
            &reason,
        );
    }

    let (router, engine) = router_with_engine();
    generate_and_wait(&router, &engine).await;
    let access_key_id = principal(
        &router,
        "a granted caller",
        "report-reader-granted",
        &[("read".to_string(), statement("Allow", "iam:GetCredentialReport", "*"))],
    )
    .await;
    let (status, body) = get(&router, Some(&access_key_id)).await;
    served("a granted caller", status, &body);
}

/// RULE: the response carries exactly the three members the output shape declares — one
/// `Content`, one `ReportFormat` and one `GeneratedTime` — so a client reading any of them
/// finds a single value.
#[tokio::test]
async fn the_response_carries_the_three_members_the_output_shape_declares() {
    let (router, engine) = router_with_engine();
    generate_and_wait(&router, &engine).await;
    let (status, body) = get(&router, None).await;
    assert_eq!(status, 200, "expected 200, got {status}: {body}");
    for member in ["Content", "ReportFormat", "GeneratedTime"] {
        assert_eq!(
            elements(&body, member).len(),
            1,
            "expected exactly one <{member}>, got {:?}: {body}",
            elements(&body, member)
        );
    }
}
