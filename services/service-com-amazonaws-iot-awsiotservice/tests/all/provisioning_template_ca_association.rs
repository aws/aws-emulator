// SPDX-License-Identifier: CC0-1.0

//! End-to-end HTTP coverage of the guard that keeps a FLEET PROVISIONING TEMPLATE from
//! being deleted out from under the registered CA certificates that provision with it.
//!
//! Every case drives the real wire path — `MultiServiceRouter::dispatch` over a restJson1
//! request signed for the `iot` signing name — so what is asserted is what an AWS SDK
//! client would see, not an internal function's return value.
//!
//! The rule under test: a resource another record in the account still REFERENCES is not
//! deleted — the delete is refused with the modelled `DeleteConflictException` (HTTP 409)
//! and nothing is removed — and it becomes deletable once nothing references it. Here the
//! referencing record is a registered CA certificate whose `registrationConfig` names the
//! template the device certificates auto-registered under that CA are provisioned with
//! (`RegisterCACertificate$registrationConfig`), so `DeleteProvisioningTemplate` on a
//! template any such CA names answers 409 `DeleteConflictException` with the template
//! still readable through `DescribeProvisioningTemplate`, while a template no CA names is
//! removed (200, empty body) and every later read of it is `ResourceNotFoundException`
//! (404).
//!
//! `DeleteCACertificate` is what RELEASES such a reference, so its own rules are pinned
//! here beside the guard that reads them: it removes the record the id names once the
//! account's state admits it — an id the CA registry holds none under is
//! `ResourceNotFoundException` (404) and a record still `ACTIVE` is
//! `CertificateStateException` (406), with nothing removed either way.
//!
//! The tables span what that rule generalises over rather than one request: the forms a
//! `registrationConfig` takes (the template name alone, beside a `roleArn`, beside a
//! `templateBody`), both `CertificateMode` values, the three forms `allowAutoRegistration`
//! and `setAsActive` each take on the wire (absent — the modelled `default false` —
//! explicitly `false`, explicitly `true`), one CA referencing a template and several, and
//! the ways a CA fails to reference one (no `registrationConfig` at all, one carrying no
//! `templateName`, one naming another template, one naming a name that differs only in
//! case). The template names are legal values of the modelled `TemplateName` (`len 1..36,
//! pattern ^[0-9A-Za-z_-]+$`, from
//! `smithy_query(query="operation",operation="DeleteProvisioningTemplate")`): one
//! character, digits and both separators, and the longest the length constraint admits.

use std::sync::Arc;

use serde_json::{json, Value};
use service_com_amazonaws_iot_awsiotservice::state::{
    CERTIFICATE_MODE_DEFAULT, CERTIFICATE_MODE_SNI_ONLY,
};
use service_com_amazonaws_iot_awsiotservice::EmulatedService;
use smithy_server_runtime::MultiServiceRouter;

/// The region this suite signs for.
const REGION: &str = "ap-south-1";
/// The role a template provisions under: its form is the caller's, and no IAM service is
/// mounted here to answer that the role cannot be assumed, so one well-formed ARN serves
/// every case.
const PROVISIONING_ROLE: &str = "arn:aws:iam::000000000000:role/IoTProvisioningRole";

/// One response, reduced to what a client branches on.
#[derive(Debug)]
struct Reply {
    status: u16,
    error_code: Option<String>,
    body: Value,
}

impl Reply {
    /// The error shape the answer names, as a client reads it: the `x-amzn-errortype`
    /// header up to the `:` that introduces the shape's documentation URI.
    fn code(&self) -> String {
        self.error_code
            .as_deref()
            .map(|value| value.split(':').next().unwrap_or(value).to_string())
            .unwrap_or_default()
    }

    /// The `message` of an error body.
    fn message(&self) -> String {
        self.body
            .get("message")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// The whole body, for an assertion's failure text.
    fn text(&self) -> String {
        self.body.to_string()
    }
}

/// The emulator under test: ONE shared engine with the IoT service mounted on it, reached
/// the way the production router reaches it.
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

