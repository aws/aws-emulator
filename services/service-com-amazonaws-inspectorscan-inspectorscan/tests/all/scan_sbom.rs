// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `ScanSbom`, driven over HTTP through the same
//! `MultiServiceRouter` the emulator serves with.
//!
//! The rules under test:
//!
//! * a component is matched by its purl ecosystem + ecosystem-canonical name +
//!   version against the advisory store, and a finding is reported exactly when
//!   the resolved version falls inside the advisory's affected range;
//! * every reported finding is a projection of the matched advisory onto the
//!   matched component — one rating per scoring vendor the advisory holds a
//!   score from, the CWEs, the fixed-in
//!   version, and an `affects[].ref` that resolves in the submitted document;
//! * the three `outputFormat` values render the SAME finding set;
//! * the operation is `@idempotent`: the same SBOM scanned twice answers the
//!   identical report and records one scan;
//! * a document that is not a CycloneDX 1.5 BOM, or that exceeds the enforced
//!   5000-component limit, is refused with a `ValidationException` (what that
//!   refusal SAYS is pinned in `scan_sbom_refusals.rs`).
//!
//! The advisory catalog these cases are written against is
//! `src/state.rs::seed_vulnerability_db`.

use serde_json::{json, Value};
use service_com_amazonaws_inspectorscan_inspectorscan::state::{
    version_in_range, ComAmazonawsInspectorscanInspectorscanState, OutputFormat, ReleaseVersion,
    VersionRange, MAX_SCANS,
};
use service_com_amazonaws_inspectorscan_inspectorscan::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;
use std::collections::BTreeSet;
use std::sync::Arc;

/// A SigV4 credential scope naming this service, which is how the router
/// resolves the request to it. Request authentication is switched off below, so
/// the signature itself is never verified.
const AUTHORIZATION: &str = "AWS4-HMAC-SHA256 \
     Credential=AKIDEMULATORTEST/20260101/us-east-1/inspector-scan/aws4_request, \
     SignedHeaders=host, Signature=00";

/// One emulator: the shared engine plus a router with only this service mounted.
pub(crate) fn emulator() -> (Arc<engine::InMemoryEngine>, MultiServiceRouter) {
    let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::clone(&engine)));
    (engine, router)
}

