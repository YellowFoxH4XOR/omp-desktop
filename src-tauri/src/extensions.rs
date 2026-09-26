//! Pi packages (extensions, skills, prompts, themes) for πDesk's private Pi.
//!
//! Discovery reads the public gallery at pi.dev/packages; installed state
//! comes from the private agent's `settings.json` plus each npm package's
//! `package.json`; update checks compare against the npm registry. Every
//! change runs the private `pi` with one validated source argument, so a
//! typed or pasted value can never become a flag, a local path, or a shell.

use crate::error::{AppError, AppResult};
use crate::harness::HarnessRegistry;
use crate::util;
use serde::Serialize;
use serde_json::Value;
use std::path::{Path, PathBuf};
use std::sync::LazyLock;
use std::time::Duration;
use tokio::process::Command;
use tokio::sync::Mutex;

const CURL: &str = "/usr/bin/curl";
const GALLERY: &str = "https://pi.dev/packages";
const REGISTRY: &str = "https://registry.npmjs.org";
const MAX_GALLERY_BYTES: usize = 4 * 1024 * 1024;
const MAX_REGISTRY_BYTES: usize = 512 * 1024;
const MAX_CARDS: usize = 60;
const MAX_UPDATE_CHECKS: usize = 50;
const MAX_OUTPUT: usize = 64 * 1024;
const PI_TIMEOUT: Duration = Duration::from_secs(300);

/// One change to the private Pi's packages at a time.
static CHANGING: LazyLock<Mutex<()>> = LazyLock::new(|| Mutex::new(()));

#[derive(Debug, Clone, Serialize, PartialEq)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPackage {
    pub name: String,
    pub description: String,
    pub author: String,
    /// Gallery's own label, e.g. "1M/mo".
    pub downloads_label: String,
    pub downloads: u64,
    pub published_ms: u64,
    pub types: Vec<String>,
    /// Source for `pi install`, e.g. `npm:pi-mcp-adapter`.
    pub source: String,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct CatalogPage {
    pub packages: Vec<CatalogPackage>,
    pub page: u32,
    pub has_more: bool,
}

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct InstalledPackage {
    pub source: String,
    pub name: String,
    pub kind: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub version: Option<String>,
    /// Set when the source pins a version (`npm:x@1.2.3`).
    #[serde(skip_serializing_if = "Option::is_none")]
    pub pinned: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub description: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub latest: Option<String>,
    pub update_available: bool,
}

// ---------------------------------------------------------------------
// Sources
// ---------------------------------------------------------------------

fn valid_npm_name(name: &str) -> bool {
    let (scope, bare) = match name.strip_prefix('@') {
        Some(rest) => match rest.split_once('/') {
            Some((scope, bare)) => (Some(scope), bare),
            None => return false,
        },
        None => (None, name),
    };
    let part = |value: &str| {
        !value.is_empty()
            && !value.starts_with(['.', '_', '-'])
            && value
                .chars()
                .all(|c| c.is_ascii_lowercase() || c.is_ascii_digit() || "-._~".contains(c))
    };
    name.len() <= 214 && scope.is_none_or(part) && part(bare)
}

fn valid_version(version: &str) -> bool {
    !version.is_empty()
        && version.len() <= 64
        && version
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || ".+-^~".contains(c))
}

fn valid_segment(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 100
        && !value.starts_with(['.', '-'])
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || "-._".contains(c))
}

/// Split `name@version` where the name may itself start with `@scope/`.
fn split_npm(spec: &str) -> (&str, Option<&str>) {
    let at = spec
        .get(1..)
        .and_then(|rest| rest.find('@'))
        .map(|index| index + 1);
    match at {
        Some(index) => (&spec[..index], Some(&spec[index + 1..])),
        None => (spec, None),
    }
}