    async fn dispatch(&self, method: &str, path: &str, body: Option<&Value>) -> Reply {
        let host = format!("iot.{REGION}.amazonaws.com");
        let payload = body.map_or_else(bytes::Bytes::new, |value| {
            bytes::Bytes::from(value.to_string())
        });
        let request = http::Request::builder()
            .method(method)
            .uri(format!("https://{host}{path}"))
            .header("host", host)
            .header("content-type", "application/json")
            .header(
                "authorization",
                format!(
                    "AWS4-HMAC-SHA256 Credential=AKIDEXAMPLE/20260101/{REGION}/iot/aws4_request, \
                     SignedHeaders=host, Signature=0123456789abcdef"
                ),
            )
            .body(payload)
            .expect("the provisioning-template request is well formed");
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

    /// `CreateProvisioningTemplate`: `POST /provisioning-templates`.
    async fn create_template(&self, case: &str, template_name: &str, template_type: &str) {
        let created = self
            .dispatch(
                "POST",
                "/provisioning-templates",
                Some(&json!({
                    "templateName": template_name,
                    "templateBody": TEMPLATE_BODY,
                    "provisioningRoleArn": PROVISIONING_ROLE,
                    "type": template_type,
                })),
            )
            .await;
        assert_eq!(
            created.status,
            200,
            "case {case}: expected the create of {template_name} to answer 200, got {} body {}",
            created.status,
            created.text()
        );
    }

    /// `DescribeProvisioningTemplate`: `GET /provisioning-templates/{templateName}`.
    async fn describe_template(&self, template_name: &str) -> Reply {
        self.dispatch(
            "GET",
            &format!("/provisioning-templates/{template_name}"),
            None,
        )
        .await
    }

    /// `DeleteProvisioningTemplate`: `DELETE /provisioning-templates/{templateName}`.
    async fn delete_template(&self, template_name: &str) -> Reply {
        self.dispatch(
            "DELETE",
            &format!("/provisioning-templates/{template_name}"),
            None,
        )
        .await
    }

    /// `RegisterCACertificate`: `POST /cacertificate`, with `setAsActive` and
    /// `allowAutoRegistration` bound as `@httpQuery` parameters — absent from the query
    /// when the case names none — and everything else in the body.
    async fn register_ca(&self, case: &str, registration: &CaRegistration) -> String {
        let mut query: Vec<String> = Vec::new();
        if let Some(set_as_active) = registration.set_as_active {
            query.push(format!("setAsActive={set_as_active}"));
        }
        if let Some(allow_auto_registration) = registration.allow_auto_registration {
            query.push(format!("allowAutoRegistration={allow_auto_registration}"));
        }
        let path = if query.is_empty() {
            "/cacertificate".to_string()
        } else {
            format!("/cacertificate?{}", query.join("&"))
        };

        let mut body = json!({
            "caCertificate": ca_certificate(registration.subject),
            "certificateMode": registration.certificate_mode,
        });
        if registration.certificate_mode == CERTIFICATE_MODE_DEFAULT {
            // "If certificateMode is DEFAULT or not provided, the verificationCertificate
            // field must not be empty" (`RegisterCACertificate$verificationCertificate`).
            body["verificationCertificate"] =
                json!(end_entity_certificate(registration.subject));
        }
        if let Some(configuration) = &registration.registration_config {
            body["registrationConfig"] = configuration.clone();
        }

        let registered = self.dispatch("POST", &path, Some(&body)).await;
        assert_eq!(
            registered.status,
            200,
            "case {case}: expected the registration of the CA under {} to answer 200, got {} \
             body {}",
            registration.subject,
            registered.status,
            registered.text()
        );
        registered
            .body
            .get("certificateId")
            .and_then(Value::as_str)
            .unwrap_or_default()
            .to_string()
    }

    /// `DescribeCACertificate`: `GET /cacertificate/{certificateId}`.
    async fn describe_ca(&self, certificate_id: &str) -> Reply {
        self.dispatch("GET", &format!("/cacertificate/{certificate_id}"), None)
            .await
    }

    /// `DeleteCACertificate`: `DELETE /cacertificate/{certificateId}`.
    async fn delete_ca(&self, certificate_id: &str) -> Reply {
        self.dispatch("DELETE", &format!("/cacertificate/{certificate_id}"), None)
            .await
    }
}

/// A provisioning template body: a JSON document whose `Resources` section declares what
/// provisioning creates, which is what the service reads a body for.
const TEMPLATE_BODY: &str = r#"{
  "Parameters": {
    "AWS::IoT::Certificate::CommonName": { "Type": "String" },
    "AWS::IoT::Certificate::Id": { "Type": "String" }
  },
  "Resources": {
    "thing": {
      "Type": "AWS::IoT::Thing",
      "Properties": { "ThingName": { "Ref": "AWS::IoT::Certificate::CommonName" } }
    },
    "certificate": {
      "Type": "AWS::IoT::Certificate",
      "Properties": { "CertificateId": { "Ref": "AWS::IoT::Certificate::Id" }, "Status": "ACTIVE" }
    }
  }
}"#;