/// `POST /scan/sbom` with `request` as the body; the status and parsed body.
pub(crate) async fn post_scan(router: &MultiServiceRouter, request: &Value) -> (u16, Value) {
    let body = serde_json::to_vec(request).expect("serialize the ScanSbom request");
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri("https://inspector-scan.us-east-1.amazonaws.com/scan/sbom")
        .header("host", "inspector-scan.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(body))
        .expect("build the ScanSbom request");
    let response = router.dispatch(req).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// `POST /scan/sbom` with `body` sent verbatim, so the TEXTUAL order of the
/// request's members is what the handler receives.
async fn post_scan_body(router: &MultiServiceRouter, body: &str) -> (u16, Value) {
    let req = http::Request::builder()
        .method(http::Method::POST)
        .uri("https://inspector-scan.us-east-1.amazonaws.com/scan/sbom")
        .header("host", "inspector-scan.us-east-1.amazonaws.com")
        .header("content-type", "application/json")
        .header("authorization", AUTHORIZATION)
        .body(bytes::Bytes::from(body.to_owned()))
        .expect("build the ScanSbom request");
    let response = router.dispatch(req).await;
    let status = response.status().as_u16();
    let parsed: Value = serde_json::from_slice(response.body()).unwrap_or(Value::Null);
    (status, parsed)
}

/// A minimal, valid CycloneDX 1.5 BOM carrying `components`.
pub(crate) fn bom(components: Vec<Value>) -> Value {
    json!({
        "bomFormat": "CycloneDX",
        "specVersion": "1.5",
        "version": 1,
        "metadata": {
            "component": { "type": "application", "name": "test-app", "version": "1.0.0" }
        },
        "components": components,
    })
}

/// A CycloneDX component with an explicit `bom-ref` and `purl`.
pub(crate) fn component(bom_ref: &str, name: &str, version: &str, purl: &str) -> Value {
    json!({
        "type": "library",
        "bom-ref": bom_ref,
        "name": name,
        "version": version,
        "purl": purl,
    })
}

/// A JSON document nested `levels` arrays deep — the shape used to prove the
/// handler refuses deep nesting instead of recursing into the thread stack.
fn deeply_nested_value(levels: usize) -> Value {
    let mut value = json!({ "bomFormat": "CycloneDX", "specVersion": "1.5" });
    for _ in 0..levels {
        value = Value::Array(vec![value]);
    }
    value
}

/// An assembly component whose `components[]` nests `levels` deep, the deepest
/// level carrying a known-vulnerable package.
fn nested_assembly(levels: usize) -> Value {
    let mut entry = component(
        "ref-deep",
        "minimist",
        "1.2.5",
        "pkg:npm/minimist@1.2.5",
    );
    for level in 0..levels {
        entry = json!({
            "type": "application",
            "bom-ref": format!("ref-assembly-{level}"),
            "name": format!("assembly-{level}"),
            "version": "1.0.0",
            "components": [entry],
        });
    }
    entry
}

/// The vulnerability ids a CycloneDX report lists, in report order.
fn cyclonedx_ids(response: &Value) -> Vec<String> {
    response["sbom"]["vulnerabilities"]
        .as_array()
        .map(|entries| {
            entries
                .iter()
                .map(|v| v["id"].as_str().unwrap_or_default().to_string())
                .collect()
        })
        .unwrap_or_default()
}

/// One row of the matching table: a component coordinate and the vulnerability
/// ids the scan must report for it.
#[derive(Debug)]
struct MatchCase {
    name: &'static str,
    version: &'static str,
    purl: Option<&'static str>,
    expected: &'static [&'static str],
}

/// The matching table. Rows differ along ecosystem (npm, PyPI, Maven, Go, deb,
/// rpm), along the version's FORM (plain, `v`-prefixed, two-component, a
/// distribution release suffix, a Debian epoch), and along where the version
/// falls relative to each advisory's published bounds (inside, exactly at the
/// fixed version, past it, and below a lower bound).
///
/// Every coordinate and bound comes from `src/state.rs::seed_vulnerability_db`.
const MATCH_CASES: &[MatchCase] = &[
    // npm, below the upper bound.
    MatchCase {
        name: "lodash",
        version: "4.17.20",
        purl: Some("pkg:npm/lodash@4.17.20"),
        expected: &["CVE-2021-23337"],
    },
    // npm, exactly at the published fixed version: the bound is exclusive.
    MatchCase {
        name: "lodash",
        version: "4.17.21",
        purl: Some("pkg:npm/lodash@4.17.21"),
        expected: &[],
    },
    // npm, past the bound.
    MatchCase {
        name: "lodash",
        version: "5.0.0",
        purl: Some("pkg:npm/lodash@5.0.0"),
        expected: &[],
    },
    MatchCase {
        name: "minimist",
        version: "1.2.5",
        purl: Some("pkg:npm/minimist@1.2.5"),
        expected: &["CVE-2021-44906"],
    },
    MatchCase {
        name: "minimist",
        version: "1.2.6",
        purl: Some("pkg:npm/minimist@1.2.6"),
        expected: &[],
    },
    // PyPI.
    MatchCase {
        name: "urllib3",
        version: "1.26.4",
        purl: Some("pkg:pypi/urllib3@1.26.4"),
        expected: &["CVE-2021-33503"],
    },
    MatchCase {
        name: "urllib3",
        version: "1.26.5",
        purl: Some("pkg:pypi/urllib3@1.26.5"),
        expected: &[],
    },
    // PyPI, a two-component version: `5.3` normalises to 5.3.0.
    MatchCase {
        name: "PyYAML",
        version: "5.3",
        purl: Some("pkg:pypi/pyyaml@5.3"),
        expected: &["CVE-2020-14343"],
    },
    // Maven: the advisory is keyed by the `group:artifact` coordinate the purl
    // namespace and name resolve to.
    MatchCase {
        name: "log4j-core",
        version: "2.14.1",
        purl: Some("pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1"),
        expected: &["CVE-2021-44228"],
    },
    MatchCase {
        name: "log4j-core",
        version: "2.15.0",
        purl: Some("pkg:maven/org.apache.logging.log4j/log4j-core@2.15.0"),
        expected: &[],
    },
    // Maven, below the advisory's LOWER bound (`>=2.0.0`).
    MatchCase {
        name: "log4j-core",
        version: "1.2.17",
        purl: Some("pkg:maven/org.apache.logging.log4j/log4j-core@1.2.17"),
        expected: &[],
    },
    MatchCase {
        name: "jackson-databind",
        version: "2.9.10",
        purl: Some("pkg:maven/com.fasterxml.jackson.core/jackson-databind@2.9.10"),
        expected: &["CVE-2020-36518"],
    },
    MatchCase {
        name: "jackson-databind",
        version: "2.12.6",
        purl: Some("pkg:maven/com.fasterxml.jackson.core/jackson-databind@2.12.6"),
        expected: &[],
    },
    // Go: the module path is the canonical name, and the version carries Go's
    // `v` prefix.
    MatchCase {
        name: "gin",
        version: "v1.7.0",
        purl: Some("pkg:golang/github.com/gin-gonic/gin@v1.7.0"),
        expected: &["CVE-2020-28483"],
    },
    MatchCase {
        name: "gin",
        version: "v1.7.7",
        purl: Some("pkg:golang/github.com/gin-gonic/gin@v1.7.7"),
        expected: &[],
    },
    // An OS package whose version carries a Debian epoch and a distribution
    // release suffix; the purl names no version, so the component's does.
    MatchCase {
        name: "openssl",
        version: "1:3.0.6-1ubuntu1",
        purl: Some("pkg:deb/debian/openssl"),
        expected: &["CVE-2022-3602"],
    },
    MatchCase {
        name: "openssl",
        version: "1:3.0.7-1ubuntu1",
        purl: Some("pkg:deb/debian/openssl"),
        expected: &[],
    },
    // An rpm package: a two-component upstream version inside the range.
    MatchCase {
        name: "glibc",
        version: "2.37",
        purl: Some("pkg:rpm/redhat/glibc@2.37"),
        expected: &["CVE-2023-4911"],
    },
    MatchCase {
        name: "glibc",
        version: "2.33",
        purl: Some("pkg:rpm/redhat/glibc@2.33"),
        expected: &[],
    },
    // A package with no advisory at all.
    MatchCase {
        name: "left-pad",
        version: "1.0.0",
        purl: Some("pkg:npm/left-pad@1.0.0"),
        expected: &[],
    },
    // The same ecosystem-canonical name in the WRONG ecosystem matches nothing:
    // the index is keyed by (ecosystem, name).
    MatchCase {
        name: "lodash",
        version: "4.17.20",
        purl: Some("pkg:pypi/lodash@4.17.20"),
        expected: &[],
    },
    // A component with no purl names no ecosystem, so it matches no advisory —
    // and is still a valid component, answered 200.
    MatchCase {
        name: "lodash",
        version: "4.17.20",
        purl: None,
        expected: &[],
    },
    // The remaining rows exercise the purl GRAMMAR, which the specification
    // states as `pkg:type/namespace/name@version?qualifiers#subpath`: the
    // optional trailing parts take no part in matching a coordinate, the type
    // and the scheme are case-insensitive, and each component is
    // percent-encoded.
    //
    // Qualifiers and a subpath are discarded, so the coordinate still matches.
    MatchCase {
        name: "lodash",
        version: "4.17.20",
        purl: Some("pkg:npm/lodash@4.17.20?arch=x86_64&repository_url=registry.npmjs.org#lib"),
        expected: &["CVE-2021-23337"],
    },
    // The purl TYPE is case-insensitive per the specification ("the type is
    // case insensitive. The canonical form is lowercase"), so the ecosystem
    // resolves whatever case it arrives in.
    MatchCase {
        name: "minimist",
        version: "1.2.5",
        purl: Some("pkg:NPM/minimist@1.2.5"),
        expected: &["CVE-2021-44906"],
    },
    // A percent-encoded name decodes before it is looked up (`%33` is `3`).
    MatchCase {
        name: "urllib3",
        version: "1.26.4",
        purl: Some("pkg:pypi/urllib%33@1.26.4"),
        expected: &["CVE-2021-33503"],
    },
    // Leading slashes after the scheme are permitted and ignored.
    MatchCase {
        name: "glibc",
        version: "2.37",
        purl: Some("pkg://rpm/redhat/glibc@2.37"),
        expected: &["CVE-2023-4911"],
    },
    // A percent-encoded NAMESPACE decodes too, and a maven coordinate's
    // namespace is its groupId (`%2E` is `.`).
    MatchCase {
        name: "log4j-core",
        version: "2.14.1",
        purl: Some("pkg:maven/org%2Eapache%2Elogging%2Elog4j/log4j-core@2.14.1"),
        expected: &["CVE-2021-44228"],
    },
    // A `@` inside a qualifier does not make the version: the version is what
    // follows the LAST `@` of the coordinate BEFORE the qualifiers, which are
    // split off first.
    MatchCase {
        name: "minimist",
        version: "1.2.5",
        purl: Some("pkg:npm/minimist@1.2.5?download_url=https://example.invalid/a@b"),
        expected: &["CVE-2021-44906"],
    },
    // A coordinate whose version the purl omits falls back to the component's
    // own `version` member.
    MatchCase {
        name: "urllib3",
        version: "1.26.4",
        purl: Some("pkg:pypi/urllib3"),
        expected: &["CVE-2021-33503"],
    },
];

#[tokio::test]
async fn each_component_is_matched_against_the_stored_advisories() {
    let (_engine, router) = emulator();
    for case in MATCH_CASES {
        let mut entry = json!({
            "type": "library",
            "bom-ref": "pkg:under-test",
            "name": case.name,
            "version": case.version,
        });
        if let Some(purl) = case.purl {
            entry["purl"] = json!(purl);
        }
        let request = json!({ "sbom": bom(vec![entry]), "outputFormat": "CYCLONE_DX_1_5" });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {case:?}: expected HTTP 200, got {status} with body {response}"
        );
        let ids = cyclonedx_ids(&response);
        let expected: Vec<String> = case.expected.iter().map(|s| (*s).to_string()).collect();
        assert_eq!(
            ids, expected,
            "case {case:?}: expected vulnerability ids {expected:?}, got {ids:?}"
        );
        for entry in response["sbom"]["vulnerabilities"]
            .as_array()
            .unwrap_or(&Vec::new())
        {
            assert_eq!(
                entry["affects"][0]["ref"].as_str(),
                // The scan identifies the components of its own report, so the
                // only component of this one is named by the first ref the scan
                // assigns, whatever ref the request carried.
                Some("comp-1"),
                "case {case:?}: affects[].ref must name the matched component, got {:?}",
                entry["affects"]
            );
        }
    }
}

