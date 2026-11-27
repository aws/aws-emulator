// SPDX-License-Identifier: CC0-1.0

//! The AWS SDK a node handler imports: the `@aws-sdk/client-*` packages this runtime
//! supplies on the handler's resolver path, in JavaScript, over the one host function that
//! makes a call ([`super::aws_api`]).
//!
//! AWS Lambda's managed `nodejs18.x` and later runtimes ship the AWS SDK for JavaScript v3
//! in the runtime's own `node_modules`, so `require('@aws-sdk/client-s3')` resolves with no
//! dependency in the deployment package and the handler can call another service. These
//! modules are that directory's contents here. They are resolved LAST — after the
//! package's own `node_modules` and after the layers' — which is the order a managed
//! runtime resolves them in, so a function that ships its own copy of a client keeps it.
//!
//! What they do and do not do:
//!
//! * The module for `@aws-sdk/client-<token>` exports a `<Token>Client` whose `send`
//!   dispatches ONE call, and a command class for every `…Command` name asked of it. The
//!   command's `input` IS the operation's input document, which is what an AWS JSON
//!   protocol puts on the wire; the host builds, signs and dispatches the request
//!   ([`super::aws_api`]) and the answer's document is what `send` resolves with.
//! * It also exports the AGGREGATED client the same package publishes beside the bare one —
//!   `DynamoDB` beside `DynamoDBClient` — which carries one method per operation the service
//!   binds, named as the operation with its first character lower cased, taking that
//!   operation's input and answering a promise of its output (or calling a node callback when
//!   one is passed). A method of it sends the very command `send` takes, so a handler written
//!   either way makes one call made one way.
//! * A client's `config` is the RESOLVED configuration a v3 client reports: every option it
//!   was built with, with `region` normalised into the async provider a v3 client normalises
//!   it into — `await client.config.region()` answers the Region the client's calls are
//!   addressed to, which for a client built with none is the one the runtime exported to the
//!   run.
//! * A client's `config.credentials` is the PROVIDER a v3 client normalises whatever it was
//!   built with into — an async function answering the credentials its calls are signed with,
//!   which for a client built with none is the default chain's: the reserved credential
//!   variables the runtime exported to the run ([`super::runtime_environment`]). What a call
//!   is signed with is still decided from what the client was built with, so reading the
//!   provider changes nothing about the call.
//! * A refusal is thrown as an instance of the error class the service's own model names for
//!   it, with the `$metadata` the answer carried — the shape a v3 client throws — so a
//!   handler that catches one by its class or by its name reads the emulated service's own
//!   answer. A refusal naming a shape that model declares NO class for is thrown as the
//!   service's own base exception, under the code the answer reported; for a service whose
//!   refusals carry the legacy query vocabulary beside the modelled shape, that code is the
//!   legacy one its answer's own header named, which is the vocabulary a consumer of such a
//!   service matches on. The MEMBERS that error shape declares stand on the thrown error
//!   beside its message, because an error shape is a structure like any other: a handler reads
//!   `error.CancellationReasons`, or whichever members its model gives that class, the way it
//!   reads a command's own output.
//! * The names a package exports are the ones the SDK's own package for that service
//!   exports: the client, a command class per operation and an error class per error shape,
//!   read from the service's model ([`exports`]). They are exported as NAMES
//!   because an ES module's `import { … } from` binds them when the module is linked, which
//!   is before any of them is read.
//! * A service whose protocol this seam does not bind, and an operation name that is not
//!   one, are REFUSED: the call is reported as not made rather than answered with a
//!   made-up success.
//! * A `Date` a handler puts on a timestamp member travels as the seconds since the epoch
//!   an AWS JSON protocol carries a timestamp as, which is the form a v3 client sends one
//!   in — not the ISO text a `Date` reports itself as. An answer's own timestamp members come
//!   back the other way round: a member the called service's model types as a timestamp is
//!   handed over as the `Date` a v3 client materialises from the epoch seconds the answer
//!   carried ([`super::blob_members`] says WHICH of an answer's values those are), so a handler
//!   reads `d instanceof Date`, `d.getTime()` and `d.toISOString()` off one exactly as it does
//!   on a managed runtime.
//! * Nothing here is the containment. Which services can be reached, the bounds on a call
//!   and the signature it carries are all host code's ([`super::aws_api`], [`super::node`]).

use super::package::PackageModule;

/// The names one of these packages exports, read from the service's own model.
///
/// It is a child of this module because the names are part of what a package IS; the file sits
/// beside this one, which is what the path states. It is reachable from the rest of the
/// execution tree (`super::sdk_model`) because the same reading of a service's model answers
/// what the PYTHON SDK's exception classes are built from.
#[path = "node_sdk_exports.rs"]
pub(super) mod exports;

/// The prefix of the package names this runtime answers for, which is the one namespace the
/// AWS SDK for JavaScript v3 publishes its service clients under.
const CLIENT_PACKAGE_PREFIX: &str = "@aws-sdk/client-";

/// The modules this runtime puts on a node handler's resolver path, under the ids the
/// runtime's own `node_modules` is keyed by.
///
/// The client packages themselves are not listed: the SDK publishes one package per AWS
/// service and a managed runtime bundles them all, so the package for any
/// `@aws-sdk/client-<token>` is answered by [`runtime_module_source`] rather than written
/// out here.
#[must_use]
pub fn runtime_modules() -> Vec<PackageModule> {
    Vec::new()
}

/// The source of the runtime-supplied module `id` names, or `None` when this runtime
/// supplies none.
///
/// `id` is the id of an entry of the runtime's own `node_modules`, so the name asked for is
/// the package name a bare specifier resolved to — or, for a package of this runtime, one of
/// its own subpaths.
#[must_use]
pub fn runtime_module_source(id: &str) -> Option<String> {
    let entry = id
        .strip_prefix(super::node::RUNTIME_ID_PREFIX)?
        .strip_prefix("node_modules/")?;
    // A package's manifest is a subpath of the package: the SDK's packages export
    // `./package.json`, which is how a handler reads the version of the client it is running
    // against (`import pj from '@aws-sdk/client-sqs/package.json'`).
    if let Some(package) = entry.strip_suffix(MANIFEST_SUBPATH) {
        return supplies_package(package).then(|| manifest_source(package));
    }
    package_source(entry)
}

/// The subpath a package's own manifest is imported by.
const MANIFEST_SUBPATH: &str = "/package.json";

/// Whether this runtime supplies the package `package` names — the question the manifest arm
/// above asks, which needs no module built for it.
fn supplies_package(package: &str) -> bool {
    package == DOCUMENT_CLIENT_PACKAGE
        || package == DYNAMODB_UTIL_PACKAGE
        || service_token(package).is_some()
}

/// The service token the client package `package` names, or `None` when it is not a client
/// package of the SDK's own namespace.
///
/// A token is a service's SDK id, lower cased with its words hyphenated, so it carries ASCII
/// letters, digits and hyphens and nothing else. Its length is bounded because it comes out of
/// the guest's own specifier: a token longer than any service's SDK id names no package.
fn service_token(package: &str) -> Option<&str> {
    let token = package.strip_prefix(CLIENT_PACKAGE_PREFIX)?;
    let named = !token.is_empty()
        && token.len() <= MAX_SERVICE_TOKEN
        && token.bytes().all(|byte| byte.is_ascii_alphanumeric() || byte == b'-');
    named.then_some(token)
}

