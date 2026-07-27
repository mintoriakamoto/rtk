//! Runs a command and produces a heuristic summary of its output.

use crate::core::guard::never_worse;
use crate::core::stream::exec_capture;
use crate::core::tracking;
use crate::core::truncate::CAP_WARNINGS;
use crate::core::utils::truncate;
use anyhow::{Context, Result};
use regex::Regex;
use std::process::Command;

const MAX_SUMMARY_LIST: usize = CAP_WARNINGS;
const MAX_SUMMARY_KEYS: usize = CAP_WARNINGS;

/// Run a command and provide a heuristic summary
pub fn run(command: &str, verbose: u8) -> Result<i32> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Running and summarizing: {}", command);
    }

    let mut cmd = if cfg!(target_os = "windows") {
        let mut c = Command::new("cmd");
        c.args(["/C", command]);
        c
    } else {
        let mut c = Command::new("sh");
        c.args(["-c", command]);
        c
    };
    let result = exec_capture(&mut cmd).context("Failed to execute command")?;

    let raw = format!("{}\n{}", result.stdout, result.stderr);

    let summary = summarize_output(&raw, command, result.success());
    let shown = never_worse(&raw, &summary);
    println!("{}", shown);
    timer.track(command, "rtk summary", &raw, shown);
    Ok(result.exit_code)
}

fn summarize_output(output: &str, command: &str, success: bool) -> String {
    let lines: Vec<&str> = output.lines().collect();
    let mut result = Vec::new();

    // Status
    let status_icon = if success { "[ok]" } else { "[FAIL]" };
    result.push(format!(
        "{} Command: {}",
        status_icon,
        truncate(command, 60)
    ));
    result.push(format!("   {} lines of output", lines.len()));
    result.push(String::new());

    // Detect type of output and summarize accordingly
    let output_type = detect_output_type(output, command);

    match output_type {
        OutputType::TestResults => summarize_tests(output, &mut result),
        OutputType::BuildOutput => summarize_build(output, &mut result),
        OutputType::LogOutput => summarize_logs_quick(output, &mut result),
        OutputType::ListOutput => summarize_list(output, &mut result),
        OutputType::JsonOutput => summarize_json(output, &mut result),
        OutputType::Generic => summarize_generic(output, &mut result),
    }

    result.join("\n")
}

#[derive(Debug)]
enum OutputType {
    TestResults,
    BuildOutput,
    LogOutput,
    ListOutput,
    JsonOutput,
    Generic,
}

fn detect_output_type(output: &str, command: &str) -> OutputType {
    let cmd_lower = command.to_lowercase();
    let out_lower = output.to_lowercase();

    if cmd_lower.contains("test") || out_lower.contains("passed") && out_lower.contains("failed") {
        OutputType::TestResults
    } else if cmd_lower.contains("build")
        || cmd_lower.contains("compile")
        || out_lower.contains("compiling")
    {
        OutputType::BuildOutput
    } else if out_lower.contains("error:")
        || out_lower.contains("warn:")
        || out_lower.contains("[info]")
    {
        OutputType::LogOutput
    } else if output.trim_start().starts_with('{') || output.trim_start().starts_with('[') {
        OutputType::JsonOutput
    } else if output.lines().all(|l| {
        l.len() < 200
            && if l.contains('\t') {
                false
            } else {
                l.split_whitespace().count() < 10
            }
    }) {
        OutputType::ListOutput
    } else {
        OutputType::Generic
    }
}