#[tokio::test]
async fn a_finding_carries_one_rating_per_scoring_vendor() {
    let (_engine, router) = emulator();
    let request = json!({
        "sbom": bom(vec![component(
            "ref-log4j",
            "log4j-core",
            "2.14.1",
            "pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1",
        )]),
        "outputFormat": "CYCLONE_DX_1_5",
    });
    let (status, response) = post_scan(&router, &request).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");

    let vulnerabilities = response["sbom"]["vulnerabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    assert_eq!(
        vulnerabilities.len(),
        1,
        "expected exactly one finding, got {vulnerabilities:?}"
    );
    let finding = &vulnerabilities[0];
    assert_eq!(
        finding["id"].as_str(),
        Some("CVE-2021-44228"),
        "expected the Log4Shell advisory, got {finding}"
    );

    let ratings = finding["ratings"].as_array().cloned().unwrap_or_default();
    let sources: Vec<&str> = ratings
        .iter()
        .map(|r| r["source"]["name"].as_str().unwrap_or_default())
        .collect();
    assert_eq!(
        sources,
        vec!["NVD", "Amazon Inspector", "EPSS"],
        "expected one rating per scoring vendor the advisory holds a score from, got \
         {ratings:?}"
    );
    for rating in &ratings {
        let source = rating["source"]["name"].as_str().unwrap_or_default();
        if source == "EPSS" {
            // An exploit probability, on its own scale and under the schema's
            // catch-all method: it is not a CVSS score, so it carries no vector.
            assert_eq!(
                rating["method"].as_str(),
                Some("other"),
                "case {source:?}: EPSS is not one of CycloneDX's named methods, got {rating}"
            );
            assert!(
                rating["score"]
                    .as_f64()
                    .is_some_and(|s| (0.0..=1.0).contains(&s)),
                "case {source:?}: an exploit probability lies in 0.0..=1.0, got {rating}"
            );
            assert!(
                rating["vector"].is_null(),
                "case {source:?}: an EPSS rating is not a CVSS vector, got {rating}"
            );
            continue;
        }
        assert_eq!(
            rating["method"].as_str(),
            Some("CVSSv31"),
            "case {source:?}: a severity rating is a CVSS v3.1 score, got {rating}"
        );
        assert!(
            rating["score"].as_f64().is_some_and(|s| (0.0..=10.0).contains(&s)),
            "case {source:?}: a CVSS base score lies in 0.0..=10.0, got {rating}"
        );
        assert!(
            rating["vector"]
                .as_str()
                .is_some_and(|v| v.starts_with("CVSS:3.1/")),
            "case {source:?}: a CVSS v3.1 rating carries a v3.1 vector, got {rating}"
        );
        assert_eq!(
            rating["severity"].as_str(),
            Some("critical"),
            "case {source:?}: CycloneDX 1.5 spells severities in lowercase, got {rating}"
        );
    }

    assert_eq!(
        finding["cwes"],
        json!([917, 502, 20]),
        "CycloneDX 1.5 types `cwes` as integers, got {}",
        finding["cwes"]
    );
    assert_eq!(
        finding["source"]["name"].as_str(),
        Some("NVD"),
        "expected the advisory's own source, got {}",
        finding["source"]
    );
    // The node republishes no advisory reference pages: which members may carry
    // an address at all is pinned in `scan_report_links.rs`.
    assert!(
        finding["advisories"].is_null(),
        "a CycloneDX node republishes no advisory reference pages, got {finding}"
    );
    let properties = finding["properties"].as_array().cloned().unwrap_or_default();
    assert!(
        properties.iter().any(|p| {
            p["name"].as_str() == Some("amazon:inspector:sbom_scanner:fixed_in_version")
                && p["value"].as_str() == Some("2.15.0")
        }),
        "expected the stored fixed-in version as a property, got {properties:?}"
    );
    assert_eq!(
        finding["affects"][0]["ref"].as_str(),
        Some("comp-1"),
        "affects[].ref must name the scan's own ref for the matched component, got {}",
        finding["affects"]
    );
}