/// The source of the package `package` names, or `None` when this runtime supplies no such
/// package.
fn package_source(package: &str) -> Option<String> {
    if package == DOCUMENT_CLIENT_PACKAGE {
        return Some(DOCUMENT_CLIENT_SOURCE.to_string());
    }
    if package == DYNAMODB_UTIL_PACKAGE {
        return Some(DYNAMODB_UTIL_SOURCE.to_string());
    }
    Some(client_source(service_token(package)?))
}

/// The longest service token a package name may carry.
///
/// Every token the SDK publishes is far shorter than this; the bound is here because the token
/// comes out of the guest's own specifier, so its length is the host's to decide and not the
/// guest's.
const MAX_SERVICE_TOKEN: usize = 64;

/// The manifest of one package this runtime supplies.
///
/// A manifest states what the package IS: its name, and the generation of the SDK whose
/// client, command and error classes it holds — the names the package exports. No release of
/// the published package is claimed beyond that generation, because nothing else of one is
/// here.
fn manifest_source(package: &str) -> String {
    ::serde_json::json!({ "name": package, "version": SDK_GENERATION })
        .to_string()
}

/// The version the manifest of a package of this runtime states: the SDK GENERATION it
/// implements, which for the `@aws-sdk/*` namespace is the AWS SDK for JavaScript v3 — the
/// generation whose client, command and error classes these packages hold. It is not a
/// published release of the package: no release of one is here to name.
const SDK_GENERATION: &str = "3.0.0";

/// One service client package, for the service `token` names: the client, the command class
/// of every operation that service binds, and the error class of every error shape it
/// declares — the names the SDK's own package for it exports, read from that service's model
/// ([`exports`]).
fn client_source(token: &str) -> String {
    let exports = exports::client_exports(token);
    let client = client_name(token);
    // The AGGREGATED client's name is the bare client's without that suffix, which is how the
    // SDK names the pair: `DynamoDB` beside `DynamoDBClient`.
    let aggregated = client
        .strip_suffix("Client")
        .unwrap_or(client.as_str())
        .to_string();
    let exception = format!("{aggregated}ServiceException");
    CLIENT_SOURCE
        .replace("__SERVICE_TOKEN__", token)
        .replace("__CLIENT_NAME__", &client)
        .replace("__AGGREGATED_NAME__", &aggregated)
        .replace("__SERVICE_EXCEPTION__", &exception)
        .replace("__OPERATIONS__", &name_list(&exports.operations))
        .replace("__ERRORS__", &name_list(&exports.errors))
}

/// `names` as the JavaScript array literal the module reads them from.
fn name_list(names: &[String]) -> String {
    ::serde_json::to_string(names).unwrap_or_else(|_| "[]".to_string())
}

/// The class the `@aws-sdk/client-<token>` package exports its client under.
///
/// The SDK names it after the service's own SDK id with the spaces dropped — `S3Client`,
/// `DynamoDBClient` — and the package name is that id lower-cased, which loses the casing.
/// The services whose casing is not the plain one are listed here, by the name their own
/// package exports; every other token is the name with its first letter upper-cased, which
/// is what the SDK's own naming gives for a single-word id.
fn client_name(token: &str) -> String {
    const EXPORTED: &[(&str, &str)] = &[
        ("s3", "S3"),
        ("appconfig", "AppConfig"),
        ("appconfigdata", "AppConfigData"),
        ("dynamodb", "DynamoDB"),
        ("dynamodbstreams", "DynamoDBStreams"),
        ("sqs", "SQS"),
        ("sns", "SNS"),
        ("sts", "STS"),
        ("ssm", "SSM"),
        ("kms", "KMS"),
        ("iam", "IAM"),
        ("ec2", "EC2"),
        ("ecs", "ECS"),
        ("ecr", "ECR"),
        ("eks", "EKS"),
        ("efs", "EFS"),
        ("rds", "RDS"),
        ("rds-data", "RDSData"),
        ("sfn", "SFN"),
        ("ses", "SES"),
        ("sesv2", "SESv2"),
        ("eventbridge", "EventBridge"),
        ("cloudwatch", "CloudWatch"),
        ("cloudwatch-logs", "CloudWatchLogs"),
        ("cloudwatch-events", "CloudWatchEvents"),
        ("cloudformation", "CloudFormation"),
        ("cloudfront", "CloudFront"),
        ("api-gateway", "APIGateway"),
        ("apigatewayv2", "ApiGatewayV2"),
        ("apigatewaymanagementapi", "ApiGatewayManagementApi"),
        ("secrets-manager", "SecretsManager"),
        ("step-functions", "SFN"),
        ("service-catalog", "ServiceCatalog"),
        ("cognito-identity", "CognitoIdentity"),
        ("cognito-identity-provider", "CognitoIdentityProvider"),
    ];
    if let Some((_, named)) = EXPORTED.iter().find(|(package, _)| *package == token) {
        return format!("{named}Client");
    }
    let mut characters = token.chars();
    let head = characters
        .next()
        .map(|first| first.to_ascii_uppercase().to_string())
        .unwrap_or_default();
    format!("{head}{}Client", characters.as_str().replace('-', ""))
}

/// The package the SDK publishes the DynamoDB document client under.
const DOCUMENT_CLIENT_PACKAGE: &str = "@aws-sdk/lib-dynamodb";

/// The package the SDK publishes the DynamoDB value conversions under.
const DYNAMODB_UTIL_PACKAGE: &str = "@aws-sdk/util-dynamodb";

/// One service client package: the client, a command class for every operation, and an error
/// class for every error shape.
///
/// The service token is the package's own name — the one the SDK publishes that service's
/// client under — and it is what the host resolves the destination from. The operation and
/// error names are that service's own model's ([`client_source`]).
const CLIENT_SOURCE: &str = r##"
'use strict';

var SERVICE = '__SERVICE_TOKEN__';

function host() {
    return globalThis.__lambdaRuntime;
}

// `new <Op>Command(input)`: the operation this command addresses is its own class name
// without the `Command` suffix, which is how a v3 command names its operation.
function commandClass(name) {
    var operation = name.slice(0, name.length - 'Command'.length);
    var Command = function (input) {
        this.input = input === undefined || input === null ? {} : input;
    };
    Command.prototype.operation = operation;
    Object.defineProperty(Command, 'name', { value: name });
    return Command;
}

// The exception the client throws, as a v3 client builds one: a real Error — so it carries
// a stack — standing on the prototype of the class that names it, which is what makes
// `error instanceof <Shape>` and `error.constructor.name` answer with the shape the service
// refused with.
function exception(Class, name, options) {
    var held = options === undefined || options === null ? {} : options;
    var error = new Error(held.message === undefined ? '' : held.message);
    Object.setPrototypeOf(error, Class.prototype);
    error.name = name;
    error.$fault = held.$fault === undefined ? 'client' : held.$fault;
    error.$metadata = held.$metadata === undefined ? {} : held.$metadata;
    return error;
}

// The base every error class of this package extends, which is the class the SDK names a
// refusal this package has no shape for.
function ServiceException(options) {
    return exception(ServiceException, '__SERVICE_EXCEPTION__', options);
}
ServiceException.prototype = Object.create(Error.prototype);
ServiceException.prototype.constructor = ServiceException;
Object.defineProperty(ServiceException, 'name', { value: '__SERVICE_EXCEPTION__' });

var classes = Object.create(null);
classes.__SERVICE_EXCEPTION__ = ServiceException;

