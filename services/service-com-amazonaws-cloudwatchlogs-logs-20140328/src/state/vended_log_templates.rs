// SPDX-License-Identifier: CC0-1.0

//! The service-managed catalogue of vended-log delivery configuration templates —
//! "the valid and default values that are used when creating delivery sources, delivery
//! destinations, and deliveries", which `DescribeConfigurationTemplates` reports.
//!
//! The catalogue is NOT an account's data: no operation creates, updates or deletes a
//! template, so it is published here and rendered on every call, which is what makes two
//! calls report the same templates.
//!
//! ## What one template says
//!
//! A template covers delivering ONE log type of ONE kind of resource to ONE kind of
//! destination, so the catalogue holds one template per (log type, resource type, delivery
//! destination type) triple a sender supports. Two different things decide what it carries:
//!
//! * The SENDER decides the record: `allowedFields` is the schema of the records that log
//!   type carries, in the order a caller restates them in `recordFields`, with the fields a
//!   delivery may not drop marked mandatory. `recordFields` is "the default record fields that
//!   will be delivered when a list of record fields is not provided" — a SUBSET of the allowed
//!   fields, because a field a caller MAY name is not always one a delivery carries when it
//!   names none ([`FIELDS_NOT_DELIVERED_BY_DEFAULT`]). The fields a particular service puts in
//!   its records are published by that service, so they are catalogue data here — one family
//!   per log type — rather than anything this service can derive. They are never the
//!   CloudWatch-Logs-generated `@`-prefixed Insights fields: those are the fields of an event
//!   INGESTED into a log group, not of a record a vending service sends.
//! * The DESTINATION decides the packaging: the output formats it can be written in, whether
//!   a field delimiter applies at all, and whether an S3 object layout (a suffix path and its
//!   variables) applies. That part IS derived, from the destination type alone.
//!
//! ## Where the rows come from
//!
//! The service and log-type pairings come from the `logType` member of
//! `PutDeliverySourceRequest`, whose own catalogue names the log types each service sends
//! (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`), plus the
//! senders AWS publishes templates for that the member's catalogue does not name. Each
//! service is named by its ARN namespace — the same spelling a stored delivery source reports
//! ([`super::delivery_source_service`]) — so a caller that holds a delivery source can find
//! the templates that apply to it, and each resource type is the resource kind within that
//! service.
//!
//! ## The order they are published in
//!
//! The catalogue is reported in the order it is stated here — one row per kind of resource,
//! each row's log types in turn, and for each log type one template per delivery destination
//! type in the order the model's `DeliveryDestinationType` enum DECLARES its values
//! ([`DELIVERY_DESTINATION_TYPE_ORDER`]), the object destination first. The order is published
//! data rather than anything derived from a template's contents, and it is a total order over
//! a catalogue nothing mutates, which is what makes `DescribeConfigurationTemplates`'s
//! pagination cursor resume at a position that does not move.

use super::{
    COLUMNAR_OUTPUT_FORMAT, CWL_DELIVERY_DESTINATION_TYPE, DELIMITED_OUTPUT_FORMATS,
    S3_DELIVERY_DESTINATION_TYPE, STREAM_DELIVERY_DESTINATION_TYPE, STRUCTURED_OUTPUT_FORMAT,
    TRACE_DELIVERY_DESTINATION_TYPE, TRACE_LOG_TYPE,
};

/// The IAM action a caller needs on a resource "to be able to successfully create a delivery
/// source on the desired resource type when calling PutDeliverySource", without the service
/// prefix an IAM action carries. The prefix is the resource's own service — the same string a
/// delivery source reports as its `service` ([`super::delivery_source_service`]) — so a
/// template names the action as `<service>:<this>`.
pub const VENDED_LOGS_DELIVERY_ACTION: &str = "AllowVendedLogDeliveryForResource";

/// The field delimiters a delivery that delivers in a delimited format may be configured
/// with.
///
/// `FieldDelimiter` is modelled only as a string of up to 5 characters and AWS publishes the
/// permitted set through this operation alone: a tab, a space and a comma.
pub const CONFIGURATION_TEMPLATE_FIELD_DELIMITERS: [&str; 3] = ["\t", " ", ","];

/// The field delimiter a delivery that delivers in a delimited format is given when the
/// request names none: NO delimiter.
///
/// `FieldDelimiter` is modelled `@length 0..5`, so the empty string is a delimiter value in its
/// own right, and it is the one a template publishes as its default: a caller that names none
/// gets records written with no delimiter between their fields rather than with one this
/// service chose for it.
pub const CONFIGURATION_TEMPLATE_DEFAULT_FIELD_DELIMITER: &str = "";

/// The variable fields an S3 delivery's suffix path may use, as ONE PUBLISHED LIST PER SENDER.
///
/// `S3DeliveryConfiguration$suffixPath` documents that "the valid variables to use in the
/// suffix path vary by log type" and that a caller finds them by calling this operation and
/// reading `allowedSuffixPathFields`, so each list is catalogue data. Every list opens with the
/// account the delivery is made in, names the Region it is made in and the variables naming the
/// sending RESOURCE — which of the two a sender states first is itself part of what it
/// publishes, so the whole list is stated per row — and closes with the time partition of the
/// object written, coarsest first.
///
/// This is the list of a sender that publishes no variable naming the sending resource at all.
const ACCOUNT_SUFFIX_PATH_FIELDS: &[&str] = &["accountid", "region", "yyyy", "MM", "dd", "HH"];

/// The published list of a sender that names the sending resource with the generic resource
/// variable, because it publishes no identifier of its own kind.
const RESOURCE_ID_SUFFIX_PATH_FIELDS: &[&str] =
    &["accountid", "region", "resource_id", "yyyy", "MM", "dd", "HH"];

/// Whether a delivery to an S3 bucket writes Hive-compatible object prefixes by default.
/// `enableHiveCompatiblePath` "causes the S3 objects that contain delivered logs to use a
/// prefix structure that allows for integration with Apache Hive", which no default suffix
/// path below does, so the default is off.
pub const CONFIGURATION_TEMPLATE_DEFAULT_HIVE_COMPATIBLE_PATH: bool = false;