fn summarize_tests(output: &str, result: &mut Vec<String>) {
    result.push("Test Results:".to_string());

    let mut passed = 0;
    let mut failed = 0;
    let mut skipped = 0;
    let mut failures = Vec::new();

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("passed") || lower.contains("✓") || lower.contains("ok") {
            // Try to extract number
            if let Some(n) = extract_number(&lower, "passed") {
                passed = n;
            } else {
                passed += 1;
            }
        }
        if lower.contains("failed") || lower.contains("[x]") || lower.contains("fail") {
            if let Some(n) = extract_number(&lower, "failed") {
                failed = n;
            }
            if !line.contains("0 failed") {
                failures.push(line.to_string());
            }
        }
        if lower.contains("skipped") || lower.contains("ignored") {
            if let Some(n) = extract_number(&lower, "skipped").or(extract_number(&lower, "ignored"))
            {
                skipped = n;
            }
        }
    }

    result.push(format!("   [ok] {} passed", passed));
    if failed > 0 {
        result.push(format!("   [FAIL] {} failed", failed));
    }
    if skipped > 0 {
        result.push(format!("   skip {} skipped", skipped));
    }

    if !failures.is_empty() {
        result.push(String::new());
        result.push("   Failures:".to_string());
        for f in failures.iter().take(5) {
            result.push(format!("   • {}", truncate(f, 70)));
        }
    }
}

fn summarize_build(output: &str, result: &mut Vec<String>) {
    result.push("Build Summary:".to_string());

    let mut errors = 0;
    let mut warnings = 0;
    let mut compiled = 0;
    let mut error_msgs = Vec::new();

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") && !lower.contains("0 error") {
            errors += 1;
            if error_msgs.len() < 5 {
                error_msgs.push(line.to_string());
            }
        }
        if lower.contains("warning") && !lower.contains("0 warning") {
            warnings += 1;
        }
        if lower.contains("compiling") || lower.contains("compiled") {
            compiled += 1;
        }
    }

    if compiled > 0 {
        result.push(format!("   {} crates/files compiled", compiled));
    }
    if errors > 0 {
        result.push(format!("   [error] {} errors", errors));
    }
    if warnings > 0 {
        result.push(format!("   [warn] {} warnings", warnings));
    }
    if errors == 0 && warnings == 0 {
        result.push("   [ok] Build successful".to_string());
    }

    if !error_msgs.is_empty() {
        result.push(String::new());
        result.push("   Errors:".to_string());
        for e in &error_msgs {
            result.push(format!("   • {}", truncate(e, 70)));
        }
    }
}

fn summarize_logs_quick(output: &str, result: &mut Vec<String>) {
    result.push("Log Summary:".to_string());

    let mut errors = 0;
    let mut warnings = 0;
    let mut info = 0;

    for line in output.lines() {
        let lower = line.to_lowercase();
        if lower.contains("error") || lower.contains("fatal") {
            errors += 1;
        } else if lower.contains("warn") {
            warnings += 1;
        } else if lower.contains("info") {
            info += 1;
        }
    }

    result.push(format!("   [error] {} errors", errors));
    result.push(format!("   [warn] {} warnings", warnings));
    result.push(format!("   [info] {} info", info));
}

fn summarize_list(output: &str, result: &mut Vec<String>) {
    let lines: Vec<&str> = output.lines().filter(|l| !l.trim().is_empty()).collect();
    result.push(format!("List ({} items):", lines.len()));

    for line in lines.iter().take(MAX_SUMMARY_LIST) {
        result.push(format!("   • {}", truncate(line, 70)));
    }
    if lines.len() > MAX_SUMMARY_LIST {
        result.push(format!("   ... +{} more", lines.len() - MAX_SUMMARY_LIST));
    }
}

fn summarize_json(output: &str, result: &mut Vec<String>) {
    result.push("JSON Output:".to_string());

    // Try to parse and show structure
    if let Ok(value) = serde_json::from_str::<serde_json::Value>(output) {
        match &value {
            serde_json::Value::Array(arr) => {
                result.push(format!("   Array with {} items", arr.len()));
            }
            serde_json::Value::Object(obj) => {
                result.push(format!("   Object with {} keys:", obj.len()));
                for key in obj.keys().take(MAX_SUMMARY_KEYS) {
                    result.push(format!("   • {}", key));
                }
                if obj.len() > MAX_SUMMARY_KEYS {
                    result.push(format!("   ... +{} more keys", obj.len() - MAX_SUMMARY_KEYS));
                }
            }
            _ => {
                result.push(format!("   {}", truncate(&value.to_string(), 100)));
            }
        }
    } else {
        result.push("   (Invalid JSON)".to_string());
    }
}

