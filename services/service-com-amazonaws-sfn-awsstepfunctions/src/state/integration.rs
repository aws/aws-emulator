// SPDX-License-Identifier: CC0-1.0

//! A `Task` state's service integration: which call its `Resource` names, and how
//! this emulator MAKES that call.
//!
//! A `Task` state "represents a single unit of work performed by a state machine",
//! and for a service integration that unit of work is one API call to another
//! service: the state's effective input — the document its `Parameters` payload
//! template builds — IS that operation's request, and the operation's response
//! document IS the state's result. That is the rule this module serves, and it is
//! served for real: the call goes to the other service's own handler in this same
//! emulator, over the in-process callback every emulated service reaches another by
//! ([`smithy_server_runtime::OperationContext::callback`]), signed with the
//! emulator's own credentials because the emulator verifies every request
//! signature. So what a `dynamodb:putItem` task writes is readable afterwards
//! through `GetItem`, and what the task reports is what DynamoDB answered — nothing
//! here synthesises a result.
//!
//! ## Which integrations are made
//!
//! A `Resource` of the form `arn:aws:states:::<namespace>:<action>` (an optimized
//! integration) or `arn:aws:states:::aws-sdk:<sdk name>:<action>` (an AWS SDK
//! integration) names a service — by its ARN namespace in the first family, by the
//! name the AWS SDKs give it in the second ([`Api::sdk_token`]) — and one of that
//! service's API operations by its name in lowerCamel. The call is made when that
//! service has an
//! [`APIS`] entry, which carries how a request to it is bound to HTTP ([`Wire`]).
//! For an `awsJson1_0` or `awsJson1_1` service the request BODY is the operation's
//! input document, which is exactly what a payload template builds, so one entry
//! covers every operation that service models: the operation name is derived from the
//! action, and nothing is written per action. A `restJson1` service spreads an
//! operation's input over the path, the query, the headers and its body — one payload
//! member where the operation binds one, else the document of every member the rest of
//! the request does not carry ([`Body`]) — so its entry carries those bindings per
//! operation ([`Rest`]) and a call is made only for an operation whose bindings are
//! stated. An `awsQuery` service takes the whole
//! request as a FORM — the action and the service's API version beside one field per
//! LEAF of the input ([`form_body`]), a member that is a collection spread over the
//! numbered fields that protocol names its parts by ([`FORM_MAPS`]) — and answers an
//! XML document wrapping the operation's own result ([`query_result`]).
//!
//! The parameters of a `Task` state's integration are written in the integration's
//! own convention, PascalCase, whatever the invoked API spells its own members in,
//! so the document the state built is put on the wire under the spelling that
//! service's model uses ([`MemberCase`]) — and a member the integration documents as
//! a document while the operation types it as a string is carried as that document's
//! JSON text ([`JSON_TEXT_MEMBERS`]). The answer is reported back to the state in
//! that same convention ([`task_result`]). Both re-spell the TOP level only, because
//! below it a document carries a caller's DATA beside the operation's members: a
//! structure member nested inside a response therefore still reads as the invoked
//! model spells it, which is a divergence this emulator carries rather than one it
//! hides — re-spelling further needs the invoked model's own member names, which
//! this crate cannot read.
//!
//! What has no entry is not faked. Each of these is reported by [`read`] and ends the
//! execution in the service's own failure state with the ASL-documented
//! [`super::asl::RUNTIME_ERROR`] and a cause saying what was not run:
//!
//! * a service with no entry — one whose wire facts are simply not listed here yet,
//!   since each entry's facts are that service's own model's;
//! * an operation of a REST service whose own bindings are not carried, since where
//!   each of its members goes on the wire is that operation's own fact, and a member
//!   of a FORM request that is a structure or a list no [`FORM_MAPS`] entry names the
//!   fields of, whose names are the invoked shape's own ([`form_body`]);
//! * a run-a-job (`.sync`) pattern over an integration that is not in the
//!   [`RUN_A_JOB`] catalogue below, because the job it starts is reported and
//!   stopped through calls only that integration names;
//! * a bare resource ARN of any other service, which names a resource this emulator
//!   does not invoke directly — only a Lambda FUNCTION's ARN is work in itself (see
//!   below).
//!
//! ## The direct Lambda invocation
//!
//! A `Resource` that is the ARN of a LAMBDA FUNCTION names no integration token: it
//! names the function, and the unit of work is one synchronous invocation of it. The
//! state's effective input is then not the operation's input but the function's EVENT,
//! so the call is the invocation that carries it — the function the `Resource` named as
//! `FunctionName`, that document as the `Payload` ([`Call::operation_input`]) — and it
//! is subject to the same access request the optimized `lambda:invoke` integration is,
//! `lambda:InvokeFunction` on that function's ARN, because it is the same call on the
//! same resource. What the task reports is the function's OWN response payload
//! ([`reported_result`]): the members around it on the wire — the status, the version
//! that ran — are facts of the API call rather than of the function's output, and this
//! form reports the output alone. An invocation Lambda SERVED whose function ERRORED is
//! not an output at all: the unit of work failed, so the task fails with the error's own
//! class out of the document the run produced and that document as its cause
//! ([`function_error`]).
//!
//! ## The run-a-job (`.sync`) pattern
//!
//! A `Resource` ending in `.sync` (or `.sync:2`) asks for the run-a-job pattern:
//! the integration's own call STARTS a job, and the task is not finished until that
//! job is. [`RUN_A_JOB`] carries, per integration, the three calls that pattern is
//! made of — the one that starts the job, the one that reports it and the one that
//! stops it — plus where the job's identifier, its status and the reason it failed
//! sit in those documents, which members of the STARTING request those two calls
//! carry beside the identifier for an integration whose job the identifier does not
//! name on its own, and the shape that stopping call types the identifier as.
//! The walk makes the starting call, records what came back
//! as the task's submission and PARKS there ([`super::asl::ParkedJob`]); the job is
//! then polled through its own reporting call until it reaches a terminal status,
//! which is what resolves the task ([`super::run_a_job`]). Nothing is synthesised:
//! the identifier, the status and the failure are the invoked service's own answers.
//!
//! ## The callback (`.waitForTaskToken`) pattern
//!
//! A `Resource` ending in `.waitForTaskToken` asks for the callback pattern: the
//! integration's own call is made exactly as a request-response one is, and what it
//! carries includes a `taskToken` the state's parameters read out of the context
//! object — so the task is finished not by that call's answer but by a later
//! `SendTaskSuccess` or `SendTaskFailure` quoting that token. [`Call::callback`] is
//! what marks such a call, and the walk records what the call answered as the task's
//! submission and SUSPENDS at it ([`super::asl::ScheduledActivity`]), which is the
//! execution waiting for an answer exactly as it waits for an activity worker's. The
//! token itself is minted before the call, because the call is what delivers it.
//!
//! The token answers the task from that instant on, which is BEFORE the call has come
//! back: the work it is handed to may report the task's outcome from inside the very
//! call that delivered it, which is how the Lambda callback pattern is written. The
//! walk therefore HANDS the token over before it makes the call ([`Tasks::hand_over`])
//! and reads back what answered it when the call returns
//! ([`Tasks::settle_handover`]) — an answer that already arrived is the state's own
//! outcome, so the walk finishes the task here rather than suspending at a token
//! nothing will quote again.
//!
//! An ACTIVITY `Resource` is not one of these: its work belongs to a worker that
//! polls this emulator, so the walk schedules a task and suspends at it rather than
//! asking for a call ([`read`] is not asked anything first by
//! [`super::asl::Interpreter`]). [`read`] still answers for one, so it is total over
//! every `Resource` form a definition can carry.
//!
//! ## The dataset a `Map` state reads
//!
//! One more call is made from here, and it is not a `Task` state's: a `Map` state's
//! `ItemReader` names an S3 dataset, and the items it iterates are read out of it
//! ([`super::item_reader`]). That read goes over this same seam — the same in-process
//! callback, the same signature, the same Region — through [`dataset_call`], whose two
//! bindings are S3's own. S3 is deliberately NOT one of [`APIS`]: a `Task` state's
//! `Resource` naming an S3 operation is still work no integration call is made for,
//! because an integration's result is the operation's own response members and this
//! emulator does not read those out of `restXml`. What the dataset read wants is the
//! answer's BYTES, which that protocol does carry.
//!
//! ## What a task may do, and what its call answers
//!
//! The call is made AS the state machine's execution role, so the policies in force
//! on that role decide whether it is made at all: the `(action, resource)` pair the
//! call is subject to is formed from the call itself ([`access_request`], over
//! [`TARGET_RESOURCES`]) and decided by [`super::task_authorization`]. A call that
//! role may not make is refused BEFORE the request goes anywhere, so nothing is
//! started for it, and the task fails with the invoked service's own access-denied
//! refusal.
//!
//! That holds for EVERY call the integration makes, not only the one the walk asks
//! for: the call that REPORTS a run-a-job pattern's job is made as the same role, and
//! one it may not make leaves the task's outcome unknowable — the job was already
//! submitted, so the task fails with ASL's own [`TASK_FAILED_ERROR`] carrying that
//! refusal, rather than with a status nothing reported. Its access request is over the
//! resource the job BELONGS to, which the request that started the job named
//! ([`JobTarget`]), because the reporting call's own request names the job instead.
//!
//! What a request-response call answers is reported to the state as the literal SDK
//! response an OPTIMIZED integration returns: the operation's own members under the
//! integration's convention, beside the SDK's response envelope
//! ([`with_sdk_envelope`]), whose status, headers and request id are that answer's
//! own. The AWS SDK family reports the operation's members alone, and the `.sync`
//! pattern reports the job's own report instead.
//!
//! ## Where the call goes
//!
//! Nowhere outside this process. The destination is never a value a caller
//! supplied: the `Resource`'s service token selects one of the [`APIS`] entries —
//! services this emulator hosts itself — and the request is handed to the
//! in-process callback, which is the emulator's own router rather than a socket.
//! No client is built, no address is resolved and no host is dialled; the host
//! header exists because the signature is computed over it. The Region is the
//! execution's own, from its state machine's ARN, so a definition cannot steer the
//! call anywhere either.
//!
//! ## What bounds it
//!
//! One interpretation runs on ONE blocking thread and HOLDS that thread while a
//! call it asked for is in flight, so the resource to bound is not only the work but
//! the threads a chain of calls can pin at once. Four bounds do it:
//!
//! * the walk charges every state transition to
//!   [`super::asl::MAX_STATE_TRANSITIONS`], which bounds how many calls ONE
//!   execution makes;
//! * [`MAX_CALL_DURATION`] bounds ONE call, server-side, on the thread that is
//!   waiting: a call that has not answered inside it fails the task instead of
//!   pinning the thread, which is also what clamps a target that holds its answer on
//!   purpose (`GetActivityTask` is a long poll);
//! * [`super::asl::MAX_CALL_WAIT_DURATION`] bounds what ONE run may spend waiting for
//!   its calls together, by clamping what each further call may wait for to what is
//!   left of it, and [`super::asl::MAX_INTERPRET_WORK`] bounds the work the
//!   interpretation itself does, which the walk consults BEFORE each call as well as
//!   at each transition, so a run that has spent it makes no further call;
//! * [`MAX_CALL_CHAIN_DEPTH`] bounds how deep a chain of in-process calls may still
//!   make one, which is what bounds the PRODUCT: a chain that re-enters this service
//!   — a task that starts another execution, or one that publishes an event a rule
//!   delivers to a state machine — holds one blocking thread per level, so the
//!   threads one inbound request can hold is that depth rather than the framework's
//!   own [`smithy_server_runtime::CALLBACK_RECURSION_LIMIT`], which is a cycle
//!   backstop set far above any legitimate call graph.
//!
//! A call refused by either ceiling is reported as the failure it is: nothing was
//! run for the task and nothing is faked. A call that TIMED OUT is the one case
//! where the task's answer and the world can differ: the wait is abandoned, but the
//! call it was waiting for is already with the other service, so a task that reports
//! `States.Timeout` may still have had its effect — which is exactly what a timed-out
//! call means on the real service too, and why its `Catch` path must not assume
//! otherwise.

use std::time::{Duration, SystemTime};

use serde_json::{Map, Value};

/// How deep a chain of in-process calls may be and still make one.
///
/// Emulator bound, NOT an AWS limit. Every interpretation holds one blocking thread
/// for its whole duration, including while a call it made is in flight, so a chain
/// that re-enters this service occupies one thread per level: a task that starts
/// another execution, and equally one that publishes an event whose rule delivers to
/// a state machine. The framework's own
/// [`smithy_server_runtime::CALLBACK_RECURSION_LIMIT`] bounds such a chain at a
/// depth chosen as a cycle backstop, far above any legitimate call graph, which is
/// the wrong order of magnitude for a resource that is HELD rather than merely
/// spent. This is the depth at which a task's call is still made, so the blocking
/// threads one inbound request can pin is this many. A task deeper than it fails
/// with the ASL-documented [`super::asl::RUNTIME_ERROR`]: nothing was run for it,
/// and nothing is faked.
///
/// Three levels is above every chain this emulator serves — a published event
/// starting an execution whose task writes to another service is two — and far below
/// the blocking pool a request's chain must not exhaust.
pub const MAX_CALL_CHAIN_DEPTH: u8 = 3;

/// The longest the walk waits for ONE integration call to answer.
///
/// Emulator bound: the call is served in THIS process, so a call that only reads or
/// writes another service's store answers in microseconds, while one that RUNS work —
/// a job script, a function's own code — takes as long as that work takes on whatever
/// the host has left to give it. A call that has not answered inside this
/// fails the task with ASL's own timeout error instead of holding the blocking thread
/// the walk runs on, which also clamps a target that holds its answer on purpose:
/// `GetActivityTask` is a long poll, and this is what stops one from pinning a thread
/// for its hold. So the figure is chosen to bound a HELD THREAD and not to impose a
/// latency expectation on the work a call performs: it is wide enough that a call whose
/// target is running caller-supplied work is not pre-empted by it, and it stays well
/// inside the per-run ceiling below.
///
/// It bounds ONE call and nothing about the interpretation around it, because a call is
/// not interpretation: a `lambda:invoke` task's call RUNS the caller's own function,
/// which Lambda bounds by that function's `Timeout` and not by anything of this
/// service's. What bounds the thread a run holds across every call it makes is
/// [`super::asl::MAX_CALL_WAIT_DURATION`], which this stays well inside.
pub const MAX_CALL_DURATION: Duration = Duration::from_secs(60);

/// The ARN namespace every service-integration `Resource` is written under: the
/// `Resource` names Step Functions' own integration, not the target resource.
const STATES_NAMESPACE: &str = "states";

/// The ARN namespace of AWS Lambda, whose functions a `Task` state invokes
/// (`awsService.arnNamespace`).
const LAMBDA_NAMESPACE: &str = "lambda";

/// The Lambda operation that invokes a function, as that service's model spells it.
const LAMBDA_INVOKE_OPERATION: &str = "Invoke";

/// The `<namespace>:<action>` the optimized family names that same invocation by,
/// which is the key its [`TARGET_RESOURCES`] entry is carried under: a `Resource`
/// naming a function directly makes the same call on the same resource, so it is
/// subject to that entry's access request too.
const LAMBDA_INVOKE_INTEGRATION: &str = "lambda:invoke";

/// The ARN namespace of Amazon SQS, whose queues a `Task` state sends to.
const SQS_NAMESPACE: &str = "sqs";

/// The ARN namespace of Amazon SNS, whose topics a `Task` state publishes to
/// (`awsService.arnNamespace`).
const SNS_NAMESPACE: &str = "sns";

/// The `<namespace>:<action>` the optimized family names the publish to a topic by,
/// which is the key its [`TARGET_RESOURCES`] and [`FORM_MAPS`] entries are carried
/// under.
const SNS_PUBLISH_INTEGRATION: &str = "sns:publish";

/// The `Resource` segment that selects the AWS SDK integration family
/// (`arn:aws:states:::aws-sdk:<service>:<action>`).
const AWS_SDK_SEGMENT: &str = "aws-sdk";

/// The kind a `Resource` names Step Functions' own HTTP Task by, and that integration's
/// one action (`arn:aws:states:::http:invoke`): the work is an HTTP request to an
/// endpoint the state's parameters name rather than a call on another service's
/// operation ([`super::http_task`]).
const HTTP_NAMESPACE: &str = "http";
/// See [`HTTP_NAMESPACE`].
const HTTP_INVOKE_ACTION: &str = "invoke";

/// The wire error code an emulated service answers for an operation nobody has
/// implemented yet (`crate::scaffolded`). It is this emulator's own gap rather than
/// the service refusing the call, so a task that meets it reports that nothing ran.
const NOT_IMPLEMENTED_CODE: &str = "NotImplemented";

/// The ASL predefined error name for a task whose own work failed
/// ("States.TaskFailed"), which is what a refusal carrying no error code at all
/// leaves.
pub const TASK_FAILED_ERROR: &str = "States.TaskFailed";

/// The most bytes the `resourceType` and `resource` a `Task` state's events report
/// carry: both target the modelled `Name` shape, whose `length` is 1..80
/// (`smithy_query(query="shape",shape="TaskScheduledEventDetails")`).
const MAX_NAME_BYTES: usize = 80;

/// The header an AWS JSON service names its error shape in, beside the `__type`
/// member of the body.
const ERROR_TYPE_HEADER: &str = "x-amzn-errortype";

/// The header an AWS JSON client names the operation it calls in
/// (`<target prefix>.<OperationName>`).
const TARGET_HEADER: &str = "x-amz-target";

/// The header a client of a QUERY-COMPATIBLE service asks for the legacy query code of
/// a refusal with, and the header such a service answers it in.
///
/// `aws.protocols#awsQueryCompatible` keeps a service's old query vocabulary alive for
/// clients that once spoke it: the error class is named by its shape in the body and by
/// its legacy code in [`QUERY_ERROR_HEADER`], whose value is `<code>;<fault>`. Step
/// Functions reaches such a service through a client of that kind, which is why the
/// code a failed task's cause reports is the legacy one.
const QUERY_MODE_HEADER: &str = "x-amzn-query-mode";
const QUERY_ERROR_HEADER: &str = "x-amzn-query-error";

/// How one service's API is reached on the wire: the facts of that service's own
/// model that a request to it is built from.
///
/// Every field is read from the target service's Smithy model with
/// `smithy_query(query="service_info")`, so this table states no wire detail of its
/// own.
#[derive(Debug)]
pub struct Api {
    /// The ARN namespace a `Resource` names this service by (`awsService.arnNamespace`).
    pub namespace: &'static str,
    /// The prefix this service publishes its own IAM ACTIONS under, where that is not
    /// its ARN namespace — the string its authorization refusals spell an action with
    /// ([`Call::action`]). `None` where the two are the same, which is the usual case.
    pub action_prefix: Option<&'static str>,
    /// Its SigV4 signing name (`sigv4Name`) — also the credential scope that routes
    /// the call to this service inside this emulator.
    pub signing_name: &'static str,
    /// The leading label of its endpoint host (`awsService.endpointPrefix`).
    pub endpoint_prefix: &'static str,
    /// How a request to it is bound to HTTP, which its model's protocol decides
    /// ([`Wire`]).
    pub wire: Wire,
    /// Its SDK id (`awsService.sdkId`), which names the service half of a task
    /// error ([`Failed::error`]) and — lower-cased — the token an AWS SDK
    /// integration's `Resource` names this service by ([`Api::sdk_token`]).
    pub sdk_id: &'static str,
    /// The name Step Functions writes for this service in the error name of a failed
    /// `Task` (`SQS.QueueDoesNotExistException`): the service's SDK id for all but
    /// Step Functions itself, whose integration failures name it by its full service
    /// name rather than by the `SFN` id its SDK carries.
    pub error_prefix: &'static str,
    /// The name this service's SDK CLIENT carries, which is the name an exception it
    /// raises reports the service under ([`sdk_exception_cause`]). It follows the
    /// model's own service shape for most services and the SDK's own class name where
    /// the two differ, so it is stated per entry rather than derived.
    pub sdk_client: &'static str,
    /// The BASE exception class of that client: the one its deserializer falls back to
    /// for an error code no error shape of this service's model declares, which is the
    /// error name a `Task` state then reports ([`Api::exception_of`]).
    pub sdk_exception: &'static str,
    /// How its model spells the members of an operation's input
    /// ([`MemberCase`]), which is what decides the spelling the document a `Task`
    /// state built goes on the wire under.
    pub member_case: MemberCase,
    /// Whether its model carries the `aws.protocols#awsQueryCompatible` modifier, so
    /// its errors have a LEGACY QUERY code beside their shape — the code a client that
    /// once spoke the query protocol still matches on, and the one such a service's own
    /// exceptions report. A client asks for it with [`QUERY_MODE_HEADER`], which is why
    /// the call made for a `Task` state asks for it too.
    pub query_compatible: bool,
}

impl Api {
    /// The host one of this service's regional endpoints answers on.
    fn host(&self, region: &str) -> String {
        format!("{}.{region}.amazonaws.com", self.endpoint_prefix)
    }

    /// The media type this service's protocol names for a request body.
    fn content_type(&self) -> String {
        match self.wire {
            Wire::Json { version, .. } => format!("application/x-amz-json-{version}"),
            // `restJson1` carries a request body as plain JSON; a payload member that
            // is a blob is the bytes themselves, which is what the one REST operation
            // bound here sends.
            Wire::Rest(_) => REST_CONTENT_TYPE.to_string(),
            // `awsQuery` carries the whole request as form fields.
            Wire::Query { .. } => FORM_CONTENT_TYPE.to_string(),
        }
    }

    /// The REST binding of `operation`, for a service whose requests are bound that
    /// way; `None` for an AWS JSON or `awsQuery` service, whose every operation takes
    /// the same request shape, and for an operation of a REST service no binding is
    /// stated for.
    fn rest(&self, operation: &str) -> Option<&'static Rest> {
        match self.wire {
            Wire::Json { .. } | Wire::Query { .. } => None,
            Wire::Rest(bound) => bound.iter().find(|rest| rest.operation == operation),
        }
    }

    /// The token an AWS SDK integration's `Resource`
    /// (`arn:aws:states:::aws-sdk:<token>:<action>`) names this service by: the
    /// name the AWS SDKs give the service, which is its SDK id lower-cased with
    /// its spaces dropped (`SFN` -> `sfn`, `EventBridge` -> `eventbridge`).
    ///
    /// That is a different token from the ARN namespace the OPTIMIZED family names
    /// the same service by (`states:startExecution` beside
    /// `aws-sdk:sfn:startExecution`), so each family is looked up under its own.
    ///
    /// Public for the same reason [`aws_sdk_call`] is: Amazon EventBridge Scheduler's
    /// universal target names a service by this same token, and holds a schedule's
    /// target to it rather than restating which of a service's names is which.
    #[must_use]
    pub fn sdk_token(&self) -> String {
        self.sdk_id
            .chars()
            .filter(|character| !character.is_whitespace())
            .flat_map(char::to_lowercase)
            .collect()
    }

    /// The exception class this service's SDK raises for the error shape `shape` an
    /// answer named, which is the error half of a failed task's error name.
    ///
    /// A shape of the service's own model is deserialized into that shape's own class,
    /// whose name is the shape name with the `Exception` suffix the SDKs give it — a
    /// shape already spelled with it keeps the one it has. Two kinds of answer name no
    /// shape of that model, so no client has a class for either and the base exception
    /// is what carries it: a shape of the request-handling framework that fronts the
    /// service ([`FRAMEWORK_NAMESPACES`]), and one of the errors EVERY service answers
    /// ([`COMMON_WIRE_CODES`]).
    fn exception_of(&self, shape: &str) -> String {
        let namespace = shape.rsplit_once('#').map(|(namespace, _)| namespace);
        let named = bare_name(shape);
        if namespace.is_some_and(|namespace| FRAMEWORK_NAMESPACES.contains(&namespace))
            || COMMON_WIRE_CODES.contains(&named)
        {
            return self.sdk_exception.to_string();
        }
        match named.ends_with(EXCEPTION_SUFFIX) {
            true => named.to_string(),
            false => format!("{named}{EXCEPTION_SUFFIX}"),
        }
    }

    /// The error name a `Task` state reports for a refusal this service answered under
    /// the error shape `shape`: this service as Step Functions names it in a task
    /// error, and the SDK exception that shape is deserialized into.
    pub fn task_error(&self, shape: &str) -> String {
        format!("{}.{}", self.error_prefix, self.exception_of(shape))
    }

    /// The error name a `Task` state reports when this service refuses a call with an
    /// error none of its own shapes declares: this service as Step Functions names it
    /// in a task error, and its base SDK exception.
    pub fn base_error(&self) -> String {
        format!("{}.{}", self.error_prefix, self.sdk_exception)
    }
}

/// The suffix the AWS SDKs give an error shape's own exception class.
const EXCEPTION_SUFFIX: &str = "Exception";

/// The namespaces of the request-handling framework that fronts an AWS service, which a
/// parameter-level refusal names its shape under.
///
/// A shape in one of them belongs to no service's model: it is raised before the
/// operation is reached (a required parameter the request did not carry, a value the
/// request framework rejects), so no SDK carries a class for it and a client
/// deserializes it into the service's base exception. The legacy code such an answer
/// also carries is what a query-compatible client matches on
/// ([`Api::query_compatible`]).
const FRAMEWORK_NAMESPACES: &[&str] = &[
    "com.amazon.coral.service",
    "com.amazon.coral.validate",
    "smithy.framework",
];

/// The wire error codes AWS answers for EVERY service — its common errors, the ones
/// the request framework raises rather than any one API.
///
/// None of them is an error shape of a service's model, so no SDK has a class for one:
/// a client deserializes it into the service's base exception ([`Api::sdk_exception`]),
/// which is the error name Step Functions then reports for the task. They are the codes
/// AWS documents as a service's common errors, which no `smithy_query` on a service
/// model carries because no shape of that model declares them.
///
/// Every one of them is spelled WITHOUT the `Exception` suffix a modelled error shape
/// carries, which is what tells the two apart: a code spelled with the suffix is a
/// shape's own name (`ConditionalCheckFailedException`, `ResourceNotFoundException`)
/// and names that shape's class, so no such code belongs here.
const COMMON_WIRE_CODES: &[&str] = &[
    "IncompleteSignature",
    "InternalFailure",
    "InvalidAction",
    "InvalidClientTokenId",
    "InvalidParameterCombination",
    "InvalidParameterValue",
    "InvalidQueryParameter",
    "MalformedQueryString",
    "MissingAction",
    "MissingAuthenticationToken",
    "MissingParameter",
    "NotAuthorized",
    "OptInRequired",
    "RequestExpired",
    "ServiceUnavailable",
    "Throttling",
    "ValidationError",
];

