// SPDX-License-Identifier: CC0-1.0

//! The seam a RUNNING handler reaches the rest of this emulator through: one AWS API call,
//! made by the host on the guest's behalf.
//!
//! AWS Lambda's managed runtimes ship the AWS SDK for the handler's language on its import
//! path, so a handler can call another AWS service with no dependency of its own — and a
//! very large share of real functions exist to do exactly that. This module is the host half
//! of that: the guest hands over a service token, an operation name and the operation's
//! input document, and the host builds the HTTP request that service's own protocol
//! describes, signs it, and dispatches it through the in-process callback
//! ([`::smithy_server_runtime::EmbeddedEmulator`]) into the same emulator the function is
//! running inside. The answer is that service's own answer.
//!
//! What the guest can and cannot do here:
//!
//! * The destination is ALWAYS this emulator. The request is dispatched through the
//!   callback transport, which hands it to this process's router; no host name the guest
//!   supplies is resolved and no socket is opened, so no call can leave the process. The
//!   framework's own callback recursion limit bounds a chain of self-invocations.
//! * The guest never sees a credential. The signature is put on the request by host code
//!   with the credential the invocation was given — the session of the function's own
//!   EXECUTION ROLE ([`super::execution_role_session`]), or the credential the framework
//!   hands this emulator's handlers where no session could be minted — exactly as every
//!   other in-process call between two emulated services is signed
//!   ([`::service_common::in_process_sigv4`]); nothing of it is returned to the guest, and
//!   the guest cannot ask for a call to be signed as anything else. So the service that
//!   SERVES the call judges it against the policies in force on that role, which is what
//!   "the function runs as its execution role" means on the wire.
//! * Only the services in [`APIS`] are reachable, and only the operations their own models
//!   bind. A token that is not in that table is REFUSED rather than guessed at: the guest
//!   is told the service is unknown, and nothing is sent.
//! * What an SDK CLIENT puts on a request of its own is put on it here, once for every SDK
//!   this runtime supplies: the input members the called service's model marks as an
//!   idempotency token are filled with a generated value when the guest left them out, which
//!   is what every AWS SDK does before it signs (`super::idempotency_token`).
//! * Every bound is the host's. The size of the document the guest may send, the size of
//!   the answer it may read back and how long one call may take are all checked here and in
//!   [`super::python`], against the run's own remaining deadline.
//!
//! Nothing about a call is synthesised: a status, an error code, a message or a value the
//! guest reads back came from the emulated service that answered, and a call this module
//! cannot make is reported as not made.

use ::std::time::Duration;

use ::smithy_server_runtime::EmbeddedEmulator;

/// The header an AWS JSON client names the operation it calls in
/// (`<target prefix>.<OperationName>`).
const TARGET_HEADER: &str = "x-amz-target";

/// The header an AWS answer names the error shape of a refusal in.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The header a client of a QUERY-COMPATIBLE service opts into the legacy error vocabulary
/// with, which is the only thing that makes the service answer [`QUERY_ERROR_HEADER`].
///
/// A service whose model carries the `awsQueryCompatible` protocol modifier moved from the
/// query protocol to an AWS JSON one while its clients kept matching on the query-era error
/// codes, so every SDK client of such a service sends this and no client of any other
/// service does.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";

/// The header such a service names the LEGACY code of a refusal in, as `<code>;<fault>`.
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// The header an AWS answer carries the id of the request it answers in.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// The header a request made with a TEMPORARY credential carries that credential's session
/// token in, which is what a created session credential is served only with.
const SESSION_TOKEN_HEADER: &str = "x-amz-security-token";

/// The largest input document one call may carry.
///
/// A HOST bound, well above any operation's own input limit and far below what a run's
/// memory ceiling allows, so a guest cannot turn one invocation into an unbounded request.
pub const MAX_REQUEST_BYTES: usize = 1024 * 1024;

/// The largest answer one call may read back, for the same reason.
pub const MAX_RESPONSE_BYTES: usize = 8 * 1024 * 1024;

/// How many calls one run may make.
///
/// A HOST bound: a handler loops as it likes, but the work it can push into the rest of the
/// emulator from one invocation is capped, so no single invocation can drive the process
/// without end. Well above what a handler processing one event batch needs.
pub const MAX_CALLS_PER_RUN: usize = 256;

/// How one service's API is reached on the wire: the facts of that service's own model that
/// a request to it is built from.
///
/// Every field is read from the target service's Smithy model with
/// `smithy_query(query="service_info")`, so this table states no wire detail of its own.
#[derive(Debug)]
pub struct Api {
    /// The token an AWS SDK client names this service by — the service's SDK id, lower
    /// cased, which is what `boto3.client("sqs")` is given. A client is built with either
    /// spelling of a two-word id, hyphenated (`rds-data`) or run together (`rdsdata`), and
    /// both reach this one entry ([`same_service_token`]).
    pub sdk_token: &'static str,
    /// Its SigV4 signing name (`sigv4Name`) — also the credential scope that routes the
    /// call to this service inside this emulator.
    pub signing_name: &'static str,
    /// The leading label of its endpoint host (`awsService.endpointPrefix`).
    pub endpoint_prefix: &'static str,
    /// The protocol its model declares, which is what a request to it is bound by.
    pub protocol: Protocol,
    /// The service's own Smithy model, where this crate embeds one for it: the SAME file that
    /// service's emulator crate is generated from, which is what every question answered out
    /// of a called service's model is read from (`super::node_sdk_exports`). `None` for a
    /// service this crate holds no model for — those questions simply have no answer for it.
    ///
    /// It is held on the ENTRY rather than in a table keyed by one spelling of the service's
    /// token, so that every token that names ONE service answers with the same model: a call
    /// made as `logs` reads the model a call made as `cloudwatchlogs` reads.
    pub model: Option<&'static str>,
    /// The service's own model, read for the ANSWER: WHICH of its values carry bytes rather
    /// than text ([`super::blob_members`]), which only that model says. `None` for a service
    /// whose answers this seam reads some other way — Amazon S3, whose own client parses them
    /// ([`super::aws_s3`]).
    pub answer_model: Option<&'static super::blob_members::ServiceModel>,
}

/// How a request to one service is bound to HTTP, which its model's protocol trait decides.
#[derive(Debug)]
pub enum Protocol {
    /// An AWS JSON protocol (`awsJson1_0`, `awsJson1_1`): the operation is named in a
    /// header and the input document IS the request body, so one code path serves every
    /// operation and nothing per-operation is stated anywhere.
    AwsJson {
        /// The `X-Amz-Target` prefix a client names an operation under: the service shape's
        /// own name, as the model spells it.
        target_prefix: &'static str,
        /// The protocol version (`awsJson1_0` -> `1.0`), which names the media type of the
        /// request body.
        json_version: &'static str,
        /// Whether the service's model carries the `awsQueryCompatible` protocol modifier
        /// (`smithy_query(query="service_info")` reports it under `protocol_modifiers`).
        ///
        /// Such a service kept the error vocabulary of the query protocol it moved off, and
        /// every SDK client of one asks for that vocabulary ([`QUERY_MODE_HEADER`]) and
        /// reports a refusal by the legacy code the answer carries ([`QUERY_ERROR_HEADER`])
        /// rather than by the modelled shape's name — so the request this seam builds asks
        /// for it too.
        query_compatible: bool,
    },
    /// `restJson1`: each operation binds its input to a method, a URI, a query, headers and
    /// a body, member by member, so the request is built from the service's own model
    /// ([`super::rest_json`]).
    RestJson(&'static super::rest_json::RestApi),
    /// Amazon S3's `restXml`: each operation binds its input to a method, a path, a query
    /// and headers of its own and answers an XML document or the object's own bytes, so the
    /// request is built, signed and parsed by the S3 client this crate holds
    /// ([`super::aws_s3`]) rather than by either path above.
    S3Rest,
    /// CloudFront's `restXml`: the same, for the service whose operations bind their input
    /// to a path and an XML document, built by the CloudFront client this crate holds
    /// ([`super::aws_cloudfront`]).
    CloudFrontRest,
    /// Amazon SNS's `awsQuery`: the operation travels as an `Action` parameter of a
    /// form-encoded body with the input flattened into the parameters beside it, and the
    /// answer is an XML document — so the request is built, signed and parsed by the Amazon
    /// SNS client this crate holds ([`super::aws_sns`]) rather than by either JSON path
    /// above.
    SnsQuery,
}

impl Api {
    /// The host one of this service's regional endpoints answers on.
    fn host(&self, region: &str) -> String {
        format!("{}.{region}.amazonaws.com", self.endpoint_prefix)
    }

