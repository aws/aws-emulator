// SPDX-License-Identifier: CC0-1.0

#![forbid(unsafe_code)]
#![doc(hidden)]
//! Router — assembles all services into the multi-service emulator.
//!
//! # Writing integration tests
//!
//! Tests live in `router/tests/all/<name>.rs`, each a suite of the `all` target that
//! `router/tests/all/main.rs` roots, so nothing is declared by hand. Every router test
//! target links all services, so the router has exactly one; a file
//! directly under `router/tests/` fails the build. `build_router(engine)` returns the
//! `MultiServiceRouter` the production entrypoint serves; drive requests through
//! `router.dispatch(req)` and assert on the response. The framework handles
//! deserialization, routing, and engine injection.
//!
//! ## Pattern
//!
//! ```rust,ignore
//! use std::sync::Arc;
//!
//! #[tokio::test]
//! async fn test_create_thing() {
//!     let engine = Arc::new(engine::InMemoryEngine::new());
//!     let router = build_router(engine);
//!     let req = http::Request::builder()
//!         .method("POST")
//!         .uri("http://localhost/2021-01-01/things")
//!         .header("authorization", /* a SigV4 header naming the service */ "...")
//!         .body(bytes::Bytes::from_static(br#"{"Name":"my-thing"}"#))
//!         .unwrap();
//!     let resp = router.dispatch(req).await;
//!     // 501 until you implement the operation — a FRESHLY SCAFFOLDED operation
//!     // answers `NotImplemented`, so asserting 200 here fails until the `handle`
//!     // body is written. Change this to 200 as part of implementing it.
//!     assert_eq!(resp.status().as_u16(), 501);
//! }
//! ```
//!
//! ## Key points
//!
//! - **Direct construction**: each service is built from the shared engine via
//!   `<service>::EmulatedService::new(engine.clone())`, which constructs each
//!   operation's authored `impl Operation` struct through the engine-as-constructor
//!   seam. There is no builder and no per-operation registration step.
//! - **Routing**: `MultiServiceRouter` picks the service whose `can_handle` claims
//!   the request (seeded from the model's SigV4 signing name), then the service's
//!   protocol names the operation.
//! - **Deps**: a router integration test needs `service-foo`, `engine`, `http`,
//!   `bytes` and `tokio`. Put the test in the SERVICE crate rather than here:
//!   `router/Cargo.toml` is re-rendered on every service add (see the banner at the
//!   top of this file), so a dev-dependency added there is overwritten.

// @generated — RE-RENDERED ON EVERY SERVICE ADD. Edits below are OVERWRITTEN.
//
// Put hand-written code in a SERVICE crate, or in `router/src/main.rs`. Those are
// yours; this file, `router/Cargo.toml` and the workspace `Cargo.toml` are replaced
// whenever a service is added, so an edit here is lost.

use smithy_server_runtime::MultiServiceRouter;
use std::sync::Arc;
pub use engine::InMemoryEngine;

pub fn new_engine() -> Arc<InMemoryEngine> {
    Arc::new(InMemoryEngine::new())
}

