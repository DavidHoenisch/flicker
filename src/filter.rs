// Log line filtering based on regex patterns
//
// DESIGN: Compile regexes once at startup, not on every line.
// This is much more efficient than compiling on each check.

use anyhow::Result;
use regex::Regex;

pub struct LogFilter {
    match_patterns: Vec<Regex>,   // Whitelist: must match at least one
    exclude_patterns: Vec<Regex>, // Blacklist: must not match any
}

impl LogFilter {
    /// Create a new filter from pattern strings
    /// Returns error if any regex pattern is invalid
    pub fn new(match_on: Vec<String>, exclude_on: Vec<String>) -> Result<Self> {
        // Compile all match patterns
        let mut match_patterns = Vec::new();
        for pattern in match_on {
            let regex = Regex::new(&pattern)
                .map_err(|e| anyhow::anyhow!("Invalid match_on regex '{}': {}", pattern, e))?;
            match_patterns.push(regex);
        }

        // Compile all exclude patterns
        let mut exclude_patterns = Vec::new();
        for pattern in exclude_on {
            let regex = Regex::new(&pattern)
                .map_err(|e| anyhow::anyhow!("Invalid exclude_on regex '{}': {}", pattern, e))?;
            exclude_patterns.push(regex);
        }

        Ok(Self {
            match_patterns,
            exclude_patterns,
        })
    }

    /// Check if a log line should be shipped
    ///
    /// DESIGN CHOICE: Two-stage filtering logic
    /// 1. If match_patterns is non-empty: Line must match at least one pattern
    /// 2. If exclude_patterns is non-empty: Line must not match any pattern
    ///
    /// This allows flexible filtering:
    /// - match_on only: Whitelist mode (only ship matching lines)
    /// - exclude_on only: Blacklist mode (ship all except matching lines)
    /// - Both: Whitelist then blacklist (ship matching lines except excluded ones)
    pub fn should_ship(&self, line: &str) -> bool {
        // Stage 1: Check match patterns (whitelist)
        if !self.match_patterns.is_empty() {
            let matches_any = self.match_patterns.iter().any(|regex| regex.is_match(line));
            if !matches_any {
                return false; // Doesn't match whitelist, skip
            }
        }

        // Stage 2: Check exclude patterns (blacklist)
        if !self.exclude_patterns.is_empty() {
            let matches_any = self
                .exclude_patterns
                .iter()
                .any(|regex| regex.is_match(line));
            if matches_any {
                return false; // Matches blacklist, skip
            }
        }

        // Passed all filters
        true
    }

    /// Returns true if this filter has no patterns (passes everything)
    pub fn is_passthrough(&self) -> bool {
        self.match_patterns.is_empty() && self.exclude_patterns.is_empty()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_no_filters() {
        let filter = LogFilter::new(vec![], vec![]).unwrap();
        assert!(filter.is_passthrough());
        assert!(filter.should_ship("any line"));
        assert!(filter.should_ship("ERROR: something"));
    }

    #[test]
    fn test_match_only() {
        let filter = LogFilter::new(vec!["ERROR".to_string(), "WARN".to_string()], vec![]).unwrap();

        assert!(!filter.is_passthrough());
        assert!(filter.should_ship("ERROR: something bad"));
        assert!(filter.should_ship("WARN: watch out"));
        assert!(!filter.should_ship("INFO: all good"));
        assert!(!filter.should_ship("DEBUG: details"));
    }

    #[test]
    fn test_exclude_only() {
        let filter =
            LogFilter::new(vec![], vec!["DEBUG".to_string(), "TRACE".to_string()]).unwrap();

        assert!(!filter.is_passthrough());
        assert!(filter.should_ship("ERROR: something bad"));
        assert!(filter.should_ship("INFO: all good"));
        assert!(!filter.should_ship("DEBUG: details"));
        assert!(!filter.should_ship("TRACE: very verbose"));
    }

    #[test]
    fn test_match_and_exclude() {
        // Match ERROR/WARN, but exclude lines containing "ignore"
        let filter = LogFilter::new(
            vec!["ERROR".to_string(), "WARN".to_string()],
            vec!["ignore".to_string()],
        )
        .unwrap();

        assert!(filter.should_ship("ERROR: something bad"));
        assert!(filter.should_ship("WARN: watch out"));
        assert!(!filter.should_ship("ERROR: ignore this"));
        assert!(!filter.should_ship("WARN: please ignore"));
        assert!(!filter.should_ship("INFO: all good"));
    }

    #[test]
    fn test_regex_patterns() {
        // Match lines starting with timestamp pattern
        let filter = LogFilter::new(vec![r"^\[\d{4}-\d{2}-\d{2}".to_string()], vec![]).unwrap();

        assert!(filter.should_ship("[2025-12-03 14:23:45] Log message"));
        assert!(!filter.should_ship("Log message without timestamp"));
    }

    #[test]
    fn test_invalid_regex() {
        let result = LogFilter::new(vec!["[invalid".to_string()], vec![]);
        assert!(result.is_err());
    }
}
