// SPDX-License-Identifier: CC0-1.0

//! The `AWS` integration: how a method request is served by making the AWS SERVICE CALL
//! its `uri` names, in process.
//!
//! `IntegrationType` gives `AWS` "for integrating the API method request with an AWS
//! service action, including the Lambda function-invoking action", and
//! `Integration$uri` gives the form that action is named in: "for integrations of the
//! `aws` type, the URI is of the form
//! `arn:aws:apigateway:{region}:{subdomain.service|service}:path|action/{service_api}`".
//! Two parts of it decide everything here — the SUBDOMAIN, which names the service the
//! call is made to, and the resource, which names the call in one of the two documented
//! forms: `path/{resource-path}`, the service's own REST path, or
//! `action/{ActionName}&{parameter}={value}`.
//!
//! The SUBDOMAIN decides which service is called, and each service's own reading of the
//! resource decides which of its calls that is:
//!
//! * AMAZON S3, whose object actions are the ones the `path` form addresses as a path
//!   (`path/{bucket}/{key}`), and whose BUCKET actions are what the same form addresses
//!   when its path names a bucket and nothing under it (`path/{bucket}`);
//! * AMAZON SQS, whose queue actions the `path` form addresses by the queue
//!   (`path/{account-id}/{queue-name}`) and whose action and parameters the integration
//!   request then carries as its own query api's wire form does ([`sqs`]);
//! * AMAZON DYNAMODB, whose actions the `action` form names and whose inputs the
//!   integration request carries in its BODY, as that service's own JSON api's wire form
//!   does ([`dynamodb`]);
//! * AMAZON SNS, whose actions the `action` form names and whose parameters the integration
//!   request then carries as its own query api's wire form does ([`sns`]) — the `path` form
//!   of a `uri` naming it addresses that api's root, which is where every action of it is
//!   reached.
//!
//! The call is made THROUGH THE EMULATOR this service is running within, over the
//! in-process transport `OperationContext::callback` hands a handler: nothing is invented
//! and NO CONNECTION IS OPENED, so what the method request is answered with is what that
//! service itself answered the integration request. The destination is not an address this
//! data plane dials: the `uri`'s own Region label only names which Region's endpoint the
//! client is configured for, the transport serves it in process whatever that label says,
//! and a `uri` naming any other service's action — or a form this module does not read — is
//! left to [`super::perform_integration`]'s refusal, which states the configuration rather
//! than inventing a backend answer. An answer the transport itself refused, rather than the
//! service, is told apart from a backend answer and reported as an integration that reached
//! nothing ([`refusal_of`]).
//!
//! # Who the call is made as
//!
//! `Integration$credentials` is "the credentials required for the integration": a role
//! API Gateway assumes to make the call. So the role decides whether the call happens at
//! all, and both halves of that are IAM's own records, read through the ONE reading of
//! them this tree has:
//!
//! * the role has to be one this service can assume — `sts:AssumeRole` granted to
//!   [`SERVICE_PRINCIPAL`] by the role's trust policy
//!   ([`::service_common::iam_role::role_is_unassumable`]). A role nothing can assume
//!   makes no call, so the integration could not be performed at all and is answered
//!   through the api's `API_CONFIGURATION_ERROR` gateway response, exactly as every other
//!   integration this stage cannot perform is;
//! * the call it would make has to be one the role's own identity-based policies allow on
//!   the resource it addresses ([`::service_common::iam_authorization`], the tree's one
//!   evaluation of policy documents). A call they do not allow is REFUSED, and it is
//!   refused the way the service it was addressed to refuses an unauthorized call: HTTP
//!   403 with the `AccessDenied` code and the sentence that service publishes for it. That
//!   refusal is then mapped like any other backend answer (below), which is what lets an
//!   api that declares an integration response for the 4xx class answer its caller 403 and
//!   one that declares none answer through its default.
//!
//! Nothing is refused on evidence this emulator does not have: a `credentials` value
//! naming no role of this account, an emulator with no IAM mounted, and an enumeration of
//! the role's policies that could not be read all leave the call to be made, for the
//! reason `::service_common::iam_role` gives. The documented "caller's identity"
//! spelling ([`CALLER_CREDENTIALS`]) names no role to assume at all.
//!
//! What never travels is the CALLER'S OWN credential. The integration request is signed
//! with the credentials the in-process callback hands this handler, which is what a role
//! the integration names is resolved against; an `Authorization` header (or a session
//! token) the client presented is not carried onto a call to a destination that client did
//! not choose, and neither it nor the signing credentials are reported or logged.
//!
//! # How the answer becomes the method response
//!
//! A non-proxy integration's answer is MAPPED, and `IntegrationResponse$selectionPattern`
//! says how: "the regular expression (regex) pattern used to choose an integration
//! response based on the response from the back end … If the back end is an AWS Lambda
//! function, the AWS Lambda function error header is matched. For all other HTTP and AWS
//! back ends, the HTTP status code is matched." So the pattern is matched against the
//! status the service answered with, the integration response with no pattern is the
//! fallback ([`super::default_integration_response`]), and the `statusCode` of the one
//! selected is the status the CALLER sees. Its response template renders the body when it
//! declares one, read off what the backend answered; with no template the backend's own
//! bytes and content type are what the caller gets, which is what passing a response
//! through means.

use std::collections::BTreeMap;

use ::aws_smithy_runtime_api::client::orchestrator::HttpResponse;
use ::aws_smithy_runtime_api::client::result::SdkError;
use ::aws_smithy_types::error::metadata::ProvideErrorMetadata;
use ::service_common::iam_authorization::{self as iam, Decision};
use ::smithy_server_runtime::OperationContext;

use crate::ops::put_integration_response::is_static_mapping_value;
use crate::ops::types::{GetIntegrationOutput, GetIntegrationResponseOutput};
use crate::state::MethodEntry;

use super::super::mapping_template::TemplateInputs;
use super::{
    default_integration_response, mapped_response_headers, request_mapping,
    rendered_mapping_template, response_body, IntegratedResponse, MethodRequest, RequestMapping,
    UnservedIntegration, API_CONFIGURATION_ERROR, JSON_CONTENT_TYPE, METHOD_REQUEST_HEADER_PREFIX,
    METHOD_REQUEST_PATH_PREFIX, METHOD_REQUEST_QUERY_PREFIX, UNSUPPORTED_MEDIA_TYPE,
};

mod dynamodb;
mod query_form;
mod sns;
mod sqs;

/// The integration type whose `uri` names an AWS service action: `AWS`, "for integrating
/// the API method request with an AWS service action" (`IntegrationType`). `AWS_PROXY` is
/// the Lambda proxy type and is not this one.
const AWS_SERVICE_INTEGRATION_TYPE: &str = "AWS";

/// The vendor field of every AWS integration ARN, and the resource prefix of each of the
/// two forms its resource takes — read from the operation that VALIDATES an integration's
/// `uri` ([`crate::ops::put_integration`]), so the call that stores an ARN and this plane,
/// which performs it, agree on what one is.
use crate::ops::put_integration::{ACTION_FORM_PREFIX, APIGATEWAY_VENDOR, PATH_FORM_PREFIX};

/// The subdomain field that names Amazon S3, one of the services whose calls this module
/// makes.
const S3_SUBDOMAIN: &str = "s3";

/// What Amazon S3 is called in the sentence an integration that reached no answer carries.
const S3_SERVICE_NAME: &str = "Amazon S3";

/// The `Bucket` parameter of the action form, as the Amazon S3 API names it.
const BUCKET_PARAMETER: &str = "Bucket";

/// The `Key` parameter of the action form, as the Amazon S3 API names it.
const KEY_PARAMETER: &str = "Key";

/// The mapping-expression prefix of a PATH parameter of the INTEGRATION request, which is
/// how `Integration$requestParameters` names the value a `{placeholder}` in the `uri`
/// stands for: `integration.request.path.{name}`.
const INTEGRATION_REQUEST_PATH_PREFIX: &str = "integration.request.path.";

/// The service principal an integration's execution role has to trust for API Gateway to
/// make the call under it, as a trust document names one.
const SERVICE_PRINCIPAL: &str = "apigateway.amazonaws.com";

/// The `credentials` value that asks for the CALLER's own identity to be passed through
/// rather than for a role to be assumed: `Integration$credentials` documents it as "to
/// require that the caller's identity be passed through from the request, specify the
/// string `arn:aws:iam::\*:user/\*`". It names no role, so there is no role to resolve.
const CALLER_CREDENTIALS: &str = "arn:aws:iam::*:user/*";