// The class of one error shape, held so that the class a handler imported by name and the
// class a refusal is thrown as are the one class.
function errorClass(name) {
    var held = classes[name];
    if (held !== undefined) {
        return held;
    }
    var Thrown = function (options) {
        return exception(Thrown, name, options);
    };
    Thrown.prototype = Object.create(ServiceException.prototype);
    Thrown.prototype.constructor = Thrown;
    Object.defineProperty(Thrown, 'name', { value: name });
    classes[name] = Thrown;
    return Thrown;
}

// The classes a refusal carrying NO DOCUMENT is thrown as, by the status it answered.
//
// An answer with no body names no error shape — a `HEAD` "returns a generic 400 Bad Request,
// 403 Forbidden, or 404 Not Found HTTP status code. A message body isn't included, so you can't
// determine the exception beyond these HTTP response codes" (`HeadBucket`'s own documentation) —
// so what such a refusal carries in place of a shape is the STATUS itself. A v3 client has no
// shape to name the error it throws after and names it after that status: its reason phrase as
// one word, which is the name a handler branches on (`error.name === 'NotFound'` for the probe
// of something that is not there). Source: the three statuses that sentence names, each under
// its own reason phrase.
var STATUS_CLASSES = { '400': 'BadRequest', '403': 'Forbidden', '404': 'NotFound' };

// The class ONE refusal is thrown as: the error shape it named, or — for a refusal that named
// no shape because its answer carried no document — the class its status stands for above. A
// status none of them covers is left exactly as the answer named it rather than renamed here.
function statusClass(named) {
    var held = STATUS_CLASSES[named];
    return held === undefined ? named : held;
}

// The shape one of a refusal's carriers names: an error shape travels as `<shape>` or as
// `<namespace>#<shape>`, and the name a client reports is the shape's own. A carrier the
// answer did not fill names nothing.
function shapeNamed(carried) {
    return carried === undefined || carried === null
        ? ''
        : String(carried).split('#').pop().split(':')[0].trim();
}

// Whether this package has a CLASS for `name`: the error shapes the called service's own model
// declares are the classes the SDK's package for it exports ([`exports`]), and a refusal naming
// one of them is the one a v3 client throws a modelled class for.
function declaresError(name) {
    return name !== '' && ERRORS.indexOf(name) >= 0;
}

// The error a v3 client throws for a service's refusal: an instance of the class the error
// shape names, its message, and the metadata of the answer that carried them.
function serviceError(answered) {
    var document = {};
    try {
        document = JSON.parse(answered.body);
    } catch (error) {
        document = {};
    }
    // WHICH error the refusal is: the shape the answer's own document names in `__type`, else
    // the one the answer's error-type header named, else — for an answer carrying no document
    // at all — the class its status stands for.
    var named = shapeNamed(document.__type);
    if (named === '') {
        named = shapeNamed(answered.errorType);
    }
    var shape = statusClass(named);
    // A refusal of a QUERY-COMPATIBLE service carries TWO error vocabularies: the modern
    // Smithy shape in the document and the LEGACY code in the answer's own header. This
    // client's error classes are the called model's shapes, so a refusal naming one of them is
    // thrown as that class under that name — which is what `instanceof QueueDoesNotExist`
    // answers for. A refusal naming a shape the model declares NO class for has none to throw:
    // such a refusal is the base exception, under the legacy code the header carried, which is
    // the vocabulary a consumer of a query-compatible service matches on.
    var legacy = typeof answered.queryErrorCode === 'string' ? answered.queryErrorCode.trim() : '';
    var message = document.message === undefined
        ? (document.Message === undefined ? '' : document.Message)
        : document.message;
    var options = {
        message: message,
        $metadata: {
            httpStatusCode: answered.status,
            requestId: answered.requestId === undefined ? undefined : answered.requestId
        },
        $fault: answered.status >= 500 ? 'server' : 'client'
    };
    var error;
    if (declaresError(shape)) {
        // The class the shape names, which carries that shape as its own name — so a handler
        // reads one name off the error and off its class.
        error = new (errorClass(shape))(options);
    } else {
        // No class of this package names this refusal, so the base exception carries the code
        // the answer REPORTED: the legacy one where the answer carried it, else the shape its
        // own document named. An answer naming neither leaves the base exception's own name.
        error = new ServiceException(options);
        if (legacy !== '') {
            error.name = legacy;
        } else if (shape !== '') {
            error.name = shape;
        }
    }
    // The MEMBERS the refusal's own error shape declares stand on the error a v3 client throws:
    // an error shape is a structure like any other, so a handler reads
    // `error.CancellationReasons` — or whichever members its model gives that class — the way it
    // reads a command's own output. WHICH members one carries, and the key the wire carries each
    // of them in, is the called service's model's to say; the value is the service's own. A key
    // that differs from the member's only in CASE is the same member, which is how an AWS JSON
    // refusal spells its message.
    var carried = answered.errorMembers;
    if (carried !== null && typeof carried === 'object') {
        var folded = Object.create(null);
        var keys = Object.keys(document);
        for (var key = 0; key < keys.length; key += 1) {
            folded[keys[key].toLowerCase()] = document[keys[key]];
        }
        var members = Object.keys(carried);
        for (var at = 0; at < members.length; at += 1) {
            var wire = String(carried[members[at]]);
            var held = document[wire] === undefined ? folded[wire.toLowerCase()] : document[wire];
            if (held !== undefined) {
                error[members[at]] = held;
            }
        }
    }
    return error;
}

// The input members a handler may hand the TEXT of its bytes, by service: a client of one
// of these sends the UTF-8 of a string given for a member its model types as a blob, which
// is what a text body is, so the text becomes bytes here and `bytesSent` below encodes them
// with every other value a call carries bytes in.
var BLOB_PARAMETERS = { s3: ['Body'] };

// A native date a handler puts on a timestamp member, rendered the way the protocol carries
// one: an SDK takes a `Date` for a member the model types as a timestamp and sends the
// seconds since the epoch, where `JSON.stringify` on its own would write the ISO text a
// `Date` reports itself as. The walk copies plain objects and arrays only, so a value held
// as bytes — a typed array, a Buffer — is handed on exactly as it arrived.
function timestamps(value) {
    if (value instanceof Date) {
        return Math.floor(value.getTime() / 1000);
    }
    if (Array.isArray(value)) {
        return value.map(timestamps);
    }
    if (value !== null && typeof value === 'object') {
        var prototype = Object.getPrototypeOf(value);
        if (prototype !== Object.prototype && prototype !== null) {
            return value;
        }
        var rendered = {};
        Object.keys(value).forEach(function (name) {
            rendered[name] = timestamps(value[name]);
        });
        return rendered;
    }
    return value;
}

// Every value a call carries BYTES in, as the protocol carries one: a JSON protocol has no
// byte type, so a blob travels as the base64 text of its bytes, which is what an SDK sends
// for a member its service's model types as one. The bytes a handler holds are a typed
// array — a `Buffer`, a `Uint8Array`, any other view of an `ArrayBuffer`, the buffer
// itself — so one of those reached at ANY depth of the input is encoded, and every other
// value is handed on exactly as it arrived.
function bytesSent(value) {
    if (value instanceof Uint8Array) {
        return globalThis.Buffer.from(value).toString('base64');
    }
    if (value instanceof ArrayBuffer) {
        return globalThis.Buffer.from(new Uint8Array(value)).toString('base64');
    }
    if (ArrayBuffer.isView(value)) {
        return globalThis.Buffer
            .from(new Uint8Array(value.buffer, value.byteOffset, value.byteLength))
            .toString('base64');
    }
    if (Array.isArray(value)) {
        return value.map(bytesSent);
    }
    if (value !== null && typeof value === 'object') {
        var prototype = Object.getPrototypeOf(value);
        if (prototype !== Object.prototype && prototype !== null) {
            return value;
        }
        var sent = {};
        Object.keys(value).forEach(function (name) {
            sent[name] = bytesSent(value[name]);
        });
        return sent;
    }
    return value;
}

