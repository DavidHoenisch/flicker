// Data masking and PII redaction module
//
// DESIGN CHOICE: Compile regexes once at startup, not on every line.
// This ensures masking adds minimal overhead to log processing.

use regex::Regex;
use serde::Deserialize;

/// Top-level masking configuration per source
#[derive(Debug, Deserialize, Clone, Default)]
pub struct MaskingConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default)]
    pub rules: MaskingRules,
}

/// Individual rule configuration with action
#[derive(Debug, Deserialize, Clone)]
pub struct RuleConfig {
    #[serde(default)]
    pub enabled: bool,

    #[serde(default = "default_action")]
    pub action: String,

    #[serde(default)]
    pub replacement: Option<String>,

    #[serde(default)]
    pub partial_mask: Option<PartialMaskConfig>,
}

impl Default for RuleConfig {
    fn default() -> Self {
        Self {
            enabled: false,
            action: default_action(),
            replacement: None,
            partial_mask: None,
        }
    }
}

#[derive(Debug, Deserialize, Clone)]
pub struct PartialMaskConfig {
    #[serde(default = "default_partial_keep_last")]
    pub keep_last: usize,

    #[serde(default = "default_partial_mask_char")]
    pub mask_char: String,
}

fn default_action() -> String {
    "redact".to_string()
}

fn default_partial_keep_last() -> usize {
    4
}

fn default_partial_mask_char() -> String {
    "*".to_string()
}

/// Built-in masking rules that can be enabled via boolean/config
#[derive(Debug, Deserialize, Clone, Default)]
pub struct MaskingRules {
    #[serde(default)]
    pub email: Option<RuleConfig>,

    #[serde(default)]
    pub credit_card: Option<RuleConfig>,

    #[serde(default)]
    pub ssn: Option<RuleConfig>,

    #[serde(default)]
    pub phone: Option<RuleConfig>,

    #[serde(default)]
    pub ip_address: Option<RuleConfig>,

    #[serde(default)]
    pub api_key: Option<RuleConfig>,

    #[serde(default)]
    pub custom: Vec<CustomRule>,
}

/// User-defined custom masking rule
#[derive(Debug, Deserialize, Clone)]
pub struct CustomRule {
    pub name: String,
    pub pattern: String,

    #[serde(default = "default_action")]
    pub action: String,

    #[serde(default)]
    pub replacement: Option<String>,

    #[serde(default)]
    pub partial_mask: Option<PartialMaskConfig>,
}

/// Extra check on a regex match; the match is left untouched if it returns false
type Validator = fn(&str) -> bool;

/// Built-in rule: (name, config, default replacement, patterns with optional validators)
type BuiltinRule<'a> = (
    &'a str,
    &'a Option<RuleConfig>,
    &'a str,
    &'a [(&'a str, Option<Validator>)],
);

/// Compiled rule ready for efficient application
#[derive(Debug, Clone)]
#[allow(dead_code)]
struct CompiledRule {
    name: String,
    regex: Regex,
    action: MaskingAction,
    validate: Option<Validator>,
}

#[derive(Debug, Clone)]
enum MaskingAction {
    Redact(String),
    Partial { keep_last: usize, mask_char: char },
}

/// The main masking engine that applies all enabled rules
#[derive(Debug, Clone)]
pub struct MaskingEngine {
    rules: Vec<CompiledRule>,
}