/// Accepts `pi install <source>`, `npm:<name>[@version]`, a bare npm name,
/// `git:<host>/<owner>/<repo>[@ref]`, or `https://<host>/<owner>/<repo>`.
/// Returns the canonical source passed to `pi` as one argument.
pub fn normalize_source(input: &str) -> AppResult<String> {
    let invalid = || {
        AppError::new(
            "Use an npm package (npm:name) or a git repository (git:github.com/owner/repo).",
        )
    };
    let mut text = input.trim();
    if let Some(rest) = text.strip_prefix("pi install ") {
        text = rest.trim();
    }
    if text.is_empty()
        || text.len() > 300
        || text.chars().any(|c| c.is_whitespace() || c.is_control())
    {
        return Err(invalid());
    }
    if let Some(repo) = text
        .strip_prefix("git:")
        .or_else(|| text.strip_prefix("https://"))
    {
        let (path, reference) = match repo.split_once('@') {
            Some((path, reference)) => (path, Some(reference)),
            None => (repo, None),
        };
        let path = path.trim_end_matches('/').trim_end_matches(".git");
        let parts: Vec<&str> = path.split('/').collect();
        let host_ok = parts.first().is_some_and(|host| {
            host.contains('.')
                && host
                    .chars()
                    .all(|c| c.is_ascii_alphanumeric() || ".-".contains(c))
        });
        if parts.len() != 3 || !host_ok || !parts[1..].iter().all(|part| valid_segment(part)) {
            return Err(invalid());
        }
        if reference.is_some_and(|reference| !valid_segment(reference)) {
            return Err(invalid());
        }
        return Ok(match reference {
            Some(reference) => format!("git:{path}@{reference}"),
            None => format!("git:{path}"),
        });
    }
    let spec = text.strip_prefix("npm:").unwrap_or(text);
    let (name, version) = split_npm(spec);
    if !valid_npm_name(name) || version.is_some_and(|v| !valid_version(v)) {
        return Err(invalid());
    }
    Ok(format!("npm:{spec}"))
}

fn npm_parts(source: &str) -> Option<(&str, Option<&str>)> {
    let (name, version) = split_npm(source.strip_prefix("npm:")?);
    valid_npm_name(name).then_some((name, version))
}

// ---------------------------------------------------------------------
// Network (curl with fixed arguments)
// ---------------------------------------------------------------------

fn encode(value: &str) -> String {
    value
        .bytes()
        .map(|b| match b {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' => {
                (b as char).to_string()
            }
            _ => format!("%{b:02X}"),
        })
        .collect()
}

async fn fetch(url: &str, limit: usize) -> AppResult<Vec<u8>> {
    let mut command = Command::new(CURL);
    command
        .args([
            "-sSfL",
            "--proto",
            "=https",
            "--max-time",
            "20",
            "--max-filesize",
        ])
        .arg(limit.to_string())
        .args(["-H", "Accept: text/html,application/json", "--"])
        .arg(url)
        .env_clear()
        .kill_on_drop(true);
    let output = command
        .output()
        .await
        .map_err(|_| AppError::new("Could not reach the network."))?;
    if !output.status.success() || output.stdout.len() > limit {
        return Err(AppError::new(
            "The package service did not respond. Check your connection and try again.",
        ));
    }
    Ok(output.stdout)
}

// ---------------------------------------------------------------------
// Gallery
// ---------------------------------------------------------------------

fn unescape(text: &str) -> String {
    text.replace("&quot;", "\"")
        .replace("&#39;", "'")
        .replace("&#x27;", "'")
        .replace("&lt;", "<")
        .replace("&gt;", ">")
        .replace("&amp;", "&")
}

fn clip(text: String, max: usize) -> String {
    if text.chars().count() <= max {
        text
    } else {
        text.chars().take(max).collect::<String>() + "…"
    }
}

fn attribute<'a>(html: &'a str, name: &str) -> Option<&'a str> {
    let marker = format!("{name}=\"");
    let start = html.find(&marker)? + marker.len();
    let end = html[start..].find('"')?;
    Some(&html[start..start + end])
}

fn between<'a>(html: &'a str, open: &str, close: &str) -> Option<&'a str> {
    let start = html.find(open)? + open.len();
    let end = html[start..].find(close)?;
    Some(&html[start..start + end])
}

fn strip_tags(html: &str) -> String {
    let mut out = String::new();
    let mut in_tag = false;
    for c in html.chars() {
        match c {
            '<' => in_tag = true,
            '>' => in_tag = false,
            _ if !in_tag => out.push(c),
            _ => {}
        }
    }
    unescape(out.trim())
}