/// Assemble the multi-service router the production entrypoint serves.
///
/// Registrations are DERIVED from the scaffolded service set, not hand-maintained:
/// adding a service re-renders this whole file. `router/tests/all/boot_serve.rs` exercises
/// this function.
///
/// Keep this a plain `fn` whose registrations stay in the `#[inline(never)]`
/// `mount_chunk_NN` helpers below, one frame at a time. Collapsing them into a single
/// chained expression holds every service's `new` frame at once and overflows
/// the stack here, aborting before either listener binds.
pub fn build_router(engine: Arc<InMemoryEngine>) -> MultiServiceRouter {
    let router = MultiServiceRouter::new();
    let router = mount_chunk_00(router, &engine);
    let router = mount_chunk_01(router, &engine);
    let router = mount_chunk_02(router, &engine);
    let router = mount_chunk_03(router, &engine);
    let router = mount_chunk_04(router, &engine);
    let router = mount_chunk_05(router, &engine);
    let router = mount_chunk_06(router, &engine);
    let router = mount_chunk_07(router, &engine);
    let router = mount_chunk_08(router, &engine);
    let router = mount_chunk_09(router, &engine);
    let router = mount_chunk_10(router, &engine);
    let router = mount_chunk_11(router, &engine);
    let router = mount_chunk_12(router, &engine);
    let router = mount_chunk_13(router, &engine);
    let router = mount_chunk_14(router, &engine);
    let router = mount_chunk_15(router, &engine);
    let router = mount_chunk_16(router, &engine);
    let router = mount_chunk_17(router, &engine);
    let router = mount_chunk_18(router, &engine);
    let router = mount_chunk_19(router, &engine);
    let router = mount_chunk_20(router, &engine);
    let router = mount_chunk_21(router, &engine);
    let router = mount_chunk_22(router, &engine);
    let router = mount_chunk_23(router, &engine);
    let router = mount_chunk_24(router, &engine);
    let router = mount_chunk_25(router, &engine);
    let router = mount_chunk_26(router, &engine);
    let router = mount_chunk_27(router, &engine);
    let router = mount_chunk_28(router, &engine);
    let router = mount_chunk_29(router, &engine);
    let router = mount_chunk_30(router, &engine);
    let router = mount_chunk_31(router, &engine);
    let router = mount_chunk_32(router, &engine);
    let router = mount_chunk_33(router, &engine);
    let router = mount_chunk_34(router, &engine);
    let router = mount_chunk_35(router, &engine);
    let router = mount_chunk_36(router, &engine);
    let router = mount_chunk_37(router, &engine);
    let router = mount_chunk_38(router, &engine);
    let router = mount_chunk_39(router, &engine);
    mount_chunk_40(router, &engine)
}

/// Register services 00 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_00(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_abusesignal_abusereportapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_accessanalyzer_accessanalyzer::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_account_account::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_accountaccess_awsaccountaccess::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_accountaccessruntime_awsaccountaccessruntime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_acm_certificatemanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_acmpca_acmprivateca::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentdiscovery_belfryglobaldp::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentregistry_agentregistry::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_agentregistrycontrol_agentregistrycontrol::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_aiops_aiops::EmulatedService::new(engine.clone()));
    router
}

