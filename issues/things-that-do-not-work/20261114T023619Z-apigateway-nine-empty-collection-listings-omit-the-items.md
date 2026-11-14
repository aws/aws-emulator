Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: apigateway.authorizer-token-missing-identitysource-badrequest apigateway.authorizer-ttl-exceeds-max-rejected apigateway.create-authorizer-invalid-type-rejected apigateway.concurrent-create-domain-name-access-association-duplicate-race apigateway.concurrent-domain-name-access-association-duplicate apigateway.domain-access-association-concurrent-reject-delete apigateway.reject-domain-name-access-association-after-accept-conflict apigateway.reject-domain-name-access-association-invalid-fails apigateway.concurrent-create-usage-plan-key-nonexistent-key-rejected apigateway.concurrent-delete-usage-plan-key-double-delete-rejected apigateway.create-usage-plan-key-invalid-type-rejected apigateway.create-usage-plan-key-wrong-type-fails apigateway.create-documentation-part-duplicate-location-fails apigateway.import-documentation-parts-bulk-create apigateway.import-documentation-parts-malformed-body-rejected apigateway.create-vpc-link-missing-target-arns-rejected apigateway.vpc-link-invalid-target-arn-rejected apigateway.e2e-custom-domain-base-path-routing apigateway.get-usage-plans-lists-created apigateway.update-usage-plan-and-list-plans apigateway.import-api-keys-bulk-create apigateway.import-api-keys-bulk-server-validation apigateway.import-api-keys-rollback-on-failonwarnings-rejected apigateway.op-contract-create-api-key apigateway.op-contract-delete-request-validator apigateway.update-request-validator-nonexistent-error
Issue-Shape-Id: com.amazonaws.apigateway#BackplaneControlService
Issue-Key: apigateway.authorizer-token-missing-identitysource-badrequest|run#9|KeyError|authorizers = ag.get_authorizers(restApiId=api_id)["items"]
Issue-Key: apigateway.authorizer-ttl-exceeds-max-rejected|run#13|KeyError|names = {a["name"] for a in apigw.get_authorizers(restApiId=api_id)["items"]}
Issue-Key: apigateway.create-authorizer-invalid-type-rejected|run#9|KeyError|authorizers = apigw.get_authorizers(restApiId=api_id)["items"]
Issue-Key: apigateway.concurrent-create-domain-name-access-association-duplicate-race|run#5|KeyError|assert isinstance(control["items"], list), (
Issue-Key: apigateway.concurrent-domain-name-access-association-duplicate|_poll_association_gone#31|KeyError|items = apigw.get_domain_name_access_associations(resourceOwner="SELF")["items"]
Issue-Key: apigateway.domain-access-association-concurrent-reject-delete|run#23|KeyError|assocs_after = ag.get_domain_name_access_associations()["items"]
Issue-Key: apigateway.reject-domain-name-access-association-after-accept-conflict|run#26|KeyError|arns_after = {i["domainNameAccessAssociationArn"] for i in apigw.get_domain_name_access_associations()["items"]}
Issue-Key: apigateway.reject-domain-name-access-association-invalid-fails|run#12|KeyError|associations = apigw.get_domain_name_access_associations()["items"]
Issue-Key: apigateway.concurrent-create-usage-plan-key-nonexistent-key-rejected|run#12|KeyError|remaining = apigw.get_usage_plan_keys(usagePlanId=plan_id)["items"]
Issue-Key: apigateway.concurrent-delete-usage-plan-key-double-delete-rejected|run#17|KeyError|remaining = apigw.get_usage_plan_keys(usagePlanId=plan_id)["items"]
Issue-Key: apigateway.create-usage-plan-key-invalid-type-rejected|run#11|KeyError|items = apigw.get_usage_plan_keys(usagePlanId=plan_id)["items"]
Issue-Key: apigateway.create-usage-plan-key-wrong-type-fails|run#12|KeyError|items = ag.get_usage_plan_keys(usagePlanId=plan_id)["items"]
Issue-Key: apigateway.create-documentation-part-duplicate-location-fails|run#13|KeyError|parts_after_delete = apigw.get_documentation_parts(restApiId=api_id)["items"]
Issue-Key: apigateway.import-documentation-parts-bulk-create|run#4|KeyError|baseline = apigw.get_documentation_parts(restApiId=api_id)["items"]
Issue-Key: apigateway.import-documentation-parts-malformed-body-rejected|run#4|KeyError|baseline = apigw.get_documentation_parts(restApiId=api_id)["items"]
Issue-Key: apigateway.create-vpc-link-missing-target-arns-rejected|run#6|KeyError|links = apigw.get_vpc_links()["items"]
Issue-Key: apigateway.vpc-link-invalid-target-arn-rejected|run#7|KeyError|listed_names = {v["name"] for v in apigw.get_vpc_links()["items"]}
Issue-Key: apigateway.e2e-custom-domain-base-path-routing|run#38|KeyError|remaining = apigw.get_base_path_mappings(domainName=domain_name)["items"]
Issue-Key: apigateway.get-usage-plans-lists-created|_find_plan#16|KeyError|for item in resp["items"]:
Issue-Key: apigateway.update-usage-plan-and-list-plans|_all_usage_plans#17|KeyError|return [p for page in apigw.get_paginator("get_usage_plans").paginate() for p in page["items"]]
Issue-Key: apigateway.import-api-keys-bulk-create|run#28|KeyError|still_listed = apigw.get_api_keys(nameQuery=name1)["items"]
Issue-Key: apigateway.import-api-keys-bulk-server-validation|run#16|KeyError|listed = apigw.get_api_keys(nameQuery=invalid_name)["items"]
Issue-Key: apigateway.import-api-keys-rollback-on-failonwarnings-rejected|run#13|KeyError|invalid_listed = apigw.get_api_keys(nameQuery=invalid_name)["items"]
Issue-Key: apigateway.op-contract-create-api-key|run#16|KeyError|listed = apigw.get_api_keys(nameQuery=bad_name)["items"]
Issue-Key: apigateway.op-contract-delete-request-validator|run#17|KeyError|remaining = apigw.get_request_validators(restApiId=api_id)["items"]
Issue-Key: apigateway.update-request-validator-nonexistent-error|run#16|KeyError|remaining_ids = {v["id"] for v in apigw.get_request_validators(restApiId=api_id)["items"]}
Issue-Commit: 0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6

