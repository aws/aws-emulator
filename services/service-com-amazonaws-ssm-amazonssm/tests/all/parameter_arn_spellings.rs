// SPDX-License-Identifier: CC0-1.0

//! Reading Parameter Store by the ARN a read REPORTS, end to end over the front door.
//!
//! RULE under test: a read that takes "the name or Amazon Resource Name (ARN) of the
//! parameter" (`smithy_query(query="doc", shape="GetParameterRequest", member="Name")`)
//! resolves either spelling against the endpoint the request was signed for, and a field an
//! ARN leaves EMPTY names no account and no Region at all, so it constrains nothing. The ARN
//! a read reports therefore resolves the very parameter it was reported for — including the
//! ACCOUNT-LESS ARN reported for a parameter AWS publishes in every account and Region, which
//! no account owns — while an ARN naming another account or another Region names nothing
//! reachable here.
//!
//! Every case goes through the wire a client uses: `POST /` with
//! `X-Amz-Target: AmazonSSM.<Operation>` and an `awsJson1_1` body (`OPERATIONS.md`, "Wire
//! protocol"). Nothing reaches into a state struct.
//!
//! The suites are tables spanning the axes the rule generalises over: who owns the parameter
//! (the account's own writes, and AWS's published catalog — one name per published namespace,
//! discovered through the hierarchy read itself rather than written here), the name's form and
//! depth, its `Type`, how the ARN spells its account and Region fields, which read is called,
//! and whether a selector rides on the ARN.

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use super::parameter_store::{call, error_code, ACCOUNT, REGION};
use super::public_parameter_catalog::{emulator, names, recursive_listing};

/// The hierarchy AWS publishes its own parameters in, as a client writes it.
const PUBLISHED_ROOT: &str = "/aws";

/// How many leading path segments of a published name make its NAMESPACE, which the
/// published rows take one name from each of: `/aws/service/<publisher>`.
const NAMESPACE_SEGMENTS: usize = 3;

/// An account that is not this endpoint's — any well-formed 12-digit id other than
/// [`ACCOUNT`], asserted different below.
const OTHER_ACCOUNT: &str = "210987654321";

/// A Region that is not the one every case signs for ([`REGION`]).
const OTHER_REGION: &str = "eu-central-1";

/// The refusal a read of a name this endpoint resolves nothing for answers with
/// (`smithy_query(query="operation", operation="GetParameter")`: `ParameterNotFound`,
/// HTTP 400).
const NOT_FOUND: &str = "ParameterNotFound";

/// See [`NOT_FOUND`].
const NOT_FOUND_STATUS: u16 = 400;

/// How a case writes the `Name` it reads a parameter by.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Spelling {
    /// The parameter's name itself, as the read reported it.
    Name,
    /// The ARN the read reported for it, exactly as reported.
    ReportedArn,
    /// That ARN with its account field emptied — the spelling AWS writes an account-less
    /// resource in, which is already how a published parameter's ARN is reported.
    ArnWithoutAccount,
    /// That ARN with its Region field emptied.
    ArnWithoutRegion,
    /// That ARN with both emptied.
    ArnWithoutAccountOrRegion,
    /// That ARN with its account field naming [`OTHER_ACCOUNT`].
    ArnOfAnotherAccount,
    /// That ARN with its Region field naming [`OTHER_REGION`].
    ArnOfAnotherRegion,
}

/// Every spelling, in the order the suites report their cases in.
const SPELLINGS: &[Spelling] = &[
    Spelling::Name,
    Spelling::ReportedArn,
    Spelling::ArnWithoutAccount,
    Spelling::ArnWithoutRegion,
    Spelling::ArnWithoutAccountOrRegion,
    Spelling::ArnOfAnotherAccount,
    Spelling::ArnOfAnotherRegion,
];

impl Spelling {
    /// Whether this spelling names the parameter it was built from at the endpoint the
    /// request is signed for.
    fn resolves(self) -> bool {
        !matches!(
            self,
            Spelling::ArnOfAnotherAccount | Spelling::ArnOfAnotherRegion
        )
    }

    /// `name` written this way, given the `arn` a read reported for it.
    fn spell(self, name: &str, arn: &str) -> String {
        match self {
            Spelling::Name => name.to_string(),
            Spelling::ReportedArn => arn.to_string(),
            Spelling::ArnWithoutAccount => rewritten(arn, None, Some("")),
            Spelling::ArnWithoutRegion => rewritten(arn, Some(""), None),
            Spelling::ArnWithoutAccountOrRegion => rewritten(arn, Some(""), Some("")),
            Spelling::ArnOfAnotherAccount => rewritten(arn, None, Some(OTHER_ACCOUNT)),
            Spelling::ArnOfAnotherRegion => rewritten(arn, Some(OTHER_REGION), None),
        }
    }
}