/// Parse the gallery's server-rendered cards. Unknown or malformed cards are
/// skipped, never guessed.
pub fn parse_gallery(html: &str, page: u32) -> CatalogPage {
    let mut packages = Vec::new();
    for chunk in html
        .split("data-package-card=\"true\"")
        .skip(1)
        .take(MAX_CARDS)
    {
        let card = chunk.split("</article>").next().unwrap_or(chunk);
        let Some(name) = attribute(card, "data-package-name").map(unescape) else {
            continue;
        };
        let source = attribute(card, "data-copy-text")
            .and_then(|text| text.strip_prefix("pi install "))
            .map(unescape)
            .and_then(|text| normalize_source(&text).ok());
        let Some(source) = source else { continue };
        if !valid_npm_name(&name) && !source.starts_with("git:") {
            continue;
        }
        let meta: Vec<String> = between(card, "<div class=\"packages-meta\">", "</div>")
            .map(|block| {
                block
                    .split("<span>")
                    .skip(1)
                    .filter_map(|span| span.split("</span>").next())
                    .map(strip_tags)
                    .collect()
            })
            .unwrap_or_default();
        let types = attribute(card, "data-package-types")
            .unwrap_or("")
            .split_whitespace()
            .filter(|kind| matches!(*kind, "extension" | "skill" | "theme" | "prompt"))
            .map(String::from)
            .collect();
        packages.push(CatalogPackage {
            description: clip(
                between(card, "<p class=\"packages-desc\">", "</p>")
                    .map(strip_tags)
                    .unwrap_or_default(),
                300,
            ),
            author: clip(meta.first().cloned().unwrap_or_default(), 60),
            downloads_label: clip(meta.get(1).cloned().unwrap_or_default(), 20),
            downloads: attribute(card, "data-package-downloads")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            published_ms: attribute(card, "data-package-date")
                .and_then(|n| n.parse().ok())
                .unwrap_or(0),
            types,
            source,
            name: clip(name, 214),
        });
    }
    let has_more = html.contains(&format!("page={}\"", page + 1))
        || html.contains(&format!("page={}&", page + 1));
    CatalogPage {
        packages,
        page,
        has_more,
    }
}

pub async fn catalog(query: &str, kind: &str, sort: &str, page: u32) -> AppResult<CatalogPage> {
    if query.len() > 100 || !matches!(kind, "" | "extension" | "skill" | "theme" | "prompt") {
        return Err(AppError::new("Invalid package search."));
    }
    let sort = if matches!(sort, "downloads" | "recent" | "name") {
        sort
    } else {
        "downloads"
    };
    let page = page.clamp(1, 200);
    let mut url = format!("{GALLERY}?sort={sort}&page={page}");
    if !query.trim().is_empty() {
        url.push_str(&format!("&name={}", encode(query.trim())));
    }
    if !kind.is_empty() {
        url.push_str(&format!("&type={kind}"));
    }
    let html = fetch(&url, MAX_GALLERY_BYTES).await?;
    Ok(parse_gallery(&String::from_utf8_lossy(&html), page))
}

// ---------------------------------------------------------------------
// Installed packages
// ---------------------------------------------------------------------

fn npm_manifest(root: &Path, name: &str) -> Option<Value> {
    if !valid_npm_name(name) {
        return None;
    }
    let path: PathBuf = root
        .join("agent/npm/node_modules")
        .join(name)
        .join("package.json");
    let metadata = std::fs::symlink_metadata(&path).ok()?;
    if !metadata.is_file() || metadata.len() > 512 * 1024 {
        return None;
    }
    serde_json::from_slice(&std::fs::read(path).ok()?).ok()
}

pub fn installed(root: &Path) -> AppResult<Vec<InstalledPackage>> {
    Ok(crate::pi_settings::read_packages(root)?
        .into_iter()
        .map(|source| {
            if let Some((name, pinned)) = npm_parts(&source) {
                let manifest = npm_manifest(root, name);
                let text = |key: &str| {
                    manifest
                        .as_ref()
                        .and_then(|m| m.get(key))
                        .and_then(Value::as_str)
                        .map(|value| clip(value.to_string(), 300))
                };
                InstalledPackage {
                    name: name.to_string(),
                    kind: "npm".into(),
                    version: text("version"),
                    pinned: pinned.map(String::from),
                    description: text("description"),
                    latest: None,
                    update_available: false,
                    source,
                }
            } else {
                let name = source
                    .strip_prefix("git:")
                    .unwrap_or(&source)
                    .split('@')
                    .next()
                    .unwrap_or(&source)
                    .rsplit('/')
                    .next()
                    .unwrap_or(&source)
                    .to_string();
                InstalledPackage {
                    kind: if source.starts_with("git:") || source.starts_with("https://") {
                        "git"
                    } else {
                        "local"
                    }
                    .into(),
                    name,
                    version: None,
                    pinned: None,
                    description: None,
                    latest: None,
                    update_available: false,
                    source,
                }
            }
        })
        .collect())
}

