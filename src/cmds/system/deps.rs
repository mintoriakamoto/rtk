//! Summarizes project dependencies from lock files and manifests.

use crate::core::guard::never_worse;
use crate::core::tracking;
use crate::core::truncate::{reduced, CAP_WARNINGS};
use anyhow::Result;
use regex::Regex;
use std::fs;
use std::path::Path;
use std::sync::LazyLock;

const MAX_DEPS: usize = CAP_WARNINGS;
// dev deps are secondary to prod — show fewer.
const MAX_DEV_DEPS: usize = reduced(CAP_WARNINGS, 5);

static CARGO_DEP_RE: LazyLock<Regex> = LazyLock::new(|| {
    Regex::new(r#"^([a-zA-Z0-9_-]+)\s*=\s*(?:"([^"]+)"|.*version\s*=\s*"([^"]+)")"#).unwrap()
});
static CARGO_SECTION_RE: LazyLock<Regex> = LazyLock::new(|| Regex::new(r"^\[([^\]]+)\]").unwrap());
static REQUIREMENTS_DEP_RE: LazyLock<Regex> =
    LazyLock::new(|| Regex::new(r"^([a-zA-Z0-9_-]+)([=<>!~]+.*)?$").unwrap());

/// Summarize project dependencies
pub fn run(path: &Path, verbose: u8) -> Result<()> {
    let timer = tracking::TimedExecution::start();

    let dir = if path.is_file() {
        path.parent().unwrap_or(Path::new("."))
    } else {
        path
    };

    if verbose > 0 {
        eprintln!("Scanning dependencies in: {}", dir.display());
    }

    let mut found = false;
    let mut rtk = String::new();
    let mut raw = String::new();

    let cargo_path = dir.join("Cargo.toml");
    if cargo_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&cargo_path).unwrap_or_default());
        rtk.push_str("Rust (Cargo.toml):\n");
        rtk.push_str(&summarize_cargo_str(&cargo_path)?);
    }

    let package_path = dir.join("package.json");
    if package_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&package_path).unwrap_or_default());
        rtk.push_str("Node.js (package.json):\n");
        rtk.push_str(&summarize_package_json_str(&package_path)?);
    }

    let requirements_path = dir.join("requirements.txt");
    if requirements_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&requirements_path).unwrap_or_default());
        rtk.push_str("Python (requirements.txt):\n");
        rtk.push_str(&summarize_requirements_str(&requirements_path)?);
    }

    let pyproject_path = dir.join("pyproject.toml");
    if pyproject_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&pyproject_path).unwrap_or_default());
        rtk.push_str("Python (pyproject.toml):\n");
        rtk.push_str(&summarize_pyproject_str(&pyproject_path)?);
    }

    let gomod_path = dir.join("go.mod");
    if gomod_path.exists() {
        found = true;
        raw.push_str(&fs::read_to_string(&gomod_path).unwrap_or_default());
        rtk.push_str("Go (go.mod):\n");
        rtk.push_str(&summarize_gomod_str(&gomod_path)?);
    }

    if !found {
        rtk.push_str(&format!("No dependency files found in {}", dir.display()));
    }

    let shown = never_worse(&raw, &rtk);
    print!("{}", shown);
    timer.track("cat */deps", "rtk deps", &raw, shown);
    Ok(())
}

fn summarize_cargo_str(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    Ok(summarize_cargo_content(&content))
}

fn summarize_cargo_content(content: &str) -> String {
    let mut current_section = String::new();
    let mut deps = Vec::new();
    let mut dev_deps = Vec::new();
    let mut out = String::new();

    for line in content.lines() {
        if let Some(caps) = CARGO_SECTION_RE.captures(line) {
            current_section = caps
                .get(1)
                .map(|m| m.as_str().to_string())
                .unwrap_or_default();
        } else if let Some(caps) = CARGO_DEP_RE.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps
                .get(2)
                .or(caps.get(3))
                .map(|m| m.as_str())
                .unwrap_or("*");
            let dep = format!("{} ({})", name, version);
            match current_section.as_str() {
                "dependencies" => deps.push(dep),
                "dev-dependencies" => dev_deps.push(dep),
                _ => {}
            }
        }
    }

    if !deps.is_empty() {
        out.push_str(&format!("  Dependencies ({}):\n", deps.len()));
        for d in deps.iter().take(MAX_DEPS) {
            out.push_str(&format!("    {}\n", d));
        }
        if deps.len() > MAX_DEPS {
            out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
        }
    }
    if !dev_deps.is_empty() {
        out.push_str(&format!("  Dev ({}):\n", dev_deps.len()));
        for d in dev_deps.iter().take(MAX_DEV_DEPS) {
            out.push_str(&format!("    {}\n", d));
        }
        if dev_deps.len() > MAX_DEV_DEPS {
            out.push_str(&format!("    ... +{} more\n", dev_deps.len() - MAX_DEV_DEPS));
        }
    }
    out
}

