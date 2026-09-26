//! Vulnerability advisory database for `noct audit`.
//!
//! Minimal line-oriented format (dependency-free, in keeping with the
//! hand-rolled manifest parser ethos — ADR-017):
//!
//! ```text
//! # comment lines start with `#`; blank lines are ignored.
//! # One advisory per line, five `|`-separated fields:
//! #   name | vulnerable_req | advisory_id | severity | summary
//! sibling|0.3.5|RUSTSEC-2026-0001|high|example vuln in sibling
//! ```
//!
//! Field rules:
//! - `name`: lowercase snake_case package name (`[a-z][a-z0-9_]*`, max
//!   64 chars — same shape as manifest dependency names).
//! - `vulnerable_req`: a v1 version requirement (`1.2.3` for caret,
//!   `=1.2.3` for exact) with the exact [`VersionReq`] matching
//!   semantics from `manifest.rs` (full caret semantics).
//! - `advisory_id`: opaque non-empty token, no whitespace.
//! - `severity`: exactly one of `low`, `moderate`, `high`, `critical`
//!   (anything else is a loud error).
//! - `summary`: non-empty free text (may contain `|`; it is the
//!   remainder of the line after the fourth `|`).
//!
//! Loader notes:
//! - A missing file is an empty database, NOT an error (the default
//!   state — no advisories configured).
//! - Malformed entries are loud [`ManifestError`]s carrying the
//!   1-based file line number.

use std::fs;
use std::path::Path;

use crate::manifest::{req_satisfied, ManifestError, SemVer, VersionReq};

/// Advisory severity. Only these four spellings are accepted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Severity {
    Low,
    Moderate,
    High,
    Critical,
}

impl Severity {
    pub fn as_str(self) -> &'static str {
        match self {
            Severity::Low => "low",
            Severity::Moderate => "moderate",
            Severity::High => "high",
            Severity::Critical => "critical",
        }
    }

    pub fn parse(s: &str, line: usize) -> Result<Severity, ManifestError> {
        match s {
            "low" => Ok(Severity::Low),
            "moderate" => Ok(Severity::Moderate),
            "high" => Ok(Severity::High),
            "critical" => Ok(Severity::Critical),
            other => Err(ManifestError {
                line,
                message: format!(
                    "unknown severity `{other}` (expected low, moderate, high, or critical)"
                ),
            }),
        }
    }
}

/// One advisory entry: versions of `package` satisfying `req` are
/// considered vulnerable.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Advisory {
    pub package: String,
    pub req: VersionReq,
    pub id: String,
    pub severity: Severity,
    pub summary: String,
}

/// An advisory database: the ordered list of entries from one file.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct AdvisoryDb {
    pub advisories: Vec<Advisory>,
}

impl AdvisoryDb {
    /// All advisories whose package name matches and whose vulnerable
    /// requirement is satisfied by `version` (existing
    /// [`req_satisfied`] semantics — caret and exact).
    pub fn matching(&self, name: &str, version: SemVer) -> Vec<&Advisory> {
        self.advisories
            .iter()
            .filter(|a| a.package == name && req_satisfied(a.req, version))
            .collect()
    }
}

/// Parse database text (see module docs for the format). Blank lines
/// and `#` comment lines are skipped; every other line must hold five
/// `|`-separated fields or it is a line-numbered error.
pub fn parse_database(text: &str) -> Result<AdvisoryDb, ManifestError> {
    let mut advisories = Vec::new();
    for (idx, raw) in text.lines().enumerate() {
        let line_no = idx + 1;
        let line = raw.strip_suffix('\r').unwrap_or(raw);
        let trimmed = line.trim();
        if trimmed.is_empty() || trimmed.starts_with('#') {
            continue;
        }
        advisories.push(parse_entry(line, line_no)?);
    }
    Ok(AdvisoryDb { advisories })
}

/// Load a database file. A missing file is an empty database (not an
/// error); any other I/O failure is a loud line-0 error, and
/// malformed entries carry their file line number.
pub fn load_database(path: &Path) -> Result<AdvisoryDb, ManifestError> {
    match fs::read_to_string(path) {
        Ok(text) => parse_database(&text),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(AdvisoryDb {
            advisories: Vec::new(),
        }),
        Err(e) => Err(ManifestError {
            line: 0,
            message: format!("cannot read advisory database `{}`: {e}", path.display()),
        }),
    }
}

fn parse_entry(line: &str, line_no: usize) -> Result<Advisory, ManifestError> {
    let mut parts = line.splitn(5, '|');
    let (Some(name), Some(req_s), Some(id), Some(severity_s), Some(summary)) = (
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
        parts.next(),
    ) else {
        return Err(ManifestError {
            line: line_no,
            message: format!(
                "malformed advisory entry `{line}` \
                 (expected `name|vulnerable_req|advisory_id|severity|summary`)"
            ),
        });
    };
    let name = name.trim();
    let req_s = req_s.trim();
    let id = id.trim();
    let severity_s = severity_s.trim();
    let summary = summary.trim();
    if !valid_package_name(name) {
        return Err(ManifestError {
            line: line_no,
            message: format!("bad package name `{name}` (lowercase snake_case)"),
        });
    }
    let req = VersionReq::parse(req_s, line_no).map_err(|e| ManifestError {
        line: e.line,
        message: format!("bad vulnerable version requirement: {}", e.message),
    })?;
    if id.is_empty() || id.chars().any(char::is_whitespace) {
        return Err(ManifestError {
            line: line_no,
            message: format!("bad advisory id `{id}` (non-empty, no whitespace)"),
        });
    }
    let severity = Severity::parse(severity_s, line_no)?;
    if summary.is_empty() {
        return Err(ManifestError {
            line: line_no,
            message: "advisory summary must not be empty".to_string(),
        });
    }
    Ok(Advisory {
        package: name.to_string(),
        req,
        id: id.to_string(),
        severity,
        summary: summary.to_string(),
    })
}

/// Same shape rule as manifest dependency names: start with a
/// lowercase letter, `[a-z0-9_]*`, max 64 chars.
fn valid_package_name(name: &str) -> bool {
    let mut chars = name.chars();
    match chars.next() {
        Some(c) if c.is_ascii_lowercase() => {}
        _ => return false,
    }
    name.len() <= 64
        && name
            .chars()
            .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || c == '_')
}
