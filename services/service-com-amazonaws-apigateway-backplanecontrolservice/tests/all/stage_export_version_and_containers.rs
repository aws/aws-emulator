// SPDX-License-Identifier: CC0-1.0

//! End-to-end tests for what an exported definition SAYS ABOUT ITSELF: the version the
//! downloaded file is named for, and the containers each OpenAPI dialect always writes.
//!
//! Every case drives the real wire path — a `restJson1` HTTP request through the
//! multi-service router into the mounted service — so what is asserted is what a client
//! sees, not what a handler returns. The emulator, the `export` request builder and the
//! `CreateRestApi`/`PutMethod`/`CreateDeployment` helpers are this crate's existing
//! harness, reused from `rest_api_lifecycle`, `stage_export` and
//! `execute_api_data_plane`.
//!
//! The rules these cases assert, each over a SET of inputs rather than one recorded call:
//!
//! * the attachment is named for the export type and the VERSION of the definition it
//!   carries — the same version the document's own `info` states — so two exports of one
//!   stage download the same file name;
//! * an OpenAPI 3.0 document carries its Components Object and every Response Object's
//!   `content` map whether or not anything was put in them, while the 2.0 dialect, which
//!   has neither container, carries neither.
//!
//! The rows differ along the API's `version` (set, set to another form, and absent, where
//! the definition is identified by its deployment instead), along the dialect, along the
//! serialization the `Accept` header asks for, and along how many method responses a
//! method declares.

use serde_json::{json, Value};

use crate::execute_api_data_plane::{deploy, put_integration, put_method};
use crate::rest_api_lifecycle::{create_api, create_resource, string_member, Gateway};
use crate::stage_export::export;

/// The two dialects `exportType` accepts, from `GetExportRequest$exportType`: "'oas30'
/// for OpenAPI 3.0.x and 'swagger' for Swagger/OpenAPI 2.0".
const DIALECTS: &[&str] = &["swagger", "oas30"];

/// The serializations `accepts` selects, with the file extension each attachment carries:
/// "Currently application/json and application/yaml are supported"
/// (`GetExportRequest$accepts`).
const SERIALIZATIONS: &[(&str, &str)] = &[
    ("application/json", "json"),
    ("application/yaml", "yaml"),
];

/// `CreateRestApi` with `version` set as given, answering the API id and its root
/// resource id. The sibling harness's `create_api` sets no version, which is this table's
/// last row.
async fn create_versioned_api(gateway: &Gateway, name: &str, version: &str) -> (String, String) {
    let case = format!("CreateRestApi({name}, version={version:?})");
    let answer = gateway
        .call(
            "POST",
            "/restapis",
            Some(json!({ "name": name, "version": version })),
        )
        .await;
    assert_eq!(
        answer.status, 201,
        "case {case}: expected 201, got {} with {:?}",
        answer.status, answer.body
    );
    (
        string_member(&answer.body, "id", &case),
        string_member(&answer.body, "rootResourceId", &case),
    )
}

/// An API with one method on a child resource, deployed to `stage`, so there is a
/// definition to export.
async fn deployed_api(gateway: &Gateway, api: &str, root: &str, stage: &str) {
    let created = create_resource(gateway, api, root, "widgets").await;
    let resource = string_member(&created, "id", "CreateResource(widgets)");
    put_method(
        gateway,
        api,
        &resource,
        "GET",
        json!({ "authorizationType": "NONE" }),
    )
    .await;
    // The integration a deployment requires of every method: `MOCK`, the type that needs
    // no backend.
    put_integration(gateway, api, &resource, "GET", json!({ "type": "MOCK" })).await;
    deploy(gateway, api, stage).await;
}

#[tokio::test]
async fn the_attachment_is_named_for_the_export_type_and_the_definition_s_version() {
    // Rows differ along the API's `version`: the plain form a caller stamps a release
    // with, a longer form carrying a pre-release part, and none at all — where the
    // document states what identifies the definition instead, and the file name follows
    // it rather than the clock.
    let versions: &[(&str, Option<&str>)] = &[
        ("a version of digits", Some("1.0")),
        ("a version with a pre-release part", Some("2.1.0-beta.3")),
        ("no version at all", None),
    ];

    let gateway = Gateway::new();
    for (case, version) in versions {
        let name = format!("export-version-{}", case.replace(' ', "-"));
        let (api, root) = match version {
            Some(version) => create_versioned_api(&gateway, &name, version).await,
            None => create_api(&gateway, &name).await,
        };
        deployed_api(&gateway, &api, &root, "dev").await;

        for dialect in DIALECTS {
            for (accepts, extension) in SERIALIZATIONS {
                let case = format!("{case} as {dialect} in {accepts}");
                let answer =
                    export(&gateway, &api, "dev", dialect, Some(accepts), &[]).await;
                assert_eq!(
                    answer.status, 200,
                    "case {case}: expected 200, got {} with {:?}",
                    answer.status,
                    String::from_utf8_lossy(&answer.body)
                );
                // The version the DOCUMENT states, which the file name has to carry: for
                // a row that set one it is that version, and for the row that set none it
                // is whatever identifies the definition — read from the document rather
                // than restated here, because the rule is that the two agree.
                let stated = document_version(&answer.body, accepts, &case);
                if let Some(version) = version {
                    assert_eq!(
                        stated, *version,
                        "case {case}: the document states the API's own version, got \
                         {stated:?}"
                    );
                }
                let expected =
                    format!("attachment; filename=\"{dialect}_{stated}.{extension}\"");
                assert_eq!(
                    answer.content_disposition.as_deref(),
                    Some(expected.as_str()),
                    "case {case}: expected the attachment {expected:?}, got {:?}",
                    answer.content_disposition
                );

                // Reported, not stamped: a second export of the same stage downloads the
                // same file name.
                let again = export(&gateway, &api, "dev", dialect, Some(accepts), &[]).await;
                assert_eq!(
                    again.content_disposition, answer.content_disposition,
                    "case {case}: two exports of one stage name the same file, got {:?} \
                     then {:?}",
                    answer.content_disposition, again.content_disposition
                );
            }
        }
    }
}