/// The error code an AWS service refuses an unauthorized call with, and the sentence
/// Amazon S3 publishes beside it ("AccessDenied — Access Denied", Amazon S3 "Error
/// responses").
const ACCESS_DENIED_CODE: &str = "AccessDenied";

/// See [`ACCESS_DENIED_CODE`].
const ACCESS_DENIED_MESSAGE: &str = "Access Denied";

/// The HTTP status that refusal is answered with (Amazon S3 "Error responses": 403
/// Forbidden).
const ACCESS_DENIED_STATUS: u16 = 403;

/// The media type an Amazon S3 REST error body is XML under.
const XML_CONTENT_TYPE: &str = "application/xml";

/// The most bytes one integration answer may carry into a method response: API Gateway's
/// published quota for a payload is 10 MB, so an answer past it is one this stage cannot
/// pass on rather than one buffered without bound.
const MAX_INTEGRATION_PAYLOAD_BYTES: usize = 10 * 1024 * 1024;

/// One Amazon S3 OBJECT action, which is what the `path` form of an integration `uri`
/// addresses as `path/{bucket}/{key}`.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ObjectAction {
    /// `GetObject`: the object's bytes.
    Get,
    /// `HeadObject`: the object's metadata, with no body.
    Head,
    /// `PutObject`: the method request's payload becomes the object.
    Put,
    /// `DeleteObject`: the object is removed.
    Delete,
}

impl ObjectAction {
    /// The action the integration's own HTTP method names, which is what the PATH form
    /// carries instead of an action name: `Integration$httpMethod` is "the integration's
    /// HTTP method type", and an object is read with `GET`, headed with `HEAD`, written
    /// with `PUT` and removed with `DELETE`.
    fn of_verb(verb: &str) -> Option<Self> {
        match verb.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::Get),
            "HEAD" => Some(Self::Head),
            "PUT" => Some(Self::Put),
            "DELETE" => Some(Self::Delete),
            _ => None,
        }
    }

    /// The action the ACTION form names, by the Amazon S3 operation's own name.
    fn of_action_name(name: &str) -> Option<Self> {
        match name {
            "GetObject" => Some(Self::Get),
            "HeadObject" => Some(Self::Head),
            "PutObject" => Some(Self::Put),
            "DeleteObject" => Some(Self::Delete),
            _ => None,
        }
    }

    /// The IAM action a call of it is authorized by. `HeadObject` is authorized by
    /// `s3:GetObject` — "You need the `s3:GetObject` permission for this operation"
    /// (`HeadObject`'s documentation) — so the two share one action.
    fn iam_action(self) -> &'static str {
        match self {
            Self::Get | Self::Head => "s3:GetObject",
            Self::Put => "s3:PutObject",
            Self::Delete => "s3:DeleteObject",
        }
    }
}

/// What an `AWS` integration addresses, as its `uri` resolves against this call: the
/// object, the Region of the service endpoint the call is made at, and the action.
#[derive(Debug)]
struct ObjectCall {
    /// The partition the integration ARN names, which the object's own ARN carries.
    partition: String,
    /// The Region the integration ARN names, or the api's own when it names none.
    region: String,
    bucket: String,
    key: String,
    action: ObjectAction,
}

impl ObjectCall {
    /// The ARN of the object this call addresses, which is the resource an identity-based
    /// policy grants the action on: an Amazon S3 object ARN carries neither Region nor
    /// account (`arn:{partition}:s3:::{bucket}/{key}`).
    fn object_arn(&self) -> String {
        format!(
            "arn:{partition}:s3:::{bucket}/{key}",
            partition = self.partition,
            bucket = self.bucket,
            key = self.key
        )
    }
}

/// One Amazon S3 BUCKET action, which is what the `path` form addresses when its resource
/// path names a bucket and nothing under it (`path/{bucket}` — Amazon S3's own REST path
/// for the bucket itself).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum BucketAction {
    /// `ListObjects`, the call Amazon S3's REST API binds to `GET /{Bucket}`: the bucket's
    /// own listing, answered as the `ListBucketResult` document
    /// (`smithy_query(query="operation",operation="ListObjects")`).
    List,
    /// `HeadBucket`, bound to `HEAD /{Bucket}`: whether the bucket is there and reachable,
    /// with no body.
    Head,
}

impl BucketAction {
    /// The action the integration's own HTTP method names, as the PATH form carries it:
    /// Amazon S3 binds the bucket's listing to `GET` and its head to `HEAD`.
    fn of_verb(verb: &str) -> Option<Self> {
        match verb.to_ascii_uppercase().as_str() {
            "GET" => Some(Self::List),
            "HEAD" => Some(Self::Head),
            _ => None,
        }
    }

    /// The action the ACTION form names, by the Amazon S3 operation's own name.
    fn of_action_name(name: &str) -> Option<Self> {
        match name {
            "ListObjects" => Some(Self::List),
            "HeadBucket" => Some(Self::Head),
            _ => None,
        }
    }

    /// The HTTP method Amazon S3's REST API binds this action to.
    fn verb(self) -> &'static str {
        match self {
            Self::List => "GET",
            Self::Head => "HEAD",
        }
    }

    /// The IAM action a call of it is authorized by. Both are `s3:ListBucket`:
    /// `ListObjects` needs "the `s3:ListBucket` permission", and `HeadBucket` documents
    /// "you must have permissions to perform the `s3:ListBucket` action".
    fn iam_action(self) -> &'static str {
        "s3:ListBucket"
    }
}

/// What an `AWS` integration addresses when its `uri` names a BUCKET rather than an object:
/// the bucket, the Region of the service endpoint the call is made at, and the action.
#[derive(Debug)]
struct BucketCall {
    /// The partition the integration ARN names, which the bucket's own ARN carries.
    partition: String,
    /// The Region the integration ARN names, or the api's own when it names none.
    region: String,
    bucket: String,
    action: BucketAction,
}

impl BucketCall {
    /// The ARN of the bucket this call addresses, which is the resource an identity-based
    /// policy grants the action on: an Amazon S3 bucket ARN carries neither Region nor
    /// account, and names no key (`arn:{partition}:s3:::{bucket}`).
    fn bucket_arn(&self) -> String {
        format!(
            "arn:{partition}:s3:::{bucket}",
            partition = self.partition,
            bucket = self.bucket
        )
    }
}

/// What the AWS service answered the integration request: the three things a mapped
/// method response is built from.
#[derive(Debug)]
struct Answered {
    status: u16,
    /// The content type the answer declared, or `None` for one that declared none.
    content_type: Option<String>,
    body: Vec<u8>,
}

/// Amazon S3's REST error body: the code, the sentence and the resource the call
/// addressed, which is the envelope every Amazon S3 refusal carries.
fn error_body(code: &str, message: &str, resource: &str) -> Vec<u8> {
    format!(
        "<?xml version=\"1.0\" encoding=\"UTF-8\"?>\n<Error><Code>{code}</Code>\
         <Message>{message}</Message><Resource>{resource}</Resource></Error>"
    )
    .into_bytes()
}

/// Perform this call through the method's `AWS` integration, or `None` when the
/// integration is not an AWS service call this module makes — in which case the caller
/// performs it from the api's own records ([`super::perform_integration`]).
///
/// `region` is the api's own Region, which is the endpoint the call is made at when the
/// integration ARN names none of its own.
pub(super) async fn performed(
    ctx: &OperationContext<'_>,
    region: &str,
    method: &MethodEntry,
    call: &MethodRequest,
) -> Option<Result<IntegratedResponse, UnservedIntegration>> {
    let integration = method.integration.as_ref()?;
    if integration.r#type.as_deref() != Some(AWS_SERVICE_INTEGRATION_TYPE) {
        return None;
    }
    let addressed = addressed_call(integration, region, call)?;
    let target = match addressed {
        Ok(target) => target,
        Err(unserved) => return Some(Err(unserved)),
    };
    Some(served(ctx, integration, method, call, &target).await)
}