/// How a request to one service is bound to HTTP, which is what its model's protocol
/// decides.
///
/// A `Task` state's integration parameters are the invoked operation's INPUT, and the
/// protocol says where that input goes on the wire: an AWS JSON protocol carries the
/// whole of it as the request body, while a REST protocol spreads it over the path,
/// the query, the headers and one payload member — so the binding is read per
/// operation for such a service ([`Rest`]).
#[derive(Debug)]
pub enum Wire {
    /// `awsJson1_0` or `awsJson1_1`: the request body IS the operation's input
    /// document, and the operation is named in a header.
    Json {
        /// The `X-Amz-Target` prefix a client names an operation of this service
        /// under: the service shape's own name, as the model spells it.
        target_prefix: &'static str,
        /// The protocol version its model declares (`awsJson1_0` -> `1.0`), which
        /// names the media type of the request body.
        version: &'static str,
    },
    /// `restJson1`: each operation binds its input members to a method, a URI, the
    /// query, headers and one payload, so a call is made only for an operation whose
    /// bindings are stated here.
    Rest(&'static [Rest]),
    /// `awsQuery`: the request is a FORM — the operation named in an `Action` field
    /// beside the service's API version, and the input's members as further fields —
    /// and the answer is an XML document wrapping the operation's own result.
    Query {
        /// The API version its model declares (`service.version`), which every request
        /// to such a service names beside the action.
        version: &'static str,
    },
}

/// The media type an `awsQuery` request body carries.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The media type a `restJson1` request and the body of its answer carry.
const REST_CONTENT_TYPE: &str = "application/json";

/// What the BODY of one operation's request — and of its answer — carries, which is
/// the operation's own `httpPayload` binding, or the absence of one.
///
/// A REST protocol spreads an operation's members over the path, the query, the
/// headers and the body, and which of those the body is comes from the operation
/// itself (`smithy_query(query="operation")`): a member it binds as its `httpPayload`
/// IS the body, and a structure with no such member is carried as the DOCUMENT of
/// every member no other binding takes — the default a REST-JSON protocol gives one.
/// An operation whose every member is bound elsewhere carries no body at all, which is
/// what a read addressed by its path and its query sends.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Body {
    /// Every member no path, query or header binding carries, as one JSON document.
    Document,
    /// The one member the operation binds as its `httpPayload`: that member is the
    /// whole body, and no other member is in it. The member is reported as the
    /// DOCUMENT those bytes hold where they are one, which is what an integration
    /// whose payload member carries a document (a function's own JSON response)
    /// reports.
    Member(&'static str),
    /// The same binding for a payload member the invoked model types as a BLOB of
    /// bytes rather than as a document: an object's own content. It is reported as the
    /// TEXT those bytes are, whatever they hold, which is what the integration answers
    /// — a state reads a JSON object's content with `States.StringToJson` precisely
    /// because the member it arrives in is a string. Bytes that are not UTF-8 are
    /// reported as the text they lossily decode to, because a state's data is a JSON
    /// document and a JSON string holds no other bytes; the datasets this emulator
    /// reads and writes are text ([`super::item_reader`], [`super::result_writer`]).
    Text(&'static str),
    /// Nothing: every member of this operation is bound somewhere else.
    Empty,
}

/// How ONE operation of a REST service binds its input and output members to HTTP.
///
/// Every field is read from that operation's own
/// `smithy_query(query="operation")` — `http`, `httpLabel`, `httpQuery`,
/// `httpHeader`, `httpPayload` and `httpResponseCode` — so this table states no wire
/// detail of its own. An operation with no entry is one no call is made for, which
/// [`read`] says rather than sending a request its protocol does not describe.
#[derive(Debug)]
pub struct Rest {
    /// The operation this binds, as its model spells it.
    pub operation: &'static str,
    /// Its request method (`http.method`).
    pub method: &'static str,
    /// Its request URI (`http.uri`), in which `{<member>}` is an `httpLabel` member
    /// standing for the value the request names it with.
    pub uri: &'static str,
    /// What the request carries as its BODY ([`Body`]).
    pub request_body: Body,
    /// The members the request carries as HEADERS (`httpHeader`), each with the field
    /// name it goes under.
    pub headers: &'static [(&'static str, &'static str)],
    /// The members the request carries in the QUERY string (`httpQuery`), each with
    /// the parameter name it goes under.
    pub query: &'static [(&'static str, &'static str)],
    /// What the answer's BODY is read as ([`Body`]).
    pub response_body: Body,
    /// The output member the answer's STATUS is (`httpResponseCode`).
    pub response_status: &'static str,
    /// The output members the answer carries as HEADERS, each with the field name it
    /// is read from.
    pub response_headers: &'static [(&'static str, &'static str)],
}

/// The REST bindings of the Lambda operations a `Task` state's integration calls.
///
/// `smithy_query(query="operation",operation="Invoke",model_path="services/
/// service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`: `http` POST
/// `/2015-03-31/functions/{FunctionName}/invocations`, `FunctionName` an `httpLabel`,
/// `Payload` the `httpPayload`, `InvocationType`, `LogType` and `ClientContext`
/// `httpHeader`s, `Qualifier` an `httpQuery`; the output's `StatusCode` is its
/// `httpResponseCode`, `Payload` its `httpPayload`, and `ExecutedVersion`,
/// `FunctionError` and `LogResult` its headers.
const LAMBDA_REST: &[Rest] = &[Rest {
    operation: "Invoke",
    method: "POST",
    uri: "/2015-03-31/functions/{FunctionName}/invocations",
    request_body: Body::Member("Payload"),
    headers: &[
        ("InvocationType", "X-Amz-Invocation-Type"),
        ("LogType", "X-Amz-Log-Type"),
        ("ClientContext", "X-Amz-Client-Context"),
    ],
    query: &[("Qualifier", "Qualifier")],
    response_body: Body::Member("Payload"),
    response_status: "StatusCode",
    response_headers: &[
        ("ExecutedVersion", "X-Amz-Executed-Version"),
        ("FunctionError", "X-Amz-Function-Error"),
        ("LogResult", "X-Amz-Log-Result"),
    ],
}];

/// The REST bindings of the AWS Backup operations a `Task` state's integration calls.
///
/// Read from that service's own model with
/// `smithy_query(query="operation",operation="StartBackupJob,DescribeBackupJob,
/// ListBackupJobs,StartCopyJob,CreateBackupVault,DescribeBackupVault,
/// DescribeRecoveryPoint,ListRecoveryPointsByBackupVault",model_path="services/
/// service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/
/// com-amazonaws-backup-cryocontrollerusermanager.json")`. None of them binds a
/// member as its `httpPayload`, and none binds an output member to the status or to a
/// header, so each request carries the members its path and its query do not
/// ([`Body::Document`]) and each answer's body IS the operation's output document.
const BACKUP_REST: &[Rest] = &[
    // `http` PUT `/backup-jobs`, with every input member in the body.
    Rest {
        operation: "StartBackupJob",
        method: "PUT",
        uri: "/backup-jobs",
        request_body: Body::Document,
        headers: &[],
        query: &[],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` GET `/backup-jobs/{BackupJobId}`, `BackupJobId` its one member and an
    // `httpLabel`, so the request carries no body.
    Rest {
        operation: "DescribeBackupJob",
        method: "GET",
        uri: "/backup-jobs/{BackupJobId}",
        request_body: Body::Empty,
        headers: &[],
        query: &[],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` GET `/backup-jobs`, whose every input member is an `httpQuery` — each
    // under the parameter name that member's own binding states.
    Rest {
        operation: "ListBackupJobs",
        method: "GET",
        uri: "/backup-jobs",
        request_body: Body::Empty,
        headers: &[],
        query: &[
            ("NextToken", "nextToken"),
            ("MaxResults", "maxResults"),
            ("ByResourceArn", "resourceArn"),
            ("ByState", "state"),
            ("ByBackupVaultName", "backupVaultName"),
            ("ByCreatedBefore", "createdBefore"),
            ("ByCreatedAfter", "createdAfter"),
            ("ByResourceType", "resourceType"),
            ("ByAccountId", "accountId"),
            ("ByCompleteAfter", "completeAfter"),
            ("ByCompleteBefore", "completeBefore"),
            ("ByParentJobId", "parentJobId"),
            ("ByMessageCategory", "messageCategory"),
        ],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` PUT `/copy-jobs`, with every input member in the body.
    Rest {
        operation: "StartCopyJob",
        method: "PUT",
        uri: "/copy-jobs",
        request_body: Body::Document,
        headers: &[],
        query: &[],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` PUT `/backup-vaults/{BackupVaultName}`: the vault's name is the path, and
    // the members beside it (`BackupVaultTags`, `EncryptionKeyArn`,
    // `CreatorRequestId`) are the body.
    Rest {
        operation: "CreateBackupVault",
        method: "PUT",
        uri: "/backup-vaults/{BackupVaultName}",
        request_body: Body::Document,
        headers: &[],
        query: &[],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` GET `/backup-vaults/{BackupVaultName}`, with `BackupVaultAccountId` an
    // `httpQuery`.
    Rest {
        operation: "DescribeBackupVault",
        method: "GET",
        uri: "/backup-vaults/{BackupVaultName}",
        request_body: Body::Empty,
        headers: &[],
        query: &[("BackupVaultAccountId", "backupVaultAccountId")],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` GET `/backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}`:
    // TWO `httpLabel` members, the second of them the recovery point's own ARN.
    Rest {
        operation: "DescribeRecoveryPoint",
        method: "GET",
        uri: "/backup-vaults/{BackupVaultName}/recovery-points/{RecoveryPointArn}",
        request_body: Body::Empty,
        headers: &[],
        query: &[("BackupVaultAccountId", "backupVaultAccountId")],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    // `http` GET `/backup-vaults/{BackupVaultName}/recovery-points`, whose filters are
    // `httpQuery` members beside the vault in its path.
    Rest {
        operation: "ListRecoveryPointsByBackupVault",
        method: "GET",
        uri: "/backup-vaults/{BackupVaultName}/recovery-points",
        request_body: Body::Empty,
        headers: &[],
        query: &[
            ("BackupVaultAccountId", "backupVaultAccountId"),
            ("NextToken", "nextToken"),
            ("MaxResults", "maxResults"),
            ("ByResourceArn", "resourceArn"),
            ("ByResourceType", "resourceType"),
            ("ByBackupPlanId", "backupPlanId"),
            ("ByCreatedBefore", "createdBefore"),
            ("ByCreatedAfter", "createdAfter"),
            ("ByParentRecoveryPointArn", "parentRecoveryPointArn"),
        ],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
];

///
/// A `Task` state's integration parameters are written in PascalCase whatever the
/// invoked API spells its own members in — `StateMachineArn` for a nested execution
/// whose API member is `stateMachineArn` — so the document the state built is put on
/// the wire under the spelling that service's model uses. The two conventions differ
/// in the FIRST character and nothing else, which is the whole of this.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MemberCase {
    /// `stateMachineArn`, `taskDefinition`: the first character is lower case.
    LowerCamel,
    /// `TableName`, `QueueUrl`: the first character is upper case.
    Pascal,
}

impl MemberCase {
    /// `member` under this convention's spelling of it.
    fn spell(self, member: &str) -> String {
        let mut characters = member.chars();
        let Some(first) = characters.next() else {
            return String::new();
        };
        match self {
            Self::LowerCamel => first.to_lowercase().chain(characters).collect(),
            Self::Pascal => first.to_uppercase().chain(characters).collect(),
        }
    }
}

/// How the query protocol spreads a MAP member of one integration's request over form
/// fields.
///
/// An `awsQuery` request is a flat form, so a member that is a map is carried as one
/// NUMBERED ENTRY per pair — `<member>.entry.<n>.<key>` beside
/// `<member>.entry.<n>.<value>` — and the names its key and value halves go under are
/// the map shape's own (`key` and `value` unless that shape's members carry an
/// `xmlName`). Those names are facts of the INVOKED model, which this crate cannot
/// read, so they are stated per member here; a collection member no entry names is
/// one no call is made for ([`form_body`]), exactly as before.
#[derive(Debug, Clone, Copy)]
pub struct FormMap {
    /// The member, as the WIRE spelling of the request names it ([`MemberCase`]).
    pub member: &'static str,
    /// The name each pair's KEY is written under.
    pub key: &'static str,
    /// The name each pair's VALUE is written under.
    pub value: &'static str,
    /// Whether each pair's value is a STRUCTURE, whose own members are then one field
    /// each below that name (`<member>.entry.<n>.<value>.<Member>`); a map of scalars
    /// carries the value itself under it.
    pub structured: bool,
}

/// The map members of the integration requests this emulator binds to a form field,
/// keyed by the `<namespace>:<action>` the OPTIMIZED family names the integration by.
///
/// The key serves BOTH families, unlike the conversions an integration itself
/// documents: what these state is the shape of the INVOKED operation's own member, so
/// it holds however the `Resource` named that operation.
const FORM_MAPS: &[(&str, &[FormMap])] = &[
    // `arn:aws:states:::sns:publish` carries `Publish`'s `MessageAttributes`, a map
    // whose key is written `Name` and whose value `Value`, each value a
    // `MessageAttributeValue` structure of `DataType`, `StringValue` and `BinaryValue`
    // (`smithy_query(query="shape",shape="MessageAttributeMap,MessageAttributeValue",
    // model_path="services/service-com-amazonaws-sns-amazonsimplenotificationservice/
    // smithy-model/com-amazonaws-sns-amazonsimplenotificationservice.json")`: the map's
    // key member carries `xmlName` `Name` and its value member `xmlName` `Value`).
    (
        SNS_PUBLISH_INTEGRATION,
        &[FormMap {
            member: "MessageAttributes",
            key: "Name",
            value: "Value",
            structured: true,
        }],
    ),
];

/// The services a `Task` state's integration call is made to, and how a request to
/// each is bound to HTTP ([`Wire`]) — so the document the state built IS the
/// operation's input, wherever that protocol puts it.
///
/// One entry per SERVICE, never per action: for an AWS JSON service the action of a
/// `Resource` names the operation, so every operation it models is reachable. A REST
/// service binds each operation's members to the path, the query, the headers and one
/// payload separately, so its entry lists the operations whose bindings are stated and
/// [`read`] makes no call for any other.
///
/// It is a LIST rather than a rule because each entry's wire facts are that
/// service's own model's, which this crate cannot read; a service whose model is
/// mounted here but has no entry yet is one no call is made for, and [`read`] says
/// exactly that rather than claiming anything about its protocol.
pub const APIS: &[Api] = &[
    // `services/service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json`: awsJson1_1, sigv4Name `athena`,
    // endpointPrefix `athena`, sdkId `Athena`, service shape `AmazonAthena`.
    Api {
        namespace: "athena",
        action_prefix: None,
        signing_name: "athena",
        endpoint_prefix: "athena",
        wire: Wire::Json {
            target_prefix: "AmazonAthena",
            version: "1.1",
        },
        sdk_id: "Athena",
        error_prefix: "Athena",
        // The client class `AmazonAthenaClient`, whose base exception is
        // `AmazonAthenaException` — the model's service shape `AmazonAthena`.
        sdk_client: "AmazonAthena",
        sdk_exception: "AmazonAthenaException",
        // `StartQueryExecution`'s input: `QueryString`, `WorkGroup`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-backup-cryocontrollerusermanager/smithy-model/
    // com-amazonaws-backup-cryocontrollerusermanager.json`: restJson1, sigv4Name
    // `backup`, endpointPrefix `backup`, arnNamespace `backup`, sdkId `Backup` — a
    // REST protocol, so each operation's own bindings are what a request to it is
    // built from ([`BACKUP_REST`]).
    Api {
        namespace: "backup",
        action_prefix: None,
        signing_name: "backup",
        endpoint_prefix: "backup",
        wire: Wire::Rest(BACKUP_REST),
        sdk_id: "Backup",
        error_prefix: "Backup",
        // The client class `AWSBackupClient`, whose base exception is
        // `AWSBackupException`; the model's service shape is the service's internal
        // name (`CryoControllerUserManager`), which the SDK's own names do not carry.
        sdk_client: "AWSBackup",
        sdk_exception: "AWSBackupException",
        // `StartBackupJob`'s input: `BackupVaultName`, `ResourceArn`, `IamRoleArn`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
    // com-amazonaws-dynamodb-dynamodb-20120810.json`: awsJson1_0, sigv4Name
    // `dynamodb`, endpointPrefix `dynamodb`, sdkId `DynamoDB`, service shape
    // `DynamoDB_20120810`.
    Api {
        namespace: "dynamodb",
        action_prefix: None,
        signing_name: "dynamodb",
        endpoint_prefix: "dynamodb",
        wire: Wire::Json {
            target_prefix: "DynamoDB_20120810",
            version: "1.0",
        },
        sdk_id: "DynamoDB",
        error_prefix: "DynamoDB",
        // The one client whose names follow neither the model's service shape
        // (`DynamoDB_20120810`) nor its SDK id: the client is `AmazonDynamoDBv2` and
        // its base exception `AmazonDynamoDBException`, which is how a DynamoDB
        // refusal already reports itself to a `Task` state.
        sdk_client: "AmazonDynamoDBv2",
        sdk_exception: "AmazonDynamoDBException",
        // `PutItem`'s input: `TableName`, `Item`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-sqs-amazonsqs/smithy-model/
    // com-amazonaws-sqs-amazonsqs.json`: awsJson1_0, sigv4Name `sqs`,
    // endpointPrefix `sqs`, sdkId `SQS`, service shape `AmazonSQS`.
    Api {
        namespace: "sqs",
        action_prefix: None,
        signing_name: "sqs",
        endpoint_prefix: "sqs",
        wire: Wire::Json {
            target_prefix: "AmazonSQS",
            version: "1.0",
        },
        sdk_id: "SQS",
        error_prefix: "SQS",
        // The client class `AmazonSQSClient`, whose base exception is
        // `AmazonSQSException` — the model's service shape `AmazonSQS`.
        sdk_client: "AmazonSQS",
        sdk_exception: "AmazonSQSException",
        // `SendMessage`'s input: `QueueUrl`, `MessageBody`.
        member_case: MemberCase::Pascal,
        // Amazon SQS is the one service here whose model carries
        // `aws.protocols#awsQueryCompatible`, the modifier that keeps its errors'
        // legacy query codes (`AWS.SimpleQueueService.NonExistentQueue`) beside their
        // shapes.
        query_compatible: true,
    },
    // `services/service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
    // com-amazonaws-sns-amazonsimplenotificationservice.json`: awsQuery, sigv4Name
    // `sns`, endpointPrefix `sns`, arnNamespace `sns`, sdkId `SNS`, version
    // `2010-03-31` — a FORM protocol, so the operation's input goes on the wire as form
    // fields beside the action it names ([`Wire::Query`]).
    Api {
        namespace: SNS_NAMESPACE,
        // Amazon SNS publishes its actions as `SNS:<ApiName>` ("Actions defined by
        // Amazon Simple Notification Service", IAM Service Authorization Reference),
        // which is the ONE spelling this tree holds for them — the same one this
        // emulator's SNS service words its own refusals with — so a refused publish
        // names `SNS:Publish` rather than the ARN namespace.
        action_prefix: Some("SNS"),
        signing_name: SNS_NAMESPACE,
        endpoint_prefix: SNS_NAMESPACE,
        wire: Wire::Query {
            version: "2010-03-31",
        },
        sdk_id: "SNS",
        error_prefix: "SNS",
        // The client class `AmazonSNSClient`, whose base exception is
        // `AmazonSNSException`; the model's service shape is the service's full name
        // (`AmazonSimpleNotificationService`), which the SDK's own names do not carry.
        sdk_client: "AmazonSNS",
        sdk_exception: "AmazonSNSException",
        // `Publish`'s input: `TopicArn`, `Message`, `MessageAttributes`.
        member_case: MemberCase::Pascal,
        // Its model declares the `awsQuery` protocol itself rather than the
        // compatibility modifier, so an error of it is named by its own query code and
        // there is no second name to ask for.
        query_compatible: false,
    },
    // `services/service-com-amazonaws-eventbridge-awsevents/smithy-model/
    // com-amazonaws-eventbridge-awsevents.json`: awsJson1_1, sigv4Name `events`,
    // endpointPrefix `events`, sdkId `EventBridge`, service shape `AWSEvents`.
    Api {
        namespace: "events",
        action_prefix: None,
        signing_name: "events",
        endpoint_prefix: "events",
        wire: Wire::Json {
            target_prefix: "AWSEvents",
            version: "1.1",
        },
        sdk_id: "EventBridge",
        error_prefix: "EventBridge",
        // The client the events API is reached through is
        // `AmazonCloudWatchEventsClient`, whose base exception is
        // `AmazonCloudWatchEventsException`.
        sdk_client: "AmazonCloudWatchEvents",
        sdk_exception: "AmazonCloudWatchEventsException",
        // `PutEvents`' input: `Entries`, `EndpointId`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // This service itself, for a task that starts another execution:
    // `services/service-com-amazonaws-sfn-awsstepfunctions/smithy-model/
    // com-amazonaws-sfn-awsstepfunctions.json`: awsJson1_0, sigv4Name `states`,
    // endpointPrefix `states`, sdkId `SFN`, service shape `AWSStepFunctions`.
    Api {
        namespace: STATES_NAMESPACE,
        action_prefix: None,
        signing_name: STATES_NAMESPACE,
        endpoint_prefix: STATES_NAMESPACE,
        wire: Wire::Json {
            target_prefix: "AWSStepFunctions",
            version: "1.0",
        },
        sdk_id: "SFN",
        // A failed `states:startExecution` task is reported under the service's own
        // name rather than under the `SFN` id its SDK carries
        // (`StepFunctions.AWSStepFunctionsException` for a refusal it does not model).
        error_prefix: "StepFunctions",
        // The client class `AWSStepFunctionsClient`, whose base exception is
        // `AWSStepFunctionsException` — the model's service shape `AWSStepFunctions`.
        sdk_client: "AWSStepFunctions",
        sdk_exception: "AWSStepFunctionsException",
        // `StartExecution`'s input: `stateMachineArn`, `name`, `input`.
        member_case: MemberCase::LowerCamel,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-kinesis-kinesis-20131202/smithy-model/
    // com-amazonaws-kinesis-kinesis-20131202.json`: awsJson1_1, sigv4Name
    // `kinesis`, endpointPrefix `kinesis`, sdkId `Kinesis`, service shape
    // `Kinesis_20131202`.
    Api {
        namespace: "kinesis",
        action_prefix: None,
        signing_name: "kinesis",
        endpoint_prefix: "kinesis",
        wire: Wire::Json {
            target_prefix: "Kinesis_20131202",
            version: "1.1",
        },
        sdk_id: "Kinesis",
        error_prefix: "Kinesis",
        // The client class `AmazonKinesisClient`, whose base exception is
        // `AmazonKinesisException`.
        sdk_client: "AmazonKinesis",
        sdk_exception: "AmazonKinesisException",
        // `PutRecord`'s input: `StreamName`, `Data`, `PartitionKey`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-ssm-amazonssm/smithy-model/
    // com-amazonaws-ssm-amazonssm.json`: awsJson1_1, sigv4Name `ssm`,
    // endpointPrefix `ssm`, sdkId `SSM`, service shape `AmazonSSM`.
    Api {
        namespace: "ssm",
        action_prefix: None,
        signing_name: "ssm",
        endpoint_prefix: "ssm",
        wire: Wire::Json {
            target_prefix: "AmazonSSM",
            version: "1.1",
        },
        sdk_id: "SSM",
        error_prefix: "SSM",
        // The client class `AmazonSSMClient`, whose base exception is
        // `AmazonSSMException` — the model's service shape `AmazonSSM`.
        sdk_client: "AmazonSSM",
        sdk_exception: "AmazonSSMException",
        // `GetParameter`'s input: `Name`, `WithDecryption`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-ecs-amazonec2containerservicev20141113/
    // smithy-model/com-amazonaws-ecs-amazonec2containerservicev20141113.json`:
    // awsJson1_1, sigv4Name `ecs`, endpointPrefix `ecs`, sdkId `ECS`, service shape
    // `AmazonEC2ContainerServiceV20141113`.
    Api {
        namespace: "ecs",
        action_prefix: None,
        signing_name: "ecs",
        endpoint_prefix: "ecs",
        wire: Wire::Json {
            target_prefix: "AmazonEC2ContainerServiceV20141113",
            version: "1.1",
        },
        sdk_id: "ECS",
        error_prefix: "ECS",
        // The client class `AmazonECSClient`, whose base exception is
        // `AmazonECSException`; the model's service shape carries the API's date
        // (`AmazonEC2ContainerServiceV20141113`), which the SDK's own names do not.
        sdk_client: "AmazonECS",
        sdk_exception: "AmazonECSException",
        // `RunTask`'s input: `cluster`, `taskDefinition`, `overrides`.
        member_case: MemberCase::LowerCamel,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-glue-awsglue/smithy-model/
    // com-amazonaws-glue-awsglue.json`: awsJson1_1, sigv4Name `glue`, endpointPrefix
    // `glue`, sdkId `Glue`, service shape `AWSGlue`.
    Api {
        namespace: "glue",
        action_prefix: None,
        signing_name: "glue",
        endpoint_prefix: "glue",
        wire: Wire::Json {
            target_prefix: "AWSGlue",
            version: "1.1",
        },
        sdk_id: "Glue",
        error_prefix: "Glue",
        // The client class `AWSGlueClient`, whose base exception is
        // `AWSGlueException` — the model's service shape `AWSGlue`.
        sdk_client: "AWSGlue",
        sdk_exception: "AWSGlueException",
        // `StartJobRun`'s input: `JobName`, `Arguments`, `Timeout`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
    // com-amazonaws-lambda-awsgirapiservice.json`: restJson1, sigv4Name `lambda`,
    // endpointPrefix `lambda`, sdkId `Lambda` — a REST protocol, so each operation's
    // own bindings are what a request to it is built from ([`LAMBDA_REST`]).
    Api {
        namespace: LAMBDA_NAMESPACE,
        action_prefix: None,
        signing_name: LAMBDA_NAMESPACE,
        endpoint_prefix: LAMBDA_NAMESPACE,
        wire: Wire::Rest(LAMBDA_REST),
        sdk_id: "Lambda",
        error_prefix: "Lambda",
        // The client class `AWSLambdaClient`, whose base exception is
        // `AWSLambdaException` — the model's service shape `AWSLambda`.
        sdk_client: "AWSLambda",
        sdk_exception: "AWSLambdaException",
        // `Invoke`'s input: `FunctionName`, `Payload`, `InvocationType`.
        member_case: MemberCase::Pascal,
        query_compatible: false,
    },
    // `services/service-com-amazonaws-sts-awssecuritytokenservicev20110615/
    // smithy-model/com-amazonaws-sts-awssecuritytokenservicev20110615.json`: awsQuery,
    // sigv4Name `sts`, endpointPrefix `sts`, sdkId `STS`, version `2011-06-15` — a FORM
    // protocol, so the operation's input goes on the wire as form fields beside the
    // action it names ([`Wire::Query`]).
    Api {
        namespace: "sts",
        action_prefix: None,
        signing_name: "sts",
        endpoint_prefix: "sts",
        wire: Wire::Query {
            version: "2011-06-15",
        },
        sdk_id: "STS",
        error_prefix: "STS",
        // The client class `AWSSecurityTokenServiceClient`, whose base exception is
        // `AWSSecurityTokenServiceException` — the model's service shape
        // `AWSSecurityTokenServiceV20110615` without the version it carries.
        sdk_client: "AWSSecurityTokenService",
        sdk_exception: "AWSSecurityTokenServiceException",
        // `AssumeRole`'s input: `RoleArn`, `RoleSessionName`, `DurationSeconds`.
        member_case: MemberCase::Pascal,
        // Its model declares the `awsQuery` protocol itself rather than the
        // compatibility modifier, so an error of it is named by its own query code and
        // there is no second name to ask for.
        query_compatible: false,
    },
];

/// Amazon S3, for the calls this crate makes to it: the DATASET a `Map` state's
/// `ItemReader` names ([`super::item_reader`]), the result objects its `ResultWriter`
/// writes ([`super::result_writer`]), and a `Task` state whose `Resource` names one of
/// its operations through the AWS SDK integration family.
///
/// Deliberately NOT one of [`APIS`], which is where the OPTIMIZED family's services are
/// looked up: S3 publishes no optimized integration, so `arn:aws:states:::s3:<action>`
/// names no `Task` state's work — it is how a `Map` state's `ItemReader` and
/// `ResultWriter` name their dataset. [`read`] reaches these facts for the AWS SDK
/// family's own token (`aws-sdk:s3:<action>`) and reports an optimized `Resource` as
/// work nothing was run for, exactly as before.
///
/// `services/service-com-amazonaws-s3-amazons3/smithy-model/
/// com-amazonaws-s3-amazons3.json`: restXml, sigv4Name `s3`, endpointPrefix `s3`,
/// sdkId `S3`, arnNamespace `s3`.
static S3_API: Api = Api {
    namespace: S3_NAMESPACE,
    action_prefix: None,
    signing_name: S3_NAMESPACE,
    endpoint_prefix: S3_NAMESPACE,
    wire: Wire::Rest(S3_REST),
    sdk_id: "S3",
    error_prefix: "S3",
    // The client class `AmazonS3Client`, whose base exception is `AmazonS3Exception`.
    sdk_client: "AmazonS3",
    sdk_exception: "AmazonS3Exception",
    // `GetObject`'s input: `Bucket`, `Key`; `ListObjectsV2`'s: `Bucket`, `Prefix`.
    member_case: MemberCase::Pascal,
    query_compatible: false,
};

/// S3's ARN namespace, its signing name and its endpoint prefix, which are the same
/// label.
const S3_NAMESPACE: &str = "s3";

/// The REST bindings of the S3 operations this crate calls: the ones a dataset read
/// and a dataset write make, and the ones a `Task` state's AWS SDK integration names
/// ([`S3_SDK_OPERATIONS`]).
///
/// `smithy_query(query="operation",operation="ListObjectsV2,GetObject,PutObject,
/// HeadBucket,DeleteObject",model_path="services/service-com-amazonaws-s3-amazons3/
/// smithy-model/com-amazonaws-s3-amazons3.json")`: `ListObjectsV2` is `http` GET
/// `/{Bucket}?list-type=2` with `Bucket` an `httpLabel` and `Prefix`,
/// `ContinuationToken` and `MaxKeys` `httpQuery` members, answering an XML document
/// as the whole body; `GetObject` is GET `/{Bucket}/{Key+}?x-id=GetObject` with
/// `Bucket` and the GREEDY `Key` as `httpLabel`s, its output binds `Body` as its
/// `httpPayload` and `ETag`, `ContentType` and `VersionId` to headers whose value is
/// the text they carry; `PutObject` is PUT `/{Bucket}/{Key+}?x-id=PutObject` over
/// the same two labels, with `Body` as the request's own `httpPayload` and `ETag` bound
/// to the answer's `ETag` header; `HeadBucket` is HEAD `/{Bucket}` over the one label,
/// every member of its output bound to a header; `DeleteObject` is DELETE
/// `/{Bucket}/{Key+}?x-id=DeleteObject` over the two labels, with `VersionId` an
/// `httpQuery` member, `MFA`, `BypassGovernanceRetention`, `IfMatch` and the rest
/// `httpHeader`s, and every member of its output bound to a header.
const S3_REST: &[Rest] = &[
    Rest {
        operation: LIST_OBJECTS_OPERATION,
        method: "GET",
        uri: "/{Bucket}?list-type=2",
        request_body: Body::Empty,
        headers: &[],
        query: &[
            ("Prefix", "prefix"),
            ("ContinuationToken", "continuation-token"),
            ("MaxKeys", "max-keys"),
        ],
        response_body: Body::Text(DATASET_BODY),
        response_status: "",
        response_headers: &[],
    },
    Rest {
        operation: GET_OBJECT_OPERATION,
        method: "GET",
        uri: "/{Bucket}/{Key+}?x-id=GetObject",
        request_body: Body::Empty,
        headers: &[],
        query: &[],
        response_body: Body::Text(DATASET_BODY),
        response_status: "",
        // The output members a read's answer carries beside the object's bytes: the
        // ones whose value IS the text the header holds, so what the result reports is
        // the answer's own. A member the model types as a number or an instant
        // (`ContentLength`, `LastModified`) is deliberately absent: its header carries
        // text, and reporting that text under a member a client reads as a number
        // would be the wrong type rather than the answer.
        response_headers: &[
            ("ETag", "ETag"),
            ("ContentType", "Content-Type"),
            ("VersionId", "x-amz-version-id"),
        ],
    },
    Rest {
        operation: PUT_OBJECT_OPERATION,
        method: "PUT",
        uri: "/{Bucket}/{Key+}?x-id=PutObject",
        // The object's own bytes are the request's body: the operation binds `Body` as
        // its `httpPayload`, so what the write carries under that member IS the object.
        request_body: Body::Member(DATASET_BODY),
        headers: &[],
        query: &[],
        // A write answers no document at all — every member of its output is a header —
        // so the one the caller of a write reads is the entity tag the answer carried.
        response_body: Body::Empty,
        response_status: "",
        response_headers: &[("ETag", "ETag")],
    },
    Rest {
        operation: HEAD_BUCKET_OPERATION,
        method: "HEAD",
        uri: "/{Bucket}",
        // `Bucket` is the path, and `ExpectedBucketOwner` the one other input member,
        // is a header: a HEAD of a bucket sends no body at all.
        request_body: Body::Empty,
        headers: &[("ExpectedBucketOwner", "x-amz-expected-bucket-owner")],
        query: &[],
        // A HEAD carries no body: every member of this operation's output is a header,
        // so the answer's headers ARE its output.
        response_body: Body::Empty,
        response_status: "",
        // The output members whose value IS the text the header holds. The remaining
        // one, `AccessPointAlias`, the model types as a boolean, and reporting a
        // header's text under a member a client reads as a flag would be the wrong type
        // rather than the answer — the same reading `GetObject`'s own numbers and
        // instants are left out under.
        response_headers: &[
            ("BucketArn", "x-amz-bucket-arn"),
            ("BucketRegion", "x-amz-bucket-region"),
            ("BucketLocationName", "x-amz-bucket-location-name"),
            ("BucketLocationType", "x-amz-bucket-location-type"),
        ],
    },
    Rest {
        operation: DELETE_OBJECT_OPERATION,
        method: "DELETE",
        uri: "/{Bucket}/{Key+}?x-id=DeleteObject",
        // Both labels are in the path and every other input member is a header or a
        // query parameter, so a delete sends no body.
        request_body: Body::Empty,
        headers: &[
            ("MFA", "x-amz-mfa"),
            ("RequestPayer", "x-amz-request-payer"),
            (
                "BypassGovernanceRetention",
                "x-amz-bypass-governance-retention",
            ),
            ("ExpectedBucketOwner", "x-amz-expected-bucket-owner"),
            ("IfMatch", "If-Match"),
        ],
        query: &[("VersionId", "versionId")],
        // A delete answers no body either: its output is the headers its answer
        // carried, exactly as a write's is.
        response_body: Body::Empty,
        response_status: "",
        // `DeleteMarker` is left out for the reason `HeadBucket`'s `AccessPointAlias`
        // is: the model types it as a boolean.
        response_headers: &[
            ("VersionId", "x-amz-version-id"),
            ("RequestCharged", "x-amz-request-charged"),
        ],
    },
];

/// The S3 operations a dataset read makes, as that model spells them.
pub const LIST_OBJECTS_OPERATION: &str = "ListObjectsV2";
/// See [`LIST_OBJECTS_OPERATION`].
pub const GET_OBJECT_OPERATION: &str = "GetObject";
/// The S3 operation a dataset WRITE makes, as that model spells it: the one call a
/// `Map` state's `ResultWriter` is made of ([`super::result_writer`]).
pub const PUT_OBJECT_OPERATION: &str = "PutObject";
/// The S3 operations a `Task` state names that no dataset call makes, as that model
/// spells them.
const HEAD_BUCKET_OPERATION: &str = "HeadBucket";
/// See [`HEAD_BUCKET_OPERATION`].
const DELETE_OBJECT_OPERATION: &str = "DeleteObject";

/// The S3 operations a `Task` state's AWS SDK integration call IS made for.
///
/// Each is an operation whose answer this emulator reads as the operation's own
/// output: `GetObject` binds the object's bytes as its output's `httpPayload`, and
/// `PutObject`, `HeadBucket` and `DeleteObject` answer no body at all, so the headers
/// their answers carry ARE their output members. A LISTING's answer is a restXml
/// document whose elements this emulator does not read as that operation's output
/// members, so a `Task` naming it is reported as work nothing was run for rather than
/// answered with that document's text — the dataset read that DOES consume it reads the
/// text itself ([`super::item_reader`]).
const S3_SDK_OPERATIONS: &[&str] = &[
    GET_OBJECT_OPERATION,
    PUT_OBJECT_OPERATION,
    HEAD_BUCKET_OPERATION,
    DELETE_OBJECT_OPERATION,
];

/// The member a dataset read reports the answer's BODY under: `GetObject`'s own
/// `Body`, which that operation binds as its `httpPayload`, and — for the listing,
/// whose protocol carries its document as the whole body — that document's text.
pub const DATASET_BODY: &str = "Body";

/// The dataset call `operation` names, for the reader that reads a `Map` state's
/// dataset ([`super::item_reader`]) and the writer that writes its results
/// ([`super::result_writer`]); `None` for an operation these bindings do not carry.
///
/// A dataset call is NOT a `Task` state's integration: its result is not reported to
/// any state, so the call carries no SDK envelope, no run-a-job pattern and no
/// conversion — only where the request goes and how the answer's body comes back.
#[must_use]
pub fn dataset_call(operation: &str) -> Option<Call> {
    let rest = S3_API.rest(operation)?;
    Some(Call {
        api: &S3_API,
        operation: rest.operation.to_string(),
        rest: Some(rest),
        job: None,
        callback: false,
        parsed_output: false,
        json_text: &[],
        json_text_at: &[],
        form_maps: &[],
        sdk_envelope: false,
        epoch_millis: &[],
        targets: None,
        invokes: None,
        endpoint: false,
        // The caller hands this call the object's own BYTES, already rendered: the
        // dataset this writes is the text that writer built ([`super::result_writer`]),
        // not a value out of a state's document.
        payload_document: false,
    })
}

/// One integration call: the service's API, and the operation of it the `Resource`'s
/// action named.
#[derive(Debug, Clone)]
pub struct Call {
    pub api: &'static Api,
    /// The operation name as the service's model spells it, derived from the action.
    pub operation: String,
    /// How this operation binds its members to HTTP, for a service whose protocol is
    /// a REST one ([`Rest`]); `None` for an AWS JSON service, whose request body IS
    /// the operation's input document.
    pub rest: Option<&'static Rest>,
    /// The run-a-job pattern this call STARTS, for a `Resource` that asked for one
    /// (`.sync`, `.sync:2`): the task is finished when the job this call starts is,
    /// so the walk parks at it instead of placing the response as the result.
    /// `None` for a request-response integration, whose call IS the whole task.
    pub job: Option<&'static RunAJob>,
    /// Whether the `Resource` asked for the callback (`.waitForTaskToken`) pattern:
    /// the call is made, and the task is then finished by a `SendTaskSuccess` or
    /// `SendTaskFailure` quoting the `taskToken` that call carried, so the walk
    /// suspends at it instead of placing the response as the result.
    pub callback: bool,
    /// Whether the `Resource` asked for the pattern's SECOND revision (`.sync:2`),
    /// which differs in one thing: a document the job carries as JSON TEXT is
    /// reported as the parsed document rather than as that text
    /// ([`RunAJob::json_text_at`]).
    pub parsed_output: bool,
    /// The members of this call's request the invoked operation types as a STRING
    /// while the integration documents them as a document, so the document the
    /// state built is carried as that document's JSON text
    /// ([`JSON_TEXT_MEMBERS`]). Empty for every call that converts nothing.
    pub json_text: &'static [&'static str],
    /// The same conversion for a member NESTED inside the request rather than at its
    /// top level, each named by the path to it through the request's objects and lists
    /// ([`JSON_TEXT_AT_MEMBERS`]). Empty for every call that converts nothing.
    pub json_text_at: &'static [&'static [&'static str]],
    /// The MAP members of this call's request, for a request a FORM protocol spreads
    /// over one field per leaf ([`FORM_MAPS`]). Empty for every call whose request
    /// carries no such member, and for every protocol that carries the request as a
    /// document.
    pub form_maps: &'static [FormMap],
    /// Whether the task's result is the literal SDK response of this call, which
    /// carries the SDK's own response envelope beside the operation's members
    /// ([`with_sdk_envelope`]).
    ///
    /// True for the OPTIMIZED integration family, whose result is that SDK response
    /// as it stands. False for the AWS SDK family, whose result is the invoked
    /// operation's own members, and for the calls a run-a-job pattern makes to REPORT
    /// and to STOP its job: those are not a task's own work, so what they answer is
    /// read rather than reported.
    pub sdk_envelope: bool,
    /// The members of this call's RESPONSE that carry an instant, which the result
    /// reports in whole epoch milliseconds ([`EPOCH_MILLIS_MEMBERS`]). Empty for every
    /// call that converts nothing.
    pub epoch_millis: &'static [&'static str],
    /// Where this call's request names the resource it acts ON, for the integrations
    /// whose target is named by the request ([`TARGET_RESOURCES`]); `None` for a call
    /// no such entry is carried for.
    pub targets: Option<&'static TargetResource>,
    /// The Lambda FUNCTION this task invokes DIRECTLY, as the `Resource` named it, for
    /// a `Resource` that is a function's ARN rather than one of this service's
    /// integration tokens.
    ///
    /// `None` for every `Resource` that names an integration, whose request IS the
    /// document the state built. For a direct invocation that document is the
    /// function's event instead ([`Call::operation_input`]), and the task's result is
    /// the function's own response payload ([`reported_result`]).
    pub invokes: Option<String>,
    /// Whether this call's request is a JSON document a STATE built, whose payload
    /// member therefore goes on the wire as the TEXT of the JSON value that document
    /// carries for it: a string as the JSON string literal it is in the document, an
    /// object or an array as its own JSON text.
    ///
    /// True for every call a `Task` state's work is made of, because "the state's
    /// effective input IS that operation's request": what the state wrote for the
    /// payload member is a JSON value, and the bytes the member carries are that
    /// value's text. False for a dataset call, which is handed the object's own bytes
    /// instead ([`dataset_call`]).
    pub payload_document: bool,
    /// Whether the work is an HTTP REQUEST the state's own parameters describe rather
    /// than an API call to an operation of a service ([`super::http_task`]).
    ///
    /// True for exactly one `Resource`, Step Functions' own HTTP Task
    /// (`arn:aws:states:::http:invoke`): its endpoint, method, headers, query and body
    /// are the task's parameters and its result is that endpoint's own response, so
    /// NONE of the wire facts an [`Api`] states is read for it — no operation's input
    /// is built ([`wired`]), no response document is read as an operation's output
    /// ([`answer_of`]) and no [`TARGET_RESOURCES`] entry names a resource it acts on.
    /// The entry it carries is this service's own, because `http:invoke` is Step
    /// Functions' own integration, served by Step Functions.
    pub endpoint: bool,
}

/// The member of `Invoke`'s input that names the function to invoke, which that
/// operation binds as its `httpLabel`, and the member carrying the event, which it
/// binds as its `httpPayload` — both as [`LAMBDA_REST`] reads them off the operation's
/// own bindings.
const FUNCTION_NAME_MEMBER: &str = "FunctionName";
/// See [`FUNCTION_NAME_MEMBER`].
const PAYLOAD_MEMBER: &str = "Payload";

/// The member of `Invoke`'s OUTPUT that carries Lambda's own signal that the invocation
/// ran but the function ERRORED — "If present, indicates that an error occurred during
/// function execution" — which that operation binds to the `x-amz-function-error`
/// response header ([`LAMBDA_REST`]).
const FUNCTION_ERROR_MEMBER: &str = "FunctionError";

/// The member of the error document a failed run answers with that names the error's own
/// CLASS, which is the name a `Task` state reports for such a failure, so a definition
/// retrying or catching the exception the handler raised names that class.
const ERROR_TYPE_MEMBER: &str = "errorType";

/// The name a failed invocation is reported under when its error document names no class
/// of its own, which is how Step Functions names an unhandled Lambda error it cannot
/// attribute to one: "Unhandled errors in Lambda are reported as `Lambda.Unknown` in the
/// error output", a name a definition's `Retry` or `Catch` matches on.
const LAMBDA_UNKNOWN_ERROR: &str = "Lambda.Unknown";

/// How the diagnosis of PARAMETERS the integration could not build its call from opens,
/// which is what tells it from every other cause and makes the state name itself around
/// it ([`super::asl::names_its_state`]).
///
/// `pub(super)`, because the module that words the state around a diagnosis is the one
/// that reads this opening.
pub(super) const UNUSABLE_PARAMETERS_OPENING: &str = "The Parameters '";

/// The failure the task's own PARAMETERS are, for parameters the invoked operation
/// cannot be called with at all, or `None` for parameters that build a call.
///
/// `Payload` is "The JSON that you want to provide to your Lambda function as input"
/// (`smithy_query(query="doc",shape="InvocationRequest",member="Payload",model_path=
/// "services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`), so the parameter carries the JSON
/// DOCUMENT the invocation is made with — an object or an array. A bare JSON scalar is a
/// value rather than a document, so parameters whose `Payload` resolved to one name no
/// invocation this call can be built from, and the task fails on its own parameters
/// before any request is sent.
///
/// The diagnosis is the one the service words for parameters it could not start a task
/// with: the parameters AS RESOLVED, and then the deserialization of the member that
/// could not be read — the member's own name and the JSON value it was handed, both
/// taken from those resolved parameters, because which value a path resolved to is what
/// a reader has to be told.
///
/// A parameter the request carries as the NULL document names no value at all, which is
/// how this module's own request builder reads one ([`Rest::wired`]): such a request is
/// the one that names no payload, and a function invoked with no event is what it asks
/// for, so it is not refused here.
///
/// Only the integration forms whose request IS the task's parameters are judged here
/// ([`Call::invokes`]): a `Resource` that names the FUNCTION carries the state's data as
/// the event rather than as a parameter, so there is no parameter to refuse.
///
/// Pure, so the reading of every parameter form is pinned by this module's own tests.
fn unusable_parameters(call: &Call, request: &Value) -> Option<Failed> {
    if call.api.namespace != LAMBDA_NAMESPACE
        || call.operation != LAMBDA_INVOKE_OPERATION
        || call.invokes.is_some()
    {
        return None;
    }
    let payload = request.get(PAYLOAD_MEMBER)?;
    if matches!(
        payload,
        Value::Object(_) | Value::Array(_) | Value::Null
    ) {
        return None;
    }
    Some(Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "{UNUSABLE_PARAMETERS_OPENING}{request}' could not be used to start the Task: \
             [Cannot deserialize the '{PAYLOAD_MEMBER}' of the invocation from {} value \
             {payload}]",
            json_kind(payload)
        ),
    })
}

/// How the deserialization of a member names the JSON VALUE it was handed: the kind of
/// value the document carried where the member takes another, which is the only part of
/// that sentence the request decides besides the value itself.
fn json_kind(value: &Value) -> &'static str {
    match value {
        Value::Null => "Null",
        Value::Bool(_) => "Boolean",
        Value::Number(_) => "Number",
        Value::String(_) => "String",
        Value::Array(_) => "Array",
        Value::Object(_) => "Object",
    }
}

impl Call {
    /// The document the invoked operation receives as its INPUT, for a task whose state
    /// built `document`.
    ///
    /// For a `Resource` naming an INTEGRATION the two are the same thing: "the state's
    /// effective input IS that operation's request". For a `Resource` naming a Lambda
    /// FUNCTION the state's document is the EVENT the function is invoked with, so the
    /// operation's input is the invocation that carries it: the function the `Resource`
    /// named, and the document as the payload.
    #[must_use]
    pub fn operation_input(&self, document: &Value) -> Value {
        let Some(function) = self.invokes.as_deref() else {
            return document.clone();
        };
        let mut input = Map::new();
        input.insert(
            FUNCTION_NAME_MEMBER.to_string(),
            Value::String(function.to_string()),
        );
        input.insert(PAYLOAD_MEMBER.to_string(), document.clone());
        Value::Object(input)
    }

    /// How a refusal of this call NAMES the work it is: the operation and the service
    /// for a call to a service's API, and the request itself for the HTTP Task, which
    /// calls no operation of any service ([`Call::endpoint`]).
    fn described(&self) -> String {
        match self.endpoint {
            true => "the HTTP request this task makes".to_string(),
            false => format!("the '{}' call of {}", self.operation, self.api.sdk_id),
        }
    }

    /// The IAM action this call is taken under: the prefix the invoked service
    /// publishes its actions under ([`Api::action_prefix`], its ARN namespace for all
    /// but one) and the operation's own name.
    ///
    /// That is the action for every operation that publishes its permission under its
    /// own name, which is the usual case; an operation that publishes another is
    /// carried by its [`TARGET_RESOURCES`] entry ([`TargetResource::action`]).
    #[must_use]
    pub fn action(&self) -> String {
        format!(
            "{}:{}",
            self.api.action_prefix.unwrap_or(self.api.namespace),
            self.operation
        )
    }
}

/// Where an integration's request names the resource its call acts ON, which IAM
/// action that call is subject to, and how a refusal of it is reported to the `Task`
/// state.
///
/// A `Task` state's integration call is made AS the state machine's execution role,
/// so it is subject to that role's own policies — an IAM access request over an
/// `(action, resource)` pair. Both halves are the integration's own: the action is the
/// one the invoked operation publishes, which is not always the operation's own name,
/// and which member of the request names the resource, and in what form, is a fact of
/// that integration — which is why there is one entry per integration rather than a
/// rule.
#[derive(Debug)]
pub struct TargetResource {
    /// The `<namespace>:<action>` the OPTIMIZED family names this integration by.
    pub integration: &'static str,
    /// The IAM action this call is subject to, for an operation whose PUBLISHED
    /// permission is not its own name (`lambda:InvokeFunction` for `Invoke`); `None`
    /// where the action is the invoked service's namespace and the operation's name,
    /// which is the usual case ([`access_request`]).
    pub action: Option<&'static str>,
    /// The member of the integration's request that names the resource the call acts
    /// on, named in the integration's own PascalCase.
    pub member: &'static str,
    /// The resource that member names when the request OMITS it, for an operation
    /// that documents a default for it; `None` where it documents none, and a request
    /// omitting the member then names no resource at all — which is a request the
    /// invoked operation answers for itself rather than an authorization decision
    /// ([`access_request`]).
    pub default: Option<&'static str>,
    /// How that member spells the resource it names ([`TargetNaming`]).
    pub named: TargetNaming,
    /// The error the invoked operation MODELS for a call its caller is not authorized
    /// to make ([`Denied`]); `None` where that operation models none, whose refusal
    /// reaches the state under the invoked service's BASE SDK exception
    /// ([`Api::base_error`]) — the class its client deserializes an unmodelled code
    /// into.
    pub denied: Option<Denied>,
}

/// The error one operation answers a call its caller is not authorized to make with,
/// as that operation's own model declares it.
///
/// A refusal the role gate makes on the invoked service's behalf
/// ([`super::task_authorization`]) is the refusal THAT SERVICE would have answered, so
/// where the operation models an access-denied error of its own the task reports that
/// error's class and the status and code its answer carries — which is what a
/// definition's `Retry` and `Catch` match on. Every field is read from the operation's
/// own `smithy_query(query="operation")`, so this states no wire detail of its own.
#[derive(Debug, Clone, Copy)]
pub struct Denied {
    /// The SDK exception class the error shape is deserialized into, which is the
    /// error half of the name a `Task` state reports ([`Api::task_error`]).
    pub exception: &'static str,
    /// The code the answer names the class by — for an `awsQuery` service the error's
    /// own query code (`awsQueryError.code`), which is what that service's client
    /// matches on.
    pub code: &'static str,
    /// The status that refusal answers with (`httpError`).
    pub status: u16,
}

/// How the member of an integration's request that names the call's target SPELLS it.
///
/// An IAM access request is over the resource's ARN, whatever the operation's own
/// member takes: an operation that addresses its resource by ARN carries the resource
/// itself, while one that addresses it by NAME carries a name that is the ARN's last
/// field, and the ARN it names is this account's own ARN for that resource in the
/// Region the call is made in. Some services address a resource by neither — a queue by
/// its url, a function by any of the forms Lambda documents for one — and there the ARN
/// is read the way that service addresses the resource. Which of these a member is, is
/// a fact of the invoked operation's model, so it is stated per entry rather than
/// guessed from the value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TargetNaming {
    /// The member carries the resource's ARN, so it IS the resource the access
    /// request is over.
    Arn,
    /// The member carries the resource's NAME inside one account and Region, so the
    /// ARN it names is minted from it. A value already written as an ARN is the
    /// resource itself, because such a member is one the invoked operation takes
    /// either way.
    Named {
        /// The ARN namespace of the service the resource belongs to.
        service: &'static str,
        /// What precedes the name in the ARN's resource field, the resource type the
        /// invoked service writes it under (`table/`).
        prefix: &'static str,
    },
    /// The member carries a queue's `QueueUrl`, whose last path segment is the
    /// `QueueName` the queue's ARN carries
    /// (`arn:<partition>:sqs:<region>:<account>:<QueueName>`) and whose endpoint host
    /// names the Region that ARN is in.
    QueueUrl,
    /// The member carries a `FunctionName` in one of the forms Lambda documents for
    /// it: `my-function[:qualifier]`, the function's ARN, or the partial
    /// `<account>:function:my-function[:qualifier]`
    /// (`smithy_query(query="shape",shape="NamespacedFunctionName",model_path="services/
    /// service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
    /// com-amazonaws-lambda-awsgirapiservice.json")`). Each names the same function,
    /// whose ARN is what `lambda:InvokeFunction` is taken on.
    FunctionName,
}

impl TargetNaming {
    /// The ARN of the resource `named` names, as read in the Region `region` the call
    /// is made in.
    ///
    /// A value already written as an ARN is the resource itself whichever of these a
    /// member is: such a member is one the invoked operation takes either way, and the
    /// resource the access request is over is the same one. `None` for a value that
    /// names no resource of the kind at all — there is nothing to decide an access
    /// request over, which the caller leaves to the invoked operation's own refusal.
    fn arn_of(self, named: &str, region: &str) -> Option<String> {
        if named.is_empty() {
            return None;
        }
        if named.starts_with(ARN_PREFIX) {
            return Some(named.to_string());
        }
        match self {
            Self::Arn => Some(named.to_string()),
            Self::Named { service, prefix } => Some(::engine::emulator_arn(
                service,
                region,
                &format!("{prefix}{named}"),
            )),
            // A queue url is `<endpoint>/<account>/<QueueName>`, so the name is its
            // last path segment — and the queue's own Region is the one its endpoint
            // host names (`sqs.<region>.amazonaws.com`), which is not always the
            // Region the call is made in: a task may send to a queue in another one.
            // An endpoint host that names none leaves the call's own Region, which is
            // where a queue addressed through this emulator's own endpoint lives.
            Self::QueueUrl => {
                let name = named.trim_end_matches('/').rsplit('/').next()?;
                (!name.is_empty()).then(|| {
                    let endpoint = endpoint_region(named);
                    ::engine::emulator_arn(
                        SQS_NAMESPACE,
                        endpoint.as_deref().unwrap_or(region),
                        name,
                    )
                })
            }
            // Every partial form writes the function's name last, after the labels
            // the longer ones carry before it; the qualifier a form may close with
            // stays on the ARN, because a grant may be written for one version or
            // alias of a function.
            Self::FunctionName => {
                let tail = named
                    .rsplit_once(FUNCTION_RESOURCE_TYPE_PREFIX)
                    .map_or(named, |(_, tail)| tail);
                (!tail.is_empty()).then(|| {
                    ::engine::emulator_arn(
                        LAMBDA_NAMESPACE,
                        region,
                        &format!("{FUNCTION_RESOURCE_TYPE_PREFIX}{tail}"),
                    )
                })
            }
        }
    }
}

/// The Region the endpoint host of `url` names, for a url addressed to one of a
/// service's own regional endpoints (`<prefix>.<region>.<suffix>`); `None` for a url
/// whose host names no Region, which is what this emulator's own endpoint does.
///
/// The authority and the bare host come from the ONE shared reading of a url's host
/// (`service_common::data_plane_addressing`), and the label is held to the grammar every
/// model's ARN pattern states for a Region field (`service_common::arn::region_label`),
/// because it ends up in the ARN an access request is decided over. The Region is the
/// SECOND of the host's dot-separated labels — the one an AWS regional endpoint host
/// puts after the service's endpoint prefix — and a host with no label after it carries
/// no Region at all.
fn endpoint_region(url: &str) -> Option<String> {
    let authority = ::service_common::data_plane_addressing::addressed_host(url, None)?;
    let host = ::service_common::data_plane_addressing::host_only(&authority).to_string();
    let mut labels = host.split('.');
    let _prefix = labels.next()?;
    let region = labels.next()?;
    let further = labels.next().is_some();
    let shaped = ::service_common::arn::region_label(region).is_some() && region.contains('-');
    (further && shaped).then(|| region.to_string())
}

/// What every ARN opens with, which is what makes a value one rather than one of the
/// shorter forms a service also addresses a resource by.
const ARN_PREFIX: &str = "arn:";

/// The resource-type label a function's ARN and the partial forms of a `FunctionName`
/// write before the function's own name.
const FUNCTION_RESOURCE_TYPE_PREFIX: &str = "function:";

/// The integrations whose request names the resource their call acts on.
///
/// One entry per integration, because which member names the target — and how that
/// member spells it — is that integration's own fact. An entry also needs the invoked
/// service's base SDK exception, the one a refusal its operation does not model is
/// reported under, so an entry is added from that service's own model and SDK rather
/// than guessed at. An integration with no entry is one no access request is formed
/// for, and the call is made as before.
///
/// An entry is keyed by the `<namespace>:<action>` the OPTIMIZED family writes, and
/// serves BOTH families: the AWS SDK family names the same service and the same
/// action, so it makes the same call on the same resource and is subject to the same
/// access request. Only the SPELLING of the request's members differs between them,
/// which [`TargetResource::resource_of`] reads either way.
///
/// Nothing here is per ACTION beyond the entry: the IAM action an entry's call is
/// subject to is the invoked service's namespace and the operation's own name
/// ([`access_request`]) unless the entry states otherwise, which it does only where
/// the invoked operation publishes its permission under a different name, so two
/// actions of one service usually differ only in which member names their target.
pub const TARGET_RESOURCES: &[TargetResource] = &[
    // `arn:aws:states:::states:startExecution` starts an execution of the state
    // machine its `StateMachineArn` parameter names, which is the resource
    // `states:StartExecution` is taken on
    // (`smithy_query(query="operation",operation="StartExecution")` on this crate's
    // own model: `stateMachineArn` is the ARN "of the state machine to execute").
    // A refusal this operation does not model is reported under this service's own
    // base SDK exception, the one the AWS SDKs deserialize an unmodelled Step
    // Functions error into ([`Api::base_error`]).
    TargetResource {
        integration: "states:startExecution",
        action: None,
        member: "StateMachineArn",
        default: None,
        named: TargetNaming::Arn,
        denied: None,
    },
    // DynamoDB's item-level integrations each act on ONE table, the one their
    // `TableName` parameter names: "The name of the table to contain the item" /
    // "containing the requested item" / "from which to delete the item"
    // (`smithy_query(query="operation",operation="PutItem,GetItem,UpdateItem,
    // DeleteItem",model_path="services/
    // service-com-amazonaws-dynamodb-dynamodb-20120810/smithy-model/
    // com-amazonaws-dynamodb-dynamodb-20120810.json")`). The member is the table's
    // NAME rather than its ARN, and a table's ARN writes that name under the `table`
    // resource type — "The Amazon Resource Name (ARN) that uniquely identifies the
    // table" (`TableDescription$TableArn` of the same model, which that service mints
    // as `table/<name>`) — so the resource the access request is over is minted from
    // it. A refusal none of these operations models is reported under DynamoDB's base
    // SDK exception.
    TargetResource {
        integration: "dynamodb:putItem",
        action: None,
        member: "TableName",
        default: None,
        named: DYNAMODB_TABLE,
        denied: None,
    },
    TargetResource {
        integration: "dynamodb:getItem",
        action: None,
        member: "TableName",
        default: None,
        named: DYNAMODB_TABLE,
        denied: None,
    },
    TargetResource {
        integration: "dynamodb:updateItem",
        action: None,
        member: "TableName",
        default: None,
        named: DYNAMODB_TABLE,
        denied: None,
    },
    TargetResource {
        integration: "dynamodb:deleteItem",
        action: None,
        member: "TableName",
        default: None,
        named: DYNAMODB_TABLE,
        denied: None,
    },
    // `arn:aws:states:::sqs:sendMessage` sends to the queue its `QueueUrl` parameter
    // addresses, which is the queue `sqs:SendMessage` is taken on
    // (`smithy_query(query="operation",operation="SendMessage",model_path="services/
    // service-com-amazonaws-sqs-amazonsqs/smithy-model/
    // com-amazonaws-sqs-amazonsqs.json")`: `QueueUrl` is "the URL of the Amazon SQS
    // queue to which a message is sent"). Amazon SQS models no access-denied error on
    // `SendMessage`, so a refusal of it is reported under that service's base SDK
    // exception.
    TargetResource {
        integration: "sqs:sendMessage",
        action: None,
        member: "QueueUrl",
        default: None,
        named: TargetNaming::QueueUrl,
        denied: None,
    },
    // `arn:aws:states:::sns:publish` publishes to the topic its `TopicArn` parameter
    // names — "The topic you want to publish to" — which is the resource
    // `sns:Publish` is taken on
    // (`smithy_query(query="operation",operation="Publish",model_path="services/
    // service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
    // com-amazonaws-sns-amazonsimplenotificationservice.json")`). Unlike the
    // integrations above, `Publish` MODELS the refusal of an unauthorized call:
    // `AuthorizationErrorException` "indicates that the user has been denied access to
    // the requested resource", answered with HTTP 403 under the query code
    // `AuthorizationError` — so that is the error a denied publish reaches the state
    // as, rather than the service's base SDK exception.
    TargetResource {
        integration: SNS_PUBLISH_INTEGRATION,
        action: None,
        member: "TopicArn",
        // `Publish` documents no default destination: "If you don't specify a value for
        // the TopicArn parameter, you must specify a value for the PhoneNumber or
        // TargetArn parameters", so a request naming none names no topic at all.
        default: None,
        named: TargetNaming::Arn,
        denied: Some(Denied {
            exception: "AuthorizationErrorException",
            code: "AuthorizationError",
            status: 403,
        }),
    },
    // `arn:aws:states:::lambda:invoke` invokes the function its `FunctionName`
    // parameter names. The action is `lambda:InvokeFunction` — the permission Lambda
    // publishes for the `Invoke` API, which is not the operation's own name — and the
    // resource is that function's ARN. `Invoke` models no access-denied error either
    // (`smithy_query(query="operation",operation="Invoke",model_path="services/
    // service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
    // com-amazonaws-lambda-awsgirapiservice.json")`), so a refusal of it is reported
    // under Lambda's base SDK exception.
    TargetResource {
        integration: "lambda:invoke",
        action: Some("lambda:InvokeFunction"),
        member: "FunctionName",
        default: None,
        named: TargetNaming::FunctionName,
        denied: None,
    },
    // `arn:aws:states:::athena:startQueryExecution` runs its query IN a workgroup —
    // `WorkGroup` is "the name of the workgroup in which the query is being started"
    // (`smithy_query(query="operation",operation="StartQueryExecution",
    // model_path="services/service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json")`) — and a query's actions are taken on
    // that workgroup, which Athena writes under the `workgroup` resource type. A
    // request naming none runs in the account's own default workgroup: "if a workgroup
    // is not specified, [...] the primary workgroup"
    // (`ListQueryExecutionsInput$WorkGroup` of the same model), which is the name the
    // access request is then over. Athena models no access-denied error on these
    // operations, so a refusal of one is reported under that service's base SDK
    // exception.
    TargetResource {
        integration: "athena:startQueryExecution",
        action: None,
        member: "WorkGroup",
        default: Some(ATHENA_DEFAULT_WORK_GROUP),
        named: ATHENA_WORK_GROUP,
        denied: None,
    },
    // `arn:aws:states:::glue:startJobRun` runs the job its `JobName` parameter names —
    // "the name of the job definition to use", `@required`
    // (`smithy_query(query="operation",operation="StartJobRun",model_path="services/
    // service-com-amazonaws-glue-awsglue/smithy-model/com-amazonaws-glue-awsglue.json")`)
    // — and a run's actions are taken on that job, which Glue writes under the `job`
    // resource type. Glue models no access-denied error on these operations either.
    TargetResource {
        integration: "glue:startJobRun",
        action: None,
        member: "JobName",
        default: None,
        named: GLUE_JOB,
        denied: None,
    },
];

/// An Athena workgroup, named by the `WorkGroup` one of that service's query
/// operations takes.
const ATHENA_WORK_GROUP: TargetNaming = TargetNaming::Named {
    service: "athena",
    prefix: "workgroup/",
};

/// The workgroup a query runs in when the request names none: "if a workgroup is not
/// specified, a list of available query execution IDs for the queries in the primary
/// workgroup is returned" (`ListQueryExecutionsInput$WorkGroup`), the default
/// workgroup every account carries.
const ATHENA_DEFAULT_WORK_GROUP: &str = "primary";

/// A Glue job, named by the `JobName` one of that service's run operations takes.
const GLUE_JOB: TargetNaming = TargetNaming::Named {
    service: "glue",
    prefix: "job/",
};

/// A DynamoDB table, named by the table name one of its item-level operations takes.
const DYNAMODB_TABLE: TargetNaming = TargetNaming::Named {
    service: "dynamodb",
    prefix: "table/",
};

impl TargetResource {
    /// The ARN of the resource this call's `request` names, in the Region `region`
    /// the call is made in.
    ///
    /// The member is read under both spellings a definition may write it in — the
    /// integration's own PascalCase and the invoked model's ([`MemberCase`]) — because
    /// either reaches the operation ([`wire_request`]). What the value spells is the
    /// entry's own fact ([`TargetNaming`]): a NAME is the last field of this account's
    /// ARN for that resource in this Region, an ARN is the resource itself, and a url
    /// or one of a service's own shorter forms is read the way that service addresses
    /// the resource.
    fn resource_of(&self, request: &Value, region: &str) -> Option<String> {
        self.named.arn_of(&self.named_in(request)?, region)
    }

    /// How `request` ADDRESSES that resource: the member's own value, and the default
    /// the invoked operation documents for a request that OMITS it.
    ///
    /// A member the request carries as something other than text addresses no resource
    /// of any name, and the default does not stand in for it: the request named
    /// something, so what to answer for it is the invoked operation's own refusal
    /// rather than an access request over a resource nothing named.
    ///
    /// Public because the calls a run-a-job pattern makes for ITSELF are over the
    /// resource the STARTING request named, which those calls do not carry
    /// ([`RunAJob::acts_on_named`]).
    pub fn named_in(&self, request: &Value) -> Option<String> {
        match [MemberCase::Pascal, MemberCase::LowerCamel]
            .into_iter()
            .find_map(|case| request.get(case.spell(self.member)))
        {
            Some(named) => named.as_str().map(str::to_string),
            None => self.default.map(str::to_string),
        }
    }
}

/// The access request this call is subject to: the IAM action the invoked operation
/// publishes, and the ARN of the resource the request names, as addressed in the
/// Region `region` the call is made in.
///
/// `region` is the Region the call is made in, which is where a resource the request
/// named by NAME lives.
///
/// `None` when no [`TARGET_RESOURCES`] entry is carried for the integration, or when
/// the request names no resource at all — a request the invoked operation will itself
/// refuse, which is its own answer to give rather than an authorization decision.
pub fn access_request(call: &Call, request: &Value, region: &str) -> Option<(String, String)> {
    let targets = call.targets?;
    // The resource is the one the invoked operation's own INPUT names, which for a
    // `Resource` naming a function directly is not the document the state built: the
    // call is the same one on the same function either way, so it is judged on the
    // request that is actually made ([`Call::operation_input`]).
    let resource = targets.resource_of(&call.operation_input(request), region)?;
    // The action is the one the invoked service publishes for the operation
    // ([`Call::action`]), unless the entry states the permission that operation
    // PUBLISHES instead.
    let action = targets.action.map_or_else(|| call.action(), str::to_string);
    Some((action, resource))
}

/// The members an OPTIMIZED integration documents as a document while the operation
/// it calls types them as a string, named by the WIRE spelling of the member
/// ([`MemberCase`]) under the `<namespace>:<action>` the optimized family names the
/// integration by.
///
/// Only that family converts anything: an AWS SDK integration's parameters ARE the
/// invoked operation's own members, so a document written for one goes on the wire
/// as it stands.
const JSON_TEXT_MEMBERS: &[(&str, &[&str])] = &[
    // `arn:aws:states:::states:startExecution` takes the child execution's input as
    // the JSON document the child receives, while `StartExecution`'s own `input`
    // member is "the string that contains the JSON input data for the execution"
    // (`smithy_query(query="operation",operation="StartExecution")` on this crate's
    // own model).
    ("states:startExecution", &["input"]),
    // `arn:aws:states:::sqs:sendMessage` takes the message's body as the JSON document
    // the message carries, while `SendMessage`'s own `MessageBody` member is a String
    // (`smithy_query(query="operation",operation="SendMessage",model_path="services/
    // service-com-amazonaws-sqs-amazonsqs/smithy-model/com-amazonaws-sqs-amazonsqs.json")`).
    ("sqs:sendMessage", &["MessageBody"]),
];

/// The same conversion for a member the invoked operation types as a string that sits
/// NESTED inside the request, named by the path to it: each segment is a member of the
/// object reached so far, and a LIST on the way is walked element by element, because an
/// integration that takes a list of entries documents each entry the same way.
///
/// Separate from [`JSON_TEXT_MEMBERS`] only in the shape of what it names; the rule is
/// the one both state, that a member the invoked operation types as a STRING goes on the
/// wire as the JSON text of the document the state built, wherever in the request that
/// member sits.
const JSON_TEXT_AT_MEMBERS: &[(&str, &[&[&str]])] = &[
    // `arn:aws:states:::events:putEvents` takes each entry's event detail as the JSON
    // document the event carries, while `PutEventsRequestEntry$Detail` is a String
    // member documented as "a valid JSON object [...] may contain fields and nested
    // sub-objects"
    // (`smithy_query(query="shape",shape="PutEventsRequestEntry",model_path="services/
    // service-com-amazonaws-eventbridge-awsevents/smithy-model/
    // com-amazonaws-eventbridge-awsevents.json")`).
    ("events:putEvents", &[&["Entries", "Detail"]]),
];

/// The members of an integration's RESPONSE that carry an INSTANT, named by the
/// spelling the invoked model gives them.
///
/// A `Task` state reads the SDK response of its call, and the SDK renders a timestamp
/// as its own date type — whole MILLISECONDS since the epoch — while the `awsJson`
/// protocol carries one on the wire as epoch SECONDS with a fractional part. The
/// members are the invoked model's own, which this crate cannot read for another
/// service, so there is one entry per integration and a later entry is added beside
/// them rather than guessed at; an integration with no entry converts nothing.
const EPOCH_MILLIS_MEMBERS: &[(&str, &[&str])] = &[
    // `StartExecution`'s output is `executionArn` and `startDate`, "the date the
    // execution is started", a Timestamp
    // (`smithy_query(query="operation",operation="StartExecution")` on this crate's
    // own model).
    ("states:startExecution", &["startDate"]),
];

/// The suffix a `Resource` names the run-a-job integration pattern with, and the
/// revision of it that parses the job's own output
/// (`arn:aws:states:::states:startExecution.sync:2`).
const RUN_A_JOB_PATTERN: &str = "sync";
const RUN_A_JOB_PATTERN_2: &str = "sync:2";

/// The `.waitForTaskToken` pattern, whose outcome is a `SendTaskSuccess` or
/// `SendTaskFailure` from outside the execution rather than a job this emulator
/// polls.
const CALLBACK_PATTERN: &str = "waitForTaskToken";

/// Whether `resource` asks for the callback (`.waitForTaskToken`) pattern.
///
/// Read from the `Resource` alone, so it can be asked BEFORE the call it names is
/// read: a callback task carries the heartbeat members an answer from outside the
/// execution can be bounded by, which a request-response task cannot owe
/// ([`super::asl`] decides a state's members from this).
pub fn asks_for_callback(resource: &str) -> bool {
    read(resource).is_ok_and(|call| call.callback)
}

/// One run-a-job (`.sync`) service integration: the three calls the pattern is made
/// of, and where the job's identifier, status and failure sit in their documents.
///
/// The pattern itself is not in any Smithy model — it is Step Functions' own
/// behaviour over another service's API — so each entry names the operations and
/// members of the INVOKED service's model, cited on the entry. Only integrations
/// whose reporting and stopping calls this emulator can really make have an entry:
/// an integration with none is reported by [`read`] as one no job was started for,
/// rather than parked at a job nothing could poll.
#[derive(Debug)]
pub struct RunAJob {
    /// The `<service>:<action>` of the `Resource` that starts the job, as the
    /// optimized integration spells it.
    pub starts: &'static str,
    /// The member of the STARTING call's response that identifies the job.
    pub job_id: &'static str,
    /// The operation that reports the job, and the member of its request the
    /// identifier goes in.
    pub describe: &'static str,
    pub describe_member: &'static str,
    /// Where the job's status sits in the reporting call's response.
    pub status_at: &'static [&'static str],
    /// Where the document the TASK REPORTS sits in that same response: the member the
    /// integration answers with, and empty for an integration that answers with the
    /// whole response.
    ///
    /// Step Functions reports what the invoked service said about the job, and for an
    /// operation whose answer WRAPS the job's own document — Glue's `GetJobRun`, whose
    /// response carries the run under `JobRun` — what it reports is that document
    /// rather than the wrapper, so a task's `ResultPath` and a `Catch` reading the
    /// failure's cause address the job's own members (`Id`, `JobRunState`) directly.
    /// An operation whose answer IS the job's document carries no wrapper to unwrap.
    pub reports_at: &'static [&'static str],
    /// The status values that mean the job finished its work, and those that mean it
    /// ended without finishing it. A status in neither list is the job still running.
    pub succeeded: &'static [&'static str],
    pub failed: &'static [&'static str],
    /// Where the reporting call's response carries a document of the job's own as a
    /// JSON-encoded STRING — its input, its output — each of which the pattern's
    /// second revision reports as the parsed document; empty where that service
    /// carries none.
    pub json_text_at: &'static [&'static [&'static str]],
    /// The operation that stops the job, and the member of its request the
    /// identifier goes in.
    pub stop: &'static str,
    pub stop_member: &'static str,
    /// Whether the stopping call's identifier member is a LIST of identifiers, as
    /// that operation's own model types it — a batch stop takes the ids of several
    /// jobs, and the one job a task started is a batch of one.
    pub stops_in_batch: bool,
    /// The members of the STARTING call's REQUEST that the reporting and stopping
    /// calls carry BESIDE the job's identifier, because those operations do not name
    /// the job by that identifier alone: each is a member of the job's own identity
    /// rather than of the run the identifier names. Named in the integration's own
    /// PascalCase, and empty for a job its identifier names on its own.
    pub names_job_with: &'static [&'static str],
    /// Where the resource the pattern's OWN calls are taken on is named, for the
    /// access request each is subject to ([`JobTarget`]); `None` for an integration
    /// whose target resource is not stated, whose calls are then made as before.
    pub acts_on: Option<JobTarget>,
}

/// Where the resource the calls that REPORT and STOP a job are taken on is named.
///
/// Those calls are made as the execution role too, so each is subject to an access
/// request — and the resource that request is over is not in the call's own request:
/// a reporting operation names the JOB by its identifier, while IAM takes the action
/// on the resource the job belongs to. Which resource that is, is the integration's
/// own fact, so it is stated per entry.
#[derive(Debug, Clone, Copy)]
pub enum JobTarget {
    /// The job's own identifier IS the resource's ARN, so the pattern's calls are
    /// taken on the thing the starting call minted (a nested execution).
    Identifier,
    /// The job belongs to the resource the STARTING integration acts on, named by the
    /// `<namespace>:<action>` of that integration: its [`TARGET_RESOURCES`] entry says
    /// which member of the starting request names it and how (a query's workgroup, a
    /// run's job).
    Started(&'static str),
}

/// The members of the starting request that name the job, beside its own identifier
/// ([`RunAJob::names_job_with`]), as the calls that report and stop it carry them.
pub type JobNaming = ::std::collections::BTreeMap<String, String>;

impl RunAJob {
    /// The members of the starting call's `request` that the calls which REPORT and
    /// STOP the job carry beside its identifier.
    ///
    /// Each is read under both spellings a definition may write it in — the
    /// integration's own PascalCase and the invoked model's ([`MemberCase`]) — since
    /// either reaches the operation ([`wire_request`]), and is carried on under the
    /// name this entry gives it. A member the request did not carry is not invented:
    /// the invoked operation answers for a request that omits one, which is its own
    /// answer to give.
    pub fn naming(&self, request: &Value) -> JobNaming {
        let mut naming = JobNaming::new();
        for member in self.names_job_with {
            let named = [MemberCase::Pascal, MemberCase::LowerCamel]
                .into_iter()
                .find_map(|case| request.get(case.spell(member)))
                .and_then(Value::as_str);
            if let Some(value) = named {
                naming.insert((*member).to_string(), value.to_string());
            }
        }
        naming
    }

    /// How the request that STARTED the job addresses the resource the pattern's own
    /// calls are taken on ([`JobTarget`]), and `None` for an integration that states
    /// none or a request that named none.
    ///
    /// Read where the job is STARTED, because that request is where the resource is
    /// named: the calls that report and stop the job carry the job's identifier
    /// instead, so what they are authorized over is carried with the parked job
    /// ([`crate::state::JobTask::acts_on`]).
    pub fn acts_on_named(&self, request: &Value, job_id: &str) -> Option<String> {
        match self.acts_on? {
            JobTarget::Identifier => Some(job_id.to_string()),
            JobTarget::Started(integration) => TARGET_RESOURCES
                .iter()
                .find(|entry| entry.integration == integration)?
                .named_in(request),
        }
    }

    /// The ARN of that resource, as addressed in the Region `region` the pattern's
    /// calls are made in ([`TargetNaming::arn_of`]).
    pub fn acts_on_arn(&self, named: &str, region: &str) -> Option<String> {
        match self.acts_on? {
            JobTarget::Identifier => TargetNaming::Arn.arn_of(named, region),
            JobTarget::Started(integration) => TARGET_RESOURCES
                .iter()
                .find(|entry| entry.integration == integration)?
                .named
                .arn_of(named, region),
        }
    }

    /// Whether `status` is one the job ENDED on, and whether it ended having done its
    /// work: `None` while the job is still running.
    fn finished(&self, status: &str) -> Option<bool> {
        if self.succeeded.contains(&status) {
            return Some(true);
        }
        if self.failed.contains(&status) {
            return Some(false);
        }
        None
    }

    /// The value `path` names in `document`, as text.
    fn text_at(document: &Value, path: &[&str]) -> Option<String> {
        let mut node = document;
        for member in path {
            node = node.get(*member)?;
        }
        match node {
            Value::String(text) => Some(text.clone()),
            Value::Null => None,
            other => Some(other.to_string()),
        }
    }

    /// The job's status, as the reporting call's `answer` carries it.
    pub fn status_of(&self, answer: &Value) -> Option<String> {
        Self::text_at(answer, self.status_at)
    }

    /// Whether the job `answer` reports has ENDED, and with what: `Ok(answer)` for
    /// one that did its work, `Err(the cause its task fails with)` for one that did
    /// not, and `None` while it is still running.
    ///
    /// ONE document answers for the job whichever way it ended: the invoked service's
    /// own report of it. A job that did its work makes the task succeed WITH that
    /// report, and a job that ended without doing its work makes the task fail with
    /// that same report as the failure's `cause` — the JSON text of it, in the
    /// integration's own convention ([`task_result`]), because `cause` is a string.
    /// So a `Catch` reading why a job failed reads the whole of what the service said
    /// about it — its status, its identity and the error and reason it ended on —
    /// rather than one member of it picked out here.
    pub fn outcome_of(&self, answer: &Value, parsed_output: bool) -> Option<Result<Value, String>> {
        let status = self.status_of(answer)?;
        let reported = self.reported(answer, parsed_output);
        match self.finished(&status)? {
            true => Some(Ok(reported)),
            false => Some(Err(task_result(&reported).to_string())),
        }
    }

    /// The document the task reports as its result: what the reporting call said about
    /// the job — the member of its response that carries the job's own document
    /// ([`RunAJob::reports_at`]) — with each document the job carries as JSON TEXT
    /// parsed in place for the pattern's second revision.
    ///
    /// A response the wrapper is absent from is reported whole: the task answers with
    /// what the service said, so nothing is invented where the member it names is not
    /// there.
    fn reported(&self, answer: &Value, parsed_output: bool) -> Value {
        let mut reported = answer.clone();
        if parsed_output {
            for path in self.json_text_at {
                parse_in_place(&mut reported, path);
            }
        }
        let mut wrapped = &reported;
        for member in self.reports_at {
            match wrapped.get(*member) {
                Some(inner) => wrapped = inner,
                None => return reported,
            }
        }
        wrapped.clone()
    }
}

/// Replace the JSON TEXT `path` names in `document` with the document that text
/// holds, leaving `document` as it is where the path names nothing or names text
/// that is not JSON.
fn parse_in_place(document: &mut Value, path: &[&str]) {
    let Some((last, leading)) = path.split_last() else {
        return;
    };
    let mut node = document;
    for member in leading {
        match node.get_mut(*member) {
            Some(next) => node = next,
            None => return,
        }
    }
    let parsed = node
        .get(*last)
        .and_then(Value::as_str)
        .and_then(|text| ::serde_json::from_str::<Value>(text).ok());
    if let (Some(parsed), Some(object)) = (parsed, node.as_object_mut()) {
        object.insert((*last).to_string(), parsed);
    }
}

/// The run-a-job integrations this emulator performs.
///
/// One entry per `<service>:<action>` that STARTS a job, naming the calls that
/// report and stop it. A `.sync` `Resource` outside this list starts nothing: see
/// [`read`].
pub const RUN_A_JOB: &[RunAJob] = &[
    // A nested execution of a state machine, through this service's own API:
    // `StartExecution` answers `executionArn`, `DescribeExecution` reports `status`
    // (`ExecutionStatus`: RUNNING, SUCCEEDED, FAILED, TIMED_OUT, ABORTED) beside
    // `error`, `cause` and the string `output`, and `StopExecution` stops it
    // (`smithy_query(query="operation",operation="StartExecution,DescribeExecution,
    // StopExecution")` on this crate's own model).
    RunAJob {
        starts: "states:startExecution",
        job_id: "executionArn",
        describe: "DescribeExecution",
        describe_member: "executionArn",
        status_at: &["status"],
        // `DescribeExecution`'s answer IS the execution's own document, so there is no
        // wrapper to unwrap.
        reports_at: &[],
        succeeded: &[super::EXECUTION_STATUS_SUCCEEDED],
        failed: &[
            super::EXECUTION_STATUS_FAILED,
            super::EXECUTION_STATUS_TIMED_OUT,
            super::EXECUTION_STATUS_ABORTED,
        ],
        // `DescribeExecution` carries both the execution's `input` and its `output`
        // as "the string that contains the JSON input data" and the JSON output of
        // the execution.
        json_text_at: &[&["input"], &["output"]],
        stop: "StopExecution",
        stop_member: "executionArn",
        stops_in_batch: false,
        // `DescribeExecution` and `StopExecution` each name the execution by its own
        // ARN alone.
        names_job_with: &[],
        // Both of those calls are taken on the EXECUTION the start minted, whose ARN
        // is the identifier it answered.
        acts_on: Some(JobTarget::Identifier),
    },
    // An Athena query: `StartQueryExecution` answers `QueryExecutionId`,
    // `GetQueryExecution` reports `QueryExecution.Status.State`
    // (`QueryExecutionState`: QUEUED, RUNNING, SUCCEEDED, FAILED, CANCELLED) beside
    // `StateChangeReason`, and `StopQueryExecution` stops it
    // (`smithy_query(query="operation",operation="StartQueryExecution,
    // GetQueryExecution,StopQueryExecution",model_path="services/
    // service-com-amazonaws-athena-amazonathena/smithy-model/
    // com-amazonaws-athena-amazonathena.json")`).
    RunAJob {
        starts: "athena:startQueryExecution",
        job_id: "QueryExecutionId",
        describe: "GetQueryExecution",
        describe_member: "QueryExecutionId",
        status_at: &["QueryExecution", "Status", "State"],
        // `GetQueryExecution` answers `{"QueryExecution": …}`, which is the document
        // this integration reports — the task's result carries the wrapper, as the
        // service's own answer does.
        reports_at: &[],
        succeeded: &["SUCCEEDED"],
        failed: &["FAILED", "CANCELLED"],
        json_text_at: &[],
        stop: "StopQueryExecution",
        stop_member: "QueryExecutionId",
        stops_in_batch: false,
        // `GetQueryExecution` and `StopQueryExecution` each name the query by its own
        // id alone.
        names_job_with: &[],
        // Both of those calls are taken on the WORKGROUP the query runs in, which the
        // starting request named ([`TARGET_RESOURCES`]).
        acts_on: Some(JobTarget::Started("athena:startQueryExecution")),
    },
    // A Glue job run: `StartJobRun` answers `JobRunId`, `GetJobRun` reports
    // `JobRun.JobRunState` (`JobRunState`: STARTING, RUNNING, STOPPING, STOPPED,
    // SUCCEEDED, FAILED, TIMEOUT, ERROR, WAITING, EXPIRED) beside
    // `JobRun.ErrorMessage`, and `BatchStopJobRun` stops it. Both of those name the
    // run by the `JobName` of the job it is a run of — `@required` on each of them —
    // beside the run's own id, which `GetJobRun` takes as `RunId` and
    // `BatchStopJobRun` as the `JobRunIds` LIST
    // (`smithy_query(query="operation",operation="StartJobRun,GetJobRun,
    // BatchStopJobRun",model_path="services/service-com-amazonaws-glue-awsglue/
    // smithy-model/com-amazonaws-glue-awsglue.json")`).
    RunAJob {
        starts: "glue:startJobRun",
        job_id: "JobRunId",
        describe: "GetJobRun",
        describe_member: "RunId",
        status_at: &["JobRun", "JobRunState"],
        // `GetJobRun` answers `{"JobRun": …}` and this integration reports the RUN
        // itself: the task's result carries the run's own members (`Id`, `JobName`,
        // `JobRunState`, `ErrorMessage`, the instants and the capacity) at its top
        // level, which is what a `ResultPath` of such a task addresses and what the
        // failure's cause carries for a run that ended without doing its work.
        reports_at: &["JobRun"],
        succeeded: &["SUCCEEDED"],
        // The `JobRunState` values a run never moves out of again; `STARTING`,
        // `RUNNING`, `STOPPING` and `WAITING` each still owe a further state.
        failed: &["STOPPED", "FAILED", "TIMEOUT", "ERROR", "EXPIRED"],
        json_text_at: &[],
        stop: "BatchStopJobRun",
        stop_member: "JobRunIds",
        stops_in_batch: true,
        names_job_with: &["JobName"],
        // Both of those calls are taken on the JOB the run belongs to, which the
        // starting request named ([`TARGET_RESOURCES`]).
        acts_on: Some(JobTarget::Started("glue:startJobRun")),
    },
];

/// What a task's own work failed with: the error NAME the state reports (which its
/// `Retry` and `Catch` are matched against) and the cause that says what happened.
#[derive(Debug, Clone)]
pub struct Failed {
    pub error: String,
    pub cause: String,
}

/// What one integration call answered: the response document, which becomes the
/// state's result, or the failure the state reports.
pub type Answer = Result<Value, Failed>;

/// What a `SendTaskSuccess` or `SendTaskFailure` answered a callback task's token with:
/// the `output` a success reported, or the `error` and `cause` a failure named — each
/// member exactly as that call carried it, and absent where it named none.
pub type Replied = Result<Value, super::asl::Failure>;

/// Where a walk records the HANDOVER of a callback (`.waitForTaskToken`) token, and
/// reads back what a `SendTask*` answered it with while the call carrying it was still
/// in flight.
///
/// The token answers its task from the instant the call carries it, which is BEFORE that
/// call comes back: the work the token is handed to reports its outcome through
/// `SendTaskSuccess`/`SendTaskFailure` from inside that very call — which is how the
/// Lambda callback pattern is written — so the token has to be answerable then. The
/// handover is what makes it so: the operation serving the answer records it against the
/// handover and tells its caller the task is answered, and the walk that handed the token
/// over reads that answer when its call comes back, so the execution is continued ONCE,
/// by the request that owns the walk.
///
/// The records live in this service's own store and are reached through the engine the
/// operation holds ([`super::callbacks`]): the walk knows nothing of either and only asks.
pub trait CallbackHandovers: Send + Sync {
    /// Record that the work of `state_name` in `execution_arn` has been handed `token`,
    /// so a `SendTask*` quoting it is accepted from now on.
    fn handed_over(&self, token: &str, execution_arn: &str, state_name: &str);

    /// What a `SendTask*` answered `token` with while its call was in flight, and the END
    /// of the handover either way: after this the token is answered through the task the
    /// store records for it.
    fn settled(&self, token: &str) -> Option<Replied>;
}

/// See [`CallbackHandovers`].
pub type Handovers = ::std::sync::Arc<dyn CallbackHandovers>;

/// The Map Run a Distributed Map state of the execution being walked is ALREADY
/// running, for a walk that CONTINUES an attempt rather than beginning one.
///
/// "To redrive a workflow that includes a Distributed Map state whose Map Run failed,
/// you must redrive the parent workflow", and a redrive "preserves the results and
/// execution history of the successful steps, and doesn't rerun these steps": the Map
/// state is entered again, but it is the SAME Map Run — it keeps the ARN a client is
/// holding — and the items whose child workflow execution already succeeded are not
/// run a second time.
#[derive(Debug, Clone)]
pub struct ResumedMapRun {
    /// The Map Run's own ARN, which this attempt goes on reporting under.
    pub arn: String,
    /// What the attempt being continued ran each item as, by item index: the child
    /// workflow execution that item SUCCEEDED as, and `None` for an item that failed
    /// or was never started — the ones this attempt runs again.
    pub items: Vec<Option<FinishedChild>>,
}

/// One child workflow execution an earlier attempt of a Distributed Map state's item
/// SUCCEEDED as, as the store holds it.
///
/// It is what the item contributes to this attempt without being run: its result
/// stands as the iteration's own, and the execution record itself is left untouched,
/// so the per-item work it did is not repeated.
#[derive(Debug, Clone)]
pub struct FinishedChild {
    pub name: String,
    pub arn: String,
    /// "The string that contains the JSON input data" that execution was started
    /// with, and the output it succeeded with.
    pub input: String,
    pub output: Option<String>,
    /// The instants that execution really ran between, as its own record carries them:
    /// it is not run again, so those are the dates it goes on reporting.
    pub start_date: ::std::time::SystemTime,
    pub stop_date: ::std::time::SystemTime,
    /// "The total number of items processed in a child workflow execution", as that
    /// record carries it.
    pub item_count: i32,
}

/// What answers [`Tasks::map_run`]: the Map Run the Distributed Map state labelled
/// `label` — the `occurrence`-th of this walk's entries into a state with that label —
/// is already running, or `None` where it is starting a new one.
///
/// A closure rather than a trait, for the same reason [`RoleGate`] is one: what it
/// answers lives in this service's own store and is reached through the engine the
/// operation holds ([`super::map_runs`]), and the walk knows nothing of either.
pub type MapRuns =
    ::std::sync::Arc<dyn Fn(usize, &str) -> Option<ResumedMapRun> + Send + Sync>;

/// What performs a `Task` state's service integration for one walk.
///
/// The walk is synchronous — it runs on a blocking thread and its states are a
/// function of the execution's own data — while an API call is asynchronous, so the
/// walk asks for a call through this seam and the asynchronous operation running it
/// ([`interpret`]) makes it.
pub trait Tasks: Send + Sync {
    /// Make `call` with `request` as the operation's input document, waiting at most
    /// `within` for it to answer.
    ///
    /// `within` is the state's own `TimeoutSeconds` when it declares one — "if the
    /// task runs longer than the specified seconds, this state fails with a
    /// States.Timeout error name" — so the member bounds the span it documents
    /// rather than only being compared with it afterwards, clamped by what the run may
    /// still spend waiting ([`super::asl::MAX_CALL_WAIT_DURATION`]). `None` leaves the
    /// emulator's own ceiling ([`MAX_CALL_DURATION`]) as the only bound, and a
    /// declared timeout longer than that ceiling does not lift it.
    fn perform(&self, call: &Call, request: &Value, within: Option<Duration>) -> Answer;

    /// Record that this walk has handed `token` to the work `call` performs, over the
    /// state `state_name` of the execution `execution_arn` ([`CallbackHandovers`]).
    ///
    /// Asked BEFORE the call is made, because the token travels WITH it. The default
    /// records nothing, for a walk with no store to record it in — which leaves the token
    /// answerable from the moment the store records the suspended task, as it was before.
    fn hand_over(&self, _token: &str, _execution_arn: &str, _state_name: &str) {}

    /// What a `SendTask*` answered `token` with while the call carrying it was in flight,
    /// and the end of that handover ([`CallbackHandovers::settled`]).
    ///
    /// Asked once the call has come back: an answer here is the task's own outcome, so the
    /// walk is finished at this state instead of suspending at it. The default answers
    /// nothing, which suspends.
    fn settle_handover(&self, _token: &str) -> Option<Replied> {
        None
    }

    /// Why the execution role this walk runs as may NOT make `call` with `request`,
    /// or `None` when nothing of the kind can be shown.
    ///
    /// A `Task` state's integration call is made as the state machine's execution
    /// role, so a call that role's policies do not allow never happens: the walk asks
    /// this BEFORE [`Tasks::perform`] and fails the task with what this answers. The
    /// default shows nothing, for a walk with no role to judge the call against.
    fn authorize(&self, _call: &Call, _request: &Value) -> Option<Failed> {
        None
    }

    /// The Map Run the Distributed Map state labelled `label` is ALREADY running
    /// ([`ResumedMapRun`]), for the `occurrence`-th entry this walk makes into a state
    /// with that label — one Distributed Map nested inside another is entered once per
    /// outer item, and each of those entries is a Map Run of its own.
    ///
    /// Asked as the state is entered, because what it answers decides both the ARN the
    /// Map Run reports and which of the state's items this attempt runs. The default
    /// answers nothing, which is what a walk that BEGINS an attempt does: every item
    /// runs, under a Map Run of its own.
    fn map_run(&self, _occurrence: usize, _label: &str) -> Option<ResumedMapRun> {
        None
    }
}

/// A [`Tasks`] that makes no call, for a walk with no operation around it to make
/// one (`TestState`, which stands a mock in for the invocation instead).
#[derive(Debug, Default)]
pub struct Unserved;

impl Tasks for Unserved {
    fn perform(&self, call: &Call, _request: &Value, _within: Option<Duration>) -> Answer {
        Err(Failed {
            error: super::asl::RUNTIME_ERROR.to_string(),
            cause: format!(
                "{} is not made outside an execution, so nothing was run for this task",
                call.described()
            ),
        })
    }
}

/// The integration call `resource` names, or why no call is made for it.
///
/// The `Err` is the cause an execution that reaches such a state fails with: it says
/// what the `Resource` named and what about it was not run, and never claims the
/// work happened.
pub fn read(resource: &str) -> Result<Call, String> {
    let shown = super::asl::echoed(resource);
    // A LAMBDA FUNCTION's ARN names no integration token: it names the function, and
    // the task's work is one synchronous invocation of it (`direct_invocation`). It is
    // the first question asked, because it is the one reading of a `Resource` that needs
    // no integration field at all.
    if names_a_function(resource).is_some() {
        return direct_invocation(resource, &shown);
    }
    let fields: Vec<&str> = resource.splitn(6, ':').collect();
    let (vendor, rest) = match fields.as_slice() {
        ["arn", _partition, vendor, _region, _account, rest] => (*vendor, *rest),
        _ => {
            return Err(format!(
                "'{shown}' is not an ARN, so it names no service integration and nothing was run \
                 for it"
            ))
        }
    };
    if vendor != STATES_NAMESPACE {
        return Err(format!(
            "'{shown}' names a resource of '{}' rather than a service integration, which this \
             emulator does not invoke directly, so nothing was run for it",
            super::asl::echoed(vendor)
        ));
    }
    // `aws-sdk:<service>:<action>` for the AWS SDK family, `<service>:<action>` for
    // the optimized one: both name a service and an action, which is all the call
    // needs.
    let (aws_sdk, (service, action)) = match rest.split_once(':') {
        Some((AWS_SDK_SEGMENT, tail)) => (
            true,
            tail.split_once(':').ok_or_else(|| {
                format!("'{shown}' names no action of its service, so nothing was run for it")
            })?,
        ),
        Some(pair) => (false, pair),
        None => {
            return Err(format!(
                "'{shown}' names no action of its service, so nothing was run for it"
            ))
        }
    };
    if service == super::ACTIVITY_RESOURCE {
        return Err(format!(
            "'{shown}' names an activity, whose work is handed to a worker that polls \
             'GetActivityTask' for it rather than being an integration call, so no call was \
             made and nothing was run for it"
        ));
    }
    // Step Functions' own HTTP Task names no other service: the work is one HTTP
    // request the state's parameters describe ([`super::http_task`]).
    if service == HTTP_NAMESPACE {
        return http_invoke(&shown, action);
    }
    // "<action>.sync", "<action>.sync:2" and "<action>.waitForTaskToken" name an
    // integration PATTERN after the action: the action is what is called either way,
    // and the pattern says what finishes the task.
    let (action, pattern) = match action.split_once('.') {
        Some((action, pattern)) => (action, Some(pattern)),
        None => (action, None),
    };
    if action.is_empty() {
        return Err(format!(
            "'{shown}' names no action of its service, so nothing was run for it"
        ));
    }
    // Each family names the service by its OWN token: the optimized one by the
    // service's ARN namespace, the AWS SDK one by the name the SDKs give it
    // ([`Api::sdk_token`]).
    //
    // S3 is reached through the AWS SDK family ALONE: its wire facts are stated
    // apart from [`APIS`] ([`S3_API`]) because the service publishes no optimized
    // integration of its own, so `arn:aws:states:::s3:<action>` names no `Task`
    // state's work — it is the `Resource` of a `Map` state's `ItemReader` and
    // `ResultWriter`, which read and write their dataset through
    // [`dataset_call`] instead.
    let api = match aws_sdk && service == S3_API.sdk_token() {
        true => {
            let named = operation_of(action);
            if !S3_SDK_OPERATIONS.contains(&named.as_str()) {
                return Err(format!(
                    "'{shown}' names the '{named}' operation of {}, whose answer this emulator \
                     does not read as that operation's own output members, so nothing was run \
                     for it",
                    S3_API.sdk_id
                ));
            }
            &S3_API
        }
        false => APIS
            .iter()
            .find(|api| match aws_sdk {
                true => api.sdk_token() == service,
                false => api.namespace == service,
            })
            .ok_or_else(|| {
                format!(
                    "'{shown}' names an integration with '{}', which is not one of the services \
                     this emulator makes a 'Task' state's integration call to, so nothing was \
                     run for it",
                    super::asl::echoed(service)
                )
            })?,
    };
    // The run-a-job pattern is performed for the integrations [`RUN_A_JOB`] names
    // the reporting and stopping calls of; the callback pattern waits for a
    // `SendTaskSuccess` nothing inside the execution makes.
    let (job, parsed_output, callback) = match pattern {
        None => (None, false, false),
        Some(pattern @ (RUN_A_JOB_PATTERN | RUN_A_JOB_PATTERN_2)) => {
            let starts = format!("{service}:{action}");
            let job = RUN_A_JOB
                .iter()
                .find(|job| job.starts == starts)
                .ok_or_else(|| {
                    format!(
                        "'{shown}' asks for the run-a-job ('{}') pattern of an integration whose \
                         job this emulator cannot report or stop, so no job was started and \
                         nothing was run for it",
                        super::asl::echoed(pattern)
                    )
                })?;
            (Some(job), pattern == RUN_A_JOB_PATTERN_2, false)
        }
        // The callback pattern's call is the same call the request-response form
        // makes; what differs is that its answer does not finish the task.
        Some(pattern) if pattern == CALLBACK_PATTERN => (None, false, true),
        Some(pattern) => {
            return Err(format!(
                "'{shown}' names '{}', which is not one of this service's integration patterns, \
                 so nothing was run for it",
                super::asl::echoed(pattern)
            ))
        }
    };
    // The conversion a member of this call's request needs is the integration's,
    // so it is read off the optimized `<namespace>:<action>` and is empty for the
    // AWS SDK family, whose parameters are the operation's own members.
    let integration = format!("{}:{action}", api.namespace);
    let json_text = match aws_sdk {
        true => &[][..],
        false => members_of(JSON_TEXT_MEMBERS, &integration),
    };
    // The same for a member of this request that is nested inside it.
    let json_text_at = match aws_sdk {
        true => &[][..],
        false => paths_of(JSON_TEXT_AT_MEMBERS, &integration),
    };
    // The same for the instants its RESPONSE carries: the optimized family's result
    // is the SDK response, which renders a date as epoch milliseconds.
    let epoch_millis = match aws_sdk {
        true => &[][..],
        false => members_of(EPOCH_MILLIS_MEMBERS, &integration),
    };
    let operation = operation_of(action);
    // A REST service binds each operation's members to the path, the query, the
    // headers and one payload separately, so a call is made only for an operation
    // whose bindings this emulator carries; one with none is reported as work nothing
    // was run for rather than sent as a request its protocol does not describe.
    let rest = match api.wire {
        // An AWS JSON service's every operation takes the same request shape, and a
        // FORM one's is built from the action and the input's own members.
        Wire::Json { .. } | Wire::Query { .. } => None,
        Wire::Rest(_) => Some(api.rest(&operation).ok_or_else(|| {
            format!(
                "'{shown}' names the '{operation}' operation of {}, whose request this emulator \
                 does not bind to that service's REST protocol, so nothing was run for it",
                api.sdk_id
            )
        })?),
    };
    Ok(Call {
        api,
        operation,
        rest,
        job,
        callback,
        parsed_output,
        json_text,
        json_text_at,
        // What a form field names the parts of a map member by is the INVOKED model's
        // own fact, so it holds for both families: the entry is read for each.
        form_maps: form_maps_of(&integration),
        // The OPTIMIZED family's result is the literal SDK response; the AWS SDK
        // family's is the invoked operation's own members.
        sdk_envelope: !aws_sdk,
        epoch_millis,
        // What the call acts ON is the same resource whichever family named it, so
        // the entry is read for both.
        targets: TARGET_RESOURCES
            .iter()
            .find(|entry| entry.integration == integration),
        // Only a `Resource` that names a function itself invokes one directly; this
        // one named an integration, whose request is the document the state built.
        invokes: None,
        endpoint: false,
        // The request IS the document this state built, so its payload member carries
        // the text of the value that document holds for it.
        payload_document: true,
    })
}

/// The AWS SDK call the operation `operation` of the service `service` names, for a
/// caller that names an API operation the way this service's AWS SDK integration family
/// does.
///
/// `service` is the token the AWS SDKs give the service ([`Api::sdk_token`]) — its SDK
/// service id, which is not always the endpoint prefix its API answers on (`sfn` for AWS
/// Step Functions, whose endpoint and signing name are `states`) — and `operation` is
/// that SDK's own name for the operation, in the lowerCamelCase the family writes
/// (`startExecution`). The reading is [`read`]'s, over the `Resource` form that family
/// states, so the wire facts of every service this emulator calls are stated ONCE
/// ([`APIS`]) however the caller named the operation.
///
/// Public because this service is not the only one whose target names an API operation
/// rather than a resource: Amazon EventBridge Scheduler's UNIVERSAL target
/// (`arn:aws:scheduler:::aws-sdk:<service>:<operation>`) names the same service and the
/// same operation, and resolves it here rather than carrying a second copy of those
/// facts. The `Err` says what was named and what about it is not called, for a caller
/// that reports work nothing was run for.
pub fn aws_sdk_call(service: &str, operation: &str) -> Result<Call, String> {
    // The family's own `Resource` form, which [`read`] is the one reading of; the
    // partition and the empty Region and account fields are the form's, and no field of
    // it is read as an address.
    read(&format!(
        "arn:aws:{STATES_NAMESPACE}:::{AWS_SDK_SEGMENT}:{service}:{operation}"
    ))
}

/// The HTTP Task `arn:aws:states:::http:invoke` names: one HTTP request to the endpoint
/// the state's own parameters name, made and reported by [`super::http_task`].
///
/// `action` is everything the `Resource` wrote after the `http` kind, so an action that
/// is not the one this integration has — including the integration PATTERNS written
/// after one (`.sync`, `.waitForTaskToken`), which this integration does not take —
/// names work no request is made for rather than a request assembled out of it.
///
/// `shown` is the `Resource` as a cause may echo it ([`super::asl::echoed`]).
fn http_invoke(shown: &str, action: &str) -> Result<Call, String> {
    if action != HTTP_INVOKE_ACTION {
        return Err(format!(
            "'{shown}' names '{}' of this service's HTTP Task, which performs only its \
             '{HTTP_INVOKE_ACTION}' action, so nothing was run for it",
            super::asl::echoed(action)
        ));
    }
    let api = APIS
        .iter()
        .find(|api| api.namespace == STATES_NAMESPACE)
        .ok_or_else(|| {
            format!("'{shown}' names this service's HTTP Task, which this emulator does not make")
        })?;
    Ok(Call {
        // `http:invoke` is Step Functions' OWN integration, served by Step Functions
        // itself, so the entry is this service's — and none of its wire facts is read
        // for this call, whose request is the task's parameters rather than an
        // operation's input ([`Call::endpoint`]).
        api,
        operation: HTTP_INVOKE_ACTION.to_string(),
        // Every one of these belongs to a call on a service's operation, and this call
        // is on none: there is no REST binding to build a request with, no job to park
        // at, no token to wait for, no member to convert, no SDK response to wrap and
        // no resource of another service the call acts on.
        rest: None,
        job: None,
        callback: false,
        parsed_output: false,
        json_text: &[],
        json_text_at: &[],
        form_maps: &[],
        sdk_envelope: false,
        epoch_millis: &[],
        targets: None,
        invokes: None,
        endpoint: true,
        // No operation's input is built for this call at all ([`Call::endpoint`]), so
        // there is no payload member to put on the wire.
        payload_document: false,
    })
}

/// The call a `Resource` that is a Lambda FUNCTION's ARN names: one synchronous
/// invocation of that function, with the state's effective input as the event it
/// carries ([`Call::operation_input`]).
///
/// It is the same call on the same resource as the optimized `lambda:invoke`
/// integration makes, so it is bound to the wire by that operation's own bindings
/// ([`LAMBDA_REST`]) and subject to the same access request
/// (`lambda:InvokeFunction` on the function's ARN, [`TARGET_RESOURCES`]). What differs
/// is the two ends a definition sees: the request is built from the function the
/// `Resource` named rather than from a `FunctionName` parameter, and the result is the
/// function's own payload rather than the SDK response around it.
///
/// `shown` is the `Resource` as a cause may echo it ([`super::asl::echoed`]).
fn direct_invocation(function: &str, shown: &str) -> Result<Call, String> {
    let unserved = || {
        format!(
            "'{shown}' names a Lambda function whose invocation this emulator does not bind to \
             that service's REST protocol, so nothing was run for it"
        )
    };
    let api = APIS
        .iter()
        .find(|api| api.namespace == LAMBDA_NAMESPACE)
        .ok_or_else(unserved)?;
    let rest = api.rest(LAMBDA_INVOKE_OPERATION).ok_or_else(unserved)?;
    Ok(Call {
        api,
        operation: LAMBDA_INVOKE_OPERATION.to_string(),
        rest: Some(rest),
        // A function ARN asks for no integration pattern: the invocation IS the task's
        // whole work, so there is no job to park at, no callback to wait for and no
        // member to convert.
        job: None,
        callback: false,
        parsed_output: false,
        json_text: &[],
        json_text_at: &[],
        form_maps: &[],
        // The result of this form is the function's own payload, not the SDK response
        // the optimized integration reports ([`reported_result`]).
        sdk_envelope: false,
        epoch_millis: &[],
        targets: TARGET_RESOURCES
            .iter()
            .find(|entry| entry.integration == LAMBDA_INVOKE_INTEGRATION),
        invokes: Some(function.to_string()),
        endpoint: false,
        // The function's EVENT is the document this state built
        // ([`Call::operation_input`]), and the payload member carries that document's
        // own text.
        payload_document: true,
    })
}

/// The members `table` names for the integration `integration`, and none when it
/// names it at all.
fn members_of(
    table: &'static [(&'static str, &'static [&'static str])],
    integration: &str,
) -> &'static [&'static str] {
    table
        .iter()
        .find(|(named, _)| *named == integration)
        .map_or(&[][..], |(_, members)| *members)
}

/// The PATHS `table` names for `integration`, and none for one with no entry — the
/// nested form of [`members_of`].
fn paths_of(
    table: &'static [(&'static str, &'static [&'static [&'static str]])],
    integration: &str,
) -> &'static [&'static [&'static str]] {
    table
        .iter()
        .find(|(named, _)| *named == integration)
        .map_or(&[][..], |(_, paths)| *paths)
}

/// The MAP members [`FORM_MAPS`] names for `integration`, and none for one with no
/// entry — the same reading as [`members_of`], over what a form field names the parts
/// of a map by.
fn form_maps_of(integration: &str) -> &'static [FormMap] {
    FORM_MAPS
        .iter()
        .find(|(named, _)| *named == integration)
        .map_or(&[][..], |(_, maps)| *maps)
}

/// The call that reports the job `job` started under `job_id`, and its request.
///
/// `naming` is what the starting call's own request said the job belongs to
/// ([`RunAJob::naming`]), which this call carries beside the identifier for an
/// integration whose reporting operation needs it.
pub fn describing(
    api: &'static Api,
    job: &'static RunAJob,
    job_id: &str,
    naming: &JobNaming,
) -> (Call, Value) {
    (
        pattern_call(api, job.describe),
        job_request(
            job.describe_member,
            Value::String(job_id.to_string()),
            naming,
        ),
    )
}

/// The call that STOPS the job `job` started under `job_id`, and its request.
///
/// `naming` is as [`describing`]'s. The identifier goes in as the stopping operation
/// types that member: a batch stop takes a LIST of ids, and the one job a task started
/// is a batch of one ([`RunAJob::stops_in_batch`]).
pub fn stopping(
    api: &'static Api,
    job: &'static RunAJob,
    job_id: &str,
    naming: &JobNaming,
) -> (Call, Value) {
    let identifier = match job.stops_in_batch {
        true => Value::Array(vec![Value::String(job_id.to_string())]),
        false => Value::String(job_id.to_string()),
    };
    (
        pattern_call(api, job.stop),
        job_request(job.stop_member, identifier, naming),
    )
}

/// The request one of a run-a-job pattern's own calls is made with: the job's
/// identifier under `member`, beside every member of the starting request that names
/// the job it belongs to.
fn job_request(member: &str, identifier: Value, naming: &JobNaming) -> Value {
    let mut request = Map::new();
    request.insert(member.to_string(), identifier);
    for (member, value) in naming {
        request.insert(member.clone(), Value::String(value.clone()));
    }
    Value::Object(request)
}

/// The call one of a run-a-job pattern's own calls — or one this service makes for work
/// of its own ([`own_call`]) — is made as.
///
/// The job's REPORT and its STOP are READ, not reported as a task's own result, so
/// neither parks at a job of its own, converts a member of its answer or carries the
/// integration's response envelope; a call this service makes for itself reports its
/// answer to nobody at all, so it carries none of them either.
fn pattern_call(api: &'static Api, operation: &str) -> Call {
    Call {
        api,
        operation: operation.to_string(),
        // Every run-a-job pattern this emulator performs is over an AWS JSON service
        // ([`RUN_A_JOB`]), so neither of its own calls carries a REST binding.
        rest: None,
        job: None,
        callback: false,
        parsed_output: false,
        json_text: &[],
        json_text_at: &[],
        form_maps: &[],
        sdk_envelope: false,
        epoch_millis: &[],
        targets: None,
        invokes: None,
        endpoint: false,
        // Every such call is to an AWS JSON service, whose request body IS the
        // operation's input document: no payload member is bound for one.
        payload_document: false,
    }
}

/// The call this service makes to `operation` of the service whose ARN namespace is
/// `namespace`, for work of its OWN rather than for a `Task` state.
///
/// Step Functions reaches another service on its own account too — it publishes an
/// execution's status changes onto the account's event bus ([`super::events`]) — and such
/// a call is the same in-process, signed request a task's call is, built from the same
/// one table of wire facts ([`APIS`]). What differs is only that no `Resource` named it:
/// there is no integration pattern, no result to convert and no response envelope to
/// carry, because nothing reports the answer as a state's result.
///
/// `None` for a namespace [`APIS`] states no wire facts for, which is a service this
/// crate makes no call to at all.
pub fn own_call(namespace: &str, operation: &str) -> Option<Call> {
    let api = APIS.iter().find(|api| api.namespace == namespace)?;
    Some(pattern_call(api, operation))
}

/// The operation name an action names: an action is the operation's name in
/// lowerCamel (`putItem` -> `PutItem`), so the operation is the action with its
/// first character upper-cased and nothing else changed.
fn operation_of(action: &str) -> String {
    let mut characters = action.chars();
    match characters.next() {
        Some(first) => first.to_uppercase().chain(characters).collect(),
        None => String::new(),
    }
}

/// How a `Task` state's own history events name the work its `Resource` asked for:
/// "the service name of the resource in a task state" and "the action of the resource
/// called by a task state"
/// (`smithy_query(query="shape",shape="TaskScheduledEventDetails")`).
///
/// A `Resource` names the integration as `<kind>:<action>`, where the kind is the
/// service alone for the optimized family (`lambda:invoke`) and the family plus the
/// service for the AWS SDK one (`aws-sdk:dynamodb:putItem`): the KIND is read from
/// the FRONT, so the action is everything after it — which is what a `Resource`
/// naming an integration PATTERN carries, since a pattern's own revision is written
/// with a colon (`startExecution.sync:2`) and belongs to the action rather than
/// being a field of its own. A `Resource` with no action at all is its own kind,
/// which is what a state naming work no call is made for reports.
pub fn named_work(resource: &str) -> (String, String) {
    let rest = match resource.splitn(6, ':').collect::<Vec<&str>>().as_slice() {
        ["arn", _partition, _vendor, _region, _account, rest] => (*rest).to_string(),
        _ => resource.to_string(),
    };
    // The AWS SDK family's kind is two fields (`aws-sdk:<service>`), every other
    // kind is one, and the action is the rest of the text either way.
    let split = match rest.split_once(':') {
        Some((AWS_SDK_SEGMENT, tail)) => tail
            .split_once(':')
            .map(|(service, action)| (format!("{AWS_SDK_SEGMENT}:{service}"), action.to_string())),
        Some((kind, action)) => Some((kind.to_string(), action.to_string())),
        None => None,
    };
    let (kind, action) = match split {
        Some((kind, action)) if !kind.is_empty() && !action.is_empty() => (kind, action),
        _ => (rest.clone(), rest),
    };
    (clip_name(kind), clip_name(action))
}

/// The LAMBDA FUNCTION a `Resource` names DIRECTLY, and `None` for every `Resource`
/// that names an integration instead.
///
/// The ONE statement of that reading: a `Resource` whose ARN is a function of the
/// Lambda namespace makes the task's unit of work one invocation of that function
/// ([`direct_invocation`]), which is what decides both the call the task makes and the
/// FAMILY of history events its lifecycle is recorded under
/// (`super::asl`'s work-event families). The function is answered as the `Resource`
/// named it, because that ARN is what the invocation addresses and what the history
/// reports.
#[must_use]
pub fn names_a_function(resource: &str) -> Option<&str> {
    match resource.splitn(6, ':').collect::<Vec<&str>>().as_slice() {
        ["arn", _partition, vendor, _region, _account, rest]
            if *vendor == LAMBDA_NAMESPACE && rest.starts_with(FUNCTION_RESOURCE_TYPE_PREFIX) =>
        {
            Some(resource)
        }
        _ => None,
    }
}

/// `name` within the bytes the modelled `Name` shape (len 1..80) carries, cut at a
/// character boundary: a `Resource` is a caller's text, so what it names is reported
/// through the member's own bound rather than past it.
fn clip_name(name: String) -> String {
    clip(name, MAX_NAME_BYTES)
}

/// `arn` within the bytes the modelled `Arn` shape (len 1..256) carries — the bound
/// [`super::MAX_ARN_BYTES`] states for every ARN of this service — for the same reason
/// [`clip_name`] clips a name: the `Resource` the ARN is read from is a caller's text.
///
/// It is a wider bound than the `Name` the `Task*` details carry, because the member it
/// bounds is the function's whole ARN rather than one field of a `Resource`
/// (`smithy_query(query="shape",shape="LambdaFunctionScheduledEventDetails")`).
#[must_use]
pub fn clip_arn(arn: String) -> String {
    clip(arn, super::MAX_ARN_BYTES)
}

/// `text` within `most` bytes, cut at a character boundary.
fn clip(text: String, most: usize) -> String {
    if text.len() <= most {
        return text;
    }
    let mut end = most;
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    text[..end].to_string()
}

/// One integration call handed from the walk to the operation that is running it.
struct Handover {
    call: Call,
    request: Value,
    /// Where the answer goes. A std channel, not a `oneshot`, because the side
    /// WAITING for it is a blocking thread that must not wait forever: this is the
    /// one receiver with a timeout ([`MAX_CALL_DURATION`]). Sending never blocks.
    answer: ::std::sync::mpsc::Sender<Answer>,
}

/// The [`Tasks`] a walk inside an operation performs its calls through: each call is
/// handed to that operation's own asynchronous task, which makes it and hands the
/// answer back.
///
/// One of these exists only INSIDE the blocking closure [`interpret`] spawns — it is
/// private, its field is private, and nothing else constructs one — which is what
/// makes the blocking handover below sound: a blocking send from an async worker
/// would be a defect, and there is no way to reach this from one.
struct Dispatcher {
    calls: ::tokio::sync::mpsc::Sender<Handover>,
    /// How deep a chain of in-process calls this interpretation is already running
    /// in — the depth the request that started it arrived at — against
    /// [`MAX_CALL_CHAIN_DEPTH`].
    depth: u8,
    /// What decides whether the execution role this walk runs as may make a call,
    /// when the call that started the walk could resolve that role; `None` leaves
    /// every call unjudged, which is what a walk with no role to ask about does.
    role: Option<RoleGate>,
    /// Where the handover of a callback token is recorded and its answer read back
    /// ([`CallbackHandovers`]); `None` for a walk with no store to record one in.
    handovers: Option<Handovers>,
    /// What answers which Map Run a Distributed Map state of this walk is already
    /// running ([`MapRuns`]); `None` for a walk that begins an attempt, whose every
    /// Map state starts a Map Run of its own.
    map_runs: Option<MapRuns>,
}

/// What decides whether the execution role a walk runs as may make one integration
/// call: the `(action, resource)` the call is subject to is formed from the call
/// itself ([`access_request`]), and this answers the refusal the task reports when
/// the policies in force on the role do not allow it.
///
/// A closure rather than a trait, because the documents it reads live in another
/// service's store and are reached through the engine the operation holds
/// ([`super::task_authorization`]): the walk knows nothing of either, and asks only
/// the question.
pub type RoleGate = ::std::sync::Arc<dyn Fn(&Call, &Value) -> Option<Failed> + Send + Sync>;

/// What a task fails with when the call could not be handed over or answered at all
/// — the walk and the operation running it parted company, which is this emulator
/// failing to ask rather than the service refusing.
fn unavailable(call: &Call) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "{} could not be handed to the request running the execution, so nothing was run \
             for it",
            call.described()
        ),
    }
}

