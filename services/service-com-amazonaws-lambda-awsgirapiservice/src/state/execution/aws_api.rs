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
//! * The guest never sees the INVOCATION's credential. The signature is put on the request by
//!   host code with the credential the invocation was given — the session of the function's
//!   own EXECUTION ROLE ([`super::execution_role_session`]), or the credential the framework
//!   hands this emulator's handlers where no session could be minted — exactly as every
//!   other in-process call between two emulated services is signed
//!   ([`::service_common::in_process_sigv4`]); nothing of it is returned to the guest, and the
//!   guest can neither read it nor ask for a call to be signed as it. So the service that
//!   SERVES the call judges it against the policies in force on that role, which is what
//!   "the function runs as its execution role" means on the wire.
//! * A call made through an SDK client the handler built WITH CREDENTIALS OF ITS OWN is signed
//!   with those instead ([`CallerIdentity::supplied`]), because that is what every AWS SDK
//!   does with the credentials its client holds — a handler that assumed another role calls
//!   the next service as that session, and the serving service judges it on the policies in
//!   force on THAT identity. Nothing is minted here and nothing is taken on trust: a
//!   credential this emulator did not vend verifies nowhere.
//! * A call made through an SDK client the handler built WITH AN ENDPOINT OF ITS OWN is
//!   addressed at that endpoint ([`ClientEndpoint`]), because that is where every AWS SDK
//!   sends the calls of such a client — and for an API whose resource is named ONLY by its
//!   endpoint, the address is part of the request. The destination is still this emulator's
//!   own router; what the endpoint decides is the `Host` and the path the serving service
//!   reads the addressing off.
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
    /// Every service a call may be made to holds one, whichever way its REQUEST is bound: the
    /// EXCEPTION CLASS a refusal of a call to it is raised as is the class named after the
    /// error shape its model declares for the operation called, and the class the SDK this
    /// runtime supplies exports under that name is built from this very list
    /// (`super::aws_sdk_guest`, `super::node_sdk_guest`) — so a service holding no model here
    /// would have every refusal of it raised as the SDK's base error class, which is not the
    /// class a handler catches it by.
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
    /// Amazon SES's `awsQuery`: the same binding, for the EMAIL service a handler that exists
    /// to mail hands its message to, built by the client this crate holds for it
    /// ([`super::aws_ses`]).
    SesQuery,
    /// Amazon SNS's `awsQuery`: the operation travels as an `Action` parameter of a
    /// form-encoded body with the input flattened into the parameters beside it, and the
    /// answer is an XML document — so the request is built, signed and parsed by the Amazon
    /// SNS client this crate holds ([`super::aws_sns`]) rather than by either JSON path
    /// above.
    SnsQuery,
    /// The security token service's `awsQuery`: the same binding, for the service a handler
    /// assumes another role through, built by the client this crate holds for it
    /// ([`super::aws_sts`]).
    StsQuery,
    /// The IDENTITY service's `awsQuery`: the same binding, for the service a handler reads
    /// the role it runs as and the documents in force on that role through — and administers
    /// an identity through — built by the client this crate holds for it
    /// ([`super::aws_iam`]).
    IamQuery,
    /// Amazon RDS's `awsQuery`: the same binding, for the service a handler reads a database
    /// through, puts a new master credential in force on, and drives that database's Amazon S3
    /// export and its role associations through, built by the client this crate holds for it
    /// ([`super::aws_rds`]).
    RdsQuery,
    /// AWS CloudFormation's `awsQuery`: the same binding, for the service a handler reads the
    /// STACK it was deployed by through — its template, its resources, its outputs and its
    /// events — and drives a stack of its own through, built by the client this crate holds for
    /// it ([`super::aws_cloudformation`]).
    CloudFormationQuery,
    /// Amazon EC2's `ec2Query`: a form-encoded `Action` answered with an XML document, for the
    /// service a handler reads a SECURITY GROUP's rules through and revokes or authorizes them
    /// on, built by the client this crate holds for it ([`super::aws_ec2`]).
    Ec2Query,
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
            | Protocol::SesQuery
            | Protocol::SnsQuery
            | Protocol::StsQuery
            | Protocol::IamQuery
            | Protocol::RdsQuery
            | Protocol::CloudFormationQuery
            | Protocol::Ec2Query => "application/json".to_string(),
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
    // `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/
    // com-amazonaws-sts-awssecuritytokenservicev20110615.json`: awsQuery, sigv4Name `sts`,
    // endpointPrefix `sts`, sdkId `STS`. Its operations carry their input as the parameters of
    // a form-encoded `Action` and answer an XML document, so the request is built by
    // [`super::aws_sts`].
    //
    // ASSUMING ANOTHER ROLE is the shape AWS documents for a function that reaches a resource
    // of another account, or that narrows what one of its own calls may do: the handler calls
    // `AssumeRole` as its own execution role and then calls the next service with the session
    // it was vended. Reading back WHICH identity the function runs as is the other call every
    // handler that logs or audits its own calls makes. A handler that cannot reach this
    // service can do neither.
    Api {
        sdk_token: "sts",
        signing_name: "sts",
        endpoint_prefix: "sts",
        model: Some(STS_MODEL),
        protocol: Protocol::StsQuery,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_sts`]); none of them carries bytes.
        answer_model: None,
    },
    // `services/service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/
    // com-amazonaws-iam-awsidentitymanagementv20100508.json`: awsQuery, sigv4Name `iam`,
    // endpointPrefix `iam`, sdkId `IAM`. Its operations carry their input as the parameters of
    // a form-encoded `Action` and answer an XML document, so the request is built by
    // [`super::aws_iam`].
    //
    // WHAT A FUNCTION MAY DO is this service's to answer: the execution role a function runs
    // as is an IAM role, and the documents embedded in and attached to it are what every
    // service judges the function's own calls against. So a handler that reports the identity
    // it runs as, that checks what that identity is allowed to do, or that PROVISIONS an
    // identity for a workload reaches this service — and until it can, a call it is allowed to
    // make and a call its role explicitly denies come back indistinguishable.
    Api {
        sdk_token: "iam",
        signing_name: "iam",
        endpoint_prefix: "iam",
        model: Some(IAM_MODEL),
        protocol: Protocol::IamQuery,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_iam`]); none of them carries bytes.
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
    // `services/service-com-amazonaws-scheduler-awschronosservice/smithy-model/
    // com-amazonaws-scheduler-awschronosservice.json`: restJson1, sigv4Name `scheduler`,
    // endpointPrefix `scheduler`, sdkId `Scheduler`. Its requests are bound per operation out
    // of that same model ([`SCHEDULER`]).
    //
    // A SCHEDULE is on both sides of a function: EventBridge Scheduler is one of the things
    // that invokes a function at all, and a handler that writes, reads and deletes the
    // schedules a workload runs on is the other half — `CreateSchedule` for the work a handler
    // defers to a future instant, `GetSchedule`/`ListSchedules` to report what is pending,
    // `DeleteSchedule` to stand it down, and `CreateScheduleGroup` for the group they live in.
    // A handler that cannot reach this service can neither defer work nor report on it, and
    // the two faults an operator has to tell apart — a missing GROUP
    // (`ResourceNotFoundException`) and a missing GRANT (`AccessDeniedException`) — are this
    // service's own answers, observable from inside a function only once the call is made.
    Api {
        sdk_token: "scheduler",
        signing_name: "scheduler",
        endpoint_prefix: "scheduler",
        model: Some(SCHEDULER_MODEL),
        protocol: Protocol::RestJson(&SCHEDULER),
        answer_model: Some(&SCHEDULER_ANSWERS),
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
        model: Some(OPENSEARCH_MODEL),
        protocol: Protocol::RestJson(&OPENSEARCH),
        answer_model: Some(&OPENSEARCH_ANSWERS),
    },
    // The same service under the token its ENDPOINT PREFIX gives it, which is the other
    // name an SDK client for it is built with (`boto3.client("es")`).
    Api {
        sdk_token: "es",
        signing_name: "es",
        endpoint_prefix: "es",
        model: Some(OPENSEARCH_MODEL),
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
    // `services/service-com-amazonaws-ses-simpleemailservice/smithy-model/
    // com-amazonaws-ses-simpleemailservice.json`: awsQuery, sigv4Name `ses`, endpointPrefix
    // `email`, sdkId `SES`. Its operations carry their input as the parameters of a
    // form-encoded `Action` and answer an XML document rather than a JSON one, so the request
    // is built by [`super::aws_ses`].
    //
    // MAILING is why a very large share of functions exist at all: a scheduled report, a
    // sign-up confirmation or an alert is composed by a handler and handed to this service,
    // which is the surface the AWS SDKs have published for it since 2010 and the one almost
    // every existing handler mails through. A handler that cannot reach it cannot send at all,
    // so no message id is ever issued and nothing downstream of a send happens.
    Api {
        sdk_token: "ses",
        signing_name: "ses",
        endpoint_prefix: "email",
        model: Some(SES_MODEL),
        protocol: Protocol::SesQuery,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_ses`]); none of them carries bytes.
        answer_model: None,
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
        model: Some(XRAY_MODEL),
        protocol: Protocol::RestJson(&XRAY),
        answer_model: Some(&XRAY_ANSWERS),
    },
    // `services/service-com-amazonaws-rds-amazonrdsv19/smithy-model/
    // com-amazonaws-rds-amazonrdsv19.json`: awsQuery, sigv4Name `rds`, endpointPrefix `rds`,
    // sdkId `RDS`. Its operations carry their input as the parameters of a form-encoded
    // `Action` and answer an XML document rather than a JSON one, so the request is built by
    // [`super::aws_rds`].
    //
    // The DATABASE ITSELF is what a handler holding a database credential acts on: the
    // rotation function Secrets Manager invokes puts the new master password in force with
    // `ModifyDBInstance` (or `ModifyDBCluster`) on the instance the secret belongs to, and
    // reads that instance back to find the address and the status it reports — so a handler
    // that cannot reach this service cannot carry out the one step a credential rotation
    // exists for. EXPORTING that database's data to Amazon S3, and associating the IAM role the
    // engine reaches Amazon S3 under, are the other calls a function drives — very often as a
    // CloudFormation custom resource's provider, which starts the export and reports the result
    // back to the stack.
    Api {
        sdk_token: "rds",
        signing_name: "rds",
        endpoint_prefix: "rds",
        model: Some(RDS_MODEL),
        protocol: Protocol::RdsQuery,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_rds`]); none of them carries bytes.
        answer_model: None,
    },
    // `services/service-com-amazonaws-cloudformation-cloudformation/smithy-model/
    // com-amazonaws-cloudformation-cloudformation.json`: awsQuery, sigv4Name `cloudformation`,
    // endpointPrefix `cloudformation`, sdkId `CloudFormation`. Its operations carry their input
    // as the parameters of a form-encoded `Action` and answer an XML document rather than a
    // JSON one, so the request is built by [`super::aws_cloudformation`].
    //
    // THE STACK A FUNCTION WAS DEPLOYED BY is what a handler declared in a template reads: the
    // template it came from (`GetTemplate`), the resources the stack deployed and the physical
    // ids they were given (`ListStackResources`, `DescribeStackResources`), and the stack's own
    // status, parameters and outputs (`DescribeStacks`) — the way a function finds the queue,
    // table or api its own stack created without being told its name. DRIVING a stack —
    // creating, updating or deleting one, and signalling a resource that waits on work the
    // handler did — is the other shape, and the one a deployment handler or a custom resource's
    // provider takes. A handler that cannot reach this service can do neither.
    Api {
        sdk_token: "cloudformation",
        signing_name: "cloudformation",
        endpoint_prefix: "cloudformation",
        model: Some(CLOUDFORMATION_MODEL),
        protocol: Protocol::CloudFormationQuery,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_cloudformation`]); none of them carries bytes.
        answer_model: None,
    },
    // `services/service-com-amazonaws-ec2-amazonec2/smithy-model/
    // com-amazonaws-ec2-amazonec2.json`: ec2Query, sigv4Name `ec2`, endpointPrefix `ec2`,
    // sdkId `EC2`. Its operations carry their input as the parameters of a form-encoded
    // `Action` and answer an XML document, so the request is built by [`super::aws_ec2`].
    //
    // A SECURITY GROUP's own rules are what a handler that polices a network changes, and what
    // the provider of a CloudFormation custom resource over a VPC exists to change: the
    // provider's create revokes the rules the VPC's default group came with and its delete
    // authorizes them again. A handler that cannot reach this service answers such a request
    // with a failure, and the stack that declared the resource rolls back.
    Api {
        sdk_token: "ec2",
        signing_name: "ec2",
        endpoint_prefix: "ec2",
        model: Some(EC2_MODEL),
        protocol: Protocol::Ec2Query,
        // The answer of such a call is read by that client out of the XML the service sent,
        // and the document this seam hands the guest is built from the output members it
        // reported ([`super::aws_ec2`]); none of them carries bytes.
        answer_model: None,
    },
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
    // `services/service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/
    // com-amazonaws-codepipeline-codepipeline-20150709.json`: awsJson1_1, sigv4Name
    // `codepipeline`, endpointPrefix `codepipeline`, sdkId `CodePipeline`, service shape
    // `CodePipeline_20150709`.
    //
    // A function that is a PIPELINE'S OWN INVOKE ACTION has to reach this service:
    // CodePipeline hands the function the job of that action and the function "must call
    // either PutJobSuccessResult or PutJobFailureResult" — the call every such function AWS
    // documents ends with, and the one that publishes the action's output variables for the
    // actions after it — so a handler that cannot reach CodePipeline cannot report the
    // outcome it was invoked to produce, and the action it performs never settles.
    Api {
        sdk_token: "codepipeline",
        signing_name: "codepipeline",
        endpoint_prefix: "codepipeline",
        model: Some(CODEPIPELINE_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "CodePipeline_20150709",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&CODEPIPELINE_ANSWERS),
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
        model: Some(CLOUDFRONT_MODEL),
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
    Api {
        sdk_token: "glue",
        signing_name: "glue",
        endpoint_prefix: "glue",
        model: Some(GLUE_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSGlue",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: None,
    },
    // `services/service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/
    // smithy-model/com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json`:
    // restJson1, sigv4Name `bedrock`, endpointPrefix `bedrock-agent-runtime`, sdkId
    // `Bedrock Agent Runtime`. Its requests are bound per operation out of that same model
    // ([`BEDROCK_AGENT_RUNTIME`]).
    //
    // THE GLUE OF AN AGENT is the shape that reaches this service: a function invoked by a
    // user-facing surface asks an agent alias to answer the turn (`InvokeAgent`) or queries a
    // knowledge base for the passages behind an answer (`Retrieve`), and reports what came
    // back. A handler that cannot reach it can neither invoke an agent nor cite a source — and
    // the two faults an operator has to tell apart there, a missing GRANT
    // (`AccessDeniedException`) and a wrong IDENTIFIER (`ResourceNotFoundException` for a
    // well-formed one, `ValidationException` for a malformed one), are this service's own
    // answers, observable from inside a function only once the call is made.
    Api {
        sdk_token: "bedrock-agent-runtime",
        signing_name: "bedrock",
        endpoint_prefix: "bedrock-agent-runtime",
        model: Some(BEDROCK_AGENT_RUNTIME_MODEL),
        protocol: Protocol::RestJson(&BEDROCK_AGENT_RUNTIME),
        answer_model: Some(&BEDROCK_AGENT_RUNTIME_ANSWERS),
    },
    // `services/service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/
    // smithy-model/com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json`:
    // restJson1, sigv4Name `bedrock`, endpointPrefix `bedrock-agent`, sdkId `Bedrock Agent`.
    // Its requests are bound per operation out of that same model ([`BEDROCK_AGENT`]).
    //
    // THE BUILD-TIME PLANE of an agent, beside the runtime plane above: the agent, its
    // aliases and action groups, and the knowledge base its passages come from are created,
    // read and listed here, so a function that stands an agent up — or that reports the one
    // it was told to use — reaches this service and not the runtime one. The token is the one
    // the AWS SDK for Python builds a client for it with (`boto3.client("bedrock-agent")`),
    // which is its endpoint prefix, and it names a different service from
    // `bedrock-agent-runtime` under the comparison a client's token is matched by
    // ([`same_service_token`]).
    Api {
        sdk_token: "bedrock-agent",
        signing_name: "bedrock",
        endpoint_prefix: "bedrock-agent",
        model: Some(BEDROCK_AGENT_MODEL),
        protocol: Protocol::RestJson(&BEDROCK_AGENT),
        answer_model: Some(&BEDROCK_AGENT_ANSWERS),
    },
    // `services/service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/
    // com-amazonaws-opensearchserverless-opensearchserverless.json`: awsJson1_0, sigv4Name
    // `aoss`, endpointPrefix `aoss`, sdkId `OpenSearchServerless`, service shape
    // `OpenSearchServerless`.
    //
    // A handler that stands up or inspects the VECTOR STORE behind a knowledge base reaches
    // this control plane: the collection the vectors live in, the policies that admit a reader
    // and the collection's own endpoint are read and written here, and the collection's data
    // plane is then reached over HTTP ([`super::guest_http`]).
    Api {
        sdk_token: "opensearchserverless",
        signing_name: "aoss",
        endpoint_prefix: "aoss",
        model: Some(OPENSEARCHSERVERLESS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "OpenSearchServerless",
            json_version: "1.0",
            query_compatible: false,
        },
        answer_model: Some(&OPENSEARCHSERVERLESS_ANSWERS),
    },
    // `services/service-com-amazonaws-s3vectors-s3vectors/smithy-model/
    // com-amazonaws-s3vectors-s3vectors.json`: restJson1, sigv4Name `s3vectors`,
    // endpointPrefix `s3vectors`, sdkId `S3Vectors`. Its requests are bound per operation out
    // of that same model ([`S3VECTORS`]).
    //
    // THE VECTOR STORE a function writes and searches: the embeddings a handler computed for
    // the event it just processed are written into an index (`PutVectors`), the nearest ones to
    // a query vector are read back (`QueryVectors`, `GetVectors`, `ListVectors`), and the bucket
    // and index they live in are created and listed here — so a handler that cannot reach this
    // service can neither store nor retrieve a vector, and the two faults an operator has to
    // tell apart, a missing GRANT (`AccessDeniedException`) and a wrong NAME
    // (`NotFoundException`), are this service's own answers, observable from inside a function
    // only once the call is made.
    Api {
        sdk_token: "s3vectors",
        signing_name: "s3vectors",
        endpoint_prefix: "s3vectors",
        model: Some(S3VECTORS_MODEL),
        protocol: Protocol::RestJson(&S3VECTORS),
        answer_model: Some(&S3VECTORS_ANSWERS),
    },
    // `services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/
    // com-amazonaws-kinesis-kinesis-20131202.json`: awsJson1_1, sigv4Name `kinesis`,
    // endpointPrefix `kinesis`, sdkId `Kinesis`, service shape `Kinesis_20131202`.
    //
    // A DATA STREAM is on both sides of a function: it is one of the event sources that
    // invokes a function at all ([`super::super::event_source_poller`]), and the stream a
    // handler WRITES is how one stage of a streaming pipeline hands its work to the next —
    // the records a handler produces from the batch it was handed, forwarded with
    // `PutRecord`/`PutRecords`, and the shards a handler reads itself with
    // `GetShardIterator`/`GetRecords`. A handler that cannot reach this service can consume a
    // stream but produce nothing onto one, so the effect of an invocation a stream drove is
    // observable nowhere.
    Api {
        sdk_token: "kinesis",
        signing_name: "kinesis",
        endpoint_prefix: "kinesis",
        model: Some(KINESIS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "Kinesis_20131202",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&KINESIS),
    },
    // `services/service-com-amazonaws-cognitoidentityprovider-
    // awscognitoidentityproviderservice/smithy-model/
    // com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json`:
    // awsJson1_1, sigv4Name `cognito-idp`, endpointPrefix `cognito-idp`, sdkId `Cognito
    // Identity Provider`, service shape `AWSCognitoIdentityProviderService`.
    //
    // A USER POOL is on both sides of a function. It is one of the event sources that invokes
    // a function at all — every user-pool Lambda trigger hands its event to one — and a
    // handler that administers the pool it serves is the other half: the user an invitation
    // flow creates, the user a handler reads back or lists, and the password or the
    // enabled/disabled state it sets are all written and read through these Admin operations.
    // A handler that cannot reach this service can neither carry a sign-up flow on nor report
    // anything about the identity it was invoked for, so the effect of such an invocation is
    // observable nowhere.
    //
    // The token is the one the AWS SDK for Python builds a client for this service with
    // (`boto3.client("cognito-idp")`), which is its endpoint prefix.
    Api {
        sdk_token: "cognito-idp",
        signing_name: "cognito-idp",
        endpoint_prefix: "cognito-idp",
        model: Some(COGNITO_IDP_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSCognitoIdentityProviderService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&COGNITO_IDP),
    },
    // The same service under the token its own SDK id gives it, as EventBridge and CloudWatch
    // Logs are above: `Cognito Identity Provider` hyphenated is the name the JavaScript SDK
    // publishes its client package under, and run together it is `cognitoidentityprovider` —
    // one token under the comparison a client's own is matched by ([`same_service_token`]).
    Api {
        sdk_token: "cognito-identity-provider",
        signing_name: "cognito-idp",
        endpoint_prefix: "cognito-idp",
        model: Some(COGNITO_IDP_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AWSCognitoIdentityProviderService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&COGNITO_IDP),
    },
    // `services/service-com-amazonaws-iot-awsiotservice/smithy-model/
    // com-amazonaws-iot-awsiotservice.json`: restJson1, sigv4Name `iot`, endpointPrefix `iot`,
    // sdkId `IoT`. Its requests are bound per operation out of that same model ([`IOT`]).
    //
    // A DEVICE REGISTRY is on both sides of a function. A rule of the message broker invokes a
    // function with the message a device published, and the handler that serves it registers,
    // reads, lists or re-attributes the thing the message came from (`CreateThing`,
    // `DescribeThing`, `ListThings`, `UpdateThing`) and asks where the device's own data plane
    // answers (`DescribeEndpoint`) — so a handler that cannot reach this service can consume a
    // device's message but record nothing about the device, and the effect of such an
    // invocation is observable nowhere.
    Api {
        sdk_token: "iot",
        signing_name: "iot",
        endpoint_prefix: "iot",
        model: Some(IOT_MODEL),
        protocol: Protocol::RestJson(&IOT),
        answer_model: Some(&IOT_ANSWERS),
    },
    // `services/service-com-amazonaws-iotdataplane-iotmoonrakerservice/smithy-model/
    // com-amazonaws-iotdataplane-iotmoonrakerservice.json`: restJson1, sigv4Name `iotdata`,
    // endpointPrefix `data-ats.iot`, sdkId `IoT Data Plane`. Its requests are bound per
    // operation out of that same model ([`IOT_DATA`]).
    //
    // The device DATA plane is the other half of the registry above, and it is the half a
    // function acts through: the endpoint the control plane answers `DescribeEndpoint` with is
    // the one a handler then builds this client for, to publish a message to a topic
    // (`Publish`) or to read and write a thing's reported and desired state
    // (`GetThingShadow`, `UpdateThingShadow`). So a handler that cannot reach this service can
    // learn where the data plane answers and then do nothing with it: the message is never
    // published and the shadow never moves.
    //
    // This is the token a Python client is built with (`boto3.client("iot-data")`); the
    // JavaScript SDK's own spelling of the same id is the entry below.
    Api {
        sdk_token: "iot-data",
        signing_name: "iotdata",
        endpoint_prefix: "data-ats.iot",
        model: Some(IOT_DATA_MODEL),
        protocol: Protocol::RestJson(&IOT_DATA),
        answer_model: Some(&IOT_DATA_ANSWERS),
    },
    // The same service under the token its own SDK id gives it, which is the name the
    // JavaScript SDK publishes that service's client package under
    // (`@aws-sdk/client-iot-data-plane`).
    Api {
        sdk_token: "iot-data-plane",
        signing_name: "iotdata",
        endpoint_prefix: "data-ats.iot",
        model: Some(IOT_DATA_MODEL),
        protocol: Protocol::RestJson(&IOT_DATA),
        answer_model: Some(&IOT_DATA_ANSWERS),
    },
    // `services/service-com-amazonaws-transfer-transferservice/smithy-model/
    // com-amazonaws-transfer-transferservice.json`: awsJson1_1, sigv4Name `transfer`,
    // endpointPrefix `transfer`, sdkId `Transfer`, service shape `TransferService`.
    //
    // A MANAGED WORKFLOW's custom step is one of the places AWS documents a function calling
    // the service that invoked it: the step hands its Lambda target an execution id, a workflow
    // id and a callback token, and "you must include those with their callback as well as
    // providing a status" (`SendWorkflowStepState`) — so a target that cannot reach this service
    // can never report its outcome, and the execution that is waiting on it never finishes.
    // Reading the file the step acts on and the server, user or workflow the event names
    // (`DescribeServer`, `DescribeUser`, `DescribeExecution`, `ListExecutions`) is the rest of
    // what such a handler does.
    Api {
        sdk_token: "transfer",
        signing_name: "transfer",
        endpoint_prefix: "transfer",
        model: Some(TRANSFER_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "TransferService",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&TRANSFER),
    },
    // `services/service-com-amazonaws-batch-awsbatchv20160810/smithy-model/
    // com-amazonaws-batch-awsbatchv20160810.json`: restJson1, sigv4Name `batch`,
    // endpointPrefix `batch`, sdkId `Batch`. Its requests are bound per operation out of that
    // same model ([`BATCH`]).
    //
    // A FUNCTION THAT SUBMITS A JOB is the standard front door of a batch workload: the
    // long-running work a handler cannot do inside one invocation is handed to this service
    // with `SubmitJob`, and the handler answers its caller the job id it was given, which is
    // the id every later read of that work — `DescribeJobs`, `ListJobs`, `CancelJob`,
    // `TerminateJob` — is made against. Registering a definition and reading the queues and
    // compute environments a submission is placed on is the rest of what such a handler does.
    // A handler that cannot reach this service can neither start that work nor report on it.
    Api {
        sdk_token: "batch",
        signing_name: "batch",
        endpoint_prefix: "batch",
        model: Some(BATCH_MODEL),
        protocol: Protocol::RestJson(&BATCH),
        answer_model: Some(&BATCH_ANSWERS),
    },
    // `services/service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/
    // com-amazonaws-ecr-amazonec2containerregistry-v20150921.json`: awsJson1_1, sigv4Name `ecr`,
    // endpointPrefix `api.ecr`, sdkId `ECR`, service shape
    // `AmazonEC2ContainerRegistry_V20150921`.
    //
    // THE IMAGE a container workload runs is this service's: a handler that starts containerised
    // work reads the repository it is to run from and the images in it (`DescribeRepositories`,
    // `DescribeImages`, `BatchGetImage`), creates the repository that work will publish to, and
    // vends the registry credential a pull is made with (`GetAuthorizationToken`) — so a handler
    // that cannot reach this service cannot tell an image that exists from one that does not,
    // which is exactly the fact a container job's outcome turns on.
    Api {
        sdk_token: "ecr",
        signing_name: "ecr",
        endpoint_prefix: "api.ecr",
        model: Some(ECR_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AmazonEC2ContainerRegistry_V20150921",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&ECR_ANSWERS),
    },
    // `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/
    // com-amazonaws-ecs-amazonec2containerservicev20141113.json`: awsJson1_1, sigv4Name `ecs`,
    // endpointPrefix `ecs`, sdkId `ECS`, service shape `AmazonEC2ContainerServiceV20141113`.
    //
    // THE TASK a container workload runs AS is this service's: a handler that runs work too long
    // for one invocation hands it to a task (`RunTask`), reads what that task came to
    // (`DescribeTasks`, `ListTasks`), and manages the services and definitions behind it — and a
    // task is also what another service's placed job runs as, so the handle such a job reports
    // is only resolvable from inside a function once this service is reachable.
    Api {
        sdk_token: "ecs",
        signing_name: "ecs",
        endpoint_prefix: "ecs",
        model: Some(ECS_MODEL),
        protocol: Protocol::AwsJson {
            target_prefix: "AmazonEC2ContainerServiceV20141113",
            json_version: "1.1",
            query_compatible: false,
        },
        answer_model: Some(&ECS_ANSWERS),
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
pub(super) const STS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-sts-awssecuritytokenservicev20110615/smithy-model/\
     com-amazonaws-sts-awssecuritytokenservicev20110615.json"
);
pub(super) const IAM_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-iam-awsidentitymanagementv20100508/smithy-model/\
     com-amazonaws-iam-awsidentitymanagementv20100508.json"
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
pub(super) const SES_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-ses-simpleemailservice/smithy-model/\
     com-amazonaws-ses-simpleemailservice.json"
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
pub(super) const CLOUDFRONT_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-cloudfront-cloudfront2020-05-31/smithy-model/\
     com-amazonaws-cloudfront-cloudfront2020-05-31.json"
);
pub(super) const RDS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-rds-amazonrdsv19/smithy-model/\
     com-amazonaws-rds-amazonrdsv19.json"
);
pub(super) const GLUE_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-glue-awsglue/smithy-model/\
     com-amazonaws-glue-awsglue.json"
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
pub(super) const CODEPIPELINE_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-codepipeline-codepipeline-20150709/smithy-model/\
     com-amazonaws-codepipeline-codepipeline-20150709.json"
);
pub(super) const REKOGNITION_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-rekognition-rekognitionservice/smithy-model/\
     com-amazonaws-rekognition-rekognitionservice.json"
);
pub(super) const BEDROCK_AGENT_RUNTIME_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice/\
     smithy-model/\
     com-amazonaws-bedrockagentruntime-amazonbedrockagentruntimeservice.json"
);
pub(super) const BEDROCK_AGENT_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda/\
     smithy-model/\
     com-amazonaws-bedrockagent-amazonbedrockagentbuildtimelambda.json"
);
pub(super) const OPENSEARCHSERVERLESS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-opensearchserverless-opensearchserverless/smithy-model/\
     com-amazonaws-opensearchserverless-opensearchserverless.json"
);
pub(super) const KINESIS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/\
     com-amazonaws-kinesis-kinesis-20131202.json"
);
pub(super) const COGNITO_IDP_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-cognitoidentityprovider-\
     awscognitoidentityproviderservice/smithy-model/\
     com-amazonaws-cognitoidentityprovider-awscognitoidentityproviderservice.json"
);
pub(super) const IOT_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-iot-awsiotservice/smithy-model/\
     com-amazonaws-iot-awsiotservice.json"
);
pub(super) const IOT_DATA_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-iotdataplane-iotmoonrakerservice/smithy-model/\
     com-amazonaws-iotdataplane-iotmoonrakerservice.json"
);
pub(super) const S3VECTORS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-s3vectors-s3vectors/smithy-model/\
     com-amazonaws-s3vectors-s3vectors.json"
);
pub(super) const TRANSFER_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-transfer-transferservice/smithy-model/\
     com-amazonaws-transfer-transferservice.json"
);
pub(super) const EC2_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-ec2-amazonec2/smithy-model/\
     com-amazonaws-ec2-amazonec2.json"
);
pub(super) const CLOUDFORMATION_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-cloudformation-cloudformation/smithy-model/\
     com-amazonaws-cloudformation-cloudformation.json"
);
pub(super) const SCHEDULER_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-scheduler-awschronosservice/smithy-model/\
     com-amazonaws-scheduler-awschronosservice.json"
);
pub(super) const BATCH_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-batch-awsbatchv20160810/smithy-model/\
     com-amazonaws-batch-awsbatchv20160810.json"
);
pub(super) const ECR_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-ecr-amazonec2containerregistry-v20150921/smithy-model/\
     com-amazonaws-ecr-amazonec2containerregistry-v20150921.json"
);
pub(super) const ECS_MODEL: &str = include_str!(
    "../../../../service-com-amazonaws-ecs-amazonec2containerservicev20141113/smithy-model/\
     com-amazonaws-ecs-amazonec2containerservicev20141113.json"
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
static CODEPIPELINE_ANSWERS: super::blob_members::ServiceModel =
    super::blob_members::ServiceModel {
        source: CODEPIPELINE_MODEL,
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
static BEDROCK_AGENT_RUNTIME_ANSWERS: super::blob_members::ServiceModel =
    super::blob_members::ServiceModel {
        source: BEDROCK_AGENT_RUNTIME_MODEL,
        plan: ::std::sync::OnceLock::new(),
    };
static OPENSEARCHSERVERLESS_ANSWERS: super::blob_members::ServiceModel =
    super::blob_members::ServiceModel {
        source: OPENSEARCHSERVERLESS_MODEL,
        plan: ::std::sync::OnceLock::new(),
    };
static BEDROCK_AGENT_ANSWERS: super::blob_members::ServiceModel =
    super::blob_members::ServiceModel {
        source: BEDROCK_AGENT_MODEL,
        plan: ::std::sync::OnceLock::new(),
    };
static KINESIS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: KINESIS_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static COGNITO_IDP: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: COGNITO_IDP_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static IOT_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: IOT_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static IOT_DATA_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: IOT_DATA_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static TRANSFER: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: TRANSFER_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static S3VECTORS_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: S3VECTORS_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static SCHEDULER_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: SCHEDULER_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static BATCH_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: BATCH_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static ECR_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: ECR_MODEL,
    plan: ::std::sync::OnceLock::new(),
};
static ECS_ANSWERS: super::blob_members::ServiceModel = super::blob_members::ServiceModel {
    source: ECS_MODEL,
    plan: ::std::sync::OnceLock::new(),
};

/// The Bedrock Agent runtime plane's own model, which a call to it is bound by — embedded
/// exactly as [`APIGATEWAY`] is, and for the same reason: the wire facts of each operation are
/// that model's, so this seam states none of them.
static BEDROCK_AGENT_RUNTIME: super::rest_json::RestApi = super::rest_json::RestApi {
    model: BEDROCK_AGENT_RUNTIME_MODEL,
    namespace: "com.amazonaws.bedrockagentruntime",
    parsed: ::std::sync::OnceLock::new(),
};

/// The Bedrock Agent BUILD-TIME plane's own model, which a call to it is bound by — embedded
/// exactly as [`BEDROCK_AGENT_RUNTIME`] is, and for the same reason: the wire facts of each
/// operation are that model's, so this seam states none of them.
static BEDROCK_AGENT: super::rest_json::RestApi = super::rest_json::RestApi {
    model: BEDROCK_AGENT_MODEL,
    namespace: "com.amazonaws.bedrockagent",
    parsed: ::std::sync::OnceLock::new(),
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

/// AWS IoT's own model, which a call to it is bound by — embedded exactly as the models above
/// are and for the same reason: the wire facts of each of its operations are that model's, so
/// this seam states none of them.
static IOT: super::rest_json::RestApi = super::rest_json::RestApi {
    model: IOT_MODEL,
    namespace: "com.amazonaws.iot",
    parsed: ::std::sync::OnceLock::new(),
};

/// AWS IoT's DATA plane's own model, embedded the same way and for the same reason.
static IOT_DATA: super::rest_json::RestApi = super::rest_json::RestApi {
    model: IOT_DATA_MODEL,
    namespace: "com.amazonaws.iotdataplane",
    parsed: ::std::sync::OnceLock::new(),
};

/// The vector store's own model, which a call to it is bound by — embedded exactly as the
/// models above are and for the same reason: the wire facts of each of its operations are that
/// model's, so this seam states none of them.
static S3VECTORS: super::rest_json::RestApi = super::rest_json::RestApi {
    model: S3VECTORS_MODEL,
    namespace: "com.amazonaws.s3vectors",
    parsed: ::std::sync::OnceLock::new(),
};

/// EventBridge Scheduler's own model, which a call to it is bound by — embedded exactly as the
/// models above are and for the same reason: the wire facts of each of its operations are that
/// model's, so this seam states none of them.
static SCHEDULER: super::rest_json::RestApi = super::rest_json::RestApi {
    model: SCHEDULER_MODEL,
    namespace: "com.amazonaws.scheduler",
    parsed: ::std::sync::OnceLock::new(),
};

/// AWS Batch's own model, which a call to it is bound by — embedded exactly as the models above
/// are and for the same reason: the wire facts of each of its operations are that model's, so
/// this seam states none of them.
static BATCH: super::rest_json::RestApi = super::rest_json::RestApi {
    model: BATCH_MODEL,
    namespace: "com.amazonaws.batch",
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
/// It is the session of the function's own execution role, so it is the credential a managed
/// runtime hands the handler it runs: it reaches the request's `Authorization` and
/// `x-amz-security-token` headers, and the three reserved credential variables of the run's
/// own environment, which is where the SDK's default chain inside the handler reads it
/// ([`super::runtime_environment`]). It reaches no response and no log of this emulator's,
/// and the credential the FRAMEWORK hands this emulator's handlers is never one of these:
/// where no session could be minted, a run is given no credential at all.
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

impl CallerIdentity {
    /// The credentials an SDK CLIENT THE GUEST BUILT was given, read from the document that
    /// client hands over with each of its calls — `Ok(None)` for a client built with none,
    /// which signs as the identity the invocation itself was given.
    ///
    /// Every AWS SDK signs a call with the credentials the client was built with: a handler
    /// that was vended a session by the token service and builds a client with it is calling
    /// AS that session, and the service serving the call judges it against the policies in
    /// force on THAT identity. So the values are read here and put on the request's signature
    /// and session-token headers exactly as the invocation's own are; nothing is minted and
    /// nothing is trusted — a credential this emulator did not vend verifies nowhere, which is
    /// the answer the serving service gives it.
    ///
    /// `Err` for a document that is not a credential at all, so a call the guest asked to be
    /// signed as something this seam could not read is not quietly signed as the invocation's
    /// own identity.
    pub fn supplied(document: &str) -> Result<Option<Self>, String> {
        if document.trim().is_empty() {
            return Ok(None);
        }
        let read: ::serde_json::Value = ::serde_json::from_str(document)
            .map_err(|error| format!("the client's credentials are not a document: {error}"))?;
        let text = |name: &str| {
            read.get(name)
                .and_then(::serde_json::Value::as_str)
                .unwrap_or_default()
                .to_string()
        };
        let access_key_id = text("accessKeyId");
        let secret_access_key = text("secretAccessKey");
        if access_key_id.is_empty() || secret_access_key.is_empty() {
            return Err(
                "the client was built with an access key id and a secret access key together, \
                 or with neither"
                    .to_string(),
            );
        }
        Ok(Some(Self {
            access_key_id,
            secret_access_key,
            session_token: text("sessionToken"),
        }))
    }

    /// The SESSION TOKEN this credential travels with, or `None` for a long-term credential,
    /// which has none: the header a temporary credential is served with is put on a request
    /// only where there is a token to put in it.
    #[must_use]
    pub fn token(&self) -> Option<&str> {
        (!self.session_token.is_empty()).then_some(self.session_token.as_str())
    }
}

/// The longest endpoint URL this seam reads. A value past it is no address of this emulator's,
/// so it is not read at all and the call is addressed at the called service's own endpoint.
const MAX_ENDPOINT_CHARS: usize = 2048;

/// The ENDPOINT an SDK CLIENT the guest built was configured with, as a request is addressed
/// at it: the authority it names, and the base path in front of the operation's own binding.
///
/// WHY a client's endpoint has to travel at all: every AWS SDK sends the calls of a client
/// built with an explicit endpoint to THAT endpoint, and for an API whose resource is named
/// only by its endpoint the address IS part of the request. API Gateway's `@connections`
/// surface is one — its operations take no api and no stage member
/// (`smithy_query(query="operation", operation="PostToConnection", model_path="services/service-com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi/smithy-model/com-amazonaws-apigatewaymanagementapi-apigatewaymanagementapi.json")`),
/// and AWS publishes them on the owning api's own invoke endpoint, the URL a backend's client
/// is configured with — so WHICH api's stage a call addresses, and therefore the `execute-api`
/// resource it is authorized on, is readable nowhere else in the request. A call dispatched at
/// the service's own regional host instead addresses no stage at all.
///
/// Only an absolute `http`/`https` URL naming an authority and a plain path is read: anything
/// else leaves the call addressed at the called service's own regional endpoint, exactly as a
/// client built with no endpoint is. Nothing here opens a socket — the destination is still
/// the in-process callback, as the module docs state — so what an endpoint decides is the
/// `Host` field and the path, and never where the request goes. For the same reason only the
/// AUTHORITY and the PATH travel: the SCHEME selects a transport, and this dispatch has one
/// transport, so an endpoint written either way reaches the same router. What a value may NOT
/// carry is read strictly, so only a plain base path is ever put in front of the operation's
/// own: no userinfo on the authority, no query, and no empty or dot segment — written out or
/// percent-encoded — in the path.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClientEndpoint {
    /// The authority the request is addressed at: the host, with the port where the URL names
    /// one.
    authority: String,
    /// The path in front of the operation's own binding — empty, or `/` followed by segments.
    base_path: String,
}

impl ClientEndpoint {
    /// What `url` addresses, or `None` for a value this seam does not read as an endpoint (see
    /// the type's own documentation).
    #[must_use]
    pub fn configured(url: &str) -> Option<Self> {
        let url = url.trim();
        if url.is_empty() || url.len() > MAX_ENDPOINT_CHARS {
            return None;
        }
        let parsed: ::http::Uri = url.parse().ok()?;
        if !matches!(parsed.scheme_str(), Some("http" | "https")) {
            return None;
        }
        let authority = parsed.authority()?.to_string();
        // An authority carrying USERINFO is not an endpoint's: it would travel as part of the
        // `Host` field, which names a host and a port and nothing else.
        if authority.contains('@') {
            return None;
        }
        // A query is no part of an endpoint, and a base path may not walk: a value carrying
        // either is not read as one, so nothing a guest supplies can address a path of its own
        // devising in front of the operation's.
        if parsed.query().is_some() {
            return None;
        }
        let base_path = parsed.path().trim_end_matches('/').to_string();
        if base_path.split('/').skip(1).any(|segment| !plain(segment)) {
            return None;
        }
        Some(Self {
            authority,
            base_path,
        })
    }
}

/// Whether one path segment is a plain one: a segment a base path may carry, rather than an
/// empty one or a dot segment that walks.
///
/// A dot segment counts however it is written, because a path is read after its escapes are
/// decoded (RFC 3986 §2.3: a percent-encoded unreserved character "should be decoded to its
/// corresponding unreserved character"), so `%2e%2e` is the same segment as `..`.
fn plain(segment: &str) -> bool {
    let decoded = segment.to_ascii_lowercase().replace("%2e", ".");
    !decoded.is_empty() && decoded != "." && decoded != ".."
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
    /// The data planes this emulator has OPENED and the addresses they answer at, for a
    /// STREAM SOCKET the handler opens itself ([`super::guest_socket`]). `None` where this
    /// emulator vends no such records, and then no address answers at all.
    dialable: Option<::service_common::dialable_data_planes::DialablePlanes>,
    /// The VPC the function this invocation runs is ATTACHED to
    /// ([`super::vpc_network`]): the private address its traffic leaves from and the addresses
    /// it reaches there. `None` for a function attached to no VPC, which then has no route to
    /// a VPC address at all.
    attachment: Option<super::vpc_network::VpcAttachment>,
    /// The base URL this emulator is reached on, which a URL the handler's SDK PRESIGNS has
    /// to lead back to ([`super::presigned_url`]). `None` where the run was given no
    /// endpoint, in which case no URL is signed at all rather than one signed for an address
    /// nothing answers.
    endpoint: Option<::smithy_server_runtime::EndpointUrl>,
    /// The ENDPOINT the SDK CLIENT a call was made through was configured with, where it was
    /// built with one ([`ClientEndpoint`]) — `None` for a client built with none, whose calls
    /// are addressed at the called service's own regional endpoint.
    client_endpoint: Option<ClientEndpoint>,
    /// The state of the EXECUTION ENVIRONMENT this run happens in, which the runtime API is
    /// served from ([`super::runtime_api`]): the function the environment is running, and the
    /// extensions registered in it. `None` for a seam built for no invocation of a function,
    /// where that interface has no environment to answer from.
    sandbox: Option<super::runtime_api::Sandbox>,
    /// What decides whether a call of this seam LEAVES the network the run happens in — the
    /// policies of the VPC endpoints a machine's traffic travels through, say
    /// ([`::service_common::vpc_endpoint_calls`]). `None` for a run with no such control in
    /// force on it, whose calls are decided by the called service alone.
    egress: Option<::std::sync::Arc<dyn ::service_common::vpc_endpoint_calls::CallEgress>>,
    /// WHERE the SQL a Spark program of this run submits is planned and run
    /// ([`::service_common::spark_runtime::SessionSql`]): the engine the service that submitted
    /// the program holds, over the catalog the statement's names resolve in and the data behind
    /// them. `None` for a run that was given no engine — every run that is no Spark program's —
    /// whose statements are answered by nothing rather than by an engine holding no catalog.
    session_sql: Option<::std::sync::Arc<dyn ::service_common::spark_runtime::SessionSql>>,
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
            dialable: None,
            attachment: None,
            endpoint: None,
            client_endpoint: None,
            sandbox: None,
            egress: None,
            session_sql: None,
        })
    }

    /// The same access, with `engine` planning and running the SQL a Spark program of this run
    /// submits — the engine of the service that submitted it, over its own catalog.
    ///
    /// `None` leaves a statement answered by nothing, which is the position every run that
    /// submits no Spark program is in.
    #[must_use]
    pub fn running_statements_on(
        mut self,
        engine: Option<::std::sync::Arc<dyn ::service_common::spark_runtime::SessionSql>>,
    ) -> Self {
        self.session_sql = engine;
        self
    }

    /// The engine the SQL a Spark program of this run submits is planned and run by, where it
    /// was given one.
    #[must_use]
    pub fn session_sql(
        &self,
    ) -> Option<&::std::sync::Arc<dyn ::service_common::spark_runtime::SessionSql>> {
        self.session_sql.as_ref()
    }

    /// The same access, with `egress` deciding whether a call of it leaves the network the run
    /// happens in: a call that network does not carry is answered the refusal the called
    /// service answers it with, and nothing is dispatched for it.
    ///
    /// `None` leaves every call to the called service's own decision, which is the position a
    /// run outside any such network is in.
    #[must_use]
    pub fn leaving_through(
        mut self,
        egress: Option<::std::sync::Arc<dyn ::service_common::vpc_endpoint_calls::CallEgress>>,
    ) -> Self {
        self.egress = egress;
        self
    }

    /// The same access, inside the execution environment `sandbox` holds the state of — the
    /// invocation's own, which is what the runtime API this run addresses answers from
    /// ([`super::runtime_api`]).
    ///
    /// A seam with no sandbox is one built for no invocation of a function: the Extensions API
    /// then has no environment to register an extension in, and says so.
    #[must_use]
    pub fn in_sandbox(mut self, sandbox: super::runtime_api::Sandbox) -> Self {
        self.sandbox = Some(sandbox);
        self
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

    /// The credential every call of this access is signed with, where one was minted for the
    /// run: the session of the function's execution role.
    ///
    /// It is read for the run's own ENVIRONMENT ([`super::runtime_environment`]), because the
    /// credential a managed runtime signs a handler's calls with is the one it exports to the
    /// handler. `None` where no session was minted, in which case the run is given no
    /// credential variable at all rather than one this emulator did not mint for the role.
    #[must_use]
    pub fn identity(&self) -> Option<&CallerIdentity> {
        self.identity.as_ref()
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

    /// The same access, opening a connection to a data plane this emulator OPENED against
    /// `dialable` — the records the service that opened that plane wrote
    /// ([`::service_common::dialable_data_planes`]).
    ///
    /// `None` leaves every address answering nowhere, so a connection a handler opens to one
    /// is answered the way a host with nothing listening answers it: the position every
    /// address was in before a service opened a plane.
    #[must_use]
    pub fn dialing_opened_planes(
        mut self,
        dialable: Option<::service_common::dialable_data_planes::DialablePlanes>,
    ) -> Self {
        self.dialable = dialable;
        self
    }

    /// The records a STREAM SOCKET's destination is resolved against, for the seam that holds
    /// a running handler's connections ([`super::guest_socket`]).
    #[must_use]
    pub fn dialing(&self) -> Option<::service_common::dialable_data_planes::DialablePlanes> {
        self.dialable.clone()
    }

    /// The same access, with the function ATTACHED to the VPC `attachment` describes — the
    /// interfaces Lambda stood up for its `VpcConfig` and the address space they reach
    /// ([`super::vpc_network`]).
    ///
    /// `None` leaves the run attached to no VPC, which is a function whose configuration named
    /// no subnet: its datagrams then reach only the in-process X-Ray daemon, and an address of
    /// a VPC is one it has no route to.
    #[must_use]
    pub fn attached_in_vpc(
        mut self,
        attachment: Option<super::vpc_network::VpcAttachment>,
    ) -> Self {
        self.attachment = attachment;
        self
    }

    /// The VPC attachment a datagram's route and a connection's source address are read off,
    /// for that same seam.
    #[must_use]
    pub fn inside_vpc(&self) -> Option<super::vpc_network::VpcAttachment> {
        self.attachment.clone()
    }

    /// The async runtime a call of this seam is driven on, for a caller that has to block its
    /// own thread on one: the handle the invocation's seam was built with, which is the
    /// runtime the emulator's own planes and services run on.
    #[must_use]
    pub fn runtime(&self) -> ::tokio::runtime::Handle {
        self.runtime.clone()
    }

    /// The same access, signing a URL the handler asks the SDK to PRESIGN for `endpoint` —
    /// the base URL this emulator is reached on, which is the one address a URL handed to a
    /// later caller can be fetched from ([`super::presigned_url`]).
    ///
    /// `None` leaves such a URL unsigned, which is reported as a URL that was not signed:
    /// this emulator answers on its own endpoint alone, so a URL addressed anywhere else is
    /// one nothing can be fetched from.
    #[must_use]
    pub fn addressed_at(mut self, endpoint: ::smithy_server_runtime::EndpointUrl) -> Self {
        self.endpoint = Some(endpoint);
        self
    }

    /// The same access, addressing every call it makes at `endpoint` — the endpoint the SDK
    /// CLIENT the call was made through was configured with ([`ClientEndpoint`]).
    ///
    /// `None` addresses the call at the called service's own regional endpoint, which is where
    /// an SDK sends the calls of a client built with no endpoint of its own.
    #[must_use]
    pub fn addressed_by_client(mut self, endpoint: Option<ClientEndpoint>) -> Self {
        self.client_endpoint = endpoint;
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
        // WHICH operation of that service the name names, under the spelling its own model
        // gives it ([`super::sdk_model::operation_named`]): the name arrives from an SDK
        // CLIENT, which derived it from the method the handler called, so a name whose
        // operation carries an acronym arrives cased as that SDK's naming rule cased it and
        // not as the service spells it. The model's spelling is what the request carries, and
        // a name that service's model declares no operation of is one it does not have.
        let named = super::sdk_model::operation_named(api.sdk_token, operation);
        let operation = match &named {
            super::sdk_model::Named::Operation(modelled) => modelled.as_str(),
            super::sdk_model::Named::Unbound => return Err(Refusal::UnknownOperation),
            super::sdk_model::Named::Unheld => operation,
        };
        if body.len() > MAX_REQUEST_BYTES {
            return Err(Refusal::RequestTooLarge);
        }
        let region = if well_formed_region(region) {
            region
        } else {
            &self.region
        };
        // Whether the NETWORK the run happens in carries this call at all: a control the
        // account wrote on that network — the policy of a VPC endpoint the traffic travels
        // through — is decided before anything is dispatched, and a call it does not carry is
        // answered the refusal the called service answers it with. The decision is the one
        // reading of such a document this workspace holds, handed what the request itself says.
        if let Some(refused) = self.egress.as_ref().and_then(|egress| {
            egress.refusal(api.signing_name, operation, region, body)
        }) {
            return Ok(refusal_answered(&refused));
        }
        // Every AWS SDK client generates a value for an input member the service's model marks
        // as an idempotency token when the caller left it out, and sends it with the request
        // (`super::idempotency_token`) — so this seam, which is the lower half of every SDK
        // this runtime supplies, fills one here, before the request is built and signed.
        let filled = super::idempotency_token::filled(api.sdk_token, operation, body);
        let body = filled.as_deref().unwrap_or(body);
        // And every client puts a TIMESTAMP a caller handed it in the form the called
        // protocol carries one in, whatever form the caller had it in
        // (`super::sdk_timestamps`) — the other half of what the client, rather than the
        // service, does to an input document.
        let in_form = super::sdk_timestamps::in_protocol_form(api.sdk_token, operation, body);
        let body = in_form.as_deref().unwrap_or(body);
        match ::tokio::time::timeout(within, self.dispatched(api, region, operation, body)).await
        {
            // WHERE the answer carries an INSTANT: a JSON protocol carries a timestamp as the
            // seconds since the epoch, and the SDK the guest calls hands the caller its own
            // language's instant for one, so the host says which of the answer's values those
            // are ([`super::blob_members`]). Read HERE, after the dispatch, so every protocol
            // this seam reaches is served by one reading: a service whose answer is built from
            // the output members its own client reported ([`super::sdk_call`]) states its
            // instants in the same form a JSON body carries them in.
            Ok(Ok(answer)) => Ok(Answer {
                instants: answer_instants(api.sdk_token, operation, &answer).await,
                ..answer
            }),
            Ok(refused) => refused,
            Err(_elapsed) => Err(Refusal::TimedOut),
        }
    }
    /// Mint the AUTHENTICATION TOKEN a handler presents to a database endpoint that serves
    /// IAM database authentication, for `user` on `host`:`port`.
    ///
    /// Not a call: the SDKs build this token in the client
    /// ([`super::db_auth_token`]), so nothing is dispatched and no service answers. It is
    /// here beside [`call`](Self::call) because the credential it is signed with is the one
    /// this seam holds — the identity a call of this seam is signed as, so the token's
    /// credential scope names the very key the run was given — and because the guest reaches
    /// both the same way.
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
                self.identity.as_ref(),
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
                    Caller {
                        identity: self.identity.as_ref(),
                        trace_header: self.trace_header.as_deref(),
                        addressed_at: self.client_endpoint.as_ref(),
                    },
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
                    self.trace_header.as_deref(),
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
                    self.trace_header.as_deref(),
                )
                .await
            }
            Protocol::SesQuery => {
                super::aws_ses::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                    self.trace_header.as_deref(),
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
                    self.trace_header.as_deref(),
                )
                .await
            }
            Protocol::StsQuery => {
                super::aws_sts::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                    self.trace_header.as_deref(),
                )
                .await
            }
            // IAM is reached at ONE endpoint whatever Region the client the guest built
            // names, so the Region is not threaded into this call
            // ([`super::aws_iam::SIGNING_REGION`]).
            Protocol::IamQuery => {
                super::aws_iam::call(
                    &self.callback,
                    self.identity.as_ref(),
                    operation,
                    body,
                    self.trace_header.as_deref(),
                )
                .await
            }
            Protocol::RdsQuery => {
                super::aws_rds::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                    self.trace_header.as_deref(),
                )
                .await
            }
            Protocol::CloudFormationQuery => {
                super::aws_cloudformation::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                    self.trace_header.as_deref(),
                )
                .await
            }
            Protocol::Ec2Query => {
                super::aws_ec2::call(
                    &self.callback,
                    self.identity.as_ref(),
                    region,
                    operation,
                    body,
                    self.trace_header.as_deref(),
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
                sandbox: self.sandbox.as_ref(),
            },
        ))
    }

    /// The URL one call carries ITS OWN signature in, as an SDK presigns one.
    ///
    /// Nothing is sent. The URL addresses the base URL this emulator is reached on — the one
    /// address a later caller can fetch it from — and carries a signature over exactly that
    /// request, made with the SAME identity the handler's ordinary calls are made as: the
    /// session of the function's own execution role
    /// ([`super::execution_role_session`]). The guest holds that credential no more than it
    /// holds any other: what it gets back is the key id, the session token and a signature
    /// over the one request the URL names — never the secret they were signed with — good
    /// for `expires_in` and for nothing else, which is what a presigned URL is, and the
    /// service that serves that later request judges it as that identity, exactly as it
    /// judges a call the handler makes itself.
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
        // The operation under the spelling the called service's own model gives it, for the
        // reason a dispatched call carries that spelling ([`Self::call_within`]): the name
        // arrives from an SDK client's own naming rule. A service whose model this emulator
        // holds none of, or a name its model declares no operation of, is left as it arrived
        // for the client below to report on.
        let named = super::sdk_model::operation_named(api.sdk_token, operation);
        let operation = named.modelled().unwrap_or(operation);
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
        let Some(endpoint) = self.endpoint.as_ref() else {
            return Err(Refusal::NotMade(
                "this invocation was given no endpoint for a signed URL to lead back to, so \
                 none was signed"
                    .to_string(),
            ));
        };
        let region = if well_formed_region(region) {
            region.to_string()
        } else {
            self.region.clone()
        };
        let signing = super::presigned_url::Signing {
            endpoint,
            identity: self.identity.as_ref(),
            callback: &self.callback,
            region: &region,
        };
        super::aws_s3::presigned(
            &signing,
            api.signing_name,
            operation,
            body,
            expires_in.min(MAX_PRESIGNED_LIFETIME),
        )
    }
}