/// `serialNumber` identifies the BOM instance: a submitted one is preserved and
/// an absent one is stamped, and both are stable across a repeat of the same
/// document.
#[tokio::test]
async fn a_submitted_serial_number_is_preserved_and_an_absent_one_is_stamped() {
    let (_engine, router) = emulator();
    let components = vec![component(
        "ref-urllib3",
        "urllib3",
        "1.26.4",
        "pkg:pypi/urllib3@1.26.4",
    )];

    // Row 1: the caller names the BOM instance, so the report answers with the
    // name the caller gave it.
    let submitted_serial = "urn:uuid:3e671687-395b-41f5-a30f-a58921a69b79";
    let mut with_serial = bom(components.clone());
    with_serial["serialNumber"] = json!(submitted_serial);
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": with_serial.clone(), "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    assert_eq!(
        response["sbom"]["serialNumber"].as_str(),
        Some(submitted_serial),
        "case \"submitted serialNumber\": expected the submitted serial number, got {}",
        response["sbom"]["serialNumber"]
    );

    // Row 2: an EMPTY serial number names no instance, so one is stamped.
    let mut empty_serial = bom(components.clone());
    empty_serial["serialNumber"] = json!("");
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": empty_serial, "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    assert!(
        response["sbom"]["serialNumber"]
            .as_str()
            .is_some_and(|serial| serial.starts_with("urn:uuid:")),
        "case \"empty serialNumber\": expected a stamped urn:uuid serial number, got {}",
        response["sbom"]["serialNumber"]
    );

    // Row 3: the same document twice answers the same serial number.
    let (status, repeat) = post_scan(
        &router,
        &json!({ "sbom": with_serial, "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {repeat}");
    assert_eq!(
        repeat["sbom"]["serialNumber"].as_str(),
        Some(submitted_serial),
        "case \"repeat\": a repeat must answer the same serial number, got {}",
        repeat["sbom"]["serialNumber"]
    );
}

#[tokio::test]
async fn the_report_preserves_the_submitted_bom_and_stamps_its_identity() {
    let (_engine, router) = emulator();
    let submitted = bom(vec![component(
        "ref-urllib3",
        "urllib3",
        "1.26.4",
        "pkg:pypi/urllib3@1.26.4",
    )]);
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": submitted, "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");

    let report = &response["sbom"];
    assert_eq!(
        report["bomFormat"].as_str(),
        Some("CycloneDX"),
        "the report preserves bomFormat, got {report}"
    );
    assert_eq!(
        report["specVersion"].as_str(),
        Some("1.5"),
        "the report preserves specVersion, got {report}"
    );
    assert_eq!(
        report["components"][0]["name"].as_str(),
        Some("urllib3"),
        "the report preserves the submitted components, got {}",
        report["components"]
    );
    assert!(
        report["serialNumber"]
            .as_str()
            .is_some_and(|s| s.starts_with("urn:uuid:") && s.len() == "urn:uuid:".len() + 36),
        "the report stamps a UUID urn serialNumber, got {}",
        report["serialNumber"]
    );
    assert!(
        report["metadata"]["timestamp"]
            .as_str()
            .is_some_and(|t| t.ends_with('Z') && t.contains('T')),
        "the report stamps an RFC 3339 metadata.timestamp, got {}",
        report["metadata"]
    );
    assert_eq!(
        report["metadata"]["component"]["name"].as_str(),
        Some("test-app"),
        "the report preserves the submitted metadata, got {}",
        report["metadata"]
    );
}

#[tokio::test]
async fn a_component_without_a_bom_ref_is_given_one_the_findings_resolve_against() {
    let (_engine, router) = emulator();
    let submitted = bom(vec![json!({
        "type": "library",
        "name": "minimist",
        "version": "1.2.5",
        "purl": "pkg:npm/minimist@1.2.5",
    })]);
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": submitted, "outputFormat": "CYCLONE_DX_1_5" }),
    )
    .await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");

    let stamped = response["sbom"]["components"][0]["bom-ref"]
        .as_str()
        .map(str::to_string);
    assert!(
        stamped.is_some(),
        "a component with no bom-ref must be stamped with one, got {}",
        response["sbom"]["components"]
    );
    assert_eq!(
        response["sbom"]["vulnerabilities"][0]["affects"][0]["ref"]
            .as_str()
            .map(str::to_string),
        stamped,
        "affects[].ref must resolve to the stamped component bom-ref, got {}",
        response["sbom"]
    );
}

#[tokio::test]
async fn a_clean_sbom_is_answered_with_an_empty_finding_list() {
    let (_engine, router) = emulator();
    for (case, submitted) in [
        (
            "a package with no advisory",
            bom(vec![component(
                "ref-clean",
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )]),
        ),
        ("an empty components array", bom(Vec::new())),
        (
            "no components member at all",
            json!({ "bomFormat": "CycloneDX", "specVersion": "1.5" }),
        ),
        (
            // CycloneDX 1.5 makes `version` OPTIONAL on a component (only
            // `type` and `name` are required), so an unversioned entry is
            // scanned and matches nothing — it is not a refusal.
            "a component with no version, beside a vulnerable one that has none",
            bom(vec![
                json!({ "type": "operating-system", "bom-ref": "ref-os", "name": "debian" }),
                json!({ "type": "library", "bom-ref": "ref-unversioned", "name": "lodash" }),
                json!({
                    "type": "library",
                    "bom-ref": "ref-purl-unversioned",
                    "name": "lodash",
                    "purl": "pkg:npm/lodash",
                }),
            ]),
        ),
    ] {
        let (status, response) = post_scan(&router, &json!({ "sbom": submitted })).await;
        assert_eq!(
            status, 200,
            "case {case:?}: a clean SBOM is not an error, got {status}: {response}"
        );
        assert_eq!(
            response["sbom"]["vulnerabilities"],
            json!([]),
            "case {case:?}: expected an empty vulnerabilities array, got {}",
            response["sbom"]["vulnerabilities"]
        );
    }
}

#[tokio::test]
async fn every_output_format_reports_the_same_finding_set() {
    let (_engine, router) = emulator();
    let submitted = bom(vec![
        component(
            "ref-log4j",
            "log4j-core",
            "2.14.1",
            "pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1",
        ),
        component("ref-lodash", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
        component("ref-clean", "left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
    ]);
    let expected: BTreeSet<String> = ["CVE-2021-23337", "CVE-2021-44228"]
        .into_iter()
        .map(str::to_string)
        .collect();

    // CYCLONE_DX_1_5 answers the submitted document augmented with the scan.
    {
        let request = json!({ "sbom": submitted, "outputFormat": "CYCLONE_DX_1_5" });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case CYCLONE_DX_1_5: expected HTTP 200, got {status}: {response}"
        );
        let ids: BTreeSet<String> = cyclonedx_ids(&response).into_iter().collect();
        assert_eq!(
            ids, expected,
            "case CYCLONE_DX_1_5: expected finding ids {expected:?}, got {ids:?}"
        );
        assert_eq!(
            response["sbom"]["bomFormat"].as_str(),
            Some("CycloneDX"),
            "case CYCLONE_DX_1_5: the CycloneDX form answers a CycloneDX BOM, got {}",
            response["sbom"]
        );
    }

    // An absent `outputFormat` answers the Inspector-native report, so its
    // matches are read where that form carries them.
    for requested in [None, Some("INSPECTOR")] {
        let mut request = json!({ "sbom": submitted });
        if let Some(format) = requested {
            request["outputFormat"] = json!(format);
        }
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case {requested:?}: expected HTTP 200, got {status}: {response}"
        );
        let ids: BTreeSet<String> = response["sbom"]["vulnerabilities"]
            .as_array()
            .cloned()
            .unwrap_or_default()
            .iter()
            .map(|f| f["id"].as_str().unwrap_or_default().to_string())
            .collect();
        assert_eq!(
            ids, expected,
            "case {requested:?}: expected finding ids {expected:?}, got {ids:?}"
        );
    }

    // INSPECTOR: a vulnerabilities list.
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": submitted, "outputFormat": "INSPECTOR" }),
    )
    .await;
    assert_eq!(status, 200, "INSPECTOR: expected HTTP 200, got {response}");
    let findings = response["sbom"]["vulnerabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ids: BTreeSet<String> = findings
        .iter()
        .map(|f| f["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids, expected,
        "INSPECTOR: expected finding ids {expected:?}, got {ids:?}"
    );
    assert_eq!(
        response["sbom"]["componentCount"].as_u64(),
        Some(3),
        "INSPECTOR: the report counts the scanned components, got {}",
        response["sbom"]
    );
    for finding in &findings {
        assert!(
            finding["inspectorScore"]
                .as_f64()
                .is_some_and(|s| (0.0..=10.0).contains(&s)),
            "INSPECTOR: every finding carries an Inspector score, got {finding}"
        );
        assert!(
            finding["vulnerablePackages"][0]["name"].as_str().is_some(),
            "INSPECTOR: every finding names its vulnerable package, got {finding}"
        );
        assert!(
            finding["remediation"]["recommendation"]["text"]
                .as_str()
                .is_some_and(|t| t.contains("Update")),
            "INSPECTOR: every finding with a fix carries a remediation, got {finding}"
        );
    }

    // INSPECTOR_ALT: the same matches, one node per matched advisory, in a list
    // (the form's own members are pinned in `scan_inspector_alt_form.rs`).
    let (status, response) = post_scan(
        &router,
        &json!({ "sbom": submitted, "outputFormat": "INSPECTOR_ALT" }),
    )
    .await;
    assert_eq!(
        status, 200,
        "INSPECTOR_ALT: expected HTTP 200, got {response}"
    );
    let nodes = response["sbom"]["vulnerabilities"]
        .as_array()
        .cloned()
        .unwrap_or_default();
    let ids: BTreeSet<String> = nodes
        .iter()
        .map(|node| node["id"].as_str().unwrap_or_default().to_string())
        .collect();
    assert_eq!(
        ids, expected,
        "INSPECTOR_ALT: expected finding ids {expected:?}, got {ids:?}"
    );
}

#[tokio::test]
async fn an_identical_scan_is_idempotent_and_a_different_one_is_recorded_separately() {
    let (engine, router) = emulator();
    let first = json!({
        "sbom": bom(vec![component(
            "ref-lodash",
            "lodash",
            "4.17.20",
            "pkg:npm/lodash@4.17.20",
        )]),
    });

    let (status, one) = post_scan(&router, &first).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {one}");
    let (status, two) = post_scan(&router, &first).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {two}");
    assert_eq!(
        one, two,
        "an @idempotent repeat must answer the identical report; got {one} then {two}"
    );

    {
        let store = engine.state::<ComAmazonawsInspectorscanInspectorscanState>();
        let store = store.lock();
        let scans = store.scans();
        assert_eq!(
            scans.len(),
            1,
            "a repeat of the same scan records no second scan, got {scans:?}"
        );
        assert_eq!(
            scans[0].finding_ids,
            vec!["CVE-2021-23337".to_string()],
            "the recorded scan lists the findings it reported, got {:?}",
            scans[0]
        );
        assert_eq!(
            scans[0].component_count, 1,
            "the recorded scan counts the components it scanned, got {:?}",
            scans[0]
        );
    }

    // A different document is a different scan, with a distinguishable identity.
    let second = json!({
        "sbom": bom(vec![component(
            "ref-urllib3",
            "urllib3",
            "1.26.4",
            "pkg:pypi/urllib3@1.26.4",
        )]),
        "outputFormat": "INSPECTOR",
    });
    let (status, response) = post_scan(&router, &second).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");

    let store = engine.state::<ComAmazonawsInspectorscanInspectorscanState>();
    let store = store.lock();
    let scans = store.scans();
    assert_eq!(
        scans.len(),
        2,
        "a different document is recorded as its own scan, got {scans:?}"
    );
    let ids: BTreeSet<&str> = scans.iter().map(|s| s.scan_id.as_str()).collect();
    assert_eq!(
        ids.len(),
        2,
        "two scans must have distinguishable ids, got {ids:?}"
    );
    assert_eq!(
        response["sbom"]["scanId"].as_str(),
        Some(scans[1].scan_id.as_str()),
        "the Inspector report names the recorded scan id, got {}",
        response["sbom"]
    );
}

#[tokio::test]
async fn the_scan_audit_store_stays_bounded_however_many_documents_are_scanned() {
    let (engine, router) = emulator();
    // Each document differs, so each is its own scan record; the store must
    // still cap at MAX_SCANS and keep the newest, evicting oldest-first.
    let total = MAX_SCANS + 37;
    for i in 0..total {
        let request = json!({
            "sbom": bom(vec![component(
                &format!("ref-{i}"),
                "left-pad",
                &format!("1.0.{i}"),
                &format!("pkg:npm/left-pad@1.0.{i}"),
            )]),
        });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 200,
            "case scan {i}: expected HTTP 200, got {status}: {response}"
        );
    }

    let store = engine.state::<ComAmazonawsInspectorscanInspectorscanState>();
    let store = store.lock();
    assert_eq!(
        store.scan_count(),
        MAX_SCANS,
        "case {total} distinct scans: the audit store must cap at {MAX_SCANS}, got {}",
        store.scan_count()
    );
    let scans = store.scans();
    assert_eq!(
        scans.len(),
        MAX_SCANS,
        "case {total} distinct scans: the eviction order must track the store, got {}",
        scans.len()
    );
    assert_eq!(
        store.next_serial(),
        u64::try_from(total).expect("the scan count fits a u64") + 1,
        "every distinct scan still advances the serial counter, got {}",
        store.next_serial()
    );
}

/// One row of the refusal table: a document that is not a CycloneDX 1.5 BOM.
///
/// What the refusal SAYS is pinned in `scan_sbom_refusals.rs`, which asserts
/// the service's wording per class of defect; this table asserts that each of
/// these documents is refused at all rather than scanned.
#[derive(Debug)]
struct RefusalCase {
    what: &'static str,
    sbom: Value,
}

#[tokio::test]
async fn a_document_that_is_not_a_cyclonedx_1_5_bom_is_refused() {
    let (_engine, router) = emulator();
    let cases = vec![
        RefusalCase {
            what: "a string document",
            sbom: json!("not a bom"),
        },
        RefusalCase {
            what: "an array document",
            sbom: json!([1, 2, 3]),
        },
        RefusalCase {
            what: "a number document",
            sbom: json!(7),
        },
        RefusalCase {
            what: "a missing bomFormat",
            sbom: json!({ "specVersion": "1.5", "components": [] }),
        },
        RefusalCase {
            what: "a non-CycloneDX bomFormat",
            sbom: json!({ "bomFormat": "SPDX", "specVersion": "1.5", "components": [] }),
        },
        RefusalCase {
            what: "a missing specVersion",
            sbom: json!({ "bomFormat": "CycloneDX", "components": [] }),
        },
        RefusalCase {
            what: "an earlier CycloneDX spec version",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.4", "components": [] }),
        },
        RefusalCase {
            what: "a later CycloneDX spec version",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.6", "components": [] }),
        },
        RefusalCase {
            what: "components that are not an array",
            sbom: json!({ "bomFormat": "CycloneDX", "specVersion": "1.5", "components": {} }),
        },
        RefusalCase {
            what: "a component that is not an object",
            sbom: bom(vec![json!("lodash@4.17.20")]),
        },
        RefusalCase {
            what: "a component with neither name/version nor purl",
            sbom: bom(vec![json!({ "type": "library" })]),
        },
        RefusalCase {
            // CycloneDX 1.5 makes `type` required on a component and constrains
            // it to the specification's `classification` enumeration.
            what: "a component carrying no type",
            sbom: bom(vec![json!({ "name": "lodash", "version": "4.17.20" })]),
        },
        RefusalCase {
            what: "a component whose type is not a CycloneDX component type",
            sbom: bom(vec![json!({
                "type": "package",
                "name": "lodash",
                "version": "4.17.20",
            })]),
        },
        RefusalCase {
            // `name` is required too, even when a purl could supply one.
            what: "a component carrying no name",
            sbom: bom(vec![json!({
                "type": "library",
                "version": "4.17.20",
                "purl": "pkg:npm/lodash@4.17.20",
            })]),
        },
        RefusalCase {
            // Every submitted `bom-ref` is retained for the uniqueness check
            // CycloneDX requires, so its length is bounded.
            what: "a component whose bom-ref is longer than a scan resolves",
            sbom: bom(vec![json!({
                "type": "library",
                "bom-ref": "r".repeat(513),
                "name": "lodash",
                "version": "4.17.20",
            })]),
        },
        RefusalCase {
            // A nested `components` member that is not an array would drop the
            // whole subtree from the count, the match and the report.
            what: "a nested components member that is not an array",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": { "name": "lodash" },
            })]),
        },
        RefusalCase {
            what: "a component carrying a malformed purl",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "lodash@4.17.20",
            })]),
        },
        RefusalCase {
            // The purl specification fixes the scheme to the lowercase literal
            // `pkg`, so an upper-case one is not a package URL.
            what: "a purl whose scheme is not the lowercase \"pkg\"",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "PKG:npm/lodash@4.17.20",
            })]),
        },
        RefusalCase {
            // A purl names a type and a name; a coordinate carrying only the
            // scheme and a type names no package at all.
            what: "a purl naming a type but no name",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:npm",
            })]),
        },
        RefusalCase {
            // A purl names a type and a name; a coordinate carrying only the
            // scheme names neither.
            what: "a purl carrying only the scheme",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:",
            })]),
        },
        RefusalCase {
            // The type is the first path segment, and purl types are
            // alphanumeric with `.`, `-` and `+`.
            what: "a purl whose type is not a purl type",
            sbom: bom(vec![json!({
                "type": "library",
                "name": "lodash",
                "version": "4.17.20",
                "purl": "pkg:np m/lodash@4.17.20",
            })]),
        },
        RefusalCase {
            // CycloneDX requires `bom-ref` to be unique within a document.
            what: "two components sharing one bom-ref",
            sbom: bom(vec![
                component("same", "lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
                component("same", "minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
            ]),
        },
        RefusalCase {
            what: "a nested component that is not an object",
            sbom: bom(vec![json!({
                "type": "application",
                "bom-ref": "ref-app",
                "name": "app",
                "version": "1.0.0",
                "components": ["lodash@4.17.20"],
            })]),
        },
        RefusalCase {
            what: "a document nested deeper than it can be parsed",
            sbom: deeply_nested_value(96),
        },
    ];

    for case in &cases {
        let (status, response) = post_scan(&router, &json!({ "sbom": case.sbom })).await;
        assert_eq!(
            status, 400,
            "case {:?}: expected HTTP 400, got {status}: {response}",
            case.what
        );
        assert_eq!(
            response["__type"].as_str(),
            Some("ValidationException"),
            "case {:?}: expected a modeled ValidationException, got {response}",
            case.what
        );
        assert!(
            response["message"]
                .as_str()
                .is_some_and(|m| m.starts_with("Input SBOM failed validation: ")),
            "case {:?}: a refusal must carry the service's own sentence, got {response}",
            case.what
        );
        // The sentence is the whole of what the refusal states; that it carries
        // nothing beside it is pinned in `scan_sbom_refusal_reason.rs`.
    }
}