fn summarize_package_json_str(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    summarize_package_json_content(&content)
}

fn summarize_package_json_content(content: &str) -> Result<String> {
    let json: serde_json::Value = serde_json::from_str(content)?;
    let mut out = String::new();

    if let Some(name) = json.get("name").and_then(|v| v.as_str()) {
        let version = json.get("version").and_then(|v| v.as_str()).unwrap_or("?");
        out.push_str(&format!("  {} @ {}\n", name, version));
    }
    if let Some(deps) = json.get("dependencies").and_then(|v| v.as_object()) {
        out.push_str(&format!("  Dependencies ({}):\n", deps.len()));
        for (i, (name, version)) in deps.iter().enumerate() {
            if i >= MAX_DEPS {
                out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
                break;
            }
            out.push_str(&format!(
                "    {} ({})\n",
                name,
                version.as_str().unwrap_or("*")
            ));
        }
    }
    if let Some(dev_deps) = json.get("devDependencies").and_then(|v| v.as_object()) {
        out.push_str(&format!("  Dev Dependencies ({}):\n", dev_deps.len()));
        for (i, (name, _)) in dev_deps.iter().enumerate() {
            if i >= MAX_DEV_DEPS {
                out.push_str(&format!("    ... +{} more\n", dev_deps.len() - MAX_DEV_DEPS));
                break;
            }
            out.push_str(&format!("    {}\n", name));
        }
    }
    Ok(out)
}

fn summarize_requirements_str(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    Ok(summarize_requirements_content(&content))
}

fn summarize_requirements_content(content: &str) -> String {
    let mut deps = Vec::new();
    let mut out = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        if let Some(caps) = REQUIREMENTS_DEP_RE.captures(line) {
            let name = caps.get(1).map(|m| m.as_str()).unwrap_or("");
            let version = caps.get(2).map(|m| m.as_str()).unwrap_or("");
            deps.push(format!("{}{}", name, version));
        }
    }

    out.push_str(&format!("  Packages ({}):\n", deps.len()));
    for d in deps.iter().take(MAX_DEPS) {
        out.push_str(&format!("    {}\n", d));
    }
    if deps.len() > MAX_DEPS {
        out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
    }
    out
}

fn summarize_pyproject_str(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    Ok(summarize_pyproject_content(&content))
}

fn summarize_pyproject_content(content: &str) -> String {
    let mut in_deps = false;
    let mut deps = Vec::new();
    let mut out = String::new();

    for line in content.lines() {
        if line.contains("dependencies") && line.contains("[") {
            in_deps = true;
            continue;
        }
        if in_deps {
            if line.trim() == "]" {
                break;
            }
            let line = line
                .trim()
                .trim_matches(|c| c == '"' || c == '\'' || c == ',');
            if !line.is_empty() {
                deps.push(line.to_string());
            }
        }
    }

    if !deps.is_empty() {
        out.push_str(&format!("  Dependencies ({}):\n", deps.len()));
        for d in deps.iter().take(MAX_DEPS) {
            out.push_str(&format!("    {}\n", d));
        }
        if deps.len() > MAX_DEPS {
            out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
        }
    }
    out
}

fn summarize_gomod_str(path: &Path) -> Result<String> {
    let content = fs::read_to_string(path)?;
    Ok(summarize_gomod_content(&content))
}

fn summarize_gomod_content(content: &str) -> String {
    let mut module_name = String::new();
    let mut go_version = String::new();
    let mut deps = Vec::new();
    let mut in_require = false;
    let mut out = String::new();

    for line in content.lines() {
        let line = line.trim();
        if line.starts_with("module ") {
            module_name = line.trim_start_matches("module ").to_string();
        } else if line.starts_with("go ") {
            go_version = line.trim_start_matches("go ").to_string();
        } else if line == "require (" {
            in_require = true;
        } else if line == ")" {
            in_require = false;
        } else if in_require && !line.starts_with("//") {
            let parts: Vec<&str> = line.split_whitespace().collect();
            if parts.len() >= 2 {
                deps.push(format!("{} {}", parts[0], parts[1]));
            }
        } else if line.starts_with("require ") && !line.contains("(") {
            deps.push(line.trim_start_matches("require ").to_string());
        }
    }

    if !module_name.is_empty() {
        out.push_str(&format!("  {} (go {})\n", module_name, go_version));
    }
    if !deps.is_empty() {
        out.push_str(&format!("  Dependencies ({}):\n", deps.len()));
        for d in deps.iter().take(MAX_DEPS) {
            out.push_str(&format!("    {}\n", d));
        }
        if deps.len() > MAX_DEPS {
            out.push_str(&format!("    ... +{} more\n", deps.len() - MAX_DEPS));
        }
    }
    out
}