# apigateway: nine empty-collection listings omit the items member entirely, so every "is it listed / is it gone?" read raises KeyError (9 causes)

> _Note: this issue was identified against the code base at commit `0bdb628a6eeeae829cbfa42cf4e1c8a43263b6d6`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetAuthorizers

## Description
9 net-new root causes, all of the apigateway control plane, all of the same family: a 200 listing of an EMPTY collection is served with the collection member ABSENT instead of present-and-empty. Each cause below is a distinct operation with its own failing checks; the shared paging helper they all feed is the common component, but each operation's response path has to report a present member before its checks pass.

Related, NOT covering: open issue 20261114T013849Z-acm-apigateway-apigateway-getdomainnames-omits-the-items-member-entirely-when.md reports exactly this defect for GetDomainNames ONLY (one operation, one check). The nine operations below are not named by it and none of their checks is marked owned by it. A fix to the shared helper may close all ten at once; a fix scoped to get_domain_names.rs will not.

Common AWS statement (applies to every cause): Smithy services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json models each of these operations with an output member `items` (a list, jsonName "item") plus `position`, HTTP GET, success 200, and several of them additionally carry `paginated {inputToken: position, outputToken: position, items: items, pageSize: limit}`. The model does not state member presence, so for the empty case this is recorded behavior only: an AWS 200 listing always carries its collection member on the wire under the JSON key `item`, surfaced by SDKs as `items`; a collection holding nothing answers an empty list, never an absent member. Common emulator statement: the handler assigns `page.reported_items(items)` into the output's `items`, and that helper returns `None` whenever the underlying sequence is empty, after which the serializer drops `items` as an absent optional. Common client consequence: the idiomatic read — `response["items"]`, or letting a paginator accumulate its `items` result key — raises `KeyError: 'items'` in botocore clients and yields a missing/None list in typed SDKs, exactly on the account/API that owns nothing yet. Every delete-confirmation and no-phantom-state poll is unwritable, and in each check below the KeyError also ABORTS the scenario before its real subject (the refusal code/message under test) is ever asserted. Common fix location: each operation's response path in services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/<op>.rs plus the shared `Page::reported_items` in src/state.rs — a 200 listing must report a present collection member (empty list when there is nothing to report), not omit it.