/// What a task fails with when the operation running the execution is STILL inside
/// the previous call of this walk, so there is nothing to hand this one to.
///
/// One call is in flight at a time, and the walk waits for each answer before it
/// transitions — so this is reachable only after a call TIMED OUT server-side while
/// its target went on holding it. The walk is not held with it: the task fails with
/// ASL's own timeout error, which the state's `Retry` and `Catch` may match, and no
/// call is made for it.
fn still_calling(call: &Call) -> Failed {
    Failed {
        error: super::asl::TIMEOUT_ERROR.to_string(),
        cause: format!(
            "{} could not be made while the call this execution made before it has still not \
             answered, so nothing was run for it",
            call.described()
        ),
    }
}

/// What a task fails with when the chain of in-process calls it sits in is already
/// as deep as this emulator carries one ([`MAX_CALL_CHAIN_DEPTH`]): the call is NOT
/// made, so the thread this walk holds is the last one the chain pins.
fn too_deep(call: &Call) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "{} would be made from a chain of in-process calls already \
             {MAX_CALL_CHAIN_DEPTH} deep, which is as deep as this emulator carries one, so \
             nothing was run for it",
            call.described()
        ),
    }
}

/// What a task fails with when its call did not answer inside the span it was given
/// (`waited`): ASL's own timeout error, which the state's `Retry` and `Catch` may
/// match, rather than a thread held until the call feels like answering.
///
/// `waited` is the SHORTEST of the spans that bound this call: the state's own
/// `TimeoutSeconds` where it declares one, what is left of the waiting the run may do
/// ([`super::asl::MAX_CALL_WAIT_DURATION`], which the walk clamps the state's figure
/// to before handing the call over) and [`MAX_CALL_DURATION`] — so the cause states
/// the bound that actually expired.
fn timed_out(call: &Call, waited: Duration) -> Failed {
    Failed {
        error: super::asl::TIMEOUT_ERROR.to_string(),
        cause: format!(
            "{} did not answer inside the {waited:?} this task waits for one",
            call.described()
        ),
    }
}

