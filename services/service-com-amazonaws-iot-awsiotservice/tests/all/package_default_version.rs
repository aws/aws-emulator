// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of `UpdatePackage`, the in-place update of one software
//! package in the AWS IoT Software Package Catalog, read back through the `GetPackage`
//! reader that proves what it persisted.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a
//! restJson1 request signed for the `iot` signing name — so what is asserted is what an
//! AWS SDK client would see, not an internal function's return value.
//!
//! The rule under test: an update of an existing resource is a PATCH of the record the
//! create persisted — each member the request supplies replaces the stored one, a member
//! it leaves out keeps the value already stored, the resource's identity (its name and
//! its ARN) and its creation stamp never move, and its last-modified stamp moves to the
//! instant of the update; a member that names another resource is accepted only while
//! that resource exists, and otherwise refused with the operation's modelled not-found
//! error; two members the model documents as mutually exclusive are refused together
//! with a 400 `ValidationException`; an update whose resource the store does not hold is
//! refused with the modelled not-found error; and every refusal leaves the record
//! byte-for-byte as it was. Here that means `UpdatePackage` patches `description`,
//! promotes one of the package's own versions with `defaultVersionName` and removes that
//! designation with `unsetDefaultVersion`, answers 200 with an empty body, refuses a
//! `packageName` the catalog does not hold and a `defaultVersionName` the package does
//! not hold with `ResourceNotFoundException` (HTTP 404), and refuses a request naming a
//! default version while also unsetting it — the one combination the model documents as
//! impossible (`UpdatePackageRequest$defaultVersionName`: "You cannot name a
//! defaultVersion and set unsetDefaultVersion equal to true at the same time").
//!
//! The table spans what that rule generalises over — which members an update carries
//! (none, each one alone, all at once), the length of a supplied value at its published
//! bounds, the form and the number of the package's version names, which of several
//! versions is promoted, whether a stored designation survives, is replaced or is
//! removed, whether the update carries a `clientToken`, and the region the calls are
//! signed for.

use std::sync::Arc;
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The account id every emulated ARN carries (`emulator_server::EMULATOR_ACCOUNT_ID`).
const ACCOUNT_ID: &str = "000000000000";
/// The region the harness signs for unless a case names another.
const REGION: &str = "us-east-1";

