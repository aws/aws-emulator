// SPDX-License-Identifier: CC0-1.0

//! The per-type CONFIGURATION a data source is set up through: which members carry one,
//! how many of them one request may carry, and the refusals for a request that does not
//! describe a data source AppSync could reach.
//!
//! A `DataSourceType` is configured by exactly ONE member of
//! `CreateDataSourceRequest` / `UpdateDataSourceRequest` — `dynamodbConfig`,
//! `lambdaConfig`, `elasticsearchConfig`, `openSearchServiceConfig`, `httpConfig`,
//! `relationalDatabaseConfig` or `eventBridgeConfig`
//! (`smithy_query(query="operation", operation="CreateDataSource,UpdateDataSource")`),
//! and `NONE` / `AMAZON_BEDROCK_RUNTIME` by none at all. The configuration a request may
//! carry is therefore exactly that one member: ANY OTHER configuration member it carries
//! is a second backend for one data source — whether it comes beside the type's own
//! configuration or in place of it — and a data source cannot reach two. Such a request
//! is refused as too many configurations, naming the type the request declared; a request
//! that carries none at all for a type that names one is refused as the absent
//! configuration instead, because there the fix is to add one rather than to remove one.
//!
//! Both are `BadRequestException` (HTTP 400), the one client error either operation models
//! for a request that is not well formed.
//!
//! The order the checks run in is the order the faults overrule each other: a request
//! that carries a configuration the declared type does not name is refused for that,
//! whether or not the type's own configuration is also there, because the fix is to remove
//! one rather than to add one.

use super::{missing_data_source_config_message, DataSourceRecord};

/// The `DataSourceType` whose backend is a relational database, i.e. the one type
/// `relationalDatabaseConfig` configures. Source: the `DataSourceType` enum.
pub const RELATIONAL_DATABASE_TYPE: &str = "RELATIONAL_DATABASE";

/// The configuration members a data source candidate carries, in the order the request
/// shape declares them.
///
/// Source: the configuration members of `CreateDataSourceRequest` /
/// `UpdateDataSourceRequest`; [`DataSourceRecord`] keeps one field per member, so the
/// members listed here are the whole set a request can fill.
pub fn supplied_data_source_configs(record: &DataSourceRecord) -> Vec<&'static str> {
    [
        ("dynamodbConfig", record.dynamodb_config.is_some()),
        ("lambdaConfig", record.lambda_config.is_some()),
        ("elasticsearchConfig", record.elasticsearch_config.is_some()),
        (
            "openSearchServiceConfig",
            record.open_search_service_config.is_some(),
        ),
        ("httpConfig", record.http_config.is_some()),
        (
            "relationalDatabaseConfig",
            record.relational_database_config.is_some(),
        ),
        ("eventBridgeConfig", record.event_bridge_config.is_some()),
    ]
    .into_iter()
    .filter_map(|(member, carried)| carried.then_some(member))
    .collect()
}

/// Whether `record` carries the configuration member `member`.
pub fn carries_data_source_config(record: &DataSourceRecord, member: &str) -> bool {
    supplied_data_source_configs(record).contains(&member)
}

/// The configuration member a `DataSourceType` is set up through, or `None` for a type
/// configured through no member at all.
///
/// Source: the per-type configuration members of `CreateDataSourceRequest` /
/// `UpdateDataSourceRequest` and the `DataSourceType` values they configure —
/// `dynamodbConfig` ("Amazon DynamoDB settings"), `lambdaConfig` ("Lambda settings"),
/// `elasticsearchConfig` / `openSearchServiceConfig` ("Amazon OpenSearch Service
/// settings"), `httpConfig` ("HTTP endpoint settings"), `relationalDatabaseConfig`
/// ("Relational database settings") and `eventBridgeConfig` ("Amazon EventBridge
/// settings"). `NONE` and `AMAZON_BEDROCK_RUNTIME` are configured by no member of either
/// request, so a data source of either type is complete without one.
pub fn data_source_config_member(r#type: &str) -> Option<&'static str> {
    match r#type {
        "AMAZON_DYNAMODB" => Some("dynamodbConfig"),
        "AWS_LAMBDA" => Some("lambdaConfig"),
        "AMAZON_ELASTICSEARCH" => Some("elasticsearchConfig"),
        "AMAZON_OPENSEARCH_SERVICE" => Some("openSearchServiceConfig"),
        "HTTP" => Some("httpConfig"),
        "RELATIONAL_DATABASE" => Some("relationalDatabaseConfig"),
        "AMAZON_EVENTBRIDGE" => Some("eventBridgeConfig"),
        _ => None,
    }
}