impl Tasks for Dispatcher {
    fn perform(&self, call: &Call, request: &Value, within: Option<Duration>) -> Answer {
        // The chain's depth is decided BEFORE the handover: a call this deep is not
        // made at all, so the chain stops pinning threads here.
        if self.depth >= MAX_CALL_CHAIN_DEPTH {
            return Err(too_deep(call));
        }
        // The state's own `TimeoutSeconds` bounds the call when it asks for less
        // than the emulator's ceiling; it cannot ask for more.
        let waited = within.map_or(MAX_CALL_DURATION, |timeout| timeout.min(MAX_CALL_DURATION));
        let (answer, answered) = ::std::sync::mpsc::channel();
        let handover = Handover {
            call: call.clone(),
            request: request.clone(),
            answer,
        };
        // The walk runs on a blocking thread (`spawn_blocking`), never on an async
        // worker, so it may block here — but the handover must not be able to block
        // WITHOUT a bound. The walk has at most one call outstanding, so the single
        // buffered slot is normally free; it is NOT free when a previous call of this
        // walk timed out server-side and its target has still not answered, because
        // the serving half is then still inside that call. A send that waited there
        // would hold this blocking thread for the stuck target's own hold time rather
        // than for any ceiling this module states, so the handover is attempted
        // WITHOUT waiting and a slot that is not free fails the task instead:
        // nothing was handed over for it, and nothing is faked.
        match self.calls.try_send(handover) {
            Ok(()) => {}
            Err(::tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Err(unavailable(call))
            }
            Err(::tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                return Err(still_calling(call))
            }
        }
        // The wait is CLAMPED server-side: whatever the call is doing, this thread
        // is released at that bound.
        match answered.recv_timeout(waited) {
            Ok(answer) => answer,
            Err(::std::sync::mpsc::RecvTimeoutError::Timeout) => Err(timed_out(call, waited)),
            Err(::std::sync::mpsc::RecvTimeoutError::Disconnected) => Err(unavailable(call)),
        }
    }