/// The `OutputFormat`s a delivery to a destination that stores ONE STRUCTURED RECORD for each
/// delivered item can be written in: a log group of this service, or the X-Ray trace
/// destination that receives spans, takes the structured format alone.
const RECORD_DESTINATION_OUTPUT_FORMATS: [&str; 1] = [STRUCTURED_OUTPUT_FORMAT];

/// The `OutputFormat`s a delivery to a destination that receives a STREAM of records can be
/// written in: each per-record encoding, delimited or structured.
const STREAM_DESTINATION_OUTPUT_FORMATS: [&str; 3] = ["plain", STRUCTURED_OUTPUT_FORMAT, "w3c"];

/// The `OutputFormat`s a delivery that WRITES OBJECTS can be written in: every per-record
/// encoding a stream takes, plus the columnar file layout, which is a property of a file and
/// so applies only where objects are written.
const OBJECT_DESTINATION_OUTPUT_FORMATS: [&str; 4] = [
    "plain",
    STRUCTURED_OUTPUT_FORMAT,
    "w3c",
    COLUMNAR_OUTPUT_FORMAT,
];

/// One record field of a log type's schema: the name a caller names it by in `recordFields`,
/// and whether a delivery MUST carry it (`RecordField$mandatory` — "the record field must be
/// present in the recordFields parameter").
pub type TemplateField = (&'static str, bool);

/// The fields of a record schema that a delivery created WITHOUT a record field list does not
/// carry: the record's own ingestion timestamp, and the bare id of the sending resource that
/// the resource ARN beside it already names.
///
/// `allowedFields` is "the allowed fields that a caller CAN use in the recordFields parameter"
/// while `recordFields` is "the default record fields that WILL BE DELIVERED when a list of
/// record fields is not provided": the default list is a subset of the allowed ones, and these
/// two are allowed without being delivered by default. A field a delivery may not drop is
/// delivered whatever this says, because a mandatory field "must be present in the
/// recordFields parameter".
const FIELDS_NOT_DELIVERED_BY_DEFAULT: [&str; 2] = ["timestamp", "resource_id"];

/// The record schema of a log type whose records are the generic vended-log envelope of a
/// newer sender: the record's own timestamp, the ARN and the id of the resource that sent it,
/// and the instant of the event — the resource ARN and the event instant mandatory, because a
/// delivered record that attributes itself to nothing is not the record — followed by the
/// dimensions that sender adds and, last, the field carrying the logged content.
const AIDEVOPS_APPLICATION_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("event_timestamp", true),
    ("log_level", false),
    ("operation", false),
    ("request_id", false),
    ("message", true),
];

/// The record schema of an API Gateway stage's execution logs: the vended-log envelope, the
/// stage and request dimensions of the call being logged, and the logged line itself. The
/// envelope is optional here — only the logged line may not be dropped.
const APIGATEWAY_EXECUTION_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", false),
    ("resource_id", false),
    ("event_timestamp", false),
    ("api_id", false),
    ("stage", false),
    ("resource_path", false),
    ("http_method", false),
    ("payload", true),
];

/// The record schema of an AutoLoop subscription's logs: the vended-log envelope, the instant
/// the line itself was logged, the identifiers of the transformer, object, event and trace the
/// line belongs to, and the line.
const AUTOLOOP_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("event_timestamp", true),
    ("log_timestamp", false),
    ("transformer_arn", false),
    ("object_identifier", false),
    ("event_identifier", false),
    ("trace_identifier", false),
    ("log_message", true),
];

/// The record schema of a Bedrock agent's or knowledge base's logs: a single event envelope
/// whose every field is mandatory, because the record is one event document and a delivery
/// that drops any part of it is not that document.
const BEDROCK_EVENT_FIELDS: &[TemplateField] = &[
    ("timestamp", true),
    ("resourceId", true),
    ("traceId", true),
    ("spanId", true),
    ("sessionId", true),
    ("requestId", true),
    ("operation", true),
    ("attributes", true),
    ("body", true),
    ("eventType", true),
    ("eventVersion", true),
    ("eventName", true),
    ("level", true),
];

/// The record schema of an AgentCore resource's logs and traces: the vended-log envelope the
/// resource attributes its records with, then the OpenTelemetry log record itself.
///
/// The envelope is the one every newer sender writes — the record's own timestamp, the ARN and
/// the id of the resource that sent it and the instant of the event — widened with the
/// dimensions an agent call is identified by: the account it ran in, the request, session,
/// span and trace it belongs to, the service and operation that ran, the payloads it exchanged,
/// and the resource and attribute documents the call carried. The record closes with the
/// OpenTelemetry log record the resource emits — the event's instant and its body mandatory,
/// its severity and the span it belongs to optional — because an AgentCore record carries both
/// the envelope and the telemetry item it envelopes.
const AGENTCORE_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("event_timestamp", true),
    ("account_id", false),
    ("request_id", false),
    ("session_id", false),
    ("span_id", false),
    ("trace_id", false),
    ("service_name", false),
    ("operation", false),
    ("request_payload", false),
    ("response_payload", false),
    ("resource", false),
    ("attributes", false),
    ("timeUnixNano", true),
    ("severityNumber", false),
    ("severityText", false),
    ("body", true),
    ("traceId", false),
    ("spanId", false),
];

/// The record schema of an EKS cluster's logs: the vended-log envelope plus the controller and
/// reconciliation dimensions a cluster controller logs under — the logged line, the error it
/// reports, the controller that reconciled and the work queue it drained.
const EKS_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("event_timestamp", true),
    ("level", false),
    ("stream", false),
    ("message", false),
    ("err", false),
    ("error", false),
    ("controller", false),
    ("controllerGroup", false),
    ("controllerKind", false),
    ("reconcileID", false),
    ("worker-count", false),
    ("attempt", false),
    ("delay", false),
    ("totalItems", false),
];

/// The record schema of an EventBridge subscriber's or event bus's logs: the vended-log
/// envelope, whose instant is the millisecond the message was handled, plus the subscriber, the
/// request, the event and the invocation the line belongs to, and the line — its detail and the
/// error it reports.
const EVENTS_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("message_timestamp_ms", true),
    ("subscriber_name", false),
    ("request_id", false),
    ("event_id", false),
    ("invocation_id", false),
    ("message_type", false),
    ("log_level", false),
    ("details", false),
    ("error", false),
];

