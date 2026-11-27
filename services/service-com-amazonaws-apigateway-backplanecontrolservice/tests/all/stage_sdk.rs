// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for `GetSdk`: the client SDK a deployed stage is generated into, read
//! back through the wire path a client downloads it over.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. [`Gateway`] and the `CreateRestApi`/`CreateResource`/
//! `PutMethod`/`CreateDeployment` helpers are this crate's existing harness.
//!
//! The tests assert the RULE over sets of inputs rather than one recorded call: a `GetSdk`
//! call answers the ZIP archive of a client project generated for the DEPLOYMENT the named
//! stage serves, in the language `sdkType` names, configured by the request's `parameters`,
//! with every value of the project taken from the stored records. The rows differ along what
//! that rule generalises over — every generator `GetSdkRequest$sdkType` documents, both
//! spellings of a configuration property's name, a configured value and the default the
//! property's own description states for it, both build systems the `java` property
//! publishes, the number and nesting of resource paths, the verbs on a path (the `ANY`
//! catch-all included), a method with an `operationName` and one without, path, query string
//! and header request parameters, a method that requires an api key, and the ways an
//! `sdkType`, a required parameter, an api id and a stage name can miss.

use std::collections::{BTreeMap, BTreeSet};

use bytes::Bytes;
use percent_encoding::{utf8_percent_encode, AsciiSet, NON_ALPHANUMERIC};
use serde_json::{json, Value};

use crate::execute_api_data_plane::{deploy, put_integration, put_method};
use crate::rest_api_lifecycle::{create_resource, string_member, Gateway};

/// The characters a query string name or value is percent-encoded against: everything
/// outside RFC 3986's unreserved set, so a configured value carrying a space or a dot
/// reaches the service as the one value it is.
const QUERY: &AsciiSet = &NON_ALPHANUMERIC
    .remove(b'-')
    .remove(b'.')
    .remove(b'_')
    .remove(b'~');

/// The generators `GetSdkRequest$sdkType` documents ("Currently java, javascript, android,
/// objectivec (for iOS), swift (for iOS), and ruby are supported"), each with the
/// `parameters` its own required properties ask for.
///
/// Source: `smithy_query(query="doc", shape="GetSdkRequest", member="sdkType")` for the set,
/// and for the required properties `GetSdkTypes`' published `configurationProperties` (the
/// `java` and `ruby` rows, which `tests/all/sdk_types.rs` pins) and
/// `smithy_query(query="doc", shape="GetSdkRequest", member="parameters")` for the rest:
/// "For sdkType of objectivec or swift, a parameter named classPrefix is required. For
/// sdkType of android, parameters named groupId, artifactId, artifactVersion, and
/// invokerPackage are required. For sdkType of java, parameters named serviceName and
/// javaPackageName are required."
type GeneratorRow = (&'static str, &'static [(&'static str, &'static str)]);

/// The `java` configuration, under the property names the catalog publishes.
const JAVA_PARAMETERS: &[(&str, &str)] = &[
    ("service.name", "Pet Store"),
    ("java.package-name", "com.petstore.client"),
];

/// The same `java` configuration under the camelCase spellings
/// `GetSdkRequest$parameters` documents: the two spellings name ONE property each.
const JAVA_CAMEL_CASE_PARAMETERS: &[(&str, &str)] = &[
    ("serviceName", "Pet Store"),
    ("javaPackageName", "com.petstore.client"),
];

/// Every generator and the configuration each case below generates with.
const GENERATORS: &[GeneratorRow] = &[
    ("java", JAVA_PARAMETERS),
    ("javascript", &[]),
    (
        "android",
        &[
            ("groupId", "com.petstore"),
            ("artifactId", "petstore-android"),
            ("artifactVersion", "2.1.0"),
            ("invokerPackage", "com.petstore.invoker"),
        ],
    ),
    ("objectivec", &[("classPrefix", "PS")]),
    ("swift", &[("classPrefix", "PS")]),
    ("ruby", &[("service.name", "Pet Store")]),
];