    /// The media type this service's protocol names for a JSON request body.
    fn content_type(&self) -> String {
        match self.protocol {
            Protocol::AwsJson { json_version, .. } => {
                format!("application/x-amz-json-{json_version}")
            }
            Protocol::RestJson(_)
            | Protocol::S3Rest
            | Protocol::CloudFrontRest
            | Protocol::SnsQuery => "application/json".to_string(),
        }
    }
}

/// The services a running handler may call, and how a request to each is bound to HTTP.
///
/// One entry per SERVICE, never per action: a service's protocol is what binds its requests,
/// and each protocol here is bound once for every operation the service models — an AWS JSON
/// one through the target header and the input document, a `restJson1` one through that
/// service's own model ([`super::rest_json`]) — so no operation is described here.
///
/// It is a LIST rather than a rule because each entry's wire facts are that service's own
/// model's: which protocol it speaks, what it signs as, and where it answers. A token with no
/// entry is one no call is made for, and [`AwsAccess::call`] says exactly that rather than
/// sending a request whose shape it is guessing at.
pub const APIS: &[Api] = &[
    // `services/service-com-amazonaws-sqs-amazonsqs/smithy-model/
    // com-amazonaws-sqs-amazonsqs.json`: awsJson1_0, sigv4Name `sqs`, endpointPrefix
    // `sqs`, sdkId `SQS`, service shape `AmazonSQS`.
    Api {
        sdk_token: "sqs",
        signing_name: "sqs",
        endpoint_prefix: "sqs",
        model: Some(SQS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AmazonSQS",
            json_version: "1.0",
            // `protocol_modifiers: ["awsQueryCompatible"]`.
            query_compatible: true,
        },
        answer_model: Some(&SQS),
    },
    // `services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
    // com-amazonaws-sns-amazonsimplenotificationservice.json`: awsQuery, sigv4Name `sns`,
    // endpointPrefix `sns`, sdkId `SNS`. Its operations carry their input as the parameters of
    // a form-encoded `Action` and answer an XML document rather than a JSON one, so the
    // request is built by [`super::aws_sns`].
    //
    // PUBLISHING A NOTIFICATION is what a function that exists to tell the rest of a system
    // about the event it just handled does, and SUBSCRIBING a queue, a function or an endpoint
    // to a topic is what makes that fan-out exist at all — so a handler that cannot reach this
    // service cannot be the glue between two services, which is the shape most Lambda code
    // has.
    Api {
        sdk_token: "sns",
        signing_name: "sns",
        endpoint_prefix: "sns",
        model: Some(SNS_MODEL),
        protocol: Protocol::SnsQuery,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_sns`]); none of them carries bytes.
        answer_model: None,
    },
    // `services/service-com-amazonaws-eventbridge-awsevents/smithy-model/
    // com-amazonaws-eventbridge-awsevents.json`: awsJson1_1, sigv4Name `events`,
    // endpointPrefix `events`, sdkId `EventBridge`, service shape `AWSEvents`.
    Api {
        sdk_token: "events",
        signing_name: "events",
        endpoint_prefix: "events",
        model: Some(EVENTBRIDGE_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSEvents",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&EVENTBRIDGE),
    },
    // The same service under the token its own SDK id gives it, which is the other name an
    // SDK client is built with (`boto3.client("eventbridge")`).
    Api {
        sdk_token: "eventbridge",
        signing_name: "events",
        endpoint_prefix: "events",
        model: Some(EVENTBRIDGE_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSEvents",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&EVENTBRIDGE),
    },
    // `services/service-com-amazonaws-pipes-pipes/smithy-model/com-amazonaws-pipes-pipes.json`:
    // restJson1, sigv4Name `pipes`, endpointPrefix `pipes`, sdkId `Pipes`. Its requests are
    // bound per operation out of that same model ([`PIPES`]).
    //
    // A pipe's own TARGET is a Lambda function, and the handler at the end of a pipe is where
    // a client drives and inspects the pipe that invoked it: reading the pipe's configuration,
    // and starting or stopping the flow it is itself part of, are the calls such a function
    // makes — so a handler that cannot reach this service cannot see or steer the pipe it
    // serves.
    Api {
        sdk_token: "pipes",
        signing_name: "pipes",
        endpoint_prefix: "pipes",
        model: Some(PIPES_MODEL),
        protocol: Protocol::RestJson(&PIPES),
        answer_model: Some(&PIPES_ANSWERS),
    },
    // `services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
    // com-amazonaws-dynamodb-dynamodb-20120810.json`: awsJson1_0, sigv4Name `dynamodb`,
    // endpointPrefix `dynamodb`, sdkId `DynamoDB`, service shape `DynamoDB_20120810`.
    Api {
        sdk_token: "dynamodb",
        signing_name: "dynamodb",
        endpoint_prefix: "dynamodb",
        model: Some(DYNAMODB_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "DynamoDB_20120810",
            json_version: "1.0",
            query_compatible: false,
        },
        answer_model: Some(&DYNAMODB),
    },
    // `services/service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/
    // com-amazonaws-cloudwatchlogs-logs-20140328.json`: awsJson1_1, sigv4Name `logs`,
    // endpointPrefix `logs`, sdkId `CloudWatch Logs`, service shape `Logs_20140328`. This is
    // also the API the runtime itself delivers an invocation's own log over
    // ([`super::log_delivery`]), so the host and the guest reach one service.
    Api {
        sdk_token: "logs",
        signing_name: "logs",
        endpoint_prefix: "logs",
        model: Some(CLOUDWATCH_LOGS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "Logs_20140328",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&CLOUDWATCH_LOGS),
    },
    // The same service under the token its own SDK id gives it, as EventBridge is above.
    Api {
        sdk_token: "cloudwatchlogs",
        signing_name: "logs",
        endpoint_prefix: "logs",
        model: Some(CLOUDWATCH_LOGS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "Logs_20140328",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&CLOUDWATCH_LOGS),
    },
    // `services/service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/
    // com-amazonaws-cloudwatch-graniteserviceversion20100801.json`: awsJson1_0 (beside the
    // legacy `awsQuery` its SDKs keep speaking), sigv4Name `monitoring`, endpointPrefix
    // `monitoring`, sdkId `CloudWatch`, service shape `GraniteServiceVersion20100801`.
    //
    // A function that PUBLISHES ITS OWN METRIC is the other half of an instrumented handler:
    // a custom datapoint a handler counts over the event it just processed is submitted
    // through `PutMetricData`, and read back through this service's own metric reads, so a
    // handler that cannot reach it can report nothing about its own work.
    Api {
        sdk_token: "cloudwatch",
        signing_name: "monitoring",
        endpoint_prefix: "monitoring",
        model: Some(CLOUDWATCH_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "GraniteServiceVersion20100801",
            json_version: "1.0",
            // `protocol_modifiers: ["awsQueryCompatible"]`, which is the legacy `awsQuery`
            // vocabulary this service's clients keep matching on.
            query_compatible: true,
        },
        answer_model: Some(&CLOUDWATCH),
    },
    // `services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/
    // com-amazonaws-apigateway-backplanecontrolservice.json`: restJson1, sigv4Name
    // `apigateway`, endpointPrefix `apigateway`, sdkId `API Gateway`. Its requests are bound
    // per operation out of that same model ([`APIGATEWAY`]).
    Api {
        sdk_token: "apigateway",
        signing_name: "apigateway",
        endpoint_prefix: "apigateway",
        model: Some(APIGATEWAY_MODEL),
        protocol: Protocol::RestJson(&APIGATEWAY),
        answer_model: Some(&APIGATEWAY_ANSWERS),
    },
    // `services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/
    // smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json`:
    // restJson1, sigv4Name `execute-api`, endpointPrefix `execute-api`, sdkId
    // `ApiGatewayManagementApi`. Its requests are bound per operation out of that same model
    // ([`APIGATEWAYMANAGEMENTAPI`]).
    //
    // A WEBSOCKET API'S BACKEND is one of the functions AWS documents as calling the service
    // that invoked it: the `@connections` API is how a handler pushes a payload to a
    // connection that is not the one it is answering (a fan-out, a broadcast) and how it
    // learns that a connection is gone, so a handler that cannot reach it cannot serve a
    // WebSocket api's server-to-client half at all.
    Api {
        sdk_token: "apigatewaymanagementapi",
        signing_name: "execute-api",
        endpoint_prefix: "execute-api",
        model: Some(APIGATEWAYMANAGEMENTAPI_MODEL),
        protocol: Protocol::RestJson(&APIGATEWAYMANAGEMENTAPI),
        answer_model: Some(&APIGATEWAYMANAGEMENTAPI_ANSWERS),
    },
    // `services/service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/
    // com-amazonaws-secretsmanager-secretsmanager.json`: awsJson1_1, sigv4Name
    // `secretsmanager`, endpointPrefix `secretsmanager`, sdkId `Secrets Manager`, service
    // shape `secretsmanager`.
    //
    // A SECRETS MANAGER ROTATION FUNCTION is one of the functions AWS documents as calling
    // the service that invoked it: each of the four steps of a rotation reads the secret's
    // metadata and writes a version of it through these operations, so a handler that
    // cannot reach them cannot rotate anything.
    Api {
        sdk_token: "secretsmanager",
        signing_name: "secretsmanager",
        endpoint_prefix: "secretsmanager",
        model: Some(SECRETSMANAGER_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "secretsmanager",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&SECRETSMANAGER),
    },
    // `services/service-com-amazonaws-kms-trentservice/smithy-model/
    // com-amazonaws-kms-trentservice.json`: awsJson1_1, sigv4Name `kms`, endpointPrefix `kms`,
    // sdkId `KMS`, service shape `TrentService`.
    //
    // ENVELOPE ENCRYPTION is what a handler that is handed a ciphertext does with it: a
    // function invoked with a payload another service sealed under a customer managed key —
    // the code a user pool's custom sender trigger delivers, an object's data key, a sealed
    // parameter — reads the plaintext back with `Decrypt`, and a handler that seals its own
    // writes calls `Encrypt` or `GenerateDataKey`. A handler that cannot reach this service
    // can neither open what it was sent nor protect what it stores.
    Api {
        sdk_token: "kms",
        signing_name: "kms",
        endpoint_prefix: "kms",
        model: Some(KMS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "TrentService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&KMS),
    },
    // `services/service-com-amazonaws-ssm-amazonssm/smithy-model/
    // com-amazonaws-ssm-amazonssm.json`: awsJson1_1, sigv4Name `ssm`, endpointPrefix `ssm`,
    // sdkId `SSM`, service shape `AmazonSSM`.
    //
    // PARAMETER STORE is where a function keeps the configuration it reads and the state it
    // records: a handler reads the parameter that tells it what to do (`GetParameter`) and
    // writes what it did (`PutParameter`), which is how the work ONE invocation performed is
    // observed from outside it, so a handler that cannot reach this service can neither read
    // its own configuration nor record anything.
    Api {
        sdk_token: "ssm",
        signing_name: "ssm",
        endpoint_prefix: "ssm",
        model: Some(SSM_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AmazonSSM",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&SSM),
    },
    // `services/service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/
    // com-amazonaws-opensearch-amazonopensearchservice.json`: restJson1, sigv4Name `es`,
    // endpointPrefix `es`, sdkId `OpenSearch`. Its requests are bound per operation out of
    // that same model ([`OPENSEARCH`]).
    //
    // A handler that indexes or searches documents reads the DOMAIN it is to reach from
    // this control plane (`DescribeDomain`), and then reaches the domain's own data plane
    // over HTTP ([`super::guest_http`]).
    Api {
        sdk_token: "opensearch",
        signing_name: "es",
        endpoint_prefix: "es",
        // The only question this seam answers out of OpenSearch's model is how a request to it
        // is bound, which the protocol entry below carries ([`OPENSEARCH`]).
        model: None,
        protocol: Protocol::RestJson(&OPENSEARCH),
        answer_model: Some(&OPENSEARCH_ANSWERS),
    },
    // The same service under the token its ENDPOINT PREFIX gives it, which is the other
    // name an SDK client for it is built with (`boto3.client("es")`).
    Api {
        sdk_token: "es",
        signing_name: "es",
        endpoint_prefix: "es",
        model: None,
        protocol: Protocol::RestJson(&OPENSEARCH),
        answer_model: Some(&OPENSEARCH_ANSWERS),
    },
    // `services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/
    // com-amazonaws-sfn-awsstepfunctions.json`: awsJson1_0, sigv4Name `states`,
    // endpointPrefix `states`, sdkId `SFN`, service shape `AWSStepFunctions`.
    //
    // A STATE MACHINE'S OWN TASK is the other half of a Step Functions workflow: a `Task`
    // state invokes a function, and the function carries the workflow on by starting,
    // stopping or reporting on an execution — and a callback task reports its outcome back
    // through `SendTaskSuccess`/`SendTaskFailure` — so a handler that cannot reach this
    // service cannot take part in one.
    //
    // The token is the one an SDK client names this service by: the AWS SDK for Python's
    // own name for it is `stepfunctions`, which is what `boto3.client("stepfunctions")` is
    // given.
    Api {
        sdk_token: "stepfunctions",
        signing_name: "states",
        endpoint_prefix: "states",
        model: Some(SFN_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSStepFunctions",
            json_version: "1.0",
            query_compatible: false,
        },
        answer_model: Some(&SFN),
    },
    // The same service under the token its own SDK id gives it, as EventBridge and
    // CloudWatch Logs are above.
    Api {
        sdk_token: "sfn",
        signing_name: "states",
        endpoint_prefix: "states",
        model: Some(SFN_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSStepFunctions",
            json_version: "1.0",
            query_compatible: false,
        },
        answer_model: Some(&SFN),
    },
    // `services/service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/
    // com-amazonaws-sesv2-simpleemailservice-v2.json`: restJson1, sigv4Name `ses`,
    // endpointPrefix `email`, sdkId `SESv2`. Its requests are bound per operation out of
    // that same model ([`SESV2`]), so all 116 operations it binds are reachable — sending a
    // message, and managing an identity, a configuration set, a contact list or a
    // suppression list, are what a function that exists to send mail does.
    Api {
        sdk_token: "sesv2",
        signing_name: "ses",
        endpoint_prefix: "email",
        model: Some(SESV2_MODEL),
        protocol: Protocol::RestJson(&SESV2),
        answer_model: Some(&SESV2_ANSWERS),
    },
    // `services/service-com-amazonaws-s3-amazons3/smithy-model/
    // com-amazonaws-s3-amazons3.json`: restXml, sigv4Name `s3`, endpointPrefix `s3`, sdkId
    // `S3`. Its operations bind their input to a path, a query and headers rather than to a
    // document, and answer XML or an object's own bytes, so the request is built by
    // [`super::aws_s3`].
    Api {
        sdk_token: "s3",
        signing_name: "s3",
        endpoint_prefix: "s3",
        model: Some(S3_MODEL),
        protocol: Protocol::S3Rest,
        answer_model: None,
    },
    // `services/service-com-amazonaws-xray-awsxray/smithy-model/
    // com-amazonaws-xray-awsxray.json`: restJson1, sigv4Name `xray`, endpointPrefix `xray`,
    // sdkId `XRay`. Its requests are bound per operation out of that same model ([`XRAY`]).
    //
    // A TRACED function reports its own work to X-Ray: the segment documents an instrumented
    // handler writes to the X-Ray daemon are submitted through `PutTraceSegments`
    // ([`super::xray_daemon`]), and a handler using the X-Ray SDK directly calls the
    // same operations, so a function that cannot reach this service cannot report a trace at
    // all.
    Api {
        sdk_token: "xray",
        signing_name: "xray",
        endpoint_prefix: "xray",
        // As OpenSearch above: the one question asked out of X-Ray's model is how a request to
        // it is bound, which [`XRAY`] carries.
        model: None,
        protocol: Protocol::RestJson(&XRAY),
        answer_model: Some(&XRAY_ANSWERS),
    },
    // `services/service-com-amazonaws-rdsdata-rdsdataservice/smithy-model/
    // com-amazonaws-rdsdata-rdsdataservice.json`: restJson1, sigv4Name `rds-data`,
    // endpointPrefix `rds-data`, sdkId `RDS Data`. Its requests are bound per operation out
    // of that same model ([`RDS_DATA`]).
    //
    // The DATA API is how a function reaches an Aurora cluster's SQL without a database
    // connection of its own: a handler runs a statement, opens a transaction and commits or
    // rolls it back through these operations, so a handler that cannot reach them cannot
    // read or write the database it exists to serve.
    Api {
        sdk_token: "rds-data",
        signing_name: "rds-data",
        endpoint_prefix: "rds-data",
        model: Some(RDS_DATA_MODEL),
        protocol: Protocol::RestJson(&RDS_DATA),
        answer_model: Some(&RDS_DATA_ANSWERS),
    },
    // `services/service-com-amazonaws-appconfigdata-appconfigdata/smithy-model/
    // com-amazonaws-appconfigdata-appconfigdata.json`: restJson1, sigv4Name `appconfig`,
    // endpointPrefix `appconfigdata`, sdkId `AppConfigData`. Its requests are bound per
    // operation out of that same model ([`APPCONFIGDATA`]).
    //
    // The CONFIGURATION a function runs against is read through this data plane: a handler
    // opens a configuration session over an (application, environment, configuration profile)
    // triple and polls the deployed document's own bytes, which is what a feature-flagged
    // handler branches on — so a handler that cannot reach it can only ever serve its own
    // hard-coded default, and cannot tell a configuration it was served from an identifier
    // that resolves to nothing.
    Api {
        sdk_token: "appconfigdata",
        signing_name: "appconfig",
        endpoint_prefix: "appconfigdata",
        model: Some(APPCONFIGDATA_MODEL),
        protocol: Protocol::RestJson(&APPCONFIGDATA),
        answer_model: Some(&APPCONFIGDATA_ANSWERS),
    },
    // `services/service-com-amazonaws-appconfig-amazonappconfig/smithy-model/
    // com-amazonaws-appconfig-amazonappconfig.json`: restJson1, sigv4Name `appconfig`,
    // endpointPrefix `appconfig`, sdkId `AppConfig`. Its requests are bound per operation out
    // of that same model ([`APPCONFIG`]).
    //
    // The CONTROL PLANE beside the data plane above: the application, environment,
    // configuration profile and hosted version a session is opened over, and the deployment
    // that put one in force, are read and written through these operations — so a handler that
    // authors or inspects the configuration it later reads reaches them here. It signs as
    // `appconfig` exactly as the data plane does, and the two are told apart by the paths
    // their own models bind.
    Api {
        sdk_token: "appconfig",
        signing_name: "appconfig",
        endpoint_prefix: "appconfig",
        model: Some(APPCONFIG_MODEL),
        protocol: Protocol::RestJson(&APPCONFIG),
        answer_model: Some(&APPCONFIG_ANSWERS),
    },
    // `services/service-com-amazonaws-configservice-starlingdoveservice/smithy-model/
    // com-amazonaws-configservice-starlingdoveservice.json`: awsJson1_1, sigv4Name `config`,
    // endpointPrefix `config`, sdkId `Config Service`, service shape `StarlingDoveService`.
    //
    // A function that is an AWS Config RULE'S OWN EVALUATOR has to reach this service: Config
    // invokes the function with a `ResultToken` and the function reports what it found with
    // `PutEvaluations`, which "is required in every Lambda function that is invoked by an
    // Config rule" — so a handler that cannot reach Config cannot deliver the verdict it was
    // invoked to produce. Reading the recorded configuration it judges (the resource reads and
    // the Config query language) is the same surface.
    Api {
        sdk_token: "config",
        signing_name: "config",
        endpoint_prefix: "config",
        model: Some(CONFIG_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "StarlingDoveService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&CONFIG_ANSWERS),
    },
    // The same service under the token its own SDK id gives it, as EventBridge and CloudWatch
    // Logs are above: `Config Service` run together is `configservice`.
    Api {
        sdk_token: "configservice",
        signing_name: "config",
        endpoint_prefix: "config",
        model: Some(CONFIG_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "StarlingDoveService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&CONFIG_ANSWERS),
    },
    // `services/service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/
    // com-amazonaws-codedeploy-codedeploy-20141006.json`: awsJson1_1, sigv4Name `codedeploy`,
    // endpointPrefix `codedeploy`, sdkId `CodeDeploy`, service shape `CodeDeploy_20141006`.
    //
    // A function that is a DEPLOYMENT'S OWN VALIDATION FUNCTION has to reach this service:
    // CodeDeploy invokes the function for a lifecycle event of a Lambda or Amazon ECS
    // deployment, handing it the deployment id and the hook execution id, and the function
    // reports what it found with `PutLifecycleEventHookExecutionStatus` — the call every such
    // function AWS documents ends with — so a handler that cannot reach CodeDeploy cannot
    // deliver the verdict it was invoked to produce, and the deployment it gates never learns
    // of it.
    Api {
        sdk_token: "codedeploy",
        signing_name: "codedeploy",
        endpoint_prefix: "codedeploy",
        model: Some(CODEDEPLOY_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "CodeDeploy_20141006",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&CODEDEPLOY_ANSWERS),
    },
    // `services/service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/
    // com-amazonaws-cloudfront-cloudfront2020-05-31.json`: restXml, sigv4Name `cloudfront`,
    // endpointPrefix `cloudfront`, sdkId `CloudFront`. Its operations bind their input to a
    // path and an XML document rather than to a JSON body, so the request is built by
    // [`super::aws_cloudfront`].
    //
    // A function that REFRESHES THE EDGE after writing the object behind it is the shape that
    // reaches this service: the write lands in the origin and the handler invalidates the
    // paths the distribution is still serving, so a handler that cannot reach CloudFront can
    // only leave stale content being served.
    Api {
        sdk_token: "cloudfront",
        signing_name: "cloudfront",
        endpoint_prefix: "cloudfront",
        model: None,
        protocol: Protocol::CloudFrontRest,
        answer_model: None,
    },
    // `services/service-com-amazonaws-rekognition-rekognitionservice/smithy-model/
    // com-amazonaws-rekognition-rekognitionservice.json`: awsJson1_1, sigv4Name `rekognition`,
    // endpointPrefix `rekognition`, sdkId `Rekognition`, service shape `RekognitionService`.
    //
    // ANALYSING THE OBJECT THAT TRIGGERED THE INVOCATION is the shape of a function an object
    // store's event notification runs: the handler is handed the bucket and the key, asks this
    // service what the image holds (`DetectLabels`, `DetectText`, `DetectModerationLabels`,
    // the face and comparison operations) and records the answer somewhere a later reader
    // addresses — so a handler that cannot reach it can record nothing about the object it was
    // invoked for, and the pipeline behind the event never produces its row.
    Api {
        sdk_token: "rekognition",
        signing_name: "rekognition",
        endpoint_prefix: "rekognition",
        model: Some(REKOGNITION_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "RekognitionService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&REKOGNITION_ANSWERS),
    },
    // `services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
    // com-amazonaws-lambda-awsgirapiservice.json`: restJson1, sigv4Name `lambda`,
    // endpointPrefix `lambda`, sdkId `Lambda`. Its requests are bound per operation out of
    // that same model ([`LAMBDA`]) — this crate's OWN model, so the surface a handler
    // reaches is exactly the one this service serves.
    //
    // A function that INVOKES ANOTHER FUNCTION — or itself — is the shape that reaches this
    // service: a fan-out handler hands each item to a worker function, a step of a pipeline
    // calls the next one, and a self-invoking handler carries a chain on. Without it such a
    // handler can make no progress at all, and the recursive-loop guardrail
    // ([`super::super::recursion`]) has no chain to detect, because the chain is made of
    // these very calls.
    Api {
        sdk_token: "lambda",
        signing_name: "lambda",
        endpoint_prefix: "lambda",
        model: Some(LAMBDA_MODEL),
        protocol: Protocol::RestJson(&LAMBDA),
        answer_model: Some(&LAMBDA_ANSWERS),
    },
    // `services/service-com-amazonaws-glue-awsglue/smithy-model/
    // com-amazonaws-glue-awsglue.json`: awsJson1_1, sigv4Name `glue`, endpointPrefix `glue`,
    // sdkId `Glue`, service shape `AWSGlue`.
    //
    // THE DATA CATALOG is what a program that reads or writes a data lake resolves its tables
    // against: a script handed a database and a table name reads that table's metadata — where
    // its data lives and which columns it declares — before it can read a row of it, and
    // registers what it wrote afterwards. A program that cannot reach this service cannot
    // perform a catalog-backed read at all, and the Lake Formation governance of such a read is
    // what this service applies to it.
    //
    // No model is held for it here: its requests are bound by the AWS JSON protocol above —
    // the operation in the target header, the input document as the body — so nothing of its
    // model is read to build one.
    Api {
        sdk_token: "glue",
        signing_name: "glue",
        endpoint_prefix: "glue",
        model: None,
        protocol: Protocol::AwsJson {
            target_prefix: "AWSGlue",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: None,
    },
];

/// Whether `named` and `asked` name the ONE service.
///
/// The SDKs spell one service's id two ways: hyphenated, which is the name a `boto3` client
/// is built with for a two-word id (`rds-data`) and the name the JavaScript SDK publishes
/// that service's package under (`@aws-sdk/client-cloudwatch-logs`), and run together, which
/// is the spelling the same id gives with its spaces simply dropped (`cloudwatchlogs`). Both
/// are lower case and differ in nothing else, so comparing two WELL-FORMED tokens with their
/// hyphens dropped is what makes those two spellings one token: a client built with either
/// reaches the same service, while a string no SDK spells a service by — a different case, a
/// hyphen with no word beside it, a character no token carries — names no service, which is
/// what a client built with it is told.
#[must_use]
pub fn same_service_token(named: &str, asked: &str) -> bool {
    well_formed_token(named) && well_formed_token(asked) && compacted(named).eq(compacted(asked))
}

/// Whether `token` is spelled the way an SDK spells a service token: a non-empty run of
/// lower-case ASCII alphanumeric words, each pair of them held apart by one hyphen.
///
/// What this keeps out is a string no SDK publishes for any service — `RDS-Data`, `-sqs`,
/// `rds--data`, `rds_data`, a token of hyphens alone: it is told it names no service rather
/// than read as a spelling of one.
fn well_formed_token(token: &str) -> bool {
    !token.is_empty()
        && token.split('-').all(|word| {
            !word.is_empty()
                && word
                    .bytes()
                    .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit())
        })
}

/// One service token's characters, as [`same_service_token`] compares them: its hyphens
/// dropped, which is the one thing its two spellings differ in.
fn compacted(token: &str) -> impl Iterator<Item = char> + '_ {
    token.chars().filter(|character| *character != '-')
}

/// The reachable service `service` names, or `None` when no entry of [`APIS`] answers to that
/// token.
///
/// ONE service is named by two spellings of its SDK id: an SDK client is built from that id
/// with the words run together (`boto3.client("secretsmanager")`) and the AWS SDK for
/// JavaScript publishes the same service's client package with them hyphenated
/// (`@aws-sdk/client-secrets-manager`), whose token is what a handler importing it hands over.
/// Both spell the same service, so both reach it ([`same_service_token`]); a token no entry
/// answers to under either spelling is still refused.
fn api_for(service: &str) -> Option<&'static Api> {
    APIS.iter()
        .find(|api| same_service_token(api.sdk_token, service))
}

/// The Smithy MODEL of each service a call may be made to, embedded from the crate that
/// service's own emulator is generated from — the SAME file, so nothing read out of it can
/// drift from the surface that service serves. The include is a COMPILE-TIME read: a model
/// that moves fails this crate's build rather than changing what a call does.
///
/// Which service each belongs to is stated on that service's own [`APIS`] entry
/// ([`Api::model`]); what is read out of them is the names the service's own SDK package
/// exports, which an `import` of that package binds, and the input members an SDK client fills
/// in itself (`super::node_sdk_exports`, which answers both from one reading), and for a
/// `restJson1` service where each member of a request belongs ([`super::rest_json`]).
pub(super) const SQS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-sqs-amazonsqs/smithy-model/\
     com-amazonaws-sqs-amazonsqs.json"
);
pub(super) const SNS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/\
     com-amazonaws-sns-amazonsimplenotificationservice.json"
);
pub(super) const EVENTBRIDGE_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-eventbridge-awsevents/smithy-model/\
     com-amazonaws-eventbridge-awsevents.json"
);
pub(super) const PIPES_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-pipes-pipes/smithy-model/\
     com-amazonaws-pipes-pipes.json"
);
pub(super) const DYNAMODB_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/\
     com-amazonaws-dynamodb-dynamodb-20120810.json"
);
pub(super) const CLOUDWATCH_LOGS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-cloudwatchlogs-logs-20140328/smithy-model/\
     com-amazonaws-cloudwatchlogs-logs-20140328.json"
);
pub(super) const CLOUDWATCH_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-cloudwatch-graniteserviceversion20100801/smithy-model/\
     com-amazonaws-cloudwatch-graniteserviceversion20100801.json"
);
pub(super) const SECRETSMANAGER_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-secretsmanager-secretsmanager/smithy-model/\
     com-amazonaws-secretsmanager-secretsmanager.json"
);
pub(super) const SSM_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-ssm-amazonssm/smithy-model/\
     com-amazonaws-ssm-amazonssm.json"
);
pub(super) const KMS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-kms-trentservice/smithy-model/\
     com-amazonaws-kms-trentservice.json"
);
pub(super) const APIGATEWAY_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/\
     com-amazonaws-apigateway-backplanecontrolservice.json"
);
pub(super) const APIGATEWAYMANAGEMENTAPI_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/\
     smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json"
);
pub(super) const SESV2_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-sesv2-simpleemailservice-v2/smithy-model/\
     com-amazonaws-sesv2-simpleemailservice-v2.json"
);
pub(super) const S3_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-s3-amazons3/smithy-model/com-amazonaws-s3-amazons3.json"
);
pub(super) const SFN_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-sfn-awsstepfunctions/smithy-model/\
     com-amazonaws-sfn-awsstepfunctions.json"
);
pub(super) const RDS_DATA_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-rdsdata-rdsdataservice/smithy-model/\
     com-amazonaws-rdsdata-rdsdataservice.json"
);
pub(super) const OPENSEARCH_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-opensearch-amazonopensearchservice/smithy-model/\
     com-amazonaws-opensearch-amazonopensearchservice.json"
);
pub(super) const XRAY_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-xray-awsxray/smithy-model/\
     com-amazonaws-xray-awsxray.json"
);
pub(super) const APPCONFIGDATA_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-appconfigdata-appconfigdata/smithy-model/\
     com-amazonaws-appconfigdata-appconfigdata.json"
);
pub(super) const APPCONFIG_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-appconfig-amazonappconfig/smithy-model/\
     com-amazonaws-appconfig-amazonappconfig.json"
);
pub(super) const CONFIG_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-configservice-starlingdoveservice/smithy-model/\
     com-amazonaws-configservice-starlingdoveservice.json"
);
pub(super) const CODEDEPLOY_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-codedeploy-codedeploy-20141006/smithy-model/\
     com-amazonaws-codedeploy-codedeploy-20141006.json"
);
pub(super) const REKOGNITION_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-rekognition-rekognitionservice/smithy-model/\
     com-amazonaws-rekognition-rekognitionservice.json"
);
/// THIS service's own model, read the same way as every other entry's: a call a handler makes
/// to Lambda is bound by the model this crate is generated from, so the surface a running
/// function reaches is the surface this emulator serves.
pub(super) const LAMBDA_MODEL: &str =
    include_str!("../../../smithy-model/com-amazonaws-lambda-awsgirapiservice.json");