/// What `integration`'s `uri` addresses, over the services this module makes calls to, or
/// `None` when it names none of them — which leaves it to the caller's own refusal.
///
/// The SUBDOMAIN of the integration ARN names the service, so each service's own reader is
/// asked in turn and the first that claims the `uri` decides what the call is; a `uri` that
/// IS that service's and still places no call is a configuration this stage cannot serve
/// rather than one left to another reader.
fn addressed_call(
    integration: &GetIntegrationOutput,
    default_region: &str,
    call: &MethodRequest,
) -> Option<Result<ServiceCall, UnservedIntegration>> {
    if let Some(s3) = addressed_s3(integration, default_region, &call.inputs) {
        return Some(
            s3.map(|call| match call {
                S3Call::Object(object) => ServiceCall::Object(object),
                S3Call::Bucket(bucket) => ServiceCall::Bucket(bucket),
            })
            .map_err(|reason| unanswered(&reason)),
        );
    }
    if let Some(queue) = sqs::addressed_queue(integration, default_region, call) {
        return Some(queue.map(ServiceCall::Queue));
    }
    if let Some(table) = dynamodb::addressed_table(integration, default_region, call) {
        return Some(table.map(ServiceCall::Table));
    }
    if let Some(topic) = sns::addressed_topic(integration, default_region, call) {
        return Some(topic.map(ServiceCall::Topic));
    }
    None
}

/// The call an `AWS` integration's `uri` addresses, in whichever of the services this
/// module makes calls to. Everything the role check, the dispatch and the refusal need of
/// one is read through this, so a service is added by teaching it one more call rather than
/// by a second copy of those rules.
#[derive(Debug)]
enum ServiceCall {
    /// An Amazon S3 object action.
    Object(ObjectCall),
    /// An Amazon S3 bucket action.
    Bucket(BucketCall),
    /// An Amazon SQS queue action.
    Queue(sqs::QueueCall),
    /// An Amazon DynamoDB table action.
    Table(dynamodb::TableCall),
    /// An Amazon SNS topic action.
    Topic(sns::TopicCall),
}

/// What an Amazon S3 integration `uri` addresses: the bucket itself, or an object in it.
#[derive(Debug)]
enum S3Call {
    Object(ObjectCall),
    Bucket(BucketCall),
}

impl ServiceCall {
    /// The Region of the service endpoint the call is made at.
    fn region(&self) -> &str {
        match self {
            Self::Object(object) => &object.region,
            Self::Bucket(bucket) => &bucket.region,
            Self::Queue(queue) => queue.region(),
            Self::Table(table) => table.region(),
            Self::Topic(topic) => topic.region(),
        }
    }

    /// The IAM action a call of it is authorized by.
    ///
    /// Owned, because one of these services names the action the `uri` CARRIES rather than one
    /// of a fixed set: Amazon DynamoDB's api is reached by naming its operation, so the action
    /// a call of it is authorized by is that operation's own name.
    fn iam_action(&self) -> String {
        match self {
            Self::Object(object) => object.action.iam_action().to_string(),
            Self::Bucket(bucket) => bucket.action.iam_action().to_string(),
            Self::Queue(queue) => queue.iam_action().to_string(),
            Self::Table(table) => table.iam_action(),
            Self::Topic(topic) => topic.iam_action(),
        }
    }

    /// The ARN of the resource it addresses, which is what an identity-based policy grants
    /// that action on.
    fn resource_arn(&self) -> String {
        match self {
            Self::Object(object) => object.object_arn(),
            Self::Bucket(bucket) => bucket.bucket_arn(),
            Self::Queue(queue) => queue.queue_arn(),
            Self::Table(table) => table.table_arn(),
            Self::Topic(topic) => topic.topic_arn(),
        }
    }

    /// The refusal the service answers a call the caller may not make with, in that
    /// service's own error envelope and naming the resource the call addressed.
    ///
    /// `principal` is the role the call would have been made under, for the service that words
    /// its refusal with the principal as IAM does; the services whose envelope names only the
    /// resource do not carry it.
    fn access_denied(&self, principal: &str) -> Answered {
        match self {
            Self::Object(object) => Answered {
                status: ACCESS_DENIED_STATUS,
                content_type: Some(XML_CONTENT_TYPE.to_string()),
                body: error_body(
                    ACCESS_DENIED_CODE,
                    ACCESS_DENIED_MESSAGE,
                    &format!("/{}/{}", object.bucket, object.key),
                ),
            },
            Self::Bucket(bucket) => Answered {
                status: ACCESS_DENIED_STATUS,
                content_type: Some(XML_CONTENT_TYPE.to_string()),
                body: error_body(
                    ACCESS_DENIED_CODE,
                    ACCESS_DENIED_MESSAGE,
                    &format!("/{}", bucket.bucket),
                ),
            },
            Self::Queue(queue) => sqs::access_denied(queue),
            Self::Table(table) => dynamodb::access_denied(table, principal),
            Self::Topic(topic) => sns::access_denied(topic, principal),
        }
    }

    /// Make the call, in process, and report what the service answered.
    async fn answered(
        &self,
        ctx: &OperationContext<'_>,
        call: &MethodRequest,
    ) -> Result<Answered, UnservedIntegration> {
        match self {
            Self::Object(object) => answered_by_s3(ctx, object, call).await,
            Self::Bucket(bucket) => answered_by_s3_bucket(ctx, bucket).await,
            Self::Queue(queue) => sqs::answered(ctx, queue, call).await,
            Self::Table(table) => dynamodb::answered(ctx, table).await,
            Self::Topic(topic) => sns::answered(ctx, topic).await,
        }
    }
}

/// The method response this call earns: the role decides whether the call is made, the
/// service answers it, and the integration response maps that answer.
async fn served(
    ctx: &OperationContext<'_>,
    integration: &GetIntegrationOutput,
    method: &MethodEntry,
    call: &MethodRequest,
    target: &ServiceCall,
) -> Result<IntegratedResponse, UnservedIntegration> {
    let answered = match authorization(ctx, integration, target).await {
        Authorization::Unassumable(reason) => return Err(unanswered(&reason)),
        Authorization::Withheld => {
            target.access_denied(integration.credentials.as_deref().unwrap_or_default())
        }
        Authorization::Admitted => target.answered(ctx, call).await?,
    };
    mapped(method, integration, call, answered)
}

/// What the integration's execution role makes of this call.
#[derive(Debug)]
enum Authorization {
    /// There is a role to make the call under, and the policies in force on it allow the
    /// call — or nothing about the role could be shown, which refuses nothing.
    Admitted,
    /// The role cannot be assumed by this service at all, so no call is made: the
    /// sentence says which role and why, for the log and for the execution log
    /// `TestInvokeMethod` reports.
    Unassumable(String),
    /// The role may not make this call, so the service refuses it in its own words
    /// ([`ServiceCall::access_denied`]).
    Withheld,
}

/// What the integration's `credentials` role makes of the call `target` addresses.
async fn authorization(
    ctx: &OperationContext<'_>,
    integration: &GetIntegrationOutput,
    target: &ServiceCall,
) -> Authorization {
    let role_arn = integration
        .credentials
        .as_deref()
        .map(str::trim)
        .filter(|credentials| !credentials.is_empty() && *credentials != CALLER_CREDENTIALS);
    let Some(role_arn) = role_arn else {
        return Authorization::Admitted;
    };
    if ::service_common::iam_role::role_is_unassumable(
        ctx,
        target.region(),
        role_arn,
        SERVICE_PRINCIPAL,
    )
    .await
    {
        return Authorization::Unassumable(format!(
            "the integration's execution role {role_arn} does not let {SERVICE_PRINCIPAL} \
             assume it, so no call can be made under it"
        ));
    }
    // The permissions in force on the role are what IAM holds for it; a role whose
    // policies could not be READ grounds no refusal, so only an enumeration IAM ANSWERED
    // decides this.
    let Some(documents) = role_identity_documents(ctx, target.region(), role_arn).await else {
        return Authorization::Admitted;
    };
    let resource = target.resource_arn();
    let action = target.iam_action();
    let request = iam::Request::new(&action, &resource, &[]);
    if iam::identity_decision(&documents, &request) == Decision::Allowed {
        return Authorization::Admitted;
    }
    ::tracing::warn!(
        %action,
        "the integration's execution role may not make this call, so the service refuses it"
    );
    Authorization::Withheld
}