/// `TemplateType$JITP` — the kind of template a CA's `registrationConfig` names, and
/// `TemplateType$FLEET_PROVISIONING` — the other value the modelled enum declares
/// (`smithy_query(query="shape",shape="TemplateType")`). The guard is about the REFERENCE
/// and not about the kind of template, so the cases span both.
const JITP: &str = "JITP";
const FLEET_PROVISIONING: &str = "FLEET_PROVISIONING";

/// One `RegisterCACertificate` request, as a case names it.
struct CaRegistration {
    /// The CA's common name, which is the SUBJECT the material certifies under: a
    /// different one per registration, because one subject may be registered only so many
    /// times for an account.
    subject: &'static str,
    /// `certificateMode` — `SNI_ONLY` carries no verification certificate, `DEFAULT` one.
    certificate_mode: &'static str,
    /// `setAsActive`, absent when the case names none.
    set_as_active: Option<bool>,
    /// `allowAutoRegistration`, absent when the case names none.
    allow_auto_registration: Option<bool>,
    /// `registrationConfig`, absent when the case names none.
    registration_config: Option<Value>,
}

impl CaRegistration {
    /// A registration in `SNI_ONLY` mode naming nothing further: the shortest legal
    /// request, which the cases vary one member of.
    fn new(subject: &'static str) -> Self {
        Self {
            subject,
            certificate_mode: CERTIFICATE_MODE_SNI_ONLY,
            set_as_active: None,
            allow_auto_registration: None,
            registration_config: None,
        }
    }

    fn mode(mut self, certificate_mode: &'static str) -> Self {
        self.certificate_mode = certificate_mode;
        self
    }

    fn active(mut self, set_as_active: bool) -> Self {
        self.set_as_active = Some(set_as_active);
        self
    }

    fn auto_registration(mut self, allow_auto_registration: bool) -> Self {
        self.allow_auto_registration = Some(allow_auto_registration);
        self
    }

    fn registration_config(mut self, configuration: Value) -> Self {
        self.registration_config = Some(configuration);
        self
    }
}

/// A self-signed certificate AUTHORITY under `common_name`, as the PEM a caller registers.
fn ca_certificate(common_name: &str) -> String {
    certificate(
        common_name,
        ::rcgen::IsCa::Ca(::rcgen::BasicConstraints::Unconstrained),
    )
}

/// An END-ENTITY certificate under `common_name`: readable material that is no authority,
/// which is what a `DEFAULT` registration's verification certificate is.
fn end_entity_certificate(common_name: &str) -> String {
    certificate(common_name, ::rcgen::IsCa::ExplicitNoCa)
}

/// The certificate the two helpers above differ only in the `basicConstraints` of.
fn certificate(common_name: &str, is_ca: ::rcgen::IsCa) -> String {
    let key = ::rcgen::KeyPair::generate().expect("a key pair is generated");
    let mut params =
        ::rcgen::CertificateParams::new(Vec::<String>::new()).expect("certificate parameters");
    let mut subject = ::rcgen::DistinguishedName::new();
    subject.push(::rcgen::DnType::CommonName, common_name);
    params.distinguished_name = subject;
    params.not_before = ::rcgen::date_time_ymd(2020, 1, 1);
    params.not_after = ::rcgen::date_time_ymd(2040, 1, 1);
    params.is_ca = is_ca;
    params
        .self_signed(&key)
        .expect("a self-signed certificate")
        .pem()
}

/// The sentence AWS IoT refuses the delete of a template a CA still provisions with.
fn associated_with_cas_message(template_name: &str) -> String {
    format!(
        "The template {template_name} cannot be deleted as it is associated with one or more \
         CAs for auto registration."
    )
}

/// The sentence every reader of ONE template refuses a name the account holds none under
/// with.
fn missing_template_message(template_name: &str) -> String {
    format!("Unable to find template named {template_name}")
}