/// The longest a presigned URL this seam hands back is good for, which is the window a query
/// signature may carry at all ([`super::presigned_url::MAX_LIFETIME`]).
const MAX_PRESIGNED_LIFETIME: Duration = super::presigned_url::MAX_LIFETIME;

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
    /// The members the ERROR SHAPE of a refusal declares, which the SDK the guest calls hands
    /// the caller beside the refusal's code and message: the name each value is reported under
    /// against the key [`Self::body`] carries it in ([`super::sdk_model::ClientExports`]).
    ///
    /// An error shape is a structure like any other — `TransactionCanceledException` carries the
    /// per-action cancellation reasons, `ConditionalCheckFailedException` the item that failed
    /// the condition — and a client of the service reads those members off the refusal it was
    /// handed, so WHICH members one carries is the called service's own model's to say. Empty
    /// for every answer that is not a refusal, and for a refusal whose shape that model declares
    /// nothing of.
    pub error_members: ::std::collections::BTreeMap<String, String>,
    /// The output members of [`Self::body`] that carry BYTES rather than text, as the
    /// base64 the answer's document carries them as — an object's own payload, say, or the
    /// member a REST-bound answer binds to the whole body whose shape is a `@streaming` blob.
    /// The SDK the guest calls hands each of them over as the stream its own client answers
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
    /// WHERE in [`Self::body`] the answer carries an INSTANT: one path per value the called
    /// operation's own model types as a `timestamp` ([`super::blob_members`]), in the same form
    /// [`Self::blobs`] takes.
    ///
    /// A JSON protocol carries a timestamp as the seconds since the epoch, and the SDK the
    /// guest calls hands the caller its language's own instant for one — a `Date` in JavaScript
    /// — so a handler reads an instant where it reads one on a managed runtime. Empty for an
    /// answer carrying none, and for every refusal.
    pub instants: Vec<::serde_json::Value>,
    /// The member a REST-bound answer binds to the WHOLE body whose shape is a blob the model
    /// does NOT mark `@streaming`, as the base64 of the body's own bytes.
    ///
    /// Such a body is one an SDK COLLECTS rather than hands over unread, so the form the guest
    /// reads it in is its own SDK's: the JavaScript SDK gives the caller the collected bytes
    /// themselves ([`super::node_sdk_guest`]), while the Python SDK wraps the payload of every
    /// output whose payload shape is a blob in a streaming body ([`super::aws_sdk_guest`]).
    /// Empty for an answer binding none — which is every answer of an AWS JSON service.
    pub payload_blobs: Vec<String>,
}