/// Register services 01 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_01(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_amp_amazonprometheusservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplify_amplify::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplifybackend_amplifybackend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_amplifyuibuilder_amplifyuibuilder::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigateway_backplanecontrolservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigatewaymanagementapi_apigatewaymanagementapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apigatewayv2_apigatewayv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apiportal_apiportalcontrolservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appconfig_amazonappconfig::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appconfigdata_appconfigdata::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appfabric_fabricfrontendservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 02 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_02(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_appflow_sandstoneconfigurationservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appintegrations_amazonappintegrationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationautoscaling_anyscalefrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationcostprofiler_awsapplicationcostprofiler::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationdiscoveryservice_awsposeidonservice_v2015_11_01::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationinsights_ec2windowsbarleyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_applicationsignals_applicationsignals::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appmesh_appmesh::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_apprunner_apprunner::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appstream_photonadminproxyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_appsync_awsdeepdishcontrolplaneservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 03 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_03(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_arcregionswitch_arcregionswitch::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_arczonalshift_percdataplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_artifact_artifact::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_athena_amazonathena::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_auditmanager_bedrockassessmentmanagerlambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_autoscaling_autoscaling_2011_01_01::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_autoscalingplans_anyscalescalingplannerfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_b2bi_b2bi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_backup_cryocontrollerusermanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_backupgateway_backuponpremises_v20210101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_backupsearch_cryobackupsearchservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 04 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_04(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_batch_awsbatchv20160810::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmdashboards_awsbcmdashboardsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmdataexports_awsbillingandcostmanagementdataexports::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmpricingcalculator_awsbcmpricingcalculator::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bcmrecommendedactions_awsbillingandcostmanagementrecommendedactions::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrock_amazonbedrockcontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagent_amazonbedrockagentbuildtimelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagentcore_amazonbedrockagentcore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagentcorecontrol_amazonbedrockagentcorecontrol::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockagentruntime_amazonbedrockagentruntimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockdataautomation_amazonbedrockkeystonebuildtimeservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 05 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_05(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_bedrockdataautomationruntime_amazonbedrockkeystoneruntimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_bedrockruntime_amazonbedrockfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_billing_awsbilling::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_billingconductor_awsbillingconductor::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_braket_braket::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_budgets_awsbudgetservicegateway::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chatbot_wheatleyorchestration_20171011::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chime_ucbuzzconsoleservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkidentity_chimeidentityservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkmediapipelines_chimesdkmediapipelinesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkmeetings_chimemeetingssdkservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 06 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_06(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_chimesdkmessaging_chimemessagingservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_chimesdkvoice_chimesdktelephonyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cleanrooms_awsbastioncontrolplaneservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cleanroomsml_awsstarkcontrolservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloud9_awscloud9workspacemanagementservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudcontrol_cloudapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_clouddirectory_amazonclouddirectory_20170111::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudformation_cloudformation::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudfront_cloudfront2020_05_31::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudfrontkeyvaluestore_cloudfrontkeyvaluestore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudhsm_cloudhsmfrontendservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 07 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_07(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_cloudhsmv2_baldrapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudsearch_a9searchcloudconfigservice2013::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudsearchdomain_amazoncloudsearch2013::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudtrail_cloudtrail_20131101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudtraildata_cloudtraildataservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatch_graniteserviceversion20100801::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatchevents_awsevents::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatchlogs_logs_20140328::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cloudwatchomni_cloudwatchomnifrontend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codeartifact_codeartifactcontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codebuild_codebuild_20161006::EmulatedService::new(engine.clone()));
    router
}

/// Register services 08 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_08(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_codecatalyst_codecatalyst::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codecommit_codecommit_20150413::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codeconnections_codeconnections_20231201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codedeploy_codedeploy_20141006::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codeguruprofiler_codeguruprofiler::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codegurureviewer_awsgurufrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codegurusecurity_awscodegurusecurity::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codepipeline_codepipeline_20150709::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codestarconnections_codestar_connections_20191201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_codestarnotifications_codestarnotifications_20191015::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cognitoidentity_awscognitoidentityservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 09 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_09(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_cognitoidentityprovider_awscognitoidentityproviderservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_cognitosync_awscognitosyncservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_comprehend_comprehend_20171127::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_comprehendmedical_comprehendmedical_20181030::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_computeoptimizer_computeoptimizerservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_computeoptimizerautomation_computeoptimizerautomationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_configservice_starlingdoveservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connect_amazonconnectservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcampaigns_amazonconnectcampaignservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcampaignsv2_amazonconnectcampaignservicev2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectcases_amazonconnectcases::EmulatedService::new(engine.clone()));
    router
}

/// Register services 10 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_10(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_connectcontactlens_amazonconnectcontactlens::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connecthealth_connecthealth::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectparticipant_amazonconnectparticipantservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_connectscheduling_amazonconnectscheduling::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_continuum_continuumcontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_controlcatalog_controlcatalog::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_controltower_awscontroltowerapis::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_costandusagereportservice_awsorigamiservicegatewayservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_costexplorer_awsinsightsindexservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_costoptimizationhub_costoptimizationhubservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_customerprofiles_customerprofiles_20200815::EmulatedService::new(engine.clone()));
    router
}