/// The record schema of an Application Load Balancer's access logs: the access-log record of
/// a load balancer, every field mandatory because the record is one fixed-shape request line.
const ALB_ACCESS_FIELDS: &[TemplateField] = &[
    ("type", true),
    ("time", true),
    ("elb", true),
    ("client_ip", true),
    ("client_port", true),
    ("target_ip", true),
    ("target_port", true),
    ("request_processing_time", true),
    ("target_processing_time", true),
    ("response_processing_time", true),
    ("elb_status_code", true),
    ("target_status_code", true),
    ("received_bytes", true),
    ("sent_bytes", true),
    ("request_verb", true),
    ("request_url", true),
    ("request_proto", true),
    ("user_agent", true),
    ("ssl_cipher", true),
    ("ssl_protocol", true),
    ("target_group_arn", true),
    ("trace_id", true),
    ("domain_name", true),
    ("chosen_cert_arn", true),
    ("matched_rule_priority", true),
    ("request_creation_time", true),
    ("actions_executed", true),
    ("redirect_url", true),
    ("error_reason", true),
    ("target_port_list", true),
    ("target_status_code_list", true),
    ("classification", true),
    ("classification_reason", true),
    ("conn_trace_id", true),
];

/// The record schema of an Application Load Balancer's connection logs: the TLS handshake of
/// one client connection, every field mandatory for the same reason as the access log.
const ALB_CONNECTION_FIELDS: &[TemplateField] = &[
    ("time", true),
    ("client_ip", true),
    ("client_port", true),
    ("listener_port", true),
    ("tls_protocol", true),
    ("tls_cipher", true),
    ("tls_handshake_latency", true),
    ("leaf_client_cert_subject", true),
    ("leaf_client_cert_validity", true),
    ("leaf_client_cert_serial_number", true),
    ("tls_verify_status", true),
    ("conn_trace_id", true),
];

/// The record schema of an Application Load Balancer's health-check logs: one health check of
/// one target, every field mandatory for the same reason as the access log.
const ALB_HEALTH_CHECK_FIELDS: &[TemplateField] = &[
    ("time", true),
    ("elb", true),
    ("target_group_arn", true),
    ("target_ip", true),
    ("target_port", true),
    ("health_check_port", true),
    ("health_check_protocol", true),
    ("health_check_path", true),
    ("health_check_status", true),
    ("health_check_reason", true),
    ("health_check_latency", true),
];

/// The record schema of a Network Load Balancer's access logs: one TLS connection through the
/// load balancer, every field mandatory for the same reason as the access log.
const NLB_ACCESS_FIELDS: &[TemplateField] = &[
    ("type", true),
    ("version", true),
    ("time", true),
    ("elb", true),
    ("listener", true),
    ("client_ip", true),
    ("client_port", true),
    ("destination_ip", true),
    ("destination_port", true),
    ("connection_time", true),
    ("tls_handshake_time", true),
    ("received_bytes", true),
    ("sent_bytes", true),
    ("incoming_tls_alpn", true),
    ("chosen_cert_arn", true),
    ("chosen_cert_serial", true),
    ("tls_cipher", true),
    ("tls_protocol_version", true),
    ("tls_named_group", true),
    ("domain_name", true),
    ("alpn_fe_protocol", true),
    ("alpn_be_protocol", true),
    ("alpn_client_preference_list", true),
    ("tls_connection_creation_time", true),
];

/// The record schema of a CloudFront distribution's access logs: the W3C-named fields of a
/// standard access log, the date and time of the request mandatory.
const CLOUDFRONT_ACCESS_FIELDS: &[TemplateField] = &[
    ("date", true),
    ("time", true),
    ("x-edge-location", false),
    ("sc-bytes", false),
    ("c-ip", false),
    ("cs-method", false),
    ("cs-protocol", false),
    ("cs-host", false),
    ("cs-uri-stem", false),
    ("sc-status", false),
    ("cs-referer", false),
    ("cs-user-agent", false),
    ("cs-uri-query", false),
    ("cs-cookie", false),
    ("x-edge-result-type", false),
    ("x-edge-request-id", false),
    ("x-host-header", false),
    ("cs-bytes", false),
    ("time-taken", false),
    ("x-forwarded-for", false),
    ("ssl-protocol", false),
    ("ssl-cipher", false),
    ("x-edge-response-result-type", false),
    ("cs-protocol-version", false),
    ("fle-status", false),
    ("fle-encrypted-fields", false),
    ("c-port", false),
    ("time-to-first-byte", false),
    ("x-edge-detailed-result-type", false),
    ("sc-content-type", false),
    ("sc-content-len", false),
    ("sc-range-start", false),
    ("sc-range-end", false),
    ("timestamp", false),
    ("origin-fbl", false),
    ("origin-lbl", false),
    ("asn", false),
];

/// The record schema of an Amplify app's access logs: one request served by the app, its
/// instant mandatory.
const AMPLIFY_ACCESS_FIELDS: &[TemplateField] = &[
    ("time", true),
    ("ip", false),
    ("method", false),
    ("protocol", false),
    ("host", false),
    ("path", false),
    ("status", false),
    ("size", false),
    ("referrer", false),
    ("user_agent", false),
    ("duration", false),
];

/// The record schema of a web ACL's logs: one inspected request and the action taken on it —
/// the instant, the action and the request mandatory, the rule detail optional.
const WAF_FIELDS: &[TemplateField] = &[
    ("timestamp", true),
    ("formatVersion", false),
    ("webaclId", false),
    ("terminatingRuleId", false),
    ("terminatingRuleType", false),
    ("action", true),
    ("httpSourceName", false),
    ("httpSourceId", false),
    ("ruleGroupList", false),
    ("rateBasedRuleList", false),
    ("nonTerminatingMatchingRules", false),
    ("requestHeadersInserted", false),
    ("responseCodeSent", false),
    ("httpRequest", true),
    ("labels", false),
];