/// Numeric semver comparison of the release part; pre-release tags sort
/// before their release, which is enough to say "newer is available".
pub fn is_newer(latest: &str, current: &str) -> bool {
    let parse = |v: &str| -> (Vec<u64>, bool) {
        let v = v.trim_start_matches('v');
        let (release, pre) = match v.split_once('-') {
            Some((release, _)) => (release, true),
            None => (v.split('+').next().unwrap_or(v), false),
        };
        (
            release.split('.').map(|n| n.parse().unwrap_or(0)).collect(),
            pre,
        )
    };
    let (a, a_pre) = parse(latest);
    let (b, b_pre) = parse(current);
    for i in 0..a.len().max(b.len()) {
        let (x, y) = (
            a.get(i).copied().unwrap_or(0),
            b.get(i).copied().unwrap_or(0),
        );
        if x != y {
            return x > y;
        }
    }
    b_pre && !a_pre
}

pub async fn check_updates(root: &Path) -> AppResult<Vec<InstalledPackage>> {
    let mut packages = installed(root)?;
    for package in packages
        .iter_mut()
        .filter(|p| p.kind == "npm")
        .take(MAX_UPDATE_CHECKS)
    {
        let url = format!(
            "{REGISTRY}/{}/latest",
            encode(&package.name).replace("%40", "@")
        );
        let Ok(body) = fetch(&url, MAX_REGISTRY_BYTES).await else {
            continue;
        };
        let latest = serde_json::from_slice::<Value>(&body)
            .ok()
            .and_then(|json| {
                json.get("version")
                    .and_then(Value::as_str)
                    .map(String::from)
            })
            .filter(|version| valid_version(version));
        if let (Some(latest), Some(current)) = (&latest, &package.version) {
            package.update_available = is_newer(latest, current);
        }
        package.latest = latest;
    }
    Ok(packages)
}

// ---------------------------------------------------------------------
// Changes through the private pi
// ---------------------------------------------------------------------

async fn run_pi(registry: &HarnessRegistry, args: &[&str]) -> AppResult<()> {
    let root = util::pidesk_root();
    let exe = registry
        .executable_path(crate::dto::HarnessKind::Pi)
        .await?;
    let _changing = CHANGING.lock().await;
    util::prepare_private_home(&root)?;
    let mut command = Command::new(exe);
    util::configure_private_command(&mut command, &root);
    // Run outside any project so no project-local Pi settings apply.
    command.current_dir(&root).args(args).kill_on_drop(true);
    let output = tokio::time::timeout(PI_TIMEOUT, command.output())
        .await
        .map_err(|_| AppError::new("Pi took too long. Check your connection and try again."))?
        .map_err(|_| AppError::new("Could not start the private Pi."))?;
    if output.status.success() {
        return Ok(());
    }
    let mut text = String::from_utf8_lossy(&output.stderr).into_owned();
    if text.trim().is_empty() {
        text = String::from_utf8_lossy(&output.stdout).into_owned();
    }
    let tail: String = text
        .chars()
        .rev()
        .take(MAX_OUTPUT.min(1200))
        .collect::<Vec<_>>()
        .into_iter()
        .rev()
        .collect();
    Err(AppError::new(format!(
        "Pi could not complete this: {}",
        util::redact_secrets(tail.trim())
    )))
}

pub async fn install(registry: &HarnessRegistry, input: &str) -> AppResult<String> {
    let source = normalize_source(input)?;
    run_pi(registry, &["install", &source, "--no-approve"]).await?;
    Ok(source)
}

fn installed_source(root: &Path, source: &str) -> AppResult<()> {
    if crate::pi_settings::read_packages(root)?
        .iter()
        .any(|s| s == source)
    {
        Ok(())
    } else {
        Err(AppError::new("That package is not installed."))
    }
}

pub async fn remove(registry: &HarnessRegistry, source: &str) -> AppResult<()> {
    installed_source(&util::pidesk_root(), source)?;
    run_pi(registry, &["remove", source, "--no-approve"]).await
}