/// Register services 11 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_11(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_databasemigrationservice_amazondmsv20160101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_databrew_awsgluedatabrew::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dataexchange_dataexchange::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_datapipeline_datapipeline::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_datasync_fmrsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_datazone_datazone::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dax_amazondaxv3::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_deadline_deadline::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_detective_amazondetective::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_devicefarm_devicefarm_20150623::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_devopsagent_devopsagent::EmulatedService::new(engine.clone()));
    router
}

/// Register services 12 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_12(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_devopsguru_capstonecontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_directconnect_overtureservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_directoryservice_directoryservice_20150416::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_directoryservicedata_directoryservicedata::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dlm_dlm_20180112::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_docdb_amazonrdsv19::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_docdbelastic_chimeradblionfishservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_drs_elasticdisasterrecoveryservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dsql_dsql::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dynamodb_dynamodb_20120810::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_dynamodbstreams_dynamodbstreams_20120810::EmulatedService::new(engine.clone()));
    router
}

/// Register services 13 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_13(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_ebs_ebs::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ec2_amazonec2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ec2instanceconnect_awsec2instanceconnectservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ecr_amazonec2containerregistry_v20150921::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ecrpublic_spencerfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ecs_amazonec2containerservicev20141113::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_efs_magnolioapiservice_v20150201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eks_awswesleyfrontend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eksauth_eksauthfrontend::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticache_amazonelasticachev9::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticbeanstalk_awselasticbeanstalkservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 14 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_14(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_elasticloadbalancing_elasticloadbalancing_v7::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticloadbalancingv2_elasticloadbalancing_v10::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elasticsearchservice_amazonelasticsearchservice2015::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elbproxy_elbproxy::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_elementalinference_elementalinference::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_emr_elasticmapreduce::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_emrcontainers_awschicagowebservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_emrserverless_awstoledowebservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_endusermessaging_endusermessaging::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_entityresolution_awsveniceservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_eventbridge_awsevents::EmulatedService::new(engine.clone()));
    router
}

/// Register services 15 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_15(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_eventbridgev2_awseventsv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_evs_amazonelasticvmwareservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_firehose_firehose_20150804::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_fis_faultinjectionsimulator::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_fms_awsfms_20180101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_forecast_amazonforecast::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_forecastquery_amazonforecastruntime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_frauddetector_awshawksnestservicefacade::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_freetier_awsfreetierservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_fsx_awssimbaapiservice_v20180301::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_gamelift_gamelift::EmulatedService::new(engine.clone()));
    router
}

/// Register services 16 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_16(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_gameliftstreams_gameliftstreams::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_geomaps_mapsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_geoplaces_placesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_georoutes_routesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_glacier_glacier::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_globalaccelerator_globalaccelerator_v20180706::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_glue_awsglue::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_grafana_awsgrafanacontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_greengrass_greengrass::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_greengrassv2_greengrassv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_groundstation_groundstation::EmulatedService::new(engine.clone()));
    router
}

/// Register services 17 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_17(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_guardduty_guarddutyapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_health_awshealth_20160804::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_healthlake_healthlake::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iam_awsidentitymanagementv20100508::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iamtoolbox_authrequestservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_identitystore_awsidentitystore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_imagebuilder_imagebuilder::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_inspector_inspectorservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_inspector2_inspector2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_inspectorscan_inspectorscan::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_interconnect_interconnect::EmulatedService::new(engine.clone()));
    router
}

/// Register services 18 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_18(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_internetmonitor_internetmonitor20210603::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_invoicing_invoicing::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iot_awsiotservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotdataplane_iotmoonrakerservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotdeviceadvisor_iotsenateservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotfleetwise_iotautobahncontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotjobsdataplane_iotlaserthingjobmanagerexternalservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotmanagedintegrations_iotmanagedintegrations::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotsecuretunneling_iotsecuredtunneling::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotsitewise_awsiotsitewise::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotthingsgraph_iotthingsgraphfrontendservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 19 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_19(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_iottwinmaker_awsiottwinmaker::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_iotwireless_iotwireless::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ivs_amazoninteractivevideoservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ivschat_amazoninteractivevideoservicechat::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ivsrealtime_amazoninteractivevideoservicerealtime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kafka_kafka::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kafkaconnect_kafkaconnect::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kendra_awskendrafrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kendraranking_awskendrarerankingfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_keyspaces_keyspacesservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_keyspacesstreams_keyspacesstreams::EmulatedService::new(engine.clone()));
    router
}