/// The record schema of a CloudWatch Logs Insights query-execution log: the query that ran,
/// its instant and its identity mandatory, its statistics optional.
const INSIGHTS_QUERY_FIELDS: &[TemplateField] = &[
    ("eventTimestamp", true),
    ("accountId", false),
    ("queryId", true),
    ("queryLanguage", false),
    ("queryString", true),
    ("status", false),
    ("logGroups", false),
    ("startTime", false),
    ("endTime", false),
    ("recordsScanned", false),
    ("recordsMatched", false),
    ("bytesScanned", false),
    ("executionDurationMillis", false),
];

/// The record schema of an S3 bucket's server access logs: one request against the bucket,
/// the bucket owner, the bucket and the instant mandatory.
const S3_SERVER_ACCESS_FIELDS: &[TemplateField] = &[
    ("bucketowner", true),
    ("bucket", true),
    ("requestdatetime", true),
    ("remoteip", false),
    ("requester", false),
    ("requestid", false),
    ("operation", false),
    ("key", false),
    ("request_uri", false),
    ("httpstatus", false),
    ("errorcode", false),
    ("bytessent", false),
    ("objectsize", false),
    ("totaltime", false),
    ("turnaroundtime", false),
    ("referrer", false),
    ("useragent", false),
    ("versionid", false),
    ("hostid", false),
    ("sigv", false),
    ("ciphersuite", false),
    ("authtype", false),
    ("endpoint", false),
    ("tlsversion", false),
    ("accesspointarn", false),
    ("aclrequired", false),
];

/// The record schema of a Security Hub finding log: ONE mandatory field carrying the finding
/// event itself.
///
/// A finding is delivered as the whole finding document rather than as a row of columns, so the
/// log type publishes a single field and marks it mandatory: there is no part of the event a
/// delivery may drop and still be delivering findings.
const SECURITY_FINDING_FIELDS: &[TemplateField] = &[("event", true)];

/// The record schema of a SES Mail Manager application log: the vended-log envelope plus the
/// traffic policy and message the line belongs to, and the line.
const SES_APPLICATION_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("event_timestamp", true),
    ("traffic_policy_id", false),
    ("rule_set_id", false),
    ("message_id", false),
    ("action", false),
    ("payload", true),
];

/// The record schema of a SES Mail Manager traffic-policy debug log: the vended-log envelope
/// plus the policy statement evaluated and its result, and the logged line.
const SES_TRAFFIC_POLICY_DEBUG_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("resource_arn", true),
    ("resource_id", false),
    ("event_timestamp", true),
    ("traffic_policy_id", false),
    ("policy_statement", false),
    ("evaluation_result", false),
    ("payload", true),
];

/// The record schema of a MediaTailor playback configuration's logs: one ad-insertion event —
/// its instant and the account it happened in mandatory, the VAST, ad and beacon detail
/// optional, because which of them a record carries depends on the event.
const MEDIATAILOR_FIELDS: &[TemplateField] = &[
    ("eventTimestamp", true),
    ("awsAccountId", true),
    ("eventType", false),
    ("sessionId", false),
    ("originId", false),
    ("playbackConfigurationName", false),
    ("vastResponse", false),
    ("vastAds", false),
    ("adBreaks", false),
    ("beaconInfo", false),
    ("trackingEvents", false),
];

/// The record schema of a WorkMail organization's access-control logs: one access decision —
/// the organization, the instant, the request it was made for, the identity it was made for and
/// the decision mandatory, and the dimensions it was made on optional.
const WORKMAIL_ACCESS_CONTROL_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("organization_arn", true),
    ("event_timestamp", true),
    ("request_id", true),
    ("user_id", true),
    ("impersonator_id", false),
    ("protocol", true),
    ("source_ip", true),
    ("user_agent", false),
    ("scope", true),
    ("rule_id", false),
    ("access_granted", true),
];

/// The record schema of a WorkMail organization's authentication logs: one authentication
/// attempt and its result.
const WORKMAIL_AUTHENTICATION_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("organization_arn", true),
    ("event_timestamp", true),
    ("user_id", false),
    ("protocol", false),
    ("source_ip", false),
    ("authentication_result", true),
];

/// The record schema of a WorkMail organization's availability-provider logs: one
/// availability lookup against a configured provider and its outcome.
const WORKMAIL_AVAILABILITY_PROVIDER_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("organization_arn", true),
    ("event_timestamp", true),
    ("provider_name", false),
    ("domain", false),
    ("request_type", false),
    ("response_status", true),
];

/// The record schema of a WorkMail organization's mailbox-access logs: one access to one
/// mailbox and the operation performed.
const WORKMAIL_MAILBOX_ACCESS_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("organization_arn", true),
    ("event_timestamp", true),
    ("user_id", false),
    ("mailbox_owner_id", false),
    ("protocol", false),
    ("source_ip", false),
    ("operation", true),
];

/// The record schema of a WorkMail organization's personal-access-token logs: one event in
/// the life of a token.
const WORKMAIL_PERSONAL_ACCESS_TOKEN_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("organization_arn", true),
    ("event_timestamp", true),
    ("user_id", false),
    ("token_id", false),
    ("scope", false),
    ("event_name", true),
];

/// The record schema of an Amazon Q Business application's logs: the application the record
/// belongs to, the instant of the event, the kind of log it was written as and the account it
/// happened in.
///
/// The application and the instant are mandatory, because a delivered record that attributes
/// itself to neither an application nor a moment is not the record; the record's own ingestion
/// timestamp is allowed without being delivered by default, as it is for every sender that
/// publishes one ([`FIELDS_NOT_DELIVERED_BY_DEFAULT`]). One schema covers both of the log types
/// this sender sends, the way the other senders with several log types of one record publish
/// one schema for them.
const QBUSINESS_FIELDS: &[TemplateField] = &[
    ("timestamp", false),
    ("application_id", true),
    ("event_timestamp", true),
    ("log_type", false),
    ("account_id", false),
];