#[tokio::test]
async fn the_enforced_component_limit_is_enforced_at_its_bound() {
    let (_engine, router) = emulator();
    // The limit the service enforces: exactly 5000 components are scanned and
    // one more is refused. (The `sbom` member's own documentation still says
    // "This format limits you to passing 2000 components before throwing a
    // ValidException error", which the service does not enforce.)
    for (count, expected_status) in [(1_usize, 200_u16), (2001, 200), (5000, 200), (5001, 400)] {
        let components: Vec<Value> = (0..count)
            .map(|i| {
                component(
                    &format!("ref-{i}"),
                    "left-pad",
                    "1.0.0",
                    "pkg:npm/left-pad@1.0.0",
                )
            })
            .collect();
        let (status, response) = post_scan(&router, &json!({ "sbom": bom(components) })).await;
        assert_eq!(
            status, expected_status,
            "case {count} components: expected HTTP {expected_status}, got {status}"
        );
        if expected_status == 400 {
            assert!(
                response["message"]
                    .as_str()
                    .is_some_and(|m| m.contains("5000")),
                "case {count} components: the refusal must name the enforced limit, got {response}"
            );
        }
    }
}

#[tokio::test]
async fn a_nested_assembly_is_scanned_and_counted_against_the_documented_limit() {
    let (_engine, router) = emulator();

    // CycloneDX 1.5 lets a component carry its own `components[]`. A vulnerable
    // package below the first level is reported like any other, with an
    // `affects[].ref` that resolves to that nested component.
    let request = json!({
        "sbom": bom(vec![nested_assembly(3)]),
        "outputFormat": "CYCLONE_DX_1_5",
    });
    let (status, response) = post_scan(&router, &request).await;
    assert_eq!(status, 200, "expected HTTP 200, got {status}: {response}");
    assert_eq!(
        cyclonedx_ids(&response),
        vec!["CVE-2021-44906".to_string()],
        "a nested component must be scanned, got {}",
        response["sbom"]["vulnerabilities"]
    );
    assert_eq!(
        response["sbom"]["vulnerabilities"][0]["affects"][0]["ref"].as_str(),
        // The walk assigns refs in document order, parents before children, so
        // the three assemblies take comp-1..comp-3 and the vulnerable leaf
        // below them takes comp-4.
        Some("comp-4"),
        "affects[].ref must name the nested component, got {}",
        response["sbom"]["vulnerabilities"][0]["affects"]
    );
    // The nested tree is preserved in the report, not flattened into it.
    assert_eq!(
        response["sbom"]["components"][0]["components"][0]["components"][0]["components"][0]
            ["name"]
            .as_str(),
        Some("minimist"),
        "the report preserves the submitted assembly tree, got {}",
        response["sbom"]["components"]
    );

    // Every entry of the tree counts against the enforced 5000-component limit,
    // not just the top level: 1 top-level assembly carrying 5000 children is
    // 5001 components.
    for (children, expected_status) in [(4998_usize, 200_u16), (4999, 200), (5000, 400)] {
        let leaves: Vec<Value> = (0..children)
            .map(|i| {
                component(
                    &format!("ref-leaf-{i}"),
                    "left-pad",
                    "1.0.0",
                    "pkg:npm/left-pad@1.0.0",
                )
            })
            .collect();
        let assembly = json!({
            "type": "application",
            "bom-ref": "ref-assembly",
            "name": "assembly",
            "version": "1.0.0",
            "components": leaves,
        });
        let (status, response) = post_scan(&router, &json!({ "sbom": bom(vec![assembly]) })).await;
        assert_eq!(
            status, expected_status,
            "case an assembly of {children} nested components ({} in total): expected HTTP \
             {expected_status}, got {status}: {response}",
            children + 1
        );
    }
}