/// The sentence the DELETE itself refuses a name the account holds no template under with.
const UNDELETABLE_MISSING_TEMPLATE: &str = "Unable to delete template, template not found";

/// The sentence every operation addressing ONE registered CA certificate refuses an id the
/// account's CA registry holds none under with.
const MISSING_CA_CERTIFICATE: &str = "Requested CA Certificate does not exist.";

/// One case of the refusal table: a template, the CA registrations made against it, and
/// the label the assertions name it by.
struct Referenced {
    label: &'static str,
    template_name: &'static str,
    template_type: &'static str,
    cas: Vec<CaRegistration>,
}

/// RULE: a provisioning template any registered CA certificate's `registrationConfig`
/// names is NOT deleted — 409 `DeleteConflictException`, nothing removed — however that
/// configuration is shaped and whatever else the CA's registration asked for.
#[tokio::test]
async fn a_template_a_registered_ca_provisions_with_is_not_deleted() {
    let cases = vec![
        Referenced {
            label: "the template name alone, auto registration allowed",
            template_name: "jitp_template",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-one")
                .active(false)
                .auto_registration(true)
                .registration_config(json!({ "templateName": "jitp_template" }))],
        },
        Referenced {
            label: "a configuration naming the provisioning role beside the template",
            template_name: "JITP-Template-2",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-two")
                .auto_registration(true)
                .registration_config(json!({
                    "templateName": "JITP-Template-2",
                    "roleArn": PROVISIONING_ROLE,
                }))],
        },
        Referenced {
            label: "a configuration carrying a body beside the template name",
            template_name: "T3",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-three")
                .active(true)
                .auto_registration(true)
                .registration_config(json!({
                    "templateName": "T3",
                    "templateBody": TEMPLATE_BODY,
                    "roleArn": PROVISIONING_ROLE,
                }))],
        },
        Referenced {
            label: "auto registration left out of the request altogether",
            template_name: "t",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-four")
                .registration_config(json!({ "templateName": "t" }))],
        },
        Referenced {
            label: "auto registration explicitly disabled, the reference still recorded",
            template_name: "9-digit_template-0",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-five")
                .auto_registration(false)
                .registration_config(json!({ "templateName": "9-digit_template-0" }))],
        },
        Referenced {
            label: "a CA registered in DEFAULT mode",
            template_name: "default_mode_template",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-six")
                .mode(CERTIFICATE_MODE_DEFAULT)
                .auto_registration(true)
                .registration_config(json!({ "templateName": "default_mode_template" }))],
        },
        Referenced {
            label: "three CAs provisioning with one template",
            template_name: "shared_template",
            template_type: JITP,
            cas: vec![
                CaRegistration::new("ca-seven")
                    .auto_registration(true)
                    .registration_config(json!({ "templateName": "shared_template" })),
                CaRegistration::new("ca-eight")
                    .auto_registration(false)
                    .registration_config(json!({ "templateName": "shared_template" })),
                CaRegistration::new("ca-nine")
                    .mode(CERTIFICATE_MODE_DEFAULT)
                    .auto_registration(true)
                    .registration_config(json!({
                        "templateName": "shared_template",
                        "roleArn": PROVISIONING_ROLE,
                    })),
            ],
        },
        Referenced {
            // The reference is what holds the template back, not the kind of template:
            // a `FLEET_PROVISIONING` template a CA names is held back the same way.
            label: "a fleet provisioning template a CA names",
            template_name: "fleet_template",
            template_type: FLEET_PROVISIONING,
            cas: vec![CaRegistration::new("ca-ten")
                .auto_registration(true)
                .registration_config(json!({ "templateName": "fleet_template" }))],
        },
        Referenced {
            // The longest name the modelled `TemplateName` admits (`len 1..36`).
            label: "the longest template name the model admits",
            template_name: "abcdefghijklmnopqrstuvwxyz0123456789",
            template_type: JITP,
            cas: vec![CaRegistration::new("ca-eleven")
                .auto_registration(true)
                .registration_config(
                    json!({ "templateName": "abcdefghijklmnopqrstuvwxyz0123456789" }),
                )],
        },
    ];

    let emulator = Emulator::new();
    // One emulator for every case, so each template is created beside the others and a
    // refusal is about the reference rather than about an empty account.
    for case in &cases {
        emulator
            .create_template(case.label, case.template_name, case.template_type)
            .await;
        for registration in &case.cas {
            emulator.register_ca(case.label, registration).await;
        }
    }

    for case in &cases {
        // The refusal holds for as long as ANY CA names the template, so asking twice
        // changes nothing: the delete is a refusal both times.
        for attempt in 1..=2 {
            let refused = emulator.delete_template(case.template_name).await;
            assert_eq!(
                (refused.status, refused.code(), refused.message()),
                (
                    409,
                    "DeleteConflictException".to_string(),
                    associated_with_cas_message(case.template_name)
                ),
                "case {} (attempt {attempt}): expected the delete of {} to be refused 409 \
                 DeleteConflictException naming the template, got {} body {}",
                case.label,
                case.template_name,
                refused.status,
                refused.text()
            );
        }

        // Nothing was removed: the template is still there to read.
        let described = emulator.describe_template(case.template_name).await;
        assert_eq!(
            (
                described.status,
                described
                    .body
                    .get("templateName")
                    .and_then(Value::as_str)
                    .unwrap_or_default()
                    .to_string()
            ),
            (200, case.template_name.to_string()),
            "case {}: a refused delete must leave {} readable, got {} body {}",
            case.label,
            case.template_name,
            described.status,
            described.text()
        );
    }
}

