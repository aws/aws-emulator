Issue-Severity: HIGH
Issue-Corpus: simparity-behavioral-aws-v1
Issue-Corpus-Kind: behavioral
Issue-Trace-Ids: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one
Issue-Key: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one|_domain_names#20|KeyError|for item in page["items"]:
Issue-Commit: 3a4cb0b889f06dace04686cde54ab1ad3ca2e94d

# apigateway GetDomainNames omits the items member entirely when the account holds no custom domain, so a client reading response['items'] raises KeyError

> _Note: this issue was identified against the code base at commit `3a4cb0b889f06dace04686cde54ab1ad3ca2e94d`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
GetDomainNames

## Description
1 net-new root cause.

Cause 1 of 1: GetDomainNames answers an empty collection with a 200 body that carries NO collection member at all, instead of a present-but-empty list. Failing check: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one.

AWS: Smithy (services/service-com-amazonaws-apigateway-backplanecontrolservice/smithy-model/com-amazonaws-apigateway-backplanecontrolservice.json) models GetDomainNames -> DomainNames with members `items` (ListOfDomainName, jsonName "item") and `position`, http GET /domainnames, success 200, and `paginated {inputToken: position, outputToken: position, items: items, pageSize: limit}`. The model does not state member presence, so for the empty case this is recorded behavior only: a 200 listing from AWS always carries the collection member, serialized on the wire under the JSON key `item` and surfaced by the SDK as `items`; an account with no custom domains answers an empty list, not an absent member. The replayed check relies on exactly that — it walks the get_domain_names paginator and reads `page["items"]` on every page unconditionally, including the poll it runs immediately after deleting the only domain it created (i.e. with the collection empty) and again at the end of the scenario.

Emulator: that poll gets a 200 whose body has no `item` key, so the parsed response has no `items` member and the check dies with `KeyError: 'items'` at `for item in page["items"]`. The cause is the shared paging helper: `Page::reported_items` returns `None` whenever the underlying sequence is empty ("absent when the collection being listed holds no element at all, so a listing of nothing is not a present-but-empty page"), and the handler assigns that straight into `GetDomainNamesOutput.items`, which the serializer then drops as an absent optional. The helper is shared by every position/limit listing of this crate, so the same absent-member answer is served by the service's other list operations on an empty collection.

Why: a client that does the idiomatic thing — iterate `response['items']`, or let a paginator's result key `items` accumulate — crashes (KeyError in botocore-based clients, a missing/None list in typed SDKs) on an account that simply owns no custom domain yet, instead of iterating zero elements. Any "is this resource gone?"/"is this resource listed?" poll, which is how callers confirm a delete, is unwritable against the emulator. Here it also masks the rest of the scenario: the PENDING_VALIDATION refusal (BadRequestException, message "Certificate is not in an ISSUED state, and cannot be used") and the custom-domain TLS behavior were never exercised at all, so this one divergence hides the whole remainder of the check.

Fix: the GetDomainNames response path — specifically the shared paging helper in this service's state that the handler feeds its collection member from; a 200 listing must report a present collection member (empty list when there is nothing to report) rather than omitting it, for the empty-collection case as well as the empty-window case it already handles.

Traces: acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one. Code: services/service-com-amazonaws-apigateway-backplanecontrolservice/src/ops/get_domain_names.rs lines 166-178 (`items: page.reported_items(items)`) and services/service-com-amazonaws-apigateway-backplanecontrolservice/src/state.rs lines 2773-2787 (`fn reported_items` returning `None` when `sequence_empty`). No open issue covers this; the nearest apigateway custom-domain issues (20261111T014037Z-apigateway-custom-domain-representation-diverges-on-domain-arn.md) are about field VALUES of a listed domain, not the absence of the collection member.

## Evidence
Expected (AWS): GET /domainnames with no custom domains -> 200 with the collection member present and empty; the SDK-parsed response contains `items` (wire key `item`), so `page["items"]` yields an empty list and the paginator's `items` result key resolves.

Actual (emulator): GET /domainnames with no custom domains -> 200 whose body omits `item` entirely, so the parsed response has no `items` key.

Replay output for acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one:
  File "check_acm__apigateway_regional_custom_domain_refuses_a_pending_certificate_then_serves_the_issued_one.py", line 303, in <lambda>
    lambda: _domain_names(apigw),
  File "check_acm__apigateway_regional_custom_domain_refuses_a_pending_certificate_then_serves_the_issued_one.py", line 224, in _domain_names
    for item in page["items"]
  KeyError: 'items'

Check code that depends on the member being present:
  def _domain_names(apigw):
      names = set()
      for page in apigw.get_paginator("get_domain_names").paginate():
          for item in page["items"]:
              names.add(item["domainName"])
      return names

Emulator source responsible:
  src/ops/get_domain_names.rs: `GetDomainNamesOutput { items: page.reported_items(items), position: page.next_position.clone() }`
  src/state.rs: `pub fn reported_items<Element>(&self, items: Vec<Element>) -> Option<Vec<Element>> { if self.sequence_empty { None } else { Some(items) } }`

Note on values: the domain name, certificate ARNs and ids in the recording are per-run values and are not part of the requirement; the requirement is that the 200 body of a listing always carries its collection member.

## Steps to Reproduce
1. Point an AWS SDK/boto3 apigateway client at the emulator in an account/region that holds no custom domain names (or create one with CreateDomainName and then DeleteDomainName so the collection is empty again).
2. Call GetDomainNames (e.g. `apigw.get_paginator("get_domain_names").paginate()`), and read `page["items"]` for each page — as the replayed check's `_domain_names()` helper does.
3. Observed: the 200 response body has no `item` key, the parsed response has no `items` member, and the read raises `KeyError: 'items'`. Expected: `items == []`.
4. Full scenario: replay check acm__apigateway.regional-custom-domain-refuses-a-pending-certificate-then-serves-the-issued-one; it imports an ISSUED certificate, creates and deletes the REGIONAL custom domain, then polls GetDomainNames for the domain's disappearance and errors there before ever reaching the PENDING_VALIDATION refusal assertions.
