use crate::core::utils::{composer_tool_paths, resolve_binary, resolved_command};
use regex::Regex;
use std::path::Path;
use std::process::Command;
use std::sync::LazyLock;

static ANSI_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"\x1b\[[0-9;]*[A-Za-z]").unwrap());
static CONTROL_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"[\x00-\x08\x0B\x0C\x0E-\x1F\x7F]").unwrap());

pub fn php_tool_command(tool: &str) -> Command {
    for local_tool in composer_tool_paths(tool) {
        let local_tool_name = local_tool.to_string_lossy().into_owned();
        // Route through resolved_command (the sanctioned constructor) rather than
        // a raw dynamic command constructor, so the binary still resolves
        // PATHEXT-aware on Windows and the security scan's no-dynamic-exec rule holds.
        if resolve_binary(&local_tool_name).is_ok() || local_tool.exists() {
            return resolved_command(&local_tool_name);
        }
    }

    resolved_command(tool)
}

fn composer_tool_exists(tool: &str) -> bool {
    composer_tool_paths(tool).into_iter().any(|local_tool| {
        let local_tool_name = local_tool.to_string_lossy().into_owned();
        resolve_binary(&local_tool_name).is_ok() || local_tool.exists()
    })
}

pub fn strip_ansi_and_controls(input: &str) -> String {
    let no_ansi = ANSI_RE.replace_all(input, "");
    CONTROL_RE.replace_all(&no_ansi, "").to_string()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PhpTestRunner {
    Pest,
    Phpunit,
    Unknown,
}

pub fn detect_php_test_runner() -> PhpTestRunner {
    // Pest's canonical marker is the `vendor/bin/pest` binary (composer dep).
    // There is no root `pest.php` file — Pest's bootstrap lives at `tests/Pest.php`
    // — so a root-level `pest.php` check both never matches Pest and false-positives
    // on unrelated utility files in PHPUnit-only projects.
    if composer_tool_exists("pest") {
        return PhpTestRunner::Pest;
    }

    if composer_tool_exists("phpunit")
        || Path::new("phpunit.xml").exists()
        || Path::new("phpunit.xml.dist").exists()
    {
        return PhpTestRunner::Phpunit;
    }

    PhpTestRunner::Unknown
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_strip_ansi_colors() {
        let input = "\x1b[32mPASS\x1b[0m Tests\\Unit\\ExampleTest";
        assert_eq!(strip_ansi_and_controls(input), "PASS Tests\\Unit\\ExampleTest");
    }

    #[test]
    fn test_strip_cursor_movement_sequences() {
        let input = "\x1b[2K\x1b[1A\x1b[2K  Running tests...";
        assert_eq!(strip_ansi_and_controls(input), "  Running tests...");
    }

    #[test]
    fn test_strip_control_chars_keeps_newlines_and_tabs() {
        let input = "line1\nline2\tcol\x07\x08\x00done";
        assert_eq!(strip_ansi_and_controls(input), "line1\nline2\tcoldone");
    }

    #[test]
    fn test_plain_and_unicode_text_unchanged() {
        assert_eq!(strip_ansi_and_controls(""), "");
        assert_eq!(strip_ansi_and_controls("✓ réussi — 3 tests"), "✓ réussi — 3 tests");
    }
}
