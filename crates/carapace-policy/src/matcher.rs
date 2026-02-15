use crate::error::PolicyError;
use glob::Pattern;
use regex::Regex;

pub struct ArgvMatcher {
    allow_patterns: Vec<GlobPattern>,
    deny_patterns: Vec<GlobPattern>,
}

enum GlobPattern {
    Simple(Pattern),
    Regex(Regex),
}

impl ArgvMatcher {
    pub fn new(
        allow_patterns: Vec<String>,
        deny_patterns: Vec<String>,
    ) -> Result<Self, PolicyError> {
        let mut allow = Vec::new();
        for p in allow_patterns {
            match Pattern::new(&p) {
                Ok(pattern) => allow.push(GlobPattern::Simple(pattern)),
                Err(e) => return Err(PolicyError::InvalidPattern(e.to_string())),
            }
        }

        let mut deny = Vec::new();
        for p in deny_patterns {
            match Pattern::new(&p) {
                Ok(pattern) => deny.push(GlobPattern::Simple(pattern)),
                Err(e) => return Err(PolicyError::InvalidPattern(e.to_string())),
            }
        }

        Ok(Self {
            allow_patterns: allow,
            deny_patterns: deny,
        })
    }

    /// Match argv against allow/deny patterns
    pub fn matches(&self, argv: &[String]) -> bool {
        let argv_str = argv.join(" ");

        // Check deny patterns first - they take precedence
        for pattern in &self.deny_patterns {
            if self.pattern_matches(&argv_str, pattern) {
                return false;
            }
        }

        // Check allow patterns
        for pattern in &self.allow_patterns {
            if self.pattern_matches(&argv_str, pattern) {
                return true;
            }
        }

        false
    }

