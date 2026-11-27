Issue-Source: trace-sweep-divergence
Issue-Trace: controlcatalog-catalog-list-controls-aws-config-configrule
Issue-Step: task#4
Issue-Severity: HIGH
Issue-Corpus: simparity-wire-aws-v1
Issue-Corpus-Kind: wire
Issue-Trace-Ids: controlcatalog-catalog-list-controls-aws-config-configrule controlcatalog-catalog-list-controls-aws-securityhub-securitycontrol
Issue-Shape-Id: com.amazonaws.controlcatalog#ControlCatalog
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[*]
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/a6kwsgw65ayxiqg7dhzbz2n7w"].Description
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/a6kwsgw65ayxiqg7dhzbz2n7w"].Name
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/8abhdcvu10syqrdyfgr2300px"].Description
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/8abhdcvu10syqrdyfgr2300px"].Name
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/dfbp3c8jerraflh2g68427p4k"].Description
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/bd9d97ndoue5lss1zjjiecle4"].Description
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/bd9d97ndoue5lss1zjjiecle4"].Name
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/bn8ygh3zsy33drdeojmu09a4z"].Description
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/bn8ygh3zsy33drdeojmu09a4z"].Name
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/3ucly6t3j33t5zhbzy45l619n"].Description
Issue-Key: controlcatalog|ListControls|2xx->2xx|$.Controls[Arn="arn:aws:controlcatalog:::control/3ucly6t3j33t5zhbzy45l619n"].Name
Issue-Commit: abdde85f1d8fe8fb2f53ad2d78f36e36b00e237f

# controlcatalog ListControls serves a divergent AWS::Config::ConfigRule control catalog (7 causes)

> _Note: this issue was identified against the code base at commit `abdde85f1d8fe8fb2f53ad2d78f36e36b00e237f`. If you are an issue fixer working on this, the code base you are currently working in can have drifted, so you might need to trace the issue back to the current code base._

## Operation
ListControls

## Description
ListControls, filtered to the AWS Config implementation type (Filter.Implementations.Types = ["AWS::Config::ConfigRule"]), does not serve AWS's published control catalog. The 7 causes below are all the same component fault — the emulator's Config-rule control catalog is a locally authored data set rather than the catalog AWS publishes — but they fail in two distinct ways: whole catalog records are absent from the served page (cause 1), and individual records are served with re-worded Name/Description text (causes 2-7). All 7 steps answer HTTP 200 with a well-formed ListControlsResponse; only the catalog content diverges.

Smithy (services/service-com-amazonaws-controlcatalog-controlcatalog/smithy-model): ListControls POST /list-controls, output ListControlsResponse.Controls is @required list<ControlSummary>; ControlSummary.Name is @required ("The display name of the control"), ControlSummary.Description is @required ("A description of the control, as it may appear in the console. Describes the functionality of the control"), ControlSummary.Arn is @required with pattern ^arn:(aws(?:[-a-z]*)?):(controlcatalog|controltower):[a-zA-Z0-9-]*::control/[0-9a-zA-Z_\-]+$, and ControlSummary.Aliases carries the human-readable designators. ControlFilter.Implementations.Types is documented as "you can filter for controls implemented as Amazon Web Services Config Rules by specifying AWS::Config::ConfigRule as a type". The model does not enumerate the catalog's contents, so the per-record Name/Description text and the record set itself are recorded behavior only: the requirement is that the operation reproduce AWS's published catalog rows verbatim for the global, account-independent ARNs the fixture names. These control ARNs (arn:aws:controlcatalog:::control/<id>) are AWS-global identifiers, not per-run values.

--- Cause 1 of 7: a filtered ListControls page serves no catalog record at all ---
AWS: for the AWS::Config::ConfigRule implementation filter, a ListControls page is populated with the catalog's Config-rule ControlSummary rows. The recorded page's first element is the ACM RSA key-length control: Arn arn:aws:controlcatalog:::control/1ysrvaplv6a00p99mim170vqy, Aliases ["CONFIG.ACM.DT.1"], Behavior DETECTIVE, Severity HIGH, ParameterRequirementSummary NONE, Implementation {Type: AWS::Config::ConfigRule, Identifier: ACM_CERTIFICATE_RSA_CHECK}, GovernedProviders ["AWS"], GovernedResources ["AWS::CertificateManager::Certificate"], Name "Checks if RSA certificates managed by AWS Certificate Manager (ACM) have a key length of at least '2048' bits", with the matching Description (recorded behavior; the model does not list catalog rows).
Emulator: answers 200 but the response has no member at $.Controls[0] — the Controls list for that page is empty, so the record (and every record that page should carry) is missing. This is the dominant cause in the corpus: the cluster covers 1244 replayed steps of the fixture.
Why: a client enumerating governance controls by implementation type sees a catalog that is partly or wholly empty. Paging to completion yields fewer controls than AWS has, so control-coverage reporting, drift checks and any lookup keyed on a published control ARN silently find nothing rather than failing loudly.
Fix: the ListControls handler's catalog projection/state — the Config-rule control data set the filtered page is built from must contain AWS's published rows, and the page assembler must emit them for this filter instead of an empty list.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#4 (representative; 1244 affected steps, representative members task#4..task#28).