/// One log type a kind of resource sends, and what a delivery of it may carry.
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationTemplateLogType {
    /// `ConfigurationTemplate$logType` — "which log type this configuration template applies
    /// to", spelled as the `logType` member of `PutDeliverySourceRequest` spells it.
    pub log_type: &'static str,
    /// `ConfigurationTemplate$allowedFields` — "the allowed fields that a caller can use in
    /// the recordFields parameter", in the order `recordFields` restates them
    /// (`CreateDelivery$recordFields` is "the list of record fields to be delivered to the
    /// destination, IN ORDER"), each paired with whether a delivery MUST carry it.
    pub fields: &'static [TemplateField],
    /// `ConfigurationTemplate$s3TablesIntegration` — the datasource name and type of the S3
    /// Tables integration for this log type, for the log types AWS publishes one for, and
    /// `None` for the rest.
    pub s3_tables_integration: Option<(&'static str, &'static str)>,
}

/// One row of the catalogue behind `DescribeConfigurationTemplates`: a kind of resource that
/// can send vended logs, and the log types it sends.
#[derive(Debug, Clone, Copy)]
pub struct ConfigurationTemplateSource {
    /// `ConfigurationTemplate$service` — "A string specifying which service this
    /// configuration template applies to", the sending resource's ARN namespace.
    pub service: &'static str,
    /// `ConfigurationTemplate$resourceType` — the kind of resource of that service which
    /// sends the logs.
    pub resource_type: &'static str,
    /// `ConfigurationTemplate$allowedSuffixPathFields` — "the list of variable fields that can
    /// be used in the suffix path of a delivery that delivers to an S3 bucket" of these logs,
    /// as this sender publishes it ([`ACCOUNT_SUFFIX_PATH_FIELDS`]).
    pub suffix_path_fields: &'static [&'static str],
    /// `defaultDeliveryConfigValues.s3DeliveryConfiguration.suffixPath` — the suffix path an
    /// S3 delivery of these logs is given when the request names none. Every variable in it
    /// is one the same template allows, because a default a caller cannot restate is no
    /// default.
    pub s3_suffix_path: &'static str,
    /// The log types this kind of resource sends, one template each per delivery destination
    /// type they can be sent to.
    pub log_types: &'static [ConfigurationTemplateLogType],
}

impl ConfigurationTemplateSource {
    /// The delivery destination types a delivery of `log_type` can be sent to, in the order
    /// the model's `DeliveryDestinationType` enum declares them
    /// ([`DELIVERY_DESTINATION_TYPE_ORDER`]): the X-Ray trace delivery destination
    /// ([`TRACE_DELIVERY_DESTINATION_TYPE`], the kind `PutDeliveryDestination` documents as
    /// needing no destination configuration) receives TRACES and nothing else, while every log
    /// type — traces included — can be delivered to each of the destination types that receive
    /// records.
    pub fn delivery_destination_types(log_type: &str) -> Vec<String> {
        DELIVERY_DESTINATION_TYPE_ORDER
            .iter()
            .filter(|destination_type| {
                **destination_type != TRACE_DELIVERY_DESTINATION_TYPE || log_type == TRACE_LOG_TYPE
            })
            .map(|destination_type| (*destination_type).to_string())
            .collect()
    }

    /// Every template this row contributes: one per (log type, delivery destination type) pair
    /// it supports, because a template carries "the valid and default values" for delivering
    /// ONE log type to ONE kind of destination.
    pub fn templates(&self) -> Vec<crate::ops::types::ConfigurationTemplate> {
        self.log_types
            .iter()
            .flat_map(|log_type| {
                Self::delivery_destination_types(log_type.log_type)
                    .into_iter()
                    .map(|destination_type| self.template(log_type, &destination_type))
                    .collect::<Vec<_>>()
            })
            .collect()
    }

    /// This row's template for one of its log types delivered to one kind of destination.
    ///
    /// The record — `allowedFields`, and the `recordFields` default naming those of them a
    /// delivery carries when it names none — comes from the LOG TYPE, whose sending service
    /// decides what a record carries. The packaging is DERIVED from the destination type,
    /// because what a delivery may be configured with is a property of what receives it: the
    /// formats it can be written in, whether a field delimiter applies at all, and whether an
    /// S3 object layout applies.
    fn template(
        &self,
        log_type: &ConfigurationTemplateLogType,
        destination_type: &str,
    ) -> crate::ops::types::ConfigurationTemplate {
        let output_formats = configuration_template_output_formats(destination_type);
        let is_delimited = output_formats
            .iter()
            .any(|format| DELIMITED_OUTPUT_FORMATS.contains(&format.as_str()));
        let writes_objects = destination_type == S3_DELIVERY_DESTINATION_TYPE;
        let s3_delivery_configuration =
            writes_objects.then(|| crate::ops::types::S3DeliveryConfiguration {
                suffix_path: Some(self.s3_suffix_path.to_string()),
                enable_hive_compatible_path: Some(CONFIGURATION_TEMPLATE_DEFAULT_HIVE_COMPATIBLE_PATH),
            });
        let field_delimiter =
            is_delimited.then(|| CONFIGURATION_TEMPLATE_DEFAULT_FIELD_DELIMITER.to_string());
        crate::ops::types::ConfigurationTemplate {
            service: Some(self.service.to_string()),
            log_type: Some(log_type.log_type.to_string()),
            resource_type: Some(self.resource_type.to_string()),
            delivery_destination_type: Some(destination_type.to_string()),
            default_delivery_config_values: Some(
                crate::ops::types::ConfigurationTemplateDeliveryConfigValues {
                    // A delivery created without a field list carries the record the log type
                    // sends, in the order a caller would restate them in, less the fields that
                    // are allowed without being delivered by default
                    // ([`FIELDS_NOT_DELIVERED_BY_DEFAULT`]).
                    record_fields: Some(
                        log_type
                            .fields
                            .iter()
                            .filter(|(field, mandatory)| {
                                *mandatory || !FIELDS_NOT_DELIVERED_BY_DEFAULT.contains(field)
                            })
                            .map(|(field, _mandatory)| (*field).to_string())
                            .collect(),
                    ),
                    // The delimiter a delivery is given when the request names none, published
                    // for a template whose delivery is written in a delimited format
                    // ([`CONFIGURATION_TEMPLATE_DEFAULT_FIELD_DELIMITER`]).
                    field_delimiter,
                    s3_delivery_configuration,
                },
            ),
            allowed_fields: Some(
                log_type
                    .fields
                    .iter()
                    .map(|(field, mandatory)| crate::ops::types::RecordField {
                        name: Some((*field).to_string()),
                        mandatory: Some(*mandatory),
                    })
                    .collect(),
            ),
            allowed_output_formats: Some(output_formats),
            allowed_action_for_allow_vended_logs_delivery_for_resource: Some(format!(
                "{}:{VENDED_LOGS_DELIVERY_ACTION}",
                self.service
            )),
            allowed_field_delimiters: is_delimited.then(|| {
                CONFIGURATION_TEMPLATE_FIELD_DELIMITERS
                    .iter()
                    .map(|delimiter| (*delimiter).to_string())
                    .collect()
            }),
            allowed_suffix_path_fields: writes_objects.then(|| {
                self.suffix_path_fields
                    .iter()
                    .map(|field| (*field).to_string())
                    .collect()
            }),
            // The schema of "a configuration that can be set when calling PutDeliverySource":
            // no log type in this catalogue takes one, so none is reported.
            delivery_source_configuration: None,
            s3_tables_integration: log_type.s3_tables_integration.map(|(name, kind)| {
                crate::ops::types::S3TablesIntegration {
                    datasource_name: Some(name.to_string()),
                    datasource_type: Some(kind.to_string()),
                }
            }),
        }
    }
}