/// `arn` with its Region and account fields replaced by the ones given; `None` keeps the
/// field as written.
///
/// An ARN is six colon-separated fields,
/// `arn:<partition>:<vendor>:<region>:<account>:<resource>`, and the resource keeps every
/// colon and slash it carries.
fn rewritten(arn: &str, region: Option<&str>, account: Option<&str>) -> String {
    let mut fields: Vec<&str> = arn.splitn(6, ':').collect();
    assert_eq!(
        fields.len(),
        6,
        "an ssm ARN carries six fields, got {arn:?}"
    );
    if let Some(region) = region {
        fields[3] = region;
    }
    if let Some(account) = account {
        fields[4] = account;
    }
    fields.join(":")
}

/// One parameter the suites read, and the identity the reads must agree on for it.
#[derive(Debug)]
struct Subject {
    /// What this row stands for, for a case label.
    label: String,
    /// The `Name` a read reports for it.
    name: String,
    /// The `ARN` a read reports for it.
    arn: String,
    /// The whole reported `Parameter`, which every resolving spelling must report
    /// identically.
    parameter: Value,
}

/// The account's own parameters the suites read, as `(label, name, type, value)` — the rows
/// differ in name form and depth (a flat name, a two-level name, a deep hierarchy) and in
/// `ParameterType` (`smithy_query(query="shape", shape="ParameterType")`), so the rule is
/// asserted over more than one shape of the account's own name space.
const OWN_PARAMETERS: &[(&str, &str, &str, &str)] = &[
    ("a flat name", "db-host", "String", "db.internal"),
    (
        "a two-level hierarchy",
        "/app/db-host",
        "String",
        "db.dev.internal",
    ),
    (
        "a deep hierarchy",
        "/app/prod/eu/web/tier/db-host",
        "String",
        "db.prod.internal",
    ),
    (
        "a list value",
        "/app/prod/allowed-cidrs",
        "StringList",
        "10.0.0.0/8,10.1.0.0/16",
    ),
];

/// `GetParameter` for `name`, as `(status, body)`.
async fn get_parameter(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GetParameter", &json!({ "Name": name })).await
}

/// `GetParameters` for the single `name`, as `(status, body)`.
async fn get_parameters(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GetParameters", &json!({ "Names": [name] })).await
}

/// `GetParameterHistory` for `name`, as `(status, body)`.
async fn get_parameter_history(router: &MultiServiceRouter, name: &str) -> (u16, Value) {
    call(router, "GetParameterHistory", &json!({ "Name": name })).await
}

/// Write the account's own rows of [`OWN_PARAMETERS`] and read each back by name, returning
/// the identity every spelling of it must resolve to.
async fn own_subjects(router: &MultiServiceRouter) -> Vec<Subject> {
    let mut subjects = Vec::new();
    for (label, name, type_, value) in OWN_PARAMETERS.iter().copied() {
        let (status, body) = call(
            router,
            "PutParameter",
            &json!({ "Name": name, "Value": value, "Type": type_ }),
        )
        .await;
        assert_eq!(
            status, 200,
            "case {label}: PutParameter {name} must succeed, got {status} with {body}"
        );
        subjects.push(subject(router, &format!("the account's own, {label}"), name).await);
    }
    subjects
}

/// One name AWS publishes in each namespace of its own hierarchy, read back by name.
///
/// The names are DISCOVERED through the hierarchy read rather than written here, so the rows
/// are whatever this endpoint publishes and no row names a parameter the catalog does not.
async fn published_subjects(router: &MultiServiceRouter) -> Vec<Subject> {
    let published = names(&recursive_listing(router, "the published catalog", PUBLISHED_ROOT).await);
    assert!(
        !published.is_empty(),
        "the published catalog must publish at least one parameter under {PUBLISHED_ROOT}, \
         got none"
    );
    let mut namespaces: Vec<String> = Vec::new();
    let mut subjects = Vec::new();
    for name in published {
        let namespace: String = name
            .split('/')
            .take(NAMESPACE_SEGMENTS + 1)
            .collect::<Vec<_>>()
            .join("/");
        if namespaces.contains(&namespace) {
            continue;
        }
        namespaces.push(namespace.clone());
        subjects.push(subject(router, &format!("AWS's own, under {namespace}"), &name).await);
    }
    subjects
}