/// The name AppSync states a `DataSourceType` by in a refusal: the service's own name for
/// the backend that type names.
///
/// It is the SERVICE's vocabulary rather than the model's, and not the modelled value in
/// any casing. ONE derivation, stated once and applied to every value: the vendor prefix
/// the wire form carries (`AWS_`, `AMAZON_`) is dropped and each remaining word is
/// capitalised — which is how a recorded refusal states `AWS_LAMBDA`, as `Lambda`. The one
/// value the service names outside that derivation is `AMAZON_DYNAMODB`, which a recorded
/// refusal states as `dynamo` rather than as its product name, so that one value is the
/// exception the derivation is read against.
///
/// Those two values are the ones a recorded refusal states; every other value takes the
/// derivation's reading of the same rule, which a later recording of a refusal naming one
/// of them can refine.
pub fn data_source_type_name(r#type: &str) -> String {
    /// The `DataSourceType` the service names outside the derivation below, and the name
    /// it states it by (the recorded exception).
    const UNDERIVED: (&str, &str) = ("AMAZON_DYNAMODB", "dynamo");
    /// The vendor prefixes a `DataSourceType`'s wire form carries and its name does not.
    const VENDOR_PREFIXES: &[&str] = &["AWS_", "AMAZON_"];

    if r#type == UNDERIVED.0 {
        return UNDERIVED.1.to_string();
    }
    let named = VENDOR_PREFIXES
        .iter()
        .find_map(|prefix| r#type.strip_prefix(prefix))
        .unwrap_or(r#type);
    named.split('_').map(capitalised).collect()
}

/// `word` with its first character upper-cased and the rest lower-cased, which is how a
/// `DataSourceType`'s SHOUTING wire form reads as the backend's name.
fn capitalised(word: &str) -> String {
    let mut characters = word.chars();
    match characters.next() {
        Some(first) => first
            .to_uppercase()
            .chain(characters.flat_map(char::to_lowercase))
            .collect(),
        None => String::new(),
    }
}

/// The text `BadRequestException` (HTTP 400) carries for a request that carries a
/// configuration member the declared type does not name: the type the request declares,
/// stated by the service's own name for it ([`data_source_type_name`]).
///
/// One data source reaches ONE backend, so a configuration beside — or instead of — the
/// one its type names is a second backend the data source cannot have; the type is what
/// says which single configuration the caller meant to keep.
pub fn multiple_data_source_configs_message(r#type: &str) -> String {
    format!(
        "Multiple configs given for data source of type {}.",
        data_source_type_name(r#type)
    )
}

/// The text `BadRequestException` (HTTP 400) carries for a relational-database data
/// source whose configuration names no source type.
///
/// `RelationalDatabaseDataSourceConfig$relationalDatabaseSourceType` is the "source type
/// for the relational database" — the only form of relational backend AppSync reaches,
/// `RDS_HTTP_ENDPOINT`, is named through it — so a configuration without it says nothing
/// about how the database is reached. The sentence is fixed and names nothing of the
/// request.
pub const MISSING_RELATIONAL_DATABASE_SOURCE_TYPE_MESSAGE: &str =
    "Missing relational database source type";

/// AppSync's fixed, request-independent text for a data source that would act with the
/// CALLER's own credentials on an API that does not authenticate its callers with IAM,
/// carried by `BadRequestException` (HTTP 400).
///
/// `DynamodbDataSourceConfig$useCallerCredentials` has AppSync reach the table as the
/// CALLER rather than as the data source's own role, so the call has to arrive with an
/// IAM identity to act as: that is what an API authenticating with `AWS_IAM` gives it,
/// and an API in any other authorization mode has no caller credentials to pass on. The
/// sentence names neither the API nor the data source.
pub const CALLER_CREDENTIALS_REQUIRE_IAM_MESSAGE: &str =
    "Caller credentials can only be used with AWS IAM authenticated API";

/// The refusal text for a candidate whose settings ask for the CALLER's credentials on an
/// API that is not IAM authenticated (`iam_authenticated_api`), or `None` for one AppSync
/// could reach.
///
/// Whether the API authenticates with IAM is the one reading
/// [`crate::service::graphql_api::configures`] gives — its own `authenticationType` or any
/// of its additional providers — so a data source is judged against the modes the API
/// really set up.
pub fn caller_credentials_refusal(
    record: &DataSourceRecord,
    iam_authenticated_api: bool,
) -> Option<&'static str> {
    let uses_caller_credentials = record
        .dynamodb_config
        .as_ref()
        .is_some_and(|config| config.use_caller_credentials);
    (uses_caller_credentials && !iam_authenticated_api)
        .then_some(CALLER_CREDENTIALS_REQUIRE_IAM_MESSAGE)
}

/// AppSync's fixed, request-independent text for a data source whose ENDPOINT
/// configuration is not one AppSync could reach, carried by `BadRequestException`
/// (HTTP 400).
pub const INVALID_ENDPOINT_CONFIGURATION_MESSAGE: &str = "Invalid endpoint configuration.";

/// AppSync's fixed, request-independent text for a relational-database configuration that
/// names the RDS HTTP endpoint as its source type and carries no settings for it, carried
/// by `BadRequestException` (HTTP 400).
///
/// It names the BLOCK the request left out, the way the per-type data-source configuration
/// does ([`super::missing_data_source_config_message`], "Missing `<backend>` data source
/// config"): a nested settings block the declared source type is set up through is a
/// configuration of its own, so a request that omits it is told which block to add rather
/// than that the endpoint it describes is unusable — there is no endpoint to judge yet. The
/// block is `rdsHttpEndpointConfig`, "Amazon RDS HTTP endpoint settings", which the service
/// names `RDS Http`.
pub const MISSING_RDS_HTTP_CONFIG_MESSAGE: &str = "Missing RDS Http config";

/// The refusal text for a candidate whose endpoint configuration names no endpoint
/// AppSync could reach, or `None` for one that does.
///
/// A data source is reached AT an endpoint, so the configuration that carries one has to
/// carry a usable one:
///
/// * an `HTTP` data source is reached at `httpConfig.endpoint` — "the HTTP URL endpoint"
///   — so a configuration with no endpoint, or one that is not an absolute URL, names
///   nowhere to send a request;
/// * an `AMAZON_ELASTICSEARCH` or `AMAZON_OPENSEARCH_SERVICE` data source is reached at
///   its own `endpoint` ("the endpoint"), which is a domain's URL, so the same reading
///   applies to it;
/// * a `RELATIONAL_DATABASE` data source whose source type is `RDS_HTTP_ENDPOINT` is
///   reached through `relationalDatabaseConfig.rdsHttpEndpointConfig` ("Amazon RDS HTTP
///   endpoint settings") — the cluster's own ARN ("Amazon RDS cluster Amazon Resource
///   Name") in the Region that endpoint is served in — so settings that name no cluster or
///   no Region name nowhere either. Settings that are ABSENT are refused as the missing
///   block instead ([`MISSING_RDS_HTTP_CONFIG_MESSAGE`]), because there the fix is to add
///   the block rather than to correct the endpoint inside it.
///
/// Every other type is reached by an identifier or an ARN of its own rather than by an
/// endpoint the request describes, so there is no endpoint configuration to judge.
pub fn endpoint_configuration_refusal(record: &DataSourceRecord) -> Option<&'static str> {
    let http_endpoint_unusable = record.r#type == super::HTTP_DATA_SOURCE_TYPE
        && record.http_config.as_ref().is_some_and(|config| {
            config
                .endpoint
                .as_deref()
                .is_none_or(|endpoint| !super::is_absolute_url(endpoint))
        });
    let search_endpoint_unusable = record
        .elasticsearch_config
        .as_ref()
        .is_some_and(|config| !super::is_absolute_url(&config.endpoint))
        || record
            .open_search_service_config
            .as_ref()
            .is_some_and(|config| !super::is_absolute_url(&config.endpoint));
    if http_endpoint_unusable || search_endpoint_unusable {
        return Some(INVALID_ENDPOINT_CONFIGURATION_MESSAGE);
    }
    rds_endpoint_refusal(record)
}