/// The `OutputFormat`s a delivery to `destination_type` can be written in.
///
/// DERIVED from what receives the delivery: a destination that stores one structured record
/// per delivered item takes the structured format alone
/// ([`RECORD_DESTINATION_OUTPUT_FORMATS`]), a destination that receives a stream of records
/// takes each per-record encoding ([`STREAM_DESTINATION_OUTPUT_FORMATS`]), and a delivery that
/// writes objects takes those plus the columnar file layout
/// ([`OBJECT_DESTINATION_OUTPUT_FORMATS`]).
pub fn configuration_template_output_formats(destination_type: &str) -> Vec<String> {
    let published: &[&str] = if destination_type == CWL_DELIVERY_DESTINATION_TYPE
        || destination_type == TRACE_DELIVERY_DESTINATION_TYPE
    {
        &RECORD_DESTINATION_OUTPUT_FORMATS
    } else if destination_type == S3_DELIVERY_DESTINATION_TYPE {
        &OBJECT_DESTINATION_OUTPUT_FORMATS
    } else {
        &STREAM_DESTINATION_OUTPUT_FORMATS
    };
    published
        .iter()
        .map(|format| (*format).to_string())
        .collect()
}

/// One log type of a row, with no S3 Tables integration published for it.
const fn log_type(
    log_type: &'static str,
    fields: &'static [TemplateField],
) -> ConfigurationTemplateLogType {
    ConfigurationTemplateLogType {
        log_type,
        fields,
        s3_tables_integration: None,
    }
}

/// One log type of a row, with the S3 Tables datasource AWS publishes for it.
const fn log_type_with_s3_tables(
    log_type: &'static str,
    fields: &'static [TemplateField],
    datasource_name: &'static str,
    datasource_type: &'static str,
) -> ConfigurationTemplateLogType {
    ConfigurationTemplateLogType {
        log_type,
        fields,
        s3_tables_integration: Some((datasource_name, datasource_type)),
    }
}

/// The suffix path an S3 delivery of a sender that publishes no identifier of its own kind
/// defaults to: the Region, the sending resource and the time partition.
const RESOURCE_ID_SUFFIX_PATH: &str = "{region}/{resource_id}/{yyyy}/{MM}/{dd}/{HH}/";

/// The suffix path an S3 delivery DEFAULTS to for a sender that partitions its delivered
/// objects by TIME ALONE: the Region the delivery is made in, then the hour the object covers.
///
/// A sender publishes a variable naming the sending resource among the variables a caller MAY
/// write into a suffix path of its own ([`ACCOUNT_SUFFIX_PATH_FIELDS`] and the per-row lists
/// below), which is not the same as the DEFAULT it publishes; a sender whose default partitions
/// by the sending resource, by the day, or not at all states that default on its row
/// ([`RESOURCE_ID_SUFFIX_PATH`], [`DAILY_SUFFIX_PATH`], [`NO_SUFFIX_PATH`]).
const DEFAULT_DELIVERY_SUFFIX_PATH: &str = "{region}/{yyyy}/{MM}/{dd}/{HH}/";

/// The suffix path of a sender that publishes NO prefix of its own: delivered objects are
/// written under the prefix the destination itself configures, so the default adds nothing to
/// it.
///
/// `FieldDelimiter` is not the only member whose default is the empty value — a sender that
/// partitions its objects in no way publishes an empty suffix path, and a delivery of its logs
/// reports that rather than a partition this service chose for it.
const NO_SUFFIX_PATH: &str = "";

/// The suffix path an S3 delivery of a sender that partitions its objects by DAY defaults to:
/// the Region the delivery is made in, then the day the object covers.
const DAILY_SUFFIX_PATH: &str = "{region}/{yyyy}/{MM}/{dd}/";

/// The order one log type's templates are published in, one per delivery destination type: the
/// order the model's `DeliveryDestinationType` enum DECLARES its values
/// (`smithy_query(query="shape",shape="DeliveryDestinationType")` answers
/// `[S3, CWL, FH, XRAY]`), so a slice of the catalogue opens with the template for the
/// destination that writes objects rather than with whichever spelling sorts first.
///
/// Every value is one the model names
/// ([`crate::ops::types::DELIVERY_DESTINATION_TYPE_VALUES`]), never a string typed here.
const DELIVERY_DESTINATION_TYPE_ORDER: [&str; 4] = [
    S3_DELIVERY_DESTINATION_TYPE,
    CWL_DELIVERY_DESTINATION_TYPE,
    STREAM_DELIVERY_DESTINATION_TYPE,
    TRACE_DELIVERY_DESTINATION_TYPE,
];