/// AWS X-Ray's own model, which a call to it is bound by, embedded exactly as the API
/// Gateway model below is and for the same reason: the wire facts of its operations are that
/// model's, so this seam states none of them.
static XRAY: super::rest_json::RestApi = super::rest_json::RestApi {
    model: XRAY_MODEL,
    namespace: "com.amazonaws.xray",
    parsed: ::std::sync::OnceLock::new(),
};

/// The model this emulator holds for the service `token` names, and WHERE in [`APIS`] that
/// service's entry sits — so a reader of these models holds what it read per ENTRY rather than
/// per name asked for, and a token no entry answers to parses nothing and holds nothing,
/// however many distinct ones a run names.
///
/// The token is matched as [`api_for`] matches it, and the model comes off the entry that
/// answers, so every token that names ONE service answers with that service's model.
/// `None` for a service this emulator holds no model for.
pub(super) fn held_model(token: &str) -> Option<(usize, &'static str)> {
    APIS.iter()
        .enumerate()
        .find(|(_at, api)| same_service_token(api.sdk_token, token))
        .and_then(|(at, api)| api.model.map(|model| (at, model)))
}

/// WHICH values of one service's answers carry BYTES, read out of that service's own model on
/// the first call to it ([`super::blob_members`]).
///
/// One per service a call may be made to whose answers this seam reads itself: an AWS JSON one,
/// whose answer document IS the body, and a `restJson1` one, whose answer this seam reads
/// through [`super::rest_json`]. Each stands on the same embedded file above, so nothing read
/// here can drift from the surface the service serves, and what each KEEPS is the compact plan
/// rather than the model document.
static SQS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: SQS_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static EVENTBRIDGE: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: EVENTBRIDGE_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static DYNAMODB: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: DYNAMODB_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static PIPES_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: PIPES_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static CLOUDWATCH_LOGS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: CLOUDWATCH_LOGS_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static CLOUDWATCH: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: CLOUDWATCH_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static SECRETSMANAGER: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: SECRETSMANAGER_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static SFN: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: SFN_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static SSM: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: SSM_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static KMS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: KMS_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static APIGATEWAY_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: APIGATEWAY_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static APIGATEWAYMANAGEMENTAPI_ANSWERS: super::blob_members::ServiceModel =
    super::blob_members::ServiceModel {
        source: APIGATEWAYMANAGEMENTAPI_MODEL,
        plan: ::std::sync::OnceLock::new(),
    };