/// Every identity-based policy document IAM holds for the role `role_arn` names — the
/// documents of the managed policies attached to it, and its inline policies — or `None`
/// when this emulator could not read them at all (there is no IAM to ask, the value names
/// no role of this account, or an answer could not be read), which grounds no refusal.
///
/// An enumeration that succeeds and reports NO policy is a role that carries none, which
/// is an answer rather than a silence: it is reported as the empty set, and a call no
/// policy allows is withheld.
async fn role_identity_documents(
    ctx: &OperationContext<'_>,
    region: &str,
    role_arn: &str,
) -> Option<Vec<String>> {
    let role_name = ::service_common::iam_role::role_name(role_arn)?;
    let iam = ::service_common::iam_client::iam_client(ctx, region);
    let mut documents = Vec::new();

    let attached = iam
        .list_attached_role_policies()
        .role_name(role_name)
        .send()
        .await
        .ok()?;
    for policy in attached.attached_policies() {
        let policy_arn = policy.policy_arn()?;
        let described = iam.get_policy().policy_arn(policy_arn).send().await.ok()?;
        let version_id = described.policy()?.default_version_id()?;
        let version = iam
            .get_policy_version()
            .policy_arn(policy_arn)
            .version_id(version_id)
            .send()
            .await
            .ok()?;
        documents.push(version.policy_version()?.document()?.to_string());
    }

    let inline = iam
        .list_role_policies()
        .role_name(role_name)
        .send()
        .await
        .ok()?;
    for policy_name in inline.policy_names() {
        let policy = iam
            .get_role_policy()
            .role_name(role_name)
            .policy_name(policy_name)
            .send()
            .await
            .ok()?;
        documents.push(policy.policy_document().to_string());
    }

    Some(documents)
}

/// Make the call `object` addresses against Amazon S3, in process, and report what it
/// answered.
///
/// `Err` only where there is no answer to report at all — the call did not reach the
/// service, or its answer is larger than one method response may carry — because an
/// integration that produced no answer is one this stage could not perform. Every answer
/// the service DID give, refusals included, is reported as [`Answered`] and mapped.
async fn answered_by_s3(
    ctx: &OperationContext<'_>,
    object: &ObjectCall,
    call: &MethodRequest,
) -> Result<Answered, UnservedIntegration> {
    let client = s3_client(ctx, &object.region);
    match object.action {
        ObjectAction::Get => match client
            .get_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .send()
            .await
        {
            Ok(answer) => {
                let content_type = answer.content_type().map(str::to_string);
                // The CLAMP PRECEDES the allocation: an answer that declares more bytes
                // than one method response carries is refused before any of them is
                // buffered, and what IS collected is bounded again below, for an answer
                // that declared no length at all.
                if let Some(declared) = answer.content_length() {
                    bounded(usize::try_from(declared).unwrap_or(usize::MAX))?;
                }
                let collected = answer.body.collect().await.map_err(|error| {
                    ::tracing::warn!(
                        error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
                        "the object the integration read could not be collected"
                    );
                    unanswered("the object's bytes could not be read in full")
                })?;
                let body = collected.into_bytes().to_vec();
                bounded(body.len())?;
                Ok(Answered {
                    status: 200,
                    content_type,
                    body,
                })
            }
            Err(error) => refused(&error, object),
        },
        ObjectAction::Head => match client
            .head_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .send()
            .await
        {
            Ok(answer) => Ok(Answered {
                status: 200,
                content_type: answer.content_type().map(str::to_string),
                body: Vec::new(),
            }),
            Err(error) => refused(&error, object),
        },
        ObjectAction::Put => {
            let payload = call.inputs.payload.clone().unwrap_or_default();
            bounded(payload.len())?;
            let mut put = client
                .put_object()
                .bucket(&object.bucket)
                .key(&object.key)
                .body(::aws_sdk_s3::primitives::ByteStream::from(payload));
            // The integration request carries the method request's own media type, which
            // is what the object is then stored and served under.
            if !call.media_type().is_empty() {
                put = put.content_type(call.media_type());
            }
            match put.send().await {
                Ok(_) => Ok(Answered {
                    status: 200,
                    content_type: None,
                    body: Vec::new(),
                }),
                Err(error) => refused(&error, object),
            }
        }
        ObjectAction::Delete => match client
            .delete_object()
            .bucket(&object.bucket)
            .key(&object.key)
            .send()
            .await
        {
            // Amazon S3 answers a delete `204 No Content`, which carries no body.
            Ok(_) => Ok(Answered {
                status: 204,
                content_type: None,
                body: Vec::new(),
            }),
            Err(error) => refused(&error, object),
        },
    }
}

/// Make the call `bucket` addresses against Amazon S3, in process, and report what it
/// answered.
///
/// A bucket call is made as Amazon S3's REST API binds it — the method and the bucket's own
/// path, signed for `s3` in the call's Region — and what the method response carries is the
/// service's OWN answer: its status, its media type and its bytes, which for a listing is
/// the `ListBucketResult` document Amazon S3 renders. The request is dispatched over the
/// in-process callback transport, so it is served by the Amazon S3 this emulator mounts and
/// no connection is opened; the signature is the tree's one in-process signer, with the
/// credential the callback hands this handler, which is never reported or logged.
///
/// `Err` only where there is no answer FROM THE SERVICE to report at all — the request could
/// not be built or signed, it reached no Amazon S3 in this emulator, or the answer is larger
/// than one method response may carry. Every answer the service DID give, refusals included,
/// is reported as [`Answered`] and mapped.
async fn answered_by_s3_bucket(
    ctx: &OperationContext<'_>,
    bucket: &BucketCall,
) -> Result<Answered, UnservedIntegration> {
    // The endpoint every in-process call to Amazon S3 is made at, with the bucket in the
    // PATH: this emulator serves one endpoint per service, so a per-bucket host would name
    // an address its router could not resolve ([`s3_client`] configures the same).
    let host = format!("s3.{}.amazonaws.com", bucket.region);
    let built = ::http::Request::builder()
        .method(bucket.action.verb())
        .uri(format!("https://{host}/{}", bucket.bucket))
        .header(::http::header::HOST, &host)
        .body(Vec::new());
    let request = built.map_err(|error| {
        ::tracing::warn!(%error, "the integration's Amazon S3 bucket call does not render an \
                                  HTTP request");
        unanswered(
            "the integration's uri does not render a request to Amazon S3, so no call was made",
        )
    })?;
    answered_in_process(
        ctx,
        request,
        S3_SUBDOMAIN,
        &bucket.region,
        S3_SERVICE_NAME,
        // Amazon S3's REST answers declare their own media type, and an answer that declared
        // none is reported as carrying none.
        None,
    )
    .await
}

/// An integration whose answer this stage could not produce at all, for the reason
/// `reason` gives.
fn unanswered(reason: &str) -> UnservedIntegration {
    UnservedIntegration {
        refusal: API_CONFIGURATION_ERROR,
        reason: reason.to_string(),
    }
}

/// The body the integration request carries: the request template this call selects RENDERED,
/// which is the text API Gateway sends the backend, or the method request's own payload where
/// no template maps it and the `passthroughBehavior` lets it through.
///
/// One reading for every service this module calls, because it is the one rule
/// `Integration$requestTemplates` and `Integration$passthroughBehavior` state between them,
/// whatever wire form the rendered text is in.
fn integration_request_body(
    integration: &GetIntegrationOutput,
    call: &MethodRequest,
) -> Result<Vec<u8>, UnservedIntegration> {
    let body = match request_mapping(integration, call) {
        RequestMapping::Mapped(template) => rendered_mapping_template(template, &call.inputs)
            .map(String::into_bytes)
            .map_err(|reason| {
                unanswered(&format!(
                    "the {media} request template of {bytes} bytes is not one this emulator \
                     renders: {reason}",
                    media = call.media_type(),
                    bytes = template.len()
                ))
            })?,
        RequestMapping::PassedThrough => call.inputs.payload.clone().unwrap_or_default(),
        RequestMapping::Unsupported => {
            return Err(UnservedIntegration {
                refusal: UNSUPPORTED_MEDIA_TYPE,
                reason: format!(
                    "no integration request template maps {}, and the integration's \
                     passthroughBehavior does not pass an unmapped body through",
                    call.media_type()
                ),
            })
        }
    };
    bounded(body.len())?;
    Ok(body)
}