    fn authorize(&self, call: &Call, request: &Value) -> Option<Failed> {
        self.role.as_ref()?(call, request)
    }

    fn hand_over(&self, token: &str, execution_arn: &str, state_name: &str) {
        if let Some(handovers) = &self.handovers {
            handovers.handed_over(token, execution_arn, state_name);
        }
    }

    fn settle_handover(&self, token: &str) -> Option<Replied> {
        self.handovers.as_ref()?.settled(token)
    }

    fn map_run(&self, occurrence: usize, label: &str) -> Option<ResumedMapRun> {
        self.map_runs.as_ref()?(occurrence, label)
    }
}

/// Run `work` — one interpretation of a definition — on a blocking thread, serving
/// the integration calls its `Task` states make from this asynchronous call.
///
/// The walk is CPU work a caller's definition sizes, so it does not run on an async
/// worker; the calls it asks for are asynchronous, so they are made here, on the
/// operation's own task, with the walk waiting for each answer. Both halves are
/// polled together, and the serving half ends when the walk drops its sender — so
/// the call returns exactly when the interpretation is over.
///
/// The thread the walk holds is held ACROSS each call, so how deep a chain of such
/// calls may go is what bounds the threads one inbound request can pin: the chain's
/// depth is read off the request that started this interpretation and carried into
/// the walk, where [`MAX_CALL_CHAIN_DEPTH`] decides whether another call is made.
///
/// `region` is the Region the calls are made in: the execution's own, which is the
/// Region its state machine's ARN names, and `acting` is what this walk performs its
/// work AS ([`super::task_authorization::Acting`]).
pub async fn interpret<Work, Outcome>(
    ctx: &smithy_server_runtime::OperationContext<'_>,
    region: &str,
    acting: super::task_authorization::Acting,
    work: Work,
) -> Result<Outcome, ::tokio::task::JoinError>
where
    Work: FnOnce(&dyn Tasks) -> Outcome + Send + 'static,
    Outcome: Send + 'static,
{
    interpret_through(ctx.callback(), chain_depth(ctx), region, acting, work).await
}