static OPENSEARCH_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: OPENSEARCH_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static XRAY_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: XRAY_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static SESV2_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: SESV2_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static RDS_DATA_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: RDS_DATA_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static APPCONFIGDATA_ANSWERS: super::blob_members::ServiceModel =
    super::blob_members::ServiceModel {
        source: APPCONFIGDATA_MODEL,
        plan: ::std::sync::OnceLock::new(),
    };
static APPCONFIG_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: APPCONFIG_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static CONFIG_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: CONFIG_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static CODEDEPLOY_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: CODEDEPLOY_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static REKOGNITION_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: REKOGNITION_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static LAMBDA_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: LAMBDA_MODEL,
    plan: ::std::sync::OnceLock::new(),
};

/// THIS service's own model, which a call a handler makes to Lambda is bound by — embedded
/// exactly as [`APIGATEWAY`] is, and for the same reason: the wire facts of each operation
/// are that model's, so this seam states none of them.
static LAMBDA: super::rest_json::RestApi = super::rest_json::RestApi {
    model: LAMBDA_MODEL,
    namespace: "com.amazonaws.lambda",
    parsed: ::std::sync::OnceLock::new(),
};

/// Amazon API Gateway's own model, which a call to it is bound by.
///
/// The model is the one the API Gateway emulator's own crate is generated from, embedded
/// here: the wire facts of 124 operations — a method, a URI, which member is a label, a query
/// parameter, a header or a body member, and what the wire calls each one — are that model's,
/// so this seam states none of them and none can drift from the surface the service serves.
/// The include is a COMPILE-TIME read of that file: a model that moves fails this crate's
/// build rather than changing what a call does.
static APIGATEWAY: super::rest_json::RestApi = super::rest_json::RestApi {
    model: APIGATEWAY_MODEL,
    namespace: "com.amazonaws.apigateway",
    parsed: ::std::sync::OnceLock::new(),
};

