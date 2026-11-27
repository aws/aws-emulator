// SPDX-License-Identifier: CC0-1.0

//! The instance-type contract of `CreateElasticsearchDomain`, driven as real
//! restJson1 requests through the router.
//!
//! The rule under test: a cluster member whose target is the service's
//! instance-type enum is accepted for EVERY member that enum declares and
//! refused for a string it does not, so a request naming a generally-available
//! instance type reaches the handler and a domain is created for it.
//!
//! The accepted rows below are one per instance FAMILY the enum carries, and
//! the rejected rows are the ways a string can fall outside it (a missing
//! service suffix, another service's suffix, a family the enum has no member
//! for, a case-shifted member). The catalog's source is this service's own
//! model:
//!
//! ```text
//! smithy_query(query="shape",shape="ESPartitionInstanceType",model_path="services/service-com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015/smithy-model/com-amazonaws-elasticsearchservice-amazonelasticsearchservice2015.json")
//! ```

use serde_json::{json, Value};
use smithy_server_runtime::MultiServiceRouter;

use crate::harness::{call_in, router, Answer};

/// The region the test signs for.
const REGION: &str = "us-west-2";

async fn create(router: &MultiServiceRouter, body: &Value) -> Answer {
    call_in(router, REGION, "POST", "/2015-01-01/es/domain", Some(body)).await
}

async fn describe(router: &MultiServiceRouter, name: &str) -> Answer {
    call_in(
        router,
        REGION,
        "GET",
        &format!("/2015-01-01/es/domain/{name}"),
        None,
    )
    .await
}

/// One member of `ESPartitionInstanceType` per family the enum declares — the
/// smallest size of each, so every row is a type a caller would really ask for.
/// `t2.small.elasticsearch` is one row of this set, not the rule.
const ONE_PER_FAMILY: [&str; 13] = [
    "c4.large.elasticsearch",
    "c5.large.elasticsearch",
    "d2.xlarge.elasticsearch",
    "i2.xlarge.elasticsearch",
    "i3.large.elasticsearch",
    "m3.medium.elasticsearch",
    "m4.large.elasticsearch",
    "m5.large.elasticsearch",
    "r3.large.elasticsearch",
    "r4.large.elasticsearch",
    "r5.large.elasticsearch",
    "t2.micro.elasticsearch",
    "ultrawarm1.medium.elasticsearch",
];

/// Every family's member is accepted, and the created domain reports the type it
/// was asked for.
#[tokio::test]
async fn every_modelled_instance_type_is_accepted_and_persisted() {
    for (index, instance_type) in ONE_PER_FAMILY.iter().enumerate() {
        let router = router();
        let name = format!("instance-type-{index}");
        let created = create(
            &router,
            &json!({
                "DomainName": name,
                "ElasticsearchVersion": "7.10",
                "ElasticsearchClusterConfig": {"InstanceType": instance_type, "InstanceCount": 1},
                "EBSOptions": {"EBSEnabled": true, "VolumeType": "gp2", "VolumeSize": 10}
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {instance_type:?}: create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.at(
                instance_type,
                "/DomainStatus/ElasticsearchClusterConfig/InstanceType"
            ),
            &json!(instance_type),
            "case {instance_type:?}: the created domain must report the type asked for, got {:?}",
            created
                .body
                .pointer("/DomainStatus/ElasticsearchClusterConfig/InstanceType")
        );

        let read = describe(&router, &name).await;
        assert_eq!(
            read.at(
                instance_type,
                "/DomainStatus/ElasticsearchClusterConfig/InstanceType"
            ),
            &json!(instance_type),
            "case {instance_type:?}: a later describe must report the stored type, got {:?}",
            read.body
                .pointer("/DomainStatus/ElasticsearchClusterConfig/InstanceType")
        );
    }
}

/// The same enum is the target of `DedicatedMasterType`, so the rule holds
/// there too: a member of it is accepted and persisted.
#[tokio::test]
async fn the_dedicated_master_type_takes_the_same_instance_types() {
    for (index, instance_type) in ["m3.medium.elasticsearch", "c5.large.elasticsearch"]
        .iter()
        .enumerate()
    {
        let router = router();
        let name = format!("master-type-{index}");
        let created = create(
            &router,
            &json!({
                "DomainName": name,
                "ElasticsearchVersion": "7.10",
                "ElasticsearchClusterConfig": {
                    "InstanceType": "t2.small.elasticsearch",
                    "InstanceCount": 2,
                    "DedicatedMasterEnabled": true,
                    "DedicatedMasterType": instance_type,
                    "DedicatedMasterCount": 3
                }
            }),
        )
        .await;
        assert_eq!(
            created.status, 200,
            "case {instance_type:?}: create must answer 200, got {} {}",
            created.status, created.body
        );
        assert_eq!(
            created.at(
                instance_type,
                "/DomainStatus/ElasticsearchClusterConfig/DedicatedMasterType"
            ),
            &json!(instance_type),
            "case {instance_type:?}: the created domain must report the master type asked for, got {:?}",
            created
                .body
                .pointer("/DomainStatus/ElasticsearchClusterConfig/DedicatedMasterType")
        );
    }
}

/// A string the enum does not declare is refused with HTTP 400
/// `ValidationException`, and no domain is created for it.
#[tokio::test]
async fn an_instance_type_outside_the_enum_is_refused_and_stores_nothing() {
    for (case, instance_type) in [
        ("no service suffix", "t2.small"),
        ("another engine's suffix", "t2.small.search"),
        ("a family the enum has no member for", "m7g.large.elasticsearch"),
        ("a case-shifted member", "T2.small.elasticsearch"),
        ("a size the t2 family has no member for", "t2.24xlarge.elasticsearch"),
        ("a family the model no longer declares", "t3.small.elasticsearch"),
    ] {
        let router = router();
        let name = "rejected-domain";
        let created = create(
            &router,
            &json!({
                "DomainName": name,
                "ElasticsearchVersion": "7.10",
                "ElasticsearchClusterConfig": {"InstanceType": instance_type, "InstanceCount": 1}
            }),
        )
        .await;
        assert_eq!(
            created.status, 400,
            "case {case:?} ({instance_type:?}): must be refused with 400, got {} {}",
            created.status, created.body
        );
        assert!(
            created.error_type.contains("ValidationException"),
            "case {case:?} ({instance_type:?}): the error must be ValidationException, got {:?} / {}",
            created.error_type,
            created.body
        );

        let read = describe(&router, name).await;
        assert_eq!(
            read.status, 404,
            "case {case:?} ({instance_type:?}): a refused create must store no domain, got {} {}",
            read.status, read.body
        );
    }
}