/// Read `name` back by name and hold what the read reported as this subject's identity.
async fn subject(router: &MultiServiceRouter, label: &str, name: &str) -> Subject {
    let (status, body) = get_parameter(router, name).await;
    assert_eq!(
        status, 200,
        "case {label}: GetParameter {name} must be served, got {status} with {body}"
    );
    let parameter = body["Parameter"].clone();
    let reported = parameter["Name"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: a served read reports a Name, got {parameter}"))
        .to_string();
    let arn = parameter["ARN"]
        .as_str()
        .unwrap_or_else(|| panic!("case {label}: a served read reports an ARN, got {parameter}"))
        .to_string();
    Subject {
        label: label.to_string(),
        name: reported,
        arn,
        parameter,
    }
}

/// RULE: the ARN a read reports resolves the very parameter it was reported for, through
/// every read that takes a name or an ARN, with the account field written, emptied or absent
/// — and an ARN naming another account or another Region resolves nothing.
#[tokio::test]
async fn every_spelling_of_a_reported_arn_resolves_the_same_parameter() {
    assert_ne!(
        OTHER_ACCOUNT, ACCOUNT,
        "the other-account row must name an account that is not this endpoint's"
    );
    assert_ne!(
        OTHER_REGION, REGION,
        "the other-Region row must name a Region that is not the one cases sign for"
    );
    let router = emulator();
    let mut subjects = own_subjects(&router).await;
    subjects.extend(published_subjects(&router).await);

    for subject in &subjects {
        for spelling in SPELLINGS.iter().copied() {
            let asked = spelling.spell(&subject.name, &subject.arn);
            let case = format!("{} read as {spelling:?} ({asked})", subject.label);

            let (status, body) = get_parameter(&router, &asked).await;
            if spelling.resolves() {
                assert_eq!(
                    status, 200,
                    "case {case}: expected GetParameter to be served, got {status} with {body}"
                );
                assert_eq!(
                    body["Parameter"], subject.parameter,
                    "case {case}: expected the parameter {} reports, got {body}",
                    subject.name
                );
            } else {
                assert_eq!(
                    status, NOT_FOUND_STATUS,
                    "case {case}: expected HTTP {NOT_FOUND_STATUS} from GetParameter, got \
                     {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    NOT_FOUND,
                    "case {case}: expected {NOT_FOUND} from GetParameter, got {body}"
                );
            }

            let (status, body) = get_parameters(&router, &asked).await;
            assert_eq!(
                status, 200,
                "case {case}: GetParameters reports an unresolvable name rather than refusing, \
                 got {status} with {body}"
            );
            let served = body["Parameters"].as_array().cloned().unwrap_or_default();
            let unresolved = body["InvalidParameters"]
                .as_array()
                .cloned()
                .unwrap_or_default();
            if spelling.resolves() {
                assert_eq!(
                    served.as_slice(),
                    std::slice::from_ref(&subject.parameter),
                    "case {case}: expected GetParameters to report the one parameter {}, got \
                     {body}",
                    subject.name
                );
                assert!(
                    unresolved.is_empty(),
                    "case {case}: expected no unresolvable name from GetParameters, got {body}"
                );
            } else {
                assert!(
                    served.is_empty(),
                    "case {case}: expected GetParameters to report no parameter, got {body}"
                );
                assert_eq!(
                    unresolved.as_slice(),
                    [Value::String(asked.clone())],
                    "case {case}: expected GetParameters to report the asked name as \
                     unresolvable, got {body}"
                );
            }

            let (status, body) = get_parameter_history(&router, &asked).await;
            if spelling.resolves() {
                assert_eq!(
                    status, 200,
                    "case {case}: expected GetParameterHistory to be served, got {status} with \
                     {body}"
                );
                let history = body["Parameters"].as_array().cloned().unwrap_or_default();
                let newest = history
                    .last()
                    .unwrap_or_else(|| panic!("case {case}: expected a history entry, got {body}"));
                assert_eq!(
                    (&newest["Name"], &newest["Value"], &newest["Version"]),
                    (
                        &subject.parameter["Name"],
                        &subject.parameter["Value"],
                        &subject.parameter["Version"]
                    ),
                    "case {case}: expected the history's newest entry to be the parameter the \
                     single read reports, got {body}"
                );
            } else {
                assert_eq!(
                    status, NOT_FOUND_STATUS,
                    "case {case}: expected HTTP {NOT_FOUND_STATUS} from GetParameterHistory, \
                     got {status} with {body}"
                );
                assert_eq!(
                    error_code(&body),
                    NOT_FOUND,
                    "case {case}: expected {NOT_FOUND} from GetParameterHistory, got {body}"
                );
            }
        }
    }
}

/// RULE: a `name:selector` suffix rides on an ARN exactly as it rides on a bare name
/// (`GetParameterRequest$Name`: "To query by parameter label, use "Name": "name:label". To
/// query by parameter version, use "Name": "name:version""), whichever way the ARN spells
/// its account and Region.
#[tokio::test]
async fn a_selector_rides_on_every_resolving_spelling_of_an_arn() {
    let router = emulator();
    let name = "/app/prod/db-password-rotation";
    let label = "release-candidate";
    // Three versions, so a selector has something to choose between: the newest is what an
    // unselected read reports and the second is what the rows below select.
    let values = ["first", "second", "third"];
    for value in values {
        let (status, body) = call(
            &router,
            "PutParameter",
            &json!({ "Name": name, "Value": value, "Type": "String", "Overwrite": true }),
        )
        .await;
        assert_eq!(
            status, 200,
            "PutParameter {name}={value} must succeed, got {status} with {body}"
        );
    }
    let (status, body) = call(
        &router,
        "LabelParameterVersion",
        &json!({ "Name": name, "ParameterVersion": 2, "Labels": [label] }),
    )
    .await;
    assert_eq!(
        status, 200,
        "LabelParameterVersion must succeed, got {status} with {body}"
    );

    let subject = subject(&router, "a parameter with three versions", name).await;
    // Each row selects version 2 — once by number, once by the label attached to it — so
    // both selector forms are asserted against the same version's value.
    let selectors = ["2", label];
    for spelling in SPELLINGS
        .iter()
        .copied()
        .filter(|spelling| spelling.resolves())
    {
        let base = spelling.spell(&subject.name, &subject.arn);
        for selector in selectors {
            let asked = format!("{base}:{selector}");
            let case = format!("{spelling:?} with selector {selector} ({asked})");
            let (status, body) = get_parameter(&router, &asked).await;
            assert_eq!(
                status, 200,
                "case {case}: expected GetParameter to be served, got {status} with {body}"
            );
            let parameter = &body["Parameter"];
            assert_eq!(
                (
                    parameter["Version"].as_i64(),
                    parameter["Value"].as_str(),
                    parameter["Selector"].as_str(),
                ),
                (Some(2), Some(values[1]), Some(format!(":{selector}").as_str())),
                "case {case}: expected the selected version reported with its selector, got \
                 {body}"
            );
            assert_eq!(
                parameter["ARN"].as_str(),
                Some(subject.arn.as_str()),
                "case {case}: expected the parameter's one ARN however it was asked for, got \
                 {body}"
            );
        }
    }
}

/// RULE: an ARN is still only a spelling of a name the endpoint resolves — an ARN whose
/// resource names a parameter neither the account's store nor AWS's published catalog holds
/// is the modelled miss, whichever way its account and Region are spelled.
#[tokio::test]
async fn an_arn_of_a_parameter_nothing_holds_is_the_modelled_miss() {
    let router = emulator();
    // One name in the account's own space and one in AWS's published hierarchy, neither of
    // which anything holds: the published one can never be written at all
    // (`PutParameter` refuses the `/aws` hierarchy), so the catalog is the only thing that
    // could hold it.
    let absent = [
        ("the account's own space", "/app/never-written"),
        ("AWS's own hierarchy", "/aws/service/never-published/leaf"),
    ];
    for (where_, name) in absent {
        let arn = format!("arn:aws:ssm:{REGION}:{ACCOUNT}:parameter{name}");
        for spelling in SPELLINGS.iter().copied() {
            let asked = spelling.spell(name, &arn);
            let case = format!("{where_}: {spelling:?} ({asked})");
            let (status, body) = get_parameter(&router, &asked).await;
            assert_eq!(
                status, NOT_FOUND_STATUS,
                "case {case}: expected HTTP {NOT_FOUND_STATUS}, got {status} with {body}"
            );
            assert_eq!(
                error_code(&body),
                NOT_FOUND,
                "case {case}: expected {NOT_FOUND}, got {body}"
            );
        }
    }
}