fn summarize_generic(output: &str, result: &mut Vec<String>) {
    let lines: Vec<&str> = output.lines().collect();

    result.push("Output:".to_string());

    // First few lines
    for line in lines.iter().take(5) {
        if !line.trim().is_empty() {
            result.push(format!("   {}", truncate(line, 75)));
        }
    }

    if lines.len() > 10 {
        result.push("   ...".to_string());
        // Last few lines
        for line in lines.iter().skip(lines.len() - 3) {
            if !line.trim().is_empty() {
                result.push(format!("   {}", truncate(line, 75)));
            }
        }
    }
}

fn extract_number(text: &str, after: &str) -> Option<usize> {
    let re = Regex::new(&format!(r"(\d+)\s*{}", after)).ok()?;
    re.captures(text)
        .and_then(|c| c.get(1))
        .and_then(|m| m.as_str().parse().ok())
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_extract_number() {
        assert_eq!(extract_number("42 passed; 0 failed", "passed"), Some(42));
        assert_eq!(extract_number("42 passed; 0 failed", "failed"), Some(0));
        assert_eq!(extract_number("no numbers here", "passed"), None);
    }

    #[test]
    fn test_test_results_summary() {
        let input = "running 3 tests\ntest a ... ok\ntest b ... ok\ntest c ... ok\ntest result: ok. 3 passed; 0 failed; 1 ignored; 0 measured\n";
        let out = summarize_output(input, "cargo test", true);
        assert!(out.contains("[ok] Command: cargo test"));
        assert!(out.contains("Test Results:"));
        assert!(out.contains("[ok] 3 passed"));
        assert!(out.contains("skip 1 skipped"));
        assert!(!out.contains("[FAIL]"));
    }

    #[test]
    fn test_build_summary_with_errors() {
        let input = "Compiling demo v0.1.0\nerror[E0308]: mismatched types\n --> src/main.rs:4:5\nwarning: unused variable: `x`\n";
        let out = summarize_output(input, "cargo build", false);
        assert!(out.contains("[FAIL] Command: cargo build"));
        assert!(out.contains("Build Summary:"));
        assert!(out.contains("[error] 1 errors"));
        assert!(out.contains("[warn] 1 warnings"));
        assert!(out.contains("error[E0308]: mismatched types"));
    }

    #[test]
    fn test_json_object_summary() {
        let input = r#"{"name": "demo", "version": "1.0.0", "private": true}"#;
        let out = summarize_output(input, "cat package.json", true);
        assert!(out.contains("JSON Output:"));
        assert!(out.contains("Object with 3 keys:"));
        assert!(out.contains("• name"));
    }

    #[test]
    fn test_list_summary_truncates() {
        let input = (0..25).map(|i| format!("item-{}\n", i)).collect::<String>();
        let out = summarize_output(&input, "ls", true);
        assert!(out.contains("List (25 items):"));
        assert!(out.contains(&format!("... +{} more", 25 - MAX_SUMMARY_LIST)));
    }

    #[test]
    fn test_empty_output_does_not_panic() {
        let out = summarize_output("", "true", true);
        assert!(out.contains("0 lines of output"));
    }

    #[test]
    fn test_token_savings_on_test_output() {
        let mut input = String::from("running 40 tests\n");
        for i in 0..40 {
            input.push_str(&format!("test module::case_{} ... ok\n", i));
        }
        input.push_str("test result: ok. 40 passed; 0 failed; 0 ignored; 0 measured; 0 filtered out\n");
        let out = summarize_output(&input, "cargo test", true);
        let savings = 100.0 - (count_tokens(&out) as f64 / count_tokens(&input) as f64 * 100.0);
        assert!(savings >= 60.0, "Expected >=60% savings, got {:.1}%", savings);
    }
}