/// One case of the deletable table: why the CA registrations made against the account do
/// NOT reference the template the case deletes.
struct Unreferenced {
    label: &'static str,
    template_name: &'static str,
    cas: Vec<CaRegistration>,
}

/// RULE: a provisioning template NO registered CA certificate's `registrationConfig` names
/// is deleted — 200 with an empty body — after which every read of it is
/// `ResourceNotFoundException` (404). The guard reads the reference, so a CA that carries
/// none, one that names no template, and one that names another template each leave the
/// delete alone.
#[tokio::test]
async fn a_template_no_registered_ca_provisions_with_is_deleted() {
    // The template every "another template" row points a CA at, so the account holds a
    // referencing CA throughout and the successes are not about an empty CA registry.
    let other_template = "other_template";

    let cases = vec![
        Unreferenced {
            label: "no CA registered against the account at all",
            template_name: "unreferenced_template",
            cas: Vec::new(),
        },
        Unreferenced {
            label: "a CA carrying no registrationConfig",
            template_name: "no_configuration",
            cas: vec![CaRegistration::new("free-ca-one").auto_registration(true)],
        },
        Unreferenced {
            label: "a registrationConfig naming only a role",
            template_name: "role_only_configuration",
            cas: vec![CaRegistration::new("free-ca-two")
                .auto_registration(true)
                .registration_config(json!({ "roleArn": PROVISIONING_ROLE }))],
        },
        Unreferenced {
            label: "a registrationConfig naming another template",
            template_name: "named_elsewhere",
            cas: vec![CaRegistration::new("free-ca-three")
                .auto_registration(true)
                .registration_config(json!({ "templateName": other_template }))],
        },
        Unreferenced {
            // `TemplateName`'s pattern `^[0-9A-Za-z_-]+$` admits both cases, and two names
            // differing in case are two templates, so a CA naming one does not hold the
            // other back.
            label: "a registrationConfig naming a name differing only in case",
            template_name: "CaseSensitive",
            cas: vec![CaRegistration::new("free-ca-four")
                .auto_registration(true)
                .registration_config(json!({ "templateName": "casesensitive" }))],
        },
        Unreferenced {
            label: "a registrationConfig naming a name this one is a prefix of",
            template_name: "prefix",
            cas: vec![CaRegistration::new("free-ca-five")
                .auto_registration(true)
                .registration_config(json!({ "templateName": "prefix_extended" }))],
        },
    ];

    let emulator = Emulator::new();
    emulator
        .create_template("the other template", other_template, JITP)
        .await;
    emulator
        .create_template("the prefixed template", "prefix_extended", JITP)
        .await;
    emulator
        .create_template("the lower-case template", "casesensitive", JITP)
        .await;
    for case in &cases {
        emulator
            .create_template(case.label, case.template_name, JITP)
            .await;
        for registration in &case.cas {
            emulator.register_ca(case.label, registration).await;
        }
    }

    for case in &cases {
        let deleted = emulator.delete_template(case.template_name).await;
        assert_eq!(
            (deleted.status, deleted.body.clone()),
            (200, json!({})),
            "case {}: expected the delete of {} to answer an empty 200, got {} body {}",
            case.label,
            case.template_name,
            deleted.status,
            deleted.text()
        );

        // The record is gone, so the reader refuses the name the delete took.
        let described = emulator.describe_template(case.template_name).await;
        assert_eq!(
            (described.status, described.code(), described.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                missing_template_message(case.template_name)
            ),
            "case {}: expected {} to be gone, got {} body {}",
            case.label,
            case.template_name,
            described.status,
            described.text()
        );

        // And the delete of a name the account now holds no template under states its own
        // miss rather than the conflict.
        let again = emulator.delete_template(case.template_name).await;
        assert_eq!(
            (again.status, again.code(), again.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                UNDELETABLE_MISSING_TEMPLATE.to_string()
            ),
            "case {}: expected the second delete of {} to answer 404, got {} body {}",
            case.label,
            case.template_name,
            again.status,
            again.text()
        );
    }

    // The template the rows pointed their CAs at is still held back, so the successes
    // above are about the reference and not about the guard being gone.
    let refused = emulator.delete_template(other_template).await;
    assert_eq!(
        (refused.status, refused.code(), refused.message()),
        (
            409,
            "DeleteConflictException".to_string(),
            associated_with_cas_message(other_template)
        ),
        "case the referenced template: expected 409, got {} body {}",
        refused.status,
        refused.text()
    );
}

