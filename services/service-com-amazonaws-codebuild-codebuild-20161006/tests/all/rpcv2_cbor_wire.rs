// SPDX-License-Identifier: CC0-1.0

//! This service over its `rpcv2Cbor` wire.
//!
//! RULE: a service whose model declares several protocols serves a request over whichever
//! of them the request arrives on, with no entry outranking another — so the same stored
//! state answers an `awsJson1_1` request (`POST /` with `X-Amz-Target`, the wire every
//! other suite here drives) and an `rpcv2Cbor` one (`POST
//! /service/<service-shape>/operation/<Operation>` with a CBOR body), each in its own
//! envelope: a CBOR answer is labelled `application/cbor` and carries the
//! `smithy-protocol` header that protocol's specification requires.
//!
//! The rows differ along what the rule generalises over: the operation class driven (a
//! create, a delete) and whether the request is served or REFUSED — a refusal travels in
//! the same envelope as a success.
//!
//! Every body here goes through the generated schema serializer (`CborCodec` + the
//! operation's `*_INPUT_SCHEMA`), the same path a client built from this project's types
//! takes, and every answer is read back through the generated deserializer.

use std::sync::Arc;

use aws_smithy_schema::codec::{Codec, FinishSerializer};
use aws_smithy_schema::serde::{SerializableStruct, ShapeSerializer};
use aws_smithy_schema::Schema;
use aws_smithy_schema_server::{CborCodec, DeserializableStruct};
use service_com_amazonaws_codebuild_codebuild_20161006::ops::types::{
    CreateProjectInput, CreateProjectOutput, DeleteProjectInput, DeleteProjectOutput,
    ProjectArtifacts, ProjectEnvironment, ProjectSource, CREATE_PROJECT_INPUT_SCHEMA,
    DELETE_PROJECT_INPUT_SCHEMA,
};
use service_com_amazonaws_codebuild_codebuild_20161006::{state, EmulatedService};
use smithy_server_runtime::MultiServiceRouter;

/// The Region the request's SigV4 credential scope names, which is also what the router
/// resolves this service's route key from.
const REGION: &str = "us-west-2";

/// This service's SigV4 signing name.
const SIGNING_NAME: &str = "codebuild";

/// The Smithy service shape name, which is the `rpcv2Cbor` URI's service segment (service
/// shape `CodeBuild_20161006`, as `OPERATIONS.md` names it).
const SERVICE_SHAPE: &str = "CodeBuild_20161006";

/// The media type an `rpcv2Cbor` body is labelled with, in both directions.
const CBOR_CONTENT_TYPE: &str = "application/cbor";

/// The value the `smithy-protocol` header carries on that wire.
const CBOR_PROTOCOL: &str = "rpc-v2-cbor";

/// What one dispatched request answered.
struct Answer {
    status: u16,
    content_type: String,
    smithy_protocol: String,
    body: Vec<u8>,
}

impl Answer {
    /// Whether the CBOR body carries `needle` as text — enough to name the error class a
    /// refusal reports, whose envelope is the protocol's rather than an operation shape.
    fn mentions(&self, needle: &str) -> bool {
        self.body
            .windows(needle.len())
            .any(|window| window == needle.as_bytes())
    }

    fn render(&self) -> String {
        String::from_utf8_lossy(&self.body).escape_debug().to_string()
    }
}

/// A request body as the generated serializer writes it.
fn cbor_body<I: SerializableStruct>(schema: &Schema, input: &I) -> Vec<u8> {
    let codec = CborCodec::default();
    let mut serializer = codec.create_serializer();
    serializer
        .write_struct(schema, input)
        .expect("a serializable request body");
    serializer.finish()
}

/// One router carrying only this service, over one shared engine.
fn emulator() -> MultiServiceRouter {
    let mut router = MultiServiceRouter::new();
    router.set_request_authentication(false);
    router.register_service(EmulatedService::new(Arc::new(
        engine::InMemoryEngine::new(),
    )));
    router
}

/// Dispatch one `rpcv2Cbor` request for `operation`, with `input` serialized against
/// `schema`.
async fn over_the_wire<I: SerializableStruct>(
    router: &MultiServiceRouter,
    operation: &str,
    schema: &Schema,
    input: &I,
) -> Answer {
    let request = http::Request::builder()
        .method("POST")
        .uri(format!("/service/{SERVICE_SHAPE}/operation/{operation}"))
        .header("content-type", CBOR_CONTENT_TYPE)
        .header("smithy-protocol", CBOR_PROTOCOL)
        .header(
            "authorization",
            format!(
                "AWS4-HMAC-SHA256 Credential=AKIDCODEBUILDTEST/20260101/{REGION}/\
                 {SIGNING_NAME}/aws4_request, SignedHeaders=host, Signature=0f0f"
            ),
        )
        .body(bytes::Bytes::from(cbor_body(schema, input)))
        .expect("a well-formed request");
    let response = router.dispatch(request).await;
    let header = |name: &str| {
        response
            .headers()
            .get(name)
            .and_then(|value| value.to_str().ok())
            .unwrap_or_default()
            .to_string()
    };
    Answer {
        status: response.status().as_u16(),
        content_type: header("content-type"),
        smithy_protocol: header("smithy-protocol"),
        body: response.body().to_vec(),
    }
}