#[tokio::test]
async fn a_component_tree_nested_too_deep_is_refused() {
    let (_engine, router) = emulator();
    let (status, response) = post_scan(&router, &json!({ "sbom": bom(vec![nested_assembly(20)]) }))
        .await;
    assert_eq!(
        status, 400,
        "a component tree deeper than a scan resolves is refused, got {status}: {response}"
    );
    assert!(
        response["message"]
            .as_str()
            .is_some_and(|message| message.contains("components")),
        "the refusal must name the components member, got {response}"
    );
    assert!(
        response["__type"].as_str() == Some("ValidationException"),
        "a refusal carries the ValidationException error code, got {response}"
    );
}

#[tokio::test]
async fn an_output_format_the_model_does_not_declare_is_refused() {
    let (_engine, router) = emulator();
    for requested in ["CYCLONE_DX_1_6", "cyclone_dx_1_5", "SPDX", ""] {
        let request = json!({
            "sbom": bom(vec![component(
                "ref-clean",
                "left-pad",
                "1.0.0",
                "pkg:npm/left-pad@1.0.0",
            )]),
            "outputFormat": requested,
        });
        let (status, response) = post_scan(&router, &request).await;
        assert_eq!(
            status, 400,
            "case {requested:?}: an undeclared outputFormat is refused, got {status}: {response}"
        );
    }
}