Cause 1 of 9: GetAuthorizers (GET /restapis/{restApiId}/authorizers) omits `items` for an API with no authorizers. Failing checks: apigateway.authorizer-token-missing-identitysource-badrequest, apigateway.authorizer-ttl-exceeds-max-rejected, apigateway.create-authorizer-invalid-type-rejected. Each creates a fresh REST API, has CreateAuthorizer refused, then reads get_authorizers(restApiId=...)["items"] to prove nothing was created, and dies there.

Cause 2 of 9: GetDomainNameAccessAssociations (GET /domainnameaccessassociations) omits `items` when the account holds no access association — both with and without the `resourceOwner` query. Failing checks: apigateway.concurrent-create-domain-name-access-association-duplicate-race, apigateway.concurrent-domain-name-access-association-duplicate, apigateway.domain-access-association-concurrent-reject-delete, apigateway.reject-domain-name-access-association-after-accept-conflict, apigateway.reject-domain-name-access-association-invalid-fails. One of them uses the read as an explicit POSITIVE CONTROL ("the read side of this same API family is not a blanket-failing stub") before racing the create, so the absent member is the first thing it hits.

Cause 3 of 9: GetUsagePlanKeys (GET /usageplans/{usagePlanId}/keys) omits `items` for a usage plan with no keys. Failing checks: apigateway.concurrent-create-usage-plan-key-nonexistent-key-rejected, apigateway.concurrent-delete-usage-plan-key-double-delete-rejected, apigateway.create-usage-plan-key-invalid-type-rejected, apigateway.create-usage-plan-key-wrong-type-fails.

Cause 4 of 9: GetDocumentationParts (GET /restapis/{restApiId}/documentation/parts) omits `items` for an API with no documentation parts. Failing checks: apigateway.create-documentation-part-duplicate-location-fails, apigateway.import-documentation-parts-bulk-create, apigateway.import-documentation-parts-malformed-body-rejected. Two of them take the empty listing as a BASELINE on the very first call of the scenario.

Cause 5 of 9: GetVpcLinks (GET /vpclinks) omits `items` when the account holds no VPC link. Failing checks: apigateway.create-vpc-link-missing-target-arns-rejected, apigateway.vpc-link-invalid-target-arn-rejected.

Cause 6 of 9: GetBasePathMappings (GET /domainnames/{domainName}/basepathmappings) omits `items` for a custom domain whose mappings have all been deleted. Failing check: apigateway.e2e-custom-domain-base-path-routing, which reads it to prove no dangling base path mapping is left behind.

Cause 7 of 9: GetUsagePlans (GET /usageplans) omits `items` when the account holds no usage plan, including through the get_usage_plans paginator. Failing checks: apigateway.get-usage-plans-lists-created, apigateway.update-usage-plan-and-list-plans.

Cause 8 of 9: GetApiKeys (GET /apikeys) omits `items` when the `name` (nameQuery) filter selects nothing — the normal answer for "this key was NOT imported/created". Failing checks: apigateway.import-api-keys-bulk-create, apigateway.import-api-keys-bulk-server-validation, apigateway.import-api-keys-rollback-on-failonwarnings-rejected, apigateway.op-contract-create-api-key. Note this is the FILTERED-empty case as well as the collection-empty case: a filter that matches nothing must still answer `items: []`.

Cause 9 of 9: GetRequestValidators (GET /restapis/{restApiId}/requestvalidators) omits `items` for an API with no request validators left. Failing checks: apigateway.op-contract-delete-request-validator, apigateway.update-request-validator-nonexistent-error.