/// The `@connections` management API of an API Gateway WebSocket api, which a call to it is
/// bound by — embedded exactly as [`APIGATEWAY`] is, and for the same reason: the wire facts
/// of its three operations are that model's, so this seam states none of them.
static APIGATEWAYMANAGEMENTAPI: super::rest_json::RestApi = super::rest_json::RestApi {
    model: APIGATEWAYMANAGEMENTAPI_MODEL,
    namespace: "com.amazonaws.apigatewaymanagementapi",
    parsed: ::std::sync::OnceLock::new(),
};

/// EventBridge Pipes' own model, which a call to it is bound by — embedded exactly as
/// [`APIGATEWAY`] is, and for the same reason: the wire facts of each operation are that
/// model's, so this seam states none of them.
static PIPES: super::rest_json::RestApi = super::rest_json::RestApi {
    model: PIPES_MODEL,
    namespace: "com.amazonaws.pipes",
    parsed: ::std::sync::OnceLock::new(),
};

/// Amazon OpenSearch Service's own model, which a call to it is bound by — embedded exactly
/// as [`APIGATEWAY`] is, and for the same reason: the wire facts of each operation are that
/// model's, so this seam states none of them.
static OPENSEARCH: super::rest_json::RestApi = super::rest_json::RestApi {
    model: OPENSEARCH_MODEL,
    namespace: "com.amazonaws.opensearch",
    parsed: ::std::sync::OnceLock::new(),
};

/// Amazon SES API v2's own model, which a call to it is bound by, embedded exactly as the
/// API Gateway model above is and for the same reason: the wire facts of its operations are
/// that model's, so this seam states none of them.
static SESV2: super::rest_json::RestApi = super::rest_json::RestApi {
    model: SESV2_MODEL,
    namespace: "com.amazonaws.sesv2",
    parsed: ::std::sync::OnceLock::new(),
};

/// The RDS Data API's own model, which a call to it is bound by, embedded exactly as the
/// models above are and for the same reason: the wire facts of its operations are that
/// model's, so this seam states none of them.
static RDS_DATA: super::rest_json::RestApi = super::rest_json::RestApi {
    model: RDS_DATA_MODEL,
    namespace: "com.amazonaws.rdsdata",
    parsed: ::std::sync::OnceLock::new(),
};

/// AppConfig's DATA plane's own model, which a call to it is bound by, embedded exactly as the
/// models above are and for the same reason: the wire facts of its operations are that model's,
/// so this seam states none of them.
static APPCONFIGDATA: super::rest_json::RestApi = super::rest_json::RestApi {
    model: APPCONFIGDATA_MODEL,
    namespace: "com.amazonaws.appconfigdata",
    parsed: ::std::sync::OnceLock::new(),
};

/// AppConfig's CONTROL plane's own model, embedded the same way and for the same reason.
static APPCONFIG: super::rest_json::RestApi = super::rest_json::RestApi {
    model: APPCONFIG_MODEL,
    namespace: "com.amazonaws.appconfig",
    parsed: ::std::sync::OnceLock::new(),
};

/// Every service token a call may name, in the order [`APIS`] lists them — what the guest
/// is told when it names one that is not there.
#[must_use]
pub fn reachable_services() -> Vec<&'static str> {
    APIS.iter().map(|api| api.sdk_token).collect()
}

/// The CREDENTIAL one invocation's calls are signed with.
///
/// Held by the host and never reachable from guest code: it reaches the request's
/// `Authorization` and `x-amz-security-token` headers and nothing else — no response, no
/// log and no environment variable of the run ([`super::runtime_environment`] puts no
/// credential in a run's environment).
#[derive(Clone)]
pub struct CallerIdentity {
    /// The access key id, which is where the service serving a call reads WHO made it.
    pub access_key_id: String,
    /// Its secret, used to sign and nowhere else.
    pub secret_access_key: String,
    /// The session token a temporary credential is served only with.
    pub session_token: String,
}

/// Prints no field at all, so neither the key nor its secret can reach a log through a
/// debug form of this type or of anything holding one.
impl ::std::fmt::Debug for CallerIdentity {
    fn fmt(&self, f: &mut ::std::fmt::Formatter<'_>) -> ::std::fmt::Result {
        f.debug_struct("CallerIdentity").finish_non_exhaustive()
    }
}

/// What a running handler reaches the rest of this emulator through.
///
/// Built from the operation context of the call that started the run, so the callback it
/// holds is the one the framework handed that handler, and the Region is the one the
/// invocation was addressed to. It is `Clone` and `'static` because a run happens on a
/// thread of its own, past the lifetime of the request that started it.
#[derive(Debug, Clone)]
pub struct AwsAccess {
    /// The in-process callback every call is dispatched through.
    callback: EmbeddedEmulator,
    /// The async runtime the dispatch is driven on. A run's thread is not one of its
    /// workers, so a call is driven by blocking that thread on this handle rather than by
    /// making the interpreter asynchronous.
    runtime: ::tokio::runtime::Handle,
    /// The Region a call is made in unless the SDK client the guest built names another.
    region: String,
    /// The credential every call is signed with: the session of the function's execution
    /// role, or `None` where no session could be minted for it, in which case the call is
    /// signed with the credential the framework hands this emulator's handlers.
    identity: Option<CallerIdentity>,
    /// The TRACING HEADER every call carries, which is the invocation's own
    /// ([`super::trace_context`]) — `None` for a run that has no trace context.
    ///
    /// An AWS SDK running inside a Lambda invocation reads the context the runtime reports
    /// in `_X_AMZN_TRACE_ID` and stamps it on each request it makes, which is what makes the
    /// hops of ONE request chain recognisable as one chain by the service that serves them —
    /// the very thing Lambda's recursive-loop guardrail counts ([`super::super::recursion`]).
    /// So the seam the handler calls through carries it, and a hop that carries none belongs
    /// to no chain any service can place.
    ///
    /// It is the HOST's, like the credential beside it: guest code cannot read it off this
    /// seam or set one of its own, so a handler cannot hand its call a chain it chose.
    trace_header: Option<String>,
    /// Which service answers at an address this emulator published for a resource's own
    /// data plane, for a request the handler SENDS ITSELF to one
    /// ([`super::guest_http`]). `None` where this emulator vends no such records.
    published: Option<::service_common::published_data_planes::PublishedPlanes>,
}

impl AwsAccess {
    /// The access a handler invoked by this request gets, or `None` where there is no async
    /// runtime to dispatch a call on — in which case a run simply has no SDK and says so.
    #[must_use]
    pub fn new(callback: EmbeddedEmulator, region: &str) -> Option<Self> {
        let runtime = ::tokio::runtime::Handle::try_current().ok()?;
        Some(Self {
            callback,
            runtime,
            region: region.to_string(),
            identity: None,
            trace_header: None,
            published: None,
        })
    }

    /// The same access, signing every call it makes as `identity` — the session of the
    /// function's own execution role ([`super::execution_role_session`]).
    ///
    /// `None` leaves it signing with the framework's own credential, which is the position
    /// an invocation whose execution role this emulator holds no record of is in: nothing
    /// is known about that role, so nothing is claimed about it.
    #[must_use]
    pub fn signing_as(mut self, identity: Option<CallerIdentity>) -> Self {
        self.identity = identity;
        self
    }

    /// The same access, stamping `trace_header` on every call it makes — the tracing header
    /// of the invocation whose handler is calling, as an SDK inside a function stamps it (see
    /// [`Self::trace_header`]).
    #[must_use]
    pub fn tracing(mut self, trace_header: String) -> Self {
        self.trace_header = Some(trace_header);
        self
    }

