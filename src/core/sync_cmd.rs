//! Uploads local savings aggregates to an rtk portal (`rtk portal ...`).
//!
//! The portal bills a share of measured savings, so this is the metering link
//! between the local tracking DB and the customer's invoice. Only per-day
//! aggregates leave the machine — never commands, paths, or output.

use anyhow::{bail, Context, Result};
use clap::Subcommand;
use serde::{Deserialize, Serialize};
use std::io::Write as IoWrite;
use std::path::PathBuf;

use crate::core::tracking::{DayStats, Tracker};

/// Days of history uploaded when `--since` is not given. Re-uploading a day is
/// safe (the portal upserts on `[device, date]`), so a rolling window keeps
/// late-arriving or corrected days accurate without unbounded payloads.
pub const DEFAULT_SINCE_DAYS: i64 = 30;

/// The portal caps a single request at 400 days.
const MAX_DAYS_PER_REQUEST: usize = 400;

const HTTP_TIMEOUT_SECS: u64 = 15;

#[derive(Subcommand, Debug)]
pub enum PortalSubcommand {
    /// Store portal URL + device token for this machine
    Login {
        /// Portal base URL, e.g. https://portal.example.com
        #[arg(long)]
        url: String,
        /// Device token issued by the portal's Settings page.
        /// Falls back to the RTK_DEVICE_TOKEN env var.
        #[arg(long)]
        token: Option<String>,
    },
    /// Forget the stored portal credentials on this machine
    Logout,
    /// Show which portal this machine reports to and when it last synced
    Status,
    /// Upload savings aggregates to the portal
    Sync {
        /// Upload the last N days of history
        #[arg(long, default_value_t = DEFAULT_SINCE_DAYS)]
        since: i64,
        /// Print the exact payload without sending it
        #[arg(long)]
        dry_run: bool,
    },
}

/// Credentials live outside config.toml: the token is a secret and the file is
/// written 0600, whereas config.toml is routinely shared/committed.
#[derive(Debug, Default, Serialize, Deserialize)]
pub struct Credentials {
    pub portal_url: String,
    pub device_token: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub last_sync: Option<String>,
}

pub fn credentials_path() -> Result<PathBuf> {
    let dir = dirs::config_dir().context("Could not resolve the user config directory")?;
    Ok(dir.join("rtk").join("credentials.toml"))
}

pub fn load_credentials() -> Result<Credentials> {
    let path = credentials_path()?;
    let raw = std::fs::read_to_string(&path).with_context(|| {
        format!(
            "Not logged in to a portal (no {}). Run `rtk portal login --url <URL> --token <TOKEN>`",
            path.display()
        )
    })?;
    toml::from_str(&raw).with_context(|| format!("Failed to parse {}", path.display()))
}

fn save_credentials(creds: &Credentials) -> Result<()> {
    let path = credentials_path()?;
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)
            .with_context(|| format!("Failed to create {}", parent.display()))?;
    }
    let body = toml::to_string_pretty(creds).context("Failed to serialize credentials")?;

    // Create with 0600 from the start — writing then chmod'ing would leave the
    // token world-readable for a moment.
    let mut opts = std::fs::OpenOptions::new();
    opts.write(true).create(true).truncate(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        opts.mode(0o600);
    }
    let mut file = opts
        .open(&path)
        .with_context(|| format!("Failed to write {}", path.display()))?;
    file.write_all(body.as_bytes())
        .with_context(|| format!("Failed to write {}", path.display()))?;
    Ok(())
}

/// One day of aggregates in the portal's wire format.
#[derive(Debug, Serialize, PartialEq)]
pub struct SyncDay {
    pub date: String,
    pub commands: usize,
    pub input_tokens: usize,
    pub output_tokens: usize,
    pub saved_tokens: usize,
}

#[derive(Debug, Serialize)]
struct SyncPayload<'a> {
    days: &'a [SyncDay],
}

impl From<&DayStats> for SyncDay {
    fn from(d: &DayStats) -> Self {
        Self {
            date: d.date.clone(),
            commands: d.commands,
            input_tokens: d.input_tokens,
            output_tokens: d.output_tokens,
            saved_tokens: d.saved_tokens,
        }
    }
}

/// Select the days to upload: those on/after `cutoff`, newest-capped to the
/// portal's per-request limit.
///
/// Kept free of I/O so the windowing rules stay unit-testable.
pub fn select_days(days: &[DayStats], cutoff: &str) -> Vec<SyncDay> {
    let mut selected: Vec<SyncDay> = days
        .iter()
        .filter(|d| d.date.as_str() >= cutoff)
        .map(SyncDay::from)
        .collect();

    // Keep the most recent window if the range is huge — dropping the oldest
    // days is right, since those were uploaded by earlier syncs.
    if selected.len() > MAX_DAYS_PER_REQUEST {
        selected.sort_by(|a, b| a.date.cmp(&b.date));
        selected = selected.split_off(selected.len() - MAX_DAYS_PER_REQUEST);
    }
    selected
}

