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
//! service has an entry stating its wire facts — an [`APIS`] one for a service an
//! optimized integration names, and an [`AWS_SDK_APIS`] one for a service only the AWS
//! SDK family names — which carries how a request to it is bound to HTTP ([`Wire`]).
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
//! A call the invoked service REFUSED is the task's own failure, named for the service
//! and for the exception that answer resolves to ([`Api::task_error`]). The exception
//! half is the answer's own error class, the same whichever `Resource` named the call;
//! the SERVICE half is the name the SDK that serves the naming FAMILY gives that
//! service ([`Family`]), which is why one refusal of one call reaches a definition as
//! `DynamoDB.ResourceNotFoundException` through the optimized family and as
//! `DynamoDb.ResourceNotFoundException` through the AWS SDK one. `ErrorEquals` matches
//! a name exactly, so that is the name a `Retry` or `Catch` on such a task carries.
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
//! ## The integrations whose work is an HTTP REQUEST
//!
//! Two `Resource`s name no operation of any service at all: Step Functions' own HTTP
//! Task (`arn:aws:states:::http:invoke`) and the API Gateway integration
//! (`arn:aws:states:::apigateway:invoke`). For each of them the unit of work is ONE HTTP
//! request the state's own parameters describe, and the task's result is that endpoint's
//! own response — so neither reads a wire fact of an [`Api`] ([`Call::endpoint`],
//! [`EndpointTask`]). The request itself and the reading of its answer are the HTTP
//! Task's ([`super::http_task`]); which stage of which api an API Gateway task addresses,
//! what authorizes its request and how a non-2xx answer is named to the state are that
//! integration's own ([`api_gateway`]).
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
//! ## The API Gateway invoke
//!
//! `arn:aws:states:::apigateway:invoke` names no operation of a service's API either:
//! the unit of work is one HTTP request to a DEPLOYED API STAGE, which the state's own
//! parameters address and describe, so it is read and made by the module that owns that
//! reading ([`api_gateway`]) over this same in-process transport.
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
//! * the walk records every state transition against the events one execution's
//!   history may hold ([`super::asl::MAX_HISTORY_EVENTS`]), which bounds how many
//!   calls ONE execution makes;
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

/// Amazon S3's own wire facts, kept out of [`APIS`] because S3 publishes no optimized
/// integration: it is reached as a `Map` state's dataset and through the AWS SDK family.
mod s3;
use s3::S3_API;

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

/// How many integration calls ONE walk may leave in flight while it walks on, for a walk
/// serving an inbound request; a walk already inside a chain of in-process calls may leave
/// fewer ([`Tasks::calls_in_flight`]).
///
/// Emulator bound. The calls a walk makes at one instant are the ones a `Map` state's
/// `MaxConcurrency` lets its items run together ([`super::asl::map_items`]), and that
/// figure is a caller's: a state declaring no limit asks for one call per item, of which
/// there may be as many as the dataset holds. Each in-flight call is work really running
/// in this process — a function's own code for a `lambda:invoke` task — so how many of
/// them this emulator runs beside each other is bounded here, and a state asking for more
/// concurrency than this runs that many at a time instead. It is wide enough for the
/// fan-out a definition is written for and far below what the host would have to hold.
pub const MAX_CALLS_IN_FLIGHT: usize = 16;

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

/// The ARN namespace, signing name and endpoint prefix of the API Gateway control planes,
/// which both of them answer on and are told apart by their paths.
///
/// It is the token the OPTIMIZED family names the API Gateway invoke integration by
/// ([`api_gateway`]), and the namespace the entries of both planes carry
/// ([`apigatewayv2::APIGATEWAYV2_API`]), so it is stated once here for every module that
/// reads it.
const APIGATEWAY_NAMESPACE: &str = "apigateway";

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

/// The first value `headers` carries `name` under, compared without regard to case as
/// HTTP field names are (RFC 9110); `None` for an answer that carried none.
///
/// `pub(crate)` and stated once here, because BOTH kinds of answer this crate reads a
/// header off hold their headers as the pairs the wire carried — the answer of an
/// integration call ([`Answered`]) and the answer of a request to an endpoint
/// ([`super::http_task`]) — and one comparison serves both.
pub(crate) fn carried_header<'a>(
    headers: &'a [(String, String)],
    name: &str,
) -> Option<&'a str> {
    headers
        .iter()
        .find(|(carried, _)| carried.eq_ignore_ascii_case(name))
        .map(|(_, value)| value.as_str())
}

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
    /// Its SDK id (`awsService.sdkId`), which — lower-cased — is the token an AWS SDK
    /// integration's `Resource` names this service by ([`Api::sdk_token`]), and which
    /// names the service in a diagnosis of work no call was made for. The service half
    /// of a task ERROR is the name the integration's own family writes
    /// ([`Api::error_prefix`], [`Api::sdk_error_prefix`]), which is not always this id.
    pub sdk_id: &'static str,
    /// The name Step Functions writes for this service in the error name of a failed
    /// OPTIMIZED `Task` (`SQS.QueueDoesNotExistException`): the service's SDK id for all
    /// but Step Functions itself, whose integration failures name it by its full service
    /// name rather than by the `SFN` id its SDK carries.
    pub error_prefix: &'static str,
    /// The name Step Functions writes for this service in the error name of a failed
    /// `Task` of the AWS SDK integration family (`DynamoDb.ResourceNotFoundException`):
    /// the name the AWS SDK that serves that family gives the service, which is the
    /// prefix its own client class carries. It is stated per entry for the same reason
    /// [`Api::sdk_client`] is — it is that SDK's own name for the service, which no
    /// Smithy model carries — and it is the SDK id itself wherever the two agree.
    pub sdk_error_prefix: &'static str,
    /// The name this service's SDK CLIENT carries, which is the name an exception it
    /// raises reports the service under ([`sdk_exception_cause`]). It follows the
    /// model's own service shape for most services and the SDK's own class name where
    /// the two differ, so it is stated per entry rather than derived.
    pub sdk_client: &'static str,
    /// The BASE exception class of that client: the one its deserializer falls back to
    /// for an error code no error shape of this service's model declares, which is the
    /// error name a `Task` state then reports ([`Api::exception_of`]).
    pub sdk_exception: &'static str,
    /// The status this service answers an UNAUTHORIZED call with, where the invoked
    /// operation models no access-denied error of its own — the refusal the role gate
    /// stands in for ([`super::task_authorization`]); an operation that MODELS one
    /// carries that error's own modelled status instead ([`Denied::status`]).
    ///
    /// No model declares the shape of that refusal: the authorization decision is taken
    /// before the operation is reached, so what answers it is the service's own front
    /// door rather than the operation. It is therefore a fact of the SERVICE, stated per
    /// entry — [`TARGET_FORBIDDEN_STATUS`] for a service that answers an authorization
    /// failure as a Forbidden, and [`TARGET_CLIENT_ERROR_STATUS`] for one whose front
    /// door answers it as a client error.
    pub access_denied_status: u16,
    /// The code its answer to such a call names the class by: the AWS-common
    /// [`COMMON_ACCESS_DENIED_CODE`] for most services, and its own code for one whose
    /// front door words it differently (Amazon SQS answers `AccessDenied`). A fact of the
    /// SERVICE for the same reason the status above is, so it is stated per entry.
    pub access_denied_code: &'static str,
    /// Whether its authorization layer spells the action it refused LOWER-CASED in that
    /// sentence, as Amazon SQS reports it (`sqs:sendmessage`). Only the SENTENCE is
    /// affected: IAM compares action names without regard to case, so the decision is the
    /// same either way.
    pub lowercases_refused_action: bool,
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
    /// Whether its model declares error SHAPES at all — the `errors` list of its
    /// operations (`smithy_query(query="operation")`).
    ///
    /// A service whose model declares NONE still refuses calls, with codes of its own
    /// that no shape of that model names: no client has a class for any of them, so its
    /// deserializer resolves EVERY refusal to the base exception
    /// ([`Api::sdk_exception`]), which is the error name a `Task` state then reports
    /// ([`Api::exception_of`]). True for all but one service here.
    pub models_errors: bool,
}

/// The code the AWS-common authorization refusal names its class by — "Common Errors":
/// the code a service answers an unauthorized call with where the invoked operation
/// models no access error of its own.
pub const COMMON_ACCESS_DENIED_CODE: &str = "AccessDeniedException";

impl Api {
    /// The code THIS service's front door names an authorization refusal by, where the
    /// invoked operation models no access-denied error of its own
    /// ([`Api::access_denied_code`]).
    #[must_use]
    pub fn access_denied_code(&self) -> &'static str {
        self.access_denied_code
    }

    /// `action` as THIS service's own refusal SPELLS it
    /// ([`Api::lowercases_refused_action`]).
    #[must_use]
    pub fn refused_action(&self, action: &str) -> String {
        match self.lowercases_refused_action {
            true => action.to_lowercase(),
            false => action.to_string(),
        }
    }

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
            // `awsQuery` and `ec2Query` carry the whole request as form fields.
            Wire::Query { .. } | Wire::Ec2Query { .. } => FORM_CONTENT_TYPE.to_string(),
        }
    }

    /// The REST binding of `operation`, for a service whose requests are bound that
    /// way; `None` for an AWS JSON or `awsQuery` service, whose every operation takes
    /// the same request shape, and for an operation of a REST service no binding is
    /// stated for.
    fn rest(&self, operation: &str) -> Option<&'static Rest> {
        match self.wire {
            Wire::Json { .. } | Wire::Query { .. } | Wire::Ec2Query { .. } => None,
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
    /// ([`COMMON_WIRE_CODES`]). An answer that names its class by a LEGACY QUERY CODE
    /// names the shape that service's model binds the code to, which is the shape its
    /// own client resolves the answer to ([`Wire::Query`]).
    fn exception_of(&self, shape: &str) -> String {
        // A service whose model declares no error shape at all has no class for any code
        // it answers, so its client resolves every refusal to the base exception
        // ([`Api::models_errors`]).
        if !self.models_errors {
            return self.sdk_exception.to_string();
        }
        let namespace = shape.rsplit_once('#').map(|(namespace, _)| namespace);
        let named = bare_name(shape);
        if namespace.is_some_and(|namespace| FRAMEWORK_NAMESPACES.contains(&namespace))
            || COMMON_WIRE_CODES.contains(&named)
        {
            return self.sdk_exception.to_string();
        }
        let named = self.shape_of_query_code(named);
        match named.ends_with(EXCEPTION_SUFFIX) {
            true => named.to_string(),
            false => format!("{named}{EXCEPTION_SUFFIX}"),
        }
    }

    /// The error SHAPE of this service's model that the class name `named` stands for:
    /// the shape a LEGACY QUERY CODE is bound to where this service answers its errors
    /// under one, and `named` itself everywhere else — a shape name, a code the model
    /// binds to a shape of that same name, and the answer of a service whose protocol
    /// names the shape directly.
    fn shape_of_query_code<'a>(&self, named: &'a str) -> &'a str {
        match self.wire {
            Wire::Query { exceptions, .. } => exceptions
                .iter()
                .find(|(code, _)| *code == named)
                .map_or(named, |(_, shape)| *shape),
            Wire::Json { .. } | Wire::Rest(_) | Wire::Ec2Query { .. } => named,
        }
    }

    /// The error name a `Task` state reports for a refusal this service answered under
    /// the error shape `shape`: this service as Step Functions names it in a task
    /// error of `family`, and the SDK exception that shape is deserialized into.
    pub fn task_error(&self, family: Family, shape: &str) -> String {
        format!("{}.{}", self.named_by(family), self.exception_of(shape))
    }

    /// The error name a `Task` state reports when this service refuses a call with an
    /// error none of its own shapes declares: this service as Step Functions names it
    /// in a task error of `family`, and its base SDK exception.
    pub fn base_error(&self, family: Family) -> String {
        format!("{}.{}", self.named_by(family), self.sdk_exception)
    }

    /// The service half of a failed task's error name, which is the name the SDK that
    /// serves `family` gives this service ([`Family`]).
    fn named_by(&self, family: Family) -> &'static str {
        match family {
            Family::Optimized => self.error_prefix,
            Family::AwsSdk => self.sdk_error_prefix,
        }
    }
}