/// `Ok` while `bytes` is within what one integration answer may carry
/// ([`MAX_INTEGRATION_PAYLOAD_BYTES`]).
fn bounded(bytes: usize) -> Result<(), UnservedIntegration> {
    if bytes > MAX_INTEGRATION_PAYLOAD_BYTES {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the integration's payload of {bytes} bytes is larger than the \
                 {MAX_INTEGRATION_PAYLOAD_BYTES} bytes one method response carries"
            ),
        });
    }
    Ok(())
}

/// Make `request` against the AWS service `service`, in process, and report what it
/// answered.
///
/// The ONE dispatch of every call this module places by the service's own WIRE FORM rather
/// than through an SDK client: the request is signed for `signing_name` in `region` by the
/// tree's one in-process signer, with the credential the callback hands this handler — which
/// is never reported or logged — and dispatched over the in-process callback transport, so it
/// is served by the service this emulator mounts and no connection is opened. What the method
/// response carries is then the service's OWN answer: its status, its media type and its
/// bytes.
///
/// `declared` is the media type to report for an answer that declared none, which is the one
/// that service's protocol answers in.
///
/// `Err` only where there is no answer FROM THE SERVICE to report at all — the request could
/// not be signed or rendered, it reached no such service in this emulator, or the answer is
/// larger than one method response may carry. Every answer the service DID give, refusals
/// included, is reported as [`Answered`] and mapped.
async fn answered_in_process(
    ctx: &OperationContext<'_>,
    mut request: ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    service: &str,
    declared: Option<&str>,
) -> Result<Answered, UnservedIntegration> {
    ::service_common::in_process_sigv4::sign_in_process_request(
        &mut request,
        signing_name,
        region,
        ctx,
    )
    .await
    .map_err(|error| {
        ::tracing::warn!(%error, service, "the integration's call could not be signed");
        unanswered(&format!(
            "the integration request to {service} could not be signed, so no call was made"
        ))
    })?;
    let (parts, body) = request.into_parts();
    let request = ::aws_smithy_runtime_api::http::Request::try_from(::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    ))
    .map_err(|error| {
        ::tracing::warn!(
            %error,
            service,
            "the integration's call does not render an HTTP request"
        );
        unanswered(&format!(
            "the integration's uri does not render a request to {service}, so no call was made"
        ))
    })?;
    let answer = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &ctx.callback().http_connector(),
        request,
    )
    .await
    .map_err(|error| {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(&error),
            service,
            "the integration request was not answered by the service its uri names"
        );
        unanswered(&format!(
            "the integration request to {service} was not answered, so there is no backend \
             answer to map"
        ))
    })?;
    let status = answer.status().as_u16();
    let content_type = answer
        .headers()
        .get(::http::header::CONTENT_TYPE.as_str())
        .map(str::to_string);
    let payload = answer.body().bytes().unwrap_or_default().to_vec();
    // The emulator's own FRONT DOOR refusal is not an answer from the service: a deployment
    // with that service not mounted answers the integration request that way, and mapping it
    // as a backend answer would hand the caller a status and a body for a call nothing made.
    // `service_common` owns the one reading of that refusal.
    if ::service_common::in_process_refusal::is_transport_refusal(&payload) {
        ::tracing::warn!(
            status,
            service,
            "the integration request reached no such service: this emulator mounts none, so \
             there is no backend answer to map"
        );
        return Err(unanswered(&format!(
            "the integration request reached no {service} in this emulator, so there is no \
             backend answer to map"
        )));
    }
    bounded(payload.len())?;
    Ok(Answered {
        status,
        content_type: content_type.or_else(|| declared.map(str::to_string)),
        body: payload,
    })
}

/// The SERVICE the fifth field of an integration ARN names, from the two spellings
/// `Integration$uri` documents for it: `{subdomain.service|service}`. So the service is the
/// field itself, or the label after its last dot where the `uri` carries a subdomain in
/// front of it — which names the same service either way.
fn named_service(field: &str) -> &str {
    match field.rsplit_once('.') {
        Some((_, service)) => service,
        None => field,
    }
}

/// What the service answered a call it REFUSED, read off the answer itself: the status it
/// sent, the media type and the bytes it sent, the code and sentence it reported, and the
/// headers it carried — for the service's own renderer to shape into the body a mapped
/// method response carries.
///
/// `Err` only where there is no answer FROM THE SERVICE to report at all — the call did not
/// reach it, its answer could not be read, or the answer came from the front door rather
/// than from the service — because an integration that produced no answer is one this stage
/// could not perform rather than one refused by something nothing asked.
///
/// The FRONT DOOR's own refusal is not an answer from the service: a deployment with that
/// service not mounted answers the integration request that way, and mapping it as a
/// backend answer would hand the caller a status and a body for a call nothing made.
/// `service_common` owns the one reading of that refusal.
fn refusal_of<E>(
    error: &SdkError<E, HttpResponse>,
    service: &str,
) -> Result<Refusal, UnservedIntegration>
where
    E: ProvideErrorMetadata + ::std::error::Error + 'static,
{
    let SdkError::ServiceError(answered) = error else {
        ::tracing::warn!(
            error = %::aws_smithy_types::error::display::DisplayErrorContext(error),
            "the integration request was not answered by the service its uri names"
        );
        return Err(unanswered(&format!(
            "the integration request to {service} was not answered, so there is no backend \
             answer to map"
        )));
    };
    let answer = answered.raw();
    // The body the service sent, when the answer carried one that was buffered; a refusal
    // whose body was not is left empty, and each service's renderer writes its own envelope
    // from the code and sentence the answer itself reported.
    let body = answer.body().bytes().unwrap_or_default().to_vec();
    if ::service_common::in_process_refusal::is_transport_refusal(&body) {
        ::tracing::warn!(
            status = answer.status().as_u16(),
            "the integration request reached no such service: this emulator mounts none, so \
             there is no backend answer to map"
        );
        return Err(unanswered(&format!(
            "the integration request reached no {service} in this emulator, so there is no \
             backend answer to map"
        )));
    }
    Ok(Refusal {
        status: answer.status().as_u16(),
        content_type: answer
            .headers()
            .get(::http::header::CONTENT_TYPE.as_str())
            .map(str::to_string),
        code: answered.err().code().map(str::to_string),
        message: answered.err().message().map(str::to_string),
        headers: answer
            .headers()
            .iter()
            .map(|(name, value)| (name.to_ascii_lowercase(), value.to_string()))
            .collect(),
        body,
    })
}

/// A refusal the service itself answered with, as [`refusal_of`] read it off the answer.
#[derive(Debug)]
struct Refusal {
    status: u16,
    /// The media type the answer declared, or `None` for one that declared none.
    content_type: Option<String>,
    /// The error code the answer reported, or `None` for one that reported none.
    code: Option<String>,
    /// The sentence it reported beside that code.
    message: Option<String>,
    /// The bytes the answer carried, empty for one that carried none.
    body: Vec<u8>,
    /// Every header the answer carried, by lowercased name — which is where a service whose
    /// model is `awsQueryCompatible` reports the legacy query code of the class it threw.
    headers: BTreeMap<String, String>,
}

/// What Amazon S3 answered a call it refused, as the method response maps it: the service's
/// own bytes where it sent any, and otherwise its REST error envelope rendered from the
/// code and sentence the answer reported.
fn refused<E>(
    error: &SdkError<E, HttpResponse>,
    object: &ObjectCall,
) -> Result<Answered, UnservedIntegration>
where
    E: ProvideErrorMetadata + ::std::error::Error + 'static,
{
    let refusal = refusal_of(error, S3_SERVICE_NAME)?;
    let body = match refusal.body.is_empty() {
        false => refusal.body,
        true => error_body(
            refusal.code.as_deref().unwrap_or(ACCESS_DENIED_CODE),
            refusal.message.as_deref().unwrap_or(ACCESS_DENIED_MESSAGE),
            &format!("/{}/{}", object.bucket, object.key),
        ),
    };
    bounded(body.len())?;
    Ok(Answered {
        status: refusal.status,
        content_type: refusal
            .content_type
            .or_else(|| Some(XML_CONTENT_TYPE.to_string())),
        body,
    })
}