pub fn cutoff_date(since_days: i64) -> String {
    (chrono::Utc::now() - chrono::Duration::days(since_days))
        .format("%Y-%m-%d")
        .to_string()
}

pub fn run_subcommand(cmd: &PortalSubcommand, verbose: u8) -> Result<i32> {
    match cmd {
        PortalSubcommand::Login { url, token } => {
            let token = token
                .clone()
                .or_else(|| std::env::var("RTK_DEVICE_TOKEN").ok())
                .filter(|t| !t.trim().is_empty())
                .context(
                    "No device token. Pass --token <TOKEN> or set RTK_DEVICE_TOKEN. \
                     Create one on the portal's Settings page.",
                )?;

            let portal_url = url.trim_end_matches('/').to_string();
            if !portal_url.starts_with("http://") && !portal_url.starts_with("https://") {
                bail!("Portal URL must start with http:// or https:// (got {portal_url})");
            }

            save_credentials(&Credentials {
                portal_url: portal_url.clone(),
                device_token: token.trim().to_string(),
                last_sync: None,
            })?;
            println!("Logged in to {portal_url}");
            println!("Credentials: {}", credentials_path()?.display());
            println!("Next: `rtk portal sync` to upload your savings history.");
            Ok(0)
        }
        PortalSubcommand::Logout => {
            let path = credentials_path()?;
            // nosemgrep: filesystem-deletion -- logout removes only rtk's own
            // credentials file. The path is derived from the config dir and a
            // fixed filename (never user input), and deleting it is the entire
            // point of the command: the device token must not outlive logout.
            match std::fs::remove_file(&path) {
                Ok(()) => println!("Logged out (removed {})", path.display()),
                Err(e) if e.kind() == std::io::ErrorKind::NotFound => {
                    println!("Not logged in — nothing to remove.");
                }
                Err(e) => {
                    return Err(e).with_context(|| format!("Failed to remove {}", path.display()))
                }
            }
            Ok(0)
        }
        PortalSubcommand::Status => {
            let creds = load_credentials()?;
            println!("Portal:    {}", creds.portal_url);
            println!("Token:     {}", redact(&creds.device_token));
            println!(
                "Last sync: {}",
                creds.last_sync.as_deref().unwrap_or("never")
            );
            Ok(0)
        }
        PortalSubcommand::Sync { since, dry_run } => run(*since, *dry_run, verbose),
    }
}

/// Mask all but the last 4 characters so `rtk sync status` is safe to paste
/// into a support thread.
fn redact(token: &str) -> String {
    let visible: String = token
        .chars()
        .rev()
        .take(4)
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    if token.chars().count() <= 4 {
        return "****".to_string();
    }
    format!("****{visible}")
}

/// Upload aggregates. `dry_run` prints the payload without sending it, so a
/// customer can see exactly what would leave the machine.
pub fn run(since_days: i64, dry_run: bool, verbose: u8) -> Result<i32> {
    let tracker = Tracker::new().context("Failed to open the rtk tracking database")?;
    let days = tracker
        .get_all_days()
        .context("Failed to read daily statistics")?;

    let cutoff = cutoff_date(since_days);
    let selected = select_days(&days, &cutoff);

    if selected.is_empty() {
        println!("No tracked activity since {cutoff} — nothing to sync.");
        return Ok(0);
    }

    let total_saved: usize = selected.iter().map(|d| d.saved_tokens).sum();
    let total_cmds: usize = selected.iter().map(|d| d.commands).sum();

    if dry_run {
        let payload = SyncPayload { days: &selected };
        println!(
            "{}",
            serde_json::to_string_pretty(&payload).context("Failed to render payload")?
        );
        eprintln!(
            "dry-run: {} days, {} commands, {} tokens saved — nothing sent.",
            selected.len(),
            total_cmds,
            total_saved
        );
        return Ok(0);
    }

    let mut creds = load_credentials()?;
    if verbose > 0 {
        eprintln!(
            "syncing {} days to {} (since {cutoff})",
            selected.len(),
            creds.portal_url
        );
    }

    let body = serde_json::to_string(&SyncPayload { days: &selected })
        .context("Failed to serialize sync payload")?;
    let url = format!("{}/api/v1/sync", creds.portal_url);

    let response = post_json(&url, &creds.device_token, &body)?;

    creds.last_sync = Some(chrono::Utc::now().to_rfc3339());
    // A failure to record the timestamp must not look like a failed upload —
    // the data is already on the server.
    if let Err(e) = save_credentials(&creds) {
        eprintln!("rtk: warning: synced, but could not update last_sync: {e:#}");
    }

    println!(
        "Synced {} days ({} commands, {} tokens saved) to {}",
        selected.len(),
        total_cmds,
        total_saved,
        creds.portal_url
    );
    if verbose > 0 && !response.is_empty() {
        eprintln!("portal: {response}");
    }
    Ok(0)
}