/// Which of the two integration FAMILIES a `Resource` named a call by, which is what
/// decides the SDK a failed `Task`'s error name is written in ([`Api::task_error`]).
///
/// Step Functions serves the two families through different clients of the invoked
/// service, and an error name carries the service as the serving client names it: the
/// AWS SDK family (`arn:aws:states:::aws-sdk:<sdk name>:<action>`) is served by the AWS
/// SDK, so its errors carry that SDK's own name for the service
/// ([`Api::sdk_error_prefix`]), while an optimized integration
/// (`arn:aws:states:::<namespace>:<action>`) carries the name Step Functions' own
/// integration writes for it ([`Api::error_prefix`]).
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Family {
    /// `arn:aws:states:::<namespace>:<action>`, and the direct invocation of a Lambda
    /// FUNCTION's ARN, which is the same call the optimized `lambda:invoke` integration
    /// makes.
    Optimized,
    /// `arn:aws:states:::aws-sdk:<sdk name>:<action>`.
    AwsSdk,
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
        /// How the LEGACY QUERY CODE such an answer names an error class by maps to the
        /// error SHAPE of this service's model, for each code whose shape is not the
        /// code itself ([`Api::exception_of`] gives a shape the `Exception` suffix a
        /// class name ends in).
        ///
        /// An `awsQuery` refusal names its class by the `awsQueryError` code alone, and
        /// that code is a name of its own: the service's client resolves the answer to
        /// the class of the SHAPE its model binds the code to, which is what a `Retry`
        /// or a `Catch` then matches on. Each entry is `(code, shape)` as that service's
        /// model states the pair, so this table carries no naming rule of its own; a
        /// code whose shape name is the code's is not listed, because the suffix rule
        /// already answers for it.
        exceptions: &'static [(&'static str, &'static str)],
    },
    /// `ec2Query`: a FORM protocol like `awsQuery` and not the same one — a list is
    /// numbered under the field its own member goes under, the field names are the
    /// invoked model's per operation, and an answer wraps the operation's members in no
    /// result element. Amazon EC2's protocol, whose bindings and whose reading of an
    /// answer are stated beside its own entry ([`ec2`]).
    Ec2Query {
        /// The API version its model declares (`service.version`), which every request
        /// to such a service names beside the action.
        version: &'static str,
    },
}

/// The media type an `awsQuery` request body carries.
const FORM_CONTENT_TYPE: &str = "application/x-www-form-urlencoded";

/// The media type a `restJson1` request and the body of its answer carry.
const REST_CONTENT_TYPE: &str = "application/json";

/// The media type the request of a service whose REST protocol carries a payload member
/// as an XML document (`restXml`) sends it under.
const REST_XML_CONTENT_TYPE: &str = "application/xml";

/// The LIST members of one operation's payload document, each with the element its
/// entries are written under.
///
/// An XML list is its entries themselves, each under the element the list's own member
/// names (`<Path>` for CloudFront's `PathList`) — a fact of the INVOKED model, which this
/// crate cannot read, so it is stated by the binding that carries the document
/// ([`Body::XmlMember`]) exactly as the field names of a form's map member are
/// ([`FormMap`]). Empty for a payload carrying no list member.
///
/// An entry is keyed by the list member's own NAME, wherever in that payload the member
/// sits: one name is one list of one shape throughout a payload, which is what the
/// documents bound here are. A payload carrying two lists of the SAME member name whose
/// entries are written under DIFFERENT elements states no fact this can hold, and is a
/// payload no call is made for until the key is the path to the member rather than its
/// name.
pub type Lists = &'static [(&'static str, &'static str)];

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
    /// The same binding again for a service whose REST protocol carries that payload
    /// member as an XML DOCUMENT (`restXml`): the member's own value is what the
    /// document describes, and the document is the body ([`xml_body`]).
    XmlMember {
        /// The member the operation binds as its `httpPayload`.
        member: &'static str,
        /// The XML namespace a REQUEST's root element carries, which is the invoked
        /// service's own (`xmlNamespace`); nothing is read from an answer's.
        namespace: &'static str,
        /// The list members of that document ([`Lists`]).
        lists: Lists,
    },
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
/// `smithy_query(query="operation",operation="Invoke,CreateCodeSigningConfig,
/// GetCodeSigningConfig,ListCodeSigningConfigs",model_path="services/
/// service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`: `Invoke` is `http` POST
/// `/2015-03-31/functions/{FunctionName}/invocations`, `FunctionName` an `httpLabel`,
/// `Payload` the `httpPayload`, `InvocationType`, `LogType` and `ClientContext`
/// `httpHeader`s, `Qualifier` an `httpQuery`; the output's `StatusCode` is its
/// `httpResponseCode`, `Payload` its `httpPayload`, and `ExecutedVersion`,
/// `FunctionError` and `LogResult` its headers. The three code-signing operations bind no
/// payload member and no output member to the status or to a header: the write is POST
/// `/2020-04-22/code-signing-configs` (code 201) with every member in the body, the read
/// is GET `/2020-04-22/code-signing-configs/{CodeSigningConfigArn}` over that one
/// `httpLabel`, and the listing is GET `/2020-04-22/code-signing-configs` with `Marker`
/// and `MaxItems` `httpQuery` members — so each answer's body IS the operation's own
/// output document.
const LAMBDA_REST: &[Rest] = &[
    Rest {
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
    },
    Rest {
        operation: "CreateCodeSigningConfig",
        method: "POST",
        uri: "/2020-04-22/code-signing-configs",
        request_body: Body::Document,
        headers: &[],
        query: &[],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    Rest {
        operation: "GetCodeSigningConfig",
        method: "GET",
        uri: "/2020-04-22/code-signing-configs/{CodeSigningConfigArn}",
        request_body: Body::Empty,
        headers: &[],
        query: &[],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
    Rest {
        operation: LIST_CODE_SIGNING_CONFIGS_OPERATION,
        method: "GET",
        uri: "/2020-04-22/code-signing-configs",
        request_body: Body::Empty,
        headers: &[],
        query: &[("Marker", "Marker"), ("MaxItems", "MaxItems")],
        response_body: Body::Document,
        response_status: "",
        response_headers: &[],
    },
];

/// The Lambda operation that lists the account's code signing configurations, as that
/// model spells it: an operation whose published permission names no resource type, which
/// is what its [`TARGET_RESOURCES`] entry states.
const LIST_CODE_SIGNING_CONFIGS_OPERATION: &str = "ListCodeSigningConfigs";

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

/// The status a service that answers an authorization failure as a FORBIDDEN carries,
/// which is what the metadata of such a refusal's message reports (`Status Code: 403`).
///
/// Amazon SQS, Amazon SNS, AWS Lambda, Amazon S3 and AWS STS each answer a call the
/// caller's policies do not allow this way, which is how a refusal of one of their calls
/// reaches a `Task` state ([`Api::access_denied_status`]). It is the INVOKED service's
/// status, not this one's, which is why it is named for the target.
const TARGET_FORBIDDEN_STATUS: u16 = 403;

/// The status an invoked service whose front door answers every client error as a BAD
/// REQUEST carries for the same refusal (`Status Code: 400`).
const TARGET_CLIENT_ERROR_STATUS: u16 = 400;

/// The legacy query codes Amazon SNS names an error class by whose SHAPE is spelled
/// differently, as `(code, shape)` ([`Wire::Query`]).
///
/// Read from that service's own model
/// (`smithy_query(query="operation",operation="Publish",model_path="services/
/// service-com-amazonaws-sns-amazonsimplenotificationservice/smithy-model/
/// com-amazonaws-sns-amazonsimplenotificationservice.json")`, and `query="shape"` for
/// the errors its other operations declare): every other error of SNS is bound to the
/// code that is its own shape name without the `Exception` suffix (`NotFound` ->
/// `NotFoundException`, `AuthorizationError` -> `AuthorizationErrorException`), which the
/// suffix rule already names, so this one pair is the whole table.
const SNS_QUERY_EXCEPTIONS: &[(&str, &str)] = &[("ParameterValueInvalid", "InvalidParameterValueException")];

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
        // S3 is reached through the AWS SDK family alone, which is the family a dataset
        // call's own refusal would be named in.
        family: Family::AwsSdk,
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
        endpoint: None,
        // The caller hands this call the object's own BYTES, already rendered: the
        // dataset this writes is the text that writer built ([`super::result_writer`]),
        // not a value out of a state's document.
        payload_document: false,
        // A dataset call is the `Map` state's own reading and writing rather than a
        // `Task` state's work, so no `Credentials` names a role for it.
        acting_as: None,
    })
}

/// One integration call: the service's API, and the operation of it the `Resource`'s
/// action named.
#[derive(Debug, Clone)]
pub struct Call {
    pub api: &'static Api,
    /// Which integration family the `Resource` named this call by, which decides the
    /// SDK a refusal of it is named in ([`Family`]).
    pub family: Family,
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
    /// instead ([`dataset_call`]), and for a call whose request is an SDK request a
    /// CLIENT wrote ([`aws_sdk_call`]), where a payload member is written as the text
    /// of the bytes it carries.
    pub payload_document: bool,
    /// Which of the two integrations whose work is a REQUEST TO AN ENDPOINT the state's
    /// own parameters describe this call is, rather than an API call to an operation of a
    /// service ([`EndpointTask`]); `None` for every call on an operation.
    ///
    /// For such a call NONE of the wire facts an [`Api`] states is read — no operation's
    /// input is built ([`wired`]), no response document is read as an operation's output
    /// ([`answer_of`]) and no [`TARGET_RESOURCES`] entry names a resource it acts on.
    ///
    /// The endpoint such a call names comes from a caller's own definition, and it stays
    /// INSIDE this process: the request is handed to the in-process transport every
    /// emulated service reaches another by, so nothing is resolved or dialled and an
    /// endpoint this emulator does not serve is answered by the front door's own refusal
    /// ([`super::http_task`], "Where the request goes").
    pub endpoint: Option<EndpointTask>,
    /// The ROLE this call is made under, for a `Task` state that declares `Credentials`:
    /// "Specify a target role the state machine's execution role must be able to assume"
    /// — Step Functions resolves that role's ARN, assumes it, and makes the integration
    /// call with the credentials the assume vends.
    ///
    /// `None` for every call made as the state machine's own execution role, which is
    /// every call a state declaring no `Credentials` makes and every call the service
    /// makes for itself. It decides both halves of acting as the role: WHICH role's
    /// policies the call is judged against ([`Tasks::authorize`]) and WHOSE session it is
    /// signed with ([`super::task_authorization::Acting::task_signing`]).
    pub acting_as: Option<String>,
}

/// The two `Resource` forms whose unit of work is one HTTP REQUEST to an endpoint the
/// state's own parameters describe, rather than a call on an operation of a service's
/// API ([`Call::endpoint`]).
///
/// Both are served over the in-process transport and report what the endpoint answered
/// under the same four members ([`super::http_task`]); what differs is WHICH endpoint
/// the parameters name, what authorization the request carries, and how a non-2xx answer
/// is named to the state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndpointTask {
    /// `arn:aws:states:::http:invoke`, Step Functions' own HTTP Task: the parameters
    /// name the absolute URL of any endpoint and the request is authorized by an
    /// EventBridge connection ([`super::http_task`]).
    Http,
    /// `arn:aws:states:::apigateway:invoke`, the optimized Amazon API Gateway
    /// integration: the parameters name a deployed API STAGE and the request is signed
    /// as the execution role where the parameters ask for it ([`api_gateway`]).
    ApiGateway,
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