/// The entry paths each generator's project is made of, for the configurations above.
const PROJECTS: &[(&str, &[&str])] = &[
    (
        "java",
        &[
            "README.md",
            "pom.xml",
            "src/main/java/com/petstore/client/PetStoreClient.java",
            "src/main/java/com/petstore/client/PetStoreClientImpl.java",
        ],
    ),
    ("javascript", &["README.md", "index.js", "package.json"]),
    (
        "android",
        &[
            "README.md",
            "build.gradle",
            "settings.gradle",
            "src/main/java/com/petstore/invoker/PetstoreAndroidClient.java",
        ],
    ),
    ("objectivec", &["PSClient.h", "PSClient.m", "README.md"]),
    (
        "swift",
        &["Package.swift", "README.md", "Sources/PSClient/PSClient.swift"],
    ),
    ("ruby", &["README.md", "lib/pet_store.rb", "pet_store.gemspec"]),
];

/// The api's own `version`, so the attachment's file name — which is named for the version
/// of the definition the SDK was generated from — is a stored value rather than a clock
/// reading.
const API_VERSION: &str = "1.0";

/// The stage every case generates an SDK of.
const STAGE: &str = "prod";

/// The region the harness's requests are signed for, which is the region the api is created
/// in and so the region its invoke endpoint names.
const REGION: &str = "us-east-1";

/// The deployed methods of [`deployed_api`], in the order a generated client carries them:
/// the client method's name, its verb and its resource path.
const DEPLOYED: &[(&str, &str, &str)] = &[
    ("get", "GET", "/"),
    ("listPets", "GET", "/pets"),
    ("createPet", "POST", "/pets"),
    ("anyPetsPetId", "ANY", "/pets/{petId}"),
    ("getPetsPetId", "GET", "/pets/{petId}"),
];