/// Send the payload through the codebase's single egress point
/// (`core::http`), mapping portal status codes to actionable guidance.
fn post_json(url: &str, token: &str, body: &str) -> Result<String> {
    use crate::core::http::{self, HttpError};

    let auth = format!("Bearer {token}");
    let headers = [("Authorization", auth.as_str())];

    match http::post_json(
        url,
        body,
        &headers,
        std::time::Duration::from_secs(HTTP_TIMEOUT_SECS),
    ) {
        Ok(body) => Ok(body),
        // Surface the portal's own message: 401 means a bad/revoked token,
        // 402 means billing — both are user-actionable, not bugs.
        Err(HttpError::Status(code, detail)) => {
            let hint = match code {
                401 => " — token rejected; re-run `rtk portal login` with a fresh device token",
                402 => " — subscription inactive; check billing on the portal",
                _ => "",
            };
            bail!("Portal returned HTTP {code}{hint}: {}", detail.trim())
        }
        Err(HttpError::Transport(msg)) => bail!("Failed to reach the portal: {msg}"),
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn day(date: &str, saved: usize) -> DayStats {
        DayStats {
            date: date.to_string(),
            commands: 3,
            input_tokens: saved * 2,
            output_tokens: saved,
            saved_tokens: saved,
            savings_pct: 50.0,
            total_time_ms: 30,
            avg_time_ms: 10,
        }
    }

    #[test]
    fn test_select_days_filters_by_cutoff() {
        let days = vec![
            day("2026-01-01", 10),
            day("2026-02-01", 20),
            day("2026-03-01", 30),
        ];
        let selected = select_days(&days, "2026-02-01");
        assert_eq!(selected.len(), 2);
        assert_eq!(selected[0].date, "2026-02-01");
        assert_eq!(selected[1].date, "2026-03-01");
    }

    #[test]
    fn test_select_days_maps_all_metering_fields() {
        let days = vec![day("2026-01-01", 100)];
        let selected = select_days(&days, "2026-01-01");
        assert_eq!(
            selected[0],
            SyncDay {
                date: "2026-01-01".to_string(),
                commands: 3,
                input_tokens: 200,
                output_tokens: 100,
                saved_tokens: 100,
            }
        );
    }

    #[test]
    fn test_select_days_caps_at_request_limit_keeping_newest() {
        let days: Vec<DayStats> = (1..=450)
            .map(|i| day(&format!("2026-{:02}-{:02}", (i % 12) + 1, (i % 28) + 1), i))
            .collect();
        let selected = select_days(&days, "0000-00-00");
        assert_eq!(selected.len(), MAX_DAYS_PER_REQUEST);
        // Sorted ascending, so the window must end at the newest date present.
        let newest = days.iter().map(|d| d.date.as_str()).max().unwrap();
        assert_eq!(selected.last().unwrap().date, newest);
    }

    #[test]
    fn test_select_days_empty_when_nothing_recent() {
        let days = vec![day("2020-01-01", 10)];
        assert!(select_days(&days, "2026-01-01").is_empty());
    }

    #[test]
    fn test_cutoff_date_is_iso() {
        let c = cutoff_date(DEFAULT_SINCE_DAYS);
        assert_eq!(c.len(), 10, "expected YYYY-MM-DD, got {c}");
        assert!(c.chars().filter(|c| *c == '-').count() == 2);
    }

    #[test]
    fn test_redact_hides_all_but_last_four() {
        assert_eq!(redact("rtk_dev_abcdef1234"), "****1234");
        assert_eq!(redact("abc"), "****");
    }

    #[test]
    fn test_credentials_roundtrip_toml() {
        let creds = Credentials {
            portal_url: "https://portal.example.com".to_string(),
            device_token: "tok_123".to_string(),
            last_sync: Some("2026-07-29T00:00:00Z".to_string()),
        };
        let text = toml::to_string_pretty(&creds).unwrap();
        let back: Credentials = toml::from_str(&text).unwrap();
        assert_eq!(back.portal_url, creds.portal_url);
        assert_eq!(back.device_token, creds.device_token);
        assert_eq!(back.last_sync, creds.last_sync);
    }

    /// Payload must contain only aggregates — no commands, paths, or output.
    #[test]
    fn test_payload_contains_no_command_text() {
        let days = vec![day("2026-01-01", 10)];
        let selected = select_days(&days, "2026-01-01");
        let json = serde_json::to_string(&SyncPayload { days: &selected }).unwrap();
        assert!(json.contains("saved_tokens"));
        assert!(
            !json.contains("cmd"),
            "payload must not carry command text: {json}"
        );
        assert!(!json.contains('/'), "payload must not carry paths: {json}");
    }
}
