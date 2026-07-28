//! Deduplicates repeated log lines and shows counts instead.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::truncate::{reduced, CAP_WARNINGS};
use anyhow::Result;
use regex::Regex;
use std::collections::HashMap;
use std::fs;
use std::io::{self, BufRead};
use std::path::Path;
use std::sync::LazyLock;

static TIMESTAMP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"^\d{4}[-/]\d{2}[-/]\d{2}[T ]\d{2}:\d{2}:\d{2}[.,]?\d*\s*").unwrap()
});
static UUID_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"[0-9a-fA-F]{8}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{4}-[0-9a-fA-F]{12}")
        .unwrap()
});
static HEX_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"0x[0-9a-fA-F]+").unwrap());
static NUM_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\b\d{4,}\b").unwrap());
static PATH_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"/[\w./\-]+").unwrap());

// Severity keywords, matched case-insensitively in a single pass per line
// (avoids allocating a lowercased copy of every line). The error set also
// covers labels above ERROR (CRITICAL, FATAL, ALERT, EMERGENCY, SEVERE,
// PANIC) — the most important lines in a log.
static ERROR_KW_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r"(?i)error|fatal|panic|critical|alert|emerg|severe").unwrap()
});
static WARN_KW_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)warn|notice").unwrap());
static INFO_KW_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"(?i)info").unwrap());

/// Filter and deduplicate log output
pub fn run_file(file: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    if verbose > 0 {
        eprintln!("Analyzing log: {}", file.display());
    }

    let content = fs::read_to_string(file)?;
    let result = analyze_logs(&content);
    let shown = never_worse(&content, &result);
    println!("{}", shown);
    timer.track(
        &format!("cat {}", file.display()),
        "rtk log",
        &content,
        shown,
    );
    Ok(())
}

/// Filter logs from stdin
pub fn run_stdin(_verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let mut content = String::new();
    let stdin = io::stdin();
    for line in stdin.lock().lines() {
        content.push_str(&line?);
        content.push('\n');
    }

    let result = analyze_logs(&content);
    let shown = never_worse(&content, &result);
    println!("{}", shown);

    timer.track("log (stdin)", "rtk log (stdin)", &content, shown);

    Ok(())
}

/// For use by other modules
pub fn run_stdin_str(content: &str) -> String {
    analyze_logs(content)
}