function sentInput(input) {
    input = timestamps(input);
    var members = BLOB_PARAMETERS[SERVICE];
    if (members !== undefined && input !== null && typeof input === 'object') {
        var held = {};
        Object.keys(input).forEach(function (name) { held[name] = input[name]; });
        members.forEach(function (member) {
            if (typeof held[member] === 'string') {
                held[member] = globalThis.Buffer.from(held[member], 'utf8');
            }
        });
        input = held;
    }
    return bytesSent(input);
}

// A member of an answer that carries BYTES is handed over the way a v3 client hands over a
// streamed body: the bytes are already here, so the two transforms resolve at once.
function streamedBody(base64Text) {
    var bytes = new Uint8Array(globalThis.Buffer.from(base64Text, 'base64'));
    return {
        transformToByteArray: function () { return Promise.resolve(bytes); },
        transformToString: function (encoding) {
            return Promise.resolve(
                globalThis.Buffer.from(bytes).toString(
                    encoding === undefined ? 'utf8' : encoding
                )
            );
        }
    };
}

// The member an answer binds to the WHOLE body whose blob is not a streaming one is handed
// over the way a v3 client hands over a body it COLLECTED: the caller is given the bytes
// themselves, as a byte array carrying the two transforms a collected body also answers. So a
// handler reads such a payload as bytes — `Buffer.from(payload)`, `payload.length`, an index —
// exactly as it does on a managed runtime, where only a streaming body is handed over unread.
//
// The transforms of a COLLECTED body answer their value directly, where a streamed body's
// answer a promise ([`streamedBody`]): the bytes are already read, so there is nothing for a
// caller to await.
function collectedBody(base64Text) {
    var bytes = new Uint8Array(globalThis.Buffer.from(base64Text, 'base64'));
    bytes.transformToByteArray = function () { return bytes; };
    bytes.transformToString = function (encoding) {
        return globalThis.Buffer.from(bytes).toString(encoding === undefined ? 'utf8' : encoding);
    };
    return bytes;
}

// The value ONE path reaches, replaced by what `convert` makes of it. `path` is the keys and
// indices that reach that value from the document's root, as the runtime read them out of the
// called operation's own model; a path this document does not hold is left alone, and so is a
// value `convert` answers `undefined` for — one that did not arrive in the form the protocol
// carries that kind of value in.
function materialise(document, path, convert) {
    if (!Array.isArray(path) || path.length === 0) {
        return;
    }
    var held = document;
    for (var at = 0; at < path.length - 1; at += 1) {
        if (held === null || typeof held !== 'object') {
            return;
        }
        held = held[path[at]];
    }
    if (held === null || typeof held !== 'object') {
        return;
    }
    var last = path[path.length - 1];
    var made = convert(held[last]);
    if (made !== undefined) {
        held[last] = made;
    }
}

// Every OTHER value an answer carries BYTES in is handed over as the byte array a v3 client
// gives a caller for a blob member, decoded from the base64 a JSON protocol travels it as.
function materialiseBytes(document, path) {
    materialise(document, path, function (held) {
        return typeof held === 'string'
            ? new Uint8Array(globalThis.Buffer.from(held, 'base64'))
            : undefined;
    });
}

// Every value an answer carries an INSTANT in is handed over as the `Date` a v3 client gives a
// caller for a member its service's model types as a timestamp: an AWS JSON protocol carries one
// as the SECONDS since the epoch — whole or fractional — which is that many thousand
// milliseconds, and a member whose own trait states the ISO text instead carries that text. A
// value in neither form is left exactly as it arrived rather than handed over as an invalid
// date.
function materialiseInstant(document, path) {
    materialise(document, path, function (held) {
        if (typeof held === 'number') {
            return Number.isFinite(held) ? new Date(held * 1000) : undefined;
        }
        if (typeof held !== 'string') {
            return undefined;
        }
        var parsed = new Date(held);
        return Number.isNaN(parsed.getTime()) ? undefined : parsed;
    });
}

// A call this runtime did not make at all is raised INSIDE the client, before any request is
// sent, so it is classified as what it is — the SDK's own refusal to make the call — and never
// under a name a SERVICE answers a request with: no service judged this call, so no code of any
// service's model describes it, and a caller that read one would retry or re-resolve
// credentials against a condition no request and no credential can change. The name is the one
// the runtime's other bundled SDK gives the same condition ([`super::aws_sdk_guest`]), so one
// condition carries one classification whichever language the handler is written in.
var CALL_NOT_MADE = 'SdkCallNotMade';

function notMade(answered) {
    var error = new Error(answered.detail === undefined || answered.detail === null
        ? 'The request was not made: ' + answered.outcome
        : answered.detail);
    error.name = CALL_NOT_MADE;
    if (answered.outcome === 'unknown_service') {
        error.name = 'NotImplemented';
        error.message = 'This emulator makes no call to ' + SERVICE
            + '; the services it reaches are: ' + answered.detail;
    }
    error.$metadata = {};
    return error;
}

function Client(configuration) {
    var held = configuration === undefined || configuration === null ? {} : configuration;
    var region = held.region === undefined ? globalThis.process.env.AWS_REGION : held.region;
    // The RESOLVED configuration, which is what a v3 client reports as its own: every option
    // it was built with, with the two a client normalises turned into the PROVIDERS it
    // normalises them into ([`credentialProvider`], [`regionProvider`]) — so an option this
    // seam does not read is still the option the handler passed, and `config.region` and
    // `config.credentials` are read the way a v3 client's are, by calling them.
    this.config = Object.assign({}, held, {
        region: regionProvider(region),
        credentials: credentialProvider(held.credentials),
        endpoint: held.endpoint,
        serviceId: SERVICE
    });
    // The credentials the client was built WITH, exactly as they were given: what a call is
    // signed with is decided from these ([`signedWith`]), and not from the provider above,
    // which a handler may read and which resolves the environment's for a client built with
    // none.
    this.builtWith = held.credentials;
    // The Region this client's calls are addressed to, resolved when the client was built.
    // Held beside the configuration because `config.region` is a provider, and answered to
    // the host as text ([`Client.prototype.send`]); it is not an option of the SDK's own
    // client, so it is not a member a handler reads off one.
    Object.defineProperty(this, 'inRegion', {
        value: typeof region === 'string' ? region : '',
        enumerable: false
    });
}

Object.defineProperty(Client, 'name', { value: '__CLIENT_NAME__' });

// The three reserved variables a managed runtime exports the invocation's own credentials in,
// which is where the SDK's DEFAULT credential chain reads them.
var CREDENTIAL_VARIABLES = ['AWS_ACCESS_KEY_ID', 'AWS_SECRET_ACCESS_KEY', 'AWS_SESSION_TOKEN'];