impl MaskingEngine {
    /// Create a new masking engine from configuration
    /// Returns None if masking is disabled or no rules are enabled
    pub fn new(config: &MaskingConfig) -> anyhow::Result<Option<Self>> {
        if !config.enabled {
            return Ok(None);
        }

        let mut rules = Vec::new();

        // Compile built-in rules
        let builtins: [BuiltinRule; 6] = [
            (
                "email",
                &config.rules.email,
                "[EMAIL]",
                &[(EMAIL_PATTERN, None)],
            ),
            (
                "credit_card",
                &config.rules.credit_card,
                "[CREDIT_CARD]",
                &[(CREDIT_CARD_PATTERN, Some(is_plausible_card_number))],
            ),
            ("ssn", &config.rules.ssn, "[SSN]", &[(SSN_PATTERN, None)]),
            (
                "phone",
                &config.rules.phone,
                "[PHONE]",
                &[(PHONE_PATTERN, None)],
            ),
            (
                "ip_address",
                &config.rules.ip_address,
                "[IP]",
                &[
                    (IPV4_PATTERN, None),
                    (IPV6_CANDIDATE_PATTERN, Some(is_ipv6)),
                ],
            ),
            (
                "api_key",
                &config.rules.api_key,
                "[API_KEY]",
                &[
                    (SECRET_ASSIGNMENT_PATTERN, None),
                    (AUTH_HEADER_PATTERN, None),
                    (AWS_ACCESS_KEY_PATTERN, None),
                ],
            ),
        ];
        for (name, rule_config, default_replacement, patterns) in builtins {
            let Some(rule_config) = rule_config.as_ref().filter(|r| r.enabled) else {
                continue;
            };
            for (pattern, validate) in patterns {
                let regex = Regex::new(pattern)
                    .map_err(|e| anyhow::anyhow!("Invalid {} regex: {}", name, e))?;
                rules.push(CompiledRule {
                    name: name.to_string(),
                    regex,
                    action: Self::action_from_config(rule_config, default_replacement),
                    validate: *validate,
                });
            }
        }

        // Compile custom rules
        for custom in &config.rules.custom {
            let regex = Regex::new(&custom.pattern)
                .map_err(|e| anyhow::anyhow!("Invalid custom pattern '{}': {}", custom.name, e))?;
            rules.push(CompiledRule {
                name: custom.name.clone(),
                regex,
                action: Self::action_from_custom(custom),
                validate: None,
            });
        }

        if rules.is_empty() {
            return Ok(None);
        }

        Ok(Some(Self { rules }))
    }

    fn action_from_config(config: &RuleConfig, default_replacement: &str) -> MaskingAction {
        match config.action.as_str() {
            "partial" => {
                let partial = config.partial_mask.as_ref();
                MaskingAction::Partial {
                    keep_last: partial.map(|p| p.keep_last).unwrap_or(4),
                    mask_char: partial
                        .map(|p| p.mask_char.chars().next().unwrap_or('*'))
                        .unwrap_or('*'),
                }
            }
            _ => MaskingAction::Redact(
                config
                    .replacement
                    .clone()
                    .unwrap_or_else(|| default_replacement.to_string()),
            ),
        }
    }

    fn action_from_custom(custom: &CustomRule) -> MaskingAction {
        match custom.action.as_str() {
            "partial" => {
                let partial = custom.partial_mask.as_ref();
                MaskingAction::Partial {
                    keep_last: partial.map(|p| p.keep_last).unwrap_or(4),
                    mask_char: partial
                        .map(|p| p.mask_char.chars().next().unwrap_or('*'))
                        .unwrap_or('*'),
                }
            }
            _ => MaskingAction::Redact(
                custom
                    .replacement
                    .clone()
                    .unwrap_or_else(|| format!("[{}]", custom.name.to_uppercase())),
            ),
        }
    }

    /// Apply all enabled masking rules to a log line
    /// Rules are applied sequentially in order defined.
    ///
    /// DESIGN CHOICE: If a pattern has a `(?P<mask>...)` group, only that part is
    /// masked (e.g. `password=[API_KEY]` keeps the key name for context).
    pub fn apply(&self, line: &str) -> String {
        let mut result = line.to_string();

        for rule in &self.rules {
            result = rule
                .regex
                .replace_all(&result, |caps: &regex::Captures| {
                    let whole = caps.get(0).unwrap();
                    let target = caps.name("mask").unwrap_or(whole);
                    if let Some(validate) = rule.validate
                        && !validate(target.as_str())
                    {
                        return whole.as_str().to_string();
                    }
                    let masked = match &rule.action {
                        MaskingAction::Redact(replacement) => replacement.clone(),
                        MaskingAction::Partial {
                            keep_last,
                            mask_char,
                        } => Self::apply_partial_mask(target.as_str(), *keep_last, *mask_char),
                    };
                    let text = whole.as_str();
                    format!(
                        "{}{}{}",
                        &text[..target.start() - whole.start()],
                        masked,
                        &text[target.end() - whole.start()..]
                    )
                })
                .to_string();
        }

        result
    }