fn analyze_logs(content: &str) -> String {
    let mut result = Vec::new();
    // normalized line -> (count, first original line)
    let mut error_counts: HashMap<String, (usize, &str)> = HashMap::new();
    let mut warn_counts: HashMap<String, (usize, &str)> = HashMap::new();
    let mut info_counts: HashMap<String, usize> = HashMap::new();

    for line in content.lines() {
        // Categorize first — uncategorized lines (the majority in most logs)
        // skip normalization entirely.
        let is_error = ERROR_KW_RE.is_match(line);
        let is_warn = !is_error && WARN_KW_RE.is_match(line);
        if !is_error && !is_warn && !INFO_KW_RE.is_match(line) {
            continue;
        }

        // Normalize for deduplication
        let normalized = normalize_log_line(line);

        if is_error {
            error_counts.entry(normalized).or_insert((0, line)).0 += 1;
        } else if is_warn {
            warn_counts.entry(normalized).or_insert((0, line)).0 += 1;
        } else {
            *info_counts.entry(normalized).or_insert(0) += 1;
        }
    }

    // Summary
    let total_errors: usize = error_counts.values().map(|(c, _)| c).sum();
    let total_warnings: usize = warn_counts.values().map(|(c, _)| c).sum();
    let total_info: usize = info_counts.values().sum();

    result.push("Log Summary".to_string());
    result.push(format!(
        "   [error] {} errors ({} unique)",
        total_errors,
        error_counts.len()
    ));
    result.push(format!(
        "   [warn] {} warnings ({} unique)",
        total_warnings,
        warn_counts.len()
    ));
    result.push(format!("   [info] {} info messages", total_info));
    result.push(String::new());

    // Errors with counts
    if !error_counts.is_empty() {
        result.push("[ERRORS]".to_string());

        // Sort by count
        let mut error_list: Vec<_> = error_counts.iter().collect();
        error_list.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

        const MAX_LOG_ERRORS: usize = CAP_WARNINGS;
        for (_, (count, original)) in error_list.iter().take(MAX_LOG_ERRORS) {
            let truncated = if original.len() > 100 {
                let t: String = original.chars().take(97).collect();
                format!("{}...", t)
            } else {
                original.to_string()
            };

            if *count > 1 {
                result.push(format!("   [×{}] {}", count, truncated));
            } else {
                result.push(format!("   {}", truncated));
            }
        }

        if error_list.len() > MAX_LOG_ERRORS {
            result.push(format!(
                "   ... +{} more unique errors",
                error_list.len() - MAX_LOG_ERRORS
            ));
        }
        result.push(String::new());
    }

    // Warnings with counts
    if !warn_counts.is_empty() {
        result.push("[WARNINGS]".to_string());

        let mut warn_list: Vec<_> = warn_counts.iter().collect();
        warn_list.sort_by(|a, b| b.1 .0.cmp(&a.1 .0));

        // warnings are lower severity than errors — show fewer.
        const MAX_LOG_WARNS: usize = reduced(CAP_WARNINGS, 5);
        for (_, (count, original)) in warn_list.iter().take(MAX_LOG_WARNS) {
            let truncated = if original.len() > 100 {
                let t: String = original.chars().take(97).collect();
                format!("{}...", t)
            } else {
                original.to_string()
            };

            if *count > 1 {
                result.push(format!("   [×{}] {}", count, truncated));
            } else {
                result.push(format!("   {}", truncated));
            }
        }

        if warn_list.len() > MAX_LOG_WARNS {
            result.push(format!(
                "   ... +{} more unique warnings",
                warn_list.len() - MAX_LOG_WARNS
            ));
        }
    }

    result.join("\n")
}

fn normalize_log_line(line: &str) -> String {
    // Chain Cow results — a pattern that doesn't match costs no allocation.
    let s = TIMESTAMP_RE.replace_all(line, "");
    let s = UUID_RE.replace_all(&s, "<UUID>");
    let s = HEX_RE.replace_all(&s, "<HEX>");
    let s = NUM_RE.replace_all(&s, "<NUM>");
    let s = PATH_RE.replace_all(&s, "<PATH>");
    s.trim().to_string()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_analyze_logs() {
        let logs = r#"
2024-01-01 10:00:00 ERROR: Connection failed to /api/server
2024-01-01 10:00:01 ERROR: Connection failed to /api/server
2024-01-01 10:00:02 ERROR: Connection failed to /api/server
2024-01-01 10:00:03 WARN: Retrying connection
2024-01-01 10:00:04 INFO: Connected
"#;
        let result = analyze_logs(logs);
        assert!(result.contains("×3"));
        assert!(result.contains("ERRORS"));
    }

    #[test]
    fn test_analyze_logs_extended_severity_keywords() {
        let logs = "2024-01-01 10:00:00 CRITICAL: disk full\n\
                    2024-01-01 10:00:01 ALERT: memory pressure\n\
                    2024-01-01 10:00:02 emerg: system shutdown imminent\n\
                    2024-01-01 10:00:03 SEVERE: data corruption detected\n\
                    2024-01-01 10:00:04 notice: config reloaded\n";
        let result = analyze_logs(logs);
        assert!(result.contains("ERRORS"), "critical/alert/emerg/severe should count as errors");
        assert!(result.contains("WARNINGS"), "notice should count as warning");
    }

    #[test]
    fn test_analyze_logs_multibyte() {
        let logs = format!(
            "2024-01-01 10:00:00 ERROR: {} connection failed\n\
             2024-01-01 10:00:01 WARN: {} retry attempt\n",
            "ข้อผิดพลาด".repeat(15),
            "คำเตือน".repeat(15)
        );
        let result = analyze_logs(&logs);
        // Should not panic even with very long multi-byte messages
        assert!(result.contains("ERRORS"));
    }
}