// WHAT `client.config.credentials` IS: a v3 client normalises whatever it was built with into
// a PROVIDER — an async function answering the credentials the client signs with — so a
// handler reads its own identity by calling it. A client built with a credentials object is
// given a provider answering that object; one built with a provider function keeps it; and one
// built with NONE is given the default chain's, which in a Lambda execution environment is the
// three reserved environment variables the runtime exported.
//
// A chain with no credentials to find answers the error the SDK's own chain answers for an
// environment holding none, rather than an empty credential a call would be signed with.
//
// The environment it reads is the RUN's own — the map the runtime built for this invocation
// ([`super::runtime_environment`]), which is all a guest has — and never this host's process
// environment, which no code here can reach.
function credentialProvider(credentials) {
    if (typeof credentials === 'function') {
        return credentials;
    }
    if (credentials !== undefined && credentials !== null) {
        return function () { return Promise.resolve(credentials); };
    }
    return function () {
        var environment = globalThis.process.env;
        var keyId = environment[CREDENTIAL_VARIABLES[0]];
        var secret = environment[CREDENTIAL_VARIABLES[1]];
        if (typeof keyId !== 'string' || keyId === ''
            || typeof secret !== 'string' || secret === '') {
            var missing = new Error('Could not load credentials from any providers');
            missing.name = 'CredentialsProviderError';
            return Promise.reject(missing);
        }
        var resolved = { accessKeyId: keyId, secretAccessKey: secret };
        var token = environment[CREDENTIAL_VARIABLES[2]];
        if (typeof token === 'string' && token !== '') {
            resolved.sessionToken = token;
        }
        return Promise.resolve(resolved);
    };
}

// WHAT `client.config.region` IS: a v3 client normalises the Region it was built with into a
// PROVIDER too — an async function answering the Region its calls are addressed to — which is
// what `await client.config.region()` reads. A client built with none is given the provider of
// the default chain, which in a Lambda execution environment answers the reserved Region
// variable the runtime exported to the run.
function regionProvider(region) {
    if (typeof region === 'function') {
        return region;
    }
    var resolved = typeof region === 'string' ? region : '';
    return function () { return Promise.resolve(resolved); };
}

// WHICH CREDENTIALS a client's calls are signed with, as the document the runtime signs with:
// the ones the client was built with (`new Client({ credentials: { accessKeyId,
// secretAccessKey, sessionToken } })`), which is how a handler calls a service with a session
// it was vended, and the empty document for a client built with none — whose calls are signed
// as the identity the invocation was given. Part of a credential is not a credential, and the
// SDK says so rather than signing as something else. A credentials PROVIDER — the function
// form the SDK also takes — is reported as a call this runtime does not make, naming it,
// rather than signed as the invocation's own identity.
function signedWith(credentials) {
    if (credentials === undefined || credentials === null) {
        return '';
    }
    if (typeof credentials === 'function') {
        var refused = new Error('This emulator makes no call with a credentials provider'
            + ' function; build the client with credentials: { accessKeyId, secretAccessKey,'
            + ' sessionToken }');
        refused.name = 'NotImplemented';
        refused.$metadata = {};
        throw refused;
    }
    if (typeof credentials !== 'object') {
        throw new TypeError('Credentials must carry an accessKeyId and a secretAccessKey');
    }
    var keyId = credentials.accessKeyId;
    var secret = credentials.secretAccessKey;
    if (typeof keyId !== 'string' || typeof secret !== 'string') {
        throw new TypeError('Credentials must carry an accessKeyId and a secretAccessKey');
    }
    return JSON.stringify({
        accessKeyId: keyId,
        secretAccessKey: secret,
        sessionToken: typeof credentials.sessionToken === 'string'
            ? credentials.sessionToken
            : ''
    });
}

// WHERE a client's calls are addressed: the endpoint the client was built with
// (`new Client({ endpoint: 'https://host/stage' })`), which is where the SDK sends every call
// of such a client. A client built with none is addressed at the service's own regional
// endpoint, which the empty value stands for. A `URL` — or anything else carrying the address
// written out under `href`, which is how the SDK reads such a value — is addressed at that;
// an endpoint given as parts or as a provider function is not one this runtime reads, so such
// a client is addressed as one built with none rather than at an address assembled here.
function addressedAt(endpoint) {
    if (typeof endpoint === 'string') {
        return endpoint;
    }
    if (endpoint !== null && typeof endpoint === 'object' && typeof endpoint.href === 'string') {
        return endpoint.href;
    }
    return '';
}

Client.prototype.send = function (command) {
    var self = this;
    return new Promise(function (resolve, reject) {
        var operation = command === null || command === undefined ? undefined : command.operation;
        if (typeof operation !== 'string') {
            reject(new TypeError('The argument must be a command of this client'));
            return;
        }
        var region = typeof self.inRegion === 'string' ? self.inRegion : '';
        var answered;
        try {
            answered = JSON.parse(host().awsCall(
                SERVICE,
                operation,
                region,
                JSON.stringify(sentInput(command.input === undefined ? {} : command.input)),
                signedWith(self.builtWith),
                addressedAt(self.config.endpoint)
            ));
        } catch (error) {
            reject(error);
            return;
        }
        if (answered.outcome !== 'answered') {
            reject(notMade(answered));
            return;
        }
        if (answered.status >= 400) {
            reject(serviceError(answered));
            return;
        }
        var document = {};
        try {
            document = answered.body === '' ? {} : JSON.parse(answered.body);
        } catch (error) {
            document = {};
        }
        var streams = answered.streams === undefined || answered.streams === null
            ? []
            : answered.streams;
        streams.forEach(function (member) {
            if (typeof document[member] === 'string') {
                document[member] = streamedBody(document[member]);
            }
        });
        var blobs = answered.blobs === undefined || answered.blobs === null
            ? []
            : answered.blobs;
        blobs.forEach(function (path) {
            materialiseBytes(document, path);
        });
        var collected = answered.payloadBlobs === undefined || answered.payloadBlobs === null
            ? []
            : answered.payloadBlobs;
        collected.forEach(function (member) {
            if (typeof document[member] === 'string') {
                document[member] = collectedBody(document[member]);
            }
        });
        var instants = answered.instants === undefined || answered.instants === null
            ? []
            : answered.instants;
        instants.forEach(function (path) {
            materialiseInstant(document, path);
        });
        document.$metadata = {
            httpStatusCode: answered.status,
            requestId: answered.requestId === undefined ? undefined : answered.requestId
        };
        resolve(document);
    });
};

Client.prototype.destroy = function () { return undefined; };

// The AGGREGATED client of this package: the SDK publishes `<Name>` beside `<Name>Client`,
// and that class extends the bare client with one METHOD per operation the service binds —
// the operation's own name with its first character lower cased — so a handler calls one as
// `new DynamoDB({}).putItem(input)` rather than through a command. A method sends the very
// command `send` takes, so the call it makes is the bare client's call.
function Aggregated(configuration) {
    Client.call(this, configuration);
    if (OPERATIONS.length > 0) {
        // Every method this client carries is installed below, out of the service's own
        // model, so a name it does not carry names no operation of that service: it answers
        // `undefined`, exactly as a misspelled export of this package does.
        return this;
    }
    // A service this emulator holds no model for states no operation list, so there is
    // nothing to install: the name asked for is what names the operation, which is the same
    // answer this package gives a `<Op>Command` name it holds no class for. Whether the call
    // is MADE is still the host's decision, so an operation or a service it does not bind is
    // reported as a call not made rather than as a member that is not there.
    return new Proxy(this, {
        get: function (target, name) {
            if (typeof name === 'string' && !(name in target) && namesOperation(name)) {
                return operationMethod(name.charAt(0).toUpperCase() + name.slice(1));
            }
            return target[name];
        }
    });
}
Aggregated.prototype = Object.create(Client.prototype);
Aggregated.prototype.constructor = Aggregated;
Object.defineProperty(Aggregated, 'name', { value: '__AGGREGATED_NAME__' });