/// The answer a call the NETWORK did not carry is reported with: the called service's own
/// status, code and sentence, in the two members the SDK the guest calls reads a refusal's code
/// and message from.
///
/// It is an ANSWER rather than a [`Refusal`]: the call was refused by a policy in force on it,
/// which is the service's own HTTP answer to the request — so the guest's SDK raises the class
/// that code names, exactly as it does for a refusal the service itself worded. Nothing was
/// dispatched, so no request id is reported and no member of any output shape is.
fn refusal_answered(refused: &::service_common::vpc_endpoint_calls::RefusedCall) -> Answer {
    let document = ::serde_json::json!({
        "__type": refused.code,
        "message": refused.message,
    });
    Answer {
        status: refused.status,
        body: document.to_string(),
        error_type: Some(refused.code.clone()),
        query_error_code: None,
        request_id: None,
        error_members: ::std::collections::BTreeMap::new(),
        streams: Vec::new(),
        blobs: Vec::new(),
        instants: Vec::new(),
        payload_blobs: Vec::new(),
    }
}

/// What the CALLER of one dispatch brings to it, beside the operation and the input document:
/// the credential the request is signed with, the tracing context it carries, and the endpoint
/// the SDK client it was made through was configured with.
///
/// One value rather than three parameters, because all three are facts about the CLIENT and
/// the invocation rather than about the request, and every one of them is read in one place.
struct Caller<'a> {
    /// The credential the request is signed with, or `None` to sign with the credential the
    /// framework hands this emulator's handlers ([`signed`]).
    identity: Option<&'a CallerIdentity>,
    /// The tracing header the request carries, or `None` for a run with no trace context.
    trace_header: Option<&'a str>,
    /// The endpoint the request is addressed at, or `None` to address it at the called
    /// service's own regional endpoint ([`ClientEndpoint`]).
    addressed_at: Option<&'a ClientEndpoint>,
}