/// As [`interpret`], from a callback handle and a chain depth rather than from a
/// request's own context.
///
/// A run-a-job pattern's outcome is not served by the request that started it: the
/// job is polled after that call answered, so the task that polls it holds its own
/// clone of the callback ([`super::run_a_job`]) and interprets the continuation
/// through this. Everything else is identical, including the bound on how deep a
/// chain of in-process calls may still make one.
pub async fn interpret_through<Work, Outcome>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    depth: u8,
    region: &str,
    acting: super::task_authorization::Acting,
    work: Work,
) -> Result<Outcome, ::tokio::task::JoinError>
where
    Work: FnOnce(&dyn Tasks) -> Outcome + Send + 'static,
    Outcome: Send + 'static,
{
    // One call is in flight at a time: the walk waits for each answer before it
    // transitions, so a deeper channel would buffer nothing.
    let (calls, mut handovers) = ::tokio::sync::mpsc::channel::<Handover>(1);
    let role = acting.role;
    let connections = acting.connections;
    let handed = acting.handovers;
    let resumed_map_runs = acting.map_runs;
    let walk = ::tokio::task::spawn_blocking(move || {
        let dispatcher = Dispatcher {
            calls,
            depth,
            role,
            handovers: handed,
            map_runs: resumed_map_runs,
        };
        work(&dispatcher)
    });
    let serve = async {
        while let Some(handover) = handovers.recv().await {
            // The SERVING half is bounded by the same ceiling the waiting half is:
            // a call that has not answered inside it is abandoned here too, so this
            // loop returns to `handovers.recv()` and the walk's next call is handed
            // over rather than waiting for a target that is still holding the
            // previous one (`GetActivityTask` is a long poll). Nothing is faked for
            // an abandoned call: the walk reported the timeout itself.
            // The HTTP Task's work is a request to the endpoint its own parameters
            // name rather than a call on a service's operation, so it is made by the
            // module that owns that reading — over the same transport, under the same
            // ceiling, and reported to the walk through the same seam.
            let made = async {
                match handover.call.endpoint {
                    true => {
                        super::http_task::invoke(
                            callback,
                            connections.as_ref(),
                            &handover.request,
                        )
                        .await
                    }
                    false => {
                        call_api(callback, region, &handover.call, &handover.request).await
                    }
                }
            };
            let answer = match ::tokio::time::timeout(MAX_CALL_DURATION, made).await {
                Ok(answer) => answer,
                Err(_elapsed) => Err(timed_out(&handover.call, MAX_CALL_DURATION)),
            };
            // A walk that has stopped waiting — it timed out, or it is gone — cannot
            // be told; it reported the timeout itself.
            let _ = handover.answer.send(answer);
        }
    };
    let (walked, ()) = ::tokio::join!(walk, serve);
    walked
}