#[cfg(test)]
mod tests {
    use super::*;

    fn count_tokens(s: &str) -> usize {
        s.split_whitespace().count()
    }

    #[test]
    fn test_cargo_sections_and_versions() {
        let input = r#"[package]
name = "rtk"
version = "0.42.4"
edition = "2021"

[dependencies]
clap = { version = "4", features = ["derive"] }
anyhow = "1.0"
regex = "1"

[dev-dependencies]
tempfile = "3"

[profile.release]
opt-level = 3
"#;
        let out = summarize_cargo_content(input);
        assert!(out.contains("Dependencies (3):"));
        assert!(out.contains("clap (4)"));
        assert!(out.contains("anyhow (1.0)"));
        assert!(out.contains("Dev (1):"));
        assert!(out.contains("tempfile (3)"));
        // [package] and [profile.release] entries must not leak into deps
        assert!(!out.contains("opt-level"));
        assert!(!out.contains("edition"));
    }

    #[test]
    fn test_cargo_truncates_past_cap() {
        let mut input = String::from("[dependencies]\n");
        for i in 0..(MAX_DEPS + 7) {
            input.push_str(&format!("crate{} = \"1.0\"\n", i));
        }
        let out = summarize_cargo_content(&input);
        assert!(out.contains(&format!("Dependencies ({}):", MAX_DEPS + 7)));
        assert!(out.contains("... +7 more"));
    }

    #[test]
    fn test_requirements_versions_and_comments() {
        let input = "# pinned deps\nrequests==2.31.0\nflask>=2.0\nnumpy\n\n";
        let out = summarize_requirements_content(input);
        assert!(out.contains("Packages (3):"));
        assert!(out.contains("requests==2.31.0"));
        assert!(out.contains("flask>=2.0"));
        assert!(out.contains("numpy"));
    }

    #[test]
    fn test_package_json_deps_and_dev() {
        let input = r#"{
  "name": "demo",
  "version": "1.2.3",
  "dependencies": {"react": "^18.0.0", "next": "14.1.0"},
  "devDependencies": {"vitest": "^1.0.0"}
}"#;
        let out = summarize_package_json_content(input).expect("valid json");
        assert!(out.contains("demo @ 1.2.3"));
        assert!(out.contains("Dependencies (2):"));
        assert!(out.contains("react (^18.0.0)"));
        assert!(out.contains("Dev Dependencies (1):"));
        assert!(summarize_package_json_content("not json").is_err());
    }

    #[test]
    fn test_pyproject_dependency_array() {
        let input = "[project]\nname = \"demo\"\ndependencies = [\n    \"requests>=2.0\",\n    \"click\",\n]\n";
        let out = summarize_pyproject_content(input);
        assert!(out.contains("Dependencies (2):"));
        assert!(out.contains("requests>=2.0"));
        assert!(out.contains("click"));
    }

    #[test]
    fn test_gomod_module_and_requires() {
        let input = "module github.com/acme/api\n\ngo 1.22\n\nrequire (\n\tgithub.com/gin-gonic/gin v1.9.1\n\tgolang.org/x/sync v0.6.0\n)\n";
        let out = summarize_gomod_content(input);
        assert!(out.contains("github.com/acme/api (go 1.22)"));
        assert!(out.contains("Dependencies (2):"));
        assert!(out.contains("github.com/gin-gonic/gin v1.9.1"));
    }

    #[test]
    fn test_empty_inputs_do_not_panic() {
        assert_eq!(summarize_cargo_content(""), "");
        assert_eq!(summarize_requirements_content(""), "  Packages (0):\n");
        assert_eq!(summarize_pyproject_content(""), "");
        assert_eq!(summarize_gomod_content(""), "");
    }

    #[test]
    fn test_cargo_token_savings() {
        // Realistic manifest: metadata + long dep list, summary caps at MAX_DEPS.
        let mut input = String::from(
            "[package]\nname = \"demo\"\nversion = \"1.0.0\"\nedition = \"2021\"\nauthors = [\"Dev Name\"]\ndescription = \"A demo application with a long description field\"\nlicense = \"MIT\"\nrepository = \"https://github.com/acme/demo\"\nkeywords = [\"cli\", \"demo\", \"example\"]\n\n[dependencies]\n",
        );
        for i in 0..30 {
            input.push_str(&format!(
                "crate{} = {{ version = \"1.{}.0\", features = [\"default\", \"extra\"] }}\n",
                i, i
            ));
        }
        input.push_str("\n[profile.release]\nopt-level = 3\nlto = true\ncodegen-units = 1\n");
        let out = summarize_cargo_content(&input);
        let savings =
            100.0 - (count_tokens(&out) as f64 / count_tokens(&input) as f64 * 100.0);
        assert!(savings >= 60.0, "Expected >=60% savings, got {:.1}%", savings);
    }
}