// The names a promise resolution and a JSON serialization read off a value, which no
// operation of a service is named: they stay absent, so a client is never taken for a
// thenable and never asked to render itself.
var NOT_OPERATIONS = ['then', 'catch', 'finally', 'toJSON'];

// Whether `name` is of the form an operation method carries: an operation's own name with its
// first character lower cased.
function namesOperation(name) {
    return /^[a-z][A-Za-z0-9]*$/.test(name) && NOT_OPERATIONS.indexOf(name) < 0;
}

// The method one operation is called by. The input is the command's own, and the answer is
// what `send` resolves with; a node callback is called with that answer instead, which is the
// other form the SDK's aggregated client takes — its methods are `(input, options, callback)`,
// so the callback is whichever of the two trailing arguments is a function.
function operationMethod(operation) {
    return function (input, options, callback) {
        var done = typeof options === 'function' ? options : callback;
        var sent = this.send(new (commandFor(operation))(input));
        if (typeof done !== 'function') {
            return sent;
        }
        sent.then(
            function (answered) { done(null, answered); },
            function (error) { done(error); }
        );
        return undefined;
    };
}

// The command class of one operation, which is the very class this package exports for it, so
// the class a handler imported and the class a method sends are one class.
function commandFor(operation) {
    var name = operation + 'Command';
    if (members[name] === undefined) {
        members[name] = commandClass(name);
    }
    return members[name];
}

// The name the aggregated client carries one operation's method under.
function methodName(operation) {
    return operation.charAt(0).toLowerCase() + operation.slice(1);
}

// What this package exports, under the names the SDK's own package for this service exports
// them by: the client — `S3Client` for `client-s3`, `DynamoDBClient` for `client-dynamodb` —
// the aggregated client beside it — `S3`, `DynamoDB` — the command class of every operation
// the service binds, the error class of every error shape it declares, and the base exception
// the others extend. They are OWN members so that
// an `import { … } from` of this package, which binds its names when the module is linked,
// finds them. A name a handler asks for that this module holds no value for is answered as
// the kind of member it names — a command class for a `…Command`, the client for a `…Client`
// — and as nothing otherwise, so a typo is `undefined` rather than a stand-in.
var members = Object.create(null);
members.__CLIENT_NAME__ = Client;
members.__AGGREGATED_NAME__ = Aggregated;
members.__SERVICE_EXCEPTION__ = ServiceException;

var OPERATIONS = __OPERATIONS__;
var ERRORS = __ERRORS__;

(function () {
    var at = 0;
    var method = '';
    for (at = 0; at < OPERATIONS.length; at += 1) {
        members[OPERATIONS[at] + 'Command'] = commandClass(OPERATIONS[at] + 'Command');
        method = methodName(OPERATIONS[at]);
        // A member the bare client already carries is never shadowed: `send` is what an
        // operation method itself calls, so a method installed over it would call itself.
        if (!(method in Client.prototype)) {
            Aggregated.prototype[method] = operationMethod(OPERATIONS[at]);
        }
    }
    for (at = 0; at < ERRORS.length; at += 1) {
        members[ERRORS[at]] = errorClass(ERRORS[at]);
    }
})();

module.exports = new Proxy(members, {
    get: function (target, name) {
        if (typeof name !== 'string') {
            return target[name];
        }
        if (Object.prototype.hasOwnProperty.call(target, name)) {
            return target[name];
        }
        if (name.length > 'Command'.length
            && name.slice(name.length - 'Command'.length) === 'Command') {
            target[name] = commandClass(name);
            return target[name];
        }
        if (name.length > 'Client'.length
            && name.slice(name.length - 'Client'.length) === 'Client') {
            target[name] = Client;
            return target[name];
        }
        return undefined;
    },
    has: function (target, name) {
        return typeof name === 'string'
            && (Object.prototype.hasOwnProperty.call(target, name)
                || name.slice(name.length - 'Command'.length) === 'Command'
                || name.slice(name.length - 'Client'.length) === 'Client');
    }
});
"##;

/// `@aws-sdk/util-dynamodb`: the two conversions between a plain JavaScript value and the
/// attribute value DynamoDB's own wire form carries it as.
const DYNAMODB_UTIL_SOURCE: &str = r##"
'use strict';

// The attribute types DynamoDB names a value by: the scalar ones, the sets, and the two
// document types.
function convertToAttr(value, options) {
    var held = options === undefined || options === null ? {} : options;
    if (value === null) {
        return { NULL: true };
    }
    if (value === undefined) {
        if (held.removeUndefinedValues === true) {
            return undefined;
        }
        throw new Error('Pass options.removeUndefinedValues=true to remove undefined values');
    }
    if (typeof value === 'boolean') {
        return { BOOL: value };
    }
    if (typeof value === 'number') {
        if (!isFinite(value)) {
            throw new Error('Special numeric value ' + value + ' is not allowed');
        }
        return { N: String(value) };
    }
    if (typeof value === 'bigint') {
        return { N: value.toString() };
    }
    if (typeof value === 'string') {
        if (value === '' && held.convertEmptyValues === true) {
            return { NULL: true };
        }
        return { S: value };
    }
    if (value instanceof Uint8Array) {
        return { B: globalThis.Buffer.from(value).toString('base64') };
    }
    if (value instanceof Set) {
        return setAttribute(value, held);
    }
    if (Array.isArray(value)) {
        return {
            L: value
                .map(function (member) { return convertToAttr(member, held); })
                .filter(function (member) { return member !== undefined; })
        };
    }
    if (typeof value === 'object') {
        var map = {};
        var keys = Object.keys(value);
        for (var index = 0; index < keys.length; index += 1) {
            var converted = convertToAttr(value[keys[index]], held);
            if (converted !== undefined) {
                map[keys[index]] = converted;
            }
        }
        return { M: map };
    }
    throw new Error('Unsupported type passed: ' + typeof value);
}

// A Set is one of the three typed sets, decided by what its members are — which is the
// only thing that decides it, because DynamoDB has no mixed set type.
function setAttribute(value, options) {
    var members = Array.prototype.slice.call(value);
    if (members.length === 0) {
        if (options.convertEmptyValues === true) {
            return { NULL: true };
        }
        throw new Error('Pass a non-empty set, or options.convertEmptyValues=true');
    }
    if (members.every(function (member) { return typeof member === 'string'; })) {
        return { SS: members };
    }
    if (members.every(function (member) {
        return typeof member === 'number' || typeof member === 'bigint';
    })) {
        return { NS: members.map(String) };
    }
    if (members.every(function (member) { return member instanceof Uint8Array; })) {
        return {
            BS: members.map(function (member) {
                return globalThis.Buffer.from(member).toString('base64');
            })
        };
    }
    throw new Error('Only Number Set (NS), Binary Set (BS) or String Set (SS) are allowed');
}