## Evidence
Expected (AWS), for every one of the nine operations: a 200 whose body carries the collection member (wire key `item`), so the SDK-parsed response contains `items` and `response["items"]` is `[]` when nothing matches; a paginator's `items` result key resolves to an empty page.

Actual (emulator): a 200 whose body omits `item` entirely, so the parsed response has no `items` key at all and the read raises `KeyError: 'items'`.

Replay output, one line per cause (the frame is the check's own read; the exception is identical in all 26):
  GetAuthorizers — check_apigateway_authorizer_token_missing_identitysource_badrequest.py:57 `authorizers = ag.get_authorizers(restApiId=api_id)["items"]` -> KeyError: 'items'
  GetAuthorizers — check_apigateway_authorizer_ttl_exceeds_max_rejected.py:47 `names = {a["name"] for a in apigw.get_authorizers(restApiId=api_id)["items"]}` -> KeyError: 'items'
  GetAuthorizers — check_apigateway_create_authorizer_invalid_type_rejected.py:40 `authorizers = apigw.get_authorizers(restApiId=api_id)["items"]` -> KeyError: 'items'
  GetDomainNameAccessAssociations — check_apigateway_concurrent_create_domain_name_access_association_duplicate_race.py:28 `assert isinstance(control["items"], list)` -> KeyError: 'items'
  GetDomainNameAccessAssociations — check_apigateway_concurrent_domain_name_access_association_duplicate.py:200 `items = apigw.get_domain_name_access_associations(resourceOwner="SELF")["items"]` -> KeyError: 'items'
  GetDomainNameAccessAssociations — check_apigateway_domain_access_association_concurrent_reject_delete.py:272 `assocs_after = ag.get_domain_name_access_associations()["items"]` -> KeyError: 'items'
  GetDomainNameAccessAssociations — check_apigateway_reject_domain_name_access_association_after_accept_conflict.py:80 `... for i in apigw.get_domain_name_access_associations()["items"]` -> KeyError: 'items'
  GetDomainNameAccessAssociations — check_apigateway_reject_domain_name_access_association_invalid_fails.py:51 `associations = apigw.get_domain_name_access_associations()["items"]` -> KeyError: 'items'
  GetUsagePlanKeys — check_apigateway_concurrent_create_usage_plan_key_nonexistent_key_rejected.py:51 `remaining = apigw.get_usage_plan_keys(usagePlanId=plan_id)["items"]` -> KeyError: 'items'
  GetUsagePlanKeys — check_apigateway_concurrent_delete_usage_plan_key_double_delete_rejected.py:56 same read -> KeyError: 'items'
  GetUsagePlanKeys — check_apigateway_create_usage_plan_key_invalid_type_rejected.py:35 `items = apigw.get_usage_plan_keys(usagePlanId=plan_id)["items"]` -> KeyError: 'items'
  GetUsagePlanKeys — check_apigateway_create_usage_plan_key_wrong_type_fails.py:49 `items = ag.get_usage_plan_keys(usagePlanId=plan_id)["items"]` -> KeyError: 'items'
  GetDocumentationParts — check_apigateway_create_documentation_part_duplicate_location_fails.py:49 `parts_after_delete = apigw.get_documentation_parts(restApiId=api_id)["items"]` -> KeyError: 'items'
  GetDocumentationParts — check_apigateway_import_documentation_parts_bulk_create.py:20 `baseline = apigw.get_documentation_parts(restApiId=api_id)["items"]` -> KeyError: 'items'
  GetDocumentationParts — check_apigateway_import_documentation_parts_malformed_body_rejected.py:21 same read -> KeyError: 'items'
  GetVpcLinks — check_apigateway_create_vpc_link_missing_target_arns_rejected.py:28 `links = apigw.get_vpc_links()["items"]` -> KeyError: 'items'
  GetVpcLinks — check_apigateway_vpc_link_invalid_target_arn_rejected.py:31 `listed_names = {v["name"] for v in apigw.get_vpc_links()["items"]}` -> KeyError: 'items'
  GetBasePathMappings — check_apigateway_e2e_custom_domain_base_path_routing.py:99 `remaining = apigw.get_base_path_mappings(domainName=domain_name)["items"]` -> KeyError: 'items'
  GetUsagePlans — check_apigateway_get_usage_plans_lists_created.py:46 `for item in resp["items"]:` -> KeyError: 'items'
  GetUsagePlans — check_apigateway_update_usage_plan_and_list_plans.py:58 `[p for page in apigw.get_paginator("get_usage_plans").paginate() for p in page["items"]]` -> KeyError: 'items'
  GetApiKeys — check_apigateway_import_api_keys_bulk_create.py:75 `still_listed = apigw.get_api_keys(nameQuery=name1)["items"]` -> KeyError: 'items'
  GetApiKeys — check_apigateway_import_api_keys_bulk_server_validation.py:44 `listed = apigw.get_api_keys(nameQuery=invalid_name)["items"]` -> KeyError: 'items'
  GetApiKeys — check_apigateway_import_api_keys_rollback_on_failonwarnings_rejected.py:56 `invalid_listed = apigw.get_api_keys(nameQuery=invalid_name)["items"]` -> KeyError: 'items'
  GetApiKeys — check_apigateway_op_contract_create_api_key.py:50 `listed = apigw.get_api_keys(nameQuery=bad_name)["items"]` -> KeyError: 'items'
  GetRequestValidators — check_apigateway_op_contract_delete_request_validator.py:56 `remaining = apigw.get_request_validators(restApiId=api_id)["items"]` -> KeyError: 'items'
  GetRequestValidators — check_apigateway_update_request_validator_nonexistent_error.py:70 `remaining_ids = {v["id"] for v in apigw.get_request_validators(restApiId=api_id)["items"]}` -> KeyError: 'items'

Emulator source responsible (one call site per cause, all feeding one helper):
  src/ops/get_authorizers.rs line 197: `items: page.reported_items(items)` inside `Ok(Paged::new(GetAuthorizersOutput { .. }))`; the same `page.reported_items(items)` assignment appears in get_api_keys.rs, get_usage_plans.rs, get_usage_plan_keys.rs, get_documentation_parts.rs, get_request_validators.rs, get_vpc_links.rs, get_base_path_mappings.rs and get_domain_name_access_associations.rs.
  src/state.rs (around lines 2773-2787): `pub fn reported_items<Element>(&self, items: Vec<Element>) -> Option<Vec<Element>> { if self.sequence_empty { None } else { Some(items) } }` — the `None` is what the serializer then drops.

Note on values: the api ids, plan ids, key names, domain names and VPC endpoint ids in these recordings are per-run values and are not part of the requirement. The requirement is structural: the 200 body of a listing always carries its collection member, including when the collection — or the filtered selection — is empty.

## Steps to Reproduce
1. Point a boto3/AWS-SDK `apigateway` client at the emulator.
2. For each operation, read the empty collection:
   - `apigw.create_rest_api(name=..., endpointConfiguration={"types":["REGIONAL"]})`, then `apigw.get_authorizers(restApiId=api_id)["items"]`, `apigw.get_documentation_parts(restApiId=api_id)["items"]`, `apigw.get_request_validators(restApiId=api_id)["items"]`.
   - `apigw.create_usage_plan(name=...)`, then `apigw.get_usage_plan_keys(usagePlanId=plan_id)["items"]`.
   - With no VPC links / no access associations / no usage plans in the account: `apigw.get_vpc_links()["items"]`, `apigw.get_domain_name_access_associations()["items"]` (and with `resourceOwner="SELF"`), `apigw.get_paginator("get_usage_plans").paginate()` reading `page["items"]`.
   - `apigw.get_api_keys(nameQuery="a-name-no-key-has")["items"]`.
   - Create a REGIONAL custom domain, add then delete its base path mappings, and read `apigw.get_base_path_mappings(domainName=domain)["items"]`.
3. Observed in every case: HTTP 200 whose body has no `item` key, the parsed response has no `items` member, and the read raises `KeyError: 'items'`. Expected in every case: `items == []`.
4. Full scenarios: replay the 26 check ids in trace_ids. Each is refused-then-verify or delete-then-verify, and each aborts at the listing before asserting the refusal code/message it exists to check.