/// Register services 20 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_20(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_kinesis_kinesis_20131202::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisanalytics_kinesisanalytics_20150814::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisanalyticsv2_kinesisanalytics_20180523::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideo_kinesisvideo_20170930::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideoarchivedmedia_awsacuityreader::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideomedia_awsacuityinletservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideosignaling_awsacuitysignalingservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kinesisvideowebrtcstorage_awsacuityroutingservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_kms_trentservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lakeformation_awslakeformation::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lambda_awsgirapiservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 21 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_21(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_lambdacore_lambdacoreapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lambdamicrovms_lambdamicrovms::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lambdaweb_lambdaweb::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_launchwizard_launchwizard::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lexmodelbuildingservice_awsdeepsensemodelbuildingservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lexmodelsv2_lexmodelbuildingservicev2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lexruntimeservice_awsdeepsenseruntimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lexruntimev2_awsdeepsenseruntimeserviceapi2_0::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_licensemanager_awslicensemanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_licensemanagerlinuxsubscriptions_licensemanagerlinuxsubscriptions::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_licensemanagerusersubscriptions_licensemanagerusersubscriptions::EmulatedService::new(engine.clone()));
    router
}

/// Register services 22 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_22(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_lightsail_lightsail_20161128::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_location_locationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_lookoutequipment_awslookoutequipmentfrontendservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_m2_awssupernovacontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_machinelearning_amazonml_20141212::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_macie2_macie2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mailmanager_mailmanagersvc::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_managedblockchain_taigawebservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_managedblockchainquery_tietonchainqueryservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplaceagreement_awsmpcommerceservice_v20200301::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplacecatalog_awsmpseymour::EmulatedService::new(engine.clone()));
    router
}

/// Register services 23 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_23(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_marketplacecommerceanalytics_marketplacecommerceanalytics20150701::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplacedeployment_awsmpdeploymentparametersservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplacediscovery_awsmarketplacediscovery::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplaceentitlementservice_awsmpentitlementservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplacemetering_awsmpmeteringservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_marketplacereporting_awsmarketplacereporting::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediaconnect_mediaconnect::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediaconvert_mediaconvert::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_medialive_medialive::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediapackage_mediapackage::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediapackagev2_mediapackagev2::EmulatedService::new(engine.clone()));
    router
}

/// Register services 24 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_24(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_mediapackagevod_mediapackagevod::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediastore_mediastore_20170901::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediastoredata_mediastoreobject_20170901::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mediatailor_mediatailor::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_medicalimaging_ahigatewayservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_memorydb_amazonmemorydb::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mgn_applicationmigrationservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_migrationhub_awsmigrationhub::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_migrationhubconfig_awsmigrationhubmultiaccountservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_migrationhuborchestrator_awsmigrationhuborchestrator::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_migrationhubrefactorspaces_refactorspaces::EmulatedService::new(engine.clone()));
    router
}

/// Register services 25 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_25(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_migrationhubstrategy_awsmigrationhubstrategyrecommendation::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mpa_awsfluffycoreservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mq_mq::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mturk_mturkrequesterservicev20170117::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mwaa_amazonmwaa::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_mwaaserverless_amazonmwaaserverless::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_neptune_amazonrdsv19::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_neptunedata_amazonneptunedataplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_neptunegraph_amazonneptunegraph::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_networkfirewall_networkfirewall_20201112::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_networkflowmonitor_networkflowmonitor::EmulatedService::new(engine.clone()));
    router
}