/// The refusal text for a relational-database candidate whose RDS HTTP endpoint settings
/// are absent or name no endpoint AppSync could reach, or `None` for one that does (and for
/// a candidate that is not one).
fn rds_endpoint_refusal(record: &DataSourceRecord) -> Option<&'static str> {
    if record.r#type != RELATIONAL_DATABASE_TYPE {
        return None;
    }
    let config = record.relational_database_config.as_ref()?;
    if config.relational_database_source_type.as_deref() != Some(RDS_HTTP_ENDPOINT_SOURCE) {
        return None;
    }
    let Some(endpoint) = config.rds_http_endpoint_config.as_ref() else {
        return Some(MISSING_RDS_HTTP_CONFIG_MESSAGE);
    };
    let names_no_endpoint = endpoint
        .db_cluster_identifier
        .as_deref()
        .is_none_or(str::is_empty)
        || endpoint.aws_region.as_deref().is_none_or(str::is_empty);
    names_no_endpoint.then_some(INVALID_ENDPOINT_CONFIGURATION_MESSAGE)
}

/// The one `RelationalDatabaseSourceType` the model declares: "the relational database
/// source type is an Amazon Relational Database Service (Amazon RDS) HTTP endpoint"
/// (`smithy_query(query="shape", shape="RelationalDatabaseSourceType")`).
pub const RDS_HTTP_ENDPOINT_SOURCE: &str = "RDS_HTTP_ENDPOINT";