    /// The same access, resolving an address this emulator PUBLISHED for a resource's own
    /// data plane against `published` — the records the service that opened that listener
    /// wrote ([`::service_common::published_data_planes`]).
    ///
    /// `None` leaves such an address resolving to nothing, so a request the handler sends to
    /// one is reported as a request that was not made: the position every address was in
    /// before a service published one.
    #[must_use]
    pub fn resolving_published_planes(
        mut self,
        published: Option<::service_common::published_data_planes::PublishedPlanes>,
    ) -> Self {
        self.published = published;
        self
    }

    /// The Region a call with no Region of its own is made in.
    #[must_use]
    pub fn region(&self) -> &str {
        &self.region
    }

    /// Make ONE call: `operation` of `service`, carrying `body` as its input document.
    ///
    /// `region` is the Region the SDK client the guest built names, which is honoured when
    /// it is a Region name at all and otherwise replaced by the invocation's own — a
    /// guest-supplied value reaches the request's host and credential scope, so it is
    /// checked here rather than taken as given. `within` bounds the call: it is what the
    /// run has left of its deadline, so a call cannot outlast the invocation that made it.
    pub fn call(
        &self,
        service: &str,
        operation: &str,
        region: &str,
        body: &str,
        within: Duration,
    ) -> Result<Answer, Refusal> {
        self.runtime
            .block_on(self.call_within(service, operation, region, body, within))
    }

    /// The SAME call, for a caller that is already on the async runtime — the runtime itself
    /// delivering an invocation's log ([`super::log_delivery`]) rather than a guest making an
    /// SDK call from its own thread. Blocking that thread on the handle, which is what
    /// [`call`](Self::call) does, is not open to a caller already running on it.
    pub async fn call_within(
        &self,
        service: &str,
        operation: &str,
        region: &str,
        body: &str,
        within: Duration,
    ) -> Result<Answer, Refusal> {
        let Some(api) = api_for(service) else {
            return Err(Refusal::UnknownService);
        };
        if !well_formed_operation(operation) {
            return Err(Refusal::UnknownOperation);
        }
        if body.len() > MAX_REQUEST_BYTES {
            return Err(Refusal::RequestTooLarge);
        }
        let region = if well_formed_region(region) {
            region
        } else {
            &self.region
        };
        // Every AWS SDK client generates a value for an input member the service's model marks
        // as an idempotency token when the caller left it out, and sends it with the request
        // (`super::idempotency_token`) — so this seam, which is the lower half of every SDK
        // this runtime supplies, fills one here, before the request is built and signed.
        let filled = super::idempotency_token::filled(api.sdk_token, operation, body);
        let body = filled.as_deref().unwrap_or(body);
        match ::tokio::time::timeout(within, self.dispatched(api, region, operation, body)).await
        {
            Ok(answer) => answer,
            Err(_elapsed) => Err(Refusal::TimedOut),
        }
    }
    /// Mint the AUTHENTICATION TOKEN a handler presents to a database endpoint that serves
    /// IAM database authentication, for `user` on `host`:`port`.
    ///
    /// Not a call: the SDKs build this token in the client
    /// ([`super::db_auth_token`]), so nothing is dispatched and no service answers. It is
    /// here beside [`call`](Self::call) because the credential it is signed with is the one
    /// this seam holds, and because the guest reaches both the same way.
    ///
    /// `region` is the Region the SDK client the guest built names, honoured when it is a
    /// Region name at all and otherwise replaced by the invocation's own — the same reading
    /// [`call`](Self::call) gives it, for the same reason: the value reaches the token's
    /// credential scope.
    pub fn db_auth_token(
        &self,
        host: &str,
        port: u16,
        user: &str,
        region: &str,
    ) -> Result<String, Refusal> {
        let region = if well_formed_region(region) {
            region
        } else {
            &self.region
        };
        self.runtime
            .block_on(super::db_auth_token::db_auth_token(
                &self.callback,
                host,
                port,
                user,
                region,
            ))
            .map_err(Refusal::NotMade)
    }

    /// One call, built the way the called service's own protocol binds it.
    async fn dispatched(
        &self,
        api: &Api,
        region: &str,
        operation: &str,
        body: &str,
    ) -> Result<Answer, Refusal> {
        match api.protocol {
            Protocol::AwsJson { .. } | Protocol::RestJson(_) => {
                dispatch(
                    &self.callback,
                    api,
                    region,
                    operation,
                    body,
                    self.identity.as_ref(),
                    self.trace_header.as_deref(),
                )
                .await
            }
            Protocol::S3Rest => {
                super::aws_s3::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                )
                .await
            }
            // CloudFront is reached at ONE endpoint whatever Region the client the guest
            // built names, so the Region is not threaded into this call
            // ([`super::aws_cloudfront::SIGNING_REGION`]).
            Protocol::CloudFrontRest => {
                super::aws_cloudfront::call(
                    &self.callback,
                    self.identity.as_ref(),
                    operation,
                    body,
                )
                .await
            }
            Protocol::SnsQuery => {
                super::aws_sns::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                )
                .await
            }
        }
    }

    /// ONE HTTP request the handler built and signed ITSELF, sent into this emulator:
    /// `method` `url`, carrying `headers` (a JSON array of name/value pairs) and `body` (its
    /// bytes as base64 text).
    ///
    /// The address a URL may name, the signature and the destination are
    /// [`super::guest_http`]'s; `within` is what the run has left of its deadline, so a
    /// request cannot outlast the invocation that made it.
    pub fn send(
        &self,
        method: &str,
        url: &str,
        headers: &str,
        body: &str,
        within: Duration,
    ) -> Result<super::guest_http::Answered, Refusal> {
        use ::base64::Engine;

        let headers: Vec<(String, String)> = ::serde_json::from_str::<Vec<(String, String)>>(
            headers,
        )
        .map_err(|error| Refusal::NotMade(format!("the request's headers are not a list: {error}")))?;
        let body = ::base64::engine::general_purpose::STANDARD
            .decode(body)
            .map_err(|error| Refusal::NotMade(format!("the request's body is not base64: {error}")))?;
        self.runtime.block_on(super::guest_http::sent(
            &super::guest_http::Sending {
                callback: &self.callback,
                default_region: &self.region,
                method,
                url,
                headers: &headers,
                body: &body,
                within,
                identity: self.identity.as_ref(),
                published: self.published.as_ref(),
            },
        ))
    }

    /// The URL one call carries ITS OWN signature in, as an SDK presigns one.
    ///
    /// Nothing is sent. The URL names the service's own endpoint and carries a signature
    /// over exactly that request, made with the SAME identity the handler's ordinary calls
    /// are made as — the session of the function's own execution role
    /// ([`super::execution_role_session`]). The guest holds that credential no more than it
    /// holds any other: what it gets back is an access key id and a signature over the one
    /// request the URL names, good for `expires_in` and for nothing else, which is what a
    /// presigned URL is.
    ///
    /// Only an OBJECT URL is presigned ([`super::aws_s3::presigned`]), because that is the
    /// one a handler hands out; a URL asked for of any other service is reported as one that
    /// was not signed, naming the service.
    pub fn presigned(
        &self,
        service: &str,
        operation: &str,
        region: &str,
        body: &str,
        expires_in: Duration,
    ) -> Result<String, Refusal> {
        let Some(api) = api_for(service) else {
            return Err(Refusal::UnknownService);
        };
        if !well_formed_operation(operation) {
            return Err(Refusal::UnknownOperation);
        }
        if body.len() > MAX_REQUEST_BYTES {
            return Err(Refusal::RequestTooLarge);
        }
        if !matches!(api.protocol, Protocol::S3Rest) {
            return Err(Refusal::NotMade(format!(
                "the AWS SDK this runtime supplies presigns an object URL only; {} carries \
                 its input in a request it signs when it is made",
                api.sdk_token
            )));
        }
        let region = if well_formed_region(region) {
            region.to_string()
        } else {
            self.region.clone()
        };
        let expires_in = expires_in.min(MAX_PRESIGNED_LIFETIME);
        self.runtime.block_on(super::aws_s3::presigned(
            &self.callback,
            self.identity.as_ref(),
            &region,
            operation,
            body,
            expires_in,
        ))
    }
}

/// The longest a presigned URL this seam hands back is good for.
///
/// SigV4's own ceiling for a query-signed request is seven days
/// ("X-Amz-Expires … the maximum value is 604800"), so a guest asking for longer is given a
/// URL good for that, not one that is refused.
const MAX_PRESIGNED_LIFETIME: Duration = Duration::from_secs(604_800);

/// Why a call was not made at all.
///
/// Each of these is reported to the guest as the failure it is; none of them is answered
/// with a made-up success.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Refusal {
    /// The token names no service this seam reaches ([`APIS`]).
    UnknownService,
    /// The operation name is not one an AWS JSON target header can carry.
    UnknownOperation,
    /// The input document is larger than [`MAX_REQUEST_BYTES`].
    RequestTooLarge,
    /// The answer is larger than [`MAX_RESPONSE_BYTES`].
    ResponseTooLarge,
    /// The call did not finish inside what the run had left.
    TimedOut,
    /// The call could not be built, signed or dispatched; the string is why.
    NotMade(String),
}

/// What one call came back with: the emulated service's own answer.
#[derive(Debug, Clone)]
pub struct Answer {
    /// The HTTP status it answered.
    pub status: u16,
    /// Its body, as text.
    pub body: String,
    /// The error shape it named in [`ERROR_TYPE_HEADER`], for a refusal.
    pub error_type: Option<String>,
    /// The LEGACY error code it named in [`QUERY_ERROR_HEADER`], for a refusal answered by a
    /// query-compatible service, with the fault the header states after it dropped.
    ///
    /// It is the code the SDK whose own model keys such a service's error classes by it
    /// reports a refusal under ([`super::aws_sdk_guest`]). `None` for every answer that
    /// carried none, which is every answer of every other service.
    pub query_error_code: Option<String>,
    /// The id it gave the request it answered.
    pub request_id: Option<String>,
    /// The output members of [`Self::body`] that carry BYTES rather than text, as the
    /// base64 the answer's document carries them as — an object's own payload, say, or the
    /// member a REST-bound answer binds to the whole body whose shape is a blob. The
    /// SDK the guest calls hands each of them over as the stream its own client answers
    /// with ([`super::aws_sdk_guest`]), so a handler reads bytes where it reads bytes on a
    /// managed runtime. Empty for every answer whose members are all text, which is every
    /// AWS JSON one.
    pub streams: Vec<String>,
    /// WHERE in [`Self::body`] the answer carries BYTES rather than text: one path per value
    /// the called operation's own model types as a non-streaming blob
    /// ([`super::blob_members`]), each a JSON array of the object keys and array indices that
    /// reach it from the document's root. A JSON protocol carries such a value as the base64
    /// of its bytes, and the SDK the guest calls hands over the BYTES, so a handler reads
    /// bytes where it reads bytes on a managed runtime. Empty for an answer carrying none.
    pub blobs: Vec<::serde_json::Value>,
}