function convertToNative(attribute, options) {
    var held = options === undefined || options === null ? {} : options;
    if (attribute === null || typeof attribute !== 'object') {
        throw new Error('No value defined: ' + attribute);
    }
    if (attribute.NULL !== undefined) {
        return null;
    }
    if (attribute.BOOL !== undefined) {
        return attribute.BOOL;
    }
    if (attribute.S !== undefined) {
        return attribute.S;
    }
    if (attribute.N !== undefined) {
        return held.wrapNumbers === true ? { value: attribute.N } : Number(attribute.N);
    }
    if (attribute.B !== undefined) {
        return new Uint8Array(globalThis.Buffer.from(attribute.B, 'base64'));
    }
    if (attribute.SS !== undefined) {
        return new Set(attribute.SS);
    }
    if (attribute.NS !== undefined) {
        return new Set(attribute.NS.map(Number));
    }
    if (attribute.BS !== undefined) {
        return new Set(attribute.BS.map(function (member) {
            return new Uint8Array(globalThis.Buffer.from(member, 'base64'));
        }));
    }
    if (attribute.L !== undefined) {
        return attribute.L.map(function (member) { return convertToNative(member, held); });
    }
    if (attribute.M !== undefined) {
        var out = {};
        var keys = Object.keys(attribute.M);
        for (var index = 0; index < keys.length; index += 1) {
            out[keys[index]] = convertToNative(attribute.M[keys[index]], held);
        }
        return out;
    }
    throw new Error('Unsupported type passed: ' + Object.keys(attribute).join(', '));
}

function marshall(value, options) {
    var converted = convertToAttr(value, options);
    return converted === undefined || converted.M === undefined
        ? converted
        : converted.M;
}

function unmarshall(item, options) {
    return convertToNative({ M: item === undefined || item === null ? {} : item }, options);
}

exports.convertToAttr = convertToAttr;
exports.convertToNative = convertToNative;
exports.marshall = marshall;
exports.unmarshall = unmarshall;
exports.NumberValue = function (value) { return { value: String(value) }; };
"##;

/// `@aws-sdk/lib-dynamodb`: the DOCUMENT client, which is the low-level client with the
/// attribute-value conversion applied to the members that carry items.
const DOCUMENT_CLIENT_SOURCE: &str = r##"
'use strict';

var conversions = require('@aws-sdk/util-dynamodb');

// The operation each document command addresses: the command's own name without the
// `Command` suffix, mapped to the low-level operation of the same action. The four
// single-item actions are named `…Item` on the wire and the batch and transaction ones
// carry their own suffix, which is the whole of the difference between the two clients'
// command sets.
var OPERATIONS = {
    Get: 'GetItem',
    Put: 'PutItem',
    Delete: 'DeleteItem',
    Update: 'UpdateItem',
    Query: 'Query',
    Scan: 'Scan',
    BatchGet: 'BatchGetItem',
    BatchWrite: 'BatchWriteItem',
    TransactGet: 'TransactGetItems',
    TransactWrite: 'TransactWriteItems',
    ExecuteStatement: 'ExecuteStatement',
    ExecuteTransaction: 'ExecuteTransaction'
};

// The members whose value is an ITEM — a map of attribute name to value — wherever they
// appear in a command's input, and the ones whose value is a LIST of items. Every document
// command's input carries its items under one of these names, so the conversion is the one
// walk below rather than a rule per command.
var ITEM_MEMBERS = ['Item', 'Key', 'ExpressionAttributeValues', 'ExclusiveStartKey'];
var ITEM_LIST_MEMBERS = ['Keys', 'Parameters'];

// The same, on the way back out.
var OUTPUT_ITEM_MEMBERS = ['Item', 'Attributes', 'LastEvaluatedKey'];
var OUTPUT_ITEM_LIST_MEMBERS = ['Items'];

function marshalledTree(value, options) {
    if (value === null || typeof value !== 'object' || Array.isArray(value)) {
        return value;
    }
    var out = Array.isArray(value) ? [] : {};
    var keys = Object.keys(value);
    for (var index = 0; index < keys.length; index += 1) {
        var name = keys[index];
        var held = value[name];
        if (ITEM_MEMBERS.indexOf(name) >= 0) {
            out[name] = conversions.marshall(held, options);
        } else if (ITEM_LIST_MEMBERS.indexOf(name) >= 0 && Array.isArray(held)) {
            out[name] = held.map(function (member) {
                return conversions.marshall(member, options);
            });
        } else if (Array.isArray(held)) {
            out[name] = held.map(function (member) {
                return marshalledTree(member, options);
            });
        } else {
            out[name] = marshalledTree(held, options);
        }
    }
    return out;
}

function unmarshalledTree(value, options) {
    if (value === null || typeof value !== 'object') {
        return value;
    }
    if (Array.isArray(value)) {
        return value.map(function (member) { return unmarshalledTree(member, options); });
    }
    var out = {};
    var keys = Object.keys(value);
    for (var index = 0; index < keys.length; index += 1) {
        var name = keys[index];
        var held = value[name];
        if (OUTPUT_ITEM_MEMBERS.indexOf(name) >= 0) {
            out[name] = conversions.unmarshall(held, options);
        } else if (OUTPUT_ITEM_LIST_MEMBERS.indexOf(name) >= 0 && Array.isArray(held)) {
            out[name] = held.map(function (member) {
                return conversions.unmarshall(member, options);
            });
        } else {
            out[name] = unmarshalledTree(held, options);
        }
    }
    return out;
}

function commandClass(name) {
    var action = name.slice(0, name.length - 'Command'.length);
    var operation = OPERATIONS[action];
    var Command = function (input) {
        if (operation === undefined) {
            throw new TypeError(
                'The document client addresses no operation named ' + action
            );
        }
        this.input = input === undefined || input === null ? {} : input;
        this.operation = operation;
        this.documentCommand = true;
    };
    Object.defineProperty(Command, 'name', { value: name });
    return Command;
}

// "DynamoDBDocumentClient.from(client)": the same client, sending the same calls, with the
// conversion applied on the way in and out.
function DynamoDBDocumentClient(client, translateConfig) {
    this.client = client;
    this.config = client.config;
    var held = translateConfig === undefined || translateConfig === null ? {} : translateConfig;
    this.marshallOptions = held.marshallOptions === undefined ? {} : held.marshallOptions;
    this.unmarshallOptions = held.unmarshallOptions === undefined ? {} : held.unmarshallOptions;
}

DynamoDBDocumentClient.from = function (client, translateConfig) {
    return new DynamoDBDocumentClient(client, translateConfig);
};

DynamoDBDocumentClient.prototype.send = function (command) {
    var self = this;
    if (command === null || command === undefined || command.documentCommand !== true) {
        // A low-level command sent through the document client is sent as it is, which is
        // what the document client does with one.
        return this.client.send(command);
    }
    var translated = {
        input: marshalledTree(command.input, this.marshallOptions),
        operation: command.operation
    };
    return this.client.send(translated).then(function (answered) {
        return unmarshalledTree(answered, self.unmarshallOptions);
    });
};

DynamoDBDocumentClient.prototype.destroy = function () {
    return this.client.destroy();
};

// What this package exports: the document client, and the command class of every action it
// addresses — the actions `OPERATIONS` above maps, which are the ones the document client
// has a command for. They are OWN members so that an `import { … } from` of this package
// finds them.
var members = Object.create(null);
members.DynamoDBDocumentClient = DynamoDBDocumentClient;
members.DynamoDBDocument = DynamoDBDocumentClient;