/// Build, sign and dispatch one request, and read the answer back.
async fn dispatch(
    callback: &EmbeddedEmulator,
    api: &Api,
    region: &str,
    operation: &str,
    body: &str,
    caller: Caller<'_>,
) -> Result<Answer, Refusal> {
    sent(callback, api, region, operation, body, caller)
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
    caller: Caller<'_>,
) -> Result<Answer, Sent> {
    let Caller {
        identity,
        trace_header,
        addressed_at,
    } = caller;
    // WHERE the request is addressed: the endpoint the SDK client was configured with, where
    // it was built with one, and otherwise the called service's own regional endpoint
    // ([`ClientEndpoint`]). The base path goes in front of the path the operation's own
    // binding names, which is how every SDK addresses a client's endpoint.
    let host = addressed_at.map_or_else(|| api.host(region), |at| at.authority.clone());
    let base_path = addressed_at.map_or("", |at| at.base_path.as_str());
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
                .uri(format!("https://{host}{base_path}/"))
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
                .uri(format!("https://{host}{base_path}{}", bound.target));
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
        Protocol::S3Rest
        | Protocol::CloudFrontRest
        | Protocol::SesQuery
        | Protocol::SnsQuery
        | Protocol::StsQuery
        | Protocol::IamQuery
        | Protocol::RdsQuery
        | Protocol::CloudFormationQuery
        | Protocol::Ec2Query => {
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
    // text those bytes happened to decode to: the bytes travel as base64, and WHETHER the SDK
    // the guest calls hands them over as a stream or as the bytes it collected is what that
    // blob's own shape says ([`super::rest_json::PayloadBlob`]).
    let (body, streams, payload_blobs) = match (&api.protocol, (200..300).contains(&status)) {
        (Protocol::RestJson(rest), true) => {
            match super::rest_json::carried_payload(rest, operation, &body, payload) {
                Some((document, carried)) if carried.streaming => {
                    (document, vec![carried.member], Vec::new())
                }
                Some((document, carried)) => (document, Vec::new(), vec![carried.member]),
                None => (body, Vec::new(), Vec::new()),
            }
        }
        _ => (body, Vec::new(), Vec::new()),
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
    // WHERE the answer carries an INSTANT is the called service's own model's to say as well,
    // and it is read in ONE place for every protocol this seam reaches
    // ([`AwsAccess::call_within`]), because a service whose answer this seam builds from the
    // output members its own client reported hands its instants over in the same form a JSON
    // body carries them in.
    let instants = Vec::new();
    // WHICH members a REFUSAL carries is the called service's own model's to say too: an error
    // shape is a structure, and a client hands its members to the caller beside the code and the
    // message. Read on a blocking thread for the same reason the answer's blob paths are.
    let error_members = match (200..300).contains(&status) {
        true => ::std::collections::BTreeMap::new(),
        false => {
            let token = api.sdk_token;
            let named = error_type.clone();
            let body = body.clone();
            ::tokio::task::spawn_blocking(move || refused_members(token, named.as_deref(), &body))
                .await
                .unwrap_or_default()
        }
    };
    Ok(Answer {
        status,
        body,
        error_type,
        query_error_code,
        request_id,
        error_members,
        streams,
        blobs,
        instants,
        payload_blobs,
    })
}

/// WHERE the answer of a call to the service `token` carries an INSTANT: one path per value
/// `answered` holds that the called operation's own model types as a `timestamp`
/// ([`super::blob_members`]).
///
/// A JSON protocol carries a timestamp as the seconds since the epoch, and every AWS SDK hands
/// the caller its own language's instant for one, because the model says which members are
/// timestamps. The answer is read from the same held reading of that model every other question
/// about the called service is answered from ([`super::sdk_model::client_exports`]).
///
/// Nothing is reported for a REFUSAL: an answer that is not a success carries no output member
/// at all. The reading runs on a BLOCKING THREAD of the runtime for the reason the blob paths
/// are read on one: reading a service's model for the first time in this process parses a
/// document of several megabytes, and the thread this is called on is the RUN's own, whose
/// faulted pages are what the invocation's memory ceiling is measured from.
async fn answer_instants(
    token: &'static str,
    operation: &str,
    answered: &Answer,
) -> Vec<::serde_json::Value> {
    if !(200..300).contains(&answered.status) {
        return Vec::new();
    }
    let operation = operation.to_string();
    let body = answered.body.clone();
    ::tokio::task::spawn_blocking(move || {
        let document =
            ::serde_json::from_str::<::serde_json::Value>(&body).unwrap_or(::serde_json::Value::Null);
        super::sdk_model::client_exports(token)
            .answer_instants
            .answer_paths(&operation, &document)
    })
    .await
    .unwrap_or_default()
}

/// The members the error shape a refusal named declares, as the SDK the guest calls reports
/// them ([`Answer::error_members`]).
///
/// WHICH shape the refusal is, is read the way a client reads it: the shape the answer named in
/// its own header, else the one its document names in `__type`. A refusal naming no shape the
/// service `token`'s model declares members of carries none, which is the position every
/// AWS-common refusal is in — no service's model declares one.
fn refused_members(
    token: &'static str,
    error_type: Option<&str>,
    body: &str,
) -> ::std::collections::BTreeMap<String, String> {
    let document = ::serde_json::from_str::<::serde_json::Value>(body).ok();
    let named = document
        .as_ref()
        .and_then(|document| document.get("__type"))
        .and_then(::serde_json::Value::as_str);
    let Some(shape) = [error_type, named]
        .into_iter()
        .flatten()
        .filter_map(|named| named.split('#').next_back())
        .filter_map(|named| named.split(':').next())
        .map(str::trim)
        .find(|named| !named.is_empty())
    else {
        return ::std::collections::BTreeMap::new();
    };
    super::sdk_model::client_exports(token)
        .error_members
        .get(shape)
        .cloned()
        .unwrap_or_default()
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
    // The header a TEMPORARY credential is served with, put on only where there is a token to
    // put in it: a long-term credential a handler built its client with carries none.
    if let Some(token) = identity.token() {
        request.headers_mut().insert(
            SESSION_TOKEN_HEADER,
            ::http::HeaderValue::from_str(token).map_err(|error| error.to_string())?,
        );
    }
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
        api_for, held_model, reachable_services, well_formed_operation, well_formed_region,
        ClientEndpoint, APIS,
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
            (
                "the user pool service, by the token the Python SDK names it",
                "cognito-idp",
                Some("cognito-idp"),
            ),
            (
                "the same service by the JavaScript package's token for it",
                "cognito-identity-provider",
                Some("cognito-idp"),
            ),
            (
                "the same service with the words of that id run together",
                "cognitoidentityprovider",
                Some("cognito-idp"),
            ),
            ("a single-word id", "s3", Some("s3")),
            ("a service no entry answers to", "polly", None),
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
    /// The rows are the services this table reaches under more than one token, plus a token no
    /// entry answers to at all.
    #[test]
    fn both_tokens_of_a_service_answer_with_its_own_model() {
        // (case, two tokens of one service, whether a model is held for it)
        let rows: &[(&str, &str, &str, bool)] = &[
            ("CloudWatch Logs", "logs", "cloudwatchlogs", true),
            ("Step Functions", "stepfunctions", "sfn", true),
            ("EventBridge", "events", "eventbridge", true),
            ("Secrets Manager", "secretsmanager", "secrets-manager", true),
            (
                "Cognito Identity Provider",
                "cognito-identity-provider",
                "cognitoidentityprovider",
                true,
            ),
            ("IoT Data Plane", "iot-data", "iotdata", true),
            ("OpenSearch", "opensearch", "es", true),
            ("no service at all", "polly", "not-a-service", false),
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
                // Amazon SES's `awsQuery`, the same way, in `super::super::aws_ses`.
                super::Protocol::SesQuery => assert_eq!(
                    api.signing_name, "ses",
                    "case {:?}: expected the email service's own signing name on its protocol \
                     entry",
                    api.sdk_token
                ),
                // Amazon SNS's `awsQuery`, the same way, in `super::super::aws_sns`.
                super::Protocol::SnsQuery => assert_eq!(
                    api.signing_name, "sns",
                    "case {:?}: expected Amazon SNS's own signing name on its protocol entry",
                    api.sdk_token
                ),
                // The token service's `awsQuery`, the same way, in `super::super::aws_sts`.
                super::Protocol::StsQuery => assert_eq!(
                    api.signing_name, "sts",
                    "case {:?}: expected the token service's own signing name on its protocol \
                     entry",
                    api.sdk_token
                ),
                // The identity service's `awsQuery`, the same way, in
                // `super::super::aws_iam`.
                super::Protocol::IamQuery => assert_eq!(
                    api.signing_name, "iam",
                    "case {:?}: expected the identity service's own signing name on its \
                     protocol entry",
                    api.sdk_token
                ),
                // Amazon RDS's `awsQuery`, the same way, in `super::super::aws_rds`.
                super::Protocol::RdsQuery => assert_eq!(
                    api.signing_name, "rds",
                    "case {:?}: expected Amazon RDS's own signing name on its protocol entry",
                    api.sdk_token
                ),
                // AWS CloudFormation's `awsQuery`, the same way, in
                // `super::super::aws_cloudformation`.
                super::Protocol::CloudFormationQuery => assert_eq!(
                    api.signing_name, "cloudformation",
                    "case {:?}: expected AWS CloudFormation's own signing name on its protocol \
                     entry",
                    api.sdk_token
                ),
                // Amazon EC2's `ec2Query`, the same way, in `super::super::aws_ec2`.
                super::Protocol::Ec2Query => assert_eq!(
                    api.signing_name, "ec2",
                    "case {:?}: expected Amazon EC2's own signing name on its protocol entry",
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
            // The device registry and its data plane are two services whose tokens share a
            // leading word, so neither spelling of one reaches the other.
            ("a two-word id hyphenated", "iot-data", "iot-data"),
            ("the same id run together", "iotdata", "iot-data"),
            ("the id it shares a word with", "iot", "iot"),
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
            ("a service this emulator does not serve", "polly"),
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

    /// RULE: a client's endpoint decides the authority a call is addressed at and the base
    /// path in front of the operation's own binding, and a value that is no absolute
    /// `http`/`https` URL naming an authority and a plain path is not read as an endpoint at
    /// all — such a call is addressed at the called service's own regional endpoint.
    ///
    /// The rows span what the rule generalises over: whether the URL names a port, how its
    /// path is written (none, a stage, a stage with a trailing slash, several segments), the
    /// scheme, and the forms a base path may not take.
    #[test]
    fn a_clients_endpoint_decides_the_authority_and_the_base_path() {
        // Longer than this seam reads, for the row that pins the bound.
        let past_the_bound = format!(
            "https://127.0.0.1:45273/{}",
            "p".repeat(super::MAX_ENDPOINT_CHARS)
        );
        // (case, the configured value, the authority and base path it addresses)
        type Row<'a> = (&'a str, &'a str, Option<(&'a str, &'a str)>);
        let rows: &[Row<'_>] = &[
            (
                "the published endpoint of an api's stage",
                "https://abc123.execute-api.us-east-1.amazonaws.com/prod",
                Some(("abc123.execute-api.us-east-1.amazonaws.com", "/prod")),
            ),
            (
                "the same with a trailing slash",
                "https://abc123.execute-api.us-east-1.amazonaws.com/prod/",
                Some(("abc123.execute-api.us-east-1.amazonaws.com", "/prod")),
            ),
            (
                "an address carrying a port, as this emulator's own planes do",
                "https://127.0.0.1:45273/prod",
                Some(("127.0.0.1:45273", "/prod")),
            ),
            (
                "an endpoint with no path at all",
                "https://127.0.0.1:45273",
                Some(("127.0.0.1:45273", "")),
            ),
            (
                "an endpoint whose path is a bare slash",
                "http://localhost:8080/",
                Some(("localhost:8080", "")),
            ),
            (
                "a base path of several segments, as a custom domain's mapping adds",
                "https://api.example.test/v1/socket",
                Some(("api.example.test", "/v1/socket")),
            ),
            ("the empty value, which is a client built with none", "", None),
            ("a value naming no authority", "/prod/@connections", None),
            (
                "a scheme no AWS endpoint is addressed by",
                "ws://127.0.0.1:45273/prod",
                None,
            ),
            ("a path that walks", "https://127.0.0.1:45273/prod/../other", None),
            (
                "a path that walks percent-encoded",
                "https://127.0.0.1:45273/prod/%2E%2E/other",
                None,
            ),
            (
                "an authority carrying userinfo",
                "https://user@127.0.0.1:45273/prod",
                None,
            ),
            (
                "a path carrying an empty segment",
                "https://127.0.0.1:45273/prod//x",
                None,
            ),
            (
                "a value carrying a query",
                "https://127.0.0.1:45273/prod?x=1",
                None,
            ),
            ("a value longer than this seam reads", &past_the_bound, None),
        ];
        for (case, configured, expected) in rows {
            let addressed = ClientEndpoint::configured(configured);
            let read = addressed
                .as_ref()
                .map(|at| (at.authority.as_str(), at.base_path.as_str()));
            assert_eq!(
                read, *expected,
                "case {case:?}: expected {expected:?}, got {read:?}"
            );
        }
    }
}