/// Build, sign and dispatch one request, and read the answer back.
async fn dispatch(
    callback: &EmbeddedEmulator,
    api: &Api,
    region: &str,
    operation: &str,
    body: &str,
    identity: Option<&CallerIdentity>,
    trace_header: Option<&str>,
) -> Result<Answer, Refusal> {
    sent(callback, api, region, operation, body, identity, trace_header)
        .await
        .map_err(|detail| match detail {
            Sent::TooLarge => Refusal::ResponseTooLarge,
            Sent::NotMade(detail) => Refusal::NotMade(detail),
        })
}

/// Why one dispatch produced no answer.
enum Sent {
    /// The answer is larger than [`MAX_RESPONSE_BYTES`].
    TooLarge,
    /// The request could not be built, signed or dispatched.
    NotMade(String),
}

/// The dispatch itself.
async fn sent(
    callback: &EmbeddedEmulator,
    api: &Api,
    region: &str,
    operation: &str,
    body: &str,
    identity: Option<&CallerIdentity>,
    trace_header: Option<&str>,
) -> Result<Answer, Sent> {
    let host = api.host(region);
    let mut builder = ::http::Request::builder().header("host", host.clone());
    // The invocation's own tracing context, stamped on the request the way an SDK inside a
    // function stamps it (see [`AwsAccess::trace_header`]). It is put on BEFORE the signature
    // below, so it travels signed with the rest of the request.
    if let Some(trace) = trace_header {
        builder = builder.header(super::trace_context::TRACE_HEADER, trace);
    }
    let sent_body = match &api.protocol {
        Protocol::AwsJson {
            target_prefix,
            query_compatible,
            ..
        } => {
            builder = builder
                .method("POST")
                .uri(format!("https://{host}/"))
                .header("content-type", api.content_type())
                .header(TARGET_HEADER, format!("{target_prefix}.{operation}"));
            // A client of a query-compatible service asks for the legacy error vocabulary
            // its own clients match on, and only a request that asks for it is answered
            // with those codes.
            if *query_compatible {
                builder = builder.header(QUERY_MODE_HEADER, "true");
            }
            body.as_bytes().to_vec()
        }
        Protocol::RestJson(rest) => {
            // The input document the guest sent is keyed by the operation's own member
            // names; where each belongs on the wire is that service's model's to say.
            let input = ::serde_json::from_str::<::serde_json::Value>(body)
                .unwrap_or(::serde_json::Value::Null);
            let bound = super::rest_json::bind(rest, operation, &input).map_err(Sent::NotMade)?;
            builder = builder
                .method(bound.method.as_str())
                .uri(format!("https://{host}{}", bound.target));
            if bound.json_body {
                builder = builder.header("content-type", api.content_type());
            }
            for (name, value) in bound.headers {
                builder = builder.header(name, value);
            }
            bound.body
        }
        // Amazon S3's own protocol is bound by the client [`super::aws_s3`] holds,
        // CloudFront's by the one [`super::aws_cloudfront`] holds and Amazon SNS's by the one
        // [`super::aws_sns`] holds, which is where a call to each is dispatched
        // ([`AwsAccess::dispatched`]): no request to them is built here.
        Protocol::S3Rest | Protocol::CloudFrontRest | Protocol::SnsQuery => {
            return Err(Sent::NotMade(format!(
                "{} binds its requests with its own protocol rather than with a JSON one",
                api.sdk_token
            )))
        }
    };
    let mut request = builder
        .body(sent_body)
        .map_err(|error| Sent::NotMade(error.to_string()))?;
    signed(&mut request, api.signing_name, region, callback, identity)
        .await
        .map_err(Sent::NotMade)?;
    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| Sent::NotMade(error.to_string()))?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &callback.http_connector(),
        request,
    )
    .await
    .map_err(|error| Sent::NotMade(error.to_string()))?;
    let status = response.status().as_u16();
    let header = |name: &str| response.headers().get(name).map(|value| value.to_string());
    let error_type = header(ERROR_TYPE_HEADER);
    // `<code>;<fault>`: the code is the part a client reports, and the fault is the one the
    // status already says, so only the code is read back.
    let query_error_code = header(QUERY_ERROR_HEADER).and_then(|carried| {
        let code = carried.split(';').next().unwrap_or_default().trim();
        (!code.is_empty()).then(|| code.to_string())
    });
    let request_id = header(REQUEST_ID_HEADER);
    let payload = response.body().bytes().ok_or_else(|| {
        Sent::NotMade("the in-process answer carried a streaming body".to_string())
    })?;
    if payload.len() > MAX_RESPONSE_BYTES {
        return Err(Sent::TooLarge);
    }
    let answered = String::from_utf8_lossy(payload).into_owned();
    // A REST-bound answer carries its output members where that operation's own model says,
    // so what the guest reads back is the output document keyed by those members' own names.
    // A REFUSAL is left exactly as the service worded it: its code and message are the
    // service's own, and the SDK raises them as they are.
    let body = match (&api.protocol, status) {
        (Protocol::RestJson(rest), 200..=299) => super::rest_json::read(
            rest,
            operation,
            status,
            &Answered(response.headers()),
            &answered,
        ),
        _ => answered,
    };
    // A member the answer binds to the WHOLE body whose shape is a blob is the body's own
    // BYTES, with nothing of the protocol encoded around them, so it is not read back as the
    // text those bytes happened to decode to: the bytes travel as base64 and the SDK the guest
    // calls hands them over as the stream its own client answers one with
    // ([`super::rest_json::streamed_payload`]).
    let (body, streams) = match (&api.protocol, (200..300).contains(&status)) {
        (Protocol::RestJson(rest), true) => {
            match super::rest_json::streamed_payload(rest, operation, &body, payload) {
                Some((document, member)) => (document, vec![member]),
                None => (body, Vec::new()),
            }
        }
        _ => (body, Vec::new()),
    };
    // WHICH of the answer's values carry bytes is the called service's own model's to say, so
    // the SDK the guest calls hands each of them over as bytes rather than as the base64 a
    // JSON protocol travels a blob as. A REFUSAL carries no output member at all.
    //
    // Read on a BLOCKING THREAD of the runtime rather than here: reading a service's model for
    // the first time in this process parses a document of several megabytes, and the thread
    // this is called on is the RUN's own, whose faulted pages are what the invocation's memory
    // ceiling is measured from ([`super::python::Bounds`]). The work a call makes for the
    // emulator is not the handler's memory, so it is not spent against it.
    let blobs = match (api.answer_model, (200..300).contains(&status)) {
        (Some(model), true) => {
            let operation = operation.to_string();
            let body = body.clone();
            ::tokio::task::spawn_blocking(move || {
                let document = ::serde_json::from_str::<::serde_json::Value>(&body)
                    .unwrap_or(::serde_json::Value::Null);
                model.answer_paths(&operation, &document)
            })
            .await
            .unwrap_or_default()
        }
        _ => Vec::new(),
    };
    Ok(Answer {
        status,
        body,
        error_type,
        query_error_code,
        request_id,
        streams,
        blobs,
    })
}

/// SigV4-sign one in-process request, as the identity the invocation was given.
///
/// The session of the function's execution role when one was minted for it — whose token
/// travels in the header a temporary credential is served with, put on the request BEFORE
/// the signature so it is signed with the rest — and otherwise the credential the framework
/// hands this emulator's handlers, which is what every other in-process call presents.
pub(super) async fn signed(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    callback: &EmbeddedEmulator,
    identity: Option<&CallerIdentity>,
) -> Result<(), String> {
    let Some(identity) = identity else {
        return ::service_common::in_process_sigv4::sign_callback_request(
            request,
            signing_name,
            region,
            callback,
        )
        .await;
    };
    request.headers_mut().insert(
        SESSION_TOKEN_HEADER,
        ::http::HeaderValue::from_str(&identity.session_token)
            .map_err(|error| error.to_string())?,
    );
    ::service_common::in_process_sigv4::sign_in_process_request_as(
        request,
        signing_name,
        region,
        &identity.access_key_id,
        &identity.secret_access_key,
    )
    .await
}

/// The headers of one answer, as the REST reading above reads them.
struct Answered<'a>(&'a ::aws_smithy_runtime_api::http::Headers);

impl super::rest_json::HeaderSource for Answered<'_> {
    fn header(&self, name: &str) -> Option<String> {
        self.0.get(name).map(str::to_string)
    }

    fn headers_with(&self, prefix: &str) -> Vec<(String, String)> {
        // A header name arrives lower-cased whatever case the model's prefix is written in,
        // so the prefix is matched the way HTTP compares a header name.
        let prefix = prefix.to_ascii_lowercase();
        self.0
            .iter()
            .filter_map(|(name, value)| {
                name.to_ascii_lowercase()
                    .strip_prefix(prefix.as_str())
                    .map(|rest| (rest.to_string(), value.to_string()))
            })
            .collect()
    }
}

/// The longest operation name a call may address.
///
/// Every AWS operation name is far shorter; the bound is here because the name travels in a
/// request HEADER, so its length is the host's to decide and not the guest's.
const MAX_OPERATION_NAME: usize = 128;

/// Whether `operation` is a name an AWS JSON target header can carry: a non-empty run of
/// ASCII letters and digits, no longer than [`MAX_OPERATION_NAME`].
///
/// This is what keeps a guest-supplied string out of the request's header structure: a name
/// carrying a separator, a control character or a newline is not an operation name, so no
/// request is built for it.
fn well_formed_operation(operation: &str) -> bool {
    !operation.is_empty()
        && operation.len() <= MAX_OPERATION_NAME
        && operation
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric())
        && operation.starts_with(|first: char| first.is_ascii_uppercase())
}

/// The longest Region name a call may name, for the same reason: it travels in the
/// request's host and in its credential scope.
const MAX_REGION_NAME: usize = 64;