/// `version_in_range` is the one place an SBOM's package version meets an
/// advisory's bounds, so the forms a version arrives in are pinned here
/// directly: the HTTP table above exercises the same rule end-to-end.
#[test]
fn a_package_version_is_reduced_to_the_upstream_release_the_range_names() {
    // Each row: the version as an SBOM states it, the advisory interval, and
    // whether the version is inside it. The intervals are the ones
    // `src/state.rs::seed_vulnerability_db` states, plus the open-ended and
    // unbounded shapes an advisory can take; the versions span the forms an
    // SBOM writes them in (plain, `v`-prefixed, two- and four-component, a
    // Debian epoch with a distribution release suffix, and no release number
    // at all).
    const BELOW_4_17_21: VersionRange = VersionRange::before(ReleaseVersion::new(4, 17, 21));
    const BELOW_1_7_7: VersionRange = VersionRange::before(ReleaseVersion::new(1, 7, 7));
    const BELOW_5_4: VersionRange = VersionRange::before(ReleaseVersion::new(5, 4, 0));
    const GLIBC: VersionRange =
        VersionRange::between(ReleaseVersion::new(2, 34, 0), ReleaseVersion::new(2, 38, 1));
    const OPENSSL: VersionRange =
        VersionRange::between(ReleaseVersion::new(3, 0, 0), ReleaseVersion::new(3, 0, 7));
    const JACKSON: VersionRange =
        VersionRange::between(ReleaseVersion::new(2, 0, 0), ReleaseVersion::new(2, 12, 6));
    const LOG4J: VersionRange =
        VersionRange::between(ReleaseVersion::new(2, 0, 0), ReleaseVersion::new(2, 15, 0));
    // No fix has shipped: everything from the introducing release on.
    const UNFIXED: VersionRange = VersionRange::from(ReleaseVersion::new(1, 2, 0));
    // Neither bound: an advisory that bounds nothing matches nothing.
    const UNBOUNDED: VersionRange = VersionRange {
        from_including: None,
        before_excluding: None,
    };

    let cases: &[(&str, &VersionRange, bool)] = &[
        ("4.17.20", &BELOW_4_17_21, true),
        ("4.17.21", &BELOW_4_17_21, false),
        ("v1.7.0", &BELOW_1_7_7, true),
        ("v1.7.7", &BELOW_1_7_7, false),
        ("5.3", &BELOW_5_4, true),
        ("5.4", &BELOW_5_4, false),
        ("2.37", &GLIBC, true),
        ("2.33", &GLIBC, false),
        ("1:3.0.6-1ubuntu1", &OPENSSL, true),
        ("1:3.0.7-1ubuntu1", &OPENSSL, false),
        ("3.0.6", &OPENSSL, true),
        ("2.13.2.1", &JACKSON, false),
        ("2.9.10.8", &JACKSON, true),
        ("1.2.17", &LOG4J, false),
        ("2.0.0", &LOG4J, true),
        // The lower bound is inclusive and the upper bound exclusive, which is
        // how an advisory states "introduced in" and "fixed in".
        ("2.34.0", &GLIBC, true),
        ("2.38.1", &GLIBC, false),
        ("2.33.9", &GLIBC, false),
        ("2.38.0", &GLIBC, true),
        // An advisory with no fixed release yet affects every later release.
        ("1.2.0", &UNFIXED, true),
        ("99.0.0", &UNFIXED, true),
        ("1.1.9", &UNFIXED, false),
        // An advisory bounding nothing matches nothing, rather than every
        // release ever published.
        ("1.2.3", &UNBOUNDED, false),
        // No leading release number at all: nothing to compare, so no match.
        ("unknown", &BELOW_4_17_21, false),
        ("", &BELOW_4_17_21, false),
        ("latest", &BELOW_4_17_21, false),
        // A bound written with fewer numbers pads the rest with zero, so the
        // `5.4` bound excludes 5.4.0 and everything after it.
        ("5.4.1", &BELOW_5_4, false),
        ("5.3.9", &BELOW_5_4, true),
        // A pre-release is compared as its release, the approximation
        // `version_in_range` documents.
        ("5.4.0-rc1", &BELOW_5_4, false),
        ("5.3.0-rc1", &BELOW_5_4, true),
    ];
    for &(version, range, expected) in cases {
        let actual = version_in_range(version, range);
        assert_eq!(
            actual, expected,
            "case version {version:?} against range {range:?}: expected {expected}, got {actual}"
        );
    }
}