/// The Amazon S3 client the integration request is made with.
///
/// The transport is the emulator's own, in process, so the call is served by the Amazon S3
/// this emulator mounts and no connection is opened; the credentials are the ones the
/// callback hands over, which sign this call and are never reported or logged.
fn s3_client(ctx: &OperationContext<'_>, region: &str) -> ::aws_sdk_s3::Client {
    let config = ::aws_sdk_s3::Config::builder()
        .behavior_version(::aws_sdk_s3::config::BehaviorVersion::latest())
        .http_client(ctx.callback().http_client())
        .credentials_provider(ctx.callback().aws_credentials_provider())
        .region(::aws_sdk_s3::config::Region::new(region.to_string()))
        .endpoint_url(format!("https://s3.{region}.amazonaws.com"))
        // The emulator serves one endpoint for every service, so the bucket travels in the
        // path rather than in a per-bucket host the router could not resolve.
        .force_path_style(true)
        // The transport is in-process: there is nothing to retry, time out or watch for a
        // stalled stream, and each of those would need a sleep implementation this client
        // is not given.
        .retry_config(::aws_sdk_s3::config::retry::RetryConfig::disabled())
        .timeout_config(::aws_sdk_s3::config::timeout::TimeoutConfig::disabled())
        .identity_cache(::aws_sdk_s3::config::IdentityCache::no_cache())
        .stalled_stream_protection(
            ::aws_sdk_s3::config::StalledStreamProtectionConfig::disabled(),
        )
        .build();
    ::aws_sdk_s3::Client::from_conf(config)
}

/// The method response the integration's own responses map `answered` to.
fn mapped(
    method: &MethodEntry,
    integration: &GetIntegrationOutput,
    call: &MethodRequest,
    answered: Answered,
) -> Result<IntegratedResponse, UnservedIntegration> {
    let Some(selected) = selected_response(integration, answered.status) else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the integration has no integration response for the {} the backend \
                 answered, so the call maps to no method response",
                answered.status
            ),
        });
    };
    let Some(status) = selected
        .status_code
        .as_deref()
        .and_then(|code| code.parse::<u16>().ok())
    else {
        return Err(UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: "the integration response this call maps to carries no status code the \
                     response can be sent with"
                .to_string(),
        });
    };
    let templated = selected
        .response_templates
        .as_ref()
        .is_some_and(|templates| !templates.is_empty());
    let mut headers = mapped_response_headers(Some(method), selected, status);
    // The media type is the method response's own `Content-Type` mapping where it declares
    // one; see `super::taken_content_type`.
    let mapped_type = super::taken_content_type(&mut headers);
    let (content_type, body) = if templated {
        // The template reads its payload off what the INTEGRATION answered, which is the
        // only difference between rendering it here and rendering it for a `MOCK`.
        let inputs = TemplateInputs {
            payload: Some(answered.body),
            ..call.inputs.clone()
        };
        let body = response_body(selected, call, &inputs).map_err(|template| UnservedIntegration {
            refusal: API_CONFIGURATION_ERROR,
            reason: format!(
                "the {} response template of {} bytes is not one this emulator renders: {}",
                template.content_type, template.bytes, template.reason
            ),
        })?;
        // A templated response is rendered from the api's own records, so the media type it
        // is served with is the mapping's, and `application/json` when none is declared —
        // never the key of the template the caller's `Accept` chose.
        (
            mapped_type.unwrap_or_else(|| JSON_CONTENT_TYPE.to_string()),
            body,
        )
    } else {
        // No template: the backend's own bytes and content type are the method response's,
        // which is what passing a response through means — unless the method response
        // declares a `Content-Type` mapping of its own, which overrides it.
        (
            mapped_type
                .or(answered.content_type)
                .unwrap_or_else(|| JSON_CONTENT_TYPE.to_string()),
            answered.body,
        )
    };
    Ok(IntegratedResponse {
        status,
        content_type,
        body,
        headers,
    })
}

/// The integration response a backend answer of `status` maps to: the one whose
/// `selectionPattern` matches that status, and otherwise the one that declares no pattern.
///
/// Several patterned responses matching one status is a configuration the api's records
/// decide, so the choice is made by status code — the key the responses are stored under —
/// and is therefore the same for every call whatever order the map yields.
///
/// The responses are walked IN that order and the walk STOPS at the first match, so a call
/// compiles no pattern past the one it selects. How many it can compile at all is bounded by
/// the model rather than by a number chosen here: the responses are keyed by
/// `IntegrationResponse$statusCode`, whose `@pattern` is `^[1-5]\d\d$`
/// (`smithy_query(query="shape",shape="StatusCode")`), so one integration holds at most one
/// response per HTTP status — and each compile is bounded in turn ([`pattern_matches`]).
fn selected_response(
    integration: &GetIntegrationOutput,
    status: u16,
) -> Option<&GetIntegrationResponseOutput> {
    let responses = integration.integration_responses.as_ref()?;
    let answered = status.to_string();
    let mut ordered: Vec<&GetIntegrationResponseOutput> = responses.values().collect();
    ordered.sort_by(|left, right| left.status_code.cmp(&right.status_code));
    ordered
        .into_iter()
        .find(|response| {
            response
                .selection_pattern
                .as_deref()
                .filter(|pattern| !pattern.is_empty())
                .is_some_and(|pattern| pattern_matches(pattern, &answered))
        })
        .or_else(|| default_integration_response(integration))
}

/// Whether the `selectionPattern` `pattern` matches `value`, which is the backend status
/// for an AWS integration.
///
/// The pattern is a regular expression a CONTROL-PLANE call stored, not one a request
/// carries, and the subject is the three characters of a status code: it is matched WHOLE,
/// so a pattern for a class of statuses (`4\d{2}`) selects exactly that class rather than
/// every status one of its digits appears in. It is built through
/// [`super::stored_pattern`] — this data plane's one compile of a caller-stored expression,
/// under its EXPLICIT compiled-size and DFA budgets, by the maintained engine whose matching
/// is linear in the subject with no backtracking blowup — so a stored pattern cannot make a
/// request to this stage expensive. A pattern that is not a regular expression this engine
/// builds within those budgets matches nothing, and is said so.
fn pattern_matches(pattern: &str, value: &str) -> bool {
    let anchored = format!("^(?:{pattern})$");
    match super::stored_pattern(&anchored) {
        Some(regex) => regex.is_match(value),
        None => {
            ::tracing::warn!(
                "an integration response's selectionPattern is not a regular expression \
                 this emulator evaluates within its compiled-size budget, so it selects \
                 nothing"
            );
            false
        }
    }
}

/// What `integration`'s `uri` addresses, when it names an Amazon S3 call:
///
/// * `None` when the `uri` names no AWS service call this module makes — another
///   service's action, or no ARN at all — which leaves it to the caller's own refusal;
/// * `Some(Err(reason))` when it DOES name Amazon S3 and this call cannot be placed
///   against it, which is a configuration this stage cannot serve;
/// * `Some(Ok(call))` for the call to make.
fn addressed_s3(
    integration: &GetIntegrationOutput,
    default_region: &str,
    inputs: &TemplateInputs,
) -> Option<Result<S3Call, String>> {
    let uri = integration.uri.as_deref()?;
    let arn = ::service_common::arn::parse(uri)?;
    if arn.vendor != APIGATEWAY_VENDOR || arn.account != S3_SUBDOMAIN {
        return None;
    }
    Some(resolved_s3(integration, &arn, default_region, inputs))
}