/// Register services 26 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_26(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_networkmanager_networkmanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_networkmonitor_networkmonitor::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_networksecuritymanager_piccolocustomerapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_notifications_notifications::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_notificationscontacts_notificationscontacts::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_novaact_amazonnovaagentsdataplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_oam_oamservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_observabilityadmin_observabilityadmin::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_odb_odb::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_omics_omics::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_opensearch_amazonopensearchservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 27 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_27(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_opensearchmemory_opensearchmemory::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_opensearchserverless_opensearchserverless::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_organizations_awsorganizationsv20161128::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_osis_amazonopensearchingestionservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_outposts_outpostsolafservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_partnercentralaccount_partnercentralaccount::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_partnercentralbenefits_partnercentralbenefitsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_partnercentralchannel_partnercentralchannel::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_partnercentralmarketing_awspartnerdemandgenerator::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_partnercentralrevenuemeasurement_partnercentralrevenuemeasurement::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_partnercentralselling_awspartnercentralselling::EmulatedService::new(engine.clone()));
    router
}

/// Register services 28 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_28(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_paymentcryptography_paymentcryptographycontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_paymentcryptographydata_paymentcryptographydataplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_payments_awspayments::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pcaconnectorad_pcaconnectorad::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pcaconnectorscep_pcaconnectorscep::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pcs_awsparallelcomputingservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_personalize_amazonpersonalize::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_personalizeevents_amazonpersonalizeevents::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_personalizeruntime_amazonpersonalizeruntime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pi_performanceinsightsv20180227::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pinpoint_pinpoint::EmulatedService::new(engine.clone()));
    router
}

/// Register services 29 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_29(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_pinpointemail_amazonpinpointemailservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pinpointsmsvoice_pinpointsmsvoice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pinpointsmsvoicev2_pinpointsmsvoicev2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pipes_pipes::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_polly_parrot_v1::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pricing_awspricelistservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_pricingplanmanager_awspricingplanmanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_proton_awsproton20200720::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_qapps_qappsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_qbusiness_expertq::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_qconnect_wisdomservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 30 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_30(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_quicksight_quicksight_20180401::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ram_amazonresourcesharing::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rbin_amazonrecyclebin::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rds_amazonrdsv19::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rdsdata_rdsdataservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_redshift_redshiftserviceversion20121201::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_redshiftdata_redshiftdata::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_redshiftserverless_redshiftserverless::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rekognition_rekognitionservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_repostspace_repostspace::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_resiliencehub_awsresiliencehub::EmulatedService::new(engine.clone()));
    router
}

/// Register services 31 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_31(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_resiliencehubv2_ngrhservicecore::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_resourceexplorer2_resourceexplorer::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_resourcegroups_ardi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_resourcegroupstaggingapi_resourcegroupstaggingapi_20170126::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rolesanywhere_rolesanywhere::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53_awsdnsv20130401::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53domains_route53domains_v20140515::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53globalresolver_ec2dnsglobalresolvercustomerapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53profiles_route53profiles::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53recoverycluster_togglecustomerapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53recoverycontrolconfig_route53recoverycontrolconfig::EmulatedService::new(engine.clone()));
    router
}

/// Register services 32 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_32(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_route53recoveryreadiness_route53recoveryreadiness::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_route53resolver_route53resolver::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rtbfabric_rtbfabric::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_rum_rum::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_s3_amazons3::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_s3control_awss3controlservicev20180820::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_s3files_s3files::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_s3outposts_s3outposts::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_s3tables_s3tablebuckets::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_s3vectors_s3vectors::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemaker_sagemaker::EmulatedService::new(engine.clone()));
    router
}