/// The `info.version` of an exported document, whichever serialization it was written in.
fn document_version(body: &[u8], accepts: &str, case: &str) -> String {
    let text = String::from_utf8(body.to_vec())
        .unwrap_or_else(|error| panic!("case {case}: expected a UTF-8 export, got {error}"));
    if accepts == "application/yaml" {
        let documents = ::yaml_rust2::yaml::YamlLoader::load_from_str(&text)
            .unwrap_or_else(|error| panic!("case {case}: expected a YAML export, got {error}"));
        let document = documents
            .first()
            .unwrap_or_else(|| panic!("case {case}: expected one YAML document, got {text}"));
        return document["info"]["version"]
            .as_str()
            .unwrap_or_else(|| panic!("case {case}: expected info.version, got {text}"))
            .to_string();
    }
    let document: Value = serde_json::from_str(&text)
        .unwrap_or_else(|error| panic!("case {case}: expected a JSON export, got {error}"));
    document["info"]["version"]
        .as_str()
        .unwrap_or_else(|| panic!("case {case}: expected info.version, got {text}"))
        .to_string()
}

#[tokio::test]
async fn an_oas30_document_carries_its_components_and_every_response_s_content() {
    // Rows differ along what a method declares, because the two containers are written
    // whether or not the definition has anything to put in them: a method with no
    // response at all, one with a single response, and one with several — and the API
    // declares no authorizer, so there is no security scheme to fill `components` with.
    let responses: &[(&str, &[&str])] = &[
        ("no method response", &[]),
        ("one method response", &["200"]),
        ("several method responses", &["200", "404", "500"]),
    ];

    let gateway = Gateway::new();
    for (case, declared) in responses {
        let name = format!("export-containers-{}", case.replace(' ', "-"));
        let (api, root) = create_api(&gateway, &name).await;
        let created = create_resource(&gateway, &api, &root, "widgets").await;
        let resource = string_member(&created, "id", "CreateResource(widgets)");
        put_method(
            &gateway,
            &api,
            &resource,
            "GET",
            json!({ "authorizationType": "NONE" }),
        )
        .await;
        put_integration(&gateway, &api, &resource, "GET", json!({ "type": "MOCK" })).await;
        for status in *declared {
            crate::method_response::put_method_response(
                &gateway,
                &api,
                &resource,
                "GET",
                status,
                json!({}),
            )
            .await;
        }
        deploy(&gateway, &api, "dev").await;

        let oas30 = export(&gateway, &api, "dev", "oas30", None, &[]).await;
        let document = oas30.json(case);
        let components = document["components"].as_object().unwrap_or_else(|| {
            panic!("case {case}: an oas30 document carries a components object, got {document}")
        });
        assert!(
            !components.contains_key("securitySchemes"),
            "case {case}: an API with no authorizer declares no security scheme, got \
             {document}"
        );
        for status in *declared {
            let response = &document["paths"]["/widgets"]["get"]["responses"][status];
            let content = response["content"].as_object().unwrap_or_else(|| {
                panic!(
                    "case {case}: an oas30 response object carries a content map, got \
                     {response}"
                )
            });
            assert!(
                content.is_empty(),
                "case {case}: a response whose method references no model describes no \
                 representation, got {response}"
            );
        }

        // The 2.0 dialect has neither container, so an export in it carries neither: a
        // document says what its own version of OpenAPI declares.
        let swagger = export(&gateway, &api, "dev", "swagger", None, &[]).await;
        let document = swagger.json(case);
        assert!(
            document.get("components").is_none(),
            "case {case}: a 2.0 document has no components object, got {document}"
        );
        for status in *declared {
            let response = &document["paths"]["/widgets"]["get"]["responses"][status];
            assert!(
                response.get("content").is_none(),
                "case {case}: a 2.0 response object has no content map, got {response}"
            );
        }
    }
}