/// A SigV4 `Authorization` header whose credential scope names the `iot` signing name
/// and `region`: the scope is how the router picks the claiming service, and how the
/// service picks the region its ARNs are scoped to. The signature itself is never
/// checked here — the harness turns request authentication off, exactly as an
/// unauthenticated emulator runs.
fn signed_for_iot(region: &str) -> String {
    format!(
        "AWS4-HMAC-SHA256 Credential=AKIDTESTIOT/20260101/{region}/iot/aws4_request, \
         SignedHeaders=host;x-amz-date, Signature=0000"
    )
}

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// A string member of the body, or `None` when it is absent or not a string.
    fn text(&self, name: &str) -> Option<String> {
        self.body
            .get(name)
            .and_then(Value::as_str)
            .map(str::to_string)
    }

    /// A timestamp member as the epoch seconds the restJson1 protocol renders a
    /// timestamp as.
    fn epoch(&self, name: &str) -> Option<f64> {
        self.body.get(name).and_then(Value::as_f64)
    }

    /// Whether the response carried no members at all — what an operation whose output
    /// shape binds none answers with.
    fn is_empty_payload(&self) -> bool {
        match &self.body {
            Value::Null => true,
            Value::Object(members) => members.is_empty(),
            _ => false,
        }
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it,
/// reached the way the production router reaches it.
struct Emulator {
    router: MultiServiceRouter,
}

impl Emulator {
    fn new() -> Self {
        let engine: Arc<engine::InMemoryEngine> = Arc::new(engine::InMemoryEngine::new());
        let mut router = MultiServiceRouter::new();
        router.set_request_authentication(false);
        router.register_service(EmulatedService::new(engine));
        Self { router }
    }

    async fn call(&self, method: &str, path: &str, body: Option<&Value>, region: &str) -> Reply {
        let payload = body.map_or_else(Vec::new, |value| {
            serde_json::to_vec(value).expect("a request body serializes")
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://iot.{region}.amazonaws.com{path}"))
            .header("authorization", signed_for_iot(region))
            .header("content-type", "application/json")
            .body(bytes::Bytes::from(payload))
            .expect("a well-formed request");

        let response = self.router.dispatch(request).await;
        let status = response.status().as_u16();
        let error_code = response
            .headers()
            .get("x-amzn-errortype")
            .and_then(|value| value.to_str().ok())
            .map(str::to_string);
        let bytes = response.into_body();
        let body = if bytes.is_empty() {
            Value::Null
        } else {
            serde_json::from_slice(&bytes).unwrap_or(Value::Null)
        };
        Reply {
            status,
            error_code,
            body,
        }
    }

    /// `CreatePackage`: `PUT /packages/{packageName}`.
    async fn create_package_in(&self, package: &str, body: &Value, region: &str) -> Reply {
        self.call("PUT", &format!("/packages/{package}"), Some(body), region)
            .await
    }

    async fn create_package(&self, package: &str, body: &Value) -> Reply {
        self.create_package_in(package, body, REGION).await
    }

    /// `CreatePackageVersion`: `PUT /packages/{packageName}/versions/{versionName}`.
    async fn create_version_in(&self, package: &str, version: &str, region: &str) -> Reply {
        self.call(
            "PUT",
            &format!("/packages/{package}/versions/{version}"),
            Some(&json!({})),
            region,
        )
        .await
    }

    async fn create_version(&self, package: &str, version: &str) -> Reply {
        self.create_version_in(package, version, REGION).await
    }

    /// `UpdatePackage`: `PATCH /packages/{packageName}`, with `clientToken` bound to the
    /// query string (`smithy_query(query="operation",operation="UpdatePackage")`).
    async fn update_package_in(
        &self,
        package: &str,
        body: &Value,
        client_token: Option<&str>,
        region: &str,
    ) -> Reply {
        let path = match client_token {
            Some(token) => format!("/packages/{package}?clientToken={token}"),
            None => format!("/packages/{package}"),
        };
        self.call("PATCH", &path, Some(body), region).await
    }

    async fn update_package(&self, package: &str, body: &Value) -> Reply {
        self.update_package_in(package, body, None, REGION).await
    }

    /// `GetPackage`: `GET /packages/{packageName}`.
    async fn get_package_in(&self, package: &str, region: &str) -> Reply {
        self.call("GET", &format!("/packages/{package}"), None, region)
            .await
    }

    async fn get_package(&self, package: &str) -> Reply {
        self.get_package_in(package, REGION).await
    }

    /// `GetPackageVersion`: `GET /packages/{packageName}/versions/{versionName}`.
    async fn get_version_in(&self, package: &str, version: &str, region: &str) -> Reply {
        self.call(
            "GET",
            &format!("/packages/{package}/versions/{version}"),
            None,
            region,
        )
        .await
    }

    async fn get_version(&self, package: &str, version: &str) -> Reply {
        self.get_version_in(package, version, REGION).await
    }
}

/// The ARN a package created in `region` must carry.
fn expected_arn(region: &str, package: &str) -> String {
    format!("arn:aws:iot:{region}:{ACCOUNT_ID}:package/{package}")
}

/// Epoch seconds now, for bounding a reported timestamp.
fn now_epoch_seconds() -> f64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("the clock is after the epoch")
        .as_secs_f64()
}

/// One update request: a label, the body the package is created with, the versions it
/// holds, the version already designated default before the update, the body the update
/// carries, the `clientToken` it carries, the region every call is signed for, and what
/// every later read must report.
struct Case {
    label: &'static str,
    create: Value,
    versions: Vec<String>,
    stored_default: Option<String>,
    update: Value,
    client_token: Option<String>,
    region: &'static str,
    description: Option<String>,
    default_version_name: Option<String>,
}

/// The table every case below is built from. It spans what the rule generalises over:
///
/// * which members the update carries — none at all, `description`,
///   `defaultVersionName` and `unsetDefaultVersion` each alone, and the legal
///   combinations of them;
/// * the length of a supplied value at its published bounds — `ResourceDescription` is
///   `length 0..1024` and `VersionName` `length 1..64`
///   (`smithy_query(query="operation",operation="UpdatePackage")`), so the rows cover
///   one character and the longest legal value of each;
/// * the form of a version name, over every character class its modelled
///   `pattern ^[a-zA-Z0-9-_.]+$` allows;
/// * how many versions the package holds, and which of them is promoted;
/// * whether a designation was already stored — so a row covers setting one where there
///   was none, replacing one, removing one, and leaving one alone;
/// * whether the update carries a `clientToken` — the `@idempotencyToken`,
///   `length 36..64, pattern ^\S{36,64}$`, so the rows cover absent, the shortest legal
///   token and the longest;
/// * the region the calls are signed for, which is the region the package's ARN is
///   scoped to and which an update must leave alone.
fn cases() -> Vec<Case> {
    let longest_version = "v".repeat(64);
    vec![
        Case {
            label: "no member at all: the stored description and designation survive",
            create: json!({ "description": "before" }),
            versions: vec!["1.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({}),
            client_token: None,
            region: REGION,
            description: Some("before".to_string()),
            default_version_name: Some("1.0.0".to_string()),
        },
        Case {
            label: "the description alone, leaving the stored designation",
            create: json!({ "description": "before" }),
            versions: vec!["1.0.0".to_string(), "2.0.0".to_string()],
            stored_default: Some("2.0.0".to_string()),
            update: json!({ "description": "after" }),
            client_token: None,
            region: REGION,
            description: Some("after".to_string()),
            default_version_name: Some("2.0.0".to_string()),
        },
        Case {
            label: "the longest legal description, onto a package created without one",
            create: json!({}),
            versions: Vec::new(),
            stored_default: None,
            update: json!({ "description": "d".repeat(1024) }),
            client_token: None,
            region: REGION,
            description: Some("d".repeat(1024)),
            default_version_name: None,
        },
        Case {
            label: "the shortest legal description",
            create: json!({ "description": "before" }),
            versions: Vec::new(),
            stored_default: None,
            update: json!({ "description": "d" }),
            client_token: None,
            region: REGION,
            description: Some("d".to_string()),
            default_version_name: None,
        },
        Case {
            label: "the default version alone, leaving the stored description",
            create: json!({ "description": "kept" }),
            versions: vec!["1.0.0".to_string()],
            stored_default: None,
            update: json!({ "defaultVersionName": "1.0.0" }),
            client_token: None,
            region: REGION,
            description: Some("kept".to_string()),
            default_version_name: Some("1.0.0".to_string()),
        },
        Case {
            label: "both settable members at once, onto a package carrying neither",
            create: json!({}),
            versions: vec!["1.0.0".to_string()],
            stored_default: None,
            update: json!({ "description": "both", "defaultVersionName": "1.0.0" }),
            client_token: None,
            region: REGION,
            description: Some("both".to_string()),
            default_version_name: Some("1.0.0".to_string()),
        },
        Case {
            label: "a one-character version name promoted",
            create: json!({}),
            versions: vec!["v".to_string()],
            stored_default: None,
            update: json!({ "defaultVersionName": "v" }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some("v".to_string()),
        },
        Case {
            label: "the longest legal version name promoted",
            create: json!({}),
            versions: vec![longest_version.clone()],
            stored_default: None,
            update: json!({ "defaultVersionName": longest_version.clone() }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some(longest_version),
        },
        Case {
            label: "a version name using every character class its pattern allows",
            create: json!({}),
            versions: vec!["Ver_01-aZ.9".to_string()],
            stored_default: None,
            update: json!({ "defaultVersionName": "Ver_01-aZ.9" }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some("Ver_01-aZ.9".to_string()),
        },
        Case {
            label: "one of several versions promoted",
            create: json!({ "description": "many versions" }),
            versions: vec![
                "1.0.0".to_string(),
                "1.0.1".to_string(),
                "2.0.0".to_string(),
            ],
            stored_default: None,
            update: json!({ "defaultVersionName": "1.0.1" }),
            client_token: None,
            region: REGION,
            description: Some("many versions".to_string()),
            default_version_name: Some("1.0.1".to_string()),
        },
        Case {
            label: "a stored designation replaced by another version",
            create: json!({}),
            versions: vec!["1.0.0".to_string(), "2.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({ "defaultVersionName": "2.0.0" }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some("2.0.0".to_string()),
        },
        Case {
            label: "a stored designation set again to the same version",
            create: json!({}),
            versions: vec!["1.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({ "defaultVersionName": "1.0.0" }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some("1.0.0".to_string()),
        },
        Case {
            label: "unsetDefaultVersion true removes the stored designation",
            create: json!({ "description": "kept" }),
            versions: vec!["1.0.0".to_string(), "2.0.0".to_string()],
            stored_default: Some("2.0.0".to_string()),
            update: json!({ "unsetDefaultVersion": true }),
            client_token: None,
            region: REGION,
            description: Some("kept".to_string()),
            default_version_name: None,
        },
        Case {
            label: "unsetDefaultVersion true on a package that has no designation",
            create: json!({}),
            versions: vec!["1.0.0".to_string()],
            stored_default: None,
            update: json!({ "unsetDefaultVersion": true }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: None,
        },
        Case {
            label: "unsetDefaultVersion false leaves the stored designation",
            create: json!({}),
            versions: vec!["1.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({ "unsetDefaultVersion": false }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some("1.0.0".to_string()),
        },
        Case {
            label: "unsetDefaultVersion false beside a named default version",
            create: json!({}),
            versions: vec!["1.0.0".to_string(), "2.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({ "defaultVersionName": "2.0.0", "unsetDefaultVersion": false }),
            client_token: None,
            region: REGION,
            description: None,
            default_version_name: Some("2.0.0".to_string()),
        },
        Case {
            label: "the description removed alongside an unset is left stored",
            create: json!({ "description": "stays" }),
            versions: vec!["1.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({ "description": "stays", "unsetDefaultVersion": true }),
            client_token: None,
            region: REGION,
            description: Some("stays".to_string()),
            default_version_name: None,
        },
        Case {
            label: "the shortest legal clientToken",
            create: json!({}),
            versions: vec!["1.0.0".to_string()],
            stored_default: None,
            update: json!({ "description": "tokenized", "defaultVersionName": "1.0.0" }),
            client_token: Some("t".repeat(36)),
            region: REGION,
            description: Some("tokenized".to_string()),
            default_version_name: Some("1.0.0".to_string()),
        },
        Case {
            label: "the longest legal clientToken",
            create: json!({}),
            versions: vec!["1.0.0".to_string()],
            stored_default: Some("1.0.0".to_string()),
            update: json!({ "unsetDefaultVersion": true }),
            client_token: Some("t".repeat(64)),
            region: REGION,
            description: None,
            default_version_name: None,
        },
        Case {
            label: "a package in another region keeps its ARN scoped to that region",
            create: json!({ "description": "elsewhere" }),
            versions: vec!["1.0.0".to_string()],
            stored_default: None,
            update: json!({ "description": "patched elsewhere", "defaultVersionName": "1.0.0" }),
            client_token: None,
            region: "eu-west-1",
            description: Some("patched elsewhere".to_string()),
            default_version_name: Some("1.0.0".to_string()),
        },
    ]
}

#[tokio::test]
async fn an_update_patches_the_members_it_carries_and_get_package_reports_them() {
    let emulator = Emulator::new();

    for (index, case) in cases().into_iter().enumerate() {
        let Case {
            label,
            create,
            versions,
            stored_default,
            update,
            client_token,
            region,
            description,
            default_version_name,
        } = case;
        let package = format!("patched-package-{index}");

        let created = emulator.create_package_in(&package, &create, region).await;
        assert_eq!(
            created.status, 200,
            "case {label:?}: the package must be creatable, got {created:?}"
        );
        for version in &versions {
            let created_version = emulator.create_version_in(&package, version, region).await;
            assert_eq!(
                created_version.status, 200,
                "case {label:?}: version {version:?} must be creatable, got {created_version:?}"
            );
        }
        if let Some(version) = &stored_default {
            let promoted = emulator
                .update_package_in(
                    &package,
                    &json!({ "defaultVersionName": version }),
                    None,
                    region,
                )
                .await;
            assert_eq!(
                promoted.status, 200,
                "case {label:?}: the stored designation must be settable, got {promoted:?}"
            );
        }

        let before_update = emulator.get_package_in(&package, region).await;
        assert_eq!(
            before_update.status, 200,
            "case {label:?}: the package must be readable before the update, got {before_update:?}"
        );
        let creation_date = before_update.epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the created package must report a creationDate, got {before_update:?}")
        });

        let before = now_epoch_seconds();
        let updated = emulator
            .update_package_in(&package, &update, client_token.as_deref(), region)
            .await;
        let after = now_epoch_seconds();

        assert_eq!(
            updated.status, 200,
            "case {label:?}: an update must answer 200, got {updated:?}"
        );
        // `UpdatePackageResponse` binds no members.
        assert!(
            updated.is_empty_payload(),
            "case {label:?}: an update must answer an empty payload, got {updated:?}"
        );

        let read = emulator.get_package_in(&package, region).await;
        assert_eq!(
            read.status, 200,
            "case {label:?}: the package must still be readable, got {read:?}"
        );
        assert_eq!(
            read.text("description"),
            description,
            "case {label:?}: expected description {description:?}, got {read:?}"
        );
        assert_eq!(
            read.text("defaultVersionName"),
            default_version_name,
            "case {label:?}: expected defaultVersionName {default_version_name:?}, got {read:?}"
        );
        // The identity an update never moves: the name it is keyed by and the ARN the
        // create minted for the region it was created in.
        assert_eq!(
            read.text("packageName"),
            Some(package.clone()),
            "case {label:?}: expected packageName {package:?}, got {read:?}"
        );
        assert_eq!(
            read.text("packageArn"),
            Some(expected_arn(region, &package)),
            "case {label:?}: expected {:?}, got {read:?}",
            expected_arn(region, &package)
        );
        let read_creation = read.epoch("creationDate").unwrap_or_else(|| {
            panic!("case {label:?}: the reader must report a creationDate, got {read:?}")
        });
        assert!(
            (read_creation - creation_date).abs() < f64::EPSILON,
            "case {label:?}: the creation date must not move, expected {creation_date}, \
             got {read_creation}"
        );

        // "The date that the package was last updated": this update is it.
        let last_modified = read.epoch("lastModifiedDate").unwrap_or_else(|| {
            panic!("case {label:?}: the reader must report a lastModifiedDate, got {read:?}")
        });
        assert!(
            last_modified >= before.floor() && last_modified <= after.ceil(),
            "case {label:?}: lastModifiedDate {last_modified} must lie in [{before}, {after}]"
        );
        assert!(
            last_modified >= read_creation,
            "case {label:?}: lastModifiedDate {last_modified} must not precede the \
             creationDate {read_creation}"
        );

        // A designation names a version; removing or replacing it never removes the
        // version itself.
        for version in &versions {
            let read_version = emulator.get_version_in(&package, version, region).await;
            assert_eq!(
                read_version.status, 200,
                "case {label:?}: version {version:?} must survive the update, got \
                 {read_version:?}"
            );
        }

        // Reading does not change what a later caller sees.
        let again = emulator.get_package_in(&package, region).await;
        assert_eq!(
            again.body, read.body,
            "case {label:?}: a second read must report the same package, got {again:?}"
        );
    }
}

#[tokio::test]
async fn the_designation_follows_the_last_update_across_a_packages_versions() {
    let emulator = Emulator::new();
    let package = "promotion-package";
    let versions = ["1.0.0", "1.0.1", "2.0.0"];

    assert_eq!(
        emulator.create_package(package, &json!({})).await.status,
        200,
        "the package must be creatable"
    );
    for version in versions {
        assert_eq!(
            emulator.create_version(package, version).await.status,
            200,
            "case {version:?}: the version must be creatable"
        );
    }
    let fresh = emulator.get_package(package).await;
    assert_eq!(
        fresh.text("defaultVersionName"),
        None,
        "a package nobody promoted a version in has no default version, got {fresh:?}"
    );

    // Each update is an assignment, so the designation follows the LAST one — set,
    // replaced, removed and set again, each step read back through `GetPackage`, and a
    // step repeated changes nothing.
    let steps: Vec<(Value, Option<&str>)> = vec![
        (json!({ "defaultVersionName": "1.0.0" }), Some("1.0.0")),
        (json!({ "defaultVersionName": "1.0.0" }), Some("1.0.0")),
        (json!({ "defaultVersionName": "2.0.0" }), Some("2.0.0")),
        (json!({ "unsetDefaultVersion": true }), None),
        (json!({ "unsetDefaultVersion": true }), None),
        (json!({ "defaultVersionName": "1.0.1" }), Some("1.0.1")),
        (json!({ "unsetDefaultVersion": false }), Some("1.0.1")),
        (json!({}), Some("1.0.1")),
    ];
    for (body, expected) in steps {
        let updated = emulator.update_package(package, &body).await;
        assert_eq!(
            updated.status, 200,
            "case {body:?}: the update must answer 200, got {updated:?}"
        );
        let read = emulator.get_package(package).await;
        assert_eq!(
            read.text("defaultVersionName").as_deref(),
            expected,
            "case {body:?}: expected defaultVersionName {expected:?}, got {read:?}"
        );
        // Every version is still a version of the package, whichever one is default.
        for version in versions {
            let read_version = emulator.get_version(package, version).await;
            assert_eq!(
                read_version.status, 200,
                "case {body:?}: version {version:?} must still exist, got {read_version:?}"
            );
        }
    }
}

#[tokio::test]
async fn naming_a_default_version_while_unsetting_it_is_refused() {
    let emulator = Emulator::new();
    let package = "exclusive-package";

    assert_eq!(
        emulator
            .create_package(package, &json!({ "description": "stored" }))
            .await
            .status,
        200,
        "the package must be creatable"
    );
    assert_eq!(
        emulator.create_version(package, "1.0.0").await.status,
        200,
        "the version must be creatable"
    );
    assert_eq!(
        emulator
            .update_package(package, &json!({ "defaultVersionName": "1.0.0" }))
            .await
            .status,
        200,
        "the designation must be settable"
    );
    let before = emulator.get_package(package).await;

    // The two members give the same field opposite instructions, so the request is
    // refused whatever else it carries — including a version the package does not even
    // hold, because the request itself is what is wrong.
    let attempts = [
        json!({ "defaultVersionName": "1.0.0", "unsetDefaultVersion": true }),
        json!({ "defaultVersionName": "2.0.0", "unsetDefaultVersion": true }),
        json!({ "defaultVersionName": "v".repeat(64), "unsetDefaultVersion": true }),
        json!({
            "description": "and a description",
            "defaultVersionName": "1.0.0",
            "unsetDefaultVersion": true,
        }),
    ];
    for attempt in &attempts {
        let refused = emulator.update_package(package, attempt).await;
        assert_eq!(
            refused.status, 400,
            "case {attempt:?}: naming a default version while unsetting it must be \
             refused 400, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ValidationException"),
            "case {attempt:?}: expected ValidationException, got {refused:?}"
        );
        let read = emulator.get_package(package).await;
        assert_eq!(
            read.body, before.body,
            "case {attempt:?}: a refused update must change nothing, got {read:?}"
        );
    }

    // Either member on its own, and a named version beside an unset that asks for no
    // removal, are all accepted.
    for (body, expected) in [
        (
            json!({ "defaultVersionName": "1.0.0", "unsetDefaultVersion": false }),
            Some("1.0.0"),
        ),
        (json!({ "unsetDefaultVersion": true }), None),
        (json!({ "defaultVersionName": "1.0.0" }), Some("1.0.0")),
    ] {
        let accepted = emulator.update_package(package, &body).await;
        assert_eq!(
            accepted.status, 200,
            "case {body:?}: the update must be accepted, got {accepted:?}"
        );
        let read = emulator.get_package(package).await;
        assert_eq!(
            read.text("defaultVersionName").as_deref(),
            expected,
            "case {body:?}: expected defaultVersionName {expected:?}, got {read:?}"
        );
    }
}

#[tokio::test]
async fn an_update_of_a_package_or_version_the_catalog_does_not_hold_is_not_found() {
    let emulator = Emulator::new();

    // Package names of every legal form — `PackageName` is `length 1..128,
    // pattern ^[a-zA-Z0-9-_.]+$` — with none of them created.
    let names = [
        "a".to_string(),
        "never-created".to_string(),
        "Never_Created.1".to_string(),
        "n".repeat(128),
    ];
    for package in &names {
        let refused = emulator
            .update_package(package, &json!({ "description": "nothing to patch" }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {package:?}: an update of an uncreated package must be refused 404, \
             got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {package:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        let read = emulator.get_package(package).await;
        assert_eq!(
            read.status, 404,
            "case {package:?}: the refused update must record nothing, got {read:?}"
        );
    }

    // A package that exists holds no version yet, so every version name it could be
    // asked to promote is a 404 — and the package is left exactly as it was.
    let package = &names[1];
    assert_eq!(
        emulator
            .create_package(package, &json!({ "description": "stored" }))
            .await
            .status,
        200,
        "the package must be creatable"
    );
    let before = emulator.get_package(package).await;
    for version in ["a", "1.0.0", "Ver_01-aZ.9", &"v".repeat(64)] {
        let refused = emulator
            .update_package(package, &json!({ "defaultVersionName": version }))
            .await;
        assert_eq!(
            refused.status, 404,
            "case {version:?}: promoting a version the package does not hold must be \
             refused 404, got {refused:?}"
        );
        assert_eq!(
            refused.error_code.as_deref(),
            Some("ResourceNotFoundException"),
            "case {version:?}: expected ResourceNotFoundException, got {refused:?}"
        );
        let read = emulator.get_package(package).await;
        assert_eq!(
            read.body, before.body,
            "case {version:?}: a refused update must change nothing, got {read:?}"
        );
    }

    // Creating exactly one of those versions makes exactly that one promotable.
    assert_eq!(
        emulator.create_version(package, "1.0.0").await.status,
        200,
        "the version must be creatable"
    );
    for version in ["a", "1.0.0", "Ver_01-aZ.9", &"v".repeat(64)] {
        let updated = emulator
            .update_package(package, &json!({ "defaultVersionName": version }))
            .await;
        let expected = if version == "1.0.0" { 200 } else { 404 };
        assert_eq!(
            updated.status, expected,
            "case {version:?}: expected {expected}, got {updated:?}"
        );
    }
    let read = emulator.get_package(package).await;
    assert_eq!(
        read.text("defaultVersionName").as_deref(),
        Some("1.0.0"),
        "only the version the package holds may be promoted, got {read:?}"
    );
}

#[tokio::test]
async fn an_update_touches_only_the_package_it_names() {
    let emulator = Emulator::new();

    // The same version name in two packages is two records: an update names ONE
    // package, so every other record is untouched.
    let packages = ["left-package", "right-package"];
    for package in packages {
        assert_eq!(
            emulator
                .create_package(package, &json!({ "description": package }))
                .await
                .status,
            200,
            "case {package:?}: the package must be creatable"
        );
        assert_eq!(
            emulator.create_version(package, "1.0.0").await.status,
            200,
            "case {package:?}: the version must be creatable"
        );
    }
    let mut before = Vec::new();
    for package in packages {
        before.push((package, emulator.get_package(package).await.body));
    }

    let updated = emulator
        .update_package(
            packages[0],
            &json!({ "description": "patched", "defaultVersionName": "1.0.0" }),
        )
        .await;
    assert_eq!(
        updated.status, 200,
        "the named package must be updatable, got {updated:?}"
    );

    for (package, recorded) in before {
        let read = emulator.get_package(package).await;
        if package == packages[0] {
            assert_eq!(
                read.text("description").as_deref(),
                Some("patched"),
                "case {package:?}: the named package must be patched, got {read:?}"
            );
            assert_eq!(
                read.text("defaultVersionName").as_deref(),
                Some("1.0.0"),
                "case {package:?}: the named package must carry the designation, got {read:?}"
            );
        } else {
            assert_eq!(
                read.body, recorded,
                "case {package:?}: an update of another package must change nothing \
                 here, got {read:?}"
            );
        }
    }
}