/// How deep a chain of in-process calls the request being served arrived at.
///
/// The framework stamps the depth on every request a handler's callback sends and
/// carries it in the request's extensions, so a request from outside the emulator
/// reads zero and each nested one reads one more
/// (`vendor/smithy-server-runtime/src/operation.rs`). This is the SAME counter the
/// router bounds a cycle with, read here for the resource that counter does not
/// bound: the blocking thread each level of the chain holds.
///
/// Public, because the task that polls a parked job interprets the continuation
/// outside the request that started the job and carries that request's depth into it.
pub fn chain_depth(ctx: &smithy_server_runtime::OperationContext<'_>) -> u8 {
    ctx.parts()
        .extensions
        .get::<smithy_server_runtime::CallbackRecursionDepth>()
        .map_or(0, |depth| depth.0)
}

/// The document a `Task` state REPORTS as the result of its integration call:
/// `answer`'s top-level members under the spelling a service integration's result is
/// written in, which is PascalCase.
///
/// The result of a service integration is the invoked operation's response document,
/// and a `Task` state reads it in the integration's own convention whatever the
/// invoked model spells its own members in — so a `ResultSelector`, a `ResultPath`
/// or a later state's path reads `$.ExecutionArn` of an operation whose own member is
/// `executionArn`. The wire spelling belongs to the call, not to the result.
///
/// Only the TOP level is re-spelled, for the reason [`wire_request`] gives: below it
/// a response carries a caller's own DATA beside the operation's members — the
/// attributes of an item, the output document of a child execution — and data is
/// never re-spelled.
pub fn task_result(answer: &Value) -> Value {
    let Some(members) = answer.as_object() else {
        return answer.clone();
    };
    let mut reported = Map::new();
    for (member, value) in members {
        reported.insert(MemberCase::Pascal.spell(member), value.clone());
    }
    Value::Object(reported)
}

/// The document a `Task` state REPORTS as the result of `call`, which depends on what
/// its `Resource` named.
///
/// A `Resource` naming an INTEGRATION reports the invoked operation's response
/// document, read in the integration's own convention ([`task_result`]). A `Resource`
/// naming a LAMBDA FUNCTION reports what that function RETURNED — its own document,
/// which is a caller's data and not an operation's members — so it is reported exactly
/// as the function wrote it, keys and all.
#[must_use]
pub fn reported_result(call: &Call, answer: &Value) -> Value {
    match call.invokes {
        Some(_) => answer.clone(),
        None => task_result(answer),
    }
}

/// The document `request` goes on the wire as: every TOP-LEVEL member under the
/// spelling the invoked service's model uses for it ([`MemberCase`]), and every
/// member the integration documents as a document the operation types as a string
/// carried as that document's JSON text ([`Call::json_text`]).
///
/// A `Task` state's integration parameters are the invoked operation's input, and
/// they are written in the integration's own convention — PascalCase — whatever the
/// invoked API spells its members in, so the spelling is settled here rather than
/// handing a service a member name its model does not carry.
///
/// Only the TOP level is re-spelled. Below it a document carries a caller's DATA
/// beside the operation's members — a DynamoDB attribute name is a map key, an
/// EventBridge detail is a caller's own document — and data is never re-spelled. A
/// member the operation types as a STRING is carried as JSON text wherever it sits,
/// top level or nested ([`Call::json_text_at`]), because that is a fact of the invoked
/// operation's own model rather than of the request's shape. A value already a string
/// is left as it is, so a member written as the text the operation types goes through
/// untouched.
fn wire_request(call: &Call, request: &Value) -> Value {
    let Some(members) = request.as_object() else {
        return request.clone();
    };
    let mut wire = Map::new();
    for (member, value) in members {
        let member = call.api.member_case.spell(member);
        let value = match call.json_text.contains(&member.as_str()) {
            true if !value.is_string() => Value::String(value.to_string()),
            _ => value.clone(),
        };
        wire.insert(member, value);
    }
    let mut document = Value::Object(wire);
    for path in call.json_text_at {
        as_json_text(&mut document, path);
    }
    document
}

/// Replace the document the member `path` names in `document` with that document's JSON
/// text, leaving `document` as it is where the path names nothing and leaving a value
/// that is already text untouched.
///
/// The inverse of [`parse_in_place`], and the nested form of [`Call::json_text`]: a LIST
/// on the way is walked element by element, so one path names the same member of every
/// entry of a request that takes a list of them.
fn as_json_text(document: &mut Value, path: &[&str]) {
    if let Some(items) = document.as_array_mut() {
        for item in items {
            as_json_text(item, path);
        }
        return;
    }
    let Some((member, rest)) = path.split_first() else {
        return;
    };
    let Some(node) = document.get_mut(*member) else {
        return;
    };
    if !rest.is_empty() {
        as_json_text(node, rest);
        return;
    }
    if !node.is_string() {
        *node = Value::String(node.to_string());
    }
}

/// One request, built for the protocol the invoked service's model declares.
struct Wired {
    method: &'static str,
    /// The request-target, which for an AWS JSON service is the root and for a REST
    /// one is the operation's own path and query.
    uri: String,
    /// The operation's name as the AWS JSON protocol carries it
    /// (`<target prefix>.<OperationName>`); `None` for a REST request, which names
    /// the operation by its method and path.
    target: Option<String>,
    /// The headers this operation's own members are bound to, beside the ones every
    /// request carries.
    headers: Vec<(&'static str, String)>,
    body: Vec<u8>,
}

/// The request `call` makes with `request` as the operation's input, or why it could
/// not be built.
fn wired(call: &Call, request: &Value) -> Result<Wired, String> {
    let document = wire_request(call, &call.operation_input(request));
    let Some(rest) = call.rest else {
        // An AWS JSON request body IS the operation's input document, posted to the
        // root and named in a header.
        let target = match call.api.wire {
            Wire::Json { target_prefix, .. } => format!("{target_prefix}.{}", call.operation),
            // An `awsQuery` request is a FORM, posted to the root, with the operation
            // named in it rather than in a header.
            Wire::Query { version } => {
                return Ok(Wired {
                    method: "POST",
                    uri: "/".to_string(),
                    target: None,
                    headers: Vec::new(),
                    body: form_body(&call.operation, version, &document, call.form_maps)?
                        .into_bytes(),
                })
            }
            // A service whose protocol is a REST one reaches this only for a call
            // carrying no binding, which [`read`] does not produce.
            Wire::Rest(_) => {
                return Err(format!(
                    "the '{}' call of {} carries no request binding for that service's REST \
                     protocol",
                    call.operation, call.api.sdk_id
                ))
            }
        };
        let body = ::serde_json::to_vec(&document).map_err(|error| error.to_string())?;
        return Ok(Wired {
            method: "POST",
            uri: "/".to_string(),
            target: Some(target),
            headers: Vec::new(),
            body,
        });
    };
    rest.wired(&document, call.payload_document)
}

/// The `application/x-www-form-urlencoded` body an `awsQuery` request carries:
/// `Action` naming the operation, `Version` naming the service's API version, and one
/// field per LEAF of the operation's input.
///
/// Each field name and each value is percent-encoded by the maintained encoder
/// ([`::percent_encoding`]), over RFC 3986's unreserved set — which is the encoding
/// AWS's own canonical request states — so a value carrying a delimiter is one field
/// rather than several, and a map key the caller chose cannot read as the protocol's
/// own `.` separator.
///
/// A member whose value is a MAP is spread as this protocol spreads one: one numbered
/// `entry` per pair, each carrying the pair's key and its value under the names that
/// map's own key and value members are written with ([`FormMap`]), and a value that is
/// a structure carrying its own members as one field each below it. Those names are the
/// invoked shape's, so a collection member no entry is carried for — any list, and a
/// map or structure this emulator does not state the field names of — is still NOT
/// bound: the call is not made for one rather than made with a field the service does
/// not carry.
fn form_body(
    operation: &str,
    version: &str,
    document: &Value,
    maps: &[FormMap],
) -> Result<String, String> {
    let mut body = format!("Action={operation}&Version={version}");
    for (member, value) in document.as_object().into_iter().flatten() {
        let spread = match maps.iter().find(|map| map.member == *member) {
            Some(map) => Spread::Map(map),
            None => Spread::Scalar,
        };
        let mut fields = Vec::new();
        form_fields(&encoded(member), value, spread, &mut fields)?;
        for (field, value) in fields {
            body.push('&');
            body.push_str(&field);
            body.push('=');
            body.push_str(&encoded(&value));
        }
    }
    Ok(body)
}

/// What one node of a form request's document is spread over fields AS, which is a
/// fact of the member the invoked model declares at that place rather than of the
/// value.
#[derive(Debug, Clone, Copy)]
enum Spread<'a> {
    /// One field, which is what every scalar is; a structure or a list reached here
    /// is a member whose field names this emulator does not carry, so no call is made
    /// for it.
    Scalar,
    /// A MAP, whose pairs are numbered `entry` fields under the names its key and
    /// value members are written with.
    Map(&'a FormMap),
    /// A STRUCTURE, each of whose members is one field named `<this>.<member>`.
    Structure,
}

/// The `entry` wrapper this protocol numbers a map's pairs under
/// (`<member>.entry.<n>.<key>`).
const MAP_ENTRY: &str = "entry";

/// The fields the node `name` of a form request goes on the wire as, appended to
/// `fields` as (already-encoded name, raw value) pairs.
fn form_fields(
    name: &str,
    value: &Value,
    spread: Spread<'_>,
    fields: &mut Vec<(String, String)>,
) -> Result<(), String> {
    // A member the request did not carry is no field at all.
    match value {
        Value::Null => Ok(()),
        Value::String(text) => {
            fields.push((name.to_string(), text.clone()));
            Ok(())
        }
        Value::Bool(_) | Value::Number(_) => {
            fields.push((name.to_string(), value.to_string()));
            Ok(())
        }
        Value::Object(members) => match spread {
            Spread::Map(map) => {
                for (index, (key, held)) in members.iter().enumerate() {
                    let entry = format!("{name}.{MAP_ENTRY}.{}", index + 1);
                    fields.push((format!("{entry}.{}", encoded(map.key)), key.clone()));
                    let at = format!("{entry}.{}", encoded(map.value));
                    let held_as = match map.structured {
                        true => Spread::Structure,
                        false => Spread::Scalar,
                    };
                    form_fields(&at, held, held_as, fields)?;
                }
                Ok(())
            }
            Spread::Structure => {
                for (member, held) in members {
                    let at = format!("{name}.{}", encoded(member));
                    form_fields(&at, held, Spread::Scalar, fields)?;
                }
                Ok(())
            }
            Spread::Scalar => Err(unbound_form_member(name, "structure")),
        },
        Value::Array(_) => Err(unbound_form_member(name, "list")),
    }
}

/// Why no call is made for a request carrying the member `name` as the collection
/// `kind`: this protocol spreads such a member over one field per leaf, under names
/// that are the invoked shape's own.
fn unbound_form_member(name: &str, kind: &str) -> String {
    format!(
        "this task's '{name}' is a {kind}, which this emulator does not bind to that service's \
         form-field protocol"
    )
}

/// `text` percent-encoded as ONE field of a form ([`UNRESERVED_ONLY`]).
fn encoded(text: &str) -> String {
    ::percent_encoding::utf8_percent_encode(text, UNRESERVED_ONLY).to_string()
}

/// The characters a value percent-encoded as ONE component escapes: everything but RFC
/// 3986's unreserved set, which is the encoding AWS's own canonical request states for a
/// query component and what a form field carries.
///
/// `pub(super)`, because it is the same set wherever a value of this crate is written
/// into a query or a form: the field of the form an `awsQuery` request IS
/// ([`form_body`]), and the query parameter and form field an HTTP Task sends
/// ([`super::http_task`]).
pub(super) const UNRESERVED_ONLY: &::percent_encoding::AsciiSet =
    &::percent_encoding::NON_ALPHANUMERIC
        .remove(b'-')
        .remove(b'.')
        .remove(b'_')
        .remove(b'~');

impl Rest {
    /// The request this operation's bindings put `document` on the wire as.
    ///
    /// `payload_document` says what the payload member's value IS: a value out of a
    /// JSON document a state built, whose bytes are that value's own JSON text, or the
    /// bytes a caller handed the member already rendered ([`Call::payload_document`]).
    fn wired(&self, document: &Value, payload_document: bool) -> Result<Wired, String> {
        let mut uri = String::new();
        let mut remaining = self.uri;
        // Each `{<member>}` of the path stands for the value the request names that
        // member with; a request naming none has no path to send, so the call is not
        // made rather than made against a path with a hole in it.
        while let Some((before, after)) = remaining.split_once('{') {
            uri.push_str(before);
            let (label, tail) = after
                .split_once('}')
                .ok_or_else(|| format!("the '{}' path binding is malformed", self.operation))?;
            // A label written `{<member>+}` is a GREEDY one, which spans as many path
            // segments as the value has: an object key is bound that way, so the
            // separator between its segments is part of the value rather than a
            // delimiter the value may not carry.
            let (label, component) = match label.strip_suffix('+') {
                Some(greedy) => (greedy, Component::GreedyLabel),
                None => (label, Component::Label),
            };
            let named = member_text(document, label).ok_or_else(|| {
                format!("this task's parameters name no '{label}', which the call's path needs")
            })?;
            uri.push_str(&one_component(&named, label, component)?);
            remaining = tail;
        }
        uri.push_str(remaining);
        for (member, parameter) in self.query {
            let Some(named) = member_text(document, member) else {
                continue;
            };
            let value = one_component(&named, member, Component::Query)?;
            // The operation's own `http.uri` may already carry a literal query — which
            // is how a service names a subresource (`?list-type=2`) — so what separates
            // this parameter from what is already there is read off the target as it
            // stands rather than assumed to be its first.
            uri.push(if uri.contains('?') { '&' } else { '?' });
            uri.push_str(parameter);
            uri.push('=');
            uri.push_str(&value);
        }
        let headers = self
            .headers
            .iter()
            .filter_map(|(member, field)| Some((*field, member_text(document, member)?)))
            .collect();
        let body = match self.request_body {
            // Every member is bound to the path, the query or a header, so there is no
            // body to send.
            Body::Empty => Vec::new(),
            // The payload member IS the body. For a value out of the document a state
            // built, the bytes are that JSON value's own TEXT — a string arrives as the
            // JSON string literal it is in that document, an object or an array as its
            // own document — because what the member carries is a value of that
            // document rather than bytes. A member handed bytes already rendered
            // carries them as they stand.
            Body::Member(member) | Body::Text(member) => match document.get(member) {
                None | Some(Value::Null) => Vec::new(),
                Some(Value::String(text)) if !payload_document => text.clone().into_bytes(),
                Some(value) => ::serde_json::to_vec(value).map_err(|error| error.to_string())?,
            },
            // The document of every member no other binding carries: one already on
            // the wire in the path, the query or a header is not in the body as well,
            // and a request whose members are all bound elsewhere carries no body.
            Body::Document => {
                let carried = self.unbound(document);
                match carried.is_empty() {
                    true => Vec::new(),
                    false => ::serde_json::to_vec(&Value::Object(carried))
                        .map_err(|error| error.to_string())?,
                }
            }
        };
        Ok(Wired {
            method: self.method,
            uri,
            target: None,
            headers,
            body,
        })
    }

    /// The response document this operation's bindings read a 2xx answer as: the
    /// status, the headers and the body each under the member they are bound to.
    ///
    /// Every value in it is the ANSWER's own, and a member the answer carried nothing
    /// for is absent rather than filled in.
    fn result(&self, answered: &Answered) -> Value {
        let mut members = Map::new();
        if !self.response_status.is_empty() {
            members.insert(
                self.response_status.to_string(),
                Value::Number(answered.status.into()),
            );
        }
        for (member, field) in self.response_headers {
            if let Some(value) = answered.header(field) {
                members.insert((*member).to_string(), Value::String(value));
            }
        }
        if !answered.payload.is_empty() {
            // The body is reported as the document it holds, which is what the
            // integration's result carries; a body that is not one is reported as
            // its own text rather than dropped.
            let payload = ::serde_json::from_slice::<Value>(&answered.payload).unwrap_or_else(
                |_| Value::String(String::from_utf8_lossy(&answered.payload).to_string()),
            );
            match self.response_body {
                Body::Member(member) => {
                    members.insert(member.to_string(), payload);
                }
                // A payload member the model types as a BLOB is the bytes themselves,
                // so it is reported as the TEXT it is even where those bytes parse as a
                // document: that is the string the integration answers, and the one a
                // state reads with `States.StringToJson`.
                Body::Text(member) => {
                    members.insert(
                        member.to_string(),
                        Value::String(String::from_utf8_lossy(&answered.payload).to_string()),
                    );
                }
                // The body IS the operation's output document, so the members it
                // holds are the answer's own: they are reported beside the ones the
                // status and the headers carry rather than under a member of their
                // own. A body that is not that document carries no member of this
                // operation's, and nothing is invented for one.
                Body::Document => {
                    if let Value::Object(carried) = payload {
                        members.extend(carried);
                    }
                }
                // An operation that binds no body reports none, whatever bytes the
                // answer happened to carry.
                Body::Empty => {}
            }
        }
        Value::Object(members)
    }

    /// The members of `document` this operation binds NOWHERE else — not to its path,
    /// its query or a header — which are the ones its body carries
    /// ([`Body::Document`]).
    fn unbound(&self, document: &Value) -> Map<String, Value> {
        let labels = self.labels();
        document
            .as_object()
            .into_iter()
            .flatten()
            .filter(|(member, _)| {
                !labels.iter().any(|label| label == member)
                    && !self.query.iter().any(|(bound, _)| bound == member)
                    && !self.headers.iter().any(|(bound, _)| bound == member)
            })
            .map(|(member, value)| (member.clone(), value.clone()))
            .collect()
    }

    /// The members this operation binds to its PATH, as the `{<member>}` holes of its
    /// `http.uri` name them, a GREEDY label's own `+` dropped.
    fn labels(&self) -> Vec<&'static str> {
        let mut labels = Vec::new();
        let mut remaining = self.uri;
        while let Some((_, after)) = remaining.split_once('{') {
            let Some((label, tail)) = after.split_once('}') else {
                break;
            };
            labels.push(label.strip_suffix('+').unwrap_or(label));
            remaining = tail;
        }
        labels
    }

    /// The output member this operation binds the answer's BODY to, for an operation
    /// that binds one ([`Body::Member`]); `None` where the body is the output document
    /// itself or the answer carries none.
    fn response_member(&self) -> Option<&'static str> {
        match self.response_body {
            Body::Member(member) | Body::Text(member) => Some(member),
            Body::Document | Body::Empty => None,
        }
    }
}

/// The text `document` names `member` with: a value given as a string is that string,
/// and any other value is its JSON text, which is how a path, a query parameter or a
/// header carries one. `None` for a member the document does not carry.
fn member_text(document: &Value, member: &str) -> Option<String> {
    match document.get(member) {
        None | Some(Value::Null) => None,
        Some(Value::String(text)) => Some(text.clone()),
        Some(value) => Some(value.to_string()),
    }
}

/// Which part of a request-target a member's value goes into, which decides one thing:
/// whether an EMPTY value is one that part can carry.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Component {
    /// A path label, which stands for one whole segment: an empty one would collapse
    /// the path it is part of, so there is no request to send for it.
    Label,
    /// A GREEDY path label (`{Key+}`), which stands for as many segments as the value
    /// holds: the separator between them is part of the value, so it is carried rather
    /// than refused. An empty one still collapses the path.
    GreedyLabel,
    /// A query parameter's value, which an operation may model as admitting the empty
    /// string (`Invoke`'s `Qualifier` does), and which a request carries as the
    /// parameter with nothing after its `=`.
    Query,
}

/// `value`, when it is text a request-target carries as ONE component — a path segment
/// or a query parameter's value — and `Err` naming `member` when it is not.
///
/// A member an operation binds to the path or the query is one whose own modelled
/// pattern admits only characters a URI component admits: a function's name, its ARN and
/// a version qualifier are written with the unreserved characters plus `:` and `$`, all
/// of which a component carries as they stand. A caller's parameter CAN hold anything,
/// so a value outside that set is one the request cannot address — the delimiters are
/// what would make it more than one component — and the call is not made rather than
/// made against a target assembled out of it. Nothing is escaped here: a value that
/// needed escaping names no resource of the invoked operation either.
///
/// The path SEPARATOR is the one character whose place depends on the component: a
/// greedy label spans several segments and a prefix or a key written in a query names
/// one, so each of those carries it, while a single-segment label does not.
fn one_component(value: &str, member: &str, component: Component) -> Result<String, String> {
    let punctuation = match component {
        Component::Label => URI_COMPONENT_PUNCTUATION,
        Component::GreedyLabel | Component::Query => URI_PATH_PUNCTUATION,
    };
    let carried = value
        .bytes()
        .all(|byte| byte.is_ascii_alphanumeric() || punctuation.contains(&byte));
    let empty = value.is_empty()
        && matches!(component, Component::Label | Component::GreedyLabel);
    if empty || !carried {
        return Err(format!(
            "this task's '{member}' is not text a request-target carries as one component, so it \
             addresses nothing the call can be made against"
        ));
    }
    Ok(value.to_string())
}

/// The punctuation a URI component carries unescaped beside the alphanumerics: RFC
/// 3986's unreserved set (`-`, `.`, `_`, `~`) and the two sub-delimiters an ARN and a
/// version qualifier are written with (`:`, `$`).
const URI_COMPONENT_PUNCTUATION: &[u8] = b"-._~:$";

/// [`URI_COMPONENT_PUNCTUATION`] plus the path separator, for the components that span
/// segments: a greedy label (an object's key) and a query parameter naming a path (a
/// listing's prefix).
const URI_PATH_PUNCTUATION: &[u8] = b"-._~:$/";

/// Stop every job a walk started and then ABANDONED.
///
/// A `Parallel` branch's run-a-job task starts a job and parks at it, and a SIBLING
/// branch can fail while that job is still running: the state has failed, so no task
/// is waiting at the job any more — and the job is work of another service, so it is
/// stopped through the stopping call its own integration names, in the Region the
/// starting call was made in. The same crossing of the service boundary `StopExecution`
/// makes for a job the execution it stops was parked at, for the same reason: a job
/// nothing will resolve is not left running.
///
/// `not_before` is the instant the failure that abandoned the job comes due — the
/// instant the execution around it ENDS, which the walk computed from the `Wait`
/// states the failing branch passed and nothing slept for. The job stays running
/// until then: it is the sibling's failure that stops it, so a job abandoned by a
/// failure the clock has not reached yet is still a job of its own service, readable
/// and stoppable there, exactly as it is on the service. Reaching that instant is
/// handed to a task of its own, because the call that planned the run must answer
/// while the job is still running; a `None`, and an instant already past, stop it
/// here.
///
/// That wait is the definition's to choose, so a job whose abandoning failure is owed
/// further out than this process holds a job at all
/// ([`crate::state::MAX_PARKED_JOB_DURATION`]) is LET GO of instead, exactly as the
/// job a task is still parked at is once that span runs out: the alternative is
/// stopping it at the ceiling, which would report the job ended before the failure
/// that ends it had happened.
///
/// What the invoked service answers changes nothing here: the task's own outcome is
/// already the failure of the state around it.
pub async fn stop_abandoned(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    abandoned: &[crate::state::PlannedJob],
    not_before: Option<std::time::SystemTime>,
) {
    let owed = not_before
        .and_then(|due| due.duration_since(crate::state::now()).ok())
        .unwrap_or_default();
    if owed > crate::state::MAX_PARKED_JOB_DURATION {
        return;
    }
    for job in abandoned {
        let (call, request) = stopping(job.api, job.job, &job.job_id, &job.naming);
        if owed.is_zero() {
            let _answered = call_api(callback, region, &call, &request).await;
            continue;
        }
        let callback = callback.clone();
        let region = region.to_string();
        ::tokio::spawn(async move {
            ::tokio::time::sleep(owed).await;
            let _answered = call_api(&callback, &region, &call, &request).await;
        });
    }
}

/// Make one integration call and read what the service answered.
///
/// Public, because the calls a run-a-job pattern is made of besides the one the walk
/// asks for — the one that REPORTS the job and the one that STOPS it — are made
/// outside any walk ([`super::run_a_job`]), through this same seam.
pub async fn call_api(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    call: &Call,
    request: &Value,
) -> Answer {
    let api = call.api;
    // Parameters the invoked operation cannot be called with at all fail the task on its
    // own parameters, before a request is built out of them ([`unusable_parameters`]).
    if let Some(failed) = unusable_parameters(call, request) {
        return Err(failed);
    }
    let built = match wired(call, request) {
        Ok(built) => built,
        Err(why) => {
            return Err(Failed {
                error: super::asl::RUNTIME_ERROR.to_string(),
                cause: format!(
                    "the request this task would have sent could not be built: {why}"
                ),
            })
        }
    };
    match dispatch(callback, api, region, built).await {
        Ok(answered) => {
            let read = match call.rest {
                // A REST answer's members are spread over its status, its headers and
                // its body, so the operation's own bindings are what reads it; a
                // refusal is read the same way whichever protocol carried it.
                Some(rest) if (200..300).contains(&answered.status) => Ok(rest.result(&answered)),
                _ => match api.wire {
                    // An `awsQuery` answer is an XML document wrapping the operation's
                    // own result, so the result is read out of that wrapper.
                    Wire::Query { .. } if (200..300).contains(&answered.status) => {
                        query_result(&call.operation, &answered.payload)
                    }
                    _ => answer_of(
                        api,
                        &call.operation,
                        answered.status,
                        answered.error_type.as_deref(),
                        &answered.payload,
                        answered.header(REQUEST_ID_HEADER).as_deref(),
                        answered.query_error_code().as_deref(),
                    ),
                },
            };
            // The result of an OPTIMIZED integration is the literal SDK response of
            // the call, which renders each instant the response carries as whole
            // epoch milliseconds and carries the SDK's own response envelope beside
            // the operation's members; a refusal carries no result to wrap. A
            // `Resource` that named the FUNCTION reports the function's own payload
            // out of that response instead.
            match read {
                // An invocation Lambda SERVED whose function ERRORED is not a result:
                // the unit of work this task is failed, so it is read as the failure it
                // is before any result is built out of the answer.
                Ok(document) => match function_error(call, &document) {
                    Some(failed) => Err(failed),
                    None if call.invokes.is_some() => Ok(invocation_payload(call, &document)),
                    None if call.sdk_envelope => Ok(with_sdk_envelope(
                        in_epoch_millis(document, call.epoch_millis),
                        &answered,
                        call.rest,
                    )),
                    None => Ok(in_epoch_millis(document, call.epoch_millis)),
                },
                other => other,
            }
        }
        Err(why) => Err(Failed {
            error: super::asl::RUNTIME_ERROR.to_string(),
            cause: format!(
                "the '{}' call of {} could not be made: {why}",
                call.operation, api.sdk_id
            ),
        }),
    }
}