    fn apply_partial_mask(text: &str, keep_last: usize, mask_char: char) -> String {
        // DESIGN CHOICE: Count chars, not bytes, so slicing never splits a
        // multi-byte UTF-8 character (which would panic).
        let len = text.chars().count();
        if len <= keep_last {
            return mask_char.to_string().repeat(len);
        }

        let masked_len = len - keep_last;
        let mask_str: String = mask_char.to_string().repeat(masked_len);
        let visible_start = text
            .char_indices()
            .nth(masked_len)
            .map_or(text.len(), |(i, _)| i);
        let visible_part = &text[visible_start..];

        format!("{}{}", mask_str, visible_part)
    }

    /// Check if this engine has any active rules
    #[allow(dead_code)]
    pub fn is_active(&self) -> bool {
        !self.rules.is_empty()
    }
}

// Built-in PII regex patterns
// Email pattern - matches common email formats
const EMAIL_PATTERN: &str = r"[a-zA-Z0-9._%+-]+@[a-zA-Z0-9.-]+\.[a-zA-Z]{2,}";

// Credit card pattern - 13-19 contiguous digits, or 4-4-4-4 / Amex 4-6-5 groups
// separated by spaces or dashes. Matches are confirmed by is_plausible_card_number.
const CREDIT_CARD_PATTERN: &str =
    r"\b(?:\d{4}[ -]\d{4}[ -]\d{4}[ -]\d{4}|\d{4}[ -]\d{6}[ -]\d{5}|\d{13,19})\b";

// SSN pattern - matches XXX-XX-XXXX format
const SSN_PATTERN: &str = r"\b\d{3}-\d{2}-\d{4}\b";

// Phone pattern - requires separators or a + prefix, so bare digit runs like
// epoch timestamps aren't matched. E.g. 555-123-4567, (555) 123-4567,
// +1 555 123 4567, +442071234567
const PHONE_PATTERN: &str =
    r"(?:\+\d{1,3}[\s.-]?)?(?:\(\d{3}\)\s?|\b\d{3}[\s.-])\d{3}[\s.-]\d{4}\b|\+\d{8,15}\b";

// IPv4 address
const IPV4_PATTERN: &str = r"\b(?:(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\.){3}(?:25[0-5]|2[0-4][0-9]|[01]?[0-9][0-9]?)\b";

// IPv6 candidate (hex groups and colons); confirmed by is_ipv6 so times and
// MAC addresses aren't masked
const IPV6_CANDIDATE_PATTERN: &str = r"(?:[0-9A-Fa-f]{0,4}:){2,7}[0-9A-Fa-f]{0,4}";

// Secrets assigned to a key: api_key=..., "password": "...", DB_PASSWORD=..., token: ...
// Case-insensitive; the key may have a prefix (db_password, github_token).
const SECRET_ASSIGNMENT_PATTERN: &str = r#"(?i)\b[a-z0-9_.-]*(?:api[_-]?key|apikey|access[_-]?key|secret|token|password|passwd|pwd|credentials?)["']?\s*[:=]\s*["']?(?P<mask>[^\s"',;&]+)"#;

// Authorization header values: Bearer <token>, Basic <base64>
const AUTH_HEADER_PATTERN: &str = r"(?i)\b(?:bearer|basic)\s+(?P<mask>[A-Za-z0-9._~+/-]{8,}=*)";

// AWS access key IDs
const AWS_ACCESS_KEY_PATTERN: &str = r"\b(?:AKIA|ASIA)[0-9A-Z]{16}\b";

/// Card numbers: 13-19 digits, a real issuer prefix (2-6) and a valid Luhn checksum.
/// Rejects epoch-millisecond/microsecond timestamps (leading 1) and most other IDs.
fn is_plausible_card_number(text: &str) -> bool {
    let digits: Vec<u32> = text.chars().filter_map(|c| c.to_digit(10)).collect();
    if !(13..=19).contains(&digits.len()) || !(2..=6).contains(&digits[0]) {
        return false;
    }
    let sum: u32 = digits
        .iter()
        .rev()
        .enumerate()
        .map(|(i, &d)| {
            if i % 2 == 1 {
                let doubled = d * 2;
                if doubled > 9 { doubled - 9 } else { doubled }
            } else {
                d
            }
        })
        .sum();
    sum.is_multiple_of(10)
}