/// The TYPE a parameter the invocation is built from must carry, as the diagnosis of
/// unusable parameters names it.
///
/// `Payload` targets a BLOB and is the operation's `httpPayload`
/// (`smithy_query(query="operation",operation="Invoke",model_path=
/// "services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`), and a blob's value on the wire is the
/// member's own text — so the type that field requires is the STRING it is deserialized
/// into, whatever JSON value a path resolved it to.
const PAYLOAD_MEMBER_TYPE: &str = "STRING";

/// The failure the task's own PARAMETERS are, for parameters the invoked operation
/// cannot be called with at all, or `None` for parameters that build a call.
///
/// `Payload` is "The JSON that you want to provide to your Lambda function as input"
/// (`smithy_query(query="doc",shape="InvocationRequest",member="Payload",model_path=
/// "services/service-com-amazonaws-lambda-awsgirapiservice/smithy-model/
/// com-amazonaws-lambda-awsgirapiservice.json")`) and it targets a BLOB, whose value is
/// the TEXT the invocation carries ([`PAYLOAD_MEMBER_TYPE`]): a parameter resolved to a
/// JSON string IS that text, and one resolved to an object or an array is the document
/// the invocation is made with, which this call writes as its text ([`Rest::wired`]).
/// Any OTHER bare JSON value — a number, a boolean — is neither, so parameters whose
/// `Payload` resolved to one name no invocation this call can be built from, and the task
/// fails on its own parameters before any request is sent.
///
/// The diagnosis is the one the service words for parameters it could not start a task
/// with: the parameters AS RESOLVED, and then the field that could not be used and the
/// TYPE that field requires — the member's own name out of those parameters and the type
/// the invoked operation gives it — because what a reader has to be told about such a
/// refusal is which field is unusable and what it takes, rather than the value one run
/// happened to hand it.
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
        Value::Object(_) | Value::Array(_) | Value::String(_) | Value::Null
    ) {
        return None;
    }
    Some(Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "{UNUSABLE_PARAMETERS_OPENING}{request}' could not be used to start the Task: \
             [The value for the field '{PAYLOAD_MEMBER}' must be a {PAYLOAD_MEMBER_TYPE}]"
        ),
    })
}

impl Call {
    /// The error name a failed `Task` reports for a refusal the invoked service
    /// answered under the error shape `shape`, named in the SDK this call's own
    /// integration family is served by ([`Api::task_error`]).
    #[must_use]
    pub fn task_error(&self, shape: &str) -> String {
        self.api.task_error(self.family, shape)
    }

    /// The error name a failed `Task` reports when the invoked service refuses the call
    /// with an error none of its own shapes declares ([`Api::base_error`]).
    #[must_use]
    pub fn base_error(&self) -> String {
        self.api.base_error(self.family)
    }

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
    /// for a call to a service's API, and the request itself for a `Resource` whose work
    /// is one HTTP request, which calls no operation of any service
    /// ([`Call::endpoint`]).
    ///
    /// `pub(crate)`, because the walk words one refusal of its own about a call: the one
    /// it declines to make a SECOND time ([`super::asl`]).
    pub(crate) fn described(&self) -> String {
        match self.endpoint {
            Some(EndpointTask::Http) => "the HTTP request this task makes".to_string(),
            Some(EndpointTask::ApiGateway) => {
                "the API Gateway request this task makes".to_string()
            }
            None => format!("the '{}' call of {}", self.operation, self.api.sdk_id),
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

/// The access request this call is subject to: the IAM action the invoked operation
/// publishes, and the ARN of the resource the request names, as addressed in the
/// Region `region` the call is made in.
///
/// The FIRST of the access requests the call is subject to ([`access_requests`]), for a
/// caller about a call over a single resource. `None` when it is subject to none.
pub fn access_request(call: &Call, request: &Value, region: &str) -> Option<(String, String)> {
    access_requests(call, request, region).into_iter().next()
}

/// EVERY access request this call is subject to: the IAM action the invoked operation
/// publishes, over each resource the request names, as addressed in the Region `region`
/// the call is made in.
///
/// `region` is the Region the call is made in, which is where a resource the request
/// named by NAME lives.
///
/// One access request for a call that acts on ONE resource, which is the usual case, and
/// one per resource for a call whose request names a resource per unit of work it carries
/// ([`TargetResource::every_named_in`]) — a publication of several entries is one call
/// over each of the buses its entries name, so each of them is decided. The same resource
/// named twice is one access request: naming it again asks for nothing further.
///
/// EMPTY when no [`TARGET_RESOURCES`] entry is carried for the integration, and when the
/// request names no resource at all — a request the invoked operation will itself refuse,
/// which is its own answer to give rather than an authorization decision.
#[must_use]
pub fn access_requests(call: &Call, request: &Value, region: &str) -> Vec<(String, String)> {
    let Some(targets) = call.targets else {
        return Vec::new();
    };
    // The resources are the ones the invoked operation's own INPUT names, which for a
    // `Resource` naming a function directly is not the document the state built: the
    // call is the same one on the same function either way, so it is judged on the
    // request that is actually made ([`Call::operation_input`]).
    let input = call.operation_input(request);
    // The action is the one the invoked service publishes for the operation
    // ([`Call::action`]), unless the entry states the permission that operation
    // PUBLISHES instead.
    let action = targets.action.map_or_else(|| call.action(), str::to_string);
    let mut asked: Vec<(String, String)> = Vec::new();
    for named in targets.every_named_in(&input) {
        // What the value spells is the entry's own fact ([`TargetNaming`]): a NAME is the
        // last field of this account's ARN for that resource in this Region, an ARN is
        // the resource itself, and a url or one of a service's own shorter forms is read
        // the way that service addresses the resource.
        let Some(resource) = targets.named.arn_of(&named, region) else {
            continue;
        };
        // A state machine is named by an unqualified ARN, a version ARN or an alias ARN
        // alike — three ways of naming ONE state machine to run — so the resource such a
        // call is taken on is the state machine itself, read the way this service's own
        // front door reads it (`crate::service::request_conditions::unqualified_arn`): a
        // grant of the action on a state machine therefore covers starting it by a version
        // or an alias ARN. Nothing else is affected, since no other ARN carries that
        // qualifier.
        let resource = crate::service::request_conditions::unqualified_arn(&resource).to_string();
        let asking = (action.clone(), resource);
        if !asked.contains(&asking) {
            asked.push(asking);
        }
    }
    asked
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
pub const RUN_A_JOB_PATTERN: &str = "sync";
pub const RUN_A_JOB_PATTERN_2: &str = "sync:2";

/// The `.waitForTaskToken` pattern, whose outcome is a `SendTaskSuccess` or
/// `SendTaskFailure` from outside the execution rather than a job this emulator
/// polls.
pub const CALLBACK_PATTERN: &str = "waitForTaskToken";

/// Which of the three patterns above `named` is — the text a `Resource` writes after
/// its action — or `None` for text naming none of them.
///
/// THE reading of that suffix: [`read`] asks it of the text after the action it parsed
/// out of a `Resource` (and refuses what this answers `None` for), and
/// [`named_pattern`] asks it of a `Resource` whose action it finds in the text, so the
/// pattern a task's call is performed under and the pattern a definition is JUDGED for
/// are decided by one function.
fn pattern_after_a_dot(named: &str) -> Option<&'static str> {
    [RUN_A_JOB_PATTERN, RUN_A_JOB_PATTERN_2, CALLBACK_PATTERN]
        .into_iter()
        .find(|pattern| *pattern == named)
}

/// The integration PATTERN `resource` names after its action, read from the TEXT
/// alone: one of the three above, or `None` for a `Resource` that names no pattern.
///
/// It is read from the text rather than through [`read`] because it is asked of a
/// definition before any call is resolved ([`super::asl::diagnose_task_resource`] asks
/// which patterns a workflow type can perform): a `Resource` naming an integration this
/// emulator cannot perform still names the pattern it asked for. The ACTION it reads the
/// suffix of is the last colon-delimited field the pattern could follow — which is the
/// field `read` parses the action out of — and the suffix itself is read by the one
/// function that reads it ([`pattern_after_a_dot`]).
pub fn named_pattern(resource: &str) -> Option<&'static str> {
    let opens = resource.find('.')?;
    let action = match resource[..opens].rfind(':') {
        Some(field) => &resource[field + 1..],
        None => resource,
    };
    let (_, named) = action.split_once('.')?;
    pattern_after_a_dot(named)
}

/// Whether `resource` asks for the callback (`.waitForTaskToken`) pattern.
///
/// Read from the `Resource` alone, so it can be asked BEFORE the call it names is
/// read: a callback task carries the heartbeat members an answer from outside the
/// execution can be bounded by, which a request-response task cannot owe
/// ([`super::asl`] decides a state's members from this).
pub fn asks_for_callback(resource: &str) -> bool {
    read(resource).is_ok_and(|call| call.callback)
}

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

    /// When the work holding `token` last reported PROGRESS: the instant the handover was
    /// recorded, moved on by every `SendTaskHeartbeat` quoting the token.
    ///
    /// Asked WHILE the call carrying the token is in flight, so the heartbeat window the
    /// scheduling state declared is measured from the last heartbeat rather than from the
    /// hand-over for ever. `None` where no unanswered handover of the token is held, which
    /// is a task waiting on no further heartbeat.
    fn progressed_at(&self, token: &str) -> Option<SystemTime>;
}

/// See [`CallbackHandovers`].
pub type Handovers = ::std::sync::Arc<dyn CallbackHandovers>;

/// Whether this emulator holds the ACTIVITY an ARN names, for the walk that reaches a
/// `Task` state whose `Resource` is one.
///
/// "An activity is a worker — a program — that you host": the task such a state
/// schedules is queued under the activity a `CreateActivity` call created, so a
/// `Resource` naming one that does not exist names no queue for a worker to poll. The
/// activities live in this service's own store and are reached through the engine the
/// operation holds ([`super::activity_resources`]), so the walk knows nothing of either
/// and only asks.
pub type Activities = ::std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

/// Whether the STATE MACHINE an ARN names is of the EXPRESS workflow type, for the
/// walk that reaches a `Task` state whose nested-execution integration names it.
///
/// The two workflow types are reported differently, so which one the child is decides
/// how the task runs it ([`run_child_synchronously`]). The state machines live in this
/// service's own store and are reached through the engine the operation holds
/// ([`super::express_machines`]), so the walk knows nothing of either and only asks.
pub type ExpressMachines = ::std::sync::Arc<dyn Fn(&str) -> bool + Send + Sync>;

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

/// One integration call that has been STARTED and has not answered yet
/// ([`Tasks::begin`]): the work is being done while the walk holds this, and
/// [`InFlight::settle`] is what takes its answer.
pub struct InFlight {
    /// The call that was started, and the request it was started with: what
    /// [`InFlight::is_for`] answers by, so the answer of a started call is never read as
    /// the answer of a different one.
    call: Call,
    request: Value,
    /// Where the answer arrives: the same channel [`Tasks::perform`] waits on.
    answered: ::std::sync::mpsc::Receiver<Answer>,
    /// The whole span the walk may wait for this call, as `within` bounded it, measured
    /// from the instant it was started.
    waited: Duration,
    /// When it was started, which is when the work it asks for BEGAN: the span a task
    /// reports is measured from here, so a call started while a sibling's was in flight
    /// reports the span it really took rather than what was left of it.
    began: ::std::time::SystemTime,
    /// The HEARTBEAT window this call's own task is alive inside, for a call carrying a
    /// callback token whose state declared one: `None` for every call nothing heartbeats,
    /// which is bounded by `waited` alone.
    alive: Option<Liveness>,
}

/// The HEARTBEAT window of a CALLBACK task, as the walk that hands the token over states
/// it: the token the call carries and the span the state's `HeartbeatSeconds` opens.
///
/// "If more time than the specified seconds elapses between heartbeats from the task, this
/// state fails with a States.Timeout error name": the member bounds the span between two
/// reports of progress, so it is stated apart from the `TimeoutSeconds` that bounds the
/// task's whole duration and is re-read while the call is in flight
/// ([`Tasks::perform_while_alive`]).
#[derive(Debug, Clone)]
pub struct Alive {
    /// The token the call carries, which a `SendTaskHeartbeat` quotes to report progress.
    pub token: String,
    /// The window `HeartbeatSeconds` opens, which each heartbeat starts again.
    pub window: Duration,
}

/// [`Alive`] with the records the last heartbeat is read back from, which is what an
/// in-flight call is really held to.
struct Liveness {
    alive: Alive,
    handovers: Handovers,
}

impl Liveness {
    /// What is LEFT of the heartbeat window at `now`: the last progress the work reported
    /// plus the window, less the instant this is asked at.
    ///
    /// Re-read at every turn of the wait, so a heartbeat that arrived since the last turn
    /// moves the bound on; `None` where nothing holds an unanswered handover of the token
    /// any more, which is a task waiting on no further heartbeat.
    fn left(&self, now: SystemTime) -> Option<Duration> {
        let progressed = self.handovers.progressed_at(&self.alive.token)?;
        let due = progressed.checked_add(self.alive.window)?;
        Some(due.duration_since(now).unwrap_or_default())
    }
}

impl InFlight {
    /// Whether this is the call `call` with `request` asks for, so the walk that started
    /// it takes exactly its own answer.
    #[must_use]
    pub fn is_for(&self, call: &Call, request: &Value) -> bool {
        ::std::ptr::eq(self.call.api, call.api)
            && self.call.operation == call.operation
            && self.call.invokes == call.invokes
            && self.call.endpoint == call.endpoint
            && self.call.callback == call.callback
            && self.request == *request
    }