/// The resources that can send vended logs, the log types each of them sends and the record
/// schema of each — the catalogue `DescribeConfigurationTemplates` reports templates from.
///
/// See this module's own documentation for where the rows come from.
pub const CONFIGURATION_TEMPLATE_SOURCES: &[ConfigurationTemplateSource] = &[
    ConfigurationTemplateSource {
        service: "aidevops",
        resource_type: "agent-space",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: RESOURCE_ID_SUFFIX_PATH,
        log_types: &[log_type("APPLICATION_LOGS", AIDEVOPS_APPLICATION_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "amplify",
        resource_type: "app",
        suffix_path_fields: &["accountid", "region", "app_id", "yyyy", "MM", "dd", "HH"],
        s3_suffix_path: "{region}/{app_id}/{yyyy}/{MM}/{dd}/{HH}/",
        log_types: &[
            log_type("ACCESS_LOGS", AMPLIFY_ACCESS_FIELDS),
            log_type("WAF_LOGS", WAF_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "apigateway",
        resource_type: "stages",
        suffix_path_fields: &[
            "accountid", "region", "api_id", "stage", "yyyy", "MM", "dd", "HH",
        ],
        s3_suffix_path: "{region}/{api_id}/{stage}/",
        log_types: &[log_type("EXECUTION_LOGS", APIGATEWAY_EXECUTION_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "autoloop",
        resource_type: "subscription",
        suffix_path_fields: ACCOUNT_SUFFIX_PATH_FIELDS,
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[log_type("ERROR_LOGS", AUTOLOOP_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "bedrock",
        resource_type: "agent-alias",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", BEDROCK_EVENT_FIELDS),
            log_type("EVENT_LOGS", BEDROCK_EVENT_FIELDS),
            log_type(TRACE_LOG_TYPE, BEDROCK_EVENT_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock",
        resource_type: "knowledge-base",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: RESOURCE_ID_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", BEDROCK_EVENT_FIELDS),
            log_type(TRACE_LOG_TYPE, BEDROCK_EVENT_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock-agentcore",
        resource_type: "gateway",
        suffix_path_fields: &[
            "accountid",
            "region",
            "gateway_id",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", AGENTCORE_FIELDS),
            log_type(TRACE_LOG_TYPE, AGENTCORE_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock-agentcore",
        resource_type: "memory",
        suffix_path_fields: &[
            "accountid",
            "region",
            "memory_id",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", AGENTCORE_FIELDS),
            log_type(TRACE_LOG_TYPE, AGENTCORE_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock-agentcore",
        resource_type: "payments",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", AGENTCORE_FIELDS),
            log_type(TRACE_LOG_TYPE, AGENTCORE_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock-agentcore",
        resource_type: "runtime",
        suffix_path_fields: &[
            "accountid",
            "region",
            "runtime_id",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", AGENTCORE_FIELDS),
            log_type("USAGE_LOGS", AGENTCORE_FIELDS),
            log_type(TRACE_LOG_TYPE, AGENTCORE_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock-agentcore",
        resource_type: "tools",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", AGENTCORE_FIELDS),
            log_type("USAGE_LOGS", AGENTCORE_FIELDS),
            log_type(TRACE_LOG_TYPE, AGENTCORE_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "bedrock-agentcore",
        resource_type: "workload-identity",
        suffix_path_fields: &[
            "accountid",
            "region",
            "workload_identity_id",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", AGENTCORE_FIELDS),
            log_type(TRACE_LOG_TYPE, AGENTCORE_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "cloudfront",
        resource_type: "distribution",
        suffix_path_fields: &[
            "accountid",
            "region",
            "distribution_id",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: "{region}/{distribution_id}/{yyyy}/{MM}/{dd}/{HH}/",
        log_types: &[log_type("ACCESS_LOGS", CLOUDFRONT_ACCESS_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "eks",
        resource_type: "cluster",
        suffix_path_fields: &[
            "accountid",
            "cluster_id",
            "region",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: "{region}/{cluster_id}/{yyyy}/{MM}/{dd}/{HH}/",
        log_types: &[
            log_type("AUTO_MODE_BLOCK_STORAGE_LOGS", EKS_FIELDS),
            log_type("AUTO_MODE_COMPUTE_LOGS", EKS_FIELDS),
            log_type("AUTO_MODE_IPAM_LOGS", EKS_FIELDS),
            log_type("AUTO_MODE_LOAD_BALANCING_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_ACK_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_ARGOCD_APPLICATION_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_ARGOCD_APPLICATIONSET_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_ARGOCD_COMMITSERVER_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_ARGOCD_REPOSERVER_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_ARGOCD_SERVER_LOGS", EKS_FIELDS),
            log_type("EKS_CAPABILITY_KRO_LOGS", EKS_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "elasticloadbalancing",
        resource_type: "loadbalancer",
        suffix_path_fields: ACCOUNT_SUFFIX_PATH_FIELDS,
        s3_suffix_path: DAILY_SUFFIX_PATH,
        log_types: &[
            log_type_with_s3_tables("ALB_ACCESS_LOGS", ALB_ACCESS_FIELDS, "aws_alb", "access"),
            log_type_with_s3_tables(
                "ALB_CONNECTION_LOGS",
                ALB_CONNECTION_FIELDS,
                "aws_alb",
                "connection",
            ),
            log_type("ALB_HEALTH_CHECK_LOGS", ALB_HEALTH_CHECK_FIELDS),
            log_type("NLB_ACCESS_LOGS", NLB_ACCESS_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "events",
        resource_type: "event-bus",
        suffix_path_fields: &[
            "accountid",
            "event_bus_name",
            "region",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: "{region}/{event_bus_name}/{yyyy}/{MM}/{dd}/{HH}/",
        log_types: &[
            log_type("ERROR_LOGS", EVENTS_FIELDS),
            log_type("INFO_LOGS", EVENTS_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "events",
        resource_type: "subscriber",
        suffix_path_fields: &[
            "accountid",
            "subscriber_name",
            "region",
            "yyyy",
            "MM",
            "dd",
            "HH",
        ],
        s3_suffix_path: "{region}/{subscriber_name}/{yyyy}/{MM}/{dd}/{HH}/",
        log_types: &[
            log_type("ERROR_LOGS", EVENTS_FIELDS),
            log_type("INFO_LOGS", EVENTS_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "logs",
        resource_type: "log-group",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: RESOURCE_ID_SUFFIX_PATH,
        log_types: &[log_type("INSIGHTS_QUERY_LOGS", INSIGHTS_QUERY_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "mediatailor",
        resource_type: "playbackConfiguration",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: RESOURCE_ID_SUFFIX_PATH,
        log_types: &[
            log_type("AD_DECISION_SERVER_LOGS", MEDIATAILOR_FIELDS),
            log_type("MANIFEST_SERVICE_LOGS", MEDIATAILOR_FIELDS),
            log_type("TRANSCODE_LOGS", MEDIATAILOR_FIELDS),
        ],
    },
    // Amazon Q Business sends the logs of its APPLICATION resource, and the `logType` member's
    // own catalogue names both types it sends ("For Amazon Q, the valid values are EVENT_LOGS
    // and SYNC_JOB_LOGS",
    // `smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`).
    ConfigurationTemplateSource {
        service: "qbusiness",
        resource_type: "application",
        suffix_path_fields: ACCOUNT_SUFFIX_PATH_FIELDS,
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("EVENT_LOGS", QBUSINESS_FIELDS),
            log_type("SYNC_JOB_LOGS", QBUSINESS_FIELDS),
        ],
    },
    ConfigurationTemplateSource {
        service: "s3",
        resource_type: "bucket",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: RESOURCE_ID_SUFFIX_PATH,
        log_types: &[log_type("S3_SERVER_ACCESS_LOGS", S3_SERVER_ACCESS_FIELDS)],
    },
    // Security Hub sends its findings from EITHER of its two hub resources, which the
    // `logType` member's own catalogue states for this log type: "For the
    // SECURITY_FINDING_LOGS logType, use a wildcard ARN for the hub resource. For AWS Security
    // Hub, use arn:aws:securityhub:us-east-1:111122223333:hub/* and for Amazon Web Services
    // Security Hub, use arn:aws:securityhub:us-east-1:111122223333:hubv2/*"
    // (`smithy_query(query="doc",shape="PutDeliverySourceRequest",member="logType")`). So the
    // catalogue publishes a row per hub resource type, and a caller holding a source over
    // either one finds the templates that apply to it. The newer hub resource is published
    // FIRST, so a caller that takes the first SECURITY_FINDING_LOGS template of the service
    // binds its delivery to the hub resource the service vends findings from today.
    ConfigurationTemplateSource {
        service: "securityhub",
        resource_type: "hubv2",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        // Security Hub publishes no object prefix of its own: a delivery of its findings
        // writes under the prefix its destination configures.
        s3_suffix_path: NO_SUFFIX_PATH,
        log_types: &[log_type("SECURITY_FINDING_LOGS", SECURITY_FINDING_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "securityhub",
        resource_type: "hub",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: NO_SUFFIX_PATH,
        log_types: &[log_type("SECURITY_FINDING_LOGS", SECURITY_FINDING_FIELDS)],
    },
    ConfigurationTemplateSource {
        service: "ses",
        resource_type: "mailmanager",
        suffix_path_fields: RESOURCE_ID_SUFFIX_PATH_FIELDS,
        s3_suffix_path: RESOURCE_ID_SUFFIX_PATH,
        log_types: &[
            log_type("APPLICATION_LOGS", SES_APPLICATION_FIELDS),
            log_type(
                "TRAFFIC_POLICY_DEBUG_LOGS",
                SES_TRAFFIC_POLICY_DEBUG_FIELDS,
            ),
        ],
    },
    ConfigurationTemplateSource {
        service: "workmail",
        resource_type: "organization",
        suffix_path_fields: ACCOUNT_SUFFIX_PATH_FIELDS,
        // WorkMail partitions its delivered objects by time alone, under the Region the
        // delivery was made in.
        s3_suffix_path: DEFAULT_DELIVERY_SUFFIX_PATH,
        log_types: &[
            log_type("ACCESS_CONTROL_LOGS", WORKMAIL_ACCESS_CONTROL_FIELDS),
            log_type("AUTHENTICATION_LOGS", WORKMAIL_AUTHENTICATION_FIELDS),
            log_type(
                "WORKMAIL_AVAILABILITY_PROVIDER_LOGS",
                WORKMAIL_AVAILABILITY_PROVIDER_FIELDS,
            ),
            log_type(
                "WORKMAIL_MAILBOX_ACCESS_LOGS",
                WORKMAIL_MAILBOX_ACCESS_FIELDS,
            ),
            log_type(
                "WORKMAIL_PERSONAL_ACCESS_TOKEN_LOGS",
                WORKMAIL_PERSONAL_ACCESS_TOKEN_FIELDS,
            ),
        ],
    },
];

/// Every configuration template of this service, in the order the catalogue publishes them.
///
/// One enumeration, built from [`CONFIGURATION_TEMPLATE_SOURCES`] on every call, row by row and
/// within a row log type by log type and destination type by destination type (see this
/// module's own documentation): the catalogue is service-managed — no operation creates,
/// updates or deletes a template — so two calls report the same templates in the same order,
/// and that order is what makes `DescribeConfigurationTemplates`'s pagination cursor resume at
/// a position that does not move.
pub fn configuration_templates() -> Vec<crate::ops::types::ConfigurationTemplate> {
    CONFIGURATION_TEMPLATE_SOURCES
        .iter()
        .flat_map(ConfigurationTemplateSource::templates)
        .collect()
}

/// What identifies one configuration template: the four values that distinguish it from every
/// other — its service, log type, resource type and delivery destination type — joined by a
/// character none of them admits (`Service` is `^[\w_-]*$`, `LogType` `^[\w]*$`,
/// `ResourceType` `^[\w-_]*$`, and `DeliveryDestinationType` is an enum of letters).
///
/// It is what a pagination cursor names the position it resumes after, which is a position in
/// the enumeration [`configuration_templates`] publishes rather than an order of its own.
pub fn configuration_template_key(template: &crate::ops::types::ConfigurationTemplate) -> String {
    [
        template.service.as_deref().unwrap_or_default(),
        template.log_type.as_deref().unwrap_or_default(),
        template.resource_type.as_deref().unwrap_or_default(),
        template
            .delivery_destination_type
            .as_deref()
            .unwrap_or_default(),
    ]
    .join("|")
}