/// Assert `answer` came back in this protocol's own envelope.
fn assert_envelope(case: &str, answer: &Answer) {
    assert_eq!(
        (
            answer.content_type.as_str(),
            answer.smithy_protocol.as_str()
        ),
        (CBOR_CONTENT_TYPE, CBOR_PROTOCOL),
        "case {case:?}: expected this protocol's envelope headers, got {:?} and {:?} with \
         body {}",
        answer.content_type,
        answer.smithy_protocol,
        answer.render()
    );
}

/// A `CreateProject` input for `name`: the smallest project this service accepts.
fn create_input(name: &str) -> CreateProjectInput {
    CreateProjectInput {
        name: name.to_string(),
        source: ProjectSource {
            r#type: state::NO_SOURCE_TYPE.to_string(),
            buildspec: Some("version: 0.2".to_string()),
            ..Default::default()
        },
        artifacts: ProjectArtifacts {
            r#type: "NO_ARTIFACTS".to_string(),
            ..Default::default()
        },
        environment: ProjectEnvironment {
            r#type: "LINUX_CONTAINER".to_string(),
            image: "aws/codebuild/standard:7.0".to_string(),
            compute_type: "BUILD_GENERAL1_SMALL".to_string(),
            ..Default::default()
        },
        service_role: "arn:aws:iam::000000000000:role/service-role/codebuild".to_string(),
        ..Default::default()
    }
}

#[tokio::test]
async fn a_project_is_created_and_deleted_over_the_rpcv2_cbor_wire() {
    let router = emulator();
    let codec = CborCodec::default();

    // (case, the name to create)
    for (case, name) in [
        ("a project named for this wire", "cbor-wire-project"),
        ("a second project on the same store", "cbor-wire-project-2"),
    ] {
        let created = over_the_wire(
            &router,
            "CreateProject",
            CREATE_PROJECT_INPUT_SCHEMA,
            &create_input(name),
        )
        .await;
        assert_eq!(
            created.status,
            200,
            "case {case:?}: CreateProject expected 200, got {} with body {}",
            created.status,
            created.render()
        );
        assert_envelope(case, &created);
        let mut deserializer = codec.create_deserializer(&created.body);
        let output = CreateProjectOutput::deserialize(&mut deserializer)
            .expect("the CreateProject response deserializes");
        let project = output
            .project
            .unwrap_or_else(|| panic!("case {case:?}: the create reports the stored project"));
        assert_eq!(
            project.name.as_deref(),
            Some(name),
            "case {case:?}: expected the project it was asked to create, got {:?}",
            project.name
        );

        // The delete of that same stored project, over the same wire: it answers the
        // disposition of the webhook the project carried, which is the member this crate
        // declares locally (`local_model_additions`).
        let deleted = over_the_wire(
            &router,
            "DeleteProject",
            DELETE_PROJECT_INPUT_SCHEMA,
            &DeleteProjectInput {
                name: name.to_string(),
            },
        )
        .await;
        assert_eq!(
            deleted.status,
            200,
            "case {case:?}: DeleteProject expected 200, got {} with body {}",
            deleted.status,
            deleted.render()
        );
        assert_envelope(case, &deleted);
        let mut deserializer = codec.create_deserializer(&deleted.body);
        let disposition = DeleteProjectOutput::deserialize(&mut deserializer)
            .expect("the DeleteProject response deserializes");
        assert_eq!(
            disposition.webhook_deleted_status.as_deref(),
            Some(state::webhook_deleted_status(false)),
            "case {case:?}: a project carrying no webhook removes none, got {:?}",
            disposition.webhook_deleted_status
        );
    }
}

#[tokio::test]
async fn a_request_this_service_refuses_is_refused_over_the_rpcv2_cbor_wire() {
    let router = emulator();
    // This service states a request it cannot accept as `InvalidInputException` with HTTP
    // 400 — on this wire as on the other. The two cases break a constraint of a different
    // operation each: the name a delete names is held to at least one character
    // (`NonEmptyString`, length 1..), and so is the name a create asks for (`ProjectName`,
    // length 2.. and its own pattern).
    let refused = over_the_wire(
        &router,
        "DeleteProject",
        DELETE_PROJECT_INPUT_SCHEMA,
        &DeleteProjectInput {
            name: String::new(),
        },
    )
    .await;
    assert_refusal("an empty name on the delete", &refused);

    let refused = over_the_wire(
        &router,
        "CreateProject",
        CREATE_PROJECT_INPUT_SCHEMA,
        &create_input(""),
    )
    .await;
    assert_refusal("an empty name on the create", &refused);
}

/// Assert `answer` is this service's own refusal, in this protocol's envelope.
fn assert_refusal(case: &str, answer: &Answer) {
    assert_eq!(
        answer.status,
        400,
        "case {case:?}: expected the modelled refusal, got {} with body {}",
        answer.status,
        answer.render()
    );
    assert_envelope(case, answer);
    assert!(
        answer.mentions("InvalidInputException"),
        "case {case:?}: expected the error class this service refuses with, got {}",
        answer.render()
    );
}