--- Cause 2 of 7: ACM certificate-expiration control Name/Description re-worded ---
AWS: for arn:aws:controlcatalog:::control/a6kwsgw65ayxiqg7dhzbz2n7w the catalog publishes Name "Checks whether ACM Certificates in your account are marked for expiration within the specified number of days" and Description "Checks whether ACM Certificates in your account are marked for expiration within the specified number of days. Certificates provided by ACM are automatically renewed. ACM does not automatically renew certificates that you import." (recorded behavior only).
Emulator: serves Name "Checks if AWS Certificate Manager Certificates in your account are marked for expiration within the specified number of days" and Description "Checks if AWS Certificate Manager Certificates in your account are marked for expiration within the specified number of days. Certificates provided by ACM are automatically renewed. ACM does not automatically renew certificates that you import. The rule is NON_COMPLIANT if your certificates are about to expire." — the leading clause is re-worded ("whether ACM" -> "if AWS Certificate Manager") and an extra NON_COMPLIANT sentence is appended.
Why: Name/Description are the strings a console or report renders for the control. A client that matches, de-duplicates or displays controls by their published text gets a value AWS never returns, so catalog diffing reports spurious changes.
Fix: the ListControls catalog data set (Name/Description for this record), not the serializer.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#3.

--- Cause 3 of 7: load-balancer logging control Name/Description re-worded ---
AWS: for arn:aws:controlcatalog:::control/8abhdcvu10syqrdyfgr2300px the catalog publishes Name "Checks whether the Application Load Balancers and the Classic Load Balancers have logging enabled" and Description "Checks whether the Application Load Balancers and the Classic Load Balancers have logging enabled." (recorded behavior only).
Emulator: serves Name "Checks if the Application Load Balancer and the Classic Load Balancer have logging enabled" and Description "Checks if the Application Load Balancer and the Classic Load Balancer have logging enabled. The rule is NON_COMPLIANT if the access_logs.s3.enabled is false or access_logs.S3.bucket is not equal to the s3BucketName that you provided." — singular/plural rewrite plus an appended NON_COMPLIANT sentence AWS does not carry in this record.
Why: same as cause 2 — the published display text for the control is not reproducible from the emulator.
Fix: the ListControls catalog data set for this record.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#354.

--- Cause 4 of 7: Redshift require_SSL control Description re-worded ---
AWS: for arn:aws:controlcatalog:::control/dfbp3c8jerraflh2g68427p4k the catalog publishes Description "Checks if Amazon Redshift clusters require TLS/SSL encryption to connect to SQL clients. The rule is NON_COMPLIANT if any Amazon Redshift cluster has parameter require_SSL not set to true." (recorded behavior only).
Emulator: serves "Checks if Amazon Redshift clusters require TLS/SSL encryption to connect to SQL clients. The rule is NON_COMPLIANT if the cluster's parameter group does not require an encrypted connection." — the second sentence is re-authored and drops the named parameter (require_SSL) the published text cites.
Why: same as cause 2; additionally the emulator's wording loses the parameter name a remediation workflow keys on.
Fix: the ListControls catalog data set for this record.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#582.

--- Cause 5 of 7: S3 bucket-logging control Name/Description re-worded ---
AWS: for arn:aws:controlcatalog:::control/bd9d97ndoue5lss1zjjiecle4 the catalog publishes Name "Checks if logging is enabled for your S3 buckets" and Description "Checks if logging is enabled for your S3 buckets. The rule is NON_COMPLIANT if logging is not enabled." (recorded behavior only).
Emulator: serves Name "Checks whether logging is enabled for your S3 buckets" and Description "Checks whether logging is enabled for your S3 buckets. The rule is NON_COMPLIANT if logging is not enabled." — "if" is rewritten to "whether" (the inverse of the substitution made in causes 2 and 3, so the data set is paraphrased rather than consistently transformed).
Why: same as cause 2.
Fix: the ListControls catalog data set for this record.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#620.