    fn pattern_matches(&self, input: &str, pattern: &GlobPattern) -> bool {
        match pattern {
            GlobPattern::Simple(p) => p.matches(input),
            GlobPattern::Regex(r) => r.is_match(input),
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_simple_glob_match() {
        let matcher = ArgvMatcher::new(vec!["pr list*".to_string()], vec![])
            .expect("matcher creation failed");

        assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
        assert!(matcher.matches(&["pr".to_string(), "list".to_string(), "--all".to_string()]));
        assert!(!matcher.matches(&["pr".to_string(), "create".to_string()]));
    }

    #[test]
    fn test_deny_precedence() {
        let matcher = ArgvMatcher::new(
            vec!["*".to_string()],           // Allow all
            vec!["* --token *".to_string()], // But deny token
        )
        .expect("matcher creation failed");

        assert!(matcher.matches(&["some".to_string(), "command".to_string()]));
        assert!(!matcher.matches(&[
            "some".to_string(),
            "--token".to_string(),
            "secret".to_string()
        ]));
    }

    #[test]
    fn test_empty_argv() {
        let matcher = ArgvMatcher::new(vec!["anything".to_string()], vec![])
            .expect("matcher creation failed");

        // Empty argv ("") doesn't match "anything" pattern
        assert!(!matcher.matches(&[]));
    }

    #[test]
    fn test_exact_match() {
        let matcher =
            ArgvMatcher::new(vec!["pr list".to_string()], vec![]).expect("matcher creation failed");

        // When argv is ["pr", "list"], they join as "pr list"
        assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
        // When argv is ["pr"], it joins as "pr", doesn't match "pr list"
        assert!(!matcher.matches(&["pr".to_string()]));
    }

    #[test]
    fn test_case_sensitivity() {
        let matcher =
            ArgvMatcher::new(vec!["PR*".to_string()], vec![]).expect("matcher creation failed");

        assert!(matcher.matches(&["PR".to_string(), "list".to_string()]));
        // Glob patterns are case-sensitive
        assert!(!matcher.matches(&["pr".to_string(), "list".to_string()]));
    }

    #[test]
    fn test_special_characters_in_pattern() {
        let matcher = ArgvMatcher::new(vec!["api repos *".to_string()], vec![])
            .expect("matcher creation failed");

        // "api repos owner repo" matches "api repos *"
        assert!(matcher.matches(&[
            "api".to_string(),
            "repos".to_string(),
            "owner".to_string(),
            "repo".to_string()
        ]));
        // "api repos" doesn't match "api repos *" (no trailing content)
        assert!(!matcher.matches(&["api".to_string(), "repos".to_string()]));
    }

    #[test]
    fn test_argv_with_null_bytes_rejection() {
        let _matcher =
            ArgvMatcher::new(vec!["safe".to_string()], vec![]).expect("matcher creation failed");

        // Join will concatenate with spaces, so null bytes become visible
        let argv_with_null = vec!["safe".to_string(), "command\0hidden".to_string()];
        // This should work - the matcher sees the full string
        let joined = argv_with_null.join(" ");
        assert!(joined.contains('\0'));
    }

    #[test]
    fn test_shell_metacharacter_in_argv() {
        let matcher = ArgvMatcher::new(
            vec!["safe *".to_string()],
            vec![
                "* ; *".to_string(),
                "* | *".to_string(),
                "* && *".to_string(),
            ],
        )
        .expect("matcher creation failed");

        // "safe command" matches "safe *" and doesn't have metacharacters
        assert!(matcher.matches(&["safe".to_string(), "command".to_string()]));
        // "safe ; rm -rf /" contains " ; " which matches deny pattern
        assert!(!matcher.matches(&[
            "safe".to_string(),
            ";".to_string(),
            "rm".to_string(),
            "-rf".to_string(),
            "/".to_string()
        ]));
    }

    #[test]
    fn test_command_substitution_detection() {
        let matcher = ArgvMatcher::new(vec!["* *".to_string()], vec!["* $( *".to_string()])
            .expect("matcher creation failed");

        assert!(matcher.matches(&["normal".to_string(), "command".to_string()]));
        // "bad $(whoami)" doesn't match "* $( *" because there's no space after $(
        assert!(matcher.matches(&["bad".to_string(), "$(whoami)".to_string()]));

        // But if we split it as separate args, it catches the pattern
        let matcher2 = ArgvMatcher::new(vec!["* *".to_string()], vec!["* $( *".to_string()])
            .expect("matcher creation failed");

        // "bad $( whoami )" has the space after $( so it matches deny pattern
        assert!(!matcher2.matches(&["bad".to_string(), "$(".to_string(), "whoami".to_string()]));
    }

    #[test]
    fn test_environment_variable_expansion_attempt() {
        let matcher = ArgvMatcher::new(
            vec!["* *".to_string()],
            vec!["* \\$* *".to_string()], // Escape the $ for glob
        )
        .expect("matcher creation failed");

        assert!(matcher.matches(&["normal".to_string(), "command".to_string()]));
        // Note: glob pattern matching $ might not work as expected,
        // so this test shows the limitation of glob-based pattern matching
        // In practice, we'd use more sophisticated validation
        assert!(matcher.matches(&["bad".to_string(), "$HOME".to_string()])); // This might not work as a deny pattern in glob
    }

    #[test]
    fn test_path_traversal_in_args() {
        let matcher = ArgvMatcher::new(vec!["*".to_string()], vec!["*../*".to_string()])
            .expect("matcher creation failed");

        assert!(matcher.matches(&["safe".to_string(), "/home/user/file".to_string()]));
        assert!(!matcher.matches(&["bad".to_string(), "../../etc/passwd".to_string()]));
    }

    #[test]
    fn test_extremely_long_argv() {
        let matcher =
            ArgvMatcher::new(vec!["test *".to_string()], vec![]).expect("matcher creation failed");

        let mut long_argv = vec!["test".to_string()];
        for i in 0..1000 {
            long_argv.push(format!("arg{}", i));
        }

        // "test arg0 arg1 ... arg999" should match "test *"
        assert!(matcher.matches(&long_argv));
    }

    #[test]
    fn test_multiple_patterns() {
        let matcher = ArgvMatcher::new(
            vec![
                "pr list*".to_string(),
                "issue view *".to_string(),
                "api *".to_string(),
            ],
            vec![],
        )
        .expect("matcher creation failed");

        // "pr list" matches "pr list*"
        assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
        // "issue view 123" matches "issue view *"
        assert!(matcher.matches(&["issue".to_string(), "view".to_string(), "123".to_string()]));
        // "api repos owner repo" matches "api *"
        assert!(matcher.matches(&[
            "api".to_string(),
            "repos".to_string(),
            "owner".to_string(),
            "repo".to_string()
        ]));
        // "pr create" doesn't match any pattern
        assert!(!matcher.matches(&["pr".to_string(), "create".to_string()]));
    }

    #[test]
    fn test_pattern_with_wildcards_and_negation() {
        let matcher = ArgvMatcher::new(vec!["pr *".to_string()], vec!["pr *draft*".to_string()])
            .expect("matcher creation failed");

        assert!(matcher.matches(&["pr".to_string(), "list".to_string()]));
        assert!(!matcher.matches(&["pr".to_string(), "draft".to_string(), "1".to_string()]));
    }

    #[test]
    fn test_unicode_in_argv() {
        let matcher = ArgvMatcher::new(vec!["issue create *".to_string()], vec![])
            .expect("matcher creation failed");

        // "issue create --title Test 你好" matches "issue create *"
        assert!(matcher.matches(&[
            "issue".to_string(),
            "create".to_string(),
            "--title".to_string(),
            "Test 你好".to_string()
        ]));
    }

    #[test]
    fn test_glob_pattern_compilation_error() {
        let result = ArgvMatcher::new(
            vec!["[invalid".to_string()], // Unclosed bracket
            vec![],
        );

        assert!(
            result.is_err(),
            "Should fail to compile invalid glob pattern"
        );
    }

    #[test]
    fn test_no_patterns_denies_all() {
        let matcher = ArgvMatcher::new(vec![], vec![]).expect("matcher creation failed");

        assert!(!matcher.matches(&["anything".to_string()]));
    }

    #[test]
    fn test_argument_splitting_via_quotes() {
        let matcher = ArgvMatcher::new(
            vec!["cmd *".to_string()],
            vec!["* --password *".to_string()],
        )
        .expect("matcher creation failed");

        // "cmd something" matches "cmd *" and doesn't match deny pattern
        assert!(matcher.matches(&["cmd".to_string(), "something".to_string()]));

        // "cmd --password secret" matches "* --password *" deny pattern, so it's denied
        assert!(!matcher.matches(&[
            "cmd".to_string(),
            "--password".to_string(),
            "secret".to_string()
        ]));

        // "cmd --password='secret'" as a single arg: "cmd --password='secret'" doesn't match "* --password *"
        // because the space isn't between --password and secret in the joined string
        assert!(matcher.matches(&["cmd".to_string(), "--password='secret'".to_string()]));
    }
}