    /// When the call was started, which is when the work it asks for began.
    #[must_use]
    pub fn began(&self) -> ::std::time::SystemTime {
        self.began
    }

    /// Hold this call to the HEARTBEAT window `alive` states as well, reading the last
    /// heartbeat back through `handovers`.
    ///
    /// `None` leaves the call held to its own span alone, which is every call nothing
    /// heartbeats.
    #[must_use]
    fn while_alive(mut self, alive: Option<(Alive, Handovers)>) -> Self {
        self.alive = alive.map(|(alive, handovers)| Liveness { alive, handovers });
        self
    }

    /// Wait for this call's answer, for what is left of the span it was given.
    ///
    /// The bound is the one [`Tasks::perform`] holds a call to, counted from the instant
    /// the call was STARTED: a call that has not answered inside it is the task's own
    /// timeout, and the thread the walk holds is released there.
    ///
    /// A call carrying a callback token whose state declared `HeartbeatSeconds` is held to
    /// that window TOO, and that window is re-read at every turn of the wait: each
    /// `SendTaskHeartbeat` on the token moves it on, so the member bounds the span between
    /// two reports of progress rather than the whole span the work takes, and a window
    /// that really did lapse closes the task at the instant it was due.
    pub fn settle(self) -> Answer {
        loop {
            match self.answered.try_recv() {
                Ok(answer) => return answer,
                Err(::std::sync::mpsc::TryRecvError::Disconnected) => {
                    return Err(unavailable(&self.call))
                }
                Err(::std::sync::mpsc::TryRecvError::Empty) => {}
            }
            let now = super::now();
            let left = self
                .waited
                .saturating_sub(now.duration_since(self.began).unwrap_or_default());
            if left.is_zero() {
                return Err(timed_out(&self.call, self.waited));
            }
            // What is LEFT of the heartbeat window, asked again on every turn: a window
            // nothing moved on has lapsed, and the task it belongs to is closed by the
            // bound its own state declared.
            let alive = match &self.alive {
                Some(liveness) => match liveness.left(now) {
                    Some(window) if window.is_zero() => {
                        return Err(stopped_heartbeating(&self.call, liveness.alive.window))
                    }
                    window => window,
                },
                None => None,
            };
            let turn = alive.map_or(left, |alive| alive.min(left));
            match self.answered.recv_timeout(turn) {
                Ok(answer) => return answer,
                // Whichever bound this turn ended on, the next turn decides what it was:
                // the whole span the call was given, or a heartbeat window that may have
                // been moved on while this turn waited.
                Err(::std::sync::mpsc::RecvTimeoutError::Timeout) => continue,
                Err(::std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    return Err(unavailable(&self.call))
                }
            }
        }
    }
}

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

    /// Make `call` with `request` as [`Tasks::perform`] does, held ALSO to the heartbeat
    /// window of the callback token it carries (`alive`).
    ///
    /// "This action resets the Heartbeat clock": the window is re-read while the call is
    /// in flight, so each `SendTaskHeartbeat` quoting the token moves the bound on and the
    /// member bounds the span between two reports of progress rather than the whole span
    /// the work takes — which is the one thing `HeartbeatSeconds` exists to do. A window
    /// nothing reports progress inside closes the task at the instant it was due.
    ///
    /// The default ignores the window, for a [`Tasks`] with no record of the hand-over to
    /// read a heartbeat back from: such a call is held to `within` alone, exactly as every
    /// call nothing heartbeats is.
    fn perform_while_alive(
        &self,
        call: &Call,
        request: &Value,
        within: Option<Duration>,
        alive: &Alive,
    ) -> Answer {
        let _ = alive;
        self.perform(call, request, within)
    }

    /// START `call` with `request` and answer what the walk takes its answer from later,
    /// without waiting for it here.
    ///
    /// It is what lets the items of one `Map` state's concurrency run their work AT THE
    /// SAME TIME: each item's call is started as the walk reaches it and the walk goes on
    /// to the next item, so the calls are in flight together and the answer each waits for
    /// is taken through [`InFlight::settle`] ([`super::asl::map_items`]). `within` bounds
    /// the wait exactly as it bounds [`Tasks::perform`]'s, counted from the instant the
    /// call was started.
    ///
    /// `None` means nothing was started and nothing was handed over — which is what the
    /// default answers, for a [`Tasks`] that performs one call at a time — so the caller
    /// performs the call instead.
    fn begin(&self, _call: &Call, _request: &Value, _within: Option<Duration>) -> Option<InFlight> {
        None
    }

    /// How many calls this walk may leave IN FLIGHT while it walks on, beside the one it
    /// is waiting for ([`Tasks::begin`]).
    ///
    /// The default is none, which is what a [`Tasks`] that starts nothing has: its calls
    /// are performed one at a time, so a caller that asks before leaving one in flight is
    /// told there is no room for it.
    fn calls_in_flight(&self) -> usize {
        0
    }

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