(function () {
    var actions = Object.keys(OPERATIONS);
    for (var at = 0; at < actions.length; at += 1) {
        members[actions[at] + 'Command'] = commandClass(actions[at] + 'Command');
    }
})();

module.exports = new Proxy(members, {
    get: function (target, name) {
        if (typeof name !== 'string' || Object.prototype.hasOwnProperty.call(target, name)) {
            return target[name];
        }
        if (name.length > 'Command'.length
            && name.slice(name.length - 'Command'.length) === 'Command') {
            target[name] = commandClass(name);
            return target[name];
        }
        return undefined;
    }
});
"##;

#[cfg(test)]
mod tests {    use super::*;

    /// A client package of the SDK's own namespace is supplied for any service token, and
    /// nothing else is — one row per form a bare specifier can take.
    #[test]
    fn the_runtime_supplies_a_client_package_for_any_service_token() {
        let supplied: &[&str] = &[
            "runtime:node_modules/@aws-sdk/client-s3",
            "runtime:node_modules/@aws-sdk/client-dynamodb",
            "runtime:node_modules/@aws-sdk/client-lambda",
            "runtime:node_modules/@aws-sdk/client-sqs",
        ];
        for id in supplied {
            let source = runtime_module_source(id);
            assert!(
                source.is_some_and(|text| text.contains("Client.prototype.send")),
                "case {id:?}: expected the client package's source, got none"
            );
        }
        let absent: &[&str] = &[
            "runtime:node_modules/@aws-sdk/client-",
            "runtime:node_modules/@aws-sdk/lib-s3",
            "runtime:node_modules/express",
            "runtime:node_modules/@aws-sdk/client-s3/../../etc/passwd",
            "node_modules/@aws-sdk/client-s3",
            "layer:node_modules/@aws-sdk/client-s3",
        ];
        for id in absent {
            assert!(
                runtime_module_source(id).is_none(),
                "case {id:?}: this runtime supplies no such module, so none must resolve"
            );
        }
        // A token longer than any service's SDK id names no package of this runtime, so the
        // length a specifier may carry is this runtime's and not the guest's.
        let longest = format!(
            "runtime:node_modules/@aws-sdk/client-{}",
            "s".repeat(MAX_SERVICE_TOKEN)
        );
        let past = format!(
            "runtime:node_modules/@aws-sdk/client-{}",
            "s".repeat(MAX_SERVICE_TOKEN + 1)
        );
        assert!(
            runtime_module_source(&longest).is_some(),
            "case {longest:?}: a token of the longest length admitted still names a package"
        );
        assert!(
            runtime_module_source(&past).is_none(),
            "case {past:?}: a token past that length names no package of this runtime"
        );
        // The two packages that are not a service client: the document client and the
        // value conversions it is built on.
        for id in [
            "runtime:node_modules/@aws-sdk/lib-dynamodb",
            "runtime:node_modules/@aws-sdk/util-dynamodb",
        ] {
            assert!(
                runtime_module_source(id).is_some(),
                "case {id:?}: expected the package's source, got none"
            );
        }
    }

    /// The token the package name carries is the one baked into the module, so the call a
    /// client makes names the service its own package does, and the client is exported
    /// under the name that package exports it by.
    #[test]
    fn the_client_names_the_service_its_package_does() {
        let cases: &[(&str, &str)] = &[
            ("s3", "S3Client"),
            ("dynamodb", "DynamoDBClient"),
            ("sqs", "SQSClient"),
            ("lambda", "LambdaClient"),
            ("eventbridge", "EventBridgeClient"),
            ("secrets-manager", "SecretsManagerClient"),
            ("rds-data", "RDSDataClient"),
            ("scheduler", "SchedulerClient"),
        ];
        for (token, exported) in cases {
            let id = format!("runtime:node_modules/@aws-sdk/client-{token}");
            let source = runtime_module_source(&id).expect("the client package's source");
            assert!(
                source.contains(&format!("var SERVICE = '{token}';")),
                "case {token:?}: expected the package's own token in the module, got a module \
                 naming another service"
            );
            assert!(
                source.contains(&format!("members.{exported} = Client;")),
                "case {token:?}: expected the client exported as {exported}, got a module \
                 exporting another name"
            );
            let exception = format!("{}ServiceException", exported.trim_end_matches("Client"));
            assert!(
                source.contains(&format!("members.{exception} = ServiceException;")),
                "case {token:?}: expected the base exception exported as {exception}, got a \
                 module exporting another name"
            );
        }
    }

    /// A package's own MANIFEST is a subpath of it, which is how a handler reads the package
    /// it is running against (`import pj from '@aws-sdk/client-sqs/package.json'`), and the
    /// manifest names that package — one row per package this runtime supplies, plus the
    /// manifest of a package it supplies none of.
    #[test]
    fn a_supplied_package_carries_its_own_manifest() {
        for package in [
            "@aws-sdk/client-sqs",
            "@aws-sdk/client-s3",
            "@aws-sdk/lib-dynamodb",
            "@aws-sdk/util-dynamodb",
        ] {
            let id = format!("runtime:node_modules/{package}/package.json");
            let source = runtime_module_source(&id)
                .unwrap_or_else(|| panic!("case {package:?}: expected the manifest, got none"));
            let read: ::serde_json::Value = ::serde_json::from_str(&source)
                .unwrap_or_else(|_| panic!("case {package:?}: the manifest is not JSON"));
            assert_eq!(
                read.get("name").and_then(::serde_json::Value::as_str),
                Some(package),
                "case {package:?}: expected the manifest to name the package, got {source}"
            );
            assert!(
                read.get("version")
                    .and_then(::serde_json::Value::as_str)
                    .is_some_and(|version| version.starts_with("3.")),
                "case {package:?}: expected the manifest to state the SDK generation it \
                 implements, got {source}"
            );
        }
        for id in [
            "runtime:node_modules/express/package.json",
            "runtime:node_modules/@aws-sdk/lib-s3/package.json",
            "runtime:node_modules/package.json",
        ] {
            assert!(
                runtime_module_source(id).is_none(),
                "case {id:?}: this runtime supplies no such package, so it holds no manifest \
                 of one"
            );
        }
    }

    /// The names a client package exports are the service's own model's: the command class of
    /// every operation it binds and the class of every error shape it declares, each an OWN
    /// member of the package, which is what an ES module import of it binds by name.
    #[test]
    fn a_client_package_exports_its_services_commands_and_errors() {
        // Each row: the package's token, a command the service's model binds, and an error
        // class it declares.
        let cases: &[(&str, &str, &str)] = &[
            ("sqs", "GetQueueUrlCommand", "QueueDoesNotExist"),
            ("sqs", "ReceiveMessageCommand", "OverLimit"),
            ("dynamodb", "GetItemCommand", "ResourceNotFoundException"),
            ("secretsmanager", "GetSecretValueCommand", "ResourceNotFoundException"),
        ];
        for (token, command, error) in cases {
            let id = format!("runtime:node_modules/@aws-sdk/client-{token}");
            let source = runtime_module_source(&id).expect("the client package's source");
            let operation = command.trim_end_matches("Command");
            assert!(
                source.contains(&format!("\"{operation}\"")),
                "case {token:?}: expected {command} among the commands the model's operations \
                 give, got a module without it"
            );
            assert!(
                source.contains(&format!("\"{error}\"")),
                "case {token:?}: expected {error} among the classes the model's error shapes \
                 give, got a module without it"
            );
        }
    }
}
