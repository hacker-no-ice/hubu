use std::{fs, path::Path};

use crate::policy::{engine::validate_policy, error::PolicyLoadError, model::Policy};

impl Policy {
    /// Read, deserialize, and validate a policy from a YAML file.
    ///
    /// This is the recommended entrypoint for human-authored policy files. A
    /// policy that parses but fails validation is returned as
    /// [`PolicyLoadError::Validation`], so invalid field/value combinations are
    /// rejected before evaluation.
    pub fn from_yaml_file(path: impl AsRef<Path>) -> Result<Self, PolicyLoadError> {
        let path = path.as_ref();
        let yaml = fs::read_to_string(path).map_err(|source| PolicyLoadError::ReadFile {
            path: path.display().to_string(),
            source,
        })?;

        Self::from_yaml_str(&yaml)
    }

    /// Deserialize and validate a policy from a YAML string.
    ///
    /// Useful for tests, embedded defaults, and API payloads that have already
    /// been read into memory.
    pub fn from_yaml_str(yaml: &str) -> Result<Self, PolicyLoadError> {
        let policy: Policy = serde_yaml_ng::from_str(yaml)?;
        validate_policy(&policy)?;
        Ok(policy)
    }
}

#[cfg(test)]
mod tests {
    use std::{fs, path::PathBuf};

    use crate::policy::{
        error::{PolicyLoadError, PolicyValidationError},
        model::{Effect, Policy},
    };

    fn valid_policy_yaml() -> &'static str {
        r#"
id: base_spending_policy
version: 2026-05-22.1
owner_user_id: 00000000-0000-4000-8000-000000000123
default_effect: needs_approval
rules:
  - id: deny_large_purchase
    effect: deny
    reason: Purchase exceeds the $500 single-transaction limit
    when:
      op: gt
      field: amount
      value:
        money_cents: 50000

  - id: allow_small_meals
    effect: allow
    reason: Meals under $50 are pre-approved
    when:
      op: all
      conditions:
        - op: eq
          field: category
          value:
            string: meals
        - op: lte
          field: amount
          value:
            money_cents: 5000
"#
    }

    const STARTER_POLICY_YAML: &str = include_str!("../../../../policies/starter-policy.yaml");
    const TEST_POLICY_YAML: &str = include_str!("../../../../policies/test-policy.yaml");

    fn write_temp_policy_file(contents: &str) -> PathBuf {
        let path = std::env::temp_dir().join(format!(
            "hubu-policy-{}.yaml",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .expect("system clock should be after unix epoch")
                .as_nanos()
        ));

        fs::write(&path, contents).expect("test should write temp policy file");
        path
    }

    #[test]
    fn loads_and_validates_policy_from_yaml_str() {
        let policy = Policy::from_yaml_str(valid_policy_yaml()).expect("policy should load");

        assert_eq!(policy.id, "base_spending_policy");
        assert_eq!(policy.version, "2026-05-22.1");
        assert_eq!(
            policy.owner_user_id.to_string(),
            "00000000-0000-4000-8000-000000000123"
        );
        assert_eq!(policy.default_effect, Effect::NeedsApproval);
        assert_eq!(policy.rules.len(), 2);
    }

    #[test]
    fn loads_repository_policy_fixtures() {
        let starter =
            Policy::from_yaml_str(STARTER_POLICY_YAML).expect("starter policy should load");
        assert_eq!(starter.id, "starter_spending_policy");
        assert_eq!(starter.rules.len(), 2);

        let test = Policy::from_yaml_str(TEST_POLICY_YAML).expect("test policy should load");
        assert_eq!(test.id, "test_policy");
        assert_eq!(test.rules.len(), 2);
    }

    #[test]
    fn loads_and_validates_policy_from_yaml_file() {
        let path = write_temp_policy_file(valid_policy_yaml());

        let policy = Policy::from_yaml_file(&path).expect("policy should load from file");

        assert_eq!(policy.id, "base_spending_policy");
        assert_eq!(policy.rules.len(), 2);

        fs::remove_file(path).expect("test should remove temp policy file");
    }

    #[test]
    fn loads_policy_without_owner_user_id_for_authoring() {
        let yaml = r#"
id: authoring_policy
version: draft-1
default_effect: needs_approval
rules:
  - id: deny_large_purchase
    effect: deny
    reason: Purchase exceeds the $5 single-transaction limit
    when:
      op: gt
      field: amount
      value:
        money_cents: 500
"#;

        let policy = Policy::from_yaml_str(yaml).expect("policy should load without owner_user_id");

        assert_eq!(policy.id, "authoring_policy");
        assert_eq!(policy.rules.len(), 1);
    }

    #[test]
    fn rejects_yaml_policy_that_fails_validation() {
        let invalid_yaml = r#"
id: base_spending_policy
version: 2026-05-22.1
owner_user_id: 00000000-0000-4000-8000-000000000123
default_effect: needs_approval
rules:
  - id: invalid_amount_type
    effect: deny
    reason: Amount should use money cents
    when:
      op: gt
      field: amount
      value:
        string: "50000"
"#;

        let error = Policy::from_yaml_str(invalid_yaml).expect_err("policy should be invalid");

        assert!(matches!(
            error,
            PolicyLoadError::Validation {
                source: PolicyValidationError::FieldValueMismatch { .. }
            }
        ));
    }

    #[test]
    fn rejects_yaml_policy_missing_required_fields() {
        let error = Policy::from_yaml_str(
            r#"
id: missing_version
default_effect: deny
rules: []
"#,
        )
        .expect_err("missing version should fail");

        assert!(matches!(error, PolicyLoadError::ParseYaml { .. }));
        assert_eq!(error.to_string(), "failed to parse policy yaml");
    }

    #[test]
    fn rejects_unknown_policy_fields() {
        let invalid_yaml = r#"
id: base_spending_policy
version: 2026-05-22.1
owner_user_id: 00000000-0000-4000-8000-000000000123
default_effect: needs_approval
daily_limit_cents: 1000
rules: []
"#;

        let error =
            Policy::from_yaml_str(invalid_yaml).expect_err("unknown policy field should fail");

        assert!(matches!(error, PolicyLoadError::ParseYaml { .. }));
        assert!(error.to_string().contains("failed to parse policy yaml"));
    }

    #[test]
    fn rejects_unknown_condition_fields() {
        let invalid_yaml = r#"
id: base_spending_policy
version: 2026-05-22.1
owner_user_id: 00000000-0000-4000-8000-000000000123
default_effect: needs_approval
rules:
  - id: allow_approved_merchants
    effect: allow
    reason: approved merchants are allowed
    when:
      op: in
      field: merchant
      value:
        string: openai
"#;

        let error =
            Policy::from_yaml_str(invalid_yaml).expect_err("unknown condition field should fail");

        assert!(matches!(error, PolicyLoadError::ParseYaml { .. }));
        assert!(error.to_string().contains("failed to parse policy yaml"));
    }

    #[test]
    fn reports_yaml_parse_errors() {
        let error =
            Policy::from_yaml_str("this is not: [valid yaml").expect_err("yaml should fail");

        assert!(matches!(error, PolicyLoadError::ParseYaml { .. }));
        assert_eq!(error.to_string(), "failed to parse policy yaml");
    }
}