/// The call the integration ARN `arn` names, with every `{placeholder}` of its resource
/// resolved against this request.
///
/// The `path` form's resource is Amazon S3's own REST path, so WHAT it addresses is read
/// off that path: `path/{bucket}/{key}` addresses the object, and `path/{bucket}` — a path
/// naming a bucket and nothing under it — addresses the BUCKET itself, each with the
/// action the integration's `httpMethod` names for that resource. The `action` form names
/// the operation instead, and the operation says which of the two it is.
fn resolved_s3(
    integration: &GetIntegrationOutput,
    arn: &::service_common::arn::Arn<'_>,
    default_region: &str,
    inputs: &TemplateInputs,
) -> Result<S3Call, String> {
    let region = match arn.region.is_empty() {
        true => default_region,
        false => arn.region,
    };
    let partition = arn.partition.to_string();
    let resource = substituted(arn.resource, integration, inputs)?;
    let verb = integration.http_method.as_deref().unwrap_or_default();
    if let Some(path) = resource.strip_prefix(PATH_FORM_PREFIX) {
        let path = path.trim_start_matches('/');
        // The bucket is the first segment; what follows it is Amazon S3's own object path.
        // A path that names only the bucket — with or without the trailing slash Amazon
        // S3's own bucket path carries — addresses the BUCKET.
        let (bucket, key) = match path.split_once('/') {
            Some((bucket, key)) => (bucket, key),
            None => (path, ""),
        };
        if key.is_empty() {
            let action = BucketAction::of_verb(verb).ok_or_else(|| {
                format!(
                    "the integration's httpMethod {verb} names no Amazon S3 bucket action \
                     this emulator performs"
                )
            })?;
            nonempty_bucket(bucket)?;
            return Ok(S3Call::Bucket(BucketCall {
                partition,
                region: region.to_string(),
                bucket: bucket.to_string(),
                action,
            }));
        }
        let action = ObjectAction::of_verb(verb).ok_or_else(|| {
            format!(
                "the integration's httpMethod {verb} names no Amazon S3 object action this \
                 emulator performs"
            )
        })?;
        nonempty_bucket(bucket)?;
        return Ok(S3Call::Object(ObjectCall {
            partition,
            region: region.to_string(),
            bucket: bucket.to_string(),
            key: key.to_string(),
            action,
        }));
    }
    let Some(named) = resource.strip_prefix(ACTION_FORM_PREFIX) else {
        return Err(
            "the integration's uri addresses neither the path nor the action form of an \
             Amazon S3 call"
                .to_string(),
        );
    };
    let mut parts = named.split('&');
    let name = parts.next().unwrap_or_default();
    let mut bucket = None;
    let mut key = None;
    for pair in parts {
        match pair.split_once('=') {
            Some((BUCKET_PARAMETER, value)) => bucket = Some(value.to_string()),
            Some((KEY_PARAMETER, value)) => key = Some(value.to_string()),
            _ => {}
        }
    }
    if let Some(action) = ObjectAction::of_action_name(name) {
        let (Some(bucket), Some(key)) = (bucket, key) else {
            return Err(format!(
                "the integration's uri names the Amazon S3 action {name} without both a \
                 {BUCKET_PARAMETER} and a {KEY_PARAMETER} parameter, so it addresses no object"
            ));
        };
        nonempty_bucket(&bucket)?;
        if key.is_empty() {
            return Err(
                "the integration's uri names an empty bucket or key, so it addresses no object"
                    .to_string(),
            );
        }
        return Ok(S3Call::Object(ObjectCall {
            partition,
            region: region.to_string(),
            bucket,
            key,
            action,
        }));
    }
    if let Some(action) = BucketAction::of_action_name(name) {
        let Some(bucket) = bucket else {
            return Err(format!(
                "the integration's uri names the Amazon S3 action {name} without a \
                 {BUCKET_PARAMETER} parameter, so it addresses no bucket"
            ));
        };
        nonempty_bucket(&bucket)?;
        return Ok(S3Call::Bucket(BucketCall {
            partition,
            region: region.to_string(),
            bucket,
            action,
        }));
    }
    Err(format!(
        "the integration's uri names the Amazon S3 action {name}, which this emulator does \
         not perform"
    ))
}

/// `Ok` while `bucket` names a bucket at all: a `uri` whose bucket resolved to nothing
/// addresses no resource, so no call is placed for it.
fn nonempty_bucket(bucket: &str) -> Result<(), String> {
    if bucket.is_empty() {
        return Err(
            "the integration's uri names an empty bucket, so it addresses no Amazon S3 \
             resource"
                .to_string(),
        );
    }
    Ok(())
}

/// `resource` with every `{name}` placeholder replaced by the value the integration maps
/// onto that path parameter of the integration request.
///
/// A placeholder the `requestParameters` map nothing onto, and one mapped from a source
/// this request carries nothing for, leave the `uri` naming no object: the integration
/// cannot be performed, rather than being addressed at whatever text is left behind. Text
/// that opens a placeholder and never closes it is not one, and stands as it was stored.
fn substituted(
    resource: &str,
    integration: &GetIntegrationOutput,
    inputs: &TemplateInputs,
) -> Result<String, String> {
    if !resource.contains('{') {
        return Ok(resource.to_string());
    }
    let mut resolved = String::with_capacity(resource.len());
    let mut rest = resource;
    while let Some((before, after_open)) = rest.split_once('{') {
        let Some((name, after)) = after_open.split_once('}') else {
            break;
        };
        let mapped = mapped_path_parameter(name, integration, inputs)?;
        resolved.push_str(before);
        resolved.push_str(&mapped);
        rest = after;
    }
    resolved.push_str(rest);
    Ok(resolved)
}

/// The value the integration maps onto the integration request's path parameter `name`.
fn mapped_path_parameter(
    name: &str,
    integration: &GetIntegrationOutput,
    inputs: &TemplateInputs,
) -> Result<String, String> {
    let mapping = format!("{INTEGRATION_REQUEST_PATH_PREFIX}{name}");
    let Some(source) = integration
        .request_parameters
        .as_ref()
        .and_then(|parameters| parameters.get(&mapping))
    else {
        return Err(format!(
            "the integration's uri names the parameter {name}, which its requestParameters \
             map no value onto"
        ));
    };
    mapped_value(source, inputs).ok_or_else(|| {
        format!(
            "the integration maps {mapping} from {source}, which is neither a static value in \
             single quotes nor a method request parameter this call carries"
        )
    })
}

/// The value a `requestParameters` source names for this call: the literal between the
/// quotes of a static value, or the method request parameter the expression names — read
/// off the same inputs a mapping template of this call is rendered from, so a simulated
/// invocation and a real one resolve one mapping alike.
fn mapped_value(source: &str, inputs: &TemplateInputs) -> Option<String> {
    if is_static_mapping_value(source) {
        return source
            .strip_prefix('\'')
            .and_then(|rest| rest.strip_suffix('\''))
            .map(str::to_string);
    }
    if let Some(name) = source.strip_prefix(METHOD_REQUEST_PATH_PREFIX) {
        return inputs.path.get(name).cloned();
    }
    if let Some(name) = source.strip_prefix(METHOD_REQUEST_QUERY_PREFIX) {
        return inputs.querystring.get(name).cloned();
    }
    if let Some(name) = source.strip_prefix(METHOD_REQUEST_HEADER_PREFIX) {
        return named_header(&inputs.header, name);
    }
    None
}

/// The value `headers` carries under `name`, compared without regard to case as HTTP field
/// names are: a mapping expression names a header in the case the developer wrote, and the
/// request carries it in the case the client sent.
fn named_header(headers: &BTreeMap<String, String>, name: &str) -> Option<String> {
    headers
        .iter()
        .find(|(carried, _)| carried.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.clone())
}

#[cfg(test)]
mod tests {
    //! What an integration `uri` addresses, which is the one part of this module that
    //! needs neither a backend nor a role to decide.

    use super::*;

    /// An integration of the `AWS` type with `uri`, `httpMethod` and `requestParameters`.
    fn integration(
        uri: &str,
        verb: &str,
        parameters: &[(&str, &str)],
    ) -> GetIntegrationOutput {
        GetIntegrationOutput {
            r#type: Some(AWS_SERVICE_INTEGRATION_TYPE.to_string()),
            http_method: Some(verb.to_string()),
            uri: Some(uri.to_string()),
            request_parameters: Some(
                parameters
                    .iter()
                    .map(|(key, value)| ((*key).to_string(), (*value).to_string()))
                    .collect(),
            ),
            ..GetIntegrationOutput::default()
        }
    }

    /// The call's inputs, carrying one path parameter and one header.
    fn inputs() -> TemplateInputs {
        TemplateInputs {
            payload: None,
            path: BTreeMap::from([("item".to_string(), "notes/today.txt".to_string())]),
            querystring: BTreeMap::from([("name".to_string(), "chosen.txt".to_string())]),
            header: BTreeMap::from([("x-object".to_string(), "headed.txt".to_string())]),
            stage_variables: BTreeMap::new(),
            context: BTreeMap::new(),
        }
    }