/// Whether `region` is a Region name at all: a non-empty run of lower-case ASCII letters,
/// digits and hyphens, no longer than [`MAX_REGION_NAME`].
fn well_formed_region(region: &str) -> bool {
    !region.is_empty()
        && region.len() <= MAX_REGION_NAME
        && region
            .bytes()
            .all(|byte| byte.is_ascii_lowercase() || byte.is_ascii_digit() || byte == b'-')
}

#[cfg(test)]
mod tests {
    use super::{
        api_for, held_model, reachable_services, well_formed_operation, well_formed_region, APIS,
    };

    /// RULE: a service is reached by either spelling of its SDK id — the words run together, as
    /// an SDK client is built from it, and hyphenated, as the JavaScript SDK's client package is
    /// published under it — and by no other token.
    ///
    /// The rows span the spellings a guest hands over: the table's own token, the hyphenated
    /// package token of the services whose id is more than one word, and tokens no service
    /// answers to. What a row names is the SERVICE the token must reach, by its signing name,
    /// because that is what identifies a service whichever token names it.
    #[test]
    fn a_service_is_reached_by_either_spelling_of_its_token() {
        // (case, the token a guest named, the signing name of the service it must reach)
        let rows: &[(&str, &str, Option<&str>)] = &[
            (
                "the token the table states",
                "secretsmanager",
                Some("secretsmanager"),
            ),
            (
                "the JavaScript package's token for it",
                "secrets-manager",
                Some("secretsmanager"),
            ),
            (
                "the CloudWatch Logs package token",
                "cloudwatch-logs",
                Some("logs"),
            ),
            (
                "the API Gateway package token",
                "api-gateway",
                Some("apigateway"),
            ),
            (
                "the Step Functions package token",
                "step-functions",
                Some("states"),
            ),
            ("a single-word id", "s3", Some("s3")),
            ("a service no entry answers to", "kinesis", None),
            ("the empty token", "", None),
        ];
        for (case, named, expected) in rows {
            let reached = api_for(named).map(|api| api.signing_name);
            assert_eq!(
                reached, *expected,
                "case {case:?}: expected {named:?} to reach the service signing as {expected:?}, \
                 got {reached:?}"
            );
        }
    }

    /// RULE: every token that names ONE service answers with that service's own model, so what
    /// is read out of a model does not depend on which of its spellings a guest built its
    /// client with.
    ///
    /// The rows are the services this table reaches under more than one token, plus a service
    /// whose model this crate does not hold and one no entry answers to at all.
    #[test]
    fn both_tokens_of_a_service_answer_with_its_own_model() {
        // (case, two tokens of one service, whether a model is held for it)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("CloudWatch Logs", "logs", "cloudwatchlogs", true),
            ("Step Functions", "stepfunctions", "sfn", true),
            ("EventBridge", "events", "eventbridge", true),
            ("Secrets Manager", "secretsmanager", "secrets-manager", true),
            // The one question asked out of OpenSearch's model is how a request to it is
            // bound, which its protocol entry carries, so no model is held on the entry.
            ("OpenSearch", "opensearch", "es", false),
            ("no service at all", "kinesis", "not-a-service", false),
        ];
        for (case, one, other, held) in rows {
            let first = held_model(one).map(|(_at, model)| model);
            let second = held_model(other).map(|(_at, model)| model);
            assert_eq!(
                first.is_some(),
                *held,
                "case {case:?}: expected a model to be held={held} for {one:?}, got \
                 {}",
                first.is_some()
            );
            assert_eq!(
                first.map(str::len),
                second.map(str::len),
                "case {case:?}: expected {one:?} and {other:?} to answer with the one model"
            );
            assert!(
                first.is_none()
                    || first.zip(second).is_some_and(|(one, other)| ::std::ptr::eq(one, other)),
                "case {case:?}: expected {one:?} and {other:?} to answer with the one model, \
                 got two"
            );
        }
    }

    /// Every entry of the table names ONE service's wire facts, and the two tokens that
    /// name EventBridge reach the same service: an SDK client is built with the service's
    /// SDK id as often as with its endpoint prefix, and both are that service.
    #[test]
    fn the_table_names_each_reachable_service_once_per_token() {
        for api in APIS {
            assert!(
                !api.sdk_token.is_empty()
                    && !api.signing_name.is_empty()
                    && !api.endpoint_prefix.is_empty(),
                "case {:?}: every wire fact of a reachable service is stated",
                api.sdk_token
            );
            match &api.protocol {
                super::Protocol::AwsJson {
                    target_prefix,
                    json_version,
                    ..
                } => assert!(
                    !target_prefix.is_empty()
                        && (*json_version == "1.0" || *json_version == "1.1"),
                    "case {:?}: expected an AWS JSON target prefix and version, got {:?} {:?}",
                    api.sdk_token,
                    target_prefix,
                    json_version
                ),
                // A REST-bound service states no operation of its own in the table above:
                // the model it carries is what every one of its requests is bound by, so
                // the one fact to hold is that the model parsed and binds operations at all.
                super::Protocol::RestJson(rest) => {
                    let bound = rest.model().operation_count();
                    assert!(
                        bound > 0,
                        "case {:?}: expected the embedded model to bind its operations, got \
                         {bound}",
                        api.sdk_token
                    );
                }
                // S3's own protocol binds each operation itself; the binding and its bounds
                // are `super::super::aws_s3`'s, tested there.
                super::Protocol::S3Rest => assert_eq!(
                    api.signing_name, "s3",
                    "case {:?}: expected S3's own signing name on the S3 protocol entry",
                    api.sdk_token
                ),
                // CloudFront's, the same way, in `super::super::aws_cloudfront`.
                super::Protocol::CloudFrontRest => assert_eq!(
                    api.signing_name, "cloudfront",
                    "case {:?}: expected CloudFront's own signing name on its protocol entry",
                    api.sdk_token
                ),
                // Amazon SNS's `awsQuery`, the same way, in `super::super::aws_sns`.
                super::Protocol::SnsQuery => assert_eq!(
                    api.signing_name, "sns",
                    "case {:?}: expected Amazon SNS's own signing name on its protocol entry",
                    api.sdk_token
                ),
            }
        }
        let tokens = reachable_services();
        let mut sorted = tokens.clone();
        sorted.sort_unstable();
        sorted.dedup();
        assert_eq!(
            sorted.len(),
            tokens.len(),
            "case {tokens:?}: expected each service token once, got {sorted:?}"
        );
        // And once under the comparison a client's own token is matched by, so no token a
        // handler builds a client with can answer to two entries.
        for (at, api) in APIS.iter().enumerate() {
            let twice = APIS
                .iter()
                .skip(at + 1)
                .find(|other| super::same_service_token(api.sdk_token, other.sdk_token));
            assert!(
                twice.is_none(),
                "case {:?}: expected one entry per service token, got a second spelling of \
                 it in the same table",
                api.sdk_token
            );
        }
    }

    /// Either spelling of ONE service's SDK id reaches that one service, over a TABLE whose
    /// rows differ along the spelling a client is built with: the id hyphenated, the same id
    /// run together, and the spellings no SDK publishes — another case, a hyphen with no word
    /// beside it, a character no token carries — which reach no service at all.
    #[test]
    fn either_spelling_of_a_service_token_reaches_the_one_service() {
        let reached = [
            ("the token the table spells", "rds-data", "rds-data"),
            ("the same id run together", "rdsdata", "rds-data"),
            ("a two-word id hyphenated", "cloudwatch-logs", "cloudwatchlogs"),
            ("the same id run together", "cloudwatchlogs", "cloudwatchlogs"),
            ("a one-word id", "sqs", "sqs"),
            ("an id carrying a digit", "s3", "s3"),
        ];
        for (case, asked, expected) in reached {
            let found = super::api_for(asked).map(|api| api.sdk_token);
            assert_eq!(
                found,
                Some(expected),
                "case {case:?}: expected {asked:?} to reach the {expected:?} entry, got \
                 {found:?}"
            );
        }
        for (case, asked) in [
            ("a service this emulator does not serve", "kinesis"),
            ("the empty token", ""),
            ("a token of hyphens alone", "--"),
            ("one with a doubled hyphen", "rds--data"),
            ("one opening with a hyphen", "-rds-data"),
            ("one closing with a hyphen", "rdsdata-"),
            ("one carrying a character no token carries", "rds.data"),
            ("one joined with an underscore", "rds_data"),
            ("one written in another case", "RDS-Data"),
            ("a one-word id in another case", "SQS"),
        ] {
            let found = super::api_for(asked).map(|api| api.sdk_token);
            assert!(
                found.is_none(),
                "case {case:?}: expected {asked:?} to reach no service, got {found:?}"
            );
        }
    }

    /// What a guest may name as an operation and as a Region, over a TABLE whose rows
    /// differ along the forms a header or a host must never be given: the rule is that only
    /// a plain name passes, so everything carrying a separator, a space, a control
    /// character or the wrong case is refused.
    #[test]
    fn only_a_plain_name_reaches_a_request_header_or_host() {
        let operations = [
            ("an operation name", "SendMessage", true),
            ("one carrying a digit", "DynamoDB20120810", true),
            ("the empty name", "", false),
            ("a lower-case name", "sendMessage", false),
            ("one carrying a dot", "AmazonSQS.SendMessage", false),
            ("one carrying a newline", "SendMessage\r\nHost: elsewhere", false),
            ("one carrying a space", "Send Message", false),
            ("one carrying a hyphen", "Send-Message", false),
            ("one past the length bound", &"A".repeat(129), false),
        ];
        for (case, operation, expected) in operations {
            let actual = well_formed_operation(operation);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected well_formed_operation({operation:?}) to be \
                 {expected}, got {actual}"
            );
        }
        let regions = [
            ("a Region name", "us-west-2", true),
            ("another Region name", "ap-southeast-4", true),
            ("the empty name", "", false),
            ("one carrying an upper-case letter", "US-WEST-2", false),
            ("one carrying a dot", "us-west-2.evil", false),
            ("one carrying a slash", "us-west-2/path", false),
            ("one carrying a newline", "us-west-2\nX: y", false),
            ("one past the length bound", &"a".repeat(65), false),
        ];
        for (case, region, expected) in regions {
            let actual = well_formed_region(region);
            assert_eq!(
                actual, expected,
                "case {case:?}: expected well_formed_region({region:?}) to be {expected}, \
                 got {actual}"
            );
        }
    }
}