/// Register services 33 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_33(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_sagemakera2iruntime_amazonsagemakera2iruntime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakeredge_amazonsagemakeredge::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakerfeaturestoreruntime_amazonsagemakerfeaturestoreruntime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakergeospatial_sagemakergeospatial::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakerjobruntime_agenticrftruntimeservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakermetrics_sagemakermetricsservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakerruntime_amazonsagemakerruntime::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakerruntimehttp2_amazonsagemakerruntimehttp2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sagemakertrainingsessionruntime_legorouterservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_savingsplans_awssavingsplan::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_scheduler_awschronosservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 34 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_34(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_schemas_schemas::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_secretsmanager_secretsmanager::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_securityagent_securityagent::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_securityhub_securityhubapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_securityir_securityincidentresponse::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_securitylake_securitylake::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_serverlessapplicationrepository_serverlessapplicationrepository::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_servicecatalog_aws242servicecatalogservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_servicecatalogappregistry_aws242appregistry::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_servicediscovery_route53autonaming_v20170314::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_servicequotas_servicequotasv20190624::EmulatedService::new(engine.clone()));
    router
}

/// Register services 35 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_35(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_ses_simpleemailservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sesv2_simpleemailservice_v2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sfn_awsstepfunctions::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_shield_awsshield_20160616::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_signer_wallabyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_signerdata_signerdataplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_signin_signin::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_simpledbv2_simpledbv2::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_snowball_awsiesnowballjobmanagementservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_snowdevicemanagement_snowdevicemanagement::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sns_amazonsimplenotificationservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 36 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_36(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_socialmessaging_socialmessaging::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sqs_amazonsqs::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssm_amazonssm::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssmcontacts_ssmcontacts::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssmguiconnect_ssmguiconnect::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssmincidents_ssmincidents::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssmquicksetup_quicksetup::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssmsap_ssmsap::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sso_swbportalservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssoadmin_swbexternalservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_ssooidc_awsssooidcservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 37 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_37(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_storagegateway_storagegateway_20130630::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sts_awssecuritytokenservicev20110615::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_supplychain_galaxypublicapigateway::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_support_awssupport_20130415::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_supportapp_supportapp::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_supportauthz_supportauthz::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_sustainability_awssustainabilityapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_swf_simpleworkflowservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_synthetics_synthetics::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_taxsettings_taxsettings::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_textract_textract::EmulatedService::new(engine.clone()));
    router
}

/// Register services 38 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_38(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_timestreaminfluxdb_amazontimestreaminfluxdb::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_timestreamquery_timestream_20181101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_timestreamwrite_timestream_20181101::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_transcribe_transcribe::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_transcribestreaming_transcribe::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_transfer_transferservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_translate_awsshinefrontendservice_20170701::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_trustedadvisor_trustedadvisor::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_uxc_awsaccountuxsetting::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_verifiedpermissions_verifiedpermissions::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_voiceid_voiceid::EmulatedService::new(engine.clone()));
    router
}

/// Register services 39 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_39(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_vpclattice_mercurycontrolplane::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_waf_awswaf_20150824::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_wafregional_awswaf_regional_20161128::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_wafv2_awswaf_20190729::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_wellarchitected_wellarchitectedapiservicelambda::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_wickr_wickradminapi::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_wisdom_wisdomservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_workdocs_awsgorillaboyservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_workmail_workmailservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_workmailmessageflow_giraffemessageintransitservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_workspaces_workspacesservice::EmulatedService::new(engine.clone()));
    router
}

/// Register services 40 of this workspace on `router`.
///
/// `#[inline(never)]` is LOAD-BEARING: inlined back into `build_router` the frames
/// would coalesce again and the stack saving would be lost.
#[inline(never)]
fn mount_chunk_40(
    mut router: MultiServiceRouter,
    engine: &Arc<InMemoryEngine>,
) -> MultiServiceRouter {
    router.register_service(service_com_amazonaws_workspacesinstances_eucmifrontendapiservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_workspacesthinclient_thinclient::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_workspacesweb_awserminecontrolplaneservice::EmulatedService::new(engine.clone()));
    router.register_service(service_com_amazonaws_xray_awsxray::EmulatedService::new(engine.clone()));
    router
}