/// RULE: the delete states the template's ABSENCE before any reference is read, so a name
/// the account holds no template under is `ResourceNotFoundException` (404) even when a
/// registered CA's `registrationConfig` names that very name — there is nothing to hold
/// back.
#[tokio::test]
async fn a_name_the_account_holds_no_template_under_is_a_not_found() {
    let emulator = Emulator::new();
    emulator
        .register_ca(
            "a CA naming a template nobody created",
            &CaRegistration::new("dangling-ca")
                .auto_registration(true)
                .registration_config(json!({ "templateName": "never_created" })),
        )
        .await;

    for template_name in ["never_created", "nothing_names_this"] {
        let refused = emulator.delete_template(template_name).await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                UNDELETABLE_MISSING_TEMPLATE.to_string()
            ),
            "case {template_name}: expected 404 ResourceNotFoundException, got {} body {}",
            refused.status,
            refused.text()
        );
    }
}

/// RULE: the guard reads the reference the CA registry CURRENTLY holds, so removing the
/// referencing record releases the template: the same delete that was refused while the CA
/// existed succeeds once `DeleteCACertificate` has taken it, and the template is then gone
/// for every reader.
///
/// The rows differ in how many CAs name the template, which is what "one or more" spans: a
/// template two CAs name is released only by the LAST of them.
#[tokio::test]
async fn a_template_is_deletable_once_no_ca_provisions_with_it() {
    let cases: Vec<(&str, &str, Vec<&'static str>)> = vec![
        ("one CA names the template", "released_by_one", vec![
            "releasing-ca-one",
        ]),
        ("two CAs name the template", "released_by_two", vec![
            "releasing-ca-two",
            "releasing-ca-three",
        ]),
    ];

    let emulator = Emulator::new();
    for (label, template_name, subjects) in &cases {
        emulator.create_template(label, template_name, JITP).await;
        let mut registered: Vec<String> = Vec::new();
        for subject in subjects {
            // Registered INACTIVE, which is the state a CA certificate is deletable in.
            registered.push(
                emulator
                    .register_ca(
                        label,
                        &CaRegistration::new(subject)
                            .active(false)
                            .auto_registration(true)
                            .registration_config(json!({ "templateName": template_name })),
                    )
                    .await,
            );
        }

        // While any of them is registered, the delete is refused and nothing is removed.
        for (released, certificate_id) in registered.iter().enumerate() {
            let refused = emulator.delete_template(template_name).await;
            assert_eq!(
                (refused.status, refused.code(), refused.message()),
                (
                    409,
                    "DeleteConflictException".to_string(),
                    associated_with_cas_message(template_name)
                ),
                "case {label} (with {} CA(s) still registered): expected 409, got {} body {}",
                registered.len() - released,
                refused.status,
                refused.text()
            );

            let removed = emulator.delete_ca(certificate_id).await;
            assert_eq!(
                (removed.status, removed.body.clone()),
                (200, json!({})),
                "case {label}: expected the delete of CA {certificate_id} to answer an empty \
                 200, got {} body {}",
                removed.status,
                removed.text()
            );
            // The CA is gone for its own reader too, so the reference it carried is gone
            // with it rather than merely acknowledged.
            let described_ca = emulator.describe_ca(certificate_id).await;
            assert_eq!(
                described_ca.status,
                404,
                "case {label}: expected CA {certificate_id} to be gone, got {} body {}",
                described_ca.status,
                described_ca.text()
            );
        }

        // Nothing names the template any more, so the same delete succeeds and the
        // template is gone for every reader.
        let deleted = emulator.delete_template(template_name).await;
        assert_eq!(
            (deleted.status, deleted.body.clone()),
            (200, json!({})),
            "case {label}: expected the delete of {template_name} to answer an empty 200 once \
             no CA names it, got {} body {}",
            deleted.status,
            deleted.text()
        );
        let described = emulator.describe_template(template_name).await;
        assert_eq!(
            (described.status, described.code(), described.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                missing_template_message(template_name)
            ),
            "case {label}: expected {template_name} to be gone, got {} body {}",
            described.status,
            described.text()
        );
    }
}