    /// Why the execution role this walk runs as may NOT have the CHILD WORKFLOW
    /// EXECUTIONS of a Distributed Map state started for it, or `None` when nothing of
    /// the kind can be shown.
    ///
    /// Step Functions starts those executions ITSELF, on the state machine the Map Run
    /// belongs to, so no invoked service answers this one and it fails the whole run
    /// before its first item: it is therefore decided on what the account HOLDS about
    /// the role ([`super::task_authorization::Evidence::HeldDocuments`]) rather than on
    /// the implicit deny an invoked service's own front door would apply to a call made
    /// as that role ([`Tasks::authorize`]). The default shows nothing, for a walk with
    /// no role to judge the start against.
    fn authorize_child_start(&self, _call: &Call, _request: &Value) -> Option<Failed> {
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

    /// Whether this emulator HOLDS the activity `arn` names, which is what a `Task`
    /// state scheduling a task for it needs ([`Activities`]).
    ///
    /// Asked BEFORE the task is scheduled: a `Resource` naming an activity that does
    /// not exist names no queue a worker could poll, so nothing is scheduled for it and
    /// the state fails. The default answers that it does, for a walk with no store to
    /// ask — which is evidence of nothing, so the task is scheduled exactly as it was
    /// before.
    fn activity(&self, _arn: &str) -> bool {
        true
    }

    /// Whether the state machine `arn` names is of the EXPRESS workflow type, for a
    /// `Task` state whose nested-execution integration must know how to run the child
    /// it names ([`ExpressMachines`]).
    ///
    /// Asked BEFORE the child is started, because it decides which call starts it: an
    /// Express child has no execution-scoped read for the run-a-job pattern's own
    /// reporting call to poll, so the task runs that child to completion in the call
    /// that starts it ([`run_child_synchronously`]). The default answers that it is
    /// not, for a walk with no store to ask — which is evidence of nothing, so the
    /// pattern is performed exactly as it was before.
    fn express_child(&self, _arn: &str) -> bool {
        false
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
    // and the pattern says what finishes the task. WHICH of the three it is, is
    // [`pattern_after_a_dot`]'s answer, so this reading and the create-time one
    // ([`named_pattern`]) cannot disagree, and text naming none of them is refused
    // below.
    let (action, pattern) = match action.split_once('.') {
        Some((action, pattern)) => (action, Some(pattern)),
        None => (action, None),
    };
    if action.is_empty() {
        return Err(format!(
            "'{shown}' names no action of its service, so nothing was run for it"
        ));
    }
    // The OPTIMIZED `apigateway:invoke` integration calls a deployed API STAGE rather
    // than an operation of a service's API: the stage, the path, the method, the headers,
    // the query and the body are the task's own parameters, so it is read there and none
    // of the per-operation wire facts below is read for it ([`api_gateway`]). The AWS SDK
    // family's token for the same service names that service's CONTROL plane instead, and
    // is resolved with every other operation call below.
    if !aws_sdk && service == APIGATEWAY_NAMESPACE {
        return api_gateway::read_invoke(&shown, action, pattern);
    }
    // Each family names the service by its OWN token: the optimized one by the
    // service's ARN namespace, the AWS SDK one by the name the SDKs give it
    // ([`Api::sdk_token`]), and two services are reached through the AWS SDK family
    // ALONE ([`sdk_or_optimized_api`]).
    let api = sdk_or_optimized_api(aws_sdk, service, action, &shown)?;
    // The run-a-job pattern is performed for the integrations [`RUN_A_JOB`] names
    // the reporting and stopping calls of; the callback pattern waits for a
    // `SendTaskSuccess` nothing inside the execution makes.
    let (job, parsed_output, callback) = match pattern.map(|named| (named, pattern_after_a_dot(named))) {
        None => (None, false, false),
        Some((pattern, Some(RUN_A_JOB_PATTERN | RUN_A_JOB_PATTERN_2))) => {
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
        Some((_, Some(CALLBACK_PATTERN))) => (None, false, true),
        Some((pattern, _)) => {
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
        Wire::Json { .. } | Wire::Query { .. } | Wire::Ec2Query { .. } => None,
        Wire::Rest(_) => Some(api.rest(&operation).ok_or_else(|| {
            format!(
                "'{shown}' names the '{operation}' operation of {}, whose request this emulator \
                 does not bind to that service's REST protocol, so nothing was run for it",
                api.sdk_id
            )
        })?),
    };
    // The `ec2Query` protocol spreads a request over form fields under names the invoked
    // model gives each member per operation, so a call is made only for an operation
    // whose own bindings this emulator carries ([`ec2::form`]) — exactly as a REST
    // service's is above.
    if matches!(api.wire, Wire::Ec2Query { .. }) && ec2::form(&operation).is_none() {
        return Err(format!(
            "'{shown}' names the '{operation}' operation of {}, whose request this emulator does \
             not bind to that service's form-field protocol, so nothing was run for it",
            api.sdk_id
        ));
    }
    Ok(Call {
        api,
        // Which family the `Resource` read as is what names a refusal of this call.
        family: match aws_sdk {
            true => Family::AwsSdk,
            false => Family::Optimized,
        },
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
        endpoint: None,
        // The request IS the document this state built, so its payload member carries
        // the text of the value that document holds for it.
        payload_document: true,
        // WHO the call is made as is the `Task` state's to say, not the `Resource`'s: a
        // state declaring `Credentials` sets it on the call it asks for.
        acting_as: None,
    })
}

/// The service `service` names — read as the SDK's own token for it where `aws_sdk`, and
/// as its ARN namespace otherwise — among the services [`APIS`] states the wire facts of,
/// and those reached through the AWS SDK family ALONE beside them ([`AWS_SDK_APIS`]).
///
/// A service AWS publishes no OPTIMIZED integration for is named by the family's own
/// token and by nothing else, so `arn:aws:states:::<its namespace>:<action>` names no
/// `Task` state's work: those services' wire facts are stated apart from [`APIS`] — the
/// table the optimized family is looked up in — and are reached here. Amazon S3's
/// `Resource` of that form is a `Map` state's `ItemReader` and `ResultWriter` instead,
/// which read and write their dataset through [`dataset_call`].
///
/// `action` is the action the `Resource` named, for the one service whose set of CALLED
/// operations is narrower than the operations its bindings carry; `shown` is the
/// `Resource` as a cause may echo it ([`super::asl::echoed`]).
fn sdk_or_optimized_api(
    aws_sdk: bool,
    service: &str,
    action: &str,
    shown: &str,
) -> Result<&'static Api, String> {
    // S3 is called only for the operations whose ANSWER this emulator reads as that
    // operation's own output members.
    if aws_sdk && service == S3_API.sdk_token() {
        let named = operation_of(action);
        if !S3_SDK_OPERATIONS.contains(&named.as_str()) {
            return Err(format!(
                "'{shown}' names the '{named}' operation of {}, whose answer this emulator does \
                 not read as that operation's own output members, so nothing was run for it",
                S3_API.sdk_id
            ));
        }
        return Ok(&S3_API);
    }
    // Every other service reached through the AWS SDK family alone is looked up in the
    // table of those services, under that family's own token for it. Each is called for
    // the operations its own bindings carry, which the REST reading below holds every
    // operation of a REST service to ([`read`]); an AWS JSON or FORM service in it binds
    // every operation it models the same way, so the token reaches each of them.
    if aws_sdk {
        if let Some(api) = AWS_SDK_APIS
            .iter()
            .copied()
            .find(|api| api.sdk_token() == service)
        {
            return Ok(api);
        }
    }
    APIS.iter()
        .find(|api| match aws_sdk {
            true => api.sdk_token() == service,
            false => api.namespace == service,
        })
        .ok_or_else(|| {
            format!(
                "'{shown}' names an integration with '{}', which is not one of the services this \
                 emulator makes a 'Task' state's integration call to, so nothing was run for it",
                super::asl::echoed(service)
            )
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
///
/// The request such a caller hands the call is the invoked operation's OWN request as a
/// client WROTE it, rather than a document a `Task` state's payload template built, so a
/// member the operation binds as its `httpPayload` carries the BYTES of its text and the
/// request body is those bytes as they stand ([`Call::payload_document`]).
pub fn aws_sdk_call(service: &str, operation: &str) -> Result<Call, String> {
    // The family's own `Resource` form, which [`read`] is the one reading of; the
    // partition and the empty Region and account fields are the form's, and no field of
    // it is read as an address.
    let mut call = read(&format!(
        "arn:aws:{STATES_NAMESPACE}:::{AWS_SDK_SEGMENT}:{service}:{operation}"
    ))?;
    call.payload_document = false;
    Ok(call)
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
        // This service's own integration, named the optimized family's way.
        family: Family::Optimized,
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
        endpoint: Some(EndpointTask::Http),
        // No operation's input is built for this call at all ([`Call::endpoint`]), so
        // there is no payload member to put on the wire.
        payload_document: false,
        // Set by the `Task` state that declares `Credentials`.
        acting_as: None,
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
        // It is the same call on the same resource the OPTIMIZED `lambda:invoke`
        // integration makes, so a refusal of it is named that family's way.
        family: Family::Optimized,
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
        // The work is Lambda's own `Invoke` operation, bound to that service's REST
        // protocol above, rather than a request to an endpoint the parameters name.
        endpoint: None,
        // The function's EVENT is the document this state built
        // ([`Call::operation_input`]), and the payload member carries that document's
        // own text.
        payload_document: true,
        // Set by the `Task` state that declares `Credentials`.
        acting_as: None,
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
    family: Family,
    job: &'static RunAJob,
    job_id: &str,
    naming: &JobNaming,
) -> (Call, Value) {
    let mut call = pattern_call(api, family, job.describe);
    // What this call ANSWERS is what the task reports as its result, so the instants
    // in it are rendered as every other value a task reads from a call is: in the
    // SDK's own date type, whole epoch milliseconds ([`in_epoch_millis`]).
    call.epoch_millis = job.instants_at;
    (
        call,
        job_request(
            job.describe_member,
            Value::String(job_id.to_string()),
            naming,
        ),
    )
}

/// The INTEGRATION a nested execution of a state machine is started through, as the
/// optimized family names it.
const NESTED_EXECUTION_INTEGRATION: &str = "states:startExecution";

/// The operation that runs a nested EXPRESS execution to COMPLETION in the call that
/// starts it: "Starts a Synchronous Express state machine execution".
const SYNCHRONOUS_CHILD_OPERATION: &str = "StartSyncExecution";

/// The access request the call that runs an EXPRESS child to completion is subject to:
/// `states:StartExecution` on the child's state machine.
///
/// The permission is the nested-execution INTEGRATION's, whichever of this service's
/// calls the emulator runs the child with — the state's `Resource` names starting a
/// child execution, and that is what the execution role is asked for.
static SYNCHRONOUS_CHILD_TARGET: TargetResource = TargetResource {
    integration: NESTED_EXECUTION_INTEGRATION,
    action: Some("states:StartExecution"),
    member: "StateMachineArn",
    default: None,
    named: TargetNaming::Arn,
    denied: None,
};

/// Whether `resource` names the integration a nested execution of a state machine is
/// started through ([`NESTED_EXECUTION_INTEGRATION`]), read from the TEXT alone.
///
/// Asked of a definition before any call is resolved, beside [`named_pattern`]: which
/// INTEGRATION a `Resource` names decides whether the run-a-job pattern on it is a defect
/// of the submitted document or a failure of each execution
/// ([`super::asl::diagnose_task_resource`]). The integration is this service's OWN — the
/// job it starts is another execution of it — which is why it is read here rather than
/// spelled out at the caller.
pub fn names_nested_execution(resource: &str) -> bool {
    let named = match resource.split_once('.') {
        Some((before, _)) => before,
        None => resource,
    };
    named.ends_with(NESTED_EXECUTION_INTEGRATION)
}

/// The state machine a nested-execution run-a-job call would start a child of, as its
/// `request` names it; `None` for every other call.
///
/// Read BEFORE the call is made, because which workflow type that child is decides
/// which call runs it ([`run_child_synchronously`]).
#[must_use]
pub fn nested_child(call: &Call, request: &Value) -> Option<String> {
    let job = call.job?;
    if job.starts != NESTED_EXECUTION_INTEGRATION {
        return None;
    }
    call.targets?.named_in(request)
}

/// `call` as the call that runs its child to COMPLETION, for the nested-execution
/// run-a-job call it is.
///
/// An EXPRESS child's execution ARN carries the resource type `DescribeExecution`
/// refuses — "This API action is not supported by EXPRESS state machines" — so the job
/// the run-a-job pattern would poll cannot be reported at all. What the pattern
/// promises is that the task ends when the CHILD does, and this service's own API runs
/// an Express state machine to completion in ONE call, answering the same record of
/// the run that the report of a STANDARD child carries. So that is the call such a
/// task makes, and what it answers IS the job's report
/// ([`ran_to_completion`]): the task is not parked at a job, and what the child ended
/// as is read out of that one answer exactly as the poll of a parked job reads it.
#[must_use]
pub fn run_child_synchronously(call: &Call) -> Call {
    let mut synchronous = call.clone();
    synchronous.operation = SYNCHRONOUS_CHILD_OPERATION.to_string();
    // The access request stays the integration's own, and the resource it is over is
    // still the child's state machine.
    synchronous.targets = Some(&SYNCHRONOUS_CHILD_TARGET);
    // The answer IS the job's report rather than the literal SDK response of a call
    // that merely started one, so it carries the instants that report carries, in the
    // SDK's own date type, and no response envelope.
    synchronous.epoch_millis = call.job.map_or(&[], |job| job.instants_at);
    synchronous.sdk_envelope = false;
    synchronous
}

/// Whether `call` has already run its job to completion by the time it answers
/// ([`run_child_synchronously`]), which is what tells the task not to park at a job.
#[must_use]
pub fn ran_to_completion(call: &Call) -> bool {
    call.job.is_some() && call.operation == SYNCHRONOUS_CHILD_OPERATION
}

/// The call that STOPS the job `job` started under `job_id`, and its request.
///
/// `naming` is as [`describing`]'s. The identifier goes in as the stopping operation
/// types that member: a batch stop takes a LIST of ids, and the one job a task started
/// is a batch of one ([`RunAJob::stops_in_batch`]).
pub fn stopping(
    api: &'static Api,
    family: Family,
    job: &'static RunAJob,
    job_id: &str,
    naming: &JobNaming,
) -> (Call, Value) {
    let identifier = match job.stops_in_batch {
        true => Value::Array(vec![Value::String(job_id.to_string())]),
        false => Value::String(job_id.to_string()),
    };
    (
        pattern_call(api, family, job.stop),
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
fn pattern_call(api: &'static Api, family: Family, operation: &str) -> Call {
    Call {
        api,
        family,
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
        endpoint: None,
        // Every such call is to an AWS JSON service, whose request body IS the
        // operation's input document: no payload member is bound for one.
        payload_document: false,
        // The pattern's own calls, and the calls this service makes for itself, are made
        // as the execution role: no `Task` state's `Credentials` names a role for them.
        acting_as: None,
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
    // No `Resource` named this call, so no integration family did either: it is
    // addressed by the namespace the optimized family names a service by, and nothing
    // reports a refusal of it as a task's error.
    Some(pattern_call(api, Family::Optimized, operation))
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

/// The SERVICE and OPERATION a call this service made OUT is reported by, for work named
/// the way a `Task` state's own history events name it ([`named_work`]).
///
/// A call out of an instrumented service is reported by the name the AWS SDKs give the
/// service it addressed — that service's model's `sdkId` — and by the API operation the
/// call made, which is what a reader of the call sees beside the resource it addressed.
/// Both are read off the one table of wire facts the call itself was built from
/// ([`APIS`], [`AWS_SDK_APIS`]), so what names a called service in a trace of the run is
/// what names it in that task's own refusals and diagnoses.
///
/// `None` for work no call was made for: an action of a service this crate holds no wire
/// facts of.
#[must_use]
pub fn called_work(resource_type: &str, action: &str) -> Option<(&'static str, String)> {
    let (aws_sdk, service) = match resource_type.split_once(':') {
        Some((AWS_SDK_SEGMENT, service)) => (true, service),
        _ => (false, resource_type),
    };
    let api = sdk_or_optimized_api(aws_sdk, service, action, resource_type).ok()?;
    Some((api.sdk_id, operation_of(action)))
}

/// The same two names for the INVOCATION a `Resource` that is a Lambda function's ARN
/// makes: the task's unit of work is one `Invoke` of that function
/// ([`direct_invocation`]), so the call out it records is AWS Lambda's own.
#[must_use]
pub fn invoked_function_work() -> Option<(&'static str, String)> {
    let api = APIS
        .iter()
        .find(|api| api.namespace == LAMBDA_NAMESPACE)?;
    Some((api.sdk_id, LAMBDA_INVOKE_OPERATION.to_string()))
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
/// answer back — and several of a walk's calls may be in flight together, which is what
/// the items of a `Map` state's concurrency run their work at the same time through
/// ([`Tasks::begin`], [`MAX_CALLS_IN_FLIGHT`]).
///
/// One of these exists only INSIDE the blocking closure [`interpret`] spawns — it is
/// private, its field is private, and nothing else constructs one — which is what
/// makes the blocking wait below sound: a blocking wait from an async worker
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
    /// What decides whether the CHILD WORKFLOW EXECUTIONS of a Distributed Map state of
    /// this walk may be started for the execution role
    /// ([`super::task_authorization::Evidence::HeldDocuments`]); `None` leaves them
    /// unjudged, which is what a walk with no role to ask about does.
    child_starts: Option<RoleGate>,
    /// Where the handover of a callback token is recorded and its answer read back
    /// ([`CallbackHandovers`]); `None` for a walk with no store to record one in.
    handovers: Option<Handovers>,
    /// What answers which Map Run a Distributed Map state of this walk is already
    /// running ([`MapRuns`]); `None` for a walk that begins an attempt, whose every
    /// Map state starts a Map Run of its own.
    map_runs: Option<MapRuns>,
    /// What answers whether this emulator holds the activity a `Task` state's
    /// `Resource` names ([`Activities`]); `None` for a walk with no store to ask.
    activities: Option<Activities>,
    /// What answers whether a state machine a nested-execution task names is of the
    /// EXPRESS workflow type ([`ExpressMachines`]); `None` for a walk with no store to
    /// ask.
    express_machines: Option<ExpressMachines>,
    /// What decides a call a `Task` state asked to be made under a role of its OWN
    /// ([`super::task_authorization::TaskRoles`]); `None` for a walk with no execution
    /// role to assume such a role from.
    task_roles: Option<super::task_authorization::TaskRoles>,
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

/// What answers WHERE one integration call is addressed: the Region the call is made in
/// and the access request over its target is formed in, for a call whose target is named
/// by a value that carries no Region of its own; `None` leaves the call in the Region the
/// caller runs in.
///
/// A closure rather than a trait, for the reason [`RoleGate`] is one: the answer is read
/// out of the OWNING service's store through the engine the operation holds
/// ([`super::target_addressing`]), and the walk knows neither.
pub type Addressing = ::std::sync::Arc<dyn Fn(&Call, &Value) -> Option<String> + Send + Sync>;

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

/// What a task fails with when the operation running the execution has not taken the
/// calls already handed to it, so there is nowhere to hand this one.
///
/// The serving half takes each handover as it arrives and performs it beside the ones
/// already in flight ([`interpret_through`]), so the queue it is handed over through is
/// normally empty — this is reachable only when as many calls are queued as this emulator
/// queues for one walk and none of them has been taken. The walk is not held with them:
/// NOTHING is handed over for this call, so the failure is the one every other decline of
/// this module carries ([`super::asl::RUNTIME_ERROR`], as [`too_deep`] and [`unavailable`]
/// do) rather than ASL's timeout name, which names work that ran past a bound the state
/// declared for it.
fn still_calling(call: &Call) -> Failed {
    Failed {
        error: super::asl::RUNTIME_ERROR.to_string(),
        cause: format!(
            "{} could not be made while the calls this execution made before it have still not \
             been taken, so nothing was run for it",
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

/// What a task fails with when the HEARTBEAT window of the callback token its call carries
/// lapsed (`window`): the work holding the token went that long without reporting progress,
/// which is the same ASL timeout name a whole-task bound running out carries.
///
/// "If more time than the specified seconds elapses between heartbeats from the task, this
/// state fails with a States.Timeout error name" — so the window bounds the IDLE span
/// between two heartbeats, and only a window nothing moved on reaches here.
///
/// It is named by the HEARTBEAT name of that closure, which is the name the language
/// states for this bound alone: the conversion into what the state reports
/// ([`super::asl::reported`]) is where the closure takes the one name every bound that ran
/// out is reported under, and where the other name it answers to is attached — so a
/// catcher or a retrier naming either takes it over, and nothing has to tell a heartbeat
/// window's lapse from a `TimeoutSeconds`' by its cause.
fn stopped_heartbeating(call: &Call, window: Duration) -> Failed {
    Failed {
        error: super::asl::HEARTBEAT_TIMEOUT_ERROR.to_string(),
        cause: format!(
            "the task token {} carries went {window:?} without a heartbeat",
            call.described()
        ),
    }
}

impl Dispatcher {
    /// Hand `call` with `request` to the asynchronous task that makes it, and answer what
    /// the walk takes its answer from.
    ///
    /// The call is in flight from here: the serving half takes the handover as it arrives
    /// and performs it beside whatever else this walk has started
    /// ([`interpret_through`]), so what the caller decides is only whether it waits for
    /// the answer now ([`Tasks::perform`]) or later ([`Tasks::begin`]).
    fn hand_over_call(
        &self,
        call: &Call,
        request: &Value,
        within: Option<Duration>,
    ) -> Result<InFlight, Failed> {
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
        // WITHOUT a bound. The serving half takes every handover as it arrives, so the
        // buffered slots are normally free; a send that waited for one would hold this
        // blocking thread for however long the half that has not taken them is busy,
        // rather than for any ceiling this module states. So the handover is attempted
        // WITHOUT waiting and a queue that is full fails the task instead: nothing was
        // handed over for it, and nothing is faked.
        match self.calls.try_send(handover) {
            Ok(()) => {}
            Err(::tokio::sync::mpsc::error::TrySendError::Closed(_)) => {
                return Err(unavailable(call))
            }
            Err(::tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                return Err(still_calling(call))
            }
        }
        Ok(InFlight {
            call: call.clone(),
            request: request.clone(),
            answered,
            waited,
            began: super::now(),
            alive: None,
        })
    }
}

impl Tasks for Dispatcher {
    fn perform(&self, call: &Call, request: &Value, within: Option<Duration>) -> Answer {
        // The wait is CLAMPED server-side: whatever the call is doing, this thread
        // is released at that bound.
        self.hand_over_call(call, request, within)
            .map_or_else(Err, InFlight::settle)
    }

    fn perform_while_alive(
        &self,
        call: &Call,
        request: &Value,
        within: Option<Duration>,
        alive: &Alive,
    ) -> Answer {
        // The heartbeats of a callback task are reported against the hand-over this walk
        // recorded, so the window is read back through the same records a `SendTask*`
        // answers the token through: a walk with none of them cannot observe a heartbeat
        // at all, and its call is held to `within` alone.
        let watched = self
            .handovers
            .clone()
            .map(|handovers| (alive.clone(), handovers));
        self.hand_over_call(call, request, within)
            .map_or_else(Err, |in_flight| in_flight.while_alive(watched).settle())
    }

    fn begin(&self, call: &Call, request: &Value, within: Option<Duration>) -> Option<InFlight> {
        // A call that could not be handed over has started nothing, so the caller is told
        // nothing is in flight and performs it instead — which reports the refusal above
        // as the task's own failure rather than losing it here.
        self.hand_over_call(call, request, within).ok()
    }

    /// How many calls THIS walk may leave in flight: the figure for a walk serving an
    /// inbound request, halved for each level of in-process calls it already sits in.
    ///
    /// Every level of a chain holds a blocking thread per walk, and a call in flight may
    /// be an execution of its own ([`MAX_CALL_CHAIN_DEPTH`]), so what bounds the threads
    /// one inbound request can pin is the PRODUCT over the levels rather than the figure
    /// at any one of them. Halving per level keeps that product inside the blocking pool
    /// a runtime keeps by default, and never below one call, which is what lets the items
    /// of a `Map` state nested one level down run their work together at all.
    fn calls_in_flight(&self) -> usize {
        (MAX_CALLS_IN_FLIGHT >> u32::from(self.depth)).max(1)
    }

    fn authorize(&self, call: &Call, request: &Value) -> Option<Failed> {
        // A `Task` state that declares `Credentials` asks for the call to be made under
        // the role it names, so WHO the call is judged as is that role and not the
        // execution role: the assume this service has to take to act as it comes first,
        // and only then does that role's own gate decide the call. A walk with no such
        // seam at all — an empty execution role to assume FROM, or an engine carrying no
        // IAM store — shows nothing about either, and leaves the call unjudged exactly as
        // it leaves every other call of that walk unjudged (the module's "nothing is
        // refused on evidence this emulator does not have" rule).
        if let Some(target) = &call.acting_as {
            let decided = self.task_roles.as_ref()?(target);
            if let Some(refused) = decided.refused {
                return Some(refused);
            }
            return decided.gate.as_ref()?(call, request);
        }
        self.role.as_ref()?(call, request)
    }

    fn authorize_child_start(&self, call: &Call, request: &Value) -> Option<Failed> {
        self.child_starts.as_ref()?(call, request)
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

    fn activity(&self, arn: &str) -> bool {
        self.activities
            .as_ref()
            .is_none_or(|activities| activities(arn))
    }

    fn express_child(&self, arn: &str) -> bool {
        self.express_machines
            .as_ref()
            .is_some_and(|express| express(arn))
    }
}

/// Run `work` — one interpretation of a definition — on a blocking thread, serving
/// the integration calls its `Task` states make from this asynchronous call.
///
/// The walk is CPU work a caller's definition sizes, so it does not run on an async
/// worker; the calls it asks for are asynchronous, so they are made here, on the
/// operation's own task, with the walk waiting for each answer — and the calls a `Map`
/// state's concurrency starts for its items are served beside each other, so they run at
/// the same time ([`Tasks::begin`]). Both halves are
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
    interpret_through(
        ctx.callback(),
        chain_depth(ctx),
        region,
        // The credential store a session of a `Task` state's own `Credentials` role is
        // recorded in, which is the serving request's
        // ([`super::task_authorization::TaskSigning`]).
        Some(ctx.credential_store()),
        acting,
        work,
    )
    .await
}

/// As [`interpret`], from a callback handle and a chain depth rather than from a
/// request's own context.
///
/// A run-a-job pattern's outcome is not served by the request that started it: the
/// job is polled after that call answered, so the task that polls it holds its own
/// clone of the callback ([`super::run_a_job`]) and interprets the continuation
/// through this. Everything else is identical, including the bound on how deep a
/// chain of in-process calls may still make one.
///
/// `store` is where a session of a role a `Task` state's own `Credentials` names is
/// recorded; a caller with none — a continuation the clock runs rather than a request —
/// makes such a call under a session only where one is already held.
pub async fn interpret_through<Work, Outcome>(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    depth: u8,
    region: &str,
    store: Option<&::emulator_server::CredentialStore>,
    acting: super::task_authorization::Acting,
    work: Work,
) -> Result<Outcome, ::tokio::task::JoinError>
where
    Work: FnOnce(&dyn Tasks) -> Outcome + Send + 'static,
    Outcome: Send + 'static,
{
    // As many calls as this emulator carries in flight for one walk, plus the one a
    // handover that has not been taken yet sits in: the walk hands over without waiting
    // ([`Dispatcher::hand_over_call`]), and the serving half takes each handover as it
    // arrives, so the queue is what keeps a handover from failing while the half that
    // serves it is between two of them.
    let (calls, mut handovers) =
        ::tokio::sync::mpsc::channel::<Handover>(MAX_CALLS_IN_FLIGHT + 1);
    let role = acting.role;
    let child_starts = acting.child_starts;
    let connections = acting.connections;
    let handed = acting.handovers;
    let resumed_map_runs = acting.map_runs;
    let known_activities = acting.activities;
    let express_machines = acting.express_machines;
    // WHERE each of this walk's calls is addressed, for a call whose target is named by a
    // value that carries no Region of its own ([`super::target_addressing`]).
    let addressing = acting.addressing;
    // WHO the calls of this walk are made as: a session of the state machine's
    // execution role, which is the identity Step Functions does an execution's work as
    // ([`super::execution_role_session`]).
    let signing = acting.signing;
    // And what serves a call a `Task` state asked to be made under a role of its OWN: the
    // gate that decides it, taken by the walk, and the session it is signed with, taken
    // here as each call is served ([`super::task_authorization::TaskRoles`]).
    let task_roles = acting.task_roles;
    let task_signing = acting.task_signing;
    let walk = ::tokio::task::spawn_blocking(move || {
        let dispatcher = Dispatcher {
            calls,
            depth,
            role,
            child_starts,
            handovers: handed,
            map_runs: resumed_map_runs,
            activities: known_activities,
            express_machines,
            task_roles,
        };
        work(&dispatcher)
    });
    let serve = async {
        // Each handover is served on a task of its own, so the calls one walk has handed
        // over are IN FLIGHT TOGETHER: that is what makes the items of a `Map` state's
        // concurrency really run at the same time ([`super::asl::map_items`]), and it is
        // also what lets this half go back to the queue while a target holds a call
        // (`GetActivityTask` is a long poll) instead of serving one call at a time. The
        // tasks are owned by this set, so none of them outlives the interpretation.
        let mut serving = ::tokio::task::JoinSet::new();
        while let Some(handover) = handovers.recv().await {
            // The tasks that have already answered are reaped here rather than left to
            // accumulate: the set holds exactly the calls still in flight.
            while serving.try_join_next().is_some() {}
            let callback = callback.clone();
            let connections = connections.clone();
            // WHO this call is signed as: a session of the state machine's execution
            // role, or — for a call a `Task` state asked to be made under a role of its
            // own — a session of THAT role, minted here because the role is one the walk
            // resolved as it ran. A call made under a task's own role is NEVER signed as
            // the execution role instead: the gate judged it as the named role, so signing
            // it as another identity would make a service resolve a caller the decision
            // was not taken for. A role no session could be minted for leaves the call
            // signed with this emulator's own credential, exactly as a walk with no
            // session at all does.
            let signing = match &handover.call.acting_as {
                Some(target) => task_signing.as_ref().and_then(|mint| mint(target, store)),
                None => signing.clone(),
            };
            // WHERE this call goes: the Region the resource its request names is held in,
            // by the same reading the gate that judged the call formed its access request
            // in ([`super::target_addressing`]), so a call is made in the Region it was
            // judged in — and the execution's own Region for every call whose target
            // names none.
            let region = addressing
                .as_ref()
                .and_then(|addressing| addressing(&handover.call, &handover.request))
                .unwrap_or_else(|| region.to_string());
            serving.spawn(async move {
                // The SERVING half is bounded by the same ceiling the waiting half is:
                // a call that has not answered inside it is abandoned here too, so
                // nothing holds the walk's answer past that bound. Nothing is faked for
                // an abandoned call: the walk reported the timeout itself.
                // The work of a `Resource` that names an HTTP REQUEST — Step Functions'
                // own HTTP Task, and the API Gateway integration — is a request to the
                // endpoint its own parameters name rather than a call on a service's
                // operation, so it is made by the module that owns that reading — over
                // the same transport, under the same ceiling, and reported to the walk
                // through the same seam.
                //
                // That transport is what makes the endpoint a caller's parameters name
                // safe to hand over here: EVERY arm below goes to
                // `EmbeddedEmulator::http_connector`, this emulator's own in-process
                // router, so no client is built, no name is resolved and no address is
                // dialled — an `ApiEndpoint` naming anything this emulator does not serve
                // reaches nothing and is answered by the front door's own refusal
                // (`super::http_task`, "Where the request goes"). There is therefore no
                // address for an allowlist to screen and nothing of the host on the
                // request: no header of the request being served travels on it, and the
                // only credential it carries is the emulated one that integration
                // authorizes its request with — the connection's own for the HTTP Task,
                // and a session of the execution role for the API Gateway one.
                let made = async {
                    match handover.call.endpoint {
                        Some(EndpointTask::Http) => {
                            super::http_task::invoke(
                                &callback,
                                connections.as_ref(),
                                &handover.request,
                            )
                            .await
                        }
                        // The API Gateway invoke goes over that same transport, to the
                        // stage the parameters name, signed as the execution role where
                        // the parameters ask for it ([`api_gateway`]).
                        Some(EndpointTask::ApiGateway) => {
                            api_gateway::invoke(
                                &callback,
                                &region,
                                &handover.request,
                                signing.as_ref(),
                            )
                            .await
                        }
                        None => {
                            call_api_as(
                                &callback,
                                &region,
                                &handover.call,
                                &handover.request,
                                signing.as_ref(),
                            )
                            .await
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
            });
        }
        // The walk has dropped its sender, so no further call is coming: the calls still
        // in flight are waited out here, which is what makes every call this
        // interpretation asked for over by the time it returns.
        while serving.join_next().await.is_some() {}
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
/// `answer`'s members under the spelling a service integration's result is written in,
/// which is PascalCase.
///
/// The result of a service integration is the invoked operation's response document,
/// and a `Task` state reads it in the integration's own convention whatever the
/// invoked model spells its own members in — so a `ResultSelector`, a `ResultPath`
/// or a later state's path reads `$.ExecutionArn` of an operation whose own member is
/// `executionArn`. The wire spelling belongs to the call, not to the result.
///
/// Only the operation's OWN members are re-spelled, for the reason [`wire_request`]
/// gives: a response also carries a caller's own DATA — the attributes of an item, the
/// output document of a child execution — and data is never re-spelled. What tells the
/// two apart below the top level is the wire TYPE DISCRIMINATOR
/// ([`crate::service::wire`]): a nested object carrying one is a structure of the
/// invoked model, so its own members are re-spelled too, while a document the response
/// carries is reported exactly as it came back. The discriminator itself is the
/// protocol's tag rather than a member of the response, so it is not part of the
/// result at any level.
pub fn task_result(answer: &Value) -> Value {
    let Some(members) = answer.as_object() else {
        return answer.clone();
    };
    let mut reported = Map::new();
    for (member, value) in members {
        if member == TYPE_DISCRIMINATOR {
            continue;
        }
        let value = match value {
            Value::Object(nested) if nested.contains_key(TYPE_DISCRIMINATOR) => {
                task_result(value)
            }
            other => other.clone(),
        };
        reported.insert(MemberCase::Pascal.spell(member), value);
    }
    Value::Object(reported)
}

/// The key the wire protocol names a structure's own shape in
/// ([`crate::service::wire::SWF_BASE_MODEL_NAMESPACE`]), which is what tells a
/// structure of the invoked model from a document the response carries.
const TYPE_DISCRIMINATOR: &str = "__type";

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
        let (call, request) = stopping(job.api, job.family, job.job, &job.job_id, &job.naming);
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
    call_api_as(callback, region, call, request, None).await
}

/// As [`call_api`], made AS the identity `signing` names: a SESSION of the state
/// machine's execution role, which is who an execution's work is really done by
/// ([`super::execution_role_session`]), so the service serving the call resolves that
/// role's session as its caller.
///
/// `None` signs with this emulator's own credential, which is what a call made outside
/// an execution's own work presents.
pub async fn call_api_as(
    callback: &smithy_server_runtime::EmbeddedEmulator,
    region: &str,
    call: &Call,
    request: &Value,
    signing: Option<&super::execution_role_session::RoleSession>,
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
    // The call goes to the endpoint the request's own target names when that target
    // carries one ([`addressed_region`]), and to the Region it is handed otherwise: the
    // caller's own, or — for a target named by a value that carries no Region at all —
    // the one it was resolved to before the call was handed over
    // ([`super::target_addressing`]).
    let addressed = addressed_region(call, request);
    let region = addressed.as_deref().unwrap_or(region);
    match dispatch(callback, api, region, built, signing).await {
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
                    // An `ec2Query` answer wraps them in no result element at all, and
                    // names each under the element its own model gives it ([`ec2`]).
                    Wire::Ec2Query { .. } if (200..300).contains(&answered.status) => {
                        ec2::ec2_result(&call.operation, &answered.payload)
                    }
                    _ => answer_of(
                        Called {
                            api,
                            family: call.family,
                            operation: &call.operation,
                        },
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
                Ok(document) => match function_error(call, &document, &answered.payload) {
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

/// The member of that envelope that carries the status the invoked service answered.
const SDK_HTTP_STATUS: &str = "HttpStatusCode";

/// The HTTP status the answer to one `Task` state's call came back with, read off the
/// RESULT that call answered.
///
/// The envelope the SDK puts the response's HTTP facts in ([`with_sdk_envelope`]) carries
/// it, and every value in that envelope is the answer's own — so this is the status the
/// invoked service really answered, which is what a trace of the call reports.
///
/// `None` for a result that keeps none of the answer's HTTP facts: the function payload a
/// `Resource` that is a function's ARN reports, the document a callback answered, or text
/// that is no document at all.
#[must_use]
pub fn answered_status(result: &str) -> Option<u16> {
    let document: Value = ::serde_json::from_str(result).ok()?;
    u16::try_from(
        document
            .get(SDK_HTTP_METADATA)?
            .get(SDK_HTTP_STATUS)?
            .as_u64()?,
    )
    .ok()
}

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
/// trace the handler raised reach the client rather than a sentence written here. WHICH
/// rendering of it the state reads is the integration's own: a `Resource` that named the
/// FUNCTION makes the invocation's answer the task's, so the document reaches the state as
/// the run answered it — `answered`, the response body's bytes, text and member order and
/// separators included — while the optimized integration's result is the SDK response,
/// whose `Payload` is a member this emulator read as a document, so there the state reads
/// that document as the integration renders it.
///
/// Pure, so the reading of every answer form is pinned by this module's own tests.
fn function_error(call: &Call, document: &Value, answered: &[u8]) -> Option<Failed> {
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
        // A `Resource` that named the FUNCTION makes the invocation's own answer the
        // task's, so the document it reports is the one the run ANSWERED, text and all:
        // the body's bytes as they came back, which is what the function's runtime wrote.
        // The optimized integration reports the SDK response instead, whose `Payload` is
        // a member this emulator read as a document — so what it reports is that document
        // as the integration renders it, which is the text below.
        Some(_) if call.invokes.is_some() && !answered.is_empty() => {
            String::from_utf8_lossy(answered).to_string()
        }
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
        SDK_HTTP_STATUS.to_string(),
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
/// `request_id` is the identifier the answer named the request it refused by in a HEADER,
/// which the SDK reports in the exception's metadata; an answer that names it in the error
/// ENVELOPE instead — which is where a form protocol carries it — is read out of the
/// document ([`envelope_request_id`]), and a refusal naming none anywhere is reported
/// without one rather than with an invented identifier. `query_code` is the legacy query
/// code the answer named the class by for a service whose model is query-compatible
/// ([`Api::query_compatible`]), which is the code such a service's own client reports.
///
/// Pure, so the reading of every answer form is pinned by this module's own tests.
fn answer_of(
    called: Called<'_>,
    status: u16,
    error_type: Option<&str>,
    payload: &[u8],
    request_id: Option<&str>,
    query_code: Option<&str>,
) -> Answer {
    let Called {
        api,
        family,
        operation,
    } = called;
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
    let named_request = request_id
        .map(str::to_string)
        .or_else(|| document.as_ref().and_then(envelope_request_id));
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
            api.task_error(family, &shape),
            // The code the exception reports is the one that service's own client
            // matches the class on: the legacy query code where the service names
            // one, and the error shape's name where it does not.
            sdk_exception_cause(
                called,
                &message,
                query_code.unwrap_or_else(|| bare_name(&shape)),
                status,
                named_request.as_deref(),
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

/// WHICH call an answer is being read for: the service whose API answered, the
/// integration family whose `Resource` named the call — the two that together decide
/// the error name a refusal is reported under ([`Api::task_error`]) — and the operation
/// it was made on.
#[derive(Debug, Clone, Copy)]
struct Called<'a> {
    api: &'a Api,
    family: Family,
    operation: &'a str,
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
/// metadata that answer carried, and that rendering is what Step Functions puts in the
/// task's cause — which is why a `Catch` path reading the cause finds the status and the
/// service in it. WHICH rendering is the serving client's
/// ([`sdk_exception_message`]). The ONE rendering of it: a refusal the authorization gate
/// makes on the service's behalf is reported the same way, through that same function. A
/// fact the answer did not carry is left out rather than invented: a refusal naming no
/// request is reported without one, and a refusal carrying no message of its own says
/// which call of which service came back under that code.
fn sdk_exception_cause(
    called: Called<'_>,
    message: &str,
    code: &str,
    status: u16,
    request_id: Option<&str>,
) -> String {
    let Called {
        api,
        family,
        operation,
    } = called;
    let message = match message.is_empty() {
        true => format!("{} refused the '{operation}' call with {code}", api.sdk_id),
        false => message.to_string(),
    };
    sdk_exception_message(api, family, &message, code, status, request_id)
}

/// One SDK exception as its own client renders it: the service's sentence, then the
/// metadata the answer carried.
///
/// Step Functions serves the two integration families through two different clients of
/// the invoked service ([`Family`]), and each generation of client renders a service
/// exception in its own form — so the cause carries the form the SERVING client wrote:
///
/// * the current AWS SDK's message is the sentence followed by ONE parenthesised group of
///   COMMA-separated fields — the service as that SDK's own client names it
///   ([`Api::sdk_error_prefix`]), the status, and the refused request — and then a second
///   group naming how many attempts the call took ([`sdk_attempt_metadata`]). It names no
///   error code and no proxy. It serves the AWS SDK family ([`current_sdk_serves`]);
/// * an OPTIMIZED integration is served by the earlier generation of clients,
///   whose message is the sentence followed by SEMICOLON-separated fields: the name that
///   client carries ([`Api::sdk_client`]), the status, the code the answer named the
///   class by, the refused request, and the proxy the call went through — there is none,
///   so `Proxy: null`.
///
/// Public to this crate because a refusal the emulator answers FOR the invoked service
/// ([`super::task_authorization`]) is the same exception reaching the state the same
/// way: one rendering, so the two cannot drift. `request_id` is the identifier the
/// answer named the request by, left out where there is none — a call that was never
/// made has no request to name.
pub fn sdk_exception_message(
    api: &Api,
    family: Family,
    message: &str,
    code: &str,
    status: u16,
    request_id: Option<&str>,
) -> String {
    match current_sdk_serves(family) {
        true => sdk_attempt_metadata(api.sdk_error_prefix, message, status, request_id),
        false => {
            let request = request_id.map_or_else(String::new, |id| format!("Request ID: {id}; "));
            format!(
                "{message}{SDK_SERVICE_FIELD}{}{SDK_FIELD_SEPARATOR}{SDK_STATUS_FIELD}{status}\
                 {SDK_FIELD_SEPARATOR}Error Code: {code}{SDK_FIELD_SEPARATOR}{request}Proxy: null)",
                api.sdk_client
            )
        }
    }
}

/// Whether the client Step Functions makes a `Task` of `family`'s call with is the CURRENT
/// AWS SDK, whose rendering of an exception [`sdk_attempt_metadata`] writes.
///
/// The AWS SDK family IS that SDK, whichever service it calls. An OPTIMIZED integration is
/// served by the client Step Functions' own integration with that service carries, and every
/// one of those is of the EARLIER generation, so which rendering a cause carries follows from
/// the family alone.
///
/// What shows that is the form a refusal of such a call closes with. A `Task` denied
/// `lambda:InvokeFunction` reports a cause closing with the earlier client's metadata: the
/// service as that client names it ([`Api::sdk_client`]), the fields semicolon-separated, the
/// code the answer named the class by, the refused request and `Proxy: null`, with no attempt
/// count beside them. That holds whether the `Resource` names the function's ARN or the
/// optimized `lambda:invoke` integration, which are the same call; the same invocation named
/// through the AWS SDK family (`arn:aws:states:::aws-sdk:lambda:invoke`) is made with the
/// current SDK and carries its form instead.
fn current_sdk_serves(family: Family) -> bool {
    matches!(family, Family::AwsSdk)
}

/// The field an SDK exception's own metadata names the status the answer carried by.
const SDK_STATUS_FIELD: &str = "Status Code: ";

/// The HTTP status a REFUSAL this service reported came back with, read off the cause it
/// states.
///
/// Both renderings of an SDK exception put the answer's own status in the exception's
/// metadata under [`SDK_STATUS_FIELD`] — which is why "a `Catch` path reading the cause
/// finds the status and the service in it" — so the status of a refused call is read back
/// from the one place the cause states it, and the reading is stated here beside the two
/// renderings that write it.
///
/// `None` for a cause that states no status: a failure no answer came back for (work this
/// emulator did not run, a call that timed out), and a task whose work ANSWERED and
/// reported a failure of its own, like a function that raised.
#[must_use]
pub fn refused_status(cause: &str) -> Option<u16> {
    let (_, metadata) = cause.rsplit_once(SDK_STATUS_FIELD)?;
    metadata
        .chars()
        .take_while(|character| character.is_ascii_digit())
        .collect::<String>()
        .parse()
        .ok()
}

/// How many attempts the client made for the call a refusal came back from, which the
/// current SDK's rendering reports last.
///
/// One: this emulator's client makes each integration call once and does not make it
/// again, so a refusal is the first attempt's. A `Retry` in the state's own definition
/// does not add to it — each retry is a fresh call, made by a client of its own.
const SDK_ATTEMPTS: u32 = 1;

/// The field the OPTIMIZED family's rendering opens its metadata with, and the separator
/// between its fields.
const SDK_SERVICE_FIELD: &str = " (Service: ";
/// See [`SDK_SERVICE_FIELD`].
const SDK_FIELD_SEPARATOR: &str = "; ";
/// The field that rendering names the refused request by.
const SDK_REQUEST_FIELD: &str = "Request ID: ";

/// The refusal `cause` as the CURRENT AWS SDK renders it, for a cause the EARLIER
/// generation of clients rendered.
///
/// One answer, two renderings ([`sdk_exception_message`]): the client an optimized
/// integration is served through states the metadata as semicolon-separated fields and
/// names the service by its own client's name, while the current SDK states them
/// comma-separated with the attempt count and names the service as that SDK does. What a
/// TRACE of the call reports is the current SDK's rendering, because the segments a
/// service records about its own calls are written through that SDK — so a cause the
/// earlier client rendered is re-rendered here, out of the fields it states and the table
/// those names come from, rather than reported in the other client's words.
///
/// `None` for a cause that is not that rendering: one the current SDK already rendered,
/// a failure no service answered at all, and a refusal naming a service this crate holds
/// no wire facts of.
#[must_use]
pub fn current_sdk_rendering(cause: &str) -> Option<String> {
    let (message, metadata) = cause.rsplit_once(SDK_SERVICE_FIELD)?;
    let mut fields = metadata.strip_suffix(')')?.split(SDK_FIELD_SEPARATOR);
    // The first field is the service as the serving client names it; the rendering of the
    // current SDK separates its own fields with commas, so a cause it rendered carries
    // its whole metadata in this one field and names no service of this form.
    let client = fields.next().filter(|client| !client.contains(','))?;
    let api = APIS
        .iter()
        .chain(AWS_SDK_APIS.iter().copied())
        .find(|api| api.sdk_client == client)?;
    let mut status = None;
    let mut request_id = None;
    for field in fields {
        if let Some(digits) = field.strip_prefix(SDK_STATUS_FIELD) {
            status = digits.parse::<u16>().ok();
        }
        if let Some(named) = field.strip_prefix(SDK_REQUEST_FIELD) {
            request_id = Some(named);
        }
    }
    Some(sdk_attempt_metadata(
        api.sdk_error_prefix,
        message,
        status?,
        request_id,
    ))
}

/// A refusal as the CURRENT AWS SDK renders it: `service`'s own sentence, the metadata
/// the answer carried, and how many attempts the call took ([`SDK_ATTEMPTS`]).
///
/// `pub(super)` because a KMS refusal this service relays ([`super::encryption`]) reached
/// it through a client of that same generation, so it is rendered here too rather than
/// worded a second time. `service` is the name that SDK's own client gives the service.
pub(super) fn sdk_attempt_metadata(
    service: &str,
    message: &str,
    status: u16,
    request_id: Option<&str>,
) -> String {
    let request = request_id.map_or_else(String::new, |id| format!(", Request ID: {id}"));
    format!(
        "{message} (Service: {service}, {SDK_STATUS_FIELD}{status}{request}) \
         (SDK Attempt Count: {SDK_ATTEMPTS})"
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

/// The identifier a refusal's own ENVELOPE names the request it answers by, under either
/// of the spellings a form protocol writes it in: `RequestId` in the `awsQuery` envelope
/// and `RequestID` in the `ec2Query` one, read out of the document the refusal was read as
/// ([`super::xml_answer::as_document`], which carries the envelope's own children beside
/// the error's).
///
/// It is the identifier the SDK reports in the exception's metadata for such a service,
/// which no header of those answers carries — so this is where the id a `Catch` path reads
/// out of the cause comes from.
fn envelope_request_id(document: &Value) -> Option<String> {
    ["RequestId", "RequestID"]
        .iter()
        .find_map(|member| document.get(*member).and_then(Value::as_str))
        .filter(|named| !named.is_empty())
        .map(str::to_string)
}

#[cfg(test)]
mod tests;

/// The in-process CALL an integration is made as: the signed request and the credential
/// it is signed with.
mod dispatching;
/// The catalogue of run-a-job (`.sync`) integrations, and the readings over it.
mod jobs;
pub use dispatching::*;
pub use jobs::*;

/// What a `Task` state's event publication carries, and what a rejected entry makes of it.
mod published_events;
pub use published_events::*;

/// The result schema of the operation an integration calls, which a MOCKED result is
/// judged against.
pub mod mock_result;

/// The wire facts of every service a `Task` state's integration call is made to.
mod apis;
pub use apis::*;

/// Amazon CloudFront's own wire facts, reached through the AWS SDK integration family
/// alone.
mod cloudfront;

/// AWS Account Management's own wire facts, reached through the AWS SDK integration
/// family alone.
mod account;

/// Amazon API Gateway V2's own wire facts, reached through the AWS SDK integration family
/// alone.
mod apigatewayv2;

/// Amazon RDS's own wire facts, reached through the AWS SDK integration family alone.
mod rds;

/// Amazon EC2's own wire facts and per-operation form bindings, reached through the AWS
/// SDK integration family alone.
mod ec2;

/// The optimized Amazon API Gateway integration: the `Task` state whose work is a
/// request to a deployed API stage.
mod api_gateway;

/// The BODY of a call to a service whose protocol carries a payload member as an XML
/// document.
mod xml_body;

/// How one call's REQUEST is rendered for the wire: the invoked operation's own
/// document, the HTTP request a REST protocol binds it to, and the fields a FORM
/// protocol spreads it over.
mod wire_request;
pub(crate) use wire_request::*;

/// WHERE an integration's request names the resource its call acts on, and the access
/// request over it the execution role is judged by.
mod target_resources;
pub use target_resources::*;