/// The `OutputFormat` the state keeps and the model's own `OutputFormat` enum
/// must enumerate exactly the same wire values: the handler resolves a request's
/// `outputFormat` through `OutputFormat::from_wire`, so a value the model
/// declares that the state cannot parse would be refused 400 (and one the state
/// accepts that the model does not declare would be served). This pins the two
/// lists together, against the generated
/// `crate::ops::types::OUTPUT_FORMAT_VALUES`.
#[test]
fn every_modeled_output_format_value_round_trips_through_the_state_enum() {
    let modeled = service_com_amazonaws_inspectorscan_inspectorscan::ops::types::OUTPUT_FORMAT_VALUES;
    for value in modeled {
        let parsed = OutputFormat::from_wire(value);
        assert!(
            parsed.is_some(),
            "case {value:?}: the model declares this outputFormat, so the state must parse it"
        );
        let round_tripped = parsed.map(OutputFormat::as_wire);
        assert_eq!(
            round_tripped,
            Some(*value),
            "case {value:?}: the wire spelling must survive the round trip, got {round_tripped:?}"
        );
    }
    // And nothing BEYOND the modeled list parses: the three variants the state
    // declares are exactly the three the model does.
    let parsed_count = modeled
        .iter()
        .filter_map(|value| OutputFormat::from_wire(value))
        .map(OutputFormat::as_wire)
        .collect::<BTreeSet<_>>()
        .len();
    assert_eq!(
        parsed_count,
        modeled.len(),
        "the state must have one variant per modeled value, got {parsed_count} for {modeled:?}"
    );
}

/// The same SBOM, written with its members in four different textual orders.
/// Only the ORDER differs: every document carries the same `bomFormat`,
/// `specVersion`, `version`, and the same single vulnerable component.
///
/// The rule: the identity of a scan is a property of the document's CONTENT, so
/// a repeat is recognised however the members are ordered on the wire. (Nothing
/// in JSON gives object members an order, and `serde_json`'s own serialization
/// is key-ordered only while its additive `preserve_order` feature is off, so
/// the handler canonicalizes before it digests.)
const SAME_DOCUMENT_DIFFERENT_ORDER: &[&str] = &[
    r#"{"sbom":{"bomFormat":"CycloneDX","specVersion":"1.5","version":1,"components":[{"type":"library","bom-ref":"ref-lodash","name":"lodash","version":"4.17.20","purl":"pkg:npm/lodash@4.17.20"}]}}"#,
    r#"{"sbom":{"components":[{"purl":"pkg:npm/lodash@4.17.20","version":"4.17.20","name":"lodash","bom-ref":"ref-lodash","type":"library"}],"version":1,"specVersion":"1.5","bomFormat":"CycloneDX"}}"#,
    r#"{"sbom":{"specVersion":"1.5","components":[{"name":"lodash","purl":"pkg:npm/lodash@4.17.20","type":"library","version":"4.17.20","bom-ref":"ref-lodash"}],"bomFormat":"CycloneDX","version":1}}"#,
    r#"{"sbom":{"version":1,"bomFormat":"CycloneDX","components":[{"bom-ref":"ref-lodash","type":"library","name":"lodash","version":"4.17.20","purl":"pkg:npm/lodash@4.17.20"}],"specVersion":"1.5"}}"#,
];

#[tokio::test]
async fn the_scan_identity_is_a_property_of_the_document_not_of_its_member_order() {
    let (engine, router) = emulator();

    let mut reports: Vec<Value> = Vec::new();
    for (case, body) in SAME_DOCUMENT_DIFFERENT_ORDER.iter().enumerate() {
        let (status, response) = post_scan_body(&router, body).await;
        assert_eq!(
            status, 200,
            "case {case}: expected HTTP 200, got {status}: {response}"
        );
        assert_eq!(
            cyclonedx_ids(&response),
            vec!["CVE-2021-23337".to_string()],
            "case {case}: the vulnerable component must be reported, got {response}"
        );
        reports.push(response);
    }

    for (case, report) in reports.iter().enumerate().skip(1) {
        assert_eq!(
            &reports[0], report,
            "case {case}: the same document in a different member order is the same \
             @idempotent scan and must answer the identical report"
        );
    }

    let store = engine.state::<ComAmazonawsInspectorscanInspectorscanState>();
    let store = store.lock();
    let scans = store.scans();
    assert_eq!(
        scans.len(),
        1,
        "every one of those requests carries the same document, so exactly one scan \
         is recorded, got {scans:?}"
    );
}

/// Component coordinates the invariant suite in `scan_invariants.rs` draws
/// from: vulnerable and clean, across the ecosystems
/// `src/state.rs::seed_vulnerability_db` covers.
pub(crate) const CATALOG: &[(&str, &str, &str)] = &[
    ("lodash", "4.17.20", "pkg:npm/lodash@4.17.20"),
    ("lodash", "4.17.21", "pkg:npm/lodash@4.17.21"),
    ("minimist", "1.2.5", "pkg:npm/minimist@1.2.5"),
    ("urllib3", "1.26.4", "pkg:pypi/urllib3@1.26.4"),
    ("PyYAML", "5.3", "pkg:pypi/pyyaml@5.3"),
    (
        "log4j-core",
        "2.14.1",
        "pkg:maven/org.apache.logging.log4j/log4j-core@2.14.1",
    ),
    (
        "jackson-databind",
        "2.9.10",
        "pkg:maven/com.fasterxml.jackson.core/jackson-databind@2.9.10",
    ),
    ("gin", "v1.7.0", "pkg:golang/github.com/gin-gonic/gin@v1.7.0"),
    ("glibc", "2.37", "pkg:rpm/redhat/glibc@2.37"),
    ("left-pad", "1.0.0", "pkg:npm/left-pad@1.0.0"),
];