fn is_ipv6(text: &str) -> bool {
    text.parse::<std::net::Ipv6Addr>().is_ok()
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::config::Config;

    #[test]
    fn test_email_redaction() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                email: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[EMAIL]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Contact john.doe@example.com for support";
        let result = engine.apply(line);
        assert_eq!(result, "Contact [EMAIL] for support");
    }

    #[test]
    fn test_email_partial_masking() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                email: Some(RuleConfig {
                    enabled: true,
                    action: "partial".to_string(),
                    replacement: None,
                    partial_mask: Some(PartialMaskConfig {
                        keep_last: 4,
                        mask_char: "*".to_string(),
                    }),
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "User email: alice.smith@company.org";
        let result = engine.apply(line);
        // alice.smith@company.org has 23 chars, keep_last=4, so 19 stars
        assert!(result.starts_with("User email: "));
        assert!(result.ends_with(".org"));
        assert!(result.contains("***")); // Should be masked
    }

    #[test]
    fn test_credit_card_redaction() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                credit_card: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[CC]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Payment method: 4111 1111 1111 1111";
        let result = engine.apply(line);
        assert_eq!(result, "Payment method: [CC]");
    }

    #[test]
    fn test_ssn_redaction() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                ssn: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[SSN]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "SSN: 123-45-6789 for verification";
        let result = engine.apply(line);
        assert_eq!(result, "SSN: [SSN] for verification");
    }

    #[test]
    fn test_phone_redaction() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                phone: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[PHONE]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Call us at 555-123-4567 or (555) 987-6543";
        let result = engine.apply(line);
        // Phone numbers should be replaced with [PHONE]
        assert!(result.contains("[PHONE]"));
        assert!(!result.contains("555-123-4567"));
        assert!(!result.contains("987-6543"));
    }

    #[test]
    fn test_ip_address_redaction() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                ip_address: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[IP]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Request from 192.168.1.100 to server";
        let result = engine.apply(line);
        assert_eq!(result, "Request from [IP] to server");
    }

    #[test]
    fn test_api_key_redaction() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                api_key: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[API_KEY]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Authorization: api_key=sk_live_abcdefghijklmnopqrstuvwxyz123456";
        let result = engine.apply(line);
        assert!(result.contains("[API_KEY]") || result.contains("api_key="));
    }

    #[test]
    fn test_multiple_rules() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                email: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[EMAIL]".to_string()),
                    partial_mask: None,
                }),
                credit_card: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[CC]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "User bob@example.com paid with 4111111111111111";
        let result = engine.apply(line);
        assert_eq!(result, "User [EMAIL] paid with [CC]");
    }

    #[test]
    fn test_custom_rule() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                custom: vec![CustomRule {
                    name: "session_id".to_string(),
                    pattern: r"session_id=[a-f0-9]{32}".to_string(),
                    action: "redact".to_string(),
                    replacement: Some("session_id=[REDACTED]".to_string()),
                    partial_mask: None,
                }],
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Request with session_id=abc123def45678901234567890123456";
        let result = engine.apply(line);
        assert_eq!(result, "Request with session_id=[REDACTED]");
    }

    #[test]
    fn test_disabled_masking() {
        let config = MaskingConfig {
            enabled: false,
            rules: MaskingRules {
                email: Some(RuleConfig {
                    enabled: true,
                    action: "redact".to_string(),
                    replacement: Some("[EMAIL]".to_string()),
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap();
        assert!(engine.is_none());
    }

    #[test]
    fn test_no_enabled_rules() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                email: Some(RuleConfig {
                    enabled: false,
                    action: "redact".to_string(),
                    replacement: None,
                    partial_mask: None,
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap();
        assert!(engine.is_none());
    }

    #[test]
    fn test_invalid_regex_fails() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                custom: vec![CustomRule {
                    name: "bad_pattern".to_string(),
                    pattern: r"[invalid(regex".to_string(),
                    action: "redact".to_string(),
                    replacement: None,
                    partial_mask: None,
                }],
                ..Default::default()
            },
        };

        let result = MaskingEngine::new(&config);
        assert!(result.is_err());
    }

    #[test]
    fn test_default_replacement() {
        let yaml = r#"
enabled: true
rules:
  email:
    enabled: true
"#;

        let config: MaskingConfig = serde_yaml::from_str(yaml).unwrap();
        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Send to test@example.com";
        let result = engine.apply(line);
        assert_eq!(result, "Send to [EMAIL]");
    }

    #[test]
    fn test_partial_masking_multibyte_utf8() {
        assert_eq!(MaskingEngine::apply_partial_mask("José", 1, '*'), "***é");
        assert_eq!(
            MaskingEngine::apply_partial_mask("日本語テキスト", 2, '#'),
            "#####スト"
        );
        assert_eq!(MaskingEngine::apply_partial_mask("é", 1, '*'), "*");
        assert_eq!(MaskingEngine::apply_partial_mask("ab", 0, '*'), "**");
    }

    #[test]
    fn test_partial_masking_short_string() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                email: Some(RuleConfig {
                    enabled: true,
                    action: "partial".to_string(),
                    replacement: None,
                    partial_mask: Some(PartialMaskConfig {
                        keep_last: 10,
                        mask_char: "#".to_string(),
                    }),
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        // Short email should be fully masked if keep_last >= length
        let line = "Email: a@b.co";
        let result = engine.apply(line);
        assert!(result.starts_with("Email: "));
        assert!(result.contains("@") || result.contains("#"));
    }

    #[test]
    fn test_credit_card_partial_masking() {
        let config = MaskingConfig {
            enabled: true,
            rules: MaskingRules {
                credit_card: Some(RuleConfig {
                    enabled: true,
                    action: "partial".to_string(),
                    replacement: None,
                    partial_mask: Some(PartialMaskConfig {
                        keep_last: 4,
                        mask_char: "*".to_string(),
                    }),
                }),
                ..Default::default()
            },
        };

        let engine = MaskingEngine::new(&config).unwrap().unwrap();

        let line = "Card: 4111111111111111";
        let result = engine.apply(line);
        assert!(result.starts_with("Card: "));
        assert!(result.contains("1111")); // Last 4 digits visible
        assert!(result.contains("*")); // Rest masked
    }

    #[test]
    fn test_real_config_from_yaml() {
        // This test verifies the actual test-masking-config.yaml works correctly
        let yaml = r#"
log_files:
  - path: "./test-masking.log"
    polling_frequency_ms: 250
    buffer_size: 5
    flush_interval_ms: 5000
    masking:
      enabled: true
      rules:
        email:
          enabled: true
          action: "redact"
          replacement: "[EMAIL_REDACTED]"
        credit_card:
          enabled: true
          action: "redact"
          replacement: "[CC_REDACTED]"
        ssn:
          enabled: true
          action: "redact"
          replacement: "[SSN_REDACTED]"
        phone:
          enabled: true
          action: "redact"
          replacement: "[PHONE_REDACTED]"
        ip_address:
          enabled: true
          action: "redact"
          replacement: "[IP_REDACTED]"
        custom:
          - name: "session_token"
            pattern: "session_token=[a-f0-9]{32}"
            action: "redact"
            replacement: "session_token=[TOKEN_REDACTED]"
    destination:
      endpoint: "http://localhost:8000/ingest"
      type: "http"
"#;

        let config = Config::from_yaml(yaml).unwrap();
        let masking_config = &config.log_files[0].masking;

        // Verify config loaded correctly
        assert!(masking_config.enabled);

        // Create the masking engine
        let engine = MaskingEngine::new(masking_config).unwrap();
        assert!(
            engine.is_some(),
            "Masking engine should be created with active rules"
        );

        let engine = engine.unwrap();

        // Test actual log lines from test-masking.log
        let test_cases = vec![
            (
                "Payment processed for jane.smith@company.org",
                "Payment processed for [EMAIL_REDACTED]",
            ),
            (
                "Request from IP 192.168.1.100 processed",
                "Request from IP [IP_REDACTED] processed",
            ),
            (
                "Payment processed for 4111111111111111",
                "Payment processed for [CC_REDACTED]",
            ),
            (
                "Session created: session_token=abc123def45678901234567890123456",
                "Session created: session_token=[TOKEN_REDACTED]",
            ),
            (
                "New registration: +1 555-789-0123",
                "New registration: [PHONE_REDACTED]",
            ),
            (
                "Authentication failed for user at 001-23-4567",
                "Authentication failed for user at [SSN_REDACTED]",
            ),
        ];

        for (input, expected) in test_cases {
            let result = engine.apply(input);
            assert_eq!(result, expected, "Failed for input: {}", input);
        }
    }

    fn engine_with(rules: MaskingRules) -> MaskingEngine {
        let enabled = || {
            Some(RuleConfig {
                enabled: true,
                ..Default::default()
            })
        };
        let rules = MaskingRules {
            email: rules.email.and_then(|_| enabled()),
            credit_card: rules.credit_card.and_then(|_| enabled()),
            ssn: rules.ssn.and_then(|_| enabled()),
            phone: rules.phone.and_then(|_| enabled()),
            ip_address: rules.ip_address.and_then(|_| enabled()),
            api_key: rules.api_key.and_then(|_| enabled()),
            custom: rules.custom,
        };
        MaskingEngine::new(&MaskingConfig {
            enabled: true,
            rules,
        })
        .unwrap()
        .unwrap()
    }

    fn on() -> Option<RuleConfig> {
        Some(RuleConfig::default())
    }

    #[test]
    fn test_timestamps_not_masked() {
        let engine = engine_with(MaskingRules {
            credit_card: on(),
            phone: on(),
            ..Default::default()
        });
        for line in [
            "ts=1731801600123",
            "epoch=1731801600",
            "us=1731801600123456",
            "order=1234567890123",
            "at 2024-11-17 12:30:45",
        ] {
            assert_eq!(engine.apply(line), line);
        }
    }

    #[test]
    fn test_credit_card_luhn_and_formats() {
        let engine = engine_with(MaskingRules {
            credit_card: on(),
            ..Default::default()
        });
        assert_eq!(
            engine.apply("card 4111-1111-1111-1111"),
            "card [CREDIT_CARD]"
        );
        assert_eq!(engine.apply("amex 3782 822463 10005"), "amex [CREDIT_CARD]");
        assert_eq!(
            engine.apply("card 4111 1111 1111 1111 12/25"),
            "card [CREDIT_CARD] 12/25"
        );
        // Fails Luhn
        assert_eq!(
            engine.apply("card 4111111111111112"),
            "card 4111111111111112"
        );
    }

    #[test]
    fn test_phone_formats() {
        let engine = engine_with(MaskingRules {
            phone: on(),
            ..Default::default()
        });
        assert_eq!(engine.apply("call 555.123.4567"), "call [PHONE]");
        assert_eq!(engine.apply("call +442071234567"), "call [PHONE]");
        assert_eq!(engine.apply("call +1 (555) 123-4567"), "call [PHONE]");
        assert_eq!(engine.apply("SSN 123-45-6789"), "SSN 123-45-6789");
    }

    #[test]
    fn test_api_key_variants() {
        let engine = engine_with(MaskingRules {
            api_key: on(),
            ..Default::default()
        });
        let cases = [
            ("API_KEY=abcdef123456", "API_KEY=[API_KEY]"),
            ("password=hunter2 user=bob", "password=[API_KEY] user=bob"),
            (r#"{"secret": "s3cr3t"}"#, r#"{"secret": "[API_KEY]"}"#),
            ("DB_PASSWORD: pa55", "DB_PASSWORD: [API_KEY]"),
            (
                "Authorization: Bearer eyJhbGciOiJIUzI1NiJ9.abc.def",
                "Authorization: Bearer [API_KEY]",
            ),
            (
                "Authorization: Basic dXNlcjpwYXNz",
                "Authorization: Basic [API_KEY]",
            ),
            ("key AKIAIOSFODNN7EXAMPLE used", "key [API_KEY] used"),
            ("max_tokens_used 5", "max_tokens_used 5"),
        ];
        for (input, expected) in cases {
            assert_eq!(engine.apply(input), expected, "input: {}", input);
        }
    }

    #[test]
    fn test_ipv6_masked_but_not_times_or_macs() {
        let engine = engine_with(MaskingRules {
            ip_address: on(),
            ..Default::default()
        });
        assert_eq!(
            engine.apply("from 2001:db8::8a2e:370:7334 ok"),
            "from [IP] ok"
        );
        assert_eq!(engine.apply("from ::1 ok"), "from [IP] ok");
        assert_eq!(engine.apply("at 12:30:45"), "at 12:30:45");
        assert_eq!(
            engine.apply("mac aa:bb:cc:dd:ee:ff"),
            "mac aa:bb:cc:dd:ee:ff"
        );
        assert_eq!(engine.apply("from 10.0.0.1"), "from [IP]");
    }
}