/// RULE: `DeleteCACertificate` removes the record the id names once the account's state
/// admits it — an id the CA registry holds nothing under is `ResourceNotFoundException`
/// (404), and a record still `ACTIVE` is `CertificateStateException` (406), with nothing
/// removed either way.
#[tokio::test]
async fn a_ca_certificate_is_deleted_only_when_the_registry_admits_it() {
    let emulator = Emulator::new();

    // An id of the modelled form (`CertificateId`, len 64..64, pattern ^(0x)?[a-fA-F0-9]+$)
    // that the registry holds nothing under, in both cases the pattern admits.
    for unknown in ["0".repeat(64), "ABCDEF0123456789".repeat(4)] {
        let refused = emulator.delete_ca(&unknown).await;
        assert_eq!(
            (refused.status, refused.code(), refused.message()),
            (
                404,
                "ResourceNotFoundException".to_string(),
                MISSING_CA_CERTIFICATE.to_string()
            ),
            "case {unknown}: expected 404 ResourceNotFoundException, got {} body {}",
            refused.status,
            refused.text()
        );
    }

    // A CA registered as ACTIVE is one the account still validates device certificates
    // against, so the delete is refused and the record stays readable.
    let active = emulator
        .register_ca(
            "an active CA",
            &CaRegistration::new("active-ca").active(true),
        )
        .await;
    let refused = emulator.delete_ca(&active).await;
    assert_eq!(
        (refused.status, refused.code(), refused.message()),
        (
            406,
            "CertificateStateException".to_string(),
            format!(
                "CA Certificate must be deactivated (not ACTIVE) before deletion. Id: {active}"
            )
        ),
        "case an active CA: expected 406 CertificateStateException, got {} body {}",
        refused.status,
        refused.text()
    );
    assert_eq!(
        emulator.describe_ca(&active).await.status,
        200,
        "case an active CA: a refused delete must leave the record readable"
    );

    // An INACTIVE one is removed, in both the modes a registration takes.
    for (label, mode) in [
        ("an inactive SNI_ONLY CA", CERTIFICATE_MODE_SNI_ONLY),
        ("an inactive DEFAULT-mode CA", CERTIFICATE_MODE_DEFAULT),
    ] {
        let certificate_id = emulator
            .register_ca(
                label,
                &CaRegistration::new(match mode {
                    CERTIFICATE_MODE_DEFAULT => "inactive-default-ca",
                    _ => "inactive-sni-ca",
                })
                .mode(mode)
                .active(false),
            )
            .await;
        let deleted = emulator.delete_ca(&certificate_id).await;
        assert_eq!(
            (deleted.status, deleted.body.clone()),
            (200, json!({})),
            "case {label}: expected an empty 200, got {} body {}",
            deleted.status,
            deleted.text()
        );
        assert_eq!(
            emulator.describe_ca(&certificate_id).await.status,
            404,
            "case {label}: expected the record to be gone"
        );
    }
}