/// The member a task's result carries the HTTP facts of the call under, as the SDK
/// response reports them: the status the call answered, and its headers under both
/// spellings the SDK exposes them in.
const SDK_HTTP_METADATA: &str = "SdkHttpMetadata";
/// What a task whose `Resource` named a Lambda FUNCTION reports out of the
/// invocation's answer: the function's own response payload, the member that operation
/// binds its body to.
///
/// The rest of the answer's members are facts of the API CALL — the status it came
/// back with, the version that ran — rather than of the function's output, and this
/// form is the function's output. An invocation whose function returned nothing
/// answers no payload at all, which is reported as the null document rather than as an
/// object the function did not produce.
///
/// Pure, so the reading is pinned by this module's own tests.
fn invocation_payload(call: &Call, document: &Value) -> Value {
    let member = call
        .rest
        .and_then(Rest::response_member)
        .unwrap_or(PAYLOAD_MEMBER);
    document.get(member).cloned().unwrap_or(Value::Null)
}

/// The failure a Lambda invocation that RAN and whose function ERRORED is to the `Task`
/// state that asked for it, or `None` for any other answer.
///
/// An invocation is served with HTTP 200 whether the function returned a value or
/// raised: "The status code in the API response doesn't reflect function errors", and
/// what distinguishes the two is [`FUNCTION_ERROR_MEMBER`], which Lambda sets only when
/// "an error occurred during function execution". A `Task` state's work is the function's
/// RUN, so a run that errored is a failed task rather than a result whose document
/// happens to describe an error — and the name the state reports for it is the error's
/// own class out of the document the run produced, which is what a `Retry` or a `Catch`
/// naming the exception the handler raised matches. A document carrying no class names
/// nothing of the function's own to report, so the failure is reported under the name an
/// unattributable Lambda error is reported by ([`LAMBDA_UNKNOWN_ERROR`]).
///
/// The cause is the error document ITSELF, as the run produced it, so the message and the
/// trace the handler raised reach the client rather than a sentence written here.
///
/// Pure, so the reading of every answer form is pinned by this module's own tests.
fn function_error(call: &Call, document: &Value) -> Option<Failed> {
    if call.api.namespace != LAMBDA_NAMESPACE || call.operation != LAMBDA_INVOKE_OPERATION {
        return None;
    }
    let signal = document
        .get(FUNCTION_ERROR_MEMBER)
        .and_then(Value::as_str)
        .filter(|signal| !signal.is_empty())?;
    let payload = document.get(
        call.rest
            .and_then(Rest::response_member)
            .unwrap_or(PAYLOAD_MEMBER),
    );
    let error = payload
        .and_then(|payload| payload.get(ERROR_TYPE_MEMBER))
        .and_then(Value::as_str)
        .filter(|class| !class.is_empty())
        .unwrap_or(LAMBDA_UNKNOWN_ERROR)
        .to_string();
    let cause = match payload {
        Some(Value::String(text)) => text.clone(),
        Some(document) => document.to_string(),
        None => format!(
            "{} reported a '{signal}' function error for this invocation, and the run produced \
             no error document",
            call.api.sdk_id
        ),
    };
    Some(Failed { error, cause })
}

/// The member a task's result carries the call's response metadata under: the
/// identifier of the request the call made.
const SDK_RESPONSE_METADATA: &str = "SdkResponseMetadata";

/// The header an AWS service names the request it answered in, which the SDK reports
/// as the response's request id.
const REQUEST_ID_HEADER: &str = "x-amzn-requestid";

/// How many milliseconds one second holds, which is the whole of the conversion
/// between the wire form of a timestamp and the SDK's.
const MILLIS_PER_SECOND: f64 = 1_000.0;

/// The largest figure of milliseconds this conversion reports: 2^53, the last integer
/// a double holds exactly, so a value past it is left as it came back rather than
/// reported as an integer it is not. It is some 285 000 years from the epoch, far past
/// any instant a service answers.
const MAX_EPOCH_MILLIS: f64 = 9_007_199_254_740_992.0;

/// `document` with each of `members` reported in whole epoch MILLISECONDS.
///
/// An `awsJson` timestamp is on the wire as epoch SECONDS with a fractional part,
/// while the SDK response a `Task` state reads renders a date as the integer number of
/// milliseconds; the instant is the same one either way, so the value is the answer's
/// own and only its unit changes. A member the answer did not carry, or carried as
/// something other than a number this can convert, is left exactly as it came back.
///
/// Pure, so the conversion is pinned by this module's own tests.
fn in_epoch_millis(document: Value, members: &[&str]) -> Value {
    if members.is_empty() {
        return document;
    }
    let Value::Object(mut carried) = document else {
        return document;
    };
    for member in members {
        // The member is read under both spellings a response may carry it in, as the
        // request's own members are ([`MemberCase`]).
        for spelling in [
            MemberCase::LowerCamel.spell(member),
            MemberCase::Pascal.spell(member),
        ] {
            let Some(seconds) = carried.get(&spelling).and_then(Value::as_f64) else {
                continue;
            };
            let millis = (seconds * MILLIS_PER_SECOND).round();
            if millis.is_finite() && millis.abs() < MAX_EPOCH_MILLIS {
                carried.insert(spelling, Value::Number((millis as i64).into()));
            }
        }
    }
    Value::Object(carried)
}

/// `document` as a `Task` state reports the literal SDK response of the call that
/// answered it: the operation's own members, plus the envelope the SDK puts the
/// response's HTTP facts in.
///
/// Every value in it is the ANSWER's own — the status the call came back with, and the
/// headers it carried, the request id among them — so a definition reading
/// `$.SdkHttpMetadata.HttpStatusCode` or `$.SdkResponseMetadata.RequestId` reads what
/// the call really answered, and a fact the answer did not carry is reported by
/// nothing rather than filled in: the envelope's members are there because the
/// integration returns them, and each holds exactly what came back. A response whose
/// body is not an object carries no members to put the envelope beside, so it is
/// reported as it stands.
///
/// A header is reported under the FIELD NAME the invoked operation binds it to, which
/// is the name the service writes on the wire and so the name the SDK's own envelope
/// exposes it under: HTTP field names are case-insensitive and the client library this
/// call goes through normalises every one it parses to lower case, so the spelling the
/// envelope reports is read back off that operation's bindings ([`Rest`]) rather than
/// from the parsed answer. A header the operation binds no member to is reported as the
/// answer carried it.
///
/// Pure, so the shape of the envelope is pinned by this module's own tests.
fn with_sdk_envelope(document: Value, answered: &Answered, rest: Option<&Rest>) -> Value {
    let Value::Object(mut members) = document else {
        return document;
    };
    // `HttpHeaders` carries one value per header and `AllHttpHeaders` every value a
    // header was repeated with, which is the pair the SDK exposes.
    let mut single = Map::new();
    let mut all: Map<String, Value> = Map::new();
    for (carried, value) in &answered.headers {
        let name = bound_field(rest, carried);
        single.insert(name.clone(), Value::String(value.clone()));
        match all.get_mut(&name).and_then(Value::as_array_mut) {
            Some(values) => values.push(Value::String(value.clone())),
            None => {
                all.insert(name, Value::Array(vec![Value::String(value.clone())]));
            }
        }
    }
    let mut http = Map::new();
    if !single.is_empty() {
        http.insert("AllHttpHeaders".to_string(), Value::Object(all));
        http.insert("HttpHeaders".to_string(), Value::Object(single));
    }
    http.insert(
        "HttpStatusCode".to_string(),
        Value::Number(answered.status.into()),
    );
    members.insert(SDK_HTTP_METADATA.to_string(), Value::Object(http));
    let mut response = Map::new();
    if let Some(request_id) = answered.header(REQUEST_ID_HEADER) {
        response.insert("RequestId".to_string(), Value::String(request_id));
    }
    members.insert(SDK_RESPONSE_METADATA.to_string(), Value::Object(response));
    Value::Object(members)
}

/// The field name `carried` is reported under: the spelling the invoked operation's
/// own bindings give that header ([`Rest::response_headers`]) where it binds a member
/// to it, and the spelling the answer carried it in where it binds none.
///
/// HTTP field names are case-insensitive, so the two name one header; which of them the
/// envelope reports is what a definition reading `$.SdkHttpMetadata.HttpHeaders` sees,
/// and that is the name the invoked service writes — the one its model states.
///
/// A header the operation binds NO member to — the transport's own `content-type` and
/// `content-length`, the request id — is reported as the answer carried it, because
/// nothing in the operation's model states a spelling for it and this emulator does not
/// invent one.
///
/// Pure, so it is pinned by this module's own tests.
fn bound_field(rest: Option<&Rest>, carried: &str) -> String {
    rest.into_iter()
        .flat_map(|rest| rest.response_headers.iter())
        .find(|(_, field)| field.eq_ignore_ascii_case(carried))
        .map_or_else(|| carried.to_string(), |(_, field)| (*field).to_string())
}

/// The result or the failure one answer carries.
///
/// A 2xx answers the response document, which IS the state's result; an operation
/// whose output carries no member answers an empty body, which is the empty
/// document. Anything else is a refusal, reported as the error name Step Functions
/// gives a failed service call — the service as it names it in a task error, and the
/// SDK exception the code it answered is deserialized into ([`Api::task_error`]) — so
/// a definition's `Retry` and `Catch` match on the error the service's own client
/// raises. The cause is that exception's own message ([`sdk_exception_cause`]).
/// A refusal with no code at all is left as ASL's own [`TASK_FAILED_ERROR`].
///
/// `request_id` is the identifier the answer named the request it refused by, which
/// the SDK reports in the exception's metadata; `None` for an answer that named none,
/// and nothing is invented to fill the field. `query_code` is the legacy query code the
/// answer named the class by for a service whose model is query-compatible
/// ([`Api::query_compatible`]), which is the code such a service's own client reports.
///
/// Pure, so the reading of every answer form is pinned by this module's own tests.
fn answer_of(
    api: &Api,
    operation: &str,
    status: u16,
    error_type: Option<&str>,
    payload: &[u8],
    request_id: Option<&str>,
    query_code: Option<&str>,
) -> Answer {
    let document: Option<Value> = if payload.is_empty() {
        None
    } else {
        // A refusal is read in whichever of the two protocols this crate calls carried
        // it: an AWS JSON body is the document itself, and a REST-XML one carries its
        // `Code` and `Message` as the document's own elements, which is read to the
        // same object so one reading answers for both.
        ::serde_json::from_slice(payload)
            .ok()
            .or_else(|| super::xml_answer::as_document(payload))
    };
    if (200..300).contains(&status) {
        return Ok(document.unwrap_or_else(|| Value::Object(Map::new())));
    }
    let shape = error_shape(error_type, document.as_ref());
    let message = document
        .as_ref()
        .and_then(error_message)
        .unwrap_or_default();
    // A gap in this emulator is not the service refusing the call: nothing was run
    // for the task, which is what it reports.
    if shape.as_deref().map(bare_name) == Some(NOT_IMPLEMENTED_CODE) {
        return Err(Failed {
            error: super::asl::RUNTIME_ERROR.to_string(),
            cause: format!(
                "the '{operation}' operation of {} is not implemented by this emulator, so \
                 nothing was run for this task",
                api.sdk_id
            ),
        });
    }
    let (error, cause) = match shape {
        Some(shape) => (
            api.task_error(&shape),
            // The code the exception reports is the one that service's own client
            // matches the class on: the legacy query code where the service names
            // one, and the error shape's name where it does not.
            sdk_exception_cause(
                api,
                &message,
                query_code.unwrap_or_else(|| bare_name(&shape)),
                status,
                request_id,
                operation,
            ),
        ),
        None => (
            TASK_FAILED_ERROR.to_string(),
            format!(
                "{} answered the '{operation}' call with HTTP {status} and no error code",
                api.sdk_id
            ),
        ),
    };
    Err(Failed { error, cause })
}

/// What an `awsQuery` 2xx answered, as the operation's own output document.
///
/// Such an answer wraps the operation's result in a `<{Operation}Response>` element
/// carrying a `<{Operation}Result>` element and the `<ResponseMetadata>` beside it, so
/// the operation's own members are the children of that result element — which is what
/// this reports ([`super::xml_answer::as_value`]). An operation whose output carries no
/// member answers no result element, which is the empty document; a document that is
/// not that shape is reported from the response element with its metadata left out,
/// rather than read as members it does not carry.
fn query_result(operation: &str, payload: &[u8]) -> Answer {
    let root = super::xml_answer::read(payload).map_err(|why| Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!("the answer to the '{operation}' call could not be read: {why}"),
    })?;
    if let Some(result) = root.child(&format!("{operation}{RESULT_SUFFIX}")) {
        return Ok(super::xml_answer::as_value(result));
    }
    let mut reported = root.clone();
    reported
        .children
        .retain(|child| child.name != RESPONSE_METADATA_ELEMENT);
    Ok(super::xml_answer::as_value(&reported))
}

/// The element an `awsQuery` answer wraps the operation's own members in, and the one
/// it carries the call's own metadata in beside them.
const RESULT_SUFFIX: &str = "Result";
/// See [`RESULT_SUFFIX`].
const RESPONSE_METADATA_ELEMENT: &str = "ResponseMetadata";

/// The cause a failed `Task` reports for a refusal: the message the exception the
/// invoked service's SDK raised carries.
///
/// An AWS SDK renders a service exception as the service's own message followed by the
/// metadata that answer carried — the client the call was made with, the status, the
/// error code and the identifier of the request — and that rendering is what Step
/// Functions puts in the task's cause, which is why a `Catch` path reading the cause
/// finds the service's error code in it. The ONE rendering of it: a refusal the
/// authorization gate makes on the service's behalf is reported the same way, through
/// [`sdk_exception_message`] below. A fact the answer did not carry is left out rather
/// than invented: a refusal naming no request is reported without one, and a refusal
/// carrying no message of its own says which call of which service came back under that
/// code.
fn sdk_exception_cause(
    api: &Api,
    message: &str,
    code: &str,
    status: u16,
    request_id: Option<&str>,
    operation: &str,
) -> String {
    let message = match message.is_empty() {
        true => format!("{} refused the '{operation}' call with {code}", api.sdk_id),
        false => message.to_string(),
    };
    sdk_exception_message(api.sdk_client, &message, code, status, request_id)
}

/// One SDK exception as its own client renders it: the service's sentence, then the
/// metadata the answer carried.
///
/// Public to this crate because a refusal the emulator answers FOR the invoked service
/// ([`super::task_authorization`]) is the same exception reaching the state the same
/// way: one rendering, so the two cannot drift. `client` is the name that service's own
/// SDK client carries ([`Api::sdk_client`]), which is what an exception reports the
/// service under; `request_id` is the identifier the answer named the request by, left
/// out where there is none — a call that was never made has no request to name.
pub fn sdk_exception_message(
    client: &str,
    message: &str,
    code: &str,
    status: u16,
    request_id: Option<&str>,
) -> String {
    let request = request_id.map_or_else(String::new, |id| format!("Request ID: {id}; "));
    format!(
        "{message} (Service: {client}; Status Code: {status}; Error Code: {code}; \
         {request}Proxy: null)"
    )
}

/// The error SHAPE an AWS JSON refusal names: the `X-Amzn-Errortype` header, else the
/// body's `__type` discriminator, else its `code` member.
///
/// The NAMESPACE is kept, because it is what says whose shape the class belongs to: a
/// service's own model, or the request-handling framework in front of it
/// ([`FRAMEWORK_NAMESPACES`]). Only the decoration that is no part of a shape id — a URL
/// after `:` — is dropped.
/// The error SHAPE an AWS JSON refusal names: the `X-Amzn-Errortype` header, else the
/// body's `__type` discriminator, else its `code` member.
///
/// The NAMESPACE is kept, because it is what says whose shape the class belongs to: a
/// service's own model, or the request-handling framework in front of it
/// ([`FRAMEWORK_NAMESPACES`]). Only the decoration that is no part of a shape id — a URL
/// after `:` — is dropped.
fn error_shape(error_type: Option<&str>, document: Option<&Value>) -> Option<String> {
    let discriminator = error_type.map(str::to_string).or_else(|| {
        let document = document?;
        ["__type", "code", "Code"]
            .iter()
            .find_map(|member| document.get(*member).and_then(Value::as_str))
            .map(str::to_string)
    })?;
    let named = discriminator
        .split(':')
        .next()
        .unwrap_or_default()
        .trim()
        .to_string();
    (!named.is_empty()).then_some(named)
}

/// The bare name of a shape id: what follows its namespace, read through the tree's one
/// reading of a shape id ([`::service_common::shape_local_name`]).
fn bare_name(shape: &str) -> &str {
    ::service_common::shape_local_name(shape)
}

/// The message an AWS JSON refusal carries, under either of the member spellings the
/// protocol allows.
fn error_message(document: &Value) -> Option<String> {
    ["message", "Message"]
        .iter()
        .find_map(|member| document.get(*member).and_then(Value::as_str))
        .map(str::to_string)
}

/// Sign one request with the emulator's own credentials and hand it to the service
/// its credential scope names, in process.
///
/// It is signed because the emulator verifies every request signature; the
/// credential scope is also what routes it, so the service that owns the operation
/// is the one that serves the call. The answer is its status, the error type it
/// named in the header, and its body.
async fn dispatch(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    api: &Api,
    region: &str,
    built: Wired,
) -> Result<Answered, String> {
    let mut builder = ::http::Request::builder()
        .method(built.method)
        .uri(&built.uri)
        .header("host", api.host(region));
    // A media type describes a BODY: a request that carries none — a REST read bound
    // to its path and its query alone — names none either.
    if !built.body.is_empty() {
        builder = builder.header("content-type", api.content_type());
    }
    if let Some(target) = &built.target {
        builder = builder.header(TARGET_HEADER, target);
    }
    // A query-compatible service names a refusal by its legacy query code as well as
    // by its shape, and only for a client that asks for it.
    if api.query_compatible {
        builder = builder.header(QUERY_MODE_HEADER, "true");
    }
    for (field, value) in &built.headers {
        builder = builder.header(*field, value);
    }
    let mut request = builder
        .body(built.body)
        .map_err(|error| error.to_string())?;

    let (access_key_id, secret_access_key) = credentials(callback).await?;
    sign(
        &mut request,
        api.signing_name,
        region,
        &access_key_id,
        &secret_access_key,
    )?;

    let (parts, body) = request.into_parts();
    let request = ::http::Request::from_parts(
        parts,
        ::aws_smithy_types::body::SdkBody::from(::bytes::Bytes::from(body)),
    );
    let request = ::aws_smithy_runtime_api::http::Request::try_from(request)
        .map_err(|error| error.to_string())?;
    let response = ::aws_smithy_runtime_api::client::http::HttpConnector::call(
        &callback.http_connector(),
        request,
    )
    .await
    .map_err(|error| error.to_string())?;
    let status = response.status().as_u16();
    let error_type = response
        .headers()
        .get(ERROR_TYPE_HEADER)
        .map(str::to_string);
    // Every header the answer carried, in the order it carried them: the task's own
    // result reports them for an integration whose result is the literal SDK response
    // ([`with_sdk_envelope`]), so they are read off the answer rather than invented.
    let headers: Vec<(String, String)> = response
        .headers()
        .iter()
        .map(|(name, value)| (name.to_string(), value.to_string()))
        .collect();
    let payload = response
        .body()
        .bytes()
        .ok_or("the in-process answer carried a streaming body")?
        .to_vec();
    Ok(Answered {
        status,
        error_type,
        headers,
        payload,
    })
}

/// Make one integration call and answer the RAW answer the invoked service gave: the
/// status, the error shape it named and its body.
///
/// The seam for a caller that reports what a call came to in ITS OWN vocabulary rather
/// than as a `Task` state's result: Amazon EventBridge Scheduler's universal target
/// reports a failed invocation by the error code and the message the answer carries and
/// decides from the status whether the invocation is retried
/// ([`Target$RetryPolicy`](https://docs.aws.amazon.com/scheduler/latest/APIReference/API_RetryPolicy.html)),
/// which the ASL error names [`call_api`] reads an answer into do not carry. The request
/// is built and signed exactly as [`call_api`] builds and signs it, so both callers make
/// the same call.
pub async fn call_answered(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    call: &Call,
    request: &Value,
) -> Result<Answered, String> {
    let built = wired(call, request)?;
    dispatch(callback, call.api, region, built).await
}

/// What one in-process call came back with.
pub struct Answered {
    /// The HTTP status the service answered.
    status: u16,
    /// The error shape it named in [`ERROR_TYPE_HEADER`], for a refusal.
    error_type: Option<String>,
    /// Every header of the answer, as it carried them.
    headers: Vec<(String, String)>,
    /// Its body.
    payload: Vec<u8>,
}

impl Answered {
    /// The HTTP status the invoked service answered.
    #[must_use]
    pub fn status(&self) -> u16 {
        self.status
    }

    /// The error shape it named in [`ERROR_TYPE_HEADER`]; `None` for an answer that
    /// named none.
    #[must_use]
    pub fn error_type(&self) -> Option<&str> {
        self.error_type.as_deref()
    }

    /// Its body, as it answered it.
    #[must_use]
    pub fn payload(&self) -> &[u8] {
        &self.payload
    }

    /// The first value the answer carried `name` under, compared without regard to
    /// case as HTTP field names are.
    fn header(&self, name: &str) -> Option<String> {
        self.headers
            .iter()
            .find(|(carried, _)| carried.eq_ignore_ascii_case(name))
            .map(|(_, value)| value.clone())
    }

    /// The LEGACY query code a query-compatible service named this refusal's class by,
    /// out of [`QUERY_ERROR_HEADER`]'s `<code>;<fault>`; `None` for an answer that
    /// carried none, which is every answer of a service that is not query-compatible
    /// and every class such a service's model states no legacy code for.
    fn query_error_code(&self) -> Option<String> {
        let carried = self.header(QUERY_ERROR_HEADER)?;
        let code = carried.split(';').next().unwrap_or_default().trim();
        (!code.is_empty()).then(|| code.to_string())
    }
}

/// The credentials a callback signs with: the emulator's own, through the provider
/// the framework hands every handler — which falls back to its placeholder pair when
/// the router vends none (request authentication off, where nothing verifies a
/// signature). They sign this call and are never reported or logged.
async fn credentials(
    callback: &smithy_server_runtime::EmbeddedEmulator,
) -> Result<(String, String), String> {
    use ::aws_credential_types::provider::ProvideCredentials;

    let credentials = callback
        .aws_credentials_provider()
        .provide_credentials()
        .await
        .map_err(|error| error.to_string())?;
    Ok((
        credentials.access_key_id().to_string(),
        credentials.secret_access_key().to_string(),
    ))
}

/// SigV4-sign one in-process request to another emulated service, in place.
///
/// The settings are the ones the emulator's own verifier re-signs with
/// (`vendor/server/src/request_authentication.rs`), so the signature this produces
/// is the one that verifier computes over the same request — which is what lets a
/// task's call be served with request authentication ON.
///
/// TO LIFT: this, [`dispatch`] and [`credentials`] are the SECOND copy of the
/// signed in-process callback glue in this tree; the first is
/// `services/service-com-amazonaws-cloudformation-cloudformation/src/state/provisioning.rs`
/// (`sign_callback_request`, `dispatch`, `callback_credentials`), whose settings
/// must stay identical to the verifier's. Their shared home is the vended framework
/// beside that verifier, which cannot be edited from here, and `services/common`
/// would put the signer on every service in the workspace. Whoever lifts it takes
/// BOTH copies and CloudFormation's `tests/all/callback_signature.rs`, which pins
/// the settings, together — and a third caller must lift rather than copy again.
fn sign(
    request: &mut ::http::Request<Vec<u8>>,
    signing_name: &str,
    region: &str,
    access_key_id: &str,
    secret_access_key: &str,
) -> Result<(), String> {
    use ::aws_sigv4::http_request::{
        sign as sign_request, PayloadChecksumKind, SignableBody, SignableRequest, SigningSettings,
    };

    let identity: ::aws_smithy_runtime_api::client::identity::Identity =
        ::aws_credential_types::Credentials::builder()
            .provider_name("aws-emulator-sfn")
            .access_key_id(access_key_id)
            .secret_access_key(secret_access_key)
            .build()
            .into();
    let mut settings = SigningSettings::default();
    settings.payload_checksum_kind = PayloadChecksumKind::NoHeader;
    settings.excluded_headers = None;
    let params = ::aws_sigv4::sign::v4::SigningParams::builder()
        .identity(&identity)
        .region(region)
        .name(signing_name)
        .settings(settings)
        .time(SystemTime::now())
        .build()
        .map_err(|error| error.to_string())?;
    let body = request.body().clone();
    let signable_headers: Vec<(String, String)> = request
        .headers()
        .iter()
        .map(|(name, value)| {
            (
                name.as_str().to_string(),
                value.to_str().unwrap_or_default().to_string(),
            )
        })
        .collect();
    let signable = SignableRequest::new(
        request.method().as_str(),
        request.uri().to_string(),
        signable_headers
            .iter()
            .map(|(name, value)| (name.as_str(), value.as_str())),
        SignableBody::Bytes(&body),
    )
    .map_err(|error| error.to_string())?;
    let (instructions, _signature) = sign_request(signable, &params.into())
        .map_err(|error| error.to_string())?
        .into_parts();
    instructions.apply_to_request_http1x(request);
    Ok(())
}

#[cfg(test)]
mod tests;