/// The refusal text for a data-source candidate whose CONFIGURATION does not describe a
/// backend AppSync could reach, or `None` for one that does.
///
/// The one reading both the create and the update judge their candidate by, so the two
/// cannot disagree about which requests describe a usable data source:
///
/// 1. a configuration member the declared type does not name — a second one beside the
///    type's own, or one in place of it — is refused as too many configurations, naming
///    the declared type;
/// 2. no configuration at all for a type that names one — refused as the absent one,
///    naming that backend ([`missing_data_source_config_message`]);
/// 3. a relational-database configuration that names no source type
///    ([`MISSING_RELATIONAL_DATABASE_SOURCE_TYPE_MESSAGE`]);
/// 4. a nested settings block the declared source type is set up through that the request
///    omits, or an endpoint configuration that names no endpoint AppSync could reach
///    ([`endpoint_configuration_refusal`]).
///
/// Why 1 is about the MEMBERS rather than about how many there are: a recorded create
/// declaring one type and carrying a SINGLE configuration of another is answered with the
/// too-many sentence, not with the absent-configuration one — the configuration a data
/// source may carry is the one its type names, so any other is an extra whether or not
/// the type's own is there too, and counting them would answer the wrong one of these two
/// sentences for that request. A request carrying NOTHING is the only one with no extra
/// configuration to remove, which is why that one is 2.
pub fn data_source_configuration_refusal(record: &DataSourceRecord) -> Option<String> {
    let own = data_source_config_member(&record.r#type);
    if supplied_data_source_configs(record)
        .iter()
        .any(|member| Some(*member) != own)
    {
        return Some(multiple_data_source_configs_message(&record.r#type));
    }
    if let Some(member) = record.missing_config() {
        return Some(missing_data_source_config_message(member));
    }
    if record.r#type == RELATIONAL_DATABASE_TYPE
        && record
            .relational_database_config
            .as_ref()
            .and_then(|config| config.relational_database_source_type.as_deref())
            .is_none_or(str::is_empty)
    {
        return Some(MISSING_RELATIONAL_DATABASE_SOURCE_TYPE_MESSAGE.to_string());
    }
    endpoint_configuration_refusal(record).map(str::to_string)
}

#[cfg(test)]
mod tests {
    use super::*;

    /// A candidate of `type` carrying the configuration members `members` names, each of
    /// them USABLE: the endpoint-carrying ones name an endpoint, so a row is refused for
    /// what it is about rather than for an endpoint it left out.
    fn candidate(r#type: &str, members: &[&str]) -> DataSourceRecord {
        let mut record = DataSourceRecord {
            name: "ds".to_string(),
            arn: "arn".to_string(),
            description: None,
            r#type: r#type.to_string(),
            service_role_arn: Some("arn:aws:iam::000000000000:role/appsync".to_string()),
            dynamodb_config: None,
            lambda_config: None,
            elasticsearch_config: None,
            open_search_service_config: None,
            http_config: None,
            relational_database_config: None,
            event_bridge_config: None,
            metrics_config: None,
        };
        for member in members {
            match *member {
                "dynamodbConfig" => record.dynamodb_config = Some(Default::default()),
                "lambdaConfig" => record.lambda_config = Some(Default::default()),
                "elasticsearchConfig" => record.elasticsearch_config = Some(Default::default()),
                "openSearchServiceConfig" => {
                    record.open_search_service_config = Some(Default::default())
                }
                "httpConfig" => {
                    record.http_config = Some(crate::ops::types::HttpDataSourceConfig {
                        endpoint: Some("https://orders.example.com".to_string()),
                        ..Default::default()
                    })
                }
                "relationalDatabaseConfig" => {
                    record.relational_database_config =
                        Some(crate::ops::types::RelationalDatabaseDataSourceConfig {
                            relational_database_source_type: None,
                            rds_http_endpoint_config: Some(
                                crate::ops::types::RdsHttpEndpointConfig {
                                    aws_region: Some("eu-west-1".to_string()),
                                    db_cluster_identifier: Some("orders-cluster".to_string()),
                                    ..Default::default()
                                },
                            ),
                        })
                }
                "eventBridgeConfig" => record.event_bridge_config = Some(Default::default()),
                other => panic!("case {other:?}: not a configuration member"),
            }
        }
        record
    }

    /// RULE: a candidate carrying a configuration its declared type does not name is
    /// refused as too many, whichever type it declares and whichever configurations they
    /// are.
    #[test]
    fn several_configurations_are_refused_as_too_many() {
        for (case, r#type, members, expected) in [
            (
                "the declared type's own configuration and another",
                "AMAZON_DYNAMODB",
                &["dynamodbConfig", "lambdaConfig"][..],
                "Multiple configs given for data source of type dynamo.",
            ),
            (
                "two configurations, neither of the declared type",
                "AMAZON_DYNAMODB",
                &["lambdaConfig", "httpConfig"][..],
                "Multiple configs given for data source of type dynamo.",
            ),
            (
                "ONE configuration, not the declared type's",
                "AMAZON_DYNAMODB",
                &["lambdaConfig"][..],
                "Multiple configs given for data source of type dynamo.",
            ),
            (
                "another type, its own configuration and another",
                "AWS_LAMBDA",
                &["lambdaConfig", "eventBridgeConfig"][..],
                "Multiple configs given for data source of type Lambda.",
            ),
            (
                "a type configured by no member at all, carrying one",
                "NONE",
                &["lambdaConfig"][..],
                "Multiple configs given for data source of type None.",
            ),
        ] {
            let refusal = data_source_configuration_refusal(&candidate(r#type, members));
            assert_eq!(
                refusal.as_deref(),
                Some(expected),
                "case {case:?}: expected the too-many sentence, got {refusal:?}"
            );
        }
    }

    /// RULE: exactly one configuration, of the declared type, is accepted — except for a
    /// relational database that names no source type.
    #[test]
    fn one_configuration_of_the_declared_type_is_accepted() {
        for (case, r#type, member) in [
            ("dynamo", "AMAZON_DYNAMODB", "dynamodbConfig"),
            ("lambda", "AWS_LAMBDA", "lambdaConfig"),
            ("http", "HTTP", "httpConfig"),
            ("event bridge", "AMAZON_EVENTBRIDGE", "eventBridgeConfig"),
        ] {
            let refusal = data_source_configuration_refusal(&candidate(r#type, &[member]));
            assert!(
                refusal.is_none(),
                "case {case:?}: expected no refusal, got {refusal:?}"
            );
        }

        let mut relational = candidate(RELATIONAL_DATABASE_TYPE, &["relationalDatabaseConfig"]);
        let refusal = data_source_configuration_refusal(&relational);
        assert_eq!(
            refusal.as_deref(),
            Some(MISSING_RELATIONAL_DATABASE_SOURCE_TYPE_MESSAGE),
            "case \"a relational database with no source type\": expected the missing \
             source-type sentence, got {refusal:?}"
        );
        if let Some(config) = relational.relational_database_config.as_mut() {
            config.relational_database_source_type = Some("RDS_HTTP_ENDPOINT".to_string());
        }
        let refusal = data_source_configuration_refusal(&relational);
        assert!(
            refusal.is_none(),
            "case \"a relational database naming its source type\": expected no refusal, \
             got {refusal:?}"
        );
    }

    /// RULE: an endpoint-carrying configuration that names no endpoint AppSync could
    /// reach is refused, whichever of the two types carries one.
    #[test]
    fn an_unusable_endpoint_configuration_is_refused() {
        // An HTTP data source with no endpoint at all, and with one that is no absolute
        // URL: neither names anywhere to send a request.
        for (case, endpoint) in [
            ("no endpoint at all", None),
            ("an empty endpoint", Some("")),
            ("a bare host", Some("orders.example.com")),
            ("a path with no scheme or host", Some("/orders")),
        ] {
            let mut record = candidate("HTTP", &["httpConfig"]);
            if let Some(config) = record.http_config.as_mut() {
                config.endpoint = endpoint.map(str::to_string);
            }
            let refusal = data_source_configuration_refusal(&record);
            assert_eq!(
                refusal.as_deref(),
                Some(INVALID_ENDPOINT_CONFIGURATION_MESSAGE),
                "case {case:?}: expected the endpoint sentence, got {refusal:?}"
            );
        }

        // A relational database whose source type names the RDS HTTP endpoint and whose
        // endpoint settings name no cluster or no Region: the settings are THERE, so what
        // is refused is the endpoint they describe.
        for (case, endpoint) in [
            (
                "endpoint settings that name nothing",
                crate::ops::types::RdsHttpEndpointConfig::default(),
            ),
            (
                "endpoint settings naming no cluster",
                crate::ops::types::RdsHttpEndpointConfig {
                    aws_region: Some("eu-west-1".to_string()),
                    ..Default::default()
                },
            ),
            (
                "endpoint settings naming no Region",
                crate::ops::types::RdsHttpEndpointConfig {
                    db_cluster_identifier: Some("orders-cluster".to_string()),
                    ..Default::default()
                },
            ),
        ] {
            let mut record = candidate(RELATIONAL_DATABASE_TYPE, &["relationalDatabaseConfig"]);
            if let Some(config) = record.relational_database_config.as_mut() {
                config.relational_database_source_type = Some(RDS_HTTP_ENDPOINT_SOURCE.to_string());
                config.rds_http_endpoint_config = Some(endpoint);
            }
            let refusal = data_source_configuration_refusal(&record);
            assert_eq!(
                refusal.as_deref(),
                Some(INVALID_ENDPOINT_CONFIGURATION_MESSAGE),
                "case {case:?}: expected the endpoint sentence, got {refusal:?}"
            );
        }

        // The same source type with NO settings block at all names the block to add.
        let mut record = candidate(RELATIONAL_DATABASE_TYPE, &["relationalDatabaseConfig"]);
        if let Some(config) = record.relational_database_config.as_mut() {
            config.relational_database_source_type = Some(RDS_HTTP_ENDPOINT_SOURCE.to_string());
            config.rds_http_endpoint_config = None;
        }
        let refusal = data_source_configuration_refusal(&record);
        assert_eq!(
            refusal.as_deref(),
            Some(MISSING_RDS_HTTP_CONFIG_MESSAGE),
            "case \"no endpoint settings at all\": expected the missing-block sentence, got \
             {refusal:?}"
        );

        // A search endpoint that is no absolute URL.
        for (case, endpoint) in [
            ("an empty OpenSearch endpoint", ""),
            ("a bare OpenSearch host", "search.example.com"),
        ] {
            let mut record = candidate("AMAZON_OPENSEARCH_SERVICE", &["openSearchServiceConfig"]);
            if let Some(config) = record.open_search_service_config.as_mut() {
                config.endpoint = endpoint.to_string();
            }
            let refusal = data_source_configuration_refusal(&record);
            assert_eq!(
                refusal.as_deref(),
                Some(INVALID_ENDPOINT_CONFIGURATION_MESSAGE),
                "case {case:?}: expected the endpoint sentence, got {refusal:?}"
            );
        }

        // With a cluster, a Region and a URL, the same candidates are accepted.
        let mut record = candidate(RELATIONAL_DATABASE_TYPE, &["relationalDatabaseConfig"]);
        if let Some(config) = record.relational_database_config.as_mut() {
            config.relational_database_source_type = Some(RDS_HTTP_ENDPOINT_SOURCE.to_string());
            config.rds_http_endpoint_config = Some(crate::ops::types::RdsHttpEndpointConfig {
                aws_region: Some("eu-west-1".to_string()),
                db_cluster_identifier: Some("orders-cluster".to_string()),
                ..Default::default()
            });
        }
        let refusal = data_source_configuration_refusal(&record);
        assert!(
            refusal.is_none(),
            "case \"a relational database naming its cluster and Region\": expected no \
             refusal, got {refusal:?}"
        );
        let mut record = candidate("AMAZON_OPENSEARCH_SERVICE", &["openSearchServiceConfig"]);
        if let Some(config) = record.open_search_service_config.as_mut() {
            config.endpoint = "https://search-orders.eu-west-1.es.amazonaws.com".to_string();
        }
        let refusal = data_source_configuration_refusal(&record);
        assert!(
            refusal.is_none(),
            "case \"an OpenSearch domain naming its URL\": expected no refusal, got \
             {refusal:?}"
        );
    }

    /// RULE: a candidate carrying NO configuration is refused as the ABSENT one for the
    /// type it declares, so the two faults keep their own sentences.
    #[test]
    fn no_configuration_is_refused_as_the_absent_one() {
        for (case, r#type, expected) in [
            (
                "dynamo",
                "AMAZON_DYNAMODB",
                "Missing DynamoDB data source config",
            ),
            ("lambda", "AWS_LAMBDA", "Missing Lambda data source config"),
            ("http", "HTTP", "Missing HTTP endpoint data source config"),
            (
                "a relational database",
                RELATIONAL_DATABASE_TYPE,
                "Missing relational database data source config",
            ),
        ] {
            let refusal = data_source_configuration_refusal(&candidate(r#type, &[]));
            assert_eq!(
                refusal.as_deref(),
                Some(expected),
                "case {case:?}: expected the absent-configuration sentence, got {refusal:?}"
            );
        }

        // A type configured through no member is complete with no configuration at all.
        for r#type in ["NONE", "AMAZON_BEDROCK_RUNTIME"] {
            let refusal = data_source_configuration_refusal(&candidate(r#type, &[]));
            assert!(
                refusal.is_none(),
                "case {type:?} with no configuration: expected no refusal, got {refusal:?}"
            );
        }
    }
}