/// One row of the missing-parameter table: the generator, the `parameters` the request
/// carries, and every property the refusal has to name, in the order it names them.
type MissingRow<'a> = (&'a str, &'a [(&'a str, &'a str)], &'a [&'a str]);

/// One row of the resolution-order table: the case, the `restApiId` the request names, its
/// `sdkType` and the `parameters` it carries.
type PrecedenceRow<'a> = (&'a str, &'a str, &'a str, &'a [(&'a str, &'a str)]);

/// One answered `GetSdk`: the status, the error code of a refusal, the two headers the
/// operation binds, and the body bytes.
struct Sdk {
    status: u16,
    error: Option<String>,
    content_type: Option<String>,
    content_disposition: Option<String>,
    body: Vec<u8>,
}

impl Sdk {
    /// The refusal's `message`, for an answer that is one.
    fn message(&self) -> String {
        serde_json::from_slice::<Value>(&self.body)
            .ok()
            .and_then(|body| {
                body.get("message")
                    .and_then(Value::as_str)
                    .map(str::to_owned)
            })
            .unwrap_or_default()
    }

    /// The files of the generated project, by archive entry path, with the case named on
    /// failure.
    fn entries(&self, case: &str) -> BTreeMap<String, String> {
        assert_eq!(
            self.status,
            200,
            "case {case}: expected 200 from GetSdk, got {} with {:?}",
            self.status,
            String::from_utf8_lossy(&self.body)
        );
        let mut archive = zip::ZipArchive::new(std::io::Cursor::new(self.body.clone()))
            .unwrap_or_else(|error| panic!("case {case}: expected a zip archive, got {error}"));
        let mut files: BTreeMap<String, String> = BTreeMap::new();
        for index in 0..archive.len() {
            let mut entry = archive
                .by_index(index)
                .unwrap_or_else(|error| panic!("case {case}: expected entry {index}, got {error}"));
            let name = entry
                .name()
                .unwrap_or_else(|error| {
                    panic!("case {case}: expected entry {index}'s name, got {error}")
                })
                .to_string();
            let mut text = String::new();
            std::io::Read::read_to_string(&mut entry, &mut text).unwrap_or_else(|error| {
                panic!("case {case}: expected the text of {name}, got {error}")
            });
            files.insert(name, text);
        }
        files
    }

    /// Every file of the project as one text, which is what a language-independent
    /// assertion about the generated sources reads.
    fn project(&self, case: &str) -> String {
        self.entries(case).into_values().collect::<Vec<String>>().join("\n")
    }
}

/// `GetSdk` for `stage` of `api`, through the raw wire path so the response HEADERS
/// (`Content-Type`, `Content-Disposition`) are visible beside the payload.
///
/// `parameters` are the query string parameters the `parameters` map is bound to.
async fn get_sdk(
    gateway: &Gateway,
    api: &str,
    stage: &str,
    sdk_type: &str,
    parameters: &[(&str, &str)],
) -> Sdk {
    let query: Vec<String> = parameters
        .iter()
        .map(|(name, value)| {
            format!(
                "{}={}",
                utf8_percent_encode(name, QUERY),
                utf8_percent_encode(value, QUERY)
            )
        })
        .collect();
    let path = format!("/restapis/{api}/stages/{stage}/sdks/{sdk_type}");
    let uri = if query.is_empty() {
        path
    } else {
        format!("{path}?{}", query.join("&"))
    };
    let request = http::Request::builder()
        .method("GET")
        .uri(uri)
        .header("host", "apigateway.us-east-1.amazonaws.com")
        .header(
            "authorization",
            "AWS4-HMAC-SHA256 \
             Credential=AKIAIOSFODNN7EXAMPLE/20260101/us-east-1/apigateway/aws4_request, \
             SignedHeaders=host, Signature=0",
        )
        .body(Bytes::new())
        .expect("the test request is well formed");
    let response = gateway.send(request).await;
    let status = response.status().as_u16();
    let header = |name: &str| -> Option<String> {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .map(str::to_owned)
    };
    let error = header("x-amzn-errortype");
    let content_type = header("content-type");
    let content_disposition = header("content-disposition");
    Sdk {
        status,
        error,
        content_type,
        content_disposition,
        body: response.into_body().to_vec(),
    }
}

/// `CreateRestApi` for `name` at [`API_VERSION`], answering the api id and its root resource
/// id.
///
/// This suite creates its own api rather than reusing the shared helper because the
/// attachment's file name is the VERSION of the definition, and an api created without one
/// is named for its deployment instead.
async fn create_versioned_api(gateway: &Gateway, name: &str) -> (String, String) {
    let answer = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": name, "version": API_VERSION })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case CreateRestApi({name}): expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    let api = string_member(&answer.body, "id", "CreateRestApi");
    let root = string_member(&answer.body, "rootResourceId", "CreateRestApi");
    (api, root)
}

/// An api whose deployed stage carries the methods of [`DEPLOYED`]: nested paths, a path
/// template parameter, the `ANY` catch-all, a method with an `operationName` and one
/// without, query string and header request parameters, a method that requires an api key,
/// and a method whose integration names the media type a body is sent as.
///
/// Answers the api id, the deployment id and the root resource id.
async fn deployed_api(gateway: &Gateway) -> (String, String, String) {
    let (api, root) = create_versioned_api(gateway, "sdk-pet-store").await;

    // The root resource: a method whose name the generator derives from the verb alone.
    put_method(gateway, &api, &root, "GET", json!({ "authorizationType": "NONE" })).await;
    put_integration(gateway, &api, &root, "GET", json!({ "type": "MOCK" })).await;

    let pets = string_member(
        &create_resource(gateway, &api, &root, "pets").await,
        "id",
        "CreateResource(pets)",
    );
    put_method(
        gateway,
        &api,
        &pets,
        "GET",
        json!({
            "authorizationType": "NONE",
            "operationName": "listPets",
            "requestParameters": {
                "method.request.querystring.type": true,
                "method.request.querystring.page": false,
                "method.request.header.x-trace": false,
            },
        }),
    )
    .await;
    put_integration(gateway, &api, &pets, "GET", json!({ "type": "MOCK" })).await;
    put_method(
        gateway,
        &api,
        &pets,
        "POST",
        json!({
            "authorizationType": "AWS_IAM",
            "operationName": "createPet",
            "apiKeyRequired": true,
        }),
    )
    .await;
    // The media type a generated client sends a body as comes from the records: this
    // integration's request template is keyed by one.
    put_integration(
        gateway,
        &api,
        &pets,
        "POST",
        json!({
            "type": "MOCK",
            "requestTemplates": { "application/xml": "<pet/>" },
        }),
    )
    .await;

    let one_pet = string_member(
        &create_resource(gateway, &api, &pets, "{petId}").await,
        "id",
        "CreateResource({petId})",
    );
    put_method(
        gateway,
        &api,
        &one_pet,
        "GET",
        json!({
            "authorizationType": "NONE",
            "requestParameters": { "method.request.path.petId": true },
        }),
    )
    .await;
    put_integration(gateway, &api, &one_pet, "GET", json!({ "type": "MOCK" })).await;
    put_method(
        gateway,
        &api,
        &one_pet,
        "ANY",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    // Every method carries the integration a deployment requires of it: `MOCK`, the type
    // that needs no backend.
    put_integration(gateway, &api, &one_pet, "ANY", json!({ "type": "MOCK" })).await;

    let deployment = deploy(gateway, &api, STAGE).await;
    (api, deployment, root)
}

/// The stage's invoke endpoint, which every generated client is bound to.
fn endpoint(api: &str) -> String {
    format!("https://{api}.execute-api.{REGION}.amazonaws.com/{STAGE}")
}

/// Assert the generated project states every deployed method and the endpoint it calls, and
/// nothing the records do not carry.
fn assert_project_describes_the_deployment(project: &str, api: &str, deployment: &str, case: &str) {
    assert!(
        project.contains(&endpoint(api)),
        "case {case}: expected the project to call {}, got {project}",
        endpoint(api)
    );
    assert!(
        project.contains(deployment),
        "case {case}: expected the project to name the deployment {deployment}, got {project}"
    );
    for (method, verb, path) in DEPLOYED {
        assert!(
            project.contains(&format!("{verb} {path}")),
            "case {case}: expected the project to carry the operation {verb} {path}, got {project}"
        );
        assert!(
            project.contains(method),
            "case {case}: expected the project to carry a {method} method, got {project}"
        );
    }
    // The api key header is in the project because one deployed method requires a key.
    assert!(
        project.contains("x-api-key"),
        "case {case}: expected the project to send an api key, got {project}"
    );
    // Nothing the api does not carry: a path never created is in no generated client.
    assert!(
        !project.contains("/undeployed"),
        "case {case}: expected no undeployed path in the project, got {project}"
    );
}

/// Assert every entry of a generated archive names a file INSIDE the project: a relative
/// path with no parent segment, so extracting one writes nothing outside the directory it is
/// extracted into.
fn assert_entries_are_inside_the_project(entries: &BTreeMap<String, String>, case: &str) {
    for path in entries.keys() {
        assert!(
            !path.starts_with('/')
                && !path.starts_with('\\')
                && !path.split('/').any(|segment| segment == ".." || segment.is_empty()),
            "case {case}: expected {path:?} to name a file of the project"
        );
    }
}

/// The `Operations` section of a generated README, whose method names are the same for every
/// generator.
fn readme_operations(entries: &BTreeMap<String, String>, case: &str) -> Vec<String> {
    let readme = entries
        .get("README.md")
        .unwrap_or_else(|| panic!("case {case}: expected a README.md, got {:?}", entries.keys()));
    readme
        .lines()
        .filter_map(|line| line.strip_prefix("* `"))
        .filter_map(|line| line.split('`').next())
        .map(str::to_owned)
        .collect()
}

#[tokio::test]
async fn every_documented_generator_answers_the_project_of_the_deployment_the_stage_serves() {
    let gateway = Gateway::new();
    let (api, deployment, _) = deployed_api(&gateway).await;

    for (sdk_type, parameters) in GENERATORS {
        let case = format!("GetSdk({sdk_type})");
        let answer = get_sdk(&gateway, &api, STAGE, sdk_type, parameters).await;
        assert_eq!(
            answer.status,
            200,
            "case {case}: expected 200, got {} with {:?}",
            answer.status,
            String::from_utf8_lossy(&answer.body)
        );
        // The artifact is the FILE it is: a zip archive, reported as one, served as an
        // attachment named for the generator and the version of the definition.
        assert_eq!(
            answer.content_type.as_deref(),
            Some("application/zip"),
            "case {case}: expected the archive's media type, got {:?}",
            answer.content_type
        );
        assert_eq!(
            answer.content_disposition.as_deref(),
            Some(format!("attachment; filename=\"{sdk_type}_{API_VERSION}.zip\"").as_str()),
            "case {case}: expected the attachment named for the generator and the version, \
             got {:?}",
            answer.content_disposition
        );
        assert!(
            answer.body.starts_with(b"PK"),
            "case {case}: expected a zip archive's own first bytes, got {:?}",
            &answer.body[..answer.body.len().min(8)]
        );

        let entries = answer.entries(&case);
        let expected = PROJECTS
            .iter()
            .find(|(generator, _)| generator == sdk_type)
            .map(|(_, paths)| *paths)
            .unwrap_or_else(|| panic!("case {case}: the table names every generator's project"));
        assert_eq!(
            entries.keys().cloned().collect::<Vec<String>>(),
            expected
                .iter()
                .map(|path| (*path).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected the project's own files, got {:?}",
            entries.keys()
        );
        for (path, text) in &entries {
            assert!(
                !text.trim().is_empty(),
                "case {case}: expected {path} to carry the generated text, got {text:?}"
            );
        }
        assert_entries_are_inside_the_project(&entries, &case);

        assert_project_describes_the_deployment(
            &answer.project(&case),
            &api,
            &deployment,
            &case,
        );
        // Every generator's README lists the same client methods, in resource-path then
        // verb order: the methods are the deployment's, not the language's.
        assert_eq!(
            readme_operations(&entries, &case),
            DEPLOYED
                .iter()
                .map(|(method, ..)| (*method).to_string())
                .collect::<Vec<String>>(),
            "case {case}: expected the deployed methods in path order, got {:?}",
            entries.get("README.md")
        );
    }
}

#[tokio::test]
async fn a_configuration_property_is_read_under_either_of_its_published_spellings() {
    let gateway = Gateway::new();
    let (api, _, _) = deployed_api(&gateway).await;

    // One property, two spellings: the name the catalog publishes and the camelCase name
    // `GetSdkRequest$parameters` documents. Both configure the same generator the same way,
    // so the two archives are the same archive.
    let published = get_sdk(&gateway, &api, STAGE, "java", JAVA_PARAMETERS).await;
    let camel_case = get_sdk(&gateway, &api, STAGE, "java", JAVA_CAMEL_CASE_PARAMETERS).await;
    assert_eq!(
        published.entries("GetSdk(java, published spellings)"),
        camel_case.entries("GetSdk(java, camelCase spellings)"),
        "case GetSdk(java): expected the two spellings of one property to generate one \
         project"
    );

    // A spelling of neither form configures nothing, so the required property is missing.
    let unknown = get_sdk(
        &gateway,
        &api,
        STAGE,
        "java",
        &[("service_name", "Pet Store"), ("javapackagename", "com.pets")],
    )
    .await;
    assert_eq!(
        (unknown.status, unknown.error.as_deref()),
        (400, Some("BadRequestException")),
        "case GetSdk(java, unknown spellings): expected the modelled refusal, got {:?}",
        String::from_utf8_lossy(&unknown.body)
    );
}

#[tokio::test]
async fn a_configured_value_is_used_and_an_unset_property_falls_back_to_its_published_default()
{
    let gateway = Gateway::new();
    let (api, _, _) = deployed_api(&gateway).await;

    // `java`, configured with nothing but the two required properties: the group id
    // defaults to the package name, the artifact id to the service name and the version to
    // `1.0-SNAPSHOT`, each as that property's own published description states, and the
    // project is a Maven one.
    let defaulted = get_sdk(&gateway, &api, STAGE, "java", JAVA_PARAMETERS)
        .await
        .entries("GetSdk(java, defaults)");
    let pom = defaulted
        .get("pom.xml")
        .expect("case GetSdk(java, defaults): a Maven project carries a pom.xml");
    for expected in [
        "<groupId>com.petstore.client</groupId>",
        "<artifactId>pet-store</artifactId>",
        "<version>1.0-SNAPSHOT</version>",
    ] {
        assert!(
            pom.contains(expected),
            "case GetSdk(java, defaults): expected {expected} in the pom, got {pom}"
        );
    }

    // The same call with every optional property configured: each configured value is the
    // one the project carries.
    let configured = get_sdk(
        &gateway,
        &api,
        STAGE,
        "java",
        &[
            ("service.name", "Pet Store"),
            ("java.package-name", "com.petstore.client"),
            ("java.group-id", "com.petstore.group"),
            ("java.artifact-id", "petstore-client"),
            ("java.artifact-version", "3.2.1"),
            ("java.license-text", "Copyright the pet store."),
        ],
    )
    .await
    .entries("GetSdk(java, configured)");
    let pom = configured
        .get("pom.xml")
        .expect("case GetSdk(java, configured): a Maven project carries a pom.xml");
    for expected in [
        "<groupId>com.petstore.group</groupId>",
        "<artifactId>petstore-client</artifactId>",
        "<version>3.2.1</version>",
    ] {
        assert!(
            pom.contains(expected),
            "case GetSdk(java, configured): expected {expected} in the pom, got {pom}"
        );
    }
    // The configured license text is injected into every source file's header, which is
    // what that property publishes itself as.
    for (path, text) in &configured {
        if path.ends_with(".java") {
            assert!(
                text.starts_with("/*\n * Copyright the pet store.\n */\n"),
                "case GetSdk(java, configured): expected the license banner on {path}, got \
                 {text}"
            );
        }
    }

    // The build systems the `java.build-system` property publishes ("Currently supported:
    // maven, gradle"), and a value that names neither, which gets the first of them.
    for (configured_system, expected, absent) in [
        ("maven", "pom.xml", "build.gradle"),
        ("gradle", "build.gradle", "pom.xml"),
        ("MAVEN", "pom.xml", "build.gradle"),
        ("ant", "pom.xml", "build.gradle"),
    ] {
        let case = format!("GetSdk(java, build system {configured_system})");
        let parameters: Vec<(&str, &str)> = JAVA_PARAMETERS
            .iter()
            .copied()
            .chain([("java.build-system", configured_system)])
            .collect();
        let entries = get_sdk(&gateway, &api, STAGE, "java", &parameters)
            .await
            .entries(&case);
        assert!(
            entries.contains_key(expected),
            "case {case}: expected the project to carry {expected}, got {:?}",
            entries.keys()
        );
        assert!(
            !entries.contains_key(absent),
            "case {case}: expected no {absent} in the project, got {:?}",
            entries.keys()
        );
    }

    // `ruby`: the gem name defaults to the service name and the version to `1.0.0`, as
    // those properties' descriptions state; a configured value is used instead.
    let defaulted = get_sdk(&gateway, &api, STAGE, "ruby", &[("service.name", "Pet Store")])
        .await
        .entries("GetSdk(ruby, defaults)");
    let gemspec = defaulted
        .get("pet_store.gemspec")
        .expect("case GetSdk(ruby, defaults): the gem is named for the service");
    assert!(
        gemspec.contains("spec.version = \"1.0.0\""),
        "case GetSdk(ruby, defaults): expected the published default version, got {gemspec}"
    );
    let configured = get_sdk(
        &gateway,
        &api,
        STAGE,
        "ruby",
        &[
            ("service.name", "Pet Store"),
            ("ruby.gem-name", "petstore_api"),
            ("ruby.gem-version", "4.5.6"),
        ],
    )
    .await
    .entries("GetSdk(ruby, configured)");
    assert_eq!(
        configured.keys().cloned().collect::<BTreeSet<String>>(),
        ["README.md", "lib/petstore_api.rb", "petstore_api.gemspec"]
            .iter()
            .map(|path| (*path).to_string())
            .collect::<BTreeSet<String>>(),
        "case GetSdk(ruby, configured): expected the configured gem's own files, got {:?}",
        configured.keys()
    );
    let gemspec = configured
        .get("petstore_api.gemspec")
        .expect("case GetSdk(ruby, configured): the gem is named as configured");
    assert!(
        gemspec.contains("spec.version = \"4.5.6\""),
        "case GetSdk(ruby, configured): expected the configured version, got {gemspec}"
    );

    // The gem name is the only configured value that NAMES FILES of the project, so a value
    // that is not a single path segment is derived into one: every entry of the archive stays
    // a file of the project.
    for gem_name in [
        "lib/../../escaped",
        "../escape",
        "/absolute",
        "pet store",
        "...",
    ] {
        let case = format!("GetSdk(ruby, gem name {gem_name:?})");
        let entries = get_sdk(
            &gateway,
            &api,
            STAGE,
            "ruby",
            &[("service.name", "Pet Store"), ("ruby.gem-name", gem_name)],
        )
        .await
        .entries(&case);
        assert_entries_are_inside_the_project(&entries, &case);
    }
}

#[tokio::test]
async fn an_sdk_type_no_generator_writes_a_client_in_is_the_modelled_bad_request() {
    let gateway = Gateway::new();
    let (api, _, _) = deployed_api(&gateway).await;

    // The ways an sdkType can miss the documented set: a language API Gateway generates no
    // client in, a documented id in the wrong case, one with a trailing character, a
    // plausible other spelling, and an id shaped like one of this service's minted ids.
    for sdk_type in [
        "cobol",
        "typescript",
        "JAVA",
        "java2",
        "objective-c",
        "node",
        "zzzzzzzzzz",
    ] {
        let case = format!("GetSdk({sdk_type})");
        let answer = get_sdk(&gateway, &api, STAGE, sdk_type, &[]).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("BadRequestException")),
            "case {case}: expected the modelled refusal, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
        // Fixed text naming the generator the service has none of, with the sdkType taken
        // from the request's own path label.
        assert_eq!(
            answer.message(),
            format!("No SDK generator for type '{sdk_type}'"),
            "case {case}: expected the generator refusal naming {sdk_type:?}, got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn a_generator_missing_a_required_parameter_is_the_modelled_bad_request() {
    let gateway = Gateway::new();
    let (api, _, _) = deployed_api(&gateway).await;

    // Every generator that declares a required property, configured without it: the refusal
    // names EVERY property the generator cannot be run without that the request left unset,
    // so the rows differ along how many are missing (all of them, one of several, one of
    // one) and along which spelling satisfied the ones that are set.
    let missing: &[MissingRow<'_>] = &[
        ("java", &[], &["service.name", "java.package-name"][..]),
        (
            "java",
            &[("service.name", "Pet Store")],
            &["java.package-name"][..],
        ),
        (
            "java",
            &[("javaPackageName", "com.pets")],
            &["service.name"][..],
        ),
        ("ruby", &[], &["service.name"][..]),
        ("ruby", &[("service.name", "   ")], &["service.name"][..]),
        ("objectivec", &[], &["classPrefix"][..]),
        ("swift", &[("classPrefix", "")], &["classPrefix"][..]),
        (
            "android",
            &[],
            &["groupId", "artifactId", "artifactVersion", "invokerPackage"][..],
        ),
        (
            "android",
            &[
                ("groupId", "com.petstore"),
                ("artifactId", "petstore-android"),
                ("artifactVersion", "2.1.0"),
            ],
            &["invokerPackage"][..],
        ),
    ];
    for (sdk_type, parameters, properties) in missing {
        let case = format!("GetSdk({sdk_type}, without {properties:?})");
        let answer = get_sdk(&gateway, &api, STAGE, sdk_type, parameters).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (400, Some("BadRequestException")),
            "case {case}: expected the modelled refusal, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
        // Fixed text, then the missing property names bracketed and comma-separated; the
        // sdkType itself is not named.
        assert_eq!(
            answer.message(),
            format!(
                "Required SDK properties are missing: [{}]",
                properties.join(", ")
            ),
            "case {case}: expected the refusal to name {properties:?}, got {:?}",
            answer.message()
        );
        assert!(
            !answer.message().contains("sdkType"),
            "case {case}: the refusal must not name the sdkType, got {:?}",
            answer.message()
        );
    }

    // `javascript` declares no required parameter, so it generates with none set at all.
    let generated = get_sdk(&gateway, &api, STAGE, "javascript", &[]).await;
    assert_eq!(
        generated.status, 200,
        "case GetSdk(javascript, no parameters): expected 200, got {} with {:?}",
        generated.status,
        String::from_utf8_lossy(&generated.body)
    );
}

#[tokio::test]
async fn the_stage_is_resolved_before_the_requests_own_sdk_type_and_parameters() {
    let gateway = Gateway::new();
    let (api, _, _) = deployed_api(&gateway).await;

    // A request whose URI names no stored stage is refused for the STAGE whether or not its
    // sdkType and its parameters would also have been rejected: the resources the path names
    // are resolved before the request's own contents. The rows differ along which half of
    // the pair is unresolvable and along what else about the request is wrong.
    let rows: &[PrecedenceRow<'_>] = &[
        ("a stage the api does not hold", api.as_str(), "ruby", &[]),
        (
            "a stage the api does not hold, with a configured generator",
            api.as_str(),
            "java",
            &[("service.name", "Pet Store")],
        ),
        (
            "a stage the api does not hold, with a generator this service has none of",
            api.as_str(),
            "cobol",
            &[],
        ),
        ("an api no record answers to", "zzzzzzzzzz", "ruby", &[]),
        (
            "an api no record answers to, with a generator this service has none of",
            "zzzzzzzzzz",
            "objective-c",
            &[],
        ),
    ];
    for (case, api_id, sdk_type, parameters) in rows {
        let answer = get_sdk(&gateway, api_id, "nosuchstage", sdk_type, parameters).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (404, Some("NotFoundException")),
            "case {case}: expected the stage refusal, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.message(),
            "Invalid stage identifier specified",
            "case {case}: expected the stage refusal, got {:?}",
            answer.message()
        );
    }

    // The same requests against the stage the api DOES hold are refused for what they carry,
    // so the order above is a precedence and not a swallowed check.
    let parameters = get_sdk(&gateway, &api, STAGE, "ruby", &[]).await;
    assert_eq!(
        (parameters.status, parameters.message()),
        (
            400,
            "Required SDK properties are missing: [service.name]".to_string()
        ),
        "case a resolvable stage without the required properties: expected the parameter \
         refusal, got {:?}",
        String::from_utf8_lossy(&parameters.body)
    );
    let generator = get_sdk(&gateway, &api, STAGE, "cobol", &[]).await;
    assert_eq!(
        (generator.status, generator.message()),
        (400, "No SDK generator for type 'cobol'".to_string()),
        "case a resolvable stage naming a generator this service has none of: expected the \
         generator refusal, got {:?}",
        String::from_utf8_lossy(&generator.body)
    );
}

#[tokio::test]
async fn an_unresolvable_api_or_stage_is_the_modelled_not_found() {
    let gateway = Gateway::new();
    let (api, _, _) = deployed_api(&gateway).await;

    // A stage is addressed by the pair, so either half missing leaves the stage
    // unresolvable and the refusal reports the stage.
    let unresolvable: &[(&str, &str)] = &[
        (api.as_str(), "nosuchstage"),
        (api.as_str(), "PROD"),
        ("zzzzzzzzzz", STAGE),
        ("zzzzzzzzzz", "nosuchstage"),
    ];
    for (api_id, stage) in unresolvable {
        let case = format!("GetSdk({api_id}/{stage})");
        let answer = get_sdk(&gateway, api_id, stage, "javascript", &[]).await;
        assert_eq!(
            (answer.status, answer.error.as_deref()),
            (404, Some("NotFoundException")),
            "case {case}: expected the modelled refusal, got {:?}",
            String::from_utf8_lossy(&answer.body)
        );
        assert_eq!(
            answer.message(),
            "Invalid stage identifier specified",
            "case {case}: expected the stage refusal, got {:?}",
            answer.message()
        );
    }
}

#[tokio::test]
async fn the_sdk_is_of_the_deployment_the_stage_serves_and_generating_one_changes_nothing() {
    let gateway = Gateway::new();
    let (api, deployment, root) = deployed_api(&gateway).await;

    let stage_before = gateway
        .call("GET", &format!("/restapis/{api}/stages/{STAGE}"), None)
        .await;
    let first = get_sdk(&gateway, &api, STAGE, "javascript", &[]).await;
    let first_entries = first.entries("GetSdk(javascript, first)");

    // A read does not change what a later caller sees: the same call answers the same
    // project — the same archive, byte for byte — and the stage's own record is untouched.
    let again = get_sdk(&gateway, &api, STAGE, "javascript", &[]).await;
    assert_eq!(
        again.entries("GetSdk(javascript, again)"),
        first_entries,
        "case GetSdk(javascript) repeated: expected the same project"
    );
    assert_eq!(
        again.body, first.body,
        "case GetSdk(javascript) repeated: expected the same archive bytes"
    );
    assert_eq!(
        again.content_disposition, first.content_disposition,
        "case GetSdk(javascript) repeated: expected the same attachment name"
    );
    let stage_after = gateway
        .call("GET", &format!("/restapis/{api}/stages/{STAGE}"), None)
        .await;
    assert_eq!(
        stage_after.body, stage_before.body,
        "case GetSdk(javascript): generating an SDK must not change the stage, got {:?} after \
         {:?}",
        stage_after.body, stage_before.body
    );

    // A method put on the api AFTER the deployment is not in the SDK: what the stage
    // serves is the snapshot it points at.
    let undeployed = string_member(
        &create_resource(&gateway, &api, &root, "undeployed").await,
        "id",
        "CreateResource(undeployed)",
    );
    put_method(
        &gateway,
        &api,
        &undeployed,
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    put_integration(&gateway, &api, &undeployed, "GET", json!({ "type": "MOCK" })).await;
    let unchanged = get_sdk(&gateway, &api, STAGE, "javascript", &[]).await;
    assert_eq!(
        unchanged.entries("GetSdk(javascript, before redeploying)"),
        first_entries,
        "case GetSdk(javascript): expected the deployed snapshot, not the live api"
    );

    // Redeploying the api re-points the stage, and the SDK carries the new method.
    let redeployment = deploy(&gateway, &api, STAGE).await;
    assert_ne!(
        redeployment, deployment,
        "case CreateDeployment: expected a new deployment of the api"
    );
    let redeployed = get_sdk(&gateway, &api, STAGE, "javascript", &[]).await;
    let project = redeployed.project("GetSdk(javascript, redeployed)");
    assert!(
        project.contains("GET /undeployed"),
        "case GetSdk(javascript, redeployed): expected the newly deployed method, got {project}"
    );
    assert!(
        project.contains(&redeployment),
        "case GetSdk(javascript, redeployed): expected the project to name the deployment \
         {redeployment}, got {project}"
    );
}