    /// One row of the table below: the case name, the integration whose `uri` is read, and
    /// what it addresses — `None` for a configuration this stage cannot serve.
    type Row = (&'static str, GetIntegrationOutput, Option<Addressed>);

    /// What a row expects the `uri` to address: an object, or a bucket.
    #[derive(Debug, Clone, Copy, PartialEq, Eq)]
    enum Addressed {
        Object(&'static str, &'static str, ObjectAction),
        Bucket(&'static str, BucketAction),
    }

    /// What `read` addresses, in the row vocabulary above.
    fn addressed(call: &S3Call) -> Addressed {
        match call {
            S3Call::Object(object) => {
                Addressed::Object(leaked(&object.bucket), leaked(&object.key), object.action)
            }
            S3Call::Bucket(bucket) => Addressed::Bucket(leaked(&bucket.bucket), bucket.action),
        }
    }

    /// `value` as a `&'static str`, so a read call compares against a row's own literals.
    /// Test-only, and bounded by the number of rows.
    fn leaked(value: &str) -> &'static str {
        Box::leak(value.to_string().into_boxed_str())
    }

    #[test]
    fn the_uri_decides_which_object_call_an_aws_integration_makes() {
        // Rows differ along what the rule generalises over: the FORM the uri names the
        // call in, the verb that names the action in the path form, the action the action
        // form names, WHICH RESOURCE the path addresses (an object under a bucket, and a
        // bucket on its own), and where a placeholder's value is mapped from.
        let cases: Vec<Row> = vec![
            (
                "the path form, read as a GetObject",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:path/shelf/plain.txt",
                    "GET",
                    &[],
                ),
                Some(Addressed::Object("shelf", "plain.txt", ObjectAction::Get)),
            ),
            (
                "the path form with a PUT, read as a PutObject",
                integration(
                    "arn:aws:apigateway:eu-west-2:s3:path/shelf/written.txt",
                    "PUT",
                    &[],
                ),
                Some(Addressed::Object("shelf", "written.txt", ObjectAction::Put)),
            ),
            (
                "the path form with a DELETE, read as a DeleteObject",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf/gone.txt", "DELETE", &[]),
                Some(Addressed::Object("shelf", "gone.txt", ObjectAction::Delete)),
            ),
            (
                "a placeholder mapped from a path parameter, key segments kept",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:path/shelf/{object}",
                    "GET",
                    &[("integration.request.path.object", "method.request.path.item")],
                ),
                Some(Addressed::Object(
                    "shelf",
                    "notes/today.txt",
                    ObjectAction::Get,
                )),
            ),
            (
                "a placeholder mapped from a query parameter",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:path/shelf/{object}",
                    "GET",
                    &[(
                        "integration.request.path.object",
                        "method.request.querystring.name",
                    )],
                ),
                Some(Addressed::Object("shelf", "chosen.txt", ObjectAction::Get)),
            ),
            (
                "a placeholder mapped from a header, named in another case",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:path/shelf/{object}",
                    "HEAD",
                    &[(
                        "integration.request.path.object",
                        "method.request.header.X-Object",
                    )],
                ),
                Some(Addressed::Object("shelf", "headed.txt", ObjectAction::Head)),
            ),
            (
                "a placeholder mapped from a static value",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:path/{bucket}/fixed.txt",
                    "GET",
                    &[("integration.request.path.bucket", "'static-shelf'")],
                ),
                Some(Addressed::Object(
                    "static-shelf",
                    "fixed.txt",
                    ObjectAction::Get,
                )),
            ),
            (
                "the action form, which names the action itself",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:action/GetObject&Bucket=shelf&Key=acted.txt",
                    "GET",
                    &[],
                ),
                Some(Addressed::Object("shelf", "acted.txt", ObjectAction::Get)),
            ),
            (
                "the action form naming PutObject against the GET verb: the action wins",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:action/PutObject&Bucket=shelf&Key=acted.txt",
                    "GET",
                    &[],
                ),
                Some(Addressed::Object("shelf", "acted.txt", ObjectAction::Put)),
            ),
            (
                "the path form naming a bucket alone, read as a ListObjects",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf", "GET", &[]),
                Some(Addressed::Bucket("shelf", BucketAction::List)),
            ),
            (
                "the path form naming a bucket alone with a trailing slash",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf/", "GET", &[]),
                Some(Addressed::Bucket("shelf", BucketAction::List)),
            ),
            (
                "the path form naming a bucket alone with a HEAD, read as a HeadBucket",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf", "HEAD", &[]),
                Some(Addressed::Bucket("shelf", BucketAction::Head)),
            ),
            (
                "a bucket placeholder mapped from a path parameter",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:path/{bucket}",
                    "GET",
                    &[("integration.request.path.bucket", "'mapped-shelf'")],
                ),
                Some(Addressed::Bucket("mapped-shelf", BucketAction::List)),
            ),
            (
                "the action form naming ListObjects",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:action/ListObjects&Bucket=shelf",
                    "GET",
                    &[],
                ),
                Some(Addressed::Bucket("shelf", BucketAction::List)),
            ),
            (
                "a verb that names no bucket action",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf", "PUT", &[]),
                None,
            ),
            (
                "the action form naming ListObjects without a bucket",
                integration(
                    "arn:aws:apigateway:us-east-1:s3:action/ListObjects",
                    "GET",
                    &[],
                ),
                None,
            ),
            (
                "a verb that names no object action",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf/plain.txt", "PATCH", &[]),
                None,
            ),
            (
                "a placeholder nothing is mapped onto",
                integration("arn:aws:apigateway:us-east-1:s3:path/shelf/{object}", "GET", &[]),
                None,
            ),
            (
                "a form that is neither path nor action",
                integration("arn:aws:apigateway:us-east-1:s3:shelf/plain.txt", "GET", &[]),
                None,
            ),
        ];
        for (case, integration, expected) in &cases {
            let read = addressed_s3(integration, "us-east-1", &inputs());
            let read = read
                .unwrap_or_else(|| panic!("case {case:?}: the uri names an Amazon S3 call"));
            match expected {
                Some(expected) => {
                    let call = read
                        .as_ref()
                        .unwrap_or_else(|error| panic!("case {case:?}: unplaced: {error}"));
                    let actual = addressed(call);
                    assert_eq!(
                        actual, *expected,
                        "case {case:?}: expected {expected:?}, got {actual:?}"
                    );
                }
                None => assert!(
                    read.is_err(),
                    "case {case:?}: expected a configuration this stage cannot serve, got \
                     {read:?}"
                ),
            }
        }
    }

    #[test]
    fn an_integration_naming_another_service_or_no_arn_is_not_one_of_these_calls() {
        // Rows differ along WHY the uri names no Amazon S3 object call: another service's
        // subdomain, another vendor in the ARN, and a value that is no ARN at all.
        let cases = [
            (
                "another service's action",
                "arn:aws:apigateway:us-east-1:dynamodb:action/PutItem",
            ),
            (
                "an ARN of another vendor",
                "arn:aws:s3:us-east-1:123456789012:path/shelf/plain.txt",
            ),
            ("a formed URL rather than an ARN", "https://example.test/x"),
        ];
        for (case, uri) in &cases {
            let read = addressed_s3(&integration(uri, "GET", &[]), "us-east-1", &inputs());
            assert!(
                read.is_none(),
                "case {case:?}: expected the uri to name none of these calls, got {read:?}"
            );
        }
    }

    #[test]
    fn a_selection_pattern_chooses_by_the_status_the_backend_answered() {
        // Rows differ along the status answered and the patterns stored: a class pattern,
        // an exact one, one that matches nothing, and the fallback with no pattern.
        let responses = [("200", None), ("403", Some("4\\d{2}")), ("500", Some("5\\d{2}"))];
        let integration = GetIntegrationOutput {
            integration_responses: Some(
                responses
                    .iter()
                    .map(|(status, pattern)| {
                        (
                            (*status).to_string(),
                            GetIntegrationResponseOutput {
                                status_code: Some((*status).to_string()),
                                selection_pattern: pattern.map(str::to_string),
                                ..GetIntegrationResponseOutput::default()
                            },
                        )
                    })
                    .collect(),
            ),
            ..GetIntegrationOutput::default()
        };
        let cases = [
            ("a 200 answer maps to the response with no pattern", 200_u16, "200"),
            ("a 403 answer maps to the 4xx class", 403, "403"),
            ("a 404 answer maps to the same 4xx class", 404, "403"),
            ("a 503 answer maps to the 5xx class", 503, "500"),
            ("a 302 answer matches no pattern and falls back", 302, "200"),
        ];
        for (case, status, expected) in &cases {
            let selected = selected_response(&integration, *status)
                .unwrap_or_else(|| panic!("case {case:?}: expected a selected response"));
            assert_eq!(
                selected.status_code.as_deref(),
                Some(*expected),
                "case {case:?}: expected the {expected} integration response, got {:?}",
                selected.status_code
            );
        }
    }
}