--- Cause 6 of 7: SNS KMS-encryption control Name/Description re-worded ---
AWS: for arn:aws:controlcatalog:::control/bn8ygh3zsy33drdeojmu09a4z the catalog publishes Name "Checks if SNS topics are encrypted with AWS Key Management Service (AWS KMS)" and Description "Checks if SNS topics are encrypted with AWS Key Management Service (AWS KMS). The rule is NON_COMPLIANT if an SNS topic is not encrypted with AWS KMS. Optionally, specify the key ARNs, the alias ARNs, the alias name, or the key IDs for the rule to check." (recorded behavior only).
Emulator: serves Name "Checks if an Amazon Simple Notification Service (Amazon SNS) topic is encrypted with AWS Key Management Service (AWS KMS)" and Description "Checks if an Amazon Simple Notification Service (Amazon SNS) topic is encrypted with AWS Key Management Service (AWS KMS). The rule is NON_COMPLIANT if an Amazon SNS topic is not encrypted with AWS KMS. Optionally, you can specify the AWS KMS key ARNs, the AWS KMS key aliases, or the AWS KMS key IDs the topic has to be encrypted with." — expanded service names and a re-authored optional-parameter sentence.
Why: same as cause 2.
Fix: the ListControls catalog data set for this record.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#679.

--- Cause 7 of 7: VPC default-security-group control Name/Description re-worded ---
AWS: for arn:aws:controlcatalog:::control/3ucly6t3j33t5zhbzy45l619n the catalog publishes Name "Checks if the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic" and Description "Checks if the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic. The rule is NON_COMPLIANT if the default security group has one or more inbound or outbound traffic rules." (recorded behavior only).
Emulator: serves Name "Checks whether the default security group ..." and Description "Checks whether the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic. The rule is NON_COMPLIANT if the default security group has one or more inbound or outbound rules." — "if" -> "whether" and "inbound or outbound traffic rules" -> "inbound or outbound rules".
Why: same as cause 2.
Fix: the ListControls catalog data set for this record.
Traces: controlcatalog-catalog-list-controls-aws-config-configrule#task#710.

Relation to open issues (none of these cover the records above; the replay reports no owning issue for any of the 7 clusters): 20261118T190218Z-controlcatalog-listcontrols-rewrites-the-published-description-of-the covers only the S3 account-level block-public-access control's Description; 20261115T014344Z-controlcatalog-config-rule-controls-are-served-under-emulator-minted covers ARN minting and emptied Aliases, not Name/Description text or an empty page; 20261113T140928Z-controlcatalog-taxonomy-and-control-text-are-emulator-authored and 20261116T095506Z-controlcatalog-common-control-and-domain-catalog-text-is are about the common-control/domain taxonomy. Several previously resolved issues (20261116T095506Z-controlcatalog-listcontrols-rewrites-aws-config-rule-control-name, 20261118T033122Z-controlcatalog-listcontrols-serves-re-worded-name-description-and-invented) fixed OTHER Config-rule records; the 6 records in causes 2-7 are still drifted and cause 1 shows the filtered page can come back with no rows at all.

## Evidence
Replay of trace set simparity-wire-aws-v1 against the emulator built from this checkout: 1408 steps, 52 passed, 1356 mismatched, score 3.7%. Fixture controlcatalog-catalog-list-controls-aws-config-configrule, operation ListControls, all 7 clusters below expected 200 and got 200 (shape/status correct, catalog content wrong); none carries a stub_path and none is owned by an open issue.

Cause 1 — step task#4 (cluster covers 1244 steps, representatives task#4..task#28):
  $.Controls[0]: missing
    expected {'Aliases': ['CONFIG.ACM.DT.1'], 'Arn': 'arn:aws:controlcatalog:::control/1ysrvaplv6a00p99mim170vqy', 'Behavior': 'DETECTIVE', 'CreateTime': 1678060800.0, 'Description': "Checks if RSA certificates managed by AWS Certificate Manager (ACM) have a key length of at least '2048' bits. The rule is NON_COMPLIANT if the minimum key length is less than 2048 bits.", 'GovernedProviders': ['AWS'], 'GovernedResources': ['AWS::CertificateManager::Certificate'], 'Implementation': {'Identifier': 'ACM_CERTIFICATE_RSA_CHECK', 'Type': 'AWS::Config::ConfigRule'}, 'Name': "Checks if RSA certificates managed by AWS Certificate Manager (ACM) have a key length of at least '2048' bits", 'ParameterRequirementSummary': 'NONE', 'Severity': 'HIGH'}
    got None

Cause 2 — step task#3, control/a6kwsgw65ayxiqg7dhzbz2n7w:
  .Description expected 'Checks whether ACM Certificates in your account are marked for expiration within the specified number of days. Certificates provided by ACM are automatically renewed. ACM does not automatically renew certificates that you import.'
            got 'Checks if AWS Certificate Manager Certificates in your account are marked for expiration within the specified number of days. Certificates provided by ACM are automatically renewed. ACM does not automatically renew certificates that you import. The rule is NON_COMPLIANT if your certificates are about to expire.'
  .Name expected 'Checks whether ACM Certificates in your account are marked for expiration within the specified number of days'
        got 'Checks if AWS Certificate Manager Certificates in your account are marked for expiration within the specified number of days'