pub async fn update(registry: &HarnessRegistry, source: &str) -> AppResult<()> {
    let root = util::pidesk_root();
    installed_source(&root, source)?;
    if let Some((name, Some(pinned))) = npm_parts(source) {
        return Err(AppError::new(format!(
            "{name} is pinned to {pinned}. Remove it and install npm:{name} to follow updates."
        )));
    }
    run_pi(registry, &["update", source, "--no-approve"]).await
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn sources_normalize_and_reject_flags_paths_and_shell() {
        assert_eq!(
            normalize_source("pi install npm:pi-mcp-adapter").unwrap(),
            "npm:pi-mcp-adapter"
        );
        assert_eq!(
            normalize_source("pi-mcp-adapter").unwrap(),
            "npm:pi-mcp-adapter"
        );
        assert_eq!(
            normalize_source("npm:@scope/pkg@1.2.3").unwrap(),
            "npm:@scope/pkg@1.2.3"
        );
        assert_eq!(
            normalize_source("https://github.com/o/r.git").unwrap(),
            "git:github.com/o/r"
        );
        assert_eq!(
            normalize_source("git:github.com/o/r@v1").unwrap(),
            "git:github.com/o/r@v1"
        );
        for bad in [
            "",
            "--global",
            "-e",
            "./local",
            "/abs/path",
            "~/x",
            "npm:Bad",
            "npm:a b",
            "npm:a;rm",
            "git:github.com/o",
            "git:github.com/o/r/x",
            "git:gh/../r/x",
            "npm:@scope",
            "npm:x@$(id)",
            "git:github.com/-o/r",
            "file:///etc",
            "pi install  ",
            "npm:",
            "npm:é@1",
            "@",
        ] {
            assert!(normalize_source(bad).is_err(), "{bad}");
        }
    }

    #[test]
    fn gallery_cards_parse_into_packages() {
        let html = r#"<article class="surface-panel content-card" data-package-card="true" data-package-name="pi-mcp-adapter" data-package-types="extension skill" data-package-downloads="1037931" data-package-date="1790147591556"><p class="packages-desc">MCP &amp; more</p><div class="packages-meta"><span>nicopreme</span><span>1M/mo</span><span>3d ago</span></div><button data-copy-text="pi install npm:pi-mcp-adapter">Copy</button></article>
<article data-package-card="true" data-package-name="evil"><button data-copy-text="pi install --global">x</button></article>
<a href="/packages?page=2">next</a>"#;
        let page = parse_gallery(html, 1);
        assert!(page.has_more);
        assert_eq!(
            page.packages,
            vec![CatalogPackage {
                name: "pi-mcp-adapter".into(),
                description: "MCP & more".into(),
                author: "nicopreme".into(),
                downloads_label: "1M/mo".into(),
                downloads: 1037931,
                published_ms: 1790147591556,
                types: vec!["extension".into(), "skill".into()],
                source: "npm:pi-mcp-adapter".into(),
            }]
        );
    }

    #[test]
    fn versions_compare_numerically() {
        assert!(is_newer("1.10.0", "1.9.9"));
        assert!(is_newer("2.0.0", "2.0.0-beta.1"));
        assert!(!is_newer("1.2.3", "1.2.3"));
        assert!(!is_newer("1.2.3-rc.1", "1.2.3"));
        assert!(!is_newer("0.9.0", "1.0.0"));
    }

    #[test]
    fn installed_reads_settings_and_npm_manifests() {
        let root = std::env::temp_dir().join(format!("pidesk-ext-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(root.join("agent/npm/node_modules/@s/pkg")).unwrap();
        std::fs::write(
            root.join("agent/settings.json"),
            r#"{"packages":["npm:@s/pkg","npm:pinned@1.0.0",{"source":"git:github.com/o/tool"}]}"#,
        )
        .unwrap();
        std::fs::write(
            root.join("agent/npm/node_modules/@s/pkg/package.json"),
            r#"{"version":"1.4.0","description":"Scoped"}"#,
        )
        .unwrap();
        let list = installed(&root).unwrap();
        assert_eq!(list.len(), 3);
        assert_eq!(
            (
                list[0].name.as_str(),
                list[0].version.as_deref(),
                list[0].description.as_deref()
            ),
            ("@s/pkg", Some("1.4.0"), Some("Scoped"))
        );
        assert_eq!(list[1].pinned.as_deref(), Some("1.0.0"));
        assert_eq!(
            (list[2].kind.as_str(), list[2].name.as_str()),
            ("git", "tool")
        );
        std::fs::remove_dir_all(root).unwrap();
    }
}