Cause 3 — step task#354, control/8abhdcvu10syqrdyfgr2300px:
  .Description expected 'Checks whether the Application Load Balancers and the Classic Load Balancers have logging enabled.'
            got 'Checks if the Application Load Balancer and the Classic Load Balancer have logging enabled. The rule is NON_COMPLIANT if the access_logs.s3.enabled is false or access_logs.S3.bucket is not equal to the s3BucketName that you provided.'
  .Name expected 'Checks whether the Application Load Balancers and the Classic Load Balancers have logging enabled'
        got 'Checks if the Application Load Balancer and the Classic Load Balancer have logging enabled'

Cause 4 — step task#582, control/dfbp3c8jerraflh2g68427p4k:
  .Description expected 'Checks if Amazon Redshift clusters require TLS/SSL encryption to connect to SQL clients. The rule is NON_COMPLIANT if any Amazon Redshift cluster has parameter require_SSL not set to true.'
            got "Checks if Amazon Redshift clusters require TLS/SSL encryption to connect to SQL clients. The rule is NON_COMPLIANT if the cluster's parameter group does not require an encrypted connection."

Cause 5 — step task#620, control/bd9d97ndoue5lss1zjjiecle4:
  .Description expected 'Checks if logging is enabled for your S3 buckets. The rule is NON_COMPLIANT if logging is not enabled.'
            got 'Checks whether logging is enabled for your S3 buckets. The rule is NON_COMPLIANT if logging is not enabled.'
  .Name expected 'Checks if logging is enabled for your S3 buckets'
        got 'Checks whether logging is enabled for your S3 buckets'

Cause 6 — step task#679, control/bn8ygh3zsy33drdeojmu09a4z:
  .Description expected 'Checks if SNS topics are encrypted with AWS Key Management Service (AWS KMS). The rule is NON_COMPLIANT if an SNS topic is not encrypted with AWS KMS. Optionally, specify the key ARNs, the alias ARNs, the alias name, or the key IDs for the rule to check.'
            got 'Checks if an Amazon Simple Notification Service (Amazon SNS) topic is encrypted with AWS Key Management Service (AWS KMS). The rule is NON_COMPLIANT if an Amazon SNS topic is not encrypted with AWS KMS. Optionally, you can specify the AWS KMS key ARNs, the AWS KMS key aliases, or the AWS KMS key IDs the topic has to be encrypted with.'
  .Name expected 'Checks if SNS topics are encrypted with AWS Key Management Service (AWS KMS)'
        got 'Checks if an Amazon Simple Notification Service (Amazon SNS) topic is encrypted with AWS Key Management Service (AWS KMS)'

Cause 7 — step task#710, control/3ucly6t3j33t5zhbzy45l619n:
  .Description expected 'Checks if the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic. The rule is NON_COMPLIANT if the default security group has one or more inbound or outbound traffic rules.'
            got 'Checks whether the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic. The rule is NON_COMPLIANT if the default security group has one or more inbound or outbound rules.'
  .Name expected 'Checks if the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic'
        got 'Checks whether the default security group of any Amazon Virtual Private Cloud (Amazon VPC) does not allow inbound or outbound traffic'

## Steps to Reproduce
1. Clone the repo and boot the emulator built from this checkout.
2. Replay the wire fixture corpus (trace set simparity-wire-aws-v1).
3. Fixture controlcatalog-catalog-list-controls-aws-config-configrule drives ListControls (POST /list-controls) with Filter.Implementations.Types = ["AWS::Config::ConfigRule"], paging with MaxResults/NextToken.
4. Cause 1: replay step task#4 (and any of task#5..task#28). The response is 200 but $.Controls is empty — there is no element at $.Controls[0], where AWS returns the Config-rule ControlSummary row for arn:aws:controlcatalog:::control/1ysrvaplv6a00p99mim170vqy (Implementation.Identifier ACM_CERTIFICATE_RSA_CHECK).
5. Causes 2-7: replay steps task#3, task#354, task#582, task#620, task#679, task#710 and compare, for the control ARN named in each, the served ControlSummary.Name / ControlSummary.Description against the recorded published text. Each differs in fixed sentence text (re-worded leading clause and/or an appended or re-authored NON_COMPLIANT sentence), not in any per-run value.
6. To confirm a fix, re-replay all seven steps listed above: controlcatalog-catalog-list-controls-aws-config-configrule at task#4, task#3, task#354, task#582, task#620, task#679 and task#710.
